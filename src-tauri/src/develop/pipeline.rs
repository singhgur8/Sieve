//! Develop pipeline, shared by the preview and the full-resolution export (so it is
//! resolution-independent: spatial radii are relative to the frame or given at full
//! resolution and scaled). Camera Raw's (Lightroom's) processing model, Process Version
//! 2012+, as far as it is observable (see `develop::tone`, `develop::parity`, `profiles`):
//!
//! A. Per pixel: camera RGB x white-balance multipliers (clipped at the sensor white) ->
//!    linear ProPhoto through the camera profile's matrices for this white balance
//!    (`ForwardMatrix` / `ColorMatrix` interpolated by temperature, DNG model) with the
//!    Calibration panel folded in (`develop::camera`).
//! B. Noise reduction on that scene-linear image (`parity::denoise`; banded for exports).
//! C. Local operators on log-luminance fields, as luminance gains (hue/saturation
//!    preserving): Shadows/Highlights = fitted response tables ([`LocalTone`]) at an
//!    adaptation luminance from the whole uncropped source ([`ToneContext`]: two guided-filter
//!    bases on a ~512 px grid + image-adaptive references), clarity/texture, dehaze.
//! D. A pointwise chain evaluated through a shaped 3D LUT built per render (tetrahedral
//!    interpolation; 33^3 drafts, 65^3 previews and exports): profile HueSatMap ->
//!    exposure (+baseline) gain -> profile LookTable -> look HSV table ->
//!    HSL/vibrance/saturation or the B&W mix -> shadow tint -> the global tone sliders
//!    (Whites/Contrast/Blacks, exposure shoulder) composed with the base tone curve and the
//!    look's and the user's parametric + master curves, applied hue-preservingly ("RGB
//!    tone": max/min channels through the curve, the middle one interpolated; so Contrast
//!    and Blacks change saturation like Camera Raw) -> look RGB table -> per-channel point
//!    curves -> colour grading -> output colour space (+ `.cube` LUT on sRGB values).
//! E. On the encoded output: post-crop vignette and grain (fused with D), then capture
//!    sharpening on luminance.
//!
//! Display-referred sources (JPEG/TIFF/...) use the same path without camera profile,
//! baseline exposure and base curve, so neutral settings reproduce the file.

use rayon::prelude::*;

use crate::ipc::types::{Histogram, ParametricAdjustments};
use crate::lut::{Interpolation, Lut};
use crate::profiles::dcp::{self as dcpm, HsvTable};
use crate::profiles::table::{BigTable, RgbTable};

use super::camera::{self, ColorSetup, Profile, PROPHOTO_TO_XYZ};
use super::parity::{self, CurveLuts, Grade, GrainGen, Vignette, Working, PROPHOTO_Y};
use super::source::ColorInfo;
use super::tone::{self, LocalTone, ToneModel, ToneSliders};
use super::wb;

/// Display-referred 8-bit sRGB output.
#[derive(Debug, Clone)]
pub struct RenderedImage {
    pub width: u32,
    pub height: u32,
    /// Interleaved RGB8, orientation applied.
    pub rgb: Vec<u8>,
    pub histogram: Histogram,
}

/// Speed/quality trade-off of a render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    /// Slider drags: 33^3 LUT, no luminance noise reduction.
    Draft,
    /// Editor preview: 65^3 LUT, full processing.
    Preview,
    /// Export: full processing, finer output encoding tables.
    Export,
}

impl Quality {
    fn lut_size(self) -> usize {
        match self {
            Quality::Draft => 33,
            Quality::Preview => 65,
            Quality::Export => 65,
        }
    }
}

/// Where the input sits in the (cropped) frame, for position-dependent stages.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// Top-left of the frame and its size, in this input's pixel units (a region render
    /// covers part of the frame).
    pub frame_x: f32,
    pub frame_y: f32,
    pub frame_w: f32,
    pub frame_h: f32,
    /// Input px per full-resolution frame px.
    pub scale: f32,
}

impl View {
    /// The input is the whole frame at `scale`.
    pub fn whole(width: u32, height: u32, scale: f32) -> Self {
        View { frame_x: 0.0, frame_y: 0.0, frame_w: width as f32, frame_h: height as f32, scale }
    }
}

/// Pipeline input: camera RGB already cropped/resampled/oriented.
pub struct RenderInput<'a> {
    pub width: u32,
    pub height: u32,
    /// Interleaved camera RGB16 (not white balanced; white level 65535).
    pub pixels: &'a [u16],
    pub color: &'a ColorInfo,
    /// Long edge of the whole (cropped) frame in this input's pixel units (radius reference).
    pub frame_long_edge: f32,
    pub view: View,
    pub profile: &'a Profile,
    /// Grain seed (the image id).
    pub seed: u64,
    pub quality: Quality,
    /// Whole-frame local tone context; `None` = compute from this input (whole frame).
    /// Region renders must pass the whole frame's ([`ToneContext::compute`]).
    pub tone: Option<&'a ToneContext>,
}

impl<'a> RenderInput<'a> {
    /// Whole-frame input at full resolution with `profile` (tests, simple callers).
    pub fn simple(width: u32, height: u32, pixels: &'a [u16], color: &'a ColorInfo, profile: &'a Profile) -> Self {
        RenderInput {
            width,
            height,
            pixels,
            color,
            frame_long_edge: width.max(height) as f32,
            view: View::whole(width, height, 1.0),
            profile,
            seed: 0,
            quality: Quality::Preview,
            tone: None,
        }
    }
}

/// Default raw baseline exposure (EV) when the camera is unknown.
pub const BASELINE_EV: f32 = 0.35;
/// log2 of scene mid-grey (0.18).
const LOG2_GREY: f32 = -2.473_931_2;

// Radii as a fraction of the frame's long edge (gaussian sigma).
const SIGMA_MASK: f32 = 0.012;
const SIGMA_TEXTURE: f32 = 0.0022;
const SIGMA_HAZE: f32 = 0.02;

const XYZ_FROM_SRGB: [[f64; 3]; 3] =
    [[0.4124564, 0.3575761, 0.1804375], [0.2126729, 0.7151522, 0.0721750], [0.0193339, 0.1191920, 0.9503041]];

fn mat_mul(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    dcpm::mul_mm(a, b)
}

fn to_f32(m: [[f64; 3]; 3]) -> [[f32; 3]; 3] {
    m.map(|r| r.map(|v| v as f32))
}

