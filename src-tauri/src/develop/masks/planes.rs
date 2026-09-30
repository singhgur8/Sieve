//! Per-pixel local parameter planes and the non-additive per-group blends.

use std::sync::{Arc, OnceLock};

use rayon::prelude::*;

use super::{GroupBlend, GroupWeights, LocalParam, LocalPlanes};
use crate::ipc::types::{CurvePoint, LocalAdjustments, MaskGroup, PointCurves};

/// Every [`LocalParam`] in declaration (= index) order.
pub const ALL_PARAMS: [LocalParam; LocalParam::COUNT] = [
    LocalParam::Temperature,
    LocalParam::Tint,
    LocalParam::Exposure,
    LocalParam::Contrast,
    LocalParam::Highlights,
    LocalParam::Shadows,
    LocalParam::Whites,
    LocalParam::Blacks,
    LocalParam::Texture,
    LocalParam::Clarity,
    LocalParam::Dehaze,
    LocalParam::Hue,
    LocalParam::Saturation,
    LocalParam::Sharpness,
    LocalParam::Noise,
    LocalParam::Moire,
    LocalParam::Defringe,
];

pub fn param_value(a: &LocalAdjustments, p: LocalParam) -> f32 {
    match p {
        LocalParam::Temperature => a.temperature,
        LocalParam::Tint => a.tint,
        LocalParam::Exposure => a.exposure,
        LocalParam::Contrast => a.contrast,
        LocalParam::Highlights => a.highlights,
        LocalParam::Shadows => a.shadows,
        LocalParam::Whites => a.whites,
        LocalParam::Blacks => a.blacks,
        LocalParam::Texture => a.texture,
        LocalParam::Clarity => a.clarity,
        LocalParam::Dehaze => a.dehaze,
        LocalParam::Hue => a.hue,
        LocalParam::Saturation => a.saturation,
        LocalParam::Sharpness => a.sharpness,
        LocalParam::Noise => a.noise,
        LocalParam::Moire => a.moire,
        LocalParam::Defringe => a.defringe,
    }
}

fn curves_identity(c: &PointCurves) -> bool {
    [&c.master, &c.red, &c.green, &c.blue].iter().all(|v| PointCurves::is_identity(v))
}

pub fn build(masks: &[MaskGroup], weights: &GroupWeights) -> Option<LocalPlanes> {
    let shared: Vec<Option<Arc<Vec<f32>>>> = weights.groups.iter().map(|w| w.clone().map(Arc::new)).collect();
    from_shared(masks, &shared, weights.width, weights.height)
}

/// [`build`] over shared weight planes (`weights[i]` belongs to `masks[i]`; no copies).
pub fn from_shared(
    masks: &[MaskGroup],
    weights: &[Option<Arc<Vec<f32>>>],
    width: u32,
    height: u32,
) -> Option<LocalPlanes> {
    let n = width as usize * height as usize;
    let active: Vec<(&MaskGroup, &Arc<Vec<f32>>)> =
        masks.iter().zip(weights).filter_map(|(g, w)| w.as_ref().filter(|w| w.len() == n).map(|w| (g, w))).collect();
    let mut planes: Vec<Arc<Vec<f32>>> = Vec::new();
    let mut max_w: Vec<f32> = Vec::new();
    let mut terms: Vec<Vec<(usize, f32)>> = vec![Vec::new(); LocalParam::COUNT];
    for (g, w) in &active {
        let values: Vec<(LocalParam, f32)> =
            ALL_PARAMS.iter().map(|&p| (p, param_value(&g.adjustments, p))).filter(|(_, v)| *v != 0.0).collect();
        if values.is_empty() {
            continue;
        }
        let k = planes.len();
        planes.push(Arc::clone(w));
        max_w.push(w.par_iter().copied().reduce(|| 0.0f32, f32::max));
        for (p, v) in values {
            terms[p as usize].push((k, v));
        }
    }
    let blends: Vec<GroupBlend> = active
        .iter()
        .filter(|(g, _)| g.adjustments.color.saturation > 0.0 || !curves_identity(&g.adjustments.tone_curve))
        .map(|(g, w)| GroupBlend {
            weight: Arc::clone(w),
            color: g.adjustments.color,
            tone_curve: g.adjustments.tone_curve.clone(),
            curve_refine_saturation: g.adjustments.curve_refine_saturation,
        })
        .collect();
    if terms.iter().all(Vec::is_empty) && blends.is_empty() {
        return None;
    }
    let bounds = terms.iter().map(|t| t.iter().map(|&(k, v)| v.abs() * max_w[k]).sum()).collect();
    Some(LocalPlanes {
        width,
        height,
        weights: planes,
        terms,
        bounds,
        planes: (0..LocalParam::COUNT).map(|_| OnceLock::new()).collect(),
        blends,
    })
}

impl LocalPlanes {
    /// Some active group sets `param`.
    #[inline]
    pub fn has(&self, param: LocalParam) -> bool {
        !self.terms[param as usize].is_empty()
    }

