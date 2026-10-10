//! Lightroom-style "Auto" (IPC v14): Basic-panel auto tone and auto white balance.
//! Contract by the architect; bodies: rust-engine-dev.
//!
//! - [`auto_tone`]: render `adjustments` with the requested sliders at 0 (the rest as given,
//!   incl. white balance and profile) through `DevelopCache::render_image` at a small size,
//!   measure the output and solve absolute values for the requested sliders (`keys`, a subset of
//!   `AdjustmentField::AUTO_TONE`; `None` = all of them). Target Lightroom's behaviour:
//!   exposure from the (face-weighted, when faces are known) mid-tone level, whites/blacks to
//!   the clip points without clipping skin, highlights/shadows to recover detail, contrast,
//!   vibrance and saturation modest. Results are within slider ranges, rounded like Lightroom
//!   (exposure to 0.05 EV, others to integers). Deterministic.
//! - [`auto_white_balance`]: temperature/tint that neutralise the scene's estimated illuminant
//!   (grey-world on low-chroma, unclipped pixels with a skin-tone guard), via
//!   `camera::values_of_multipliers` like the WB picker; clamped to slider ranges.
//! - Errors: the develop source cannot be decoded -> the usual `file_missing` / `decode_failed`.
//!
//! # Auto tone
//! Measurements are on the rendered 8-bit sRGB output (what the user sees), uncropped (so the
//! analysis face boxes, in the oriented preview frame, line up), without the LUT, with every
//! Auto slider at 0 (a slider's Auto value does not depend on which sliders were requested).
//! 1. Exposure stage: secant solve on the render so the tonal key (mean of median and mean
//!    L*) meets [`AutoParams::key`]; with faces the face mean L* weighs [`FACE_WEIGHT`]. A
//!    share ([`AutoParams::keep`]) of the frame's own deviation is kept.
//! 2. Slider models: linear in the exposure-stage EV and scene features ([`Stats::features`]),
//!    fitted to Camera Raw's own Auto ([`MODELS`]); Contrast and Vibrance are nearly constant
//!    there (6, 20).
//! 3. Skin guard: while more than [`SKIN_CLIP_MAX`] of the skin pixels (face boxes, else
//!    skin-coloured pixels) have a channel at or above 250, lower Whites, then Highlights,
//!    then Exposure.
//!
//! # Not analysed yet (`faces = None`)
//! Skin-coloured pixels stand in for the face boxes, conservatively (warm bright scenes are
//! full of skin-coloured pixels that are not skin): the mask is estimated once, on the render
//! with every Auto slider at 0, from unclipped pixels only, kept only as compact regions
//! ([`estimate_skin`]); if it covers more than [`EST_SKIN_MAX`] of the frame it is not skin.
//! The guard is bounded ([`ESTIMATED_GUARD`]), so Auto without faces stays close to Auto
//! with them. Callers first try on-demand detection ([`resolve_faces`], `ml::auto_faces`:
//! the analysis' detector on the neutral render), so `None` only reaches this function when
//! the detector is unavailable.
//!
//! # Auto white balance
//! Grey-world on near-neutral pixels of the camera-space source (skin and sky chroma
//! excluded), converted like the WB picker; clamped to Camera Raw's Auto range
//! ([`AUTO_WB_MAX_TEMP`], [`AUTO_WB_TINT`]).

use super::{DevelopCache, SourceImage};
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::{AdjustmentField, AutoToneValues, NormRect, ParametricAdjustments, WhiteBalanceValues};
use crate::lut::LutLibrary;

/// Long edge of the measurement renders.
const MEASURE_EDGE: u32 = 384;
/// Target tonal key (L*): mean of the median and mean L* of the frame.
const KEY_TARGET: f32 = 48.0;
/// Target mean face L* (skin mid-tone of a well exposed portrait).
const FACE_TARGET: f32 = 42.0;
/// Share of the frame's own key deviation kept by Auto exposure (see [`AutoTargets::keep`]).
const KEY_KEEP: f32 = 0.5;
/// Weight of the face term in the exposure target when faces are known.
const FACE_WEIGHT: f32 = 0.7;
/// Max share of skin pixels with a channel >= 250 in the result.
pub const SKIN_CLIP_MAX: f32 = 0.003;

/// Smallest / largest share of the frame an estimated skin mask may cover.
const EST_SKIN_MIN: f32 = 0.005;
pub const EST_SKIN_MAX: f32 = 0.30;
/// Cell edge (px of the measurement render) of the skin region grid.
const SKIN_CELL: usize = 8;

/// Bounds of the skin guard when the skin is estimated (no face boxes): the guard may cost
/// at most this much relative to the slider models' values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EstimatedGuard {
    /// Lowest Whites (the model's value if that is lower already).
    pub whites_floor: f32,
    /// Most the Highlights may go below the model.
    pub highlights_drop: f32,
    /// Most the Exposure may go below the model (EV).
    pub exposure_drop: f32,
}

pub const ESTIMATED_GUARD: EstimatedGuard =
    EstimatedGuard { whites_floor: -30.0, highlights_drop: 30.0, exposure_drop: 0.3 };

/// Faces known from the analysis (normalized, oriented preview frame, uncropped).
pub fn auto_tone(
    cache: &DevelopCache,
    src: &SourceImage,
    adjustments: &ParametricAdjustments,
    keys: &[AdjustmentField],
) -> AppResult<AutoToneValues> {
    auto_tone_with_faces(cache, src, adjustments, keys, None)
}

/// 8-bit sRGB -> linear.
fn lstar_lut() -> [f32; 256] {
    let mut out = [0.0; 256];
    for (i, o) in out.iter_mut().enumerate() {
        let e = i as f64 / 255.0;
        let lin = if e <= 0.04045 { e / 12.92 } else { ((e + 0.055) / 1.055).powf(2.4) };
        *o = lin as f32;
    }
    out
}

fn y_to_lstar(y: f32) -> f32 {
    let y = y.max(0.0);
    if y > 216.0 / 24389.0 {
        116.0 * y.cbrt() - 16.0
    } else {
        y * 24389.0 / 27.0
    }
}

/// Per-render measurements.
struct Stats {
    /// L* per pixel.
    l: Vec<f32>,
    /// Brightest channel per pixel (8-bit).
    maxc: Vec<u8>,
    /// Luminance per pixel (8-bit sRGB-encoded).
    luma: Vec<u8>,
    /// Chroma (a*, b* magnitude, approximate) per pixel.
    chroma: Vec<f32>,
    /// Pixel is skin (inside a face box, else skin-coloured).
    skin: Vec<bool>,
    /// Render width (pixels are row-major).
    width: usize,
}

