//! Edit history + adjustment writes (catalog SQL, migration 0005). Every command-path write
//! of `adjustments` goes through here so history, `neutral` and the history cursor stay
//! consistent (`db::repo::save_adjustments` is the history-less primitive).
//!
//! Model: `adjustment_history` holds full snapshots per image, oldest first by id.
//! `adjustments.history_entry_id` is the cursor; `adjustments.params_json` always equals the
//! cursor entry's snapshot. The XMP dirty flag is set by the 0005 triggers.
//!
//! Rules:
//! - First write for an image: insert an [`ORIGINAL_LABEL`] entry holding the state before the
//!   edit (the stored adjustments, or neutral defaults if none), then the new entry.
//! - `commit`: no-op (returns the history unchanged) if `adj` equals the current snapshot.
//!   Coalesce: if the cursor is the newest entry, its label equals `label`, it is not the
//!   Original entry, and its `updated_at` is within [`COALESCE_WINDOW_MS`], replace that
//!   snapshot (and bump `updated_at`) instead of pushing. Otherwise delete the redo tail
//!   (entries after the cursor) and push.
//! - Keep at most [`MAX_ENTRIES`] per image: prune the oldest entries after Original.
//! - Undo at the first entry / redo at the last are no-ops (state returned unchanged).
//! - Batch functions are atomic (one transaction; unknown id -> `not_found`, nothing written)
//!   and push one entry per image (unchanged images get none).
//! - Validate `adj` (`ParametricAdjustments::validate`) and `label` (1..=100 chars) ->
//!   `invalid_argument`. Unknown image -> `not_found`.

use rusqlite::Connection;

use crate::ipc::error::AppResult;
use crate::ipc::types::{
    AdjustmentField, AdjustmentHistory, EditState, HistoryEntryId, ImageId, ParametricAdjustments,
};

pub const ORIGINAL_LABEL: &str = "Original";
/// Consecutive saves with the same label within this window merge into one entry
/// (keyboard nudges, debounced saves during a drag).
pub const COALESCE_WINDOW_MS: i64 = 1500;
pub const MAX_ENTRIES: usize = 200;

/// Labels used by backend batch operations.
pub const LABEL_PASTE: &str = "Paste Settings";
pub const LABEL_SYNC: &str = "Sync Settings";
pub const LABEL_RESET: &str = "Reset";
pub const LABEL_READ_XMP: &str = "Read from XMP";
/// Preset label: `format!("{LABEL_PRESET_PREFIX}{name}")`.
pub const LABEL_PRESET_PREFIX: &str = "Preset: ";

/// Saves `adj` as the image's adjustments with a history entry labelled `label`.
pub fn commit(
    conn: &mut Connection,
    id: ImageId,
    adj: &ParametricAdjustments,
    label: &str,
) -> AppResult<AdjustmentHistory> {
    let _ = (conn, id, adj, label);
    todo!("rust-engine-dev: develop::history::commit")
}

/// For each of `ids`: current adjustments with the `fields` groups copied from `src`
/// (`ParametricAdjustments::copy_fields`), committed with `label`. Paste, preset, reset
/// (`src` = defaults, all fields) and sync use this. `fields` must be non-empty.
pub fn apply_fields(
    conn: &mut Connection,
    ids: &[ImageId],
    src: &ParametricAdjustments,
    fields: &[AdjustmentField],
    label: &str,
) -> AppResult<()> {
    let _ = (conn, ids, src, fields, label);
    todo!("rust-engine-dev: develop::history::apply_fields")
}

pub fn undo(conn: &mut Connection, id: ImageId) -> AppResult<EditState> {
    let _ = (conn, id);
    todo!("rust-engine-dev: develop::history::undo")
}

pub fn redo(conn: &mut Connection, id: ImageId) -> AppResult<EditState> {
    let _ = (conn, id);
    todo!("rust-engine-dev: develop::history::redo")
}

/// Moves the cursor to `entry_id` (must belong to `id`, else `not_found`).
pub fn goto(conn: &mut Connection, id: ImageId, entry_id: HistoryEntryId) -> AppResult<EditState> {
    let _ = (conn, id, entry_id);
    todo!("rust-engine-dev: develop::history::goto")
}

/// Empty history (`currentEntryId = null`) for an image that was never edited.
pub fn history(conn: &Connection, id: ImageId) -> AppResult<AdjustmentHistory> {
    let _ = (conn, id);
    todo!("rust-engine-dev: develop::history::history")
}
