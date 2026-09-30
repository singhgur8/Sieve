//! Render-space image statistics. [`render_stats`] (glue, architect) renders through the
//! develop cache; [`measure`] (vision-ml-dev) computes the numbers from the 8-bit output.

use super::{color, STATS_MAX_EDGE};
use crate::develop::pipeline::RenderedImage;
use crate::develop::{DevelopCache, SourceImage};
use crate::ipc::error::AppResult;
use crate::ipc::types::{
    ImageId, ImageStats, LumaPercentiles, NeutralEstimate, NormRect, OklabColor, ParametricAdjustments, WhiteBalance,
    WhiteBalanceValues,
};
use crate::lut::LutLibrary;

/// Contract. Blocking. Renders `src` with `adjustments` (validated by the caller) at
/// [`STATS_MAX_EDGE`] (cropped to `region`, oriented normalized coords, if given) through the
/// editor's source + pipeline (decoding and caching the RAW if needed) and measures it.
pub fn render_stats(
    cache: &DevelopCache,
    luts: &LutLibrary,
    src: &SourceImage,
    adjustments: &ParametricAdjustments,
    region: Option<NormRect>,
) -> AppResult<ImageStats> {
    let rendered = cache.render_image(src, adjustments, region, STATS_MAX_EDGE, luts)?;
    let mut stats = measure(src.id, &rendered.image, region);
    stats.white_balance = effective_white_balance(adjustments, rendered.as_shot);
    stats.as_shot = rendered.as_shot;
    stats.lut_missing = rendered.lut_missing;
    Ok(stats)
}

/// White balance `adjustments` resolve to: `custom` values, or the camera's as-shot values.
pub fn effective_white_balance(
    adjustments: &ParametricAdjustments,
    as_shot: Option<WhiteBalanceValues>,
) -> Option<WhiteBalanceValues> {
    match adjustments.white_balance {
        WhiteBalance::Custom { temperature_k, tint } => Some(WhiteBalanceValues { temperature_k, tint }),
        WhiteBalance::AsShot => as_shot,
    }
}

/// Contract (pure). Statistics of an 8-bit sRGB render (see `ImageStats` field docs for the
/// definitions: linearize with the sRGB EOTF, Y with Rec.709 weights, Oklab from linear sRGB).
/// Neutral estimate: e.g. mean of near-neutral, mid-to-bright, unclipped pixels (low Oklab
/// chroma after removing the global cast iteratively), grey-world fallback with
/// `coverage = 0`. Leave `white_balance`, `as_shot`, `lut_missing` at `None`/`false` (the
/// caller fills them); set `image_id`, `region` from the arguments. Never NaN.
pub fn measure(image_id: ImageId, image: &RenderedImage, region: Option<NormRect>) -> ImageStats {
    let (w, h) = (image.width as usize, image.height as usize);
    let n = (w * h).min(image.rgb.len() / 3);
    let px = &image.rgb[..n * 3];
    let lin = color::srgb_to_linear_table();

    // Whole-frame luminance statistics.
    let mut sum_y = 0.0f64;
    let mut sum_log = 0.0f64;
    let mut hist = vec![0u32; HIST_BINS];
    let (mut clip_hi, mut clip_lo) = (0u32, 0u32);
    for p in px.as_chunks::<3>().0 {
        let y = color::luma([lin[p[0] as usize], lin[p[1] as usize], lin[p[2] as usize]]);
        let ly = y.max(color::MIN_LUMA).log2();
        sum_y += f64::from(y);
        sum_log += f64::from(ly);
        hist[hist_bin(ly)] += 1;
        if p[0] >= 254 || p[1] >= 254 || p[2] >= 254 {
            clip_hi += 1;
        }
        if p[0] <= 1 && p[1] <= 1 && p[2] <= 1 {
            clip_lo += 1;
        }
    }
    let nf = n.max(1) as f64;
    let percentile = |q: f64| hist_percentile(&hist, n, q);

    let samples = sample_pixels(px, w, h);
    let neutral = neutral_estimate(&samples);
    let ns = samples.len().max(1) as f64;
    let mut mean_lab = [0.0f64; 3];
    for s in &samples {
        for (m, v) in mean_lab.iter_mut().zip(s.lab) {
            *m += f64::from(v);
        }
    }
    let mean_lab = mean_lab.map(|v| (v / ns) as f32);

    ImageStats {
        image_id,
        region,
        width: image.width,
        height: image.height,
        mean_luma: color::finite((sum_y / nf) as f32, 0.0),
        log_mean_luma: if n == 0 { color::MIN_LUMA.log2() } else { color::finite((sum_log / nf) as f32, -14.0) },
        percentiles: LumaPercentiles {
            p1: percentile(0.01),
            p10: percentile(0.10),
            p50: percentile(0.50),
            p90: percentile(0.90),
            p99: percentile(0.99),
        },
        clipped_highlights: (f64::from(clip_hi) / nf) as f32,
        clipped_shadows: (f64::from(clip_lo) / nf) as f32,
        mean_oklab: OklabColor {
            l: color::finite(mean_lab[0], 0.0),
            a: color::finite(mean_lab[1], 0.0),
            b: color::finite(mean_lab[2], 0.0),
        },
        neutral,
        white_balance: None,
        as_shot: None,
        lut_missing: false,
    }
}

