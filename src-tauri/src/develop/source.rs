//! Develop source: half-size linear camera-RGB decode via LibRaw, and the per-render
//! "prepared" input (region crop + resample + orientation) the pipeline consumes.
//!
//! LibRaw settings (`native/libraw_shim.c`): `half_size = 1`, no white balance
//! (`user_mul = 1`), `output_color = 0` (raw camera colour), `gamm = {1,1}`,
//! `no_auto_bright = 1`, `output_bps = 16`, `user_flip = 0` (orientation is applied here
//! from the catalog's EXIF orientation), `highlight = 0`, `adjust_maximum_thr = 0` (the
//! white level never depends on image content). Output: black-subtracted, white = 65535.

use std::path::Path;

use rayon::prelude::*;

use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::NormRect;
use crate::raw::libraw;

use super::wb;

/// Colour metadata the pipeline needs.
#[derive(Debug, Clone, PartialEq)]
pub struct ColorInfo {
    /// As-shot camera multipliers (R, G, B), normalized to G = 1. `None` if unrecorded.
    pub as_shot_mul: Option<[f32; 3]>,
    /// Daylight (D65) multipliers from the colour matrix, G = 1 (fallback for as-shot).
    pub daylight_mul: [f32; 3],
    /// White-balanced camera RGB -> linear sRGB (D65), rows sum to 1 (LibRaw `rgb_cam`).
    pub rgb_cam: [[f32; 3]; 3],
    /// XYZ -> camera RGB (LibRaw `cam_xyz`, Adobe `ColorMatrix`), for temperature/tint.
    pub xyz_to_cam: [[f32; 3]; 3],
}

impl ColorInfo {
    /// Multipliers for as-shot white balance.
    pub fn as_shot(&self) -> [f32; 3] {
        self.as_shot_mul.unwrap_or(self.daylight_mul)
    }
}

/// Linear camera-RGB image plus the colour metadata the pipeline needs.
#[derive(Debug, Clone)]
pub struct LinearImage {
    pub width: u32,
    pub height: u32,
    /// Interleaved RGB, black-subtracted and scaled so the white level = 65535. Not rotated.
    pub pixels: Vec<u16>,
    pub color: ColorInfo,
    /// Full-size output dimensions before orientation.
    pub full_width: u32,
    pub full_height: u32,
}

impl LinearImage {
    pub fn bytes(&self) -> usize {
        self.pixels.len() * 2
    }
}

fn normalized_mul(m: [f32; 4]) -> Option<[f32; 3]> {
    let g = m[1];
    let ok = m[..3].iter().all(|v| v.is_finite() && *v > 0.0);
    ok.then(|| [m[0] / g, 1.0, m[2] / g])
}

const XYZ_FROM_SRGB: [[f64; 3]; 3] =
    [[0.4124564, 0.3575761, 0.1804375], [0.2126729, 0.7151522, 0.0721750], [0.0193339, 0.1191920, 0.9503041]];

fn mat_mul(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

/// Colour info from LibRaw's data; derives `cam_xyz` from `rgb_cam` + `pre_mul` when the
/// camera has no Adobe matrix (dcraw's `cam_xyz_coeff` inverted).
pub fn color_info(c: &libraw::ColorData) -> ColorInfo {
    let daylight_mul = normalized_mul(c.pre_mul).unwrap_or([1.0; 3]);
    let mut xyz_to_cam = c.cam_xyz;
    let empty = xyz_to_cam.iter().flatten().all(|v| v.abs() < 1e-9);
    if empty {
        let rgb_cam: [[f64; 3]; 3] = c.rgb_cam.map(|r| r.map(f64::from));
        if let (Some(cam_rgb_norm), Some(srgb_from_xyz)) = (wb::invert3(&rgb_cam), wb::invert3(&XYZ_FROM_SRGB)) {
            // cam_rgb = diag(1 / pre_mul) * cam_rgb_norm; cam_xyz = cam_rgb * (XYZ -> sRGB).
            let mut cam_rgb = cam_rgb_norm;
            for (i, row) in cam_rgb.iter_mut().enumerate() {
                let pm = f64::from(daylight_mul[i]).max(1e-6);
                row.iter_mut().for_each(|v| *v /= pm);
            }
            xyz_to_cam = mat_mul(&cam_rgb, &srgb_from_xyz).map(|r| r.map(|v| v as f32));
        }
    }
    ColorInfo { as_shot_mul: normalized_mul(c.cam_mul), daylight_mul, rgb_cam: c.rgb_cam, xyz_to_cam }
}

/// Blocking half-size decode (~0.3-0.8 s for 24-33 MP on Apple Silicon).
pub fn decode_half_size(path: &Path) -> AppResult<LinearImage> {
    let d = libraw::decode_linear(path, true).map_err(|e| AppError::internal(format!("{}: {e}", path.display())))?;
    let color = color_info(&d.color);
    let (full_width, full_height) = if d.color.width > 0 && d.color.height > 0 {
        (d.color.width, d.color.height)
    } else {
        (d.width * 2, d.height * 2)
    };
    Ok(LinearImage { width: d.width, height: d.height, pixels: d.pixels, color, full_width, full_height })
}

/// Camera RGB cropped/resampled to the render size with orientation applied.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub width: u32,
    pub height: u32,
    /// Interleaved camera RGB (unscaled, not white balanced), oriented.
    pub pixels: Vec<u16>,
    /// Long edge of the whole (uncropped) frame in this image's pixel units.
    pub frame_long_edge: f32,
}

