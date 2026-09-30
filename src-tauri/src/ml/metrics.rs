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
use super::{FaceMetrics, HighlightStats, ImageMetrics, TileStats};
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
/// Cell size (preview px) of the blown-highlight map.
const BLOWN_CELL: usize = 32;
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
    let blown = imgproc::BlownMap::new(rgb, w, h, BLOWN_CELL, 2);
    let highlights = HighlightStats {
        blown: blown.total(),
        center: blown.share_in(0.25, 0.25, 0.75, 0.75),
        region: blown.largest_region(),
    };

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

    Ok(ImageMetrics {
        width: w as u32,
        height: h as u32,
        faces,
        global_sharpness: tiles.p90,
        exposure,
        phash,
        tiles,
        highlights,
    })
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

/// Side of the square eye crop fed to the eye-state CNN, in eye widths (corner to
/// corner). MRL-style crops include lids and some brow/cheek.
pub const EYE_BOX: f32 = 2.2;
/// Tight per-eye crops count for sharpness only with at least this much detail (mean
/// |gradient|, luma levels/px); flat or tiny crops give meaningless ratios.
pub const TIGHT_EYE_MIN_TEXTURE: f32 = 5.0;

/// Head pose / FaceMesh EAR are measured only for faces whose more-open eye has an EAR
/// below this (possible blinks). Any `blinkEar` above it cannot produce a blink.
pub const POSE_EAR_GATE: f32 = 0.3;

/// FaceMesh eye contour points for the 6-point EAR: corners p1/p4, upper p2/p3,
/// lower p6/p5 (MediaPipe topology; image-left and image-right eye).
const MESH_EYE_A: [usize; 6] = [33, 160, 158, 133, 153, 144];
const MESH_EYE_B: [usize; 6] = [362, 385, 387, 263, 373, 380];

/// 6-point EAR on FaceMesh landmarks: (|p2-p6| + |p3-p5|) / (2 |p1-p4|).
pub fn mesh_ear(mesh: &[[f32; 3]], [p1, p2, p3, p4, p5, p6]: [usize; 6]) -> Option<f32> {
    let d = |a: usize, b: usize| dist([mesh[a][0], mesh[a][1]], [mesh[b][0], mesh[b][1]]);
    let width = d(p1, p4);
    (width >= 1.0).then(|| (d(p2, p6) + d(p3, p5)) / (2.0 * width))
}

