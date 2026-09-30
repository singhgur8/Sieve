//! Render-space image statistics. [`render_stats`] (glue, architect) renders through the
//! develop cache; [`measure`] (vision-ml-dev) computes the numbers from the 8-bit output.

use super::STATS_MAX_EDGE;
use crate::develop::pipeline::RenderedImage;
use crate::develop::{DevelopCache, SourceImage};
use crate::ipc::error::AppResult;
use crate::ipc::types::{ImageId, ImageStats, NormRect, ParametricAdjustments, WhiteBalance, WhiteBalanceValues};
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
    let _ = (image_id, image, region);
    todo!("vision-ml-dev: luma mean/log-mean/percentiles, clipping, mean Oklab, neutral estimate")
}
