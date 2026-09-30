//! Look profiles (`crs:PresetType="Look"` XMP files, and `<crs:Look>` structs in sidecars)
//! (rust-engine-dev).
//!
//! A look file's top-level `rdf:Description` carries: `crs:UUID`, `crs:Name` (rdf:Alt),
//! `crs:Group` (rdf:Alt), `crs:SupportsAmount`, `crs:SupportsMonochrome`,
//! `crs:CameraModelRestriction`, `crs:CameraProfile`, `crs:ConvertToGrayscale`,
//! `crs:LookTable` / `crs:RGBTable` (MD5 references) + the matching `crs:Table_<MD5>` values,
//! `crs:RGBTableAmount`, and ordinary develop settings (e.g. `crs:Clarity2012`,
//! `crs:ToneCurvePV2012*`) that the look applies. A sidecar's `<crs:Look>` holds the same
//! inside `crs:Parameters` (without the tables for Adobe's bundled looks; with `crs:Table_*`
//! on the sidecar's top level for looks that are not installed).
//!
//! Parsing reuses the read-only structured API of `xmp::packet` (`Packet::top`,
//! `Packet::look`).

use crate::ipc::types::{ParametricAdjustments, ProfileSettings};
use crate::xmp::crs::{self, CrsSource, CRS_NS};
use crate::xmp::packet::{Packet, ScopeSource};

use super::table::{self, BigTable};

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
    /// `crs:SupportsOutputReferred`: usable on non-RAW (display-referred) sources.
    pub supports_output_referred: bool,
    /// Decoded `LookTable` and/or `RGBTable`.
    pub tables: Vec<BigTable>,
    /// `crs:RGBTableAmount` (scales the RGB table's blend; 1 when absent).
    pub rgb_table_amount: f32,
    /// The look's own develop settings (parsed with `xmp::crs::decode` over the look's
    /// properties, neutral where absent): applied at `amount` beneath the user's settings.
    pub parameters: ParametricAdjustments,
}

fn parse_bool(v: Option<String>) -> bool {
    v.is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1"))
}

