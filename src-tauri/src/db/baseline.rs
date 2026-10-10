//! Baseline edit storage (Phase 10, IPC v21, migration 0021): runs, per-photo results,
//! provenance, scope resolution. Written by the architect; rust-engine-dev owns it from here.
//! The engine (`develop::baseline`) computes [`BaselineDraft`]s; [`store_results`] writes them
//! as one edit batch (kind `baseline`) plus results and provenance, atomically.
//!
//! Provenance rule (docs/architecture.md, "Baseline edit"): a photo is on the baseline while its
//! history cursor is the entry the baseline wrote (`baseline_provenance.history_entry_id`, and
//! that entry carries the run's `batch_id`). Derived on read, so per-photo undo / redo, later
//! edits and batch undo need no bookkeeping.

use std::collections::HashMap;

use rusqlite::{params, params_from_iter, Connection, OptionalExtension};

use crate::db::now_ms;
use crate::db::projects::{self, FolderScope};
use crate::develop::batches::{self, BatchItem, BatchKind};
use crate::ipc::error::{AppError, AppResult, ErrorKind};
use crate::ipc::types::*;

/// A photo's edit state as the run sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotoEditState {
    /// Neutral settings (never edited, or reset).
    Unedited,
    /// Its settings are what an earlier baseline wrote ([`BaselineState::OnBaseline`]).
    OnBaseline,
    /// Edited otherwise (by the user, a paste / scene apply / style, or in Lightroom).
    Edited,
}

/// One photo in the run's scope, capture order.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopePhoto {
    pub entry: RawImageEntry,
    pub state: PhotoEditState,
}

/// What the engine decided for one photo (stored by [`store_results`]).
#[derive(Debug, Clone, PartialEq)]
pub struct BaselineDraft {
    pub result: BaselinePhotoResult,
    /// The settings to write (`Some` only for `applied` / `flagged`).
    pub adjustments: Option<ParametricAdjustments>,
}

// ---------------------------------------------------------------------------
// Scope
// ---------------------------------------------------------------------------

