//! Export engine (Phase 6): full-resolution develop, resize, output sharpening, colour
//! conversion + ICC, encoding, metadata, file naming, job queue. Owned by rust-engine-dev.
//!
//! The architect fixed only the surface used by `ipc::commands` and `lib.rs`:
//! [`ExportConfig`], [`Exporter`] (`new`, `config`, `capabilities`, `recover_interrupted`,
//! `enqueue`, `cancel`, `jobs`), [`plan`], [`presets`], and the memory policy
//! ([`memory_budget_bytes`], [`estimate_image_bytes`], [`MAX_PARALLEL`]). The compute split
//! ([`develop`], [`naming`], [`encode`], [`metadata`]) is a suggested seam with exact
//! signatures; internals may change as long as the contract below holds.
//!
//! Contract (details in `docs/architecture.md`, "Export"):
//! - Jobs: `enqueue` validates, resolves the destination (`choose` is rejected by the command;
//!   `folder` + subfolder is created, must be writable -> else `io` error, nothing queued),
//!   snapshots every image's stored adjustments *now* (later edits do not affect the job),
//!   inserts `export_jobs` + `export_items` (status `pending`, `seq` = position in `ids`)
//!   and returns the `queued` job immediately. Unknown ids -> `not_found`, nothing queued.
//!   Jobs run one at a time in id order on one `export` worker thread with its own SQLite
//!   connection (never the command `Catalog` mutex).
//! - Per job: images are developed concurrently, bounded by [`MAX_PARALLEL`] and a weighted
//!   semaphore over [`memory_budget_bytes`] (each image acquires [`estimate_image_bytes`],
//!   capped at the budget so an oversized image runs alone). Files are finished in any order;
//!   `{seq}` always follows `ids` order. Emits throttled `exportProgress`, updates the
//!   `export_items` / `export_jobs` counters, and ends with exactly one `exportFinished`.
//! - Per image: `develop::decode_full` (LibRaw full demosaic, same settings as the preview
//!   decode except `half_size = 0`) -> resample linear camera RGB to `develop::output_size`
//!   (orientation applied) -> the *same* parametric pipeline as the preview
//!   (`develop::pipeline`, resolution-independent) -> encode to the target colour space ->
//!   output sharpening -> quantize to the format's bit depth -> `encode::write_file` to
//!   `<name>.<ext>.sieve-tmp` in the target dir, fsync, rename (atomic; no partial files on
//!   cancel/crash). A missing LUT exports without it (not a failure). Per-image errors
//!   (decode, I/O) become `ExportFailure`s; the job continues.
//! - Cancel: in-flight images finish or abort at a checkpoint (their temp file is removed);
//!   images not started stay `pending` in `export_items`; state `cancelled`. Cancelling a
//!   queued job ends it immediately (`exportFinished { cancelled: true, elapsedMs: 0 }`).
//!   Cancelling a finished job is a no-op; unknown id -> `not_found`.
//! - The export path must not use or evict [`crate::develop::DevelopCache`].
//! - WYSIWYG check: exporting at the preview's size (sRGB, 8-bit, no sharpening) must match
//!   `render_preview` of the same adjustments within 2 levels per channel (pre-JPEG).

pub mod color;
pub mod develop;
pub mod encode;
pub(crate) mod heic;
pub mod metadata;
pub mod naming;
pub mod presets;
pub mod tiffw;

use std::collections::{HashSet, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, OptionalExtension};
use tauri::{AppHandle, Runtime};
use tauri_specta::Event;

use crate::db::{self, now_ms, repo};
use crate::ipc::error::{AppError, AppResult, ErrorKind};
use crate::ipc::events::{ExportFinished, ExportProgress};
use crate::ipc::types::{
    parse_filename_template, ExportCapabilities, ExportDestination, ExportFailure, ExportFormatKind, ExportJob,
    ExportJobId, ExportJobState, ExportPlan, ExportSettings, ImageId, ParametricAdjustments, PlannedFile,
    RawImageEntry,
};
use crate::lut::LutLibrary;
use naming::{Claims, Resolved};

/// Most images developed at once within a job. LibRaw's demosaic is largely single-threaded,
/// so 2-4 images overlap decode with other images' (rayon-parallel) pipeline/encode; more only
/// adds memory pressure.
pub const MAX_PARALLEL: usize = 4;

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

/// Default export memory budget: 25% of physical RAM, clamped to 2..=8 GiB. `override_mb`
/// (`SIEVE_EXPORT_MEMORY_MB`) wins when set (min 512 MiB).
pub fn memory_budget_bytes(physical_ram_bytes: u64, override_mb: Option<u64>) -> u64 {
    match override_mb {
        Some(mb) => mb.max(512) * MIB,
        None => (physical_ram_bytes / 4).clamp(2 * GIB, 8 * GIB),
    }
}

/// Peak memory estimate for exporting one image: LibRaw decode (~16 B/px of the source:
/// raw buffer + 4x16-bit image + RGB16 copy) plus the output-size working set (~24 B/px:
/// f32 RGB in/out of the pipeline and the quantized output) plus 64 MiB for encoder and LUT
/// buffers. 24 MP full-res ~= 1.0 GB, 61 MP full-res ~= 2.4 GB, any source -> 2048 px ~= 0.5 GB.
pub fn estimate_image_bytes(source_pixels: u64, output_pixels: u64) -> u64 {
    source_pixels * 16 + output_pixels * 24 + 64 * MIB
}

/// Physical RAM (`hw.memsize`); 16 GiB if unknown.
pub fn physical_ram_bytes() -> u64 {
    #[cfg(target_os = "macos")]
    {
        let mut v: u64 = 0;
        let mut len = std::mem::size_of::<u64>();
        // SAFETY: name is NUL-terminated; `v` has room for the u64 result.
        let rc = unsafe {
            libc::sysctlbyname(c"hw.memsize".as_ptr(), (&mut v as *mut u64).cast(), &mut len, std::ptr::null_mut(), 0)
        };
        if rc == 0 && v > 0 {
            return v;
        }
    }
    16 * GIB
}

