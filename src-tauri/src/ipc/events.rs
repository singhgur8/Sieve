//! Typed backend -> frontend events. Defined in Phase 1, emitted from Phase 2 on.

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;

use super::types::ImageId;

/// Progress of `import_folder` / metadata scan.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct ImportProgress {
    pub done: u32,
    pub total: u32,
}

/// An embedded preview finished extracting.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct ThumbnailReady {
    pub image_id: ImageId,
    pub path: String,
    pub width: u32,
    pub height: u32,
}

/// Progress of the culling/analysis worker.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisProgress {
    pub done: u32,
    pub total: u32,
}
