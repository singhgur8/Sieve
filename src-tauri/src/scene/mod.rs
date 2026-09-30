//! Scenes + few-shot scene matching (Phase 7): group frames shot under the same light, then
//! grade a whole scene from 1-2 graded anchors by *relative* adjustments.
//!
//! Ownership: vision-ml-dev owns [`features`], [`detect`], [`stats`], [`matching`] (every
//! `todo!()`), and may add modules. The architect fixed the surface used by `ipc::commands`:
//! the constants and types in this file, [`progress_emitter`], the function signatures marked
//! "Contract" and [`store`] (catalog SQL for scenes, implemented + tested by the architect;
//! vision-ml-dev may extend it). See `docs/architecture.md`, "Scenes & matching".
//!
//! Data flow
//! - Detection (`detect_scenes`): [`store::detection_frames`] (catalog) ->
//!   [`features::compute_missing`] (2048 px previews, rayon, off the catalog lock) ->
//!   [`store::save_features`] + [`detect::group`] (pure) -> [`store::replace_scenes`].
//! - Stats (`get_render_stats`): [`stats::render_stats`] renders through
//!   `DevelopCache::render_image` (same source + pipeline as the editor, LUT included) and
//!   measures the 8-bit output.
//! - Matching (`match_scene`): [`matching::match_images`] renders anchors and targets via
//!   [`stats::render_stats`] and solves each target with [`matching::solve`]. `solve` only sees
//!   a reference `ImageStats`, a base `ParametricAdjustments` and a "measure these adjustments"
//!   callback, so Phase 9 (match to an arbitrary reference photo) reuses it unchanged.
//! - Apply (`apply_scene_match`): `develop::history::commit_batch` (one entry per image).

pub mod color;
pub mod detect;
pub mod features;
pub mod matching;
pub mod stats;
pub mod store;

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_specta::Event;

use crate::develop::SourceImage;
use crate::ipc::events::SceneProgress;
use crate::ipc::types::{BurstGroupId, FolderId, ImageId, ParametricAdjustments, SceneTask};

/// Long edge (px) of the renders measured by [`stats::render_stats`] (and so by matching and
/// `get_render_stats`). Small enough to be fast (the pipeline cost is per pixel), large enough
/// for stable percentiles.
pub const STATS_MAX_EDGE: u32 = 640;

/// Acceptance tolerances (roadmap Phase 7): a matched target at strength 1 must render within
/// these of its reference. Exposure: |`logMeanLuma` difference| in EV.
pub const TOLERANCE_EV: f32 = 0.15;
/// White point: Euclidean distance of `neutral.{a,b}` (Oklab; ~1 ΔE_OK x 0.01).
pub const TOLERANCE_AB: f32 = 0.012;

/// History label of `apply_scene_match` when the caller passes none.
pub const LABEL_MATCH: &str = "Match Scene";

/// Stored in `scene_features.version`. Bump when [`SceneFeatures`] or its computation changes:
/// rows with another version are recomputed by the next detection.
pub const FEATURES_VERSION: &str = "scene-features-v1";

/// Appearance features of one preview for scene detection (stored as JSON in
/// `scene_features`; not on the wire). Suggested content; vision-ml-dev owns the fields and the
/// similarity measure (bump [`FEATURES_VERSION`] when changing them).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneFeatures {
    /// log2 geometric-mean luminance of the preview (linearized sRGB).
    pub log_mean_luma: f32,
    /// Normalized histogram of encoded luma (e.g. 16 bins, sums to 1).
    pub luma_hist: Vec<f32>,
    /// Normalized 2D histogram of Oklab a/b (e.g. 8 x 8 over -0.2..0.2, sums to 1).
    pub ab_hist: Vec<f32>,
    /// Mean Oklab L, a, b.
    pub mean_oklab: [f32; 3],
}

/// One image considered by scene detection, in detection order (folder, then capture time,
/// images without one last by file name).
#[derive(Debug, Clone, PartialEq)]
pub struct DetectFrame {
    pub id: ImageId,
    pub folder_id: FolderId,
    pub captured_at_ms: Option<i64>,
    pub file_name: String,
    /// Burst members must end up in the same scene.
    pub burst_group_id: Option<BurstGroupId>,
    /// 2048 px preview (`None` while the thumbnail is not ready).
    pub preview_path: Option<PathBuf>,
    /// Current stored features (`None` = missing or stale; computed by
    /// [`features::compute_missing`]).
    pub features: Option<SceneFeatures>,
}

/// An image as matching needs it (resolved from the catalog by the command).
#[derive(Debug, Clone, PartialEq)]
pub struct MatchImage {
    pub src: SourceImage,
    pub captured_at_ms: Option<i64>,
    /// Stored adjustments (neutral defaults if never edited).
    pub adjustments: ParametricAdjustments,
}

/// Progress callback `(done, total)`; must be cheap and thread-safe (called from rayon workers).
pub type Progress<'a> = &'a (dyn Fn(u32, u32) + Sync);

/// Emits throttled [`SceneProgress`] events for `task` (at most every 100 ms, always the final
/// `done == total`).
pub fn progress_emitter(app: AppHandle, task: SceneTask) -> impl Fn(u32, u32) + Send + Sync {
    let last: Mutex<Option<Instant>> = Mutex::new(None);
    move |done, total| {
        let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
        let due = last.is_none_or(|t| t.elapsed() >= Duration::from_millis(100));
        if done >= total || due {
            *last = Some(Instant::now());
            let _ = SceneProgress { task, done, total }.emit(&app);
        }
    }
}
