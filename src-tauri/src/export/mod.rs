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

pub mod develop;
pub mod encode;
pub mod metadata;
pub mod naming;
pub mod presets;

use std::path::PathBuf;
use std::sync::Arc;

use rusqlite::Connection;
use tauri::AppHandle;

use crate::ipc::error::AppResult;
use crate::ipc::types::{ExportCapabilities, ExportJob, ExportJobId, ExportPlan, ExportSettings, ImageId};
use crate::lut::LutLibrary;

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

/// Resolved at startup by `lib.rs`.
#[derive(Debug, Clone)]
pub struct ExportConfig {
    /// Catalog file; the export worker opens its own connection to it.
    pub catalog_path: PathBuf,
    /// `SIEVE_EXPORT_MEMORY_MB`; `None` = derive from physical RAM ([`memory_budget_bytes`]).
    pub memory_budget_mb: Option<u64>,
}

/// Managed Tauri state: export job queue + worker. Cheap to clone.
#[derive(Clone)]
pub struct Exporter {
    config: ExportConfig,
    luts: LutLibrary,
    // rust-engine-dev: queue, cancel flags, worker handle, live job counters.
    _inner: Arc<()>,
}

impl Exporter {
    pub fn new(config: ExportConfig, luts: LutLibrary) -> Self {
        Self { config, luts, _inner: Arc::new(()) }
    }

    pub fn config(&self) -> &ExportConfig {
        &self.config
    }

    pub fn luts(&self) -> &LutLibrary {
        &self.luts
    }

    /// Encoder availability (probed once, cached), `MAX_PARALLEL` and the memory budget.
    pub fn capabilities(&self) -> ExportCapabilities {
        todo!("rust-engine-dev: Exporter::capabilities")
    }

    /// Blocking; called once at startup. Marks jobs left `queued`/`running` by a previous
    /// session as `interrupted` (finished_at = now). Must not fail startup on a fresh catalog.
    pub fn recover_interrupted(&self) -> AppResult<()> {
        // rust-engine-dev: UPDATE export_jobs SET state = 'interrupted' ... No-op until implemented.
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
        let _ = (app, ids, settings, preset_name);
        todo!("rust-engine-dev: Exporter::enqueue")
    }

    /// Requests cancellation (see module docs). Returns immediately.
    pub fn cancel(&self, app: &AppHandle, job_id: ExportJobId) -> AppResult<()> {
        let _ = (app, job_id);
        todo!("rust-engine-dev: Exporter::cancel")
    }

    /// Blocking. Queued and running jobs first (with live counters), then the 50 most recent
    /// finished jobs, newest first. `failures` lists every failed item of each job.
    pub fn jobs(&self) -> AppResult<Vec<ExportJob>> {
        todo!("rust-engine-dev: Exporter::jobs")
    }
}

/// Dry run over the command connection: resolves the output dir and each image's final
/// path under the collision policy (in `ids` order, including intra-job duplicates) without
/// touching the disk except `exists` checks. Unknown ids -> `not_found`. `settings` is
/// validated and its destination is not `choose`.
pub fn plan(conn: &Connection, ids: &[ImageId], settings: &ExportSettings) -> AppResult<ExportPlan> {
    let _ = (conn, ids, settings);
    todo!("rust-engine-dev: export::plan")
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
}
