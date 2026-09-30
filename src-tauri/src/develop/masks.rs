//! Local adjustments (Phase 7c, IPC v10): mask evaluation at any resolution, per-pixel local
//! parameters for the shared pipeline (preview + export), mask overlays and the AI matte
//! cache. Owned by rust-engine-dev; the architect fixed the surface below (types, function
//! signatures, semantics); the bodies are rust-engine-dev's.
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

mod cache;
mod eval;
mod overlay;
mod planes;
pub mod render;

#[cfg(test)]
mod tests_eval;

pub use eval::{crop_frame, geometry_affine, luminance_weight, Affine};
pub use overlay::{guide_from_srgb8, linear_srgb_to_lab};

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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
        self.sample_f64(f64::from(p.x), f64::from(p.y))
    }

    /// [`Self::sample`] on f64 coordinates (render loops).
    #[inline]
    pub fn sample_f64(&self, x: f64, y: f64) -> f32 {
        let b = &self.bounds;
        let (bw, bh) = (f64::from(b.width), f64::from(b.height));
        if self.width == 0 || self.height == 0 || bw <= 0.0 || bh <= 0.0 {
            return 0.0;
        }
        let u = (x - f64::from(b.x)) / bw;
        let v = (y - f64::from(b.y)) / bh;
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return 0.0;
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let fx = (u * w as f64 - 0.5).clamp(0.0, (w - 1) as f64);
        let fy = (v * h as f64 - 0.5).clamp(0.0, (h - 1) as f64);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
        let d = &self.data;
        let at = |x: usize, y: usize| f32::from(d[y * w + x]);
        let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
        let bot = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
        (top + (bot - top) * ty) / 255.0
    }

    /// Mean value over the whole sensor frame, 0..=1 (`AiMaskInfo.coverage`).
    pub fn coverage(&self) -> f32 {
        if self.data.is_empty() {
            return 0.0;
        }
        let mean = self.data.iter().map(|&v| u64::from(v)).sum::<u64>() as f64 / (self.data.len() as f64 * 255.0);
        let b = &self.bounds;
        // The bitmap's area inside the frame (bitmaps may overhang it slightly).
        let w = (f64::from(b.x + b.width).min(1.0) - f64::from(b.x).max(0.0)).max(0.0);
        let h = (f64::from(b.y + b.height).min(1.0) - f64::from(b.y).max(0.0)).max(0.0);
        (mean * w * h).clamp(0.0, 1.0) as f32
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
    lru: cache::SharedLru,
}

impl MaskCache {
    /// No I/O (called during app setup).
    pub fn new(config: MaskCacheConfig) -> Self {
        Self { config: Arc::new(config), lru: Arc::new(Mutex::new(cache::Lru::default())) }
    }

    fn file(&self, image_id: ImageId, digest: &str) -> PathBuf {
        self.config.cache_dir.join(cache::rel_path(image_id, digest))
    }

