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
use crate::ipc::types::{CropSettings, NormRect};

use super::transform::Geometry;
use crate::raw::libraw;
use crate::raw::raster::{self, SourceColorSpace};

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
    /// Per-unit camera calibration (DNG `CameraCalibration`, diagonal R, G, B): this
    /// camera's RGB = diag(calibration) * the reference unit's the colour matrices describe.
    /// `[1; 3]` when unknown. See [`camera_calibration`].
    pub calibration: [f32; 3],
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
    /// Display-referred source (non-RAW, Phase 7b): pixels are the file's own linear light,
    /// so the pipeline must skip `BASELINE_EV` and the base (filmic) curve; neutral
    /// adjustments then reproduce the file. `false` for RAW decodes.
    pub display_referred: bool,
    /// Colour encoding of a raster source (`None` for RAW). `Unknown` =>
    /// `DevelopWarningCode::SourceColorAssumed` (detail: `SourceColorSpace::assumed_detail`).
    pub source_color: Option<SourceColorSpace>,
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
    ColorInfo {
        as_shot_mul: normalized_mul(c.cam_mul),
        daylight_mul,
        rgb_cam: c.rgb_cam,
        xyz_to_cam,
        calibration: camera_calibration(c),
    }
}

/// Per-model reference white-balance preset of Adobe's camera calibration: Camera Raw /
/// DNG Converter compensate unit-to-unit sensor variation with
/// `CameraCalibration = diag(K / preset)` (G = 1), where `preset` is the camera's own
/// white-balance preset for a fixed light (normalized to G) and `K` a per-model constant.
/// Measured from Adobe DNG Converter 17.5 conversions (`tools/acr-oracle/cc_probe.py`;
/// two EOS M6 Mark II frames with different daylight presets confirm the form exactly).
/// (make, model, preset light source (EXIF code), K red, K blue).
const CALIBRATION_REFERENCE: &[(&str, &str, u8, f32, f32)] = &[
    ("Sony", "ILCE-7M4", 1, 2.354_37, 1.599_60),
    ("Canon", "EOS M6 Mark II", 1, 1.756_93, 1.599_58),
    ("Fujifilm", "X-M5", 21, 2.036_36, 1.652_27),
];

/// Adobe's per-unit camera calibration for a raw file (see [`CALIBRATION_REFERENCE`]);
/// `[1; 3]` for other cameras or when the preset is missing.
pub fn camera_calibration(c: &libraw::ColorData) -> [f32; 3] {
    let Some(&(_, _, light, kr, kb)) = CALIBRATION_REFERENCE
        .iter()
        .find(|(make, model, ..)| c.make.eq_ignore_ascii_case(make) && c.model.eq_ignore_ascii_case(model))
    else {
        return [1.0; 3];
    };
    let p = match light {
        1 => c.wb_daylight,
        21 => c.wb_d65,
        _ => return [1.0; 3],
    };
    if p[0] <= 0 || p[1] <= 0 || p[2] <= 0 {
        return [1.0; 3];
    }
    let g = p[1] as f32;
    let cc = [kr * g / p[0] as f32, 1.0, kb * g / p[2] as f32];
    // Guard against misparsed presets: real calibrations are within a few percent.
    if cc.iter().all(|v| (0.8..1.25).contains(v)) {
        cc
    } else {
        [1.0; 3]
    }
}

/// What the colour pipeline needs to know about a decoded source besides its pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceMeta {
    pub format: crate::ipc::types::ImageFormat,
    /// Camera make / model (LibRaw's normalized names for RAWs).
    pub make: Option<String>,
    pub model: Option<String>,
    /// Already rendered (JPEG/HEIC/TIFF/PNG): no baseline exposure, no base tone curve, no
    /// camera profile.
    pub display_referred: bool,
    /// Per-file raw baseline exposure when the file tells it (Fujifilm `RawExposureBias`,
    /// DNG `BaselineExposure`); `None` = the camera default.
    pub baseline_exposure: Option<f32>,
    /// Fujifilm X-Trans colour filter array (LibRaw `filters == 9`).
    pub xtrans: bool,
}

impl SourceMeta {
    /// Metadata of a non-RAW (display-referred) source.
    pub fn display_referred(format: crate::ipc::types::ImageFormat) -> Self {
        SourceMeta { format, make: None, model: None, display_referred: true, baseline_exposure: None, xtrans: false }
    }

