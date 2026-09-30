//! Phase 7b Lightroom-parity stages for the shared develop pipeline (preview + export).
//! Wired into `develop::pipeline` in this order (see `profiles` docs for the camera profile
//! and look stages):
//!
//! 1. Geometry: [`crop_geometry`] decides the output frame (crop + straighten) before
//!    resampling; `RenderOptions.region` and output sizes refer to the cropped frame.
//! 2. Scene-referred, linear working space (linear ProPhoto): [`calibration_matrix`]
//!    (folded into camera -> working), white balance, [`denoise`] (on the demosaiced linear
//!    image, before tone), then the PV2012 tone model (`develop::tone`) and the local
//!    operators (clarity/texture/dehaze).
//! 3. Pointwise colour/tone chain (evaluated through a shaped 3D LUT per render): profile
//!    HueSatMap, global tone, profile/look tables, HSL / vibrance / saturation or
//!    B&W ([`gray_mix`]), [`shadow_tint`], base tone curve composed with the look's and the
//!    user's parametric + master point curves ([`CurveLuts`], hue-preserving "RGB tone"),
//!    per-channel point curves, [`color_grade`], look RGB table, output transform.
//! 4. Display-referred spatial stages on the encoded output: [`vignette`] (post-crop),
//!    [`grain`], [`sharpen`] (capture sharpening on luminance).
//!
//! Resolution independence: every spatial radius is given at full resolution and scaled by
//! `scale` = working px per full-res px (previews are downscaled; exports may be too), so a
//! 1024 px draft, the 2048 px preview and a full-res export look alike. Grain is seeded per
//! image and generated in cropped-frame full-resolution coordinates.
//!
//! Curve shapes (parametric regions, point-curve spline, domains) were fitted to Adobe
//! Camera Raw renders (`param_data.rs`, `tools/acr-oracle`): the point curve is the DNG SDK's
//! natural cubic spline on sRGB-encoded values, the parametric curve is applied first.
//!
//! Phase 7c forward-compatibility (masks): stages take their parameters from plain structs
//! so a per-pixel mask blend can override them later.

use rayon::prelude::*;

use super::param_data as pd;
use crate::ipc::types::{
    CameraCalibration, ColorGrading, ColorWheel, CropSettings, CurvePoint, Grain, HslChannels, NoiseReduction,
    ParametricCurve, PointCurves, PostCropVignette, Sharpening, ToneCurve, VignetteStyle,
};

/// Interleaved RGB f32 working image (linear or display-referred depending on the stage).
pub struct Working<'a> {
    pub width: usize,
    pub height: usize,
    pub rgb: &'a mut [f32],
}

/// Tone-curve lookup tables over 0..=1 (`CurveLuts::SIZE` entries each, linear interpolation
/// between entries). `None` = identity (skip).
#[derive(Debug, Clone, PartialEq)]
pub struct CurveLuts {
    /// Parametric curve composed with the master point curve (applied to R, G, B).
    pub master: Option<Vec<f32>>,
    pub red: Option<Vec<f32>>,
    pub green: Option<Vec<f32>>,
    pub blue: Option<Vec<f32>>,
}

impl CurveLuts {
    pub const SIZE: usize = 4096;

    pub fn identity() -> Self {
        CurveLuts { master: None, red: None, green: None, blue: None }
    }
}

/// Evaluates a curve LUT (clamped input).
#[inline]
pub fn eval_lut(lut: &[f32], x: f32) -> f32 {
    let n = lut.len() - 1;
    let t = x.clamp(0.0, 1.0) * n as f32;
    let i = (t as usize).min(n - 1);
    let f = t - i as f32;
    lut[i] + (lut[i + 1] - lut[i]) * f
}

fn build_lut(f: impl Fn(f32) -> f32) -> Vec<f32> {
    let n = CurveLuts::SIZE;
    (0..n).map(|i| f(i as f32 / (n - 1) as f32).clamp(0.0, 1.0)).collect()
}

fn compose(a: Option<Vec<f32>>, b: Option<Vec<f32>>) -> Option<Vec<f32>> {
    match (a, b) {
        (None, b) => b,
        (a, None) => a,
        (Some(a), Some(b)) => Some(a.iter().map(|&x| eval_lut(&b, x)).collect()),
    }
}

fn lerp_identity(lut: Option<Vec<f32>>, amount: f32) -> Option<Vec<f32>> {
    let lut = lut?;
    if amount <= 0.0 {
        return None;
    }
    if (amount - 1.0).abs() < 1e-6 {
        return Some(lut);
    }
    let n = (lut.len() - 1) as f32;
    Some(lut.iter().enumerate().map(|(i, &v)| (i as f32 / n + (v - i as f32 / n) * amount).clamp(0.0, 1.0)).collect())
}

// ---------------------------------------------------------------------------
// Parametric curve
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Region {
    Shadows,
    Darks,
    Lights,
    Highlights,
}

/// Region-local coordinate (split-warped) of `t`; `None` outside the region.
fn warp(r: Region, t: f32, s1: f32, s2: f32, s3: f32) -> Option<f32> {
    let t = t.clamp(1e-6, 1.0 - 1e-6);
    match r {
        Region::Shadows => {
            let g = 0.5f32.ln() / (s1 / s2).ln();
            (t < s2).then(|| (t / s2).powf(g))
        }
        Region::Highlights => {
            let g = 0.5f32.ln() / ((s3 - s2) / (1.0 - s2)).ln();
            (t > s2).then(|| ((t - s2) / (1.0 - s2)).powf(g))
        }
        Region::Darks | Region::Lights => {
            let g = 0.5f32.ln() / s2.ln();
            Some(t.powf(g))
        }
    }
}

fn unwarp(r: Region, u: f32, s1: f32, s2: f32, s3: f32) -> f32 {
    let u = u.clamp(0.0, 1.0);
    match r {
        Region::Shadows => {
            let g = 0.5f32.ln() / (s1 / s2).ln();
            s2 * u.powf(1.0 / g)
        }
        Region::Highlights => {
            let g = 0.5f32.ln() / ((s3 - s2) / (1.0 - s2)).ln();
            s2 + (1.0 - s2) * u.powf(1.0 / g)
        }
        Region::Darks | Region::Lights => {
            let g = 0.5f32.ln() / s2.ln();
            u.powf(1.0 / g)
        }
    }
}

fn shape(r: Region, positive: bool) -> (&'static [f32], &'static [f32]) {
    match (r, positive) {
        (Region::Shadows, true) => (&pd::SHADOWS_POS_50, &pd::SHADOWS_POS_100),
        (Region::Shadows, false) => (&pd::SHADOWS_NEG_50, &pd::SHADOWS_NEG_100),
        (Region::Darks, true) => (&pd::DARKS_POS_50, &pd::DARKS_POS_100),
        (Region::Darks, false) => (&pd::DARKS_NEG_50, &pd::DARKS_NEG_100),
        (Region::Lights, true) => (&pd::LIGHTS_POS_50, &pd::LIGHTS_POS_100),
        (Region::Lights, false) => (&pd::LIGHTS_NEG_50, &pd::LIGHTS_NEG_100),
        (Region::Highlights, true) => (&pd::HIGHLIGHTS_POS_50, &pd::HIGHLIGHTS_POS_100),
        (Region::Highlights, false) => (&pd::HIGHLIGHTS_NEG_50, &pd::HIGHLIGHTS_NEG_100),
    }
}

/// Lightroom's parametric region curve (regions split at the three splits, each region
/// amount bends that part of the curve; 0 everywhere = identity), as a LUT.
/// Model (fitted to Camera Raw): each region slider displaces the value in the region's
/// split-warped coordinate by a measured shape, regions applied in order Shadows, Darks,
/// Lights, Highlights; the result is clamped and made monotone.
pub fn parametric_curve_lut(curve: &ParametricCurve) -> Option<Vec<f32>> {
    let regions = [
        (Region::Shadows, curve.shadows),
        (Region::Darks, curve.darks),
        (Region::Lights, curve.lights),
        (Region::Highlights, curve.highlights),
    ];
    if regions.iter().all(|(_, a)| *a == 0.0) {
        return None;
    }
    let mut s = [curve.shadow_split, curve.midtone_split, curve.highlight_split].map(|v| (v / 100.0).clamp(0.01, 0.99));
    if !(s[0] < s[1] && s[1] < s[2]) {
        s = [0.25, 0.5, 0.75];
    }
    let [s1, s2, s3] = s;
    let n = pd::PARAM_N as f32;
    let at = |tab: &[f32], u: f32| {
        let t = u.clamp(0.0, 1.0) * n;
        let i = (t as usize).min(pd::PARAM_N - 1);
        tab[i] + (tab[i + 1] - tab[i]) * (t - i as f32)
    };
    let mut lut = build_lut(|t| {
        let mut out = t;
        for &(r, a) in &regions {
            let a = (a / 100.0).clamp(-1.0, 1.0);
            if a == 0.0 {
                continue;
            }
            let Some(u) = warp(r, out, s1, s2, s3) else { continue };
            let (s50, s100) = shape(r, a > 0.0);
            let m = a.abs();
            let d = if m <= 0.5 {
                2.0 * m * at(s50, u)
            } else {
                let f = (m - 0.5) / 0.5;
                at(s50, u) * (1.0 - f) + at(s100, u) * f
            };
            out = unwarp(r, u + d, s1, s2, s3);
        }
        out
    });
    let mut hi = 0.0f32;
    for v in lut.iter_mut() {
        hi = hi.max(*v);
        *v = hi;
    }
    Some(lut)
}

// ---------------------------------------------------------------------------
// Point curves: natural cubic spline (DNG SDK `dng_spline_solver`).
// ---------------------------------------------------------------------------

/// Natural cubic spline through `(x, y)` (x strictly increasing), constant beyond the ends.
pub struct Spline {
    x: Vec<f64>,
    y: Vec<f64>,
    m: Vec<f64>,
}

impl Spline {
    pub fn new(points: &[(f64, f64)]) -> Spline {
        let x: Vec<f64> = points.iter().map(|p| p.0).collect();
        let y: Vec<f64> = points.iter().map(|p| p.1).collect();
        let n = x.len();
        let mut m = vec![0.0; n];
        if n > 2 {
            // Tridiagonal system for the second derivatives, natural end conditions.
            let h: Vec<f64> = x.windows(2).map(|w| w[1] - w[0]).collect();
            let mut a = vec![0.0; n];
            let mut b = vec![1.0; n];
            let mut c = vec![0.0; n];
            let mut r = vec![0.0; n];
            for i in 1..n - 1 {
                a[i] = h[i - 1];
                b[i] = 2.0 * (h[i - 1] + h[i]);
                c[i] = h[i];
                r[i] = 6.0 * ((y[i + 1] - y[i]) / h[i] - (y[i] - y[i - 1]) / h[i - 1]);
            }
            // Thomas algorithm.
            for i in 1..n {
                let w = a[i] / b[i - 1];
                b[i] -= w * c[i - 1];
                r[i] -= w * r[i - 1];
            }
            m[n - 1] = r[n - 1] / b[n - 1];
            for i in (0..n - 1).rev() {
                m[i] = (r[i] - c[i] * m[i + 1]) / b[i];
            }
        }
        Spline { x, y, m }
    }

    pub fn eval(&self, v: f64) -> f64 {
        let n = self.x.len();
        if v <= self.x[0] {
            return self.y[0];
        }
        if v >= self.x[n - 1] {
            return self.y[n - 1];
        }
        let i = self.x.partition_point(|&x| x <= v).clamp(1, n - 1) - 1;
        let h = self.x[i + 1] - self.x[i];
        let t = v - self.x[i];
        let a = (self.m[i + 1] - self.m[i]) / (6.0 * h);
        let b = self.m[i] / 2.0;
        let c = (self.y[i + 1] - self.y[i]) / h - h * (2.0 * self.m[i] + self.m[i + 1]) / 6.0;
        self.y[i] + c * t + b * t * t + a * t * t * t
    }
}

/// Point curve through `points` (0..=255 axes) with Lightroom's interpolation (natural
/// cubic spline as in the DNG SDK; constant beyond the end points), as a LUT over 0..=1.
pub fn point_curve_lut(points: &[CurvePoint]) -> Option<Vec<f32>> {
    if points.len() < 2 || PointCurves::is_identity(points) {
        return None;
    }
    let pts: Vec<(f64, f64)> = points.iter().map(|p| (f64::from(p[0]) / 255.0, f64::from(p[1]) / 255.0)).collect();
    if pts.windows(2).any(|w| w[1].0 <= w[0].0) {
        return None;
    }
    let s = Spline::new(&pts);
    Some(build_lut(|t| s.eval(f64::from(t)) as f32))
}