    /// XMP read path: decodes the sidecar's Lightroom mattes into the cache (origin
    /// `lightroom`, model version `lr:<crs:ModelVersion>`), skipping those already cached.
    /// Returns per-matte errors (the component then renders empty / `needs_update`).
    pub fn import_lightroom(
        &self,
        conn: &Connection,
        image_id: ImageId,
        mattes: &[crate::xmp::masks::LightroomMatte],
    ) -> Vec<String> {
        let mut errors = Vec::new();
        for m in mattes {
            let cached = cache::by_digest(conn, image_id, &m.digest).ok().flatten().is_some()
                && self.file(image_id, &m.digest).exists();
            if cached {
                continue;
            }
            let result = crate::xmp::masks::decode_matte(m).and_then(|mask| {
                let matte = NewMatte {
                    kind: m.kind.clone(),
                    target: m.target.clone(),
                    reference_point: m.reference_point,
                    origin: AiMaskOrigin::Lightroom,
                    digest: Some(m.digest.clone()),
                    model_version: format!("lr:{}", m.model_version.as_deref().unwrap_or("unknown")),
                    input_digest: m.input_digest.clone(),
                };
                self.put(conn, image_id, &matte, &mask).map(|_| ()).map_err(|e| e.message)
            });
            if let Err(e) = result {
                errors.push(format!("matte {}: {e}", m.digest));
            }
        }
        errors
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
        if mask.width == 0 || mask.height == 0 || mask.data.len() != mask.width as usize * mask.height as usize {
            return Err(crate::ipc::error::AppError::invalid("matte size does not match its data"));
        }
        let png = cache::encode_png(mask).map_err(crate::ipc::error::AppError::internal)?;
        let digest = match &matte.digest {
            Some(d) => d.trim().to_ascii_uppercase(),
            None => crate::xmp::masks::md5_hex(&png),
        };
        let rel = cache::rel_path(image_id, &digest);
        cache::write_atomic(&self.config.cache_dir.join(&rel), &png)?;
        let row = cache::Row {
            digest: digest.clone(),
            kind: matte.kind.clone(),
            origin: matte.origin,
            model_version: matte.model_version.clone(),
            width: mask.width,
            height: mask.height,
            bounds: mask.bounds,
            coverage: mask.coverage(),
        };
        cache::insert(conn, image_id, &row, matte.input_digest.as_deref(), &rel, crate::db::now_ms())?;
        cache::lock(&self.lru).put((image_id, digest), Arc::new(mask.clone()));
        let ai = AiMask { target: matte.target.clone(), reference_point: matte.reference_point, digest: None };
        Ok(row.info(&ai))
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
        let row = match &ai.digest {
            Some(d) => cache::by_digest(conn, image_id, d)?,
            None => cache::newest_sieve(conn, image_id, &ai.cache_kind(), model_version)?,
        };
        Ok(row.map(|r| r.info(ai)))
    }

    /// Decoded matte by digest (LRU; loads the PNG on a miss).
    pub fn load(&self, image_id: ImageId, digest: &str) -> AppResult<Option<Arc<AlphaMask>>> {
        let key = (image_id, digest.to_owned());
        if let Some(m) = cache::lock(&self.lru).get(&key) {
            return Ok(Some(m));
        }
        let Some(mask) = cache::load_file(&self.file(image_id, digest))? else { return Ok(None) };
        let mask = Arc::new(mask);
        cache::lock(&self.lru).put(key, mask.clone());
        Ok(Some(mask))
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
        use crate::ipc::types::AiMaskState;
        let mut out = Vec::new();
        for g in groups {
            for c in &g.components {
                let MaskShape::Ai(ai) = &c.shape else { continue };
                let status =
                    |state, info| AiMaskStatus { group_id: g.id.clone(), component_id: c.id.clone(), state, info };
                // An explicit matte (Lightroom's or a computed one) renders whatever the models.
                if ai.digest.is_some() {
                    if let Some(info) = self.resolve(conn, image_id, ai, None)? {
                        out.push(status(AiMaskState::Ready, Some(info)));
                        continue;
                    }
                }
                let Some(family) = ai.target.family() else {
                    out.push(status(AiMaskState::Unavailable, None));
                    continue;
                };
                let model = segmenter.model_version(family);
                let plain = AiMask { digest: None, ..ai.clone() };
                if let Some(mv) = &model {
                    if let Some(info) = self.resolve(conn, image_id, &plain, Some(mv))? {
                        out.push(status(AiMaskState::Ready, Some(info)));
                        continue;
                    }
                }
                let state = if segmenter.is_computing(image_id, &ai.cache_kind()) {
                    AiMaskState::Computing
                } else if model.is_none() {
                    AiMaskState::Unavailable
                } else {
                    AiMaskState::NeedsUpdate
                };
                out.push(status(state, None));
            }
        }
        Ok(out)
    }

    /// Deletes files of rows that no longer exist (images removed) and unreferenced Sieve
    /// mattes superseded by a newer model version. Called at startup (background).
    pub fn sweep(&self, conn: &Connection) -> AppResult<usize> {
        cache::sweep(conn, &self.masks_dir(), &self.lru)
    }
}

