//! Look profiles (`crs:PresetType="Look"` XMP files, and `<crs:Look>` structs in sidecars)
//! (rust-engine-dev).
//!
//! A look file's top-level `rdf:Description` carries: `crs:UUID`, `crs:Name` (rdf:Alt),
//! `crs:Group` (rdf:Alt), `crs:SupportsAmount`, `crs:SupportsMonochrome`,
//! `crs:CameraModelRestriction`, `crs:CameraProfile`, `crs:ConvertToGrayscale`,
//! `crs:LookTable` / `crs:RGBTable` (MD5 references) + the matching `crs:Table_<MD5>` values,
//! and ordinary develop settings (e.g. `crs:Clarity2012`, `crs:ToneCurvePV2012*`) that the
//! look applies. A sidecar's `<crs:Look>` holds the same inside `crs:Parameters` (without
//! the tables for Adobe's bundled looks; with `crs:Table_*` on the sidecar's top level for
//! looks that are not installed).
//!
//! Parsing reuses `xmp::packet` (a nested-struct reader must be added there: today only
//! top-level `rdf:Description` properties are indexed).

use crate::ipc::types::ParametricAdjustments;

use super::table::BigTable;

#[derive(Debug, Clone, PartialEq)]
pub struct LookProfile {
    pub uuid: String,
    pub name: String,
    pub group: String,
    pub supports_amount: bool,
    pub monochrome: bool,
    pub camera_profile: Option<String>,
    /// `crs:CameraModelRestriction` (empty = any camera).
    pub camera_model_restriction: Option<String>,
    /// Decoded `LookTable` and/or `RGBTable`.
    pub tables: Vec<BigTable>,
    /// The look's own develop settings (parsed with `xmp::crs::decode` over the look's
    /// properties, neutral where absent): applied at `amount` beneath the user's settings.
    pub parameters: ParametricAdjustments,
}

impl LookProfile {
    /// Parses a look profile file (`Settings/**/*.xmp`). `Ok(None)` if it is not a look
    /// (`crs:PresetType` other than "Look").
    pub fn parse_file(xmp: &str) -> Result<Option<LookProfile>, String> {
        let _ = xmp;
        todo!("rust-engine-dev: profiles::look::LookProfile::parse_file")
    }

    /// The look recorded in a sidecar (`<crs:Look>` + top-level `crs:Table_*`), used when the
    /// look is not installed. `Ok(None)` if the sidecar has no look.
    pub fn from_sidecar(xmp: &str) -> Result<Option<LookProfile>, String> {
        let _ = xmp;
        todo!("rust-engine-dev: profiles::look::LookProfile::from_sidecar")
    }
}
