//! Background analysis worker (mirrors `ingest`): one thread with its own SQLite
//! connection pulls images needing analysis in batches, measures them on a small rayon
//! pool (one [`Analyzer`] per pool thread, loaded lazily), and does all DB writes and
//! events itself. While ingest is running it waits for more previews. When the queue
//! drains (and ingest is idle) it rescores every analyzed image from stored metrics and
//! regroups bursts in one transaction, then emits `AnalysisFinished`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use rayon::prelude::*;
use rusqlite::Connection;
use tauri::{AppHandle, Manager, Runtime};
use tauri_specta::Event;

use super::bursts::{apply_pins, demote, group_bursts};
use super::store::{self, BurstRow};
use super::{score, AnalysisConfig, Analyzer, BurstFrame, ImageMetrics, WorkerFlags};
use crate::db::{self, now_ms, repo};
use crate::ingest::Ingest;
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::events::{AnalysisFailed, AnalysisFinished, AnalysisProgress, AnalysisReady};
use crate::ipc::types::{AnalysisScope, ImageId};

/// Images fetched per round trip.
const BATCH: u32 = 32;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(150);
/// Poll interval while waiting for ingest to produce previews.
const INGEST_POLL: Duration = Duration::from_millis(250);
/// Overrides the analysis thread count.
const THREADS_ENV: &str = "SIEVE_ANALYSIS_THREADS";

/// A quarter of the cores, 1..=4: the ONNX sessions run on CoreML (GPU/ANE, serialized
/// by the OS anyway); CPU work per image is decode + sharpness. Measured on the 16-core
/// M3 Max: see `docs/decisions.md`.
pub fn default_threads() -> usize {
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    (cores / 4).clamp(1, 4)
}

/// Receives worker events; implemented for `AppHandle` and by tests / the eval tool.
pub trait AnalysisSink: Send + Sync {
    fn ready(&self, event: AnalysisReady);
    fn failed(&self, event: AnalysisFailed);
    fn progress(&self, event: AnalysisProgress);
    fn finished(&self, event: AnalysisFinished);
    /// Whether the ingest pipeline is still producing previews.
    fn ingest_running(&self) -> bool;
}

impl<R: Runtime> AnalysisSink for AppHandle<R> {
    fn ready(&self, e: AnalysisReady) {
        let _ = e.emit(self);
    }
    fn failed(&self, e: AnalysisFailed) {
        let _ = e.emit(self);
    }
    fn progress(&self, e: AnalysisProgress) {
        let _ = e.emit(self);
    }
    fn finished(&self, e: AnalysisFinished) {
        let _ = e.emit(self);
    }
    fn ingest_running(&self) -> bool {
        self.try_state::<Ingest>().is_some_and(|i| i.is_running())
    }
}

/// `Analysis::start`: queue `scope`, clear cancel, spawn the worker if idle.
pub fn kick<S: AnalysisSink + 'static>(
    config: &AnalysisConfig,
    flags: &WorkerFlags,
    scope: AnalysisScope,
    sink: S,
) -> AppResult<()> {
    if !matches!(scope, AnalysisScope::Pending | AnalysisScope::Rescore) {
        let mut conn = db::open(&config.catalog_path)?;
        store::queue_scope(&mut conn, &scope)?;
    }
    if scope == AnalysisScope::Rescore {
        flags.rescore.store(true, Ordering::SeqCst);
    }
    flags.cancel.store(false, Ordering::SeqCst);
    spawn_worker(config.clone(), flags.clone(), sink)
}

/// Spawns the worker unless one is running (the `running` flag is claimed here).
pub fn spawn_worker<S: AnalysisSink + 'static>(config: AnalysisConfig, flags: WorkerFlags, sink: S) -> AppResult<()> {
    if flags.running.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return Ok(());
    }
    let running = flags.running.clone();
    let spawned = std::thread::Builder::new().name("sieve-analysis".into()).spawn(move || {
        if let Err(e) = run_until_idle(&config, &sink, &flags) {
            eprintln!("[analysis] worker stopped: {}", e.message);
            flags.running.store(false, Ordering::SeqCst);
        }
    });
    if let Err(e) = spawned {
        running.store(false, Ordering::SeqCst);
        return Err(AppError::internal(format!("spawn analysis worker: {e}")));
    }
    Ok(())
}