impl Stats {
    /// `faces`: `Some` = the analysis ran (skin = the face boxes; none = no skin), `None` =
    /// unknown: `estimated` (from [`estimate_skin`]) stands in, or no skin without it.
    fn from_render(
        img: &super::pipeline::RenderedImage,
        faces: Option<&[NormRect]>,
        estimated: Option<&[bool]>,
    ) -> Stats {
        let lin = lstar_lut();
        let (w, h) = (img.width as usize, img.height as usize);
        let n = w * h;
        let mut s = Stats {
            l: Vec::with_capacity(n),
            maxc: Vec::with_capacity(n),
            luma: Vec::with_capacity(n),
            chroma: Vec::with_capacity(n),
            skin: vec![false; n],
            width: w,
        };
        for px in img.rgb.as_chunks::<3>().0 {
            let (r, g, b) = (lin[px[0] as usize], lin[px[1] as usize], lin[px[2] as usize]);
            let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let x = 0.4124 * r + 0.3576 * g + 0.1805 * b;
            let z = 0.0193 * r + 0.1192 * g + 0.9505 * b;
            let f = |t: f32| if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
            let (fx, fy, fz) = (f(x / 0.95047), f(y), f(z / 1.08883));
            let a = 500.0 * (fx - fy);
            let bb = 200.0 * (fy - fz);
            s.l.push(y_to_lstar(y));
            s.maxc.push(px[0].max(px[1]).max(px[2]));
            let ye = if y <= 0.003_130_8 { 12.92 * y } else { 1.055 * y.powf(1.0 / 2.4) - 0.055 };
            s.luma.push((ye * 255.0).round().clamp(0.0, 255.0) as u8);
            s.chroma.push(a.hypot(bb));
        }
        // Skin: the central part of each face box (boxes include hair and background).
        for f in faces.unwrap_or_default() {
            let (cx, cy) = (f.x + f.width / 2.0, f.y + f.height / 2.0);
            let (hw, hh) = (f.width * 0.35, f.height * 0.35);
            let x0 = (((cx - hw) * w as f32).floor().max(0.0)) as usize;
            let x1 = (((cx + hw) * w as f32).ceil() as usize).min(w);
            let y0 = (((cy - hh) * h as f32).floor().max(0.0)) as usize;
            let y1 = (((cy + hh) * h as f32).ceil() as usize).min(h);
            for y in y0..y1 {
                for x in x0..x1 {
                    s.skin[y * w + x] = true;
                }
            }
        }
        if faces.is_none() {
            if let Some(est) = estimated.filter(|e| e.len() == n) {
                s.skin.copy_from_slice(est);
            }
        }
        s
    }

    fn has_skin(&self) -> bool {
        self.skin.iter().any(|&v| v)
    }

    fn key(&self) -> f32 {
        let mut l = self.l.clone();
        let mid = l.len() / 2;
        let median = *l.select_nth_unstable_by(mid, f32::total_cmp).1;
        let mean = self.l.iter().sum::<f32>() / self.l.len().max(1) as f32;
        0.5 * (median + mean)
    }

    fn skin_mean_l(&self) -> Option<f32> {
        let (mut s, mut n) = (0.0f64, 0usize);
        for (l, &k) in self.l.iter().zip(&self.skin) {
            if k {
                s += f64::from(*l);
                n += 1;
            }
        }
        (n > 0).then(|| (s / n as f64) as f32)
    }

    fn frac(&self, pred: impl Fn(f32) -> bool) -> f32 {
        self.l.iter().filter(|&&l| pred(l)).count() as f32 / self.l.len().max(1) as f32
    }

    fn percentile_u8(values: impl Iterator<Item = u8>, p: f32) -> u8 {
        let mut hist = [0usize; 256];
        let mut n = 0usize;
        for v in values {
            hist[v as usize] += 1;
            n += 1;
        }
        if n == 0 {
            return 0;
        }
        let target = ((n as f32) * p).ceil().max(1.0) as usize;
        let mut acc = 0;
        for (v, c) in hist.iter().enumerate() {
            acc += c;
            if acc >= target {
                return v as u8;
            }
        }
        255
    }

    fn white_p995(&self) -> u8 {
        Self::percentile_u8(self.maxc.iter().copied(), 0.995)
    }

    fn black_p005(&self) -> u8 {
        Self::percentile_u8(self.luma.iter().copied(), 0.005)
    }

    /// Share of skin pixels with a channel >= 250.
    pub fn skin_clipped(&self) -> f32 {
        let (mut c, mut n) = (0usize, 0usize);
        for (&m, &k) in self.maxc.iter().zip(&self.skin) {
            if k {
                n += 1;
                c += usize::from(m >= 250);
            }
        }
        if n == 0 {
            0.0
        } else {
            c as f32 / n as f32
        }
    }

    fn l_sd(&self) -> f32 {
        let n = self.l.len().max(1) as f32;
        let mean = self.l.iter().sum::<f32>() / n;
        (self.l.iter().map(|l| (l - mean).powi(2)).sum::<f32>() / n).sqrt()
    }

    /// Summary for the baseline's low-key / silhouette detection ([`FrameStats`]).
    /// `faces_known`: the skin mask is face boxes (not estimated skin).
    fn frame_stats(&self, faces_known: bool) -> FrameStats {
        let n = self.l.len().max(1);
        let w = self.width.max(1);
        let h = self.l.len() / w;
        let (mut centre, mut nc, mut border, mut nb) = (0.0f64, 0usize, 0.0f64, 0usize);
        for (i, &l) in self.l.iter().enumerate() {
            let (x, y) = (i % w, i / w);
            let inner = x >= w / 4 && x < w - w / 4 && y >= h / 4 && y < h - h / 4;
            if inner {
                centre += f64::from(l);
                nc += 1;
            } else {
                border += f64::from(l);
                nb += 1;
            }
        }
        FrameStats {
            mean_l: self.l.iter().sum::<f32>() / n as f32,
            key: self.key(),
            dark: self.frac(|l| l < 20.0),
            mid: self.frac(|l| (20.0..=70.0).contains(&l)),
            bright: self.frac(|l| l > 70.0),
            white_p995: f32::from(self.white_p995()) / 255.0,
            face_l: if faces_known { self.skin_mean_l() } else { None },
            centre_l: if nc > 0 { (centre / nc as f64) as f32 } else { 0.0 },
            border_l: if nb > 0 { (border / nb as f64) as f32 } else { 0.0 },
        }
    }

    /// Mean chroma of non-skin mid-tone pixels.
    fn mean_chroma(&self) -> f32 {
        let (mut s, mut n) = (0.0f64, 0usize);
        for i in 0..self.l.len() {
            if !self.skin[i] && (15.0..95.0).contains(&self.l[i]) {
                s += f64::from(self.chroma[i]);
                n += 1;
            }
        }
        if n == 0 {
            0.0
        } else {
            (s / n as f64) as f32
        }
    }
}