    fn raw(path: &Path, c: &libraw::ColorData) -> Self {
        let format = crate::raw::format_from_extension(path).unwrap_or(crate::ipc::types::ImageFormat::Arw);
        let name = |s: &str| (!s.is_empty()).then(|| s.to_owned());
        // Camera Raw: Fujifilm baseline = -RawExposureBias - 0.65 (DNG Converter 17.5: DR100
        // bias -0.7 -> 0.07, DR200 -1.7 -> 1.07).
        // LibRaw reports -999 when a file has no DNG BaselineExposure.
        let baseline_exposure = if c.dng_baseline_exposure.abs() < 10.0 && c.dng_baseline_exposure != 0.0 {
            Some(c.dng_baseline_exposure)
        } else if format == crate::ipc::types::ImageFormat::Raf && c.fuji_expo_shift != 0.0 {
            Some(-c.fuji_expo_shift - 0.65)
        } else {
            None
        };
        SourceMeta {
            format,
            make: name(&c.make),
            model: name(&c.model),
            display_referred: false,
            baseline_exposure,
            xtrans: c.filters == 9,
        }
    }
}

/// Long edge of the editor's cached decode of a non-RAW source (the RAW equivalent is
/// LibRaw's half-size decode).
pub const RASTER_EDITOR_EDGE: u32 = 4096;

/// Non-RAW sources (by extension) are decoded by `raw::raster`; `None` = a RAW path.
fn decode_raster(path: &Path, max_edge: Option<u32>) -> Option<AppResult<(LinearImage, SourceMeta)>> {
    let format = crate::raw::format_from_extension(path).filter(|f| !f.is_raw())?;
    Some(
        raster::decode_linear(path, format, max_edge)
            .map(|img| (raster::to_linear_image(img), SourceMeta::display_referred(format)))
            .map_err(|e| AppError::internal(format!("{}: {e}", path.display()))),
    )
}

/// Blocking half-size decode (~0.3-0.8 s for 24-33 MP on Apple Silicon). Non-RAW sources:
/// linear decode downscaled to [`RASTER_EDITOR_EDGE`].
pub fn decode_half_size(path: &Path) -> AppResult<LinearImage> {
    decode_half_size_meta(path).map(|(img, _)| img)
}

/// [`decode_half_size`] plus the source's camera metadata.
pub fn decode_half_size_meta(path: &Path) -> AppResult<(LinearImage, SourceMeta)> {
    if let Some(r) = decode_raster(path, Some(RASTER_EDITOR_EDGE)) {
        return r;
    }
    let d = libraw::decode_linear(path, true).map_err(|e| AppError::internal(format!("{}: {e}", path.display())))?;
    let color = color_info(&d.color);
    let meta = SourceMeta::raw(path, &d.color);
    let (full_width, full_height) = if d.color.width > 0 && d.color.height > 0 {
        (d.color.width, d.color.height)
    } else {
        (d.width * 2, d.height * 2)
    };
    Ok((
        LinearImage {
            width: d.width,
            height: d.height,
            pixels: d.pixels,
            color,
            full_width,
            full_height,
            display_referred: false,
            source_color: None,
        },
        meta,
    ))
}

/// Blocking full-resolution decode for export: the same LibRaw settings as
/// [`decode_half_size`] except `half_size = 0` and `user_qual = 3` (AHD for Bayer, 3-pass
/// Markesteijn for X-Trans). LibRaw's buffers are released before returning;
/// `full_width/full_height` equal `width/height`. Non-RAW sources: full-size linear decode.
pub fn decode_full(path: &Path) -> AppResult<LinearImage> {
    decode_full_meta(path).map(|(img, _)| img)
}

