//! Parametric pipeline. Shared by the Phase 5 preview and the Phase 6 full-resolution
//! export, so it is resolution-independent: spatial operators (texture, clarity, dehaze,
//! the highlight/shadow mask) take radii relative to the whole frame's long edge.
//!
//! Per pixel (f32, rows in parallel):
//! 1. camera RGB x WB multipliers (G = 1), clipped at the smallest multiplier (so fully
//!    clipped sensor highlights stay neutral) -> linear Rec.2020 (LibRaw `rgb_cam` then
//!    sRGB -> Rec.2020) x 2^(exposure + baseline).
//! 2. Tone, as a luminance gain (hue/saturation preserving) in EV around mid-grey 0.18:
//!    highlights/shadows are smooth weights of a *masked* luminance (blurred log-luminance
//!    mixed with the pixel's, so local contrast survives), whites/blacks are smooth weights
//!    of the pixel luminance, clarity/texture add a multiple of (log-luminance - blurred
//!    log-luminance) at a large/small radius, contrast scales EV around grey (S-curve once
//!    the base curve is applied).
//! 3. Dehaze: a blurred dark channel estimates the veil; positive removes it
//!    (haze model J = (I - A(1 - t)) / t with a saturation lift), negative adds one.
//! 4. Vibrance/saturation/HSL on linear values with hue taken from gamma-encoded sRGB
//!    (so Lightroom's band centres apply): chroma scaling around luminance, hue rotation
//!    around the grey axis, band luminance as a saturation-weighted exposure.
//! 5. Base tone curve (filmic Hill curve: toe + shoulder, white at +2.25 EV over the raw
//!    clip at 0 EV) per channel -> Rec.2020 -> sRGB, out-of-gamut colours desaturated
//!    towards their luminance, sRGB encode (table).
//! 6. `.cube` LUT on the encoded values (amount blend) -> 8-bit, histogram.
//!
//! Blurs run on a block-averaged grid (2x2 at preview sizes, coarser for big radii) with
//! three box passes (~gaussian) and bilinear upsampling, so their cost does not grow with
//! the radius.

use rayon::prelude::*;

use crate::ipc::types::{Histogram, ParametricAdjustments, WhiteBalance};
use crate::lut::{Interpolation, Lut};

use super::source::ColorInfo;
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

/// Pipeline input: camera RGB already cropped/resampled/oriented.
pub struct RenderInput<'a> {
    pub width: u32,
    pub height: u32,
    /// Interleaved camera RGB16 (not white balanced; white level 65535).
    pub pixels: &'a [u16],
    pub color: &'a ColorInfo,
    /// Long edge of the whole frame in this input's pixel units (radius reference).
    pub frame_long_edge: f32,
}

/// Brightness offset (EV) of the default render, like DNG `BaselineExposure`.
pub const BASELINE_EV: f32 = 0.35;
/// log2 of scene mid-grey (0.18).
const LOG2_GREY: f32 = -2.473_931_2;
const LUMA_2020: [f32; 3] = [0.2627, 0.6780, 0.0593];
const LUMA_709: [f32; 3] = [0.2126, 0.7152, 0.0722];

// Radii as a fraction of the frame's long edge (gaussian sigma).
const SIGMA_MASK: f32 = 0.012;
const SIGMA_TEXTURE: f32 = 0.0022;
const SIGMA_HAZE: f32 = 0.02;

const XYZ_FROM_SRGB: [[f64; 3]; 3] =
    [[0.4124564, 0.3575761, 0.1804375], [0.2126729, 0.7151522, 0.0721750], [0.0193339, 0.1191920, 0.9503041]];
const XYZ_FROM_2020: [[f64; 3]; 3] =
    [[0.6369580, 0.1446169, 0.1688810], [0.2627002, 0.6779981, 0.0593017], [0.0000000, 0.0280727, 1.0609851]];