/// All tone-curve LUTs for `curve`, plus the look's own point curves (`look`, applied
/// before the user's curve at the look amount) when the look is resolved.
pub fn curve_luts(curve: &ToneCurve, look: Option<(&PointCurves, f32)>) -> CurveLuts {
    let look_curve = |pick: fn(&PointCurves) -> &Vec<CurvePoint>| {
        look.and_then(|(p, amount)| lerp_identity(point_curve_lut(pick(p)), amount))
    };
    let master = compose(
        compose(look_curve(|p| &p.master), parametric_curve_lut(&curve.parametric)),
        point_curve_lut(&curve.point.master),
    );
    CurveLuts {
        master,
        red: compose(look_curve(|p| &p.red), point_curve_lut(&curve.point.red)),
        green: compose(look_curve(|p| &p.green), point_curve_lut(&curve.point.green)),
        blue: compose(look_curve(|p| &p.blue), point_curve_lut(&curve.point.blue)),
    }
}

// ---------------------------------------------------------------------------
// Colour: calibration, shadow tint, grading, B&W
// ---------------------------------------------------------------------------

/// Luminance weights of linear ProPhoto (Y row of ProPhoto -> XYZ D50).
pub const PROPHOTO_Y: [f32; 3] = [0.288_040_2, 0.711_874_1, 0.000_085_7];

#[inline]
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Rotates `c` (a chroma vector, sum 0) around the grey axis by `theta` radians.
#[inline]
fn rotate_chroma(c: [f32; 3], theta: f32) -> [f32; 3] {
    let k = 0.577_350_3;
    let (s, co) = theta.sin_cos();
    let cross = [(c[2] - c[1]) * k, (c[0] - c[2]) * k, (c[1] - c[0]) * k];
    [c[0] * co + cross[0] * s, c[1] * co + cross[1] * s, c[2] * co + cross[2] * s]
}

/// Camera-calibration primaries adjustment as a 3x3 matrix to fold into camera -> working
/// (hue rotates each primary, saturation scales its chroma; identity for all zeros).
/// Working space: linear ProPhoto. Each primary's chroma (relative to the grey (1,1,1)/3)
/// is rotated around the grey axis (hue, towards the next primary for +) and scaled
/// (saturation); rows are renormalized so white stays white. Gains fitted to Camera Raw
/// renders of colour patches (`tools/acr-oracle`, effect-size ratio ~1.0 per slider).
pub fn calibration_matrix(cal: &CameraCalibration) -> [[f32; 3]; 3] {
    let prim = [cal.red, cal.green, cal.blue];
    if prim.iter().all(|p| p.hue == 0.0 && p.saturation == 0.0) {
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    }
    let mut cols = [[0.0f32; 3]; 3];
    for (i, p) in prim.iter().enumerate() {
        let mut e = [0.0f32; 3];
        e[i] = 1.0;
        let y = 1.0 / 3.0;
        let chroma = [e[0] - y, e[1] - y, e[2] - y];
        let theta = (p.hue / 100.0).clamp(-1.0, 1.0) * 30f32.to_radians() * CAL_HUE_GAIN;
        let c = rotate_chroma(chroma, theta);
        let s = 1.0 + (p.saturation / 100.0).clamp(-1.0, 1.0) * CAL_SAT_GAIN;
        cols[i] = [y + c[0] * s, y + c[1] * s, y + c[2] * s];
    }
    let mut m = [[0.0f32; 3]; 3];
    for r in 0..3 {
        for c in 0..3 {
            m[r][c] = cols[c][r];
        }
        let sum: f32 = m[r].iter().sum();
        if sum.abs() > 1e-6 {
            m[r].iter_mut().for_each(|v| *v /= sum);
        }
    }
    m
}

/// Calibration hue / saturation strength (fitted, see `tools/acr-oracle`).
pub const CAL_HUE_GAIN: f32 = 1.12;
pub const CAL_SAT_GAIN: f32 = 1.25;

/// Shadow tint (green -/magenta +) weighted towards the shadows. `rgb` linear working.
pub fn shadow_tint(rgb: [f32; 3], tint: f32) -> [f32; 3] {
    if tint == 0.0 {
        return rgb;
    }
    let y = dot(PROPHOTO_Y, rgb).max(0.0);
    // Weight: 1 in deep shadows, fading out by ~ -2 EV below the clip.
    let w = 1.0 - smoothstep(-8.0, -2.0, (y + 1e-6).log2());
    let t = (tint / 100.0).clamp(-1.0, 1.0) * 0.12 * w;
    // Magenta = +R +B -G; keep luminance.
    let out = [rgb[0] * (1.0 + t), rgb[1] * (1.0 - t), rgb[2] * (1.0 + t)];
    let y2 = dot(PROPHOTO_Y, out);
    if y2 > 1e-9 {
        out.map(|c| c * y / y2)
    } else {
        out
    }
}

#[inline]
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Unit chroma direction (encoded RGB offsets with zero mean) of a hue in degrees.
fn hue_direction(hue: f32) -> [f32; 3] {
    let rgb = crate::profiles::dcp::hsv_to_rgb((hue.rem_euclid(360.0)) / 60.0, 1.0, 1.0);
    let m = (rgb[0] + rgb[1] + rgb[2]) / 3.0;
    let c = [rgb[0] - m, rgb[1] - m, rgb[2] - m];
    let n = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt().max(1e-6);
    c.map(|v| v / n)
}

/// Strength of a colour wheel at saturation 100 (encoded RGB offset magnitude).
pub const GRADE_GAIN: f32 = 0.30;

/// Precomputed colour-grading parameters.
pub struct Grade {
    wheels: [([f32; 3], f32); 4],
    lum: [f32; 4],
    pivot: f32,
    width: f32,
    any: bool,
}

impl Grade {
    pub fn new(g: &ColorGrading) -> Self {
        let wheel = |w: &ColorWheel, gain: f32| (hue_direction(w.hue), (w.saturation / 100.0).clamp(0.0, 1.0) * gain);
        let (sh, mid) = (GRADE_GAIN * 1.85, GRADE_GAIN * 0.47);
        let wheels = [wheel(&g.shadows, sh), wheel(&g.midtones, mid), wheel(&g.highlights, sh), wheel(&g.global, mid)];
        let lum = [g.shadows.luminance, g.midtones.luminance, g.highlights.luminance, g.global.luminance]
            .map(|v| (v / 100.0).clamp(-1.0, 1.0));
        let any = wheels.iter().any(|w| w.1 > 0.0) || lum.iter().any(|v| *v != 0.0);
        // Balance moves the shadow/highlight crossover (positive favours highlights).
        let pivot = 0.5 - (g.balance / 100.0).clamp(-1.0, 1.0) * 0.3;
        let width = 0.08 + (g.blending / 100.0).clamp(0.0, 1.0) * 0.42;
        Grade { wheels, lum, pivot, width, any }
    }

    pub fn is_identity(&self) -> bool {
        !self.any
    }

    /// Region weights (shadows, midtones, highlights) for encoded luminance `l`.
    #[inline]
    fn weights(&self, l: f32) -> [f32; 3] {
        let hi = smoothstep(self.pivot - self.width, self.pivot + self.width, l);
        let sh = 1.0 - hi;
        let d = (l - self.pivot) / (self.width + 0.2);
        let mid = (1.0 - d * d).max(0.0);
        [sh * (1.0 - 0.5 * mid), mid, hi * (1.0 - 0.5 * mid)]
    }

    /// Applies grading to encoded RGB (0..=1).
    #[inline]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        if !self.any {
            return rgb;
        }
        let l = (rgb[0] * 0.2627 + rgb[1] * 0.6780 + rgb[2] * 0.0593).clamp(0.0, 1.0);
        let w = self.weights(l);
        let weights = [w[0], w[1], w[2], 1.0];
        let mut off = [0.0f32; 3];
        let mut dl = 0.0;
        for ((&(dir, s), &wk), &lum) in self.wheels.iter().zip(&weights).zip(&self.lum) {
            let a = s * wk;
            off[0] += dir[0] * a;
            off[1] += dir[1] * a;
            off[2] += dir[2] * a;
            dl += lum * wk;
        }
        // Tints scale with the pixel's brightness away from black and white.
        let room = (l * (1.0 - l) * 4.0).clamp(0.0, 1.0).sqrt();
        let mut out = [rgb[0] + off[0] * room, rgb[1] + off[1] * room, rgb[2] + off[2] * room];
        // Keep luminance, then apply the luminance sliders.
        let l2 = out[0] * 0.2627 + out[1] * 0.6780 + out[2] * 0.0593;
        let target = (l + dl * 0.125 * (1.0 - (2.0 * l - 1.0).powi(2)).max(0.0)).clamp(0.0, 1.0);
        let shift = target - l2;
        out = out.map(|c| c + shift);
        out
    }
}

/// Color grading per pixel on display-referred values: shadow/midtone/highlight wheels
/// weighted by luminance ranges (overlap from `blending`, pivot from `balance`), then global.
pub fn color_grade(rgb: [f32; 3], grading: &ColorGrading) -> [f32; 3] {
    Grade::new(grading).apply(rgb)
}

/// Oklab (Ottosson 2020) over linear ProPhoto, with Lightroom's eight colour bands placed at
/// the Oklab hues of the sRGB colours they are named after (red, orange, yellow, green,
/// aqua, blue, purple, magenta = HSV hues 0, 30, 60, 120, 180, 240, 270, 300).
pub struct Oklab {
    to_lms: [[f32; 3]; 3],
    from_lms: [[f32; 3]; 3],
    to_lab: [[f32; 3]; 3],
    from_lab: [[f32; 3]; 3],
    /// Band centres (Oklab hue degrees), increasing; wraps from the last to the first.
    pub centers: [f32; 8],
}

#[inline]
fn m3(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

impl Oklab {
    pub fn get() -> &'static Oklab {
        static O: std::sync::OnceLock<Oklab> = std::sync::OnceLock::new();
        O.get_or_init(Oklab::new)
    }

    fn new() -> Oklab {
        use crate::profiles::dcp::{bradford, invert3, mul_mm, xy_to_xyz, D50_XY};
        const M1: [[f64; 3]; 3] = [
            [0.412_221_470_8, 0.536_332_536_3, 0.051_445_992_9],
            [0.211_903_498_2, 0.680_699_545_1, 0.107_396_956_6],
            [0.088_302_461_9, 0.281_718_837_6, 0.629_978_700_5],
        ];
        const M2: [[f64; 3]; 3] = [
            [0.210_454_255_3, 0.793_617_785_0, -0.004_072_046_8],
            [1.977_998_495_1, -2.428_592_205_0, 0.450_593_709_9],
            [0.025_904_037_1, 0.782_771_766_2, -0.808_675_766_0],
        ];
        const XYZ_TO_SRGB: [[f64; 3]; 3] = [
            [3.240_454_2, -1.537_138_5, -0.498_531_4],
            [-0.969_266_0, 1.876_010_8, 0.041_556_0],
            [0.055_643_4, -0.204_025_9, 1.057_225_2],
        ];
        let d50_d65 = bradford(xy_to_xyz(D50_XY), xy_to_xyz((0.3127, 0.3290)));
        let pp_to_srgb = mul_mm(&XYZ_TO_SRGB, &mul_mm(&d50_d65, &crate::develop::camera::PROPHOTO_TO_XYZ));
        let to_lms = mul_mm(&M1, &pp_to_srgb);
        let from_lms = invert3(&to_lms).expect("invertible");
        let from_lab = invert3(&M2).expect("invertible");
        let f = |m: [[f64; 3]; 3]| m.map(|r| r.map(|v| v as f32));
        let mut ok =
            Oklab { to_lms: f(to_lms), from_lms: f(from_lms), to_lab: f(M2), from_lab: f(from_lab), centers: [0.0; 8] };
        let srgb_lin_to_pp = invert3(&pp_to_srgb).expect("invertible");
        for (k, h) in [0.0f32, 30.0, 60.0, 120.0, 180.0, 240.0, 270.0, 300.0].iter().enumerate() {
            let e = crate::profiles::dcp::hsv_to_rgb(h / 60.0, 1.0, 1.0);
            let lin = e.map(|c| f64::from(crate::develop::pipeline::srgb_decode(c)));
            let pp = crate::profiles::dcp::mul_mv(&srgb_lin_to_pp, lin).map(|v| v as f32);
            let [_, a, b] = ok.from_prophoto(pp);
            ok.centers[k] = b.atan2(a).to_degrees().rem_euclid(360.0);
        }
        // Keep increasing order (red may sit just below 360 in exotic cases).
        for k in 1..8 {
            while ok.centers[k] < ok.centers[k - 1] {
                ok.centers[k] += 360.0;
            }
        }
        ok
    }

    #[inline]
    pub fn from_prophoto(&self, v: [f32; 3]) -> [f32; 3] {
        let lms = m3(&self.to_lms, v).map(f32::cbrt);
        m3(&self.to_lab, lms)
    }

    #[inline]
    pub fn to_prophoto(&self, lab: [f32; 3]) -> [f32; 3] {
        let lms = m3(&self.from_lab, lab).map(|c| c * c * c);
        m3(&self.from_lms, lms)
    }

    /// Adjacent bands (i, j) and the weight of j for an Oklab hue (degrees).
    #[inline]
    pub fn bands(&self, hue: f32) -> (usize, usize, f32) {
        let c = &self.centers;
        let mut h = hue;
        while h < c[0] {
            h += 360.0;
        }
        while h >= c[0] + 360.0 {
            h -= 360.0;
        }
        let mut i = 7;
        for k in 0..7 {
            if h >= c[k] && h < c[k + 1] {
                i = k;
                break;
            }
        }
        let (lo, hi) = if i == 7 { (c[7], c[0] + 360.0) } else { (c[i], c[i + 1]) };
        let t = ((h - lo) / (hi - lo)).clamp(0.0, 1.0);
        (i, (i + 1) % 8, t * t * (3.0 - 2.0 * t))
    }

    /// Weight of band `k` at `hue`.
    pub fn band_weight(&self, hue: f32, k: usize) -> f32 {
        let (i, j, t) = self.bands(hue);
        if i == k {
            1.0 - t
        } else if j == k {
            t
        } else {
            0.0
        }
    }
}

