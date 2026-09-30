//! Appearance features of previews for scene detection. Owned by vision-ml-dev.
//!
//! Computed from the 2048 px preview JPEG (the camera's embedded rendering, i.e. its own
//! white balance and exposure), DCT-downscaled to ~256-512 px by TurboJPEG: a few ms per
//! image. Only global statistics are kept, so framing changes inside one light (close-up vs
//! wide) move them less than a change of light does.

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

use rayon::prelude::*;

use super::{color, DetectFrame, Progress, SceneFeatures};
use crate::ipc::types::ImageId;
use crate::raw::turbo;

/// Bins of [`SceneFeatures::luma_hist`] (encoded luma 0..=1).
pub const LUMA_BINS: usize = 16;
/// Bins per axis of [`SceneFeatures::ab_hist`] (Oklab a/b in `-AB_RANGE..AB_RANGE`, clamped).
pub const AB_BINS: usize = 8;
pub const AB_RANGE: f32 = 0.2;
/// Minimum long edge decoded (DCT scaling picks the smallest n/8 size >= this).
const DECODE_EDGE: u32 = 256;
/// At most this many pixels are sampled.
const MAX_SAMPLES: usize = 40_000;
const MAX_PIXELS: u64 = 120_000_000;

/// Contract. Features of the 2048 px preview JPEG at `preview_path` (orientation applied;
/// downsample first, e.g. ~256 px, it only needs global statistics). Errors are human-readable.
pub fn compute(preview_path: &Path) -> Result<SceneFeatures, String> {
    let bytes = std::fs::read(preview_path).map_err(|e| format!("read preview {}: {e}", preview_path.display()))?;
    let img = turbo::decode_rgb(&bytes, DECODE_EDGE, MAX_PIXELS)?;
    Ok(from_rgb(&img.pixels, img.width as usize, img.height as usize))
}

/// Features of interleaved 8-bit sRGB pixels.
pub fn from_rgb(rgb: &[u8], w: usize, h: usize) -> SceneFeatures {
    let lin = color::srgb_to_linear_table();
    let n = (w * h).min(rgb.len() / 3);
    let stride = ((n as f64 / MAX_SAMPLES as f64).sqrt().ceil() as usize).max(1);
    let mut luma_hist = vec![0.0f32; LUMA_BINS];
    let mut ab_hist = vec![0.0f32; AB_BINS * AB_BINS];
    let (mut sum_log, mut count) = (0.0f64, 0usize);
    let mut sum_lab = [0.0f64; 3];
    for y in (0..h).step_by(stride) {
        for x in (0..w).step_by(stride) {
            let i = (y * w + x) * 3;
            if i + 3 > n * 3 {
                continue;
            }
            let lrgb = [lin[rgb[i] as usize], lin[rgb[i + 1] as usize], lin[rgb[i + 2] as usize]];
            let yl = color::luma(lrgb);
            sum_log += f64::from(yl.max(color::MIN_LUMA).log2());
            let e = color::srgb_encode(yl);
            luma_hist[((e * LUMA_BINS as f32) as usize).min(LUMA_BINS - 1)] += 1.0;
            let lab = color::oklab(lrgb);
            let bin = |v: f32| {
                let t = (v + AB_RANGE) / (2.0 * AB_RANGE);
                ((t * AB_BINS as f32).floor() as isize).clamp(0, AB_BINS as isize - 1) as usize
            };
            ab_hist[bin(lab[1]) * AB_BINS + bin(lab[2])] += 1.0;
            for c in 0..3 {
                sum_lab[c] += f64::from(lab[c]);
            }
            count += 1;
        }
    }
    let nf = count.max(1) as f32;
    for v in luma_hist.iter_mut().chain(ab_hist.iter_mut()) {
        *v /= nf;
    }
    let mean = sum_lab.map(|v| color::finite((v / count.max(1) as f64) as f32, 0.0));
    SceneFeatures {
        log_mean_luma: if count == 0 { -14.0 } else { color::finite((sum_log / count as f64) as f32, -14.0) },
        luma_hist,
        ab_hist,
        mean_oklab: mean,
    }
}

