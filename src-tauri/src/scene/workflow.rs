//! Guided workflow, Edit step (IPC v14): per-project edit plan over scenes, representative
//! choice, and "Apply to scene" (relative matching from the representative, recorded as one
//! undoable edit batch).
//!
//! Ownership: the architect implemented the catalog side (plan, statuses, apply inputs and
//! commit) so the contract is live; vision-ml-dev owns [`propose_representative`] (the
//! placeholder picks the best-scored keeper; the spec is "best keeper with the most typical
//! lighting of the scene", e.g. closest to the scene's median `scene_features` among the
//! top-quality keepers, with a user-facing reason) and may refine the rest. `store::
//! replace_scenes` must carry `representative_*` (source `user`) and `applied_*` over to the
//! new scene that contains the representative (vision-ml-dev, like anchors).
//!
//! Status of a scene: `to_edit` (representative has no edits) -> `edited` -> `applied`
//! (`scenes.applied_params_json` equals the representative's current adjustments) ->
//! `outdated` (the representative changed since the last apply).
//!
//! IPC v15 (architect): `skipped` / `minor` scenes, per-photo state from
//! `develop::batches::edit_states` (applied / needs a look / auto edited), the last apply's
//! coverage (`scenes.applied_covered_json`) for `unappliedKeeperIds` and the plan's
//! `outdated` flag, `SceneApplyOptions.excludeIds`, partial commits after a cancel.

use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension};

use super::MatchImage;
use crate::db::projects::FolderScope;
use crate::db::{now_ms, repo};
use crate::develop::batches::{self, BatchItem, BatchKind};
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::*;

/// Reason shown for a user-chosen representative.
pub const REASON_USER: &str = "Chosen by you";

/// What the plan knows about one image of the project.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanImage {
    pub id: ImageId,
    pub scene_id: Option<SceneId>,
    pub pick: PickFlag,
    pub rating: u8,
    pub suggested_pick: Option<PickFlag>,
    /// `QualityScore.overall`, if analyzed.
    pub overall: Option<f32>,
    pub has_edits: bool,
}