impl Prepared {
    pub fn bytes(&self) -> usize {
        self.pixels.len() * 2
    }
}

/// Oriented size of a `w x h` image.
pub fn oriented_size(w: u32, h: u32, orientation: u8) -> (u32, u32) {
    if orientation >= 5 {
        (h, w)
    } else {
        (w, h)
    }
}

/// Maps a rect in oriented normalized coordinates to the stored (unrotated) image.
pub fn unorient_rect(r: NormRect, orientation: u8) -> NormRect {
    let map = |u: f32, v: f32| -> (f32, f32) {
        match orientation {
            2 => (1.0 - u, v),
            3 => (1.0 - u, 1.0 - v),
            4 => (u, 1.0 - v),
            5 => (v, u),
            6 => (v, 1.0 - u),
            7 => (1.0 - v, 1.0 - u),
            8 => (1.0 - v, u),
            _ => (u, v),
        }
    };
    let (ax, ay) = map(r.x, r.y);
    let (bx, by) = map(r.x + r.width, r.y + r.height);
    NormRect { x: ax.min(bx), y: ay.min(by), width: (ax - bx).abs(), height: (ay - by).abs() }
}

/// Output size (oriented) for a render of `region` at most `max_edge` long, never
/// upscaling beyond the source pixels the region covers.
pub fn output_size(src_w: u32, src_h: u32, orientation: u8, region: Option<NormRect>, max_edge: u32) -> (u32, u32) {
    let (ow, oh) = oriented_size(src_w, src_h, orientation);
    let r = region.unwrap_or(NormRect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 });
    let (rw, rh) = (r.width.clamp(0.0, 1.0) * ow as f32, r.height.clamp(0.0, 1.0) * oh as f32);
    let long = rw.max(rh).max(1.0);
    let scale = (max_edge as f32 / long).min(1.0);
    (((rw * scale).round() as u32).max(1), ((rh * scale).round() as u32).max(1))
}

/// Crops `region` (oriented coordinates), resamples to fit `max_edge`, applies orientation.
pub fn prepare(src: &LinearImage, orientation: u8, region: Option<NormRect>, max_edge: u32) -> Prepared {
    let orientation = if (1..=8).contains(&orientation) { orientation } else { 1 };
    let full = NormRect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 };
    let r_or = region.unwrap_or(full);
    let (out_w, out_h) = output_size(src.width, src.height, orientation, region, max_edge);
    let r = unorient_rect(r_or, orientation);
    let (uw, uh) = if orientation >= 5 { (out_h, out_w) } else { (out_w, out_h) };
    let crop = [
        (r.x * src.width as f32) as f64,
        (r.y * src.height as f32) as f64,
        ((r.width * src.width as f32) as f64).max(1.0),
        ((r.height * src.height as f32) as f64).max(1.0),
    ];
    let resized = resample(&src.pixels, src.width as usize, src.height as usize, crop, uw as usize, uh as usize);
    let pixels = if orientation == 1 { resized } else { orient(&resized, uw as usize, uh as usize, orientation) };
    // Frame long edge in output pixels: output px per source px times the source long edge.
    let px_scale = uw as f32 / (r.width * src.width as f32).max(1.0);
    let frame_long_edge = src.width.max(src.height) as f32 * px_scale;
    Prepared { width: out_w, height: out_h, pixels, frame_long_edge }
}

