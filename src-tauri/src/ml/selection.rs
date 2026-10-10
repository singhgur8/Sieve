//! Target selection (Phase 9, IPC v20): fills the delivery set to a target count, with ranked
//! alternatives per moment, `not_sure` / `set_aside` with reasons and a "covered by" link.
//!
//! Owned by vision-ml-dev. The architect fixed the surface used by `ipc::commands` and
//! `lib.rs`: [`TargetSelectionConfig`], [`TargetSelection`] (`new`, `is_running`, `start`,
//! `cancel`), [`TargetJob`] and the pipeline order in [`run_pipeline`]. [`select`] /
//! [`choose`] and their data types are the suggested seam; internals are free to change.
//!
//! Contract (docs/architecture.md, "Target-count culling"; user rules in docs/roadmap.md
//! Phase 9):
//! - Fill the target by priority: couple variations (many; only near-identical frames
//!   collapse), one per group setup (most faces looking, main subject looking; activity /
//!   reaction frames as extra variations), one per detail (focus on the object), candids with
//!   a visible face / action, then next best until the target. Important people boosted, the
//!   main subject most. Creative blur competes like any frame of its moment. The target is a
//!   guideline (about +/-10% when the moments call for it).
//! - Every analysed photo of the project gets a draft: `deliver`; `alternative` (of a delivered
//!   photo of the same moment, ranked 1..n); `not_sure`; `set_aside`; reasons most important
//!   first; `covered_by` (+ similarity) = the nearest delivered similar photo for every
//!   non-delivered frame that has one.
//! - Keep the user's decisions: every [`LockedChoice`] keeps its choice (they count towards the
//!   target); photos the user rejected are never delivered.
//! - Defects come from the scorer (`quality_scores.scored_pick` / reasons, which follow the
//!   project's `RejectStrictness`): a confident-defect reject is never delivered and keeps
//!   its reject suggestion through the overlay; not being chosen never rejects a photo.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rusqlite::Connection;
use tauri::AppHandle;
use tauri_specta::Event;

use crate::db::target::{self, LockedChoice, SelectionDraft};
use crate::ipc::activity::activities;
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::events::{ActivityKind, ActivityState, TargetRunFinished};
use crate::ipc::types::{ImageId, PersonId, PersonRole, PickFlag, ProjectId, ShootType, TargetRunState};
use crate::ml::{identity, moments};

/// Version of the moments + selection code (stored in `TargetRun.modelVersion` with the
/// identity model). Bump when results change.
pub const SELECTION_VERSION: &str = "selection-stub-0";

/// Resolved locations, fixed at startup by `lib.rs`.
#[derive(Debug, Clone)]
pub struct TargetSelectionConfig {
    /// Catalog file; the worker opens its own connection to it.
    pub catalog_path: PathBuf,
    /// Culling models directory (the face embedding model lives next to SCRFD).
    pub models_dir: PathBuf,
}

/// One accepted `run_target_selection`.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetJob {
    pub project_id: ProjectId,
    pub target_count: u32,
    /// Resolved (the project's when the request said `null`).
    pub shoot_type: ShootType,
}

/// Managed Tauri state: the target-selection worker (one run at a time, any project).
pub struct TargetSelection {
    config: TargetSelectionConfig,
    running: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

impl TargetSelection {
    pub fn new(config: TargetSelectionConfig) -> Self {
        Self { config, running: Arc::new(AtomicBool::new(false)), cancel: Arc::new(AtomicBool::new(false)) }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Starts `job` on a background thread and returns. The run row must already be
    /// `running` (`db::target::begin_run`). Reports `activityEvent` kind `target_selection`
    /// and ends with exactly one `TargetRunFinished`. A run in progress -> `invalid_argument`.
    pub fn start(&self, app: &AppHandle, job: TargetJob) -> AppResult<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Err(AppError::invalid("Target selection is already running"));
        }
        self.cancel.store(false, Ordering::SeqCst);
        let (config, running, cancel, app) =
            (self.config.clone(), self.running.clone(), self.cancel.clone(), app.clone());
        let spawned = std::thread::Builder::new().name("target-selection".into()).spawn(move || {
            worker(&app, &config, &job, &cancel);
            running.store(false, Ordering::SeqCst);
        });
        if let Err(e) = spawned {
            self.running.store(false, Ordering::SeqCst);
            return Err(AppError::internal(e.to_string()));
        }
        Ok(())
    }