#[inline]
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn mat3(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

pub fn srgb_encode(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

pub fn srgb_decode(e: f32) -> f32 {
    if e <= 0.040_45 {
        e / 12.92
    } else {
        ((e + 0.055) / 1.055).powf(2.4)
    }
}

/// Table over a non-negative linear domain `[0, max]` sampled at `max * u^2` (dense near 0).
struct SqrtTable {
    inv_max: f32,
    scale: f32,
    values: Vec<f32>,
}

impl SqrtTable {
    fn new(max: f32, n: usize, f: impl Fn(f32) -> f32) -> Self {
        let values = (0..n)
            .map(|i| {
                let u = i as f32 / (n - 1) as f32;
                f(max * u * u)
            })
            .collect();
        SqrtTable { inv_max: 1.0 / max, scale: (n - 1) as f32, values }
    }

    #[inline]
    fn eval(&self, x: f32) -> f32 {
        let u = if x > 0.0 { (x * self.inv_max).min(1.0).sqrt() } else { 0.0 };
        let t = u * self.scale;
        let i = (t as usize).min(self.values.len() - 2);
        let f = t - i as f32;
        let (a, b) = (self.values[i], self.values[i + 1]);
        a + (b - a) * f
    }
}

/// Uniformly sampled function of EV (local tone deltas).
struct EvTable {
    lo: f32,
    inv_step: f32,
    values: Vec<f32>,
}

impl EvTable {
    #[inline]
    fn eval(&self, x: f32) -> f32 {
        let t = ((x - self.lo) * self.inv_step).max(0.0);
        let last = self.values.len() - 1;
        let i = (t as usize).min(last - 1);
        let f = (t - i as f32).min(1.0);
        self.values[i] + (self.values[i + 1] - self.values[i]) * f
    }
}

// ---------------------------------------------------------------------------
// Blurred fields
// ---------------------------------------------------------------------------

/// Scalar field on a coarse grid, sampled bilinearly at full resolution.
struct Field {
    w: usize,
    h: usize,
    factor: f32,
    data: Vec<f32>,
}

impl Field {
    #[inline]
    fn sample(&self, x: usize, y: usize) -> f32 {
        let gx = ((x as f32 + 0.5) / self.factor - 0.5).clamp(0.0, (self.w - 1) as f32);
        let gy = ((y as f32 + 0.5) / self.factor - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (gx as usize, gy as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = (gx - x0 as f32, gy - y0 as f32);
        let r0 = &self.data[y0 * self.w..];
        let r1 = &self.data[y1 * self.w..];
        let a = r0[x0] + (r0[x1] - r0[x0]) * fx;
        let b = r1[x0] + (r1[x1] - r1[x0]) * fx;
        a + (b - a) * fy
    }
}

/// Block-mean grid with integer `factor`.
struct Grid {
    w: usize,
    h: usize,
    factor: usize,
    data: Vec<f32>,
}

impl Grid {
    fn downsample(&self, g: usize) -> Grid {
        if g <= 1 {
            return Grid { w: self.w, h: self.h, factor: self.factor, data: self.data.clone() };
        }
        let (w, h) = (self.w.div_ceil(g), self.h.div_ceil(g));
        let mut data = vec![0.0f32; w * h];
        data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for (x, out) in row.iter_mut().enumerate() {
                let (mut sum, mut n) = (0.0f32, 0u32);
                for yy in y * g..((y + 1) * g).min(self.h) {
                    for xx in x * g..((x + 1) * g).min(self.w) {
                        sum += self.data[yy * self.w + xx];
                        n += 1;
                    }
                }
                *out = sum / n.max(1) as f32;
            }
        });
        Grid { w, h, factor: self.factor * g, data }
    }

    /// Gaussian-like blur with sigma in full-resolution pixels.
    fn blurred(&self, sigma: f32) -> Field {
        let g = ((sigma / (self.factor as f32 * 3.0)).floor() as usize).max(1);
        let grid = self.downsample(g);
        let data = parity::blur(&grid.data, grid.w, grid.h, sigma / grid.factor as f32);
        Field { w: grid.w, h: grid.h, factor: grid.factor as f32, data }
    }
}

// ---------------------------------------------------------------------------
// Output spaces
// ---------------------------------------------------------------------------

/// Transfer curve (linear -> encoded) of an output colour space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Transfer {
    /// IEC 61966-2-1 (sRGB, Display P3).
    Srgb,
    /// Pure power law with this gamma (Adobe RGB (1998): 563/256).
    Gamma(f32),
}

impl Transfer {
    pub fn encode(self, v: f32) -> f32 {
        let v = v.clamp(0.0, 1.0);
        match self {
            Transfer::Srgb => srgb_encode(v),
            Transfer::Gamma(g) => v.powf(1.0 / g),
        }
    }

    pub fn decode(self, e: f32) -> f32 {
        let e = e.clamp(0.0, 1.0);
        match self {
            Transfer::Srgb => srgb_decode(e),
            Transfer::Gamma(g) => e.powf(g),
        }
    }
}

/// An RGB output colour space (D65 white).
#[derive(Debug, Clone, PartialEq)]
pub struct OutputSpace {
    /// XYZ (D65) -> linear target RGB.
    pub from_xyz: [[f64; 3]; 3],
    /// Target RGB -> XYZ (D65), the ICC colorants before chromatic adaptation.
    pub to_xyz: [[f64; 3]; 3],
    pub transfer: Transfer,
    /// The target is sRGB (a LUT's sRGB output is used as is).
    pub is_srgb: bool,
}

/// D65 white point (xy, as used by sRGB / Display P3 / Adobe RGB / Rec.2020).
pub const D65_XY: [f64; 2] = [0.3127, 0.3290];

/// RGB -> XYZ matrix for `primaries` (xy of R, G, B) and `white` (xy), Y of white = 1.
pub fn rgb_to_xyz(primaries: [[f64; 2]; 3], white: [f64; 2]) -> [[f64; 3]; 3] {
    let xyz = |p: [f64; 2]| [p[0] / p[1], 1.0, (1.0 - p[0] - p[1]) / p[1]];
    let (r, g, b) = (xyz(primaries[0]), xyz(primaries[1]), xyz(primaries[2]));
    let m = [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]];
    let wv = xyz(white);
    let inv = wb::invert3(&m).expect("primaries are independent");
    let s: Vec<f64> = (0..3).map(|i| (0..3).map(|j| inv[i][j] * wv[j]).sum()).collect();
    let mut out = m;
    for row in out.iter_mut() {
        for (j, v) in row.iter_mut().enumerate() {
            *v *= s[j];
        }
    }
    out
}

impl OutputSpace {
    pub fn srgb() -> Self {
        let from_xyz = wb::invert3(&XYZ_FROM_SRGB).expect("invertible");
        OutputSpace { from_xyz, to_xyz: XYZ_FROM_SRGB, transfer: Transfer::Srgb, is_srgb: true }
    }

    /// Any D65 RGB space from its primaries.
    pub fn from_primaries(primaries: [[f64; 2]; 3], transfer: Transfer) -> Self {
        let to_xyz = rgb_to_xyz(primaries, D65_XY);
        let from_xyz = wb::invert3(&to_xyz).expect("invertible");
        OutputSpace { from_xyz, to_xyz, transfer, is_srgb: false }
    }
}

fn d50_to_d65() -> [[f64; 3]; 3] {
    dcpm::bradford(dcpm::xy_to_xyz(dcpm::D50_XY), dcpm::xy_to_xyz((D65_XY[0], D65_XY[1])))
}

/// Linear ProPhoto (D50) -> linear RGB of `space` (D65).
fn prophoto_to(space: &OutputSpace) -> [[f64; 3]; 3] {
    mat_mul(&space.from_xyz, &mat_mul(&d50_to_d65(), &PROPHOTO_TO_XYZ))
}

/// Out-of-gamut (negative) components desaturated towards the luminance `luma . s`.
#[inline(always)]
fn gamut_map(s: [f32; 3], luma: [f32; 3]) -> [f32; 3] {
    let mn = s[0].min(s[1]).min(s[2]);
    if mn < 0.0 {
        let ys = dot(luma, s).max(0.0);
        let t = ys / (ys - mn);
        [ys + (s[0] - ys) * t, ys + (s[1] - ys) * t, ys + (s[2] - ys) * t]
    } else {
        s
    }
}

/// Blends the LUT result of `e` by `a`.
#[inline(always)]
fn apply_cube(e: [f32; 3], lut: &Lut, a: f32) -> [f32; 3] {
    let m = lut.eval(e, Interpolation::Tetrahedral);
    [e[0] + (m[0] - e[0]) * a, e[1] + (m[1] - e[1]) * a, e[2] + (m[2] - e[2]) * a]
}

// ---------------------------------------------------------------------------
// The pointwise chain
// ---------------------------------------------------------------------------

/// RGB-table primaries (DNG SDK `dng_rgb_table::primaries_*`) -> (RGB -> XYZ D65, is D50).
fn table_space(primaries: u32) -> ([[f64; 3]; 3], bool) {
    match primaries {
        1 => (rgb_to_xyz([[0.64, 0.33], [0.21, 0.71], [0.15, 0.06]], D65_XY), false),
        2 => (PROPHOTO_TO_XYZ, true),
        3 => (rgb_to_xyz([[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]], D65_XY), false),
        4 => (rgb_to_xyz([[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]], D65_XY), false),
        _ => (XYZ_FROM_SRGB, false),
    }
}

fn table_encode(gamma: u32, v: f32) -> f32 {
    let v = v.max(0.0);
    match gamma {
        0 => v,
        1 => srgb_encode(v.min(1.0)),
        2 => v.powf(1.0 / 1.8),
        3 => v.powf(1.0 / 2.2),
        _ => {
            // Rec. 2020 OETF.
            if v < 0.018_053_97 {
                4.5 * v
            } else {
                1.099_296_8 * v.powf(0.45) - 0.099_296_8
            }
        }
    }
}

fn table_decode(gamma: u32, e: f32) -> f32 {
    let e = e.max(0.0);
    match gamma {
        0 => e,
        1 => srgb_decode(e.min(1.0)),
        2 => e.powf(1.8),
        3 => e.powf(2.2),
        _ => {
            if e < 0.081_242_86 {
                e / 4.5
            } else {
                ((e + 0.099_296_8) / 1.099_296_8).powf(1.0 / 0.45)
            }
        }
    }
}

struct RgbLook<'a> {
    table: &'a RgbTable,
    amount: f32,
    to_table: [[f32; 3]; 3],
    from_table: [[f32; 3]; 3],
}

impl RgbLook<'_> {
    fn apply(&self, lin_pp: [f32; 3]) -> [f32; 3] {
        let t = mat3(&self.to_table, lin_pp);
        let clip = self.table.gamut == 0;
        let enc = t.map(|v| table_encode(self.table.gamma, if clip { v.clamp(0.0, 1.0) } else { v }));
        let m = self.table.eval(enc);
        let out_enc = [0, 1, 2].map(|k| enc[k] + (m[k] - enc[k]) * self.amount);
        let lin = out_enc.map(|e| table_decode(self.table.gamma, e));
        mat3(&self.from_table, lin)
    }
}

/// HSL strengths at +-100 fitted to Camera Raw renders of colour patches (`tools/acr-oracle`).
const HSL_HUE_DEG: f32 = 37.5;
const HSL_SAT_GAIN: f32 = 1.15;
const HSL_LUM_EV: f32 = 5.0;

/// HSL / vibrance / saturation, evaluated in Oklab (smooth everywhere, so the per-render
/// LUT interpolates it well): hue = Oklab hue angle with Lightroom's eight bands placed at
/// the Oklab hues of the sRGB colours they are named after, chroma scaled for saturation,
/// band luminance as an exposure-like gain.
struct ColorOps {
    saturation: f32,
    vibrance: f32,
    hsl: Option<[[f32; 8]; 3]>,
    lab: &'static parity::Oklab,
}

impl ColorOps {
    fn new(adj: &ParametricAdjustments) -> Self {
        let h = &adj.hsl;
        let hsl = [parity::bands(&h.hue), parity::bands(&h.saturation), parity::bands(&h.luminance)]
            .map(|b| b.map(|v| v / 100.0));
        let hsl = hsl.iter().flatten().any(|v| *v != 0.0).then_some(hsl);
        ColorOps {
            saturation: 1.0 + adj.saturation / 100.0,
            vibrance: adj.vibrance / 100.0,
            hsl,
            lab: parity::Oklab::get(),
        }
    }

    fn is_identity(&self) -> bool {
        self.saturation == 1.0 && self.vibrance == 0.0 && self.hsl.is_none()
    }

