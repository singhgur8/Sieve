//! Embedded SQLite catalog.
//!
//! Crash safety (Phase 8, see `docs/decisions.md`):
//! - WAL journal + `synchronous = NORMAL`: a crash of the app (or a killed process) never
//!   loses a committed transaction and never corrupts the file; a power loss can roll back
//!   the last few commits but leaves a consistent catalog. `checkpoint_fullfsync` makes the
//!   (rare) checkpoints use `F_FULLFSYNC`, so Apple drives' volatile caches cannot reorder
//!   them. Every multi-row write is one transaction (or savepoint).
//! - The first [`open`] of a catalog in a process ([`health`] tracks it) runs
//!   `PRAGMA quick_check`. A healthy catalog is backed up (`VACUUM INTO`, atomic) to
//!   `<catalog>.bak-1` (newest) .. `.bak-3` before any migration and at most once a day
//!   ([`BACKUP_INTERVAL_MS`]).
//! - A damaged catalog is opened **read-only** (writes fail with an explanation, see
//!   [`health`] / [`read_only_message`]) and is never backed up over the good backups. A
//!   file that is not a database at all is moved aside (`<catalog>.corrupt-<ms>`) and a
//!   new catalog is created. [`stage_restore`] + the next launch put a backup back.

pub mod repo;
pub mod schema;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags};

use crate::ipc::error::{AppError, AppResult, ErrorKind};

/// Backups kept (`.bak-1` newest .. `.bak-N`).
pub const BACKUPS_KEPT: usize = 3;
/// A startup backup is taken when the newest one is older than this.
pub const BACKUP_INTERVAL_MS: i64 = 24 * 60 * 60 * 1000;

/// State of a catalog as found by the first [`open`] of this process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogHealth {
    Ok,
    /// Integrity check failed: every connection is read-only. `reason` = first problem.
    ReadOnly {
        reason: String,
    },
    /// The file was not a database; it was moved to `moved_to` and a new catalog created.
    Replaced {
        moved_to: PathBuf,
        reason: String,
    },
}

fn health_map() -> &'static Mutex<HashMap<PathBuf, CatalogHealth>> {
    static MAP: OnceLock<Mutex<HashMap<PathBuf, CatalogHealth>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Stable identity of a catalog path (the file itself may not exist yet).
fn key(path: &Path) -> PathBuf {
    match (path.parent().and_then(|d| d.canonicalize().ok()), path.file_name()) {
        (Some(dir), Some(name)) => dir.join(name),
        _ => path.to_path_buf(),
    }
}

/// Health of the catalog at `path` (as checked by the first [`open`]; `Ok` before that).
pub fn health(path: &Path) -> CatalogHealth {
    health_map().lock().unwrap_or_else(|e| e.into_inner()).get(&key(path)).cloned().unwrap_or(CatalogHealth::Ok)
}

/// User-facing explanation for a write refused because the catalog is damaged.
pub fn read_only_message(path: &Path, reason: &str) -> String {
    let backups = list_backups(path);
    let hint = match backups.first() {
        Some(b) => format!(
            "Quit Sieve and restore the backup {} (newest of {}), or copy it over {}.",
            b.path.display(),
            backups.len(),
            path.display()
        ),
        None => "No backup exists yet; export or copy what you need, then re-import the folders.".to_owned(),
    };
    format!("The catalog is damaged ({reason}) and was opened read-only, so changes cannot be saved. {hint}")
}

/// Opens (creating if needed) the catalog at `path` and brings it to the latest schema.
/// The first call per catalog in this process also checks integrity and backs it up (see
/// the module docs); a damaged catalog yields a read-only connection instead of an error.
pub fn open(path: &Path) -> AppResult<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let first = {
        let map = health_map().lock().unwrap_or_else(|e| e.into_inner());
        !map.contains_key(&key(path))
    };
    if first {
        let found = check_on_first_open(path);
        health_map().lock().unwrap_or_else(|e| e.into_inner()).insert(key(path), found);
    }
    if let CatalogHealth::ReadOnly { .. } = health(path) {
        return open_read_only(path);
    }
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    configure(&mut conn)?;
    Ok(conn)
}

/// Read-only connection (no migrations, `query_only`).
fn open_read_only(path: &Path) -> AppResult<Connection> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    conn.pragma_update(None, "query_only", "ON")?;
    Ok(conn)
}

