//! Target selection (Phase 9, IPC v20): fills the delivery set to a target count, with ranked
//! alternatives per moment, `not_sure` / `set_aside` with reasons and a "covered by" link.
//!
//! Owned by vision-ml-dev. The architect fixed the surface used by `ipc::commands` and
//! `lib.rs`: [`TargetSelectionConfig`], [`TargetSelection`] (`new`, `is_running`, `start`,
//! `cancel`), [`TargetJob`] and the pipeline order in [`run_pipeline`]. [`select`] /
//! [`choose`] and their data types are the seam the tests use.
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
//!
//! How [`choose`] works (deterministic: every order is by priority / quality, then image id):
//! 1. Per frame: quality `q` for its moment's shot type (scorer `overall` + visible face /
//!    faces looking / expression / detail focus), an importance boost (main subject most,
//!    important people less) and the forced cases (locked, the user's own pick / reject,
//!    confident-defect reject).
//! 2. Per moment, proposals by rule: couple = every distinct variation (only frames at
//!    [`NEAR_DUP_COUPLE`] similarity or more collapse); group = exactly one setup frame (most
//!    faces looking; the main subject must be looking, else nothing is proposed and the best
//!    frame is "not sure") plus up to [`MAX_ACTIVITY`] distinct activity / reaction frames;
//!    detail = one frame with the focus on the object; candid = distinct frames with a clearly
//!    visible (crop-worthy) face or action; other = distinct good frames ("next best").
//! 3. Proposals are ranked by tier (first of each couple / group / detail / candid moment,
//!    then couple variations, group activity, candid extras, next best) + `q` + boost and
//!    delivered up to the target; first-of-moment proposals may go up to +10% over it.
//!    Nothing below the bars is ever delivered to fill the target.
//! 4. The rest: defects and the user's rejects are set aside; close calls (just below the
//!    cut), possible creative blur and borderline eyes are "not sure"; frames of a moment with
//!    a delivered photo become its ranked alternatives (at most [`MAX_ALTERNATIVES`]); else
//!    set aside. "Covered by" = the most similar delivered frame of the same moment, else the
//!    most similar delivered frame anywhere (at least [`COVER_MIN`]).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rusqlite::Connection;
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use crate::db::target::{self, LockedChoice, SelectionDraft};
use crate::ipc::activity::activities;
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::events::{ActivityKind, ActivityState, TargetRunFinished};
use crate::ipc::types::{
    AnalysisScope, ImageId, PersonId, PersonRole, PickFlag, PickOrigin, ProjectId, ShootType, ShotType,
    SuggestionReason, SuggestionReasonKind, TargetChoice, TargetReason, TargetReasonKind, TargetRunState,
};
use crate::ml::moments::{self, FrameSignals};
use crate::ml::{identity, MODEL_VERSION};

/// Version of the moments + selection code (stored in `TargetRun.modelVersion` with the
/// identity model). Bump when results change.
pub const SELECTION_VERSION: &str = "selection-v1";

// ---------------------------------------------------------------------------
// Tunables (calibrate with `examples/target_eval.rs` once real shoots are available)
// ---------------------------------------------------------------------------

/// Couple frames at least this similar are "the same picture" (only these collapse).
pub const NEAR_DUP_COUPLE: f32 = 0.9;
/// Activity frames of a group setup at least this similar to a chosen one collapse.
pub const NEAR_DUP_ACTIVITY: f32 = 0.86;
/// Candid frames at least this similar to a chosen one collapse.
pub const NEAR_DUP_CANDID: f32 = 0.82;
/// "Other" frames at least this similar to a chosen one collapse.
pub const NEAR_DUP_OTHER: f32 = 0.8;
/// Couple variation: at least this visible face (a profile / kiss still counts).
pub const COUPLE_MIN_VISIBLE: f32 = 0.3;
/// Candid: a clearly visible face (frontal-ish, decent size anywhere in the frame: crop-worthy).
pub const CANDID_MIN_VISIBLE: f32 = 0.45;
/// "Other" frames (venue, scenery): scorer overall needed for a next-best delivery.
pub const OTHER_MIN_OVERALL: f32 = 0.55;
/// Below this scorer overall a frame is never delivered automatically (junk).
pub const MIN_OVERALL: f32 = 0.25;
/// Ranked alternatives kept per delivered photo (the rest are set aside, still covered).
pub const MAX_ALTERNATIVES: u32 = 8;
/// Activity / reaction extras per group setup.
pub const MAX_ACTIVITY: u32 = 3;
/// The target is a guideline: first-of-moment frames may take it this far over.
pub const TARGET_SLACK: f32 = 0.1;
/// Proposals cut by the target within this priority of the last delivered one are close calls.
pub const CLOSE_CALL: f32 = 0.25;
/// The first this many proposals after the cut are close calls ...
pub const CLOSE_CALL_MIN: usize = 2;
/// ... or this share of the target, whichever is more.
pub const CLOSE_CALL_SHARE: f32 = 0.05;
/// Cross-moment "covered by" needs at least this similarity.
pub const COVER_MIN: f32 = 0.45;
/// Priority boost for frames showing the main subject / an important person (face visible).
pub const BOOST_MAIN: f32 = 0.3;
pub const BOOST_IMPORTANT: f32 = 0.2;
/// `SelectionDraft.score` = priority / this (0..=1).
const PRIORITY_SCALE: f32 = 6.5;

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

