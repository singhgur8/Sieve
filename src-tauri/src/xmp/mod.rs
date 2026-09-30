//! XMP sidecar sync (Phase 4). Owned by rust-engine-dev. The architect fixed only the
//! public surface used by `ipc::commands` and `lib.rs` (`XmpSyncConfig`, `SyncPolicy`,
//! `sidecar_path`, `XmpSync::{new, is_running, notify, write_images, read_images,
//! refresh_folder}`); everything behind it is free to change.
//!
//! Mapping (catalog -> sidecar, Lightroom/Bridge-compatible; see `docs/architecture.md`):
//! - `pick = reject`            -> `xmp:Rating = -1` (stars are not representable then)
//! - otherwise                  -> `xmp:Rating = <rating 0..=5>`
//! - `pick = pick`              -> `xmp:Label = "Pick"` (wins over a colour label)
//! - else `colorLabel = Some(c)`-> `xmp:Label = "Red" | "Yellow" | "Green" | "Blue" | "Purple"`
//! - else                       -> remove `xmp:Label` only if it currently holds "Pick" or one of
//!   those five names (custom label sets are preserved)
//! - non-suppressed tags        -> `lr:hierarchicalSubject` item `Sieve|<tag>` and `dc:subject`
//!   item `<tag>` (snake_case wire value). On write, first remove every `Sieve|*` item, and the
//!   `dc:subject` leaf of each removed item; other keywords are kept.
//! - update `xmp:MetadataDate` (so Lightroom notices the change); keep every other field,
//!   namespace and packet as-is. Create a minimal `x:xmpmeta` packet if no sidecar exists.
//!
//! Sidecar -> catalog (reads): `xmp:Rating -1` -> `pick = reject` (rating unchanged);
//! `0..=5` -> rating, and `pick = pick` iff `xmp:Label == "Pick"` else `unflagged`;
//! label names above -> `colorLabel` ("Pick"/absent/unknown -> `None`). `Sieve|*` keywords
//! are NOT read back (tags are catalog -> sidecar only; analysis regenerates them). A missing
//! `xmp:Rating` reads as 0.
//!
//! Sync bookkeeping (migration 0004, columns on `images`): triggers set `xmp_dirty = 1` and
//! `meta_updated_at` on any change to rating/pick/color_label/visible tags. After a successful
//! write or read set `xmp_dirty = 0, xmp_synced_at = now, xmp_mtime_ms = <sidecar mtime>,
//! xmp_error = NULL` (a read changes rating/pick/label first, which re-fires the trigger, so
//! clear the flag after applying). On failure set `xmp_error` and leave `xmp_dirty`.
//!
//! Conflict policy ([`SyncPolicy::NewerWins`], used by auto-sync): an image is "externally
//! modified" when its sidecar mtime differs from `xmp_mtime_ms`. Dirty + not externally
//! modified -> write. Dirty + externally modified -> the newer side wins: sidecar mtime >
//! `meta_updated_at` -> read, else write. Explicit commands force a direction.
//!
//! Writes must be crash-safe: write `<name>.xmp.tmp` in the same dir, fsync, rename.
//! Never touch anything under `~/Pictures` in tests: use `test-data/` copies / tempdirs.
//!
//! Implementation notes:
//! - `packet`: XML handling. `quick-xml` tokenizes (and validates) the packet; we index the
//!   RDF structure with exact byte spans and apply splices, so unrelated content is kept
//!   byte-for-byte. Attribute-form (Lightroom) and element-form (exiftool/Capture One)
//!   properties, `rdf:Bag`/`rdf:Seq`, several `rdf:Description` blocks, any prefixes.
//! - `store`: the module's SQL.
//! - A write only clears `xmp_dirty` if `meta_updated_at` is unchanged since the row was
//!   loaded, so a rating made during the write is not lost.
//! - Auto-sync: `notify` (re)arms a deadline; one `xmp-sync` thread sleeps until it passes,
//!   runs a pass on its own connection, and exits when no new deadline was set. File I/O is
//!   serialized with explicit commands by `io_lock`. When newer-wins picks the sidecar and
//!   the catalog's visible tags differ from its `Sieve|*` keywords, the merged state is
//!   written back so tags reach the sidecar too.

pub mod crs;
pub mod packet;
mod store;

pub use packet::{Desired, PacketError, SidecarValues};

use std::fs;
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use tauri::{AppHandle, Runtime};
use tauri_specta::Event;