/// HSL band centres (degrees of sRGB-encoded HSV hue), Lightroom order.
pub const BAND_CENTERS: [f32; 9] = [0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 270.0, 300.0, 360.0];

/// Smooth partition-of-unity weights: (band a, band b, weight of b).
#[inline]
pub fn band_weights(h: f32) -> (usize, usize, f32) {
    let h = h.rem_euclid(360.0);
    let mut i = 0;
    while i < 7 && h >= BAND_CENTERS[i + 1] {
        i += 1;
    }
    let t = (h - BAND_CENTERS[i]) / (BAND_CENTERS[i + 1] - BAND_CENTERS[i]);
    let s = t.clamp(0.0, 1.0);
    (i, (i + 1) % 8, s * s * (3.0 - 2.0 * s))
}

pub fn bands(c: &HslChannels) -> [f32; 8] {
    [c.red, c.orange, c.yellow, c.green, c.aqua, c.blue, c.purple, c.magenta]
}

/// Monochrome value from the B&W mixer (Lightroom's 8-band gray mix; all zeros = neutral
/// luminance conversion). `rgb` is linear ProPhoto; the result is linear.
pub fn gray_mix(rgb: [f32; 3], mixer: &HslChannels) -> f32 {
    let y = dot(PROPHOTO_Y, rgb).max(0.0);
    let mix = bands(mixer);
    if mix.iter().all(|v| *v == 0.0) {
        return y;
    }
    let lab = Oklab::get();
    let [_, a, b] = lab.from_prophoto(rgb);
    let c = (a * a + b * b).sqrt();
    let s = (c / 0.28).min(1.0);
    let (i, j, t) = lab.bands(b.atan2(a).to_degrees());
    let m = (mix[i] + (mix[j] - mix[i]) * t) / 100.0;
    y * (m * 1.5 * s).exp2()
}

// ---------------------------------------------------------------------------
// Box filters (parallel), shared by NR and sharpening.
// ---------------------------------------------------------------------------

/// 2D box mean with radius `r` (edge-clamped windows), rows and column strips in parallel.
pub fn box2d(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    if r == 0 {
        return src.to_vec();
    }
    // Horizontal running sums.
    let mut tmp = vec![0.0f32; w * h];
    tmp.par_chunks_mut(w).zip(src.par_chunks(w)).for_each(|(out, row)| {
        let mut acc = 0.0f32;
        let mut cnt = 0usize;
        for v in row.iter().take(r.min(w)) {
            acc += *v;
            cnt += 1;
        }
        for x in 0..w {
            if x + r < w {
                acc += row[x + r];
                cnt += 1;
            }
            if x > r {
                acc -= row[x - r - 1];
                cnt -= 1;
            }
            out[x] = acc / cnt as f32;
        }
    });
    // Vertical sliding sums over blocks of rows (each block primes its window from `tmp`,
    // then slides; whole rows, written straight into the output).
    const BLOCK: usize = 32;
    let mut out = vec![0.0f32; w * h];
    let row = |y: usize| &tmp[y * w..(y + 1) * w];
    out.par_chunks_mut(w * BLOCK).enumerate().for_each(|(b, chunk)| {
        let y0 = b * BLOCK;
        let mut acc = vec![0.0f32; w];
        let lo = y0.saturating_sub(r);
        let hi = (y0 + r).min(h - 1);
        for y in lo..=hi {
            acc.iter_mut().zip(row(y)).for_each(|(a, v)| *a += *v);
        }
        for (k, o) in chunk.chunks_mut(w).enumerate() {
            let y = y0 + k;
            if k > 0 {
                if y + r < h {
                    acc.iter_mut().zip(row(y + r)).for_each(|(a, v)| *a += *v);
                }
                if y > r {
                    acc.iter_mut().zip(row(y - r - 1)).for_each(|(a, v)| *a -= *v);
                }
            }
            let cnt = (y + r).min(h - 1) + 1 - y.saturating_sub(r);
            let inv = 1.0 / cnt as f32;
            o.iter_mut().zip(&acc).for_each(|(o, a)| *o = a * inv);
        }
    });
    out
}

/// [`box2d`] of `C` interleaved planes at once (one pair of passes instead of `C`).
pub fn box2d_multi<const C: usize>(src: &[[f32; C]], w: usize, h: usize, r: usize) -> Vec<[f32; C]> {
    if r == 0 {
        return src.to_vec();
    }
    let add = |a: &mut [f32; C], v: &[f32; C]| (0..C).for_each(|c| a[c] += v[c]);
    let sub = |a: &mut [f32; C], v: &[f32; C]| (0..C).for_each(|c| a[c] -= v[c]);
    let mut tmp = vec![[0.0f32; C]; w * h];
    tmp.par_chunks_mut(w).zip(src.par_chunks(w)).for_each(|(out, row)| {
        let mut acc = [0.0f32; C];
        let mut cnt = 0usize;
        for v in row.iter().take(r.min(w)) {
            add(&mut acc, v);
            cnt += 1;
        }
        for x in 0..w {
            if x + r < w {
                add(&mut acc, &row[x + r]);
                cnt += 1;
            }
            if x > r {
                sub(&mut acc, &row[x - r - 1]);
                cnt -= 1;
            }
            out[x] = acc.map(|a| a / cnt as f32);
        }
    });
    const BLOCK: usize = 32;
    let mut out = vec![[0.0f32; C]; w * h];
    let row = |y: usize| &tmp[y * w..(y + 1) * w];
    out.par_chunks_mut(w * BLOCK).enumerate().for_each(|(b, chunk)| {
        let y0 = b * BLOCK;
        let mut acc = vec![[0.0f32; C]; w];
        for y in y0.saturating_sub(r)..=(y0 + r).min(h - 1) {
            acc.iter_mut().zip(row(y)).for_each(|(a, v)| add(a, v));
        }
        for (k, o) in chunk.chunks_mut(w).enumerate() {
            let y = y0 + k;
            if k > 0 {
                if y + r < h {
                    acc.iter_mut().zip(row(y + r)).for_each(|(a, v)| add(a, v));
                }
                if y > r {
                    acc.iter_mut().zip(row(y - r - 1)).for_each(|(a, v)| sub(a, v));
                }
            }
            let cnt = (y + r).min(h - 1) + 1 - y.saturating_sub(r);
            let inv = 1.0 / cnt as f32;
            o.iter_mut().zip(&acc).for_each(|(o, a)| *o = a.map(|v| v * inv));
        }
    });
    out
}

/// Gaussian-like blur: three box passes with radius from `sigma`.
pub fn blur(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    // Three boxes of radius r have variance 3 * r(r+1)/3 = r(r+1).
    let r = ((sigma * sigma + 0.25).sqrt() - 0.5).round().max(0.0) as usize;
    if r == 0 {
        return small_blur(src, w, h, sigma);
    }
    let a = box2d(src, w, h, r);
    let b = box2d(&a, w, h, r);
    box2d(&b, w, h, r)
}

/// 3x3 gaussian for sub-pixel sigmas.
fn small_blur(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    if sigma <= 0.05 {
        return src.to_vec();
    }
    let k1 = (-1.0 / (2.0 * sigma * sigma)).exp();
    let norm = 1.0 + 2.0 * k1;
    let (c, s) = (1.0 / norm, k1 / norm);
    let mut tmp = vec![0.0f32; w * h];
    tmp.par_chunks_mut(w).zip(src.par_chunks(w)).for_each(|(o, row)| {
        for x in 0..w {
            let l = row[x.saturating_sub(1)];
            let r = row[(x + 1).min(w - 1)];
            o[x] = c * row[x] + s * (l + r);
        }
    });
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, o)| {
        let up = &tmp[y.saturating_sub(1) * w..][..w];
        let mid = &tmp[y * w..][..w];
        let dn = &tmp[(y + 1).min(h - 1) * w..][..w];
        for x in 0..w {
            o[x] = c * mid[x] + s * (up[x] + dn[x]);
        }
    });
    out
}

/// [`guided`] of two planes `p1`, `p2` by the same guide `i` (cross-guided), sharing the
/// guide's statistics; results equal two `guided` calls exactly.
pub fn guided_pair(i: &[f32], p1: &[f32], p2: &[f32], w: usize, h: usize, r: usize, eps: f32) -> (Vec<f32>, Vec<f32>) {
    let mean_i = box2d(i, w, h, r);
    let var_i: Vec<f32> = {
        let ii: Vec<f32> = i.par_iter().map(|a| a * a).collect();
        let mean_ii = box2d(&ii, w, h, r);
        mean_ii.par_iter().zip(&mean_i).map(|(m2, m)| (m2 - m * m).max(0.0)).collect()
    };
    let one = |p: &[f32]| -> Vec<f32> {
        let mean_p = box2d(p, w, h, r);
        let mut mean_ip = {
            let ip: Vec<f32> = i.par_iter().zip(p).map(|(a, b)| a * b).collect();
            box2d(&ip, w, h, r)
        };
        // a -> mean_ip's buffer, b -> mean_p's.
        let mut b = mean_p;
        mean_ip.par_iter_mut().zip(b.par_iter_mut()).enumerate().for_each(|(k, (a, bb))| {
            let cov = *a - mean_i[k] * *bb;
            let av = cov / (var_i[k] + eps);
            *bb -= av * mean_i[k];
            *a = av;
        });
        let ma = box2d(&mean_ip, w, h, r);
        drop(mean_ip);
        let mb = box2d(&b, w, h, r);
        b.par_iter_mut().enumerate().for_each(|(k, q)| *q = ma[k] * i[k] + mb[k]);
        b
    };
    let (q1, q2) = rayon::join(|| one(p1), || one(p2));
    (q1, q2)
}

/// Block means of `src` (w x h) over `s` x `s` blocks (edge blocks partial).
fn downsample(src: &[f32], w: usize, h: usize, s: usize) -> (Vec<f32>, usize, usize) {
    let (dw, dh) = (w.div_ceil(s), h.div_ceil(s));
    let mut out = vec![0.0f32; dw * dh];
    out.par_chunks_mut(dw).enumerate().for_each(|(y, row)| {
        let (y0, y1) = (y * s, ((y + 1) * s).min(h));
        for (x, o) in row.iter_mut().enumerate() {
            let (x0, x1) = (x * s, ((x + 1) * s).min(w));
            let mut acc = 0.0f32;
            for yy in y0..y1 {
                acc += src[yy * w + x0..yy * w + x1].iter().sum::<f32>();
            }
            *o = acc / ((y1 - y0) * (x1 - x0)) as f32;
        }
    });
    (out, dw, dh)
}

/// Linear coefficients of the cross-guided filter of `p1` and `p2` by `i` (all `w` x `h`),
/// box radius `r`, already box-averaged, interleaved `[a1, b1, a2, b2]`: q = a * guide + b.
fn guided_coeffs_pair(i: &[f32], p1: &[f32], p2: &[f32], w: usize, h: usize, r: usize, eps: f32) -> Vec<[f32; 4]> {
    let raw: Vec<[f32; 6]> = (0..w * h)
        .into_par_iter()
        .map(|k| {
            let (g, a, b) = (i[k], p1[k], p2[k]);
            [g, g * g, a, g * a, b, g * b]
        })
        .collect();
    let st = box2d_multi(&raw, w, h, r);
    drop(raw);
    let ab: Vec<[f32; 4]> = st
        .par_iter()
        .map(|m| {
            let var = (m[1] - m[0] * m[0]).max(0.0);
            let a1 = (m[3] - m[0] * m[2]) / (var + eps);
            let a2 = (m[5] - m[0] * m[4]) / (var + eps);
            [a1, m[2] - a1 * m[0], a2, m[4] - a2 * m[0]]
        })
        .collect();
    drop(st);
    box2d_multi(&ab, w, h, r)
}

/// Bilinear upsampling taps of a coarse grid (factor `s`) for one axis of length `n`.
fn up_taps(n: usize, coarse: usize, s: usize) -> Vec<(usize, usize, f32)> {
    (0..n)
        .map(|x| {
            let g = ((x as f32 + 0.5) / s as f32 - 0.5).clamp(0.0, (coarse - 1) as f32);
            let x0 = g as usize;
            (x0, (x0 + 1).min(coarse - 1), g - x0 as f32)
        })
        .collect()
}