/// Tonal summary of a frame as the camera exposed it (every Auto slider at 0, the given look
/// and white balance), measured on the uncropped measurement render. Used by the baseline
/// edit's low-key / silhouette detection (`develop::baseline::low_key`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameStats {
    /// Mean L*.
    pub mean_l: f32,
    /// Tonal key (mean of median and mean L*), what Auto exposure aims at.
    pub key: f32,
    /// Share of pixels with L* < 20 / 20..=70 / > 70.
    pub dark: f32,
    pub mid: f32,
    pub bright: f32,
    /// 99.5th percentile of the brightest channel, 0..=1 (highlights near 1 = the exposure
    /// was set for them).
    pub white_p995: f32,
    /// Mean L* of the face centres when face boxes are known (`None`: no faces / unknown).
    pub face_l: Option<f32>,
    /// Mean L* of the central half (by width and height) and of the ring around it.
    pub centre_l: f32,
    pub border_l: f32,
}

/// Skin mask of a photo without face boxes, from the render with every Auto slider at 0:
/// skin-coloured (warm hue, moderate chroma, mid L*), unclipped pixels, kept only in compact
/// regions (grid cells of [`SKIN_CELL`] px at least half skin, 4-connected groups of >= 3
/// cells). Empty (no skin) when the regions cover less than [`EST_SKIN_MIN`] (landscapes,
/// products) or more than [`EST_SKIN_MAX`] of the frame (warm light on sand, walls, wood:
/// not people).
fn estimate_skin(img: &super::pipeline::RenderedImage, s: &Stats) -> Vec<bool> {
    let (w, h) = (img.width as usize, img.height as usize);
    let n = w * h;
    let mut px_skin = vec![false; n];
    for (i, px) in img.rgb.as_chunks::<3>().0.iter().enumerate() {
        let (r, g, b) = (i32::from(px[0]), i32::from(px[1]), i32::from(px[2]));
        px_skin[i] = s.maxc[i] < 250
            && r > g
            && g > b
            && r - b > 20
            && r - g < 110
            && (35.0..97.0).contains(&s.l[i])
            && (8.0..55.0).contains(&s.chroma[i]);
    }
    let (cw, ch) = (w.div_ceil(SKIN_CELL), h.div_ceil(SKIN_CELL));
    let mut cell = vec![false; cw * ch];
    for cy in 0..ch {
        for cx in 0..cw {
            let (mut k, mut t) = (0usize, 0usize);
            for y in cy * SKIN_CELL..((cy + 1) * SKIN_CELL).min(h) {
                for x in cx * SKIN_CELL..((cx + 1) * SKIN_CELL).min(w) {
                    t += 1;
                    k += usize::from(px_skin[y * w + x]);
                }
            }
            cell[cy * cw + cx] = 2 * k >= t;
        }
    }
    // 4-connected groups of skin cells; groups of fewer than 3 cells are texture, not skin.
    let mut keep = vec![false; cw * ch];
    let mut seen = vec![false; cw * ch];
    let (mut stack, mut group) = (Vec::new(), Vec::new());
    for start in 0..cw * ch {
        if !cell[start] || seen[start] {
            continue;
        }
        group.clear();
        stack.push(start);
        seen[start] = true;
        while let Some(c) = stack.pop() {
            group.push(c);
            let (x, y) = (c % cw, c / cw);
            let neighbours = [
                (x > 0).then(|| c - 1),
                (x + 1 < cw).then(|| c + 1),
                (y > 0).then(|| c - cw),
                (y + 1 < ch).then(|| c + cw),
            ];
            for nc in neighbours.into_iter().flatten() {
                if cell[nc] && !seen[nc] {
                    seen[nc] = true;
                    stack.push(nc);
                }
            }
        }
        if group.len() >= 3 {
            group.iter().for_each(|&c| keep[c] = true);
        }
    }
    let mut mask = vec![false; n];
    let mut count = 0usize;
    for (i, m) in mask.iter_mut().enumerate() {
        let (x, y) = (i % w, i / w);
        if px_skin[i] && keep[(y / SKIN_CELL) * cw + x / SKIN_CELL] {
            *m = true;
            count += 1;
        }
    }
    let share = count as f32 / n.max(1) as f32;
    if !(EST_SKIN_MIN..=EST_SKIN_MAX).contains(&share) {
        mask.iter_mut().for_each(|v| *v = false);
    }
    mask
}

/// The analysis' faces of image `id`: `None` if it has not been analysed (no
/// `image_analysis.faces_json`), `Some(vec![])` if it was and has no faces.
pub fn analysis_faces(
    conn: &rusqlite::Connection,
    id: crate::ipc::types::ImageId,
) -> AppResult<Option<Vec<crate::ipc::types::FaceInfo>>> {
    use rusqlite::OptionalExtension;
    let json: Option<Option<String>> =
        conn.query_row("SELECT faces_json FROM image_analysis WHERE image_id = ?1", [id], |r| r.get(0)).optional()?;
    match json.flatten() {
        Some(j) => Ok(Some(serde_json::from_str(&j)?)),
        None => Ok(None),
    }
}

/// The face boxes auto tone weighs: faces the analysis considered (large enough to judge),
/// else every confident detection.
pub fn face_boxes(faces: &[crate::ipc::types::FaceInfo]) -> Vec<NormRect> {
    let considered: Vec<NormRect> = faces.iter().filter(|f| f.considered).map(|f| f.bbox).collect();
    if !considered.is_empty() {
        return considered;
    }
    faces.iter().filter(|f| f.detection_score >= 0.6).map(|f| f.bbox).collect()
}

/// The face boxes Auto tone should use for `src`: the analysis' (`analysis`, `Some` once
/// the analysis ran), else the on-demand detector's (`ml::auto_faces`, when `cache` has one
/// and its model loads), else `None` (the bounded skin-colour estimate). Blocking.
pub fn resolve_faces(
    cache: &DevelopCache,
    src: &SourceImage,
    analysis: Option<Vec<NormRect>>,
) -> Option<Vec<NormRect>> {
    analysis.or_else(|| cache.auto_faces()?.detect(cache, src))
}

/// Renders `adj` for measurement: uncropped, without LUT, small.
struct Probe<'a> {
    cache: &'a DevelopCache,
    src: &'a SourceImage,
    luts: LutLibrary,
    faces: Option<&'a [NormRect]>,
    /// Estimated skin (`faces = None`), fixed by the first measurement.
    estimated: Option<Vec<bool>>,
    renders: u32,
}

impl<'a> Probe<'a> {
    fn new(cache: &'a DevelopCache, src: &'a SourceImage, faces: Option<&'a [NormRect]>) -> Self {
        Probe {
            cache,
            src,
            luts: LutLibrary::new(std::env::temp_dir().join("sieve-auto-no-luts")),
            faces,
            estimated: None,
            renders: 0,
        }
    }