/// Photos of the project still waiting for a preview or for analysis with the current model
/// (the analysis worker's needs-analysis predicate, scoped to the project).
pub fn pending_analysis(conn: &Connection, project_id: ProjectId) -> AppResult<u32> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM images i JOIN folders f ON f.id = i.folder_id
         LEFT JOIN thumbnails t ON t.image_id = i.id
         LEFT JOIN image_analysis a ON a.image_id = i.id
         WHERE f.project_id = ?1 AND (
             t.status = 'pending'
             OR (t.status = 'ready' AND t.preview_path IS NOT NULL
                 AND (a.image_id IS NULL OR a.status = 'queued' OR a.model_version IS NOT ?2
                      OR COALESCE(a.analyzed_at, 0) < COALESCE(t.extracted_at, 0))))",
        rusqlite::params![project_id, MODEL_VERSION],
        |r| r.get(0),
    )?)
}

/// Selection needs the analysis: when photos of the project are not analysed yet, kicks the
/// analysis worker and waits for it (cancellable). Photos it cannot analyse are skipped by
/// [`select`] with a "not analysed" reason.
fn wait_for_analysis(
    app: &AppHandle,
    config: &TargetSelectionConfig,
    job: &TargetJob,
    cancel: &AtomicBool,
    report: &mut dyn FnMut(&str, u32, Option<u32>),
) {
    let Some(analysis) = app.try_state::<crate::ml::Analysis>() else { return };
    let Ok(conn) = crate::db::open(&config.catalog_path) else { return };
    let first = pending_analysis(&conn, job.project_id).unwrap_or(0);
    if first > 0 && !analysis.is_running() {
        if let Err(e) = analysis.start(app, AnalysisScope::Pending) {
            eprintln!("target selection: could not start analysis: {}", e.message);
            return;
        }
    }
    let mut ticks = 0u32;
    while analysis.is_running() && !cancel.load(Ordering::SeqCst) {
        if ticks.is_multiple_of(4) {
            let left = pending_analysis(&conn, job.project_id).unwrap_or(0);
            report(&format!("Waiting for analysis ({left} photos left)"), first.saturating_sub(left), Some(first));
        }
        ticks += 1;
        std::thread::sleep(Duration::from_millis(250));
    }
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
    wait_for_analysis(app, config, job, cancel, &mut report);
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
        Some("No photos to choose from yet: import and analyse the shoot first".to_owned())
    } else {
        let mut m = format!("Picked {} of {} photos", counts.deliver, counts.total);
        let skipped =
            drafts.iter().filter(|d| d.reasons.first().is_some_and(|r| r.kind == TargetReasonKind::NotAnalyzed));
        let skipped = skipped.count();
        if skipped > 0 {
            m.push_str(&format!(" ({skipped} not analysed, left for you to check)"));
        }
        if identity.people.is_none() {
            if let Some(note) = &identity.message {
                m.push_str(&format!(". {note}: shot types come from faces only"));
            }
        }
        Some(m)
    };
    let model_version = format!("{}+{}", identity.model_version.as_deref().unwrap_or("no-identity"), SELECTION_VERSION);
    Ok(PipelineOutcome { cancelled: false, message, model_version })
}

/// Everything [`select`] decides from.
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

/// One candidate photo for [`choose`].
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
    /// For reason texts ("Almost the same as DSC0412").
    pub file_name: String,
    /// The scorer's reasons, most important first (the defect text of a reject).
    pub reasons: Vec<SuggestionReason>,
    /// Tagged `creative_blur` (not suppressed).
    pub creative_blur: bool,
}

impl SelectionFrame {
    /// A plain analysed frame (fixtures / fallbacks).
    pub fn new(image_id: ImageId, overall: f32) -> Self {
        Self {
            image_id,
            captured_at_ms: None,
            overall,
            scored_pick: PickFlag::Unflagged,
            user_pick: None,
            phash: None,
            file_name: format!("DSC{image_id:04}.ARW"),
            reasons: Vec::new(),
            creative_blur: false,
        }
    }
}