/// [`decode_full`] plus the source's camera metadata.
pub fn decode_full_meta(path: &Path) -> AppResult<(LinearImage, SourceMeta)> {
    if let Some(r) = decode_raster(path, None) {
        return r;
    }
    let d = libraw::decode_linear(path, false).map_err(|e| AppError::internal(format!("{}: {e}", path.display())))?;
    let color = color_info(&d.color);
    let meta = SourceMeta::raw(path, &d.color);
    Ok((
        LinearImage {
            width: d.width,
            height: d.height,
            pixels: d.pixels,
            color,
            full_width: d.width,
            full_height: d.height,
            display_referred: false,
            source_color: None,
        },
        meta,
    ))
}
/// Camera RGB cropped/resampled to the render size with orientation applied.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub width: u32,
    pub height: u32,
    /// Interleaved camera RGB (unscaled, not white balanced), oriented.
    pub pixels: Vec<u16>,
    /// Long edge of the whole (cropped) frame in this image's pixel units.
    pub frame_long_edge: f32,
    /// Frame placement and scale (px per full-resolution frame px).
    pub view: super::pipeline::View,
    /// Per pixel, how much of it shows the image (255 = all, 0 = nothing): `Some` only when
    /// the frame leaves the (warped / rotated) image somewhere. The rest is filled with
    /// [`FILL`] after the pipeline ([`fill_outside_rgb8`] / [`fill_outside_rgb16`]), as
    /// Lightroom does, instead of repeating edge pixels.
    pub coverage: Option<Vec<u8>>,
}

impl Prepared {
    pub fn bytes(&self) -> usize {
        self.pixels.len() * 2 + self.coverage.as_ref().map_or(0, Vec::len)
    }
}

/// Fill of the area outside the image (Lightroom Classic shows and exports it white).
pub const FILL: f32 = 1.0;

/// Blends uncovered pixels of an RGB8 render towards [`FILL`].
pub fn fill_outside_rgb8(rgb: &mut [u8], coverage: &[u8]) {
    let fill = (FILL * 255.0).round();
    rgb.par_chunks_mut(3).zip(coverage.par_iter()).for_each(|(p, &c)| {
        if c < 255 {
            let k = f32::from(c) / 255.0;
            for v in p.iter_mut() {
                *v = (f32::from(*v) * k + fill * (1.0 - k)).round().clamp(0.0, 255.0) as u8;
            }
        }
    });
}

/// Blends uncovered pixels of an RGB16 render (any output encoding) towards [`FILL`].
pub fn fill_outside_rgb16(rgb: &mut [u16], coverage: &[u8]) {
    let fill = (FILL * 65535.0).round();
    rgb.par_chunks_mut(3).zip(coverage.par_iter()).for_each(|(p, &c)| {
        if c < 255 {
            let k = f32::from(c) / 255.0;
            for v in p.iter_mut() {
                *v = (f32::from(*v) * k + fill * (1.0 - k)).round().clamp(0.0, 65535.0) as u16;
            }
        }
    });
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
/// upscaling beyond the source pixels the region covers (no crop).
pub fn output_size(src_w: u32, src_h: u32, orientation: u8, region: Option<NormRect>, max_edge: u32) -> (u32, u32) {
    let (ow, oh) = oriented_size(src_w, src_h, orientation);
    fit_region((ow, oh), region, max_edge)
}

/// Size of `region` of a `frame` (oriented px) fitted into `max_edge`, never upscaled.
pub fn fit_region(frame: (u32, u32), region: Option<NormRect>, max_edge: u32) -> (u32, u32) {
    let r = region.unwrap_or(FULL);
    let (rw, rh) = (r.width.clamp(0.0, 1.0) * frame.0 as f32, r.height.clamp(0.0, 1.0) * frame.1 as f32);
    let long = rw.max(rh).max(1.0);
    let scale = (max_edge as f32 / long).min(1.0);
    (((rw * scale).round() as u32).max(1), ((rh * scale).round() as u32).max(1))
}

const FULL: NormRect = NormRect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 };

/// Oriented size of the cropped frame of `src` (source pixels). With a Transform, pass the
/// effective crop of the render [`Geometry`] (the corrected frame has the source's size).
pub fn frame_size(src_w: u32, src_h: u32, orientation: u8, crop: &CropSettings) -> (u32, u32) {
    let g = super::parity::crop_geometry(crop, src_w, src_h, orientation);
    (g.width, g.height)
}

/// Crops `region` (oriented coordinates of the cropped frame), resamples to fit `max_edge`,
/// applies crop/straighten and orientation.
pub fn prepare(
    src: &LinearImage,
    orientation: u8,
    crop: &CropSettings,
    region: Option<NormRect>,
    max_edge: u32,
) -> Prepared {
    prepare_geo(src, orientation, &Geometry::from(crop), region, max_edge)
}

