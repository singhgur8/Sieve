//! XMP sidecar sync (Phase 4). Owned by rust-engine-dev. The architect fixed only the
//! public surface used by `ipc::commands` and `lib.rs` (`XmpSyncConfig`, `SyncPolicy`,
//! `sidecar_path`, `XmpSync::{new, is_running, notify, write_images, read_images,
//! refresh_folder}`); everything behind it is free to change.
//!
//! Mapping (catalog -> sidecar, Lightroom Classic 13.2+ / Bridge; see `docs/architecture.md`
//! and `docs/decisions.md` 2026-10-03). On write the catalog state is authoritative:
//! - `pick = pick`              -> `xmpDM:pick = "1"`, `xmpDM:good = "True"`
//! - `pick = reject`            -> `xmpDM:pick = "-1"`, `xmpDM:good = "False"`
//! - `pick = unflagged`         -> `xmpDM:good` removed; `xmpDM:pick = "0"` only if the sidecar
//!   already has `xmpDM:pick` (absent = unflagged, so nothing is added)
//!   (`xmpDM` = `http://ns.adobe.com/xmp/1.0/DynamicMedia/`)
//! - `rating`                   -> `xmp:Rating = <0..=5>`, always, rejected or not (the catalog
//!   keeps stars independent of the flag). This also replaces a legacy Sieve `xmp:Rating -1`.
//! - `colorLabel = Some(c)`     -> `xmp:Label = "Red" | "Yellow" | "Green" | "Blue" | "Purple"`
//! - `colorLabel = None`        -> remove `xmp:Label` only if it holds one of those five names or
//!   the legacy Sieve `"Pick"` (custom label sets are preserved). A legacy `"Pick"` is also
//!   replaced by the colour when there is one.
//! - non-suppressed tags        -> `lr:hierarchicalSubject` item `Sieve|<tag>` and `dc:subject`
//!   item `<tag>` (snake_case wire value). On write, first remove every `Sieve|*` item, and the
//!   `dc:subject` leaf of each removed item; other keywords are kept.
//! - update `xmp:MetadataDate` (so Lightroom notices the change); keep every other field,
//!   namespace and packet as-is. Create a minimal `x:xmpmeta` packet if no sidecar exists.
//!
//! Sidecar -> catalog (reads), first match wins for the flag: `xmpDM:pick` (1 pick, -1 reject,
//! 0 unflagged), then `xmpDM:good` (True pick, False reject), then the legacy Sieve / Bridge
//! encodings (`xmp:Rating -1` -> reject, `xmp:Label "Pick"` -> pick), else unflagged.
//! `xmp:Rating 0..=5` -> rating (a missing rating reads as 0; `-1` keeps the catalog's stars);
//! label names above -> `colorLabel` ("Pick"/absent/unknown -> `None`). Attribute form
//! (Lightroom) and element form are both read. `Sieve|*` keywords are NOT read back (tags are
//! catalog -> sidecar only; analysis regenerates them).
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
//!   runs a pass on its own connection, and exits when no new deadline was set. Runs are
//!   serialized by `io_lock` (held for a whole pass / explicit run); a pass yields to a
//!   waiting explicit run between images and re-arms itself for the rest.
//! - Catalog concurrency (Phase 8c, "database is locked" on Save): connections wait
//!   `db::BUSY_TIMEOUT`; no transaction is held across file I/O (sidecar read/write first,
//!   then one short `db::write_tx` / autocommit update per image). A catalog error ends an
//!   explicit run with one error (`XmpSyncReport` is not returned) and ends an auto pass
//!   (logged; images stay dirty); only file problems are reported per image.
//! - Picking up changes made by other apps: `refresh_folder` (cheap: one `stat` per clean
//!   image, reads only sidecars whose mtime moved). When newer-wins picks the sidecar and
//!   the catalog's visible tags differ from its `Sieve|*` keywords, the merged state is
//!   written back so tags reach the sidecar too.

pub mod crs;
pub mod looks;
pub mod masks;
pub mod packet;
mod store;

pub use packet::{Desired, PacketError, ProfileWrite, SidecarValues};

