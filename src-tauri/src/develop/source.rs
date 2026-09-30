//! Develop source: half-size linear camera-RGB decode via LibRaw (suggested seam).
//!
//! LibRaw settings: `half_size = 1`, `use_camera_wb = 0`, `user_mul = {1,1,1,1}` (no WB),
//! `output_color = 0` (raw camera colour), `gamm = {1,1}`, `no_auto_bright = 1`,
//! `output_bps = 16`, `user_flip = 0` (orientation applied by the pipeline). Needs new FFI
//! entry points in `raw/libraw.rs` (`libraw_unpack`, `libraw_dcraw_process`,
//! `libraw_dcraw_make_mem_image`, `libraw_set_*`, `libraw_get_cam_mul`, colour data).

use std::path::Path;

use crate::ipc::error::AppResult;

/// Linear camera-RGB image plus the colour metadata the pipeline needs.
#[derive(Debug, Clone)]
pub struct LinearImage {
    pub width: u32,
    pub height: u32,
    /// Interleaved RGB, black-subtracted and scaled so the white level = 65535.
    pub pixels: Vec<u16>,
    /// As-shot camera multipliers (R, G, B), normalized to G = 1. `None` if unrecorded.
    pub as_shot_mul: Option<[f32; 3]>,
    /// Camera RGB -> XYZ (D50) 3x3 row-major, from LibRaw's `cam_xyz` (inverted).
    pub cam_to_xyz: [[f32; 3]; 3],
    /// Full-size output dimensions before orientation.
    pub full_width: u32,
    pub full_height: u32,
}

/// Blocking half-size decode (~300-800 ms for 24-33 MP on Apple Silicon).
pub fn decode_half_size(path: &Path) -> AppResult<LinearImage> {
    let _ = path;
    todo!("rust-engine-dev: develop::source::decode_half_size")
}
