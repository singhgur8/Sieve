//! Lightroom develop presets (`.xmp` with `crs:PresetType="Normal"`, legacy `.lrtemplate`)
//! -> [`PresetSettings`]: the `crs:` settings Sieve applies, kept exactly as found in the file
//! (catalog column `presets.settings_json`), plus the user-facing list of ignored features.
//!
//! Applying (`styles::resolve_preset`) sets exactly the properties in `PresetSettings` onto
//! the image's settings through `xmp::crs::decode_onto` (the same mapping as sidecar reads),
//! so a preset behaves like Lightroom's: every key it carries is set, nothing else changes.
//! Masks in presets (`crs:MaskGroupBasedCorrections`, e.g. Adaptive presets) are appended to
//! the image's masks, as Lightroom adds a preset's masks.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::lua::{self, Value};
use crate::ipc::types::{AdjustmentField, LookSettings, MaskGroup, ParametricAdjustments};
use crate::xmp::crs::{self, CrsSource, CRS_NS};
use crate::xmp::packet::Packet;

/// The settings of an imported preset (JSON in `presets.settings_json`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetSettings {
    /// Scalar `crs:` properties (local name -> value as written in the file).
    #[serde(default)]
    pub scalars: BTreeMap<String, String>,
    /// `rdf:Seq` properties (point curves): local name -> items (`"x, y"`).
    #[serde(default)]
    pub seqs: BTreeMap<String, Vec<String>>,
    /// `<crs:Look>` (creative profile) the preset selects.
    #[serde(default)]
    pub look: Option<LookSettings>,
    /// Mask groups the preset adds (`crs:MaskGroupBasedCorrections`).
    #[serde(default)]
    pub masks: Option<Vec<MaskGroup>>,
}

impl CrsSource for PresetSettings {
    fn scalar(&self, ns: &str, name: &str) -> Option<String> {
        (ns == CRS_NS).then(|| self.scalars.get(name).cloned()).flatten()
    }
    fn seq(&self, ns: &str, name: &str) -> Option<Vec<String>> {
        (ns == CRS_NS).then(|| self.seqs.get(name).cloned()).flatten()
    }
    fn has(&self, ns: &str, name: &str) -> bool {
        ns == CRS_NS
            && (self.scalars.contains_key(name)
                || self.seqs.contains_key(name)
                || (name == crs::LOOK && self.look.is_some()))
    }
    fn look(&self) -> Option<LookSettings> {
        self.look.clone()
    }
}

impl PresetSettings {
    /// `crs:` property names this preset sets (`StylePreset.settingKeys`), sorted.
    pub fn keys(&self) -> Vec<String> {
        let mut out: Vec<String> = self.scalars.keys().chain(self.seqs.keys()).cloned().collect();
        if self.look.is_some() {
            out.push(crs::LOOK.to_owned());
        }
        if self.masks.is_some() {
            out.push(MASKS.to_owned());
        }
        out.sort();
        out.dedup();
        out
    }

    /// Groups the preset touches (`StylePreset.fields`), in [`AdjustmentField::ALL`] order.
    pub fn fields(&self) -> Vec<AdjustmentField> {
        let touched: Vec<AdjustmentField> = self
            .keys()
            .iter()
            .filter_map(|k| if k == crs::CURVE_NAME { Some(AdjustmentField::ToneCurve) } else { field_of(k) })
            .collect();
        AdjustmentField::ALL.iter().copied().filter(|f| touched.contains(f)).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.scalars.is_empty() && self.seqs.is_empty() && self.look.is_none() && self.masks.is_none()
    }

    /// `base` with exactly this preset's settings applied (Lightroom semantics).
    pub fn apply(&self, base: &ParametricAdjustments) -> Result<ParametricAdjustments, String> {
        let mut out = base.clone();
        crs::decode_onto(self, &mut out)?;
        // `ToneCurveName2012 = "Linear"` without points (old presets): a straight curve.
        if self.scalars.get(crs::CURVE_NAME).is_some_and(|v| v.trim() == "Linear")
            && !self.seqs.contains_key("ToneCurvePV2012")
        {
            out.tone_curve.point.master = crate::ipc::types::PointCurves::default().master;
        }
        if let Some(masks) = &self.masks {
            for g in masks {
                if !out.masks.iter().any(|m| m.id == g.id) {
                    out.masks.push(g.clone());
                }
            }
        }
        Ok(out)
    }
}