/// [`prepare`] for a render [`Geometry`] (effective crop + Transform warp).
pub fn prepare_geo(
    src: &LinearImage,
    orientation: u8,
    geo: &Geometry,
    region: Option<NormRect>,
    max_edge: u32,
) -> Prepared {
    let orientation = if (1..=8).contains(&orientation) { orientation } else { 1 };
    let frame = frame_size(src.width, src.height, orientation, &geo.crop);
    let (out_w, out_h) = fit_region(frame, region, max_edge);
    prepare_sized_geo(src, orientation, geo, region, out_w, out_h)
}

/// As [`prepare`] with an explicit output size (exports).
pub fn prepare_sized(
    src: &LinearImage,
    orientation: u8,
    crop: &CropSettings,
    region: Option<NormRect>,
    out_w: u32,
    out_h: u32,
) -> Prepared {
    prepare_sized_geo(src, orientation, &Geometry::from(crop), region, out_w, out_h)
}

/// [`prepare_sized`] for a render [`Geometry`] (effective crop + Transform warp).
pub fn prepare_sized_geo(
    src: &LinearImage,
    orientation: u8,
    geo: &Geometry,
    region: Option<NormRect>,
    out_w: u32,
    out_h: u32,
) -> Prepared {
    let crop = &geo.crop;
    let orientation = if (1..=8).contains(&orientation) { orientation } else { 1 };
    let g = super::parity::frame_geometry(geo, src.width, src.height, orientation);
    let r_or = region.unwrap_or(FULL);
    let (sw, sh) = (src.width as usize, src.height as usize);
    let rotated = crop.enabled && crop.angle.abs() > 1e-4;
    // Output (u, v) -> crop frame -> corrected frame (the source without a warp).
    let a = g.to_source;
    let rr = [r_or.x as f64, r_or.y as f64, r_or.width as f64, r_or.height as f64];
    let affine = [
        a[0] * rr[2],
        a[1] * rr[3],
        a[0] * rr[0] + a[1] * rr[1] + a[2],
        a[3] * rr[2],
        a[4] * rr[3],
        a[3] * rr[0] + a[4] * rr[1] + a[5],
    ];
    let affine3 = [affine[0], affine[1], affine[2], affine[3], affine[4], affine[5], 0.0, 0.0, 1.0];
    let mut coverage = None;
    let pixels = if let Some(warp) = geo.warp {
        // Transform: then corrected frame -> source (projective).
        let m = super::transform::mul(&warp, &affine3);
        coverage = coverage_map(&m, sw, sh, out_w as usize, out_h as usize);
        resample_projective(&src.pixels, sw, sh, m, out_w as usize, out_h as usize)
    } else if !rotated {
        // Axis-aligned: the frame is an un-oriented source rectangle; exact separable path.
        let (l, t) = g.map(0.0, 0.0);
        let (r, b) = g.map(1.0, 1.0);
        let fr = NormRect {
            x: l.min(r) as f32,
            y: t.min(b) as f32,
            width: (r - l).abs() as f32,
            height: (b - t).abs() as f32,
        };
        let ru = unorient_rect(r_or, orientation);
        let rect = [
            f64::from(fr.x + ru.x * fr.width) * sw as f64,
            f64::from(fr.y + ru.y * fr.height) * sh as f64,
            (f64::from(ru.width * fr.width) * sw as f64).max(1.0),
            (f64::from(ru.height * fr.height) * sh as f64).max(1.0),
        ];
        let (uw, uh) = if orientation >= 5 { (out_h, out_w) } else { (out_w, out_h) };
        let resized = resample(&src.pixels, sw, sh, rect, uw as usize, uh as usize);
        if orientation == 1 {
            resized
        } else {
            orient(&resized, uw as usize, uh as usize, orientation)
        }
    } else {
        // Output (u, v) -> frame -> source (normalized).
        // A straightened crop may leave the image when it is not constrained to it.
        coverage = coverage_map(&affine3, sw, sh, out_w as usize, out_h as usize);
        resample_affine(&src.pixels, sw, sh, affine, out_w as usize, out_h as usize)
    };
    // Frame placement in output px and scale (output px per full-resolution frame px).
    let frame_w = out_w as f32 / r_or.width.max(1e-6);
    let frame_h = out_h as f32 / r_or.height.max(1e-6);
    let half = src.width as f32 / src.full_width.max(1) as f32;
    let scale = frame_w / g.width.max(1) as f32 * half;
    let view =
        super::pipeline::View { frame_x: -r_or.x * frame_w, frame_y: -r_or.y * frame_h, frame_w, frame_h, scale };
    Prepared { width: out_w, height: out_h, pixels, frame_long_edge: frame_w.max(frame_h), view, coverage }
}