fn nonempty(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// Neutral develop settings (nothing applied): the base the look's own settings go on.
pub fn neutral_parameters() -> ParametricAdjustments {
    let mut p = ParametricAdjustments::default();
    p.detail.sharpening.amount = 0.0;
    p.detail.noise_reduction.color = 0.0;
    p.profile = ProfileSettings::none();
    p
}

/// The look's develop settings from its properties (`crs::decode_source`, or just the v9
/// groups when no basic slider is present).
fn parameters(src: &dyn CrsSource) -> Result<ParametricAdjustments, String> {
    let mut adj = match crs::decode_source(src)? {
        Some(mut a) => {
            a.detail = neutral_parameters().detail;
            a
        }
        None => {
            let mut a = neutral_parameters();
            crs::decode_parity(src, &mut a)?;
            a
        }
    };
    adj.profile = ProfileSettings::none();
    adj.lut = None;
    Ok(adj)
}

/// Decodes the tables referenced by `LookTable` / `RGBTable` (MD5s) of `params`, found as
/// `crs:Table_<md5>` in `params` or `holder`. Missing or undecodable tables are skipped.
fn tables(params: &dyn CrsSource, holder: &dyn CrsSource) -> Vec<BigTable> {
    let mut out = Vec::new();
    for key in ["LookTable", "RGBTable"] {
        let Some(md5) = nonempty(params.scalar(CRS_NS, key)) else { continue };
        let attr = format!("Table_{md5}");
        let value = params.scalar(CRS_NS, &attr).or_else(|| holder.scalar(CRS_NS, &attr));
        if let Some(value) = value {
            match table::decode(&value, &md5) {
                Ok(t) => out.push(t),
                Err(e) => eprintln!("look table {md5}: {e}"),
            }
        }
    }
    out
}

fn rgb_amount(src: &dyn CrsSource) -> f32 {
    src.scalar(CRS_NS, "RGBTableAmount")
        .and_then(|v| v.trim().parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(1.0)
}

impl LookProfile {
    /// Parses a look profile file (`Settings/**/*.xmp`). `Ok(None)` if it is not a look
    /// (`crs:PresetType` other than "Look").
    pub fn parse_file(xmp: &str) -> Result<Option<LookProfile>, String> {
        Self::parse_file_opts(xmp, true)
    }

    /// As [`Self::parse_file`]; `with_tables = false` skips table decoding (index scans).
    pub fn parse_file_opts(xmp: &str, with_tables: bool) -> Result<Option<LookProfile>, String> {
        let packet = Packet::parse(xmp).map_err(|e| e.to_string())?;
        let top: ScopeSource = packet.top();
        if top.scalar(CRS_NS, "PresetType").map(|v| v.trim().to_owned()).as_deref() != Some("Look") {
            return Ok(None);
        }
        let uuid = top.scalar(CRS_NS, "UUID").unwrap_or_default().trim().to_ascii_uppercase();
        if uuid.is_empty() {
            return Err("look without crs:UUID".into());
        }
        let parameters = parameters(&top)?;
        let tables = if with_tables { tables(&top, &top) } else { Vec::new() };
        Ok(Some(LookProfile {
            uuid,
            name: top.text(CRS_NS, "Name").unwrap_or_default(),
            group: top.text(CRS_NS, "Group").unwrap_or_default(),
            supports_amount: parse_bool(top.scalar(CRS_NS, "SupportsAmount")),
            monochrome: parse_bool(top.scalar(CRS_NS, "ConvertToGrayscale")),
            camera_profile: nonempty(top.scalar(CRS_NS, "CameraProfile")),
            camera_model_restriction: nonempty(top.scalar(CRS_NS, "CameraModelRestriction")),
            supports_output_referred: parse_bool(top.scalar(CRS_NS, "SupportsOutputReferred")),
            tables,
            rgb_table_amount: rgb_amount(&top),
            parameters,
        }))
    }

    /// The look recorded in a sidecar (`<crs:Look>` + top-level `crs:Table_*`), used when the
    /// look is not installed. `Ok(None)` if the sidecar has no look.
    pub fn from_sidecar(xmp: &str) -> Result<Option<LookProfile>, String> {
        let packet = Packet::parse(xmp).map_err(|e| e.to_string())?;
        let top = packet.top();
        let Some(look) = packet.look() else { return Ok(None) };
        let (parameters, tables, monochrome, camera_profile, rgb) = match &look.parameters {
            Some(p) => (
                parameters(p)?,
                tables(p, &top),
                parse_bool(p.scalar(CRS_NS, "ConvertToGrayscale")),
                nonempty(p.scalar(CRS_NS, "CameraProfile")),
                rgb_amount(p),
            ),
            None => (neutral_parameters(), Vec::new(), false, None, 1.0),
        };
        Ok(Some(LookProfile {
            uuid: look.settings.uuid.clone(),
            name: look.settings.name.clone(),
            group: look.group.clone().unwrap_or_default(),
            supports_amount: look.supports_amount.unwrap_or(false),
            monochrome,
            camera_profile,
            camera_model_restriction: None,
            supports_output_referred: look.supports_output_referred.unwrap_or(false),
            tables,
            rgb_table_amount: rgb,
            parameters,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOOK: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:PresetType="Look" crs:UUID="0CFE8F8AB5F63B2A73CE0B0077D20817" crs:SupportsAmount="False"
   crs:SupportsOutputReferred="False" crs:CameraModelRestriction=""
   crs:ProcessVersion="10.0" crs:Clarity2012="+8" crs:ConvertToGrayscale="True"
   crs:CameraProfile="Adobe Standard" crs:LookTable="0123456789ABCDEF0123456789ABCDEF">
   <crs:Name><rdf:Alt><rdf:li xml:lang="x-default">Adobe Monochrome</rdf:li></rdf:Alt></crs:Name>
   <crs:Group><rdf:Alt><rdf:li xml:lang="x-default">Profiles</rdf:li></rdf:Alt></crs:Group>
   <crs:ToneCurvePV2012><rdf:Seq><rdf:li>0, 0</rdf:li><rdf:li>64, 56</rdf:li><rdf:li>255, 255</rdf:li></rdf:Seq></crs:ToneCurvePV2012>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;

    #[test]
    fn parses_look_file() {
        let l = LookProfile::parse_file(LOOK).unwrap().unwrap();
        assert_eq!(l.uuid, "0CFE8F8AB5F63B2A73CE0B0077D20817");
        assert_eq!(l.name, "Adobe Monochrome");
        assert_eq!(l.group, "Profiles");
        assert!(!l.supports_amount && l.monochrome && !l.supports_output_referred);
        assert_eq!(l.camera_profile.as_deref(), Some("Adobe Standard"));
        assert_eq!(l.camera_model_restriction, None);
        assert_eq!(l.parameters.clarity, 8.0);
        assert!(l.parameters.black_and_white.enabled);
        assert_eq!(l.parameters.tone_curve.point.master, vec![[0.0, 0.0], [64.0, 56.0], [255.0, 255.0]]);
        assert_eq!(l.parameters.detail.sharpening.amount, 0.0);
        assert!(l.tables.is_empty(), "referenced table not present");
        // Not a look.
        let preset = LOOK.replace("crs:PresetType=\"Look\"", "crs:PresetType=\"Normal\"");
        assert!(LookProfile::parse_file(&preset).unwrap().is_none());
        assert!(LookProfile::parse_file("<a><b></a>").is_err());
    }

    #[test]
    fn reads_look_from_sidecar() {
        let sidecar = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:Exposure2012="-0.87" crs:CameraProfile="Adobe Standard">
   <crs:Look>
    <rdf:Description crs:Name="Adobe Color" crs:Amount="1" crs:UUID="B952C231111CD8E0ECCF14B86BAA7077"
     crs:SupportsAmount="false" crs:SupportsMonochrome="false" crs:SupportsOutputReferred="false">
    <crs:Group><rdf:Alt><rdf:li xml:lang="x-default">Profiles</rdf:li></rdf:Alt></crs:Group>
    <crs:Parameters>
     <rdf:Description crs:Version="17.1" crs:ProcessVersion="15.4" crs:ConvertToGrayscale="False"
      crs:CameraProfile="Adobe Standard" crs:LookTable="E1095149FDB39D7A057BAB208837E2E1">
     <crs:ToneCurvePV2012><rdf:Seq><rdf:li>0, 0</rdf:li><rdf:li>22, 16</rdf:li><rdf:li>255, 255</rdf:li></rdf:Seq></crs:ToneCurvePV2012>
     </rdf:Description>
    </crs:Parameters>
    </rdf:Description>
   </crs:Look>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
        let l = LookProfile::from_sidecar(sidecar).unwrap().unwrap();
        assert_eq!(l.uuid, "B952C231111CD8E0ECCF14B86BAA7077");
        assert_eq!(l.name, "Adobe Color");
        assert_eq!(l.group, "Profiles");
        assert!(!l.monochrome);
        assert_eq!(l.parameters.tone_curve.point.master[1], [22.0, 16.0]);
        assert_eq!(l.parameters.exposure, 0.0, "the sidecar's own settings do not leak in");
        assert!(l.tables.is_empty());
        assert!(LookProfile::from_sidecar(LOOK).unwrap().is_none());
    }

    #[test]
    #[ignore = "needs Adobe Camera Raw look profiles installed"]
    fn parses_installed_adobe_color() {
        let path = "/Library/Application Support/Adobe/CameraRaw/Settings/Adobe/Profiles/Adobe Raw/Adobe Color.xmp";
        let l = LookProfile::parse_file(&std::fs::read_to_string(path).unwrap()).unwrap().unwrap();
        assert_eq!(l.uuid, "B952C231111CD8E0ECCF14B86BAA7077");
        assert_eq!(l.name, "Adobe Color");
        assert_eq!(l.tables.len(), 1);
        assert_eq!(l.parameters.tone_curve.point.master.len(), 7);
    }
}