/// Separable filter taps for one axis: output i covers source `[start, start + weights.len())`.
struct Taps {
    start: Vec<usize>,
    len: Vec<usize>,
    weights: Vec<f32>,
    stride: usize,
}

/// Triangle (tent) filter sized to the scale factor (area-like when shrinking, bilinear at
/// 1:1). `offset`/`extent` select the source span in pixels.
fn taps(src_len: usize, offset: f64, extent: f64, out_len: usize) -> Taps {
    let scale = extent / out_len as f64;
    let support = scale.max(1.0);
    let stride = (2.0 * support).ceil() as usize + 2;
    let mut start = Vec::with_capacity(out_len);
    let mut len = Vec::with_capacity(out_len);
    let mut weights = vec![0.0f32; out_len * stride];
    for i in 0..out_len {
        let center = offset + (i as f64 + 0.5) * scale - 0.5;
        let lo = ((center - support).floor() as i64 + 1).max(0) as usize;
        let hi = (((center + support).ceil() as i64).min(src_len as i64 - 1)).max(0) as usize;
        let lo = lo.min(hi);
        let n = (hi - lo + 1).min(stride);
        let w = &mut weights[i * stride..i * stride + n];
        let mut sum = 0.0f64;
        for (k, wk) in w.iter_mut().enumerate() {
            let d = ((lo + k) as f64 - center).abs() / support;
            let v = (1.0 - d).max(0.0);
            *wk = v as f32;
            sum += v;
        }
        if sum <= 0.0 {
            // Degenerate (centre outside the image): nearest pixel.
            w.iter_mut().for_each(|v| *v = 0.0);
            w[0] = 1.0;
        } else {
            w.iter_mut().for_each(|v| *v = (f64::from(*v) / sum) as f32);
        }
        start.push(lo);
        len.push(n);
    }
    Taps { start, len, weights, stride }
}

/// Resamples the `crop` (x, y, w, h in source pixels) of an interleaved RGB16 image to
/// `dw x dh`. Rows in parallel.
pub fn resample(src: &[u16], sw: usize, sh: usize, crop: [f64; 4], dw: usize, dh: usize) -> Vec<u16> {
    let tx = taps(sw, crop[0], crop[2], dw);
    let ty = taps(sh, crop[1], crop[3], dh);
    let x_lo = tx.start.iter().copied().min().unwrap_or(0);
    let x_hi = tx.start.iter().zip(&tx.len).map(|(s, l)| s + l).max().unwrap_or(0).min(sw);
    let mut out = vec![0u16; dw * dh * 3];
    out.par_chunks_mut(dw * 3).enumerate().for_each_init(
        || vec![0.0f32; (x_hi - x_lo) * 3],
        |row_buf, (y, out_row)| {
            row_buf.iter_mut().for_each(|v| *v = 0.0);
            let (ys, yn) = (ty.start[y], ty.len[y]);
            for k in 0..yn {
                let w = ty.weights[y * ty.stride + k];
                if w == 0.0 {
                    continue;
                }
                let row = &src[((ys + k) * sw + x_lo) * 3..((ys + k) * sw + x_hi) * 3];
                for (acc, &v) in row_buf.iter_mut().zip(row) {
                    *acc += w * f32::from(v);
                }
            }
            for x in 0..dw {
                let (xs, xn) = (tx.start[x] - x_lo, tx.len[x]);
                let ws = &tx.weights[x * tx.stride..x * tx.stride + xn];
                let mut acc = [0.0f32; 3];
                for (k, &w) in ws.iter().enumerate() {
                    let p = &row_buf[(xs + k) * 3..(xs + k) * 3 + 3];
                    acc[0] += w * p[0];
                    acc[1] += w * p[1];
                    acc[2] += w * p[2];
                }
                for c in 0..3 {
                    out_row[x * 3 + c] = acc[c].round().clamp(0.0, 65535.0) as u16;
                }
            }
        },
    );
    out
}