    /// Applies to linear ProPhoto.
    fn apply(&self, v: [f32; 3]) -> [f32; 3] {
        let lab = self.lab;
        let [l, a, b] = lab.from_prophoto(v);
        let c = (a * a + b * b).sqrt();
        if c < 1e-7 {
            return v;
        }
        let hue = b.atan2(a).to_degrees().rem_euclid(360.0);
        // Colourfulness 0..1 (the sRGB primaries sit around 0.25-0.32 chroma).
        let sat = (c / 0.28).min(1.0);
        let near_neutral = parity::smoothstep(0.0, 0.06, c);
        let mut chroma = self.saturation;
        if self.vibrance > 0.0 {
            // Protect skin (orange band) and already saturated colours.
            let skin = 1.0 - 0.5 * lab.band_weight(hue, 1);
            chroma *= 1.0 + self.vibrance * (1.0 - sat) * (1.0 - sat) * 1.2 * skin;
        } else if self.vibrance < 0.0 {
            chroma *= 1.0 + self.vibrance * (1.0 - 0.5 * sat);
        }
        let mut theta = 0.0f32;
        let mut lum_ev = 0.0;
        if let Some(hsl) = &self.hsl {
            let (i, j, t) = lab.bands(hue);
            let pick = |vals: &[f32; 8]| vals[i] + (vals[j] - vals[i]) * t;
            let (dh, ds, dl) = (pick(&hsl[0]), pick(&hsl[1]), pick(&hsl[2]));
            theta = dh * HSL_HUE_DEG.to_radians() * near_neutral;
            chroma *= (1.0 + ds * HSL_SAT_GAIN * near_neutral).max(0.0);
            lum_ev = dl * HSL_LUM_EV * (sat * 1.5).min(1.0);
        }
        let (sn, cs) = theta.sin_cos();
        let (a2, b2) = ((a * cs - b * sn) * chroma, (a * sn + b * cs) * chroma);
        let mut out = lab.to_prophoto([l, a2, b2]);
        if lum_ev != 0.0 {
            let g = lum_ev.exp2();
            out = out.map(|c| c * g);
        }
        let mn = out[0].min(out[1]).min(out[2]);
        if mn < 0.0 {
            let yo = dot(PROPHOTO_Y, out).max(0.0);
            let t = yo / (yo - mn);
            out = out.map(|c| yo + (c - yo) * t);
        }
        out
    }
}

/// Output stage (per pixel, after the LUT): display-linear ProPhoto -> encoded target.
struct Output<'a> {
    /// Linear ProPhoto -> linear target.
    to_target: [[f32; 3]; 3],
    luma: [f32; 3],
    /// Target transfer (linear -> encoded), tabulated.
    encode: SqrtTable,
    transfer: Transfer,
    cube: Option<(&'a Lut, f32)>,
    /// Linear ProPhoto -> linear sRGB (cube LUTs take sRGB input).
    to_srgb: [[f32; 3]; 3],
    srgb_encode: SqrtTable,
    srgb_to_target: [[f32; 3]; 3],
    is_srgb: bool,
}

impl<'a> Output<'a> {
    fn new(space: &OutputSpace, cube: Option<(&'a Lut, f32)>, quality: Quality) -> Self {
        let srgb = OutputSpace::srgb();
        let n = if quality == Quality::Export { 1 << 16 } else { 1 << 13 };
        let transfer = space.transfer;
        Output {
            to_target: to_f32(prophoto_to(space)),
            luma: to_f32(space.to_xyz)[1],
            encode: SqrtTable::new(1.0, n, |v| transfer.encode(v)),
            transfer,
            cube,
            to_srgb: to_f32(prophoto_to(&srgb)),
            srgb_encode: SqrtTable::new(1.0, n, srgb_encode),
            srgb_to_target: to_f32(mat_mul(&space.from_xyz, &XYZ_FROM_SRGB)),
            is_srgb: space.is_srgb,
        }
    }

    #[inline]
    fn apply(&self, lin_pp: [f32; 3]) -> [f32; 3] {
        match self.cube {
            Some((lut, a)) => {
                let s = gamut_map(mat3(&self.to_srgb, lin_pp), LUMA_709);
                let e = apply_cube(s.map(|c| self.srgb_encode.eval(c.min(1.0))), lut, a);
                if self.is_srgb {
                    e
                } else {
                    let lin = e.map(|c| srgb_decode(c.clamp(0.0, 1.0)));
                    let t = gamut_map(mat3(&self.srgb_to_target, lin), self.luma);
                    t.map(|c| self.transfer.encode(c))
                }
            }
            None => {
                let t = gamut_map(mat3(&self.to_target, lin_pp), self.luma);
                t.map(|c| self.encode.eval(c.min(1.0)))
            }
        }
    }
}

const LUMA_709: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// Everything after the local operators, per pixel.
struct Chain<'a> {
    hsm: Option<&'a HsvTable>,
    /// Exposure (+ baseline) as a linear gain; the rest of the global tone is in `curve`.
    exposure_gain: f32,
    display_referred: bool,
    dcp_look: Option<&'a HsvTable>,
    look_hsv: Option<(&'a HsvTable, f32)>,
    look_rgb: Option<RgbLook<'a>>,
    bw: Option<crate::ipc::types::HslChannels>,
    color: ColorOps,
    shadow_tint: f32,
    /// Global tone sliders (Whites, Contrast, Blacks, the exposure shoulder) + base curve +
    /// master curves: post-exposure linear -> display linear, applied as Camera Raw's
    /// hue-preserving RGB tone (so Contrast/Blacks change saturation like an RGB curve).
    curve: SqrtTable,
    rgb_curves: [Option<Vec<f32>>; 3],
    grade: Grade,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl Chain<'_> {
    #[inline]
    fn rgb_tone(&self, v: [f32; 3]) -> [f32; 3] {
        let k = |x: f32| self.curve.eval(x);
        let [r, g, b] = v;
        // Sort to (hi, mid, lo), map hi/lo through the curve, interpolate mid.
        let tone3 = |hi: f32, mid: f32, lo: f32| -> (f32, f32, f32) {
            let (h2, l2) = (k(hi), k(lo));
            let m2 = if hi > lo { l2 + (h2 - l2) * (mid - lo) / (hi - lo) } else { h2 };
            (h2, m2, l2)
        };
        if r >= g {
            if g >= b {
                let (h, m, l) = tone3(r, g, b);
                [h, m, l]
            } else if b >= r {
                let (h, m, l) = tone3(b, r, g);
                [m, l, h]
            } else {
                let (h, m, l) = tone3(r, b, g);
                [h, l, m]
            }
        } else if r >= b {
            let (h, m, l) = tone3(g, r, b);
            [m, h, l]
        } else if b >= g {
            let (h, m, l) = tone3(b, g, r);
            [l, m, h]
        } else {
            let (h, m, l) = tone3(g, b, r);
            [l, h, m]
        }
    }

    /// `v0`: linear ProPhoto after the local operators (pre-exposure domain).
    fn eval(&self, v0: [f32; 3]) -> [f32; 3] {
        let mut v = v0.map(|c| c.max(0.0));
        if let Some(t) = self.hsm {
            v = t.apply(v, 1.0);
        }
        v = v.map(|c| c * self.exposure_gain);
        if let Some(t) = self.dcp_look {
            v = t.apply(v, 1.0);
        }
        if let Some((t, a)) = self.look_hsv {
            v = t.apply(v, a);
        }
        if let Some(mix) = &self.bw {
            let g = parity::gray_mix(v, mix);
            v = [g; 3];
        } else if !self.color.is_identity() {
            v = self.color.apply(v);
        }
        if self.shadow_tint != 0.0 {
            v = parity::shadow_tint(v, self.shadow_tint);
        }
        v = self.rgb_tone(v);
        if let Some(l) = &self.look_rgb {
            v = l.apply(v);
        }
        let mut e = v.map(|c| srgb_encode(c.clamp(0.0, 1.0)));
        for (k, lut) in self.rgb_curves.iter().enumerate() {
            if let Some(lut) = lut {
                e[k] = parity::eval_lut(lut, e[k]);
            }
        }
        e = self.grade.apply(e);
        let _ = self.display_referred;
        // Display-linear ProPhoto; the output stage runs per pixel after the LUT.
        e.map(|c| srgb_decode(c.clamp(0.0, 1.0)))
    }
}

/// The RGB tone curve of the global sliders: post-exposure linear `c` (exposure
/// `exposure` EV, baseline included) -> `finish` (base + master curves) of the tone model's
/// output, `2^global(log2(c) - exposure)`.
fn global_tone_curve(model: &ToneModel, exposure: f32, finish: impl Fn(f32) -> f32) -> SqrtTable {
    let max = (LUT_LOG_MAX + exposure.max(0.0)).exp2();
    SqrtTable::new(max, 8192, |c| {
        if c <= 0.0 {
            return finish(0.0);
        }
        finish(model.global(c.log2() - exposure).exp2())
    })
}

/// Settings after merging the look's own parameters (at the look amount) under the user's.
fn effective(adj: &ParametricAdjustments, profile: &Profile) -> ParametricAdjustments {
    let mut a = adj.clone();
    let Some(look) = &profile.look else { return a };
    let t = profile.look_amount;
    let p = &look.parameters;
    let add = |x: &mut f32, y: f32, lo: f32, hi: f32| *x = (*x + y * t).clamp(lo, hi);
    add(&mut a.exposure, p.exposure, -5.0, 5.0);
    for (x, y) in [
        (&mut a.contrast, p.contrast),
        (&mut a.highlights, p.highlights),
        (&mut a.shadows, p.shadows),
        (&mut a.whites, p.whites),
        (&mut a.blacks, p.blacks),
        (&mut a.texture, p.texture),
        (&mut a.clarity, p.clarity),
        (&mut a.dehaze, p.dehaze),
        (&mut a.vibrance, p.vibrance),
        (&mut a.saturation, p.saturation),
    ] {
        add(x, y, -100.0, 100.0);
    }
    if look.monochrome || p.black_and_white.enabled {
        a.black_and_white.enabled = true;
    }
    a
}

/// A per-render shaped 3D LUT over linear ProPhoto (pre-exposure domain).
struct Lut3 {
    n: usize,
    inv_step: f32,
    data: Vec<[f32; 3]>,
}

const LUT_LOG_A: f32 = -13.0;
const LUT_LOG_MAX: f32 = 4.0;

impl Lut3 {
    fn build(chain: &Chain, n: usize) -> Lut3 {
        let a = LUT_LOG_A.exp2();
        let step = ((LUT_LOG_MAX.exp2() + a).log2() - LUT_LOG_A) / (n - 1) as f32;
        let node = |i: usize| ((LUT_LOG_A + i as f32 * step).exp2() - a).max(0.0);
        let coords: Vec<f32> = (0..n).map(node).collect();
        let mut data = vec![[0.0f32; 3]; n * n * n];
        data.par_chunks_mut(n * n).enumerate().for_each(|(r, plane)| {
            for g in 0..n {
                for b in 0..n {
                    plane[g * n + b] = chain.eval([coords[r], coords[g], coords[b]]);
                }
            }
        });
        Lut3 { n, inv_step: 1.0 / step, data }
    }