/// Fast guided filter (He & Sun 2015) of two planes by one guide: the linear coefficients
/// are computed on `s` x `s` block means (box radius `r / s`) and bilinearly upsampled,
/// then applied to the full-resolution guide. `s = 1` = [`guided_pair`].
#[allow(clippy::too_many_arguments)]
pub fn fast_guided_pair(
    i: &[f32],
    p1: &[f32],
    p2: &[f32],
    w: usize,
    h: usize,
    r: usize,
    eps: f32,
    s: usize,
) -> (Vec<f32>, Vec<f32>) {
    if s <= 1 || w < 2 * s || h < 2 * s {
        return guided_pair(i, p1, p2, w, h, r, eps);
    }
    let (il, lw, lh) = downsample(i, w, h, s);
    let (p1l, _, _) = downsample(p1, w, h, s);
    let (p2l, _, _) = downsample(p2, w, h, s);
    let rl = ((r as f32 / s as f32).round() as usize).max(1);
    let c = guided_coeffs_pair(&il, &p1l, &p2l, lw, lh, rl, eps);
    let (xt, yt) = (up_taps(w, lw, s), up_taps(h, lh, s));
    let mut q1 = vec![0.0f32; w * h];
    let mut q2 = vec![0.0f32; w * h];
    q1.par_chunks_mut(w).zip(q2.par_chunks_mut(w)).enumerate().for_each(|(y, (r1, r2))| {
        let (y0, y1, fy) = yt[y];
        for (x, &(x0, x1, fx)) in xt.iter().enumerate() {
            let [a1, b1, a2, b2] = bilinear4(&c, lw, y0, y1, fy, x0, x1, fx);
            let g = i[y * w + x];
            r1[x] = a1 * g + b1;
            r2[x] = a2 * g + b2;
        }
    });
    (q1, q2)
}

/// Bilinear sample of four interleaved coarse planes.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn bilinear4(c: &[[f32; 4]], lw: usize, y0: usize, y1: usize, fy: f32, x0: usize, x1: usize, fx: f32) -> [f32; 4] {
    let (a, b, d, e) = (c[y0 * lw + x0], c[y0 * lw + x1], c[y1 * lw + x0], c[y1 * lw + x1]);
    [0, 1, 2, 3].map(|k| {
        let t = a[k] + (b[k] - a[k]) * fx;
        let u = d[k] + (e[k] - d[k]) * fx;
        t + (u - t) * fy
    })
}

/// Self-guided / cross-guided filter (He et al.): smooths `p` where the guide `i` is flat,
/// preserves edges of the guide. `eps` in squared guide units.
pub fn guided(i: &[f32], p: &[f32], w: usize, h: usize, r: usize, eps: f32) -> Vec<f32> {
    // Interleaved statistics / coefficients through `box2d_multi` (per channel identical to
    // separate `box2d` passes).
    let ab: Vec<[f32; 2]> = if std::ptr::eq(i, p) {
        let raw: Vec<[f32; 2]> = i.par_iter().map(|&a| [a, a * a]).collect();
        let st = box2d_multi(&raw, w, h, r);
        drop(raw);
        st.par_iter()
            .map(|m| {
                let var = (m[1] - m[0] * m[0]).max(0.0);
                let a = var / (var + eps);
                [a, m[0] - a * m[0]]
            })
            .collect()
    } else {
        let raw: Vec<[f32; 4]> = i.par_iter().zip(p).map(|(&a, &b)| [a, a * a, b, a * b]).collect();
        let st = box2d_multi(&raw, w, h, r);
        drop(raw);
        st.par_iter()
            .map(|m| {
                let var = (m[1] - m[0] * m[0]).max(0.0);
                let cov = m[3] - m[0] * m[2];
                let a = cov / (var + eps);
                [a, m[2] - a * m[0]]
            })
            .collect()
    };
    let mab = box2d_multi(&ab, w, h, r);
    drop(ab);
    mab.par_iter().zip(i).map(|(m, g)| m[0] * g + m[1]).collect()
}

/// Range bins per `range` of [`bilateral_grid`].
const BG_ZBIN: usize = 4;

/// Bilateral grid (Chen, Paris & Durand 2007) with a *hyper-Gaussian* range kernel
/// `exp(-(|d| / range)^power)`: differences well below `range` are smoothed like a plain
/// blur of ~`sigma_s` px, differences beyond it are cut off sharply, so steps stronger than
/// ~1.2 `range` are kept without halos while moderate contrasts are averaged. Cells of
/// `sigma_s` px blurred by a unit gaussian, range bins of `range / 4`, trilinear splat /
/// slice.
pub fn bilateral_grid(data: &[f32], w: usize, h: usize, sigma_s: f32, range: f32, power: f32) -> Vec<f32> {
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let s = sigma_s.max(1.0);
    let (lo, hi) = data.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), &v| (a.min(v), b.max(v)));
    let dz = range.max(1e-3) / BG_ZBIN as f32;
    let nz = ((hi - lo) / dz).ceil().max(0.0) as usize + 3;
    let (gh, gw) = ((h as f32 / s).ceil() as usize + 2, (w as f32 / s).ceil() as usize + 2);
    let idx = |y: usize, x: usize, z: usize| (y * gw + x) * nz + z;
    let coord = |k: usize, v: f32| {
        let (gy, gx, gz) = ((k / w) as f32 / s, (k % w) as f32 / s, (v - lo) / dz);
        let (y0, x0, z0) = (gy as usize, gx as usize, gz as usize);
        (y0, x0, z0, gy - y0 as f32, gx - x0 as f32, gz - z0 as f32)
    };
    // Splat: interleaved (value sum, weight).
    let mut grid = vec![[0.0f32; 2]; gh * gw * nz];
    for (k, &v) in data.iter().enumerate() {
        let (y0, x0, z0, fy, fx, fz) = coord(k, v);
        for (dy, wy) in [(0, 1.0 - fy), (1, fy)] {
            for (dx, wx) in [(0, 1.0 - fx), (1, fx)] {
                for (dzz, wz) in [(0, 1.0 - fz), (1, fz)] {
                    let wv = wy * wx * wz;
                    let c = &mut grid[idx(y0 + dy, x0 + dx, z0 + dzz)];
                    c[0] += wv * v;
                    c[1] += wv;
                }
            }
        }
    }
    // Spatial blur (unit gaussian, clamped edges) along y then x; range kernel along z.
    let gk: Vec<f32> = {
        let k: Vec<f32> = (-3i32..=3).map(|i| (-(i * i) as f32 / 2.0).exp()).collect();
        let sum: f32 = k.iter().sum();
        k.iter().map(|v| v / sum).collect()
    };
    let zr = 2 * BG_ZBIN as i32;
    let zk: Vec<f32> = (-zr..=zr).map(|i| (-((i as f32 * dz).abs() / range).powf(power)).exp()).collect();
    let conv = |src: &[[f32; 2]], axis: usize| -> Vec<[f32; 2]> {
        let (n, stride, kernel, clamp) = match axis {
            0 => (gh, gw * nz, &gk, true),
            1 => (gw, nz, &gk, true),
            _ => (nz, 1, &zk, false),
        };
        let r = (kernel.len() / 2) as i64;
        let mut out = vec![[0.0f32; 2]; src.len()];
        out.par_iter_mut().enumerate().for_each(|(k, o)| {
            let pos = (k / stride) % n;
            let base = k - pos * stride;
            let mut acc = [0.0f32; 2];
            for (j, kv) in kernel.iter().enumerate() {
                let p = pos as i64 + j as i64 - r;
                let p = if clamp {
                    p.clamp(0, n as i64 - 1)
                } else if p < 0 || p >= n as i64 {
                    continue;
                } else {
                    p
                };
                let c = src[base + p as usize * stride];
                acc[0] += kv * c[0];
                acc[1] += kv * c[1];
            }
            *o = acc;
        });
        out
    };
    let grid = conv(&conv(&conv(&grid, 0), 1), 2);
    // Slice.
    data.par_iter()
        .enumerate()
        .map(|(k, &v)| {
            let (y0, x0, z0, fy, fx, fz) = coord(k, v);
            let mut acc = [0.0f32; 2];
            for (dy, wy) in [(0, 1.0 - fy), (1, fy)] {
                for (dx, wx) in [(0, 1.0 - fx), (1, fx)] {
                    for (dzz, wz) in [(0, 1.0 - fz), (1, fz)] {
                        let wv = wy * wx * wz;
                        let c = grid[idx(y0 + dy, x0 + dx, z0 + dzz)];
                        acc[0] += wv * c[0];
                        acc[1] += wv * c[1];
                    }
                }
            }
            if acc[1] > 1e-12 {
                acc[0] / acc[1]
            } else {
                v
            }
        })
        .collect()
}

/// Edge-aware smoothing by the domain transform recursive filter (Gastal & Oliveira 2011):
/// halo-free, edges of `data` stronger than `sigma_r` are kept, flat areas are smoothed
/// over ~`sigma_s` pixels. Three iterations, rows and columns in parallel.
pub fn domain_transform(data: &[f32], w: usize, h: usize, sigma_s: f32, sigma_r: f32) -> Vec<f32> {
    let n_iter = 3;
    let ratio = sigma_s / sigma_r.max(1e-6);
    // Derivatives of the guide (the input) along x and y.
    let dx: Vec<f32> = (0..w * h)
        .map(|k| {
            let x = k % w;
            if x == 0 {
                1.0
            } else {
                1.0 + ratio * (data[k] - data[k - 1]).abs()
            }
        })
        .collect();
    let dy: Vec<f32> =
        (0..w * h).map(|k| if k < w { 1.0 } else { 1.0 + ratio * (data[k] - data[k - w]).abs() }).collect();
    let mut img = data.to_vec();
    let pass_rows = |img: &mut [f32], d: &[f32], w: usize, a: f32| {
        img.par_chunks_mut(w).zip(d.par_chunks(w)).for_each(|(row, dr)| {
            for x in 1..w {
                let f = a.powf(dr[x]);
                row[x] += f * (row[x - 1] - row[x]);
            }
            for x in (0..w - 1).rev() {
                let f = a.powf(dr[x + 1]);
                row[x] += f * (row[x + 1] - row[x]);
            }
        });
    };
    let dy_t = transpose(&dy, w, h);
    for i in 0..n_iter {
        let sigma_h = sigma_s * 3f32.sqrt() * 2f32.powi(n_iter - 1 - i) / (4f32.powi(n_iter) - 1.0).sqrt();
        let a = (-(2f32.sqrt()) / sigma_h.max(1e-3)).exp();
        pass_rows(&mut img, &dx, w, a);
        let mut t = transpose(&img, w, h);
        pass_rows(&mut t, &dy_t, h, a);
        img = transpose(&t, h, w);
    }
    img
}

fn transpose(data: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(h).enumerate().for_each(|(x, col)| {
        for (y, v) in col.iter_mut().enumerate() {
            *v = data[y * w + x];
        }
    });
    out
}

/// Robust noise standard deviation of a plane (Immerkaer's Laplacian estimator on a
/// subsample of rows).
pub fn noise_sigma(p: &[f32], w: usize, h: usize) -> f32 {
    if w < 3 || h < 3 {
        return 0.0;
    }
    let step = (h / 256).max(1);
    let rows: Vec<usize> = (1..h - 1).step_by(step).collect();
    let mut vals: Vec<f32> = rows
        .par_iter()
        .flat_map_iter(|&y| {
            let (u, m, d) = (&p[(y - 1) * w..y * w], &p[y * w..(y + 1) * w], &p[(y + 1) * w..(y + 2) * w]);
            (1..w - 1).step_by(2).map(move |x| {
                let lap = m[x - 1] + m[x + 1] + u[x] + d[x]
                    - 4.0 * m[x]
                    - 0.5 * (u[x - 1] + u[x + 1] + d[x - 1] + d[x + 1] - 4.0 * m[x]);
                lap.abs()
            })
        })
        .collect();
    if vals.is_empty() {
        return 0.0;
    }
    let mid = vals.len() / 2;
    let (_, med, _) = vals.select_nth_unstable_by(mid, |a, b| a.total_cmp(b));
    // Median absolute response of the kernel to unit gaussian noise is ~ 0.6745 * 3.
    *med / (0.6745 * 3.0)
}

// ---------------------------------------------------------------------------
// Noise reduction
// ---------------------------------------------------------------------------

/// Luminance + colour noise reduction on the linear image (working ProPhoto).
/// Domain: square-root encoded channels (variance stabilizing for shot noise), split into
/// luminance and two opponent chroma planes. Luminance: self-guided filter (edge
/// preserving) with strength from `luminance` and the measured noise level, `detail`
/// returns part of the removed texture, `contrast` restores mid-scale local contrast.
/// Colour: cross-guided filter of the chroma planes by the luminance (edges follow
/// luminance edges) at a larger radius (`smoothness`), `detail` keeps chroma detail.
pub fn denoise(img: &mut Working, nr: &NoiseReduction, scale: f32) {
    denoise_opts(img, nr, scale, true);
}

/// As [`denoise`]; `luminance = false` skips luminance NR (drafts). Large images (exports)
/// are processed in bands of rows (flat memory: ~0.1 GB of planes instead of ~1 GB for 24 MP).
pub fn denoise_opts(img: &mut Working, nr: &NoiseReduction, scale: f32, luminance: bool) {
    let band = if img.width * img.height > NR_BAND_MIN_PIXELS { NR_BAND_ROWS } else { img.height };
    denoise_banded(img, nr, scale, luminance, band);
}