/// Integrity check, pending-restore application and backups for the first open.
fn check_on_first_open(path: &Path) -> CatalogHealth {
    apply_staged_restore(path);
    let exists = std::fs::metadata(path).map(|m| m.len() > 0).unwrap_or(false);
    if !exists {
        return CatalogHealth::Ok;
    }
    match integrity(path) {
        Ok(None) => {}
        Ok(Some(problem)) => {
            eprintln!("catalog {}: integrity check failed: {problem}", path.display());
            return CatalogHealth::ReadOnly { reason: problem };
        }
        Err(e) if is_not_a_database(&e) => {
            let moved_to = aside_path(path);
            eprintln!("catalog {}: not a database ({e}); moved to {}", path.display(), moved_to.display());
            for suffix in ["", "-wal", "-shm"] {
                let from = PathBuf::from(format!("{}{suffix}", path.display()));
                if from.exists() {
                    let _ = std::fs::rename(&from, format!("{}{suffix}", moved_to.display()));
                }
            }
            return CatalogHealth::Replaced { moved_to, reason: e.to_string() };
        }
        Err(e) => {
            // Locked / unreadable: not evidence of corruption; the normal open reports it.
            eprintln!("catalog {}: integrity check skipped: {e}", path.display());
            return CatalogHealth::Ok;
        }
    }
    // Healthy: back up before migrations and at most once per interval.
    let pending = schema_version(path).map(|v| (v as usize) < schema::MIGRATIONS.len()).unwrap_or(false);
    let stale = list_backups(path).first().is_none_or(|b| now_ms() - b.modified_ms >= BACKUP_INTERVAL_MS);
    if pending || stale {
        if let Err(e) = backup(path) {
            eprintln!("catalog {}: backup failed: {}", path.display(), e.message);
        }
    }
    CatalogHealth::Ok
}

fn is_not_a_database(e: &rusqlite::Error) -> bool {
    matches!(e.sqlite_error_code(), Some(rusqlite::ErrorCode::NotADatabase))
}

/// `Ok(None)` = `quick_check` passed; `Ok(Some(first problem))`; `Err` = could not check.
pub fn integrity(path: &Path) -> rusqlite::Result<Option<String>> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    let problems: Vec<String> =
        conn.prepare("PRAGMA quick_check(5)")?.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(match problems.as_slice() {
        [ok] if ok == "ok" => None,
        [] => Some("integrity check returned nothing".to_owned()),
        [first, ..] => Some(first.clone()),
    })
}

fn schema_version(path: &Path) -> rusqlite::Result<i64> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    conn.pragma_query_value(None, "user_version", |r| r.get(0))
}

fn aside_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.corrupt-{}", path.display(), now_ms()))
}

/// One catalog backup file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backup {
    /// 1 = newest.
    pub index: usize,
    pub path: PathBuf,
    pub modified_ms: i64,
    pub size_bytes: u64,
}

/// `<catalog>.bak-N`.
pub fn backup_path(path: &Path, index: usize) -> PathBuf {
    PathBuf::from(format!("{}.bak-{index}", path.display()))
}

/// Existing backups of `path`, newest (`.bak-1`) first.
pub fn list_backups(path: &Path) -> Vec<Backup> {
    (1..=BACKUPS_KEPT)
        .filter_map(|index| {
            let p = backup_path(path, index);
            let m = std::fs::metadata(&p).ok()?;
            let modified_ms = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            Some(Backup { index, path: p, modified_ms, size_bytes: m.len() })
        })
        .collect()
}

/// Writes a consistent snapshot of the catalog (`VACUUM INTO` a temp file, then an atomic
/// rename) as `.bak-1`, shifting older backups down and dropping the oldest beyond
/// [`BACKUPS_KEPT`]. Safe while other connections are open (reads a WAL snapshot).
pub fn backup(path: &Path) -> AppResult<PathBuf> {
    let tmp = PathBuf::from(format!("{}.bak-tmp", path.display()));
    let _ = std::fs::remove_file(&tmp);
    {
        let conn =
            Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        conn.execute("VACUUM INTO ?1", [tmp.to_string_lossy()]).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            AppError::new(ErrorKind::Io, format!("catalog backup to {} failed: {e}", tmp.display()))
        })?;
    }
    let _ = std::fs::remove_file(backup_path(path, BACKUPS_KEPT));
    for i in (1..BACKUPS_KEPT).rev() {
        let from = backup_path(path, i);
        if from.exists() {
            std::fs::rename(&from, backup_path(path, i + 1))?;
        }
    }
    let newest = backup_path(path, 1);
    std::fs::rename(&tmp, &newest)?;
    Ok(newest)
}

