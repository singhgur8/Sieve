//! Catalog SQL used by XMP sync (kept inside the xmp module).

use std::path::PathBuf;

use rusqlite::{params, Connection, OptionalExtension};

use crate::db::now_ms;
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::{ColorLabel, DevelopWarning, FolderId, ImageId, ParametricAdjustments, PickFlag};

/// XMP-relevant catalog state of one image.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageRow {
    pub id: ImageId,
    pub path: PathBuf,
    pub rating: u8,
    pub pick: PickFlag,
    pub color_label: Option<ColorLabel>,
    pub meta_updated_at: Option<i64>,
    pub xmp_mtime_ms: Option<i64>,
}

pub fn load(conn: &Connection, id: ImageId) -> AppResult<Option<ImageRow>> {
    Ok(conn
        .query_row(
            "SELECT id, path, rating, pick, color_label, meta_updated_at, xmp_mtime_ms FROM images WHERE id = ?1",
            [id],
            |r| {
                let pick: String = r.get(3)?;
                let label: Option<String> = r.get(4)?;
                Ok(ImageRow {
                    id: r.get(0)?,
                    path: PathBuf::from(r.get::<_, String>(1)?),
                    rating: r.get::<_, i64>(2)?.clamp(0, 5) as u8,
                    pick: PickFlag::parse(&pick).unwrap_or(PickFlag::Unflagged),
                    color_label: label.as_deref().and_then(ColorLabel::parse),
                    meta_updated_at: r.get(5)?,
                    xmp_mtime_ms: r.get(6)?,
                })
            },
        )
        .optional()?)
}

/// Fails with `not_found` naming the first unknown id.
pub fn ensure_exist(conn: &Connection, ids: &[ImageId]) -> AppResult<()> {
    let mut stmt = conn.prepare_cached("SELECT EXISTS (SELECT 1 FROM images WHERE id = ?1)")?;
    for &id in ids {
        if !stmt.query_row([id], |r| r.get::<_, bool>(0))? {
            return Err(AppError::not_found(format!("image {id}")));
        }
    }
    Ok(())
}

