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
//!
//! Linear undo (IPC v16): [`undo`] refuses with `conflict` while any image of the batch has
//! a history entry newer than the batch's own entry for it ([`conflict_ids`]); undoing a
//! batch also clears the applied state of the scenes whose last apply it was.
//!
//! Applies built on a batch (IPC v17, migration 0015): a scene apply made from a
//! representative whose current settings were written by batch A records A as its base
//! (`edit_batch_bases`, [`commit_recorded_with_bases`], [`base_of`]); while that apply is not
//! undone, the representative counts as a conflict of A ([`dependent_ids`]).

use std::collections::HashMap;

use rusqlite::{params, params_from_iter, Connection, OptionalExtension};

use super::history;
use crate::db::{now_ms, repo};
use crate::ipc::error::{AppError, AppResult, ErrorKind};
use crate::ipc::types::{
    EditBatchId, EditBatchInfo, EditBatchResult, EditSource, ImageEditState, ImageId, ParametricAdjustments, SceneId,
    UndoBatchResult,
};

/// `edit_batches.kind` (the IPC enum since v16).
pub use crate::ipc::types::EditBatchKind as BatchKind;

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
    commit_recorded_with_bases(conn, items, label, kind, &[])
}

/// Paste / Sync / Paste from Previous as one undoable batch (IPC v19, kind `paste`): copies
/// the `fields` groups of `src` onto every image of `ids` (duplicates ignored, first
/// occurrence kept), one `label` history entry per changed image. Atomic like
/// [`commit_recorded`]; empty `fields` -> `invalid_argument`.
pub fn apply_fields_recorded(
    conn: &mut Connection,
    ids: &[ImageId],
    src: &ParametricAdjustments,
    fields: &[crate::ipc::types::AdjustmentField],
    label: &str,
) -> AppResult<EditBatchResult> {
    if fields.is_empty() {
        return Err(AppError::invalid("fields must not be empty"));
    }
    let mut items: Vec<BatchItem> = Vec::with_capacity(ids.len());
    for &id in ids {
        if items.iter().any(|it| it.image_id == id) {
            continue;
        }
        let mut next = repo::get_adjustments(conn, id)?;
        next.copy_fields(src, fields);
        items.push(BatchItem { image_id: id, adjustments: next, scene_id: None, review_reason: None });
    }
    commit_recorded(conn, &items, label, BatchKind::Paste)
}

/// What a new batch was made from (v17): image `image_id`'s settings, written by batch
/// `base_batch_id` (a scene apply from its representative). Recorded only when one of
/// `for_ids` (the items made from it) changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchBase {
    pub image_id: ImageId,
    pub base_batch_id: EditBatchId,
    pub for_ids: Vec<ImageId>,
}