/// Resolved at startup by `lib.rs`.
#[derive(Debug, Clone)]
pub struct ExportConfig {
    /// Catalog file; the export worker opens its own connection to it.
    pub catalog_path: PathBuf,
    /// `SIEVE_EXPORT_MEMORY_MB`; `None` = derive from physical RAM ([`memory_budget_bytes`]).
    pub memory_budget_mb: Option<u64>,
}

/// Receives export events; implemented for `AppHandle` and by tests / the bench.
pub trait ExportSink: Send + Sync {
    fn progress(&self, event: ExportProgress);
    fn finished(&self, event: ExportFinished);
}

impl<R: Runtime> ExportSink for AppHandle<R> {
    fn progress(&self, event: ExportProgress) {
        let _ = event.emit(self);
    }
    fn finished(&self, event: ExportFinished) {
        let _ = event.emit(self);
    }
}

/// One image of a queued job (adjustments snapshotted at enqueue).
struct JobItem {
    seq: u32,
    entry: RawImageEntry,
    adjustments: ParametricAdjustments,
}

struct QueuedJob {
    id: ExportJobId,
    settings: ExportSettings,
    output_dir: Option<PathBuf>,
    items: Vec<JobItem>,
    sink: Arc<dyn ExportSink>,
}

#[derive(Debug, Default, Clone, Copy)]
struct Live {
    succeeded: u32,
    failed: u32,
    skipped: u32,
}

#[derive(Default)]
struct State {
    queue: VecDeque<QueuedJob>,
    running: Option<ExportJobId>,
    live: Option<(ExportJobId, Live)>,
    cancel: HashSet<ExportJobId>,
    worker: bool,
}

struct Inner {
    state: Mutex<State>,
    /// Signalled whenever a job finishes (for [`Exporter::wait_idle`]).
    idle: Condvar,
    budget: u64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Managed Tauri state: export job queue + worker. Cheap to clone.
#[derive(Clone)]
pub struct Exporter {
    config: ExportConfig,
    luts: LutLibrary,
    inner: Arc<Inner>,
}

impl Exporter {
    pub fn new(config: ExportConfig, luts: LutLibrary) -> Self {
        let budget = memory_budget_bytes(physical_ram_bytes(), config.memory_budget_mb);
        Self {
            config,
            luts,
            inner: Arc::new(Inner { state: Mutex::new(State::default()), idle: Condvar::new(), budget }),
        }
    }

    pub fn config(&self) -> &ExportConfig {
        &self.config
    }

    pub fn luts(&self) -> &LutLibrary {
        &self.luts
    }

    /// Memory budget shared by concurrently developed images, bytes.
    pub fn memory_budget(&self) -> u64 {
        self.inner.budget
    }

    /// Encoder availability (probed once, cached), `MAX_PARALLEL` and the memory budget.
    pub fn capabilities(&self) -> ExportCapabilities {
        ExportCapabilities {
            formats: encode::probe_formats(),
            max_parallel: MAX_PARALLEL as u32,
            memory_budget_mb: (self.inner.budget / MIB).min(u64::from(u32::MAX)) as u32,
        }
    }

    fn open(&self) -> AppResult<Connection> {
        db::open(&self.config.catalog_path)
    }

    /// Blocking; called once at startup. Marks jobs left `queued`/`running` by a previous
    /// session as `interrupted` (finished_at = now). Must not fail startup on a fresh catalog.
    pub fn recover_interrupted(&self) -> AppResult<()> {
        let conn = self.open()?;
        conn.execute(
            "UPDATE export_jobs SET state = 'interrupted', finished_at = ?1 WHERE state IN ('queued', 'running')",
            [now_ms()],
        )?;
        Ok(())
    }

    /// Blocking. Validates, resolves/creates the destination, snapshots adjustments, records
    /// the job and returns it `queued`; starts the worker if idle. `settings` is already
    /// validated and its destination is not `choose`; `ids` is non-empty and de-duplicated
    /// by the caller (first occurrence kept).
    pub fn enqueue(
        &self,
        app: &AppHandle,
        ids: Vec<ImageId>,
        settings: ExportSettings,
        preset_name: Option<String>,
    ) -> AppResult<ExportJob> {
        self.enqueue_with(Arc::new(app.clone()), ids, settings, preset_name)
    }

    /// [`Self::enqueue`] with any event sink (tests, bench).
    pub fn enqueue_with(
        &self,
        sink: Arc<dyn ExportSink>,
        ids: Vec<ImageId>,
        settings: ExportSettings,
        preset_name: Option<String>,
    ) -> AppResult<ExportJob> {
        if ids.is_empty() {
            return Err(AppError::invalid("nothing to export"));
        }
        settings.validate().map_err(AppError::invalid)?;
        let kind = settings.format.kind();
        if let Some(f) = encode::probe_formats().into_iter().find(|f| f.kind == kind && !f.available) {
            return Err(AppError::invalid(
                f.reason.unwrap_or_else(|| format!("{} export is unavailable", kind.as_str())),
            ));
        }
        let output_dir = output_dir(&settings)?;
        let mut conn = self.open()?;
        let entries = repo::get_images(&conn, &ids)?;
        let adjustments = ids.iter().map(|&id| repo::get_adjustments(&conn, id)).collect::<AppResult<Vec<_>>>()?;
        if let Some(dir) = &output_dir {
            ensure_writable_dir(dir)?;
        }
        let created = now_ms();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO export_jobs (state, preset_name, settings_json, output_dir, total, created_at)
             VALUES ('queued', ?1, ?2, ?3, ?4, ?5)",
            params![
                preset_name,
                serde_json::to_string(&settings)?,
                output_dir.as_ref().map(|d| d.to_string_lossy().into_owned()),
                ids.len() as i64,
                created
            ],
        )?;
        let job_id = tx.last_insert_rowid();
        {
            let mut stmt = tx.prepare("INSERT INTO export_items (job_id, seq, image_id) VALUES (?1, ?2, ?3)")?;
            for (seq, id) in ids.iter().enumerate() {
                stmt.execute(params![job_id, seq as i64, id])?;
            }
        }
        tx.commit()?;
        let job = ExportJob {
            id: job_id,
            state: ExportJobState::Queued,
            preset_name,
            format: kind,
            total: ids.len() as u32,
            done: 0,
            succeeded: 0,
            failed: 0,
            skipped: 0,
            output_dir: output_dir.as_ref().map(|d| d.to_string_lossy().into_owned()),
            failures: Vec::new(),
            created_at_ms: created,
            finished_at_ms: None,
        };
        let items = entries
            .into_iter()
            .zip(adjustments)
            .enumerate()
            .map(|(seq, (entry, adjustments))| JobItem { seq: seq as u32, entry, adjustments })
            .collect();
        let queued = QueuedJob { id: job_id, settings, output_dir, items, sink };
        let start = {
            let mut st = lock(&self.inner.state);
            st.queue.push_back(queued);
            !std::mem::replace(&mut st.worker, true)
        };
        if start {
            let this = self.clone();
            let spawned = std::thread::Builder::new().name("export".into()).spawn(move || this.worker_loop());
            if let Err(e) = spawned {
                lock(&self.inner.state).worker = false;
                return Err(AppError::internal(format!("export worker: {e}")));
            }
        }
        Ok(job)
    }