use std::fs;
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
    /// Serializes sync runs: an auto-sync pass and an explicit command (save, read, folder
    /// refresh, masks catch-up) never interleave. Held for a whole run.
    io_lock: Arc<Mutex<()>>,
    /// Explicit runs waiting for `io_lock`; a running auto pass yields to them between images
    /// (and re-arms itself), so Cmd+S never waits behind a whole-shoot auto pass.
    explicit_waiting: Arc<AtomicUsize>,
    /// Where Lightroom AI mattes read from sidecars go (v10); `None` = masks import without
    /// their mattes (AI components then report `needs_update`).
    mask_cache: Option<develop::masks::MaskCache>,
}

impl XmpSync {
    pub fn new(config: XmpSyncConfig) -> Self {
        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            debounce: DEBOUNCE,
            state: Arc::new((Mutex::new(DebounceState::default()), Condvar::new())),
            io_lock: Arc::new(Mutex::new(())),
            explicit_waiting: Arc::new(AtomicUsize::new(0)),
            mask_cache: None,
        }
    }

    /// Stores Lightroom mattes found at XMP read time in `cache` (v10).
    pub fn with_mask_cache(mut self, cache: develop::masks::MaskCache) -> Self {
        self.mask_cache = Some(cache);
        self
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

    /// One auto-sync pass over every dirty image ([`SyncPolicy::NewerWins`]). Yields to an
    /// explicit run between images (the rest is picked up by a re-armed pass). A catalog
    /// error ends the pass (logged by the worker; the images stay dirty) instead of being
    /// reported once per image.
    fn auto_pass(&self, sink: &dyn XmpSink) -> AppResult<()> {
        let mut conn = db::open(&self.config.catalog_path)?;
        if !repo::xmp_auto_sync(&conn)? {
            return Ok(());
        }
        let ids = store::dirty_ids(&conn)?;
        if ids.is_empty() {
            return Ok(());
        }
        let io = lock_ignore_poison(&self.io_lock);
        let mut event = XmpSynced { written: Vec::new(), read: Vec::new() };
        let mut result = Ok(());
        for id in ids {
            if self.explicit_waiting.load(Ordering::SeqCst) > 0 {
                self.rearm();
                break;
            }
            let row = match store::load(&conn, id) {
                Ok(Some(row)) => row,
                Ok(None) => continue,
                Err(e) => {
                    result = Err(e);
                    break;
                }
            };
            match self.sync_one(&mut conn, &row, SyncPolicy::NewerWins) {
                Ok(Outcome::Written) => event.written.push(id),
                Ok(Outcome::Read { .. }) => event.read.push(id),
                Ok(Outcome::Skipped) => {}
                Err(SyncError::File(reason)) => {
                    if let Err(e) = store::mark_failed(&conn, id, &reason) {
                        result = Err(e);
                        break;
                    }
                    sink.failed(XmpWriteFailed { image_id: id, reason });
                }
                Err(SyncError::Catalog(e)) => {
                    result = Err(e);
                    break;
                }
            }
        }
        drop(io);
        sink.synced(event);
        result
    }

    /// Schedules another auto pass after the debounce (the worker is alive: it is the caller).
    fn rearm(&self) {
        let (lock, cvar) = &*self.state;
        let mut st = lock_ignore_poison(lock);
        if st.deadline.is_none() {
            st.deadline = Some(Instant::now() + self.debounce);
        }
        cvar.notify_all();
    }

    /// Takes `io_lock` for an explicit run, making a running auto pass yield.
    fn explicit_io(&self) -> MutexGuard<'_, ()> {
        self.explicit_waiting.fetch_add(1, Ordering::SeqCst);
        let guard = lock_ignore_poison(&self.io_lock);
        self.explicit_waiting.fetch_sub(1, Ordering::SeqCst);
        guard
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

    /// Per-file problems go to `XmpSyncReport.failed`; a catalog error (locked, read-only,
    /// damaged) fails the whole call once, saying how many sidecars were done before it.
    fn run_explicit(&self, ids: &[ImageId], policy: SyncPolicy) -> AppResult<XmpSyncReport> {
        let mut conn = db::open(&self.config.catalog_path)?;
        store::ensure_exist(&conn, ids)?;
        let _io = self.explicit_io();
        let mut report = XmpSyncReport::default();
        let done = |r: &XmpSyncReport| r.succeeded + r.skipped + r.failed.len() as u32;
        for &id in ids {
            let row = store::load(&conn, id)
                .map_err(|e| catalog_failure(&self.config.catalog_path, e, done(&report), ids.len()))?
                .ok_or_else(|| AppError::not_found(format!("image {id}")))?;
            match self.sync_one(&mut conn, &row, policy) {
                Ok(Outcome::Written) => report.succeeded += 1,
                Ok(Outcome::Read { changed }) => {
                    report.succeeded += 1;
                    if changed {
                        report.changed.push(id);
                    }
                }
                Ok(Outcome::Skipped) => report.skipped += 1,
                Err(SyncError::File(reason)) => {
                    store::mark_failed(&conn, id, &reason)
                        .map_err(|e| catalog_failure(&self.config.catalog_path, e, done(&report), ids.len()))?;
                    report.failed.push(XmpFailure { image_id: id, reason });
                }
                Err(SyncError::Catalog(e)) => {
                    return Err(catalog_failure(&self.config.catalog_path, e, done(&report), ids.len()));
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
        Ok(self.refresh(&[folder_id])?.0)
    }

    /// Focus / project-open refresh: [`Self::refresh_folder`] over `folders` (e.g.
    /// `db::projects::project_folder_ids`), returning the images whose rating, flag, label or
    /// develop settings changed (for the UI to reload). Cheap when nothing changed: one
    /// `stat` per clean image. Images with unsaved catalog changes are left to auto-sync's
    /// newer-wins. Blocking.
    pub fn refresh_folders(&self, folders: &[FolderId]) -> AppResult<Vec<ImageId>> {
        Ok(self.refresh(folders)?.1)
    }

    /// `(sidecars read, images changed)`.
    fn refresh(&self, folders: &[FolderId]) -> AppResult<(u32, Vec<ImageId>)> {
        let mut conn = db::open(&self.config.catalog_path)?;
        let mut candidates = Vec::new();
        for &folder_id in folders {
            candidates.extend(store::clean_images_in_folder(&conn, folder_id)?);
        }
        let _io = self.explicit_io();
        let (mut read, mut changed) = (0, Vec::new());
        for (id, raw, recorded) in candidates {
            match file_mtime_ms(&resolve_sidecar(&raw)) {
                Some(mtime) if Some(mtime) == recorded => continue,
                Some(_) => {}
                // Non-RAW without a sidecar: embedded XMP (read-only) until a sidecar exists.
                None if non_raw_format(&raw).is_some() && recorded.is_none() => {}
                None => continue,
            }
            let Some(row) = store::load(&conn, id)? else { continue };
            // Dirtied since the candidate list was taken: the catalog has newer changes.
            if store::is_dirty(&conn, id)? {
                continue;
            }
            match self.sync_one(&mut conn, &row, SyncPolicy::SidecarWins) {
                Ok(Outcome::Read { changed: c }) => {
                    read += 1;
                    if c {
                        changed.push(id);
                    }
                }
                Ok(_) => {}
                Err(SyncError::File(reason)) => store::mark_failed(&conn, id, &reason)?,
                Err(SyncError::Catalog(e)) => return Err(e),
            }
        }
        Ok((read, changed))
    }

    /// Launch catch-up (migration 0010): images whose sidecar masks were read before v10
    /// (`masks_pending_import`) get those masks imported (only `masks`; every other setting
    /// stays as it is in the catalog), their Lightroom mattes cached, and the flag cleared.
    /// Images without a sidecar (or without masks in it) just clear the flag. Per-image
    /// errors are logged and leave the flag set (retried next launch / on `read_xmp`).
    /// Returns the number of images whose masks were imported. Blocking.
    pub fn import_pending_masks(&self) -> AppResult<u32> {
        let mut conn = db::open(&self.config.catalog_path)?;
        let _io = self.explicit_io();
        let mut imported = 0;
        for id in store::masks_pending_ids(&conn)? {
            let Some(row) = store::load(&conn, id)? else { continue };
            match self.import_masks_only(&mut conn, &row) {
                Ok(true) => imported += 1,
                Ok(false) => {}
                Err(SyncError::File(reason)) => eprintln!("masks catch-up, image {id}: {reason}"),
                Err(SyncError::Catalog(e)) => return Err(e),
            }
        }
        Ok(imported)
    }

    /// File reads first, then one short write transaction (caller holds `io_lock`).
    fn import_masks_only(&self, conn: &mut Connection, row: &ImageRow) -> Result<bool, SyncError> {
        let path = resolve_sidecar(&row.path);
        let read = match fs::read_to_string(&path) {
            Ok(text) => masks::read(&text).map_err(|e| SyncError::File(format!("{}: {e}", path.display())))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(SyncError::File(format!("{}: {e}", path.display()))),
        };
        let Some(read) = read else {
            store::clear_masks_pending(conn, row.id)?;
            return Ok(false);
        };
        // Matte files + their catalog rows: outside the transaction (file I/O).
        self.import_mattes(conn, row.id, &path, &read.mattes);
        let changed = db::write_tx(conn, |conn| {
            let current = repo::get_adjustments(conn, row.id)?;
            let changed = current.masks != read.groups;
            if changed {
                let was_dirty = store::is_dirty(conn, row.id)?;
                let adj = ParametricAdjustments { masks: read.groups.clone(), ..current };
                develop::history::commit(conn, row.id, &adj, develop::history::LABEL_READ_XMP)?;
                if !was_dirty {
                    // The masks came from the sidecar: nothing new to write back.
                    store::clear_dirty(conn, row.id)?;
                }
            }
            let mut warnings = store::develop_warnings(conn, row.id)?;
            warnings.retain(|w| w.code != crate::ipc::types::DevelopWarningCode::MasksUnsupported);
            warnings.extend(read.warnings.iter().cloned());
            store::set_develop_warnings(conn, row.id, &warnings)?;
            store::clear_masks_pending(conn, row.id)?;
            Ok(changed)
        })?;
        Ok(changed)
    }

    /// Caches Lightroom mattes read from a sidecar (errors are logged, never fatal).
    fn import_mattes(&self, conn: &Connection, id: ImageId, path: &Path, mattes: &[masks::LightroomMatte]) {
        let (Some(cache), false) = (&self.mask_cache, mattes.is_empty()) else { return };
        match masks::mattes_supported() {
            Ok(()) => {
                for e in cache.import_lightroom(conn, id, mattes) {
                    eprintln!("{}: {e}", path.display());
                }
            }
            Err(e) => eprintln!("{}: Lightroom masks not decoded: {e}", path.display()),
        }
    }

    /// Syncs one image (caller holds `io_lock`). No catalog transaction is held across file
    /// I/O: the sidecar is read / written first, then the catalog is updated in one short
    /// `BEGIN IMMEDIATE` transaction (busy-timeout aware; a deferred transaction that read
    /// before the file I/O would fail with "database is locked" as soon as any other
    /// connection committed in between). `mark_written` only clears `xmp_dirty` if nothing
    /// changed since `row` was loaded, so concurrent ratings are never lost.
    fn sync_one(&self, conn: &mut Connection, row: &ImageRow, policy: SyncPolicy) -> Result<Outcome, SyncError> {
        let path = resolve_sidecar(&row.path);
        let format = crate::raw::format_from_extension(&row.path);
        let action = match policy {
            SyncPolicy::CatalogWins => Action::Write,
            SyncPolicy::SidecarWins => {
                if path.exists() {
                    Action::Read(None)
                } else {
                    match embedded_packet(&row.path) {
                        Some(text) => Action::Read(Some(text)),
                        None => return Ok(Outcome::Skipped),
                    }
                }
            }
            SyncPolicy::NewerWins => decide(file_mtime_ms(&path), row.xmp_mtime_ms, row.meta_updated_at),
        };
        match action {
            Action::Write => {
                self.write_and_mark(conn, &path, row)?;
                Ok(Outcome::Written)
            }
            Action::Read(preloaded) => {
                let file_err = |e: &dyn std::fmt::Display| SyncError::File(format!("{}: {e}", path.display()));
                let text = match preloaded {
                    Some(t) => t,
                    None => fs::read_to_string(&path).map_err(|e| file_err(&e))?,
                };
                let mut values = match format {
                    Some(f) => packet::parse_for(&text, f),
                    None => packet::parse(&text),
                }
                .map_err(|e| file_err(&e))?;
                let mtime = file_mtime_ms(&path);
                // Masks (v10): the sidecar's groups replace the catalog's (sidecar wins, like
                // every other develop setting); Lightroom mattes go to the matte cache.
                let masks = masks::read(&text).map_err(|e| file_err(&e))?;
                values.warnings.retain(|w| w.code != crate::ipc::types::DevelopWarningCode::MasksUnsupported);
                let groups = match &masks {
                    Some(m) => {
                        values.warnings.extend(m.warnings.iter().cloned());
                        m.groups.clone()
                    }
                    None => Vec::new(),
                };
                if let Some(m) = &masks {
                    self.import_mattes(conn, row.id, &path, &m.mattes);
                }
                if let Some(e) = &values.develop_error {
                    // Ratings still sync; develop settings stay as they are in the catalog.
                    eprintln!("{}: develop settings not imported: {e}", path.display());
                }
                let (rating, pick, label) = catalog_values(&values, row.rating);
                let (changed, write_back) = db::write_tx(conn, |conn| {
                    if values.develop_error.is_none() {
                        match values.develop.as_mut() {
                            Some(adj) => adj.masks = groups,
                            None if !groups.is_empty() => {
                                let mut current = repo::get_adjustments(conn, row.id)?;
                                current.masks = groups;
                                values.develop = Some(current);
                            }
                            None => {}
                        }
                    }
                    // Develop settings first: the history commit re-fires the dirty trigger,
                    // which `apply_read` then clears.
                    let mut develop_changed = false;
                    if let Some(adj) = &values.develop {
                        let current = repo::get_adjustments(conn, row.id)?;
                        if *adj != current {
                            develop::history::commit(conn, row.id, adj, develop::history::LABEL_READ_XMP)?;
                            develop_changed = true;
                        }
                    }
                    store::set_develop_warnings(conn, row.id, &values.warnings)?;
                    if values.develop_error.is_none() {
                        store::clear_masks_pending(conn, row.id)?;
                    }
                    let changed = store::apply_read(conn, row.id, rating, pick, label, mtime)?;
                    // NewerWins: rating/pick/label came from the sidecar; if our tags differ
                    // from the sidecar's, write them too (a no-op for the values just read).
                    let write_back =
                        policy == SyncPolicy::NewerWins && !same_tags(&values, &store::visible_tags(conn, row.id)?);
                    Ok((changed || develop_changed, write_back))
                })?;
                if write_back {
                    let fresh = store::load(conn, row.id)?.ok_or_else(|| SyncError::File("image vanished".into()))?;
                    self.write_and_mark(conn, &path, &fresh)?;
                }
                Ok(Outcome::Read { changed })
            }
        }
    }

    /// Writes the catalog state snapshotted in `row` to `path`, then records it (one
    /// autocommit `UPDATE`, which waits for the write lock with the busy timeout).
    fn write_and_mark(&self, conn: &mut Connection, path: &Path, row: &ImageRow) -> Result<(), SyncError> {
        let tags = store::visible_tags(conn, row.id)?;
        let develop = store::develop_settings(conn, row.id)?;
        let pending = store::masks_pending(conn, row.id)?;
        write_sidecar(path, row, &tags, develop.as_ref(), pending).map_err(SyncError::File)?;
        store::mark_written(conn, row, file_mtime_ms(path))?;
        Ok(())
    }
}

/// Why one image could not be synced.
#[derive(Debug)]
enum SyncError {
    /// Sidecar / original problem: recorded per image (`xmp_error`, `XmpSyncReport.failed`).
    File(String),
    /// The catalog itself failed (locked, read-only, damaged): ends the run with one error.
    Catalog(AppError),
}

impl From<AppError> for SyncError {
    fn from(e: AppError) -> Self {
        SyncError::Catalog(e)
    }
}

/// One clear error for a catalog failure during an explicit run.
fn catalog_failure(catalog: &Path, e: AppError, done: u32, total: usize) -> AppError {
    let e = db::explain_error(catalog, e);
    AppError::new(
        e.kind,
        format!(
            "XMP sync stopped after {done} of {total} photos: the catalog could not be updated ({}). \
             The remaining photos stay pending and are synced on the next try.",
            e.message
        ),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Written,
    Read { changed: bool },
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    Write,
    /// Read the sidecar (`None`) or this already-loaded packet (embedded XMP).
    Read(Option<String>),
}

/// Format of a non-RAW image path.
fn non_raw_format(path: &Path) -> Option<crate::ipc::types::ImageFormat> {
    crate::raw::format_from_extension(path).filter(|f| !f.is_raw())
}

/// Embedded XMP of a non-RAW original (read-only fallback when it has no sidecar).
fn embedded_packet(image: &Path) -> Option<String> {
    let format = non_raw_format(image)?;
    crate::raw::raster::embedded_xmp(image, format).ok().flatten()
}

/// [`SyncPolicy::NewerWins`] for a dirty image.
fn decide(sidecar_mtime: Option<i64>, recorded_mtime: Option<i64>, meta_updated_at: Option<i64>) -> Action {
    match sidecar_mtime {
        None => Action::Write,
        Some(m) if Some(m) == recorded_mtime => Action::Write,
        Some(m) if m > meta_updated_at.unwrap_or(i64::MIN) => Action::Read(None),
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
    let rating = i32::from(row.rating.min(5));
    let label = row.color_label.map(label_name);
    let (develop, seqs, profile) = match develop {
        Some(adj) => {
            let mut edits = crs::encode(adj);
            let (seqs, curve_name) = crs::encode_curves(adj);
            edits.push(curve_name);
            let look_source = adj.profile.look.as_ref().and_then(|l| looks::installed(&l.uuid));
            (edits, seqs, Some(ProfileWrite { settings: adj.profile.clone(), look_source }))
        }
        None => (Vec::new(), Vec::new(), None),
    };
    Desired {
        rating,
        pick: row.pick,
        label,
        tags: tags.to_vec(),
        metadata_date: iso8601_utc(SystemTime::now()),
        develop,
        seqs,
        profile,
        format: crate::raw::format_from_extension(&row.path),
    }
}

/// Sidecar -> catalog `(rating, pick, colorLabel)` (module-doc mapping). `current_rating`
/// is kept when the sidecar's rating is the legacy reject `-1`.
fn catalog_values(v: &SidecarValues, current_rating: u8) -> (u8, PickFlag, Option<ColorLabel>) {
    let label = v.label.as_deref().map(str::trim);
    let color = label.and_then(parse_label);
    let legacy_pick = label.is_some_and(|l| l.eq_ignore_ascii_case("Pick"));
    let raw_rating = v.rating.unwrap_or(0);
    let pick = match (v.dm_pick, v.dm_good) {
        (Some(p), _) if p > 0 => PickFlag::Pick,
        (Some(p), _) if p < 0 => PickFlag::Reject,
        (Some(_), _) => PickFlag::Unflagged,
        (None, Some(true)) => PickFlag::Pick,
        (None, Some(false)) => PickFlag::Reject,
        (None, None) if raw_rating < 0 => PickFlag::Reject,
        (None, None) if legacy_pick => PickFlag::Pick,
        (None, None) => PickFlag::Unflagged,
    };
    let rating = if raw_rating < 0 { current_rating } else { raw_rating.min(5) as u8 };
    (rating, pick, color)
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
/// atomically (`<name>.xmp.tmp`, fsync, rename). Masks are written after the merge
/// (`masks::apply`) unless `masks_pending` (sidecar masks not imported yet, migration 0010).
fn write_sidecar(
    path: &Path,
    row: &ImageRow,
    tags: &[String],
    develop: Option<&ParametricAdjustments>,
    masks_pending: bool,
) -> Result<(), String> {
    let shown = path.display();
    // Never leave an orphan sidecar next to a RAW that is gone (moved, drive unplugged).
    crate::raw::access::require_original(&row.path).map_err(|e| e.message)?;
    let existing = match fs::read(path) {
        Ok(bytes) => Some(String::from_utf8(bytes).map_err(|_| format!("{shown}: sidecar is not UTF-8"))?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(crate::raw::access::io_message(path, "read", &e)),
    };
    let mut merged =
        packet::merge(existing.as_deref(), &desired(row, tags, develop)).map_err(|e| format!("{shown}: {e}"))?;
    if let (Some(adj), false) = (develop, masks_pending) {
        merged = masks::apply(&merged, &adj.masks).map_err(|e| format!("{shown}: masks: {e}"))?;
    }
    write_atomic(path, merged.as_bytes()).map_err(|e| crate::raw::access::io_message(path, "write", &e))
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

#[cfg(test)]
mod parity_tests;
#[cfg(test)]
mod tests_minimal;