    #[inline(always)]
    fn coord(&self, x: f32) -> f32 {
        let a = LUT_LOG_A.exp2();
        (((x.max(0.0) + a).log2() - LUT_LOG_A) * self.inv_step).clamp(0.0, (self.n - 1) as f32)
    }

    /// Tetrahedral interpolation.
    #[inline(always)]
    fn eval(&self, v: [f32; 3]) -> [f32; 3] {
        let n = self.n;
        let (fr, fg, fb) = (self.coord(v[0]), self.coord(v[1]), self.coord(v[2]));
        let (r0, g0, b0) = ((fr as usize).min(n - 2), (fg as usize).min(n - 2), (fb as usize).min(n - 2));
        let (dr, dg, db) = (fr - r0 as f32, fg - g0 as f32, fb - b0 as f32);
        let idx = |r: usize, g: usize, b: usize| (r * n + g) * n + b;
        let c000 = self.data[idx(r0, g0, b0)];
        let c111 = self.data[idx(r0 + 1, g0 + 1, b0 + 1)];
        let (w0, w1, w2, w3, p1, p2);
        if dr >= dg {
            if dg >= db {
                p1 = self.data[idx(r0 + 1, g0, b0)];
                p2 = self.data[idx(r0 + 1, g0 + 1, b0)];
                (w0, w1, w2, w3) = (1.0 - dr, dr - dg, dg - db, db);
            } else if dr >= db {
                p1 = self.data[idx(r0 + 1, g0, b0)];
                p2 = self.data[idx(r0 + 1, g0, b0 + 1)];
                (w0, w1, w2, w3) = (1.0 - dr, dr - db, db - dg, dg);
            } else {
                p1 = self.data[idx(r0, g0, b0 + 1)];
                p2 = self.data[idx(r0 + 1, g0, b0 + 1)];
                (w0, w1, w2, w3) = (1.0 - db, db - dr, dr - dg, dg);
            }
        } else if db >= dg {
            p1 = self.data[idx(r0, g0, b0 + 1)];
            p2 = self.data[idx(r0, g0 + 1, b0 + 1)];
            (w0, w1, w2, w3) = (1.0 - db, db - dg, dg - dr, dr);
        } else if db >= dr {
            p1 = self.data[idx(r0, g0 + 1, b0)];
            p2 = self.data[idx(r0, g0 + 1, b0 + 1)];
            (w0, w1, w2, w3) = (1.0 - dg, dg - db, db - dr, dr);
        } else {
            p1 = self.data[idx(r0, g0 + 1, b0)];
            p2 = self.data[idx(r0 + 1, g0 + 1, b0)];
            (w0, w1, w2, w3) = (1.0 - dg, dg - dr, dr - db, db);
        }
        [0, 1, 2].map(|k| w0 * c000[k] + w1 * p1[k] + w2 * p2[k] + w3 * c111[k])
    }
}

/// Output of [`develop`]: encoded values in the target's transfer curve.
struct Developed {
    width: usize,
    height: usize,
    /// Interleaved RGB, encoded (0..=1 nominal).
    rgb: Vec<f32>,
}

/// Local operator constants.
struct Local {
    /// Shadows/Highlights: delta (EV) at the adaptation luminance (pre-exposure EV).
    tone_local: Option<EvTable>,
    clarity: f32,
    texture: f32,
    dehaze: f32,
}

/// Runs the whole pipeline to encoded floats in `space`'s transfer (sRGB for previews).
fn develop(
    input: &RenderInput,
    adjustments: &ParametricAdjustments,
    cube: Option<&Lut>,
    space: &OutputSpace,
    quality: Quality,
) -> Developed {
    let profile = input.profile;
    let adj = effective(adjustments, profile);
    let (w, h) = (input.width as usize, input.height as usize);
    let setup: ColorSetup = camera::color_setup(input.color, profile, &adj.white_balance, &adj.calibration);
    let scale = input.view.scale.max(1e-3);

    // A. camera -> linear ProPhoto (pre-exposure; neutral clip = 1).
    let mut rgb = to_working(input, &setup);

    // B. Noise reduction.
    {
        let mut work = Working { width: w, height: h, rgb: &mut rgb };
        parity::denoise_opts(&mut work, &adj.detail.noise_reduction, scale, quality != Quality::Draft);
    }

    // C. Local operators.
    let (sh, hl) = (adj.shadows, adj.highlights);
    let tone_sliders = ToneSliders {
        exposure: adj.exposure + profile.baseline_ev,
        contrast: adj.contrast,
        highlights: hl,
        shadows: sh,
        whites: adj.whites,
        blacks: adj.blacks,
    };
    let local_tone = LocalTone::new(sh, hl);
    let mut local = Local {
        tone_local: None,
        clarity: adj.clarity / 100.0 * 0.6,
        texture: adj.texture / 100.0 * 0.8,
        dehaze: adj.dehaze / 100.0,
    };
    let edge = input.frame_long_edge.max(1.0);
    let need_base = !local_tone.is_identity();
    // Whole-frame adaptation (given by region renders; else computed from this input).
    let own_ctx = (need_base && input.tone.is_none()).then(|| ToneContext::from_working(&rgb, w, h));
    let ctx = input.tone.or(own_ctx.as_ref());
    if let Some(c) = ctx.filter(|_| need_base) {
        let (rs, rh) = LocalTone::references(&c.stats, adj.exposure + profile.baseline_ev);
        const LO: f32 = -32.0;
        const N: usize = 1281;
        let step = 40.0 / (N - 1) as f32;
        let values = local_tone.deltas(rs, rh, LO, step, N);
        local.tone_local = Some(EvTable { lo: LO, inv_step: 1.0 / step, values });
    }
    let need_clar = local.clarity != 0.0;
    let need_tex = local.texture != 0.0;
    let need_haze = local.dehaze > 0.0;
    let (clar, tex, haze) = if need_clar || need_tex || need_haze {
        let f0 = if w.min(h) >= 1024 {
            4
        } else if w.min(h) >= 256 {
            2
        } else {
            1
        };
        let (lum, dark) = base_grids(&rgb, w, h, f0, need_haze);
        (
            need_clar.then(|| lum.blurred(SIGMA_MASK * edge)),
            need_tex.then(|| lum.blurred(SIGMA_TEXTURE * edge)),
            dark.map(|d| d.blurred(SIGMA_HAZE * edge)),
        )
    } else {
        (None, None, None)
    };

    // D. The pointwise chain through a 3D LUT.
    let look = profile.look.as_deref();
    let look_amount = profile.look_amount;
    let look_hsv = look.and_then(|l| {
        l.tables.iter().find_map(|t| match t {
            BigTable::Look(h) => Some((h, look_amount)),
            BigTable::Rgb(_) => None,
        })
    });
    let look_rgb = look.and_then(|l| {
        l.tables.iter().find_map(|t| match t {
            BigTable::Rgb(r) => {
                let (to_xyz, d50) = table_space(r.primaries);
                let pp_xyz = if d50 { PROPHOTO_TO_XYZ } else { mat_mul(&d50_to_d65(), &PROPHOTO_TO_XYZ) };
                let from_xyz = wb::invert3(&to_xyz)?;
                let to_table = mat_mul(&from_xyz, &pp_xyz);
                let from_table = wb::invert3(&to_table)?;
                Some(RgbLook {
                    table: r,
                    amount: (look_amount * l.rgb_table_amount).clamp(r.min_amount.min(0.0), r.max_amount.max(1.0)),
                    to_table: to_f32(to_table),
                    from_table: to_f32(from_table),
                })
            }
            BigTable::Look(_) => None,
        })
    });
    let luts: CurveLuts =
        parity::curve_luts(&adj.tone_curve, look.map(|l| (&l.parameters.tone_curve.point, look_amount.min(1.0))));
    let tone_curve = profile.dcp.as_ref().and_then(|d| d.tone_curve.clone());
    let display = profile.display_referred;
    let base_fn = tone::profile_curve(tone_curve.as_deref());
    let master = luts.master.clone();
    let tone_model = ToneModel::new(ToneSliders { shadows: 0.0, highlights: 0.0, ..tone_sliders });
    let curve = global_tone_curve(&tone_model, tone_sliders.exposure, |x| {
        let t = if display { x.clamp(0.0, 1.0) } else { base_fn(x) };
        match &master {
            Some(lut) => srgb_decode(parity::eval_lut(lut, srgb_encode(t))),
            None => t,
        }
    });
    let bw = adj.black_and_white.enabled.then_some(adj.black_and_white.mixer);
    let lut_amount = adj.lut.as_ref().map_or(0.0, |l| (l.amount / 100.0).clamp(0.0, 1.0));
    let out = Output::new(space, cube.filter(|_| lut_amount > 0.0).map(|l| (l, lut_amount)), quality);
    let chain = Chain {
        hsm: setup.hsm.as_ref(),
        exposure_gain: tone_sliders.exposure.exp2(),
        display_referred: display,
        dcp_look: profile.dcp.as_ref().and_then(|d| d.look_table.as_ref()),
        look_hsv,
        look_rgb,
        bw,
        color: ColorOps::new(&adj),
        shadow_tint: adj.calibration.shadow_tint,
        curve,
        rgb_curves: [luts.red, luts.green, luts.blue],
        grade: Grade::new(&adj.color_grading),
        _marker: std::marker::PhantomData,
    };
    let lut = Lut3::build(&chain, quality.lut_size());

    // Position-dependent stages fused with D.
    let view = input.view;
    let vig = Vignette::new(&adj.effects.vignette);
    let grain = GrainGen::new(&adj.effects.grain, input.seed);
    let aspect = view.frame_w / view.frame_h.max(1e-3);
    let foot = 1.0 / scale;
    rgb.par_chunks_mut(w * 3).enumerate().for_each(|(y, row)| {
        let fy = (y as f32 + 0.5 - view.frame_y) / view.frame_h.max(1e-3);
        for x in 0..w {
            let p = &mut row[x * 3..x * 3 + 3];
            let mut v = [p[0], p[1], p[2]];
            let adapt = match (&local.tone_local, ctx) {
                (Some(_), Some(c)) => Some(c.sample((x as f32 + 0.5 - view.frame_x) / view.frame_w.max(1e-3), fy)),
                _ => None,
            };
            if adapt.is_some() || clar.is_some() || tex.is_some() || haze.is_some() || local.dehaze < 0.0 {
                v = apply_local(v, x, y, &local, adapt, clar.as_ref(), tex.as_ref(), haze.as_ref());
            }
            let mut e = out.apply(lut.eval(v));
            if vig.is_some() || grain.is_some() {
                let fx = (x as f32 + 0.5 - view.frame_x) / view.frame_w.max(1e-3);
                if let Some(vg) = &vig {
                    e = vg.apply(e, vg.mask(fx, fy, aspect));
                }
                if let Some(g) = &grain {
                    let n = g.noise(fx * view.frame_w * foot, fy * view.frame_h * foot, foot);
                    e = g.apply(e, n);
                }
            }
            p.copy_from_slice(&e);
        }
    });

    // E. Capture sharpening on the encoded luminance.
    {
        let mut work = Working { width: w, height: h, rgb: &mut rgb };
        parity::sharpen(&mut work, &adj.detail.sharpening, scale);
    }
    Developed { width: w, height: h, rgb }
}

/// Camera RGB16 -> white-balanced linear ProPhoto (pre-exposure; neutral clip = 1).
fn to_working(input: &RenderInput, setup: &ColorSetup) -> Vec<f32> {
    let (w, h) = (input.width as usize, input.height as usize);
    let mul = setup.mul.map(|m| m / 65535.0);
    let m = setup.m;
    let mut rgb = vec![0.0f32; w * h * 3];
    rgb.par_chunks_mut(w * 3).zip(input.pixels.par_chunks(w * 3)).for_each(|(out, inp)| {
        for (o, p) in out.as_chunks_mut::<3>().0.iter_mut().zip(inp.as_chunks::<3>().0) {
            let c = [
                (f32::from(p[0]) * mul[0]).min(1.0),
                (f32::from(p[1]) * mul[1]).min(1.0),
                (f32::from(p[2]) * mul[2]).min(1.0),
            ];
            o.copy_from_slice(&mat3(&m, c));
        }
    });
    rgb
}

/// Calibration aid (`tools/acr-oracle`): log2 of the scene luminance (linear ProPhoto Y,
/// pre-exposure, neutral clip = 0 EV) that the local tone operators see, per pixel.
pub fn scene_log_luminance(input: &RenderInput, adjustments: &ParametricAdjustments) -> Vec<f32> {
    let adj = effective(adjustments, input.profile);
    let setup = camera::color_setup(input.color, input.profile, &adj.white_balance, &adj.calibration);
    to_working(input, &setup).as_chunks::<3>().0.iter().map(|v| dot(PROPHOTO_Y, *v).max(1e-9).log2()).collect()
}

/// Scene log2-luminance statistics of the whole (uncropped) source, pre-exposure EV.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ToneStats {
    pub key: f32,
    pub p50: f32,
    pub p90: f32,
    pub p95: f32,
    pub p99: f32,
    pub white: f32,
}

