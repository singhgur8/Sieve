//! Edited-preview cache (IPC v19.1): rendered previews of each edited photo's current develop
//! settings, so the Library grid, Loupe, filmstrip and scenes show edits, and switching
//! between edited photos in Develop never flashes the unedited embedded preview.
//!
//! - Files: `<cacheDir>/edited/<id>_<hash>_p.jpg` (long edge 2048, the Loupe / Develop
//!   placeholder) and `<id>_<hash>_t.jpg` (512, grid), `hash` = [`settings_hash`] of the
//!   stored adjustments. Only in the app cache dir, never next to the photos. One hash per
//!   image is kept; LRU eviction (by bytes, default [`DEFAULT_CACHE_MB`],
//!   `SIEVE_EDITED_CACHE_MB`) drops whole images; [`EditedPreviews::forget`] deletes the
//!   files of removed images (`remove_project`).
//! - Served by the `sieve` scheme at `/edited/<id>/<hash>/{thumb,preview}.jpg`
//!   (content-addressed, so cacheable forever); a miss answers 404 and queues a render.
//! - Regeneration: one low-priority worker thread (macOS QoS utility, own small rayon pool)
//!   that yields while interactive renders run. Triggers: every committed adjustments write
//!   ([`notify_saved`], called by `repo::save_adjustments`: edits, paste / sync, presets,
//!   scene apply, undo / redo / batch undo, XMP reads), `prepareDevelop` neighbours
//!   ([`EditedPreviews::ensure`], urgent), entries listed with a missing / stale preview
//!   ([`attach`], self-healing after restarts and evictions) and protocol misses.
//! - A Develop settled render (`main`, full quality, whole frame) of the stored settings is
//!   reused instead of re-rendering ([`EditedPreviews::note_settled`]).

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use rusqlite::{params_from_iter, Connection, OptionalExtension};

use crate::ipc::error::{AppError, AppResult};
use crate::ipc::events::EditedPreviewChanged;
use crate::ipc::types::{EditedPreview, ImageId, ParametricAdjustments, RawImageEntry};
use crate::lut::LutLibrary;
use crate::raw::preview::Rgb;

use super::{DevelopCache, SourceImage, RENDER_SCHEME};

/// Long edge of the edited grid thumbnail.
pub const THUMB_EDGE: u32 = 512;
/// Long edge of the edited Loupe / Develop preview.
pub const PREVIEW_EDGE: u32 = 2048;
/// JPEG quality of both files (4:2:0).
pub const JPEG_QUALITY: u8 = 82;
/// Default disk budget (~0.4 MB per photo: ~2500 edited photos).
pub const DEFAULT_CACHE_MB: u64 = 1024;
/// Overrides [`DEFAULT_CACHE_MB`].
pub const CACHE_ENV: &str = "SIEVE_EDITED_CACHE_MB";
/// Bumped when the render engine changes in a way that should invalidate cached previews.
const ENGINE_VERSION: u64 = 1;
/// A settled render this large or larger is reused as the preview file.
const SETTLED_MIN_EDGE: u32 = 1536;
/// Settled renders kept for reuse (8-bit RGB, ~8 MB each).
const SETTLED_KEEP: usize = 2;
/// Coalesces bursts of saves (slider release + settled render) into one regeneration.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(350);
/// The worker waits this long after the last interactive render before starting a job.
const INTERACTIVE_IDLE: Duration = Duration::from_millis(250);
/// Retry delay while a notified write is not committed yet.
const UNCOMMITTED_RETRY: Duration = Duration::from_millis(200);
/// Give up waiting for an uncommitted write after this many retries (rolled back); a later
/// listing heals it.
const MAX_TRIES: u8 = 50;

/// Where and how much (resolved at startup).
#[derive(Debug, Clone)]
pub struct EditedConfig {
    /// `<cacheDir>/edited`.
    pub dir: PathBuf,
    pub catalog_path: PathBuf,
    pub max_bytes: u64,
}