    /// Asks the running job to stop (the previous selection is kept). No-op when idle.
    pub fn cancel(&self) {
        if self.is_running() {
            self.cancel.store(true, Ordering::SeqCst);
        }
    }
}

/// What [`run_pipeline`] produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PipelineOutcome {
    /// Stopped by `cancel` before anything was stored.
    pub cancelled: bool,
    /// User-facing summary for `TargetRun.message`.
    pub message: Option<String>,
    /// Stored in `target_runs.model_version`.
    pub model_version: String,
}

fn worker(app: &AppHandle, config: &TargetSelectionConfig, job: &TargetJob, cancel: &AtomicBool) {
    let channel = format!("target-{}", job.project_id);
    let hub = activities(app);
    let mut report = |label: &str, done: u32, total: Option<u32>| {
        if let Some(h) = &hub {
            h.progress(&channel, ActivityKind::TargetSelection, label, done, total);
        }
    };
    report("Choosing photos", 0, None);
    let result = crate::db::open(&config.catalog_path)
        .and_then(|mut conn| run_pipeline(&mut conn, &config.models_dir, job, cancel, &mut report).map(|o| (conn, o)));
    let (state, message) = match &result {
        Ok((_, o)) if o.cancelled => {
            (TargetRunState::Cancelled, Some("Stopped; the previous selection is kept".to_owned()))
        }
        Ok((_, o)) => (TargetRunState::Finished, o.message.clone()),
        Err(e) => (TargetRunState::Failed, Some(e.message.clone())),
    };
    let (conn, model_version) = match result {
        Ok((conn, o)) => (Ok(conn), (!o.cancelled).then_some(o.model_version)),
        Err(_) => (crate::db::open(&config.catalog_path), None),
    };
    let run = conn.and_then(|conn| {
        target::finish_run(&conn, job.project_id, state, message.as_deref(), model_version.as_deref())?;
        target::get_run(&conn, job.project_id, true)
    });
    if let Some(h) = &hub {
        let activity_state = match state {
            TargetRunState::Finished => ActivityState::Finished,
            TargetRunState::Cancelled => ActivityState::Cancelled,
            _ => ActivityState::Error,
        };
        h.finish(&channel, activity_state, message.clone());
    }
    match run {
        Ok(Some(run)) => {
            let _ = TargetRunFinished { run }.emit(app);
        }
        Ok(None) => {}
        Err(e) => eprintln!("target selection: could not report the run: {}", e.message),
    }
}

