//! Background ingest pipeline: embedded-preview extraction + EXIF -> catalog.
//! Owned by rust-engine-dev. The architect fixed only the public surface used by
//! `ipc::commands` and `lib.rs` (`IngestConfig`, `Ingest::{new, config, is_running,
//! start, regenerate}`, `import_status`); everything behind it is free to change.
//!
//! Design (see `docs/architecture.md`, "Ingest pipeline"):
//! - `start` is an idempotent kick: if idle, spawn a worker thread that streams
//!   `thumbnails.status = 'pending'` rows from the catalog in batches of `BATCH` and
//!   processes each batch on a rayon pool; if already running, the worker picks up
//!   newly pending rows on its next fetch. Memory is bounded by the number of pool
//!   threads (one image in flight per thread), never by the number of files.
//! - The worker opens its own SQLite connection to `config.catalog_path` (WAL allows a
//!   concurrent writer/readers) instead of contending on the command connection.
//!   Pool threads only extract/encode; the worker thread does all DB writes and events.
//! - Per image: `raw::extract` reads the container metadata + best embedded JPEG,
//!   `raw::preview::render` writes `<thumbs_dir>/<id>_2048.jpg` and `<id>_512.jpg`
//!   (orientation from the container applied), then EXIF + thumbnail row are written in
//!   one transaction and `ThumbnailReady` / `ThumbnailFailed` is emitted, plus
//!   `ImportProgress` at most every `PROGRESS_INTERVAL` and once when idle.
//! - On startup `lib.rs` calls `start` so rows left `pending` by a previous session resume.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use rayon::prelude::*;
use rusqlite::Connection;
use tauri::{AppHandle, Runtime};
use tauri_specta::Event;

use crate::db::{self, repo};
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::events::{ImportProgress, ThumbnailFailed, ThumbnailReady};
use crate::ipc::types::{ImageId, ImportStatus};
use crate::raw::{self, meta::ImageMeta};

/// Images fetched from the catalog per round trip.
const BATCH: u32 = 64;
/// Minimum spacing of `ImportProgress` events.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// Overrides the extraction thread count (default: [`default_threads`]).
const THREADS_ENV: &str = "SIEVE_INGEST_THREADS";

/// Half the cores, 2..=8. Each thread holds ~30 MB of reusable buffers; 8 threads
/// already exceed the throughput target several times over (measured ~170 files/s on
/// a 16-core M-series), and leave cores for the UI and the analysis worker.
pub fn default_threads() -> usize {
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    (cores / 2).clamp(2, 8)
}

/// Resolved locations, fixed at startup.
#[derive(Debug, Clone)]
pub struct IngestConfig {
    /// Catalog file; the pipeline opens its own connection to it.
    pub catalog_path: PathBuf,
    /// Cache root (`<app_cache_dir>` or `$SIEVE_CACHE`).
    pub cache_dir: PathBuf,
}

impl IngestConfig {
    /// Directory holding `<id>_512.jpg` thumbnails and `<id>_2048.jpg` previews.
    pub fn thumbs_dir(&self) -> PathBuf {
        self.cache_dir.join("thumbs")
    }

    pub fn thumb_path(&self, id: ImageId) -> PathBuf {
        self.thumbs_dir().join(format!("{id}_512.jpg"))
    }

    pub fn preview_path(&self, id: ImageId) -> PathBuf {
        self.thumbs_dir().join(format!("{id}_2048.jpg"))
    }
}

/// Receives pipeline events. Implemented for `AppHandle` (Tauri events) and by tests
/// and the bench tool. Called only from the worker thread.
pub trait IngestSink: Send + Sync {
    fn ready(&self, event: ThumbnailReady);
    fn failed(&self, event: ThumbnailFailed);
    fn progress(&self, event: ImportProgress);
}

impl<R: Runtime> IngestSink for AppHandle<R> {
    fn ready(&self, event: ThumbnailReady) {
        let _ = event.emit(self);
    }
    fn failed(&self, event: ThumbnailFailed) {
        let _ = event.emit(self);
    }
    fn progress(&self, event: ImportProgress) {
        let _ = event.emit(self);
    }
}

