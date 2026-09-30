//! Vision/ML culling engine: face detection, EAR blink, sharpness, exposure,
//! perceptual hash, burst grouping, scoring and auto tags.
//!
//! Owned by vision-ml-dev. The architect fixed only the public surface used by
//! `ipc::commands` and `lib.rs`: [`AnalysisConfig`], [`Analysis`] (`new`, `config`,
//! `is_running`, `start`, `cancel`), [`analysis_status`], [`MODEL_VERSION`] and
//! [`thresholds::default_thresholds`]. The compute seam below ([`Analyzer`], [`score`],
//! [`group_bursts`] and their data types) is the suggested split; its internals and any
//! extra fields/modules are free to change. See `docs/architecture.md`, "Analysis".
//!
//! Contract the implementation must honour (details in `docs/architecture.md`):
//! - Input is the 2048 px preview (`thumbnails.preview_path`), orientation applied.
//! - `start` is an idempotent kick that returns immediately; one background worker with
//!   its own SQLite connection (like `ingest`). While ingest is running the worker waits
//!   for more previews instead of exiting, so auto-analysis can be kicked right after
//!   `import_folder`.
//! - Per image: measure -> score -> write `image_analysis` + `quality_scores` + auto
//!   tags in one transaction -> emit `AnalysisReady` / `AnalysisFailed` + throttled
//!   `AnalysisProgress`. When the queue drains: regroup bursts, rescore, emit
//!   `AnalysisFinished`.
//! - Auto tags: upsert with `source = 'auto'` + confidence; delete auto tags no longer
//!   emitted *unless suppressed*; never touch suppressed rows (no resurrection) or
//!   `source = 'user'` rows. Never write `images.rating` / `images.pick`.

pub mod thresholds;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::ipc::error::AppResult;
use crate::ipc::types::{
    AnalysisScope, AnalysisStatus, CullTag, CullThresholds, ExposureStats, FaceInfo, ImageId, NormPoint, NormRect,
    QualityScore, ShootType,
};

/// Stored in `image_analysis.model_version` / `quality_scores.model_version`. Bump when
/// models or measurement code change: rows with another version count as pending.
pub const MODEL_VERSION: &str = "unimplemented-0";

// ---------------------------------------------------------------------------
// Managed state (fixed surface)
// ---------------------------------------------------------------------------

/// Resolved locations, fixed at startup by `lib.rs`.
#[derive(Debug, Clone)]
pub struct AnalysisConfig {
    /// Catalog file; the worker opens its own connection to it.
    pub catalog_path: PathBuf,
    /// Directory holding `det_10g.onnx` and `2d106det.onnx` (`$LUMENRAW_MODELS` overrides).
    pub models_dir: PathBuf,
}

/// Managed Tauri state for the analysis worker.
pub struct Analysis {
    config: AnalysisConfig,
    running: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

impl Analysis {
    pub fn new(config: AnalysisConfig) -> Self {
        Self { config, running: Arc::new(AtomicBool::new(false)), cancel: Arc::new(AtomicBool::new(false)) }
    }

    pub fn config(&self) -> &AnalysisConfig {
        &self.config
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Queues `scope` and ensures the background worker is running. Returns immediately.
    /// `Images`/`Folder`/`All` mark their rows for re-measurement (validating ids first:
    /// unknown ids fail with `not_found`); `Pending` only kicks; `Rescore` requests a
    /// rescore pass (no ML). Clears a previous cancel request.
    ///
    /// STUB (vision-ml-dev): currently a no-op so import/launch keep working.
    pub fn start(&self, app: &AppHandle, scope: AnalysisScope) -> AppResult<()> {
        let _ = (app, scope, &self.config, &self.running);
        Ok(())
    }

    /// Asks the worker to stop after the images in flight. Unprocessed work stays
    /// pending (picked up by the next `start`). Burst regrouping is skipped.
    pub fn cancel(&self) {
        if self.is_running() {
            self.cancel.store(true, Ordering::SeqCst);
        }
    }
}

/// Catalog-wide analysis counts (see `AnalysisStatus` for the buckets).
///
/// STUB (vision-ml-dev).
pub fn analysis_status(conn: &Connection, running: bool) -> AppResult<AnalysisStatus> {
    let _ = (conn, running);
    todo!("vision-ml-dev: count image_analysis / thumbnails buckets")
}

// ---------------------------------------------------------------------------
// Compute seam (suggested; internals free to change)
// ---------------------------------------------------------------------------

/// Threshold-independent measurements of one face. Coordinates as in [`NormRect`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FaceMetrics {
    pub bbox: NormRect,
    pub left_eye: NormPoint,
    pub right_eye: NormPoint,
    pub detection_score: f32,
    /// EAR of the less-open eye; `None` if landmarks were unusable.
    pub ear: Option<f32>,
    /// Normalized 0..=1 sharpness of the eye region (face crop fallback).
    pub sharpness: f32,
}

/// Threshold-independent measurements of one preview: the expensive part, stored as
/// `image_analysis.metrics_json` so rescoring needs no ML. Add fields freely (e.g.
/// directional blur stats); stored JSON is tied to `MODEL_VERSION`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageMetrics {
    /// Preview pixel size the measurements were taken on.
    pub width: u32,
    pub height: u32,
    pub faces: Vec<FaceMetrics>,
    /// Normalized 0..=1 whole-frame sharpness.
    pub global_sharpness: f32,
    pub exposure: ExposureStats,
    /// 64-bit perceptual hash (also stored as `image_analysis.phash`, bit-cast to i64).
    pub phash: u64,
}