/// Pseudo key for the masks of a preset.
pub const MASKS: &str = "MaskGroupBasedCorrections";

/// A parsed preset file.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedPreset {
    /// `crs:Name` / template title, else `None` (caller uses the file stem).
    pub name: Option<String>,
    pub supports_amount: bool,
    pub settings: PresetSettings,
    /// User-facing notes about ignored settings.
    pub warnings: Vec<String>,
}

/// What a `crs:` property of a preset is to Sieve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// Applied (sets this group).
    Setting(AdjustmentField),
    /// Describes the preset / bookkeeping; never a setting.
    Meta,
    /// A Lightroom feature Sieve does not render: ignored with this warning when the value
    /// is not neutral.
    Feature(&'static str),
}

const META: &[&str] = &[
    "PresetType",
    "Cluster",
    "UUID",
    "CameraModelRestriction",
    "Copyright",
    "ContactInfo",
    "Version",
    "HasSettings",
    "Name",
    "ShortName",
    "SortName",
    "Group",
    "Description",
    "ProcessVersion",
    "ToneCurveName",
    "ToneCurveName2010",
    "ToneCurveName2012",
    "CameraProfileDigest",
    "AsShotTemperature",
    "AsShotTint",
    "OverrideLookVignette",
    "CompatibleVersion",
    "Stubbed",
    "GrainSeed",
    "CurveRefineSaturation",
    "RequiresRGBTables",
    "AlreadyApplied",
    "RawFileName",
    "Amount",
    "HDREditMode",
    "HDRMaxValue",
    "SDRBrightness",
    "SDRContrast",
    "SDRClarity",
    "SDRHighlights",
    "SDRShadows",
    "SDRWhites",
    "SDRBlend",
    "CropConstrainToWarp",
    "CropConstrainToUnitSquare",
    "CropUnit",
    "CropWidth",
    "CropHeight",
    "Baseline",
];

const LENS: &str = "Lens Corrections not supported, ignored";
const TRANSFORM: &str = "Transform not supported, ignored";
const RETOUCH: &str = "Healing / retouch not supported, ignored";
const LEGACY_LOCAL: &str = "Legacy local adjustments (brush / gradients) not supported, ignored";
const LEGACY_PV: &str = "Process Version 2010 sliders not supported, ignored";
const POINT_COLOR: &str = "Point Color not supported, ignored";
const AUTO: &str = "Auto settings in presets not supported, ignored";
const LENS_BLUR: &str = "Lens Blur not supported, ignored";
const DENOISE: &str = "AI Denoise / Enhance not supported, ignored";
const ISO_ADAPTIVE: &str = "ISO-adaptive settings not supported, ignored";
const WB_AUTO: &str = "Auto white balance applied as As Shot";