/// Reads the project's candidate frames and decides; photos not analysed (preview missing,
/// analysis failed or not run) get a `not_sure` draft with a `not_analyzed` reason.
pub fn select(conn: &Connection, project_id: ProjectId, input: &SelectionInput) -> AppResult<Vec<SelectionDraft>> {
    let frames = load_frames(conn, project_id)?;
    let mut drafts = choose(&frames, input);
    let locked: HashMap<ImageId, &LockedChoice> = input.locked.iter().map(|l| (l.image_id, l)).collect();
    for (id, failed) in not_analysed(conn, project_id)? {
        let text = match failed {
            Some(e) => format!("Sieve could not analyse this photo: {e}"),
            None => "Not analysed yet, so Sieve could not judge it".to_owned(),
        };
        let mut d = draft(id, TargetChoice::NotSure, None, None);
        d.reasons.push(reason(TargetReasonKind::NotAnalyzed, text, None));
        if let Some(l) = locked.get(&id) {
            d.choice = l.choice;
            d.alternative_of = l.alternative_of;
            d.rank = l.rank;
            d.reasons.insert(0, reason(TargetReasonKind::UserChoice, "Your choice", None));
        }
        drafts.push(d);
    }
    Ok(drafts)
}

fn load_frames(conn: &Connection, project_id: ProjectId) -> AppResult<Vec<SelectionFrame>> {
    let sql = format!(
        "SELECT i.id, i.captured_at_ms, i.file_name, i.pick, i.pick_origin, q.overall, q.scored_pick, q.reasons_json,
                a.phash,
                EXISTS (SELECT 1 FROM image_tags g WHERE g.image_id = i.id AND g.tag = 'creative_blur'
                        AND g.suppressed = 0)
         FROM images i JOIN folders f ON f.id = i.folder_id
         JOIN image_analysis a ON a.image_id = i.id
         LEFT JOIN quality_scores q ON q.image_id = i.id
         WHERE f.project_id = ?1 AND {}
         ORDER BY i.id",
        moments::ANALYSED
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params![project_id, MODEL_VERSION], |r| {
        Ok((
            r.get::<_, ImageId>(0)?,
            r.get::<_, Option<i64>>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, f64>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, Option<String>>(7)?,
            r.get::<_, Option<i64>>(8)?,
            r.get::<_, bool>(9)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, captured, name, pick, origin, overall, scored, reasons, phash, creative) = row?;
        let pick = PickFlag::parse(&pick).unwrap_or(PickFlag::Unflagged);
        let user_pick =
            (pick != PickFlag::Unflagged && PickOrigin::parse(&origin) != Some(PickOrigin::Auto)).then_some(pick);
        out.push(SelectionFrame {
            image_id: id,
            captured_at_ms: captured,
            overall: overall as f32,
            scored_pick: PickFlag::parse(&scored).unwrap_or(PickFlag::Unflagged),
            user_pick,
            phash: phash.map(|h| h as u64),
            file_name: name,
            reasons: reasons.and_then(|r| serde_json::from_str(&r).ok()).unwrap_or_default(),
            creative_blur: creative,
        });
    }
    Ok(out)
}

/// The project's photos [`load_frames`] skips: `(id, analysis error if it failed)`.
fn not_analysed(conn: &Connection, project_id: ProjectId) -> AppResult<Vec<(ImageId, Option<String>)>> {
    let analysed = moments::analysed_ids(conn, project_id)?;
    let mut stmt = conn.prepare(
        "SELECT i.id, a.status, a.error FROM images i JOIN folders f ON f.id = i.folder_id
         LEFT JOIN image_analysis a ON a.image_id = i.id WHERE f.project_id = ?1 ORDER BY i.id",
    )?;
    let rows = stmt.query_map([project_id], |r| {
        Ok((r.get::<_, ImageId>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, status, error) = row?;
        if analysed.contains(&id) {
            continue;
        }
        let failed = (status.as_deref() == Some("failed")).then(|| error.unwrap_or_else(|| "unknown error".into()));
        out.push((id, failed));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The decision
// ---------------------------------------------------------------------------

fn reason(kind: TargetReasonKind, text: impl Into<String>, related: Option<ImageId>) -> TargetReason {
    TargetReason { kind, text: text.into(), related_image_id: related }
}

fn draft(id: ImageId, choice: TargetChoice, moment: Option<u32>, shot: Option<ShotType>) -> SelectionDraft {
    SelectionDraft {
        image_id: id,
        choice,
        moment_key: moment,
        shot_type: shot,
        alternative_of: None,
        rank: None,
        covered_by: None,
        covered_similarity: None,
        score: 0.0,
        reasons: Vec::new(),
    }
}

/// File name without extension ("DSC0412").
fn stem(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 => &name[..i],
        _ => name,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Forced {
    None,
    Locked(TargetChoice),
    UserPick,
    UserReject,
    Defect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    CoreCouple,
    CoreGroup,
    CoreDetail,
    CoreCandid,
    CoupleVariation(u32),
    GroupActivity(u32),
    CandidExtra(u32),
    NextBest(u32),
}

impl Tier {
    fn base(self) -> f32 {
        match self {
            Tier::CoreCouple => 5.0,
            Tier::CoreGroup => 4.8,
            Tier::CoreDetail => 4.6,
            Tier::CoreCandid => 4.0,
            Tier::CoupleVariation(k) => (4.2 - 0.12 * k as f32).max(2.0),
            Tier::GroupActivity(k) => 3.0 - 0.3 * k as f32,
            Tier::CandidExtra(k) => (2.2 - 0.15 * k as f32).max(1.2),
            Tier::NextBest(k) => (1.0 - 0.2 * k as f32).max(0.1),
        }
    }

    fn core(self) -> bool {
        matches!(self, Tier::CoreCouple | Tier::CoreGroup | Tier::CoreDetail | Tier::CoreCandid)
    }
}

/// Moment of a frame: its key, or (no moment) a unique negative key.
type MKey = i64;

struct Cand<'a> {
    f: &'a SelectionFrame,
    s: FrameSignals,
    moment: MKey,
    mtype: ShotType,
    q: f32,
    boost: f32,
    forced: Forced,
    eligible: bool,
}

fn main_label(shoot: ShootType) -> &'static str {
    if shoot == ShootType::Wedding {
        "the couple"
    } else {
        "the main subject"
    }
}

/// Quality 0..=1 of a frame for its moment's shot type.
fn quality(s: &FrameSignals, mtype: ShotType, overall: f32) -> f32 {
    let q = match mtype {
        ShotType::Couple => 0.6 * overall + 0.3 * s.visible_face + 0.1 * s.expression,
        ShotType::Group => {
            let share = if s.subjects > 0 { s.looking as f32 / s.subjects as f32 } else { 0.0 };
            0.5 * overall + 0.3 * share + 0.2 * s.visible_face
        }
        ShotType::Candid => 0.5 * overall + 0.35 * s.visible_face + 0.15 * s.expression.max(s.motion),
        ShotType::Detail => {
            let focus = match s.detail_in_focus {
                Some(true) => 1.0,
                None => 0.5,
                Some(false) => 0.0,
            };
            0.7 * overall + 0.3 * focus
        }
        ShotType::Other => overall,
    };
    let q = if s.eyes_borderline { q - 0.1 } else { q };
    q.clamp(0.0, 1.0)
}

fn candid_ok(s: &FrameSignals) -> bool {
    s.visible_face >= CANDID_MIN_VISIBLE || (s.is_activity() && s.visible_face >= 0.3)
}

/// Pure decision over `frames` with the signals / moments of `input.plan` (deterministic: same
/// input, same output). One draft per frame, in image id order.
pub fn choose(frames: &[SelectionFrame], input: &SelectionInput) -> Vec<SelectionDraft> {
    let signals: HashMap<ImageId, &FrameSignals> = input.plan.frames.iter().map(|s| (s.image_id, s)).collect();
    let moment_types: HashMap<u32, ShotType> = input.plan.moments.iter().map(|m| (m.key, m.shot_type)).collect();
    let locked: HashMap<ImageId, &LockedChoice> = input.locked.iter().map(|l| (l.image_id, l)).collect();
    let main = main_label(input.shoot_type);

    let mut sorted: Vec<&SelectionFrame> = frames.iter().collect();
    sorted.sort_by_key(|f| f.image_id);
    sorted.dedup_by_key(|f| f.image_id);
    let cands: Vec<Cand> = sorted
        .iter()
        .map(|&f| {
            let s = signals.get(&f.image_id).map(|s| (*s).clone()).unwrap_or_else(|| {
                let mut s = FrameSignals::new(f.image_id);
                s.phash = f.phash;
                s.captured_at_ms = f.captured_at_ms;
                s.overall = f.overall;
                s
            });
            let moment = s.moment_key.map_or(-f.image_id, MKey::from);
            let mtype = s.moment_key.and_then(|k| moment_types.get(&k).copied()).unwrap_or(s.shot_type);
            let forced = if let Some(l) = locked.get(&f.image_id) {
                Forced::Locked(l.choice)
            } else if f.user_pick == Some(PickFlag::Reject) {
                Forced::UserReject
            } else if f.user_pick == Some(PickFlag::Pick) {
                Forced::UserPick
            } else if f.scored_pick == PickFlag::Reject {
                Forced::Defect
            } else {
                Forced::None
            };
            let q = quality(&s, mtype, f.overall);
            let boost = if s.visible_face < 0.3 {
                0.0
            } else if s.main_present {
                BOOST_MAIN
            } else if s.important_present {
                BOOST_IMPORTANT
            } else {
                0.0
            };
            let eligible = forced == Forced::None && f.overall >= MIN_OVERALL;
            Cand { f, s, moment, mtype, q, boost, forced, eligible }
        })
        .collect();
    let n = cands.len();
    let sim = |a: usize, b: usize| moments::similarity(&cands[a].s, &cands[b].s);

    // Members per moment, best quality first.
    let mut by_moment: BTreeMap<MKey, Vec<usize>> = BTreeMap::new();
    for (i, c) in cands.iter().enumerate() {
        by_moment.entry(c.moment).or_default().push(i);
    }
    for members in by_moment.values_mut() {
        members.sort_by(|&a, &b| cands[b].q.total_cmp(&cands[a].q).then(cands[a].f.image_id.cmp(&cands[b].f.image_id)));
    }

    // Moments with someone's face: a frame there without a face is a back of the head.
    let people_scene: std::collections::HashSet<MKey> =
        by_moment.iter().filter(|(_, m)| m.iter().any(|&c| cands[c].s.subjects > 0)).map(|(&k, _)| k).collect();
    let hidden_face = |i: usize| {
        let s = &cands[i].s;
        s.shot_type != ShotType::Detail
            && (s.turned_away || (s.subjects == 0 && people_scene.contains(&cands[i].moment)))
    };
    let pre_delivered = |i: usize| matches!(cands[i].forced, Forced::Locked(TargetChoice::Deliver) | Forced::UserPick);
    let mut proposals: Vec<(usize, Tier)> = Vec::new();
    // Frames that need the user's eye whatever the target: (reason).
    let mut unsure: HashMap<usize, TargetReason> = HashMap::new();
    for members in by_moment.values() {
        let mtype = cands[members[0]].mtype;
        let mut chosen: Vec<usize> = members.iter().copied().filter(|&i| pre_delivered(i)).collect();
        let distinct = |c: usize, chosen: &[usize], bar: f32| chosen.iter().all(|&d| sim(c, d) < bar);
        match mtype {
            ShotType::Couple => {
                let mut k = 0;
                for &c in members {
                    let s = &cands[c].s;
                    if !cands[c].eligible || s.visible_face < COUPLE_MIN_VISIBLE || s.eyes_borderline {
                        continue;
                    }
                    if distinct(c, &chosen, NEAR_DUP_COUPLE) {
                        let tier = if chosen.is_empty() { Tier::CoreCouple } else { Tier::CoupleVariation(k) };
                        k += u32::from(!chosen.is_empty());
                        proposals.push((c, tier));
                        chosen.push(c);
                    }
                }
            }
            ShotType::Group => {
                if chosen.is_empty() {
                    let mut by_looking: Vec<usize> = members.iter().copied().filter(|&c| cands[c].eligible).collect();
                    by_looking.sort_by(|&a, &b| {
                        cands[b]
                            .s
                            .looking
                            .cmp(&cands[a].s.looking)
                            .then(cands[b].q.total_cmp(&cands[a].q))
                            .then(cands[a].f.image_id.cmp(&cands[b].f.image_id))
                    });
                    let main_in_setup = members.iter().any(|&c| cands[c].s.main_present);
                    let best = by_looking
                        .iter()
                        .copied()
                        .find(|&c| cands[c].s.looking > 0 && (!main_in_setup || cands[c].s.main_looking));
                    match best {
                        Some(b) => {
                            proposals.push((b, Tier::CoreGroup));
                            chosen.push(b);
                        }
                        None => {
                            if let Some(&b) = by_looking.first() {
                                let text = if main_in_setup {
                                    format!("{} isn't looking at the camera in any frame of this group", capital(main))
                                } else {
                                    "Nobody is looking at the camera in this group".to_owned()
                                };
                                unsure.insert(b, reason(TargetReasonKind::Other, text, None));
                            }
                        }
                    }
                }
                let mut active: Vec<usize> =
                    members.iter().copied().filter(|&c| cands[c].eligible && cands[c].s.is_activity()).collect();
                active.sort_by(|&a, &b| {
                    let (sa, sb) = (&cands[a].s, &cands[b].s);
                    (sb.expression.max(sb.motion))
                        .total_cmp(&sa.expression.max(sa.motion))
                        .then(cands[b].q.total_cmp(&cands[a].q))
                        .then(cands[a].f.image_id.cmp(&cands[b].f.image_id))
                });
                let mut k = 0;
                for c in active {
                    if k >= MAX_ACTIVITY || chosen.contains(&c) {
                        continue;
                    }
                    if distinct(c, &chosen, NEAR_DUP_ACTIVITY) {
                        proposals.push((c, Tier::GroupActivity(k)));
                        chosen.push(c);
                        k += 1;
                    }
                }
            }
            ShotType::Detail => {
                if chosen.is_empty() {
                    let ok = members
                        .iter()
                        .copied()
                        .find(|&c| cands[c].eligible && cands[c].s.detail_in_focus != Some(false));
                    match ok {
                        Some(b) => proposals.push((b, Tier::CoreDetail)),
                        None => {
                            if let Some(&b) = members.iter().find(|&&c| cands[c].eligible) {
                                let text = "Focus is not on the object in any frame of this detail";
                                unsure.insert(b, reason(TargetReasonKind::DetailOutOfFocus, text, None));
                            }
                        }
                    }
                }
            }
            ShotType::Candid => {
                let mut k = 0;
                for &c in members {
                    let s = &cands[c].s;
                    if !cands[c].eligible || !candid_ok(s) || s.eyes_borderline {
                        continue;
                    }
                    if distinct(c, &chosen, NEAR_DUP_CANDID) {
                        let tier = if chosen.is_empty() { Tier::CoreCandid } else { Tier::CandidExtra(k) };
                        k += u32::from(!chosen.is_empty());
                        proposals.push((c, tier));
                        chosen.push(c);
                    }
                }
            }
            ShotType::Other => {
                // A frame without faces is scenery only when nobody's face is in the moment;
                // among people it is a back of the head / face hidden.
                let mut k = 0;
                for &c in members {
                    let cand = &cands[c];
                    if !cand.eligible || cand.f.overall < OTHER_MIN_OVERALL || hidden_face(c) {
                        continue;
                    }
                    if distinct(c, &chosen, NEAR_DUP_OTHER) {
                        proposals.push((c, Tier::NextBest(k)));
                        chosen.push(c);
                        k += 1;
                    }
                }
            }
        }
    }

    // Fill the target.
    let priority = |c: usize, t: Tier| t.base() + cands[c].q + cands[c].boost;
    let mut ranked: Vec<(usize, Tier, f32)> = proposals.iter().map(|&(c, t)| (c, t, priority(c, t))).collect();
    ranked.sort_by(|a, b| b.2.total_cmp(&a.2).then(cands[a.0].f.image_id.cmp(&cands[b.0].f.image_id)));
    let target = input.target_count as usize;
    let cap = ((input.target_count as f32) * (1.0 + TARGET_SLACK)).floor() as usize;
    let mut delivered: Vec<Option<(Tier, f32)>> = vec![None; n];
    let mut count = (0..n).filter(|&i| pre_delivered(i)).count();
    for &(c, t, p) in &ranked {
        if count >= target {
            break;
        }
        delivered[c] = Some((t, p));
        count += 1;
    }
    for &(c, t, p) in &ranked {
        if count >= cap {
            break;
        }
        if t.core() && delivered[c].is_none() {
            delivered[c] = Some((t, p));
            count += 1;
        }
    }
    // Close calls: the next proposals in line after the cut (at least [`CLOSE_CALL_MIN`], or
    // [`CLOSE_CALL_SHARE`] of the target) and any within [`CLOSE_CALL`] priority of the last
    // delivered one.
    let cutoff = delivered.iter().flatten().map(|d| d.1).fold(f32::INFINITY, f32::min);
    let next_in_line = CLOSE_CALL_MIN.max((input.target_count as f32 * CLOSE_CALL_SHARE).ceil() as usize);
    let close_calls: std::collections::HashSet<usize> = ranked
        .iter()
        .filter(|&&(c, _, _)| delivered[c].is_none())
        .enumerate()
        .filter(|&(k, &(_, _, p))| k < next_in_line || p >= cutoff - CLOSE_CALL)
        .map(|(_, &(c, _, _))| c)
        .collect();
    let proposal_of: HashMap<usize, (Tier, f32)> = ranked.iter().map(|&(c, t, p)| (c, (t, p))).collect();
    let is_delivered = |i: usize| delivered[i].is_some() || pre_delivered(i);
    let delivered_ids: Vec<usize> = (0..n).filter(|&i| is_delivered(i)).collect();

    // Nearest delivered frame: same moment first, else the most similar anywhere.
    let nearest_same = |i: usize| -> Option<(usize, f32)> {
        by_moment[&cands[i].moment]
            .iter()
            .copied()
            .filter(|&d| d != i && is_delivered(d))
            .map(|d| (d, sim(i, d)))
            .max_by(|a, b| a.1.total_cmp(&b.1).then(cands[b.0].f.image_id.cmp(&cands[a.0].f.image_id)))
    };
    let nearest_any = |i: usize| -> Option<(usize, f32)> {
        delivered_ids
            .iter()
            .copied()
            .filter(|&d| d != i)
            .map(|d| (d, sim(i, d)))
            .filter(|&(_, s)| s >= COVER_MIN)
            .max_by(|a, b| a.1.total_cmp(&b.1).then(cands[b.0].f.image_id.cmp(&cands[a.0].f.image_id)))
    };

    let mut drafts: Vec<SelectionDraft> = Vec::with_capacity(n);
    // Alternative candidates per delivered frame: (draft index, quality).
    let mut alternatives: BTreeMap<usize, Vec<(usize, f32, ImageId)>> = BTreeMap::new();
    for (i, c) in cands.iter().enumerate() {
        let shot = Some(c.s.shot_type);
        let moment = c.s.moment_key;
        let mut d = draft(c.f.image_id, TargetChoice::SetAside, moment, shot);
        let prop = proposal_of.get(&i).copied();
        // Proposals by their priority; other frames below every proposal of the same quality.
        d.score = (prop.map_or(c.q * 0.5, |p| p.1) / PRIORITY_SCALE).clamp(0.0, 1.0);
        let name_of = |j: usize| stem(&cands[j].f.file_name).to_owned();

        if is_delivered(i) {
            d.choice = TargetChoice::Deliver;
            match c.forced {
                Forced::Locked(_) => d.reasons.push(reason(TargetReasonKind::UserChoice, "Your choice", None)),
                Forced::UserPick => d.reasons.push(reason(TargetReasonKind::UserChoice, "You picked this photo", None)),
                _ => {}
            }
            if let Some((tier, _)) = delivered[i] {
                d.reasons.push(delivered_reason(tier, &c.s));
                if c.boost > 0.0 && c.mtype != ShotType::Couple {
                    let text = if c.s.main_present {
                        format!("{} is in it", capital(main))
                    } else {
                        "Shows an important person".to_owned()
                    };
                    d.reasons.push(reason(TargetReasonKind::ImportantPerson, text, None));
                }
                if c.f.creative_blur {
                    d.reasons.push(reason(TargetReasonKind::Other, "Intentional blur, the best of its moment", None));
                }
            }
            drafts.push(d);
            continue;
        }

        let same = nearest_same(i);
        let cover = same.or_else(|| nearest_any(i));
        d.covered_by = cover.map(|(j, _)| cands[j].f.image_id);
        d.covered_similarity = cover.map(|(_, s)| (s * 1000.0).round() / 1000.0);
        let near_dup_bar = match c.mtype {
            ShotType::Couple => NEAR_DUP_COUPLE,
            ShotType::Group => NEAR_DUP_ACTIVITY,
            ShotType::Candid => NEAR_DUP_CANDID,
            _ => NEAR_DUP_OTHER,
        };
        let near_dup = same.filter(|&(_, s)| s >= near_dup_bar);

        match c.forced {
            Forced::Locked(choice) => {
                let l = locked[&c.f.image_id];
                d.choice = choice;
                d.alternative_of = l.alternative_of;
                d.rank = l.rank;
                d.reasons.push(reason(TargetReasonKind::UserChoice, "Your choice", None));
                drafts.push(d);
                continue;
            }
            Forced::UserReject => {
                d.reasons.push(reason(TargetReasonKind::UserChoice, "You rejected this photo", None));
                drafts.push(d);
                continue;
            }
            Forced::Defect => {
                let text =
                    c.f.reasons
                        .iter()
                        .find(|r| r.kind != SuggestionReasonKind::CreativeBlur)
                        .map(|r| r.text.clone())
                        .unwrap_or_else(|| "Sieve suggests rejecting this photo".to_owned());
                d.reasons.push(reason(TargetReasonKind::Defect, text, None));
                drafts.push(d);
                continue;
            }
            Forced::UserPick | Forced::None => {}
        }

        // Not sure: the user's eye is needed.
        let mut not_sure: Option<TargetReason> = unsure.get(&i).cloned();
        if not_sure.is_none() && near_dup.is_none() && c.eligible && c.f.creative_blur {
            not_sure = Some(reason(TargetReasonKind::Other, "Possible creative blur: your call", None));
        }
        if not_sure.is_none() && near_dup.is_none() && close_calls.contains(&i) {
            not_sure = Some(reason(TargetReasonKind::BelowTarget, "Close call: just missed the target", None));
        }
        let looks_ok = c.s.visible_face >= COUPLE_MIN_VISIBLE;
        if not_sure.is_none() && near_dup.is_none() && c.eligible && c.s.eyes_borderline && looks_ok {
            not_sure = Some(reason(TargetReasonKind::Other, "Eyes may be half closed: check", None));
        }
        if let Some(r) = not_sure {
            d.choice = TargetChoice::NotSure;
            d.reasons.push(r);
            if let Some((j, _)) = same {
                d.reasons.push(reason(
                    TargetReasonKind::NotBestOfSetup,
                    format!("{} was chosen for this moment", name_of(j)),
                    Some(cands[j].f.image_id),
                ));
            }
            drafts.push(d);
            continue;
        }

        // Alternative of the nearest delivered frame of the moment.
        let alt_ok = c.eligible
            && match c.mtype {
                ShotType::Candid => candid_ok(&c.s),
                ShotType::Detail => c.s.detail_in_focus != Some(false),
                ShotType::Couple => c.s.visible_face >= 0.2,
                _ => !hidden_face(i),
            };
        if let (true, Some((j, s))) = (alt_ok, same) {
            let rel = Some(cands[j].f.image_id);
            let r = if s >= near_dup_bar {
                reason(TargetReasonKind::NearDuplicate, format!("Almost the same as {}", name_of(j)), rel)
            } else if prop.is_some() {
                reason(
                    TargetReasonKind::BelowTarget,
                    format!("Good variation; the target was reached with better frames ({} kept)", name_of(j)),
                    rel,
                )
            } else {
                match c.mtype {
                    ShotType::Group => {
                        let b = &cands[j].s;
                        let text = if b.looking > c.s.looking {
                            format!("{} has more faces looking at the camera", name_of(j))
                        } else {
                            format!("{} is the best of this group setup", name_of(j))
                        };
                        reason(TargetReasonKind::NotBestOfSetup, text, rel)
                    }
                    ShotType::Detail => reason(
                        TargetReasonKind::NotBestOfSetup,
                        format!("{} is the best shot of this detail", name_of(j)),
                        rel,
                    ),
                    _ => reason(
                        TargetReasonKind::NotBestOfSetup,
                        format!("Similar to {}, which is better", name_of(j)),
                        rel,
                    ),
                }
            };
            d.choice = TargetChoice::Alternative;
            d.alternative_of = rel;
            d.reasons.push(r);
            let idx = drafts.len();
            alternatives.entry(j).or_default().push((idx, c.q, c.f.image_id));
            drafts.push(d);
            continue;
        }

        // Set aside, with why.
        let r = if c.f.overall < MIN_OVERALL {
            reason(TargetReasonKind::Other, "Weak frame overall (low score)", None)
        } else if c.mtype == ShotType::Detail && c.s.detail_in_focus == Some(false) {
            reason(TargetReasonKind::DetailOutOfFocus, "Focus is not on the object", None)
        } else if (matches!(c.mtype, ShotType::Candid | ShotType::Couple | ShotType::Group)
            && (c.s.subjects == 0 || c.s.turned_away || c.s.visible_face < CANDID_MIN_VISIBLE)
            && !c.s.is_activity())
            || hidden_face(i)
        {
            let text = if c.s.subjects == 0 {
                "No face visible (back of head or turned away)"
            } else {
                "Faces not clearly visible (turned away or too small)"
            };
            reason(TargetReasonKind::NoVisibleFace, text, None)
        } else if let Some((j, _)) = same {
            reason(
                TargetReasonKind::NotBestOfSetup,
                format!("{} is a better frame of this moment", name_of(j)),
                Some(cands[j].f.image_id),
            )
        } else {
            reason(TargetReasonKind::BelowTarget, "Good, but the target was reached with better frames", None)
        };
        d.reasons.push(r);
        drafts.push(d);
    }

    // Rank alternatives per delivered photo; beyond the strip they are set aside (still covered).
    for list in alternatives.values_mut() {
        list.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.2.cmp(&b.2)));
        for (rank, &(idx, _, _)) in list.iter().enumerate() {
            let d = &mut drafts[idx];
            if rank as u32 >= MAX_ALTERNATIVES {
                d.choice = TargetChoice::SetAside;
                d.alternative_of = None;
            } else {
                d.rank = Some(rank as u32 + 1);
            }
        }
    }
    drafts
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn delivered_reason(tier: Tier, s: &FrameSignals) -> TargetReason {
    match tier {
        Tier::CoreCouple => reason(TargetReasonKind::CoupleVariation, "Best frame of this couple moment", None),
        Tier::CoupleVariation(_) => {
            reason(TargetReasonKind::CoupleVariation, "Another pose or angle of the couple", None)
        }
        Tier::CoreGroup => reason(
            TargetReasonKind::GroupBest,
            format!("Best of this group: {} of {} faces looking at the camera", s.looking, s.subjects),
            None,
        ),
        Tier::GroupActivity(_) => {
            reason(TargetReasonKind::GroupActivity, "Group reaction (laughing, cheering), kept as an extra", None)
        }
        Tier::CoreDetail => {
            let text = if s.detail_in_focus == Some(true) {
                "Best shot of this detail, focus on the object"
            } else {
                "Best shot of this detail"
            };
            reason(TargetReasonKind::DetailBest, text, None)
        }
        Tier::CoreCandid | Tier::CandidExtra(_) => {
            let largest = s.faces.iter().filter(|f| f.subject).map(|f| f.bbox.height).fold(0.0, f32::max);
            let text = if s.is_activity() && s.visible_face < CANDID_MIN_VISIBLE {
                "Action with faces visible"
            } else if largest < 0.12 {
                "Clear face, would crop well"
            } else {
                "Faces clearly visible"
            };
            reason(TargetReasonKind::CandidVisible, text, None)
        }
        Tier::NextBest(_) => reason(TargetReasonKind::NextBest, "Good frame, added to reach the target", None),
    }
}

#[cfg(test)]
mod tests;
