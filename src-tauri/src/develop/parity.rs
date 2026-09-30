//! Phase 7b Lightroom-parity stages for the shared develop pipeline (preview + export).
//! Contract seam by the architect; bodies belong to rust-engine-dev. Wire them into
//! `develop::pipeline` in this order (after camera profile/look, see `profiles` docs):
//!
//! 1. Geometry: [`crop_geometry`] decides the output frame (crop + straighten) before
//!    resampling; `RenderOptions.region` and output sizes refer to the cropped frame.
//! 2. Scene-referred, linear working space:
//!    [`calibration_matrix`] (folded into camera -> working), white balance, exposure,
//!    [`denoise`] (on the demosaiced linear image, before tone), then the existing PV2012 tone
//!    (highlights/shadows/whites/blacks/contrast/texture/clarity/dehaze).
//! 3. Display-referred (after the base/profile tone curve, values 0..=1 in the
//!    curve's encoding): [`CurveLuts`] (parametric, then point master, then point R/G/B),
//!    HSL / vibrance / saturation (existing), [`color_grade`], [`shadow_tint`] folded into
//!    calibration if simpler, B&W via [`gray_mix`], [`vignette`] (post-crop), [`grain`].
//! 4. Output: [`sharpen`] (capture sharpening, on luminance, before output sharpening/LUT
//!    encode), LUT, encode.
//!
//! Resolution independence: every spatial radius is given at full resolution and scaled by
//! `scale` = working px per full-res px (previews are downscaled; exports may be too), so a
//! 1024 px draft, the 2048 px preview and a full-res export look alike. Grain is seeded per
//! image and generated in cropped-frame coordinates.
//!
//! Phase 7c forward-compatibility (masks): local adjustment sets will add spatially varying
//! deltas for exposure, contrast, highlights, shadows, whites, blacks, temperature/tint,
//! texture, clarity, dehaze, saturation, hue, sharpness, luminance noise, moire, defringe and
//! toning. Keep stages parameterisable per pixel (a stage takes its parameters from a
//! constants struct that can be overridden by a per-pixel mask blend) rather than baking
//! global constants into LUTs where a local variant must exist (tone, colour, detail).

use crate::ipc::types::{
    CameraCalibration, ColorGrading, CropSettings, CurvePoint, Grain, HslChannels, NoiseReduction, ParametricCurve,
    PointCurves, PostCropVignette, Sharpening, ToneCurve,
};

/// Interleaved RGB f32 working image (linear or display-referred depending on the stage).
pub struct Working<'a> {
    pub width: usize,
    pub height: usize,
    pub rgb: &'a mut [f32],
}

/// Tone-curve lookup tables over 0..=1 (`CurveLuts::SIZE` entries each, linear interpolation
/// between entries). `None` = identity (skip).
#[derive(Debug, Clone, PartialEq)]
pub struct CurveLuts {
    /// Parametric curve composed with the master point curve (applied to R, G, B).
    pub master: Option<Vec<f32>>,
    pub red: Option<Vec<f32>>,
    pub green: Option<Vec<f32>>,
    pub blue: Option<Vec<f32>>,
}

impl CurveLuts {
    pub const SIZE: usize = 4096;
}

/// Lightroom's parametric region curve (regions split at the three splits, each region
/// amount bends that part of the curve; 0 everywhere = identity), as a LUT.
pub fn parametric_curve_lut(curve: &ParametricCurve) -> Option<Vec<f32>> {
    let _ = curve;
    todo!("rust-engine-dev: develop::parity::parametric_curve_lut")
}

/// Point curve through `points` (0..=255 axes) with Lightroom's interpolation (monotone
/// cubic spline; constant beyond the end points), as a LUT over 0..=1.
pub fn point_curve_lut(points: &[CurvePoint]) -> Option<Vec<f32>> {
    let _ = points;
    todo!("rust-engine-dev: develop::parity::point_curve_lut")
}

/// All tone-curve LUTs for `curve`, plus the look's own point curves (`look`, applied
/// before the user's curve at the look amount) when the look is resolved.
pub fn curve_luts(curve: &ToneCurve, look: Option<(&PointCurves, f32)>) -> CurveLuts {
    let _ = (curve, look);
    todo!("rust-engine-dev: develop::parity::curve_luts")
}

/// Camera-calibration primaries adjustment as a 3x3 matrix to fold into camera -> working
/// (hue rotates each primary, saturation scales its chroma; identity for all zeros).
pub fn calibration_matrix(cal: &CameraCalibration) -> [[f32; 3]; 3] {
    let _ = cal;
    todo!("rust-engine-dev: develop::parity::calibration_matrix")
}

/// Shadow tint (green -/magenta +) weighted towards the shadows.
pub fn shadow_tint(rgb: [f32; 3], tint: f32) -> [f32; 3] {
    let _ = (rgb, tint);
    todo!("rust-engine-dev: develop::parity::shadow_tint")
}

/// Color grading per pixel on display-referred values: shadow/midtone/highlight wheels
/// weighted by luminance ranges (overlap from `blending`, pivot from `balance`), then global.
pub fn color_grade(rgb: [f32; 3], grading: &ColorGrading) -> [f32; 3] {
    let _ = (rgb, grading);
    todo!("rust-engine-dev: develop::parity::color_grade")
}

/// Monochrome value from the B&W mixer (Lightroom's 8-band gray mix; all zeros = neutral
/// luminance conversion).
pub fn gray_mix(rgb: [f32; 3], mixer: &HslChannels) -> f32 {
    let _ = (rgb, mixer);
    todo!("rust-engine-dev: develop::parity::gray_mix")
}

/// Luminance + colour noise reduction on the linear image.
pub fn denoise(img: &mut Working, nr: &NoiseReduction, scale: f32) {
    let _ = (img, nr, scale);
    todo!("rust-engine-dev: develop::parity::denoise")
}

/// Capture sharpening (unsharp mask on luminance with detail/edge masking), radius in
/// full-resolution px times `scale`.
pub fn sharpen(img: &mut Working, sharpening: &Sharpening, scale: f32) {
    let _ = (img, sharpening, scale);
    todo!("rust-engine-dev: develop::parity::sharpen")
}

/// Post-crop vignette over the whole (cropped) working frame.
pub fn vignette(img: &mut Working, v: &PostCropVignette) {
    let _ = (img, v);
    todo!("rust-engine-dev: develop::parity::vignette")
}

/// Film grain, deterministic for `seed` (the image id), size scaled by `scale`.
pub fn grain(img: &mut Working, g: &Grain, seed: u64, scale: f32) {
    let _ = (img, g, seed, scale);
    todo!("rust-engine-dev: develop::parity::grain")
}

/// Output frame of a crop: size in source px (orientation applied) and the affine map from
/// normalized output coordinates (0..=1, oriented) to normalized *un-oriented* source
/// coordinates (`[a, b, c, d, e, f]`: sx = a*x + b*y + c, sy = d*x + e*y + f).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CropGeometry {
    pub width: u32,
    pub height: u32,
    pub to_source: [f64; 6],
}

/// Crop/straighten geometry in Lightroom's semantics (`CropSettings` docs): `src_w/src_h`
/// un-oriented source size, `orientation` EXIF 1..=8. Disabled crop = the whole frame.
/// Verify against Lightroom exports of the user's cropped frames (99 in the sample set,
/// angles up to 10 degrees).
pub fn crop_geometry(crop: &CropSettings, src_w: u32, src_h: u32, orientation: u8) -> CropGeometry {
    let _ = (crop, src_w, src_h, orientation);
    todo!("rust-engine-dev: develop::parity::crop_geometry")
}
