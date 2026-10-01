//! Undoable multi-image edits (IPC v14, migration 0012): scene applies and style predictions
//! commit through [`commit_recorded`], which writes the per-image history entries (like
//! `history::commit_batch`) and records each changed image's settings before and after in
//! `edit_batch_items`. [`undo`] puts back the "before" settings of images whose current
//! settings still equal what the batch wrote (one "Undo <label>" history entry each); images
//! edited since are left alone and reported. Implemented by the architect; rust-engine-dev
//! owns it from here.

//!
//! Per-photo workflow state (IPC v15, migration 0013) is derived from the history entry an
//! image's cursor points at: its `source` and `batch_id`, and that batch's item for the image
//! (scene, review reason, reviewed). See [`edit_states`].

use std::collections::HashMap;

use rusqlite::{params, params_from_iter, Connection, OptionalExtension};

use super::history;
use crate::db::{now_ms, repo};
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::{
    EditBatchId, EditBatchResult, EditSource, ImageEditState, ImageId, ParametricAdjustments, SceneId, UndoBatchResult,
};

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
    /// "Needs a look" reason (scene applies whose match did not converge); `None` = fine.
    pub review_reason: Option<String>,
}

/// `(source, batch_id)` of the history entry image `id`'s cursor points at (`None` = no
/// history).
fn cursor_entry(conn: &Connection, id: ImageId) -> AppResult<Option<(Option<String>, Option<EditBatchId>)>> {
    Ok(conn
        .query_row(
            "SELECT h.source, h.batch_id FROM adjustments a JOIN adjustment_history h ON h.id = a.history_entry_id
             WHERE a.image_id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

/// Stamps the cursor entry of image `id` with `source` (`None` = keep) and `batch_id`.
fn stamp_cursor(conn: &Connection, id: ImageId, source: Option<&str>, batch_id: Option<EditBatchId>) -> AppResult<()> {
    conn.execute(
        "UPDATE adjustment_history SET source = COALESCE(?2, source), batch_id = ?3
         WHERE id = (SELECT history_entry_id FROM adjustments WHERE image_id = ?1)",
        params![id, source, batch_id],
    )?;
    Ok(())
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
    let mut before_entries = Vec::with_capacity(items.len());
    for it in items {
        befores.push(repo::get_adjustments(&tx, it.image_id)?);
        before_entries.push(cursor_entry(&tx, it.image_id)?);
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
        for ((it, before), entry) in items.iter().zip(&befores).zip(&before_entries) {
            if changed.contains(&it.image_id) {
                let (before_source, before_batch) = match entry {
                    Some((src, b)) => (src.clone(), *b),
                    // No history before the batch: neutral, or settings read from the sidecar.
                    None => (Some(EditSource::Sidecar.as_str().to_owned()), None),
                };
                inner.execute(
                    "INSERT INTO edit_batch_items (batch_id, image_id, scene_id, before_json, after_json,
                                                   before_source, before_batch_id, review_reason)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        id,
                        it.image_id,
                        it.scene_id,
                        serde_json::to_string(before)?,
                        serde_json::to_string(&repo::get_adjustments(&inner, it.image_id)?)?,
                        before_source,
                        before_batch,
                        it.review_reason
                    ],
                )?;
                stamp_cursor(&inner, it.image_id, None, Some(id))?;
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
    type Item = (ImageId, String, String, Option<String>, Option<EditBatchId>);
    let items: Vec<Item> = conn
        .prepare(
            "SELECT image_id, before_json, after_json, before_source, before_batch_id
             FROM edit_batch_items WHERE batch_id = ?1 ORDER BY rowid",
        )?
        .query_map([batch_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
        .collect::<Result<_, _>>()?;
    let mut result = UndoBatchResult::default();
    let mut restore = Vec::new();
    let mut sources = HashMap::new();
    for (id, before, after, before_source, before_batch) in items {
        let after: ParametricAdjustments = serde_json::from_str(&after)?;
        if repo::get_adjustments(conn, id)? == after {
            restore.push((id, serde_json::from_str::<ParametricAdjustments>(&before)?));
            sources.insert(id, (before_source, before_batch));
        } else {
            result.skipped_ids.push(id);
        }
    }
    let undo_label: String = format!("{LABEL_UNDO_PREFIX}{label}").chars().take(100).collect();
    let inner = conn.savepoint()?;
    result.restored_ids = history::commit_batch_in(&inner, &restore, &undo_label)?;
    // The restored settings keep the provenance they had before the batch (v15).
    for id in &result.restored_ids {
        if let Some((src, b)) = sources.get(id) {
            stamp_cursor(&inner, *id, Some(src.as_deref().unwrap_or(EditSource::User.as_str())), *b)?;
        }
    }
    inner.execute("UPDATE edit_batches SET undone_at = ?2 WHERE id = ?1", params![batch_id, now_ms()])?;
    inner.commit()?;
    Ok(result)
}

/// Reason stored for frames whose scene match did not converge and had no notes.
pub const REVIEW_REASON_DEFAULT: &str = "Exposure or white balance did not fully match the representative";

/// One row of the edit-state query.
struct StateRow {
    id: ImageId,
    neutral: Option<bool>,
    label: Option<String>,
    source: Option<String>,
    batch_id: Option<EditBatchId>,
    scene_id: Option<SceneId>,
    review_reason: Option<String>,
    reviewed_at: Option<i64>,
}

impl StateRow {
    fn into_state(self) -> ImageEditState {
        let source = match (self.neutral, &self.label) {
            // No adjustments row, or neutral settings: nothing to attribute.
            (None, _) | (Some(true), _) => EditSource::None,
            // Settings without history (written before v5, or read from a sidecar at import).
            (Some(false), None) => EditSource::Sidecar,
            (Some(false), Some(label)) => {
                self.source.as_deref().and_then(EditSource::parse).unwrap_or_else(|| history::source_for_label(label))
            }
        };
        let batch_id = if source == EditSource::None { None } else { self.batch_id };
        let needs_review = source == EditSource::SceneApply
            && batch_id.is_some()
            && self.review_reason.is_some()
            && self.reviewed_at.is_none();
        ImageEditState {
            image_id: self.id,
            edit_source: source,
            batch_id,
            applied_scene_id: if source == EditSource::SceneApply { self.scene_id } else { None },
            needs_review,
            review_reason: if needs_review { self.review_reason } else { None },
        }
    }
}

const STATE_SQL: &str = "
    SELECT i.id, a.neutral, h.label, h.source, h.batch_id, bi.scene_id, bi.review_reason, bi.reviewed_at
      FROM images i
      LEFT JOIN adjustments a ON a.image_id = i.id
      LEFT JOIN adjustment_history h ON h.id = a.history_entry_id
      LEFT JOIN edit_batch_items bi ON bi.batch_id = h.batch_id AND bi.image_id = i.id";

fn read_state(r: &rusqlite::Row<'_>) -> rusqlite::Result<StateRow> {
    Ok(StateRow {
        id: r.get(0)?,
        neutral: r.get(1)?,
        label: r.get(2)?,
        source: r.get(3)?,
        batch_id: r.get(4)?,
        scene_id: r.get(5)?,
        review_reason: r.get(6)?,
        reviewed_at: r.get(7)?,
    })
}

/// `get_edit_states`: per-photo workflow state of `ids` (given order; duplicates kept).
/// Unknown ids -> `not_found`.
pub fn edit_states(conn: &Connection, ids: &[ImageId]) -> AppResult<Vec<ImageEditState>> {
    let mut by_id: HashMap<ImageId, ImageEditState> = HashMap::with_capacity(ids.len());
    let mut unique: Vec<ImageId> = ids.to_vec();
    unique.sort_unstable();
    unique.dedup();
    for chunk in unique.chunks(500) {
        let ph = vec!["?"; chunk.len()].join(",");
        let mut stmt = conn.prepare(&format!("{STATE_SQL} WHERE i.id IN ({ph})"))?;
        for row in stmt.query_map(params_from_iter(chunk.iter()), read_state)? {
            let st = row?.into_state();
            by_id.insert(st.image_id, st);
        }
    }
    ids.iter().map(|id| by_id.get(id).cloned().ok_or_else(|| AppError::not_found(format!("image {id}")))).collect()
}

/// Per-photo state of every image matching the SQL predicate `where_images` (over alias
/// `i`, e.g. a `FolderScope` predicate on `i.folder_id`).
pub fn edit_states_where(conn: &Connection, where_images: &str) -> AppResult<HashMap<ImageId, ImageEditState>> {
    let mut stmt = conn.prepare(&format!("{STATE_SQL} WHERE {where_images}"))?;
    let rows = stmt.query_map([], read_state)?;
    let mut out = HashMap::new();
    for row in rows {
        let st = row?.into_state();
        out.insert(st.image_id, st);
    }
    Ok(out)
}

/// `mark_reviewed`: clears "needs a look" of `ids` without changing their settings (stored on
/// the apply item that wrote them, so it stays cleared). Images that do not need a look are
/// ignored; unknown ids -> `not_found`. Returns the ids that were cleared.
pub fn mark_reviewed(conn: &Connection, ids: &[ImageId]) -> AppResult<Vec<ImageId>> {
    let states = edit_states(conn, ids)?;
    let now = now_ms();
    let mut cleared = Vec::new();
    for st in states.into_iter().filter(|s| s.needs_review) {
        if cleared.contains(&st.image_id) {
            continue;
        }
        conn.execute(
            "UPDATE edit_batch_items SET reviewed_at = ?3 WHERE batch_id = ?1 AND image_id = ?2",
            params![st.batch_id, st.image_id, now],
        )?;
        cleared.push(st.image_id);
    }
    Ok(cleared)
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
                review_reason: None,
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
        let bad = vec![
            items[0].clone(),
            BatchItem { image_id: 999, adjustments: adj(2.0), scene_id: None, review_reason: None },
        ];
        assert_eq!(
            commit_recorded(&mut conn, &bad, LABEL_STYLE, BatchKind::StylePrediction).unwrap_err().kind,
            ErrorKind::NotFound
        );
        assert_eq!(repo::get_adjustments(&conn, ids[0]).unwrap(), ParametricAdjustments::default());
    }
}
