//! Background ingest pipeline: embedded-preview extraction + EXIF -> catalog.
//! Owned by rust-engine-dev. The architect fixed only the public surface used by
//! `ipc::commands` and `lib.rs` (`IngestConfig`, `Ingest::{new, config, is_running,
//! start, regenerate}`, `import_status`); everything behind it is free to change.
//!
//! Intended design (see `docs/architecture.md`, "Ingest pipeline"):
//! - `start` is an idempotent kick: if idle, spawn a worker that streams
//!   `thumbnails.status = 'pending'` rows from the catalog in small batches and
//!   processes them on a rayon pool; if already running, the worker picks up newly
//!   pending rows on its next fetch. Bounded memory: never load the whole set.
//! - The worker opens its own SQLite connection to `config.catalog_path` (WAL allows a
//!   concurrent writer/readers) instead of contending on the command connection.
//! - Per image: `raw::` extracts the embedded JPEG + EXIF, writes
//!   `<thumbs_dir>/<image_id>_512.jpg` and `<image_id>_2048.jpg` (orientation applied),
//!   updates `images` EXIF columns + `thumbnails` row, then emits `ThumbnailReady`
//!   or `ThumbnailFailed`, plus throttled `ImportProgress`.
//! - On startup `lib.rs` calls `start` so rows left `pending` by a previous session resume.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::Connection;
use tauri::AppHandle;

use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::{ImageId, ImportStatus};

/// Resolved locations, fixed at startup.
#[derive(Debug, Clone)]
pub struct IngestConfig {
    /// Catalog file; the pipeline opens its own connection to it.
    pub catalog_path: PathBuf,
    /// Cache root (`<app_cache_dir>` or `$LUMENRAW_CACHE`).
    pub cache_dir: PathBuf,
}

impl IngestConfig {
    /// Directory holding `<id>_512.jpg` thumbnails and `<id>_2048.jpg` previews.
    pub fn thumbs_dir(&self) -> PathBuf {
        self.cache_dir.join("thumbs")
    }
}

/// Managed Tauri state for the pipeline.
pub struct Ingest {
    config: IngestConfig,
    running: AtomicBool,
}

impl Ingest {
    pub fn new(config: IngestConfig) -> Self {
        Self { config, running: AtomicBool::new(false) }
    }

    pub fn config(&self) -> &IngestConfig {
        &self.config
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// Ensures the background worker is processing every `pending` thumbnail.
    /// Must return immediately (never blocks on extraction).
    /// STUB: no-op until rust-engine-dev implements the pipeline.
    pub fn start(&self, _app: &AppHandle) -> AppResult<()> {
        Ok(())
    }

    /// Resets `ids` to `pending` (deleting their cache files) and kicks the worker.
    /// Unknown ids fail the whole batch with `not_found`.
    /// STUB: not implemented yet.
    pub fn regenerate(&self, _app: &AppHandle, _ids: Vec<ImageId>) -> AppResult<()> {
        Err(AppError::internal("regenerate_thumbnails: not implemented"))
    }
}

/// Catalog-wide thumbnail counts.
pub fn import_status(conn: &Connection, running: bool) -> AppResult<ImportStatus> {
    let mut status = ImportStatus { running, ..Default::default() };
    let mut stmt = conn.prepare("SELECT status, COUNT(*) FROM thumbnails GROUP BY status")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?)))?;
    for row in rows {
        let (s, n) = row?;
        match s.as_str() {
            "pending" => status.pending = n,
            "ready" => status.ready = n,
            "failed" => status.failed = n,
            _ => {}
        }
        status.total += n;
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn import_status_counts_by_status() {
        let conn = db::open_in_memory();
        conn.execute_batch(
            "INSERT INTO folders (id, path, added_at) VALUES (1, '/f', 0);
             INSERT INTO images (id, folder_id, path, file_name, format, camera_make, file_size, file_mtime_ms, imported_at)
             VALUES (1, 1, '/f/a.arw', 'a.arw', 'arw', 'sony', 1, 0, 0),
                    (2, 1, '/f/b.arw', 'b.arw', 'arw', 'sony', 1, 0, 0),
                    (3, 1, '/f/c.arw', 'c.arw', 'arw', 'sony', 1, 0, 0);
             INSERT INTO thumbnails (image_id, status, path, preview_path, width, height)
             VALUES (1, 'ready', '/c/thumbs/1_512.jpg', '/c/thumbs/1_2048.jpg', 512, 341);
             INSERT INTO thumbnails (image_id, status, error) VALUES (2, 'failed', 'bad');
             INSERT INTO thumbnails (image_id) VALUES (3);",
        )
        .unwrap();
        let s = import_status(&conn, true).unwrap();
        assert_eq!(s, ImportStatus { total: 3, pending: 1, ready: 1, failed: 1, running: true });
    }
}
