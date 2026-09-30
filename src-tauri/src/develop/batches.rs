//! Undoable multi-image edits (IPC v14, migration 0012): scene applies and style predictions
//! commit through [`commit_recorded`], which writes the per-image history entries (like
//! `history::commit_batch`) and records each changed image's settings before and after in
//! `edit_batch_items`. [`undo`] puts back the "before" settings of images whose current
//! settings still equal what the batch wrote (one "Undo <label>" history entry each); images
//! edited since are left alone and reported. Implemented by the architect; rust-engine-dev
//! owns it from here.

use rusqlite::{params, Connection, OptionalExtension};

use super::history;
use crate::db::{now_ms, repo};
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::{EditBatchId, EditBatchResult, ImageId, ParametricAdjustments, SceneId, UndoBatchResult};

/// `edit_batches.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchKind {
    SceneApply,
    StylePrediction,
}

impl BatchKind {
    fn as_str(self) -> &'static str {
        match self {
            BatchKind::SceneApply => "scene_apply",
            BatchKind::StylePrediction => "style_prediction",
        }
    }
}

/// History label of scene applies.
pub const LABEL_APPLY_SCENE: &str = "Apply to Scene";
/// History label of `apply_style_prediction`.
pub const LABEL_STYLE: &str = "Auto Edit (My Style)";
/// Prefix of the entries `undo` writes.
pub const LABEL_UNDO_PREFIX: &str = "Undo ";

/// A planned write of one image: new settings, and the scene it was edited for.
#[derive(Debug, Clone)]
pub struct BatchItem {
    pub image_id: ImageId,
    pub adjustments: ParametricAdjustments,
    pub scene_id: Option<SceneId>,
}

/// Commits every item (one `label` history entry per changed image) and records the batch.
/// Atomic (unknown image -> `not_found`, invalid values -> `invalid_argument`, nothing
/// written). An image listed twice -> `invalid_argument`. No batch row when nothing changed.
pub fn commit_recorded(
    conn: &mut Connection,
    items: &[BatchItem],
    label: &str,
    kind: BatchKind,
) -> AppResult<EditBatchResult> {
    for (i, it) in items.iter().enumerate() {
        if items[..i].iter().any(|o| o.image_id == it.image_id) {
            return Err(AppError::invalid(format!("image {} listed twice", it.image_id)));
        }
    }
    let tx = conn.savepoint()?;
    let mut befores = Vec::with_capacity(items.len());
    for it in items {
        befores.push(repo::get_adjustments(&tx, it.image_id)?);
    }
    let pairs: Vec<(ImageId, ParametricAdjustments)> =
        items.iter().map(|it| (it.image_id, it.adjustments.clone())).collect();
    let inner = tx;
    let changed = history::commit_batch_in(&inner, &pairs, label)?;
    let batch_id = if changed.is_empty() {
        None
    } else {
        inner.execute(
            "INSERT INTO edit_batches (label, kind, created_at) VALUES (?1, ?2, ?3)",
            params![label, kind.as_str(), now_ms()],
        )?;
        let id = inner.last_insert_rowid();
        for (it, before) in items.iter().zip(&befores) {
            if changed.contains(&it.image_id) {
                inner.execute(
                    "INSERT INTO edit_batch_items (batch_id, image_id, scene_id, before_json, after_json)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        id,
                        it.image_id,
                        it.scene_id,
                        serde_json::to_string(before)?,
                        serde_json::to_string(&repo::get_adjustments(&inner, it.image_id)?)?
                    ],
                )?;
            }
        }
        Some(id)
    };
    inner.commit()?;
    Ok(EditBatchResult { batch_id, label: label.to_owned(), changed_ids: changed })
}

/// The settings batch `batch_id` wrote for `image_id`, if it changed that image.
pub fn written_by(
    conn: &Connection,
    batch_id: EditBatchId,
    image_id: ImageId,
) -> AppResult<Option<ParametricAdjustments>> {
    let json: Option<String> = conn
        .query_row(
            "SELECT after_json FROM edit_batch_items WHERE batch_id = ?1 AND image_id = ?2",
            params![batch_id, image_id],
            |r| r.get(0),
        )
        .optional()?;
    json.map(|j| serde_json::from_str(&j).map_err(AppError::from)).transpose()
}