    /// Upper bound of `|value(param, i)|` over the frame (0 when unset).
    pub fn bound(&self, param: LocalParam) -> f32 {
        self.bounds[param as usize]
    }

    /// The offset of `param` at pixel `i` (row-major index): sum over groups of weight x value.
    #[inline]
    pub fn value(&self, param: LocalParam, i: usize) -> f32 {
        let mut s = 0.0;
        for &(k, v) in &self.terms[param as usize] {
            s += self.weights[k][i] * v;
        }
        s
    }

    /// Materialized plane of `param` (computed on first use), `None` when unset.
    pub fn get(&self, param: LocalParam) -> Option<&[f32]> {
        if !self.has(param) {
            return None;
        }
        let plane = self.planes[param as usize].get_or_init(|| {
            let n = self.width as usize * self.height as usize;
            let mut plane = vec![0.0f32; n];
            plane.par_chunks_mut(4096).enumerate().for_each(|(ci, chunk)| {
                let off = ci * 4096;
                for (k, o) in chunk.iter_mut().enumerate() {
                    *o = self.value(param, off + k);
                }
            });
            plane
        });
        Some(plane)
    }

    /// Curve LUTs / tints of the group blends, ready for per-pixel use.
    pub fn prepare_blends(&self) -> PreparedBlends {
        PreparedBlends { blends: self.blends.iter().map(PreparedBlend::new).collect() }
    }
}

/// Luminance weights of linear ProPhoto (the pipeline's working space).
const Y_PP: [f32; 3] = crate::develop::parity::PROPHOTO_Y;

/// Linear sRGB (D65) -> linear ProPhoto (D50, Bradford).
fn srgb_to_prophoto() -> &'static [[f32; 3]; 3] {
    static M: OnceLock<[[f32; 3]; 3]> = OnceLock::new();
    M.get_or_init(|| {
        use crate::profiles::dcp::{bradford, invert3, mul_mm, xy_to_xyz, D50_XY};
        const SRGB_TO_XYZ: [[f64; 3]; 3] =
            [[0.4124564, 0.3575761, 0.1804375], [0.2126729, 0.7151522, 0.0721750], [0.0193339, 0.1191920, 0.9503041]];
        let d65_d50 = bradford(xy_to_xyz((0.3127, 0.3290)), xy_to_xyz(D50_XY));
        let xyz_to_pp = invert3(&crate::develop::camera::PROPHOTO_TO_XYZ).expect("invertible");
        mul_mm(&xyz_to_pp, &mul_mm(&d65_d50, &SRGB_TO_XYZ)).map(|r| r.map(|v| v as f32))
    })
}

#[inline]
fn srgb_encode(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x <= 0.003_130_8 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    }
}

#[inline]
fn srgb_decode(x: f32) -> f32 {
    if x <= 0.040_45 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}

/// Natural cubic spline through `points` (0..=255 axes; the DNG SDK's point-curve
/// interpolation, constant beyond the ends) sampled as a LUT over 0..=1.
pub fn curve_lut(points: &[CurvePoint], size: usize) -> Vec<f32> {
    let xs: Vec<f64> = points.iter().map(|p| f64::from(p[0]) / 255.0).collect();
    let ys: Vec<f64> = points.iter().map(|p| f64::from(p[1]) / 255.0).collect();
    let n = xs.len();
    let identity = || (0..size).map(|i| i as f32 / (size - 1) as f32).collect();
    if n < 2 || xs.windows(2).any(|w| w[1] <= w[0]) {
        return identity();
    }
    // Second derivatives (natural boundary).
    let mut m = vec![0.0f64; n];
    if n > 2 {
        let mut a = vec![0.0; n];
        let mut b = vec![0.0; n];
        let mut c = vec![0.0; n];
        let mut d = vec![0.0; n];
        for i in 1..n - 1 {
            let (h0, h1) = (xs[i] - xs[i - 1], xs[i + 1] - xs[i]);
            a[i] = h0;
            b[i] = 2.0 * (h0 + h1);
            c[i] = h1;
            d[i] = 6.0 * ((ys[i + 1] - ys[i]) / h1 - (ys[i] - ys[i - 1]) / h0);
        }
        // Thomas algorithm on rows 1..n-1.
        for i in 2..n - 1 {
            let k = a[i] / b[i - 1];
            b[i] -= k * c[i - 1];
            d[i] -= k * d[i - 1];
        }
        for i in (1..n - 1).rev() {
            let next = if i + 1 < n - 1 { m[i + 1] } else { 0.0 };
            m[i] = (d[i] - c[i] * next) / b[i];
        }
    }
    (0..size)
        .map(|i| {
            let x = i as f64 / (size - 1) as f64;
            let y = if x <= xs[0] {
                ys[0]
            } else if x >= xs[n - 1] {
                ys[n - 1]
            } else {
                let k = xs.windows(2).position(|w| x >= w[0] && x <= w[1]).unwrap_or(0);
                let h = xs[k + 1] - xs[k];
                let (t0, t1) = (xs[k + 1] - x, x - xs[k]);
                m[k] * t0.powi(3) / (6.0 * h)
                    + m[k + 1] * t1.powi(3) / (6.0 * h)
                    + (ys[k] / h - m[k] * h / 6.0) * t0
                    + (ys[k + 1] / h - m[k + 1] * h / 6.0) * t1
            };
            y.clamp(0.0, 1.0) as f32
        })
        .collect()
}