/// The detector box reaches (or crosses) the frame edge: the face is cut off by the
/// frame (detail shots, jewellery close-ups), so it is not the focus subject.
pub fn truncated(b: &[f32; 4], w: usize, h: usize) -> bool {
    let tol = 0.01 * (b[3] - b[1]).max(1.0);
    b[0] <= tol || b[1] <= tol || b[2] >= w as f32 - tol || b[3] >= h as f32 - tol
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
        truncated: truncated(&det.bbox, w, h),
        eye_open_prob: None,
        head_pitch: None,
        head_yaw: None,
        mesh_ear: None,
        face_luma: 0.0,
        mouth_width: None,
        blown: blown_share(rgb, w, h, &det.bbox),
    };
    if !measured {
        return Ok(f);
    }

    let (pts, crop) = models.landmarks(rgb, w, h, det)?;
    f.ear_left = ear(&pts, 0);
    f.ear_right = ear(&pts, EYE_B_OFFSET);
    f.ear = match (f.ear_left, f.ear_right) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    f.mouth_open = mouth_open(&pts);
    let eye_span = dist(pts[EYE_A.0[0]], pts[EYE_A.0[1] + EYE_B_OFFSET]).max(1.0);
    f.mouth_width = Some(dist(pts[MOUTH_CORNERS[0]], pts[MOUTH_CORNERS[1]]) / eye_span);
    let k = (iod / TARGET_IOD).round().max(1.0) as usize;

    // FaceMesh V2 (3D head pose + an independent EAR), only for possible blinks: frontal
    // faces whose more-open eye is below POSE_EAR_GATE. Keeps the per-image cost low.
    let ear_open =
        f.ear_left.into_iter().chain(f.ear_right).fold(None, |m: Option<f32>, e| Some(m.map_or(e, |m| m.max(e))));
    if iod / bh >= FRONTAL_MIN && ear_open.is_some_and(|e| e < POSE_EAR_GATE) {
        let (mesh, _) = models.face_mesh(rgb, w, h, det)?;
        let pose = super::pose::head_pose(&mesh);
        f.head_pitch = Some(pose.pitch);
        f.head_yaw = Some(pose.yaw);
        f.mesh_ear = match (mesh_ear(&mesh, MESH_EYE_A), mesh_ear(&mesh, MESH_EYE_B)) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
    }

    // Per eye: CNN eye state and tight-crop sharpness.
    let (mut open_max, mut tight_best) = (0.0f32, 0.0f32);
    for off in [0, EYE_B_OFFSET] {
        let (a, b) = (pts[EYE_A.0[0] + off], pts[EYE_A.0[1] + off]);
        let ew = dist(a, b).max(1.0);
        let c = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        open_max = open_max.max(models.eye_open(rgb, w, h, &crop, c, ew * EYE_BOX)?);
        // Tight box around the eye contour, in source pixels.
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for i in 33..43 {
            let (x, y) = crop.apply(pts[i + off][0], pts[i + off][1]);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        let side = (x1 - x0).max(y1 - y0).max(4.0);
        let s =
            imgproc::dir_sharpness(&region(rgb, w, h, [(x0 + x1) / 2.0, (y0 + y1) / 2.0], side * 0.75, side * 0.75, k));
        if s.texture >= TIGHT_EYE_MIN_TEXTURE {
            tight_best = tight_best.max(s.min);
        }
    }
    f.eye_open_prob = Some(open_max);

    // Sharpness: eye region (both eyes + lids) and whole face, normalized scale.
    let vertical = u[1].abs() > std::f32::consts::FRAC_1_SQRT_2;
    let (ex, ey) = if vertical { (0.55 * iod, 1.0 * iod) } else { (1.0 * iod, 0.55 * iod) };
    let eyes = region(rgb, w, h, mid, ex, ey, k);
    let eye_s = imgproc::dir_sharpness(&eyes);
    let center = [(det.bbox[0] + det.bbox[2]) / 2.0, (det.bbox[1] + det.bbox[3]) / 2.0];
    let face = region(rgb, w, h, center, bw * 0.4, bh * 0.45, k);
    let face_s = imgproc::dir_sharpness(&face);
    f.eye_texture = eye_s.texture;
    f.face_sharpness = face_s.mean;
    f.face_luma = face.px.iter().sum::<f32>() / face.px.len().max(1) as f32 / 255.0;
    f.anisotropy = face_s.anisotropy().max(eye_s.anisotropy());
    f.frontal = f.ear.is_some() && iod / bh >= FRONTAL_MIN;
    f.sharpness = if f.frontal { eye_sharpness(&eye_s).max(tight_best) } else { face_s.mean };
    Ok(f)
}

/// Share of blown pixels (every channel >= `imgproc::BLOWN_MIN`) in the inner part of
/// a detector box (cheeks, nose, forehead; the box corners are often bright sky behind
/// the head, which is not a defect of the face).
fn blown_share(rgb: &[u8], w: usize, h: usize, b: &[f32; 4]) -> f32 {
    let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
    let x0 = (b[0] + 0.25 * bw).clamp(0.0, w as f32) as usize;
    let y0 = (b[1] + 0.2 * bh).clamp(0.0, h as f32) as usize;
    let x1 = ((b[0] + 0.75 * bw).clamp(0.0, w as f32) as usize).max(x0);
    let y1 = ((b[1] + 0.85 * bh).clamp(0.0, h as f32) as usize).max(y0);
    let (mut n, mut hit) = (0u32, 0u32);
    for y in (y0..y1).step_by(2) {
        for x in (x0..x1).step_by(2) {
            let p = &rgb[(y * w + x) * 3..(y * w + x) * 3 + 3];
            n += 1;
            if p.iter().all(|&c| c >= imgproc::BLOWN_MIN) {
                hit += 1;
            }
        }
    }
    if n == 0 {
        0.0
    } else {
        hit as f32 / n as f32
    }
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
