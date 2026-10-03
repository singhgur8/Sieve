//! Background analysis worker (mirrors `ingest`): one thread with its own SQLite
//! connection pulls images needing analysis in batches, measures them on a small rayon
//! pool (one [`Analyzer`] per pool thread, loaded lazily), and does all DB writes and
//! events itself. While ingest is running it waits for more previews. When the queue
//! drains (and ingest is idle) it rescores every analyzed image from stored metrics and
//! regroups bursts in one transaction, then emits `AnalysisFinished`.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use rayon::prelude::*;
use rusqlite::{Connection, TransactionBehavior};
use tauri::{AppHandle, Manager, Runtime};
use tauri_specta::Event;

use super::bursts::{add_reason, annotate_soft_face, apply_pins, demote, duplicate_reason, group_bursts};
use super::store::{self, BurstRow};
use super::{score, AnalysisConfig, Analyzer, BurstFrame, ImageMetrics, WorkerFlags};
use crate::db::{self, now_ms, projects, repo};
use crate::ingest::Ingest;
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::events::{AnalysisFailed, AnalysisFinished, AnalysisProgress, AnalysisReady};
use crate::ipc::types::{AnalysisScope, CullThresholds, ImageId, ShootType};

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
        // Background-activity indicator (IPC v18): one activity per worker run, from the first
        // image counted until `AnalysisFinished` (burst grouping included).
        if let Some(a) = crate::ipc::activity::activities(self) {
            if e.total > 0 {
                let kind = crate::ipc::events::ActivityKind::Analysis;
                a.progress("analysis", kind, "Analysing photos", e.done, Some(e.total));
            }
        }
        let _ = e.emit(self);
    }
    fn finished(&self, e: AnalysisFinished) {
        if let Some(a) = crate::ipc::activity::activities(self) {
            use crate::ipc::activity::photos;
            use crate::ipc::events::ActivityState;
            let (state, message) = match (e.cancelled, e.failed) {
                (true, _) => (ActivityState::Cancelled, format!("Stopped after {}", photos(e.analyzed))),
                (false, 0) => (ActivityState::Finished, format!("Analysed {}", photos(e.analyzed))),
                (false, f) => (ActivityState::Finished, format!("Analysed {}; {f} failed", photos(e.analyzed))),
            };
            a.finish("analysis", state, Some(message));
        }
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

/// Measures previews on the pool threads (the real [`AnalyzerPool`]; a stub in tests).
trait Measure: Sync {
    /// Fails when the models cannot be loaded (checked once, before the first batch).
    fn check(&self) -> Result<(), String>;
    fn measure(&self, preview: &Path) -> Result<ImageMetrics, String>;
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

impl Measure for AnalyzerPool<'_> {
    fn check(&self) -> Result<(), String> {
        self.with(|a| {
            eprintln!("[analysis] execution providers (detector, landmarks): {:?}", a.providers());
            Ok(())
        })
    }

    fn measure(&self, preview: &Path) -> Result<ImageMetrics, String> {
        self.with(|a| a.measure(preview))
    }
}

struct Outcome {
    id: ImageId,
    result: Result<ImageMetrics, String>,
}

/// How the queue loop ended (when it did not fail).
enum End {
    Idle,
    ModelsMissing,
}

/// Processes the queue until it is empty and ingest is idle (or cancelled), then runs
/// the rescore/burst pass and clears `running`. The caller must have claimed `running`.
/// Emits exactly one `AnalysisFinished`, also when the pass fails (then `cancelled`).
pub fn run_until_idle(config: &AnalysisConfig, sink: &dyn AnalysisSink, flags: &WorkerFlags) -> AppResult<RunStats> {
    let analyzers = AnalyzerPool { models_dir: &config.models_dir, free: Mutex::new(Vec::new()) };
    run_with(config, sink, flags, &analyzers)
}

fn run_with(
    config: &AnalysisConfig,
    sink: &dyn AnalysisSink,
    flags: &WorkerFlags,
    measurer: &dyn Measure,
) -> AppResult<RunStats> {
    let mut stats = RunStats::default();
    let mut progress = Progress { done: 0, failed: 0, last: None };
    let mut conn = None;
    let end = db::open(&config.catalog_path)
        .and_then(|c| run_queue(conn.insert(c), sink, flags, measurer, &mut stats, &mut progress));
    if let Err(e) = &end {
        eprintln!("[analysis] pass failed: {}", e.message);
        stats.cancelled = true;
    }
    if stats.cancelled {
        flags.running.store(false, Ordering::SeqCst);
    } else if stats.burst_groups == 0 {
        if let Some(conn) = &conn {
            stats.burst_groups = conn.query_row("SELECT COUNT(*) FROM burst_groups", [], |r| r.get(0)).unwrap_or(0);
        }
    }
    if !matches!(end, Ok(End::ModelsMissing)) {
        sink.progress(AnalysisProgress { done: progress.done, total: progress.done, failed: progress.failed });
    }
    sink.finished(AnalysisFinished {
        analyzed: stats.analyzed,
        failed: stats.failed,
        cancelled: stats.cancelled,
        burst_groups: stats.burst_groups,
    });
    end.map(|_| stats)
}

/// The queue loop of [`run_until_idle`] (no `AnalysisFinished`; the caller emits it).
/// Results of images removed from the catalog mid-pass are dropped: they count neither
/// as done nor as analyzed / failed, and emit no per-image event.
fn run_queue(
    conn: &mut Connection,
    sink: &dyn AnalysisSink,
    flags: &WorkerFlags,
    measurer: &dyn Measure,
    stats: &mut RunStats,
    progress: &mut Progress,
) -> AppResult<End> {
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

    let mut dirty = false;
    let mut models_ok = false;
    loop {
        if flags.cancel.load(Ordering::SeqCst) {
            stats.cancelled = true;
            return Ok(End::Idle);
        }
        let batch = store::needs_analysis(conn, BATCH)?;
        if batch.is_empty() {
            if sink.ingest_running() {
                std::thread::sleep(INGEST_POLL);
                continue;
            }
            if dirty || flags.rescore.swap(false, Ordering::SeqCst) {
                stats.burst_groups = rescore_all(conn)?;
                dirty = false;
            }
            flags.running.store(false, Ordering::SeqCst);
            // Close the race with a `start` that saw `running == true` just before we
            // cleared it: reclaim the flag if work appeared.
            let more = store::count_needs(conn)? > 0 || flags.rescore.load(Ordering::SeqCst);
            if more && flags.running.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                continue;
            }
            return Ok(End::Idle);
        }
        if !models_ok {
            // Fail fast (without touching rows) when the models are missing: the images
            // stay pending and are picked up once `scripts/fetch-models.sh` has run.
            if let Err(e) = measurer.check() {
                eprintln!("[analysis] models unavailable, analysis stopped: {e}");
                stats.cancelled = true;
                return Ok(End::ModelsMissing);
            }
            models_ok = true;
        }
        progress.maybe_emit(conn, sink, true)?;

        // Each image is scored with its project's shoot type (IPC v14).
        let mut shoot = ShootTypes::default();
        for (id, _) in &batch {
            shoot.of_image(conn, *id)?;
        }
        let (tx, rx) = mpsc::channel::<Outcome>();
        let cancel = &flags.cancel;
        std::thread::scope(|s| -> AppResult<()> {
            let batch = &batch;
            let pool = &pool;
            s.spawn(move || {
                pool.install(|| {
                    batch.par_iter().for_each_with(tx, |tx, (id, path)| {
                        if cancel.load(Ordering::SeqCst) {
                            return;
                        }
                        let result = measurer.measure(Path::new(path));
                        let _ = tx.send(Outcome { id: *id, result });
                    })
                })
            });
            for out in rx {
                let recorded = match out.result {
                    Ok(metrics) => {
                        let (shoot_type, thresholds) = shoot.of_image(conn, out.id)?;
                        let scored = score(&metrics, &thresholds, shoot_type);
                        let recorded = store::record_measured(conn, out.id, &metrics, &scored)?;
                        if recorded {
                            stats.analyzed += 1;
                            sink.ready(AnalysisReady { image_id: out.id });
                        }
                        recorded
                    }
                    Err(reason) => {
                        let recorded = store::record_failed(conn, out.id, &reason)?;
                        if recorded {
                            stats.failed += 1;
                            progress.failed += 1;
                            sink.failed(AnalysisFailed { image_id: out.id, reason });
                        }
                        recorded
                    }
                };
                // An image removed mid-pass (e.g. `remove_project`) is neither done nor
                // reported; the closing rescore still runs to regroup what remains.
                dirty = true;
                progress.done += u32::from(recorded);
                progress.maybe_emit(conn, sink, false)?;
            }
            Ok(())
        })?;
    }
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
/// transaction. Returns the number of burst groups. The transaction is immediate (holds
/// the write lock while reading) so images removed concurrently are never written back.
pub fn rescore_all(conn: &mut Connection) -> AppResult<u32> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let conn: &Connection = &tx;
    let mut shoot = ShootTypes::default();
    let window: u32 = conn
        .query_row("SELECT value FROM catalog_meta WHERE key = 'burst_window_ms'", [], |r| r.get::<_, String>(0))?
        .parse()
        .unwrap_or(1500);
    let analyzed = store::load_analyzed(conn)?;
    let mut scored = Vec::with_capacity(analyzed.len());
    for a in &analyzed {
        let (shoot_type, thresholds) = shoot.of_folder(conn, a.folder_id, a.id)?;
        scored.push(score(&a.metrics, &thresholds, shoot_type));
    }

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
    for (folder, frames) in &by_folder {
        let Some((first, _)) = frames.first() else { continue };
        let burst_hash_distance = shoot.of_folder(conn, *folder, first.id)?.1.burst_hash_distance;
        let index: std::collections::HashMap<ImageId, usize> = frames.iter().map(|(f, i)| (f.id, *i)).collect();
        let times: std::collections::HashMap<ImageId, i64> =
            frames.iter().map(|(f, _)| (f.id, f.captured_at_ms)).collect();
        let plain: Vec<BurstFrame> = frames.iter().map(|(f, _)| *f).collect();
        for mut b in group_bursts(&plain, window, burst_hash_distance) {
            apply_pins(&mut b, &pins, |m| scored[index[&m]].quality.overall);
            let keeper = scored[index[&b.keeper]].quality.clone();
            let keeper_name = display_name(conn, b.keeper)?;
            let pinned = pins.contains(&b.keeper);
            let best_face =
                b.members.iter().filter_map(|m| scored[index[m]].quality.face_sharpness).fold(0.0, f32::max);
            for &m in &b.members {
                let q = &mut scored[index[&m]].quality;
                annotate_soft_face(q, best_face);
                if m != b.keeper {
                    demote(q, keeper.suggested_rating);
                    let reason = duplicate_reason(q, &keeper, b.keeper, &keeper_name, pinned);
                    add_reason(q, reason);
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

    let now = now_ms();
    for (a, s) in analyzed.iter().zip(&scored) {
        store::write_scored(&tx, a.id, s, now)?;
    }
    let n = store::write_bursts(&tx, &rows)?;
    tx.commit()?;
    Ok(n)
}

/// How reasons name another photo: its file name without the extension ("DSC0123").
fn display_name(conn: &Connection, id: ImageId) -> AppResult<String> {
    let file: String = conn.query_row("SELECT file_name FROM images WHERE id = ?1", [id], |r| r.get(0))?;
    Ok(Path::new(&file).file_stem().map_or_else(|| file.clone(), |s| s.to_string_lossy().into_owned()))
}

/// Shoot type + thresholds per image, from its project (`db::projects::shoot_type_of_image`;
/// the catalog default for images outside a project), memoized per image / folder and per
/// shoot type. A folder belongs to exactly one project, so bursts (grouped per folder) use
/// one shoot type.
#[derive(Default)]
struct ShootTypes {
    by_image: HashMap<ImageId, ShootType>,
    by_folder: HashMap<i64, ShootType>,
    thresholds: HashMap<ShootType, CullThresholds>,
}

impl ShootTypes {
    fn thresholds(&mut self, conn: &Connection, st: ShootType) -> AppResult<(ShootType, CullThresholds)> {
        if let Some(t) = self.thresholds.get(&st) {
            return Ok((st, t.clone()));
        }
        let t = repo::cull_thresholds(conn, st)?;
        self.thresholds.insert(st, t.clone());
        Ok((st, t))
    }

    fn of_image(&mut self, conn: &Connection, id: ImageId) -> AppResult<(ShootType, CullThresholds)> {
        let st = match self.by_image.get(&id) {
            Some(st) => *st,
            None => {
                let st = projects::shoot_type_of_image(conn, id)?;
                self.by_image.insert(id, st);
                st
            }
        };
        self.thresholds(conn, st)
    }

    /// Shoot type of `folder`, resolved through one of its images (`image`).
    fn of_folder(&mut self, conn: &Connection, folder: i64, image: ImageId) -> AppResult<(ShootType, CullThresholds)> {
        let st = match self.by_folder.get(&folder) {
            Some(st) => *st,
            None => {
                let st = projects::shoot_type_of_image(conn, image)?;
                self.by_folder.insert(folder, st);
                st
            }
        };
        self.thresholds(conn, st)
    }
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
    use crate::ipc::types::{ShootType, SuggestionReason, SuggestionReasonKind};
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

    fn reasons_of(conn: &Connection, id: ImageId) -> Vec<SuggestionReason> {
        let json: String =
            conn.query_row("SELECT reasons_json FROM quality_scores WHERE image_id = ?1", [id], |r| r.get(0)).unwrap();
        serde_json::from_str(&json).unwrap()
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
    fn rescore_scores_each_image_with_its_project_shoot_type() {
        let dir = tempfile::tempdir().unwrap();
        let config = catalog(dir.path(), 4, &[]);
        let mut conn = db::open(&config.catalog_path).unwrap();
        repo::set_shoot_type(&conn, ShootType::General).unwrap();
        // Folder 1 (images 1, 2) -> wedding project; folder 2 (images 3, 4) -> sports project.
        conn.execute_batch(
            "INSERT INTO projects (id, name, shoot_type, created_at) VALUES (10, 'w', 'wedding', 0), (11, 's', 'sports', 0);
             UPDATE folders SET project_id = 10 WHERE id = 1;
             INSERT INTO folders (id, path, added_at, project_id) VALUES (2, '/g', 0, 11);
             UPDATE images SET folder_id = 2, captured_at_ms = captured_at_ms + 60000 WHERE id IN (3, 4);",
        )
        .unwrap();
        // Same metrics everywhere (a soft, closed-eye face), phashes far apart (no bursts).
        let mut m = metrics(vec![face(0.4, 0.15, 0.2, 0.05)]);
        for id in 1..=4 {
            m.phash = [0u64, !0, 0x0F0F_0F0F_0F0F_0F0F, 0xF0F0_F0F0_F0F0_F0F0][id as usize - 1];
            store_metrics(&conn, id, &m);
        }
        rescore_all(&mut conn).unwrap();
        let overall = |id: ImageId| -> f32 {
            db::open(&config.catalog_path)
                .unwrap()
                .query_row("SELECT overall FROM quality_scores WHERE image_id = ?1", [id], |r| r.get(0))
                .unwrap()
        };
        let expect = |st: ShootType| score(&m, &repo::cull_thresholds(&conn, st).unwrap(), st).quality.overall;
        let (wedding, sports) = (expect(ShootType::Wedding), expect(ShootType::Sports));
        assert_ne!(wedding, sports, "test metrics must score differently per shoot type");
        for (id, want) in [(1, wedding), (2, wedding), (3, sports), (4, sports)] {
            assert!((overall(id) - want).abs() < 1e-6, "image {id}: {} vs {want}", overall(id));
        }
        // Changing a project's shoot type changes only its images at the next rescore.
        db::open(&config.catalog_path)
            .unwrap()
            .execute("UPDATE projects SET shoot_type = 'wedding' WHERE id = 11", [])
            .unwrap();
        rescore_all(&mut db::open(&config.catalog_path).unwrap()).unwrap();
        assert!((overall(3) - wedding).abs() < 1e-6);
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
        // Wait for the finish event: `running` is cleared just before it is sent.
        while rec.finished.lock().unwrap().is_empty() {
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
        // ... and say why, pointing at the keeper (file "2.arw").
        let dup = reasons_of(&conn, 1);
        assert_eq!(dup.len(), 1, "{dup:?}");
        assert_eq!((dup[0].kind, dup[0].related_image_id), (SuggestionReasonKind::DuplicateBurst, Some(2)));
        assert_eq!(dup[0].text, "Similar to 2 in this burst \u{2014} that one is sharper");
        assert!(reasons_of(&conn, 2).is_empty(), "the clean keeper needs no reason");
        assert_eq!(reasons_of(&conn, 4)[0].kind, SuggestionReasonKind::Blink);
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
        // Reasons are refreshed by the rescore: they follow the pinned keeper.
        assert!(reasons_of(&conn, 1).iter().all(|r| r.kind != SuggestionReasonKind::DuplicateBurst));
        let dup = reasons_of(&conn, 2);
        assert_eq!(dup.len(), 1, "{dup:?}");
        assert_eq!(dup[0].related_image_id, Some(1));
        assert_eq!(dup[0].text, "Similar to 1 in this burst \u{2014} you chose that one as the keeper");

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

    /// Stub measurer: the first `measure` call runs `removal` (every other call waits for
    /// it, so all results are recorded after it); ids in `fail` fail to measure.
    struct Stub<F: Fn() + Sync> {
        once: std::sync::Once,
        removal: F,
        fail: Vec<ImageId>,
    }

    impl<F: Fn() + Sync> Measure for Stub<F> {
        fn check(&self) -> Result<(), String> {
            Ok(())
        }
        fn measure(&self, preview: &Path) -> Result<ImageMetrics, String> {
            self.once.call_once(&self.removal);
            let name = preview.file_name().unwrap().to_string_lossy();
            let id: ImageId = name.split('_').next().unwrap().parse().unwrap();
            if self.fail.contains(&id) {
                return Err("broken preview".into());
            }
            let mut m = metrics(vec![face(0.4, 0.15, 0.6, 0.28)]);
            m.phash = (id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            Ok(m)
        }
    }

    fn run_stub(config: &AnalysisConfig, removal: impl Fn() + Sync, fail: Vec<ImageId>) -> (RunStats, Recorder) {
        let rec = Recorder::default();
        let flags = WorkerFlags::default();
        flags.running.store(true, Ordering::SeqCst);
        let stub = Stub { once: std::sync::Once::new(), removal, fail };
        let stats = run_with(config, &rec, &flags, &stub).unwrap();
        assert!(!flags.running.load(Ordering::SeqCst));
        (stats, rec)
    }

    fn assert_finished_once(rec: &Recorder, analyzed: u32, failed: u32) {
        let fin = rec.finished.lock().unwrap().clone();
        assert_eq!(fin.len(), 1, "exactly one AnalysisFinished");
        assert_eq!((fin[0].analyzed, fin[0].failed, fin[0].cancelled), (analyzed, failed, false));
        let last = rec.progress.lock().unwrap().last().unwrap().clone();
        assert_eq!((last.done, last.total, last.failed), (analyzed + failed, analyzed + failed, failed));
    }

    #[test]
    fn images_removed_mid_pass_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let config = catalog(dir.path(), 4, &[]);
        // 2 (measures fine) and 4 (fails to measure) disappear while being measured.
        let path = config.catalog_path.clone();
        let removal = move || {
            db::open(&path).unwrap().execute("DELETE FROM images WHERE id IN (2, 4)", []).unwrap();
        };
        let (stats, rec) = run_stub(&config, removal, vec![3, 4]);
        assert_eq!((stats.analyzed, stats.failed, stats.cancelled), (1, 1, false));
        assert_eq!(*rec.ready.lock().unwrap(), vec![1]);
        assert_eq!(rec.failed.lock().unwrap().iter().map(|f| f.image_id).collect::<Vec<_>>(), vec![3]);
        assert_finished_once(&rec, 1, 1);
        let conn = db::open(&config.catalog_path).unwrap();
        let s = store::analysis_status(&conn, false).unwrap();
        assert_eq!((s.total, s.analyzed, s.failed, s.pending), (2, 1, 1, 0));
        let rows: u32 = conn.query_row("SELECT COUNT(*) FROM image_analysis", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 2);
    }

    #[test]
    fn project_removed_mid_pass_finishes_for_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let config = catalog(dir.path(), 6, &[]);
        // Project 10: folder 1 (images 1-3); project 11: folder 2 (images 4-6).
        db::open(&config.catalog_path)
            .unwrap()
            .execute_batch(
                "INSERT INTO projects (id, name, shoot_type, created_at) VALUES (10, 'a', 'wedding', 0), (11, 'b', 'portrait', 0);
                 UPDATE folders SET project_id = 10 WHERE id = 1;
                 INSERT INTO folders (id, path, added_at, project_id) VALUES (2, '/g', 0, 11);
                 UPDATE images SET folder_id = 2 WHERE id IN (4, 5, 6);",
            )
            .unwrap();
        let path = config.catalog_path.clone();
        let removal = move || {
            let (res, ids) = projects::remove_project(&mut db::open(&path).unwrap(), 11).unwrap();
            assert_eq!((res.removed_images, ids), (3, vec![4, 5, 6]));
        };
        let (stats, rec) = run_stub(&config, removal, vec![2, 5]);
        assert_eq!((stats.analyzed, stats.failed, stats.cancelled), (2, 1, false));
        let mut ready = rec.ready.lock().unwrap().clone();
        ready.sort();
        assert_eq!(ready, vec![1, 3]);
        assert_eq!(rec.failed.lock().unwrap().iter().map(|f| f.image_id).collect::<Vec<_>>(), vec![2]);
        assert_finished_once(&rec, 2, 1);
        let conn = db::open(&config.catalog_path).unwrap();
        let s = store::analysis_status(&conn, false).unwrap();
        assert_eq!((s.total, s.analyzed, s.failed, s.pending), (3, 2, 1, 0));
        let scored: Vec<ImageId> = conn
            .prepare("SELECT image_id FROM quality_scores ORDER BY image_id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(scored, vec![1, 3]);
    }

    #[test]
    fn failed_pass_still_finishes_once() {
        let dir = tempfile::tempdir().unwrap();
        // A directory is not a catalog: the pass fails before the first batch.
        let config = AnalysisConfig { catalog_path: dir.path().to_path_buf(), models_dir: dir.path().join("none") };
        let rec = Recorder::default();
        let flags = WorkerFlags::default();
        flags.running.store(true, Ordering::SeqCst);
        let stub = Stub { once: std::sync::Once::new(), removal: || {}, fail: vec![] };
        assert!(run_with(&config, &rec, &flags, &stub).is_err());
        assert!(!flags.running.load(Ordering::SeqCst));
        let fin = rec.finished.lock().unwrap().clone();
        assert_eq!(fin.len(), 1);
        assert!(fin[0].cancelled);
    }

    /// Real previews from the Phase 2 bench cache + real models.
    /// `cargo test --release -- --ignored real_previews`
    #[test]
    #[ignore = "needs models (scripts/fetch-models.sh) and test-data previews"]
    fn real_previews_end_to_end() {
        let models = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models");
        let thumbs = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../test-data/qa-phase2/cache/thumbs"));
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
