//! Develop settings <-> XMP (Phase 5): `crs:` (Adobe Camera Raw Settings) 1:1 with
//! `ParametricAdjustments`, plus LumenRAW-only fields in the `lumenraw:` namespace.
//! The mapping table below is contract; the codec bodies belong to rust-engine-dev.
//!
//! Write (catalog -> sidecar), only for images that have an `adjustments` row (never add
//! or remove `crs:` for images not edited in LumenRAW):
//! - Set every property in [`CRS_FIELDS`] / [`CRS_HSL_BANDS`], plus `crs:ProcessVersion
//!   = "11.0"` and `crs:HasSettings = "True"`. Numbers are written signed with the shortest
//!   decimal that round-trips the f32 (`+15`, `-7.5`, `+0.7`; zero as `0`), so
//!   catalog -> XMP -> catalog is lossless for every valid value.
//! - `whiteBalance = as_shot` -> `crs:WhiteBalance = "As Shot"` and remove `crs:Temperature`
//!   / `crs:Tint`; `custom` -> `"Custom"` + both values.
//! - `lut` -> `lumenraw:LutId`, `lumenraw:LutAmount`; `null` -> remove both.
//! - Every other `crs:` property (tone curves, sharpening, crop, lens, masks, ...) and every
//!   other namespace stays byte-for-byte (span-preserving merge, as in Phase 4).
//!
//! Read (sidecar -> catalog; `read_xmp`, import, newer-wins auto-sync):
//! - Only if the packet has any property in [`CRS_FIELDS`] and `crs:ProcessVersion` is
//!   absent or >= 6.7 (PV2012+). Older process versions (2003/2010: `crs:Exposure` without
//!   the 2012 suffix) are not imported (`XmpSyncReport` counts them as succeeded, catalog
//!   develop settings unchanged).
//! - Missing owned properties read as neutral. Values are clamped to the contract ranges.
//! - `crs:WhiteBalance`: "As Shot" -> `as_shot`; "Custom" -> `custom{Temperature, Tint}`;
//!   Lightroom presets map to custom with Lightroom's values (Daylight 5500/+10, Cloudy
//!   6500/+10, Shade 7500/+10, Tungsten 2850/0, Fluorescent 3800/+21, Flash 5500/0);
//!   "Auto" -> `as_shot` (lossy; noted in `docs/architecture.md`).
//! - `lumenraw:LutId` / `LutAmount` restore `lut` (kept even if the LUT is not in the
//!   library; the render reports `lutMissing`).
//! - Applied through `develop::history::commit(.., LABEL_READ_XMP)` only when the values
//!   differ from the catalog; such images are listed in `XmpSyncReport.changed`.
//! - A sidecar without owned `crs:` properties leaves the catalog's develop settings as-is.

use crate::ipc::types::ParametricAdjustments;

pub const CRS_NS: &str = "http://ns.adobe.com/camera-raw-settings/1.0/";
pub const LUMENRAW_NS: &str = "http://lumenraw.app/ns/1.0/";
/// Written with every develop write.
pub const PROCESS_VERSION: &str = "11.0";

/// Scalar `crs:` properties and the `ParametricAdjustments` field (camelCase) they carry.
pub const CRS_FIELDS: &[(&str, &str)] = &[
    ("WhiteBalance", "whiteBalance.mode"),
    ("Temperature", "whiteBalance.temperatureK"),
    ("Tint", "whiteBalance.tint"),
    ("Exposure2012", "exposure"),
    ("Contrast2012", "contrast"),
    ("Highlights2012", "highlights"),
    ("Shadows2012", "shadows"),
    ("Whites2012", "whites"),
    ("Blacks2012", "blacks"),
    ("Texture", "texture"),
    ("Clarity2012", "clarity"),
    ("Dehaze", "dehaze"),
    ("Vibrance", "vibrance"),
    ("Saturation", "saturation"),
];

/// HSL: `crs:<prefix><Band>` for each prefix x band, e.g. `crs:HueAdjustmentOrange`
/// <-> `hsl.hue.orange`.
pub const CRS_HSL_PREFIXES: &[(&str, &str)] =
    &[("HueAdjustment", "hue"), ("SaturationAdjustment", "saturation"), ("LuminanceAdjustment", "luminance")];
pub const CRS_HSL_BANDS: &[(&str, &str)] = &[
    ("Red", "red"),
    ("Orange", "orange"),
    ("Yellow", "yellow"),
    ("Green", "green"),
    ("Aqua", "aqua"),
    ("Blue", "blue"),
    ("Purple", "purple"),
    ("Magenta", "magenta"),
];

/// `lumenraw:` properties.
pub const LUMENRAW_FIELDS: &[(&str, &str)] = &[("LutId", "lut.id"), ("LutAmount", "lut.amount")];

/// One property to set (`Some`) or remove (`None`) in the sidecar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyEdit {
    /// [`CRS_NS`] or [`LUMENRAW_NS`].
    pub ns: &'static str,
    pub name: String,
    pub value: Option<String>,
}

/// Property edits that make a sidecar carry `adj` (per the write rules above).
pub fn encode(adj: &ParametricAdjustments) -> Vec<PropertyEdit> {
    let _ = adj;
    todo!("rust-engine-dev: xmp::crs::encode")
}

/// Develop settings from a packet's properties; `get(ns, name)` returns the raw value.
/// `Ok(None)` when the packet carries no importable develop settings (see read rules).
pub fn decode(get: &dyn Fn(&str, &str) -> Option<String>) -> Result<Option<ParametricAdjustments>, String> {
    let _ = get;
    todo!("rust-engine-dev: xmp::crs::decode")
}