/// Applies EXIF orientation (2..=8) to interleaved RGB16.
pub fn orient(src: &[u16], w: usize, h: usize, orientation: u8) -> Vec<u16> {
    let (dw, dh) = if orientation >= 5 { (h, w) } else { (w, h) };
    let mut out = vec![0u16; dw * dh * 3];
    out.par_chunks_mut(dw * 3).enumerate().for_each(|(y, row)| {
        for x in 0..dw {
            let (sx, sy) = match orientation {
                2 => (w - 1 - x, y),
                3 => (w - 1 - x, h - 1 - y),
                4 => (x, h - 1 - y),
                5 => (y, x),
                6 => (y, h - 1 - x),
                7 => (w - 1 - y, h - 1 - x),
                8 => (w - 1 - y, x),
                _ => (x, y),
            };
            let s = (sy * w + sx) * 3;
            row[x * 3..x * 3 + 3].copy_from_slice(&src[s..s + 3]);
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: u32, h: u32, f: impl Fn(u32, u32) -> [u16; 3]) -> LinearImage {
        let mut pixels = Vec::new();
        for y in 0..h {
            for x in 0..w {
                pixels.extend_from_slice(&f(x, y));
            }
        }
        LinearImage {
            width: w,
            height: h,
            pixels,
            color: ColorInfo {
                as_shot_mul: None,
                daylight_mul: [1.0; 3],
                rgb_cam: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                xyz_to_cam: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            },
            full_width: w * 2,
            full_height: h * 2,
        }
    }

    #[test]
    fn identity_resample_and_flat_downscale() {
        let img = image(7, 5, |x, y| [x as u16 * 100, y as u16 * 100, 7]);
        let p = prepare(&img, 1, None, 64);
        assert_eq!((p.width, p.height), (7, 5), "never upscaled");
        assert_eq!(p.pixels, img.pixels, "1:1 is exact");
        assert_eq!(p.frame_long_edge, 7.0);

        let flat = image(400, 300, |_, _| [1000, 2000, 3000]);
        let p = prepare(&flat, 1, None, 100);
        assert_eq!((p.width, p.height), (100, 75));
        assert!(p.pixels.chunks(3).all(|c| c == [1000, 2000, 3000]));
    }

    #[test]
    fn orientation_and_region_mapping() {
        // 4x2 image, pixel value encodes (x, y).
        let img = image(4, 2, |x, y| [x as u16, y as u16, 0]);
        // Orientation 6 (rotate 90 CW for display): output 2x4, top-left shows source (0, 1).
        let p = prepare(&img, 6, None, 64);
        assert_eq!((p.width, p.height), (2, 4));
        assert_eq!(&p.pixels[0..2], &[0, 1]);
        // Orientation 8 (rotate 90 CCW): top-left shows source (3, 0).
        let p = prepare(&img, 8, None, 64);
        assert_eq!(&p.pixels[0..2], &[3, 0]);
        // Region in oriented coordinates: the bottom half of an orientation-8 frame is
        // the left half of the stored image.
        let r = NormRect { x: 0.0, y: 0.5, width: 1.0, height: 0.5 };
        assert_eq!(unorient_rect(r, 8), NormRect { x: 0.0, y: 0.0, width: 0.5, height: 1.0 });
        let p = prepare(&img, 8, Some(r), 64);
        assert_eq!((p.width, p.height), (2, 2));
        // Oriented (0,0) of the crop = stored (1, 0).
        assert_eq!(&p.pixels[0..2], &[1, 0]);
        assert_eq!(p.frame_long_edge, 4.0);
        for o in 1..=8u8 {
            let full = NormRect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 };
            assert_eq!(unorient_rect(full, o), full);
        }
    }

    #[test]
    fn output_size_respects_region_and_max_edge() {
        assert_eq!(output_size(3000, 2000, 1, None, 2048), (2048, 1365));
        assert_eq!(output_size(3000, 2000, 8, None, 2048), (1365, 2048));
        let r = NormRect { x: 0.25, y: 0.25, width: 0.25, height: 0.25 };
        assert_eq!(output_size(3000, 2000, 1, Some(r), 2048), (750, 500));
        assert_eq!(output_size(3000, 2000, 1, Some(r), 300), (300, 200));
    }
}