/// Read-only catalog connection (renders / `DevelopInfo` resolve mattes with it).
pub fn open_catalog_read_only(path: &std::path::Path) -> AppResult<Connection> {
    cache::open_read_only(path)
}

impl MatteSource for MaskCache {
    fn matte(&self, image_id: ImageId, ai: &AiMask) -> Option<Arc<AlphaMask>> {
        let digest = match &ai.digest {
            Some(d) => d.clone(),
            None => {
                let conn = cache::open_read_only(&self.config.catalog_path).ok()?;
                cache::newest_sieve(&conn, image_id, &ai.cache_kind(), None).ok()??.digest
            }
        };
        self.load(image_id, &digest).ok().flatten()
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
        let (u, v) = eval::geometry_affine(self).apply(f64::from(px), f64::from(py));
        NormPoint { x: u as f32, y: v as f32 }
    }

    /// Output pixels per sensor-frame width unit (brush radius scale).
    pub fn sensor_width_px(&self) -> f32 {
        let t = eval::geometry_affine(self);
        (f64::from(self.sensor_width) / eval::sensor_px_per_output_px(self, &t)) as f32
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
    eval::evaluate_component(component, geom, image_id, mattes, guide)
}

/// Combines `value` into `acc` with `mode` (module docs). Both planes have equal length.
pub fn combine(acc: &mut [f32], value: &[f32], mode: MaskBlendMode) {
    eval::combine(acc, value, mode)
}

/// Evaluates every active group on the output grid (rayon over rows).
pub fn evaluate(
    masks: &[MaskGroup],
    geom: &MaskGeometry,
    image_id: ImageId,
    mattes: &dyn MatteSource,
    guide: Option<&RangeGuide>,
) -> GroupWeights {
    eval::evaluate(masks, geom, image_id, mattes, guide)
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
/// (unset when every active group has 0 for it), plus the group blends. The sums are
/// evaluated per pixel from the shared group weight planes ([`LocalPlanes::value`]); a
/// materialized plane ([`LocalPlanes::get`]) is only built on request, so a render holds one
/// plane per active group, not one per parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalPlanes {
    pub width: u32,
    pub height: u32,
    /// Weight planes of the groups that set any additive parameter.
    weights: Vec<Arc<Vec<f32>>>,
    /// Per parameter: (index into `weights`, the group's value).
    terms: Vec<Vec<(usize, f32)>>,
    /// Per parameter: upper bound of `|value|`.
    bounds: Vec<f32>,
    planes: Vec<std::sync::OnceLock<Vec<f32>>>,
    pub blends: Vec<GroupBlend>,
}

impl LocalPlanes {
    /// `None` when no group is active with a non-neutral adjustment set.
    pub fn build(masks: &[MaskGroup], weights: &GroupWeights) -> Option<LocalPlanes> {
        planes::build(masks, weights)
    }

    /// [`Self::build`] over shared weight planes (`weights[i]` = group `masks[i]`, `None` =
    /// inactive/empty) on a `width x height` grid; no copies (the render's weight cache).
    pub fn from_shared(
        masks: &[MaskGroup],
        weights: &[Option<Arc<Vec<f32>>>],
        width: u32,
        height: u32,
    ) -> Option<LocalPlanes> {
        planes::from_shared(masks, weights, width, height)
    }
}

pub use planes::PreparedBlends;

/// Per-group point curve (+ refine saturation) and colour tint, each blended by the group
/// weight, on the working image: **display-linear ProPhoto** RGB (the pipeline's colour
/// after the global point curves, before the output space), row-major. The pipeline applies
/// the same per pixel through [`LocalPlanes::prepare_blends`].
pub fn apply_group_blends(rgb: &mut [[f32; 3]], local: &LocalPlanes) {
    planes::apply_group_blends(rgb, local)
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
    overlay::render_overlay(develop, mattes, ticket, src, adjustments, target, options)
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