/// Log2-luminance histogram for percentiles: `MIN_LUMA`..1 in `HIST_BINS` bins (~0.007 EV).
const HIST_BINS: usize = 2048;
const HIST_LO: f32 = -14.0;

fn hist_bin(log_y: f32) -> usize {
    let t = (log_y - HIST_LO) / -HIST_LO;
    ((t * HIST_BINS as f32) as isize).clamp(0, HIST_BINS as isize - 1) as usize
}

/// Linear luminance at quantile `q` (interpolated inside the bin).
fn hist_percentile(hist: &[u32], n: usize, q: f64) -> f32 {
    if n == 0 {
        return 0.0;
    }
    let target = q * n as f64;
    let mut acc = 0.0f64;
    for (i, &c) in hist.iter().enumerate() {
        let next = acc + f64::from(c);
        if next >= target && c > 0 {
            let frac = ((target - acc) / f64::from(c)).clamp(0.0, 1.0) as f32;
            let log_y = HIST_LO + (i as f32 + frac) * (-HIST_LO / HIST_BINS as f32);
            return log_y.exp2().min(1.0);
        }
        acc = next;
    }
    1.0
}

/// A sampled pixel for the colour statistics.
struct Sample {
    lin: [f32; 3],
    lab: [f32; 3],
    /// Smooth 0..=1 eligibility as a neutral candidate (lightness ramp, unclipped).
    elig: f32,
}

/// Colour statistics run on a regular grid of at most ~`MAX_SAMPLES` pixels.
const MAX_SAMPLES: usize = 80_000;

fn sample_pixels(px: &[u8], w: usize, h: usize) -> Vec<Sample> {
    let lin = color::srgb_to_linear_table();
    let n = w * h;
    let stride = ((n as f64 / MAX_SAMPLES as f64).sqrt().ceil() as usize).max(1);
    let mut out = Vec::with_capacity(n / (stride * stride) + w + h);
    for y in (stride / 2..h).step_by(stride) {
        for x in (stride / 2..w).step_by(stride) {
            let i = (y * w + x) * 3;
            let p = &px[i..i + 3];
            let rgb = [lin[p[0] as usize], lin[p[1] as usize], lin[p[2] as usize]];
            let lab = color::oklab(rgb);
            let unclipped = p[0].max(p[1]).max(p[2]) <= 250;
            // Lightness ramp: dark pixels carry noise / little colour information.
            let ramp = ((lab[0] - NEUTRAL_L_MIN) / NEUTRAL_L_RAMP).clamp(0.0, 1.0);
            let elig = if unclipped { ramp } else { 0.0 };
            out.push(Sample { lin: rgb, lab, elig });
        }
    }
    out
}

/// Neutral candidates start at this Oklab lightness and are fully weighted `NEUTRAL_L_RAMP` above.
const NEUTRAL_L_MIN: f32 = 0.25;
const NEUTRAL_L_RAMP: f32 = 0.20;
/// Mean-shift kernel widths (Oklab a/b), coarse to fine.
const NEUTRAL_SIGMAS: [f32; 5] = [0.06, 0.04, 0.03, 0.025, 0.02];
/// Below this effective share of sampled pixels the estimate falls back to grey-world.
pub const MIN_NEUTRAL_COVERAGE: f32 = 0.01;
/// Neutral chromaticity is reported at this luminance (independent of brightness).
const NEUTRAL_REPORT_Y: f32 = 0.5;