/// Managed Tauri state for the pipeline.
pub struct Ingest {
    config: IngestConfig,
    running: Arc<AtomicBool>,
}

impl Ingest {
    pub fn new(config: IngestConfig) -> Self {
        Self { config, running: Arc::new(AtomicBool::new(false)) }
    }

    pub fn config(&self) -> &IngestConfig {
        &self.config
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Ensures the background worker is processing every `pending` thumbnail.
    /// Returns immediately (never blocks on extraction).
    pub fn start(&self, app: &AppHandle) -> AppResult<()> {
        spawn_worker(self.config.clone(), self.running.clone(), app.clone())
    }

    /// Resets `ids` to `pending` (deleting their cache files) and kicks the worker.
    /// Unknown ids fail the whole batch with `not_found`.
    pub fn regenerate(&self, app: &AppHandle, ids: Vec<ImageId>) -> AppResult<()> {
        let mut conn = db::open(&self.config.catalog_path)?;
        reset_and_clean(&mut conn, &ids)?;
        self.start(app)
    }
}

/// Resets `ids` to pending and removes their cache files.
pub fn reset_and_clean(conn: &mut Connection, ids: &[ImageId]) -> AppResult<()> {
    for path in repo::reset_thumbnails(conn, ids)? {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

/// Spawns the worker unless one is already running. The `running` flag is claimed
/// here and released by the worker when it finds no pending rows.
pub fn spawn_worker<S: IngestSink + 'static>(config: IngestConfig, running: Arc<AtomicBool>, sink: S) -> AppResult<()> {
    if running.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return Ok(());
    }
    let flag = running.clone();
    let spawned = std::thread::Builder::new().name("sieve-ingest".into()).spawn(move || {
        if let Err(e) = run_until_idle(&config, &sink, &flag) {
            eprintln!("[ingest] worker stopped: {}", e.message);
            flag.store(false, Ordering::SeqCst);
        }
    });
    if let Err(e) = spawned {
        running.store(false, Ordering::SeqCst);
        return Err(AppError::internal(format!("spawn ingest worker: {e}")));
    }
    Ok(())
}

/// Counters for one pipeline run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunStats {
    pub done: u32,
    pub failed: u32,
}

/// Processes pending rows until none are left, then clears `running`.
/// The caller must have set `running` to `true`. Blocking; used by the worker thread,
/// tests and the bench tool.
pub fn run_until_idle(config: &IngestConfig, sink: &dyn IngestSink, running: &AtomicBool) -> AppResult<RunStats> {
    let mut conn = db::open(&config.catalog_path)?;
    let thumbs_dir = config.thumbs_dir();
    std::fs::create_dir_all(&thumbs_dir)?;
    let threads = std::env::var(THREADS_ENV)
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n: &usize| n > 0)
        .unwrap_or_else(default_threads);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("sieve-extract-{i}"))
        .build()
        .map_err(|e| AppError::internal(format!("thread pool: {e}")))?;

    let mut progress = Progress { stats: RunStats::default(), last: None };
    loop {
        let batch = repo::pending_thumbnails(&conn, BATCH)?;
        if batch.is_empty() {
            running.store(false, Ordering::SeqCst);
            // Close the race with a `start` that saw `running == true` just before we
            // cleared it: if rows appeared, try to reclaim the flag and continue.
            if repo::count_pending(&conn)? > 0
                && running.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok()
            {
                continue;
            }
            let RunStats { done, failed } = progress.stats;
            sink.progress(ImportProgress { done, total: done, failed });
            return Ok(progress.stats);
        }
        progress.maybe_emit(&conn, sink, true)?;

        let (tx, rx) = mpsc::channel::<Outcome>();
        std::thread::scope(|s| -> AppResult<()> {
            let dir = thumbs_dir.as_path();
            let batch = &batch;
            let pool = &pool;
            s.spawn(move || {
                pool.install(|| {
                    batch.par_iter().for_each_with(tx, |tx, item| {
                        let _ = tx.send(process_one(item, dir));
                    })
                })
            });
            for out in rx {
                progress.record(&mut conn, sink, out)?;
                progress.maybe_emit(&conn, sink, false)?;
            }
            Ok(())
        })?;
    }
}

