//! Capture time correction (IPC v19, migration 0017) and the per-photo metadata panel.
//!
//! `images.captured_at_ms` is the corrected time every query uses; `exif_captured_at_ms` is
//! the file's own time; `capture_time_source` says which one `captured_at_ms` is (`exif`,
//! `sidecar`, `user`). Corrections mark the sidecar dirty (`xmp_dirty`, `meta_updated_at`) so
//! the XMP writer puts the corrected time in `exif:DateTimeOriginal` /
//! `photoshop:DateCreated` (rust-engine-dev) and refresh the bounds of the scenes the photos
//! belong to. Burst regrouping is the caller's job (`edit_capture_time` kicks a rescore).
//! Implemented by the architect; rust-engine-dev owns it from here.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::db::{now_ms, repo};
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::{
    CaptureTimeEdit, CaptureTimeEditResult, CaptureTimeSnapshot, CaptureTimeSource, ImageId, ImageMetadata,
    MAX_CAPTURE_TIME_MS, MIN_CAPTURE_TIME_MS,
};

/// `(captured_at_ms, exif_captured_at_ms, capture_time_source)` of image `id`; unknown ->
/// `not_found`.
fn times_of(conn: &Connection, id: ImageId) -> AppResult<(Option<i64>, Option<i64>, CaptureTimeSource)> {
    conn.prepare_cached("SELECT captured_at_ms, exif_captured_at_ms, capture_time_source FROM images WHERE id = ?1")?
        .query_row([id], |r| {
            let src: String = r.get(2)?;
            Ok((r.get(0)?, r.get(1)?, CaptureTimeSource::parse(&src).unwrap_or_default()))
        })
        .optional()?
        .ok_or_else(|| AppError::not_found(format!("image {id}")))
}

fn check_range(ms: i64) -> AppResult<()> {
    if (MIN_CAPTURE_TIME_MS..=MAX_CAPTURE_TIME_MS).contains(&ms) {
        Ok(())
    } else {
        Err(AppError::invalid("the capture time would fall outside the years 1900..2200"))
    }
}

/// Writes one photo's corrected time; marks the sidecar dirty when it changed. Returns
/// whether it changed.
fn write_time(conn: &Connection, id: ImageId, ms: Option<i64>, source: CaptureTimeSource) -> AppResult<bool> {
    let n = conn.execute(
        "UPDATE images SET captured_at_ms = ?2, capture_time_source = ?3, xmp_dirty = 1, meta_updated_at = ?4
         WHERE id = ?1 AND (captured_at_ms IS NOT ?2 OR capture_time_source IS NOT ?3)",
        params![id, ms, source.as_str(), now_ms()],
    )?;
    Ok(n > 0)
}

/// Recomputes `started_at_ms` / `ended_at_ms` of the scenes holding any of `ids`.
fn refresh_scene_bounds(conn: &Connection, ids: &[ImageId]) -> AppResult<()> {
    let mut stmt = conn.prepare_cached(
        "UPDATE scenes SET
             started_at_ms = (SELECT MIN(captured_at_ms) FROM images WHERE scene_id = scenes.id),
             ended_at_ms = (SELECT MAX(captured_at_ms) FROM images WHERE scene_id = scenes.id)
         WHERE id = (SELECT scene_id FROM images WHERE id = ?1)",
    )?;
    for &id in ids {
        stmt.execute([id])?;
    }
    Ok(())
}

