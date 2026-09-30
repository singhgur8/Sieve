//! Typed backend -> frontend events. Defined in Phase 1, emitted from Phase 2 on.

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;

use super::types::ImageId;

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

/// Progress of the culling/analysis worker.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisProgress {
    pub done: u32,
    pub total: u32,
}
