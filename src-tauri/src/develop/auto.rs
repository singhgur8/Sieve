//! Lightroom-style "Auto" (IPC v14): Basic-panel auto tone and auto white balance.
//! Contract by the architect; bodies: rust-engine-dev.
//!
//! - [`auto_tone`]: render `adjustments` with the requested sliders at 0 (the rest as given,
//!   incl. white balance and profile) through `DevelopCache::render_image` at a small size,
//!   measure the output and solve absolute values for the requested sliders (`keys`, a subset of
//!   `AdjustmentField::AUTO_TONE`; `None` = all of them). Target Lightroom's behaviour:
//!   exposure from the (face-weighted, when faces are known) mid-tone level, whites/blacks to
//!   the clip points without clipping skin, highlights/shadows to recover detail, contrast,
//!   vibrance and saturation modest. Results are within slider ranges, rounded like Lightroom
//!   (exposure to 0.05 EV, others to integers). Deterministic.
//! - [`auto_white_balance`]: temperature/tint that neutralise the scene's estimated illuminant
//!   (grey-world on low-chroma, unclipped pixels with a skin-tone guard), via
//!   `camera::values_of_multipliers` like the WB picker; clamped to slider ranges.
//! - Errors: the develop source cannot be decoded -> the usual `file_missing` / `decode_failed`.

use super::{DevelopCache, SourceImage};
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::{AdjustmentField, AutoToneValues, ParametricAdjustments, WhiteBalanceValues};

pub fn auto_tone(
    cache: &DevelopCache,
    src: &SourceImage,
    adjustments: &ParametricAdjustments,
    keys: &[AdjustmentField],
) -> AppResult<AutoToneValues> {
    let _ = (cache, src, adjustments, keys);
    Err(AppError::internal("Auto tone is not implemented yet (IPC v14 stub, rust-engine-dev)."))
}

pub fn auto_white_balance(
    cache: &DevelopCache,
    src: &SourceImage,
    adjustments: &ParametricAdjustments,
) -> AppResult<WhiteBalanceValues> {
    let _ = (cache, src, adjustments);
    Err(AppError::internal("Auto white balance is not implemented yet (IPC v14 stub, rust-engine-dev)."))
}