    /// Requests cancellation (see module docs). Returns immediately.
    pub fn cancel(&self, app: &AppHandle, job_id: ExportJobId) -> AppResult<()> {
        let _ = app;
        self.cancel_job(job_id)
    }

    /// [`Self::cancel`] without an app handle (events go to the job's own sink).
    pub fn cancel_job(&self, job_id: ExportJobId) -> AppResult<()> {
        let queued = {
            let mut st = lock(&self.inner.state);
            if st.running == Some(job_id) {
                st.cancel.insert(job_id);
                return Ok(());
            }
            match st.queue.iter().position(|j| j.id == job_id) {
                Some(i) => st.queue.remove(i),
                None => None,
            }
        };
        let conn = self.open()?;
        match queued {
            Some(job) => {
                conn.execute(
                    "UPDATE export_jobs SET state = 'cancelled', finished_at = ?2 WHERE id = ?1",
                    params![job_id, now_ms()],
                )?;
                job.sink.finished(ExportFinished {
                    job_id,
                    succeeded: 0,
                    skipped: 0,
                    failed: Vec::new(),
                    cancelled: true,
                    output_dir: job.output_dir.map(|d| d.to_string_lossy().into_owned()),
                    elapsed_ms: 0,
                });
                self.inner.idle.notify_all();
                Ok(())
            }
            None => {
                let exists: Option<i64> =
                    conn.query_row("SELECT id FROM export_jobs WHERE id = ?1", [job_id], |r| r.get(0)).optional()?;
                match exists {
                    Some(_) => Ok(()),
                    None => Err(AppError::not_found(format!("export job {job_id}"))),
                }
            }
        }
    }

    /// Blocking. Queued and running jobs first (with live counters), then the 50 most recent
    /// finished jobs, newest first. `failures` lists every failed item of each job.
    pub fn jobs(&self) -> AppResult<Vec<ExportJob>> {
        let conn = self.open()?;
        let mut out = query_jobs(
            &conn,
            "WHERE state IN ('queued', 'running') ORDER BY CASE state WHEN 'running' THEN 0 ELSE 1 END, id",
        )?;
        let live = lock(&self.inner.state).live;
        if let Some((id, l)) = live {
            if let Some(j) = out.iter_mut().find(|j| j.id == id) {
                j.succeeded = j.succeeded.max(l.succeeded);
                j.failed = j.failed.max(l.failed);
                j.skipped = j.skipped.max(l.skipped);
                j.done = j.succeeded + j.failed + j.skipped;
            }
        }
        out.extend(query_jobs(
            &conn,
            "WHERE state NOT IN ('queued', 'running') ORDER BY created_at DESC, id DESC LIMIT 50",
        )?);
        Ok(out)
    }