/// The whole run on the worker's connection: people (`ml::identity`), moments
/// (`ml::moments`), selection ([`select`]), then storing (`db::target::store_results`, which
/// keeps locked rows and refreshes the suggestion overlay). Nothing is stored when cancelled
/// before the last step; people are stored as soon as they are known.
pub fn run_pipeline(
    conn: &mut Connection,
    models_dir: &Path,
    job: &TargetJob,
    cancel: &AtomicBool,
    report: &mut dyn FnMut(&str, u32, Option<u32>),
) -> AppResult<PipelineOutcome> {
    let cancelled = || cancel.load(Ordering::SeqCst);
    let identity = identity::update_people(conn, job.project_id, job.shoot_type, models_dir, cancel, &mut |d, t| {
        report("Finding people", d, t)
    })?;
    if let Some(people) = &identity.people {
        target::replace_people(conn, job.project_id, people)?;
    }
    if cancelled() {
        return Ok(PipelineOutcome { cancelled: true, ..PipelineOutcome::default() });
    }
    report("Grouping moments", 0, None);
    let plan = moments::detect_moments(conn, job.project_id, job.shoot_type, cancel)?;
    if cancelled() {
        return Ok(PipelineOutcome { cancelled: true, ..PipelineOutcome::default() });
    }
    report(&format!("Choosing the best {}", job.target_count), 0, None);
    let input = SelectionInput {
        target_count: job.target_count,
        shoot_type: job.shoot_type,
        people: target::people_roles(conn, job.project_id)?,
        locked: target::locked_choices(conn, job.project_id)?,
        plan,
    };
    let drafts = select(conn, job.project_id, &input)?;
    if cancelled() {
        return Ok(PipelineOutcome { cancelled: true, ..PipelineOutcome::default() });
    }
    target::store_results(conn, job.project_id, &input.plan.moments, &drafts)?;
    let counts = target::counts(conn, job.project_id)?;
    let message = if drafts.is_empty() {
        Some("Target selection is not available yet: no photos were chosen".to_owned())
    } else {
        Some(format!("Picked {} of {} photos", counts.deliver, counts.total))
    };
    let model_version = format!("{}+{}", identity.model_version.as_deref().unwrap_or("no-identity"), SELECTION_VERSION);
    Ok(PipelineOutcome { cancelled: false, message, model_version })
}

/// Everything [`select`] decides from (extend freely).
#[derive(Debug, Clone, PartialEq)]
pub struct SelectionInput {
    pub target_count: u32,
    pub shoot_type: ShootType,
    /// Effective role per person of the project.
    pub people: Vec<(PersonId, PersonRole)>,
    /// The user's decisions (keep them).
    pub locked: Vec<LockedChoice>,
    pub plan: moments::MomentPlan,
}

/// One candidate photo for [`choose`] (extend freely).
#[derive(Debug, Clone, PartialEq)]
pub struct SelectionFrame {
    pub image_id: ImageId,
    pub captured_at_ms: Option<i64>,
    /// `QualityScore.overall`.
    pub overall: f32,
    /// The scorer's own suggestion (`quality_scores.scored_pick`, strictness applied).
    pub scored_pick: PickFlag,
    /// The user's flag when set by the user.
    pub user_pick: Option<PickFlag>,
    pub phash: Option<u64>,
}

/// Reads the project's candidate frames and decides. Stub: no decisions (the run finishes
/// with an empty selection).
pub fn select(_conn: &Connection, _project_id: ProjectId, input: &SelectionInput) -> AppResult<Vec<SelectionDraft>> {
    Ok(choose(&[], input))
}

/// Pure decision over `frames` (deterministic: same input, same output). Stub: no decisions.
pub fn choose(_frames: &[SelectionFrame], _input: &SelectionInput) -> Vec<SelectionDraft> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_pipeline_runs_end_to_end() {
        let mut conn = crate::db::open_in_memory();
        conn.execute("INSERT INTO projects (id, name, shoot_type, created_at) VALUES (1, 'P', 'wedding', 0)", [])
            .unwrap();
        let job = TargetJob { project_id: 1, target_count: 800, shoot_type: ShootType::Wedding };
        let cancel = AtomicBool::new(false);
        let mut phases = Vec::new();
        let out =
            run_pipeline(&mut conn, Path::new("/nonexistent"), &job, &cancel, &mut |l, _, _| phases.push(l.to_owned()))
                .unwrap();
        assert!(!out.cancelled);
        assert!(out.message.is_some());
        assert!(phases.iter().any(|p| p.starts_with("Choosing the best 800")));
        assert_eq!(target::counts(&conn, 1).unwrap().total, 0);
        cancel.store(true, Ordering::SeqCst);
        assert!(run_pipeline(&mut conn, Path::new("/x"), &job, &cancel, &mut |_, _, _| {}).unwrap().cancelled);
    }
}