/// Contract. Blocking; parallel (rayon). Computes features for every frame with
/// `features == None` and a `preview_path`, fills them in and returns the newly computed ones
/// (for [`super::store::save_features`]). Failures leave `features = None` (detection then
/// groups that frame by time only). Calls `progress(done, total)` over the frames computed.
pub fn compute_missing(frames: &mut [DetectFrame], progress: Progress) -> Vec<(ImageId, SceneFeatures)> {
    let todo: Vec<&mut DetectFrame> =
        frames.iter_mut().filter(|f| f.features.is_none() && f.preview_path.is_some()).collect();
    let total = todo.len() as u32;
    if total == 0 {
        progress(0, 0);
        return Vec::new();
    }
    let done = AtomicU32::new(0);
    let computed: Vec<(ImageId, SceneFeatures)> = todo
        .into_par_iter()
        .filter_map(|frame| {
            let path = frame.preview_path.as_deref()?;
            let result = compute(path);
            let d = done.fetch_add(1, Ordering::Relaxed) + 1;
            progress(d, total);
            match result {
                Ok(f) => {
                    frame.features = Some(f.clone());
                    Some((frame.id, f))
                }
                Err(e) => {
                    eprintln!("scene features {}: {e}", path.display());
                    None
                }
            }
        })
        .collect();
    computed
}

/// Appearance similarity 0..=1 of two feature sets: histogram intersection of the tone
/// (luma) and colour (Oklab a/b) distributions, discounted by the difference in overall
/// brightness (EV) and mean colour. 1 = identical statistics.
pub fn similarity(a: &SceneFeatures, b: &SceneFeatures) -> f32 {
    let overlap = |x: &[f32], y: &[f32]| -> f32 {
        if x.len() != y.len() || x.is_empty() {
            return 0.0;
        }
        x.iter().zip(y).map(|(p, q)| (p.max(0.0) * q.max(0.0)).sqrt()).sum::<f32>().clamp(0.0, 1.0)
    };
    let tone = overlap(&a.luma_hist, &b.luma_hist);
    let colour = overlap(&a.ab_hist, &b.ab_hist);
    // Level terms: 1 EV of brightness or 0.05 Oklab a/b of mean colour cost ~0.3.
    let ev = (a.log_mean_luma - b.log_mean_luma).abs();
    let (da, db) = (a.mean_oklab[1] - b.mean_oklab[1], a.mean_oklab[2] - b.mean_oklab[2]);
    let cast = (da * da + db * db).sqrt();
    let level = (-(ev / 2.0).powi(2) - (cast / 0.08).powi(2)).exp();
    let s = 0.35 * tone + 0.35 * colour + 0.3 * level;
    color::finite(s, 0.0).clamp(0.0, 1.0)
}

/// Element-wise weighted mean of feature sets (a group's running appearance).
pub fn blend(acc: &SceneFeatures, add: &SceneFeatures, weight: f32) -> SceneFeatures {
    let w = weight.clamp(0.0, 1.0);
    let mix = |x: &[f32], y: &[f32]| -> Vec<f32> {
        if x.len() != y.len() {
            return y.to_vec();
        }
        x.iter().zip(y).map(|(p, q)| p + (q - p) * w).collect()
    };
    SceneFeatures {
        log_mean_luma: acc.log_mean_luma + (add.log_mean_luma - acc.log_mean_luma) * w,
        luma_hist: mix(&acc.luma_hist, &add.luma_hist),
        ab_hist: mix(&acc.ab_hist, &add.ab_hist),
        mean_oklab: [0, 1, 2].map(|c| acc.mean_oklab[c] + (add.mean_oklab[c] - acc.mean_oklab[c]) * w),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(rgb: [u8; 3], n: usize) -> Vec<u8> {
        rgb.iter().copied().cycle().take(n * 3).collect()
    }

    #[test]
    fn features_of_flat_images() {
        let f = from_rgb(&flat([128, 128, 128], 64 * 64), 64, 64);
        assert!((f.luma_hist.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert!((f.ab_hist.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert!((f.log_mean_luma - 0.2158f32.log2()).abs() < 0.01);
        assert_eq!(f.luma_hist[8], 1.0);
        assert!(f.mean_oklab[1].abs() < 1e-3 && f.mean_oklab[2].abs() < 1e-3);

        let g = from_rgb(&flat([128, 128, 128], 64 * 64), 64, 64);
        assert!((similarity(&f, &g) - 1.0).abs() < 1e-5);
        let warm = from_rgb(&flat([200, 140, 80], 64 * 64), 64, 64);
        let dark = from_rgb(&flat([30, 30, 30], 64 * 64), 64, 64);
        assert!(similarity(&f, &warm) < 0.5, "{}", similarity(&f, &warm));
        assert!(similarity(&f, &dark) < 0.4, "{}", similarity(&f, &dark));
        let half = blend(&f, &dark, 0.5);
        assert!((half.luma_hist.iter().sum::<f32>() - 1.0).abs() < 1e-5);
    }
}