    /// Blocks until no job is queued or running (tests, bench) or `timeout` passes.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut st = lock(&self.inner.state);
        while st.worker || !st.queue.is_empty() {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            st = self.inner.idle.wait_timeout(st, deadline - now).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
        }
        true
    }

    fn worker_loop(self) {
        let conn = match self.open() {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("export worker: cannot open the catalog: {}", e.message);
                None
            }
        };
        loop {
            let job = {
                let mut st = lock(&self.inner.state);
                match st.queue.pop_front() {
                    Some(j) => {
                        st.running = Some(j.id);
                        st.live = Some((j.id, Live::default()));
                        j
                    }
                    None => {
                        st.worker = false;
                        st.running = None;
                        st.live = None;
                        drop(st);
                        self.inner.idle.notify_all();
                        return;
                    }
                }
            };
            let id = job.id;
            let result = catch_unwind(AssertUnwindSafe(|| self.run_job(conn.as_ref(), job)));
            if result.is_err() {
                eprintln!("export job {id}: worker panicked");
                if let Some(c) = &conn {
                    let _ = c.execute(
                        "UPDATE export_jobs SET state = 'interrupted', finished_at = ?2 WHERE id = ?1",
                        params![id, now_ms()],
                    );
                }
            }
            {
                let mut st = lock(&self.inner.state);
                st.running = None;
                st.live = None;
                st.cancel.remove(&id);
            }
            self.inner.idle.notify_all();
        }
    }

    fn is_cancelled(&self, id: ExportJobId) -> bool {
        lock(&self.inner.state).cancel.contains(&id)
    }

    fn run_job(&self, conn: Option<&Connection>, job: QueuedJob) {
        let started = Instant::now();
        let db = |sql: &str, p: &[&dyn rusqlite::ToSql]| {
            if let Some(c) = conn {
                if let Err(e) = c.execute(sql, p) {
                    eprintln!("export job {}: {e}", job.id);
                }
            }
        };
        db("UPDATE export_jobs SET state = 'running', started_at = ?2 WHERE id = ?1", &[&job.id, &now_ms()]);
        let total = job.items.len() as u32;
        let settings = &job.settings;
        let entries: Vec<&RawImageEntry> = job.items.iter().map(|i| &i.entry).collect();
        let resolved = resolve_paths(&entries, settings, job.output_dir.as_deref(), &|p: &Path| p.exists());

        let mut live = Live::default();
        let mut failures: Vec<ExportFailure> = Vec::new();
        let mut last_emit: Option<Instant> = None;
        let emit = |live: &Live, current: Option<String>, last: &mut Option<Instant>, force: bool| {
            if force || last.is_none_or(|t| t.elapsed() >= Duration::from_millis(100)) {
                *last = Some(Instant::now());
                job.sink.progress(ExportProgress {
                    job_id: job.id,
                    done: live.succeeded + live.failed + live.skipped,
                    total,
                    failed: live.failed,
                    skipped: live.skipped,
                    current_file: current,
                });
            }
        };
        let set_live = |live: &Live| {
            if let Some((id, l)) = lock(&self.inner.state).live.as_mut() {
                if *id == job.id {
                    *l = *live;
                }
            }
        };

        // Skips first (no pixels involved).
        let mut work: Vec<(usize, PathBuf)> = Vec::new();
        for (i, (base, r)) in resolved.iter().enumerate() {
            match &r.path {
                Some(p) => work.push((i, p.clone())),
                None => {
                    live.skipped += 1;
                    db(
                        "UPDATE export_items SET status = 'skipped', output_path = ?3 WHERE job_id = ?1 AND seq = ?2",
                        &[&job.id, &job.items[i].seq, &base.to_string_lossy().into_owned()],
                    );
                }
            }
        }
        if live.skipped > 0 {
            db("UPDATE export_jobs SET skipped = ?2 WHERE id = ?1", &[&job.id, &live.skipped]);
            set_live(&live);
        }
        emit(&live, None, &mut last_emit, true);

        // Develop + encode, bounded by MAX_PARALLEL and the weighted memory semaphore.
        let budget = Budget::new(self.inner.budget);
        let next = AtomicUsize::new(0);
        let (tx, rx) = mpsc::channel::<Msg>();
        let threads = MAX_PARALLEL.min(work.len());
        std::thread::scope(|scope| {
            for _ in 0..threads {
                let tx = tx.clone();
                let (work, next, budget) = (&work, &next, &budget);
                let job = &job;
                scope.spawn(move || loop {
                    let n = next.fetch_add(1, Ordering::SeqCst);
                    let Some((i, path)) = work.get(n) else { break };
                    let item = &job.items[*i];
                    let cancelled = || self.is_cancelled(job.id);
                    if cancelled() {
                        break;
                    }
                    let need = item_estimate(&item.entry, settings).min(self.inner.budget);
                    if !budget.acquire(need, &cancelled) {
                        break;
                    }
                    let _ = tx.send(Msg::Started(item.entry.file_name.clone()));
                    let outcome =
                        catch_unwind(AssertUnwindSafe(|| export_one(item, settings, path, &self.luts, &cancelled)))
                            .unwrap_or_else(|_| Err(Failure::Error("internal error (panic) while exporting".into())));
                    budget.release(need);
                    let _ = tx.send(Msg::Done(*i, outcome));
                });
            }
            drop(tx);
            let mut in_flight: Vec<String> = Vec::new();
            for msg in rx {
                match msg {
                    Msg::Started(name) => {
                        in_flight.push(name.clone());
                        emit(&live, Some(name), &mut last_emit, false);
                    }
                    Msg::Done(i, outcome) => {
                        let item = &job.items[i];
                        if let Some(pos) = in_flight.iter().position(|n| *n == item.entry.file_name) {
                            in_flight.remove(pos);
                        }
                        match outcome {
                            Ok(path) => {
                                live.succeeded += 1;
                                db(
                                    "UPDATE export_items SET status = 'done', output_path = ?3, error = NULL
                                     WHERE job_id = ?1 AND seq = ?2",
                                    &[&job.id, &item.seq, &path.to_string_lossy().into_owned()],
                                );
                            }
                            Err(Failure::Cancelled) => continue,
                            Err(Failure::Error(reason)) => {
                                live.failed += 1;
                                db(
                                    "UPDATE export_items SET status = 'failed', error = ?3 WHERE job_id = ?1 AND seq = ?2",
                                    &[&job.id, &item.seq, &reason],
                                );
                                failures.push(ExportFailure {
                                    image_id: item.entry.id,
                                    file_name: item.entry.file_name.clone(),
                                    reason,
                                });
                            }
                        }
                        db(
                            "UPDATE export_jobs SET succeeded = ?2, failed = ?3, skipped = ?4 WHERE id = ?1",
                            &[&job.id, &live.succeeded, &live.failed, &live.skipped],
                        );
                        set_live(&live);
                        emit(&live, in_flight.last().cloned(), &mut last_emit, false);
                    }
                }
            }
        });

        let cancelled = self.is_cancelled(job.id);
        let state = if cancelled { ExportJobState::Cancelled } else { ExportJobState::Completed };
        db(
            "UPDATE export_jobs SET state = ?2, finished_at = ?3, succeeded = ?4, failed = ?5, skipped = ?6 WHERE id = ?1",
            &[&job.id, &state.as_str(), &now_ms(), &live.succeeded, &live.failed, &live.skipped],
        );
        emit(&live, None, &mut last_emit, true);
        job.sink.finished(ExportFinished {
            job_id: job.id,
            succeeded: live.succeeded,
            skipped: live.skipped,
            failed: failures,
            cancelled,
            output_dir: job.output_dir.as_ref().map(|d| d.to_string_lossy().into_owned()),
            elapsed_ms: started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32,
        });
    }
}

enum Msg {
    Started(String),
    Done(usize, Result<PathBuf, Failure>),
}

enum Failure {
    /// Stopped at a checkpoint by `cancel_export` (the item stays `pending`).
    Cancelled,
    Error(String),
}

impl From<AppError> for Failure {
    fn from(e: AppError) -> Self {
        Failure::Error(e.message)
    }
}

/// Weighted semaphore over the memory budget (bytes).
struct Budget {
    avail: Mutex<u64>,
    cv: Condvar,
}