/// Images of `scope` in capture order (images without a capture time last, by file name).
pub fn scope_images(conn: &Connection, scope: &FolderScope) -> AppResult<Vec<PlanImage>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT i.id, i.scene_id, i.pick, i.rating, q.suggested_pick, q.overall,
                EXISTS (SELECT 1 FROM adjustments a WHERE a.image_id = i.id AND a.neutral = 0)
         FROM images i LEFT JOIN quality_scores q ON q.image_id = i.id
         WHERE {}
         ORDER BY i.captured_at_ms IS NULL, i.captured_at_ms, i.file_name, i.id",
        scope.predicate("i.folder_id")
    ))?;
    let rows = stmt.query_map([], |r| {
        Ok(PlanImage {
            id: r.get(0)?,
            scene_id: r.get(1)?,
            pick: PickFlag::parse(&r.get::<_, String>(2)?).unwrap_or(PickFlag::Unflagged),
            rating: r.get(3)?,
            suggested_pick: r.get::<_, Option<String>>(4)?.and_then(|s| PickFlag::parse(&s)),
            overall: r.get(5)?,
            has_edits: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Keepers whose `QualityScore.overall` is within this of the best keeper's form the pool the
/// representative is chosen from (by typical light).
pub const REPRESENTATIVE_QUALITY_MARGIN: f32 = 0.1;
/// Reasons shown for automatic representatives.
pub const REASON_TYPICAL_BEST: &str = "Best keeper, in the scene's typical light";
pub const REASON_TYPICAL: &str = "One of the best keepers, closest to the scene's typical light";
pub const REASON_BEST: &str = "Best-scored keeper of this scene";
pub const REASON_FIRST: &str = "First keeper of this scene";

/// Proposes the representative of a scene among its `keepers` (non-empty, capture order):
/// `(image, user-facing reason)`. vision-ml-dev.
///
/// "Best keeper with the most typical lighting": the pool is the keepers whose
/// `QualityScore.overall` is within [`REPRESENTATIVE_QUALITY_MARGIN`] of the best (all keepers
/// when none is scored); among them the one whose preview appearance (`scene_features`) is
/// most similar on average to every member of the scene (the scene's medoid, so the edit
/// transfers to the rest with the smallest relative corrections). Ties: higher overall, then
/// picked, higher rating, earlier. Without features: the best-scored keeper.
pub fn propose_representative(
    conn: &Connection,
    scene_id: SceneId,
    keepers: &[PlanImage],
) -> AppResult<(ImageId, String)> {
    if keepers.is_empty() {
        return Err(AppError::internal("scene without keepers"));
    }
    let overall = |k: &PlanImage| k.overall.unwrap_or(-1.0);
    // Ordering "a is better than b": overall, pick, rating, earlier (capture order = index).
    let better = |(ia, a): (usize, &PlanImage), (ib, b): (usize, &PlanImage)| {
        overall(a)
            .total_cmp(&overall(b))
            .then((a.pick == PickFlag::Pick).cmp(&(b.pick == PickFlag::Pick)))
            .then(a.rating.cmp(&b.rating))
            .then(ib.cmp(&ia))
    };
    let (_, best) = keepers.iter().enumerate().max_by(|a, b| better(*a, *b)).expect("non-empty");
    let pool: Vec<(usize, &PlanImage)> = match best.overall {
        Some(top) => keepers
            .iter()
            .enumerate()
            .filter(|(_, k)| k.overall.is_some_and(|o| o >= top - REPRESENTATIVE_QUALITY_MARGIN))
            .collect(),
        None => keepers.iter().enumerate().collect(),
    };
    let fallback = || {
        let (id, reason) = if best.overall.is_some() { (best.id, REASON_BEST) } else { (keepers[0].id, REASON_FIRST) };
        Ok((id, reason.to_owned()))
    };
    if pool.len() < 2 {
        return fallback();
    }
    let features = member_features(conn, scene_id)?;
    if features.len() < 2 {
        return fallback();
    }
    let typicality = |id: ImageId| -> Option<f32> {
        let f = features.iter().find(|(i, _)| *i == id).map(|(_, f)| f)?;
        let others: Vec<f32> =
            features.iter().filter(|(i, _)| *i != id).map(|(_, g)| super::features::similarity(f, g)).collect();
        Some(others.iter().sum::<f32>() / others.len().max(1) as f32)
    };
    let scored: Vec<((usize, &PlanImage), f32)> =
        pool.iter().filter_map(|&(i, k)| typicality(k.id).map(|t| ((i, k), t))).collect();
    // Typicality in steps of 0.01 (differences below that are noise), then quality.
    let chosen = scored
        .iter()
        .max_by(|(a, ta), (b, tb)| (ta * 100.0).round().total_cmp(&(tb * 100.0).round()).then(better(*a, *b)))
        .map(|((_, k), _)| *k);
    let Some(chosen) = chosen else { return fallback() };
    let reason = if chosen.id == best.id { REASON_TYPICAL_BEST } else { REASON_TYPICAL };
    Ok((chosen.id, reason.to_owned()))
}

/// Current preview features of scene `scene_id`'s members.
fn member_features(conn: &Connection, scene_id: SceneId) -> AppResult<Vec<(ImageId, super::SceneFeatures)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT i.id, f.features_json FROM images i JOIN scene_features f ON f.image_id = i.id
         WHERE i.scene_id = ?1 AND f.version = ?2",
    )?;
    let rows = stmt.query_map(params![scene_id, super::FEATURES_VERSION], |r| {
        Ok((r.get::<_, ImageId>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, json) = row?;
        if let Ok(f) = serde_json::from_str(&json) {
            out.push((id, f));
        }
    }
    Ok(out)
}

struct SceneRow {
    representative_id: Option<ImageId>,
    source: Option<RepresentativeSource>,
    reason: Option<String>,
    applied_at_ms: Option<i64>,
    applied_params_json: Option<String>,
    started_at_ms: Option<i64>,
    member_count: u32,
    skipped: bool,
    /// Ids the last apply covered (`None` = applied before v15, or never).
    covered: Option<Vec<ImageId>>,
}

fn scene_row(conn: &Connection, id: SceneId) -> AppResult<SceneRow> {
    let row = conn
        .query_row(
            "SELECT representative_id, representative_source, representative_reason, applied_at_ms, applied_params_json,
                    started_at_ms, (SELECT COUNT(*) FROM images WHERE scene_id = s.id), skipped, applied_covered_json
             FROM scenes s WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    SceneRow {
                        representative_id: r.get(0)?,
                        source: r.get::<_, Option<String>>(1)?.and_then(|s| RepresentativeSource::parse(&s)),
                        reason: r.get(2)?,
                        applied_at_ms: r.get(3)?,
                        applied_params_json: r.get(4)?,
                        started_at_ms: r.get(5)?,
                        member_count: r.get(6)?,
                        skipped: r.get(7)?,
                        covered: None,
                    },
                    r.get::<_, Option<String>>(8)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| AppError::not_found(format!("scene {id}")))?;
    let (mut scene, covered) = row;
    scene.covered = covered.map(|j| serde_json::from_str(&j)).transpose()?;
    Ok(scene)
}

/// Per-photo state of scene `scene_id`'s members.
fn scene_states(conn: &Connection, scene_id: SceneId) -> AppResult<HashMap<ImageId, ImageEditState>> {
    batches::edit_states_where(conn, &format!("i.scene_id = {scene_id}"))
}

/// The plan entry of one scene. `keepers` = its keepers, `members` = all its members (capture
/// order), `states` = per-photo state covering at least `members`.
fn entry_for(
    conn: &Connection,
    scene_id: SceneId,
    keepers: &[PlanImage],
    members: &[ImageId],
    states: &HashMap<ImageId, ImageEditState>,
) -> AppResult<SceneEditEntry> {
    let row = scene_row(conn, scene_id)?;
    let keeper_of = |id: Option<ImageId>| id.and_then(|id| keepers.iter().find(|k| k.id == id));
    let (rep, source, reason) = match (keeper_of(row.representative_id), row.source) {
        (Some(k), Some(src)) => (k.clone(), src, row.reason.clone().unwrap_or_default()),
        _ => {
            // None chosen yet, or the chosen one is no longer a keeper of this scene: propose.
            let (id, reason) = propose_representative(conn, scene_id, keepers)?;
            conn.execute(
                "UPDATE scenes SET representative_id = ?2, representative_source = 'auto', representative_reason = ?3
                 WHERE id = ?1",
                params![scene_id, id, reason],
            )?;
            let k = keepers.iter().find(|k| k.id == id).cloned().ok_or_else(|| AppError::internal("bad proposal"))?;
            (k, RepresentativeSource::Auto, reason)
        }
    };
    let edited_at_ms: Option<i64> = if rep.has_edits {
        conn.query_row("SELECT MAX(updated_at) FROM adjustment_history WHERE image_id = ?1", [rep.id], |r| r.get(0))?
    } else {
        None
    };
    let status = match &row.applied_params_json {
        Some(json) => {
            let applied: ParametricAdjustments = serde_json::from_str(json)?;
            if applied == repo::get_adjustments(conn, rep.id)? {
                SceneEditStatus::Applied
            } else {
                SceneEditStatus::Outdated
            }
        }
        None if rep.has_edits => SceneEditStatus::Edited,
        None => SceneEditStatus::ToEdit,
    };
    let source_of = |id: ImageId| states.get(&id).map(|s| s.edit_source).unwrap_or(EditSource::None);
    let applied_ids: Vec<ImageId> =
        members.iter().copied().filter(|&id| id != rep.id && source_of(id) == EditSource::SceneApply).collect();
    let needs_review_ids: Vec<ImageId> =
        keepers.iter().map(|k| k.id).filter(|id| states.get(id).is_some_and(|s| s.needs_review)).collect();
    let unapplied_keeper_ids: Vec<ImageId> = if row.applied_params_json.is_none() || row.skipped {
        Vec::new()
    } else {
        keepers
            .iter()
            .map(|k| k.id)
            .filter(|&id| id != rep.id)
            .filter(|&id| match &row.covered {
                Some(covered) => {
                    !covered.contains(&id) && matches!(source_of(id), EditSource::None | EditSource::AutoStyle)
                }
                // Applied before v15 (coverage unknown): keepers that still have no edit.
                None => source_of(id) == EditSource::None,
            })
            .collect()
    };
    Ok(SceneEditEntry {
        scene_id,
        image_ids: keepers.iter().map(|k| k.id).collect(),
        member_count: row.member_count,
        representative_id: rep.id,
        representative_source: source,
        representative_reason: reason,
        edited: rep.has_edits,
        edited_at_ms,
        applied_at_ms: row.applied_at_ms,
        status,
        skipped: row.skipped,
        minor: keepers.len() as u32 <= MINOR_SCENE_MAX_KEEPERS,
        auto_edited: rep.has_edits && source_of(rep.id) == EditSource::AutoStyle,
        applied_ids,
        needs_review_ids,
        unapplied_keeper_ids,
    })
}

/// `get_edit_plan(projectId)`. Stores proposed representatives (source `auto`) so the plan is
/// stable between calls. Unknown project -> `not_found`. A scene's keepers are its members in
/// the project (scenes never span projects: detection runs per folder).
pub fn edit_plan(conn: &Connection, project_id: ProjectId) -> AppResult<EditPlan> {
    let scope = FolderScope::resolve(conn, None, Some(project_id))?; // not_found for unknown projects
    let rule = repo::keeper_rule(conn)?;
    let images = scope_images(conn, &scope)?;
    let states = batches::edit_states_where(conn, &scope.predicate("i.folder_id"))?;
    let keepers: Vec<&PlanImage> =
        images.iter().filter(|i| rule.is_keeper_values(i.pick, i.rating, i.suggested_pick)).collect();
    let mut by_scene: HashMap<SceneId, Vec<PlanImage>> = HashMap::new();
    let mut members: HashMap<SceneId, Vec<ImageId>> = HashMap::new();
    let mut unassigned = Vec::new();
    for i in &images {
        if let Some(s) = i.scene_id {
            members.entry(s).or_default().push(i.id);
        }
    }
    for k in &keepers {
        match k.scene_id {
            Some(s) => by_scene.entry(s).or_default().push((*k).clone()),
            None => unassigned.push(k.id),
        }
    }
    let mut order: Vec<(Option<i64>, SceneId)> = Vec::with_capacity(by_scene.len());
    for &s in by_scene.keys() {
        order.push((scene_row(conn, s)?.started_at_ms, s));
    }
    order.sort_by_key(|a| (a.0.is_none(), a.0, a.1));
    let mut scenes = Vec::with_capacity(order.len());
    for (_, s) in order {
        scenes.push(entry_for(conn, s, &by_scene[&s], &members[&s], &states)?);
    }
    let edit_states: Vec<ImageEditState> = keepers
        .iter()
        .map(|k| {
            states.get(&k.id).cloned().ok_or_else(|| AppError::internal(format!("no edit state for image {}", k.id)))
        })
        .collect::<AppResult<_>>()?;
    let needs_review_ids: Vec<ImageId> = edit_states.iter().filter(|s| s.needs_review).map(|s| s.image_id).collect();
    let mut counts = EditPlanCounts {
        scenes: scenes.len() as u32,
        needs_review: needs_review_ids.len() as u32,
        unassigned_keepers: unassigned.len() as u32,
        ..EditPlanCounts::default()
    };
    for e in &scenes {
        counts.minor += u32::from(e.minor);
        if e.skipped {
            counts.skipped += 1;
            continue;
        }
        counts.unapplied_keepers += e.unapplied_keeper_ids.len() as u32;
        match e.status {
            SceneEditStatus::ToEdit => counts.to_edit += 1,
            SceneEditStatus::Edited => counts.edited += 1,
            SceneEditStatus::Applied => counts.applied += 1,
            SceneEditStatus::Outdated => counts.outdated += 1,
        }
    }
    Ok(EditPlan {
        project_id,
        keeper_rule: rule,
        keeper_ids: keepers.iter().map(|k| k.id).collect(),
        outdated: counts.unassigned_keepers > 0 || counts.unapplied_keepers > 0,
        unassigned_keeper_ids: unassigned,
        scenes,
        counts,
        edit_states,
        needs_review_ids,
    })
}

/// Keepers of scene `scene_id` and all its members (capture order).
fn scene_keepers(conn: &Connection, scene_id: SceneId) -> AppResult<(Vec<PlanImage>, Vec<ImageId>)> {
    let rule = repo::keeper_rule(conn)?;
    let folders: Vec<FolderId> = conn
        .prepare_cached("SELECT DISTINCT folder_id FROM images WHERE scene_id = ?1")?
        .query_map([scene_id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut keepers = Vec::new();
    let mut members = Vec::new();
    for f in folders {
        for i in scope_images(conn, &FolderScope::from(Some(f)))? {
            if i.scene_id == Some(scene_id) {
                members.push(i.id);
                if rule.is_keeper_values(i.pick, i.rating, i.suggested_pick) {
                    keepers.push(i);
                }
            }
        }
    }
    Ok((keepers, members))
}

/// The plan entry of scene `scene_id` (must have keepers: `invalid_argument` otherwise).
fn scene_entry(conn: &Connection, scene_id: SceneId) -> AppResult<SceneEditEntry> {
    let (keepers, members) = scene_keepers(conn, scene_id)?;
    if keepers.is_empty() {
        return Err(AppError::invalid(format!("scene {scene_id} has no keepers")));
    }
    entry_for(conn, scene_id, &keepers, &members, &scene_states(conn, scene_id)?)
}

/// `set_scene_representative`: `Some(image)` must be a keeper member of the scene
/// (`invalid_argument` otherwise); `None` returns the choice to Sieve (re-proposed).
pub fn set_representative(
    conn: &Connection,
    scene_id: SceneId,
    image_id: Option<ImageId>,
) -> AppResult<SceneEditEntry> {
    scene_row(conn, scene_id)?;
    let (keepers, _) = scene_keepers(conn, scene_id)?;
    if keepers.is_empty() {
        return Err(AppError::invalid(format!("scene {scene_id} has no keepers")));
    }
    match image_id {
        Some(id) => {
            if !keepers.iter().any(|k| k.id == id) {
                return Err(AppError::invalid(format!("image {id} is not a keeper of scene {scene_id}")));
            }
            conn.execute(
                "UPDATE scenes SET representative_id = ?2, representative_source = 'user', representative_reason = ?3
                 WHERE id = ?1",
                params![scene_id, id, REASON_USER],
            )?;
        }
        None => {
            conn.execute(
                "UPDATE scenes SET representative_id = NULL, representative_source = NULL, representative_reason = NULL
                 WHERE id = ?1",
                [scene_id],
            )?;
        }
    }
    scene_entry(conn, scene_id)
}

/// `set_scene_skipped`: marks scene `scene_id` skipped in the Edit step (or includes it
/// again). Unknown scene -> `not_found`; a scene without keepers -> `invalid_argument`.
/// Settings are never touched. Survives re-detection with the representative's scene.
pub fn set_skipped(conn: &Connection, scene_id: SceneId, skipped: bool) -> AppResult<SceneEditEntry> {
    scene_row(conn, scene_id)?;
    let entry = scene_entry(conn, scene_id)?;
    if entry.skipped == skipped {
        return Ok(entry);
    }
    conn.execute("UPDATE scenes SET skipped = ?2 WHERE id = ?1", params![scene_id, skipped])?;
    scene_entry(conn, scene_id)
}

/// One scene of an apply: the representative (anchor) and the frames to match to it.
#[derive(Debug, Clone)]
pub struct SceneApplyJob {
    pub scene_id: SceneId,
    pub representative: MatchImage,
    /// The representative's current adjustments (stored as `applied_params_json`).
    pub representative_adjustments: ParametricAdjustments,
    pub targets: Vec<MatchImage>,
    /// Left alone because the user edited them after this scene's last apply.
    pub skipped: Vec<ImageId>,
    /// Left alone because the caller excluded them (`SceneApplyOptions.excludeIds`).
    pub excluded: Vec<ImageId>,
}

impl SceneApplyJob {
    /// Every frame this apply considered (stored as the scene's coverage).
    fn covered(&self) -> Vec<ImageId> {
        let mut ids: Vec<ImageId> = std::iter::once(self.representative.src.id)
            .chain(self.targets.iter().map(|t| t.src.id))
            .chain(self.skipped.iter().copied())
            .chain(self.excluded.iter().copied())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }
}

/// Scenes of `project_id` that `apply_all_edited_scenes` applies: not skipped, and status
/// edited / outdated, or applied with keepers added since (`unappliedKeeperIds`).
pub fn edited_scenes(conn: &Connection, project_id: ProjectId) -> AppResult<Vec<SceneId>> {
    Ok(edit_plan(conn, project_id)?
        .scenes
        .into_iter()
        .filter(|s| {
            !s.skipped
                && (matches!(s.status, SceneEditStatus::Edited | SceneEditStatus::Outdated)
                    || (s.status == SceneEditStatus::Applied && !s.unapplied_keeper_ids.is_empty()))
        })
        .map(|s| s.scene_id)
        .collect())
}

/// Resolves what to match for each scene. A scene whose representative has no edits ->
/// `invalid_argument` ("Edit the representative first"); unknown scene -> `not_found`.
/// Scenes with no targets left are returned with empty `targets`.
pub fn apply_inputs(
    conn: &Connection,
    scene_ids: &[SceneId],
    options: &SceneApplyOptions,
) -> AppResult<Vec<SceneApplyJob>> {
    let mut jobs = Vec::with_capacity(scene_ids.len());
    for &scene_id in scene_ids {
        let (keepers, members) = scene_keepers(conn, scene_id)?;
        if keepers.is_empty() {
            return Err(AppError::invalid(format!("scene {scene_id} has no keepers")));
        }
        let entry = entry_for(conn, scene_id, &keepers, &members, &scene_states(conn, scene_id)?)?;
        if !entry.edited {
            return Err(AppError::invalid("Edit the scene's representative photo first, then apply it to the scene."));
        }
        let applied_batch: Option<EditBatchId> =
            conn.query_row("SELECT applied_batch_id FROM scenes WHERE id = ?1", [scene_id], |r| r.get(0))?;
        let pool: Vec<ImageId> =
            if options.include_non_keepers { members } else { keepers.iter().map(|k| k.id).collect() };
        let mut target_ids = Vec::new();
        let mut skipped = Vec::new();
        let mut excluded = Vec::new();
        for id in pool.into_iter().filter(|&id| id != entry.representative_id) {
            if options.exclude_ids.contains(&id) {
                excluded.push(id);
                continue;
            }
            let user_edited = match (options.skip_user_edited, applied_batch) {
                (true, Some(b)) => match batches::written_by(conn, b, id)? {
                    Some(written) => written != repo::get_adjustments(conn, id)?,
                    None => false,
                },
                _ => false,
            };
            if user_edited {
                skipped.push(id);
            } else {
                target_ids.push(id);
            }
        }
        let rep = super::store::match_inputs(conn, &[entry.representative_id])?.remove(0);
        jobs.push(SceneApplyJob {
            scene_id,
            representative_adjustments: rep.adjustments.clone(),
            representative: rep,
            targets: super::store::match_inputs(conn, &target_ids)?,
            skipped,
            excluded,
        });
    }
    Ok(jobs)
}

/// "Needs a look" reason of a match that did not converge (its notes, else the default).
fn review_reason(p: &MatchPreview) -> Option<String> {
    if p.converged {
        return None;
    }
    Some(if p.notes.is_empty() { batches::REVIEW_REASON_DEFAULT.to_owned() } else { p.notes.join("; ") })
}

/// Commits the matched settings of every job as one edit batch ("Apply to Scene") and marks
/// the scenes applied (and not skipped). `previews[i]` are the `match_scene`-style results of
/// `jobs[i]`; jobs without previews (a cancelled apply) are not committed.
pub fn commit_apply(
    conn: &mut Connection,
    jobs: &[SceneApplyJob],
    previews: &[Vec<MatchPreview>],
    cancelled: bool,
) -> AppResult<ApplyScenesResult> {
    let mut items = Vec::new();
    for (job, pv) in jobs.iter().zip(previews) {
        for p in pv {
            items.push(BatchItem {
                image_id: p.target_id,
                adjustments: p.adjustments.clone(),
                scene_id: Some(job.scene_id),
                review_reason: review_reason(p),
            });
        }
    }
    let batch = batches::commit_recorded(conn, &items, batches::LABEL_APPLY_SCENE, BatchKind::SceneApply)?;
    let now = now_ms();
    let mut scenes = Vec::with_capacity(jobs.len());
    for (job, pv) in jobs.iter().zip(previews) {
        conn.execute(
            "UPDATE scenes SET applied_at_ms = ?2, applied_params_json = ?3,
                               applied_batch_id = COALESCE(?4, applied_batch_id),
                               applied_covered_json = ?5, skipped = 0
             WHERE id = ?1",
            params![
                job.scene_id,
                now,
                serde_json::to_string(&job.representative_adjustments)?,
                batch.batch_id,
                serde_json::to_string(&job.covered())?
            ],
        )?;
        let mut notes: Vec<String> = Vec::new();
        for p in pv {
            for n in &p.notes {
                if !notes.contains(n) {
                    notes.push(n.clone());
                }
            }
        }
        scenes.push(SceneApplyOutcome {
            scene_id: job.scene_id,
            representative_id: job.representative.src.id,
            changed_ids: pv.iter().map(|p| p.target_id).filter(|id| batch.changed_ids.contains(id)).collect(),
            skipped_ids: job.skipped.clone(),
            excluded_ids: job.excluded.clone(),
            not_converged_ids: pv.iter().filter(|p| !p.converged).map(|p| p.target_id).collect(),
            notes,
        });
    }
    Ok(ApplyScenesResult { batch, scenes, cancelled })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::develop::history;
    use crate::ipc::error::ErrorKind;

    /// Project (one folder) with 4 images in one scene: 1 picked, 2 rated 2, 3 rejected, 4
    /// untouched (suggested pick when `suggest`).
    fn fixture() -> (Connection, ProjectId, SceneId, Vec<ImageId>) {
        let mut conn = open_in_memory();
        let dir = tempfile::tempdir().unwrap();
        for n in ["A.ARW", "B.ARW", "C.ARW", "D.ARW"] {
            let mut bytes = b"II*\0".to_vec();
            bytes.resize(64, 0);
            std::fs::write(dir.path().join(n), bytes).unwrap();
        }
        let s = repo::import_folder(&mut conn, dir.path(), &ImportOptions::raw_only(false)).unwrap();
        let ids: Vec<ImageId> = conn
            .prepare("SELECT id FROM images ORDER BY file_name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        repo::set_pick(&mut conn, &[ids[0]], PickFlag::Pick).unwrap();
        repo::set_rating(&mut conn, &[ids[1]], 2).unwrap();
        repo::set_pick(&mut conn, &[ids[2]], PickFlag::Reject).unwrap();
        for (id, overall, sp) in [(ids[0], 0.6, "pick"), (ids[1], 0.9, "unflagged"), (ids[3], 0.95, "pick")] {
            conn.execute(
                "INSERT INTO quality_scores (image_id, overall, global_sharpness, face_count, clipped_highlights_pct,
                                             clipped_shadows_pct, mean_luma, model_version, analyzed_at,
                                             suggested_rating, suggested_pick)
                 VALUES (?1, ?2, 0.5, 0, 0, 0, 0.5, 'test', 1, 3, ?3)",
                params![id, overall, sp],
            )
            .unwrap();
        }
        let scene = super::super::store::create_scene(&mut conn, &ids).unwrap();
        (conn, s.project_id, scene.id, ids)
    }

    /// Preview features of a frame with mean encoded luma `luma` (0..1) and colour cast `a`.
    fn features(luma: f32, a: f32) -> super::super::SceneFeatures {
        let mut luma_hist = vec![0.0; 16];
        luma_hist[((luma * 16.0) as usize).min(15)] = 1.0;
        let mut ab_hist = vec![0.0; 64];
        ab_hist[(((a + 0.2) / 0.4 * 8.0) as usize).min(7) * 8 + 4] = 1.0;
        super::super::SceneFeatures {
            log_mean_luma: luma.max(1e-3).powf(2.2).log2(),
            luma_hist,
            ab_hist,
            mean_oklab: [luma, a, 0.0],
        }
    }

    fn store_features(conn: &mut Connection, items: &[(ImageId, super::super::SceneFeatures)]) {
        super::super::store::save_features(conn, items).unwrap();
    }

    #[test]
    fn representative_is_the_most_typical_of_the_best_keepers() {
        let (mut conn, project, scene, ids) = fixture();
        // Keepers 0 (overall 0.6), 1 (0.9), 3 (0.95); pool = {1, 3} (within 0.1 of the best).
        // 3 is the odd one out (dark, cast); 0, 1, 2 share the scene's typical light.
        store_features(
            &mut conn,
            &[
                (ids[0], features(0.5, 0.0)),
                (ids[1], features(0.52, 0.01)),
                (ids[2], features(0.5, 0.0)),
                (ids[3], features(0.1, 0.12)),
            ],
        );
        let plan = edit_plan(&conn, project).unwrap();
        let e = &plan.scenes[0];
        assert_eq!(e.scene_id, scene);
        assert_eq!(e.representative_id, ids[1], "typical light wins inside the quality pool");
        assert_eq!(e.representative_reason, REASON_TYPICAL);
        // The low-scored but typical keeper 0 is never proposed over the pool.
        let keepers: Vec<PlanImage> = scope_images(&conn, &FolderScope::resolve(&conn, None, Some(project)).unwrap())
            .unwrap()
            .into_iter()
            .filter(|i| i.id != ids[2])
            .collect();
        assert_eq!(propose_representative(&conn, scene, &keepers).unwrap().0, ids[1]);
        // When the best keeper is also typical, it is chosen with the "best" reason.
        store_features(&mut conn, &[(ids[3], features(0.51, 0.0))]);
        let (id, reason) = propose_representative(&conn, scene, &keepers).unwrap();
        assert_eq!((id, reason.as_str()), (ids[3], REASON_TYPICAL_BEST));
    }

    #[test]
    fn redetection_keeps_representative_and_applied_state() {
        let (mut conn, project, scene, ids) = fixture();
        set_representative(&conn, scene, Some(ids[1])).unwrap();
        conn.execute("UPDATE scenes SET applied_at_ms = 77, applied_params_json = '{}' WHERE id = ?1", [scene])
            .unwrap();
        let scope = FolderScope::resolve(&conn, None, Some(project)).unwrap();
        let scenes = super::super::store::replace_scenes(
            &mut conn,
            &scope,
            &[vec![ids[0]], vec![ids[1], ids[2]], vec![ids[3]]],
            false,
        )
        .unwrap();
        assert_eq!(scenes.len(), 3);
        let with_rep = scenes.iter().find(|s| s.image_ids.contains(&ids[1])).unwrap();
        type Row = (Option<ImageId>, Option<String>, Option<String>, Option<i64>, Option<String>);
        let (rep, source, reason, applied_at, json): Row =
            conn.query_row(
                "SELECT representative_id, representative_source, representative_reason, applied_at_ms, applied_params_json
                 FROM scenes WHERE id = ?1",
                [with_rep.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(rep, Some(ids[1]));
        assert_eq!(source.as_deref(), Some("user"));
        assert_eq!(reason.as_deref(), Some(REASON_USER));
        assert_eq!((applied_at, json.as_deref()), (Some(77), Some("{}")));
        // The other new scenes start without plan state.
        for s in scenes.iter().filter(|s| s.id != with_rep.id) {
            let rep: Option<ImageId> =
                conn.query_row("SELECT representative_id FROM scenes WHERE id = ?1", [s.id], |r| r.get(0)).unwrap();
            assert_eq!(rep, None);
        }
        // Merging two scenes that each had a representative: the user's choice wins.
        let a = scenes.iter().find(|s| s.image_ids == vec![ids[0]]).unwrap().id;
        set_representative(&conn, a, Some(ids[0])).unwrap();
        conn.execute("UPDATE scenes SET representative_source = 'auto' WHERE id = ?1", [a]).unwrap();
        let scenes = super::super::store::replace_scenes(&mut conn, &scope, std::slice::from_ref(&ids), false).unwrap();
        let (rep, source): (Option<ImageId>, Option<String>) = conn
            .query_row(
                "SELECT representative_id, representative_source FROM scenes WHERE id = ?1",
                [scenes[0].id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((rep, source.as_deref()), (Some(ids[1]), Some("user")));
    }

    #[test]
    fn keeper_rule_and_plan() {
        let (mut conn, folder, scene, ids) = fixture();
        let plan = edit_plan(&conn, folder).unwrap();
        // Picked, rated >= 1, and untouched-with-pick-suggestion are keepers; rejected is not.
        assert_eq!(plan.keeper_ids, vec![ids[0], ids[1], ids[3]]);
        assert!(plan.unassigned_keeper_ids.is_empty());
        assert_eq!(plan.scenes.len(), 1);
        let e = &plan.scenes[0];
        assert_eq!((e.scene_id, e.member_count, e.status), (scene, 4, SceneEditStatus::ToEdit));
        assert_eq!(e.representative_id, ids[3]); // best overall
        assert_eq!(e.representative_source, RepresentativeSource::Auto);

        repo::set_keeper_rule(&conn, &KeeperRule { min_rating: 3, use_suggestions: false }).unwrap();
        assert_eq!(edit_plan(&conn, folder).unwrap().keeper_ids, vec![ids[0]]);
        repo::set_keeper_rule(&conn, &KeeperRule::default()).unwrap();
        assert_eq!(
            repo::set_keeper_rule(&conn, &KeeperRule { min_rating: 0, use_suggestions: true }).unwrap_err().kind,
            ErrorKind::InvalidArgument
        );

        // User choice: must be a keeper member.
        assert_eq!(set_representative(&conn, scene, Some(ids[2])).unwrap_err().kind, ErrorKind::InvalidArgument);
        let e = set_representative(&conn, scene, Some(ids[1])).unwrap();
        assert_eq!((e.representative_id, e.representative_source), (ids[1], RepresentativeSource::User));
        assert_eq!(e.representative_reason, REASON_USER);

        // Not edited -> apply refused.
        let opts = SceneApplyOptions::default();
        assert_eq!(apply_inputs(&conn, &[scene], &opts).unwrap_err().kind, ErrorKind::InvalidArgument);
        let graded = ParametricAdjustments { exposure: 0.7, ..ParametricAdjustments::default() };
        history::commit(&mut conn, ids[1], &graded, "Exposure").unwrap();
        assert_eq!(edit_plan(&conn, folder).unwrap().scenes[0].status, SceneEditStatus::Edited);
        assert_eq!(edited_scenes(&conn, folder).unwrap(), vec![scene]);

        let jobs = apply_inputs(&conn, &[scene], &opts).unwrap();
        let targets: Vec<ImageId> = jobs[0].targets.iter().map(|t| t.src.id).collect();
        assert_eq!(targets, vec![ids[0], ids[3]]);
        // Fake matcher: copy the representative's settings.
        let previews: Vec<Vec<MatchPreview>> = jobs
            .iter()
            .map(|j| j.targets.iter().map(|t| fake_preview(t.src.id, &j.representative_adjustments)).collect())
            .collect();
        let r = commit_apply(&mut conn, &jobs, &previews, false).unwrap();
        assert_eq!(r.batch.changed_ids, vec![ids[0], ids[3]]);
        assert_eq!(r.scenes[0].changed_ids, vec![ids[0], ids[3]]);
        assert_eq!(edit_plan(&conn, folder).unwrap().scenes[0].status, SceneEditStatus::Applied);

        // The user retouches one target, then edits the representative again: outdated, and a
        // re-apply skips the retouched frame.
        history::commit(&mut conn, ids[0], &ParametricAdjustments { exposure: 0.1, ..graded.clone() }, "Exposure")
            .unwrap();
        history::commit(&mut conn, ids[1], &ParametricAdjustments { exposure: 0.9, ..graded }, "Exposure").unwrap();
        assert_eq!(edit_plan(&conn, folder).unwrap().scenes[0].status, SceneEditStatus::Outdated);
        let jobs = apply_inputs(&conn, &[scene], &opts).unwrap();
        assert_eq!(jobs[0].skipped, vec![ids[0]]);
        assert_eq!(jobs[0].targets.iter().map(|t| t.src.id).collect::<Vec<_>>(), vec![ids[3]]);

        // Undo the first batch: image 4 still has what that batch wrote -> restored.
        let u = batches::undo(&mut conn, r.batch.batch_id.unwrap()).unwrap();
        assert_eq!(u.restored_ids, vec![ids[3]]);
        assert_eq!(u.skipped_ids, vec![ids[0]]);
    }

    fn fake_preview(target: ImageId, adj: &ParametricAdjustments) -> MatchPreview {
        let stats = ImageStats {
            image_id: target,
            region: None,
            width: 1,
            height: 1,
            mean_luma: 0.5,
            log_mean_luma: -1.0,
            percentiles: LumaPercentiles::default(),
            clipped_highlights: 0.0,
            clipped_shadows: 0.0,
            mean_oklab: OklabColor::default(),
            neutral: NeutralEstimate::default(),
            white_balance: None,
            as_shot: None,
            lut_missing: false,
        };
        MatchPreview {
            target_id: target,
            anchor_ids: vec![],
            anchor_weight: 0.0,
            base: adj.clone(),
            full: adj.clone(),
            adjustments: adj.clone(),
            delta: MatchDelta::default(),
            reference: stats.clone(),
            before: stats.clone(),
            predicted: stats,
            converged: true,
            notes: vec![],
        }
    }

    /// Applies scene `scene` with the fake matcher; `not_converged` frames get a note.
    fn apply_fake(
        conn: &mut Connection,
        scene: SceneId,
        opts: &SceneApplyOptions,
        not_converged: &[ImageId],
    ) -> ApplyScenesResult {
        let jobs = apply_inputs(conn, &[scene], opts).unwrap();
        let previews: Vec<Vec<MatchPreview>> = jobs
            .iter()
            .map(|j| {
                j.targets
                    .iter()
                    .map(|t| {
                        let mut p = fake_preview(t.src.id, &j.representative_adjustments);
                        if not_converged.contains(&t.src.id) {
                            p.converged = false;
                            p.notes = vec!["Exposure did not match".into()];
                        }
                        p
                    })
                    .collect()
            })
            .collect();
        commit_apply(conn, &jobs, &previews, false).unwrap()
    }

    fn state(conn: &Connection, id: ImageId) -> ImageEditState {
        batches::edit_states(conn, &[id]).unwrap().remove(0)
    }

    #[test]
    fn persisted_photo_state_needs_review_and_sources() {
        let (mut conn, project, scene, ids) = fixture();
        set_representative(&conn, scene, Some(ids[1])).unwrap();
        assert_eq!(state(&conn, ids[0]).edit_source, EditSource::None);
        let graded = ParametricAdjustments { exposure: 0.7, ..ParametricAdjustments::default() };
        history::commit(&mut conn, ids[1], &graded, "Exposure").unwrap();
        assert_eq!(state(&conn, ids[1]).edit_source, EditSource::User);

        let r = apply_fake(&mut conn, scene, &SceneApplyOptions::default(), &[ids[3]]);
        let batch = r.batch.batch_id.unwrap();
        let s0 = state(&conn, ids[0]);
        assert_eq!(
            (s0.edit_source, s0.batch_id, s0.applied_scene_id, s0.needs_review),
            (EditSource::SceneApply, Some(batch), Some(scene), false)
        );
        let s3 = state(&conn, ids[3]);
        assert!(s3.needs_review);
        assert_eq!(s3.review_reason.as_deref(), Some("Exposure did not match"));

        // Persisted: a fresh plan read reports it.
        let plan = edit_plan(&conn, project).unwrap();
        assert_eq!(plan.needs_review_ids, vec![ids[3]]);
        assert_eq!(plan.counts.needs_review, 1);
        let e = &plan.scenes[0];
        assert_eq!(e.applied_ids, vec![ids[0], ids[3]]);
        assert_eq!(e.needs_review_ids, vec![ids[3]]);
        assert!(!e.auto_edited);
        assert_eq!(plan.edit_states.iter().map(|s| s.image_id).collect::<Vec<_>>(), plan.keeper_ids);

        // A user edit clears it; per-image undo back to the applied settings brings it back.
        let tweak = ParametricAdjustments { exposure: 0.9, ..graded.clone() };
        history::commit(&mut conn, ids[3], &tweak, "Exposure").unwrap();
        let s3 = state(&conn, ids[3]);
        assert_eq!((s3.edit_source, s3.needs_review, s3.batch_id), (EditSource::User, false, None));
        history::undo(&mut conn, ids[3]).unwrap();
        assert!(state(&conn, ids[3]).needs_review);
        // "Looks good" clears it for good (the settings are untouched).
        assert_eq!(batches::mark_reviewed(&conn, &[ids[3], ids[0]]).unwrap(), vec![ids[3]]);
        let s3 = state(&conn, ids[3]);
        assert_eq!((s3.edit_source, s3.needs_review), (EditSource::SceneApply, false));
        assert!(edit_plan(&conn, project).unwrap().needs_review_ids.is_empty());

        // Pasted / reset sources.
        history::apply_fields(&mut conn, &[ids[0]], &tweak, &[AdjustmentField::Exposure], history::LABEL_PASTE)
            .unwrap();
        assert_eq!(state(&conn, ids[0]).edit_source, EditSource::Pasted);
        history::commit(&mut conn, ids[0], &ParametricAdjustments::default(), history::LABEL_RESET).unwrap();
        assert_eq!(state(&conn, ids[0]).edit_source, EditSource::None);

        // Undoing the batch restores the provenance the frames had before it.
        let u = batches::undo(&mut conn, batch).unwrap();
        assert_eq!(u.restored_ids, vec![ids[3]]);
        assert_eq!(state(&conn, ids[3]).edit_source, EditSource::None);
        assert_eq!(batches::edit_states(&conn, &[999]).unwrap_err().kind, ErrorKind::NotFound);
    }

    #[test]
    fn auto_style_representative_is_reported() {
        let (mut conn, project, scene, ids) = fixture();
        set_representative(&conn, scene, Some(ids[1])).unwrap();
        let item = BatchItem {
            image_id: ids[1],
            adjustments: ParametricAdjustments { exposure: 0.4, ..ParametricAdjustments::default() },
            scene_id: None,
            review_reason: None,
        };
        let b = batches::commit_recorded(&mut conn, &[item], batches::LABEL_STYLE, BatchKind::StylePrediction).unwrap();
        let s = state(&conn, ids[1]);
        assert_eq!((s.edit_source, s.batch_id), (EditSource::AutoStyle, b.batch_id));
        assert!(edit_plan(&conn, project).unwrap().scenes[0].auto_edited);
        // The user takes over: no longer auto.
        history::commit(&mut conn, ids[1], &ParametricAdjustments { exposure: 0.5, ..Default::default() }, "Exposure")
            .unwrap();
        assert!(!edit_plan(&conn, project).unwrap().scenes[0].auto_edited);
    }

    #[test]
    fn keepers_added_after_apply_exclusions_and_outdated_plan() {
        let (mut conn, project, scene, ids) = fixture();
        // Keeper rule without suggestions: keepers 0 (pick) and 1 (2 stars).
        repo::set_keeper_rule(&conn, &KeeperRule { min_rating: 1, use_suggestions: false }).unwrap();
        set_representative(&conn, scene, Some(ids[1])).unwrap();
        let graded = ParametricAdjustments { exposure: 0.7, ..ParametricAdjustments::default() };
        history::commit(&mut conn, ids[1], &graded, "Exposure").unwrap();
        apply_fake(&mut conn, scene, &SceneApplyOptions::default(), &[]);
        let plan = edit_plan(&conn, project).unwrap();
        assert!(!plan.outdated);
        assert_eq!(plan.scenes[0].status, SceneEditStatus::Applied);
        assert!(plan.scenes[0].unapplied_keeper_ids.is_empty());

        // Two more keepers after the apply: 3 (picked, never touched) and 2 (un-rejected,
        // hand-edited -> not "unapplied").
        repo::set_pick(&mut conn, &[ids[3]], PickFlag::Pick).unwrap();
        repo::set_pick(&mut conn, &[ids[2]], PickFlag::Pick).unwrap();
        history::commit(&mut conn, ids[2], &ParametricAdjustments { exposure: -0.3, ..Default::default() }, "Exposure")
            .unwrap();
        let plan = edit_plan(&conn, project).unwrap();
        let e = &plan.scenes[0];
        assert_eq!(e.status, SceneEditStatus::Applied, "the representative did not change");
        assert_eq!(e.unapplied_keeper_ids, vec![ids[3]]);
        assert!(plan.outdated);
        assert_eq!(plan.counts.unapplied_keepers, 1);
        assert_eq!(edited_scenes(&conn, project).unwrap(), vec![scene], "apply all covers new keepers");

        // "Apply with options" excluding the new keeper: covered, so no longer unapplied.
        let opts = SceneApplyOptions { exclude_ids: vec![ids[3]], skip_user_edited: true, ..Default::default() };
        let r = apply_fake(&mut conn, scene, &opts, &[]);
        assert_eq!(r.scenes[0].excluded_ids, vec![ids[3]]);
        assert!(!r.scenes[0].changed_ids.contains(&ids[3]));
        let plan = edit_plan(&conn, project).unwrap();
        assert!(plan.scenes[0].unapplied_keeper_ids.is_empty());
        assert!(!plan.outdated);
        assert_eq!(state(&conn, ids[3]).edit_source, EditSource::None);

        // Keepers outside every scene make the plan outdated too.
        conn.execute("UPDATE images SET scene_id = NULL WHERE id = ?1", [ids[3]]).unwrap();
        let plan = edit_plan(&conn, project).unwrap();
        assert_eq!(plan.unassigned_keeper_ids, vec![ids[3]]);
        assert!(plan.outdated);
        assert_eq!(plan.counts.unassigned_keepers, 1);
    }

    #[test]
    fn skipped_and_minor_scenes() {
        let (mut conn, project, scene, ids) = fixture();
        let plan = edit_plan(&conn, project).unwrap();
        assert!(!plan.scenes[0].minor, "3 keepers");
        assert_eq!((plan.counts.scenes, plan.counts.to_edit, plan.counts.skipped), (1, 1, 0));

        let e = set_skipped(&conn, scene, true).unwrap();
        assert!(e.skipped);
        assert_eq!(e.status, SceneEditStatus::ToEdit, "status is kept");
        let plan = edit_plan(&conn, project).unwrap();
        assert_eq!((plan.counts.to_edit, plan.counts.skipped), (0, 1));
        // Skipped scenes are left alone by apply all, even when edited.
        history::commit(
            &mut conn,
            plan.scenes[0].representative_id,
            &ParametricAdjustments { exposure: 0.2, ..Default::default() },
            "Exposure",
        )
        .unwrap();
        assert!(edited_scenes(&conn, project).unwrap().is_empty());
        assert!(set_skipped(&conn, scene, true).unwrap().skipped, "idempotent");
        // Survives re-detection with the representative.
        let scope = FolderScope::resolve(&conn, None, Some(project)).unwrap();
        let scenes = super::super::store::replace_scenes(&mut conn, &scope, std::slice::from_ref(&ids), false).unwrap();
        let scene = scenes[0].id;
        assert!(edit_plan(&conn, project).unwrap().scenes[0].skipped);
        // Explicitly applying it includes it again.
        apply_fake(&mut conn, scene, &SceneApplyOptions::default(), &[]);
        assert!(!edit_plan(&conn, project).unwrap().scenes[0].skipped);
        assert!(!set_skipped(&conn, scene, false).unwrap().skipped);
        assert_eq!(set_skipped(&conn, 999, true).unwrap_err().kind, ErrorKind::NotFound);

        // Minor: at most MINOR_SCENE_MAX_KEEPERS keepers.
        repo::set_keeper_rule(&conn, &KeeperRule { min_rating: 3, use_suggestions: false }).unwrap();
        let plan = edit_plan(&conn, project).unwrap();
        assert!(plan.scenes[0].minor);
        assert_eq!(plan.counts.minor, 1);
    }

    #[test]
    fn cancelled_apply_commits_finished_scenes_only() {
        let (mut conn, project, scene, ids) = fixture();
        set_representative(&conn, scene, Some(ids[1])).unwrap();
        history::commit(&mut conn, ids[1], &ParametricAdjustments { exposure: 0.7, ..Default::default() }, "Exposure")
            .unwrap();
        // Nothing matched before the cancel: no batch, scene untouched.
        let jobs = apply_inputs(&conn, &[scene], &SceneApplyOptions::default()).unwrap();
        let r = commit_apply(&mut conn, &jobs[..0], &[], true).unwrap();
        assert!(r.cancelled);
        assert_eq!((r.batch.batch_id, r.scenes.len()), (None, 0));
        assert_eq!(edit_plan(&conn, project).unwrap().scenes[0].status, SceneEditStatus::Edited);
    }
}
