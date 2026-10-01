//! Evaluation helpers shared by `examples/style_eval.rs` and the in-app validation of
//! `ml::style` (`StyleValidation`): mean CIEDE2000 between two renders and the older
//! reference "Auto tone" (the validation's fallback when `develop::auto::auto_tone` fails;
//! an extra column in `style_eval`).

use crate::develop::pipeline::RenderedImage;
use crate::ipc::types::{ImageFormat, ParametricAdjustments};

use super::RenderFeatures;

/// sRGB (8-bit values as f32) -> CIELAB (D65).
pub fn lab(rgb: [f32; 3]) -> [f64; 3] {
    let lin = rgb.map(|c| {
        let e = f64::from(c) / 255.0;
        if e <= 0.04045 {
            e / 12.92
        } else {
            ((e + 0.055) / 1.055).powf(2.4)
        }
    });
    let x = (0.4124564 * lin[0] + 0.3575761 * lin[1] + 0.1804375 * lin[2]) / 0.95047;
    let y = 0.2126729 * lin[0] + 0.7151522 * lin[1] + 0.0721750 * lin[2];
    let z = (0.0193339 * lin[0] + 0.1191920 * lin[1] + 0.9503041 * lin[2]) / 1.08883;
    let f = |t: f64| if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIEDE2000 colour difference.
pub fn de2000(l1: [f64; 3], l2: [f64; 3]) -> f64 {
    let (lp1, a1, b1) = (l1[0], l1[1], l1[2]);
    let (lp2, a2, b2) = (l2[0], l2[1], l2[2]);
    let c1 = (a1 * a1 + b1 * b1).sqrt();
    let c2 = (a2 * a2 + b2 * b2).sqrt();
    let cm = (c1 + c2) / 2.0;
    let g = 0.5 * (1.0 - (cm.powi(7) / (cm.powi(7) + 25f64.powi(7))).sqrt());
    let (a1p, a2p) = (a1 * (1.0 + g), a2 * (1.0 + g));
    let (c1p, c2p) = ((a1p * a1p + b1 * b1).sqrt(), (a2p * a2p + b2 * b2).sqrt());
    let h = |b: f64, a: f64| {
        let v = b.atan2(a).to_degrees();
        if v < 0.0 {
            v + 360.0
        } else {
            v
        }
    };
    let (h1p, h2p) = (h(b1, a1p), h(b2, a2p));
    let dl = lp2 - lp1;
    let dc = c2p - c1p;
    let mut dh = h2p - h1p;
    if c1p * c2p != 0.0 {
        if dh > 180.0 {
            dh -= 360.0;
        } else if dh < -180.0 {
            dh += 360.0;
        }
    } else {
        dh = 0.0;
    }
    let dhh = 2.0 * (c1p * c2p).sqrt() * (dh.to_radians() / 2.0).sin();
    let lm = (lp1 + lp2) / 2.0;
    let cmp = (c1p + c2p) / 2.0;
    let hm = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * (hm - 30.0).to_radians().cos()
        + 0.24 * (2.0 * hm).to_radians().cos()
        + 0.32 * (3.0 * hm + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hm - 63.0).to_radians().cos();
    let dtheta = 30.0 * (-((hm - 275.0) / 25.0).powi(2)).exp();
    let rc = 2.0 * (cmp.powi(7) / (cmp.powi(7) + 25f64.powi(7))).sqrt();
    let sl = 1.0 + 0.015 * (lm - 50.0).powi(2) / (20.0 + (lm - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * cmp;
    let sh = 1.0 + 0.015 * cmp * t;
    let rt = -(2.0 * dtheta).to_radians().sin() * rc;
    ((dl / sl).powi(2) + (dc / sc).powi(2) + (dhh / sh).powi(2) + rt * (dc / sc) * (dhh / sh)).sqrt()
}

fn box3(px: &[u8], w: usize, h: usize) -> Vec<[f32; 3]> {
    let mut out = vec![[0.0f32; 3]; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f32; 3];
            let mut n = 0.0;
            for yy in y.saturating_sub(1)..(y + 2).min(h) {
                for xx in x.saturating_sub(1)..(x + 2).min(w) {
                    let p = &px[(yy * w + xx) * 3..];
                    for c in 0..3 {
                        acc[c] += f32::from(p[c]);
                    }
                    n += 1.0;
                }
            }
            out[y * w + x] = acc.map(|v| v / n);
        }
    }
    out
}

/// Mean CIEDE2000 of two same-sized renders (3x3 box filter, every 3rd pixel; as
/// `parity_eval`). `None` if the sizes differ.
pub fn mean_de(a: &RenderedImage, b: &RenderedImage) -> Option<f64> {
    if (a.width, a.height) != (b.width, b.height) {
        return None;
    }
    let (w, h) = (a.width as usize, a.height as usize);
    let (x, y) = (box3(&a.rgb, w, h), box3(&b.rgb, w, h));
    let d: Vec<f64> = x.iter().zip(&y).step_by(3).map(|(p, q)| de2000(lab(*p), lab(*q))).collect();
    Some(d.iter().sum::<f64>() / d.len().max(1) as f64)
}

/// Long edge of the renders [`reference_auto_tone`] measures.
pub const AUTO_MEASURE_EDGE: u32 = 320;

/// Reference "Auto tone" baseline (documented in docs/decisions.md 2026-09-30): Lightroom-style
/// global corrections from the neutral render, as-shot WB. Exposure by 3 secant steps on the
/// rendered log-mean luminance towards -2.5 (display middle grey), capped at +-2 EV, then
/// Highlights / Shadows / Whites / Blacks from the rendered percentiles, Vibrance +10.
/// `measure` renders the given settings (no crop) at [`AUTO_MEASURE_EDGE`] and measures them.
pub fn reference_auto_tone<E>(
    format: ImageFormat,
    mut measure: impl FnMut(&ParametricAdjustments) -> Result<RenderFeatures, E>,
) -> Result<ParametricAdjustments, E> {
    let mut adj = ParametricAdjustments::defaults_for(format);
    let target = -2.5f32;
    let (mut e0, mut f0) = (0.0f32, measure(&adj)?.log_mean_luma - target);
    let mut e1 = (-f0).clamp(-2.0, 2.0);
    for _ in 0..3 {
        adj.exposure = e1;
        let f1 = measure(&adj)?.log_mean_luma - target;
        if f1.abs() < 0.05 || (f1 - f0).abs() < 1e-4 {
            break;
        }
        let e2 = (e1 - f1 * (e1 - e0) / (f1 - f0)).clamp(-2.0, 2.0);
        (e0, f0, e1) = (e1, f1, e2);
    }
    adj.exposure = (e1 * 100.0).round() / 100.0;
    let m = measure(&adj)?;
    let enc = |lp: f32| {
        let y = lp.exp2();
        if y <= 0.003_130_8 {
            y * 12.92
        } else {
            1.055 * y.powf(1.0 / 2.4) - 0.055
        }
    };
    let [p1, p10, _, p90, p99] = m.log_percentiles.map(enc);
    adj.highlights = -(m.clipped_highlights * 400.0 + (p90 - 0.72).max(0.0) * 200.0).clamp(0.0, 70.0).round();
    adj.shadows = ((0.22 - p10).max(0.0) * 220.0).clamp(0.0, 60.0).round();
    adj.whites = ((0.93 - p99) * 150.0).clamp(-40.0, 40.0).round();
    adj.blacks = ((0.02 - p1) * 400.0).clamp(-40.0, 30.0).round();
    adj.vibrance = 10.0;
    Ok(adj)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn de2000_reference_pairs() {
        // Sharma et al. (2005) test data, pairs 1 and 7.
        assert!((de2000([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485]) - 2.0425).abs() < 1e-3);
        assert!((de2000([50.0, 0.0, 0.0], [50.0, -1.0, 2.0]) - 2.3669).abs() < 1e-3);
        assert_eq!(de2000([40.0, 10.0, 5.0], [40.0, 10.0, 5.0]), 0.0);
    }
}