/// Context of the local tone operator (Shadows / Highlights, [`LocalTone`]), computed on
/// the whole *uncropped, un-oriented* source like Camera Raw (a crop or zoomed region is
/// adapted exactly as in the full frame): log-luminance statistics for the image reference
/// and the adaptation luminance, an edge-aware multi-scale base of the scene log2
/// luminance (two self-guided filters with box radii `RADIUS_FINE` / `RADIUS_COARSE` px
/// per 2048 px of the source's long edge) on a ~512 px grid. Render pixels are mapped into
/// it through the crop geometry ([`ToneContext::with_crop`]).
#[derive(Debug, Clone)]
pub struct ToneContext {
    pub stats: ToneStats,
    fine: Vec<f32>,
    coarse: Vec<f32>,
    bw: usize,
    bh: usize,
    /// Render frame (normalized, oriented, cropped) -> context grid (normalized).
    map: [f32; 6],
}

/// Grid long edge of the adaptation field (and size of the source it is computed from).
pub const TONE_GRID: u32 = 512;

impl ToneContext {
    /// Context from `input` = the whole uncropped, un-oriented source (any size; ~512 px
    /// long edge is plenty) for `adjustments` (white balance / profile as rendered). The
    /// map is the identity until [`Self::with_crop`].
    pub fn compute(input: &RenderInput, adjustments: &ParametricAdjustments) -> ToneContext {
        let adj = effective(adjustments, input.profile);
        let setup = camera::color_setup(input.color, input.profile, &adj.white_balance, &adj.calibration);
        let rgb = to_working(input, &setup);
        ToneContext::from_working(&rgb, input.width as usize, input.height as usize)
    }

    /// Maps render frames of `crop` + EXIF `orientation` (un-oriented source `src_w` x
    /// `src_h`) into this context (which must be of the un-oriented, uncropped source).
    pub fn with_crop(
        mut self,
        crop: &crate::ipc::types::CropSettings,
        src_w: u32,
        src_h: u32,
        orientation: u8,
    ) -> Self {
        let g = parity::crop_geometry(crop, src_w, src_h, orientation);
        self.map = g.to_source.map(|v| v as f32);
        self
    }

    fn from_working(rgb: &[f32], w: usize, h: usize) -> ToneContext {
        use super::local_tone_data as ld;
        const LO: f32 = -32.0;
        const BINS_PER_EV: f32 = 16.0;
        const NB: usize = 36 * 16;
        let f = w.max(h).div_ceil(TONE_GRID as usize).max(1);
        let (gw, gh) = (w.div_ceil(f), h.div_ceil(f));
        // Block means of log2 Y + a histogram / sum of the per-pixel values.
        let rows: Vec<(Vec<f32>, Vec<u32>, f64)> = (0..gh)
            .into_par_iter()
            .map(|gy| {
                let mut sums = vec![0.0f32; gw];
                let mut counts = vec![0u32; gw];
                let mut hist = vec![0u32; NB];
                let mut total = 0.0f64;
                for y in gy * f..((gy + 1) * f).min(h) {
                    let row = &rgb[y * w * 3..(y + 1) * w * 3];
                    for (x, v) in row.as_chunks::<3>().0.iter().enumerate() {
                        let e = dot(PROPHOTO_Y, *v).max(1e-9).log2();
                        sums[x / f] += e;
                        counts[x / f] += 1;
                        total += f64::from(e);
                        hist[(((e - LO) * BINS_PER_EV).max(0.0) as usize).min(NB - 1)] += 1;
                    }
                }
                let means = sums.iter().zip(&counts).map(|(s, c)| s / (*c).max(1) as f32).collect();
                (means, hist, total)
            })
            .collect();
        let mut grid = Vec::with_capacity(gw * gh);
        let mut hist = vec![0u64; NB];
        let mut total = 0.0f64;
        for (means, hh, t) in rows {
            grid.extend(means);
            hist.iter_mut().zip(hh).for_each(|(a, b)| *a += u64::from(b));
            total += t;
        }
        let n = (w * h).max(1) as u64;
        let pct = |p: f64| {
            let target = ((n as f64) * p).ceil().max(1.0) as u64;
            let mut acc = 0u64;
            for (i, c) in hist.iter().enumerate() {
                acc += c;
                if acc >= target {
                    return LO + (i as f32 + 0.5) / BINS_PER_EV;
                }
            }
            LO + NB as f32 / BINS_PER_EV
        };
        let key = (total / n as f64) as f32;
        let stats = ToneStats { key, p50: pct(0.5), p90: pct(0.9), p95: pct(0.95), p99: pct(0.99), white: pct(0.995) };
        // Guided filters on the centred field (f32 variance precision).
        grid.iter_mut().for_each(|v| *v -= key);
        let long = gw.max(gh) as f32;
        let radius = |r: f32| ((r * long / 2048.0).round() as usize).max(1);
        let mut fine = parity::guided(&grid, &grid, gw, gh, radius(ld::RADIUS_FINE), ld::EPS);
        let mut coarse = parity::guided(&grid, &grid, gw, gh, radius(ld::RADIUS_COARSE), ld::EPS);
        fine.iter_mut().chain(coarse.iter_mut()).for_each(|v| *v += key);
        ToneContext { stats, fine, coarse, bw: gw, bh: gh, map: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0] }
    }

    /// Fine and coarse bases (log2 Y) at render-frame coordinates `u`, `v` (0..=1).
    #[inline]
    pub fn bases(&self, u: f32, v: f32) -> (f32, f32) {
        let m = &self.map;
        let (su, sv) = (m[0] * u + m[1] * v + m[2], m[3] * u + m[4] * v + m[5]);
        let gx = (su * self.bw as f32 - 0.5).clamp(0.0, (self.bw - 1) as f32);
        let gy = (sv * self.bh as f32 - 0.5).clamp(0.0, (self.bh - 1) as f32);
        let (x0, y0) = (gx as usize, gy as usize);
        let (x1, y1) = ((x0 + 1).min(self.bw - 1), (y0 + 1).min(self.bh - 1));
        let (fx, fy) = (gx - x0 as f32, gy - y0 as f32);
        let bil = |d: &[f32]| {
            let (r0, r1) = (&d[y0 * self.bw..], &d[y1 * self.bw..]);
            let a = r0[x0] + (r0[x1] - r0[x0]) * fx;
            let b = r1[x0] + (r1[x1] - r1[x0]) * fx;
            a + (b - a) * fy
        };
        (bil(&self.fine), bil(&self.coarse))
    }

    /// Adaptation luminance (log2 Y) at render-frame coordinates `u`, `v` (0..=1).
    #[inline]
    pub fn sample(&self, u: f32, v: f32) -> f32 {
        let (f, c) = self.bases(u, v);
        let w = super::local_tone_data::WEIGHT_FINE;
        w * f + (1.0 - w) * c
    }
}

