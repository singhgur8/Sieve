//! AI masks (Phase 7c, IPC v10): the seam between the masking contract and the segmentation
//! models. Owned by vision-ml-dev; the architect fixed the surface below. Model selection,
//! pre/post-processing and the concrete [`SegmentModel`] implementations (e.g. in
//! `ml/segment.rs`) are the owner's.
//!
//! Contract the implementation must honour:
//! - Output mattes are **sensor-frame** [`AlphaMask`]s (un-oriented, uncropped; see
//!   `ipc::masks`), 8-bit, soft edges (Lightroom mattes are soft 8-bit too). They may be
//!   cropped to the selection's bounding box (`AlphaMask.bounds`) and should have at least
//!   ~1500 px on the long edge of the full frame (Lightroom stores 2880 px for 24/33 MP
//!   files) so 1:1 zoom edges stay clean.
//! - Input: the owner's choice (2048 px preview, oriented -> un-orient the result with
//!   `ipc::types::unorient_point`; or the develop source). Record it in the docs.
//! - `compute` is blocking (called on the blocking pool by `compute_ai_mask` and by export for
//!   masks without a matte); concurrent requests for the same (image, kind) are coalesced;
//!   `is_computing` reports them. Results go to `develop::masks::MaskCache::put` with origin
//!   `sieve`, `model_version` = [`SegmentModel::id`], `input_digest` = [`source_fingerprint`].
//! - Families (`AiTargetKind`): subject, sky, background (= 1 - subject matte), people
//!   (instances by reference point; parts via a face/human parsing model), object (box
//!   prompt), landscape. Unavailable families are reported by [`Segmenter::capabilities`]
//!   with a reason; never fail app start because a model is missing.
//! - Models are permissively licensed (record licence + source in `docs/decisions.md`),
//!   loaded lazily from `<models_dir>` on first use, CoreML EP where possible.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::develop::masks::{AlphaMask, MaskCache};
use crate::develop::SourceImage;
use crate::ipc::error::AppResult;
use crate::ipc::types::{
    AiMaskInfo, AiMaskRequest, AiTarget, AiTargetKind, DetectedPerson, ImageId, MaskCapabilities, NormPoint,
};

/// Image handed to a model: interleaved sRGB8, any orientation the model owner chooses
/// (`orientation` says which; 1 = sensor frame).
pub struct SegmentInput<'a> {
    pub width: u32,
    pub height: u32,
    pub rgb: &'a [u8],
    /// EXIF orientation of `rgb` relative to the sensor frame (1..=8).
    pub orientation: u8,
}

/// What to segment.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentRequest {
    pub target: AiTarget,
    /// Sensor frame (instance selection for people / subject).
    pub reference_point: Option<NormPoint>,
}

/// One segmentation model (or pipeline of models) for one or more families.
pub trait SegmentModel: Send + Sync {
    /// Stable id incl. version, stored as `mask_cache.model_version` (e.g.
    /// `"birefnet-lite-1024@1"`). Changing it invalidates cached Sieve mattes of its kinds.
    fn id(&self) -> &str;
    /// Families this model serves.
    fn families(&self) -> &[AiTargetKind];
    /// Runs the model; the matte is in the *input's* frame (`bounds` normalized to the input),
    /// the caller converts it to the sensor frame.
    fn run(&self, request: &SegmentRequest, input: &SegmentInput) -> Result<AlphaMask, String>;
}

#[derive(Debug, Clone)]
pub struct SegmenterConfig {
    /// Same directory as the culling models (`SIEVE_MODELS`).
    pub models_dir: PathBuf,
    pub catalog_path: PathBuf,
}

/// Managed state: model registry + compute coalescing. Cheap to clone.
#[derive(Clone)]
pub struct Segmenter {
    config: Arc<SegmenterConfig>,
    mattes: MaskCache,
}

impl Segmenter {
    /// No I/O and no model loading (called during app setup).
    pub fn new(config: SegmenterConfig, mattes: MaskCache) -> Self {
        Self { config: Arc::new(config), mattes }
    }

    pub fn config(&self) -> &SegmenterConfig {
        &self.config
    }

    pub fn mattes(&self) -> &MaskCache {
        &self.mattes
    }

    /// Which families can be computed now (model files present), one entry per
    /// `AiTargetKind::ALL`, plus separable person parts / landscape categories.
    pub fn capabilities(&self) -> MaskCapabilities {
        todo!("vision-ml-dev: Segmenter::capabilities")
    }

    /// Current model id for a family (`None` = unavailable).
    pub fn model_version(&self, family: AiTargetKind) -> Option<String> {
        let _ = family;
        todo!("vision-ml-dev: Segmenter::model_version")
    }

    /// Computes (or returns the cached) matte for `request` on `src`; blocking. Cached =
    /// a `sieve` row of (image, kind, current model) whose `input_digest` still matches,
    /// unless `request.force`. Errors: `invalid` for an unavailable family (message = reason).
    pub fn compute(
        &self,
        src: &SourceImage,
        preview_path: Option<&Path>,
        request: &AiMaskRequest,
    ) -> AppResult<AiMaskInfo> {
        let _ = (src, preview_path, request);
        todo!("vision-ml-dev: Segmenter::compute")
    }

    /// A `compute` for (image, `AiMask::cache_kind`) is running.
    pub fn is_computing(&self, image_id: ImageId, kind: &str) -> bool {
        let _ = (image_id, kind);
        todo!("vision-ml-dev: Segmenter::is_computing")
    }

    /// People in the image, left to right in the displayed frame (reuses the culling face
    /// detections in `image_analysis.faces_json` when present, else runs detection).
    pub fn detect_people(&self, src: &SourceImage, preview_path: Option<&Path>) -> AppResult<Vec<DetectedPerson>> {
        let _ = (src, preview_path);
        todo!("vision-ml-dev: Segmenter::detect_people")
    }
}

/// `mask_cache.input_digest` for Sieve mattes: identifies the source pixels a matte was
/// computed from (upper-hex of file size + mtime; changes when the original is replaced).
pub fn source_fingerprint(path: &Path) -> std::io::Result<String> {
    let meta = std::fs::metadata(path)?;
    let mtime = meta.modified()?.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or_default();
    Ok(format!("{:016X}{:016X}", meta.len(), mtime as u64))
}