use crate::db::{self, repo};
use crate::develop;
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::events::{XmpSynced, XmpWriteFailed};
use crate::ipc::types::{ColorLabel, FolderId, ImageId, ParametricAdjustments, PickFlag, XmpFailure, XmpSyncReport};

use store::ImageRow;

/// Auto-sync waits this long after the last `notify` before writing, so a burst of
/// keyboard ratings becomes one pass.
pub const DEBOUNCE: Duration = Duration::from_millis(1000);

/// Resolved at startup.
#[derive(Debug, Clone)]
pub struct XmpSyncConfig {
    /// Catalog file; sync work opens its own connection (never holds the command mutex
    /// during file I/O).
    pub catalog_path: PathBuf,
}

/// Which side wins for the XMP-mapped fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncPolicy {
    /// `write_xmp`: catalog values are written regardless of sidecar changes.
    CatalogWins,
    /// `read_xmp` / import: sidecar values replace the catalog's.
    SidecarWins,
    /// Auto-sync: see the module docs.
    NewerWins,
}

/// RAW (Lightroom convention): `DSC0001.ARW` -> `DSC0001.xmp` in the same directory.
/// Non-RAW sources (v9): `IMG_1.JPG` -> `IMG_1.JPG.xmp`. Lightroom embeds XMP into JPEG/TIFF
/// (and ignores sidecars for them), which Sieve never does to originals; appending keeps a
/// JPEG's sidecar from colliding with (and clobbering) the same-stem RAW's `DSCF1234.xmp`.
/// (An existing sidecar whose extension differs only in case, e.g. `.XMP`, should be
/// reused by the implementation; on APFS it is the same file anyway.)
pub fn sidecar_path(image_path: &Path) -> PathBuf {
    let raw = crate::raw::format_from_extension(image_path).is_none_or(|f| f.is_raw());
    if raw {
        image_path.with_extension("xmp")
    } else {
        let mut name = image_path.as_os_str().to_owned();
        name.push(".xmp");
        PathBuf::from(name)
    }
}

/// The sidecar to use for `image_path`: an existing `.xmp` / `.XMP`, else [`sidecar_path`].
pub fn resolve_sidecar(image_path: &Path) -> PathBuf {
    let lower = sidecar_path(image_path);
    if lower.exists() {
        return lower;
    }
    let upper = lower.with_extension("XMP");
    if upper.exists() {
        return upper;
    }
    lower
}

/// Receives auto-sync events; implemented for `AppHandle` and by tests.
pub trait XmpSink: Send + Sync {
    fn synced(&self, event: XmpSynced);
    fn failed(&self, event: XmpWriteFailed);
}

impl<R: Runtime> XmpSink for AppHandle<R> {
    fn synced(&self, event: XmpSynced) {
        let _ = event.emit(self);
    }
    fn failed(&self, event: XmpWriteFailed) {
        let _ = event.emit(self);
    }
}

#[derive(Default)]
struct DebounceState {
    /// Run a pass once this instant has passed.
    deadline: Option<Instant>,
    /// A worker thread exists (sleeping on the deadline or running a pass).
    alive: bool,
}

/// Managed Tauri state for sidecar sync. Cheap to clone (shared state behind `Arc`).
#[derive(Clone)]
pub struct XmpSync {
    config: XmpSyncConfig,
    running: Arc<AtomicBool>,
    debounce: Duration,
    state: Arc<(Mutex<DebounceState>, Condvar)>,
    /// Serializes sidecar file I/O between commands and the auto-sync worker.
    io_lock: Arc<Mutex<()>>,
}

impl XmpSync {
    pub fn new(config: XmpSyncConfig) -> Self {
        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            debounce: DEBOUNCE,
            state: Arc::new((Mutex::new(DebounceState::default()), Condvar::new())),
            io_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Overrides the auto-sync debounce (tests).
    pub fn with_debounce(mut self, debounce: Duration) -> Self {
        self.debounce = debounce;
        self
    }

    pub fn config(&self) -> &XmpSyncConfig {
        &self.config
    }

    /// The auto-sync writer is working.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// XMP-mapped values may have changed. If `catalog_meta.xmp_auto_sync = '1'`, (re)arms
    /// the debounced background pass that syncs every `xmp_dirty` image with
    /// [`SyncPolicy::NewerWins`], emitting `XmpSynced` per pass and `XmpWriteFailed` per
    /// failure. No-op when auto-sync is off. Returns immediately; never panics.
    /// Called after user culling writes, when auto-sync is enabled, on launch and on
    /// `AnalysisFinished`.
    pub fn notify(&self, app: &AppHandle) {
        self.notify_with(app.clone());
    }

    /// [`Self::notify`] with any event sink. The auto-sync flag is checked by the worker
    /// when the debounce expires (so `notify` never touches the database).
    pub fn notify_with<S: XmpSink + 'static>(&self, sink: S) {
        let (lock, cvar) = &*self.state;
        let mut st = lock_ignore_poison(lock);
        st.deadline = Some(Instant::now() + self.debounce);
        if st.alive {
            cvar.notify_all();
            return;
        }
        st.alive = true;
        drop(st);
        let this = self.clone();
        let spawned = std::thread::Builder::new().name("xmp-sync".into()).spawn(move || this.worker_loop(sink));
        if spawned.is_err() {
            lock_ignore_poison(lock).alive = false;
        }
    }

