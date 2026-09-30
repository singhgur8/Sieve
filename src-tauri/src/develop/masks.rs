//! Local adjustments (Phase 7c, IPC v10): mask evaluation at any resolution, per-pixel local
//! parameters for the shared pipeline (preview + export), mask overlays and the AI matte
//! cache. Owned by rust-engine-dev; the architect fixed the surface below (types, function
//! signatures, semantics). Bodies marked `todo!` are the owner's.
//!
//! # Frames
//! Every mask coordinate is in the **sensor frame** (`ipc::masks` docs): normalized
//! un-oriented, uncropped image. A render (preview, region, export) maps each output pixel
//! back through region -> crop (+ straighten angle) -> orientation to the sensor frame
//! ([`MaskGeometry::sensor_point`]); masks are *evaluated* there, so any output resolution
//! works and a matte is sampled bilinearly from its stored bitmap. Isotropic lengths (brush
//! radius) are fractions of the sensor width ([`MaskGeometry::sensor_width_px`]).
//!
//! # Component values (per pixel, 0..=1)
//! - brush: strokes in order; each dab adds `flow * falloff(d / radius, feather)` towards
//!   the stroke's `density` (`v = v + (density - v) * a`, never above density); erase strokes
//!   pull towards 0 (`v = v * (1 - density * a)`); `falloff` = 1 inside `1 - feather`, smooth
//!   (cosine) to 0 at the radius. `auto_mask` refines by colour similarity to the dab centres
//!   (edge-aware; guide = [`RangeGuide`]).
//! - linear: 0 at `zero`, 1 at `full`, linear along the zero->full direction, clamped.
//! - radial: ellipse from the bounds (rotated by `angle` about its centre, in pixel space of
//!   the sensor frame so it stays an ellipse on screen); 1 inside the inner ellipse
//!   (`1 - feather/100` of the radii), smooth to 0 at the bounds; `flipped` inverts.
//!   `midpoint` / `roundness` follow Lightroom's radial filter (roundness 0 = the bounds'
//!   ellipse).
//! - luminance / color: from the [`RangeGuide`] (global WB + exposure applied, locals not):
//!   luminance = trapezoid over L* (featherLow, low, high, featherHigh), then smoothed by
//!   `smoothness`; colour = max over samples of a Gaussian of the Lab distance to the sample
//!   colour (mean of the point or area), width from `amount`.
//! - ai: the matte ([`MatteSource`]); empty if none (and `ai_mask_needs_update`).
//! - unsupported: 0 everywhere (and `masks_unsupported`).
//!
//! Then `value = opacity * (inverted ? 1 - shape : shape)`, and in component order:
//! first component starts the mask (`acc = value`); then `add: max(acc, v)`,
//! `subtract: acc * (1 - v)`, `intersect: acc * v` ([`combine`]).
//! Group weight = `acc * amount` where `active`; inactive groups/components are skipped.
//!
//! # Pipeline integration (the exact seam the pipeline owner calls)
//! In `pipeline::render` (preview) and `export::develop::render_full` (export) — same code:
//! 1. `let geom = MaskGeometry { .. }` for the render's output grid (region/crop/orientation).
//! 2. After global WB + exposure (the "range guide" point) build a [`RangeGuide`] only if any
//!    active component is a luminance/colour range or auto-masked brush
//!    ([`needs_range_guide`]).
//! 3. `let weights = evaluate(&adj.masks, &geom, image_id, mattes, guide)`; cache it per
//!    (image, geometry, hash of the groups' *components*) so local slider drags only redo
//!    step 4 (the slider latency budget, < 100 ms at 2048 px).
//! 4. `let local = LocalPlanes::build(&adj.masks, &weights)` (None = no active group: skip
//!    everything below; zero cost for unmasked images).
//! 5. Stage by stage, add the per-pixel offset to the global slider value (Lightroom's
//!    additive model), in UI units: WB (`Temperature`, `Tint`, relative shift of the
//!    effective temperature/tint), exposure (`Exposure`, EV), PV2012 tone (`Contrast`,
//!    `Highlights`, `Shadows`, `Whites`, `Blacks`), texture/clarity/dehaze, HSL-ish
//!    (`Hue`, `Saturation`), NR (`Noise`, `Moire`, `Defringe`), sharpening (`Sharpness`).
//!    Stages that precompute global LUTs evaluate the per-pixel variant only where
//!    `local.get(p)` is `Some` and the value is non-zero.
//! 6. After the global point curves: [`apply_group_blends`] (per-group point curve with
//!    refine saturation, and colour tint), each blended by the group weight.
//!
//! Warnings from `weights.warnings` join `DevelopInfo.warnings` (via `DevelopCache::info`).
//!
//! # Mattes
//! [`MaskCache`] (managed state) owns `mask_cache` rows (migration 0010) and the PNG files
//! under `<cacheDir>/masks/<imageId>/<digest>.png`, with an in-memory LRU of decoded
//! [`AlphaMask`]s. Lightroom mattes are inserted by the XMP read path
//! (`xmp::masks::decode_bitmap` -> [`MaskCache::put`]); Sieve mattes by
//! `ml::masking::Segmenter::compute`. Resolution of an AI component: `digest` set -> that row;
//! else the newest `sieve` row of (image, `AiMask::cache_kind`, current model version).

