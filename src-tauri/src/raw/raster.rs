//! Non-RAW ("raster") sources, Phase 7b: JPEG, HEIC/HEIF, TIFF, PNG.
//! Contract seam by the architect; bodies belong to rust-engine-dev.
//!
//! Rules (docs/architecture.md "Non-RAW sources"):
//! - Originals are never modified (no embedded-XMP writes, no re-encoding). Edits go to a
//!   `<file name>.xmp` sidecar (`xmp::sidecar_path`, e.g. `IMG_1.JPG.xmp`).
//! - Ingest (`extract`): EXIF (make/model/lens/capture time/exposure/orientation) and a
//!   preview for the thumbnail pipeline. For JPEG the file itself is the preview
//!   (read it into `buf`, `Preview::Embedded`; TurboJPEG is already linked). HEIC/TIFF/PNG
//!   are decoded to 8-bit sRGB (`Preview::Libraw(Thumb::Rgb)` or a new variant) at
//!   <= `preview::PREVIEW_EDGE` long edge, colour-managed from the embedded ICC.
//!   Suggested decoder: macOS ImageIO (`CGImageSource`) for HEIC/TIFF/PNG (ICC, 16-bit,
//!   orientation-aware); `sensor_layout` stays `unknown`, `camera_make` from EXIF else `other`.
//! - Develop (`decode_linear`): decode -> linearize with the source transfer curve -> 16-bit
//!   linear light in the source's primaries -> [`to_linear_image`] for the shared pipeline.
//!   The pipeline must treat such sources as *display-referred*: no `BASELINE_EV`, no base
//!   (filmic) curve, so neutral adjustments reproduce the file (within 1/255). Add a
//!   `display_referred: bool` to `develop::source::LinearImage` (or `ColorInfo`) for that.
//!   White balance: as-shot multipliers `[1, 1, 1]`, as-shot temperature/tint reported as
//!   6500 K / 0; custom temperature/tint go through the same DNG-style model with the source
//!   primaries as "camera" space.
//! - Export uses `decode_linear(path, format, None)` (full size) instead of LibRaw.
//! - Embedded XMP (Lightroom writes develop settings *into* JPEG/TIFF): read-only fallback
//!   when no sidecar exists (`embedded_xmp`), parsed with `xmp::packet::parse` like a sidecar.

use std::path::Path;

use crate::develop::source::LinearImage;
use crate::ipc::types::ImageFormat;

use super::Extracted;

/// Colour encoding of a decoded raster source, identified from its ICC profile
/// (`Unknown` = no/unsupported profile: decoded as sRGB, `DevelopWarningCode::SourceColorAssumed`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceColorSpace {
    Srgb,
    DisplayP3,
    AdobeRgb,
    /// ProPhoto / ROMM RGB (16-bit TIFFs from Lightroom/Photoshop).
    ProPhoto,
    /// Profile description if there was one.
    Unknown(Option<String>),
}

/// Linear-light decode of a raster file (orientation *not* applied, like RAW decodes).
#[derive(Debug, Clone)]
pub struct RasterImage {
    pub width: u32,
    pub height: u32,
    /// Interleaved RGB, linear light, 0..=65535, in `color_space` primaries.
    pub pixels: Vec<u16>,
    pub color_space: SourceColorSpace,
    /// Bits per sample in the file (8 or 16).
    pub bit_depth: u8,
    /// Full-size dimensions of the file (equal to `width/height` unless downscaled).
    pub full_width: u32,
    pub full_height: u32,
    /// EXIF orientation 1..=8 from the file, if any.
    pub orientation: Option<u8>,
}

/// Ingest: metadata + preview pixels for a raster file (see module docs). Same error
/// contract as `raw::extract`: `Err` only if the file cannot be opened at all.
pub fn extract(path: &Path, format: ImageFormat, buf: &mut Vec<u8>) -> Result<Extracted, String> {
    let _ = (path, format, buf);
    todo!("rust-engine-dev: raw::raster::extract (Phase 7b non-RAW ingest)")
}

/// Develop/export: decodes `path` to linear light. `max_edge` = downscale (box/area filter
/// in linear light) so the long edge is <= `max_edge` (the editor's cached source, like the
/// RAW half-size decode: use 4096); `None` = full size (export).
pub fn decode_linear(path: &Path, format: ImageFormat, max_edge: Option<u32>) -> Result<RasterImage, String> {
    let _ = (path, format, max_edge);
    todo!("rust-engine-dev: raw::raster::decode_linear (Phase 7b non-RAW develop source)")
}

/// Wraps a raster decode as the pipeline's input: "camera RGB" = source primaries,
/// `as_shot_mul = daylight_mul = [1, 1, 1]`, `rgb_cam` = source primaries -> linear sRGB
/// (D65), `xyz_to_cam` = XYZ (D65) -> source primaries; marked display-referred (module docs).
pub fn to_linear_image(img: RasterImage) -> LinearImage {
    let _ = img;
    todo!("rust-engine-dev: raw::raster::to_linear_image (Phase 7b)")
}

/// The XMP packet embedded in the file, if any (JPEG APP1 `http://ns.adobe.com/xap/1.0/`
/// incl. extended XMP, TIFF tag 700, PNG `iTXt` `XML:com.adobe.xmp`, HEIC `mime` item).
/// Read-only: Sieve never writes into originals.
pub fn embedded_xmp(path: &Path, format: ImageFormat) -> Result<Option<String>, String> {
    let _ = (path, format);
    todo!("rust-engine-dev: raw::raster::embedded_xmp (Phase 7b)")
}