fn classify(name: &str) -> Class {
    use AdjustmentField as F;
    if META.contains(&name) || name.starts_with("Supports") || name.starts_with("Table_") || name.starts_with("Enable")
    {
        return Class::Meta;
    }
    let setting = match name {
        "WhiteBalance" | "Temperature" | "Tint" => Some(F::WhiteBalance),
        "Exposure2012" => Some(F::Exposure),
        "Contrast2012" => Some(F::Contrast),
        "Highlights2012" => Some(F::Highlights),
        "Shadows2012" => Some(F::Shadows),
        "Whites2012" => Some(F::Whites),
        "Blacks2012" => Some(F::Blacks),
        "Texture" => Some(F::Texture),
        "Clarity2012" => Some(F::Clarity),
        "Dehaze" => Some(F::Dehaze),
        "Vibrance" => Some(F::Vibrance),
        "Saturation" => Some(F::Saturation),
        "HasCrop" => Some(F::Crop),
        "ConvertToGrayscale" => Some(F::BlackAndWhite),
        crs::VIGNETTE_STYLE => Some(F::Vignette),
        crs::CAMERA_PROFILE | crs::LOOK => Some(F::Profile),
        MASKS => Some(F::Masks),
        _ => None,
    };
    if let Some(f) = setting {
        return Class::Setting(f);
    }
    for ((prefix, _), f) in crs::CRS_HSL_PREFIXES.iter().zip([F::HslHue, F::HslSaturation, F::HslLuminance]) {
        if let Some(band) = name.strip_prefix(prefix) {
            if crs::CRS_HSL_BANDS.iter().any(|(b, _)| *b == band) {
                return Class::Setting(f);
            }
        }
    }
    if crs::CRS_CURVES.iter().any(|(n, _)| *n == name) {
        return Class::Setting(F::ToneCurve);
    }
    if let Some(f) = crs::PARITY_SCALARS.iter().find(|f| f.name == name) {
        let path = f.path;
        let field = if path.starts_with("toneCurve") {
            F::ToneCurve
        } else if path.starts_with("colorGrading") {
            F::ColorGrading
        } else if path.starts_with("calibration") {
            F::Calibration
        } else if path.starts_with("detail.sharpening") {
            F::Sharpening
        } else if path.starts_with("detail.noiseReduction.luminance") {
            F::NoiseReductionLuminance
        } else if path.starts_with("detail.noiseReduction.color") {
            F::NoiseReductionColor
        } else if path.starts_with("effects.vignette") {
            F::Vignette
        } else if path.starts_with("effects.grain") {
            F::Grain
        } else if path.starts_with("blackAndWhite") {
            F::BlackAndWhite
        } else {
            F::Crop
        };
        return Class::Setting(field);
    }
    let lens = [
        "LensProfile",
        "AutoLateralCA",
        "Defringe",
        "VignetteAmount",
        "VignetteMidpoint",
        "LensManual",
        "ChromaticAberration",
        "PerspectiveProfile",
    ];
    if lens.iter().any(|p| name.starts_with(p)) {
        return Class::Feature(LENS);
    }
    if name.starts_with("Perspective") || name.starts_with("Upright") || name == "AutoPerspective" {
        return Class::Feature(TRANSFORM);
    }
    if name.starts_with("Retouch") || name.starts_with("SpotRemoval") || name.starts_with("RedEye") {
        return Class::Feature(RETOUCH);
    }
    if name.ends_with("BasedCorrections") {
        return Class::Feature(LEGACY_LOCAL);
    }
    if matches!(
        name,
        "Exposure"
            | "Contrast"
            | "Brightness"
            | "FillLight"
            | "HighlightRecovery"
            | "Shadows"
            | "Clarity"
            | "ToneCurve"
    ) || name.starts_with("ToneCurveRed")
        || name.starts_with("ToneCurveGreen")
        || name.starts_with("ToneCurveBlue")
    {
        return Class::Feature(LEGACY_PV);
    }
    if name.starts_with("PointColor") {
        return Class::Feature(POINT_COLOR);
    }
    if name.starts_with("Auto") {
        return Class::Feature(AUTO);
    }
    if name.starts_with("ISO") {
        return Class::Feature(ISO_ADAPTIVE);
    }
    if name.starts_with("LensBlur") {
        return Class::Feature(LENS_BLUR);
    }
    if name.starts_with("Enhance") || name.starts_with("Denoise") || name.starts_with("RawDetails") {
        return Class::Feature(DENOISE);
    }
    Class::Feature("")
}

/// The group a key sets (`None` for meta / unsupported keys).
pub fn field_of(name: &str) -> Option<AdjustmentField> {
    match classify(name) {
        Class::Setting(f) => Some(f),
        _ => None,
    }
}

/// A value that changes nothing (`0`, `False`, empty, `100` for scales): unsupported keys
/// with neutral values are ignored silently.
fn is_neutral(name: &str, value: &str) -> bool {
    let v = value.trim();
    if v.is_empty() || v.eq_ignore_ascii_case("false") || v.eq_ignore_ascii_case("none") {
        return true;
    }
    match v.trim_start_matches('+').parse::<f64>() {
        Ok(0.0) => true,
        Ok(x) if x == 100.0 && (name.ends_with("Scale") || name == "PerspectiveScale") => true,
        // Midpoints / hue ranges describe an effect whose amount is its own key.
        Ok(_) if name.ends_with("Midpoint") || name.contains("HueLo") || name.contains("HueHi") => true,
        _ => false,
    }
}