/// Counters for one run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunStats {
    pub analyzed: u32,
    pub failed: u32,
    pub cancelled: bool,
    pub burst_groups: u32,
}

/// Lazily loaded analyzers shared by the pool threads (one in use per thread).
struct AnalyzerPool<'a> {
    models_dir: &'a Path,
    free: Mutex<Vec<Analyzer>>,
}

impl AnalyzerPool<'_> {
    fn with<T>(&self, f: impl FnOnce(&mut Analyzer) -> Result<T, String>) -> Result<T, String> {
        let taken = self.free.lock().map_err(|_| "analyzer pool poisoned".to_string())?.pop();
        let mut a = match taken {
            Some(a) => a,
            None => Analyzer::load(self.models_dir)?,
        };
        let out = f(&mut a);
        if let Ok(mut free) = self.free.lock() {
            free.push(a);
        }
        out
    }
}

struct Outcome {
    id: ImageId,
    result: Result<ImageMetrics, String>,
}

/// Processes the queue until it is empty and ingest is idle (or cancelled), then runs
/// the rescore/burst pass and clears `running`. The caller must have claimed `running`.
pub fn run_until_idle(config: &AnalysisConfig, sink: &dyn AnalysisSink, flags: &WorkerFlags) -> AppResult<RunStats> {
    let mut conn = db::open(&config.catalog_path)?;
    let threads = std::env::var(THREADS_ENV)
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n: &usize| n > 0)
        .unwrap_or_else(default_threads);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("sieve-analyze-{i}"))
        .build()
        .map_err(|e| AppError::internal(format!("thread pool: {e}")))?;
    let analyzers = AnalyzerPool { models_dir: &config.models_dir, free: Mutex::new(Vec::new()) };

    let mut stats = RunStats::default();
    let mut progress = Progress { done: 0, failed: 0, last: None };
    let mut dirty = false;
    let mut models_ok = false;
    loop {
        if flags.cancel.load(Ordering::SeqCst) {
            stats.cancelled = true;
            break;
        }
        let batch = store::needs_analysis(&conn, BATCH)?;
        if batch.is_empty() {
            if sink.ingest_running() {
                std::thread::sleep(INGEST_POLL);
                continue;
            }
            if dirty || flags.rescore.swap(false, Ordering::SeqCst) {
                stats.burst_groups = rescore_all(&mut conn)?;
                dirty = false;
            }
            flags.running.store(false, Ordering::SeqCst);
            // Close the race with a `start` that saw `running == true` just before we
            // cleared it: reclaim the flag if work appeared.
            let more = store::count_needs(&conn)? > 0 || flags.rescore.load(Ordering::SeqCst);
            if more && flags.running.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                continue;
            }
            break;
        }
        if !models_ok {
            // Fail fast (without touching rows) when the models are missing: the images
            // stay pending and are picked up once `scripts/fetch-models.sh` has run.
            if let Err(e) = analyzers.with(|a| {
                eprintln!("[analysis] execution providers (detector, landmarks): {:?}", a.providers());
                Ok(())
            }) {
                eprintln!("[analysis] models unavailable, analysis stopped: {e}");
                flags.running.store(false, Ordering::SeqCst);
                stats.cancelled = true;
                sink.finished(AnalysisFinished { analyzed: 0, failed: 0, cancelled: true, burst_groups: 0 });
                return Ok(stats);
            }
            models_ok = true;
        }
        progress.maybe_emit(&conn, sink, true)?;

        let (shoot_type, thresholds) = {
            let st = repo::shoot_type(&conn)?;
            (st, repo::cull_thresholds(&conn, st)?)
        };
        let (tx, rx) = mpsc::channel::<Outcome>();
        let cancel = &flags.cancel;
        std::thread::scope(|s| -> AppResult<()> {
            let batch = &batch;
            let pool = &pool;
            let analyzers = &analyzers;
            s.spawn(move || {
                pool.install(|| {
                    batch.par_iter().for_each_with(tx, |tx, (id, path)| {
                        if cancel.load(Ordering::SeqCst) {
                            return;
                        }
                        let result = analyzers.with(|a| a.measure(Path::new(path)));
                        let _ = tx.send(Outcome { id: *id, result });
                    })
                })
            });
            for out in rx {
                match out.result {
                    Ok(metrics) => {
                        let scored = score(&metrics, &thresholds, shoot_type);
                        store::record_measured(&mut conn, out.id, &metrics, &scored)?;
                        stats.analyzed += 1;
                        sink.ready(AnalysisReady { image_id: out.id });
                    }
                    Err(reason) => {
                        store::record_failed(&mut conn, out.id, &reason)?;
                        stats.failed += 1;
                        progress.failed += 1;
                        sink.failed(AnalysisFailed { image_id: out.id, reason });
                    }
                }
                dirty = true;
                progress.done += 1;
                progress.maybe_emit(&conn, sink, false)?;
            }
            Ok(())
        })?;
    }
    if stats.cancelled {
        flags.running.store(false, Ordering::SeqCst);
    } else if stats.burst_groups == 0 {
        stats.burst_groups = conn.query_row("SELECT COUNT(*) FROM burst_groups", [], |r| r.get(0))?;
    }
    sink.progress(AnalysisProgress { done: progress.done, total: progress.done, failed: progress.failed });
    sink.finished(AnalysisFinished {
        analyzed: stats.analyzed,
        failed: stats.failed,
        cancelled: stats.cancelled,
        burst_groups: stats.burst_groups,
    });
    Ok(stats)
}