/// Resamples the parallelogram `m` (normalized output (u, v) -> normalized source) of an
/// interleaved RGB16 image to `dw x dh`: a Lanczos pre-resample of the bounding box to the
/// output's pixel pitch, then Catmull-Rom interpolation along the rotated grid.
pub fn resample_affine(src: &[u16], sw: usize, sh: usize, m: [f64; 6], dw: usize, dh: usize) -> Vec<u16> {
    // Source px per output px along the output x axis.
    let du = ((m[0] * sw as f64).powi(2) + (m[3] * sh as f64).powi(2)).sqrt() / dw as f64;
    let map = move |u: f64, v: f64| (m[0] * u + m[1] * v + m[2], m[3] * u + m[4] * v + m[5]);
    resample_mapped(src, sw, sh, map, du, dw, dh)
}

/// [`resample_affine`] for a projective map `m` (normalized output (u, v) -> normalized
/// source, row-major 3x3): the Transform warp. The pre-resample pitch follows the finest
/// sampling over the frame (corners and centre), so no detail is lost where the warp
/// magnifies.
pub fn resample_projective(
    src: &[u16],
    sw: usize,
    sh: usize,
    m: super::transform::Mat3,
    dw: usize,
    dh: usize,
) -> Vec<u16> {
    let map = move |u: f64, v: f64| super::transform::apply(&m, u, v).unwrap_or((-1.0, -1.0));
    let mut du = f64::MAX;
    let e = 1e-3;
    for (u, v) in [(0.0, 0.0), (1.0 - e, 0.0), (0.0, 1.0), (1.0 - e, 1.0), (0.5, 0.5)] {
        let (a, b) = (map(u, v), map(u + e, v));
        let d = (((b.0 - a.0) * sw as f64).powi(2) + ((b.1 - a.1) * sh as f64).powi(2)).sqrt() / (e * dw as f64);
        if d.is_finite() && d > 0.0 {
            du = du.min(d);
        }
    }
    resample_mapped(src, sw, sh, map, if du == f64::MAX { 1.0 } else { du }, dw, dh)
}

/// Shared body of [`resample_affine`] / [`resample_projective`]: `du` = source px per output
/// px (pre-resample pitch).
fn resample_mapped(
    src: &[u16],
    sw: usize,
    sh: usize,
    map: impl Fn(f64, f64) -> (f64, f64) + Sync,
    du: f64,
    dw: usize,
    dh: usize,
) -> Vec<u16> {
    let corners = [map(0.0, 0.0), map(1.0, 0.0), map(0.0, 1.0), map(1.0, 1.0)];
    let x0 = corners.iter().map(|c| c.0).fold(f64::MAX, f64::min).clamp(0.0, 1.0) * sw as f64;
    let x1 = corners.iter().map(|c| c.0).fold(f64::MIN, f64::max).clamp(0.0, 1.0) * sw as f64;
    let y0 = corners.iter().map(|c| c.1).fold(f64::MAX, f64::min).clamp(0.0, 1.0) * sh as f64;
    let y1 = corners.iter().map(|c| c.1).fold(f64::MIN, f64::max).clamp(0.0, 1.0) * sh as f64;
    let s = (1.0 / du.max(1e-9)).min(1.0);
    let (bw, bh) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
    let (iw, ih) = (((bw * s).ceil() as usize).max(2), ((bh * s).ceil() as usize).max(2));
    let inter = resample(src, sw, sh, [x0, y0, bw, bh], iw, ih);
    let (kx, ky) = (iw as f64 / bw, ih as f64 / bh);
    let mut out = vec![0u16; dw * dh * 3];
    out.par_chunks_mut(dw * 3).enumerate().for_each(|(y, row)| {
        let v = (y as f64 + 0.5) / dh as f64;
        for x in 0..dw {
            let u = (x as f64 + 0.5) / dw as f64;
            let (nx, ny) = map(u, v);
            let px = (nx * sw as f64 - x0) * kx - 0.5;
            let py = (ny * sh as f64 - y0) * ky - 0.5;
            let p = catmull_rom(&inter, iw, ih, px as f32, py as f32);
            for c in 0..3 {
                row[x * 3 + c] = p[c].round().clamp(0.0, 65535.0) as u16;
            }
        }
    });
    out
}