    /// Measures `adj`; with `faces = None`, the first call estimates the skin mask.
    fn measure(&mut self, adj: &ParametricAdjustments) -> AppResult<Stats> {
        let mut a = adj.clone();
        a.crop.enabled = false;
        a.lut = None;
        a.effects.grain.amount = 0.0;
        a.effects.vignette.amount = 0.0;
        self.renders += 1;
        let r = self.cache.render_image(self.src, &a, None, MEASURE_EDGE, &self.luts)?;
        if self.faces.is_none() && self.estimated.is_none() {
            let s = Stats::from_render(&r.image, None, None);
            self.estimated = Some(estimate_skin(&r.image, &s));
        }
        Ok(Stats::from_render(&r.image, self.faces, self.estimated.as_deref()))
    }
}

fn round_to(v: f32, step: f32) -> f32 {
    let r = (v / step).round() * step;
    if r == 0.0 {
        0.0
    } else {
        r
    }
}

/// Exposure-stage targets of [`auto_tone`] (the app uses the defaults; evaluations override
/// them with `examples/auto_eval.rs --set name=value`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoParams {
    /// Tonal key L* (mean of median and mean) the exposure stage aims at.
    pub key: f32,
    /// Mean face L* the exposure stage aims at.
    pub face: f32,
    /// Weight of the face term when faces are known.
    pub face_weight: f32,
    /// Share of the frame's own deviation from the targets that is kept (0 = full correction;
    /// high-key and night scenes keep part of their character).
    pub keep: f32,
}

impl Default for AutoParams {
    fn default() -> Self {
        AutoParams { key: KEY_TARGET, face: FACE_TARGET, face_weight: FACE_WEIGHT, keep: KEY_KEEP }
    }
}

impl AutoParams {
    /// Sets a field by name; `false` for unknown names.
    pub fn set(&mut self, name: &str, v: f32) -> bool {
        let slot = match name {
            "key" => &mut self.key,
            "face" => &mut self.face,
            "face_weight" => &mut self.face_weight,
            "keep" => &mut self.keep,
            _ => return false,
        };
        *slot = v;
        true
    }
}