/// [`commit_recorded`], also recording `bases` (v17, `edit_batch_bases`). Unknown base batch
/// -> `not_found` (nothing written).
pub fn commit_recorded_with_bases(
    conn: &mut Connection,
    items: &[BatchItem],
    label: &str,
    kind: BatchKind,
    bases: &[BatchBase],
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
        for b in bases.iter().filter(|b| b.for_ids.iter().any(|i| changed.contains(i))) {
            inner
                .query_row("SELECT 1 FROM edit_batches WHERE id = ?1", [b.base_batch_id], |_| Ok(()))
                .optional()?
                .ok_or_else(|| AppError::not_found(format!("edit batch {}", b.base_batch_id)))?;
            inner.execute(
                "INSERT OR REPLACE INTO edit_batch_bases (batch_id, image_id, base_batch_id) VALUES (?1, ?2, ?3)",
                params![id, b.image_id, b.base_batch_id],
            )?;
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

/// The batch that wrote image `image_id`'s current settings (its history cursor's batch, v17),
/// if that batch is not undone: what an apply from this image as representative is built on.
pub fn base_of(conn: &Connection, image_id: ImageId) -> AppResult<Option<EditBatchId>> {
    let Some((_, Some(batch))) = cursor_entry(conn, image_id)? else { return Ok(None) };
    let undone: Option<Option<i64>> =
        conn.query_row("SELECT undone_at FROM edit_batches WHERE id = ?1", [batch], |r| r.get(0)).optional()?;
    Ok(match undone {
        Some(None) => Some(batch),
        _ => None,
    })
}

/// Images of batch `batch_id` (item order) whose settings from it a later scene apply that
/// is not undone was made from (v17; the representatives of those applies).
pub fn dependent_ids(conn: &Connection, batch_id: EditBatchId) -> AppResult<Vec<ImageId>> {
    Ok(conn
        .prepare_cached(
            "SELECT bi.image_id FROM edit_batch_items bi
             WHERE bi.batch_id = ?1 AND EXISTS (
                 SELECT 1 FROM edit_batch_bases bb JOIN edit_batches b ON b.id = bb.batch_id
                 WHERE bb.base_batch_id = ?1 AND bb.image_id = bi.image_id AND b.undone_at IS NULL)
             ORDER BY bi.rowid",
        )?
        .query_map([batch_id], |r| r.get(0))?
        .collect::<Result<_, _>>()?)
}

/// Every conflict of batch `batch_id` (v17, item order): [`edited_after_ids`] and
/// [`dependent_ids`]. `undo` refuses while non-empty.
pub fn conflict_ids(conn: &Connection, batch_id: EditBatchId) -> AppResult<Vec<ImageId>> {
    let edited = edited_after_ids(conn, batch_id)?;
    let dependent = dependent_ids(conn, batch_id)?;
    if dependent.is_empty() {
        return Ok(edited);
    }
    let order: Vec<ImageId> = conn
        .prepare_cached("SELECT image_id FROM edit_batch_items WHERE batch_id = ?1 ORDER BY rowid")?
        .query_map([batch_id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(order.into_iter().filter(|id| edited.contains(id) || dependent.contains(id)).collect())
}

/// Images of batch `batch_id` that were edited after it (v16), in item order: the image's
/// history cursor is newer than the batch's entry for it, and is not an entry that restored
/// this batch's settings (undo of a later batch is stamped with this batch). Images whose
/// batch entry is gone (pruned, or dropped as redo tail by a newer edit) count when their
/// settings differ from what the batch wrote. Images whose cursor is before the batch's
/// entry (the batch edit was taken back with per-image undo) do not count.
pub fn edited_after_ids(conn: &Connection, batch_id: EditBatchId) -> AppResult<Vec<ImageId>> {
    type Row = (ImageId, String, Option<i64>, Option<EditBatchId>, Option<i64>);
    let rows: Vec<Row> = conn
        .prepare_cached(
            "SELECT bi.image_id, bi.after_json, a.history_entry_id, c.batch_id,
                    (SELECT MIN(h.id) FROM adjustment_history h WHERE h.image_id = bi.image_id AND h.batch_id = ?1)
             FROM edit_batch_items bi
             LEFT JOIN adjustments a ON a.image_id = bi.image_id
             LEFT JOIN adjustment_history c ON c.id = a.history_entry_id
             WHERE bi.batch_id = ?1 ORDER BY bi.rowid",
        )?
        .query_map([batch_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
        .collect::<Result<_, _>>()?;
    let mut out = Vec::new();
    for (id, after, cursor, cursor_batch, entry) in rows {
        let Some(cursor) = cursor else { continue };
        if cursor_batch == Some(batch_id) {
            continue;
        }
        let conflict = match entry {
            Some(e) => cursor > e,
            None => repo::get_adjustments(conn, id)? != serde_json::from_str::<ParametricAdjustments>(&after)?,
        };
        if conflict {
            out.push(id);
        }
    }
    Ok(out)
}

/// `EditBatchInfo` of batch `batch_id` (unknown -> `not_found`).
pub fn batch_info(conn: &Connection, batch_id: EditBatchId) -> AppResult<EditBatchInfo> {
    let row: Option<(String, String, i64, Option<i64>, u32)> = conn
        .query_row(
            "SELECT label, kind, created_at, undone_at, (SELECT COUNT(*) FROM edit_batch_items WHERE batch_id = b.id)
             FROM edit_batches b WHERE id = ?1",
            [batch_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let (label, kind, created_at_ms, undone_at_ms, image_count) =
        row.ok_or_else(|| AppError::not_found(format!("edit batch {batch_id}")))?;
    let kind = BatchKind::parse(&kind).ok_or_else(|| AppError::internal(format!("unknown edit batch kind {kind}")))?;
    let conflict_count = if undone_at_ms.is_some() { 0 } else { conflict_ids(conn, batch_id)?.len() as u32 };
    Ok(EditBatchInfo {
        batch_id,
        label,
        kind,
        created_at_ms,
        undone_at_ms,
        image_count,
        conflict_count,
        undoable: undone_at_ms.is_none() && conflict_count == 0,
    })
}

/// `get_edit_batches`: [`batch_info`] of each id (given order). Unknown id -> `not_found`.
pub fn batch_infos(conn: &Connection, ids: &[EditBatchId]) -> AppResult<Vec<EditBatchInfo>> {
    ids.iter().map(|&id| batch_info(conn, id)).collect()
}

/// The newest batch that is not undone and changed an image matching the SQL predicate
/// `where_images` (over alias `i`), as `EditBatchInfo`.
pub fn latest_batch_where(conn: &Connection, where_images: &str) -> AppResult<Option<EditBatchInfo>> {
    let id: Option<EditBatchId> = conn
        .query_row(
            &format!(
                "SELECT b.id FROM edit_batches b WHERE b.undone_at IS NULL AND EXISTS (
                     SELECT 1 FROM edit_batch_items bi JOIN images i ON i.id = bi.image_id
                     WHERE bi.batch_id = b.id AND {where_images})
                 ORDER BY b.id DESC LIMIT 1"
            ),
            [],
            |r| r.get(0),
        )
        .optional()?;
    id.map(|id| batch_info(conn, id)).transpose()
}

/// User-facing message of a `conflict` undo.
pub fn conflict_message(n: usize) -> String {
    if n == 1 {
        "Later edits on 1 photo; undo those first".to_owned()
    } else {
        format!("Later edits on {n} photos; undo those first")
    }
}

/// User-facing message of a `conflict` undo whose only conflicts are `n` scene applies built
/// on the batch (v17).
pub fn applied_from_message(n: usize) -> String {
    if n == 1 {
        "A scene was applied from this edit since; undo that apply first".to_owned()
    } else {
        format!("{n} scenes were applied from this edit since; undo those applies first")
    }
}

/// Undoes batch `batch_id` (see the module docs). Unknown batch -> `not_found`; already undone
/// -> `invalid_argument`; photos edited after the batch -> `conflict` (nothing changed).
/// Scenes whose last apply was this batch lose their applied state (status back to
/// `edited`; apply again to re-apply). Atomic.
pub fn undo(conn: &mut Connection, batch_id: EditBatchId) -> AppResult<UndoBatchResult> {
    undo_with(conn, batch_id, false)
}

/// [`undo`], or with `keep_later_edits` (v21.1, "Undo the rest"): photos edited after the
/// batch and representatives of applies built on it ([`conflict_ids`]) are not a conflict but
/// left alone (`kept_ids`, their items stamped `kept_at`); the rest is restored as usual and
/// the batch reads undone. Applies built on the batch stay as they are.
pub fn undo_with(conn: &mut Connection, batch_id: EditBatchId, keep_later_edits: bool) -> AppResult<UndoBatchResult> {
    let row: Option<(String, Option<i64>)> = conn
        .query_row("SELECT label, undone_at FROM edit_batches WHERE id = ?1", [batch_id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?;
    let (label, undone) = row.ok_or_else(|| AppError::not_found(format!("edit batch {batch_id}")))?;
    if undone.is_some() {
        return Err(AppError::invalid("this edit was already undone"));
    }
    let kept = if keep_later_edits {
        conflict_ids(conn, batch_id)?
    } else {
        let edited = edited_after_ids(conn, batch_id)?;
        if !edited.is_empty() {
            let n = conflict_ids(conn, batch_id)?.len();
            return Err(AppError::new(ErrorKind::Conflict, conflict_message(n)));
        }
        let dependent = dependent_ids(conn, batch_id)?;
        if !dependent.is_empty() {
            return Err(AppError::new(ErrorKind::Conflict, applied_from_message(dependent.len())));
        }
        Vec::new()
    };
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
        if kept.contains(&id) {
            result.kept_ids.push(id);
            continue;
        }
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
    let now = now_ms();
    for id in &result.kept_ids {
        inner.execute(
            "UPDATE edit_batch_items SET kept_at = ?3 WHERE batch_id = ?1 AND image_id = ?2",
            params![batch_id, id, now],
        )?;
    }
    inner.execute("UPDATE edit_batches SET undone_at = ?2 WHERE id = ?1", params![batch_id, now])?;
    inner.execute(
        "UPDATE scenes SET applied_at_ms = NULL, applied_params_json = NULL, applied_batch_id = NULL,
                           applied_covered_json = NULL
         WHERE applied_batch_id = ?1",
        [batch_id],
    )?;
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
        // Scene applies that did not converge, and photos a baseline run flagged (v21).
        let needs_review = matches!(source, EditSource::SceneApply | EditSource::Baseline)
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

        // The user touches one image after the batch: linear undo refuses, nothing changes.
        history::commit(&mut conn, ids[1], &adj(1.5), "Exposure").unwrap();
        let e = undo(&mut conn, batch).unwrap_err();
        assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Conflict, "Later edits on 1 photo; undo those first"));
        assert_eq!(repo::get_adjustments(&conn, ids[0]).unwrap(), adj(1.0));
        let info = batch_info(&conn, batch).unwrap();
        assert_eq!((info.image_count, info.conflict_count, info.undoable), (2, 1, false));
        assert_eq!(info.kind, BatchKind::SceneApply);
        // Per-image undo of that edit makes the batch undoable again; one more per-image undo
        // takes the batch's own edit back, and the batch undo then leaves the image alone.
        history::undo(&mut conn, ids[1]).unwrap();
        assert!(batch_info(&conn, batch).unwrap().undoable);
        history::undo(&mut conn, ids[1]).unwrap();
        assert!(batch_info(&conn, batch).unwrap().undoable);
        let u = undo(&mut conn, batch).unwrap();
        assert_eq!(u, UndoBatchResult { restored_ids: vec![ids[0]], skipped_ids: vec![ids[1]], kept_ids: vec![] });
        assert_eq!(repo::get_adjustments(&conn, ids[0]).unwrap(), ParametricAdjustments::default());
        assert_eq!(repo::get_adjustments(&conn, ids[1]).unwrap(), ParametricAdjustments::default());
        let info = batch_info(&conn, batch).unwrap();
        assert!(info.undone_at_ms.is_some() && !info.undoable && info.conflict_count == 0);
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

    fn image_ids(conn: &Connection) -> Vec<ImageId> {
        conn.prepare("SELECT id FROM images ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn items(ids: &[ImageId], ev: f32) -> Vec<BatchItem> {
        ids.iter()
            .map(|&id| BatchItem { image_id: id, adjustments: adj(ev), scene_id: None, review_reason: None })
            .collect()
    }

    #[test]
    fn undo_is_linear_across_batches() {
        let mut conn = fixture();
        let ids = image_ids(&conn);
        // A: auto edit of all three; B: an apply over two of them, built on A.
        let a = commit_recorded(&mut conn, &items(&ids, 0.5), LABEL_STYLE, BatchKind::StylePrediction)
            .unwrap()
            .batch_id
            .unwrap();
        let b = commit_recorded(&mut conn, &items(&ids[..2], 0.9), LABEL_APPLY_SCENE, BatchKind::SceneApply)
            .unwrap()
            .batch_id
            .unwrap();
        assert_eq!(latest_batch_where(&conn, "1").unwrap().map(|i| i.batch_id), Some(b));
        // Undoing the older batch first is refused (B was built on it), nothing changes.
        let e = undo(&mut conn, a).unwrap_err();
        assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Conflict, "Later edits on 2 photos; undo those first"));
        assert_eq!(conflict_ids(&conn, a).unwrap(), vec![ids[0], ids[1]]);
        assert_eq!(repo::get_adjustments(&conn, ids[2]).unwrap(), adj(0.5));
        let infos = batch_infos(&conn, &[a, b]).unwrap();
        assert_eq!(
            infos.iter().map(|i| (i.undoable, i.conflict_count)).collect::<Vec<_>>(),
            vec![(false, 2), (true, 0)]
        );
        assert_eq!(batch_infos(&conn, &[a, 999]).unwrap_err().kind, ErrorKind::NotFound);
        // Undo B: the images are back on A's settings (stamped A), so A is undoable again.
        assert_eq!(undo(&mut conn, b).unwrap().restored_ids, vec![ids[0], ids[1]]);
        assert_eq!(latest_batch_where(&conn, "1").unwrap().map(|i| i.batch_id), Some(a));
        assert!(batch_info(&conn, a).unwrap().undoable);
        let u = undo(&mut conn, a).unwrap();
        assert_eq!(u.restored_ids, ids);
        assert!(u.skipped_ids.is_empty());
        assert_eq!(latest_batch_where(&conn, "1").unwrap(), None);
    }

    #[test]
    fn undo_keeping_later_edits_restores_the_rest() {
        let mut conn = fixture();
        let ids = image_ids(&conn);
        let a = commit_recorded(&mut conn, &items(&ids, 0.5), LABEL_STYLE, BatchKind::StylePrediction)
            .unwrap()
            .batch_id
            .unwrap();
        // The user changes one photo after the batch: linear undo refuses, nothing changes.
        history::commit(&mut conn, ids[1], &adj(1.2), "Exposure").unwrap();
        assert_eq!(undo(&mut conn, a).unwrap_err().kind, ErrorKind::Conflict);
        let info = batch_info(&conn, a).unwrap();
        assert_eq!((info.undoable, info.conflict_count), (false, 1));
        // "Undo the rest": the other two go back, the edited one keeps the user's edit.
        let u = undo_with(&mut conn, a, true).unwrap();
        assert_eq!(
            u,
            UndoBatchResult { restored_ids: vec![ids[0], ids[2]], skipped_ids: vec![], kept_ids: vec![ids[1]] }
        );
        assert_eq!(repo::get_adjustments(&conn, ids[0]).unwrap(), ParametricAdjustments::default());
        assert_eq!(repo::get_adjustments(&conn, ids[1]).unwrap(), adj(1.2));
        let kept: Option<i64> = conn
            .query_row("SELECT kept_at FROM edit_batch_items WHERE batch_id = ?1 AND image_id = ?2", [a, ids[1]], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(kept.is_some());
        let info = batch_info(&conn, a).unwrap();
        assert!(info.undone_at_ms.is_some() && !info.undoable);
        assert_eq!(undo_with(&mut conn, a, true).unwrap_err().kind, ErrorKind::InvalidArgument);
        // Without later edits, keepLaterEdits is a plain undo.
        let b = commit_recorded(&mut conn, &items(&ids[..1], 0.7), LABEL_STYLE, BatchKind::StylePrediction)
            .unwrap()
            .batch_id
            .unwrap();
        let u = undo_with(&mut conn, b, true).unwrap();
        assert_eq!((u.restored_ids, u.kept_ids), (vec![ids[0]], vec![]));
    }

    #[test]
    fn edit_after_dropped_batch_entry_conflicts() {
        let mut conn = fixture();
        let ids = image_ids(&conn);
        let a = commit_recorded(&mut conn, &items(&ids[..1], 0.5), LABEL_STYLE, BatchKind::StylePrediction)
            .unwrap()
            .batch_id
            .unwrap();
        // Per-image undo of the batch edit, then a new edit drops the batch entry (redo tail).
        history::undo(&mut conn, ids[0]).unwrap();
        assert!(batch_info(&conn, a).unwrap().undoable, "taken back per image: not a conflict");
        history::commit(&mut conn, ids[0], &adj(-0.4), "Exposure").unwrap();
        assert_eq!(undo(&mut conn, a).unwrap_err().kind, ErrorKind::Conflict);
        // An edit that happens to land on the batch's settings again is not a conflict.
        history::commit(&mut conn, ids[0], &adj(0.5), "Exposure").unwrap();
        assert!(batch_info(&conn, a).unwrap().undoable);
    }

    #[test]
    fn paste_batch_is_one_undoable_batch() {
        let mut conn = fixture();
        let ids = image_ids(&conn);
        history::commit(&mut conn, ids[0], &adj(0.7), "Exposure").unwrap();
        let src = ParametricAdjustments { exposure: 1.5, contrast: 20.0, ..ParametricAdjustments::default() };
        let fields = [crate::ipc::types::AdjustmentField::Exposure];
        // Duplicates are ignored; contrast is not in `fields`.
        let dup = [ids[0], ids[1], ids[0], ids[2]];
        let r = apply_fields_recorded(&mut conn, &dup, &src, &fields, history::LABEL_PASTE).unwrap();
        assert_eq!(r.changed_ids, ids);
        let batch = r.batch_id.unwrap();
        let info = batch_info(&conn, batch).unwrap();
        assert_eq!((info.kind, info.image_count, info.undoable), (BatchKind::Paste, 3, true));
        let a = repo::get_adjustments(&conn, ids[1]).unwrap();
        assert_eq!((a.exposure, a.contrast), (1.5, 0.0));
        assert_eq!(undo(&mut conn, batch).unwrap().restored_ids, ids);
        assert_eq!(repo::get_adjustments(&conn, ids[0]).unwrap().exposure, 0.7);
        assert!(repo::get_adjustments(&conn, ids[1]).unwrap().is_neutral());
        // Nothing changed -> no batch.
        let r = apply_fields_recorded(&mut conn, &ids[..1], &adj(0.7), &fields, history::LABEL_PASTE).unwrap();
        assert_eq!((r.batch_id, r.changed_ids.len()), (None, 0));
        let e = apply_fields_recorded(&mut conn, &ids, &src, &[], history::LABEL_PASTE).unwrap_err();
        assert_eq!(e.kind, ErrorKind::InvalidArgument);
    }
}