/// Images above this many pixels are denoised in bands of [`NR_BAND_ROWS`] rows.
pub const NR_BAND_MIN_PIXELS: usize = 4 << 20;
pub const NR_BAND_ROWS: usize = 512;

/// Parameters of one denoise run (shared by all bands).
struct NrParams {
    lum: Option<(usize, f32, f32, f32, f32)>,
    col: Option<(usize, f32)>,
    /// Rows of context each side of a band (the filters' support).
    overlap: usize,
}

/// Square-root opponent planes (luminance, R - L, B - L) of `rgb` rows.
fn nr_planes(rgb: &[f32]) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let n = rgb.len() / 3;
    let (mut yq, mut c1, mut c2) = (vec![0.0f32; n], vec![0.0f32; n], vec![0.0f32; n]);
    rgb.par_chunks(3).zip(yq.par_iter_mut().zip(c1.par_iter_mut().zip(c2.par_iter_mut()))).for_each(
        |(p, (y, (a, b)))| {
            let q = [p[0].max(0.0).sqrt(), p[1].max(0.0).sqrt(), p[2].max(0.0).sqrt()];
            let l = 0.3 * q[0] + 0.6 * q[1] + 0.1 * q[2];
            *y = l;
            *a = q[0] - l;
            *b = q[2] - l;
        },
    );
    (yq, c1, c2)
}

/// [`noise_sigma`] of the three planes of the whole image, computed from sampled row
/// triplets (no full-size planes).
fn nr_sigmas(rgb: &[f32], w: usize, h: usize) -> [f32; 3] {
    if w < 3 || h < 3 {
        return [0.0; 3];
    }
    let step = (h / 256).max(1);
    let rows: Vec<usize> = (1..h - 1).step_by(step).collect();
    let per_row: Vec<[Vec<f32>; 3]> = rows
        .par_iter()
        .map(|&y| {
            let (yq, c1, c2) = nr_planes(&rgb[(y - 1) * w * 3..(y + 2) * w * 3]);
            [&yq, &c1, &c2].map(|p| {
                let (u, m, d) = (&p[..w], &p[w..2 * w], &p[2 * w..]);
                (1..w - 1)
                    .step_by(2)
                    .map(|x| {
                        let lap = m[x - 1] + m[x + 1] + u[x] + d[x]
                            - 4.0 * m[x]
                            - 0.5 * (u[x - 1] + u[x + 1] + d[x - 1] + d[x + 1] - 4.0 * m[x]);
                        lap.abs()
                    })
                    .collect()
            })
        })
        .collect();
    [0, 1, 2].map(|k| {
        let mut vals: Vec<f32> = per_row.iter().flat_map(|r| r[k].iter().copied()).collect();
        if vals.is_empty() {
            return 0.0;
        }
        let mid = vals.len() / 2;
        let (_, med, _) = vals.select_nth_unstable_by(mid, |a, b| a.total_cmp(b));
        *med / (0.6745 * 3.0)
    })
}

/// Denoises rows of `rgb` (w wide) and returns the processed rows.
fn nr_region(rgb: &[f32], w: usize, p: &NrParams) -> Vec<f32> {
    let h = rgb.len() / 3 / w;
    // Colour radius >= 3: the fast guided filter at half resolution, fused (full-resolution
    // chroma planes are never built).
    if let Some((r, eps)) = p.col.filter(|(r, _)| *r >= 3 && w >= 4 && h >= 4) {
        return nr_region_half(rgb, w, h, p, r, eps);
    }
    // Luminance only: one plane; the untouched chroma terms are recomputed from `rgb`.
    if p.col.is_none() {
        if let Some((r, eps, keep, contrast, blur_sigma)) = p.lum {
            let yq: Vec<f32> = rgb.par_chunks(3).map(|q| nr_pixel(q)[0]).collect();
            let smooth = guided(&yq, &yq, w, h, r, eps);
            let base = if contrast > 0.0 { Some(blur(&smooth, w, h, blur_sigma)) } else { None };
            let mut out = vec![0.0f32; rgb.len()];
            out.par_chunks_mut(3).zip(rgb.par_chunks(3)).enumerate().for_each(|(k, (o, q))| {
                let [l0, c1, c2] = nr_pixel(q);
                let s = smooth[k];
                let mut v = s + (l0 - s) * keep;
                if let Some(b) = &base {
                    v += (s - b[k]) * contrast * 0.3;
                }
                nr_rgb(v, c1, c2, o);
            });
            return out;
        }
    }
    let (mut yq, mut c1, mut c2) = nr_planes(rgb);
    if let Some((r, eps, keep, contrast, blur_sigma)) = p.lum {
        let smooth = guided(&yq, &yq, w, h, r, eps);
        let base = if contrast > 0.0 { Some(blur(&smooth, w, h, blur_sigma)) } else { None };
        yq.par_iter_mut().enumerate().for_each(|(k2, y)| {
            let s = smooth[k2];
            let mut v = s + (*y - s) * keep;
            if let Some(b) = &base {
                v += (s - b[k2]) * contrast * 0.3;
            }
            *y = v;
        });
    }
    if let Some((r, eps)) = p.col {
        (c1, c2) = guided_pair(&yq, &c1, &c2, w, h, r, eps);
    }
    let mut out = vec![0.0f32; rgb.len()];
    out.par_chunks_mut(3).enumerate().for_each(|(k, o)| {
        let l = yq[k];
        let r = (c1[k] + l).max(0.0);
        let b = (c2[k] + l).max(0.0);
        let g = ((l - 0.3 * r - 0.1 * b) / 0.6).max(0.0);
        o[0] = r * r;
        o[1] = g * g;
        o[2] = b * b;
    });
    out
}

/// Square-root opponent planes of one pixel: (luminance, R - L, B - L).
#[inline(always)]
fn nr_pixel(p: &[f32]) -> [f32; 3] {
    let q = [p[0].max(0.0).sqrt(), p[1].max(0.0).sqrt(), p[2].max(0.0).sqrt()];
    let l = 0.3 * q[0] + 0.6 * q[1] + 0.1 * q[2];
    [l, q[0] - l, q[2] - l]
}

/// Inverse of [`nr_pixel`] into linear RGB.
#[inline(always)]
fn nr_rgb(l: f32, c1: f32, c2: f32, o: &mut [f32]) {
    let r = (c1 + l).max(0.0);
    let b = (c2 + l).max(0.0);
    let g = ((l - 0.3 * r - 0.1 * b) / 0.6).max(0.0);
    o[0] = r * r;
    o[1] = g * g;
    o[2] = b * b;
}

/// [`nr_region`] with the colour guided filter at half resolution ([`fast_guided_pair`]):
/// full-resolution luminance + 2x2 block means of the three planes in one pass, the
/// coefficients at half resolution, then one pass that upsamples them and rebuilds RGB.
fn nr_region_half(rgb: &[f32], w: usize, h: usize, p: &NrParams, r: usize, eps: f32) -> Vec<f32> {
    let (lw, lh) = (w.div_ceil(2), h.div_ceil(2));
    let mut yq = vec![0.0f32; w * h];
    let mut low = vec![[0.0f32; 3]; lw * lh];
    yq.par_chunks_mut(2 * w).zip(low.par_chunks_mut(lw)).enumerate().for_each(|(ly, (yrows, lrow))| {
        let rows = yrows.len() / w;
        for (lx, cell) in lrow.iter_mut().enumerate() {
            let mut acc = [0.0f32; 3];
            let mut n = 0.0f32;
            for dy in 0..rows {
                let y = 2 * ly + dy;
                for x in 2 * lx..(2 * lx + 2).min(w) {
                    let q = nr_pixel(&rgb[(y * w + x) * 3..(y * w + x) * 3 + 3]);
                    yrows[dy * w + x] = q[0];
                    acc[0] += q[0];
                    acc[1] += q[1];
                    acc[2] += q[2];
                    n += 1.0;
                }
            }
            *cell = acc.map(|v| v / n);
        }
    });
    let mut il: Vec<f32> = low.par_iter().map(|c| c[0]).collect();
    let c1l: Vec<f32> = low.par_iter().map(|c| c[1]).collect();
    let c2l: Vec<f32> = low.par_iter().map(|c| c[2]).collect();
    drop(low);
    // Luminance NR (full resolution); the colour guide is the denoised luminance.
    let mut yd = None;
    if let Some((lr, leps, keep, contrast, blur_sigma)) = p.lum {
        let smooth = guided(&yq, &yq, w, h, lr, leps);
        let base = if contrast > 0.0 { Some(blur(&smooth, w, h, blur_sigma)) } else { None };
        let mut d = yq.clone();
        d.par_iter_mut().enumerate().for_each(|(k2, y)| {
            let s = smooth[k2];
            let mut v = s + (*y - s) * keep;
            if let Some(b) = &base {
                v += (s - b[k2]) * contrast * 0.3;
            }
            *y = v;
        });
        il = downsample(&d, w, h, 2).0;
        yd = Some(d);
    }
    let guide = yd.as_deref().unwrap_or(&yq);
    let rl = ((r as f32 / 2.0).round() as usize).max(1);
    let c = guided_coeffs_pair(&il, &c1l, &c2l, lw, lh, rl, eps);
    let (xt, yt) = (up_taps(w, lw, 2), up_taps(h, lh, 2));
    let mut out = vec![0.0f32; rgb.len()];
    out.par_chunks_mut(w * 3).enumerate().for_each(|(y, orow)| {
        let (y0, y1, fy) = yt[y];
        for (x, &(x0, x1, fx)) in xt.iter().enumerate() {
            let [a1, b1, a2, b2] = bilinear4(&c, lw, y0, y1, fy, x0, x1, fx);
            let l = guide[y * w + x];
            nr_rgb(l, a1 * l + b1, a2 * l + b2, &mut orow[x * 3..x * 3 + 3]);
        }
    });
    out
}

/// [`denoise_opts`] with an explicit band height (`band_rows >= height` = whole image).
pub fn denoise_banded(img: &mut Working, nr: &NoiseReduction, scale: f32, luminance: bool, band_rows: usize) {
    let (w, h) = (img.width, img.height);
    let lum_amt = if luminance { (nr.luminance / 100.0).clamp(0.0, 1.0) } else { 0.0 };
    let col_amt = (nr.color / 100.0).clamp(0.0, 1.0);
    if (lum_amt == 0.0 && col_amt == 0.0) || w < 4 || h < 4 {
        return;
    }
    let scale = scale.clamp(0.05, 4.0);
    let sig = nr_sigmas(img.rgb, w, h);
    let lum = (lum_amt > 0.0 && sig[0] > 0.0).then(|| {
        let r = ((1.5 * scale.sqrt()).round() as usize).clamp(1, 3);
        let k = 0.6 + 3.4 * lum_amt;
        let eps = (k * sig[0]) * (k * sig[0]);
        let keep = (nr.luminance_detail / 100.0).clamp(0.0, 1.0);
        let contrast = (nr.luminance_contrast / 100.0).clamp(0.0, 1.0);
        (r, eps, 0.35 * keep * keep, contrast, 6.0 * scale.max(0.3))
    });
    let col = (col_amt > 0.0).then(|| {
        let s = sig[1].max(sig[2]).max(1e-4);
        let smooth = (nr.color_smoothness / 100.0).clamp(0.0, 1.0);
        let r = ((2.0 + 10.0 * col_amt + 6.0 * smooth) * scale).round().clamp(1.0, 24.0) as usize;
        let detail = (nr.color_detail / 100.0).clamp(0.0, 1.0);
        (r, (s * (2.0 + 6.0 * col_amt) * (1.2 - detail)).powi(2).max(1e-8))
    });
    // Support: guided = 2 boxes of r; blur = 3 boxes of ~sigma.
    let lum_support = lum.map_or(0, |(r, _, _, c, bs)| 2 * r + if c > 0.0 { 3 * (bs.ceil() as usize + 1) } else { 0 });
    // (+ the half-resolution grid of the fast guided filter: rounding + bilinear support.)
    let col_support = col.map_or(0, |(r, _)| 2 * r + 6);
    let p = NrParams { lum, col, overlap: lum_support + col_support + 2 };
    if band_rows >= h {
        let out = nr_region(img.rgb, w, &p);
        img.rgb.copy_from_slice(&out);
        return;
    }
    // Bands: each band's output is written back only after the next band has read its
    // (unprocessed) upper context.
    let mut pending: Option<(usize, Vec<f32>)> = None;
    let mut y0 = 0;
    while y0 < h {
        let y1 = (y0 + band_rows).min(h);
        // Even start: the fast guided filter's 2x2 grid stays aligned with the whole image's.
        let (a, b) = (y0.saturating_sub(p.overlap) & !1, (y1 + p.overlap).min(h));
        let out = nr_region(&img.rgb[a * w * 3..b * w * 3], w, &p);
        if let Some((py, rows)) = pending.take() {
            img.rgb[py * w * 3..py * w * 3 + rows.len()].copy_from_slice(&rows);
        }
        pending = Some((y0, out[(y0 - a) * w * 3..(y1 - a) * w * 3].to_vec()));
        y0 = y1;
    }
    if let Some((py, rows)) = pending {
        img.rgb[py * w * 3..py * w * 3 + rows.len()].copy_from_slice(&rows);
    }
}