/// Local tone context of `src` (decoded source, un-oriented) for renders of `adjustments`
/// with EXIF `orientation`: computed on a [`TONE_GRID`] px uncropped version of the source.
pub fn tone_context(
    src: &super::source::LinearImage,
    orientation: u8,
    adjustments: &ParametricAdjustments,
    profile: &Profile,
) -> ToneContext {
    let whole = super::source::prepare(src, 1, &crate::ipc::types::CropSettings::default(), None, TONE_GRID);
    tone_context_prepared(&whole, src, orientation, adjustments, profile)
}

/// [`tone_context`] from an already prepared uncropped, un-oriented `whole` source.
pub fn tone_context_prepared(
    whole: &super::source::Prepared,
    src: &super::source::LinearImage,
    orientation: u8,
    adjustments: &ParametricAdjustments,
    profile: &Profile,
) -> ToneContext {
    tone_context_uncropped(whole, src, adjustments, profile).with_crop(
        &adjustments.crop,
        src.width,
        src.height,
        orientation,
    )
}

/// The context before [`ToneContext::with_crop`]: depends only on the source and the white
/// balance / calibration / profile of `adjustments` (cacheable across slider changes).
pub fn tone_context_uncropped(
    whole: &super::source::Prepared,
    src: &super::source::LinearImage,
    adjustments: &ParametricAdjustments,
    profile: &Profile,
) -> ToneContext {
    let mut input = RenderInput::simple(whole.width, whole.height, &whole.pixels, &src.color, profile);
    input.quality = Quality::Draft;
    ToneContext::compute(&input, adjustments)
}

/// Block means of log2 luminance and of the dark channel (min RGB) of the working image.
fn base_grids(rgb: &[f32], w: usize, h: usize, f: usize, dark: bool) -> (Grid, Option<Grid>) {
    let (gw, gh) = (w.div_ceil(f), h.div_ceil(f));
    let mut lum = vec![0.0f32; gw * gh];
    let mut dk = if dark { vec![0.0f32; gw * gh] } else { Vec::new() };
    let dk_rows: Vec<&mut [f32]> =
        if dark { dk.chunks_mut(gw).collect() } else { (0..gh).map(|_| &mut [][..]).collect() };
    lum.par_chunks_mut(gw).zip(dk_rows).enumerate().for_each(|(gy, (lrow, drow))| {
        let mut sums = vec![0.0f32; gw];
        let mut dsums = vec![0.0f32; if dark { gw } else { 0 }];
        let mut counts = vec![0u32; gw];
        for y in gy * f..((gy + 1) * f).min(h) {
            let row = &rgb[y * w * 3..(y + 1) * w * 3];
            for x in 0..w {
                let v = [row[x * 3], row[x * 3 + 1], row[x * 3 + 2]];
                let yl = dot(PROPHOTO_Y, v).max(1e-6);
                let gx = x / f;
                sums[gx] += yl.log2();
                counts[gx] += 1;
                if dark {
                    dsums[gx] += v[0].min(v[1]).min(v[2]).max(0.0);
                }
            }
        }
        for gx in 0..gw {
            let n = counts[gx].max(1) as f32;
            lrow[gx] = sums[gx] / n;
            if dark {
                drow[gx] = dsums[gx] / n;
            }
        }
    });
    let lum = Grid { w: gw, h: gh, factor: f, data: lum };
    let dk = dark.then_some(Grid { w: gw, h: gh, factor: f, data: dk });
    (lum, dk)
}

/// Local tone gain (shadows/highlights at the adaptation luminance, clarity, texture)
/// and dehaze, on linear ProPhoto.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn apply_local(
    v: [f32; 3],
    x: usize,
    y: usize,
    k: &Local,
    adapt: Option<f32>,
    clar: Option<&Field>,
    tex: Option<&Field>,
    haze: Option<&Field>,
) -> [f32; 3] {
    let mut v = v;
    let yl = dot(PROPHOTO_Y, v).max(1e-9);
    let ev = yl.log2();
    let mut delta = 0.0f32;
    if let (Some(t), Some(m)) = (&k.tone_local, adapt) {
        delta += t.eval(m);
    }
    if let Some(c) = clar {
        let evb = c.sample(x, y);
        let rel = ev - LOG2_GREY + 2.5;
        let mid = 1.0 / (1.0 + (rel / 3.0) * (rel / 3.0));
        delta += k.clarity * (ev - evb) * mid;
    }
    if let Some(t) = tex {
        delta += k.texture * (ev - t.sample(x, y));
    }
    if delta != 0.0 {
        let g = delta.clamp(-12.0, 12.0).exp2();
        v = v.map(|c| c * g);
    }
    if let Some(hz) = haze {
        // Remove the locally estimated veil (scene white ~1).
        let veil = (hz.sample(x, y).max(0.0) * k.dehaze * 0.9).min(0.9);
        let t = 1.0 - veil;
        v = v.map(|c| (c - veil).max(0.0) / t);
    } else if k.dehaze < 0.0 {
        let a = -k.dehaze * 0.6;
        let fog = 0.12;
        v = v.map(|c| c + (fog - c) * a);
    }
    v
}

/// Renders `input` with `adjustments` for the preview (8-bit sRGB + histogram); `lut` is the
/// resolved `adjustments.lut` (if present in the library).
pub fn render(input: &RenderInput, adjustments: &ParametricAdjustments, lut: Option<&Lut>) -> RenderedImage {
    let srgb = OutputSpace::srgb();
    let dev = develop(input, adjustments, lut, &srgb, input.quality);
    let (w, h) = (dev.width, dev.height);
    const BAND: usize = 8;
    let mut rgb = vec![0u8; w * h * 3];
    let hist = rgb
        .par_chunks_mut(w * 3 * BAND)
        .zip(dev.rgb.par_chunks(w * 3 * BAND))
        .map(|(out, src)| {
            let mut hist = [[0u32; 256]; 4];
            for (o, s) in out.as_chunks_mut::<3>().0.iter_mut().zip(src.as_chunks::<3>().0) {
                let q = [s[0], s[1], s[2]].map(|c| (c * 255.0 + 0.5).clamp(0.0, 255.0) as u8);
                o.copy_from_slice(&q);
                hist[0][q[0] as usize] += 1;
                hist[1][q[1] as usize] += 1;
                hist[2][q[2] as usize] += 1;
                let l = (LUMA_709[0] * f32::from(q[0])
                    + LUMA_709[1] * f32::from(q[1])
                    + LUMA_709[2] * f32::from(q[2])
                    + 0.5) as usize;
                hist[3][l.min(255)] += 1;
            }
            hist
        })
        .reduce(
            || [[0u32; 256]; 4],
            |mut a, b| {
                for c in 0..4 {
                    for i in 0..256 {
                        a[c][i] += b[c][i];
                    }
                }
                a
            },
        );
    let [red, green, blue, luma] = hist.map(|h| h.to_vec());
    RenderedImage { width: input.width, height: input.height, rgb, histogram: Histogram { red, green, blue, luma } }
}

/// Renders `input` for export: the same pipeline as [`render`] with the output stage for
/// `space`, as interleaved 16-bit values in the target's transfer curve. Without a LUT,
/// linear ProPhoto goes straight to the target (keeps P3 / Adobe RGB gamut); with a LUT,
/// the LUT runs on sRGB-encoded values as in the preview and the result is converted.
pub fn render_output(
    input: &RenderInput,
    adjustments: &ParametricAdjustments,
    lut: Option<&Lut>,
    space: &OutputSpace,
) -> Vec<u16> {
    let dev = develop(input, adjustments, lut, space, Quality::Export);
    dev.rgb.par_iter().map(|&c| (c * 65535.0 + 0.5).clamp(0.0, 65535.0) as u16).collect()
}

