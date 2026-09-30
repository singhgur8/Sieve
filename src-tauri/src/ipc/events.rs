//! Typed backend -> frontend events. Defined in Phase 1, emitted from Phase 2 on.

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;

use super::types::{ExportFailure, ExportJobId, ImageId};

/// Progress of the ingest pipeline (thumbnail + EXIF extraction).
/// Counts cover the current pipeline run: images queued since the pipeline was last
/// idle (new imports and regenerations join the running batch and grow `total`).
/// `done` includes failures. `done == total` means the pipeline is idle.
/// Throttled by the emitter (at most ~10 per second, plus a final one).
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct ImportProgress {
    pub done: u32,
    pub total: u32,
    /// Of `done`, how many failed.
    pub failed: u32,
}

/// An image's thumbnail (and preview) finished extracting and its EXIF is in the
/// catalog. Mirrors `ThumbnailState::Ready`; refetch the entry for EXIF fields.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct ThumbnailReady {
    pub image_id: ImageId,
    /// Absolute path of the 512 px thumbnail.
    pub path: String,
    /// Absolute path of the 2048 px preview, if produced.
    pub preview_path: Option<String>,
    pub width: u32,
    pub height: u32,
}

/// Extraction failed for an image. Mirrors `ThumbnailState::Failed`.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct ThumbnailFailed {
    pub image_id: ImageId,
    pub reason: String,
}

/// Progress of the per-image stage of the analysis worker. Counts cover the current
/// run (`total` grows while ingest keeps producing previews). `done` includes failures.
/// Throttled like `ImportProgress`. `done == total` ends the per-image stage; burst
/// grouping/rescoring follows and the run ends with `AnalysisFinished`.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisProgress {
    pub done: u32,
    pub total: u32,
    /// Of `done`, how many failed.
    pub failed: u32,
}

/// An image's measurements, `QualityScore`, faces and auto tags were written.
/// Burst membership / `duplicate_burst` may still change until `AnalysisFinished`.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisReady {
    pub image_id: ImageId,
}

/// Analysis failed for an image (unreadable preview, model error).
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisFailed {
    pub image_id: ImageId,
    pub reason: String,
}

/// The worker went idle: bursts, tags, scores and suggestions are final for this run.
/// Refetch the visible page and `getCatalogState()` (tag counts).
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisFinished {
    /// Images measured in this run (excluding failures).
    pub analyzed: u32,
    pub failed: u32,
    /// Stopped by `cancel_analysis`; remaining work stays pending.
    pub cancelled: bool,
    /// Burst groups in the catalog after regrouping.
    pub burst_groups: u32,
}

/// The auto-sync writer finished a pass. `written`: sidecars now match the catalog.
/// `read`: sidecars that were newer than the catalog's change (edited externally) and
/// were read into the catalog instead; refetch those entries (`get_images`).
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct XmpSynced {
    pub written: Vec<ImageId>,
    pub read: Vec<ImageId>,
}

/// Auto-sync could not write (or read) an image's sidecar. Also stored in
/// `XmpSyncState.error`; the image stays `dirty` and is retried on the next pass.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct XmpWriteFailed {
    pub image_id: ImageId,
    pub reason: String,
}

/// Progress of the running export job. Throttled like `ImportProgress`. `done` includes
/// failures and skips.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct ExportProgress {
    pub job_id: ExportJobId,
    pub done: u32,
    pub total: u32,
    /// Of `done`, how many failed.
    pub failed: u32,
    /// Of `done`, how many were skipped (`collision = skip`).
    pub skipped: u32,
    /// Source RAW file name most recently started; `null` when none is in flight.
    pub current_file: Option<String>,
}

/// An export job ended (completed or cancelled). Emitted exactly once per job, also for a
/// job cancelled while still queued. `get_export_jobs()` then shows its final state.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct ExportFinished {
    pub job_id: ExportJobId,
    pub succeeded: u32,
    pub skipped: u32,
    pub failed: Vec<ExportFailure>,
    /// Stopped by `cancel_export`; images not yet started were not exported.
    pub cancelled: bool,
    /// Resolved destination incl. subfolder; `null` for `source_folder`.
    pub output_dir: Option<String>,
    /// Wall time since the job started running (excludes time queued); 0 if it never ran.
    pub elapsed_ms: u32,
}