/// Undoes batch `batch_id` (see the module docs). Unknown batch -> `not_found`; already undone
/// -> `invalid_argument`. Atomic.
pub fn undo(conn: &mut Connection, batch_id: EditBatchId) -> AppResult<UndoBatchResult> {
    let row: Option<(String, Option<i64>)> = conn
        .query_row("SELECT label, undone_at FROM edit_batches WHERE id = ?1", [batch_id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?;
    let (label, undone) = row.ok_or_else(|| AppError::not_found(format!("edit batch {batch_id}")))?;
    if undone.is_some() {
        return Err(AppError::invalid("this edit was already undone"));
    }
    let items: Vec<(ImageId, String, String)> = conn
        .prepare("SELECT image_id, before_json, after_json FROM edit_batch_items WHERE batch_id = ?1 ORDER BY rowid")?
        .query_map([batch_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?;
    let mut result = UndoBatchResult::default();
    let mut restore = Vec::new();
    for (id, before, after) in items {
        let after: ParametricAdjustments = serde_json::from_str(&after)?;
        if repo::get_adjustments(conn, id)? == after {
            restore.push((id, serde_json::from_str::<ParametricAdjustments>(&before)?));
        } else {
            result.skipped_ids.push(id);
        }
    }
    let undo_label: String = format!("{LABEL_UNDO_PREFIX}{label}").chars().take(100).collect();
    let inner = conn.savepoint()?;
    result.restored_ids = history::commit_batch_in(&inner, &restore, &undo_label)?;
    inner.execute("UPDATE edit_batches SET undone_at = ?2 WHERE id = ?1", params![batch_id, now_ms()])?;
    inner.commit()?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::ipc::error::ErrorKind;
    use crate::ipc::types::ImportOptions;

    fn fixture() -> Connection {
        let mut conn = open_in_memory();
        let dir = tempfile::tempdir().unwrap();
        for n in ["A.ARW", "B.ARW", "C.ARW"] {
            let mut bytes = b"II*\0".to_vec();
            bytes.resize(64, 0);
            std::fs::write(dir.path().join(n), bytes).unwrap();
        }
        repo::import_folder(&mut conn, dir.path(), &ImportOptions::raw_only(false)).unwrap();
        conn
    }

    fn adj(ev: f32) -> ParametricAdjustments {
        ParametricAdjustments { exposure: ev, ..ParametricAdjustments::default() }
    }

    #[test]
    fn commit_and_undo_round_trip() {
        let mut conn = fixture();
        let ids: Vec<ImageId> = conn
            .prepare("SELECT id FROM images ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        history::commit(&mut conn, ids[2], &adj(0.3), "Exposure").unwrap();
        let items: Vec<BatchItem> = ids
            .iter()
            .map(|&id| BatchItem {
                image_id: id,
                adjustments: adj(if id == ids[2] { 0.3 } else { 1.0 }),
                scene_id: None,
            })
            .collect();
        let r = commit_recorded(&mut conn, &items, LABEL_APPLY_SCENE, BatchKind::SceneApply).unwrap();
        assert_eq!(r.changed_ids, vec![ids[0], ids[1]]);
        let batch = r.batch_id.unwrap();
        assert_eq!(written_by(&conn, batch, ids[0]).unwrap(), Some(adj(1.0)));
        assert_eq!(written_by(&conn, batch, ids[2]).unwrap(), None);

        // The user touches one image after the batch: undo leaves it alone.
        history::commit(&mut conn, ids[1], &adj(1.5), "Exposure").unwrap();
        let u = undo(&mut conn, batch).unwrap();
        assert_eq!(u, UndoBatchResult { restored_ids: vec![ids[0]], skipped_ids: vec![ids[1]] });
        assert_eq!(repo::get_adjustments(&conn, ids[0]).unwrap(), ParametricAdjustments::default());
        assert_eq!(repo::get_adjustments(&conn, ids[1]).unwrap(), adj(1.5));
        let h = history::history(&conn, ids[0]).unwrap();
        assert_eq!(h.entries.last().unwrap().label, "Undo Apply to Scene");
        assert_eq!(undo(&mut conn, batch).unwrap_err().kind, ErrorKind::InvalidArgument);
        assert_eq!(undo(&mut conn, 999).unwrap_err().kind, ErrorKind::NotFound);

        // Nothing changed -> no batch; duplicates and unknown ids are refused atomically.
        let r = commit_recorded(&mut conn, &items[2..], LABEL_STYLE, BatchKind::StylePrediction).unwrap();
        assert_eq!(r.batch_id, None);
        let dup = vec![items[0].clone(), items[0].clone()];
        assert_eq!(
            commit_recorded(&mut conn, &dup, LABEL_STYLE, BatchKind::StylePrediction).unwrap_err().kind,
            ErrorKind::InvalidArgument
        );
        let bad = vec![items[0].clone(), BatchItem { image_id: 999, adjustments: adj(2.0), scene_id: None }];
        assert_eq!(
            commit_recorded(&mut conn, &bad, LABEL_STYLE, BatchKind::StylePrediction).unwrap_err().kind,
            ErrorKind::NotFound
        );
        assert_eq!(repo::get_adjustments(&conn, ids[0]).unwrap(), ParametricAdjustments::default());
    }
}