use std::path::PathBuf;
use std::sync::Arc;

use rusqlite::Connection;

use crate::ipc::error::AppResult;
use crate::ipc::types::{
    AiMask, AiMaskInfo, AiMaskOrigin, AiMaskStatus, CropSettings, DevelopWarning, ImageId, LocalColor, MaskBlendMode,
    MaskComponent, MaskGroup, MaskOverlayOptions, MaskOverlayTarget, MaskShape, NormPoint, NormRect,
    ParametricAdjustments, PointCurves, RenderedMaskOverlay,
};
use crate::ml::masking::Segmenter;

use super::{DevelopCache, RenderTicket, SourceImage};

// ---------------------------------------------------------------------------
// Mattes
// ---------------------------------------------------------------------------

/// An 8-bit matte placed in the sensor frame (255 = fully selected).
#[derive(Debug, Clone, PartialEq)]
pub struct AlphaMask {
    pub width: u32,
    pub height: u32,
    /// Placement in the sensor frame (normalized); outside it the matte is 0.
    pub bounds: NormRect,
    /// Row-major, `width * height` bytes.
    pub data: Vec<u8>,
}

impl AlphaMask {
    /// Bilinear sample at a sensor-frame point, 0..=1 (0 outside `bounds`).
    pub fn sample(&self, p: NormPoint) -> f32 {
        let _ = p;
        todo!("rust-engine-dev: AlphaMask::sample (bilinear, 0 outside bounds)")
    }

    /// Mean value over the whole sensor frame, 0..=1 (`AiMaskInfo.coverage`).
    pub fn coverage(&self) -> f32 {
        todo!("rust-engine-dev: AlphaMask::coverage")
    }
}

/// Resolves AI components to mattes during a render.
pub trait MatteSource: Send + Sync {
    /// The matte for `ai` on `image_id` (resolution rules in the module docs), or `None`.
    fn matte(&self, image_id: ImageId, ai: &AiMask) -> Option<Arc<AlphaMask>>;
}

/// No mattes (tests; renders AI components as empty).
pub struct NoMattes;

impl MatteSource for NoMattes {
    fn matte(&self, _: ImageId, _: &AiMask) -> Option<Arc<AlphaMask>> {
        None
    }
}