fn mat_mul(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

fn to_f32(m: [[f64; 3]; 3]) -> [[f32; 3]; 3] {
    m.map(|r| r.map(|v| v as f32))
}

/// sRGB -> Rec.2020 and back (linear, D65).
fn gamut_matrices() -> ([[f64; 3]; 3], [[f64; 3]; 3]) {
    let to_xyz_2020_inv = wb::invert3(&XYZ_FROM_2020).expect("invertible");
    let srgb_to_2020 = mat_mul(&to_xyz_2020_inv, &XYZ_FROM_SRGB);
    let rec2020_to_srgb = wb::invert3(&srgb_to_2020).expect("invertible");
    (srgb_to_2020, rec2020_to_srgb)
}

#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn mat3(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

/// A function sampled on a table with linear interpolation.
struct Table {
    lo: f32,
    inv_step: f32,
    values: Vec<f32>,
}

impl Table {
    fn new(lo: f32, hi: f32, n: usize, f: impl Fn(f32) -> f32) -> Self {
        let step = (hi - lo) / (n - 1) as f32;
        Table { lo, inv_step: 1.0 / step, values: (0..n).map(|i| f(lo + i as f32 * step)).collect() }
    }

    #[inline]
    fn eval(&self, x: f32) -> f32 {
        let t = ((x - self.lo) * self.inv_step).max(0.0);
        let last = self.values.len() - 1;
        let i = (t as usize).min(last - 1);
        let f = (t - i as f32).min(1.0);
        let (a, b) = (self.values[i], self.values[i + 1]);
        a + (b - a) * f
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
        // NaN and negatives -> 0; beyond max -> last value.
        let u = if x > 0.0 { (x * self.inv_max).min(1.0).sqrt() } else { 0.0 };
        let t = u * self.scale;
        let i = (t as usize).min(self.values.len() - 2);
        let f = t - i as f32;
        let (a, b) = (self.values[i], self.values[i + 1]);
        a + (b - a) * f
    }
}

fn srgb_encode(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// Filmic base curve (scene-linear -> display-linear, 0..=1): Hill function
/// `s * x^n / (x^n + k^n)` fitted so mid-grey maps to `GREY_OUT` and `WHITE` maps to 1.
pub fn base_curve(x: f32) -> f32 {
    static B: std::sync::OnceLock<BaseCurve> = std::sync::OnceLock::new();
    B.get_or_init(BaseCurve::fit).eval(x)
}

struct BaseCurve {
    n: f64,
    k: f64,
    s: f64,
}

const CURVE_N: f64 = 1.45;
/// Display-linear value of scene mid-grey (0.20 -> sRGB 124).
const GREY_OUT: f64 = 0.20;
/// Scene value rendered as pure white (raw clip at 0 EV x 2^(BASELINE_EV + ~1.9)).
const WHITE: f64 = 4.8;

impl BaseCurve {
    fn fit() -> Self {
        let n = CURVE_N;
        let hill = |x: f64, k: f64| x.powf(n) / (x.powf(n) + k.powf(n));
        // Solve hill(G)/hill(W) = GREY_OUT for k (monotone in k).
        let (mut lo, mut hi) = (1e-3f64, 100.0f64);
        for _ in 0..200 {
            let k = (lo * hi).sqrt();
            let ratio = hill(0.18, k) / hill(WHITE, k);
            if ratio > GREY_OUT {
                lo = k;
            } else {
                hi = k;
            }
        }
        let k = (lo * hi).sqrt();
        BaseCurve { n, k, s: 1.0 / hill(WHITE, k) }
    }

    fn eval(&self, x: f32) -> f32 {
        let x = f64::from(x.max(0.0));
        let xn = x.powf(self.n);
        (self.s * xn / (xn + self.k.powf(self.n))).min(1.0) as f32
    }
}

/// Tables and matrices shared by every render (built once).
struct Statics {
    curve: SqrtTable,
    encode: SqrtTable,
    srgb_to_2020: [[f64; 3]; 3],
    rec2020_to_srgb: [[f32; 3]; 3],
}

fn statics() -> &'static Statics {
    static S: std::sync::OnceLock<Statics> = std::sync::OnceLock::new();
    S.get_or_init(|| {
        let bc = BaseCurve::fit();
        let (srgb_to_2020, rec2020_to_srgb) = gamut_matrices();
        Statics {
            curve: SqrtTable::new(WHITE as f32 * 1.001, 8192, |x| bc.eval(x)),
            encode: SqrtTable::new(1.0, 4096, srgb_encode),
            srgb_to_2020,
            rec2020_to_srgb: to_f32(rec2020_to_srgb),
        }
    })
}

/// HSL band centres (degrees of sRGB-encoded HSV hue), Lightroom order.
const BAND_CENTERS: [f32; 9] = [0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 270.0, 300.0, 360.0];

/// Smooth partition-of-unity weights: (band a, band b, weight of b).
#[inline]
fn band_weights(h: f32) -> (usize, usize, f32) {
    let mut i = 0;
    while i < 7 && h >= BAND_CENTERS[i + 1] {
        i += 1;
    }
    let t = (h - BAND_CENTERS[i]) / (BAND_CENTERS[i + 1] - BAND_CENTERS[i]);
    let s = t.clamp(0.0, 1.0);
    (i, (i + 1) % 8, s * s * (3.0 - 2.0 * s))
}

/// HSV hue in degrees and saturation of a (non-negative) RGB triple.
#[inline]
fn hue_sat(e: [f32; 3]) -> (f32, f32) {
    let mx = e[0].max(e[1]).max(e[2]);
    let mn = e[0].min(e[1]).min(e[2]);
    let d = mx - mn;
    if d <= 1e-6 || mx <= 1e-6 {
        return (0.0, 0.0);
    }
    let h = if mx == e[0] {
        let h = (e[1] - e[2]) / d;
        if h < 0.0 {
            h + 6.0
        } else {
            h
        }
    } else if mx == e[1] {
        (e[2] - e[0]) / d + 2.0
    } else {
        (e[0] - e[1]) / d + 4.0
    };
    (h * 60.0, d / mx)
}

/// Blurred scalar field on a coarse grid, sampled bilinearly at full resolution.
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

    /// Gaussian-like blur (3 box passes per axis) with sigma in full-resolution pixels.
    fn blurred(&self, sigma: f32) -> Field {
        let g = ((sigma / (self.factor as f32 * 3.0)).floor() as usize).max(1);
        let mut grid = self.downsample(g);
        let k = ((sigma / grid.factor as f32).round() as usize).max(1);
        for _ in 0..3 {
            box_rows(&mut grid.data, grid.w, k);
        }
        let mut t = transpose(&grid.data, grid.w, grid.h);
        for _ in 0..3 {
            box_rows(&mut t, grid.h, k);
        }
        let data = transpose(&t, grid.h, grid.w);
        Field { w: grid.w, h: grid.h, factor: grid.factor as f32, data }
    }
}