/// `<catalog>.restore`: a backup staged by [`stage_restore`], applied by the next launch.
pub fn staged_restore_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.restore", path.display()))
}

/// Stages backup `index` to replace the catalog on the next launch (the running app keeps
/// connections open, so the swap happens before the first [`open`] of the next process).
/// The backup must pass `quick_check`.
pub fn stage_restore(path: &Path, index: usize) -> AppResult<PathBuf> {
    let src = backup_path(path, index);
    if !src.exists() {
        return Err(AppError::not_found(format!("backup {} does not exist", src.display())));
    }
    match integrity(&src) {
        Ok(None) => {}
        Ok(Some(p)) => return Err(AppError::invalid(format!("backup {} is damaged too: {p}", src.display()))),
        Err(e) => return Err(AppError::invalid(format!("backup {} cannot be read: {e}", src.display()))),
    }
    let staged = staged_restore_path(path);
    std::fs::copy(&src, &staged)?;
    Ok(staged)
}

/// Swaps a staged restore into place: the current catalog (and its WAL) is kept as
/// `<catalog>.corrupt-<ms>` (never deleted).
fn apply_staged_restore(path: &Path) {
    let staged = staged_restore_path(path);
    if !staged.exists() {
        return;
    }
    let aside = aside_path(path);
    for suffix in ["", "-wal", "-shm"] {
        let from = PathBuf::from(format!("{}{suffix}", path.display()));
        if from.exists() {
            if let Err(e) = std::fs::rename(&from, format!("{}{suffix}", aside.display())) {
                eprintln!("catalog restore: cannot move {} aside: {e}", from.display());
                return;
            }
        }
    }
    match std::fs::rename(&staged, path) {
        Ok(()) => eprintln!("catalog restored from backup; previous catalog kept as {}", aside.display()),
        Err(e) => eprintln!("catalog restore failed: {e}"),
    }
}