/// The camera's as-shot white balance for `input`'s profile (Lightroom's scale).
pub fn as_shot(color: &ColorInfo, profile: &Profile) -> Option<crate::ipc::types::WhiteBalanceValues> {
    camera::as_shot_values(color, profile)
}

/// The base tone curve (default profile curve): scene-linear -> display-linear.
pub fn base_curve(x: f32) -> f32 {
    tone::base_curve(x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::{HslChannels, LutRef, WhiteBalance};

    fn color() -> ColorInfo {
        // Camera = linear sRGB, as-shot multipliers (2, 1, 1.5).
        ColorInfo {
            as_shot_mul: Some([2.0, 1.0, 1.5]),
            daylight_mul: [2.0, 1.0, 1.5],
            rgb_cam: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            xyz_to_cam: [
                [3.2404542, -1.5371385, -0.4985314],
                [-0.969266, 1.8760108, 0.041556],
                [0.0556434, -0.2040259, 1.0572252],
            ],
            calibration: [1.0; 3],
        }
    }

    /// Plain adjustments (no detail processing, no profile) for deterministic tests.
    fn plain() -> ParametricAdjustments {
        let mut a = ParametricAdjustments::default();
        a.detail.sharpening.amount = 0.0;
        a.detail.noise_reduction.color = 0.0;
        a.profile = crate::ipc::types::ProfileSettings::none();
        a
    }

    /// Horizontal grey ramp (camera values "white balanced" by the inverse of the as-shot
    /// multipliers) with a coloured bottom half.
    fn ramp(w: u32, h: u32) -> Vec<u16> {
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let v = x as f32 / (w - 1) as f32 * 0.6;
                let rgb = if y < h / 2 { [v, v, v] } else { [v, v * 0.5, v * 0.2] };
                px.push((rgb[0] / 2.0 * 65535.0) as u16);
                px.push((rgb[1] * 65535.0) as u16);
                px.push((rgb[2] / 1.5 * 65535.0) as u16);
            }
        }
        px
    }

    /// Grey step edge: left half at `dark`, right half at `bright` (linear, pre-WB).
    fn step(w: u32, h: u32, dark: f32, bright: f32) -> Vec<u16> {
        let mut px = Vec::new();
        for _ in 0..h {
            for x in 0..w {
                let v = if x < w / 2 { dark } else { bright };
                px.push((v / 2.0 * 65535.0) as u16);
                px.push((v * 65535.0) as u16);
                px.push((v / 1.5 * 65535.0) as u16);
            }
        }
        px
    }

    /// Shadows / Highlights are edge-aware: next to an edge a flat area gets nearly the same
    /// local tone as far from it, while the slider clearly acts. Regression bounds: Camera
    /// Raw is halo-free on both steps (`tools/acr-oracle/halo_probe.py`), the guided
    /// adaptation base (best fit to real frames) leaves a small rim that decays within a
    /// few dozen px (measured worst: 15/255 at 3 px for Shadows +100 on a 3.9 EV step; 7 for
    /// a user-like Shadows +64 / Highlights -66). A halo-free base (domain transform) fits the
    /// real frames worse (held-out dE 3.13 vs 2.88), see docs/decisions.md.
    #[test]
    fn local_tone_has_no_halo_on_a_step_edge() {
        let (w, h) = (512u32, 128u32);
        let c = color();
        let p = Profile::matrix(BASELINE_EV);
        for (dark, bright, tol) in [(0.02f32, 0.3f32, 16), (0.004, 0.6, 9)] {
            let px = step(w, h, dark, bright);
            let row = |img: &RenderedImage, x: u32| i32::from(img.rgb[((h / 2 * w + x) * 3 + 1) as usize]);
            let base = render(&RenderInput::simple(w, h, &px, &c, &p), &plain(), None);
            for (sh, hl) in [(100.0, 0.0), (-100.0, 0.0), (0.0, -100.0), (64.0, -66.0)] {
                let adj = ParametricAdjustments { shadows: sh, highlights: hl, ..plain() };
                let img = render(&RenderInput::simple(w, h, &px, &c, &p), &adj, None);
                // Dark side: 3 px from the edge vs 200 px away; bright side likewise.
                let (near_d, far_d) = (row(&img, w / 2 - 3), row(&img, w / 2 - 200));
                let (near_b, far_b) = (row(&img, w / 2 + 2), row(&img, w / 2 + 200));
                let msg = format!("step {dark}|{bright} S {sh} H {hl}");
                assert!((near_d - far_d).abs() <= tol, "{msg}: dark side halo {near_d} vs {far_d}");
                assert!((near_b - far_b).abs() <= tol, "{msg}: bright side halo {near_b} vs {far_b}");
                // The rim decays away from the edge.
                assert!((row(&img, w / 2 - 30) - far_d).abs() <= 7, "{msg}: dark rim at 30 px");
                assert!((row(&img, w / 2 + 30) - far_b).abs() <= 7, "{msg}: bright rim at 30 px");
                if sh > 0.0 {
                    assert!(far_d > row(&base, w / 2 - 200), "{msg}: shadows lift the dark side");
                }
                if sh < 0.0 {
                    assert!(far_d < row(&base, w / 2 - 200), "{msg}: negative shadows darken the dark side");
                }
                if hl < 0.0 {
                    assert!(far_b < row(&base, w / 2 + 200), "{msg}: highlights darken the bright side");
                }
            }
        }
    }

    /// A zoomed region with the whole frame's context renders like the same pixels of the
    /// whole-frame render (the local tone adapts to the whole image, not the crop).
    #[test]
    fn region_uses_the_whole_frame_tone_context() {
        let (w, h) = (400u32, 200u32);
        let c = color();
        let p = Profile::matrix(BASELINE_EV);
        let px = ramp(w, h);
        let adj = ParametricAdjustments { shadows: 60.0, highlights: -50.0, ..plain() };
        let whole = render(&RenderInput::simple(w, h, &px, &c, &p), &adj, None);
        // Region: the left quarter (dark end of the ramp), same scale.
        let (rw, rh) = (w / 4, h);
        let mut rpx = Vec::new();
        for y in 0..rh {
            let s = (y * w * 3) as usize;
            rpx.extend_from_slice(&px[s..s + (rw * 3) as usize]);
        }
        let src = super::super::source::LinearImage {
            width: w,
            height: h,
            pixels: px.clone(),
            color: c.clone(),
            full_width: w,
            full_height: h,
            display_referred: false,
            source_color: None,
        };
        let ctx = tone_context(&src, 1, &adj, &p);
        let mut input = RenderInput::simple(rw, rh, &rpx, &c, &p);
        input.view = View { frame_x: 0.0, frame_y: 0.0, frame_w: w as f32, frame_h: h as f32, scale: 1.0 };
        input.frame_long_edge = w as f32;
        input.tone = Some(&ctx);
        let region = render(&input, &adj, None);
        let mut worst = 0;
        for y in [10u32, 50, 150] {
            for x in [5u32, 40, 90] {
                let a = whole.rgb[((y * w + x) * 3) as usize];
                let b = region.rgb[((y * rw + x) * 3) as usize];
                worst = worst.max((i32::from(a) - i32::from(b)).abs());
            }
        }
        assert!(worst <= 3, "region differs from the whole frame by {worst}");
    }

    fn run(adj: &ParametricAdjustments, lut: Option<&Lut>) -> RenderedImage {
        let (w, h) = (300, 200);
        let px = ramp(w, h);
        let c = color();
        let p = Profile::matrix(BASELINE_EV);
        render(&RenderInput::simple(w, h, &px, &c, &p), adj, lut)
    }

    fn mean(img: &RenderedImage, c: usize) -> f32 {
        img.rgb.chunks(3).map(|p| f32::from(p[c])).sum::<f32>() / (img.rgb.len() / 3) as f32
    }

    fn px(img: &RenderedImage, x: usize, y: usize) -> [u8; 3] {
        let i = (y * img.width as usize + x) * 3;
        [img.rgb[i], img.rgb[i + 1], img.rgb[i + 2]]
    }

    #[test]
    fn neutral_greys_stay_neutral_and_histogram_sums() {
        let img = run(&plain(), None);
        assert_eq!(img.rgb.len(), 300 * 200 * 3);
        for x in [0, 50, 150, 299] {
            let p = px(&img, x, 10);
            assert!(
                (i32::from(p[0]) - i32::from(p[2])).abs() <= 1 && (i32::from(p[0]) - i32::from(p[1])).abs() <= 1,
                "{x}: {p:?}"
            );
        }
        assert!(px(&img, 299, 10)[1] > px(&img, 150, 10)[1] && px(&img, 150, 10)[1] > px(&img, 10, 10)[1]);
        let n = 300 * 200;
        for ch in [&img.histogram.red, &img.histogram.green, &img.histogram.blue, &img.histogram.luma] {
            assert_eq!(ch.len(), 256);
            assert_eq!(ch.iter().sum::<u32>(), n);
        }
    }

    #[test]
    fn matches_camera_raw_base_curve_on_neutrals() {
        // Linear camera = sRGB primaries; a neutral at 0.18 of the raw clip with no baseline
        // renders like Camera Raw's default curve (0.18 -> ~0.38 display linear).
        let c = ColorInfo { as_shot_mul: Some([1.0, 1.0, 1.0]), daylight_mul: [1.0; 3], ..color() };
        let v = (0.18 * 65535.0) as u16;
        let px = vec![v; 16 * 16 * 3];
        let p = Profile::matrix(0.0);
        let img = render(&RenderInput::simple(16, 16, &px, &c, &p), &plain(), None);
        let got = srgb_decode(f32::from(img.rgb[0]) / 255.0);
        assert!((got - tone::base_curve(0.18)).abs() < 0.01, "{got}");
    }

    #[test]
    fn sliders_move_in_the_expected_direction() {
        let base = run(&plain(), None);
        let g = |img: &RenderedImage| mean(img, 1);
        let adj = |f: &dyn Fn(&mut ParametricAdjustments)| {
            let mut a = plain();
            f(&mut a);
            run(&a, None)
        };
        assert!(g(&adj(&|a| a.exposure = 1.0)) > g(&base) + 10.0);
        assert!(g(&adj(&|a| a.exposure = -1.0)) < g(&base) - 10.0);
        let dark = |img: &RenderedImage| px(img, 30, 10)[1];
        let bright = |img: &RenderedImage| px(img, 290, 10)[1];
        assert!(dark(&adj(&|a| a.shadows = 100.0)) > dark(&base));
        assert!(bright(&adj(&|a| a.highlights = -100.0)) < bright(&base));
        assert!(bright(&adj(&|a| a.whites = 100.0)) >= bright(&base));
        let deep = |img: &RenderedImage| px(img, 5, 10)[1];
        assert!(deep(&adj(&|a| a.blacks = -100.0)) <= deep(&base));
        let c = adj(&|a| a.contrast = 100.0);
        assert!(dark(&c) < dark(&base) && bright(&c) >= bright(&base));
        let grey = adj(&|a| a.saturation = -100.0);
        let p = px(&grey, 200, 150);
        assert!((i32::from(p[0]) - i32::from(p[2])).abs() <= 2, "{p:?}");
        let sat = |img: &RenderedImage| {
            let p = px(img, 200, 150);
            i32::from(p[0]) - i32::from(p[2])
        };
        assert!(sat(&adj(&|a| a.saturation = 50.0)) > sat(&base));
        assert!(sat(&adj(&|a| a.vibrance = 80.0)) > sat(&base));
        let warm = adj(&|a| a.white_balance = WhiteBalance::Custom { temperature_k: 9000.0, tint: 0.0 });
        let cool = adj(&|a| a.white_balance = WhiteBalance::Custom { temperature_k: 3000.0, tint: 0.0 });
        let rb = |img: &RenderedImage| {
            let p = px(img, 200, 10);
            i32::from(p[0]) - i32::from(p[2])
        };
        assert!(rb(&warm) > rb(&cool) + 20, "{} {}", rb(&warm), rb(&cool));
        let hsl = adj(&|a| a.hsl.luminance = HslChannels { orange: -100.0, red: -100.0, ..Default::default() });
        assert!(g(&hsl) < g(&base));
        assert_eq!(px(&hsl, 200, 10), px(&base, 200, 10), "greys unaffected");
        let hue = adj(&|a| a.hsl.hue = HslChannels { orange: 100.0, red: 100.0, ..Default::default() });
        assert_ne!(px(&hue, 200, 150), px(&base, 200, 150));
        assert_eq!(px(&hue, 200, 10), px(&base, 200, 10));
        for f in [
            &(|a: &mut ParametricAdjustments| a.clarity = 80.0) as &dyn Fn(&mut ParametricAdjustments),
            &|a| a.texture = -60.0,
            &|a| a.dehaze = 50.0,
            &|a| a.dehaze = -50.0,
            &|a| a.tone_curve.point.master = vec![[0.0, 30.0], [128.0, 150.0], [255.0, 255.0]],
            &|a| a.tone_curve.parametric.lights = 60.0,
        ] {
            let img = adj(f);
            let p = px(&img, 150, 10);
            assert!((i32::from(p[0]) - i32::from(p[2])).abs() <= 2, "{p:?}");
        }
        // Point curve lifts blacks.
        let lifted = adj(&|a| a.tone_curve.point.master = vec![[0.0, 40.0], [255.0, 255.0]]);
        assert!(px(&lifted, 0, 10)[1] >= 38);
        // B&W: colours become grey.
        let bw = adj(&|a| a.black_and_white.enabled = true);
        let p = px(&bw, 200, 150);
        assert!((i32::from(p[0]) - i32::from(p[1])).abs() <= 1 && (i32::from(p[1]) - i32::from(p[2])).abs() <= 1);
    }

    #[test]
    fn lut_applies_on_encoded_values_with_amount() {
        let lut = Lut::parse("LUT_1D_SIZE 2\n1 1 1\n0 0 0\n").unwrap();
        let base = run(&plain(), None);
        let mut adj = ParametricAdjustments { lut: Some(LutRef { id: "inv".into(), amount: 100.0 }), ..plain() };
        let inv = run(&adj, Some(&lut));
        for (a, b) in base.rgb.iter().zip(&inv.rgb).step_by(97) {
            assert!((i32::from(*a) + i32::from(*b) - 255).abs() <= 2, "{a} {b}");
        }
        adj.lut = Some(LutRef { id: "inv".into(), amount: 50.0 });
        let half = run(&adj, Some(&lut));
        assert!(half.rgb.iter().step_by(97).all(|&v| (i32::from(v) - 128).abs() <= 2));
    }

    #[test]
    fn lut3_matches_direct_chain() {
        let c = color();
        let p = Profile::matrix(BASELINE_EV);
        let mut adj = plain();
        adj.contrast = -40.0;
        adj.vibrance = 30.0;
        adj.hsl.saturation.green = -40.0;
        adj.tone_curve.point.master = vec![[0.0, 14.0], [44.0, 46.0], [106.0, 110.0], [255.0, 252.0]];
        adj.color_grading.midtones = crate::ipc::types::ColorWheel { hue: 185.0, saturation: 5.0, luminance: 0.0 };
        let setup = camera::color_setup(&c, &p, &adj.white_balance, &adj.calibration);
        let out = Output::new(&OutputSpace::srgb(), None, Quality::Export);
        let luts = parity::curve_luts(&adj.tone_curve, None);
        let master = luts.master.clone();
        let chain = Chain {
            hsm: setup.hsm.as_ref(),
            exposure_gain: 0.35f32.exp2(),
            display_referred: false,
            dcp_look: None,
            look_hsv: None,
            look_rgb: None,
            bw: None,
            color: ColorOps::new(&adj),
            shadow_tint: 0.0,
            curve: global_tone_curve(
                &ToneModel::new(ToneSliders { exposure: 0.35, contrast: -40.0, ..Default::default() }),
                0.35,
                |x| {
                    let t = tone::base_curve(x);
                    srgb_decode(parity::eval_lut(master.as_ref().unwrap(), srgb_encode(t)))
                },
            ),
            rgb_curves: [None, None, None],
            grade: Grade::new(&adj.color_grading),
            _marker: std::marker::PhantomData,
        };
        for (n, tol) in [(33, (1.5f32, 6.0f32)), (65, (0.5, 2.0))] {
            let lut = Lut3::build(&chain, n);
            let mut worst = 0.0f32;
            let mut errs = Vec::new();
            let mut s = 7u32;
            for _ in 0..4000 {
                let mut r = || {
                    s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (s >> 8) as f32 / (1 << 24) as f32
                };
                // Photographic colours: log-uniform brightness, HSV saturation <= 0.85.
                let mx = (-10.0 * r()).exp2();
                let lo = mx * (0.15 + 0.85 * r());
                let mid = lo + (mx - lo) * r();
                let v = match (r() * 6.0) as u32 {
                    0 => [mx, mid, lo],
                    1 => [mid, mx, lo],
                    2 => [lo, mx, mid],
                    3 => [lo, mid, mx],
                    4 => [mid, lo, mx],
                    _ => [mx, lo, mid],
                };
                // Perceptual difference: Oklab distance x 100 (~ delta E) of the encoded sRGB.
                let lab = |e: [f32; 3]| {
                    let lin = e.map(|c| srgb_decode(c.clamp(0.0, 1.0)));
                    let lms = [
                        0.412_221_47 * lin[0] + 0.536_332_54 * lin[1] + 0.051_445_99 * lin[2],
                        0.211_903_5 * lin[0] + 0.680_699_5 * lin[1] + 0.107_396_96 * lin[2],
                        0.088_302_46 * lin[0] + 0.281_718_84 * lin[1] + 0.629_978_7 * lin[2],
                    ]
                    .map(f32::cbrt);
                    [
                        0.210_454_26 * lms[0] + 0.793_617_8 * lms[1] - 0.004_072_047 * lms[2],
                        1.977_998_5 * lms[0] - 2.428_592_2 * lms[1] + 0.450_593_7 * lms[2],
                        0.025_904_037 * lms[0] + 0.782_771_77 * lms[1] - 0.808_675_77 * lms[2],
                    ]
                };
                let (la, lb) = (lab(out.apply(lut.eval(v))), lab(out.apply(chain.eval(v))));
                let d = ((la[0] - lb[0]).powi(2) + (la[1] - lb[1]).powi(2) + (la[2] - lb[2]).powi(2)).sqrt() * 100.0;
                worst = worst.max(d);
                errs.push(d);
            }
            errs.sort_by(f32::total_cmp);
            let p99 = errs[errs.len() * 99 / 100];
            assert!(p99 < tol.0 && worst < tol.1, "{n}^3: p99 {p99} worst {worst} (Oklab x100)");
        }
    }
}
