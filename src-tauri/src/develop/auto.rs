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
}

impl Stats {
    /// `faces`: `Some` = the analysis ran (skin = the face boxes; none = no skin), `None` =
    /// unknown (skin-coloured pixels stand in).
    fn from_render(img: &super::pipeline::RenderedImage, faces: Option<&[NormRect]>) -> Stats {
        let lin = lstar_lut();
        let (w, h) = (img.width as usize, img.height as usize);
        let n = w * h;
        let mut s = Stats {
            l: Vec::with_capacity(n),
            maxc: Vec::with_capacity(n),
            luma: Vec::with_capacity(n),
            chroma: Vec::with_capacity(n),
            skin: vec![false; n],
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
        let mut any_face = false;
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
                    any_face = true;
                }
            }
        }
        if faces.is_none() && !any_face {
            // Skin-coloured pixels: warm hue (a*, b* > 0, hue 20..75 deg), moderate chroma, mid L*.
            for (i, px) in img.rgb.as_chunks::<3>().0.iter().enumerate() {
                let (r, g, b) = (i32::from(px[0]), i32::from(px[1]), i32::from(px[2]));
                let l = s.l[i];
                s.skin[i] = r > g && g > b && r - b > 20 && r - g < 110 && (35.0..97.0).contains(&l) && {
                    let c = s.chroma[i];
                    (8.0..55.0).contains(&c)
                };
            }
            // Too few to matter (landscapes, products): no skin constraint.
            let count = s.skin.iter().filter(|&&v| v).count();
            if count < n / 100 {
                s.skin.iter_mut().for_each(|v| *v = false);
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

/// Renders `adj` for measurement: uncropped, without LUT, small.
struct Probe<'a> {
    cache: &'a DevelopCache,
    src: &'a SourceImage,
    luts: LutLibrary,
    faces: Option<&'a [NormRect]>,
    renders: u32,
}

impl Probe<'_> {
    fn measure(&mut self, adj: &ParametricAdjustments) -> AppResult<Stats> {
        let mut a = adj.clone();
        a.crop.enabled = false;
        a.lut = None;
        a.effects.grain.amount = 0.0;
        a.effects.vignette.amount = 0.0;
        self.renders += 1;
        let r = self.cache.render_image(self.src, &a, None, MEASURE_EDGE, &self.luts)?;
        Ok(Stats::from_render(&r.image, self.faces))
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
    let mut probe =
        Probe { cache, src, luts: LutLibrary::new(std::env::temp_dir().join("sieve-auto-no-luts")), faces, renders: 0 };
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

/// [`auto_tone_with_faces`] with explicit parameters.
pub fn auto_tone_opts(
    cache: &DevelopCache,
    src: &SourceImage,
    adjustments: &ParametricAdjustments,
    keys: &[AdjustmentField],
    faces: Option<&[NormRect]>,
    targets: AutoParams,
) -> AppResult<AutoToneValues> {
    let want = |k: AdjustmentField| keys.contains(&k);
    use AdjustmentField as F;
    // Stages 1-2 on the settings with every Auto slider at 0: a slider's Auto value does not
    // depend on which sliders were requested (Shift-double-click = that slider of full Auto).
    let mut probe =
        Probe { cache, src, luts: LutLibrary::new(std::env::temp_dir().join("sieve-auto-no-luts")), faces, renders: 0 };
    let mut work = adjustments.clone();
    (work.exposure, work.contrast, work.highlights, work.shadows) = (0.0, 0.0, 0.0, 0.0);
    (work.whites, work.blacks, work.vibrance, work.saturation) = (0.0, 0.0, 0.0, 0.0);
    let mut stats = probe.measure(&work)?;
    let has_faces = faces.is_some_and(|f| !f.is_empty()) && stats.has_skin();
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
    //    in the source).
    if want(F::Whites) || want(F::Exposure) || want(F::Highlights) {
        stats = probe.measure(&adj)?;
        let mut rounds = 0;
        while stats.has_skin() && stats.skin_clipped() > SKIN_CLIP_MAX && rounds < 20 {
            if want(F::Whites) && adj.whites > -60.0 {
                adj.whites = (adj.whites - 15.0).max(-60.0);
            } else if want(F::Highlights) && adj.highlights > -100.0 {
                adj.highlights = (adj.highlights - 20.0).max(-100.0);
            } else if want(F::Exposure) && adj.exposure > -4.0 {
                adj.exposure = round_to((adj.exposure - 0.1).max(-4.0), 0.05);
            } else {
                break;
            }
            stats = probe.measure(&adj)?;
            rounds += 1;
        }
    }

    let pick = |k: AdjustmentField, v: f32| want(k).then_some(v);
    Ok(AutoToneValues {
        exposure: pick(F::Exposure, round_to(adj.exposure.clamp(-5.0, 5.0), 0.05)),
        contrast: pick(F::Contrast, adj.contrast.clamp(-100.0, 100.0).round()),
        highlights: pick(F::Highlights, adj.highlights.clamp(-100.0, 100.0).round()),
        shadows: pick(F::Shadows, adj.shadows.clamp(-100.0, 100.0).round()),
        whites: pick(F::Whites, adj.whites.clamp(-100.0, 100.0).round()),
        blacks: pick(F::Blacks, adj.blacks.clamp(-100.0, 100.0).round()),
        vibrance: pick(F::Vibrance, adj.vibrance.clamp(-100.0, 100.0).round()),
        saturation: pick(F::Saturation, adj.saturation.clamp(-100.0, 100.0).round()),
    })
}

/// Share of skin pixels (face boxes, else skin-coloured) with a channel >= 250 in the render
/// of `adj` (uncropped, measurement size). For evaluations and tests.
pub fn skin_clipped_fraction(
    cache: &DevelopCache,
    src: &SourceImage,
    adj: &ParametricAdjustments,
    faces: Option<&[NormRect]>,
) -> AppResult<Option<f32>> {
    let mut probe =
        Probe { cache, src, luts: LutLibrary::new(std::env::temp_dir().join("sieve-auto-no-luts")), faces, renders: 0 };
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
pub fn grey_world_multipliers_opts(
    pixels: &[u16],
    width: u32,
    height: u32,
    start: [f32; 3],
    opts: WbOptions,
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
                let skin = r > 0.1 && b < -0.1 && r < 0.75 && b > -0.8 && (r - b) > 0.3;
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

    #[test]
    fn percentiles_and_rounding() {
        assert_eq!(Stats::percentile_u8([1u8, 2, 3, 4].into_iter(), 0.5), 2);
        assert_eq!(Stats::percentile_u8(std::iter::empty(), 0.5), 0);
        assert_eq!(round_to(0.374, 0.05), 0.35);
        assert_eq!(round_to(-0.01, 0.05), 0.0);
        assert!((y_to_lstar(0.18) - 49.5).abs() < 0.2);
    }
}