struct Progress {
    done: u32,
    failed: u32,
    last: Option<Instant>,
}

impl Progress {
    fn maybe_emit(&mut self, conn: &Connection, sink: &dyn AnalysisSink, force: bool) -> AppResult<()> {
        if force || self.last.is_none_or(|t| t.elapsed() >= PROGRESS_INTERVAL) {
            // Images in flight still count as needing analysis.
            let pending = store::count_needs(conn)?;
            sink.progress(AnalysisProgress { done: self.done, total: self.done + pending, failed: self.failed });
            self.last = Some(Instant::now());
        }
        Ok(())
    }
}

/// No-ML pass over every analyzed image: score with the current shoot type and
/// thresholds, regroup bursts per folder, demote non-keepers, write everything in one
/// transaction. Returns the number of burst groups.
pub fn rescore_all(conn: &mut Connection) -> AppResult<u32> {
    let shoot_type = repo::shoot_type(conn)?;
    let thresholds = repo::cull_thresholds(conn, shoot_type)?;
    let window: u32 = conn
        .query_row("SELECT value FROM catalog_meta WHERE key = 'burst_window_ms'", [], |r| r.get::<_, String>(0))?
        .parse()
        .unwrap_or(1500);
    let analyzed = store::load_analyzed(conn)?;
    let mut scored: Vec<_> = analyzed.iter().map(|a| score(&a.metrics, &thresholds, shoot_type)).collect();

    let mut by_folder: BTreeMap<i64, Vec<(BurstFrame, usize)>> = BTreeMap::new();
    for (i, a) in analyzed.iter().enumerate() {
        if let Some(t) = a.captured_at_ms {
            let frame =
                BurstFrame { id: a.id, captured_at_ms: t, phash: a.metrics.phash, overall: scored[i].quality.overall };
            by_folder.entry(a.folder_id).or_default().push((frame, i));
        }
    }
    let pins = store::pinned_keepers(conn)?;
    let mut rows = Vec::new();
    for frames in by_folder.values() {
        let index: std::collections::HashMap<ImageId, usize> = frames.iter().map(|(f, i)| (f.id, *i)).collect();
        let times: std::collections::HashMap<ImageId, i64> =
            frames.iter().map(|(f, _)| (f.id, f.captured_at_ms)).collect();
        let plain: Vec<BurstFrame> = frames.iter().map(|(f, _)| *f).collect();
        for mut b in group_bursts(&plain, window, thresholds.burst_hash_distance) {
            apply_pins(&mut b, &pins, |m| scored[index[&m]].quality.overall);
            let keeper_rating = scored[index[&b.keeper]].quality.suggested_rating;
            for &m in &b.members {
                if m != b.keeper {
                    demote(&mut scored[index[&m]].quality, keeper_rating);
                }
            }
            rows.push(BurstRow {
                started_at_ms: times[&b.members[0]],
                ended_at_ms: times[b.members.last().expect("burst has members")],
                members: b.members,
                keeper: b.keeper,
            });
        }
    }

    let tx = conn.transaction()?;
    let now = now_ms();
    for (a, s) in analyzed.iter().zip(&scored) {
        store::write_scored(&tx, a.id, s, now)?;
    }
    let n = store::write_bursts(&tx, &rows)?;
    tx.commit()?;
    Ok(n)
}