    fn worker_loop<S: XmpSink>(&self, sink: S) {
        let (lock, cvar) = &*self.state;
        loop {
            {
                let mut st = lock_ignore_poison(lock);
                loop {
                    match st.deadline {
                        None => {
                            st.alive = false;
                            return;
                        }
                        Some(d) => {
                            let now = Instant::now();
                            if now >= d {
                                st.deadline = None;
                                break;
                            }
                            st = match cvar.wait_timeout(st, d - now) {
                                Ok((g, _)) => g,
                                Err(e) => e.into_inner().0,
                            };
                        }
                    }
                }
            }
            self.running.store(true, Ordering::SeqCst);
            let result = catch_unwind(AssertUnwindSafe(|| self.auto_pass(&sink)));
            self.running.store(false, Ordering::SeqCst);
            match result {
                Ok(Err(e)) => eprintln!("xmp auto-sync pass failed: {e}"),
                Err(_) => eprintln!("xmp auto-sync pass panicked"),
                Ok(Ok(())) => {}
            }
        }
    }

    /// One auto-sync pass over every dirty image ([`SyncPolicy::NewerWins`]).
    fn auto_pass(&self, sink: &dyn XmpSink) -> AppResult<()> {
        let mut conn = db::open(&self.config.catalog_path)?;
        if !repo::xmp_auto_sync(&conn)? {
            return Ok(());
        }
        let ids = store::dirty_ids(&conn)?;
        if ids.is_empty() {
            return Ok(());
        }
        let mut event = XmpSynced { written: Vec::new(), read: Vec::new() };
        for id in ids {
            let Some(row) = store::load(&conn, id)? else { continue };
            match self.sync_one(&mut conn, &row, SyncPolicy::NewerWins) {
                Ok(Outcome::Written) => event.written.push(id),
                Ok(Outcome::Read { .. }) => event.read.push(id),
                Ok(Outcome::Skipped) => {}
                Err(reason) => {
                    store::mark_failed(&conn, id, &reason)?;
                    sink.failed(XmpWriteFailed { image_id: id, reason });
                }
            }
        }
        sink.synced(event);
        Ok(())
    }

    /// `write_xmp`: writes sidecars for `ids` now ([`SyncPolicy::CatalogWins`]), dirty or not.
    /// Unknown ids fail the whole call with `not_found` before any file is touched; per-file
    /// errors go to `XmpSyncReport.failed`. Blocking (call from `spawn_blocking`).
    pub fn write_images(&self, ids: &[ImageId]) -> AppResult<XmpSyncReport> {
        self.run_explicit(ids, SyncPolicy::CatalogWins)
    }

    /// `write_xmp_all_dirty`: [`Self::write_images`] over every `xmp_dirty` image of `folder`
    /// (all folders for `None`). Works whether or not auto-sync is on. Blocking.
    pub fn write_dirty(&self, folder: Option<FolderId>) -> AppResult<XmpSyncReport> {
        let ids = store::dirty_ids_in(&db::open(&self.config.catalog_path)?, folder)?;
        if ids.is_empty() {
            return Ok(XmpSyncReport::default());
        }
        self.write_images(&ids)
    }

    /// `read_xmp`: reads sidecars for `ids` into the catalog ([`SyncPolicy::SidecarWins`]);
    /// images without a sidecar count as `skipped`. Unknown ids -> `not_found`.
    /// Blocking (call from `spawn_blocking`).
    pub fn read_images(&self, ids: &[ImageId]) -> AppResult<XmpSyncReport> {
        self.run_explicit(ids, SyncPolicy::SidecarWins)
    }

