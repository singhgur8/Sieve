//! Adobe "big table" strings: the `crs:Table_<MD5>` attribute values in look profiles and
//! sidecars (rust-engine-dev). Reference: Adobe DNG SDK `dng_big_table.cpp`
//! (`dng_big_table::DecodeFromString` / `EncodeAsString`, `dng_look_table`, `dng_rgb_table`).
//!
//! Known format (verify against the SDK and the installed look files):
//! - ASCII text in a base-85 alphabet (Z85-like, Adobe's own character order), each 5 chars
//!   -> one little-endian u32, least-significant digit first; a short final group encodes the
//!   remaining bytes.
//! - Decoded bytes: u32 LE uncompressed length, then a zlib stream (the `flate2` crate).
//! - Decompressed: a `dng_stream` (big-endian unless flagged) holding the table type,
//!   version, dimensions and samples: HSV look tables (as DCP `ProfileLookTable`: hue shift,
//!   sat scale, val scale per sample) or RGB tables (`dng_rgb_table`: dims 1 or 3, divisions,
//!   u16 samples, primaries, gamma, gamut processing, min/max amount).
//! - The attribute name's MD5 is the MD5 of the *decompressed* table as the SDK computes its
//!   fingerprint; a decoded table whose fingerprint differs must be rejected.
//!
//! Tables are never written by Sieve; sidecar `crs:Table_*` attributes stay byte-for-byte.

use super::dcp::HsvTable;

/// A 3D RGB table (`crs:RGBTable`), sampled in its own encoding.
#[derive(Debug, Clone, PartialEq)]
pub struct RgbTable {
    /// Samples per axis (1D tables are expanded to 3D or stored with `dims = 1`).
    pub divisions: u32,
    pub dims: u8,
    /// `divisions^3` RGB samples in 0..=1, red fastest.
    pub samples: Vec<[f32; 3]>,
    /// Colour space / transfer the table is defined in (SDK `dng_rgb_table::primaries_*`,
    /// `gamma_*`), kept raw until the pipeline maps them.
    pub primaries: u32,
    pub gamma: u32,
    /// Blend range for `LookSettings.amount` (SDK `fMinAmount` / `fMaxAmount`).
    pub min_amount: f32,
    pub max_amount: f32,
}

/// A decoded table value.
#[derive(Debug, Clone, PartialEq)]
pub enum BigTable {
    Look(HsvTable),
    Rgb(RgbTable),
}

/// Decodes a `crs:Table_<md5>` value; `md5` (32 hex digits from the attribute name) is
/// checked against the table's fingerprint.
pub fn decode(value: &str, md5: &str) -> Result<BigTable, String> {
    let _ = (value, md5);
    todo!("rust-engine-dev: profiles::table::decode (Adobe big-table string)")
}