/// A matte to insert into the cache.
#[derive(Debug, Clone, PartialEq)]
pub struct NewMatte {
    /// `AiMask::cache_kind()` of what was segmented.
    pub kind: String,
    pub target: crate::ipc::types::AiTarget,
    pub reference_point: Option<NormPoint>,
    pub origin: AiMaskOrigin,
    /// Lightroom's MaskDigest for `lightroom` mattes; `None` = MD5 of the encoded PNG.
    pub digest: Option<String>,
    pub model_version: String,
    pub input_digest: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MaskCacheConfig {
    pub catalog_path: PathBuf,
    /// Cache root (`CatalogState.cacheDir`); files go to `<cacheDir>/masks/`.
    pub cache_dir: PathBuf,
}

/// AI matte cache (managed state; cheap to clone). See the module docs.
#[derive(Clone)]
pub struct MaskCache {
    config: Arc<MaskCacheConfig>,
}

impl MaskCache {
    /// No I/O (called during app setup).
    pub fn new(config: MaskCacheConfig) -> Self {
        Self { config: Arc::new(config) }
    }

    pub fn config(&self) -> &MaskCacheConfig {
        &self.config
    }

    /// `<cacheDir>/masks`.
    pub fn masks_dir(&self) -> PathBuf {
        self.config.cache_dir.join("masks")
    }

    /// Stores `mask` (PNG file + `mask_cache` row; replaces the same (image, digest)) and
    /// returns its info. `conn` = the caller's catalog connection.
    pub fn put(
        &self,
        conn: &Connection,
        image_id: ImageId,
        matte: &NewMatte,
        mask: &AlphaMask,
    ) -> AppResult<AiMaskInfo> {
        let _ = (conn, image_id, matte, mask);
        todo!("rust-engine-dev: MaskCache::put")
    }

    /// The cached matte row `ai` resolves to on `image_id` (module docs), if any.
    /// `model_version` = the current Sieve model for that kind (`None` = any).
    pub fn resolve(
        &self,
        conn: &Connection,
        image_id: ImageId,
        ai: &AiMask,
        model_version: Option<&str>,
    ) -> AppResult<Option<AiMaskInfo>> {
        let _ = (conn, image_id, ai, model_version);
        todo!("rust-engine-dev: MaskCache::resolve")
    }

    /// Decoded matte by digest (LRU; loads the PNG on a miss).
    pub fn load(&self, image_id: ImageId, digest: &str) -> AppResult<Option<Arc<AlphaMask>>> {
        let _ = (image_id, digest);
        todo!("rust-engine-dev: MaskCache::load")
    }

    /// `list_masks` status: one entry per AI component of `groups`, in order. `ready` when
    /// [`Self::resolve`] finds a matte (with the segmenter's current model version for
    /// Sieve mattes), `computing` when `segmenter.is_computing(image_id, kind)`,
    /// `unavailable` when the segmenter cannot compute that family, else `needs_update`.
    pub fn status(
        &self,
        conn: &Connection,
        image_id: ImageId,
        groups: &[MaskGroup],
        segmenter: &Segmenter,
    ) -> AppResult<Vec<AiMaskStatus>> {
        let _ = (conn, image_id, groups, segmenter);
        todo!("rust-engine-dev: MaskCache::status")
    }

    /// Deletes files of rows that no longer exist (images removed) and unreferenced Sieve
    /// mattes superseded by a newer model version. Called at startup (background).
    pub fn sweep(&self, conn: &Connection) -> AppResult<usize> {
        let _ = conn;
        todo!("rust-engine-dev: MaskCache::sweep")
    }
}

impl MatteSource for MaskCache {
    fn matte(&self, image_id: ImageId, ai: &AiMask) -> Option<Arc<AlphaMask>> {
        let _ = (image_id, ai);
        todo!("rust-engine-dev: MatteSource for MaskCache (own read-only catalog connection)")
    }
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

/// Maps an output pixel grid to the sensor frame.
#[derive(Debug, Clone, PartialEq)]
pub struct MaskGeometry {
    /// Un-oriented full image size in px (the frame masks are normalized to).
    pub sensor_width: u32,
    pub sensor_height: u32,
    /// EXIF orientation 1..=8.
    pub orientation: u8,
    /// Crop in effect (sensor frame, as `ParametricAdjustments.crop`).
    pub crop: CropSettings,
    /// `RenderOptions.region` of the (cropped, oriented) frame, if any.
    pub region: Option<NormRect>,
    /// Output grid size.
    pub width: u32,
    pub height: u32,
}

impl MaskGeometry {
    /// Sensor-frame point of the centre of output pixel (`px`, `py`).
    pub fn sensor_point(&self, px: f32, py: f32) -> NormPoint {
        let _ = (px, py);
        todo!("rust-engine-dev: MaskGeometry::sensor_point (region -> crop/angle -> orientation)")
    }