/// Non-suppressed tags, sorted.
pub fn visible_tags(conn: &Connection, id: ImageId) -> AppResult<Vec<String>> {
    let mut stmt =
        conn.prepare_cached("SELECT tag FROM image_tags WHERE image_id = ?1 AND suppressed = 0 ORDER BY tag")?;
    let tags = stmt.query_map([id], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
    Ok(tags)
}

pub fn dirty_ids(conn: &Connection) -> AppResult<Vec<ImageId>> {
    let mut stmt = conn.prepare("SELECT id FROM images WHERE xmp_dirty = 1 ORDER BY id")?;
    let ids = stmt.query_map([], |r| r.get(0))?.collect::<Result<Vec<ImageId>, _>>()?;
    Ok(ids)
}

/// Dirty images of `folder` (every folder for `None`), by id.
pub fn dirty_ids_in(conn: &Connection, folder: Option<FolderId>) -> AppResult<Vec<ImageId>> {
    let mut stmt =
        conn.prepare("SELECT id FROM images WHERE xmp_dirty = 1 AND (?1 IS NULL OR folder_id = ?1) ORDER BY id")?;
    let ids = stmt.query_map([folder], |r| r.get(0))?.collect::<Result<Vec<ImageId>, _>>()?;
    Ok(ids)
}

/// `(id, raw path, xmp_mtime_ms)` of the folder's images without pending catalog changes.
pub fn clean_images_in_folder(
    conn: &Connection,
    folder_id: FolderId,
) -> AppResult<Vec<(ImageId, PathBuf, Option<i64>)>> {
    let mut stmt =
        conn.prepare("SELECT id, path, xmp_mtime_ms FROM images WHERE folder_id = ?1 AND xmp_dirty = 0 ORDER BY id")?;
    let rows = stmt
        .query_map([folder_id], |r| Ok((r.get(0)?, PathBuf::from(r.get::<_, String>(1)?), r.get(2)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// After a successful write of the state snapshotted at `row`. The dirty flag is only
/// cleared if no XMP-mapped change happened since the snapshot (`meta_updated_at`
/// unchanged); otherwise the image stays dirty for the next pass.
pub fn mark_written(conn: &Connection, row: &ImageRow, sidecar_mtime: Option<i64>) -> AppResult<()> {
    conn.execute(
        "UPDATE images
         SET xmp_dirty = CASE WHEN meta_updated_at IS ?2 THEN 0 ELSE xmp_dirty END,
             xmp_synced_at = ?3, xmp_mtime_ms = ?4, xmp_error = NULL
         WHERE id = ?1",
        params![row.id, row.meta_updated_at, now_ms(), sidecar_mtime],
    )?;
    Ok(())
}

/// Applies sidecar values; returns whether rating/pick/label changed. Clears the dirty
/// flag the update triggers re-set.
pub fn apply_read(
    conn: &mut Connection,
    id: ImageId,
    rating: u8,
    pick: PickFlag,
    label: Option<ColorLabel>,
    sidecar_mtime: Option<i64>,
) -> AppResult<bool> {
    let tx = conn.transaction()?;
    let changed = tx.execute(
        "UPDATE images SET rating = ?2, pick = ?3, color_label = ?4
         WHERE id = ?1 AND (rating IS NOT ?2 OR pick IS NOT ?3 OR color_label IS NOT ?4)",
        params![id, rating, pick.as_str(), label.map(ColorLabel::as_str)],
    )? > 0;
    tx.execute(
        "UPDATE images SET xmp_dirty = 0, xmp_synced_at = ?2, xmp_mtime_ms = ?3, xmp_error = NULL WHERE id = ?1",
        params![id, now_ms(), sidecar_mtime],
    )?;
    tx.commit()?;
    Ok(changed)
}

/// Stored develop settings, only for images that have an `adjustments` row (images never
/// edited in Sieve get no `crs:` written).
pub fn develop_settings(conn: &Connection, id: ImageId) -> AppResult<Option<ParametricAdjustments>> {
    let has_row: bool =
        conn.query_row("SELECT EXISTS (SELECT 1 FROM adjustments WHERE image_id = ?1)", [id], |r| r.get(0))?;
    if !has_row {
        return Ok(None);
    }
    Ok(Some(crate::db::repo::get_adjustments(conn, id)?))
}

/// Stores the sidecar's unsupported-feature warnings (`RawImageEntry.developWarnings`);
/// empty clears them. Not XMP-mapped (no dirty trigger).
pub fn set_develop_warnings(conn: &Connection, id: ImageId, warnings: &[DevelopWarning]) -> AppResult<()> {
    let json = if warnings.is_empty() { None } else { Some(serde_json::to_string(warnings)?) };
    conn.execute("UPDATE images SET develop_warnings = ?2 WHERE id = ?1", params![id, json])?;
    Ok(())
}

/// `images.masks_pending_import` (migration 0010): sidecar masks not imported yet.
pub fn masks_pending(conn: &Connection, id: ImageId) -> AppResult<bool> {
    let v: i64 = conn.query_row("SELECT masks_pending_import FROM images WHERE id = ?1", [id], |r| r.get(0))?;
    Ok(v != 0)
}

/// Images whose sidecar masks still await import (launch catch-up), by id.
pub fn masks_pending_ids(conn: &Connection) -> AppResult<Vec<ImageId>> {
    let mut stmt = conn.prepare("SELECT id FROM images WHERE masks_pending_import <> 0 ORDER BY id")?;
    let ids = stmt.query_map([], |r| r.get(0))?.collect::<Result<Vec<ImageId>, _>>()?;
    Ok(ids)
}

/// `images.develop_warnings` (sidecar feature warnings).
pub fn develop_warnings(conn: &Connection, id: ImageId) -> AppResult<Vec<DevelopWarning>> {
    let json: Option<String> =
        conn.query_row("SELECT develop_warnings FROM images WHERE id = ?1", [id], |r| r.get(0))?;
    Ok(json.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default())
}

/// `images.xmp_dirty`.
pub fn is_dirty(conn: &Connection, id: ImageId) -> AppResult<bool> {
    Ok(conn.query_row("SELECT xmp_dirty FROM images WHERE id = ?1", [id], |r| r.get::<_, i64>(0))? != 0)
}

/// Clears `xmp_dirty` (the catalog equals the sidecar again).
pub fn clear_dirty(conn: &Connection, id: ImageId) -> AppResult<()> {
    conn.execute("UPDATE images SET xmp_dirty = 0 WHERE id = ?1", [id])?;
    Ok(())
}

/// The sidecar's masks were imported.
pub fn clear_masks_pending(conn: &Connection, id: ImageId) -> AppResult<()> {
    conn.execute("UPDATE images SET masks_pending_import = 0 WHERE id = ?1 AND masks_pending_import <> 0", [id])?;
    Ok(())
}

pub fn mark_failed(conn: &Connection, id: ImageId, reason: &str) -> AppResult<()> {
    conn.execute("UPDATE images SET xmp_error = ?2 WHERE id = ?1", params![id, reason])?;
    Ok(())
}