impl EditedConfig {
    /// `<cache_dir>/edited`, budget from `SIEVE_EDITED_CACHE_MB` (else the default).
    pub fn new(cache_dir: &Path, catalog_path: PathBuf) -> Self {
        let mb = std::env::var(CACHE_ENV).ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(DEFAULT_CACHE_MB);
        EditedConfig { dir: cache_dir.join("edited"), catalog_path, max_bytes: mb * 1024 * 1024 }
    }
}

/// Receives [`EditedPreviewChanged`] (the app emits it to the webview).
pub type Emitter = Arc<dyn Fn(EditedPreviewChanged) + Send + Sync>;

/// FNV-1a 64 of the engine version + the settings' JSON (stable across runs and builds).
pub fn settings_hash(adj: &ParametricAdjustments) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: u8| {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    ENGINE_VERSION.to_le_bytes().into_iter().for_each(&mut eat);
    serde_json::to_vec(adj).unwrap_or_default().into_iter().for_each(&mut eat);
    h
}

fn base_url() -> String {
    if cfg!(windows) {
        format!("http://{RENDER_SCHEME}.localhost")
    } else {
        format!("{RENDER_SCHEME}://localhost")
    }
}

/// The URLs of image `id`'s preview for settings hash `hash`.
pub fn preview_urls(id: ImageId, hash: u64) -> EditedPreview {
    let base = base_url();
    EditedPreview {
        thumb_url: format!("{base}/edited/{id}/{hash:016x}/thumb.jpg"),
        preview_url: format!("{base}/edited/{id}/{hash:016x}/preview.jpg"),
    }
}

/// `(id, hash, thumb?)` of `/edited/<id>/<hash>/{thumb,preview}.jpg`.
pub(super) fn parse_path(path: &str) -> Option<(ImageId, u64, bool)> {
    let mut parts = path.trim_start_matches('/').split('/');
    if parts.next()? != "edited" {
        return None;
    }
    let id: ImageId = parts.next()?.parse().ok()?;
    let hash = parts.next()?;
    if hash.len() != 16 {
        return None;
    }
    let hash = u64::from_str_radix(hash, 16).ok()?;
    let thumb = match parts.next()? {
        "thumb.jpg" => true,
        "preview.jpg" => false,
        _ => return None,
    };
    parts.next().is_none().then_some((id, hash, thumb))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug, Clone)]
struct Item {
    hash: u64,
    bytes: u64,
    stamp: u64,
    /// `adjustments.updated_at` the files were checked against (`None`: not since launch).
    updated_at: Option<i64>,
}

#[derive(Default)]
struct Index {
    items: HashMap<ImageId, Item>,
    clock: u64,
    bytes: u64,
}

impl Index {
    fn touch(&mut self, id: ImageId) {
        self.clock += 1;
        let now = self.clock;
        if let Some(it) = self.items.get_mut(&id) {
            it.stamp = now;
        }
    }
}

/// `(path, orientation, adjustments.neutral, adjustments.updated_at, thumbnails.status)`.
type ImageRow = (String, Option<u8>, Option<bool>, Option<i64>, Option<String>);

#[derive(Debug, Clone, Copy)]
struct Job {
    not_before: Instant,
    /// The write being waited for: `adjustments.updated_at` must be >= this.
    after: Option<i64>,
    tries: u8,
}

#[derive(Default)]
struct Queue {
    jobs: HashMap<ImageId, Job>,
    order: VecDeque<ImageId>,
}

struct Settled {
    id: ImageId,
    hash: u64,
    rgb: Arc<Rgb>,
}

struct Shared {
    config: EditedConfig,
    luts: LutLibrary,
    emit: Emitter,
    index: Mutex<Index>,
    queue: Mutex<Queue>,
    wake: Condvar,
    settled: Mutex<VecDeque<Settled>>,
    last_interactive: Mutex<Option<Instant>>,
    /// Jobs finished (tests / bench).
    processed: Mutex<u64>,
}

/// Handle to the cache + worker (cheap to clone). Created by
/// [`DevelopCache::enable_edited_previews`].
#[derive(Clone)]
pub struct EditedPreviews {
    shared: Arc<Shared>,
}