/// Runs the worker synchronously on the calling thread (eval tool, tests).
pub fn run_blocking(config: &AnalysisConfig, sink: &dyn AnalysisSink) -> AppResult<RunStats> {
    let flags = WorkerFlags::default();
    flags.running.store(true, Ordering::SeqCst);
    run_until_idle(config, sink, &flags)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ml::scoring::tests::{face, metrics};
    use crate::ml::MODEL_VERSION;
    use std::path::PathBuf;
    use std::sync::Arc;

    #[derive(Default)]
    struct Recorder {
        ready: Mutex<Vec<ImageId>>,
        failed: Mutex<Vec<AnalysisFailed>>,
        progress: Mutex<Vec<AnalysisProgress>>,
        finished: Mutex<Vec<AnalysisFinished>>,
    }

    impl AnalysisSink for Recorder {
        fn ready(&self, e: AnalysisReady) {
            self.ready.lock().unwrap().push(e.image_id);
        }
        fn failed(&self, e: AnalysisFailed) {
            self.failed.lock().unwrap().push(e);
        }
        fn progress(&self, e: AnalysisProgress) {
            self.progress.lock().unwrap().push(e);
        }
        fn finished(&self, e: AnalysisFinished) {
            self.finished.lock().unwrap().push(e);
        }
        fn ingest_running(&self) -> bool {
            false
        }
    }

    impl AnalysisSink for Arc<Recorder> {
        fn ready(&self, e: AnalysisReady) {
            (**self).ready(e)
        }
        fn failed(&self, e: AnalysisFailed) {
            (**self).failed(e)
        }
        fn progress(&self, e: AnalysisProgress) {
            (**self).progress(e)
        }
        fn finished(&self, e: AnalysisFinished) {
            (**self).finished(e)
        }
        fn ingest_running(&self) -> bool {
            false
        }
    }

    /// Catalog with `n` images (1-based ids) captured 500 ms apart, previews at
    /// `previews[i]` (or a non-existent path).
    fn catalog(dir: &Path, n: i64, previews: &[PathBuf]) -> AnalysisConfig {
        let config = AnalysisConfig { catalog_path: dir.join("cat.sqlite"), models_dir: dir.join("no-models") };
        let conn = db::open(&config.catalog_path).unwrap();
        conn.execute("INSERT INTO folders (id, path, added_at) VALUES (1, '/f', 0)", []).unwrap();
        for id in 1..=n {
            let preview =
                previews.get(id as usize - 1).map_or_else(|| dir.join(format!("{id}_2048.jpg")), |p| p.clone());
            conn.execute(
                "INSERT INTO images (id, folder_id, path, file_name, format, camera_make, file_size, file_mtime_ms,
                     imported_at, captured_at_ms, rating, pick)
                 VALUES (?1, 1, '/f/' || ?1 || '.arw', ?1 || '.arw', 'arw', 'sony', 1, 0, 0, ?2, 2, 'pick')",
                params![id, 1_000_000 + id * 500],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO thumbnails (image_id, status, path, preview_path, extracted_at)
                 VALUES (?1, 'ready', 'x', ?2, 1)",
                params![id, preview.to_string_lossy()],
            )
            .unwrap();
        }
        config
    }

    use rusqlite::params;

    fn tags_of(conn: &Connection, id: ImageId) -> Vec<String> {
        conn.prepare("SELECT tag FROM image_tags WHERE image_id = ?1 AND suppressed = 0 ORDER BY tag")
            .unwrap()
            .query_map([id], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    fn store_metrics(conn: &Connection, id: ImageId, m: &ImageMetrics) {
        conn.execute(
            "INSERT INTO image_analysis (image_id, status, model_version, analyzed_at, phash, metrics_json)
             VALUES (?1, 'done', ?2, 10, ?3, ?4)",
            params![id, MODEL_VERSION, m.phash as i64, serde_json::to_string(m).unwrap()],
        )
        .unwrap();
    }

    #[test]
    fn rescore_runs_without_models_and_groups_bursts() {
        let dir = tempfile::tempdir().unwrap();
        let config = catalog(dir.path(), 4, &[]);
        let conn = db::open(&config.catalog_path).unwrap();
        repo::set_shoot_type(&conn, crate::ipc::types::ShootType::Wedding).unwrap();
        // 1+2: same scene 500 ms apart (2 sharper -> keeper). 3: blink, different scene.
        // 4: same scene as 1/2 but closed eyes, 500 ms after 3 -> hash differs from 3.
        let mut m1 = metrics(vec![face(0.4, 0.15, 0.6, 0.28)]);
        m1.phash = 0xFFFF_0000;
        let mut m2 = metrics(vec![face(0.4, 0.15, 0.75, 0.28)]);
        m2.phash = 0xFFFF_0001;
        let mut m3 = metrics(vec![face(0.4, 0.15, 0.7, 0.05)]);
        m3.phash = !0xFFFF_0000;
        let mut m4 = metrics(vec![face(0.4, 0.15, 0.7, 0.05)]);
        m4.phash = 0xFFFF_0000;
        for (id, m) in [(1, &m1), (2, &m2), (3, &m3), (4, &m4)] {
            store_metrics(&conn, id, m);
        }
        // A suppressed auto tag and a user tag must survive.
        conn.execute_batch(
            "INSERT INTO image_tags (image_id, tag, source, confidence, suppressed) VALUES
             (3, 'blink', 'auto', 0.9, 1), (2, 'motion_blur', 'user', 1.0, 0);",
        )
        .unwrap();

        let rec = Arc::new(Recorder::default());
        let flags = WorkerFlags::default();
        kick(&config, &flags, AnalysisScope::Rescore, rec.clone()).unwrap();
        let t = Instant::now();
        while flags.running.load(Ordering::SeqCst) {
            assert!(t.elapsed() < Duration::from_secs(30));
            std::thread::sleep(Duration::from_millis(5));
        }
        let fin = rec.finished.lock().unwrap().clone();
        assert_eq!(fin.len(), 1);
        assert_eq!((fin[0].analyzed, fin[0].failed, fin[0].cancelled, fin[0].burst_groups), (0, 0, false, 1));

        let groups = repo::list_burst_groups(&conn, None).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!((groups[0].image_ids.clone(), groups[0].keeper_image_id), (vec![1, 2], Some(2)));
        assert_eq!(tags_of(&conn, 1), vec!["duplicate_burst"]);
        assert_eq!(tags_of(&conn, 2), vec!["motion_blur"], "user tag kept, keeper not duplicate");
        assert!(tags_of(&conn, 3).is_empty(), "suppressed blink not resurrected");
        assert_eq!(tags_of(&conn, 4), vec!["blink"]);
        let (pick1, pick2): (String, String) = conn
            .query_row(
                "SELECT (SELECT suggested_pick FROM quality_scores WHERE image_id = 1),
                        (SELECT suggested_pick FROM quality_scores WHERE image_id = 2)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        // Burst non-keepers are not rejected for being duplicates (never a pick either).
        assert_eq!(pick1, "unflagged");
        assert_ne!(pick2, "reject");
        // The engine never writes the user's rating / pick.
        let untouched: u32 =
            conn.query_row("SELECT COUNT(*) FROM images WHERE rating = 2 AND pick = 'pick'", [], |r| r.get(0)).unwrap();
        assert_eq!(untouched, 4);
        let faces = repo::get_faces(&conn, 4).unwrap();
        assert!(faces[0].blink && faces[0].primary);

        // A user-chosen keeper survives regrouping, and suggestions follow it.
        store::set_burst_keeper(&mut db::open(&config.catalog_path).unwrap(), groups[0].id, 1).unwrap();
        rescore_all(&mut db::open(&config.catalog_path).unwrap()).unwrap();
        let groups = repo::list_burst_groups(&conn, None).unwrap();
        assert_eq!(groups[0].keeper_image_id, Some(1));
        assert!(tags_of(&conn, 1).is_empty());
        assert_eq!(tags_of(&conn, 2), vec!["duplicate_burst", "motion_blur"]);
        let (pick1, stars1, pick2, stars2): (String, u8, String, u8) = conn
            .query_row(
                "SELECT a.suggested_pick, a.suggested_rating, b.suggested_pick, b.suggested_rating
                 FROM quality_scores a, quality_scores b WHERE a.image_id = 1 AND b.image_id = 2",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(pick2, "unflagged");
        assert!(stars2 < stars1 && pick1 != "reject", "{pick1} {stars1} / {pick2} {stars2}");

        // Narrower burst window (rescore) dissolves the group and its duplicate tag.
        repo::set_burst_window(&conn, 400).unwrap();
        assert_eq!(rescore_all(&mut db::open(&config.catalog_path).unwrap()).unwrap(), 0);
        assert!(tags_of(&conn, 1).is_empty());
    }

    #[test]
    fn missing_models_leave_images_pending() {
        let dir = tempfile::tempdir().unwrap();
        let config = catalog(dir.path(), 2, &[]);
        let rec = Recorder::default();
        let stats = run_blocking(&config, &rec).unwrap();
        assert!(stats.cancelled);
        let conn = db::open(&config.catalog_path).unwrap();
        assert_eq!(store::count_needs(&conn).unwrap(), 2);
        let s = store::analysis_status(&conn, false).unwrap();
        assert_eq!((s.pending, s.failed, s.analyzed), (2, 0, 0));
        assert!(rec.finished.lock().unwrap()[0].cancelled);
    }

    /// Real previews from the Phase 2 bench cache + real models.
    /// `cargo test --release -- --ignored real_previews`
    #[test]
    #[ignore = "needs models (scripts/fetch-models.sh) and test-data previews"]
    fn real_previews_end_to_end() {
        let models = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models");
        let thumbs = Path::new("/Users/gurjotsingh/Documents/GitHub/Sieve/test-data/qa-phase2/cache/thumbs");
        let dir = tempfile::tempdir().unwrap();
        let mut previews = Vec::new();
        for id in [1, 2, 3, 36, 43, 54, 144, 218] {
            let dst = dir.path().join(format!("p{id}.jpg"));
            std::fs::copy(thumbs.join(format!("{id}_2048.jpg")), &dst).unwrap();
            previews.push(dst);
        }
        std::fs::write(dir.path().join("broken.jpg"), b"not a jpeg").unwrap();
        previews.push(dir.path().join("broken.jpg"));
        let mut config = catalog(dir.path(), previews.len() as i64, &previews);
        config.models_dir = models;

        let rec = Recorder::default();
        let stats = run_blocking(&config, &rec).unwrap();
        assert_eq!((stats.analyzed, stats.failed, stats.cancelled), (8, 1, false));
        assert_eq!(rec.ready.lock().unwrap().len(), 8);
        assert_eq!(rec.failed.lock().unwrap()[0].image_id, 9);
        let last = rec.progress.lock().unwrap().last().unwrap().clone();
        assert_eq!((last.done, last.total, last.failed), (9, 9, 1));

        let conn = db::open(&config.catalog_path).unwrap();
        let s = store::analysis_status(&conn, false).unwrap();
        assert_eq!((s.analyzed, s.failed, s.pending, s.waiting), (8, 1, 0, 0));
        let scored: u32 = conn.query_row("SELECT COUNT(*) FROM quality_scores", [], |r| r.get(0)).unwrap();
        assert_eq!(scored, 8);
        let with_faces: u32 = conn
            .query_row("SELECT COUNT(*) FROM image_analysis WHERE faces_json LIKE '%primary%'", [], |r| r.get(0))
            .unwrap();
        assert!(with_faces >= 4, "{with_faces}");

        // Idempotent; a forced re-measure of one image runs just that one.
        let stats = run_blocking(&config, &Recorder::default()).unwrap();
        assert_eq!((stats.analyzed, stats.failed), (0, 0));
        let mut conn = db::open(&config.catalog_path).unwrap();
        store::queue_scope(&mut conn, &AnalysisScope::Images { ids: vec![4] }).unwrap();
        let stats = run_blocking(&config, &Recorder::default()).unwrap();
        assert_eq!((stats.analyzed, stats.failed), (1, 0));
    }
}
