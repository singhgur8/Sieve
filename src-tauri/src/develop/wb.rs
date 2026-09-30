//! White balance conversions (suggested seam). Temperature/tint follow Adobe's DNG model
//! (Robertson isotherms, tint = offset along the isotherm, Lightroom scale: tint 1 unit =
//! 1/3000 uv), so values read from Lightroom sidecars land close to Lightroom's rendering.

use crate::ipc::types::WhiteBalanceValues;

/// Camera multipliers (R, G, B; G = 1) that neutralize the given temperature/tint.
pub fn multipliers_for(values: WhiteBalanceValues, cam_to_xyz: &[[f32; 3]; 3]) -> [f32; 3] {
    let _ = (values, cam_to_xyz);
    todo!("rust-engine-dev: develop::wb::multipliers_for")
}

/// Inverse of [`multipliers_for`]: the temperature/tint of the as-shot multipliers.
pub fn values_for(multipliers: [f32; 3], cam_to_xyz: &[[f32; 3]; 3]) -> WhiteBalanceValues {
    let _ = (multipliers, cam_to_xyz);
    todo!("rust-engine-dev: develop::wb::values_for")
}