// ---------------------------------------------------------------------------
// Sharpening, vignette, grain (display-referred encoded values)
// ---------------------------------------------------------------------------

/// Capture sharpening (unsharp mask on luminance with detail/edge masking), radius in
/// full-resolution px times `scale`.
/// - `amount` 0..=150 -> gain; `radius` -> blur sigma; `detail` suppresses halos on strong
///   edges when low (0 = smooth, 100 = crisp); `masking` restricts sharpening to edges.
/// - Below one working pixel the effect is attenuated like Lightroom's downscaled output.
pub fn sharpen(img: &mut Working, sharpening: &Sharpening, scale: f32) {
    let (w, h) = (img.width, img.height);
    let amount = (sharpening.amount / 100.0).clamp(0.0, 1.5);
    if amount <= 0.0 || w < 3 || h < 3 {
        return;
    }
    let mut sigma = sharpening.radius.clamp(0.5, 3.0) * 0.8 * scale;
    let mut gain = amount * 1.1;
    if sigma < 0.5 {
        gain *= (sigma / 0.5).powi(2);
        sigma = 0.5;
    }
    if gain < 1e-3 {
        return;
    }
    let lum: Vec<f32> = img.rgb.par_chunks(3).map(|p| 0.2627 * p[0] + 0.6780 * p[1] + 0.0593 * p[2]).collect();
    let blurred = blur(&lum, w, h, sigma);
    let detail = (sharpening.detail / 100.0).clamp(0.0, 1.0);
    let halo = 0.015 + 0.25 * detail * detail;
    let masking = (sharpening.masking / 100.0).clamp(0.0, 1.0);
    let edge = if masking > 0.0 {
        let soft = blur(&lum, w, h, (1.5 * scale).max(0.7));
        let mut g = vec![0.0f32; w * h];
        g.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let up = &soft[y.saturating_sub(1) * w..][..w];
            let dn = &soft[(y + 1).min(h - 1) * w..][..w];
            let mid = &soft[y * w..][..w];
            for x in 0..w {
                let gx = mid[(x + 1).min(w - 1)] - mid[x.saturating_sub(1)];
                let gy = dn[x] - up[x];
                row[x] = (gx * gx + gy * gy).sqrt();
            }
        });
        let t = 0.002 + 0.06 * masking * masking;
        Some((g, t))
    } else {
        None
    };
    img.rgb.par_chunks_mut(w * 3).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let k = y * w + x;
            let d = lum[k] - blurred[k];
            let d = d / (1.0 + (d / halo).powi(2));
            let m = match &edge {
                Some((g, t)) => smoothstep(0.5 * t, 1.5 * t, g[k]),
                None => 1.0,
            };
            let delta = gain * d * m;
            let p = &mut row[x * 3..x * 3 + 3];
            for c in p.iter_mut() {
                *c += delta;
            }
        }
    });
}

/// Precomputed post-crop vignette.
pub struct Vignette {
    amount: f32,
    mid: f32,
    feather: f32,
    roundness: f32,
    highlights: f32,
    style: VignetteStyle,
}

impl Vignette {
    pub fn new(v: &PostCropVignette) -> Option<Self> {
        (v.amount != 0.0).then(|| Vignette {
            amount: (v.amount / 100.0).clamp(-1.0, 1.0),
            mid: (v.midpoint / 100.0).clamp(0.0, 1.0),
            feather: (v.feather / 100.0).clamp(0.0, 1.0),
            roundness: (v.roundness / 100.0).clamp(-1.0, 1.0),
            highlights: (v.highlights / 100.0).clamp(0.0, 1.0),
            style: v.style,
        })
    }

    /// Mask 0 (centre) .. 1 (corners) at frame-normalized position (0..=1) of a frame with
    /// aspect `aspect` (w / h).
    #[inline]
    pub fn mask(&self, u: f32, v: f32, aspect: f32) -> f32 {
        let x = (u - 0.5) * 2.0;
        let y = (v - 0.5) * 2.0;
        let r = self.roundness;
        // Roundness 0: ellipse following the frame; < 0 towards a rounded rectangle
        // (superellipse); > 0 towards a circle (in pixel space).
        let p = if r < 0.0 { 2.0 - r * 6.0 } else { 2.0 };
        let d_ell = (x.abs().powf(p) + y.abs().powf(p)).powf(1.0 / p);
        let d = if r > 0.0 {
            let a = aspect.max(1e-3);
            let dc = if a >= 1.0 { (x * x + (y / a) * (y / a)).sqrt() } else { ((x * a) * (x * a) + y * y).sqrt() };
            d_ell * (1.0 - r) + dc * r
        } else {
            d_ell
        };
        let inner = 0.3 + self.mid;
        let width = 0.1 + 1.4 * self.feather;
        smoothstep(inner - width * 0.5, inner + width * 0.5, d)
    }

    /// Applies the vignette to encoded display RGB with mask `m`.
    #[inline]
    pub fn apply(&self, rgb: [f32; 3], m: f32) -> [f32; 3] {
        if m <= 0.0 {
            return rgb;
        }
        let a = self.amount * m;
        match self.style {
            VignetteStyle::PaintOverlay => {
                let target = if a < 0.0 { 0.0 } else { 1.0 };
                rgb.map(|c| c + (target - c) * a.abs() * 0.8)
            }
            _ => {
                // Exposure-like in linear light; highlight priority protects bright pixels.
                let lin = rgb.map(|c| crate::develop::pipeline::srgb_decode(c.clamp(0.0, 1.0)));
                let l = lin[0] * 0.2627 + lin[1] * 0.6780 + lin[2] * 0.0593;
                let protect = if a < 0.0 && self.style == VignetteStyle::HighlightPriority {
                    1.0 - self.highlights * smoothstep(0.5, 1.0, l)
                } else {
                    1.0
                };
                // Colour priority keeps the channel ratios (hue and saturation) exactly.
                let g = (a * 2.0 * protect).exp2();
                lin.map(|c| crate::develop::pipeline::srgb_encode((c * g).clamp(0.0, 1.0)))
            }
        }
    }
}

/// Post-crop vignette over the whole (cropped) working frame.
pub fn vignette(img: &mut Working, v: &PostCropVignette) {
    let Some(vg) = Vignette::new(v) else { return };
    let (w, h) = (img.width, img.height);
    let aspect = w as f32 / h.max(1) as f32;
    img.rgb.par_chunks_mut(w * 3).enumerate().for_each(|(y, row)| {
        let v = (y as f32 + 0.5) / h as f32;
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32;
            let m = vg.mask(u, v, aspect);
            let p = &mut row[x * 3..x * 3 + 3];
            let out = vg.apply([p[0], p[1], p[2]], m);
            p.copy_from_slice(&out);
        }
    });
}

/// Film grain generator (value noise, two octaves), deterministic per seed.
pub struct GrainGen {
    amount: f32,
    cell: f32,
    rough: f32,
    seed: u64,
}

#[inline]
fn hash(x: i64, y: i64, seed: u64) -> f32 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ seed;
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    h = h.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    h ^= h >> 33;
    (h >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
}

impl GrainGen {
    pub fn new(g: &Grain, seed: u64) -> Option<Self> {
        (g.amount > 0.0).then(|| GrainGen {
            amount: (g.amount / 100.0).clamp(0.0, 1.0),
            cell: 0.8 + (g.size / 100.0).clamp(0.0, 1.0) * 3.2,
            rough: (g.roughness / 100.0).clamp(0.0, 1.0),
            seed: seed.wrapping_mul(0x2545_F491_4F6C_DD1D) ^ 0x5151,
        })
    }

    #[inline]
    fn value(&self, x: f32, y: f32, cell: f32, salt: u64) -> f32 {
        let gx = x / cell;
        let gy = y / cell;
        let (ix, iy) = (gx.floor() as i64, gy.floor() as i64);
        let (fx, fy) = (gx - ix as f32, gy - iy as f32);
        let s = |t: f32| t * t * (3.0 - 2.0 * t);
        let (sx, sy) = (s(fx), s(fy));
        let seed = self.seed ^ salt;
        let a = hash(ix, iy, seed);
        let b = hash(ix + 1, iy, seed);
        let c = hash(ix, iy + 1, seed);
        let d = hash(ix + 1, iy + 1, seed);
        let top = a + (b - a) * sx;
        let bot = c + (d - c) * sx;
        top + (bot - top) * sy
    }

    /// Grain offset at full-resolution frame position (px); `footprint` = full-res px per
    /// working px (averaging reduces grain on downscaled renders).
    #[inline]
    pub fn noise(&self, x: f32, y: f32, footprint: f32) -> f32 {
        let n1 = self.value(x, y, self.cell, 1);
        let n2 = self.value(x, y, self.cell * 0.5, 2);
        let n = n1 * (1.0 - 0.5 * self.rough) + n2 * self.rough * 0.8;
        let atten = (self.cell / footprint.max(self.cell)).sqrt();
        n * self.amount * 0.12 * atten
    }

    /// Adds grain to encoded RGB (luminance, strongest in the midtones).
    #[inline]
    pub fn apply(&self, rgb: [f32; 3], n: f32) -> [f32; 3] {
        let l = (rgb[0] * 0.2627 + rgb[1] * 0.6780 + rgb[2] * 0.0593).clamp(0.0, 1.0);
        let k = n * (0.3 + 2.8 * l * (1.0 - l));
        rgb.map(|c| c + k)
    }
}

/// Film grain, deterministic for `seed` (the image id), size scaled by `scale`.
pub fn grain(img: &mut Working, g: &Grain, seed: u64, scale: f32) {
    let Some(gen) = GrainGen::new(g, seed) else { return };
    let w = img.width;
    let foot = 1.0 / scale.max(1e-3);
    img.rgb.par_chunks_mut(w * 3).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let n = gen.noise((x as f32 + 0.5) * foot, (y as f32 + 0.5) * foot, foot);
            let p = &mut row[x * 3..x * 3 + 3];
            let out = gen.apply([p[0], p[1], p[2]], n);
            p.copy_from_slice(&out);
        }
    });
}

// ---------------------------------------------------------------------------
// Crop geometry
// ---------------------------------------------------------------------------

/// Output frame of a crop: size in source px (orientation applied) and the affine map from
/// normalized output coordinates (0..=1, oriented) to normalized *un-oriented* source
/// coordinates (`[a, b, c, d, e, f]`: sx = a*x + b*y + c, sy = d*x + e*y + f).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CropGeometry {
    pub width: u32,
    pub height: u32,
    pub to_source: [f64; 6],
}

impl CropGeometry {
    /// Maps normalized oriented output coordinates to normalized un-oriented source ones.
    pub fn map(&self, x: f64, y: f64) -> (f64, f64) {
        let t = &self.to_source;
        (t[0] * x + t[1] * y + t[2], t[3] * x + t[4] * y + t[5])
    }

    /// No rotation (the crop is an axis-aligned source rectangle).
    pub fn is_axis_aligned(&self) -> bool {
        let t = &self.to_source;
        (t[1].abs() < 1e-12 && t[3].abs() < 1e-12) || (t[0].abs() < 1e-12 && t[4].abs() < 1e-12)
    }
}