    /// Output pixels per sensor-frame width unit (brush radius scale).
    pub fn sensor_width_px(&self) -> f32 {
        todo!("rust-engine-dev: MaskGeometry::sensor_width_px")
    }
}

/// Image colour for range masks and brush auto-mask, on the output grid.
pub struct RangeGuide<'a> {
    pub width: u32,
    pub height: u32,
    /// Per pixel CIE Lab (L* 0..=100, a*, b*) of the image after global white balance and
    /// exposure, before any local adjustment.
    pub lab: &'a [[f32; 3]],
}

/// Per-group mask weights on the output grid (`groups[i]` belongs to `masks[i]`; `None` =
/// inactive or empty group).
#[derive(Debug, Clone, PartialEq)]
pub struct GroupWeights {
    pub width: u32,
    pub height: u32,
    /// `acc * amount`, 0..=2 (amount may exceed 1).
    pub groups: Vec<Option<Vec<f32>>>,
    /// `masks_unsupported` / `ai_mask_needs_update` with counts.
    pub warnings: Vec<DevelopWarning>,
}

/// Whether any active component needs a [`RangeGuide`] (range masks, auto-masked brush).
pub fn needs_range_guide(masks: &[MaskGroup]) -> bool {
    masks.iter().filter(|g| g.active).flat_map(|g| &g.components).filter(|c| c.active).any(|c| match &c.shape {
        MaskShape::Luminance(_) | MaskShape::Color(_) => true,
        MaskShape::Brush(b) => b.strokes.iter().any(|s| s.auto_mask),
        _ => false,
    })
}

/// One component's value plane (`opacity` and `inverted` applied), or `None` when it
/// contributes nothing renderable (unsupported, missing matte).
pub fn evaluate_component(
    component: &MaskComponent,
    geom: &MaskGeometry,
    image_id: ImageId,
    mattes: &dyn MatteSource,
    guide: Option<&RangeGuide>,
) -> Option<Vec<f32>> {
    let _ = (component, geom, image_id, mattes, guide);
    todo!("rust-engine-dev: develop::masks::evaluate_component")
}

/// Combines `value` into `acc` with `mode` (module docs). Both planes have equal length.
pub fn combine(acc: &mut [f32], value: &[f32], mode: MaskBlendMode) {
    let _ = (acc, value, mode);
    todo!("rust-engine-dev: develop::masks::combine")
}

/// Evaluates every active group on the output grid (rayon over rows).
pub fn evaluate(
    masks: &[MaskGroup],
    geom: &MaskGeometry,
    image_id: ImageId,
    mattes: &dyn MatteSource,
    guide: Option<&RangeGuide>,
) -> GroupWeights {
    let _ = (masks, geom, image_id, mattes, guide);
    todo!("rust-engine-dev: develop::masks::evaluate")
}

// ---------------------------------------------------------------------------
// Local parameters
// ---------------------------------------------------------------------------

/// Additive local parameters (UI units of `LocalAdjustments`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocalParam {
    Temperature,
    Tint,
    Exposure,
    Contrast,
    Highlights,
    Shadows,
    Whites,
    Blacks,
    Texture,
    Clarity,
    Dehaze,
    Hue,
    Saturation,
    Sharpness,
    Noise,
    Moire,
    Defringe,
}

impl LocalParam {
    pub const COUNT: usize = 17;
}

/// Non-additive per-group effects applied by [`apply_group_blends`].
#[derive(Debug, Clone, PartialEq)]
pub struct GroupBlend {
    /// The group's weight plane (shared with [`GroupWeights`]).
    pub weight: Arc<Vec<f32>>,
    pub color: LocalColor,
    pub tone_curve: PointCurves,
    pub curve_refine_saturation: f32,
}