/// Sorts scalar/list properties into settings + warnings.
struct Collector {
    settings: PresetSettings,
    warnings: Vec<String>,
}

impl Collector {
    fn new() -> Self {
        Collector { settings: PresetSettings::default(), warnings: Vec::new() }
    }

    fn warn(&mut self, w: String) {
        if !self.warnings.contains(&w) {
            self.warnings.push(w);
        }
    }

    fn scalar(&mut self, name: &str, value: &str) {
        match classify(name) {
            Class::Setting(_) => {
                if name == "WhiteBalance" && value.trim() == "Auto" {
                    self.warn(WB_AUTO.into());
                }
                self.settings.scalars.insert(name.to_owned(), value.to_owned());
            }
            Class::Meta => {
                if name == "ToneCurveName2012" {
                    // Kept so a "Linear" curve without points resets the curve.
                    self.settings.scalars.insert(name.to_owned(), value.to_owned());
                }
            }
            Class::Feature(w) => {
                if !is_neutral(name, value) {
                    self.warn(if w.is_empty() { format!("{name} not supported, ignored") } else { w.to_owned() });
                }
            }
        }
    }

    fn seq(&mut self, name: &str, items: Vec<String>) {
        match classify(name) {
            Class::Setting(_) if crs::CRS_CURVES.iter().any(|(n, _)| *n == name) => {
                self.settings.seqs.insert(name.to_owned(), items);
            }
            Class::Setting(_) | Class::Meta => {}
            Class::Feature(w) => {
                if !items.is_empty() {
                    self.warn(if w.is_empty() { format!("{name} not supported, ignored") } else { w.to_owned() });
                }
            }
        }
    }

    /// Drops values the mapping cannot read (so a bad key never breaks applying the preset)
    /// and checks the curves.
    fn finish(mut self) -> (PresetSettings, Vec<String>) {
        let names: Vec<String> = self.settings.scalars.keys().cloned().collect();
        for name in names {
            let mut probe = PresetSettings::default();
            probe.scalars.insert(name.clone(), self.settings.scalars[&name].clone());
            if crs::decode_onto(&probe, &mut ParametricAdjustments::default()).is_err() {
                self.settings.scalars.remove(&name);
                self.warn(format!("{name}: unreadable value, ignored"));
            }
        }
        let curves: Vec<String> = self.settings.seqs.keys().cloned().collect();
        for name in curves {
            if crs::parse_curve(&name, &self.settings.seqs[&name]).is_err() {
                self.settings.seqs.remove(&name);
                self.warn(format!("{name}: invalid curve, ignored"));
            }
        }
        // The curve name is only a setting as "Linear" without points (resets the curve).
        let linear_reset = self.settings.scalars.get(crs::CURVE_NAME).is_some_and(|v| v.trim() == "Linear")
            && !self.settings.seqs.contains_key("ToneCurvePV2012");
        if !linear_reset {
            self.settings.scalars.remove(crs::CURVE_NAME);
        }
        (self.settings, self.warnings)
    }
}

