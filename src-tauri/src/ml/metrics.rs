//! `Analyzer::measure`: preview JPEG -> threshold-independent [`ImageMetrics`].
//!
//! Pipeline (one image, one thread):
//! 1. TurboJPEG decode of the 2048 px preview (orientation already applied).
//! 2. Luma at ~1024 px (2x2 box) -> exposure stats, DCT pHash, tiled global sharpness.
//! 3. RGB fitted to 640 px -> SCRFD faces (score >= 0.5, NMS 0.4).
//! 4. Per face (largest first, capped): upright 192 px crop along the eye line ->
//!    106 landmarks -> EAR per eye + mouth opening; eye-region and face-crop
//!    sharpness on the full-resolution luma, box-downsampled so the inter-ocular
//!    distance is about [`TARGET_IOD`] px (faces of different sizes compare equally).

use std::path::Path;

use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};

use super::imgproc::{self, Gray};
use super::models::{Detection, Models, DET_SIZE};
use super::{FaceMetrics, ImageMetrics, TileStats};
use crate::ipc::types::{NormPoint, NormRect};
use crate::raw::turbo;

/// Refuse absurd previews (guards memory on corrupt headers).
const MAX_PIXELS: u64 = 64_000_000;
/// Inter-ocular distance (px) the eye region is normalized to before measuring blur.
pub const TARGET_IOD: f32 = 40.0;
/// Faces smaller than this share of the image height get no landmarks (unjudgeable).
const MIN_LANDMARK_FACE: f32 = 0.015;
/// At most this many faces (largest first) get landmarks + sharpness.
const MAX_FACES: usize = 16;
/// Tile grid on the long edge for global sharpness.
const TILES_LONG: usize = 8;
/// Mean |gradient| (luma levels/px) below which a tile has too little texture to judge.
const TILE_MIN_TEXTURE: f32 = 1.5;

/// Per-thread reusable state for [`measure`].
pub struct Work {
    resizer: Resizer,
    rgb: Vec<u8>,
    small: Vec<u8>,
}

impl Default for Work {
    fn default() -> Self {
        Self { resizer: Resizer::new(), rgb: Vec::new(), small: Vec::new() }
    }
}

pub fn measure(models: &mut Models, work: &mut Work, path: &Path) -> Result<ImageMetrics, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read preview {}: {e}", path.display()))?;
    let (w, h) = turbo::decode_rgb_into(&bytes, u32::MAX, MAX_PIXELS, &mut work.rgb)?;
    let (w, h) = (w as usize, h as usize);
    if w < 64 || h < 64 {
        return Err(format!("preview too small ({w}x{h})"));
    }
    let rgb = &work.rgb[..w * h * 3];

    // Whole-frame stats on ~1024 px luma.
    let k = (w.max(h) as f32 / 1024.0).round().max(1.0) as usize;
    let luma = imgproc::luma_box(rgb, w, h, k);
    let exposure = imgproc::exposure(&luma);
    let phash = imgproc::phash(&luma);
    let tiles = tile_stats(&luma);

    // Faces.
    let (sw, sh) = fit(w, h, DET_SIZE);
    work.small.resize(sw * sh * 3, 0);
    {
        let src = ImageRef::new(w as u32, h as u32, rgb, PixelType::U8x3).map_err(|e| e.to_string())?;
        let mut dst = Image::from_slice_u8(sw as u32, sh as u32, &mut work.small[..sw * sh * 3], PixelType::U8x3)
            .map_err(|e| e.to_string())?;
        let opts = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Bilinear));
        work.resizer.resize(&src, &mut dst, &opts).map_err(|e| format!("resize: {e}"))?;
    }
    let to_src = w.max(h) as f32 / sw.max(sh) as f32;
    let mut dets = models.detect(&work.small[..sw * sh * 3], sw, sh, to_src)?;
    dets.sort_by(|a, b| face_h(b).total_cmp(&face_h(a)));

    let mut faces = Vec::with_capacity(dets.len());
    for (i, det) in dets.iter().enumerate() {
        let measured = i < MAX_FACES && face_h(det) >= MIN_LANDMARK_FACE * h as f32;
        faces.push(face_metrics(models, rgb, w, h, det, measured)?);
    }

    Ok(ImageMetrics { width: w as u32, height: h as u32, faces, global_sharpness: tiles.p90, exposure, phash, tiles })
}

fn face_h(d: &Detection) -> f32 {
    d.bbox[3] - d.bbox[1]
}