/// Linear slider models fitted to Camera Raw's own Auto (Adobe DNG Converter resolving
/// `crs:AutoTone` + Auto WB on the user's 374 edited frames; ridge regression on the even
/// frames, validated on the odd ones, see `docs/decisions.md`). Inputs: `[1, exposure-stage
/// EV, features...]` ([`Stats::features`]); outputs clamped to [`MODEL_RANGES`].
#[allow(clippy::excessive_precision)]
const MODELS: [[f32; 1 + 1 + N_FEATURES]; 8] = [
    // Exposure (calibrates the exposure stage's result).
    [3.37452, 0.46315, -0.70482, 0.64905, -2.75620, -4.26411, -0.67654, -0.32532, -0.80616, 0.08421],
    // Contrast: Camera Raw's Auto gives 4..7 (mean 5.9) whatever the scene.
    [6.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    // Highlights.
    [-37.80097, 1.28665, -21.45440, -8.20648, -20.15801, -6.76630, -14.74667, -36.54991, 7.33145, 1.33387],
    // Shadows.
    [36.79535, 3.71712, 1.08612, 22.31174, 3.25563, -92.94788, 30.34745, 17.69329, -6.60557, -0.82776],
    // Whites.
    [152.50640, 0.48751, 3.12834, -20.86273, -136.36740, -35.78758, -34.09167, 8.84979, 9.53731, -0.01699],
    // Blacks.
    [-31.68836, 1.54035, -10.10425, -2.13930, 12.72737, -38.05106, -9.15832, 20.70327, -3.16905, 1.68622],
    // Vibrance: 20 (17..20 in Camera Raw, sd 0.2).
    [20.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    // Saturation.
    [3.58559, 0.08928, 0.27635, -0.57852, 0.01004, 3.38464, -6.71426, 5.62551, 1.01114, 0.44343],
];

/// Output ranges of [`MODELS`] (Camera Raw's observed Auto ranges, widened a little).
const MODEL_RANGES: [(f32, f32); 8] =
    [(-4.0, 4.0), (4.0, 7.0), (-100.0, -20.0), (0.0, 90.0), (-40.0, 60.0), (-45.0, 0.0), (17.0, 20.0), (0.0, 10.0)];

/// Stage 1: solves `adj.exposure` (secant on the tonal key, face-weighted); `stats` ends as
/// the measurement at the solved exposure.
fn solve_exposure(
    probe: &mut Probe,
    adj: &mut ParametricAdjustments,
    stats: &mut Stats,
    has_faces: bool,
    targets: &AutoParams,
) -> AppResult<()> {
    let keep = targets.keep.clamp(0.0, 0.95);
    let key_target = targets.key + keep * (stats.key() - targets.key);
    let face_target = stats.skin_mean_l().map_or(targets.face, |f| targets.face + keep * (f - targets.face));
    let err = |s: &Stats| {
        let global = s.key() - key_target;
        match (has_faces, s.skin_mean_l()) {
            (true, Some(face)) => targets.face_weight * (face - face_target) + (1.0 - targets.face_weight) * global,
            _ => global,
        }
    };
    let (mut x0, mut e0) = (adj.exposure, err(stats));
    let mut x1 = (x0 - e0 / 14.0).clamp(-4.0, 4.0);
    for _ in 0..5 {
        if (x1 - x0).abs() < 0.02 {
            break;
        }
        adj.exposure = x1;
        *stats = probe.measure(adj)?;
        let e1 = err(stats);
        if e1.abs() < 0.75 {
            x0 = x1;
            break;
        }
        let slope = if (e1 - e0).abs() > 1e-3 { (e1 - e0) / (x1 - x0) } else { 14.0 };
        let slope = slope.clamp(4.0, 40.0);
        let next = (x1 - e1 / slope).clamp(-4.0, 4.0);
        (x0, e0) = (x1, e1);
        x1 = next;
    }
    adj.exposure = round_to(x0.clamp(-4.0, 4.0), 0.05);
    *stats = probe.measure(adj)?;
    Ok(())
}

/// Number of [`Stats::features`].
pub const N_FEATURES: usize = 8;

impl Stats {
    /// Scene measurements the slider models use (after the exposure stage): share of bright
    /// (L* > 80) and dark (L* < 25) pixels, 99.5th brightest-channel and 0.5th luminance
    /// percentiles (0..1), mean chroma / 100, L* sd / 100, tonal key / 100, faces known.
    fn features(&self, has_faces: bool) -> [f32; N_FEATURES] {
        [
            self.frac(|l| l > 80.0),
            self.frac(|l| l < 25.0),
            f32::from(self.white_p995()) / 255.0,
            f32::from(self.black_p005()) / 255.0,
            self.mean_chroma() / 100.0,
            self.l_sd() / 100.0,
            self.key() / 100.0,
            if has_faces { 1.0 } else { 0.0 },
        ]
    }
}

/// Exposure-stage result and scene features of `adjustments` with every Auto slider at 0
/// (for fitting the slider models to Camera Raw, `examples/auto_eval.rs`).
pub fn exposure_features(
    cache: &DevelopCache,
    src: &SourceImage,
    adjustments: &ParametricAdjustments,
    faces: Option<&[NormRect]>,
    targets: AutoParams,
) -> AppResult<(f32, [f32; N_FEATURES])> {
    let mut adj = adjustments.clone();
    (adj.exposure, adj.contrast, adj.highlights, adj.shadows) = (0.0, 0.0, 0.0, 0.0);
    (adj.whites, adj.blacks, adj.vibrance, adj.saturation) = (0.0, 0.0, 0.0, 0.0);
    let mut probe = Probe::new(cache, src, faces);
    let mut stats = probe.measure(&adj)?;
    let has_faces = faces.is_some_and(|f| !f.is_empty()) && stats.has_skin();
    solve_exposure(&mut probe, &mut adj, &mut stats, has_faces, &targets)?;
    Ok((adj.exposure, stats.features(has_faces)))
}

/// [`auto_tone`] with the analysis face boxes (`FaceInfo.bbox`) of the image.
pub fn auto_tone_with_faces(
    cache: &DevelopCache,
    src: &SourceImage,
    adjustments: &ParametricAdjustments,
    keys: &[AdjustmentField],
    faces: Option<&[NormRect]>,
) -> AppResult<AutoToneValues> {
    auto_tone_opts(cache, src, adjustments, keys, faces, AutoParams::default())
}

/// The light sliders of Auto (Basic Auto minus Vibrance / Saturation, which are look keys of a
/// preset / baseline): what [`auto_light`] solves.
pub const AUTO_LIGHT_FIELDS: &[AdjustmentField] = &[
    AdjustmentField::Exposure,
    AdjustmentField::Contrast,
    AdjustmentField::Highlights,
    AdjustmentField::Shadows,
    AdjustmentField::Whites,
    AdjustmentField::Blacks,
];

/// Result of [`auto_light`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoLight {
    /// [`AUTO_LIGHT_FIELDS`] set (vibrance / saturation `None`).
    pub tone: AutoToneValues,
    /// Auto white balance; `None` when it could not be estimated (too few neutral pixels): the
    /// tone was then measured under `adjustments`' own white balance.
    pub white_balance: Option<WhiteBalanceValues>,
    /// The frame as exposed (every Auto slider at 0) under that white balance.
    pub frame: FrameStats,
}

/// **The** light-only Auto (v21; one function for Develop's Auto on a baseline anchor and the
/// baseline's `LightMeter`, so "Auto, then nudge" starts the anchor's offset from exactly 0):
/// 1. auto white balance of `adjustments` ([`auto_white_balance`]; depends only on the
///    source and the profile, not on the current white balance);
/// 2. auto tone of the [`AUTO_LIGHT_FIELDS`] on `adjustments` with that white balance set
///    (custom), faces as given (callers resolve them with [`resolve_faces`]).
///
/// Equivalent IPC sequence: `auto_white_balance(id, cur)` = `w`, then
/// `auto_tone(id, {...cur, whiteBalance: custom w}, AUTO_LIGHT_FIELDS)` (the `auto_tone`
/// command resolves faces the same way). Vibrance / saturation are never touched. Decode
/// errors propagate; a white balance that cannot be estimated is `None`, not an error.
pub fn auto_light(
    cache: &DevelopCache,
    src: &SourceImage,
    adjustments: &ParametricAdjustments,
    faces: Option<&[NormRect]>,
) -> AppResult<AutoLight> {
    let white_balance = match auto_white_balance(cache, src, adjustments) {
        Ok(wb) => Some(wb),
        Err(e) if e.kind == crate::ipc::error::ErrorKind::InvalidArgument => None,
        Err(e) => return Err(e),
    };
    let probe = match white_balance {
        Some(wb) => ParametricAdjustments {
            white_balance: crate::ipc::types::WhiteBalance::Custom { temperature_k: wb.temperature_k, tint: wb.tint },
            ..adjustments.clone()
        },
        None => adjustments.clone(),
    };
    let (tone, frame) = auto_tone_measured(cache, src, &probe, AUTO_LIGHT_FIELDS, faces, AutoParams::default())?;
    Ok(AutoLight { tone, white_balance, frame })
}

/// [`auto_tone_with_faces`] with explicit parameters.
pub fn auto_tone_opts(
    cache: &DevelopCache,
    src: &SourceImage,
    adjustments: &ParametricAdjustments,
    keys: &[AdjustmentField],
    faces: Option<&[NormRect]>,
    targets: AutoParams,
) -> AppResult<AutoToneValues> {
    auto_tone_measured(cache, src, adjustments, keys, faces, targets).map(|(v, _)| v)
}

/// [`auto_tone_opts`] plus the [`FrameStats`] of its first measurement (every Auto slider at 0:
/// the frame as exposed), at no extra render.
pub fn auto_tone_measured(
    cache: &DevelopCache,
    src: &SourceImage,
    adjustments: &ParametricAdjustments,
    keys: &[AdjustmentField],
    faces: Option<&[NormRect]>,
    targets: AutoParams,
) -> AppResult<(AutoToneValues, FrameStats)> {
    let want = |k: AdjustmentField| keys.contains(&k);
    use AdjustmentField as F;
    // Stages 1-2 on the settings with every Auto slider at 0: a slider's Auto value does not
    // depend on which sliders were requested (Shift-double-click = that slider of full Auto).
    let mut probe = Probe::new(cache, src, faces);
    let mut work = adjustments.clone();
    (work.exposure, work.contrast, work.highlights, work.shadows) = (0.0, 0.0, 0.0, 0.0);
    (work.whites, work.blacks, work.vibrance, work.saturation) = (0.0, 0.0, 0.0, 0.0);
    let mut stats = probe.measure(&work)?;
    let has_faces = faces.is_some_and(|f| !f.is_empty()) && stats.has_skin();
    let frame = stats.frame_stats(has_faces);
    solve_exposure(&mut probe, &mut work, &mut stats, has_faces, &targets)?;
    let features = stats.features(has_faces);
    let mut x = [0.0f32; 1 + 1 + N_FEATURES];
    x[0] = 1.0;
    x[1] = work.exposure;
    x[2..].copy_from_slice(&features);
    let model = |i: usize| {
        let v: f32 = MODELS[i].iter().zip(&x).map(|(w, x)| w * x).sum();
        v.clamp(MODEL_RANGES[i].0, MODEL_RANGES[i].1)
    };

    let mut adj = adjustments.clone();
    let slots: [(AdjustmentField, &mut f32); 8] = [
        (F::Exposure, &mut adj.exposure),
        (F::Contrast, &mut adj.contrast),
        (F::Highlights, &mut adj.highlights),
        (F::Shadows, &mut adj.shadows),
        (F::Whites, &mut adj.whites),
        (F::Blacks, &mut adj.blacks),
        (F::Vibrance, &mut adj.vibrance),
        (F::Saturation, &mut adj.saturation),
    ];
    for (i, (k, slot)) in slots.into_iter().enumerate() {
        if want(k) {
            *slot = if k == F::Exposure { round_to(model(i), 0.05) } else { model(i).round() };
        }
    }

    // 3. Skin guard: never clip skin. Whites first (down to -60), then Highlights, then
    //    Exposure; stop when the skin is clean or nothing requested can lower it (skin blown
    //    in the source). Estimated skin (no face boxes): bounded by [`ESTIMATED_GUARD`].
    if want(F::Whites) || want(F::Exposure) || want(F::Highlights) {
        let (whites_floor, highlights_floor, exposure_floor) = if faces.is_none() {
            let g = ESTIMATED_GUARD;
            (
                adj.whites.min(g.whites_floor),
                (adj.highlights - g.highlights_drop).max(-100.0),
                round_to((adj.exposure - g.exposure_drop).max(-4.0), 0.05),
            )
        } else {
            (-60.0, -100.0, -4.0)
        };
        stats = probe.measure(&adj)?;
        let mut rounds = 0;
        while stats.has_skin() && stats.skin_clipped() > SKIN_CLIP_MAX && rounds < 20 {
            if want(F::Whites) && adj.whites > whites_floor {
                adj.whites = (adj.whites - 15.0).max(whites_floor);
            } else if want(F::Highlights) && adj.highlights > highlights_floor {
                adj.highlights = (adj.highlights - 20.0).max(highlights_floor);
            } else if want(F::Exposure) && adj.exposure > exposure_floor + 0.01 {
                adj.exposure = round_to((adj.exposure - 0.1).max(exposure_floor), 0.05);
            } else {
                break;
            }
            stats = probe.measure(&adj)?;
            rounds += 1;
        }
    }

    let pick = |k: AdjustmentField, v: f32| want(k).then_some(v);
    let values = AutoToneValues {
        exposure: pick(F::Exposure, round_to(adj.exposure.clamp(-5.0, 5.0), 0.05)),
        contrast: pick(F::Contrast, adj.contrast.clamp(-100.0, 100.0).round()),
        highlights: pick(F::Highlights, adj.highlights.clamp(-100.0, 100.0).round()),
        shadows: pick(F::Shadows, adj.shadows.clamp(-100.0, 100.0).round()),
        whites: pick(F::Whites, adj.whites.clamp(-100.0, 100.0).round()),
        blacks: pick(F::Blacks, adj.blacks.clamp(-100.0, 100.0).round()),
        vibrance: pick(F::Vibrance, adj.vibrance.clamp(-100.0, 100.0).round()),
        saturation: pick(F::Saturation, adj.saturation.clamp(-100.0, 100.0).round()),
    };
    Ok((values, frame))
}

/// Share of skin pixels (face boxes, else skin-coloured) with a channel >= 250 in the render
/// of `adj` (uncropped, measurement size). For evaluations and tests.
pub fn skin_clipped_fraction(
    cache: &DevelopCache,
    src: &SourceImage,
    adj: &ParametricAdjustments,
    faces: Option<&[NormRect]>,
) -> AppResult<Option<f32>> {
    let mut probe = Probe::new(cache, src, faces);
    let s = probe.measure(adj)?;
    Ok(s.has_skin().then(|| s.skin_clipped()))
}

/// Log-chroma distance thresholds of the grey-world iterations (wide to narrow).
const WB_THRESHOLDS: [f32; 4] = [0.5, 0.32, 0.22, 0.16];

pub fn auto_white_balance(
    cache: &DevelopCache,
    src: &SourceImage,
    adjustments: &ParametricAdjustments,
) -> AppResult<WhiteBalanceValues> {
    auto_white_balance_opts(cache, src, adjustments, WbOptions::default())
}

/// Tuning of [`grey_world_multipliers`] (the app uses the defaults).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WbOptions {
    /// Brightness weighting exponent of the neutral candidates (bright neutrals such as
    /// white clothes and clouds are the best evidence).
    pub weight_pow: f32,
    /// Also exclude sky-blue candidates (cool, like skin is warm).
    pub exclude_blue: bool,
    /// 0..=1: how far from the as-shot balance towards the estimate (log-multiplier space).
    pub strength: f32,
}