/// Loaded ONNX sessions (SCRFD + 106-pt landmarks). One per worker thread.
pub struct Analyzer {
    _sessions: (),
}

impl Analyzer {
    /// Loads the models from `models_dir` (CoreML EP, CPU fallback).
    ///
    /// STUB (vision-ml-dev).
    pub fn load(models_dir: &Path) -> Result<Self, String> {
        let _ = models_dir;
        todo!("vision-ml-dev: build ort sessions")
    }

    /// Measures the preview JPEG at `preview_path`. Errors are human-readable reasons
    /// (stored in `image_analysis.error`, sent as `AnalysisFailed.reason`).
    ///
    /// STUB (vision-ml-dev).
    pub fn measure(&mut self, preview_path: &Path) -> Result<ImageMetrics, String> {
        let _ = preview_path;
        todo!("vision-ml-dev: decode, detect faces, EAR, sharpness, exposure, phash")
    }
}

/// An auto tag with its confidence 0..=1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoTag {
    pub tag: CullTag,
    pub confidence: f32,
}

/// Per-image scoring output (before burst grouping).
#[derive(Debug, Clone, PartialEq)]
pub struct Scored {
    /// `model_version` = [`MODEL_VERSION`]; includes suggested rating/pick.
    pub quality: QualityScore,
    /// Stored as `image_analysis.faces_json`, served by `get_faces`.
    pub faces: Vec<FaceInfo>,
    /// Never contains `DuplicateBurst` (that comes from [`group_bursts`]).
    pub tags: Vec<AutoTag>,
}

/// Pure: tags, scores and suggestions from measurements. Wedding/Portrait: eyes open +
/// eye sharpness dominate; `require_all_eyes_open` for group shots.
///
/// STUB (vision-ml-dev).
pub fn score(metrics: &ImageMetrics, thresholds: &CullThresholds, shoot_type: ShootType) -> Scored {
    let _ = (metrics, thresholds, shoot_type);
    todo!("vision-ml-dev: scoring")
}

/// Input to burst grouping, one per analyzed image with a capture time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BurstFrame {
    pub id: ImageId,
    /// EXIF wall-clock ms (as-if-UTC); differences are exact.
    pub captured_at_ms: i64,
    pub phash: u64,
    pub overall: f32,
}

/// A burst: at least 2 members in capture order, and its keeper.
#[derive(Debug, Clone, PartialEq)]
pub struct Burst {
    pub members: Vec<ImageId>,
    pub keeper: ImageId,
}

/// Pure: clusters `frames` (sorted by `captured_at_ms`) where consecutive gaps are
/// `<= window_ms` and hash distance `<= max_hash_distance`; picks the keeper. The caller
/// writes `burst_groups`, `images.burst_group_id`, `duplicate_burst` on non-keepers
/// and lowers their suggested pick/rating.
///
/// STUB (vision-ml-dev).
pub fn group_bursts(frames: &[BurstFrame], window_ms: u32, max_hash_distance: u32) -> Vec<Burst> {
    let _ = (frames, window_ms, max_hash_distance);
    todo!("vision-ml-dev: burst grouping")
}