/// Parses a develop preset `.xmp`. `Ok(None)` when the file is not a develop preset (a look
/// profile or another kind of XMP); `Err` for unreadable XML.
pub fn parse_xmp(text: &str) -> Result<Option<ParsedPreset>, String> {
    let packet = Packet::parse(text).map_err(|e| e.to_string())?;
    let top = packet.top();
    let kind = top.scalar(CRS_NS, "PresetType").map(|v| v.trim().to_owned());
    let has_settings = top.scalar(CRS_NS, "HasSettings").is_some_and(|v| v.trim().eq_ignore_ascii_case("true"));
    match kind.as_deref() {
        Some("Normal") => {}
        None if has_settings => {}
        _ => return Ok(None),
    }
    let mut c = Collector::new();
    let mut seen: Vec<String> = Vec::new();
    for (name, value) in top.scalars(CRS_NS) {
        if value.trim().is_empty() && text.contains(&format!("<crs:{name}")) {
            // An element with only attributes (`<crs:LensBlur crs:Active="true" .../>`) is a
            // struct: report its feature like a non-empty list.
            c.seq(&name, vec![String::new()]);
        } else {
            c.scalar(&name, &value);
        }
        seen.push(name);
    }
    for name in top.lists(CRS_NS) {
        seen.push(name.clone());
        if name == MASKS {
            continue;
        }
        let items = top.seq(CRS_NS, &name).unwrap_or_default();
        c.seq(&name, items);
    }
    // Struct properties (e.g. `crs:LensBlur`, `crs:ISODependent`): only their warnings.
    if let Ok(all) = crate::xmp::packet::top_level_properties(text) {
        for (ns, name, _) in all {
            if ns == CRS_NS && name != crs::LOOK && name != MASKS && !seen.contains(&name) {
                c.seq(&name, vec![String::new()]);
                seen.push(name);
            }
        }
    }
    if let Some(look) = packet.look() {
        c.settings.look = Some(look.settings);
    }
    if top.has(CRS_NS, MASKS) {
        match crate::xmp::masks::read(text) {
            Ok(Some(read)) if !read.groups.is_empty() => {
                if read.warnings.iter().any(|w| w.code == crate::ipc::types::DevelopWarningCode::MasksUnsupported) {
                    c.warn("Some mask types in this preset are not supported".into());
                }
                c.settings.masks = Some(read.groups);
            }
            Ok(_) => {}
            Err(e) => c.warn(format!("Masks unreadable ({e}), ignored")),
        }
    }
    let name = top.text(CRS_NS, "Name").map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    let supports_amount = top.scalar(CRS_NS, "SupportsAmount").is_some_and(|v| v.trim().eq_ignore_ascii_case("true"));
    let (settings, warnings) = c.finish();
    Ok(Some(ParsedPreset { name, supports_amount, settings, warnings }))
}

/// Why an `.lrtemplate` is not a develop preset (`Err` = unreadable).
#[derive(Debug, Clone, PartialEq)]
pub enum Template {
    Develop(ParsedPreset),
    /// Another template type (`type = "..."`), e.g. "External Editor", "Export".
    Other(String),
}

fn lua_scalar(v: &Value) -> Option<String> {
    match v {
        Value::Str(s) => Some(s.clone()),
        Value::Num(n) => Some(crs::format_num(*n as f32, crs::NumFormat::Plain)),
        Value::Bool(b) => Some(if *b { "True".into() } else { "False".into() }),
        _ => None,
    }
}

/// Lightroom localized titles: `"$$$/Key=Default text"` -> `Default text`.
fn delocalize(s: &str) -> String {
    match s.strip_prefix("$$$/") {
        Some(rest) => rest.split_once('=').map_or(rest, |(_, v)| v).to_owned(),
        None => s.to_owned(),
    }
}