/// Runs `f` atomically inside a savepoint: nests inside an open transaction (then it is
/// part of the outer one) or acts as its own transaction. `f` may call functions that use
/// savepoints themselves (the `repo`/`history`/`xmp` write functions do), but not ones
/// that `BEGIN` a transaction (`Connection::transaction`).
pub fn atomic<T>(conn: &mut Connection, f: impl FnOnce(&mut Connection) -> AppResult<T>) -> AppResult<T> {
    conn.execute_batch("SAVEPOINT sieve_atomic")?;
    match f(conn) {
        Ok(v) => {
            conn.execute_batch("RELEASE sieve_atomic")?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO sieve_atomic; RELEASE sieve_atomic");
            Err(e)
        }
    }
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
    conn.pragma_update(None, "checkpoint_fullfsync", "ON")?;
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

    /// Makes the next `open(path)` behave like the first one of a new process.
    fn forget_health(path: &Path) {
        health_map().lock().unwrap().remove(&key(path));
    }

    /// Catalog at `path` (fully migrated, checkpointed, closed) holding `n` images.
    fn seeded(path: &Path, n: i64) {
        let conn = open(path).unwrap();
        conn.execute_batch(&format!(
            "INSERT INTO folders (id, path, added_at) VALUES (1, '/f', 0);
             WITH RECURSIVE s(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM s WHERE x < {n})
             INSERT INTO images (id, folder_id, path, file_name, format, camera_make, file_size, file_mtime_ms,
                                 imported_at)
             SELECT x, 1, '/f/' || x || '.arw', x || '.arw', 'arw', 'sony', 1, 0, 0 FROM s;
             PRAGMA wal_checkpoint(TRUNCATE);"
        ))
        .unwrap();
    }

    fn ratings(conn: &Connection) -> Vec<(i64, i64)> {
        conn.prepare("SELECT rating, COUNT(*) FROM images GROUP BY rating ORDER BY rating")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn first_open_checks_backs_up_and_rotates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.sqlite");
        seeded(&path, 10);
        assert!(list_backups(&path).is_empty(), "a new catalog has nothing to back up");
        // Next launch: healthy, no backup yet -> one is taken (stale).
        forget_health(&path);
        drop(open(&path).unwrap());
        assert_eq!(health(&path), CatalogHealth::Ok);
        let b = list_backups(&path);
        assert_eq!(b.len(), 1);
        assert_eq!(integrity(&b[0].path).unwrap(), None);
        let n: i64 =
            Connection::open(&b[0].path).unwrap().query_row("SELECT COUNT(*) FROM images", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 10);
        // A fresh backup is not repeated within the interval.
        forget_health(&path);
        drop(open(&path).unwrap());
        assert_eq!(list_backups(&path).len(), 1);
        // Rotation keeps BACKUPS_KEPT, newest first.
        for _ in 0..4 {
            backup(&path).unwrap();
        }
        let b = list_backups(&path);
        assert_eq!(b.iter().map(|b| b.index).collect::<Vec<_>>(), [1, 2, 3]);
        assert!(!backup_path(&path, 4).exists());
        assert!(!PathBuf::from(format!("{}.bak-tmp", path.display())).exists());

        // An older schema is backed up (at its old version) before migrating, even when a
        // fresh backup exists.
        let old = dir.path().join("old.sqlite");
        {
            let mut conn = Connection::open(&old).unwrap();
            for (i, sql) in schema::MIGRATIONS[..9].iter().enumerate() {
                let tx = conn.transaction().unwrap();
                tx.execute_batch(sql).unwrap();
                tx.pragma_update(None, "user_version", (i + 1) as i64).unwrap();
                tx.commit().unwrap();
            }
        }
        backup(&old).unwrap();
        let conn = open(&old).unwrap();
        let v: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(v as usize, schema::MIGRATIONS.len());
        let b = list_backups(&old);
        assert_eq!(b.len(), 2, "pending migrations back up despite a fresh backup");
        assert_eq!(schema_version(&b[0].path).unwrap(), 9);
    }

    #[test]
    fn damaged_catalog_opens_read_only_and_restores_from_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.sqlite");
        seeded(&path, 3000);
        backup(&path).unwrap();
        // Scribble over the middle of the file (table/index pages), as a failing disk would.
        let mut bytes = std::fs::read(&path).unwrap();
        let mid = bytes.len() / 2;
        for b in &mut bytes[mid..mid + 16384] {
            *b = 0x5A;
        }
        std::fs::write(&path, &bytes).unwrap();

        forget_health(&path);
        let conn = open(&path).unwrap();
        let CatalogHealth::ReadOnly { reason } = health(&path) else { panic!("expected read-only") };
        let err = conn.execute("UPDATE images SET rating = 1", []).unwrap_err();
        assert!(err.to_string().contains("readonly") || err.to_string().contains("read-only"), "{err}");
        let msg = read_only_message(&path, &reason);
        assert!(msg.contains("read-only") && msg.contains(".bak-1"), "{msg}");
        // The damaged file is never backed up over the good backups.
        assert_eq!(list_backups(&path).len(), 1);
        drop(conn);

        // Restore: staged now, applied by the next launch; the damaged file is kept aside.
        stage_restore(&path, 1).unwrap();
        forget_health(&path);
        let conn = open(&path).unwrap();
        assert_eq!(health(&path), CatalogHealth::Ok);
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM images", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 3000);
        conn.execute("UPDATE images SET rating = 2 WHERE id = 1", []).unwrap();
        let aside = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".corrupt-"))
            .count();
        assert!(aside >= 1);
        assert!(!staged_restore_path(&path).exists());
        assert!(stage_restore(&path, 3).is_err(), "missing backup");
    }

    #[test]
    fn non_database_file_is_moved_aside_and_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.sqlite");
        std::fs::write(&path, vec![0x42u8; 8192]).unwrap();
        let conn = open(&path).unwrap();
        let CatalogHealth::Replaced { moved_to, .. } = health(&path) else { panic!("expected replaced") };
        assert!(moved_to.exists());
        let v: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(v as usize, schema::MIGRATIONS.len());
    }

    #[test]
    fn atomic_nests_savepoints_and_rolls_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.sqlite");
        seeded(&path, 5);
        let mut conn = open(&path).unwrap();
        // Inner repo write (its own savepoint) + a failing step: nothing is kept.
        let r: AppResult<()> = atomic(&mut conn, |c| {
            repo::set_rating(c, &[1, 2, 3], 4)?;
            Err(AppError::internal("boom"))
        });
        assert!(r.is_err());
        assert_eq!(ratings(&conn), [(0, 5)]);
        atomic(&mut conn, |c| repo::set_rating(c, &[1, 2], 5)).unwrap();
        assert_eq!(ratings(&conn), [(0, 3), (5, 2)]);
    }

    /// Kill test, child half: rewrites every rating in one batch and aborts the process
    /// from another thread while the batch is running (no unwinding, no rollback, WAL
    /// frames half written). Run by `killed_mid_batch_leaves_catalog_consistent`.
    #[test]
    #[ignore = "child process of killed_mid_batch_leaves_catalog_consistent"]
    fn crash_child() {
        let Some(path) = std::env::var_os("SIEVE_CRASH_CATALOG") else { return };
        let path = PathBuf::from(path);
        let mut conn = open(&path).unwrap();
        let ids: Vec<i64> = (1..=200_000).collect();
        let delay_ms: u64 = std::env::var("SIEVE_CRASH_DELAY_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            std::process::abort();
        });
        loop {
            repo::set_rating(&mut conn, &ids, 3).unwrap();
            repo::set_rating(&mut conn, &ids, 0).unwrap();
        }
    }

    #[test]
    fn killed_mid_batch_leaves_catalog_consistent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.sqlite");
        seeded(&path, 200_000);
        let exe = std::env::current_exe().unwrap();
        for delay in [15, 60, 150] {
            let status = std::process::Command::new(&exe)
                .args(["db::tests::crash_child", "--exact", "--ignored", "--nocapture"])
                .env("SIEVE_CRASH_CATALOG", &path)
                .env("SIEVE_CRASH_DELAY_MS", delay.to_string())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(!status.success(), "the child must die mid-batch");
            // Recovery: WAL replay on open; the batch is all-or-nothing and the file is sound.
            assert_eq!(integrity(&path).unwrap(), None);
            forget_health(&path);
            let conn = open(&path).unwrap();
            assert_eq!(health(&path), CatalogHealth::Ok);
            let r = ratings(&conn);
            assert!(r == [(0, 200_000)] || r == [(3, 200_000)], "partial batch after crash: {r:?}");
        }
    }

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

    #[test]
    fn v9_relaxes_format_check_in_place_and_keeps_children() {
        // A v8 catalog with an image and child rows in several FK tables.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cat.sqlite");
        {
            let mut conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "foreign_keys", "ON").unwrap();
            for (i, sql) in schema::MIGRATIONS[..8].iter().enumerate() {
                let tx = conn.transaction().unwrap();
                tx.execute_batch(sql).unwrap();
                tx.pragma_update(None, "user_version", (i + 1) as i64).unwrap();
                tx.commit().unwrap();
            }
            conn.execute_batch(
                "INSERT INTO folders (id, path, added_at) VALUES (1, '/f', 0);
                 INSERT INTO images (id, folder_id, path, file_name, format, camera_make, sensor_layout,
                                     file_size, file_mtime_ms, imported_at)
                 VALUES (1, 1, '/f/a.arw', 'a.arw', 'arw', 'sony', 'bayer', 1, 0, 0);
                 INSERT INTO thumbnails (image_id, status) VALUES (1, 'pending');
                 INSERT INTO adjustments (image_id, params_json, process_version, updated_at) VALUES (1, '{}', 1, 0);",
            )
            .unwrap();
            let jpeg = "INSERT INTO images (id, folder_id, path, file_name, format, camera_make, sensor_layout,
                                            file_size, file_mtime_ms, imported_at)
                        VALUES (2, 1, '/f/b.jpg', 'b.jpg', 'jpeg', 'other', 'unknown', 1, 0, 0)";
            assert!(conn.execute(jpeg, []).is_err(), "v8 rejects non-RAW formats");
        }
        // Upgrade on the same connection that then writes (schema must be reloaded).
        let conn = open(&path).unwrap();
        conn.execute(
            "INSERT INTO images (id, folder_id, path, file_name, format, camera_make, sensor_layout,
                                 file_size, file_mtime_ms, imported_at, companion_path)
             VALUES (2, 1, '/f/b.jpg', 'b.jpg', 'jpeg', 'other', 'unknown', 1, 0, 0, NULL)",
            [],
        )
        .unwrap();
        for f in ["heic", "tiff", "png"] {
            conn.execute(
                "INSERT INTO images (folder_id, path, file_name, format, camera_make, file_size, file_mtime_ms, imported_at)
                 VALUES (1, ?1, 'x', ?2, 'other', 1, 0, 0)",
                [format!("/f/x.{f}"), f.to_owned()],
            )
            .unwrap();
        }
        assert!(conn
            .execute(
                "INSERT INTO images (folder_id, path, file_name, format, camera_make, file_size, file_mtime_ms, imported_at)
                 VALUES (1, '/f/y.gif', 'y', 'gif', 'other', 1, 0, 0)",
                [],
            )
            .is_err());
        // Children survived; the constraint text is updated; integrity holds.
        let kids: (i64, i64) = conn
            .query_row("SELECT (SELECT COUNT(*) FROM thumbnails), (SELECT COUNT(*) FROM adjustments)", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(kids, (1, 1));
        let sql: String =
            conn.query_row("SELECT sql FROM sqlite_schema WHERE name = 'images'", [], |r| r.get(0)).unwrap();
        assert!(sql.contains("'jpeg', 'heic', 'tiff', 'png'") && sql.contains("develop_warnings"), "{sql}");
        let ok: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
        assert_eq!(ok, "ok");
        // A fresh connection sees the relaxed constraint too.
        drop(conn);
        let conn = open(&path).unwrap();
        conn.execute("UPDATE images SET format = 'png' WHERE id = 2", []).unwrap();
    }

    #[test]
    fn v10_flags_unimported_sidecar_masks_and_caches_mattes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cat.sqlite");
        {
            let mut conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "foreign_keys", "ON").unwrap();
            for (i, sql) in schema::MIGRATIONS[..9].iter().enumerate() {
                let tx = conn.transaction().unwrap();
                tx.execute_batch(sql).unwrap();
                tx.pragma_update(None, "user_version", (i + 1) as i64).unwrap();
                tx.commit().unwrap();
            }
            conn.execute_batch(
                r#"INSERT INTO folders (id, path, added_at) VALUES (1, '/f', 0);
                 INSERT INTO images (id, folder_id, path, file_name, format, camera_make, file_size, file_mtime_ms,
                                     imported_at, develop_warnings)
                 VALUES (1, 1, '/f/a.arw', 'a.arw', 'arw', 'sony', 1, 0, 0,
                         '[{"code":"masks_unsupported","detail":"1"},{"code":"retouch_unsupported","detail":null}]'),
                        (2, 1, '/f/b.arw', 'b.arw', 'arw', 'sony', 1, 0, 0, '[{"code":"retouch_unsupported","detail":null}]'),
                        (3, 1, '/f/c.arw', 'c.arw', 'arw', 'sony', 1, 0, 0, NULL);"#,
            )
            .unwrap();
        }
        let conn = open(&path).unwrap();
        let flags: Vec<(i64, i64, bool)> = conn
            .prepare("SELECT id, masks_pending_import, xmp_dirty FROM images ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(flags, vec![(1, 1, false), (2, 0, false), (3, 0, false)], "flag set without dirtying");

        let insert = "INSERT INTO mask_cache (image_id, digest, kind, origin, model_version, input_digest, path,
                                              width, height, bounds_x, bounds_y, bounds_w, bounds_h, coverage, created_at)
                      VALUES (1, 'E71A59AFC894F4F898F72751E30113DA', 'subject', ?1, 'lr:251659306', NULL,
                              'masks/1/E71A59AFC894F4F898F72751E30113DA.png', 1605, 1332, 0.1028, 0.0, 0.5573, 0.6937, 0.3, 0)";
        conn.execute(insert, ["lightroom"]).unwrap();
        assert!(conn.execute(insert, ["lightroom"]).is_err(), "(image, digest) is unique");
        conn.execute("DELETE FROM mask_cache", []).unwrap();
        assert!(conn.execute(insert, ["photoshop"]).is_err(), "origin is checked");
        conn.execute(insert, ["sieve"]).unwrap();
        conn.execute("DELETE FROM images WHERE id = 1", []).unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM mask_cache", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0, "cascade with the image");
    }
}