/// Caches by catalog path, so `repo` (which only has a connection) can notify and attach.
fn registry() -> &'static Mutex<Vec<(PathBuf, EditedPreviews)>> {
    static REG: OnceLock<Mutex<Vec<(PathBuf, EditedPreviews)>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(Vec::new()))
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// The cache registered for the catalog `conn` is open on, if any.
fn for_conn(conn: &Connection) -> Option<EditedPreviews> {
    let reg = lock(registry());
    if reg.is_empty() {
        return None;
    }
    let path = conn.path().filter(|p| !p.is_empty())?;
    let direct = Path::new(path);
    if let Some((_, e)) = reg.iter().find(|(p, _)| p == direct) {
        return Some(e.clone());
    }
    let c = canonical(direct);
    reg.iter().find(|(p, _)| *p == c).map(|(_, e)| e.clone())
}

/// Called by `repo::save_adjustments` after writing image `id`'s adjustments with
/// `updated_at` (possibly inside a transaction): regenerates its preview once committed.
pub fn notify_saved(conn: &Connection, id: ImageId, updated_at: i64) {
    if let Some(e) = for_conn(conn) {
        e.enqueue(id, Some(updated_at), Instant::now() + SAVE_DEBOUNCE, false);
    }
}

/// Fills `editedPreview` of edited `entries` from the cache (touching their LRU stamps)
/// and queues a render for those whose preview is missing or older than their settings.
pub fn attach(conn: &Connection, entries: &mut [RawImageEntry]) -> AppResult<()> {
    let Some(ed) = for_conn(conn) else {
        return Ok(());
    };
    let edited: Vec<ImageId> = entries.iter().filter(|e| e.has_edits).map(|e| e.id).collect();
    if edited.is_empty() {
        return Ok(());
    }
    let mut updated: HashMap<ImageId, i64> = HashMap::with_capacity(edited.len());
    for chunk in edited.chunks(500) {
        let ph = vec!["?"; chunk.len()].join(",");
        let mut stmt =
            conn.prepare(&format!("SELECT image_id, updated_at FROM adjustments WHERE image_id IN ({ph})"))?;
        let rows =
            stmt.query_map(params_from_iter(chunk.iter()), |r| Ok((r.get::<_, ImageId>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (id, at) = row?;
            updated.insert(id, at);
        }
    }
    let mut stale = Vec::new();
    {
        let mut index = lock(&ed.shared.index);
        for e in entries.iter_mut().filter(|e| e.has_edits) {
            index.touch(e.id);
            match index.items.get(&e.id) {
                Some(it) => {
                    e.edited_preview = Some(preview_urls(e.id, it.hash));
                    if it.updated_at != updated.get(&e.id).copied() {
                        stale.push(e.id);
                    }
                }
                None => stale.push(e.id),
            }
        }
    }
    for id in stale {
        ed.enqueue(id, None, Instant::now(), false);
    }
    Ok(())
}

impl EditedPreviews {
    /// Scans `config.dir` (creating it), registers the cache for its catalog and starts the
    /// worker. `develop` renders; `emit` receives every change.
    pub(super) fn start(config: EditedConfig, luts: LutLibrary, emit: Emitter, develop: DevelopCache) -> Self {
        if let Err(e) = std::fs::create_dir_all(&config.dir) {
            eprintln!("edited previews: {}: {e}", config.dir.display());
        }
        let ed = EditedPreviews {
            shared: Arc::new(Shared {
                index: Mutex::new(scan(&config.dir)),
                config,
                luts,
                emit,
                queue: Mutex::new(Queue::default()),
                wake: Condvar::new(),
                settled: Mutex::new(VecDeque::new()),
                last_interactive: Mutex::new(None),
                processed: Mutex::new(0),
            }),
        };
        ed.evict(None);
        {
            let mut reg = lock(registry());
            let key = canonical(&ed.shared.config.catalog_path);
            reg.retain(|(p, _)| *p != key);
            reg.push((key, ed.clone()));
        }
        let worker = ed.clone();
        let spawned = std::thread::Builder::new().name("edited-previews".into()).spawn(move || {
            lower_priority();
            worker.run(develop);
        });
        if let Err(e) = spawned {
            eprintln!("edited previews: worker: {e}");
        }
        ed
    }

    pub fn config(&self) -> &EditedConfig {
        &self.shared.config
    }

    /// Queues `ids` for a check / render now; `urgent` (Develop neighbours) go first.
    pub fn ensure(&self, ids: &[ImageId], urgent: bool) {
        for &id in ids {
            self.enqueue(id, None, Instant::now(), urgent);
        }
    }

    fn enqueue(&self, id: ImageId, after: Option<i64>, not_before: Instant, urgent: bool) {
        let mut q = lock(&self.shared.queue);
        let job = match q.jobs.get(&id) {
            Some(j) => Job {
                // A newer save pushes the job back (debounce); a check never delays a save.
                not_before: if after.is_some() { not_before.max(j.not_before) } else { j.not_before.min(not_before) },
                after: match (j.after, after) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                },
                tries: 0,
            },
            None => Job { not_before, after, tries: 0 },
        };
        let known = q.jobs.insert(id, job).is_some();
        if urgent {
            q.order.retain(|x| *x != id);
            q.order.push_front(id);
        } else if !known {
            q.order.push_back(id);
        }
        drop(q);
        self.shared.wake.notify_all();
    }

    /// An interactive render started: background jobs wait until the UI is idle.
    pub(super) fn note_interactive(&self) {
        *lock(&self.shared.last_interactive) = Some(Instant::now());
    }

    /// A settled Develop render of `adjustments` (whole frame, orientation + crop applied):
    /// reused as the preview if those are the stored settings when the job runs.
    pub(super) fn note_settled(&self, id: ImageId, adjustments: &ParametricAdjustments, rgb: Rgb) {
        if rgb.width.max(rgb.height) < SETTLED_MIN_EDGE {
            return;
        }
        let hash = settings_hash(adjustments);
        let mut s = lock(&self.shared.settled);
        s.retain(|x| x.id != id);
        s.push_back(Settled { id, hash, rgb: Arc::new(rgb) });
        while s.len() > SETTLED_KEEP {
            s.pop_front();
        }
    }

    fn settled(&self, id: ImageId, hash: u64) -> Option<Arc<Rgb>> {
        lock(&self.shared.settled).iter().find(|s| s.id == id && s.hash == hash).map(|s| s.rgb.clone())
    }

    /// Current preview of `id`, if cached (tests / commands).
    pub fn current(&self, id: ImageId) -> Option<(u64, EditedPreview)> {
        lock(&self.shared.index).items.get(&id).map(|it| (it.hash, preview_urls(id, it.hash)))
    }

    /// Bytes on disk (tests / bench).
    pub fn bytes(&self) -> u64 {
        lock(&self.shared.index).bytes
    }

    /// Jobs finished so far (tests / bench).
    pub fn processed(&self) -> u64 {
        *lock(&self.shared.processed)
    }

    /// Queued jobs (tests / bench).
    pub fn pending(&self) -> usize {
        lock(&self.shared.queue).jobs.len()
    }

    fn file(&self, id: ImageId, hash: u64, thumb: bool) -> PathBuf {
        let kind = if thumb { 't' } else { 'p' };
        self.shared.config.dir.join(format!("{id}_{hash:016x}_{kind}.jpg"))
    }

    /// Bytes of a cached file for the protocol (touching the LRU stamp); `None` = not
    /// cached (a render is queued).
    pub(super) fn read(&self, id: ImageId, hash: u64, thumb: bool) -> Option<Vec<u8>> {
        match std::fs::read(self.file(id, hash, thumb)) {
            Ok(b) => {
                lock(&self.shared.index).touch(id);
                Some(b)
            }
            Err(_) => {
                self.ensure(&[id], true);
                None
            }
        }
    }

    /// Deletes everything cached for `ids` (images removed from the catalog).
    pub fn forget(&self, ids: &[ImageId]) {
        {
            let mut q = lock(&self.shared.queue);
            q.jobs.retain(|id, _| !ids.contains(id));
            q.order.retain(|id| !ids.contains(id));
        }
        lock(&self.shared.settled).retain(|s| !ids.contains(&s.id));
        let removed: Vec<(ImageId, Item)> = {
            let mut index = lock(&self.shared.index);
            let out: Vec<_> = ids.iter().filter_map(|id| index.items.remove(id).map(|it| (*id, it))).collect();
            index.bytes -= out.iter().map(|(_, it)| it.bytes).sum::<u64>();
            out
        };
        for (id, it) in removed {
            self.remove_files(id, it.hash);
        }
    }

    fn remove_files(&self, id: ImageId, hash: u64) {
        for thumb in [true, false] {
            let _ = std::fs::remove_file(self.file(id, hash, thumb));
        }
    }

    /// Evicts least recently used images (never `keep`) until within the budget.
    fn evict(&self, keep: Option<ImageId>) {
        let victims: Vec<(ImageId, u64)> = {
            let mut index = lock(&self.shared.index);
            let mut out = Vec::new();
            while index.bytes > self.shared.config.max_bytes {
                let victim = index
                    .items
                    .iter()
                    .filter(|(id, _)| Some(**id) != keep)
                    .min_by_key(|(_, it)| it.stamp)
                    .map(|(id, _)| *id);
                let Some(id) = victim else { break };
                if let Some(it) = index.items.remove(&id) {
                    index.bytes -= it.bytes;
                    out.push((id, it.hash));
                }
            }
            out
        };
        for (id, hash) in victims {
            self.remove_files(id, hash);
        }
    }

    // --- worker -----------------------------------------------------------------------

    fn next_job(&self) -> (ImageId, Job) {
        let mut q = lock(&self.shared.queue);
        loop {
            let now = Instant::now();
            let ready = q.order.iter().position(|id| q.jobs.get(id).is_some_and(|j| j.not_before <= now));
            if let Some(i) = ready {
                let id = q.order.remove(i).unwrap_or_default();
                if let Some(job) = q.jobs.remove(&id) {
                    return (id, job);
                }
                continue;
            }
            let wait = q.jobs.values().map(|j| j.not_before.saturating_duration_since(now)).min();
            q = match wait {
                Some(d) => {
                    self.shared
                        .wake
                        .wait_timeout(q, d.max(Duration::from_millis(1)))
                        .unwrap_or_else(|e| e.into_inner())
                        .0
                }
                None => self.shared.wake.wait(q).unwrap_or_else(|e| e.into_inner()),
            };
        }
    }

    fn wait_for_idle(&self) {
        loop {
            let last = *lock(&self.shared.last_interactive);
            let idle = last.map_or(INTERACTIVE_IDLE, |t| t.elapsed());
            if idle >= INTERACTIVE_IDLE {
                return;
            }
            std::thread::sleep(INTERACTIVE_IDLE - idle);
        }
    }

    fn run(&self, develop: DevelopCache) {
        let mut conn: Option<Connection> = None;
        loop {
            let (id, job) = self.next_job();
            self.wait_for_idle();
            if conn.is_none() {
                match super::masks::open_catalog_read_only(&self.shared.config.catalog_path) {
                    Ok(c) => conn = Some(c),
                    Err(e) => {
                        eprintln!("edited previews: catalog: {}", e.message);
                        std::thread::sleep(Duration::from_secs(1));
                        self.retry(id, job, Duration::from_secs(1));
                        continue;
                    }
                }
            }
            let Some(c) = conn.as_ref() else { continue };
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.process(c, &develop, id, job)));
            match outcome {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    eprintln!("edited preview {id}: {}", e.message);
                    if e.kind == crate::ipc::error::ErrorKind::Database {
                        conn = None;
                    }
                }
                Err(_) => eprintln!("edited preview {id}: panicked"),
            }
            *lock(&self.shared.processed) += 1;
        }
    }

    fn retry(&self, id: ImageId, job: Job, delay: Duration) {
        if job.tries >= MAX_TRIES {
            return;
        }
        let mut q = lock(&self.shared.queue);
        if q.jobs.contains_key(&id) {
            return; // a newer request supersedes this one
        }
        q.jobs.insert(id, Job { not_before: Instant::now() + delay, after: job.after, tries: job.tries + 1 });
        q.order.push_back(id);
        drop(q);
        self.shared.wake.notify_all();
    }

    /// Brings image `id`'s cached preview in line with its stored settings.
    fn process(&self, conn: &Connection, develop: &DevelopCache, id: ImageId, job: Job) -> AppResult<()> {
        let row: Option<ImageRow> = conn
            .query_row(
                "SELECT i.path, i.orientation, a.neutral, a.updated_at, t.status
                 FROM images i LEFT JOIN adjustments a ON a.image_id = i.id
                 LEFT JOIN thumbnails t ON t.image_id = i.id WHERE i.id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let Some((path, orientation, neutral, updated_at, thumb_status)) = row else {
            self.forget(&[id]); // removed from the catalog
            return Ok(());
        };
        if let Some(after) = job.after {
            if updated_at.is_none_or(|u| u < after) {
                self.retry(id, job, UNCOMMITTED_RETRY); // not committed yet (or rolled back)
                return Ok(());
            }
        }
        if neutral != Some(false) {
            let had = lock(&self.shared.index).items.contains_key(&id);
            if had {
                self.forget(&[id]);
                (self.shared.emit)(EditedPreviewChanged { image_id: id, preview: None });
            }
            return Ok(());
        }
        if thumb_status.as_deref() == Some("pending") {
            // Orientation is not settled until extraction finished.
            self.retry(id, job, Duration::from_secs(2));
            return Ok(());
        }
        let adjustments = crate::db::repo::get_adjustments(conn, id)?;
        let hash = settings_hash(&adjustments);
        {
            let mut index = lock(&self.shared.index);
            if let Some(it) = index.items.get_mut(&id) {
                if it.hash == hash && self.file(id, hash, true).exists() && self.file(id, hash, false).exists() {
                    it.updated_at = updated_at;
                    return Ok(());
                }
            }
        }
        let rgb = match self.settled(id, hash) {
            Some(rgb) => rgb,
            None => {
                let src = SourceImage { id, path: PathBuf::from(path), orientation };
                let img = edited_pool()
                    .install(|| develop.render_detached(&src, &adjustments, PREVIEW_EDGE, &self.shared.luts))?;
                Arc::new(Rgb { width: img.width, height: img.height, pixels: img.rgb })
            }
        };
        let (preview, thumb) = edited_pool().install(|| encode_pair(&rgb))?;
        drop(rgb);
        // A newer save arrived meanwhile: its job renders the newer settings; still publish
        // this one (closer than nothing) unless the image became unedited.
        let bytes = (preview.len() + thumb.len()) as u64;
        write_atomic(&self.file(id, hash, false), &preview)?;
        write_atomic(&self.file(id, hash, true), &thumb)?;
        let old = {
            let mut index = lock(&self.shared.index);
            index.clock += 1;
            let stamp = index.clock;
            let old = index.items.insert(id, Item { hash, bytes, stamp, updated_at });
            if let Some(o) = &old {
                index.bytes -= o.bytes;
            }
            index.bytes += bytes;
            old
        };
        if let Some(o) = old.filter(|o| o.hash != hash) {
            self.remove_files(id, o.hash);
        }
        self.evict(Some(id));
        (self.shared.emit)(EditedPreviewChanged { image_id: id, preview: Some(preview_urls(id, hash)) });
        Ok(())
    }
}