struct Progress {
    stats: RunStats,
    last: Option<Instant>,
}

impl Progress {
    fn record(&mut self, conn: &mut Connection, sink: &dyn IngestSink, out: Outcome) -> AppResult<()> {
        let Outcome { id, meta, result } = out;
        match result {
            Ok(files) => {
                repo::record_extraction(conn, id, meta.as_ref(), Ok(&files))?;
                sink.ready(ThumbnailReady {
                    image_id: id,
                    path: files.thumb_path,
                    preview_path: files.preview_path,
                    width: files.width,
                    height: files.height,
                });
            }
            Err(reason) => {
                repo::record_extraction(conn, id, meta.as_ref(), Err(&reason))?;
                self.stats.failed += 1;
                sink.failed(ThumbnailFailed { image_id: id, reason });
            }
        }
        self.stats.done += 1;
        Ok(())
    }

    fn maybe_emit(&mut self, conn: &Connection, sink: &dyn IngestSink, force: bool) -> AppResult<()> {
        let due = self.last.is_none_or(|t| t.elapsed() >= PROGRESS_INTERVAL);
        if force || due {
            // Rows in flight are still `pending`, so this is exactly the remaining work.
            let pending = repo::count_pending(conn)?;
            let RunStats { done, failed } = self.stats;
            sink.progress(ImportProgress { done, total: done + pending, failed });
            self.last = Some(Instant::now());
        }
        Ok(())
    }
}

/// Result of extracting one image (produced on a pool thread).
struct Outcome {
    id: ImageId,
    meta: Option<ImageMeta>,
    result: Result<repo::ThumbFiles, String>,
}

/// Per-thread reusable buffers: embedded JPEG bytes + render scratch.
#[derive(Default)]
struct Scratch {
    jpeg: Vec<u8>,
    work: raw::preview::Work,
}

/// Embedded JPEG buffers above this are released after use (outliers).
const KEEP_JPEG_BYTES: usize = 16 << 20;

thread_local! {
    static SCRATCH: RefCell<Scratch> = RefCell::new(Scratch::default());
}

