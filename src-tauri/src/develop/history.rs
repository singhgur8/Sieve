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
//!   and push one entry per image (unchanged images get none; batches never coalesce).
//! - Validate `adj` (`ParametricAdjustments::validate`) and `label` (1..=100 chars) ->
//!   `invalid_argument`. Unknown image -> `not_found`.

use rusqlite::{params, Connection, OptionalExtension};

use crate::db::{now_ms, repo};
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::{
    AdjustmentField, AdjustmentHistory, EditState, HistoryEntry, HistoryEntryId, ImageId, ParametricAdjustments,
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

fn validate_label(label: &str) -> AppResult<()> {
    let n = label.chars().count();
    if n == 0 || n > 100 {
        return Err(AppError::invalid("label must be 1..=100 characters"));
    }
    Ok(())
}

/// Snapshot JSON overlaid on neutral defaults (snapshots written before a slider existed).
fn parse_snapshot(json: &str) -> AppResult<ParametricAdjustments> {
    fn merge(base: &mut serde_json::Value, overlay: serde_json::Value) {
        match (base, overlay) {
            (serde_json::Value::Object(b), serde_json::Value::Object(o)) => {
                for (k, v) in o {
                    match b.get_mut(&k) {
                        Some(slot) => merge(slot, v),
                        None => {
                            b.insert(k, v);
                        }
                    }
                }
            }
            (slot, v) => *slot = v,
        }
    }
    let mut value = serde_json::to_value(ParametricAdjustments::default())?;
    merge(&mut value, serde_json::from_str(json)?);
    Ok(serde_json::from_value(value)?)
}

/// `(cursor)` of the image's adjustments row: `None` = no row, `Some(None)` = row without
/// history.
fn cursor(conn: &Connection, id: ImageId) -> AppResult<Option<Option<HistoryEntryId>>> {
    Ok(conn.query_row("SELECT history_entry_id FROM adjustments WHERE image_id = ?1", [id], |r| r.get(0)).optional()?)
}

fn insert_entry(conn: &Connection, id: ImageId, label: &str, adj: &ParametricAdjustments, now: i64) -> AppResult<i64> {
    conn.execute(
        "INSERT INTO adjustment_history (image_id, label, params_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4)",
        params![id, label, serde_json::to_string(adj)?, now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Writes `adj` as the image's adjustments with the cursor at `entry`.
fn set_current(conn: &Connection, id: ImageId, adj: &ParametricAdjustments, entry: HistoryEntryId) -> AppResult<()> {
    repo::save_adjustments(conn, id, adj)?;
    conn.execute("UPDATE adjustments SET history_entry_id = ?2 WHERE image_id = ?1", params![id, entry])?;
    Ok(())
}

/// Commit logic inside an open transaction. Returns whether anything was written.
fn commit_in(
    conn: &Connection,
    id: ImageId,
    adj: &ParametricAdjustments,
    label: &str,
    coalesce: bool,
) -> AppResult<bool> {
    let current = repo::get_adjustments(conn, id)?; // not_found for unknown images
    if *adj == current {
        return Ok(false);
    }
    let now = now_ms();
    let cur = match cursor(conn, id)?.flatten() {
        Some(c) => c,
        None => insert_entry(conn, id, ORIGINAL_LABEL, &current, now)?,
    };
    let first: HistoryEntryId =
        conn.query_row("SELECT MIN(id) FROM adjustment_history WHERE image_id = ?1", [id], |r| r.get(0))?;
    let newest: HistoryEntryId =
        conn.query_row("SELECT MAX(id) FROM adjustment_history WHERE image_id = ?1", [id], |r| r.get(0))?;
    let (cur_label, cur_updated): (String, i64) =
        conn.query_row("SELECT label, updated_at FROM adjustment_history WHERE id = ?1", [cur], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    let entry = if coalesce
        && cur == newest
        && cur != first
        && cur_label == label
        && (0..=COALESCE_WINDOW_MS).contains(&(now - cur_updated))
    {
        conn.execute(
            "UPDATE adjustment_history SET params_json = ?2, updated_at = ?3 WHERE id = ?1",
            params![cur, serde_json::to_string(adj)?, now],
        )?;
        cur
    } else {
        conn.execute("DELETE FROM adjustment_history WHERE image_id = ?1 AND id > ?2", params![id, cur])?;
        let e = insert_entry(conn, id, label, adj, now)?;
        // Prune the oldest entries after Original.
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM adjustment_history WHERE image_id = ?1", [id], |r| r.get(0))?;
        let excess = count - MAX_ENTRIES as i64;
        if excess > 0 {
            conn.execute(
                "DELETE FROM adjustment_history WHERE id IN (
                     SELECT id FROM adjustment_history WHERE image_id = ?1 AND id > ?2 ORDER BY id LIMIT ?3)",
                params![id, first, excess],
            )?;
        }
        e
    };
    set_current(conn, id, adj, entry)?;
    Ok(true)
}

/// Saves `adj` as the image's adjustments with a history entry labelled `label`.
pub fn commit(
    conn: &mut Connection,
    id: ImageId,
    adj: &ParametricAdjustments,
    label: &str,
) -> AppResult<AdjustmentHistory> {
    adj.validate().map_err(AppError::invalid)?;
    validate_label(label)?;
    let tx = conn.transaction()?;
    commit_in(&tx, id, adj, label, true)?;
    let h = history(&tx, id)?;
    tx.commit()?;
    Ok(h)
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
    if fields.is_empty() {
        return Err(AppError::invalid("fields must not be empty"));
    }
    validate_label(label)?;
    let tx = conn.transaction()?;
    for &id in ids {
        let mut next = repo::get_adjustments(&tx, id)?;
        next.copy_fields(src, fields);
        next.validate().map_err(AppError::invalid)?;
        commit_in(&tx, id, &next, label, false)?;
    }
    tx.commit()?;
    Ok(())
}

/// Moves the cursor to the entry chosen by `pick` (given entries oldest first and the
/// cursor index) and returns the resulting state.
fn move_cursor(
    conn: &mut Connection,
    id: ImageId,
    pick: impl FnOnce(&[HistoryEntry], usize) -> AppResult<Option<HistoryEntryId>>,
) -> AppResult<EditState> {
    let tx = conn.transaction()?;
    let h = history(&tx, id)?;
    if let Some(cur) = h.current_entry_id {
        let idx = h.entries.iter().position(|e| e.id == cur).unwrap_or(0);
        if let Some(target) = pick(&h.entries, idx)? {
            if target != cur {
                let json: String =
                    tx.query_row("SELECT params_json FROM adjustment_history WHERE id = ?1", [target], |r| r.get(0))?;
                let adj = parse_snapshot(&json)?;
                set_current(&tx, id, &adj, target)?;
            }
        }
    } else {
        // Never edited: only goto can target anything, and there is nothing to target.
        pick(&[], 0)?;
    }
    let state = EditState { adjustments: repo::get_adjustments(&tx, id)?, history: history(&tx, id)? };
    tx.commit()?;
    Ok(state)
}

pub fn undo(conn: &mut Connection, id: ImageId) -> AppResult<EditState> {
    move_cursor(conn, id, |entries, i| Ok((i > 0).then(|| entries[i - 1].id)))
}

pub fn redo(conn: &mut Connection, id: ImageId) -> AppResult<EditState> {
    move_cursor(conn, id, |entries, i| Ok(entries.get(i + 1).map(|e| e.id)))
}

/// Moves the cursor to `entry_id` (must belong to `id`, else `not_found`).
pub fn goto(conn: &mut Connection, id: ImageId, entry_id: HistoryEntryId) -> AppResult<EditState> {
    move_cursor(conn, id, |entries, _| {
        entries
            .iter()
            .find(|e| e.id == entry_id)
            .map(|e| Some(e.id))
            .ok_or_else(|| AppError::not_found(format!("history entry {entry_id} of image {id}")))
    })
}

/// Empty history (`currentEntryId = null`) for an image that was never edited.
pub fn history(conn: &Connection, id: ImageId) -> AppResult<AdjustmentHistory> {
    let cur = match cursor(conn, id)? {
        Some(c) => c,
        None => {
            repo::get_image(conn, id)?; // not_found for unknown images
            None
        }
    };
    let mut stmt =
        conn.prepare_cached("SELECT id, label, created_at FROM adjustment_history WHERE image_id = ?1 ORDER BY id")?;
    let entries = stmt
        .query_map([id], |r| Ok(HistoryEntry { id: r.get(0)?, label: r.get(1)?, created_at_ms: r.get(2)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    let idx = cur.and_then(|c| entries.iter().position(|e| e.id == c));
    Ok(AdjustmentHistory {
        image_id: id,
        can_undo: idx.is_some_and(|i| i > 0),
        can_redo: idx.is_some_and(|i| i + 1 < entries.len()),
        current_entry_id: idx.map(|i| entries[i].id),
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::ipc::error::ErrorKind;
    use crate::ipc::types::{LutRef, WhiteBalance};

    fn setup() -> Connection {
        let conn = open_in_memory();
        conn.execute_batch(
            "INSERT INTO folders (id, path, added_at) VALUES (1, '/f', 0);
             INSERT INTO images (id, folder_id, path, file_name, format, camera_make, sensor_layout,
                                 file_size, file_mtime_ms, imported_at)
             VALUES (1, 1, '/f/a.arw', 'a.arw', 'arw', 'sony', 'bayer', 1, 0, 0),
                    (2, 1, '/f/b.arw', 'b.arw', 'arw', 'sony', 'bayer', 1, 0, 0),
                    (3, 1, '/f/c.arw', 'c.arw', 'arw', 'sony', 'bayer', 1, 0, 0);",
        )
        .unwrap();
        conn
    }

    fn exp(v: f32) -> ParametricAdjustments {
        ParametricAdjustments { exposure: v, ..Default::default() }
    }

    fn labels(h: &AdjustmentHistory) -> Vec<&str> {
        h.entries.iter().map(|e| e.label.as_str()).collect()
    }

    /// Makes the newest entry look old so the next same-label save does not coalesce.
    fn age(conn: &Connection) {
        conn.execute("UPDATE adjustment_history SET updated_at = updated_at - 10000", []).unwrap();
    }

    fn dirty(conn: &Connection, id: ImageId) -> bool {
        conn.query_row("SELECT xmp_dirty FROM images WHERE id = ?1", [id], |r| r.get(0)).unwrap()
    }

    #[test]
    fn commit_undo_redo_goto() {
        let mut conn = setup();
        let h = history(&conn, 1).unwrap();
        assert_eq!((h.entries.len(), h.current_entry_id, h.can_undo, h.can_redo), (0, None, false, false));
        assert_eq!(history(&conn, 99).unwrap_err().kind, ErrorKind::NotFound);

        let h = commit(&mut conn, 1, &exp(0.5), "Exposure").unwrap();
        assert_eq!(labels(&h), ["Original", "Exposure"]);
        assert_eq!(h.current_entry_id, Some(h.entries[1].id));
        assert!(h.can_undo && !h.can_redo);
        assert!(dirty(&conn, 1));

        // Same label within the window coalesces.
        let h = commit(&mut conn, 1, &exp(0.7), "Exposure").unwrap();
        assert_eq!(labels(&h), ["Original", "Exposure"]);
        assert_eq!(repo::get_adjustments(&conn, 1).unwrap(), exp(0.7));
        // Unchanged values: no-op.
        assert_eq!(commit(&mut conn, 1, &exp(0.7), "Other").unwrap(), h);
        // Different label pushes.
        let c = ParametricAdjustments { contrast: 20.0, ..exp(0.7) };
        let h = commit(&mut conn, 1, &c, "Contrast").unwrap();
        assert_eq!(labels(&h), ["Original", "Exposure", "Contrast"]);
        // Same label after the window pushes.
        age(&conn);
        let c2 = ParametricAdjustments { contrast: 30.0, ..exp(0.7) };
        let h = commit(&mut conn, 1, &c2, "Contrast").unwrap();
        assert_eq!(labels(&h), ["Original", "Exposure", "Contrast", "Contrast"]);

        let s = undo(&mut conn, 1).unwrap();
        assert_eq!(s.adjustments, c);
        assert!(s.history.can_redo && s.history.can_undo);
        assert_eq!(repo::get_adjustments(&conn, 1).unwrap(), c);
        let s = undo(&mut conn, 1).unwrap();
        assert_eq!(s.adjustments, exp(0.7));
        let s = undo(&mut conn, 1).unwrap();
        assert_eq!(s.adjustments, ParametricAdjustments::default());
        assert!(!s.history.can_undo);
        // Neutral again -> no edits badge, row kept.
        let neutral: bool =
            conn.query_row("SELECT neutral FROM adjustments WHERE image_id = 1", [], |r| r.get(0)).unwrap();
        assert!(neutral);
        let again = undo(&mut conn, 1).unwrap();
        assert_eq!(again, s, "undo at the first entry is a no-op");
        let s = redo(&mut conn, 1).unwrap();
        assert_eq!(s.adjustments, exp(0.7));
        let last = s.history.entries[3].id;
        let s = goto(&mut conn, 1, last).unwrap();
        assert_eq!(s.adjustments, c2);
        assert!(!s.history.can_redo);
        assert_eq!(redo(&mut conn, 1).unwrap(), s, "redo at the last entry is a no-op");

        // Goto the Original then edit: redo tail dropped; the Original entry never coalesces.
        let first = s.history.entries[0].id;
        goto(&mut conn, 1, first).unwrap();
        let h = commit(&mut conn, 1, &exp(-1.0), "Original").unwrap();
        assert_eq!(labels(&h), ["Original", "Original"]);
        assert!(!h.can_redo);

        // Errors.
        assert_eq!(goto(&mut conn, 1, 999_999).unwrap_err().kind, ErrorKind::NotFound);
        let other = commit(&mut conn, 2, &exp(1.0), "Exposure").unwrap();
        assert_eq!(goto(&mut conn, 1, other.entries[1].id).unwrap_err().kind, ErrorKind::NotFound);
        assert_eq!(commit(&mut conn, 99, &exp(1.0), "Exposure").unwrap_err().kind, ErrorKind::NotFound);
        assert_eq!(commit(&mut conn, 1, &exp(9.0), "Exposure").unwrap_err().kind, ErrorKind::InvalidArgument);
        assert_eq!(commit(&mut conn, 1, &exp(1.0), "").unwrap_err().kind, ErrorKind::InvalidArgument);
        assert_eq!(commit(&mut conn, 1, &exp(1.0), &"x".repeat(101)).unwrap_err().kind, ErrorKind::InvalidArgument);
        // Undo on a never-edited image: defaults, empty history.
        let s = undo(&mut conn, 3).unwrap();
        assert_eq!((s.adjustments, s.history.entries.len()), (ParametricAdjustments::default(), 0));
        assert_eq!(undo(&mut conn, 99).unwrap_err().kind, ErrorKind::NotFound);
    }

    #[test]
    fn original_holds_pre_existing_adjustments_and_pruning() {
        let mut conn = setup();
        // A row written before history existed (e.g. Phase 4 / XMP import without history).
        repo::save_adjustments(&conn, 1, &exp(2.0)).unwrap();
        let h = commit(&mut conn, 1, &exp(3.0), "Exposure").unwrap();
        assert_eq!(labels(&h), ["Original", "Exposure"]);
        let s = undo(&mut conn, 1).unwrap();
        assert_eq!(s.adjustments, exp(2.0));
        redo(&mut conn, 1).unwrap();

        for i in 0..(MAX_ENTRIES + 20) {
            commit(&mut conn, 1, &exp((i % 50) as f32 / 20.0 - 1.2), &format!("Step {i}")).unwrap();
        }
        let h = history(&conn, 1).unwrap();
        assert_eq!(h.entries.len(), MAX_ENTRIES);
        assert_eq!(h.entries[0].label, "Original");
        assert_eq!(h.entries.last().unwrap().label, format!("Step {}", MAX_ENTRIES + 19));
        assert_eq!(h.current_entry_id, Some(h.entries.last().unwrap().id));
    }

    #[test]
    fn apply_fields_is_atomic_and_per_image() {
        let mut conn = setup();
        commit(&mut conn, 1, &ParametricAdjustments { shadows: 40.0, ..Default::default() }, "Shadows").unwrap();
        let src = ParametricAdjustments {
            exposure: 1.0,
            shadows: -20.0,
            white_balance: WhiteBalance::Custom { temperature_k: 3200.0, tint: 5.0 },
            lut: Some(LutRef { id: "film".into(), amount: 40.0 }),
            ..Default::default()
        };
        apply_fields(&mut conn, &[1, 2], &src, &[AdjustmentField::Exposure, AdjustmentField::Lut], LABEL_PASTE)
            .unwrap();
        let a1 = repo::get_adjustments(&conn, 1).unwrap();
        assert_eq!((a1.exposure, a1.shadows, a1.white_balance), (1.0, 40.0, WhiteBalance::AsShot));
        assert_eq!(a1.lut, src.lut);
        assert_eq!(labels(&history(&conn, 1).unwrap()), ["Original", "Shadows", "Paste Settings"]);
        assert_eq!(labels(&history(&conn, 2).unwrap()), ["Original", "Paste Settings"]);
        // Batches never coalesce; unchanged images get no entry.
        apply_fields(&mut conn, &[1, 3], &src, &[AdjustmentField::Exposure], LABEL_PASTE).unwrap();
        assert_eq!(history(&conn, 1).unwrap().entries.len(), 3);
        assert_eq!(history(&conn, 3).unwrap().entries.len(), 2);
        // Unknown id: nothing written.
        let err =
            apply_fields(&mut conn, &[2, 99], &ParametricAdjustments::default(), AdjustmentField::ALL, LABEL_RESET);
        assert_eq!(err.unwrap_err().kind, ErrorKind::NotFound);
        assert_eq!(repo::get_adjustments(&conn, 2).unwrap().exposure, 1.0);
        // Reset.
        apply_fields(&mut conn, &[1, 2], &ParametricAdjustments::default(), AdjustmentField::ALL, LABEL_RESET).unwrap();
        assert!(repo::get_adjustments(&conn, 1).unwrap().is_neutral());
        assert_eq!(history(&conn, 2).unwrap().entries.last().unwrap().label, "Reset");
        assert_eq!(apply_fields(&mut conn, &[1], &src, &[], LABEL_PASTE).unwrap_err().kind, ErrorKind::InvalidArgument);
    }
}