    fn run_explicit(&self, ids: &[ImageId], policy: SyncPolicy) -> AppResult<XmpSyncReport> {
        let mut conn = db::open(&self.config.catalog_path)?;
        store::ensure_exist(&conn, ids)?;
        let mut report = XmpSyncReport::default();
        for &id in ids {
            let row = store::load(&conn, id)?.ok_or_else(|| AppError::not_found(format!("image {id}")))?;
            match self.sync_one(&mut conn, &row, policy) {
                Ok(Outcome::Written) => report.succeeded += 1,
                Ok(Outcome::Read { changed }) => {
                    report.succeeded += 1;
                    if changed {
                        report.changed.push(id);
                    }
                }
                Ok(Outcome::Skipped) => report.skipped += 1,
                Err(reason) => {
                    store::mark_failed(&conn, id, &reason)?;
                    report.failed.push(XmpFailure { image_id: id, reason });
                }
            }
        }
        Ok(report)
    }

    /// Import hook: for every image in `folder_id` that is not `xmp_dirty` and whose sidecar
    /// mtime differs from `xmp_mtime_ms` (includes never-synced images with a sidecar), read
    /// the sidecar ([`SyncPolicy::SidecarWins`]). Returns the number of sidecars read
    /// (`ImportSummary.sidecarsRead`). Must not fail the import for per-file errors
    /// (record them in `xmp_error`). Blocking.
    pub fn refresh_folder(&self, folder_id: FolderId) -> AppResult<u32> {
        let mut conn = db::open(&self.config.catalog_path)?;
        let mut read = 0;
        for (id, raw, recorded) in store::clean_images_in_folder(&conn, folder_id)? {
            let Some(mtime) = file_mtime_ms(&resolve_sidecar(&raw)) else { continue };
            if Some(mtime) == recorded {
                continue;
            }
            let Some(row) = store::load(&conn, id)? else { continue };
            match self.sync_one(&mut conn, &row, SyncPolicy::SidecarWins) {
                Ok(Outcome::Read { .. }) => read += 1,
                Ok(_) => {}
                Err(reason) => store::mark_failed(&conn, id, &reason)?,
            }
        }
        Ok(read)
    }