impl Budget {
    fn new(total: u64) -> Self {
        Budget { avail: Mutex::new(total), cv: Condvar::new() }
    }

    /// Waits for `n` bytes; `false` if cancelled while waiting.
    fn acquire(&self, n: u64, cancelled: &dyn Fn() -> bool) -> bool {
        let mut a = lock(&self.avail);
        while *a < n {
            if cancelled() {
                return false;
            }
            a = self
                .cv
                .wait_timeout(a, Duration::from_millis(100))
                .map(|(g, _)| g)
                .unwrap_or_else(|e| e.into_inner().0);
        }
        *a -= n;
        true
    }

    fn release(&self, n: u64) {
        *lock(&self.avail) += n;
        self.cv.notify_all();
    }
}

/// Memory estimate for one item from catalog dimensions (61 MP when unknown).
fn item_estimate(entry: &RawImageEntry, settings: &ExportSettings) -> u64 {
    let (w, h) = match (entry.width, entry.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w, h),
        _ => (9568, 6376),
    };
    let (ow, oh) = develop::output_size((w, h), &settings.resize);
    estimate_image_bytes(u64::from(w) * u64::from(h), u64::from(ow) * u64::from(oh))
}

/// Temp file next to the target: `<name>.<ext>.sieve-tmp`.
fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".sieve-tmp");
    path.with_file_name(name)
}

/// Develops, encodes and atomically writes one image. Checks for cancellation between stages.
fn export_one(
    item: &JobItem,
    settings: &ExportSettings,
    path: &Path,
    luts: &LutLibrary,
    cancelled: &dyn Fn() -> bool,
) -> Result<PathBuf, Failure> {
    let check = || if cancelled() { Err(Failure::Cancelled) } else { Ok(()) };
    let raw = Path::new(&item.entry.path);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Failure::Error(format!("{}: {e}", dir.display())))?;
    }
    let src = develop::decode_full(raw)?;
    check()?;
    let orientation = item.entry.orientation;
    let size = develop::planned_size(&src, orientation, &settings.resize);
    let prepared = develop::prepare_output(&src, orientation, size)?;
    // Free the full-size decode as soon as the resampled copy exists.
    let crate::develop::source::LinearImage { pixels: decoded, color, .. } = src;
    let pixels = match prepared {
        Some(p) => {
            drop(decoded);
            p
        }
        None => decoded,
    };
    check()?;
    let lut = match &item.adjustments.lut {
        Some(l) => luts.load(&l.id).unwrap_or(None),
        None => None,
    };
    let encoded = develop::develop_prepared(&pixels, size, &color, &item.adjustments, lut.as_deref(), settings);
    drop(pixels);
    check()?;
    let image = develop::finish(encoded, size, settings);
    let meta = metadata::collect(raw, &settings.metadata)?;
    check()?;
    let tmp = temp_path(path);
    let written = encode::write_file(&image, settings, &meta, &tmp);
    drop(image);
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    if cancelled() {
        let _ = std::fs::remove_file(&tmp);
        return Err(Failure::Cancelled);
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(Failure::Error(format!("{}: {e}", path.display())));
    }
    Ok(path.to_path_buf())
}

/// `folder` + subfolder; `None` for `source_folder`. `choose` -> `invalid_argument`.
fn output_dir(settings: &ExportSettings) -> AppResult<Option<PathBuf>> {
    let sub = |base: PathBuf| match &settings.subfolder {
        Some(s) => base.join(s),
        None => base,
    };
    match &settings.destination {
        ExportDestination::Folder { path } => Ok(Some(sub(PathBuf::from(path)))),
        ExportDestination::SourceFolder => Ok(None),
        ExportDestination::Choose => Err(AppError::invalid("choose a destination folder before exporting")),
    }
}

/// Creates `dir` and checks it is writable.
fn ensure_writable_dir(dir: &Path) -> AppResult<()> {
    let io = |e: std::io::Error| AppError::new(ErrorKind::Io, format!("{}: {e}", dir.display()));
    std::fs::create_dir_all(dir).map_err(io)?;
    let probe = dir.join(format!(".sieve-write-test-{}", std::process::id()));
    std::fs::write(&probe, b"").map_err(io)?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// The folder an image is written to.
fn item_dir(output_dir: Option<&Path>, entry: &RawImageEntry, settings: &ExportSettings) -> PathBuf {
    match output_dir {
        Some(d) => d.to_path_buf(),
        None => {
            let base = Path::new(&entry.path).parent().map(Path::to_path_buf).unwrap_or_default();
            match &settings.subfolder {
                Some(s) => base.join(s),
                None => base,
            }
        }
    }
}

/// Template path and resolved path of every entry, in order (`{seq}` = index + startNumber).
fn resolve_paths(
    entries: &[&RawImageEntry],
    settings: &ExportSettings,
    output_dir: Option<&Path>,
    exists: &dyn Fn(&Path) -> bool,
) -> Vec<(PathBuf, Resolved)> {
    let parts = parse_filename_template(&settings.naming.template).unwrap_or_default();
    let ext = settings.format.kind().extension();
    let mut claims = Claims::default();
    entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let dir = item_dir(output_dir, e, settings);
            let stem = naming::expand(&parts, e, i as u64 + u64::from(settings.naming.start_number));
            let base = dir.join(format!("{stem}.{ext}"));
            (base, claims.resolve(&dir, &stem, ext, settings.naming.collision, exists))
        })
        .collect()
}

