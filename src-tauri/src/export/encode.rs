//! Encoders (suggested seam; owned by rust-engine-dev). JPEG via TurboJPEG (already linked),
//! TIFF/PNG/WebP via crates of the implementer's choice, HEIC via macOS ImageIO
//! (`CGImageDestination`, `public.heic`) when available. Every file embeds the ICC profile of
//! `settings.color_space` and the resolution `settings.resize.resolution_ppi`.

use std::path::Path;

use super::develop::ExportImage;
use super::metadata::ExportMetadata;
use crate::ipc::error::AppResult;
use crate::ipc::types::{ExportColorSpace, ExportFormatInfo, ExportSettings};

/// ICC profile bytes for `space` (bundled, v2 or v4 matrix/TRC profiles: sRGB IEC61966-2.1,
/// Display P3, Adobe RGB (1998)-compatible).
pub fn icc_profile(space: ExportColorSpace) -> &'static [u8] {
    let _ = space;
    todo!("rust-engine-dev: export::encode::icc_profile")
}

/// Availability of every `ExportFormatKind`, in enum order (backs `get_export_capabilities`).
pub fn probe_formats() -> Vec<ExportFormatInfo> {
    todo!("rust-engine-dev: export::encode::probe_formats")
}

/// Blocking. Encodes `image` per `settings.format` with ICC, resolution and `metadata`, and
/// writes it to `path` (the caller passes a temp path and renames on success). An
/// unavailable format -> `invalid_argument`.
pub fn write_file(
    image: &ExportImage,
    settings: &ExportSettings,
    metadata: &ExportMetadata,
    path: &Path,
) -> AppResult<()> {
    let _ = (image, settings, metadata, path);
    todo!("rust-engine-dev: export::encode::write_file")
}
