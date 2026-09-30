//! Guided workflow, Edit step (IPC v14): per-folder edit plan over scenes, representative
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

use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension};

use super::MatchImage;
use crate::db::{now_ms, repo};
use crate::develop::batches::{self, BatchItem, BatchKind};
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::*;

/// Reason shown for a user-chosen representative.
pub const REASON_USER: &str = "Chosen by you";

/// What the plan knows about one image of the folder.
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

/// Images of `folder_id` in capture order (images without a capture time last, by file name).
pub fn folder_images(conn: &Connection, folder_id: FolderId) -> AppResult<Vec<PlanImage>> {
    let mut stmt = conn.prepare_cached(
        "SELECT i.id, i.scene_id, i.pick, i.rating, q.suggested_pick, q.overall,
                EXISTS (SELECT 1 FROM adjustments a WHERE a.image_id = i.id AND a.neutral = 0)
         FROM images i LEFT JOIN quality_scores q ON q.image_id = i.id
         WHERE i.folder_id = ?1
         ORDER BY i.captured_at_ms IS NULL, i.captured_at_ms, i.file_name, i.id",
    )?;
    let rows = stmt.query_map([folder_id], |r| {
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

/// Proposes the representative of a scene among its `keepers` (non-empty, capture order):
/// `(image, user-facing reason)`. Contract: vision-ml-dev (placeholder: highest
/// `QualityScore.overall`, then higher rating, then earlier).
pub fn propose_representative(conn: &Connection, scene_id: SceneId, keepers: &[PlanImage]) -> AppResult<(ImageId, String)> {
    let _ = (conn, scene_id);
    let best = keepers
        .iter()
        .enumerate()
        .max_by(|(ia, a), (ib, b)| {
            let (sa, sb) = (a.overall.unwrap_or(-1.0), b.overall.unwrap_or(-1.0));
            sa.total_cmp(&sb).then(a.rating.cmp(&b.rating)).then(ib.cmp(ia))
        })
        .map(|(_, k)| k)
        .ok_or_else(|| AppError::internal("scene without keepers"))?;
    let reason = if best.overall.is_some() { "Best-scored keeper of this scene" } else { "First keeper of this scene" };
    Ok((best.id, reason.to_owned()))
}

struct SceneRow {
    representative_id: Option<ImageId>,
    source: Option<RepresentativeSource>,
    reason: Option<String>,
    applied_at_ms: Option<i64>,
    applied_params_json: Option<String>,
    started_at_ms: Option<i64>,
    member_count: u32,
}

fn scene_row(conn: &Connection, id: SceneId) -> AppResult<SceneRow> {
    conn.query_row(
        "SELECT representative_id, representative_source, representative_reason, applied_at_ms, applied_params_json,
                started_at_ms, (SELECT COUNT(*) FROM images WHERE scene_id = s.id)
         FROM scenes s WHERE id = ?1",
        [id],
        |r| {
            Ok(SceneRow {
                representative_id: r.get(0)?,
                source: r.get::<_, Option<String>>(1)?.and_then(|s| RepresentativeSource::parse(&s)),
                reason: r.get(2)?,
                applied_at_ms: r.get(3)?,
                applied_params_json: r.get(4)?,
                started_at_ms: r.get(5)?,
                member_count: r.get(6)?,
            })
        },
    )
    .optional()?
    .ok_or_else(|| AppError::not_found(format!("scene {id}")))
}

fn entry_for(conn: &Connection, scene_id: SceneId, keepers: &[PlanImage]) -> AppResult<SceneEditEntry> {
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
    })
}

/// `get_edit_plan(folderId)`. Stores proposed representatives (source `auto`) so the plan is
/// stable between calls. Unknown folder -> `not_found`.
pub fn edit_plan(conn: &Connection, folder_id: FolderId) -> AppResult<EditPlan> {
    repo::workflow_step(conn, folder_id)?; // not_found for unknown folders
    let rule = repo::keeper_rule(conn)?;
    let images = folder_images(conn, folder_id)?;
    let keepers: Vec<&PlanImage> =
        images.iter().filter(|i| rule.is_keeper_values(i.pick, i.rating, i.suggested_pick)).collect();
    let mut by_scene: HashMap<SceneId, Vec<PlanImage>> = HashMap::new();
    let mut unassigned = Vec::new();
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
    order.sort_by(|a, b| (a.0.is_none(), a.0, a.1).cmp(&(b.0.is_none(), b.0, b.1)));
    let mut scenes = Vec::with_capacity(order.len());
    for (_, s) in order {
        scenes.push(entry_for(conn, s, &by_scene[&s])?);
    }
    Ok(EditPlan {
        folder_id,
        keeper_rule: rule,
        keeper_ids: keepers.iter().map(|k| k.id).collect(),
        unassigned_keeper_ids: unassigned,
        scenes,
    })
}

/// Keepers of scene `scene_id` (capture order) and the folder the plan is computed for.
fn scene_keepers(conn: &Connection, scene_id: SceneId) -> AppResult<(Vec<PlanImage>, Vec<ImageId>)> {
    let rule = repo::keeper_rule(conn)?;
    let folders: Vec<FolderId> = conn
        .prepare_cached("SELECT DISTINCT folder_id FROM images WHERE scene_id = ?1")?
        .query_map([scene_id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut keepers = Vec::new();
    let mut members = Vec::new();
    for f in folders {
        for i in folder_images(conn, f)? {
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

/// `set_scene_representative`: `Some(image)` must be a keeper member of the scene
/// (`invalid_argument` otherwise); `None` returns the choice to Sieve (re-proposed).
pub fn set_representative(conn: &Connection, scene_id: SceneId, image_id: Option<ImageId>) -> AppResult<SceneEditEntry> {
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
    entry_for(conn, scene_id, &keepers)
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
}

/// Scenes of `folder_id` that `apply_all_edited_scenes` applies (status edited / outdated).
pub fn edited_scenes(conn: &Connection, folder_id: FolderId) -> AppResult<Vec<SceneId>> {
    Ok(edit_plan(conn, folder_id)?
        .scenes
        .into_iter()
        .filter(|s| matches!(s.status, SceneEditStatus::Edited | SceneEditStatus::Outdated))
        .map(|s| s.scene_id)
        .collect())
}

/// Resolves what to match for each scene. A scene whose representative has no edits ->
/// `invalid_argument` ("Edit the representative first"); unknown scene -> `not_found`.
/// Scenes with no targets left are returned with empty `targets`.
pub fn apply_inputs(conn: &Connection, scene_ids: &[SceneId], options: &SceneApplyOptions) -> AppResult<Vec<SceneApplyJob>> {
    let mut jobs = Vec::with_capacity(scene_ids.len());
    for &scene_id in scene_ids {
        let (keepers, members) = scene_keepers(conn, scene_id)?;
        if keepers.is_empty() {
            return Err(AppError::invalid(format!("scene {scene_id} has no keepers")));
        }
        let entry = entry_for(conn, scene_id, &keepers)?;
        if !entry.edited {
            return Err(AppError::invalid("Edit the scene's representative photo first, then apply it to the scene."));
        }
        let applied_batch: Option<EditBatchId> =
            conn.query_row("SELECT applied_batch_id FROM scenes WHERE id = ?1", [scene_id], |r| r.get(0))?;
        let pool: Vec<ImageId> =
            if options.include_non_keepers { members } else { keepers.iter().map(|k| k.id).collect() };
        let mut target_ids = Vec::new();
        let mut skipped = Vec::new();
        for id in pool.into_iter().filter(|&id| id != entry.representative_id) {
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
        });
    }
    Ok(jobs)
}

/// Commits the matched settings of every job as one edit batch ("Apply to Scene") and marks
/// the scenes applied. `previews[i]` are the `match_scene`-style results of `jobs[i]`.
pub fn commit_apply(
    conn: &mut Connection,
    jobs: &[SceneApplyJob],
    previews: &[Vec<MatchPreview>],
) -> AppResult<ApplyScenesResult> {
    let mut items = Vec::new();
    for (job, pv) in jobs.iter().zip(previews) {
        for p in pv {
            items.push(BatchItem { image_id: p.target_id, adjustments: p.adjustments.clone(), scene_id: Some(job.scene_id) });
        }
    }
    let batch = batches::commit_recorded(conn, &items, batches::LABEL_APPLY_SCENE, BatchKind::SceneApply)?;
    let now = now_ms();
    let mut scenes = Vec::with_capacity(jobs.len());
    for (job, pv) in jobs.iter().zip(previews) {
        conn.execute(
            "UPDATE scenes SET applied_at_ms = ?2, applied_params_json = ?3,
                               applied_batch_id = COALESCE(?4, applied_batch_id)
             WHERE id = ?1",
            params![job.scene_id, now, serde_json::to_string(&job.representative_adjustments)?, batch.batch_id],
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
            not_converged_ids: pv.iter().filter(|p| !p.converged).map(|p| p.target_id).collect(),
            notes,
        });
    }
    Ok(ApplyScenesResult { batch, scenes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::develop::history;
    use crate::ipc::error::ErrorKind;

    /// Folder with 4 images in one scene: 1 picked, 2 rated 2, 3 rejected, 4 untouched
    /// (suggested pick when `suggest`).
    fn fixture() -> (Connection, FolderId, SceneId, Vec<ImageId>) {
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
        (conn, s.folder_id, scene.id, ids)
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
        let r = commit_apply(&mut conn, &jobs, &previews).unwrap();
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
}