fn path_string(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn process_one(item: &repo::PendingImage, thumbs_dir: &Path) -> Outcome {
    SCRATCH.with(|cell| {
        let scratch = &mut *cell.borrow_mut();
        let out = process_with(item, thumbs_dir, scratch);
        if scratch.jpeg.capacity() > KEEP_JPEG_BYTES {
            scratch.jpeg = Vec::new();
        }
        out
    })
}

fn process_with(item: &repo::PendingImage, thumbs_dir: &Path, scratch: &mut Scratch) -> Outcome {
    let id = item.id;
    let extracted = match raw::extract(Path::new(&item.path), item.format, &mut scratch.jpeg) {
        Ok(x) => x,
        Err(reason) => return Outcome { id, meta: None, result: Err(reason) },
    };
    let orientation = extracted.meta.orientation.unwrap_or(1);
    let preview_path = thumbs_dir.join(format!("{id}_2048.jpg"));
    let thumb_path = thumbs_dir.join(format!("{id}_512.jpg"));
    let result = extracted.preview.and_then(|p| {
        raw::preview::render(p.source(&scratch.jpeg), orientation, &preview_path, &thumb_path, &mut scratch.work)
    });
    let result = result.map(|r| repo::ThumbFiles {
        thumb_path: path_string(&thumb_path),
        preview_path: Some(path_string(&preview_path)),
        width: r.thumb.0,
        height: r.thumb.1,
    });
    Outcome { id, meta: Some(extracted.meta), result }
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
    use crate::ipc::types::{ImageQuery, ImportOptions, ThumbnailState};
    use crate::raw::jpeg::test_support::{quadrant_jpeg, with_exif};
    use crate::raw::tiff::build::{tiff, IfdSpec, Val};
    use std::sync::Mutex;

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

    /// Phase 8: a file that vanished between import and extraction (card pulled, folder
    /// moved) fails with the missing-original message; the pipeline carries on.
    #[test]
    fn vanished_file_fails_with_missing_message() {
        let dir = tempfile::tempdir().unwrap();
        let config = IngestConfig { catalog_path: dir.path().join("cat.sqlite"), cache_dir: dir.path().join("cache") };
        let conn = db::open(&config.catalog_path).unwrap();
        conn.execute_batch(&format!(
            "INSERT INTO folders (id, path, added_at) VALUES (1, '/f', 0);
             INSERT INTO images (id, folder_id, path, file_name, format, camera_make, file_size, file_mtime_ms, imported_at)
             VALUES (1, 1, '{}', 'gone.arw', 'arw', 'sony', 1, 0, 0);
             INSERT INTO thumbnails (image_id) VALUES (1);",
            dir.path().join("gone.arw").display()
        ))
        .unwrap();
        let rec = Recorder::default();
        let stats = run_until_idle(&config, &rec, &AtomicBool::new(true)).unwrap();
        assert_eq!((stats.done, stats.failed), (1, 1));
        let failed = rec.failed.lock().unwrap();
        assert!(failed[0].reason.starts_with(crate::raw::access::MISSING_PREFIX), "{}", failed[0].reason);
    }

    #[derive(Default)]
    struct Recorder {
        ready: Mutex<Vec<ThumbnailReady>>,
        failed: Mutex<Vec<ThumbnailFailed>>,
        progress: Mutex<Vec<ImportProgress>>,
    }

    impl IngestSink for Recorder {
        fn ready(&self, e: ThumbnailReady) {
            self.ready.lock().unwrap().push(e);
        }
        fn failed(&self, e: ThumbnailFailed) {
            self.failed.lock().unwrap().push(e);
        }
        fn progress(&self, e: ImportProgress) {
            self.progress.lock().unwrap().push(e);
        }
    }

    impl IngestSink for Arc<Recorder> {
        fn ready(&self, e: ThumbnailReady) {
            (**self).ready(e)
        }
        fn failed(&self, e: ThumbnailFailed) {
            (**self).failed(e)
        }
        fn progress(&self, e: ImportProgress) {
            (**self).progress(e)
        }
    }

    /// Synthetic Sony-style ARW: IFD0 (orientation, small preview, EXIF), IFD1 thumbnail,
    /// IFD2 large JPEG.
    fn synthetic_arw(orientation: u16) -> Vec<u8> {
        let small = quadrant_jpeg(400, 300);
        let large = quadrant_jpeg(2400, 1600);
        let thumb = quadrant_jpeg(160, 120);
        let ifd0 = IfdSpec {
            tags: vec![
                (0x010F, Val::Ascii("SONY".into())),
                (0x0110, Val::Ascii("ILCE-7M4".into())),
                (0x0112, Val::Short(vec![orientation])),
                (0x0201, Val::BlobOffset(0)),
                (0x0202, Val::Long(vec![small.len() as u32])),
                (0x8769, Val::IfdRef(3)),
            ],
            next: Some(1),
        };
        let ifd1 = IfdSpec {
            tags: vec![(0x0201, Val::BlobOffset(1)), (0x0202, Val::Long(vec![thumb.len() as u32]))],
            next: Some(2),
        };
        let ifd2 = IfdSpec {
            tags: vec![(0x0201, Val::BlobOffset(2)), (0x0202, Val::Long(vec![large.len() as u32]))],
            next: None,
        };
        let exif = IfdSpec {
            tags: vec![
                (0x9003, Val::Ascii("2026:09:26 18:36:04".into())),
                (0x9291, Val::Ascii("106".into())),
                (0x8827, Val::Short(vec![125])),
                (0xA002, Val::Long(vec![4608])),
                (0xA003, Val::Long(vec![3072])),
            ],
            next: None,
        };
        tiff(&[ifd0, ifd1, ifd2, exif], &[small, thumb, large]).0
    }

    fn setup(files: &[(&str, Vec<u8>)]) -> (tempfile::TempDir, IngestConfig) {
        let dir = tempfile::tempdir().unwrap();
        let shoot = dir.path().join("shoot");
        std::fs::create_dir(&shoot).unwrap();
        for (name, bytes) in files {
            std::fs::write(shoot.join(name), bytes).unwrap();
        }
        let config = IngestConfig { catalog_path: dir.path().join("cat.sqlite"), cache_dir: dir.path().join("cache") };
        let mut conn = db::open(&config.catalog_path).unwrap();
        repo::import_folder(&mut conn, &shoot, &ImportOptions::raw_only(false)).unwrap();
        (dir, config)
    }

    #[test]
    fn pipeline_end_to_end_with_orientation() {
        let (exif, _) = tiff(
            &[IfdSpec {
                tags: vec![(0x010F, Val::Ascii("FUJIFILM".into())), (0x0110, Val::Ascii("X-T5".into()))],
                next: None,
            }],
            &[],
        );
        let raf = crate::raw::raf::test_support::raf("X-T5", &with_exif(&quadrant_jpeg(600, 400), &exif), &[]);
        let (_dir, config) = setup(&[
            ("DSC0001.ARW", synthetic_arw(6)),
            ("DSC0002.ARW", synthetic_arw(8)),
            ("DSC0003.ARW", synthetic_arw(1)),
            ("DSCF0004.RAF", raf),
            // Valid header, nothing else: fails (direct parse and LibRaw).
            ("DSC0005.ARW", b"II*\0\x08\0\0\0\0\0".to_vec()),
        ]);

        let rec = Recorder::default();
        let running = AtomicBool::new(true);
        let stats = run_until_idle(&config, &rec, &running).unwrap();
        assert_eq!(stats, RunStats { done: 5, failed: 1 });
        assert!(!running.load(Ordering::SeqCst));

        let conn = db::open(&config.catalog_path).unwrap();
        let page = repo::list_images(
            &conn,
            &ImageQuery { sort: crate::ipc::types::ImageSort::FileName, ..Default::default() },
        )
        .unwrap();
        let by_name = |n: &str| page.items.iter().find(|e| e.file_name == n).unwrap().clone();

        // Portrait orientations come from IFD0 and swap dimensions.
        for (name, o) in [("DSC0001.ARW", 6), ("DSC0002.ARW", 8), ("DSC0003.ARW", 1)] {
            let e = by_name(name);
            assert_eq!(e.orientation, Some(o));
            assert_eq!(e.capture.captured_at_ms, Some(1_790_447_764_106));
            assert_eq!(e.camera.model.as_deref(), Some("ILCE-7M4"));
            assert_eq!((e.width, e.height), (Some(4608), Some(3072)));
            let ThumbnailState::Ready { path, preview_path, width, height } = e.thumbnail else {
                panic!("{name} not ready: {:?}", e.thumbnail)
            };
            let expect_dims = if o == 1 { (512, 341) } else { (341, 512) };
            assert_eq!((width, height), expect_dims, "{name}");
            // The 2400 px IFD2 JPEG (smallest >= 2048) was used, not the 400 px IFD0 one.
            let preview = raw::preview::decode_jpeg(&std::fs::read(preview_path.unwrap()).unwrap(), 10_000).unwrap();
            assert_eq!(preview.width.max(preview.height), 2048);
            let thumb = raw::preview::decode_jpeg(&std::fs::read(path).unwrap(), 10_000).unwrap();
            let tl = &thumb.pixels[(20 * thumb.width as usize + 20) * 3..][..3];
            let red = tl[0] > 127 && tl[1] < 128 && tl[2] < 128;
            let blue = tl[0] < 128 && tl[1] < 128 && tl[2] > 127;
            let green = tl[0] < 128 && tl[1] > 127 && tl[2] < 128;
            match o {
                6 => assert!(blue, "rotated CW: bottom-left (blue) moves to top-left, got {tl:?}"),
                8 => assert!(green, "rotated CCW: top-right (green) moves to top-left, got {tl:?}"),
                _ => assert!(red, "upright, got {tl:?}"),
            }
        }

        let fuji = by_name("DSCF0004.RAF");
        assert_eq!(fuji.camera.sensor_layout, crate::ipc::types::SensorLayout::XTrans);
        assert!(matches!(fuji.thumbnail, ThumbnailState::Ready { width: 512, height: 341, .. }));

        let bad = by_name("DSC0005.ARW");
        assert!(matches!(bad.thumbnail, ThumbnailState::Failed { .. }));

        assert_eq!(rec.ready.lock().unwrap().len(), 4);
        let failed = rec.failed.lock().unwrap();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].image_id, bad.id);
        let progress = rec.progress.lock().unwrap();
        let last = progress.last().unwrap();
        assert_eq!((last.done, last.total, last.failed), (5, 5, 1));
        assert!(progress.windows(2).all(|w| w[0].done <= w[1].done));

        // Idempotent: nothing pending, a second run is a no-op.
        running.store(true, Ordering::SeqCst);
        assert_eq!(run_until_idle(&config, &Recorder::default(), &running).unwrap(), RunStats::default());
    }

    #[test]
    fn regenerate_resets_and_reprocesses() {
        let (_dir, config) = setup(&[("A.ARW", synthetic_arw(1)), ("B.ARW", synthetic_arw(1))]);
        let running = Arc::new(AtomicBool::new(false));
        let rec = Arc::new(Recorder::default());

        // Background worker via the same path `Ingest::start` uses.
        spawn_worker(config.clone(), running.clone(), rec.clone()).unwrap();
        let wait_idle = || {
            let t = Instant::now();
            while running.load(Ordering::SeqCst) {
                assert!(t.elapsed() < Duration::from_secs(60), "worker did not finish");
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        wait_idle();
        assert_eq!(rec.ready.lock().unwrap().len(), 2);

        let mut conn = db::open(&config.catalog_path).unwrap();
        let id = repo::list_images(&conn, &ImageQuery::default()).unwrap().items[0].id;
        assert!(config.thumb_path(id).exists());
        assert_eq!(reset_and_clean(&mut conn, &[id, 424242]).unwrap_err().kind, crate::ipc::error::ErrorKind::NotFound);
        reset_and_clean(&mut conn, &[id]).unwrap();
        assert!(!config.thumb_path(id).exists() && !config.preview_path(id).exists());
        assert_eq!(repo::count_pending(&conn).unwrap(), 1);

        spawn_worker(config.clone(), running.clone(), rec.clone()).unwrap();
        wait_idle();
        assert_eq!(rec.ready.lock().unwrap().len(), 3);
        assert!(config.thumb_path(id).exists());
        assert_eq!(repo::count_pending(&conn).unwrap(), 0);
    }

    /// Real RAWs, read-only; catalog + cache go to a temp dir.
    /// `SIEVE_SAMPLES=/dir cargo test --release -- --ignored real_samples`
    #[test]
    #[ignore = "needs sample RAWs ($SIEVE_SAMPLES)"]
    fn real_samples_first_40() {
        let folder = std::env::var("SIEVE_SAMPLES").unwrap_or_else(|_| "/Users/gurjotsingh/Pictures/test RAWS".into());
        let folder = Path::new(&folder);
        assert!(folder.is_dir(), "{} missing", folder.display());
        let dir = tempfile::tempdir().unwrap();
        let config = IngestConfig { catalog_path: dir.path().join("cat.sqlite"), cache_dir: dir.path().join("cache") };
        let mut conn = db::open(&config.catalog_path).unwrap();
        repo::import_folder(&mut conn, folder, &ImportOptions::raw_only(false)).unwrap();
        conn.execute("DELETE FROM images WHERE id NOT IN (SELECT id FROM images ORDER BY file_name LIMIT 40)", [])
            .unwrap();

        let rec = Recorder::default();
        let stats = run_until_idle(&config, &rec, &AtomicBool::new(true)).unwrap();
        assert!(stats.done > 0);
        assert_eq!(stats.failed, 0, "{:?}", rec.failed.lock().unwrap().first().map(|f| f.reason.clone()));
        let nulls: u32 =
            conn.query_row("SELECT COUNT(*) FROM images WHERE captured_at_ms IS NULL", [], |r| r.get(0)).unwrap();
        assert_eq!(nulls, 0);
        // Thumbnail orientation agrees with EXIF orientation for every image.
        let mismatched: u32 = conn
            .query_row(
                "SELECT COUNT(*) FROM images i JOIN thumbnails t ON t.image_id = i.id
                 WHERE (i.orientation BETWEEN 5 AND 8) != (t.height > t.width)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(mismatched, 0);
    }

    /// v9: JPEG/PNG (and HEIC/TIFF where ImageIO exists) ingest with EXIF + oriented
    /// thumbnails; a camera JPEG next to its RAW becomes the RAW's companion.
    #[test]
    fn non_raw_sources_and_companions() {
        use crate::raw::raster::tests::fixtures;
        let dir = tempfile::tempdir().unwrap();
        let shoot = dir.path().join("shoot");
        std::fs::create_dir(&shoot).unwrap();
        std::fs::write(shoot.join("DSC0001.ARW"), synthetic_arw(1)).unwrap();
        std::fs::write(shoot.join("DSC0001.JPG"), fixtures::camera_jpeg(64, 48, 1, None)).unwrap();
        std::fs::write(shoot.join("IMG_0002.JPG"), fixtures::camera_jpeg(640, 480, 6, None)).unwrap();
        let png = crate::raw::png::test_support::encode(
            300,
            200,
            None,
            Some(&fixtures::gradient(300, 200)),
            None,
            None,
            None,
        );
        std::fs::write(shoot.join("scan.png"), png).unwrap();
        let config = IngestConfig { catalog_path: dir.path().join("cat.sqlite"), cache_dir: dir.path().join("cache") };
        let mut conn = db::open(&config.catalog_path).unwrap();
        let opts = ImportOptions { recursive: false, include_non_raw: true, pair_jpeg_with_raw: true };
        let summary = repo::import_folder(&mut conn, &shoot, &opts).unwrap();
        assert_eq!((summary.added, summary.companions), (3, 1));

        let rec = Recorder::default();
        let running = AtomicBool::new(true);
        let stats = run_until_idle(&config, &rec, &running).unwrap();
        assert_eq!(stats, RunStats { done: 3, failed: 0 }, "{:?}", rec.failed.lock().unwrap());

        let page = repo::list_images(
            &conn,
            &ImageQuery { sort: crate::ipc::types::ImageSort::FileName, ..Default::default() },
        )
        .unwrap();
        let by_name = |n: &str| page.items.iter().find(|e| e.file_name == n).unwrap().clone();
        let raw = by_name("DSC0001.ARW");
        assert!(raw.companion_path.as_deref().is_some_and(|p| p.ends_with("DSC0001.JPG")));
        let jpg = by_name("IMG_0002.JPG");
        assert_eq!(jpg.format, crate::ipc::types::ImageFormat::Jpeg);
        assert_eq!(jpg.orientation, Some(6));
        assert_eq!((jpg.width, jpg.height), (Some(640), Some(480)));
        assert_eq!(jpg.camera.model.as_deref(), Some("X-T5"));
        assert_eq!(jpg.camera.sensor_layout, crate::ipc::types::SensorLayout::Unknown);
        assert!(jpg.capture.captured_at_ms.is_some());
        assert!(matches!(jpg.thumbnail, ThumbnailState::Ready { width: 384, height: 512, .. }), "{:?}", jpg.thumbnail);
        let scan = by_name("scan.png");
        assert_eq!(scan.camera.make, crate::ipc::types::CameraMake::Other);
        assert!(
            matches!(scan.thumbnail, ThumbnailState::Ready { width: 300, height: 200, .. }),
            "{:?}",
            scan.thumbnail
        );
    }
}
