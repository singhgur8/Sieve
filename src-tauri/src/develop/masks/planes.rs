//! Per-pixel local parameter planes and the non-additive per-group blends.

use std::sync::Arc;

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
    let n = weights.width as usize * weights.height as usize;
    let active: Vec<(&MaskGroup, &Vec<f32>)> = masks
        .iter()
        .zip(&weights.groups)
        .filter_map(|(g, w)| w.as_ref().filter(|w| w.len() == n).map(|w| (g, w)))
        .collect();
    let mut planes: Vec<Option<Vec<f32>>> = vec![None; LocalParam::COUNT];
    for p in ALL_PARAMS {
        let parts: Vec<(&[f32], f32)> = active
            .iter()
            .map(|(g, w)| (w.as_slice(), param_value(&g.adjustments, p)))
            .filter(|(_, v)| *v != 0.0)
            .collect();
        if parts.is_empty() {
            continue;
        }
        let mut plane = vec![0.0f32; n];
        plane.par_chunks_mut(4096).enumerate().for_each(|(ci, chunk)| {
            let off = ci * 4096;
            let len = chunk.len();
            for (w, v) in &parts {
                for (o, x) in chunk.iter_mut().zip(&w[off..off + len]) {
                    *o += x * v;
                }
            }
        });
        planes[p as usize] = Some(plane);
    }
    let blends: Vec<GroupBlend> = active
        .iter()
        .filter(|(g, _)| g.adjustments.color.saturation > 0.0 || !curves_identity(&g.adjustments.tone_curve))
        .map(|(g, w)| GroupBlend {
            weight: Arc::new((*w).clone()),
            color: g.adjustments.color,
            tone_curve: g.adjustments.tone_curve.clone(),
            curve_refine_saturation: g.adjustments.curve_refine_saturation,
        })
        .collect();
    if planes.iter().all(Option::is_none) && blends.is_empty() {
        return None;
    }
    Some(LocalPlanes { width: weights.width, height: weights.height, planes, blends })
}

/// Rec.2020 luminance weights.
const Y2020: [f32; 3] = [0.2627, 0.6780, 0.0593];

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

/// Linear RGB of a fully saturated hue (degrees), normalized to luminance 1.
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
    let y = (lin[0] * Y2020[0] + lin[1] * Y2020[1] + lin[2] * Y2020[2]).max(1e-6);
    [lin[0] / y, lin[1] / y, lin[2] / y]
}

pub fn apply_group_blends(rgb: &mut [[f32; 3]], local: &LocalPlanes) {
    const SIZE: usize = 1024;
    for b in &local.blends {
        if b.weight.len() != rgb.len() {
            continue;
        }
        let c = &b.tone_curve;
        let has_curves = !curves_identity(c);
        let luts: Option<[Vec<f32>; 3]> = has_curves.then(|| {
            let master = curve_lut(&c.master, SIZE);
            let chan = |pts: &Vec<CurvePoint>| {
                let l = curve_lut(pts, SIZE);
                master.iter().map(|&x| lut_eval(&l, x)).collect::<Vec<f32>>()
            };
            [chan(&c.red), chan(&c.green), chan(&c.blue)]
        });
        let refine = (b.curve_refine_saturation / 100.0).clamp(0.0, 1.0);
        let tint = (b.color.saturation > 0.0).then(|| (tint_rgb(b.color.hue), b.color.saturation / 100.0 * 0.5));
        let weight = &b.weight;
        rgb.par_chunks_mut(4096).enumerate().for_each(|(ci, chunk)| {
            let off = ci * 4096;
            for (k, px) in chunk.iter_mut().enumerate() {
                let w = weight[off + k];
                if w <= 0.0 {
                    continue;
                }
                let c0 = *px;
                let mut c1 = c0;
                if let Some(l) = &luts {
                    let cur = [
                        srgb_decode(lut_eval(&l[0], srgb_encode(c0[0]))),
                        srgb_decode(lut_eval(&l[1], srgb_encode(c0[1]))),
                        srgb_decode(lut_eval(&l[2], srgb_encode(c0[2]))),
                    ];
                    let y0 = c0[0] * Y2020[0] + c0[1] * Y2020[1] + c0[2] * Y2020[2];
                    let y1 = cur[0] * Y2020[0] + cur[1] * Y2020[1] + cur[2] * Y2020[2];
                    // Refine saturation 0: luminance-only (hue/saturation kept), 100: per channel.
                    let k = if y0 > 1e-6 { y1 / y0 } else { 1.0 };
                    for i in 0..3 {
                        let lum = c0[i] * k;
                        c1[i] = lum + (cur[i] - lum) * refine;
                    }
                }
                if let Some((t, s)) = tint {
                    for i in 0..3 {
                        c1[i] *= 1.0 + (t[i] - 1.0) * s;
                    }
                }
                for i in 0..3 {
                    px[i] = (c0[i] + (c1[i] - c0[i]) * w).max(0.0);
                }
            }
        });
    }
}