/// `edit_capture_time`. Atomic: unknown id -> `not_found`; a duplicate id, a `set_exact`
/// reference outside `ids`, a `sync_cameras` frame without a capture time, or a result outside
/// [`MIN_CAPTURE_TIME_MS`]..=[`MAX_CAPTURE_TIME_MS`] -> `invalid_argument`; nothing written.
pub fn edit(conn: &mut Connection, ids: &[ImageId], mode: &CaptureTimeEdit) -> AppResult<CaptureTimeEditResult> {
    for (i, id) in ids.iter().enumerate() {
        if ids[..i].contains(id) {
            return Err(AppError::invalid(format!("image {id} listed twice")));
        }
    }
    let tx = conn.savepoint()?;
    let mut current = Vec::with_capacity(ids.len());
    for &id in ids {
        current.push((id, times_of(&tx, id)?));
    }
    // Per photo: the new (time, source), or None to skip it.
    let offset_ms: Option<i64> = match mode {
        CaptureTimeEdit::Shift { offset_ms } => Some(*offset_ms),
        CaptureTimeEdit::SetExact { reference_id, captured_at_ms } => {
            check_range(*captured_at_ms)?;
            let (_, (cur, _, _)) = current
                .iter()
                .find(|(id, _)| id == reference_id)
                .ok_or_else(|| AppError::invalid("the reference photo must be one of the selected photos"))?;
            cur.map(|c| captured_at_ms - c)
        }
        CaptureTimeEdit::SyncCameras { reference_id, target_id } => {
            let (r, _, _) = times_of(&tx, *reference_id)?;
            let (t, _, _) = times_of(&tx, *target_id)?;
            match (r, t) {
                (Some(r), Some(t)) => Some(r - t),
                _ => return Err(AppError::invalid("both photos used to sync the cameras need a capture time")),
            }
        }
        CaptureTimeEdit::Revert => None,
    };
    let mut changed_ids = Vec::new();
    let mut skipped_ids = Vec::new();
    let mut previous = Vec::new();
    for (id, (cur, exif, source)) in current {
        let target: Option<(Option<i64>, CaptureTimeSource)> = match mode {
            CaptureTimeEdit::Revert => Some((exif, CaptureTimeSource::Exif)),
            CaptureTimeEdit::SetExact { reference_id, captured_at_ms } if *reference_id == id && cur.is_none() => {
                Some((Some(*captured_at_ms), CaptureTimeSource::User))
            }
            _ => match (cur, offset_ms) {
                (Some(c), Some(off)) => {
                    let next = c.checked_add(off).ok_or_else(|| AppError::invalid("capture time offset too large"))?;
                    check_range(next)?;
                    Some((Some(next), CaptureTimeSource::User))
                }
                _ => None,
            },
        };
        match target {
            Some((ms, src)) if write_time(&tx, id, ms, src)? => {
                changed_ids.push(id);
                previous.push(CaptureTimeSnapshot { image_id: id, captured_at_ms: cur, source });
            }
            _ => skipped_ids.push(id),
        }
    }
    refresh_scene_bounds(&tx, &changed_ids)?;
    tx.commit()?;
    Ok(CaptureTimeEditResult { changed_ids, skipped_ids, offset_ms, previous })
}

/// `restore_capture_times` (undo / redo of `edit_capture_time`). Atomic: unknown id ->
/// `not_found`. Returns the ids whose time changed.
pub fn restore(conn: &mut Connection, snapshots: &[CaptureTimeSnapshot]) -> AppResult<Vec<ImageId>> {
    let tx = conn.savepoint()?;
    let mut changed = Vec::new();
    for s in snapshots {
        times_of(&tx, s.image_id)?;
        if let Some(ms) = s.captured_at_ms {
            check_range(ms)?;
        }
        if write_time(&tx, s.image_id, s.captured_at_ms, s.source)? {
            changed.push(s.image_id);
        }
    }
    refresh_scene_bounds(&tx, &changed)?;
    tx.commit()?;
    Ok(changed)
}