/// In-place box filter (radius `k`, edge-replicated) along rows of width `w`.
fn box_rows(data: &mut [f32], w: usize, k: usize) {
    let norm = 1.0 / (2 * k + 1) as f32;
    data.par_chunks_mut(w).for_each_init(
        || Vec::with_capacity(w),
        |tmp: &mut Vec<f32>, row| {
            tmp.clear();
            tmp.extend_from_slice(row);
            let at = |i: isize| tmp[i.clamp(0, w as isize - 1) as usize];
            let mut sum: f32 = (-(k as isize)..=k as isize).map(at).sum();
            for (x, out) in row.iter_mut().enumerate() {
                *out = sum * norm;
                let xi = x as isize;
                sum += at(xi + k as isize + 1) - at(xi - k as isize);
            }
        },
    );
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

/// Per-render constants.
struct Consts {
    /// Camera (WB'd, clipped, 0..1 scaled by 1/65535 folded into `mul`) -> Rec.2020 x gain.
    m: [[f32; 3]; 3],
    mul: [f32; 3],
    clip: f32,
    tone_local: Option<Table>,
    tone_global: Option<Table>,
    contrast: f32,
    clarity: f32,
    texture: f32,
    dehaze: f32,
    saturation: f32,
    vibrance: f32,
    hsl: Option<[[f32; 8]; 3]>,
    lut_amount: f32,
}

impl Consts {
    fn new(adj: &ParametricAdjustments, color: &ColorInfo, st: &Statics) -> Self {
        let mul = match adj.white_balance {
            WhiteBalance::AsShot => color.as_shot(),
            WhiteBalance::Custom { temperature_k, tint } => {
                wb::multipliers_for(crate::ipc::types::WhiteBalanceValues { temperature_k, tint }, &color.xyz_to_cam)
            }
        };
        let mul = if mul.iter().all(|v| v.is_finite() && *v > 0.0) { mul } else { [1.0; 3] };
        let clip = mul[0].min(mul[1]).min(mul[2]);
        let gain = (adj.exposure + BASELINE_EV).exp2() as f64;
        let rgb_cam: [[f64; 3]; 3] = color.rgb_cam.map(|r| r.map(f64::from));
        let m = mat_mul(&st.srgb_to_2020, &rgb_cam).map(|r| r.map(|v| (v * gain) as f32));
        let s = 1.0 / 65535.0;

        let (sh, hl) = (adj.shadows / 100.0, adj.highlights / 100.0);
        let tone_local = (sh != 0.0 || hl != 0.0).then(|| {
            Table::new(-16.0, 8.0, 1024, |m| {
                let w_sh = (1.0 - smoothstep(-3.5, 0.5, m)) * smoothstep(-11.0, -6.0, m);
                let w_hl = smoothstep(-0.5, 2.0, m);
                sh * 1.6 * w_sh + hl * 1.5 * w_hl
            })
        });
        let (wh, bl) = (adj.whites / 100.0, adj.blacks / 100.0);
        let tone_global = (wh != 0.0 || bl != 0.0).then(|| {
            Table::new(-16.0, 8.0, 1024, |e| {
                wh * 1.0 * smoothstep(0.5, 2.8, e) + bl * 1.5 * (1.0 - smoothstep(-6.5, -2.0, e))
            })
        });
        let h = &adj.hsl;
        let band = |c: &crate::ipc::types::HslChannels| {
            [c.red, c.orange, c.yellow, c.green, c.aqua, c.blue, c.purple, c.magenta].map(|v| v / 100.0)
        };
        let hsl = [band(&h.hue), band(&h.saturation), band(&h.luminance)];
        let hsl = hsl.iter().flatten().any(|v| *v != 0.0).then_some(hsl);
        Consts {
            m,
            mul: mul.map(|v| v * s),
            clip,
            tone_local,
            tone_global,
            contrast: 1.0 + adj.contrast / 100.0 * 0.45,
            clarity: adj.clarity / 100.0 * 0.6,
            texture: adj.texture / 100.0 * 0.8,
            dehaze: adj.dehaze / 100.0,
            saturation: 1.0 + adj.saturation / 100.0,
            vibrance: adj.vibrance / 100.0,
            hsl,
            lut_amount: adj.lut.as_ref().map_or(0.0, |l| l.amount),
        }
    }

    #[inline]
    fn linear(&self, p: &[u16]) -> [f32; 3] {
        let c = [
            (f32::from(p[0]) * self.mul[0]).min(self.clip),
            (f32::from(p[1]) * self.mul[1]).min(self.clip),
            (f32::from(p[2]) * self.mul[2]).min(self.clip),
        ];
        mat3(&self.m, c)
    }
}

/// Block means of log2 luminance and of the dark channel (min RGB), full-res factor `f`.
fn base_grids(input: &RenderInput, k: &Consts, f: usize, dark: bool) -> (Grid, Option<Grid>) {
    let (w, h) = (input.width as usize, input.height as usize);
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
            let row = &input.pixels[y * w * 3..(y + 1) * w * 3];
            for x in 0..w {
                let v = k.linear(&row[x * 3..x * 3 + 3]);
                let yl = dot(LUMA_2020, v).max(1e-6);
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

/// Renders `input` with `adjustments`; `lut` is the resolved `adjustments.lut` (if present
/// in the library).
pub fn render(input: &RenderInput, adjustments: &ParametricAdjustments, lut: Option<&Lut>) -> RenderedImage {
    let st = statics();
    let k = Consts::new(adjustments, input.color, st);
    let (w, h) = (input.width as usize, input.height as usize);
    let edge = input.frame_long_edge.max(1.0);

    let need_mask = k.tone_local.is_some() || k.clarity != 0.0;
    let need_tex = k.texture != 0.0;
    let need_haze = k.dehaze > 0.0;
    let f0 = if w.min(h) >= 256 { 2 } else { 1 };
    let (mask, tex, haze) = if need_mask || need_tex || need_haze {
        let (lum, dark) = base_grids(input, &k, f0, need_haze);
        let mask = need_mask.then(|| lum.blurred(SIGMA_MASK * edge));
        let tex = need_tex.then(|| lum.blurred(SIGMA_TEXTURE * edge));
        let haze = dark.map(|d| d.blurred(SIGMA_HAZE * edge));
        (mask, tex, haze)
    } else {
        (None, None, None)
    };

    let tone_any = k.tone_local.is_some() || k.tone_global.is_some() || need_mask || need_tex || k.contrast != 1.0;
    let color_any = k.saturation != 1.0 || k.vibrance != 0.0 || k.hsl.is_some() || k.dehaze > 0.0;
    let to_srgb = st.rec2020_to_srgb;
    let lut = lut.filter(|_| k.lut_amount > 0.0);

    const BAND: usize = 8;
    let mut rgb = vec![0u8; w * h * 3];
    let hist = rgb
        .par_chunks_mut(w * 3 * BAND)
        .enumerate()
        .map(|(band, out)| {
            let mut hist = [[0u32; 256]; 4];
            for (ry, out_row) in out.chunks_mut(w * 3).enumerate() {
                let y = band * BAND + ry;
                let in_row = &input.pixels[y * w * 3..(y + 1) * w * 3];
                for x in 0..w {
                    let mut v = k.linear(&in_row[x * 3..x * 3 + 3]);

                    if tone_any {
                        let yl = dot(LUMA_2020, v).max(1e-6);
                        let ev = yl.log2() - LOG2_GREY;
                        let mut delta = 0.0f32;
                        if let Some(mask) = &mask {
                            let evb = mask.sample(x, y) - LOG2_GREY;
                            if let Some(t) = &k.tone_local {
                                delta += t.eval(evb * 0.7 + ev * 0.3);
                            }
                            if k.clarity != 0.0 {
                                let mid = 1.0 / (1.0 + (ev * (1.0 / 3.0)) * (ev * (1.0 / 3.0)));
                                delta += k.clarity * (ev - evb) * mid;
                            }
                        }
                        if let Some(tex) = &tex {
                            delta += k.texture * (ev - (tex.sample(x, y) - LOG2_GREY));
                        }
                        if let Some(t) = &k.tone_global {
                            delta += t.eval(ev);
                        }
                        let ev2 = (ev + delta) * k.contrast;
                        let gain = (ev2 - ev).clamp(-12.0, 12.0).exp2();
                        v = [v[0] * gain, v[1] * gain, v[2] * gain];
                    }

                    if let Some(haze) = &haze {
                        // Remove the locally estimated veil (scene white ~1).
                        let veil = (haze.sample(x, y).max(0.0) * k.dehaze * 0.9).min(0.9);
                        let t = 1.0 - veil;
                        v = [(v[0] - veil).max(0.0) / t, (v[1] - veil).max(0.0) / t, (v[2] - veil).max(0.0) / t];
                    } else if k.dehaze < 0.0 {
                        // Add a uniform light-grey veil.
                        let a = -k.dehaze * 0.6;
                        let fog = 0.35;
                        v = [v[0] + (fog - v[0]) * a, v[1] + (fog - v[1]) * a, v[2] + (fog - v[2]) * a];
                    }

                    if color_any {
                        v = color_ops(v, &k, st);
                    }

                    // Base curve per channel, then Rec.2020 -> sRGB with gamut mapping.
                    let d = [st.curve.eval(v[0]), st.curve.eval(v[1]), st.curve.eval(v[2])];
                    let mut s = mat3(&to_srgb, d);
                    let mn = s[0].min(s[1]).min(s[2]);
                    if mn < 0.0 {
                        let ys = dot(LUMA_709, s).max(0.0);
                        let t = ys / (ys - mn);
                        s = [ys + (s[0] - ys) * t, ys + (s[1] - ys) * t, ys + (s[2] - ys) * t];
                    }
                    let mut e = [st.encode.eval(s[0]), st.encode.eval(s[1]), st.encode.eval(s[2])];
                    if let Some(lut) = lut {
                        let m = lut.eval(e, Interpolation::Tetrahedral);
                        let a = (k.lut_amount / 100.0).min(1.0);
                        e = [e[0] + (m[0] - e[0]) * a, e[1] + (m[1] - e[1]) * a, e[2] + (m[2] - e[2]) * a];
                    }
                    let q = e.map(|c| (c * 255.0 + 0.5).clamp(0.0, 255.0) as u8);
                    out_row[x * 3..x * 3 + 3].copy_from_slice(&q);
                    hist[0][q[0] as usize] += 1;
                    hist[1][q[1] as usize] += 1;
                    hist[2][q[2] as usize] += 1;
                    let l = (LUMA_709[0] * f32::from(q[0])
                        + LUMA_709[1] * f32::from(q[1])
                        + LUMA_709[2] * f32::from(q[2])
                        + 0.5) as usize;
                    hist[3][l.min(255)] += 1;
                }
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

/// Vibrance, saturation, dehaze colour lift and HSL on linear Rec.2020.
#[inline]
fn color_ops(v: [f32; 3], k: &Consts, st: &Statics) -> [f32; 3] {
    let mut v = [v[0].max(0.0), v[1].max(0.0), v[2].max(0.0)];
    // Hue/saturation as the user sees them: gamma-encoded sRGB (sqrt ~ gamma 2).
    let needs_hue = k.hsl.is_some() || k.vibrance != 0.0;
    let (hue, sat) = if needs_hue {
        let s = mat3(&st.rec2020_to_srgb, v);
        hue_sat([s[0].max(0.0).sqrt(), s[1].max(0.0).sqrt(), s[2].max(0.0).sqrt()])
    } else {
        (0.0, 0.0)
    };
    let mut chroma = k.saturation;
    if k.dehaze > 0.0 {
        chroma *= 1.0 + 0.25 * k.dehaze;
    }
    if k.vibrance > 0.0 {
        // Protect skin (orange band) and already saturated colours.
        let skin = 1.0 - 0.5 * (1.0 - ((hue - 25.0) / 25.0).abs()).max(0.0);
        chroma *= 1.0 + k.vibrance * (1.0 - sat) * (1.0 - sat) * 1.2 * skin;
    } else if k.vibrance < 0.0 {
        chroma *= 1.0 + k.vibrance * (1.0 - 0.5 * sat);
    }
    let mut lum_ev = 0.0;
    if let Some(hsl) = &k.hsl {
        let (a, b, t) = band_weights(hue);
        let pick = |vals: &[f32; 8]| vals[a] + (vals[b] - vals[a]) * t;
        let (dh, ds, dl) = (pick(&hsl[0]), pick(&hsl[1]), pick(&hsl[2]));
        if dh != 0.0 {
            // Rotate around the grey axis: +30 degrees at +100 (towards the next band).
            let theta = dh * 30.0f32.to_radians() * sat.min(1.0).sqrt();
            let avg = (v[0] + v[1] + v[2]) * (1.0 / 3.0);
            let c = [v[0] - avg, v[1] - avg, v[2] - avg];
            let kx = [(c[2] - c[1]), (c[0] - c[2]), (c[1] - c[0])].map(|q| q * 0.577_350_3);
            let (sn, cs) = theta.sin_cos();
            v = [avg + c[0] * cs + kx[0] * sn, avg + c[1] * cs + kx[1] * sn, avg + c[2] * cs + kx[2] * sn];
        }
        chroma *= (1.0 + ds).max(0.0);
        lum_ev = dl * 1.2 * (sat * 1.5).min(1.0);
    }
    let y = dot(LUMA_2020, v);
    let mut out = [y + (v[0] - y) * chroma, y + (v[1] - y) * chroma, y + (v[2] - y) * chroma];
    if lum_ev != 0.0 {
        let g = lum_ev.exp2();
        out = out.map(|c| c * g);
    }
    // Keep inside the non-negative cone by desaturating towards luminance.
    let mn = out[0].min(out[1]).min(out[2]);
    if mn < 0.0 {
        let yo = dot(LUMA_2020, out).max(0.0);
        let t = yo / (yo - mn);
        out = out.map(|c| yo + (c - yo) * t);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::{HslChannels, LutRef};

    fn color() -> ColorInfo {
        ColorInfo {
            as_shot_mul: Some([2.0, 1.0, 1.5]),
            daylight_mul: [2.0, 1.0, 1.5],
            rgb_cam: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            xyz_to_cam: [
                [3.2404542, -1.5371385, -0.4985314],
                [-0.969266, 1.8760108, 0.041556],
                [0.0556434, -0.2040259, 1.0572252],
            ],
        }
    }

    /// Horizontal grey ramp (camera values already "white balanced" by the inverse of
    /// the as-shot multipliers) with a coloured bottom half.
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

    fn run(adj: &ParametricAdjustments, lut: Option<&Lut>) -> RenderedImage {
        let (w, h) = (300, 200);
        let px = ramp(w, h);
        let c = color();
        render(&RenderInput { width: w, height: h, pixels: &px, color: &c, frame_long_edge: 300.0 }, adj, lut)
    }

    fn mean(img: &RenderedImage, c: usize) -> f32 {
        img.rgb.chunks(3).map(|p| f32::from(p[c])).sum::<f32>() / (img.rgb.len() / 3) as f32
    }

    fn px(img: &RenderedImage, x: usize, y: usize) -> [u8; 3] {
        let i = (y * img.width as usize + x) * 3;
        [img.rgb[i], img.rgb[i + 1], img.rgb[i + 2]]
    }

    #[test]
    fn base_curve_shape() {
        assert_eq!(base_curve(0.0), 0.0);
        assert!((base_curve(0.18) - GREY_OUT as f32).abs() < 1e-3);
        assert!((base_curve(WHITE as f32) - 1.0).abs() < 1e-4);
        let mut last = 0.0;
        for i in 1..1000 {
            let v = base_curve(i as f32 * 0.005);
            assert!(v >= last, "monotone");
            last = v;
        }
        let st = statics();
        for x in [0.001f32, 0.01, 0.18, 0.5, 1.0, 3.0] {
            assert!((st.curve.eval(x) - base_curve(x)).abs() < 2e-3, "{x}");
            assert!((st.encode.eval(x.min(1.0)) - srgb_encode(x.min(1.0))).abs() < 2e-3, "{x}");
        }
    }

    #[test]
    fn neutral_greys_stay_neutral_and_histogram_sums() {
        let img = run(&ParametricAdjustments::default(), None);
        assert_eq!(img.rgb.len(), 300 * 200 * 3);
        for x in [0, 50, 150, 299] {
            let p = px(&img, x, 10);
            assert!(
                (i32::from(p[0]) - i32::from(p[2])).abs() <= 1 && (i32::from(p[0]) - i32::from(p[1])).abs() <= 1,
                "{p:?}"
            );
        }
        // Monotone ramp.
        assert!(px(&img, 299, 10)[1] > px(&img, 150, 10)[1] && px(&img, 150, 10)[1] > px(&img, 10, 10)[1]);
        let n = 300 * 200;
        for ch in [&img.histogram.red, &img.histogram.green, &img.histogram.blue, &img.histogram.luma] {
            assert_eq!(ch.len(), 256);
            assert_eq!(ch.iter().sum::<u32>(), n);
        }
    }

    #[test]
    fn sliders_move_in_the_expected_direction() {
        let base = run(&ParametricAdjustments::default(), None);
        let g = |img: &RenderedImage| mean(img, 1);
        let adj = |f: &dyn Fn(&mut ParametricAdjustments)| {
            let mut a = ParametricAdjustments::default();
            f(&mut a);
            run(&a, None)
        };
        assert!(g(&adj(&|a| a.exposure = 1.0)) > g(&base) + 10.0);
        assert!(g(&adj(&|a| a.exposure = -1.0)) < g(&base) - 10.0);
        // Shadows lift dark pixels; highlights pull bright ones down.
        let dark = |img: &RenderedImage| px(img, 30, 10)[1];
        let bright = |img: &RenderedImage| px(img, 290, 10)[1];
        assert!(dark(&adj(&|a| a.shadows = 100.0)) > dark(&base));
        assert!(bright(&adj(&|a| a.highlights = -100.0)) < bright(&base));
        assert!(bright(&adj(&|a| a.whites = 100.0)) >= bright(&base));
        let deep = |img: &RenderedImage| px(img, 5, 10)[1];
        assert!(deep(&adj(&|a| a.blacks = -100.0)) < deep(&base));
        // Contrast: darks darker, brights brighter.
        let c = adj(&|a| a.contrast = 100.0);
        assert!(dark(&c) < dark(&base) && bright(&c) >= bright(&base));
        // Saturation -100 -> greyscale.
        let grey = adj(&|a| a.saturation = -100.0);
        let p = px(&grey, 200, 150);
        assert!((i32::from(p[0]) - i32::from(p[2])).abs() <= 2, "{p:?}");
        let sat = |img: &RenderedImage| {
            let p = px(img, 200, 150);
            i32::from(p[0]) - i32::from(p[2])
        };
        assert!(sat(&adj(&|a| a.saturation = 50.0)) > sat(&base));
        assert!(sat(&adj(&|a| a.vibrance = 80.0)) > sat(&base));
        // Warmer temperature -> redder greys.
        let warm = adj(&|a| a.white_balance = WhiteBalance::Custom { temperature_k: 9000.0, tint: 0.0 });
        let cool = adj(&|a| a.white_balance = WhiteBalance::Custom { temperature_k: 3000.0, tint: 0.0 });
        let rb = |img: &RenderedImage| {
            let p = px(img, 200, 10);
            i32::from(p[0]) - i32::from(p[2])
        };
        assert!(rb(&warm) > rb(&cool) + 20, "{} {}", rb(&warm), rb(&cool));
        // HSL: orange luminance -100 darkens the orange half only.
        let hsl = adj(&|a| a.hsl.luminance = HslChannels { orange: -100.0, red: -100.0, ..Default::default() });
        assert!(g(&hsl) < g(&base));
        assert_eq!(px(&hsl, 200, 10), px(&base, 200, 10), "greys unaffected");
        // Hue shift changes the colour but not greys.
        let hue = adj(&|a| a.hsl.hue = HslChannels { orange: 100.0, red: 100.0, ..Default::default() });
        assert_ne!(px(&hue, 200, 150), px(&base, 200, 150));
        assert_eq!(px(&hue, 200, 10), px(&base, 200, 10));
        // Local operators run and keep greys neutral.
        for f in [
            &(|a: &mut ParametricAdjustments| a.clarity = 80.0) as &dyn Fn(&mut ParametricAdjustments),
            &|a| a.texture = -60.0,
            &|a| a.dehaze = 50.0,
            &|a| a.dehaze = -50.0,
        ] {
            let img = adj(f);
            let p = px(&img, 150, 10);
            assert!((i32::from(p[0]) - i32::from(p[2])).abs() <= 2, "{p:?}");
        }
    }

    #[test]
    fn lut_applies_on_encoded_values_with_amount() {
        // 1D LUT that inverts: output = 1 - input.
        let lut = Lut::parse("LUT_1D_SIZE 2\n1 1 1\n0 0 0\n").unwrap();
        let base = run(&ParametricAdjustments::default(), None);
        let mut adj =
            ParametricAdjustments { lut: Some(LutRef { id: "inv".into(), amount: 100.0 }), ..Default::default() };
        let inv = run(&adj, Some(&lut));
        for (a, b) in base.rgb.iter().zip(&inv.rgb).step_by(97) {
            assert!((i32::from(*a) + i32::from(*b) - 255).abs() <= 1, "{a} {b}");
        }
        adj.lut = Some(LutRef { id: "inv".into(), amount: 50.0 });
        let half = run(&adj, Some(&lut));
        assert!(half.rgb.iter().step_by(97).all(|&v| (i32::from(v) - 128).abs() <= 1));
    }
}
