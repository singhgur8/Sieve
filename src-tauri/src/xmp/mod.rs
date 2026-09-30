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
//! - non-suppressed tags        -> `lr:hierarchicalSubject` item `LumenRAW|<tag>` and `dc:subject`
//!   item `<tag>` (snake_case wire value). On write, first remove every `LumenRAW|*` item, and the
//!   `dc:subject` leaf of each removed item; other keywords are kept.
//! - update `xmp:MetadataDate` (so Lightroom notices the change); keep every other field,
//!   namespace and packet as-is. Create a minimal `x:xmpmeta` packet if no sidecar exists.
//!
//! Sidecar -> catalog (reads): `xmp:Rating -1` -> `pick = reject` (rating unchanged);
//! `0..=5` -> rating, and `pick = pick` iff `xmp:Label == "Pick"` else `unflagged`;
//! label names above -> `colorLabel` ("Pick"/absent/unknown -> `None`). `LumenRAW|*` keywords
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

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::AppHandle;

use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::{FolderId, ImageId, XmpSyncReport};

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

/// Lightroom convention: `DSC0001.ARW` -> `DSC0001.xmp` in the same directory.
/// (An existing sidecar whose extension differs only in case, e.g. `.XMP`, should be
/// reused by the implementation; on APFS it is the same file anyway.)
pub fn sidecar_path(raw_path: &Path) -> PathBuf {
    raw_path.with_extension("xmp")
}

/// Managed Tauri state for sidecar sync. Cheap to clone (shared state behind `Arc`).
#[derive(Clone)]
pub struct XmpSync {
    config: XmpSyncConfig,
    running: Arc<AtomicBool>,
}

impl XmpSync {
    pub fn new(config: XmpSyncConfig) -> Self {
        Self { config, running: Arc::new(AtomicBool::new(false)) }
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
    pub fn notify(&self, _app: &AppHandle) {
        // STUB (rust-engine-dev): debounced worker. No-op until implemented.
    }

    /// `write_xmp`: writes sidecars for `ids` now ([`SyncPolicy::CatalogWins`]), dirty or not.
    /// Unknown ids fail the whole call with `not_found` before any file is touched; per-file
    /// errors go to `XmpSyncReport.failed`. Blocking (call from `spawn_blocking`).
    pub fn write_images(&self, _ids: &[ImageId]) -> AppResult<XmpSyncReport> {
        Err(AppError::internal("write_xmp is not implemented yet"))
    }

    /// `read_xmp`: reads sidecars for `ids` into the catalog ([`SyncPolicy::SidecarWins`]);
    /// images without a sidecar count as `skipped`. Unknown ids -> `not_found`.
    /// Blocking (call from `spawn_blocking`).
    pub fn read_images(&self, _ids: &[ImageId]) -> AppResult<XmpSyncReport> {
        Err(AppError::internal("read_xmp is not implemented yet"))
    }

    /// Import hook: for every image in `folder_id` that is not `xmp_dirty` and whose sidecar
    /// mtime differs from `xmp_mtime_ms` (includes never-synced images with a sidecar), read
    /// the sidecar ([`SyncPolicy::SidecarWins`]). Returns the number of sidecars read
    /// (`ImportSummary.sidecarsRead`). Must not fail the import for per-file errors
    /// (record them in `xmp_error`). Blocking.
    pub fn refresh_folder(&self, _folder_id: FolderId) -> AppResult<u32> {
        // STUB (rust-engine-dev): no-op so import keeps working until implemented.
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_next_to_raw() {
        assert_eq!(sidecar_path(Path::new("/s/DSC0001.ARW")), PathBuf::from("/s/DSC0001.xmp"));
        assert_eq!(sidecar_path(Path::new("/s/a.b/IMG_1.CR3")), PathBuf::from("/s/a.b/IMG_1.xmp"));
    }
}