    /// Syncs one image. `Err` carries a per-file reason (catalog errors included).
    fn sync_one(&self, conn: &mut Connection, row: &ImageRow, policy: SyncPolicy) -> Result<Outcome, String> {
        let _io = lock_ignore_poison(&self.io_lock);
        let path = resolve_sidecar(&row.path);
        let action = match policy {
            SyncPolicy::CatalogWins => Action::Write,
            SyncPolicy::SidecarWins => {
                if !path.exists() {
                    return Ok(Outcome::Skipped);
                }
                Action::Read
            }
            SyncPolicy::NewerWins => decide(file_mtime_ms(&path), row.xmp_mtime_ms, row.meta_updated_at),
        };
        match action {
            Action::Write => {
                let tags = store::visible_tags(conn, row.id).map_err(|e| e.message)?;
                let develop = store::develop_settings(conn, row.id).map_err(|e| e.message)?;
                write_sidecar(&path, row, &tags, develop.as_ref())?;
                store::mark_written(conn, row, file_mtime_ms(&path)).map_err(|e| e.message)?;
                Ok(Outcome::Written)
            }
            Action::Read => {
                let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                let values = packet::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
                if let Some(e) = &values.develop_error {
                    // Ratings still sync; develop settings stay as they are in the catalog.
                    eprintln!("{}: develop settings not imported: {e}", path.display());
                }
                // Develop settings first: the history commit re-fires the dirty trigger,
                // which `apply_read` then clears.
                let mut develop_changed = false;
                if let Some(adj) = &values.develop {
                    let current = repo::get_adjustments(conn, row.id).map_err(|e| e.message)?;
                    if *adj != current {
                        develop::history::commit(conn, row.id, adj, develop::history::LABEL_READ_XMP)
                            .map_err(|e| e.message)?;
                        develop_changed = true;
                    }
                }
                store::set_develop_warnings(conn, row.id, &values.warnings).map_err(|e| e.message)?;
                let (rating, pick, label) = catalog_values(&values, row.rating);
                let changed = store::apply_read(conn, row.id, rating, pick, label, file_mtime_ms(&path))
                    .map_err(|e| e.message)?;
                if policy == SyncPolicy::NewerWins {
                    // Rating/pick/label came from the sidecar; if our tags differ from the
                    // sidecar's, write them too (a no-op for the values just read).
                    let tags = store::visible_tags(conn, row.id).map_err(|e| e.message)?;
                    if !same_tags(&values, &tags) {
                        let fresh = store::load(conn, row.id).map_err(|e| e.message)?.ok_or("image vanished")?;
                        let develop = store::develop_settings(conn, row.id).map_err(|e| e.message)?;
                        write_sidecar(&path, &fresh, &tags, develop.as_ref())?;
                        store::mark_written(conn, &fresh, file_mtime_ms(&path)).map_err(|e| e.message)?;
                    }
                }
                Ok(Outcome::Read { changed: changed || develop_changed })
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Written,
    Read { changed: bool },
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Write,
    Read,
}

/// [`SyncPolicy::NewerWins`] for a dirty image.
fn decide(sidecar_mtime: Option<i64>, recorded_mtime: Option<i64>, meta_updated_at: Option<i64>) -> Action {
    match sidecar_mtime {
        None => Action::Write,
        Some(m) if Some(m) == recorded_mtime => Action::Write,
        Some(m) if m > meta_updated_at.unwrap_or(i64::MIN) => Action::Read,
        Some(_) => Action::Write,
    }
}

fn lock_ignore_poison<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn file_mtime_ms(path: &Path) -> Option<i64> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    Some(modified.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64)
}

/// Catalog -> sidecar values (module-doc mapping).
fn desired(row: &ImageRow, tags: &[String], develop: Option<&ParametricAdjustments>) -> Desired {
    let rating = if row.pick == PickFlag::Reject { -1 } else { i32::from(row.rating.min(5)) };
    let label = if row.pick == PickFlag::Pick { Some("Pick") } else { row.color_label.map(label_name) };
    Desired {
        rating,
        label,
        tags: tags.to_vec(),
        metadata_date: iso8601_utc(SystemTime::now()),
        develop: develop.map(crs::encode).unwrap_or_default(),
    }
}

/// Sidecar -> catalog `(rating, pick, colorLabel)`. `current_rating` is kept for rejects.
fn catalog_values(v: &SidecarValues, current_rating: u8) -> (u8, PickFlag, Option<ColorLabel>) {
    let label = v.label.as_deref().map(str::trim);
    let color = label.and_then(parse_label);
    let is_pick = label.is_some_and(|l| l.eq_ignore_ascii_case("Pick"));
    match v.rating.unwrap_or(0) {
        r if r < 0 => (current_rating, PickFlag::Reject, color),
        r => (r.min(5) as u8, if is_pick { PickFlag::Pick } else { PickFlag::Unflagged }, color),
    }
}

fn same_tags(v: &SidecarValues, tags: &[String]) -> bool {
    let prefix = format!("{}|", packet::KEYWORD_ROOT);
    let mut ours: Vec<&str> = v.hierarchical_subjects.iter().filter_map(|s| s.strip_prefix(&prefix)).collect();
    ours.sort_unstable();
    ours.dedup();
    let mut want: Vec<&str> = tags.iter().map(String::as_str).collect();
    want.sort_unstable();
    want.dedup();
    ours == want
}

/// Lightroom's default label-set names.
pub fn label_name(c: ColorLabel) -> &'static str {
    match c {
        ColorLabel::Red => "Red",
        ColorLabel::Yellow => "Yellow",
        ColorLabel::Green => "Green",
        ColorLabel::Blue => "Blue",
        ColorLabel::Purple => "Purple",
    }
}

fn parse_label(s: &str) -> Option<ColorLabel> {
    ColorLabel::ALL.iter().copied().find(|&c| label_name(c).eq_ignore_ascii_case(s))
}

/// Merges the catalog state into the sidecar (creating it if missing) and replaces it
/// atomically (`<name>.xmp.tmp`, fsync, rename).
fn write_sidecar(
    path: &Path,
    row: &ImageRow,
    tags: &[String],
    develop: Option<&ParametricAdjustments>,
) -> Result<(), String> {
    let shown = path.display();
    let existing = match fs::read(path) {
        Ok(bytes) => Some(String::from_utf8(bytes).map_err(|_| format!("{shown}: sidecar is not UTF-8"))?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("{shown}: {e}")),
    };
    let merged =
        packet::merge(existing.as_deref(), &desired(row, tags, develop)).map_err(|e| format!("{shown}: {e}"))?;
    write_atomic(path, merged.as_bytes()).map_err(|e| format!("{shown}: {e}"))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    let result = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        if let Ok(meta) = fs::metadata(path) {
            // Keep the original file's permissions.
            let _ = fs::set_permissions(&tmp, meta.permissions());
        }
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// `YYYY-MM-DDTHH:MM:SSZ`.
fn iso8601_utc(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests;