impl Default for WbOptions {
    fn default() -> Self {
        WbOptions { weight_pow: WB_WEIGHT_POW, exclude_blue: WB_EXCLUDE_BLUE, strength: WB_STRENGTH }
    }
}

const WB_WEIGHT_POW: f32 = 0.5;
/// Camera Raw's Auto white balance never goes above 7500 K (302 of the 394 oracle frames sit
/// exactly there: warm evening light is kept warm) ...
pub const AUTO_WB_MAX_TEMP: f32 = 7500.0;
/// ... and keeps Tint within 0..=30 (95 frames at 0, 73 at 30).
pub const AUTO_WB_TINT: (f32, f32) = (0.0, 30.0);
const WB_EXCLUDE_BLUE: bool = true;
const WB_STRENGTH: f32 = 1.0;

/// [`auto_white_balance`] with explicit options.
pub fn auto_white_balance_opts(
    cache: &DevelopCache,
    src: &SourceImage,
    adjustments: &ParametricAdjustments,
    opts: WbOptions,
) -> AppResult<WhiteBalanceValues> {
    let entry = cache.entry(src)?;
    cache.evict(src.id);
    let img = &entry.image;
    let profile = entry.profile(&adjustments.profile);
    let start = img.color.as_shot();
    let est = grey_world_multipliers_opts(&img.pixels, img.width, img.height, start, opts)
        .ok_or_else(|| AppError::invalid("could not estimate a white balance for this photo"))?;
    let g = start[1];
    let k = opts.strength.clamp(0.0, 1.0);
    let mul = [0, 1, 2].map(|c| {
        let s = (start[c] / g).max(1e-6);
        s * (est[c] / s).powf(k)
    });
    let v = super::camera::values_of_multipliers(mul, &img.color, &profile)
        .ok_or_else(|| AppError::invalid("could not estimate a white balance for this photo"))?;
    Ok(WhiteBalanceValues {
        temperature_k: v.temperature_k.round().clamp(super::wb::MIN_TEMP, AUTO_WB_MAX_TEMP),
        tint: v.tint.round().clamp(AUTO_WB_TINT.0, AUTO_WB_TINT.1),
    })
}