fn fit(w: usize, h: usize, edge: usize) -> (usize, usize) {
    let long = w.max(h);
    if long <= edge {
        return (w, h);
    }
    let s = edge as f32 / long as f32;
    (((w as f32 * s).round() as usize).clamp(1, edge), ((h as f32 * s).round() as usize).clamp(1, edge))
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Eye landmark indices (README): corners p1/p4, upper lid, lower lid (paired columns).
const EYE_A: ([usize; 2], [usize; 3], [usize; 3]) = ([35, 39], [41, 40, 42], [36, 33, 37]);
/// Eye B is eye A offset by 54.
const EYE_B_OFFSET: usize = 54;
/// Mouth: corners and inner-lip midpoints (verified on sample crops, see `mouth_open`).
pub const MOUTH_CORNERS: [usize; 2] = [52, 61];
pub const MOUTH_INNER: [usize; 2] = [62, 60];

/// 3-vertical Eye Aspect Ratio: mean lid distance / corner distance.
pub fn ear(pts: &[[f32; 2]; 106], offset: usize) -> Option<f32> {
    let (corners, upper, lower) = EYE_A;
    let width = dist(pts[corners[0] + offset], pts[corners[1] + offset]);
    if width < 1.0 {
        return None;
    }
    let v: f32 = upper.iter().zip(lower.iter()).map(|(&u, &l)| dist(pts[u + offset], pts[l + offset])).sum();
    Some(v / (3.0 * width))
}

/// Inner-lip gap / mouth width (0 closed, ~0.3+ laughing).
pub fn mouth_open(pts: &[[f32; 2]; 106]) -> Option<f32> {
    let width = dist(pts[MOUTH_CORNERS[0]], pts[MOUTH_CORNERS[1]]);
    if width < 1.0 {
        return None;
    }
    Some(dist(pts[MOUTH_INNER[0]], pts[MOUTH_INNER[1]]) / width)
}

fn norm_rect(b: &[f32; 4], w: usize, h: usize) -> NormRect {
    let (w, h) = (w as f32, h as f32);
    let x0 = b[0].clamp(0.0, w) / w;
    let y0 = b[1].clamp(0.0, h) / h;
    let x1 = b[2].clamp(0.0, w) / w;
    let y1 = b[3].clamp(0.0, h) / h;
    NormRect { x: x0, y: y0, width: (x1 - x0).max(0.0), height: (y1 - y0).max(0.0) }
}

fn norm_point(p: [f32; 2], w: usize, h: usize) -> NormPoint {
    NormPoint { x: (p[0] / w as f32).clamp(0.0, 1.0), y: (p[1] / h as f32).clamp(0.0, 1.0) }
}

/// Luma of a source region around `center` with half-extents `(hx, hy)`, box-downsampled by `k`.
fn region(rgb: &[u8], w: usize, h: usize, center: [f32; 2], hx: f32, hy: f32, k: usize) -> Gray {
    let x0 = (center[0] - hx).max(0.0) as usize;
    let y0 = (center[1] - hy).max(0.0) as usize;
    let x1 = ((center[0] + hx).max(0.0) as usize).min(w);
    let y1 = ((center[1] + hy).max(0.0) as usize).min(h);
    imgproc::downsample(&imgproc::luma_region(rgb, w, h, x0, y0, x1, y1), k)
}

/// Inter-ocular distance / face-box height at or above which a face counts as frontal
/// enough for EAR and eye-region sharpness (profiles and backs of heads collapse the
/// detector's eye keypoints; frontal faces are ~0.3-0.4).
pub const FRONTAL_MIN: f32 = 0.2;

/// Eye-region sharpness: the *worst* direction (motion blur leaves the perpendicular
/// direction crisp), capped by the amount of fine detail (soft eyes have almost none;
/// a contrast-invariant ratio alone rates a smooth blurred patch as "sharp").
pub fn eye_sharpness(s: &imgproc::DirSharpness) -> f32 {
    s.min.min(0.25 + s.texture / 20.0).clamp(0.0, 1.0)
}

fn face_metrics(
    models: &mut Models,
    rgb: &[u8],
    w: usize,
    h: usize,
    det: &Detection,
    measured: bool,
) -> Result<FaceMetrics, String> {
    let [l, r, nose, ..] = det.kps;
    let iod = dist(l, r).max(1.0);
    let mid = [(l[0] + r[0]) / 2.0, (l[1] + r[1]) / 2.0];
    let u = [(r[0] - l[0]) / iod, (r[1] - l[1]) / iod];
    let yaw = ((nose[0] - mid[0]) * u[0] + (nose[1] - mid[1]) * u[1]) / iod;
    let bw = (det.bbox[2] - det.bbox[0]).max(1.0);
    let bh = (det.bbox[3] - det.bbox[1]).max(1.0);

    let mut f = FaceMetrics {
        bbox: norm_rect(&det.bbox, w, h),
        left_eye: norm_point(l, w, h),
        right_eye: norm_point(r, w, h),
        detection_score: det.score,
        ear: None,
        sharpness: 0.0,
        ear_left: None,
        ear_right: None,
        mouth_open: None,
        yaw,
        iod_px: iod,
        face_sharpness: 0.0,
        eye_texture: 0.0,
        anisotropy: 0.0,
        frontal: false,
    };
    if !measured {
        return Ok(f);
    }

    let (pts, _) = models.landmarks(rgb, w, h, det)?;
    f.ear_left = ear(&pts, 0);
    f.ear_right = ear(&pts, EYE_B_OFFSET);
    f.ear = match (f.ear_left, f.ear_right) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    f.mouth_open = mouth_open(&pts);

    // Sharpness: eye region (both eyes + lids) and whole face, normalized scale.
    let k = (iod / TARGET_IOD).round().max(1.0) as usize;
    let vertical = u[1].abs() > std::f32::consts::FRAC_1_SQRT_2;
    let (ex, ey) = if vertical { (0.55 * iod, 1.0 * iod) } else { (1.0 * iod, 0.55 * iod) };
    let eyes = region(rgb, w, h, mid, ex, ey, k);
    let eye_s = imgproc::dir_sharpness(&eyes);
    let center = [(det.bbox[0] + det.bbox[2]) / 2.0, (det.bbox[1] + det.bbox[3]) / 2.0];
    let face = region(rgb, w, h, center, bw * 0.4, bh * 0.45, k);
    let face_s = imgproc::dir_sharpness(&face);
    f.eye_texture = eye_s.texture;
    f.face_sharpness = face_s.mean;
    f.anisotropy = face_s.anisotropy().max(eye_s.anisotropy());
    f.frontal = f.ear.is_some() && iod / bh >= FRONTAL_MIN;
    f.sharpness = if f.frontal { eye_sharpness(&eye_s) } else { face_s.mean };
    Ok(f)
}

/// Tiled whole-frame sharpness: textured tiles only; percentiles over them, so a
/// sharp subject in a shallow-DOF frame still scores high.
fn tile_stats(luma: &Gray) -> TileStats {
    let t = (luma.w.max(luma.h) / TILES_LONG).max(16);
    let (nx, ny) = (luma.w / t, luma.h / t);
    let mut vals: Vec<(f32, f32)> = Vec::new(); // (sharpness, anisotropy)
    let total = (nx * ny).max(1);
    for ty in 0..ny {
        for tx in 0..nx {
            let tile = imgproc::crop(luma, tx * t, ty * t, tx * t + t, ty * t + t);
            let (sh, sv, tex) = imgproc::hv_sharpness(&tile);
            if tex < TILE_MIN_TEXTURE {
                continue;
            }
            let mx = (sh + sv) / 2.0;
            let an = if mx > 0.0 { (sh - sv).abs() / mx } else { 0.0 };
            vals.push((mx, an));
        }
    }
    if vals.is_empty() {
        return TileStats { p90: 0.0, p50: 0.0, textured: 0.0, anisotropy: 0.0 };
    }
    vals.sort_by(|a, b| a.0.total_cmp(&b.0));
    let pct = |p: f32| vals[((vals.len() - 1) as f32 * p).round() as usize].0;
    // Anisotropy of the sharpest quarter (the subject, if anything is sharp).
    let top = &vals[vals.len() * 3 / 4..];
    let anisotropy = top.iter().map(|v| v.1).sum::<f32>() / top.len().max(1) as f32;
    TileStats { p90: pct(0.9), p50: pct(0.5), textured: vals.len() as f32 / total as f32, anisotropy }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts_with_eye(ear_target: f32) -> [[f32; 2]; 106] {
        let mut p = [[0.0f32; 2]; 106];
        for off in [0, EYE_B_OFFSET] {
            let x0 = off as f32;
            p[35 + off] = [x0, 0.0];
            p[39 + off] = [x0 + 30.0, 0.0];
            for (&u, &l) in EYE_A.1.iter().zip(EYE_A.2.iter()) {
                p[u + off] = [x0 + 15.0, -15.0 * ear_target];
                p[l + off] = [x0 + 15.0, 15.0 * ear_target];
            }
        }
        p
    }

    #[test]
    fn ear_math() {
        let p = pts_with_eye(0.25);
        assert!((ear(&p, 0).unwrap() - 0.25).abs() < 1e-5);
        assert!((ear(&p, EYE_B_OFFSET).unwrap() - 0.25).abs() < 1e-5);
        let closed = pts_with_eye(0.05);
        assert!(ear(&closed, 0).unwrap() < 0.06);
        let degenerate = [[0.0f32; 2]; 106];
        assert_eq!(ear(&degenerate, 0), None);
    }

    #[test]
    fn fit_keeps_aspect() {
        assert_eq!(fit(2048, 1365, 640), (640, 427));
        assert_eq!(fit(1365, 2048, 640), (427, 640));
        assert_eq!(fit(300, 200, 640), (300, 200));
    }
}