/// Oriented -> un-oriented normalized coordinate map for EXIF orientation `o` as
/// (a, b, c, d, e, f): ux = a*x + b*y + c, uy = d*x + e*y + f.
pub fn orientation_map(o: u8) -> [f64; 6] {
    match o {
        2 => [-1.0, 0.0, 1.0, 0.0, 1.0, 0.0],
        3 => [-1.0, 0.0, 1.0, 0.0, -1.0, 1.0],
        4 => [1.0, 0.0, 0.0, 0.0, -1.0, 1.0],
        5 => [0.0, 1.0, 0.0, 1.0, 0.0, 0.0],
        6 => [0.0, 1.0, 0.0, -1.0, 0.0, 1.0],
        7 => [0.0, -1.0, 1.0, -1.0, 0.0, 1.0],
        8 => [0.0, -1.0, 1.0, 1.0, 0.0, 0.0],
        _ => [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
    }
}

/// Crop/straighten geometry in Lightroom's semantics (`CropSettings` docs): `src_w/src_h`
/// un-oriented source size, `orientation` EXIF 1..=8. Disabled crop = the whole frame.
///
/// `(left, top)` and `(right, bottom)` are the crop frame's top-left and bottom-right corners
/// in un-oriented normalized source coordinates; the frame is rotated by `angle` degrees
/// (positive = clockwise in the source, y down) about the midpoint of the two corners, so
/// its size is the corner-to-corner vector rotated back by `-angle` (checked against Camera
/// Raw renders: a -5.74 degree crop of a 7008 x 4672 frame with corners (0.0364, 0.1310) and
/// (0.9636, 0.8690) renders 6120 x 4080). Output size in source pixels, oriented.
pub fn crop_geometry(crop: &CropSettings, src_w: u32, src_h: u32, orientation: u8) -> CropGeometry {
    let o = if (1..=8).contains(&orientation) { orientation } else { 1 };
    let (sw, sh) = (f64::from(src_w.max(1)), f64::from(src_h.max(1)));
    let (l, t, r, b, angle) = if crop.enabled {
        let l = f64::from(crop.left).clamp(0.0, 1.0);
        let r = f64::from(crop.right).clamp(0.0, 1.0);
        let t = f64::from(crop.top).clamp(0.0, 1.0);
        let b = f64::from(crop.bottom).clamp(0.0, 1.0);
        if r - l > 1e-6 && b - t > 1e-6 {
            (l, t, r, b, f64::from(crop.angle).clamp(-45.0, 45.0))
        } else {
            (0.0, 0.0, 1.0, 1.0, 0.0)
        }
    } else {
        (0.0, 0.0, 1.0, 1.0, 0.0)
    };
    // (left, top) and (right, bottom) are the crop frame's top-left and bottom-right corners
    // in the source; the frame is rotated by `angle`, so its size is the corner-to-corner
    // vector rotated back (verified against Camera Raw renders of straightened crops).
    let (cx, cy) = ((l + r) / 2.0 * sw, (t + b) / 2.0 * sh);
    let (dx, dy) = ((r - l) * sw, (b - t) * sh);
    let th = angle.to_radians();
    let (sn, cs) = th.sin_cos();
    let (fw, fh) = ((dx * cs + dy * sn).max(1.0), (-dx * sn + dy * cs).max(1.0));
    // Un-oriented frame coords (u, v in 0..1) -> source px:
    // p = c + R(th) * ((u - 0.5) fw, (v - 0.5) fh).
    let uv_to_src = [
        cs * fw / sw,
        -sn * fh / sw,
        (cx - cs * fw * 0.5 + sn * fh * 0.5) / sw,
        sn * fw / sh,
        cs * fh / sh,
        (cy - sn * fw * 0.5 - cs * fh * 0.5) / sh,
    ];
    // Oriented output coords -> un-oriented frame coords.
    let om = orientation_map(o);
    let m = [
        uv_to_src[0] * om[0] + uv_to_src[1] * om[3],
        uv_to_src[0] * om[1] + uv_to_src[1] * om[4],
        uv_to_src[0] * om[2] + uv_to_src[1] * om[5] + uv_to_src[2],
        uv_to_src[3] * om[0] + uv_to_src[4] * om[3],
        uv_to_src[3] * om[1] + uv_to_src[4] * om[4],
        uv_to_src[3] * om[2] + uv_to_src[4] * om[5] + uv_to_src[5],
    ];
    let (w, h) = (fw.round().max(1.0) as u32, fh.round().max(1.0) as u32);
    let (w, h) = if o >= 5 { (h, w) } else { (w, h) };
    CropGeometry { width: w, height: h, to_source: m }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::PrimaryCalibration;

    /// `box2d` equals a naive edge-clamped window mean (sizes around the row block).
    #[test]
    fn box2d_matches_naive_mean() {
        for (w, h, r) in [(37usize, 70usize, 3usize), (5, 3, 4), (64, 33, 1), (19, 100, 12)] {
            let src: Vec<f32> = (0..w * h).map(|k| ((k * 7919) % 101) as f32 / 101.0).collect();
            let got = box2d(&src, w, h, r);
            for y in 0..h {
                for x in 0..w {
                    let (mut s, mut n) = (0.0f64, 0);
                    for yy in y.saturating_sub(r)..=(y + r).min(h - 1) {
                        for xx in x.saturating_sub(r)..=(x + r).min(w - 1) {
                            s += f64::from(src[yy * w + xx]);
                            n += 1;
                        }
                    }
                    let e = (s / f64::from(n)) as f32;
                    assert!((got[y * w + x] - e).abs() < 1e-5, "{w}x{h} r{r} at {x},{y}: {} vs {e}", got[y * w + x]);
                }
            }
        }
    }

    /// The fused half-resolution colour NR equals the explicit route (opponent planes ->
    /// fast guided filter -> RGB).
    #[test]
    fn fused_colour_nr_matches_explicit_fast_guided() {
        let (w, h) = (83usize, 57usize);
        let rgb: Vec<f32> = (0..w * h * 3).map(|k| 0.05 + 0.3 * (((k * 2654435761) % 1000) as f32 / 1000.0)).collect();
        let p = NrParams { lum: None, col: Some((5, 0.002)), overlap: 0 };
        let got = nr_region(&rgb, w, &p);
        let (yq, c1, c2) = nr_planes(&rgb);
        let (q1, q2) = fast_guided_pair(&yq, &c1, &c2, w, h, 5, 0.002, 2);
        let mut want = vec![0.0f32; rgb.len()];
        for k in 0..w * h {
            nr_rgb(yq[k], q1[k], q2[k], &mut want[k * 3..k * 3 + 3]);
        }
        let worst = got.iter().zip(&want).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-5, "worst {worst}");
    }

    /// Luminance-only NR (one plane) equals the three-plane route bit for bit.
    #[test]
    fn luminance_only_nr_matches_three_plane_route() {
        let (w, h) = (71usize, 45usize);
        let rgb: Vec<f32> = (0..w * h * 3).map(|k| 0.02 + 0.4 * (((k * 2654435761) % 997) as f32 / 997.0)).collect();
        let lum = (2usize, 0.004f32, 0.2f32, 0.4f32, 3.0f32);
        let got = nr_region(&rgb, w, &NrParams { lum: Some(lum), col: None, overlap: 0 });
        let (mut yq, c1, c2) = nr_planes(&rgb);
        let smooth = guided(&yq, &yq, w, h, lum.0, lum.1);
        let base = blur(&smooth, w, h, lum.4);
        for k in 0..w * h {
            let s = smooth[k];
            yq[k] = s + (yq[k] - s) * lum.2 + (s - base[k]) * lum.3 * 0.3;
        }
        let mut want = vec![0.0f32; rgb.len()];
        for k in 0..w * h {
            nr_rgb(yq[k], c1[k], c2[k], &mut want[k * 3..k * 3 + 3]);
        }
        assert_eq!(got, want);
    }

    /// The paired cross-guided filter equals two single calls bit for bit.
    #[test]
    fn guided_pair_matches_two_guided_calls() {
        let (w, h) = (97usize, 61usize);
        let f = |k: usize, a: f32| ((k as f32 * a).sin() * 0.5 + 0.5) * (1.0 + (k % 7) as f32 * 0.1);
        let i: Vec<f32> = (0..w * h).map(|k| f(k, 0.013)).collect();
        let p1: Vec<f32> = (0..w * h).map(|k| f(k, 0.071) - 0.3).collect();
        let p2: Vec<f32> = (0..w * h).map(|k| f(k, 0.029) * 0.2).collect();
        let (a, b) = guided_pair(&i, &p1, &p2, w, h, 5, 0.01);
        assert_eq!(a, guided(&i, &p1, w, h, 5, 0.01));
        assert_eq!(b, guided(&i, &p2, w, h, 5, 0.01));
    }

    /// The hyper-Gaussian bilateral grid averages moderate contrasts (texture, soft
    /// gradients) but keeps strong steps exactly (no halo), and preserves constants.
    #[test]
    fn bilateral_grid_smooths_texture_and_keeps_strong_steps() {
        let (w, h) = (256usize, 64usize);
        // Left: -6 EV with a +-0.5 EV checkerboard texture; right: -1.5 EV flat (4.5 EV step).
        let data: Vec<f32> = (0..w * h)
            .map(|k| {
                let (x, y) = (k % w, k / w);
                if x < w / 2 {
                    -6.0 + if (x / 2 + y / 2) % 2 == 0 { 0.5 } else { -0.5 }
                } else {
                    -1.5
                }
            })
            .collect();
        let out = bilateral_grid(&data, w, h, 8.0, 2.5, 6.0);
        let at = |x: usize| out[32 * w + x];
        // Texture averaged away (far from the edge and right next to it).
        for x in [20usize, 60, 120, 125] {
            assert!((at(x) + 6.0).abs() < 0.05, "x {x}: {}", at(x));
        }
        // Bright side untouched up to the edge.
        for x in [128usize, 130, 200] {
            assert!((at(x) + 1.5).abs() < 1e-3, "x {x}: {}", at(x));
        }
        let flat = bilateral_grid(&[0.7f32; 100], 10, 10, 3.0, 2.5, 6.0);
        assert!(flat.iter().all(|v| (v - 0.7).abs() < 1e-5));
    }

    /// Banded noise reduction (exports) equals the whole-image result.
    #[test]
    fn banded_denoise_matches_whole_image() {
        let (w, h) = (160usize, 700usize);
        let mut s = 11u32;
        let mut rnd = || {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 8) as f32 / (1 << 24) as f32
        };
        let mut rgb = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                let base = if (x / 40 + y / 90) % 2 == 0 { 0.05 } else { 0.3 };
                for c in 0..3 {
                    rgb.push(base * (1.0 + 0.2 * c as f32) + 0.02 * (rnd() - 0.5));
                }
            }
        }
        let nr = NoiseReduction {
            luminance: 60.0,
            luminance_detail: 50.0,
            luminance_contrast: 40.0,
            color: 50.0,
            color_detail: 50.0,
            color_smoothness: 50.0,
        };
        let mut whole = rgb.clone();
        denoise_banded(&mut Working { width: w, height: h, rgb: &mut whole }, &nr, 1.0, true, h);
        let mut banded = rgb.clone();
        denoise_banded(&mut Working { width: w, height: h, rgb: &mut banded }, &nr, 1.0, true, 64);
        let worst = whole.iter().zip(&banded).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-4, "banded differs by {worst}");
        assert!(whole.iter().zip(&rgb).any(|(a, b)| (a - b).abs() > 1e-3), "NR had an effect");
    }

    fn is_monotone(l: &[f32]) -> bool {
        l.windows(2).all(|w| w[1] >= w[0] - 1e-6)
    }

    #[test]
    fn parametric_curve_identity_monotone_and_directions() {
        assert!(parametric_curve_lut(&ParametricCurve::default()).is_none());
        let at = |c: ParametricCurve, x: f32| eval_lut(&parametric_curve_lut(&c).unwrap(), x);
        let d = ParametricCurve::default();
        for (field, x) in [(0, 0.15f32), (1, 0.35), (2, 0.65), (3, 0.85)] {
            for sign in [-1.0f32, 1.0] {
                let mut c = d;
                let v = 60.0 * sign;
                match field {
                    0 => c.shadows = v,
                    1 => c.darks = v,
                    2 => c.lights = v,
                    _ => c.highlights = v,
                }
                let lut = parametric_curve_lut(&c).unwrap();
                assert!(is_monotone(&lut));
                let y = at(c, x);
                assert!((y - x) * sign > 0.005, "region {field} sign {sign}: {x} -> {y}");
            }
        }
        // Region locality: highlights leave the shadows alone.
        let c = ParametricCurve { highlights: 100.0, ..d };
        assert!((at(c, 0.2) - 0.2).abs() < 1e-3);
        // The user's typical curve.
        let u = ParametricCurve {
            shadows: -5.0,
            darks: -15.0,
            lights: 20.0,
            highlights: -15.0,
            shadow_split: 15.0,
            midtone_split: 35.0,
            highlight_split: 75.0,
        };
        let lut = parametric_curve_lut(&u).unwrap();
        assert!(is_monotone(&lut));
        assert!(eval_lut(&lut, 0.25) < 0.25 && eval_lut(&lut, 0.6) > 0.6);
    }

    #[test]
    fn point_curve_is_a_natural_spline_through_the_points() {
        assert!(point_curve_lut(&PointCurves::identity_curve()).is_none());
        let pts = vec![[0.0, 14.0], [44.0, 46.0], [106.0, 110.0], [255.0, 252.0]];
        let lut = point_curve_lut(&pts).unwrap();
        for p in &pts {
            let y = eval_lut(&lut, p[0] / 255.0) * 255.0;
            assert!((y - p[1]).abs() < 0.2, "{p:?} -> {y}");
        }
        assert!(is_monotone(&lut));
        // Two points: straight line (lifted blacks).
        let lut = point_curve_lut(&[[0.0, 40.0], [255.0, 255.0]]).unwrap();
        assert!((eval_lut(&lut, 0.5) - (40.0 + 215.0 * 0.5) / 255.0).abs() < 1e-3);
        // Constant beyond the end points.
        let lut = point_curve_lut(&[[30.0, 20.0], [200.0, 220.0]]).unwrap();
        assert!((eval_lut(&lut, 0.0) - 20.0 / 255.0).abs() < 1e-3);
        assert!((eval_lut(&lut, 1.0) - 220.0 / 255.0).abs() < 1e-3);
        // Natural spline (not monotone Hermite): matches Camera Raw for (64, 96).
        let lut = point_curve_lut(&[[0.0, 0.0], [64.0, 96.0], [255.0, 255.0]]).unwrap();
        let s = Spline::new(&[(0.0, 0.0), (64.0, 96.0), (255.0, 255.0)]);
        assert!((eval_lut(&lut, 0.5) * 255.0 - s.eval(127.5) as f32).abs() < 0.1);
    }

    #[test]
    fn curve_luts_compose_look_then_user() {
        let mut tc = ToneCurve::default();
        let luts = curve_luts(&tc, None);
        assert_eq!(luts, CurveLuts::identity());
        let look = PointCurves { master: vec![[0.0, 0.0], [128.0, 100.0], [255.0, 255.0]], ..Default::default() };
        let luts = curve_luts(&tc, Some((&look, 1.0)));
        assert!(eval_lut(luts.master.as_ref().unwrap(), 0.5) < 0.45);
        let half = curve_luts(&tc, Some((&look, 0.5)));
        let (a, b) = (eval_lut(luts.master.as_ref().unwrap(), 0.5), eval_lut(half.master.as_ref().unwrap(), 0.5));
        assert!(a < b && b < 0.5);
        tc.point.red = vec![[0.0, 0.0], [128.0, 160.0], [255.0, 255.0]];
        let luts = curve_luts(&tc, None);
        assert!(luts.master.is_none() && luts.green.is_none());
        assert!(eval_lut(luts.red.as_ref().unwrap(), 0.5) > 0.55);
    }

    #[test]
    fn calibration_identity_and_directions() {
        let id = calibration_matrix(&CameraCalibration::default());
        assert_eq!(id, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        let cal = CameraCalibration { red: PrimaryCalibration { hue: 0.0, saturation: 50.0 }, ..Default::default() };
        let m = calibration_matrix(&cal);
        // White stays white.
        for r in m {
            assert!((r.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        }
        // A red gets more saturated.
        let red = [0.6f32, 0.2, 0.15];
        let out = [dot(m[0], red), dot(m[1], red), dot(m[2], red)];
        assert!(out[0] - out[1] > red[0] - red[1], "{out:?}");
        // Red hue + moves red towards yellow (more green).
        let cal = CameraCalibration { red: PrimaryCalibration { hue: 60.0, saturation: 0.0 }, ..Default::default() };
        let m = calibration_matrix(&cal);
        let out = [dot(m[0], red), dot(m[1], red), dot(m[2], red)];
        assert!(out[1] > red[1] && out[2] < red[2] + 0.02, "{out:?}");
    }

    #[test]
    fn shadow_tint_and_gray_mix() {
        let grey = [0.01f32, 0.01, 0.01];
        let m = shadow_tint(grey, 50.0);
        assert!(m[0] > m[1] && m[2] > m[1], "magenta: {m:?}");
        let g = shadow_tint(grey, -50.0);
        assert!(g[1] > g[0], "green: {g:?}");
        let bright = [0.9f32, 0.9, 0.9];
        let b = shadow_tint(bright, 50.0);
        assert!((b[0] - b[1]).abs() < 1e-3, "highlights untouched");
        // Gray mix: neutral = luminance; red slider brightens reds only.
        let red = [0.5f32, 0.1, 0.08];
        let base = gray_mix(red, &HslChannels::default());
        assert!((base - dot(PROPHOTO_Y, red)).abs() < 1e-6);
        let brighter = gray_mix(red, &HslChannels { red: 60.0, ..Default::default() });
        assert!(brighter > base * 1.2);
        let blue_only = gray_mix(red, &HslChannels { blue: 60.0, ..Default::default() });
        assert!((blue_only - base).abs() < base * 0.02);
        assert!((gray_mix(grey, &HslChannels { red: 100.0, ..Default::default() }) - 0.01).abs() < 1e-4);
    }

    #[test]
    fn color_grading_shifts_hue_in_expected_direction() {
        let mid = [0.5f32, 0.5, 0.5];
        assert_eq!(color_grade(mid, &ColorGrading::default()), mid);
        let warm =
            ColorGrading { global: ColorWheel { hue: 30.0, saturation: 50.0, luminance: 0.0 }, ..Default::default() };
        let out = color_grade(mid, &warm);
        assert!(out[0] > out[1] && out[1] > out[2], "orange tint: {out:?}");
        let l = |c: [f32; 3]| c[0] * 0.2627 + c[1] * 0.6780 + c[2] * 0.0593;
        assert!((l(out) - l(mid)).abs() < 1e-4, "luminance kept");
        // Shadows wheel affects dark tones more than bright ones.
        let sh =
            ColorGrading { shadows: ColorWheel { hue: 220.0, saturation: 60.0, luminance: 0.0 }, ..Default::default() };
        let dark = color_grade([0.2, 0.2, 0.2], &sh);
        let bright = color_grade([0.85, 0.85, 0.85], &sh);
        assert!(dark[2] - dark[0] > bright[2] - bright[0], "{dark:?} {bright:?}");
        // Luminance slider.
        let lum =
            ColorGrading { midtones: ColorWheel { hue: 0.0, saturation: 0.0, luminance: 50.0 }, ..Default::default() };
        assert!(l(color_grade(mid, &lum)) > 0.52);
    }

    fn noisy_step(w: usize, h: usize, sigma: f32) -> Vec<f32> {
        let mut out = Vec::with_capacity(w * h * 3);
        let mut s = 12345u64;
        let mut rnd = || {
            // Box-Muller from xorshift.
            let mut u = || {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s >> 11) as f32 / (1u64 << 53) as f32
            };
            let (a, b) = (u().max(1e-7), u());
            (-2.0 * a.ln()).sqrt() * (std::f32::consts::TAU * b).cos()
        };
        for _y in 0..h {
            for x in 0..w {
                let v: f32 = if x < w / 2 { 0.05 } else { 0.4 };
                for _ in 0..3 {
                    let q = v.sqrt() + sigma * rnd();
                    out.push(q.max(0.0).powi(2));
                }
            }
        }
        out
    }

    #[test]
    fn denoise_reduces_noise_and_keeps_edges() {
        let (w, h) = (128, 64);
        let mut px = noisy_step(w, h, 0.02);
        let var = |p: &[f32], x0: usize, x1: usize| {
            let vals: Vec<f32> = (8..h - 8)
                .flat_map(|y| (x0..x1).map(move |x| (y, x)))
                .map(|(y, x)| p[(y * w + x) * 3 + 1].sqrt())
                .collect();
            let m = vals.iter().sum::<f32>() / vals.len() as f32;
            vals.iter().map(|v| (v - m) * (v - m)).sum::<f32>() / vals.len() as f32
        };
        let before = var(&px, 8, w / 2 - 8);
        let nr = NoiseReduction { luminance: 60.0, color: 50.0, ..Default::default() };
        denoise(&mut Working { width: w, height: h, rgb: &mut px }, &nr, 1.0);
        let after = var(&px, 8, w / 2 - 8);
        assert!(after < before * 0.5, "{before} -> {after}");
        // Edge preserved: the step stays sharp.
        let g = |x: usize| px[((h / 2) * w + x) * 3 + 1];
        assert!(g(w / 2 - 3) < 0.1 && g(w / 2 + 2) > 0.3, "{} {}", g(w / 2 - 3), g(w / 2 + 2));
        // Zero amounts: untouched.
        let mut p2 = noisy_step(32, 32, 0.02);
        let orig = p2.clone();
        let off = NoiseReduction { luminance: 0.0, color: 0.0, ..Default::default() };
        denoise(&mut Working { width: 32, height: 32, rgb: &mut p2 }, &off, 1.0);
        assert_eq!(p2, orig);
    }

    #[test]
    fn sharpening_increases_edge_contrast() {
        let (w, h) = (64, 32);
        let mut px: Vec<f32> = (0..w * h)
            .flat_map(|i| {
                let x = i % w;
                let v = 0.3 + 0.4 * smoothstep(28.0, 36.0, x as f32);
                [v, v, v]
            })
            .collect();
        let orig = px.clone();
        let s = Sharpening { amount: 100.0, radius: 1.0, detail: 50.0, masking: 0.0 };
        sharpen(&mut Working { width: w, height: h, rgb: &mut px }, &s, 1.0);
        let row = |p: &[f32], x: usize| p[((h / 2) * w + x) * 3];
        assert!(row(&px, 36) > row(&orig, 36) && row(&px, 28) < row(&orig, 28), "overshoot at the edge");
        assert!((row(&px, 5) - row(&orig, 5)).abs() < 1e-4, "flat areas unchanged");
        let mut flat = vec![0.5f32; 16 * 16 * 3];
        sharpen(&mut Working { width: 16, height: 16, rgb: &mut flat }, &s, 1.0);
        assert!(flat.iter().all(|v| (v - 0.5).abs() < 1e-5));
    }

    #[test]
    fn vignette_darkens_corners_only() {
        let v = PostCropVignette { amount: -60.0, ..Default::default() };
        let (w, h) = (60, 40);
        let mut px = vec![0.6f32; w * h * 3];
        vignette(&mut Working { width: w, height: h, rgb: &mut px }, &v);
        let at = |x: usize, y: usize| px[(y * w + x) * 3];
        assert!(at(0, 0) < 0.5, "{}", at(0, 0));
        assert!((at(30, 20) - 0.6).abs() < 0.01);
        let mut px2 = vec![0.6f32; w * h * 3];
        vignette(
            &mut Working { width: w, height: h, rgb: &mut px2 },
            &PostCropVignette { amount: 60.0, ..Default::default() },
        );
        assert!(px2[0] > 0.65);
    }

    #[test]
    fn grain_is_deterministic_and_zero_mean() {
        let g = Grain { amount: 50.0, size: 25.0, roughness: 50.0 };
        let run = |seed| {
            let mut px = vec![0.5f32; 64 * 64 * 3];
            grain(&mut Working { width: 64, height: 64, rgb: &mut px }, &g, seed, 1.0);
            px
        };
        let a = run(7);
        assert_eq!(a, run(7));
        assert_ne!(a, run(8));
        let mean = a.iter().sum::<f32>() / a.len() as f32;
        assert!((mean - 0.5).abs() < 0.01, "{mean}");
        assert!(a.iter().any(|v| (v - 0.5).abs() > 0.01));
        let mut none = vec![0.5f32; 12];
        grain(&mut Working { width: 2, height: 2, rgb: &mut none }, &Grain::default(), 1, 1.0);
        assert!(none.iter().all(|v| *v == 0.5));
    }

    #[test]
    fn crop_geometry_sizes_and_mapping() {
        let full = crop_geometry(&CropSettings::default(), 6000, 4000, 1);
        assert_eq!((full.width, full.height), (6000, 4000));
        assert_eq!(full.map(0.0, 0.0), (0.0, 0.0));
        assert_eq!(full.map(1.0, 1.0), (1.0, 1.0));
        let rot = crop_geometry(&CropSettings::default(), 6000, 4000, 6);
        assert_eq!((rot.width, rot.height), (4000, 6000));
        // Orientation 6: oriented top-left = un-oriented bottom-left.
        let (x, y) = rot.map(0.0, 0.0);
        assert!((x - 0.0).abs() < 1e-9 && (y - 1.0).abs() < 1e-9, "{x} {y}");
        let c = CropSettings { enabled: true, left: 0.1, top: 0.2, right: 0.6, bottom: 0.7, angle: 0.0 };
        let g = crop_geometry(&c, 6000, 4000, 1);
        assert_eq!((g.width, g.height), (3000, 2000));
        let (x, y) = g.map(0.0, 0.0);
        assert!((x - 0.1).abs() < 1e-6 && (y - 0.2).abs() < 1e-6);
        assert!(g.is_axis_aligned());
        // Straightened: centre fixed, corners rotate.
        let c2 = CropSettings { angle: 5.0, ..c };
        let g2 = crop_geometry(&c2, 6000, 4000, 1);
        // Corner vector (3000, 2000) rotated back by 5 degrees.
        let (sn, cs) = 5f64.to_radians().sin_cos();
        let expect = ((3000.0 * cs + 2000.0 * sn).round() as u32, (-3000.0 * sn + 2000.0 * cs).round() as u32);
        assert_eq!((g2.width, g2.height), expect);
        // The corners land on the given source points.
        let (x0, y0) = g2.map(0.0, 0.0);
        let (x1, y1) = g2.map(1.0, 1.0);
        assert!(
            (x0 - 0.1).abs() < 1e-3 && (y0 - 0.2).abs() < 1e-3 && (x1 - 0.6).abs() < 1e-3 && (y1 - 0.7).abs() < 1e-3
        );
        // Camera Raw reference: AZA06911 (7008 x 4672, -5.74 degrees) renders 6120 x 4080.
        let lr = CropSettings {
            enabled: true,
            left: 0.036395,
            top: 0.131022,
            right: 0.963605,
            bottom: 0.868978,
            angle: -5.74,
        };
        let g3 = crop_geometry(&lr, 7008, 4672, 1);
        assert!((g3.width as i32 - 6120).abs() <= 2 && (g3.height as i32 - 4080).abs() <= 2, "{g3:?}");
        let (cx, cy) = g2.map(0.5, 0.5);
        assert!((cx - 0.35).abs() < 1e-6 && (cy - 0.45).abs() < 1e-6);
        assert!(!g2.is_axis_aligned());
        let (x0, _) = g2.map(1.0, 0.0);
        assert!((x0 - 0.6).abs() > 1e-3, "top-right corner moved by the rotation");
        // Disabled or inverted: whole frame.
        let off = CropSettings { enabled: false, ..c2 };
        assert_eq!(crop_geometry(&off, 100, 50, 1).width, 100);
    }
}