#[inline]
fn lut_eval(lut: &[f32], x: f32) -> f32 {
    let n = lut.len() - 1;
    let t = x.clamp(0.0, 1.0) * n as f32;
    let i = (t as usize).min(n - 1);
    lut[i] + (lut[i + 1] - lut[i]) * (t - i as f32)
}

/// Linear ProPhoto of a fully saturated sRGB hue (degrees), normalized to luminance 1.
fn tint_rgb(hue: f32) -> [f32; 3] {
    let h = (hue.rem_euclid(360.0)) / 60.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    let (r, g, b) = match h as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    let lin = [srgb_decode(r), srgb_decode(g), srgb_decode(b)];
    let m = srgb_to_prophoto();
    let pp = [0, 1, 2].map(|k| m[k][0] * lin[0] + m[k][1] * lin[1] + m[k][2] * lin[2]);
    let y = (pp[0] * Y_PP[0] + pp[1] * Y_PP[1] + pp[2] * Y_PP[2]).max(1e-6);
    [pp[0] / y, pp[1] / y, pp[2] / y]
}

/// One group blend with its curve LUTs built.
struct PreparedBlend {
    weight: Arc<Vec<f32>>,
    luts: Option<[Vec<f32>; 3]>,
    refine: f32,
    tint: Option<([f32; 3], f32)>,
}

impl PreparedBlend {
    fn new(b: &GroupBlend) -> Self {
        const SIZE: usize = 1024;
        let c = &b.tone_curve;
        let luts = (!curves_identity(c)).then(|| {
            let master = curve_lut(&c.master, SIZE);
            let chan = |pts: &Vec<CurvePoint>| {
                let l = curve_lut(pts, SIZE);
                master.iter().map(|&x| lut_eval(&l, x)).collect::<Vec<f32>>()
            };
            [chan(&c.red), chan(&c.green), chan(&c.blue)]
        });
        PreparedBlend {
            weight: Arc::clone(&b.weight),
            luts,
            refine: (b.curve_refine_saturation / 100.0).clamp(0.0, 1.0),
            tint: (b.color.saturation > 0.0).then(|| (tint_rgb(b.color.hue), b.color.saturation / 100.0 * 0.5)),
        }
    }

    #[inline]
    fn apply(&self, i: usize, c0: [f32; 3]) -> [f32; 3] {
        let w = self.weight.get(i).copied().unwrap_or(0.0);
        if w <= 0.0 {
            return c0;
        }
        let mut c1 = c0;
        if let Some(l) = &self.luts {
            let cur = [
                srgb_decode(lut_eval(&l[0], srgb_encode(c0[0]))),
                srgb_decode(lut_eval(&l[1], srgb_encode(c0[1]))),
                srgb_decode(lut_eval(&l[2], srgb_encode(c0[2]))),
            ];
            let y0 = c0[0] * Y_PP[0] + c0[1] * Y_PP[1] + c0[2] * Y_PP[2];
            let y1 = cur[0] * Y_PP[0] + cur[1] * Y_PP[1] + cur[2] * Y_PP[2];
            // Refine saturation 0: luminance-only (hue/saturation kept), 100: per channel.
            let k = if y0 > 1e-6 { y1 / y0 } else { 1.0 };
            for i in 0..3 {
                let lum = c0[i] * k;
                c1[i] = lum + (cur[i] - lum) * self.refine;
            }
        }
        if let Some((t, s)) = self.tint {
            for (c, t) in c1.iter_mut().zip(t) {
                *c *= 1.0 + (t - 1.0) * s;
            }
        }
        [0, 1, 2].map(|k| (c0[k] + (c1[k] - c0[k]) * w).max(0.0))
    }
}

/// Group blends ready for the pipeline's per-pixel loop.
pub struct PreparedBlends {
    blends: Vec<PreparedBlend>,
}

impl PreparedBlends {
    pub fn is_empty(&self) -> bool {
        self.blends.is_empty()
    }

    /// Applies every blend, in group order, to display-linear ProPhoto `c` at pixel `i`.
    #[inline]
    pub fn apply(&self, i: usize, c: [f32; 3]) -> [f32; 3] {
        self.blends.iter().fold(c, |c, b| b.apply(i, c))
    }
}

pub fn apply_group_blends(rgb: &mut [[f32; 3]], local: &LocalPlanes) {
    let prepared = local.prepare_blends();
    if prepared.is_empty() || local.width as usize * local.height as usize != rgb.len() {
        return;
    }
    rgb.par_chunks_mut(4096).enumerate().for_each(|(ci, chunk)| {
        let off = ci * 4096;
        for (k, px) in chunk.iter_mut().enumerate() {
            *px = prepared.apply(off + k, *px);
        }
    });
}