fn query_jobs(conn: &Connection, tail: &str) -> AppResult<Vec<ExportJob>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT id, state, preset_name, settings_json, output_dir, total, succeeded, failed, skipped,
                created_at, finished_at FROM export_jobs {tail}"
    ))?;
    let rows = stmt.query_map([], |r| {
        let settings: String = r.get(3)?;
        let format = serde_json::from_str::<serde_json::Value>(&settings)
            .ok()
            .and_then(|v| v.get("format")?.get("kind")?.as_str().and_then(ExportFormatKind::parse))
            .unwrap_or(ExportFormatKind::Jpeg);
        let state: String = r.get(1)?;
        let (succeeded, failed, skipped): (u32, u32, u32) = (r.get(6)?, r.get(7)?, r.get(8)?);
        Ok(ExportJob {
            id: r.get(0)?,
            state: ExportJobState::parse(&state).unwrap_or(ExportJobState::Interrupted),
            preset_name: r.get(2)?,
            format,
            output_dir: r.get(4)?,
            total: r.get(5)?,
            done: succeeded + failed + skipped,
            succeeded,
            failed,
            skipped,
            failures: Vec::new(),
            created_at_ms: r.get(9)?,
            finished_at_ms: r.get(10)?,
        })
    })?;
    let mut jobs = rows.collect::<Result<Vec<_>, _>>()?;
    let mut fstmt = conn.prepare(
        "SELECT e.image_id, i.file_name, COALESCE(e.error, '') FROM export_items e JOIN images i ON i.id = e.image_id
         WHERE e.job_id = ?1 AND e.status = 'failed' ORDER BY e.seq",
    )?;
    for j in jobs.iter_mut().filter(|j| j.failed > 0) {
        j.failures = fstmt
            .query_map([j.id], |r| Ok(ExportFailure { image_id: r.get(0)?, file_name: r.get(1)?, reason: r.get(2)? }))?
            .collect::<Result<Vec<_>, _>>()?;
    }
    Ok(jobs)
}