/// For the XMP read path (rust-engine-dev): applies the capture time found in a sidecar
/// (`exif:DateTimeOriginal`, else `photoshop:DateCreated`, as naive ms). `None` or a time equal
/// to the file's EXIF time -> back to source `exif`; any other time -> source `sidecar`. Does
/// not mark the sidecar dirty (the catalog now agrees with it). Returns whether it changed.
pub fn apply_sidecar_time(conn: &Connection, id: ImageId, sidecar_ms: Option<i64>) -> AppResult<bool> {
    let (cur, exif, source) = times_of(conn, id)?;
    let (ms, src) = match sidecar_ms {
        Some(t) if Some(t) != exif => (Some(t), CaptureTimeSource::Sidecar),
        _ => (exif, CaptureTimeSource::Exif),
    };
    if cur == ms && source == src {
        return Ok(false);
    }
    conn.execute(
        "UPDATE images SET captured_at_ms = ?2, capture_time_source = ?3 WHERE id = ?1",
        params![id, ms, src.as_str()],
    )?;
    refresh_scene_bounds(conn, &[id])?;
    Ok(true)
}

/// The catalog part of `get_image_metadata`: everything except the values read from the file
/// (`focalLength35mm`, `exposureCompensationEv`, `flashFired`, `cameraSerial`, `gps`), which
/// stay `null` here (rust-engine-dev fills them from the file in the command).
pub fn image_metadata(conn: &Connection, id: ImageId) -> AppResult<ImageMetadata> {
    let e = repo::get_image(conn, id)?;
    let path = Path::new(&e.path);
    let sidecar = crate::xmp::sidecar_path(path);
    Ok(ImageMetadata {
        image_id: e.id,
        folder_path: path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
        extension: path.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default(),
        format: e.format,
        file_size: e.file_size,
        file_mtime_ms: e.file_mtime_ms,
        captured_at_ms: e.capture.captured_at_ms,
        original_captured_at_ms: e.capture.original_captured_at_ms,
        capture_time_source: e.capture.capture_time_source,
        camera: e.camera,
        lens: e.capture.lens,
        iso: e.capture.iso,
        shutter_seconds: e.capture.shutter_seconds,
        aperture: e.capture.aperture,
        focal_length_mm: e.capture.focal_length_mm,
        focal_length_35mm: None,
        exposure_compensation_ev: None,
        flash_fired: None,
        camera_serial: None,
        width: e.width,
        height: e.height,
        orientation: e.orientation,
        gps: None,
        sidecar_exists: sidecar.is_file(),
        sidecar_path: sidecar.to_string_lossy().into_owned(),
        companion_path: e.companion_path,
        missing: e.missing_since_ms.is_some(),
        path: e.path,
        file_name: e.file_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::ipc::error::ErrorKind;

    fn setup() -> Connection {
        let conn = open_in_memory();
        conn.execute_batch(
            "INSERT INTO folders (id, path, added_at) VALUES (1, '/f', 0);
             INSERT INTO images (id, folder_id, path, file_name, format, camera_make, sensor_layout,
                                 file_size, file_mtime_ms, imported_at, captured_at_ms, exif_captured_at_ms)
             VALUES (1, 1, '/f/a.ARW', 'a.ARW', 'arw', 'sony', 'bayer', 1, 0, 0, 10000, 10000),
                    (2, 1, '/f/b.ARW', 'b.ARW', 'arw', 'sony', 'bayer', 1, 0, 0, 20000, 20000),
                    (3, 1, '/f/c.ARW', 'c.ARW', 'arw', 'sony', 'bayer', 1, 0, 0, NULL, NULL),
                    (4, 1, '/f/d.ARW', 'd.ARW', 'arw', 'canon', 'bayer', 1, 0, 0, 3615000, 3615000);
             UPDATE images SET xmp_dirty = 0;",
        )
        .unwrap();
        conn
    }

    fn time(conn: &Connection, id: ImageId) -> (Option<i64>, Option<i64>, CaptureTimeSource) {
        times_of(conn, id).unwrap()
    }

    #[test]
    fn shift_set_sync_revert_and_restore() {
        let mut conn = setup();
        let r = edit(&mut conn, &[1, 2, 3], &CaptureTimeEdit::Shift { offset_ms: -1000 }).unwrap();
        assert_eq!((r.changed_ids, r.skipped_ids, r.offset_ms), (vec![1, 2], vec![3], Some(-1000)));
        assert_eq!(time(&conn, 1), (Some(9000), Some(10000), CaptureTimeSource::User));
        let dirty: i64 = conn.query_row("SELECT xmp_dirty FROM images WHERE id = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(dirty, 1);
        // The entry reports both times.
        let e = repo::get_image(&conn, 1).unwrap();
        assert_eq!(
            (e.capture.captured_at_ms, e.capture.original_captured_at_ms, e.capture.capture_time_source),
            (Some(9000), Some(10000), CaptureTimeSource::User)
        );

        // Undo restores the previous times and sources.
        assert_eq!(restore(&mut conn, &r.previous).unwrap(), vec![1, 2]);
        assert_eq!(time(&conn, 2), (Some(20000), Some(20000), CaptureTimeSource::Exif));

        // Set exact: the reference gets the time, the others shift by the same offset; a
        // reference without a time just gets it.
        let r =
            edit(&mut conn, &[1, 2], &CaptureTimeEdit::SetExact { reference_id: 2, captured_at_ms: 25000 }).unwrap();
        assert_eq!(r.offset_ms, Some(5000));
        assert_eq!((time(&conn, 1).0, time(&conn, 2).0), (Some(15000), Some(25000)));
        let r = edit(&mut conn, &[3], &CaptureTimeEdit::SetExact { reference_id: 3, captured_at_ms: 7 }).unwrap();
        assert_eq!((r.changed_ids, r.offset_ms), (vec![3], None));
        let err = edit(&mut conn, &[1], &CaptureTimeEdit::SetExact { reference_id: 2, captured_at_ms: 1 });
        assert_eq!(err.unwrap_err().kind, ErrorKind::InvalidArgument);

        // Sync cameras: the Canon (4) was 1 h ahead of the Sony frame 1 shot at the same moment.
        let r = edit(&mut conn, &[4], &CaptureTimeEdit::SyncCameras { reference_id: 1, target_id: 4 }).unwrap();
        assert_eq!(r.offset_ms, Some(15000 - 3615000));
        assert_eq!(time(&conn, 4).0, Some(15000));
        let err = edit(&mut conn, &[4], &CaptureTimeEdit::SyncCameras { reference_id: 99, target_id: 4 });
        assert_eq!(err.unwrap_err().kind, ErrorKind::NotFound);

        // Revert goes back to the file's time.
        let r = edit(&mut conn, &[1, 2, 3, 4], &CaptureTimeEdit::Revert).unwrap();
        assert_eq!(r.changed_ids, vec![1, 2, 3, 4]);
        assert_eq!(time(&conn, 4), (Some(3615000), Some(3615000), CaptureTimeSource::Exif));
        assert_eq!(time(&conn, 3), (None, None, CaptureTimeSource::Exif));

        // Atomic: a bad id writes nothing.
        let err = edit(&mut conn, &[1, 99], &CaptureTimeEdit::Shift { offset_ms: 1 });
        assert_eq!(err.unwrap_err().kind, ErrorKind::NotFound);
        assert_eq!(time(&conn, 1).0, Some(10000));
        let err = edit(&mut conn, &[1, 1], &CaptureTimeEdit::Shift { offset_ms: 1 });
        assert_eq!(err.unwrap_err().kind, ErrorKind::InvalidArgument);
        let err = edit(&mut conn, &[1], &CaptureTimeEdit::Shift { offset_ms: MAX_CAPTURE_TIME_MS });
        assert_eq!(err.unwrap_err().kind, ErrorKind::InvalidArgument);
    }

    #[test]
    fn sidecar_time_and_extraction_keep_corrections() {
        let mut conn = setup();
        assert!(apply_sidecar_time(&conn, 1, Some(4000)).unwrap());
        assert_eq!(time(&conn, 1), (Some(4000), Some(10000), CaptureTimeSource::Sidecar));
        assert!(!apply_sidecar_time(&conn, 1, Some(4000)).unwrap());
        // Re-extraction updates the EXIF time but keeps the correction.
        let meta = crate::raw::meta::ImageMeta { captured_at_ms: Some(11000), ..Default::default() };
        repo::record_extraction(&mut conn, 1, Some(&meta), Err("x")).unwrap();
        assert_eq!(time(&conn, 1), (Some(4000), Some(11000), CaptureTimeSource::Sidecar));
        // Same as EXIF -> source exif.
        assert!(apply_sidecar_time(&conn, 1, Some(11000)).unwrap());
        assert_eq!(time(&conn, 1), (Some(11000), Some(11000), CaptureTimeSource::Exif));
        repo::record_extraction(&mut conn, 1, Some(&meta), Err("x")).unwrap();
        let meta = crate::raw::meta::ImageMeta { captured_at_ms: Some(12000), ..Default::default() };
        repo::record_extraction(&mut conn, 1, Some(&meta), Err("x")).unwrap();
        assert_eq!(time(&conn, 1), (Some(12000), Some(12000), CaptureTimeSource::Exif));
    }

    #[test]
    fn metadata_from_catalog() {
        let conn = setup();
        let m = image_metadata(&conn, 1).unwrap();
        assert_eq!((m.extension.as_str(), m.folder_path.as_str()), ("arw", "/f"));
        assert_eq!(m.sidecar_path, "/f/a.xmp");
        assert!(!m.sidecar_exists && !m.missing && m.gps.is_none());
        assert_eq!(image_metadata(&conn, 99).unwrap_err().kind, ErrorKind::NotFound);
    }

    #[test]
    fn v17_migration_backfills_exif_time_and_accepts_paste_batches() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        let v16 = 16;
        for (i, sql) in crate::db::schema::MIGRATIONS[..v16].iter().enumerate() {
            let tx = conn.transaction().unwrap();
            tx.execute_batch(sql).unwrap();
            tx.pragma_update(None, "user_version", (i + 1) as i64).unwrap();
            tx.commit().unwrap();
        }
        conn.execute_batch(
            "INSERT INTO projects (id, name, shoot_type, created_at) VALUES (1, 'p', 'wedding', 0);
             INSERT INTO folders (id, path, added_at, project_id) VALUES (1, '/f', 0, 1);
             INSERT INTO images (id, folder_id, path, file_name, format, camera_make, file_size, file_mtime_ms,
                                 imported_at, captured_at_ms)
             VALUES (1, 1, '/f/a.arw', 'a.arw', 'arw', 'sony', 1, 0, 0, 1234);",
        )
        .unwrap();
        assert!(conn
            .execute("INSERT INTO edit_batches (label, kind, created_at) VALUES ('x', 'paste', 0)", [])
            .is_err());
        crate::db::migrate(&mut conn).unwrap();
        assert_eq!(time(&conn, 1), (Some(1234), Some(1234), CaptureTimeSource::Exif));
        conn.execute("INSERT INTO edit_batches (label, kind, created_at) VALUES ('x', 'paste', 0)", []).unwrap();
        assert!(conn
            .execute("INSERT INTO edit_batches (label, kind, created_at) VALUES ('x', 'nope', 0)", [])
            .is_err());
        let p = crate::db::projects::get_project(&conn, 1).unwrap();
        assert_eq!(p.reject_strictness, crate::ipc::types::RejectStrictness::Balanced);
        crate::db::projects::set_project_reject_strictness(&conn, 1, crate::ipc::types::RejectStrictness::Aggressive)
            .unwrap();
        assert_eq!(
            crate::db::projects::reject_strictness_of_image(&conn, 1).unwrap(),
            crate::ipc::types::RejectStrictness::Aggressive
        );
        let e =
            crate::db::projects::set_project_reject_strictness(&conn, 9, crate::ipc::types::RejectStrictness::Balanced);
        assert_eq!(e.unwrap_err().kind, ErrorKind::NotFound);
    }
}