/// `(2048 px JPEG, 512 px JPEG)` of a rendered frame.
fn encode_pair(rgb: &Rgb) -> AppResult<(Vec<u8>, Vec<u8>)> {
    let mut resizer = fast_image_resize::Resizer::new();
    let big = crate::raw::preview::fit(rgb, PREVIEW_EDGE, &mut resizer).map_err(AppError::internal)?;
    let preview =
        crate::raw::turbo::encode_rgb(&big.pixels, big.width, big.height, JPEG_QUALITY).map_err(AppError::internal)?;
    let small = crate::raw::preview::fit(&big, THUMB_EDGE, &mut resizer).map_err(AppError::internal)?;
    let thumb = crate::raw::turbo::encode_rgb(&small.pixels, small.width, small.height, JPEG_QUALITY)
        .map_err(AppError::internal)?;
    Ok((preview, thumb))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let tmp = path.with_extension("jpg.tmp");
    std::fs::write(&tmp, bytes)
        .map_err(|e| AppError::new(crate::ipc::error::ErrorKind::Io, format!("{}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        AppError::new(crate::ipc::error::ErrorKind::Io, format!("{}: {e}", path.display()))
    })
}

/// Index of the files in `dir` (newest file per image wins; others and temp files are
/// deleted). Stamps follow file mtimes so the LRU order survives restarts roughly.
fn scan(dir: &Path) -> Index {
    let mut found: HashMap<ImageId, Vec<(u64, u64, std::time::SystemTime, bool)>> = HashMap::new();
    let Ok(read) = std::fs::read_dir(dir) else {
        return Index::default();
    };
    for e in read.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        let parsed = name.strip_suffix(".jpg").and_then(|stem| {
            let mut p = stem.split('_');
            let id: ImageId = p.next()?.parse().ok()?;
            let hash = p.next().filter(|h| h.len() == 16).and_then(|h| u64::from_str_radix(h, 16).ok())?;
            let thumb = match p.next()? {
                "t" => true,
                "p" => false,
                _ => return None,
            };
            p.next().is_none().then_some((id, hash, thumb))
        });
        let Some((id, hash, thumb)) = parsed else {
            let _ = std::fs::remove_file(&path); // temp files of an interrupted write
            continue;
        };
        let meta = e.metadata().ok();
        let len = meta.as_ref().map_or(0, |m| m.len());
        let mtime = meta.and_then(|m| m.modified().ok()).unwrap_or(std::time::UNIX_EPOCH);
        found.entry(id).or_default().push((hash, len, mtime, thumb));
    }
    let mut items: Vec<(ImageId, u64, u64, std::time::SystemTime)> = Vec::new();
    for (id, files) in found {
        let newest = files.iter().max_by_key(|f| f.2).map(|f| f.0).unwrap_or_default();
        let complete = files.iter().any(|f| f.0 == newest && f.3) && files.iter().any(|f| f.0 == newest && !f.3);
        for f in &files {
            if f.0 != newest || !complete {
                let kind = if f.3 { 't' } else { 'p' };
                let _ = std::fs::remove_file(dir.join(format!("{id}_{:016x}_{kind}.jpg", f.0)));
            }
        }
        if complete {
            let bytes = files.iter().filter(|f| f.0 == newest).map(|f| f.1).sum();
            let mtime = files.iter().filter(|f| f.0 == newest).map(|f| f.2).max().unwrap_or(std::time::UNIX_EPOCH);
            items.push((id, newest, bytes, mtime));
        }
    }
    items.sort_by_key(|i| i.3);
    let mut index = Index::default();
    for (id, hash, bytes, _) in items {
        index.clock += 1;
        index.bytes += bytes;
        index.items.insert(id, Item { hash, bytes, stamp: index.clock, updated_at: None });
    }
    index
}

/// Low-priority work: below the preview and prefetch pools (macOS QoS "utility").
fn lower_priority() {
    #[cfg(target_os = "macos")]
    // SAFETY: sets the calling thread's own QoS class; no pointers involved.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
    }
}

/// Rayon pool of edited-preview renders: a quarter of the cores (min 2), utility QoS.
fn edited_pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        rayon::ThreadPoolBuilder::new()
            .num_threads((cores / 4).max(2))
            .thread_name(|i| format!("edited-preview-{i}"))
            .start_handler(|_| lower_priority())
            .build()
            .expect("edited preview thread pool")
    })
}

#[cfg(test)]
#[path = "edited_tests.rs"]
mod tests;