/// Dry run over the command connection: resolves the output dir and each image's final
/// path under the collision policy (in `ids` order, including intra-job duplicates) without
/// touching the disk except `exists` checks. Unknown ids -> `not_found`. `settings` is
/// validated and its destination is not `choose`.
pub fn plan(conn: &Connection, ids: &[ImageId], settings: &ExportSettings) -> AppResult<ExportPlan> {
    let entries = repo::get_images(conn, ids)?;
    let dir = output_dir(settings)?;
    let refs: Vec<&RawImageEntry> = entries.iter().collect();
    let resolved = resolve_paths(&refs, settings, dir.as_deref(), &|p: &Path| p.exists());
    let files: Vec<PlannedFile> = entries
        .iter()
        .zip(resolved)
        .map(|(e, (_, r))| PlannedFile {
            image_id: e.id,
            path: r.path.map(|p| p.to_string_lossy().into_owned()),
            exists: r.exists,
        })
        .collect();
    let existing = files.iter().filter(|f| f.exists).count() as u32;
    Ok(ExportPlan { output_dir: dir.map(|d| d.to_string_lossy().into_owned()), files, existing })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_policy() {
        assert_eq!(memory_budget_bytes(16 * GIB, None), 4 * GIB);
        assert_eq!(memory_budget_bytes(4 * GIB, None), 2 * GIB);
        assert_eq!(memory_budget_bytes(128 * GIB, None), 8 * GIB);
        assert_eq!(memory_budget_bytes(16 * GIB, Some(100)), 512 * MIB);
        assert_eq!(memory_budget_bytes(16 * GIB, Some(3000)), 3000 * MIB);
        let mp24 = 6000 * 4000;
        let full = estimate_image_bytes(mp24, mp24);
        assert!(full > 900 * MIB && full < 1100 * MIB, "{full}");
        // Four 24 MP full-res exports fit a 16 GB Mac's default budget.
        assert!(4 * full <= memory_budget_bytes(16 * GIB, None) + 256 * MIB);
    }

    use crate::ipc::types::{
        ChromaSubsampling, CollisionPolicy, ExportFormat, ExportPreset, FileNaming, ImportOptions, MetadataInclude,
        MetadataOptions, ResizeMode, ResizeOptions,
    };

    #[derive(Default)]
    struct Sink {
        progress: Mutex<Vec<ExportProgress>>,
        finished: Mutex<Vec<ExportFinished>>,
    }

    impl ExportSink for Sink {
        fn progress(&self, e: ExportProgress) {
            lock(&self.progress).push(e);
        }
        fn finished(&self, e: ExportFinished) {
            lock(&self.finished).push(e);
        }
    }

    fn settings(out: &Path) -> ExportSettings {
        ExportSettings {
            format: ExportFormat::Jpeg { quality: 80, chroma_subsampling: ChromaSubsampling::Yuv420 },
            color_space: crate::ipc::types::ExportColorSpace::Srgb,
            resize: ResizeOptions { mode: ResizeMode::LongEdge { px: 512 }, dont_enlarge: true, resolution_ppi: 72 },
            sharpening: None,
            naming: FileNaming {
                template: "{filename}".into(),
                start_number: 1,
                collision: CollisionPolicy::UniqueSuffix,
            },
            destination: ExportDestination::Folder { path: out.to_string_lossy().into_owned() },
            subfolder: Some("Web".into()),
            metadata: MetadataOptions {
                include: MetadataInclude::All,
                remove_location: false,
                include_keywords: true,
                copyright: None,
                creator: None,
            },
        }
    }

    /// Catalog with two images whose files are not decodable RAWs.
    fn setup(dir: &Path) -> Exporter {
        let catalog = dir.join("cat.sqlite");
        let conn = db::open(&catalog).unwrap();
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        for n in ["a", "b"] {
            std::fs::write(src.join(format!("{n}.arw")), b"II*\0garbage").unwrap();
        }
        conn.execute_batch(&format!(
            "INSERT INTO folders (id, path, added_at) VALUES (1, '{s}', 0);
             INSERT INTO images (id, folder_id, path, file_name, format, camera_make, file_size, file_mtime_ms, imported_at)
             VALUES (1, 1, '{s}/a.arw', 'a.arw', 'arw', 'sony', 1, 0, 0),
                    (2, 1, '{s}/b.arw', 'b.arw', 'arw', 'sony', 1, 0, 0);",
            s = src.display()
        ))
        .unwrap();
        let luts = LutLibrary::new(dir.join("luts"));
        Exporter::new(ExportConfig { catalog_path: catalog, memory_budget_mb: Some(1024) }, luts)
    }

    #[test]
    fn failing_images_still_complete_the_job() {
        let dir = tempfile::tempdir().unwrap();
        let ex = setup(dir.path());
        let sink = Arc::new(Sink::default());
        let out = dir.path().join("out");
        let job = ex.enqueue_with(sink.clone(), vec![2, 1], settings(&out), Some("Web".into())).unwrap();
        assert_eq!((job.state, job.total, job.format), (ExportJobState::Queued, 2, ExportFormatKind::Jpeg));
        assert_eq!(job.output_dir.as_deref(), Some(out.join("Web").to_str().unwrap()));
        assert!(out.join("Web").is_dir(), "destination created at enqueue");
        assert!(ex.wait_idle(Duration::from_secs(30)));
        let fin = lock(&sink.finished).clone();
        assert_eq!(fin.len(), 1, "exactly one exportFinished");
        assert_eq!((fin[0].succeeded, fin[0].failed.len(), fin[0].cancelled), (0, 2, false));
        assert!(fin[0].failed.iter().all(|f| f.reason.contains("LibRaw")), "{:?}", fin[0].failed);
        let last = lock(&sink.progress).last().cloned().unwrap();
        assert_eq!((last.done, last.total, last.failed), (2, 2, 2));
        let jobs = ex.jobs().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!((jobs[0].state, jobs[0].done, jobs[0].failed), (ExportJobState::Completed, 2, 2));
        assert_eq!(jobs[0].failures.iter().map(|f| f.file_name.as_str()).collect::<Vec<_>>(), ["b.arw", "a.arw"]);
        assert_eq!(jobs[0].preset_name.as_deref(), Some("Web"));
        assert!(jobs[0].finished_at_ms.is_some());
        // No temp files left behind.
        assert_eq!(std::fs::read_dir(out.join("Web")).unwrap().count(), 0);

        // Unknown ids: nothing queued.
        let e = ex.enqueue_with(sink.clone(), vec![1, 99], settings(&out), None).unwrap_err();
        assert_eq!(e.kind, ErrorKind::NotFound);
        assert_eq!(ex.jobs().unwrap().len(), 1);
        // Unwritable destination: io error.
        let mut s = settings(&out);
        s.destination = ExportDestination::Folder { path: "/dev/null/nope".into() };
        assert_eq!(ex.enqueue_with(sink, vec![1], s, None).unwrap_err().kind, ErrorKind::Io);
    }

    /// v9: exports from non-RAW sources (JPEG with EXIF + orientation, 16-bit PNG) through
    /// the raster decode; EXIF is copied from the JPEG, orientation applied, originals intact.
    #[test]
    fn non_raw_sources_export() {
        use crate::ingest::{run_until_idle, IngestConfig, IngestSink};
        use crate::ipc::events::{ImportProgress, ThumbnailFailed, ThumbnailReady};
        use crate::raw::raster::tests::fixtures;
        struct Quiet;
        impl IngestSink for Quiet {
            fn ready(&self, _: ThumbnailReady) {}
            fn failed(&self, e: ThumbnailFailed) {
                panic!("{}", e.reason)
            }
            fn progress(&self, _: ImportProgress) {}
        }

        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let jpeg = fixtures::camera_jpeg(640, 480, 6, None);
        std::fs::write(src.join("IMG_1.JPG"), &jpeg).unwrap();
        let px: Vec<u16> = (0..200 * 100).flat_map(|i| [(i * 3 % 65536) as u16, 30000, 50000]).collect();
        std::fs::write(
            src.join("b.png"),
            crate::raw::png::test_support::encode(200, 100, Some(&px), None, None, None, None),
        )
        .unwrap();
        let catalog = dir.path().join("cat.sqlite");
        {
            let mut conn = db::open(&catalog).unwrap();
            let opts = ImportOptions { recursive: false, include_non_raw: true, pair_jpeg_with_raw: true };
            assert_eq!(repo::import_folder(&mut conn, &src, &opts).unwrap().added, 2);
        }
        let cfg = IngestConfig { catalog_path: catalog.clone(), cache_dir: dir.path().join("cache") };
        run_until_idle(&cfg, &Quiet, &std::sync::atomic::AtomicBool::new(true)).unwrap();

        let ex = Exporter::new(
            ExportConfig { catalog_path: catalog, memory_budget_mb: Some(1024) },
            LutLibrary::new(dir.path().join("luts")),
        );
        let out = dir.path().join("out");
        let mut s = settings(&out);
        s.resize.mode = ResizeMode::None;
        let sink = Arc::new(Sink::default());
        ex.enqueue_with(sink.clone(), vec![1, 2], s.clone(), None).unwrap();
        assert!(ex.wait_idle(Duration::from_secs(60)));
        let fin = lock(&sink.finished).clone();
        assert_eq!(fin[0].succeeded, 2, "{:?}", fin[0].failed);

        let web = out.join("Web");
        let j = std::fs::read(web.join("IMG_1.jpg")).unwrap();
        assert_eq!(crate::raw::turbo::dimensions(&j).unwrap(), (480, 640), "orientation 6 applied");
        let exif = crate::raw::jpeg::exif_tiff(&j).expect("EXIF copied");
        let meta = crate::raw::tiff::scan_bytes(exif, false).unwrap().meta;
        assert_eq!(
            (meta.make.as_deref(), meta.model.as_deref(), meta.orientation),
            (Some("FUJIFILM"), Some("X-T5"), Some(1))
        );
        assert!(meta.date_original.is_some());
        assert!(web.join("b.jpg").exists());
        // Originals untouched.
        assert_eq!(std::fs::read(src.join("IMG_1.JPG")).unwrap(), jpeg);

        // 16-bit TIFF from the 16-bit PNG.
        s.format = ExportFormat::Tiff {
            bit_depth: crate::ipc::types::BitDepth::Sixteen,
            compression: crate::ipc::types::TiffCompression::Zip,
        };
        ex.enqueue_with(sink.clone(), vec![2], s, None).unwrap();
        assert!(ex.wait_idle(Duration::from_secs(60)));
        assert_eq!(lock(&sink.finished)[1].succeeded, 1);
        let t = crate::raw::raster::decode_linear(&web.join("b.tif"), crate::ipc::types::ImageFormat::Tiff, None);
        #[cfg(target_os = "macos")]
        assert_eq!(t.map(|i| (i.width, i.height, i.bit_depth)).unwrap(), (200, 100, 16));
        #[cfg(not(target_os = "macos"))]
        let _ = t;
    }

    #[test]
    fn cancel_queued_job_and_recover_interrupted() {
        let dir = tempfile::tempdir().unwrap();
        let ex = setup(dir.path());
        let sink = Arc::new(Sink::default());
        // Hold the worker "busy" so the job stays queued.
        lock(&ex.inner.state).worker = true;
        let job = ex.enqueue_with(sink.clone(), vec![1], settings(&dir.path().join("out")), None).unwrap();
        assert_eq!(ex.jobs().unwrap()[0].state, ExportJobState::Queued);
        ex.cancel_job(job.id).unwrap();
        let fin = lock(&sink.finished).clone();
        assert_eq!(fin.len(), 1);
        assert!(fin[0].cancelled && fin[0].elapsed_ms == 0);
        assert_eq!(ex.jobs().unwrap()[0].state, ExportJobState::Cancelled);
        ex.cancel_job(job.id).unwrap(); // finished: no-op
        assert_eq!(ex.cancel_job(999).unwrap_err().kind, ErrorKind::NotFound);
        assert_eq!(lock(&sink.finished).len(), 1);

        // A job left queued by a "previous session".
        let job2 = ex.enqueue_with(sink, vec![2], settings(&dir.path().join("out")), None).unwrap();
        lock(&ex.inner.state).queue.clear();
        ex.recover_interrupted().unwrap();
        let j = ex.jobs().unwrap().into_iter().find(|j| j.id == job2.id).unwrap();
        assert_eq!(j.state, ExportJobState::Interrupted);
        assert!(j.finished_at_ms.is_some());
        let caps = ex.capabilities();
        assert_eq!((caps.max_parallel, caps.memory_budget_mb), (4, 1024));
        assert_eq!(caps.formats.len(), 5);
    }

    #[test]
    fn plan_resolves_names_existing_files_and_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let ex = setup(dir.path());
        let conn = db::open(&ex.config.catalog_path).unwrap();
        let out = dir.path().join("out");
        let mut s = settings(&out);
        s.naming.template = "Smith-{seq:3}".into();
        s.naming.start_number = 5;
        let p = plan(&conn, &[2, 1], &s).unwrap();
        let web = out.join("Web");
        assert_eq!(p.output_dir.as_deref(), web.to_str());
        let paths: Vec<_> = p.files.iter().map(|f| f.path.clone().unwrap()).collect();
        assert_eq!(
            paths,
            [web.join("Smith-005.jpg"), web.join("Smith-006.jpg")].map(|p| p.to_string_lossy().into_owned())
        );
        assert!(!web.exists(), "plan never touches the disk");

        // Same name twice + an existing file.
        std::fs::create_dir_all(&web).unwrap();
        std::fs::write(web.join("same.jpg"), b"x").unwrap();
        s.naming.template = "same".into();
        let p = plan(&conn, &[1, 2], &s).unwrap();
        assert_eq!(p.existing, 2);
        let names: Vec<_> = p.files.iter().map(|f| f.path.clone().unwrap()).collect();
        assert!(names[0].ends_with("same-2.jpg") && names[1].ends_with("same-3.jpg"), "{names:?}");
        s.naming.collision = CollisionPolicy::Skip;
        let p = plan(&conn, &[1, 2], &s).unwrap();
        assert_eq!(p.files[0].path, None);
        assert!(p.files[1].path.as_deref().unwrap().ends_with("same-2.jpg"));

        // Source folder + subfolder.
        s.destination = ExportDestination::SourceFolder;
        s.naming.template = "{filename}".into();
        let p = plan(&conn, &[1], &s).unwrap();
        assert_eq!(p.output_dir, None);
        assert_eq!(p.files[0].path.as_deref(), dir.path().join("src/Web/a.jpg").to_str());
        assert_eq!(plan(&conn, &[7], &s).unwrap_err().kind, ErrorKind::NotFound);
        let _ = ExportPreset::builtins();
    }

    /// End-to-end on one real RAW (copied to a temp dir): every format, metadata and sizes.
    #[test]
    #[ignore = "needs sample RAWs ($SIEVE_SAMPLES)"]
    fn real_raw_exports_every_format() {
        let folder = std::env::var("SIEVE_SAMPLES").unwrap_or_else(|_| "/Users/gurjotsingh/Pictures/test RAWS".into());
        let raw = std::fs::read_dir(&folder)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| crate::raw::format_from_extension(p).is_some())
            .expect("no RAW in sample folder");
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::copy(&raw, src.join(raw.file_name().unwrap())).unwrap();
        let catalog = dir.path().join("cat.sqlite");
        {
            let mut conn = db::open(&catalog).unwrap();
            repo::import_folder(&mut conn, &src, &ImportOptions::raw_only(false)).unwrap();
        }
        let ex = Exporter::new(
            ExportConfig { catalog_path: catalog, memory_budget_mb: None },
            LutLibrary::new(dir.path().join("luts")),
        );
        let out = dir.path().join("out");
        for format in [
            ExportFormat::Jpeg { quality: 90, chroma_subsampling: ChromaSubsampling::Yuv444 },
            ExportFormat::Tiff {
                bit_depth: crate::ipc::types::BitDepth::Sixteen,
                compression: crate::ipc::types::TiffCompression::Lzw,
            },
            ExportFormat::Png { bit_depth: crate::ipc::types::BitDepth::Eight },
            ExportFormat::Webp { quality: 85, lossless: false },
            ExportFormat::Heic { quality: 80 },
        ] {
            let mut s = settings(&out);
            s.format = format;
            let sink = Arc::new(Sink::default());
            ex.enqueue_with(sink.clone(), vec![1], s, None).unwrap();
            assert!(ex.wait_idle(Duration::from_secs(120)));
            let fin = lock(&sink.finished).clone();
            assert_eq!(fin[0].succeeded, 1, "{:?}", fin[0].failed);
        }
        let files: Vec<String> = std::fs::read_dir(out.join("Web"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(files.len(), 5, "{files:?}");
    }
}
