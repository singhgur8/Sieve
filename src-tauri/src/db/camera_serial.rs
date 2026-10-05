//! Camera body serials (IPC v19.2, migration 0018): `images.camera_serial` tells two bodies of
//! the same model apart (`CameraInfo.serial`, `CameraBody` filters, Edit Capture Time > sync
//! cameras with scope `body`).
//!
//! New imports get the serial from thumbnail extraction (`repo::record_extraction`, from
//! `raw::meta::ImageMeta::serial`). Photos imported before v19.2 have
//! `camera_serial_read = 0`; [`backfill`] reads them in the background after startup (a few
//! hundred KB of EXIF per file through `raw::exif_dirs`), never blocking the app. Originals
//! that are missing are left for a later launch.
//! Implemented by the architect; rust-engine-dev owns it from here.

use std::path::Path;

use rusqlite::{params, Connection};

use crate::ipc::error::AppResult;
use crate::ipc::types::ImageId;
use crate::raw;

/// Photos read per transaction by [`backfill`].
const BATCH: u32 = 200;

/// Up to `limit` photos with id > `after` whose file was never read for its serial and whose
/// original is not known to be missing: `(id, path)`, id order.
pub fn unread(conn: &Connection, after: ImageId, limit: u32) -> AppResult<Vec<(ImageId, String)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, path FROM images
         WHERE camera_serial_read = 0 AND missing_since_ms IS NULL AND id > ?1
         ORDER BY id LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![after, limit], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Number of photos [`backfill`] still has to read (including missing originals).
pub fn unread_count(conn: &Connection) -> AppResult<u32> {
    Ok(conn.query_row("SELECT COUNT(*) FROM images WHERE camera_serial_read = 0", [], |r| r.get(0))?)
}

/// Records the serial read from photo `id`'s file (`None` = the file has none). Keeps a
/// serial already known (extraction may have run meanwhile). Returns whether the row changed.
pub fn record(conn: &Connection, id: ImageId, serial: Option<&str>) -> AppResult<bool> {
    let n = conn
        .prepare_cached(
            "UPDATE images SET camera_serial = COALESCE(camera_serial, ?2), camera_serial_read = 1
         WHERE id = ?1 AND camera_serial_read = 0",
        )?
        .execute(params![id, serial])?;
    Ok(n > 0)
}

/// The body serial of the file at `path`: `None` = the file cannot be read now (missing,
/// offline volume: try again later), `Some(None)` = read, no serial.
pub fn read_file_serial(path: &Path) -> Option<Option<String>> {
    if !path.is_file() {
        return None;
    }
    Some(match raw::exif_dirs(path) {
        Ok(d) => raw::exif_info::from_dirs(&d).camera_serial.and_then(|s| raw::meta::clean_serial(&s)),
        Err(_) => None,
    })
}

/// Reads the serial of every photo imported before v19.2 (see the module docs). Returns the
/// number of photos recorded. Call on a background thread with its own connection.
pub fn backfill(conn: &Connection) -> AppResult<u32> {
    backfill_with(conn, read_file_serial)
}

/// [`backfill`] with an injectable file reader (tests).
pub fn backfill_with(conn: &Connection, read: impl Fn(&Path) -> Option<Option<String>>) -> AppResult<u32> {
    let mut after = 0;
    let mut recorded = 0;
    loop {
        let batch = unread(conn, after, BATCH)?;
        let Some(&(last, _)) = batch.last() else { break };
        after = last;
        // Read outside the transaction (file I/O), then write the batch at once.
        let found: Vec<(ImageId, Option<String>)> =
            batch.into_iter().filter_map(|(id, path)| read(Path::new(&path)).map(|s| (id, s))).collect();
        let tx = conn.unchecked_transaction()?;
        for (id, serial) in &found {
            if record(&tx, *id, serial.as_deref())? {
                recorded += 1;
            }
        }
        tx.commit()?;
    }
    Ok(recorded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    fn setup() -> Connection {
        let conn = open_in_memory();
        conn.execute_batch(
            "INSERT INTO folders (id, path, added_at) VALUES (1, '/f', 0);
             INSERT INTO images (id, folder_id, path, file_name, format, camera_make, sensor_layout,
                                 file_size, file_mtime_ms, imported_at)
             VALUES (1, 1, '/f/a.ARW', 'a.ARW', 'arw', 'sony', 'bayer', 1, 0, 0),
                    (2, 1, '/f/b.ARW', 'b.ARW', 'arw', 'sony', 'bayer', 1, 0, 0),
                    (3, 1, '/f/gone.ARW', 'gone.ARW', 'arw', 'sony', 'bayer', 1, 0, 0),
                    (4, 1, '/f/none.ARW', 'none.ARW', 'arw', 'sony', 'bayer', 1, 0, 0);",
        )
        .unwrap();
        conn
    }

    fn serial(conn: &Connection, id: ImageId) -> (Option<String>, bool) {
        conn.query_row("SELECT camera_serial, camera_serial_read FROM images WHERE id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap()
    }

    #[test]
    fn backfill_reads_unread_photos_once_and_retries_missing_files() {
        let conn = setup();
        // Photo 2 was extracted after v19.2 meanwhile: its serial stays.
        conn.execute("UPDATE images SET camera_serial = 'X1', camera_serial_read = 1 WHERE id = 2", []).unwrap();
        assert_eq!(unread_count(&conn).unwrap(), 3);
        let reader = |p: &Path| match p.file_name().unwrap().to_str().unwrap() {
            "gone.ARW" => None,
            "none.ARW" => Some(None),
            _ => Some(Some("06258214".to_owned())),
        };
        assert_eq!(backfill_with(&conn, reader).unwrap(), 2);
        assert_eq!(serial(&conn, 1), (Some("06258214".into()), true));
        assert_eq!(serial(&conn, 2), (Some("X1".into()), true));
        assert_eq!(serial(&conn, 3), (None, false), "missing original: retried next launch");
        assert_eq!(serial(&conn, 4), (None, true), "read, no serial");
        assert_eq!(unread_count(&conn).unwrap(), 1);
        // A second pass reads only what is left.
        assert_eq!(backfill_with(&conn, |_| Some(Some("Z".into()))).unwrap(), 1);
        assert_eq!(serial(&conn, 3), (Some("Z".into()), true));
        assert_eq!(backfill_with(&conn, |_| panic!("nothing left to read")).unwrap(), 0);
    }

    #[test]
    fn missing_originals_are_not_read() {
        let conn = setup();
        conn.execute("UPDATE images SET missing_since_ms = 5 WHERE id <> 1", []).unwrap();
        assert_eq!(unread(&conn, 0, 10).unwrap(), vec![(1, "/f/a.ARW".to_owned())]);
    }
}