/// Per-pixel sums over groups of `weight_g * local_g.param` for each [`LocalParam`]
/// (a plane is `None` when every active group has 0 for it), plus the group blends.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalPlanes {
    pub width: u32,
    pub height: u32,
    planes: Vec<Option<Vec<f32>>>,
    pub blends: Vec<GroupBlend>,
}

impl LocalPlanes {
    /// `None` when no group is active with a non-neutral adjustment set.
    pub fn build(masks: &[MaskGroup], weights: &GroupWeights) -> Option<LocalPlanes> {
        let _ = (masks, weights);
        todo!("rust-engine-dev: LocalPlanes::build")
    }

    /// The offset plane for `param` (row-major, `width * height`), if any group sets it.
    pub fn get(&self, param: LocalParam) -> Option<&[f32]> {
        self.planes.get(param as usize).and_then(|p| p.as_deref())
    }
}

/// Per-group point curve (+ refine saturation) and colour tint, each blended by the group
/// weight, on the working image (linear Rec.2020 RGB, row-major). Called once after the
/// global point curves.
pub fn apply_group_blends(rgb: &mut [[f32; 3]], local: &LocalPlanes) {
    let _ = (rgb, local);
    todo!("rust-engine-dev: develop::masks::apply_group_blends")
}

// ---------------------------------------------------------------------------
// Overlay
// ---------------------------------------------------------------------------

/// `render_mask_overlay`: evaluates `target` for `adjustments` on the grid `render_preview`
/// would produce for (`options.maxEdge`, `options.region`, `adjustments.crop`), encodes the
/// plane as an 8-bit grayscale JPEG, stores it on the `mask` slot (latest-wins `ticket`) and
/// returns its URL. `Ok(None)` = superseded. An unknown group/component id is `invalid`.
pub fn render_overlay(
    develop: &DevelopCache,
    mattes: &MaskCache,
    ticket: RenderTicket,
    src: &SourceImage,
    adjustments: &ParametricAdjustments,
    target: &MaskOverlayTarget,
    options: &MaskOverlayOptions,
) -> AppResult<Option<RenderedMaskOverlay>> {
    let _ = (develop, mattes, ticket, src, adjustments, target, options);
    todo!("rust-engine-dev: develop::masks::render_overlay")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::{AiTarget, BrushMask, BrushStroke, LuminanceRange};

    fn component(shape: MaskShape) -> MaskComponent {
        MaskComponent {
            id: format!("{:032X}", 7),
            name: String::new(),
            active: true,
            mode: MaskBlendMode::Add,
            inverted: false,
            opacity: 1.0,
            shape,
        }
    }

    #[test]
    fn range_guide_is_needed_only_for_ranges_and_auto_mask() {
        let group = |c: MaskComponent| MaskGroup {
            id: format!("{:032X}", 1),
            name: String::new(),
            active: true,
            amount: 1.0,
            adjustments: Default::default(),
            components: vec![c],
        };
        let ai = component(MaskShape::Ai(AiMask { target: AiTarget::Sky, reference_point: None, digest: None }));
        assert!(!needs_range_guide(&[group(ai)]));
        let lum = component(MaskShape::Luminance(LuminanceRange {
            feather_low: 0.0,
            low: 0.1,
            high: 0.9,
            feather_high: 1.0,
            smoothness: 50.0,
        }));
        assert!(needs_range_guide(&[group(lum.clone())]));
        let mut off = group(lum);
        off.active = false;
        assert!(!needs_range_guide(&[off]));
        let stroke = |auto_mask| BrushStroke {
            radius: 0.01,
            flow: 1.0,
            feather: 0.5,
            density: 1.0,
            erase: false,
            auto_mask,
            dabs: vec![NormPoint { x: 0.5, y: 0.5 }],
        };
        let brush = |a| component(MaskShape::Brush(BrushMask { strokes: vec![stroke(a)] }));
        assert!(!needs_range_guide(&[group(brush(false))]));
        assert!(needs_range_guide(&[group(brush(true))]));
    }
}