/// Estimate of the render's neutral colour: a mean-shift in Oklab a/b over light, unclipped
/// pixels, started at their grey-world mean, so it settles on the dominant low-chroma cluster
/// around the global cast (white/grey surfaces under the scene light) rather than on saturated
/// objects. Gaussian weights keep it a smooth function of the adjustments (the solver
/// differentiates through it). `a`/`b`/`x`/`y` describe the chromaticity of the weighted mean
/// linear colour at a fixed luminance, so exposure changes alone do not move it.
fn neutral_estimate(samples: &[Sample]) -> NeutralEstimate {
    let grey_world = |weight: &dyn Fn(&Sample) -> f32| -> Option<[f64; 3]> {
        let mut acc = [0.0f64; 3];
        let mut sw = 0.0f64;
        for s in samples {
            let wgt = f64::from(weight(s));
            if wgt > 0.0 {
                for (a, v) in acc.iter_mut().zip(s.lin) {
                    *a += wgt * f64::from(v);
                }
                sw += wgt;
            }
        }
        (sw > 0.0).then(|| acc.map(|v| v / sw))
    };
    let finish = |rgb: [f64; 3], coverage: f32| -> NeutralEstimate {
        let rgb = rgb.map(|v| v as f32);
        let y = color::luma(rgb);
        if !(y.is_finite() && y > 1e-9) {
            return NeutralEstimate { x: 0.3127, y: 0.3290, a: 0.0, b: 0.0, coverage: 0.0 };
        }
        let k = NEUTRAL_REPORT_Y / y;
        let lab = color::oklab(rgb.map(|c| c * k));
        let (x, yy) = color::xy(rgb);
        NeutralEstimate {
            x: color::finite(x, 0.3127),
            y: color::finite(yy, 0.3290),
            a: color::finite(lab[1], 0.0),
            b: color::finite(lab[2], 0.0),
            coverage: coverage.clamp(0.0, 1.0),
        }
    };

    let Some(_) = grey_world(&|s| s.elig) else {
        // Nothing light and unclipped: grey-world over the whole frame.
        return finish(grey_world(&|_| 1.0).unwrap_or([1.0; 3]), 0.0);
    };
    // Start: grey-world a/b of the eligible pixels.
    let mut c = {
        let (mut sa, mut sb, mut sw) = (0.0f64, 0.0f64, 0.0f64);
        for s in samples.iter().filter(|s| s.elig > 0.0) {
            let wgt = f64::from(s.elig);
            sa += wgt * f64::from(s.lab[1]);
            sb += wgt * f64::from(s.lab[2]);
            sw += wgt;
        }
        [(sa / sw) as f32, (sb / sw) as f32]
    };
    let kernel = |s: &Sample, c: [f32; 2], sigma: f32| -> f32 {
        if s.elig <= 0.0 {
            return 0.0;
        }
        let (da, db) = (s.lab[1] - c[0], s.lab[2] - c[1]);
        s.elig * (-(da * da + db * db) / (2.0 * sigma * sigma)).exp()
    };
    for &sigma in &NEUTRAL_SIGMAS {
        let (mut sa, mut sb, mut sw) = (0.0f64, 0.0f64, 0.0f64);
        for s in samples {
            let wgt = f64::from(kernel(s, c, sigma));
            sa += wgt * f64::from(s.lab[1]);
            sb += wgt * f64::from(s.lab[2]);
            sw += wgt;
        }
        if sw <= 1e-9 {
            break;
        }
        c = [(sa / sw) as f32, (sb / sw) as f32];
    }
    let sigma = NEUTRAL_SIGMAS[NEUTRAL_SIGMAS.len() - 1];
    let total: f32 = samples.iter().map(|s| kernel(s, c, sigma)).sum();
    let coverage = total / samples.len().max(1) as f32;
    if coverage < MIN_NEUTRAL_COVERAGE {
        let rgb = grey_world(&|s| s.elig).unwrap_or([1.0; 3]);
        return finish(rgb, 0.0);
    }
    let rgb = grey_world(&|s| kernel(s, c, sigma)).unwrap_or([1.0; 3]);
    finish(rgb, coverage)
}