/// Per output pixel coverage of the source by the map `m` (normalized output -> normalized
/// source, 3x3): 255 inside, 0 outside, anti-aliased over about one output pixel. `None`
/// when every pixel is inside (the output rectangle maps to a convex quad, so its four
/// corners decide).
pub fn coverage_map(m: &super::transform::Mat3, sw: usize, sh: usize, dw: usize, dh: usize) -> Option<Vec<u8>> {
    use super::transform::apply;
    let (sw, sh) = (sw as f64, sh as f64);
    // Half a source pixel of slack: resampling covers the border pixels.
    let (ex, ey) = (0.5 / sw, 0.5 / sh);
    let inside =
        |p: Option<(f64, f64)>| p.is_some_and(|(x, y)| (-ex..=1.0 + ex).contains(&x) && (-ey..=1.0 + ey).contains(&y));
    if [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)].iter().all(|&(u, v)| inside(apply(m, u, v))) {
        return None;
    }
    // Source px per output px at the centre (the anti-aliasing width).
    let du = match (apply(m, 0.5, 0.5), apply(m, 0.5 + 1.0 / dw.max(1) as f64, 0.5)) {
        (Some(a), Some(b)) => (((b.0 - a.0) * sw).powi(2) + ((b.1 - a.1) * sh).powi(2)).sqrt().max(1e-6),
        _ => 1.0,
    };
    let mut out = vec![0u8; dw * dh];
    out.par_chunks_mut(dw.max(1)).enumerate().for_each(|(y, row)| {
        let v = (y as f64 + 0.5) / dh as f64;
        for (x, c) in row.iter_mut().enumerate() {
            let u = (x as f64 + 0.5) / dw as f64;
            *c = match apply(m, u, v) {
                Some((sx, sy)) => {
                    // Distance inside the source border, in output px.
                    let d = (sx * sw).min((1.0 - sx) * sw).min(sy * sh).min((1.0 - sy) * sh) / du;
                    ((d + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8
                }
                None => 0,
            };
        }
    });
    Some(out)
}

fn catmull_rom(img: &[u16], w: usize, h: usize, x: f32, y: f32) -> [f32; 3] {
    let wt = |t: f32| -> [f32; 4] {
        let t2 = t * t;
        let t3 = t2 * t;
        [-0.5 * t3 + t2 - 0.5 * t, 1.5 * t3 - 2.5 * t2 + 1.0, -1.5 * t3 + 2.0 * t2 + 0.5 * t, 0.5 * t3 - 0.5 * t2]
    };
    let (xi, yi) = (x.floor() as isize, y.floor() as isize);
    let (wx, wy) = (wt(x - xi as f32), wt(y - yi as f32));
    let mut acc = [0.0f32; 3];
    for (j, wyj) in wy.iter().enumerate() {
        let yy = (yi + j as isize - 1).clamp(0, h as isize - 1) as usize;
        for (i, wxi) in wx.iter().enumerate() {
            let xx = (xi + i as isize - 1).clamp(0, w as isize - 1) as usize;
            let k = wyj * wxi;
            let p = &img[(yy * w + xx) * 3..(yy * w + xx) * 3 + 3];
            acc[0] += k * f32::from(p[0]);
            acc[1] += k * f32::from(p[1]);
            acc[2] += k * f32::from(p[2]);
        }
    }
    acc
}

/// Separable filter taps for one axis: output i covers source `[start, start + weights.len())`.
struct Taps {
    start: Vec<usize>,
    len: Vec<usize>,
    weights: Vec<f32>,
    stride: usize,
}

/// Lanczos-3 kernel.
fn lanczos3(x: f64) -> f64 {
    let x = x.abs();
    if x < 1e-9 {
        1.0
    } else if x >= 3.0 {
        0.0
    } else {
        let px = std::f64::consts::PI * x;
        3.0 * px.sin() * (px / 3.0).sin() / (px * px)
    }
}

/// Lanczos-3 filter sized to the scale factor (3 lobes of the output pixel spacing when
/// shrinking; exact identity at 1:1 with integer offsets). `offset`/`extent` select the
/// source span in pixels. Shared by the preview (`prepare`) and the export resample, so
/// both see identically resampled camera RGB. Edge taps are renormalized.
fn taps(src_len: usize, offset: f64, extent: f64, out_len: usize) -> Taps {
    let scale = extent / out_len as f64;
    let support = scale.max(1.0);
    let radius = 3.0 * support;
    let stride = (2.0 * radius).ceil() as usize + 2;
    let mut start = Vec::with_capacity(out_len);
    let mut len = Vec::with_capacity(out_len);
    let mut weights = vec![0.0f32; out_len * stride];
    for i in 0..out_len {
        let center = offset + (i as f64 + 0.5) * scale - 0.5;
        let lo = ((center - radius).floor() as i64 + 1).max(0) as usize;
        let hi = (((center + radius).ceil() as i64 - 1).min(src_len as i64 - 1)).max(0) as usize;
        let lo = lo.min(hi);
        let n = (hi - lo + 1).min(stride);
        let w = &mut weights[i * stride..i * stride + n];
        let mut sum = 0.0f64;
        for (k, wk) in w.iter_mut().enumerate() {
            let v = lanczos3(((lo + k) as f64 - center) / support);
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
                calibration: [1.0; 3],
            },
            full_width: w * 2,
            full_height: h * 2,
            display_referred: false,
            source_color: None,
        }
    }

    #[test]
    fn identity_resample_and_flat_downscale() {
        let img = image(7, 5, |x, y| [x as u16 * 100, y as u16 * 100, 7]);
        let p = prepare(&img, 1, &CropSettings::default(), None, 64);
        assert_eq!((p.width, p.height), (7, 5), "never upscaled");
        assert_eq!(p.pixels, img.pixels, "1:1 is exact");
        assert_eq!(p.frame_long_edge, 7.0);

        let flat = image(400, 300, |_, _| [1000, 2000, 3000]);
        let p = prepare(&flat, 1, &CropSettings::default(), None, 100);
        assert_eq!((p.width, p.height), (100, 75));
        assert!(p.pixels.chunks(3).all(|c| c == [1000, 2000, 3000]));
    }

    #[test]
    fn orientation_and_region_mapping() {
        // 4x2 image, pixel value encodes (x, y).
        let img = image(4, 2, |x, y| [x as u16, y as u16, 0]);
        // Orientation 6 (rotate 90 CW for display): output 2x4, top-left shows source (0, 1).
        let p = prepare(&img, 6, &CropSettings::default(), None, 64);
        assert_eq!((p.width, p.height), (2, 4));
        assert_eq!(&p.pixels[0..2], &[0, 1]);
        // Orientation 8 (rotate 90 CCW): top-left shows source (3, 0).
        let p = prepare(&img, 8, &CropSettings::default(), None, 64);
        assert_eq!(&p.pixels[0..2], &[3, 0]);
        // Region in oriented coordinates: the bottom half of an orientation-8 frame is
        // the left half of the stored image.
        let r = NormRect { x: 0.0, y: 0.5, width: 1.0, height: 0.5 };
        assert_eq!(unorient_rect(r, 8), NormRect { x: 0.0, y: 0.0, width: 0.5, height: 1.0 });
        let p = prepare(&img, 8, &CropSettings::default(), Some(r), 64);
        assert_eq!((p.width, p.height), (2, 2));
        // Oriented (0,0) of the crop = stored (1, 0).
        assert_eq!(&p.pixels[0..2], &[1, 0]);
        assert_eq!(p.frame_long_edge, 4.0);
        for o in 1..=8u8 {
            let full = NormRect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 };
            assert_eq!(unorient_rect(full, o), full);
        }
    }

    /// Vertical -50 on a photo of converging verticals (built as the image of two vertical
    /// lines through the inverse warp) renders them vertical: same column at top and bottom.
    #[test]
    fn keystone_warp_straightens_converging_lines() {
        use crate::develop::transform::{self, Geometry};
        use crate::ipc::types::{ParametricAdjustments, TransformSettings};
        let (w, h) = (600u32, 400u32);
        let t = TransformSettings { vertical: -50.0, ..Default::default() };
        let adj = ParametricAdjustments { transform: t, ..Default::default() };
        let geo = Geometry::of(&adj, w * 2, h * 2);
        let to_corr = transform::invert(&geo.warp.unwrap()).unwrap();
        let img = image(w, h, |x, y| {
            let (cx, _) =
                transform::apply(&to_corr, (f64::from(x) + 0.5) / f64::from(w), (f64::from(y) + 0.5) / f64::from(h))
                    .unwrap();
            if (cx - 0.3).abs() < 0.004 || (cx - 0.7).abs() < 0.004 {
                [60000; 3]
            } else {
                [1000; 3]
            }
        });
        // Column centroid of the left line on a row.
        let centroid = |px: &[u16], width: u32, y: u32| {
            let (mut s, mut n) = (0.0, 0.0);
            for x in 0..width / 2 {
                let v = f64::from(px[((y * width + x) * 3) as usize]);
                if v > 20000.0 {
                    s += f64::from(x) * v;
                    n += v;
                }
            }
            s / n
        };
        let (src_top, src_bottom) = (centroid(&img.pixels, w, 40), centroid(&img.pixels, w, 360));
        assert!(src_top - src_bottom > 15.0, "source converges upwards: {src_top} vs {src_bottom}");
        let p = prepare_sized_geo(&img, 1, &geo, None, w, h);
        let (top, bottom) = (centroid(&p.pixels, w, 40), centroid(&p.pixels, w, 360));
        assert!((top - bottom).abs() < 1.0, "vertical after the warp: {top} vs {bottom}");
        assert!((top - 0.3 * f64::from(w)).abs() < 2.0, "{top}");
        // Widening the top pulls the bottom corners off the image: white there after the
        // pipeline.
        let cov = p.coverage.as_ref().expect("corners outside the image");
        let bl = ((h - 1) * w) as usize;
        assert_eq!((cov[0], cov[bl]), (255, 0));
        assert_eq!(cov[(h / 2 * w + w / 2) as usize], 255);
        let mut rgb = vec![10u8; (w * h * 3) as usize];
        fill_outside_rgb8(&mut rgb, cov);
        assert_eq!(&rgb[bl * 3..bl * 3 + 3], &[255, 255, 255]);
        assert_eq!(rgb[((h / 2 * w + w / 2) * 3) as usize], 10);
    }

    /// A straightened crop that leaves the image is filled (coverage), not edge-repeated;
    /// Constrain Crop leaves no empty pixel.
    #[test]
    fn rotated_frame_coverage_and_constrain() {
        use crate::develop::transform::Geometry;
        use crate::ipc::types::{ParametricAdjustments, TransformSettings};
        let img = image(300, 200, |_, _| [5000; 3]);
        let mut t = TransformSettings { rotate: 8.0, ..Default::default() };
        let adj = ParametricAdjustments { transform: t.clone(), ..Default::default() };
        let p = prepare_geo(&img, 1, &Geometry::of(&adj, 600, 400), None, 300);
        let cov = p.coverage.expect("rotated image leaves corners empty");
        assert!(cov[0] == 0 && cov[cov.len() - 1] == 0);
        t.constrain_crop = true;
        let adj = ParametricAdjustments { transform: t, ..Default::default() };
        let p = prepare_geo(&img, 1, &Geometry::of(&adj, 600, 400), None, 300);
        assert!(p.coverage.is_none(), "constrained: no empty pixels");
        assert!(p.width < 300 && p.height < 200);
        // Straightened crop (no transform) running off the image.
        let crop = CropSettings { enabled: true, left: 0.0, top: 0.0, right: 1.0, bottom: 1.0, angle: 6.0 };
        let p = prepare(&img, 1, &crop, None, 300);
        assert!(p.coverage.is_some_and(|c| c.contains(&0)));
        let inside = CropSettings { left: 0.2, top: 0.2, right: 0.8, bottom: 0.8, ..crop };
        assert!(prepare(&img, 1, &inside, None, 300).coverage.is_none());
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
