//! Metadata for exported files (suggested seam; owned by rust-engine-dev).
//!
//! Sources: the RAW's EXIF (`raw::` parsers) and the XMP sidecar (`xmp::sidecar_path`,
//! read-only). Rules per `MetadataInclude` are documented on the IPC type; always:
//! orientation = 1, `crs:` develop settings and `Sieve|*` keywords are never copied,
//! `MetadataOptions.copyright` / `creator` override the source values, software = "Sieve".
//! Verified with `exiftool` in the Phase 6 QA gate.

use std::path::Path;

use crate::ipc::error::AppResult;
use crate::ipc::types::MetadataOptions;

/// Metadata to embed, already filtered by the options. Fields are the implementer's choice
/// (e.g. an EXIF IFD blob + an XMP packet string); `Default` = nothing but the technical tags
/// the encoder always writes (ICC, resolution, orientation 1).
#[derive(Debug, Clone, Default)]
pub struct ExportMetadata {
    /// Serialized EXIF (TIFF-structured, without the `Exif\0\0` header), if any.
    pub exif: Option<Vec<u8>>,
    /// XMP packet, if any.
    pub xmp: Option<String>,
}

/// Blocking. Reads the RAW's EXIF and its sidecar (if present) and filters them per `options`.
/// A missing/unreadable sidecar is not an error (EXIF only).
pub fn collect(raw_path: &Path, options: &MetadataOptions) -> AppResult<ExportMetadata> {
    let _ = (raw_path, options);
    todo!("rust-engine-dev: export::metadata::collect")
}