/// Grey-world on near-neutral pixels with the default [`WbOptions`].
pub fn grey_world_multipliers(pixels: &[u16], width: u32, height: u32, start: [f32; 3]) -> Option<[f32; 3]> {
    grey_world_multipliers_opts(pixels, width, height, start, WbOptions::default())
}

/// Grey-world on near-neutral pixels (camera RGB, white = 65535): starting from `start`
/// (as-shot multipliers, G = 1), average the camera values of unclipped, not-too-dark pixels
/// whose white-balanced log-chroma is within a shrinking radius of neutral, excluding
/// skin-like chroma, weighting by brightness. `None` when too few neutral pixels exist.
///
/// When the as-shot start leaves a warm cast (tungsten light the camera kept warm), the
/// neutrals themselves look skin-like and the skin exclusion leaves too few candidates: only
/// then (where the guarded pass finds nothing) a second pass runs without the skin exclusion
/// (v21, baseline edit; results of frames the guarded pass handles are unchanged).
pub fn grey_world_multipliers_opts(
    pixels: &[u16],
    width: u32,
    height: u32,
    start: [f32; 3],
    opts: WbOptions,
) -> Option<[f32; 3]> {
    grey_world_pass(pixels, width, height, start, opts, true)
        .or_else(|| grey_world_pass(pixels, width, height, start, opts, false))
}

