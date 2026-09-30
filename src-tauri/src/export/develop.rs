//! Full-resolution develop seam (suggested; owned by rust-engine-dev).
//!
//! `decode_full` belongs next to `develop::source::decode_half_size` (shared LibRaw FFI and
//! settings); it lives here only to avoid conflicts with the concurrent Phase 5 work and may
//! be moved to `develop::source` (re-export it from here or update the callers).
//!
//! Pipeline sharing: `develop::pipeline::render` returns 8-bit sRGB for the preview. Export
//! needs the same stages up to the display-referred linear working image, then its own
//! output stage. Suggested refactor of `develop::pipeline` (internal, no contract change):
//! `develop_linear(src, adjustments) -> WorkingImage` (f32 linear Rec.2020, tone-mapped,
//! display-referred) + `encode_srgb8(working, lut) -> RenderedImage` for the preview, and
//! for export `encode_output(working, lut, ExportColorSpace) -> f32 RGB in the target
//! space's transfer curve`. Colour rule: without a LUT, convert linear Rec.2020 straight to
//! the target space (keeps P3 / Adobe RGB gamut, clip per channel); with a LUT, the LUT is
//! applied on sRGB-encoded values as in the preview (gamut limited to sRGB) and the result is
//! converted to the target space.

use std::path::Path;

use crate::develop::source::LinearImage;
use crate::ipc::error::AppResult;
use crate::ipc::types::{ExportSettings, ParametricAdjustments, ResizeOptions};
use crate::lut::Lut;

/// Developed, resized, sharpened, quantized pixels ready for the encoder.
#[derive(Debug, Clone)]
pub struct ExportImage {
    pub width: u32,
    pub height: u32,
    /// Interleaved RGB in the target colour space's transfer curve, orientation applied.
    pub pixels: ExportPixels,
}

#[derive(Debug, Clone)]
pub enum ExportPixels {
    Rgb8(Vec<u8>),
    Rgb16(Vec<u16>),
}

/// Blocking full-resolution decode: LibRaw full demosaic with the same settings as the
/// preview's half-size decode (camera RGB, no WB, linear, 16-bit, `highlight = 0`,
/// `user_flip = 0`) except `half_size = 0` and `user_qual = 3` (AHD for Bayer; LibRaw runs
/// its 3-pass Markesteijn interpolation for X-Trans at that quality). Releases LibRaw's
/// buffers before returning. `full_width/full_height` equal `width/height`.
pub fn decode_full(path: &Path) -> AppResult<LinearImage> {
    let _ = path;
    todo!("rust-engine-dev: export::develop::decode_full")
}

/// Output pixel size for a full-resolution (orientation-corrected) `full` size:
/// - `none` -> `full`;
/// - `long_edge{px}` / `short_edge{px}` -> that edge = px, other edge rounded to nearest;
/// - `megapixels{mp}` -> scale = sqrt(mp * 1e6 / (w * h)), both edges rounded;
/// - `width_height{w, h}` -> largest size with the same aspect that fits in w x h;
/// - `dont_enlarge` -> never larger than `full` (returns `full` when the target is larger);
/// - every edge >= 1.
pub fn output_size(full: (u32, u32), resize: &ResizeOptions) -> (u32, u32) {
    let _ = (full, resize);
    todo!("rust-engine-dev: export::develop::output_size")
}

/// Blocking: `src` (full decode) -> resample to `output_size` in linear light (orientation
/// from EXIF `orientation`, 1..=8, applied) -> shared parametric pipeline -> output colour
/// space -> output sharpening (`settings.sharpening`, radius/amount by media and output
/// size, on the encoded luminance) -> quantize to `settings.format.bit_depth()`.
/// `lut` is the resolved `adjustments.lut` (`None` if absent or missing from the library).
pub fn render_full(
    src: &LinearImage,
    orientation: Option<u8>,
    adjustments: &ParametricAdjustments,
    lut: Option<&Lut>,
    settings: &ExportSettings,
) -> AppResult<ExportImage> {
    let _ = (src, orientation, adjustments, lut, settings);
    todo!("rust-engine-dev: export::develop::render_full")
}
