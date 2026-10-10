//! Typed backend -> frontend events. Defined in Phase 1, emitted from Phase 2 on.

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;

use super::types::{
    string_enum, BaselineRun, EditedPreview, ExportFailure, ExportJobId, ImageId, SceneTask, StyleModelStatus,
    StyleTrainPhase, TargetRun,
};

string_enum! {
    /// What a background activity is doing (v18, [`ActivityEvent`]).
    pub enum ActivityKind {
        /// Import: thumbnail + EXIF extraction of new photos.
        Import => "import",
        /// Culling analysis (faces, sharpness, scores, bursts).
        Analysis => "analysis",
        /// Writing XMP sidecars (explicit save or auto-sync).
        XmpSave => "xmp_save",
        /// Paste / sync settings to many photos.
        PasteSync => "paste_sync",
        /// Apply a scene edit to its members.
        ApplyScene => "apply_scene",
        Export => "export",
        ModelDownload => "model_download",
        /// Target-count selection: people, moments, choosing the delivery set (v20).
        TargetSelection => "target_selection",
        /// Baseline edit run: per-photo Auto + the anchor's look over the shoot (v21).
        BaselineEdit => "baseline_edit",
        Other => "other",
    }
}

string_enum! {
    /// Lifecycle of a background activity (v18).
    pub enum ActivityState {
        Running => "running",
        /// Ended normally (possibly with per-item failures: see `message`).
        Finished => "finished",
        /// Ended by an error; `message` says why.
        Error => "error",
        /// Stopped by the user.
        Cancelled => "cancelled",
    }
}

/// Generic background-activity report for the corner indicator (v18). One activity = one
/// `id`: a `running` event when it starts, throttled `running` progress events (at most 10 per
/// second per activity), then exactly one terminal event (`finished` / `error` / `cancelled`).
/// Several activities may run at once (e.g. import + analysis). Emitted in addition to the
/// specific progress events (`ImportProgress`, `ExportProgress`, ...), which stay the source
/// of truth for their screens. Emitted via `ipc::activity::Activities`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEvent {
    /// Unique per activity within an app session.
    pub id: u32,
    pub kind: ActivityKind,
    /// User-facing, e.g. "Saving metadata to XMP", "Exporting 120 photos".
    pub label: String,
    pub done: u32,
    /// `null` = indeterminate (spinner without a count).
    pub total: Option<u32>,
    pub state: ActivityState,
    /// Terminal events: a short user-facing summary or the error ("Saved 685 photos; 3
    /// failed"); `null` while running unless there is something to say.
    pub message: Option<String>,
}

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

/// The edited preview of an image was rendered (or dropped) in the background (IPC v19.1):
/// mirrors `RawImageEntry.editedPreview`. `preview = null`: the photo is unedited again (show
/// the embedded thumbnail). Emitted after edits, batches (paste / sync / presets / scene
/// apply / undo) and for neighbours prerendered by `prepareDevelop`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct EditedPreviewChanged {
    pub image_id: ImageId,
    pub preview: Option<EditedPreview>,
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

/// Progress of a running `detect_scenes` (feature extraction over previews) or `match_scene`
/// (renders per target). Throttled (~10/s, always ending with `done == total`). Only drives a
/// progress bar; the command's result arrives when it resolves.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct SceneProgress {
    pub task: SceneTask,
    pub done: u32,
    pub total: u32,
}

/// Progress of the `download_models` download in flight (IPC v12). Throttled (at most ~5 per
/// second, plus one when each file completes). Byte counts cover the whole group; files
/// already installed count as done when they are reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct ModelDownloadProgress {
    pub group: String,
    /// File currently downloading / verifying.
    pub name: String,
    /// 0-based index of `name` among `fileCount` files.
    pub file_index: u32,
    pub file_count: u32,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

/// A `download_models` run ended. Emitted exactly once per accepted `download_models` call.
/// On `ok`, every file of the group is installed and verified, and `get_mask_capabilities()`
/// already reflects it (no restart). On failure `error` is user-facing (network error,
/// checksum mismatch, "model download cancelled", ...); verified files stay installed and a
/// partial file is resumed by the next `download_models`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct ModelDownloadFinished {
    pub group: String,
    pub ok: bool,
    /// Set when `download_models` was stopped by `cancel_model_download`.
    pub cancelled: bool,
    pub error: Option<String>,
}

/// Progress of `train_style_model` (IPC v14). Throttled (~5/s, plus one per phase change).
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct StyleModelProgress {
    pub phase: StyleTrainPhase,
    pub done: u32,
    pub total: u32,
}

/// A `train_style_model` run ended (IPC v14). Emitted exactly once per accepted call.
/// `status` is `style_model_status()` after the run.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct StyleModelFinished {
    pub ok: bool,
    /// Stopped by `cancel_style_training` (the previous model, if any, stays in use).
    pub cancelled: bool,
    /// User-facing reason when not `ok` (e.g. "Edit at least 20 photos first").
    pub error: Option<String>,
    pub status: StyleModelStatus,
}

/// A `run_target_selection` run ended (IPC v20): exactly once per accepted call, after the
/// selection (and the suggestion overlay) is stored. `run.state` is `finished`, `failed` or
/// `cancelled`. Refetch people / moments / selections / the grid. Progress is reported as
/// `activityEvent` kind `target_selection`.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct TargetRunFinished {
    pub run: TargetRun,
}

/// A `run_baseline` run ended (IPC v21): exactly once per accepted call, after the batch and
/// the per-photo results are stored. `run.state` is `finished`, `failed` or `cancelled`
/// (nothing written unless `finished`). Refetch the grid / edit states / results. Progress is
/// reported as `activityEvent` kind `baseline_edit`.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct BaselineRunFinished {
    pub run: BaselineRun,
}