fn grey_world_pass(
    pixels: &[u16],
    width: u32,
    height: u32,
    start: [f32; 3],
    opts: WbOptions,
    skin_guard: bool,
) -> Option<[f32; 3]> {
    let n = (width as usize) * (height as usize);
    if n == 0 || pixels.len() < n * 3 {
        return None;
    }
    let step = ((n / 250_000).max(1) as f64).sqrt().ceil() as usize;
    let mut m = start.map(|v| if v.is_finite() && v > 0.0 { v } else { 1.0 });
    let g = m[1];
    m = m.map(|v| v / g);
    let mut found = false;
    for &t in &WB_THRESHOLDS {
        let (mut sr, mut sg, mut sb, mut count, mut total) = (0.0f64, 0.0f64, 0.0f64, 0usize, 0usize);
        for y in (0..height as usize).step_by(step) {
            for x in (0..width as usize).step_by(step) {
                let i = (y * width as usize + x) * 3;
                let c = [pixels[i], pixels[i + 1], pixels[i + 2]];
                if c.iter().any(|&v| v >= 64_000) {
                    continue;
                }
                let w = [f32::from(c[0]) * m[0], f32::from(c[1]) * m[1], f32::from(c[2]) * m[2]];
                let lum = (w[0] + 2.0 * w[1] + w[2]) / (4.0 * 65535.0);
                if !(0.01..0.95).contains(&lum) || w[1] <= 0.0 || w[0] <= 0.0 || w[2] <= 0.0 {
                    continue;
                }
                total += 1;
                let r = (w[0] / w[1]).ln();
                let b = (w[2] / w[1]).ln();
                // Skin-like (warm: red up, blue down) is never evidence of the illuminant.
                let skin = skin_guard && r > 0.1 && b < -0.1 && r < 0.75 && b > -0.8 && (r - b) > 0.3;
                let sky = opts.exclude_blue && r < -0.08 && b > 0.08 && (b - r) > 0.25;
                if skin || sky || r * r + b * b > t * t {
                    continue;
                }
                let wt = f64::from(lum.powf(opts.weight_pow));
                sr += f64::from(c[0]) * wt;
                sg += f64::from(c[1]) * wt;
                sb += f64::from(c[2]) * wt;
                count += 1;
            }
        }
        if total == 0 || count < (total / 200).max(50) || sr <= 0.0 || sb <= 0.0 {
            break;
        }
        m = [(sg / sr) as f32, 1.0, (sg / sb) as f32];
        found = true;
    }
    found.then_some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grey_world_recovers_a_cast() {
        // A grey card scene under a warm light: camera RGB = grey * (1.6, 1.0, 0.55), plus
        // a skin-coloured patch that must not pull the estimate.
        let (w, h) = (200u32, 100u32);
        let mut px = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                let level = 8_000.0 + 20_000.0 * (x as f32 / w as f32);
                let (r, g, b) = if y < 30 && x < 80 {
                    // Skin: warmer than the illuminant.
                    (level * 1.6 * 1.45, level, level * 0.55 * 0.7)
                } else {
                    (level * 1.6, level, level * 0.55)
                };
                px.extend([r as u16, g as u16, b as u16]);
            }
        }
        // Start near the truth, as the camera's as-shot balance does.
        let m = grey_world_multipliers(&px, w, h, [1.0 / 1.4, 1.0, 1.0 / 0.62]).unwrap();
        assert!((m[0] - 1.0 / 1.6).abs() < 0.02, "{m:?}");
        assert!((m[2] - 1.0 / 0.55).abs() < 0.05, "{m:?}");
        // Nothing usable -> None.
        assert!(grey_world_multipliers(&[65_535; 30], 5, 2, [1.0; 3]).is_none());
    }

    /// End to end on a synthetic JPEG: a dark scene with a bright "face" patch.
    #[test]
    fn auto_tone_on_a_dark_frame_brightens_without_clipping_the_face() {
        use crate::develop::{DevelopConfig, SourceImage};
        let dir = tempfile::tempdir().unwrap();
        let (w, h) = (320u32, 240u32);
        let mut px = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                let face = (120..200).contains(&x) && (60..160).contains(&y);
                let (r, g, b) = if face {
                    // Warm skin, already bright: a naive brightening would clip it.
                    (200u8, 150u8, 120u8)
                } else {
                    let v = (20.0 + 50.0 * (x as f32 / w as f32)) as u8;
                    (v, v, (f32::from(v) * 1.1) as u8)
                };
                px.extend([r, g, b]);
            }
        }
        let jpeg = crate::raw::turbo::encode_rgb_444(&px, w, h, 95).unwrap();
        let path = dir.path().join("dark.jpg");
        std::fs::write(&path, jpeg).unwrap();
        let cache = DevelopCache::new(DevelopConfig::default());
        let src = SourceImage { id: 1, path, orientation: None };
        let base = ParametricAdjustments::defaults_for(crate::ipc::types::ImageFormat::Jpeg);
        let face = [NormRect { x: 120.0 / 320.0, y: 60.0 / 240.0, width: 80.0 / 320.0, height: 100.0 / 240.0 }];

        let all = auto_tone_with_faces(&cache, &src, &base, AdjustmentField::AUTO_TONE, Some(&face)).unwrap();
        let e = all.exposure.unwrap();
        assert!(e > 0.0 && e <= 4.0, "{all:?}");
        assert!((e * 20.0 - (e * 20.0).round()).abs() < 1e-4, "exposure in 0.05 steps: {e}");
        for v in [all.contrast, all.highlights, all.shadows, all.whites, all.blacks, all.vibrance, all.saturation] {
            let v = v.unwrap();
            assert_eq!(v, v.round());
            assert!((-100.0..=100.0).contains(&v));
        }
        let result = all.apply_to(&base);
        let clipped = skin_clipped_fraction(&cache, &src, &result, Some(&face)).unwrap().unwrap();
        assert!(clipped <= SKIN_CLIP_MAX, "skin clipped {clipped}");

        // One slider (Shift-double-click) = that slider of the full Auto; others untouched.
        let one = auto_tone_with_faces(&cache, &src, &base, &[AdjustmentField::Shadows], Some(&face)).unwrap();
        assert_eq!(one.shadows, all.shadows);
        assert!(one.exposure.is_none() && one.whites.is_none());
        // Deterministic.
        assert_eq!(auto_tone_with_faces(&cache, &src, &base, AdjustmentField::AUTO_TONE, Some(&face)).unwrap(), all);

        // Auto WB of a neutral-ish frame stays within the Auto range.
        let wb = auto_white_balance(&cache, &src, &base).unwrap();
        assert!(wb.temperature_k <= AUTO_WB_MAX_TEMP && (AUTO_WB_TINT.0..=AUTO_WB_TINT.1).contains(&wb.tint), "{wb:?}");
    }

    fn rendered(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> crate::develop::pipeline::RenderedImage {
        let rgb = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).flat_map(|(x, y)| f(x, y)).collect();
        crate::develop::pipeline::RenderedImage { width: w, height: h, rgb, histogram: Default::default() }
    }

    fn skin_share(img: &crate::develop::pipeline::RenderedImage) -> f32 {
        let s = Stats::from_render(img, None, None);
        let m = estimate_skin(img, &s);
        m.iter().filter(|&&v| v).count() as f32 / m.len() as f32
    }

    #[test]
    fn estimated_skin_needs_a_plausible_region() {
        let grey = [90u8, 92, 95];
        let skin = [205u8, 150, 120];
        // A compact skin patch (~8% of the frame) is skin.
        let face =
            rendered(240, 160, |x, y| if (100..150).contains(&x) && (40..100).contains(&y) { skin } else { grey });
        let share = skin_share(&face);
        assert!((0.06..0.1).contains(&share), "{share}");
        // Warm light over most of the frame (sand, walls): not skin.
        let warm = rendered(240, 160, |x, _| if x < 200 { skin } else { grey });
        assert_eq!(skin_share(&warm), 0.0);
        // Scattered skin-coloured pixels (texture): not skin.
        let speckle = rendered(240, 160, |x, y| if (x * 7 + y * 13) % 11 == 0 { skin } else { grey });
        assert_eq!(skin_share(&speckle), 0.0);
        // Already clipped warm highlights are not skin to protect.
        let blown =
            rendered(
                240,
                160,
                |x, y| {
                    if (100..150).contains(&x) && (40..100).contains(&y) {
                        [255, 236, 210]
                    } else {
                        grey
                    }
                },
            );
        assert_eq!(skin_share(&blown), 0.0);
    }

    /// Auto on a synthetic JPEG with analysis faces `Some(&[])` (analysed, no faces: no skin
    /// guard) and `None` (not analysed: estimated skin).
    fn auto_both(name: &str, f: impl Fn(u32, u32) -> [u8; 3]) -> (AutoToneValues, AutoToneValues) {
        use crate::develop::{DevelopConfig, SourceImage};
        let dir = tempfile::tempdir().unwrap();
        let (w, h) = (320u32, 240u32);
        let px: Vec<u8> = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).flat_map(|(x, y)| f(x, y)).collect();
        let path = dir.path().join(name);
        std::fs::write(&path, crate::raw::turbo::encode_rgb_444(&px, w, h, 95).unwrap()).unwrap();
        let cache = DevelopCache::new(DevelopConfig::default());
        let src = SourceImage { id: 1, path, orientation: None };
        let base = ParametricAdjustments::defaults_for(crate::ipc::types::ImageFormat::Jpeg);
        let analysed = auto_tone_with_faces(&cache, &src, &base, AdjustmentField::AUTO_TONE, Some(&[])).unwrap();
        let unknown = auto_tone_with_faces(&cache, &src, &base, AdjustmentField::AUTO_TONE, None).unwrap();
        (analysed, unknown)
    }

    /// Not analysed yet (no face boxes): Auto stays within [`ESTIMATED_GUARD`] of Auto on the
    /// same frame analysed without faces (the old skin-colour fallback took Whites to -60 and
    /// exposure down by up to 1.3 EV on warm, bright frames).
    #[test]
    fn auto_tone_without_faces_is_bounded() {
        let within = |a: &AutoToneValues, u: &AutoToneValues| {
            let (ea, eu) = (a.exposure.unwrap(), u.exposure.unwrap());
            assert!(eu >= ea - ESTIMATED_GUARD.exposure_drop - 1e-4, "exposure {ea} -> {eu}");
            let (wa, wu) = (a.whites.unwrap(), u.whites.unwrap());
            assert!(wu >= wa.min(ESTIMATED_GUARD.whites_floor), "whites {wa} -> {wu}");
            let (ha, hu) = (a.highlights.unwrap(), u.highlights.unwrap());
            assert!(hu >= ha - ESTIMATED_GUARD.highlights_drop, "highlights {ha} -> {hu}");
        };
        // Warm light over a quarter of a dark frame (a lit wall, sand) with the hue of skin,
        // partly clipped already: not a plausible face.
        let (a, u) = auto_both("wall.jpg", |x, y| {
            if x < 240 {
                let v = 25.0 + 35.0 * (y as f32 / 240.0);
                [v as u8, v as u8, (v * 1.05) as u8]
            } else {
                let v = 200.0 + 55.0 * (y as f32 / 240.0);
                [v as u8, (v * 0.82) as u8, (v * 0.62) as u8]
            }
        });
        within(&a, &u);
        // A compact, bright skin-coloured patch in a dark frame: protected, but bounded.
        let (a, u) = auto_both("face.jpg", |x, y| {
            if (130..190).contains(&x) && (70..150).contains(&y) {
                [236, 186, 150]
            } else {
                let v = 20 + (40 * x / 320) as u8;
                [v, v, v + 2]
            }
        });
        within(&a, &u);
        assert_ne!(a, u, "the guard protects the estimated skin");
    }

    #[test]
    fn percentiles_and_rounding() {
        assert_eq!(Stats::percentile_u8([1u8, 2, 3, 4].into_iter(), 0.5), 2);
        assert_eq!(Stats::percentile_u8(std::iter::empty(), 0.5), 0);
        assert_eq!(round_to(0.374, 0.05), 0.35);
        assert_eq!(round_to(-0.01, 0.05), 0.0);
        assert!((y_to_lstar(0.18) - 49.5).abs() < 0.2);
    }
}