/// Parses a legacy `.lrtemplate` develop preset.
pub fn parse_lrtemplate(text: &str) -> Result<Template, String> {
    let t = lua::parse_template(text)?;
    let value = t.table("value");
    let kind = t.str("type").map(str::to_owned);
    let settings = value.and_then(|v| v.table("settings"));
    match (kind.as_deref(), settings) {
        (Some("Develop") | None, Some(settings)) => {
            let mut c = Collector::new();
            for (name, v) in &settings.fields {
                match v {
                    Value::Table(tab) if crs::CRS_CURVES.iter().any(|(n, _)| n == name) => {
                        // Flat `{ x1, y1, x2, y2, ... }`.
                        let nums: Vec<f64> = tab
                            .items
                            .iter()
                            .filter_map(|i| match i {
                                Value::Num(n) => Some(*n),
                                _ => None,
                            })
                            .collect();
                        let items = nums
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .map(|p| {
                                format!(
                                    "{}, {}",
                                    crs::format_num(p[0] as f32, crs::NumFormat::Plain),
                                    crs::format_num(p[1] as f32, crs::NumFormat::Plain)
                                )
                            })
                            .collect();
                        c.seq(name, items);
                    }
                    Value::Table(tab) if name == crs::LOOK => {
                        let uuid = tab.str("UUID").unwrap_or_default().trim().to_ascii_uppercase();
                        if LookSettings::is_valid_uuid(&uuid) {
                            let amount = match tab.get("Amount") {
                                Some(Value::Num(a)) if a.is_finite() => (*a as f32).clamp(0.0, 2.0),
                                _ => 1.0,
                            };
                            let name = tab.str("Name").unwrap_or_default().trim().to_owned();
                            c.settings.look = Some(LookSettings { name, uuid, amount });
                        }
                    }
                    Value::Table(tab) => {
                        if !tab.items.is_empty() || !tab.fields.is_empty() {
                            c.seq(name, vec![String::new(); tab.items.len().max(1)]);
                        }
                    }
                    other => {
                        if let Some(s) = lua_scalar(other) {
                            c.scalar(name, &s);
                        }
                    }
                }
            }
            let name = t.str("title").map(delocalize).or_else(|| t.str("internalName").map(str::to_owned));
            let name = name.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
            let (settings, warnings) = c.finish();
            Ok(Template::Develop(ParsedPreset { name, supports_amount: false, settings, warnings }))
        }
        (Some(other), _) => Ok(Template::Other(other.to_owned())),
        (None, None) => Err("no `value.settings` table".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::WhiteBalance;

    const PRESET: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
 crs:PresetType="Normal" crs:UUID="0FB70F2CF30749A981E3FE67A48128D2" crs:SupportsAmount="True"
 crs:SupportsColor="True" crs:Version="13.2" crs:ProcessVersion="11.0" crs:Contrast2012="-10"
 crs:Highlights2012="-60" crs:ParametricShadows="+12" crs:HueAdjustmentRed="+10" crs:SplitToningShadowHue="30"
 crs:RedHue="+30" crs:Sharpness="0" crs:LensProfileEnable="1" crs:VignetteMidpoint="50" crs:AutoLateralCA="0"
 crs:ToneCurveName2012="Custom" crs:HasSettings="True">
 <crs:Name><rdf:Alt><rdf:li xml:lang="x-default">CC14</rdf:li></rdf:Alt></crs:Name>
 <crs:Group><rdf:Alt><rdf:li xml:lang="x-default">Film</rdf:li></rdf:Alt></crs:Group>
 <crs:ToneCurvePV2012><rdf:Seq><rdf:li>0, 20</rdf:li><rdf:li>255, 240</rdf:li></rdf:Seq></crs:ToneCurvePV2012>
 <crs:Look><rdf:Description crs:Name="Adobe Monochrome" crs:Amount="1" crs:UUID="0CFE8F8AB5F63B2A73CE0B0077D20817"/></crs:Look>
</rdf:Description></rdf:RDF></x:xmpmeta>"#;

    #[test]
    fn xmp_preset_keys_fields_and_warnings() {
        let p = parse_xmp(PRESET).unwrap().unwrap();
        assert_eq!(p.name.as_deref(), Some("CC14"));
        assert!(p.supports_amount);
        assert_eq!(
            p.settings.keys(),
            [
                "Contrast2012",
                "Highlights2012",
                "HueAdjustmentRed",
                "Look",
                "ParametricShadows",
                "RedHue",
                "Sharpness",
                "SplitToningShadowHue",
                "ToneCurvePV2012"
            ]
        );
        use AdjustmentField as F;
        assert_eq!(
            p.settings.fields(),
            [
                F::Contrast,
                F::Highlights,
                F::HslHue,
                F::ToneCurve,
                F::ColorGrading,
                F::Calibration,
                F::Sharpening,
                F::Profile
            ]
        );
        assert_eq!(p.warnings, [LENS]);

        // Applying sets exactly those keys.
        let base = ParametricAdjustments {
            exposure: 0.7,
            contrast: 40.0,
            vibrance: 12.0,
            white_balance: WhiteBalance::Custom { temperature_k: 4000.0, tint: 5.0 },
            ..Default::default()
        };
        let out = p.settings.apply(&base).unwrap();
        assert_eq!((out.exposure, out.contrast, out.highlights, out.vibrance), (0.7, -10.0, -60.0, 12.0));
        assert_eq!(out.white_balance, base.white_balance);
        assert_eq!(out.hsl.hue.red, 10.0);
        assert_eq!(out.tone_curve.parametric.shadows, 12.0);
        assert_eq!(out.tone_curve.point.master, vec![[0.0, 20.0], [255.0, 240.0]]);
        assert_eq!(out.calibration.red.hue, 30.0);
        assert_eq!(out.detail.sharpening.amount, 0.0);
        assert_eq!(out.color_grading.shadows.hue, 30.0);
        assert_eq!(out.color_grading.blending, 100.0, "legacy split toning");
        assert_eq!(out.profile.look.as_ref().unwrap().name, "Adobe Monochrome");
        assert_eq!(out.profile.camera_profile, base.profile.camera_profile);

        // Not a preset.
        assert!(parse_xmp(&PRESET.replace("\"Normal\"", "\"Look\"")).unwrap().is_none());
    }

    #[test]
    fn white_balance_keys() {
        let mut s = PresetSettings::default();
        s.scalars.insert("Temperature".into(), "6100".into());
        let out = s.apply(&ParametricAdjustments::default()).unwrap();
        assert_eq!(out.white_balance, WhiteBalance::Custom { temperature_k: 6100.0, tint: 0.0 });
        let base = ParametricAdjustments {
            white_balance: WhiteBalance::Custom { temperature_k: 4000.0, tint: 12.0 },
            ..Default::default()
        };
        assert_eq!(s.apply(&base).unwrap().white_balance, WhiteBalance::Custom { temperature_k: 6100.0, tint: 12.0 });
        let mut s = PresetSettings::default();
        s.scalars.insert("WhiteBalance".into(), "As Shot".into());
        assert_eq!(s.apply(&base).unwrap().white_balance, WhiteBalance::AsShot);
        s.scalars.insert("WhiteBalance".into(), "Daylight".into());
        assert_eq!(s.apply(&base).unwrap().white_balance, WhiteBalance::Custom { temperature_k: 5500.0, tint: 10.0 });
    }

    #[test]
    fn lrtemplate_develop_and_other() {
        let t = parse_lrtemplate(
            r#"s = { id = "X", internalName = "Warm", title = "$$$/Presets/Warm=Warm Film", type = "Develop",
  value = { settings = { Exposure2012 = -0.35, ConvertToGrayscale = false, WhiteBalance = "Custom", Temperature = 5900,
    ToneCurvePV2012 = { 0, 0, 64, 58, 255, 255 }, Brightness = 50, LensProfileEnable = 0 }, uuid = "U" }, version = 0 }"#,
        )
        .unwrap();
        let Template::Develop(p) = t else { panic!("{t:?}") };
        assert_eq!(p.name.as_deref(), Some("Warm Film"));
        assert_eq!(p.settings.scalars["Exposure2012"], "-0.35");
        assert_eq!(p.settings.scalars["ConvertToGrayscale"], "False");
        assert_eq!(p.settings.seqs["ToneCurvePV2012"], ["0, 0", "64, 58", "255, 255"]);
        assert_eq!(p.warnings, [LEGACY_PV]);
        let out = p.settings.apply(&ParametricAdjustments::default()).unwrap();
        assert_eq!(out.exposure, -0.35);
        assert_eq!(out.white_balance, WhiteBalance::Custom { temperature_k: 5900.0, tint: 0.0 });

        let ext = parse_lrtemplate(
            r#"s = { title = "Aftershoot", type = "External Editor", value = { editorPath = "/Applications/x" } }"#,
        )
        .unwrap();
        assert_eq!(ext, Template::Other("External Editor".into()));
        assert!(parse_lrtemplate("garbage").is_err());
    }

    #[test]
    fn bad_values_are_dropped_not_fatal() {
        let p = parse_xmp(&PRESET.replace("crs:Contrast2012=\"-10\"", "crs:Contrast2012=\"abc\"")).unwrap().unwrap();
        assert!(!p.settings.scalars.contains_key("Contrast2012"));
        assert!(p.warnings.iter().any(|w| w.contains("Contrast2012")));
    }
}
