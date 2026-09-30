//! Parametric pipeline (suggested seam). Shared by the Phase 5 preview and the Phase 6
//! full-resolution export, so it must be resolution-independent: spatial operators
//! (texture, clarity, dehaze) take radii relative to the image size.

use crate::ipc::types::{Histogram, ParametricAdjustments};
use crate::lut::Lut;

use super::source::LinearImage;

/// Display-referred 8-bit sRGB output.
#[derive(Debug, Clone)]
pub struct RenderedImage {
    pub width: u32,
    pub height: u32,
    /// Interleaved RGB8, orientation applied.
    pub rgb: Vec<u8>,
    pub histogram: Histogram,
}

/// Renders `src` (already cropped/resampled to the output size, orientation applied) with
/// `adjustments`; `lut` is the resolved `adjustments.lut` (if present in the library).
pub fn render(src: &LinearImage, adjustments: &ParametricAdjustments, lut: Option<&Lut>) -> RenderedImage {
    let _ = (src, adjustments, lut);
    todo!("rust-engine-dev: develop::pipeline::render")
}