fn project_of_image(conn: &Connection, id: ImageId) -> AppResult<ProjectId> {
    let p: Option<Option<ProjectId>> = conn
        .query_row(
            "SELECT f.project_id FROM images i JOIN folders f ON f.id = i.folder_id WHERE i.id = ?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    p.flatten().ok_or_else(|| AppError::not_found(format!("image {id}")))
}

/// Checks `settings` against the project: anchor and selection ids belong to it, the preset
/// exists. Unknown project / image / preset -> `not_found`; a photo of another project ->
/// `invalid_argument`. Returns the settings (selection ids deduplicated, order kept).
pub fn resolve_settings(
    conn: &Connection,
    project_id: ProjectId,
    settings: &BaselineSettings,
) -> AppResult<BaselineSettings> {
    projects::require_project(conn, project_id)?;
    let in_project = |id: ImageId| -> AppResult<()> {
        if project_of_image(conn, id)? != project_id {
            return Err(AppError::invalid(format!("image {id} is not in project {project_id}")));
        }
        Ok(())
    };
    in_project(settings.anchor_id)?;
    if let Some(p) = settings.preset_id {
        let ok: bool = conn.query_row("SELECT EXISTS (SELECT 1 FROM presets WHERE id = ?1)", [p], |r| r.get(0))?;
        if !ok {
            return Err(AppError::not_found(format!("preset {p}")));
        }
    }
    let mut out = settings.clone();
    if let BaselineScope::Selection { ids } = &settings.scope {
        let mut seen = Vec::with_capacity(ids.len());
        for &id in ids {
            in_project(id)?;
            if !seen.contains(&id) {
                seen.push(id);
            }
        }
        out.scope = BaselineScope::Selection { ids: seen };
    }
    Ok(out)
}

/// Edit states of `ids` (unknown ids are absent).
pub fn photo_states(conn: &Connection, ids: &[ImageId]) -> AppResult<HashMap<ImageId, PhotoEditState>> {
    let mut out = HashMap::with_capacity(ids.len());
    for chunk in ids.chunks(500) {
        let ph = vec!["?"; chunk.len()].join(",");
        let mut stmt = conn.prepare(&format!(
            "SELECT i.id, COALESCE(a.neutral, 1),
                    EXISTS (SELECT 1 FROM baseline_provenance p JOIN adjustment_history h ON h.id = p.history_entry_id
                             WHERE p.image_id = i.id AND p.history_entry_id = a.history_entry_id
                               AND h.batch_id = p.batch_id)
               FROM images i LEFT JOIN adjustments a ON a.image_id = i.id WHERE i.id IN ({ph})"
        ))?;
        let rows = stmt.query_map(params_from_iter(chunk.iter()), |r| {
            Ok((r.get::<_, ImageId>(0)?, r.get::<_, bool>(1)?, r.get::<_, bool>(2)?))
        })?;
        for row in rows {
            let (id, neutral, on_baseline) = row?;
            let state = if on_baseline {
                PhotoEditState::OnBaseline
            } else if neutral {
                PhotoEditState::Unedited
            } else {
                PhotoEditState::Edited
            };
            out.insert(id, state);
        }
    }
    Ok(out)
}

/// The photos of `settings.scope` (resolved settings) in capture order, with their edit state.
/// Keepers follow `CatalogState.keeperRule`. The anchor is included when in scope.
pub fn scope_photos(
    conn: &Connection,
    project_id: ProjectId,
    settings: &BaselineSettings,
) -> AppResult<Vec<ScopePhoto>> {
    let scope = FolderScope::resolve(conn, None, Some(project_id))?;
    let plan = crate::scene::workflow::scope_images(conn, &scope)?;
    let ids: Vec<ImageId> = match &settings.scope {
        BaselineScope::All => plan.iter().map(|p| p.id).collect(),
        BaselineScope::Keepers => {
            let rule = crate::db::repo::keeper_rule(conn)?;
            plan.iter().filter(|p| rule.is_keeper_values(p.pick, p.rating, p.suggested_pick)).map(|p| p.id).collect()
        }
        BaselineScope::Selection { ids } => plan.iter().map(|p| p.id).filter(|id| ids.contains(id)).collect(),
    };
    let states = photo_states(conn, &ids)?;
    let entries = crate::db::repo::get_images(conn, &ids)?;
    Ok(entries
        .into_iter()
        .map(|entry| {
            let state = states.get(&entry.id).copied().unwrap_or(PhotoEditState::Unedited);
            ScopePhoto { entry, state }
        })
        .collect())
}

/// Counts of a planned run.
pub fn plan_counts(photos: &[ScopePhoto], settings: &BaselineSettings) -> BaselinePlanCounts {
    let mut c = BaselinePlanCounts { in_scope: photos.len() as u32, ..Default::default() };
    for p in photos.iter().filter(|p| p.entry.id != settings.anchor_id) {
        match p.state {
            PhotoEditState::Unedited => c.to_write += 1,
            PhotoEditState::OnBaseline => {
                c.to_write += 1;
                c.on_baseline += 1;
            }
            PhotoEditState::Edited => {
                c.edited += 1;
                if settings.replace_edited {
                    c.to_write += 1;
                }
            }
        }
    }
    c
}

/// The settings the look is copied from: the anchor's current settings, or (while the anchor
/// has no edits) the preset resolved on them.
pub fn look_source(conn: &Connection, settings: &BaselineSettings) -> AppResult<ParametricAdjustments> {
    let current = crate::db::repo::get_adjustments(conn, settings.anchor_id)?;
    let neutral: bool = conn
        .query_row("SELECT neutral FROM adjustments WHERE image_id = ?1", [settings.anchor_id], |r| r.get(0))
        .optional()?
        .unwrap_or(true);
    match settings.preset_id {
        Some(p) if neutral => crate::styles::resolve_preset(conn, p, &current),
        _ => Ok(current),
    }
}

// ---------------------------------------------------------------------------
// Runs
// ---------------------------------------------------------------------------

/// Inserts a `running` run and returns its id.
pub fn begin_run(
    conn: &Connection,
    project_id: ProjectId,
    settings: &BaselineSettings,
    engine_version: &str,
) -> AppResult<BaselineRunId> {
    projects::require_project(conn, project_id)?;
    conn.execute(
        "INSERT INTO baseline_runs (project_id, anchor_id, preset_id, settings_json, state, engine_version, started_at)
         VALUES (?1, ?2, ?3, ?4, 'running', ?5, ?6)",
        params![
            project_id,
            settings.anchor_id,
            settings.preset_id,
            serde_json::to_string(settings)?,
            engine_version,
            now_ms()
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Stores the measured anchor of a run.
pub fn set_anchor(conn: &Connection, run_id: BaselineRunId, anchor: &BaselineAnchor) -> AppResult<()> {
    conn.execute(
        "UPDATE baseline_runs SET anchor_json = ?2 WHERE id = ?1",
        params![run_id, serde_json::to_string(anchor)?],
    )?;
    Ok(())
}

/// Ends a run with `state` and a user-facing `message` (does not touch results / batch).
pub fn finish_run(
    conn: &Connection,
    run_id: BaselineRunId,
    state: BaselineRunState,
    message: Option<&str>,
) -> AppResult<()> {
    conn.execute(
        "UPDATE baseline_runs SET state = ?2, message = ?3, finished_at = ?4 WHERE id = ?1",
        params![run_id, state.as_str(), message, now_ms()],
    )?;
    Ok(())
}

fn count(drafts: &[BaselineDraft]) -> BaselineCounts {
    let mut c = BaselineCounts { total: drafts.len() as u32, ..Default::default() };
    for d in drafts {
        match d.result.outcome {
            BaselineOutcome::Applied => c.applied += 1,
            BaselineOutcome::Flagged => c.flagged += 1,
            BaselineOutcome::SkippedEdited => c.skipped_edited += 1,
            BaselineOutcome::Anchor => c.anchor += 1,
            BaselineOutcome::Failed => c.failed += 1,
        }
    }
    c
}

/// Writes a finished run, atomically: one edit batch (kind `baseline`, label
/// [`BASELINE_LABEL`]) with every draft that has settings (flagged ones carry their first
/// reason as the batch item's review reason, so they show "needs review"), provenance of the
/// photos it changed, the per-photo results (replacing the project's earlier results) and the
/// run's counts / batch. Does not change the run's state (call [`finish_run`]).
pub fn store_results(
    conn: &mut Connection,
    run_id: BaselineRunId,
    drafts: &[BaselineDraft],
) -> AppResult<(EditBatchResult, BaselineCounts)> {
    conn.execute_batch("SAVEPOINT baseline_store")?;
    let r = store_in(conn, run_id, drafts);
    match &r {
        Ok(_) => conn.execute_batch("RELEASE baseline_store")?,
        Err(_) => {
            let _ = conn.execute_batch("ROLLBACK TO baseline_store; RELEASE baseline_store");
        }
    }
    r
}

fn store_in(
    conn: &mut Connection,
    run_id: BaselineRunId,
    drafts: &[BaselineDraft],
) -> AppResult<(EditBatchResult, BaselineCounts)> {
    let project_id: ProjectId = conn
        .query_row("SELECT project_id FROM baseline_runs WHERE id = ?1", [run_id], |r| r.get(0))
        .optional()?
        .ok_or_else(|| AppError::not_found(format!("baseline run {run_id}")))?;
    let items: Vec<BatchItem> = drafts
        .iter()
        .filter(|d| matches!(d.result.outcome, BaselineOutcome::Applied | BaselineOutcome::Flagged))
        .filter_map(|d| {
            d.adjustments.as_ref().map(|a| BatchItem {
                image_id: d.result.image_id,
                adjustments: a.clone(),
                scene_id: None,
                review_reason: (d.result.outcome == BaselineOutcome::Flagged).then(|| {
                    d.result.reasons.first().map(|r| r.text.clone()).unwrap_or_else(|| "Needs a look".to_owned())
                }),
            })
        })
        .collect();
    let batch = batches::commit_recorded(conn, &items, BASELINE_LABEL, BatchKind::Baseline)?;
    let now = now_ms();
    if let Some(batch_id) = batch.batch_id {
        for id in &batch.changed_ids {
            let flagged =
                drafts.iter().any(|d| d.result.image_id == *id && d.result.outcome == BaselineOutcome::Flagged);
            conn.execute(
                "INSERT OR REPLACE INTO baseline_provenance (image_id, run_id, batch_id, history_entry_id, flagged, applied_at)
                 SELECT ?1, ?2, ?3, a.history_entry_id, ?4, ?5 FROM adjustments a WHERE a.image_id = ?1",
                params![id, run_id, batch_id, flagged, now],
            )?;
        }
    }
    conn.execute(
        "DELETE FROM baseline_results WHERE run_id IN (SELECT id FROM baseline_runs WHERE project_id = ?1)",
        [project_id],
    )?;
    {
        let mut stmt = conn.prepare(
            "INSERT OR REPLACE INTO baseline_results
                 (run_id, image_id, outcome, reasons_json, scene_id, burst_group_id, auto_json, light_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for d in drafts {
            let r = &d.result;
            stmt.execute(params![
                run_id,
                r.image_id,
                r.outcome.as_str(),
                serde_json::to_string(&r.reasons)?,
                r.scene_id,
                r.burst_group_id,
                r.auto.map(|v| serde_json::to_string(&v)).transpose()?,
                r.light.map(|v| serde_json::to_string(&v)).transpose()?,
            ])?;
        }
    }
    let counts = count(drafts);
    conn.execute(
        "UPDATE baseline_runs SET batch_id = ?2, counts_json = ?3 WHERE id = ?1",
        params![run_id, batch.batch_id, serde_json::to_string(&counts)?],
    )?;
    Ok((batch, counts))
}

/// The project's newest run (`null` if never run). A `running` row while no worker runs
/// (`worker_running = false`, e.g. the app quit mid-run) reads `cancelled`. Unknown project ->
/// `not_found`.
pub fn get_run(conn: &Connection, project_id: ProjectId, worker_running: bool) -> AppResult<Option<BaselineRun>> {
    projects::require_project(conn, project_id)?;
    let id: Option<BaselineRunId> = conn
        .query_row("SELECT MAX(id) FROM baseline_runs WHERE project_id = ?1", [project_id], |r| r.get(0))
        .optional()?
        .flatten();
    match id {
        Some(id) => run_by_id(conn, id, worker_running).map(Some),
        None => Ok(None),
    }
}

/// Run `id` (see [`get_run`]). Unknown -> `not_found`.
pub fn run_by_id(conn: &Connection, id: BaselineRunId, worker_running: bool) -> AppResult<BaselineRun> {
    type Row = (
        ProjectId,
        String,
        String,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        Option<EditBatchId>,
        i64,
        Option<i64>,
    );
    let row: Option<Row> = conn
        .query_row(
            "SELECT project_id, settings_json, state, message, engine_version, anchor_json, counts_json, batch_id,
                    started_at, finished_at
               FROM baseline_runs WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                ))
            },
        )
        .optional()?;
    let (project_id, settings, state, mut message, engine_version, anchor, counts, batch_id, started, finished) =
        row.ok_or_else(|| AppError::not_found(format!("baseline run {id}")))?;
    let mut state = BaselineRunState::parse(&state).unwrap_or(BaselineRunState::Failed);
    if state == BaselineRunState::Running && !worker_running {
        state = BaselineRunState::Cancelled;
        message.get_or_insert_with(|| "Stopped before it finished; nothing was changed".to_owned());
    }
    let batch = match batch_id {
        Some(b) => match batches::batch_info(conn, b) {
            Ok(i) => Some(i),
            Err(e) if e.kind == ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        },
        None => None,
    };
    let live = live_counts(conn, id)?;
    if batch.as_ref().is_some_and(|b| b.undone_at_ms.is_some()) {
        message = Some(undone_message(&live));
    }
    Ok(BaselineRun {
        id,
        project_id,
        settings: serde_json::from_str(&settings)?,
        state,
        started_at_ms: started,
        finished_at_ms: finished,
        message,
        engine_version,
        anchor: anchor.map(|a| serde_json::from_str(&a)).transpose()?,
        counts: counts.map(|c| serde_json::from_str(&c)).transpose()?.unwrap_or_default(),
        batch,
        live,
    })
}

/// Per-photo results of the project's latest finished run, capture order (then file name),
/// filtered by `outcomes` (`None` / empty = all). Unknown project -> `not_found`.
pub fn results(
    conn: &Connection,
    project_id: ProjectId,
    outcomes: Option<&[BaselineOutcome]>,
) -> AppResult<Vec<BaselinePhotoResult>> {
    projects::require_project(conn, project_id)?;
    let mut stmt = conn.prepare(&format!(
        "SELECT r.image_id, r.outcome, r.reasons_json, r.scene_id, r.burst_group_id, r.auto_json, r.light_json,
                    p.image_id IS NOT NULL, {ON_BASELINE_SQL}, {UNDONE_SQL}
               FROM baseline_results r JOIN baseline_runs run ON run.id = r.run_id
               JOIN images i ON i.id = r.image_id
               LEFT JOIN baseline_provenance p ON p.image_id = r.image_id AND p.run_id = r.run_id
               {STATE_JOINS}
              WHERE run.project_id = ?1
              ORDER BY i.captured_at_ms IS NULL, i.captured_at_ms, i.file_name, i.id"
    ))?;
    let rows = stmt.query_map([project_id], |r| {
        Ok((
            r.get::<_, ImageId>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<SceneId>>(3)?,
            r.get::<_, Option<BurstGroupId>>(4)?,
            r.get::<_, Option<String>>(5)?,
            r.get::<_, Option<String>>(6)?,
            r.get::<_, bool>(7)?.then_some((r.get::<_, bool>(8)?, r.get::<_, bool>(9)?)),
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (image_id, outcome, reasons, scene_id, burst_group_id, auto, light, state) = row?;
        let outcome = BaselineOutcome::parse(&outcome).unwrap_or(BaselineOutcome::Failed);
        if outcomes.is_some_and(|o| !o.is_empty() && !o.contains(&outcome)) {
            continue;
        }
        out.push(BaselinePhotoResult {
            image_id,
            outcome,
            reasons: serde_json::from_str(&reasons)?,
            scene_id,
            burst_group_id,
            auto: auto.map(|a| serde_json::from_str(&a)).transpose()?,
            light: light.map(|a| serde_json::from_str(&a)).transpose()?,
            state: state.map(|(on, undone)| state_of(on, undone)),
        });
    }
    Ok(out)
}

/// `BaselineRun.live` of run `run_id` (v21.1).
pub fn live_counts(conn: &Connection, run_id: BaselineRunId) -> AppResult<BaselineLiveCounts> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {ON_BASELINE_SQL}, {UNDONE_SQL}, p.flagged AND bi.review_reason IS NOT NULL AND bi.reviewed_at IS NULL
           FROM baseline_provenance p {STATE_JOINS}
          WHERE p.run_id = ?1"
    ))?;
    let rows = stmt.query_map([run_id], |r| Ok((r.get::<_, bool>(0)?, r.get::<_, bool>(1)?, r.get::<_, bool>(2)?)))?;
    let mut c = BaselineLiveCounts::default();
    for row in rows {
        let (on, undone, look) = row?;
        c.written += 1;
        match state_of(on, undone) {
            BaselineState::OnBaseline => {
                c.on_baseline += 1;
                c.needs_look += u32::from(look);
            }
            BaselineState::UserEdited => c.user_edited += 1,
            BaselineState::Undone => c.undone += 1,
        }
    }
    Ok(c)
}

/// The run message after its batch was undone (v21.1).
pub fn undone_message(live: &BaselineLiveCounts) -> String {
    let photos = |n: u32| if n == 1 { "1 photo".to_owned() } else { format!("{n} photos") };
    match live.user_edited {
        0 => "Undone: the photos are back to how they were".to_owned(),
        1 => format!("Undone on {}; 1 photo you changed since was kept", photos(live.undone)),
        n => format!("Undone on {}; {n} photos you changed since were kept", photos(live.undone)),
    }
}

/// The anchor of `project_id`'s baseline while its latest run is finished and its batch not
/// undone (v21.1; the scene whose representative it is reads `on_baseline`).
pub fn live_anchor(conn: &Connection, project_id: ProjectId) -> AppResult<Option<ImageId>> {
    Ok(conn
        .query_row(
            "SELECT r.anchor_id FROM baseline_runs r JOIN edit_batches b ON b.id = r.batch_id
              WHERE r.id = (SELECT MAX(id) FROM baseline_runs WHERE project_id = ?1)
                AND r.state = 'finished' AND b.undone_at IS NULL",
            [project_id],
            |r| r.get::<_, Option<ImageId>>(0),
        )
        .optional()?
        .flatten())
}

/// The anchor of the live baseline of the project scene `scene_id` belongs to ([`live_anchor`]).
pub fn live_anchor_of_scene(conn: &Connection, scene_id: SceneId) -> AppResult<Option<ImageId>> {
    let project: Option<Option<ProjectId>> = conn
        .query_row(
            "SELECT f.project_id FROM images i JOIN folders f ON f.id = i.folder_id WHERE i.scene_id = ?1 LIMIT 1",
            [scene_id],
            |r| r.get(0),
        )
        .optional()?;
    match project.flatten() {
        Some(p) => live_anchor(conn, p),
        None => Ok(None),
    }
}

/// `EditPlan.baseline` (v21.1) of `project_id` given its keepers and their edit states:
/// `None` unless the latest run finished with a batch that is not undone.
pub fn plan_baseline(
    conn: &Connection,
    project_id: ProjectId,
    keeper_states: &[ImageEditState],
) -> AppResult<Option<EditPlanBaseline>> {
    let Some(run) = get_run(conn, project_id, false)? else { return Ok(None) };
    let Some(batch) = run.batch.filter(|b| b.undone_at_ms.is_none()) else { return Ok(None) };
    if run.state != BaselineRunState::Finished {
        return Ok(None);
    }
    let on: Vec<&ImageEditState> = keeper_states.iter().filter(|s| s.edit_source == EditSource::Baseline).collect();
    let keeper_ids: Vec<ImageId> = keeper_states.iter().map(|s| s.image_id).collect();
    let edited_since = provenance(conn, &keeper_ids)?
        .iter()
        .filter(|p| p.run_id == run.id && p.state == BaselineState::UserEdited)
        .count();
    Ok(Some(EditPlanBaseline {
        run_id: run.id,
        batch,
        anchor_id: run.settings.anchor_id,
        preset_id: run.settings.preset_id,
        keepers: keeper_states.len() as u32,
        on_baseline: on.len() as u32,
        needs_look: on.iter().filter(|s| s.needs_review).count() as u32,
        edited_since: edited_since as u32,
    }))
}

/// SQL joins over `baseline_provenance p` for [`ON_BASELINE_SQL`] / [`UNDONE_SQL`].
const STATE_JOINS: &str = "LEFT JOIN adjustments a ON a.image_id = p.image_id
               LEFT JOIN adjustment_history h ON h.id = a.history_entry_id
               LEFT JOIN edit_batches b ON b.id = p.batch_id
               LEFT JOIN edit_batch_items bi ON bi.batch_id = p.batch_id AND bi.image_id = p.image_id";
/// The photo's cursor is the entry the baseline wrote ([`BaselineState::OnBaseline`]).
const ON_BASELINE_SQL: &str = "COALESCE(a.history_entry_id = p.history_entry_id AND h.batch_id = p.batch_id, 0)";
/// The batch was undone and did not keep this photo (v21.1 `keepLaterEdits` stamps
/// `kept_at`): [`BaselineState::Undone`] unless on the baseline.
const UNDONE_SQL: &str = "(b.undone_at IS NOT NULL AND bi.kept_at IS NULL)";

fn state_of(on: bool, undone: bool) -> BaselineState {
    if on {
        BaselineState::OnBaseline
    } else if undone {
        BaselineState::Undone
    } else {
        BaselineState::UserEdited
    }
}

/// Provenance of `ids` (given order; photos never written by a baseline are omitted). Unknown
/// image -> `not_found`. v21.1: photos `undo_edit_batch(…, {keepLaterEdits})` kept read
/// `user_edited`.
pub fn provenance(conn: &Connection, ids: &[ImageId]) -> AppResult<Vec<BaselineProvenance>> {
    let mut by_id: HashMap<ImageId, BaselineProvenance> = HashMap::new();
    for &id in ids {
        let exists: bool = conn.query_row("SELECT EXISTS (SELECT 1 FROM images WHERE id = ?1)", [id], |r| r.get(0))?;
        if !exists {
            return Err(AppError::not_found(format!("image {id}")));
        }
    }
    let mut unique = ids.to_vec();
    unique.sort_unstable();
    unique.dedup();
    for chunk in unique.chunks(500) {
        let ph = vec!["?"; chunk.len()].join(",");
        let mut stmt = conn.prepare(&format!(
            "SELECT p.image_id, p.run_id, p.batch_id, r.anchor_id, r.preset_id, p.applied_at, p.flagged,
                    {ON_BASELINE_SQL}, {UNDONE_SQL}
               FROM baseline_provenance p
               JOIN baseline_runs r ON r.id = p.run_id
               {STATE_JOINS}
              WHERE p.image_id IN ({ph})"
        ))?;
        let rows = stmt.query_map(params_from_iter(chunk.iter()), |r| {
            let on: bool = r.get(7)?;
            let undone: bool = r.get(8)?;
            Ok(BaselineProvenance {
                image_id: r.get(0)?,
                run_id: r.get(1)?,
                batch_id: r.get(2)?,
                anchor_id: r.get(3)?,
                preset_id: r.get(4)?,
                applied_at_ms: r.get(5)?,
                flagged: r.get(6)?,
                state: state_of(on, undone),
            })
        })?;
        for row in rows {
            let p = row?;
            by_id.insert(p.image_id, p);
        }
    }
    Ok(ids.iter().filter_map(|id| by_id.get(id).cloned()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::develop::history;

    /// Project 1 (folder 1): images 1..=4 at t = 0..3000 ms; project 2 (folder 2): image 5.
    fn seed() -> Connection {
        let conn = open_in_memory();
        conn.execute_batch(
            "INSERT INTO projects (id, name, shoot_type, created_at) VALUES (1, 'p', 'wedding', 0), (2, 'q', 'wedding', 0);
             INSERT INTO folders (id, path, added_at, project_id) VALUES (1, '/f', 0, 1), (2, '/g', 0, 2);
             INSERT INTO images (id, folder_id, path, file_name, format, camera_make, sensor_layout,
                                 file_size, file_mtime_ms, imported_at, captured_at_ms)
             VALUES (1, 1, '/f/a.arw', 'a.arw', 'arw', 'sony', 'bayer', 1, 0, 0, 0),
                    (2, 1, '/f/b.arw', 'b.arw', 'arw', 'sony', 'bayer', 1, 0, 0, 1000),
                    (3, 1, '/f/c.arw', 'c.arw', 'arw', 'sony', 'bayer', 1, 0, 0, 2000),
                    (4, 1, '/f/d.arw', 'd.arw', 'arw', 'sony', 'bayer', 1, 0, 0, 3000),
                    (5, 2, '/g/e.arw', 'e.arw', 'arw', 'sony', 'bayer', 1, 0, 0, 500);",
        )
        .unwrap();
        conn
    }

    fn settings(scope: BaselineScope) -> BaselineSettings {
        BaselineSettings { anchor_id: 1, preset_id: None, scope, replace_edited: false }
    }

    fn draft(id: ImageId, outcome: BaselineOutcome, exposure: f32) -> BaselineDraft {
        let adj = ParametricAdjustments { exposure, ..Default::default() };
        let writes = matches!(outcome, BaselineOutcome::Applied | BaselineOutcome::Flagged);
        BaselineDraft {
            result: BaselinePhotoResult {
                image_id: id,
                outcome,
                reasons: if outcome == BaselineOutcome::Flagged {
                    vec![BaselineReason { kind: BaselineReasonKind::LowKey, text: "Dark on purpose".into() }]
                } else {
                    Vec::new()
                },
                scene_id: None,
                burst_group_id: None,
                auto: None,
                light: None,
                state: None,
            },
            adjustments: writes.then_some(adj),
        }
    }

    fn state_of(conn: &Connection, id: ImageId) -> BaselineState {
        provenance(conn, &[id]).unwrap()[0].state
    }

    #[test]
    fn settings_must_stay_in_the_project() {
        let conn = seed();
        assert!(resolve_settings(&conn, 1, &settings(BaselineScope::Keepers)).is_ok());
        let mut s = settings(BaselineScope::Selection { ids: vec![2, 2, 3] });
        assert_eq!(resolve_settings(&conn, 1, &s).unwrap().scope, BaselineScope::Selection { ids: vec![2, 3] });
        s.scope = BaselineScope::Selection { ids: vec![5] };
        assert_eq!(resolve_settings(&conn, 1, &s).unwrap_err().kind, ErrorKind::InvalidArgument);
        s.anchor_id = 99;
        assert_eq!(resolve_settings(&conn, 1, &s).unwrap_err().kind, ErrorKind::NotFound);
        let s = BaselineSettings { preset_id: Some(999), ..settings(BaselineScope::All) };
        assert_eq!(resolve_settings(&conn, 1, &s).unwrap_err().kind, ErrorKind::NotFound);
        assert_eq!(resolve_settings(&conn, 9, &settings(BaselineScope::All)).unwrap_err().kind, ErrorKind::NotFound);
    }

    #[test]
    fn run_store_provenance_and_rerun_rules() {
        let mut conn = seed();
        // Photo 4 has the user's own edit.
        let own = ParametricAdjustments { contrast: 30.0, ..Default::default() };
        history::commit(&mut conn, 4, &own, "Contrast").unwrap();
        let s = settings(BaselineScope::All);
        let photos = scope_photos(&conn, 1, &s).unwrap();
        assert_eq!(photos.iter().map(|p| p.entry.id).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
        let c = plan_counts(&photos, &s);
        assert_eq!((c.in_scope, c.to_write, c.on_baseline, c.edited), (4, 2, 0, 1));

        let run = begin_run(&conn, 1, &s, "test").unwrap();
        let drafts = vec![
            draft(1, BaselineOutcome::Anchor, 0.0),
            draft(2, BaselineOutcome::Applied, 0.5),
            draft(3, BaselineOutcome::Flagged, 0.2),
            draft(4, BaselineOutcome::SkippedEdited, 0.0),
        ];
        let (batch, counts) = store_results(&mut conn, run, &drafts).unwrap();
        finish_run(&conn, run, BaselineRunState::Finished, Some("done")).unwrap();
        assert_eq!(batch.changed_ids, vec![2, 3]);
        assert_eq!((counts.applied, counts.flagged, counts.anchor, counts.skipped_edited), (1, 1, 1, 1));
        let r = get_run(&conn, 1, false).unwrap().unwrap();
        assert_eq!(r.state, BaselineRunState::Finished);
        assert_eq!(r.batch.as_ref().unwrap().kind, EditBatchKind::Baseline);
        assert_eq!(results(&conn, 1, Some(&[BaselineOutcome::Flagged])).unwrap().len(), 1);
        assert_eq!(results(&conn, 1, None).unwrap().len(), 4);

        // Edit sources / needs review / provenance.
        let st = batches::edit_states(&conn, &[2, 3]).unwrap();
        assert_eq!(st[0].edit_source, EditSource::Baseline);
        assert!(!st[0].needs_review);
        assert!(st[1].needs_review);
        assert_eq!(st[1].review_reason.as_deref(), Some("Dark on purpose"));
        assert_eq!(state_of(&conn, 2), BaselineState::OnBaseline);
        assert!(provenance(&conn, &[1, 4]).unwrap().is_empty());
        assert!(provenance(&conn, &[3]).unwrap()[0].flagged);

        // The grid filter.
        let q =
            ImageQuery { project_id: Some(1), baseline_outcomes: vec![BaselineOutcome::Flagged], ..Default::default() };
        assert_eq!(crate::db::repo::list_image_ids(&conn, &q).unwrap(), vec![3]);

        // A user edit after the baseline: user_edited; a re-run skips it.
        let mut mine = crate::db::repo::get_adjustments(&conn, 2).unwrap();
        mine.shadows = 10.0;
        history::commit(&mut conn, 2, &mine, "Shadows").unwrap();
        assert_eq!(state_of(&conn, 2), BaselineState::UserEdited);
        let states = photo_states(&conn, &[2, 3]).unwrap();
        assert_eq!(states[&2], PhotoEditState::Edited);
        assert_eq!(states[&3], PhotoEditState::OnBaseline);
        // Per-photo undo back to the baseline entry: on the baseline again.
        history::undo(&mut conn, 2).unwrap();
        assert_eq!(state_of(&conn, 2), BaselineState::OnBaseline);

        // A second run replaces results; undoing its batch makes photos `undone`.
        let run2 = begin_run(&conn, 1, &s, "test").unwrap();
        let (batch2, _) = store_results(&mut conn, run2, &[draft(3, BaselineOutcome::Applied, 0.9)]).unwrap();
        finish_run(&conn, run2, BaselineRunState::Finished, None).unwrap();
        assert_eq!(results(&conn, 1, None).unwrap().len(), 1);
        assert_eq!(provenance(&conn, &[3]).unwrap()[0].run_id, run2);
        assert!(!provenance(&conn, &[3]).unwrap()[0].flagged);
        batches::undo(&mut conn, batch2.batch_id.unwrap()).unwrap();
        assert_eq!(state_of(&conn, 3), BaselineState::Undone);
        assert!(get_run(&conn, 1, false).unwrap().unwrap().batch.unwrap().undone_at_ms.is_some());
    }

    #[test]
    fn running_without_worker_reads_cancelled_and_failed_store_writes_nothing() {
        let mut conn = seed();
        let s = settings(BaselineScope::All);
        let run = begin_run(&conn, 1, &s, "test").unwrap();
        assert_eq!(get_run(&conn, 1, true).unwrap().unwrap().state, BaselineRunState::Running);
        assert_eq!(get_run(&conn, 1, false).unwrap().unwrap().state, BaselineRunState::Cancelled);
        // Unknown image in the drafts: nothing written (atomic).
        let bad = vec![draft(2, BaselineOutcome::Applied, 0.5), draft(99, BaselineOutcome::Applied, 0.5)];
        assert!(store_results(&mut conn, run, &bad).is_err());
        assert!(batches::edit_states(&conn, &[2]).unwrap()[0].edit_source == EditSource::None);
        assert!(results(&conn, 1, None).unwrap().is_empty());
        assert_eq!(get_run(&conn, 2, false).unwrap(), None);
    }
}
