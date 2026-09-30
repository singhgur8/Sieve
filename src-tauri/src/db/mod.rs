//! Embedded SQLite catalog.

pub mod repo;
pub mod schema;

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use crate::ipc::error::AppResult;

/// Opens (creating if needed) the catalog at `path` and brings it to the latest schema.
pub fn open(path: &Path) -> AppResult<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    configure(&mut conn)?;
    Ok(conn)
}

/// In-memory catalog for tests.
#[cfg(test)]
pub fn open_in_memory() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    configure(&mut conn).unwrap();
    conn
}

fn configure(conn: &mut Connection) -> AppResult<()> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    migrate(conn)
}

fn migrate(conn: &mut Connection) -> AppResult<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (i, sql) in schema::MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_apply_and_are_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cat.sqlite");
        drop(open(&path).unwrap());
        let conn = open(&path).unwrap();
        let v: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(v as usize, schema::MIGRATIONS.len());
        let fk: i64 = conn.pragma_query_value(None, "foreign_keys", |r| r.get(0)).unwrap();
        assert_eq!(fk, 1);
    }

    #[test]
    fn v5_drops_path_style_lut_refs() {
        let mut conn = Connection::open_in_memory().unwrap();
        for sql in &schema::MIGRATIONS[..4] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 4).unwrap();
        conn.execute_batch(
            "INSERT INTO folders (id, path, added_at) VALUES (1, '/f', 0);
             INSERT INTO images (id, folder_id, path, file_name, format, camera_make, sensor_layout,
                                 file_size, file_mtime_ms, imported_at)
             VALUES (1, 1, '/f/a.arw', 'a.arw', 'arw', 'sony', 'bayer', 1, 0, 0),
                    (2, 1, '/f/b.arw', 'b.arw', 'arw', 'sony', 'bayer', 1, 0, 0);
             INSERT INTO adjustments (image_id, params_json, process_version, updated_at) VALUES
                 (1, '{\"exposure\":1.0,\"lut\":{\"path\":\"/x.cube\",\"amount\":50}}', 1, 0),
                 (2, '{\"exposure\":1.0,\"lut\":{\"id\":\"film\",\"amount\":50}}', 1, 0);",
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        let lut = |id: i64| -> Option<String> {
            conn.query_row(
                "SELECT json_extract(params_json, '$.lut.id') FROM adjustments WHERE image_id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(lut(1), None);
        assert_eq!(lut(2).as_deref(), Some("film"));
        let exposure: f64 = conn
            .query_row("SELECT json_extract(params_json, '$.exposure') FROM adjustments WHERE image_id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(exposure, 1.0);
    }

    #[test]
    fn v6_export_tables_enforce_constraints() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("cat.sqlite")).unwrap();
        conn.execute_batch(
            "INSERT INTO folders (id, path, added_at) VALUES (1, '/f', 0);
             INSERT INTO images (id, folder_id, path, file_name, format, camera_make, sensor_layout,
                                 file_size, file_mtime_ms, imported_at)
             VALUES (1, 1, '/f/a.arw', 'a.arw', 'arw', 'sony', 'bayer', 1, 0, 0);
             INSERT INTO export_presets (name, settings_json, created_at, updated_at) VALUES ('Web', '{}', 0, 0);
             INSERT INTO export_jobs (id, state, settings_json, total, created_at) VALUES (1, 'queued', '{}', 1, 0);
             INSERT INTO export_items (job_id, seq, image_id) VALUES (1, 0, 1);",
        )
        .unwrap();
        let dup = conn.execute(
            "INSERT INTO export_presets (name, settings_json, created_at, updated_at) VALUES ('WEB', '{}', 0, 0)",
            [],
        );
        assert!(dup.is_err(), "preset names are unique case-insensitively");
        assert!(conn.execute("UPDATE export_jobs SET state = 'bogus'", []).is_err());
        assert!(conn.execute("UPDATE export_items SET status = 'bogus'", []).is_err());
        let status: String = conn.query_row("SELECT status FROM export_items", [], |r| r.get(0)).unwrap();
        assert_eq!(status, "pending");
        conn.execute("DELETE FROM export_jobs WHERE id = 1", []).unwrap();
        let items: i64 = conn.query_row("SELECT COUNT(*) FROM export_items", [], |r| r.get(0)).unwrap();
        assert_eq!(items, 0);
    }

    #[test]
    fn v7_scene_tables_and_cascades() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("cat.sqlite")).unwrap();
        conn.execute_batch(
            "INSERT INTO folders (id, path, added_at) VALUES (1, '/f', 0);
             INSERT INTO images (id, folder_id, path, file_name, format, camera_make, sensor_layout,
                                 file_size, file_mtime_ms, imported_at)
             VALUES (1, 1, '/f/a.arw', 'a.arw', 'arw', 'sony', 'bayer', 1, 0, 0);
             INSERT INTO scenes (id, folder_id, method, created_at, updated_at) VALUES (5, 1, 'auto', 0, 0);
             UPDATE images SET scene_id = 5, scene_anchor = 1 WHERE id = 1;
             INSERT INTO scene_features (image_id, version, features_json, computed_at) VALUES (1, 'v', '{}', 0);",
        )
        .unwrap();
        assert!(conn.execute("UPDATE scenes SET method = 'bogus'", []).is_err());
        // Scene edits must not mark sidecars dirty (scenes are not XMP-mapped).
        let dirty: bool = conn.query_row("SELECT xmp_dirty FROM images WHERE id = 1", [], |r| r.get(0)).unwrap();
        assert!(!dirty);
        conn.execute("DELETE FROM scenes WHERE id = 5", []).unwrap();
        let scene: Option<i64> = conn.query_row("SELECT scene_id FROM images WHERE id = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(scene, None);
        conn.execute("DELETE FROM images WHERE id = 1", []).unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM scene_features", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }
}
