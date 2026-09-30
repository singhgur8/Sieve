//! Develop settings <-> XMP (Phase 5): `crs:` (Adobe Camera Raw Settings) 1:1 with
//! `ParametricAdjustments`, plus Sieve-only fields in the `sieve:` namespace.
//! The mapping table below is contract; the codec bodies belong to rust-engine-dev.
//!
//! Write (catalog -> sidecar), only for images that have an `adjustments` row (never add
//! or remove `crs:` for images not edited in Sieve):
//! - Set every property in [`CRS_FIELDS`] / [`CRS_HSL_BANDS`], plus `crs:ProcessVersion
//!   = "11.0"` (never downgrading a newer one already in the sidecar, e.g. Lightroom's
//!   "15.4": `packet::merge` skips the edit) and `crs:HasSettings = "True"`. Numbers are written signed with the shortest
//!   decimal that round-trips the f32 (`+15`, `-7.5`, `+0.7`; zero as `0`), so
//!   catalog -> XMP -> catalog is lossless for every valid value.
//! - `whiteBalance = as_shot` -> `crs:WhiteBalance = "As Shot"` and remove `crs:Temperature`
//!   / `crs:Tint`; `custom` -> `"Custom"` + both values.
//! - `lut` -> `sieve:LutId`, `sieve:LutAmount`; `null` -> remove both.
//! - v9: the parity scalars ([`PARITY_SCALARS`], [`PARITY_BOOLS`]), point curves
//!   ([`encode_curves`], `rdf:Seq`s replaced only when their items change) and the profile
//!   ([`encode_profile`] + [`look_change`], `<crs:Look>` struct) are written too.
//! - Every other `crs:` property (lens, masks, retouch, `crs:Table_*`, ...) and every
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
//! - `sieve:LutId` / `LutAmount` restore `lut` (kept even if the LUT is not in the
//!   library; the render reports `lutMissing`).
//! - Applied through `develop::history::commit(.., LABEL_READ_XMP)` only when the values
//!   differ from the catalog; such images are listed in `XmpSyncReport.changed`.
//! - A sidecar without owned `crs:` properties leaves the catalog's develop settings as-is.

use crate::develop::wb;
use crate::ipc::types::{
    is_valid_lut_id, CropSettings, CurvePoint, DevelopWarning, DevelopWarningCode, HslChannels, ImageFormat,
    LookSettings, LutRef, ParametricAdjustments, ParametricCurve, PointCurves, ProfileSettings, VignetteStyle,
    WhiteBalance,
};

pub const CRS_NS: &str = "http://ns.adobe.com/camera-raw-settings/1.0/";
pub const SIEVE_NS: &str = "http://sieve.app/ns/1.0/";
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

/// `sieve:` properties.
pub const SIEVE_FIELDS: &[(&str, &str)] = &[("LutId", "lut.id"), ("LutAmount", "lut.amount")];

/// One property to set (`Some`) or remove (`None`) in the sidecar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyEdit {
    /// [`CRS_NS`] or [`SIEVE_NS`].
    pub ns: &'static str,
    pub name: String,
    pub value: Option<String>,
}

/// Minimum `crs:ProcessVersion` whose sliders we import (PV2012 = 6.7).
const MIN_PROCESS_VERSION: f32 = 6.7;

/// Signed shortest round-trip decimal: `+15`, `-7.5`, `+0.35`, zero as `0`.
/// (Rust's `Display` for `f32` is the shortest string that parses back to the same value.)
pub fn format_signed(v: f32) -> String {
    if v == 0.0 {
        "0".into()
    } else if v > 0.0 {
        format!("+{v}")
    } else {
        format!("{v}")
    }
}

fn channels(c: &HslChannels) -> [f32; 8] {
    [c.red, c.orange, c.yellow, c.green, c.aqua, c.blue, c.purple, c.magenta]
}

fn channels_mut(c: &mut HslChannels) -> [&mut f32; 8] {
    [&mut c.red, &mut c.orange, &mut c.yellow, &mut c.green, &mut c.aqua, &mut c.blue, &mut c.purple, &mut c.magenta]
}

fn scalar_values(adj: &ParametricAdjustments) -> [(&'static str, f32); 11] {
    [
        ("Exposure2012", adj.exposure),
        ("Contrast2012", adj.contrast),
        ("Highlights2012", adj.highlights),
        ("Shadows2012", adj.shadows),
        ("Whites2012", adj.whites),
        ("Blacks2012", adj.blacks),
        ("Texture", adj.texture),
        ("Clarity2012", adj.clarity),
        ("Dehaze", adj.dehaze),
        ("Vibrance", adj.vibrance),
        ("Saturation", adj.saturation),
    ]
}

/// Property edits that make a sidecar carry `adj` (per the write rules above).
/// `crs:Temperature` is written unsigned, as Lightroom does (`5500`); every other number
/// is signed.
pub fn encode(adj: &ParametricAdjustments) -> Vec<PropertyEdit> {
    let crs = |name: &str, value: Option<String>| PropertyEdit { ns: CRS_NS, name: name.to_owned(), value };
    let mut out = vec![crs("ProcessVersion", Some(PROCESS_VERSION.into()))];
    match adj.white_balance {
        WhiteBalance::AsShot => {
            out.push(crs("WhiteBalance", Some("As Shot".into())));
            out.push(crs("Temperature", None));
            out.push(crs("Tint", None));
        }
        WhiteBalance::Custom { temperature_k, tint } => {
            out.push(crs("WhiteBalance", Some("Custom".into())));
            out.push(crs("Temperature", Some(format!("{temperature_k}"))));
            out.push(crs("Tint", Some(format_signed(tint))));
        }
    }
    for (name, v) in scalar_values(adj) {
        out.push(crs(name, Some(format_signed(v))));
    }
    let groups = [&adj.hsl.hue, &adj.hsl.saturation, &adj.hsl.luminance];
    for ((prefix, _), group) in CRS_HSL_PREFIXES.iter().zip(groups) {
        for ((band, _), v) in CRS_HSL_BANDS.iter().zip(channels(group)) {
            out.push(crs(&format!("{prefix}{band}"), Some(format_signed(v))));
        }
    }
    out.extend(encode_parity(adj));
    out.push(crs("HasSettings", Some("True".into())));
    let (lut_id, lut_amount) = match &adj.lut {
        Some(l) => (Some(l.id.clone()), Some(format!("{}", l.amount))),
        None => (None, None),
    };
    out.push(PropertyEdit { ns: SIEVE_NS, name: "LutId".into(), value: lut_id });
    out.push(PropertyEdit { ns: SIEVE_NS, name: "LutAmount".into(), value: lut_amount });
    out
}

fn parse_num(name: &str, raw: &str) -> Result<f32, String> {
    let t = raw.trim();
    let t = t.strip_prefix('+').unwrap_or(t);
    match t.parse::<f32>() {
        Ok(v) if v.is_finite() => Ok(v),
        _ => Err(format!("crs:{name}: bad number {raw:?}")),
    }
}

/// Develop settings from scalar properties only (`get(ns, name)` returns the raw value);
/// see [`decode_source`].
pub fn decode(get: &dyn Fn(&str, &str) -> Option<String>) -> Result<Option<ParametricAdjustments>, String> {
    decode_source(&ScalarSource(get))
}

/// Develop settings from a packet. `Ok(None)` when the packet carries no importable develop
/// settings (see read rules). `Err` when an owned property holds something that is not a
/// number (or an invalid curve). Missing v9 properties read as RAW defaults (callers for
/// non-RAW images overlay `defaults_for` where it differs: Detail + profile).
pub fn decode_source(src: &dyn CrsSource) -> Result<Option<ParametricAdjustments>, String> {
    let get = |ns: &str, name: &str| src.scalar(ns, name);
    let crs = |name: &str| get(CRS_NS, name).map(|v| v.trim().to_owned());
    if !CRS_FIELDS.iter().any(|(name, _)| crs(name).is_some()) {
        return Ok(None);
    }
    if let Some(pv) = crs("ProcessVersion") {
        match pv.parse::<f32>() {
            Ok(v) if v >= MIN_PROCESS_VERSION => {}
            _ => return Ok(None),
        }
    }
    let num = |name: &str, lo: f32, hi: f32| -> Result<Option<f32>, String> {
        crs(name).filter(|v| !v.is_empty()).map(|v| parse_num(name, &v).map(|x| x.clamp(lo, hi))).transpose()
    };
    let mut adj = ParametricAdjustments::default();

    let temp = num("Temperature", wb::MIN_TEMP, wb::MAX_TEMP)?;
    let tint = num("Tint", wb::MIN_TINT, wb::MAX_TINT)?;
    let preset = |t: f32, n: f32| WhiteBalance::Custom { temperature_k: t, tint: n };
    adj.white_balance = match crs("WhiteBalance").as_deref() {
        Some("As Shot") | Some("Auto") => WhiteBalance::AsShot,
        Some("Daylight") => preset(5500.0, 10.0),
        Some("Cloudy") => preset(6500.0, 10.0),
        Some("Shade") => preset(7500.0, 10.0),
        Some("Tungsten") => preset(2850.0, 0.0),
        Some("Fluorescent") => preset(3800.0, 21.0),
        Some("Flash") => preset(5500.0, 0.0),
        // "Custom", missing or unknown: custom if there are values to use.
        _ => match (temp, tint) {
            (None, None) => WhiteBalance::AsShot,
            (t, n) => preset(t.unwrap_or(5500.0), n.unwrap_or(0.0)),
        },
    };

    let mut slots: [(&str, &mut f32, f32, f32); 11] = [
        ("Exposure2012", &mut adj.exposure, -5.0, 5.0),
        ("Contrast2012", &mut adj.contrast, -100.0, 100.0),
        ("Highlights2012", &mut adj.highlights, -100.0, 100.0),
        ("Shadows2012", &mut adj.shadows, -100.0, 100.0),
        ("Whites2012", &mut adj.whites, -100.0, 100.0),
        ("Blacks2012", &mut adj.blacks, -100.0, 100.0),
        ("Texture", &mut adj.texture, -100.0, 100.0),
        ("Clarity2012", &mut adj.clarity, -100.0, 100.0),
        ("Dehaze", &mut adj.dehaze, -100.0, 100.0),
        ("Vibrance", &mut adj.vibrance, -100.0, 100.0),
        ("Saturation", &mut adj.saturation, -100.0, 100.0),
    ];
    for (name, slot, lo, hi) in slots.iter_mut() {
        if let Some(v) = num(name, *lo, *hi)? {
            **slot = v;
        }
    }
    let hsl = &mut adj.hsl;
    let groups = [&mut hsl.hue, &mut hsl.saturation, &mut hsl.luminance];
    for ((prefix, _), group) in CRS_HSL_PREFIXES.iter().zip(groups) {
        for ((band, _), slot) in CRS_HSL_BANDS.iter().zip(channels_mut(group)) {
            if let Some(v) = num(&format!("{prefix}{band}"), -100.0, 100.0)? {
                *slot = v;
            }
        }
    }

    if let Some(id) = get(SIEVE_NS, "LutId").map(|v| v.trim().to_owned()).filter(|v| is_valid_lut_id(v)) {
        let amount = match get(SIEVE_NS, "LutAmount") {
            Some(raw) => parse_num("LutAmount", &raw).map_err(|e| e.replacen("crs:", "sieve:", 1))?.clamp(0.0, 100.0),
            None => 100.0,
        };
        adj.lut = Some(LutRef { id, amount });
    }
    decode_parity(src, &mut adj)?;
    Ok(Some(adj))
}

// ---------------------------------------------------------------------------
// Phase 7b (IPC v9): Lightroom develop parity mapping.
// ---------------------------------------------------------------------------

/// How a number is written (matching Lightroom's own sidecars, so diffs stay small).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumFormat {
    /// `+20`, `-5`, `0` ([`format_signed`]).
    Signed,
    /// `20`, `-0.32`, `0` (Rust shortest round-trip `Display`).
    Plain,
    /// Signed with at least one decimal: `+1.0`, `+0.8` (`crs:SharpenRadius`).
    SignedDecimal,
}

/// Formats `v` per `fmt`; every form parses back to the same f32.
pub fn format_num(v: f32, fmt: NumFormat) -> String {
    match fmt {
        NumFormat::Signed => format_signed(v),
        NumFormat::Plain => {
            if v == 0.0 {
                "0".into()
            } else {
                format!("{v}")
            }
        }
        NumFormat::SignedDecimal => {
            let s = format_signed(v);
            if s.contains('.') || s == "0" {
                if s == "0" {
                    "0.0".into()
                } else {
                    s
                }
            } else {
                format!("{s}.0")
            }
        }
    }
}

/// One scalar `crs:` property owned since v9: its `ParametricAdjustments` path (camelCase),
/// contract range (decode clamps to it), write format and accessors.
pub struct ScalarField {
    pub name: &'static str,
    pub path: &'static str,
    pub lo: f32,
    pub hi: f32,
    pub format: NumFormat,
    pub get: fn(&ParametricAdjustments) -> f32,
    pub set: fn(&mut ParametricAdjustments, f32),
}

macro_rules! scalar {
    ($name:literal, $path:literal, $lo:expr, $hi:expr, $fmt:ident, |$a:ident| $field:expr) => {
        ScalarField {
            name: $name,
            path: $path,
            lo: $lo,
            hi: $hi,
            format: NumFormat::$fmt,
            get: |$a: &ParametricAdjustments| $field,
            set: |$a: &mut ParametricAdjustments, v: f32| $field = v,
        }
    };
}

/// Every numeric v9 property (written on every develop write, read with clamping; missing ->
/// the format's default). Formats follow the user's Lightroom 15.4 sidecars.
pub const PARITY_SCALARS: &[ScalarField] = &[
    // Tone curve (parametric).
    scalar!("ParametricShadows", "toneCurve.parametric.shadows", -100.0, 100.0, Signed, |a| a
        .tone_curve
        .parametric
        .shadows),
    scalar!("ParametricDarks", "toneCurve.parametric.darks", -100.0, 100.0, Signed, |a| a.tone_curve.parametric.darks),
    scalar!("ParametricLights", "toneCurve.parametric.lights", -100.0, 100.0, Signed, |a| a
        .tone_curve
        .parametric
        .lights),
    scalar!("ParametricHighlights", "toneCurve.parametric.highlights", -100.0, 100.0, Signed, |a| a
        .tone_curve
        .parametric
        .highlights),
    scalar!("ParametricShadowSplit", "toneCurve.parametric.shadowSplit", 0.0, 100.0, Plain, |a| a
        .tone_curve
        .parametric
        .shadow_split),
    scalar!("ParametricMidtoneSplit", "toneCurve.parametric.midtoneSplit", 0.0, 100.0, Plain, |a| a
        .tone_curve
        .parametric
        .midtone_split),
    scalar!("ParametricHighlightSplit", "toneCurve.parametric.highlightSplit", 0.0, 100.0, Plain, |a| a
        .tone_curve
        .parametric
        .highlight_split),
    // Color grading (shadow/highlight wheels are Lightroom's legacy split-toning names).
    scalar!("SplitToningShadowHue", "colorGrading.shadows.hue", 0.0, 360.0, Plain, |a| a.color_grading.shadows.hue),
    scalar!("SplitToningShadowSaturation", "colorGrading.shadows.saturation", 0.0, 100.0, Plain, |a| a
        .color_grading
        .shadows
        .saturation),
    scalar!("ColorGradeShadowLum", "colorGrading.shadows.luminance", -100.0, 100.0, Signed, |a| a
        .color_grading
        .shadows
        .luminance),
    scalar!("ColorGradeMidtoneHue", "colorGrading.midtones.hue", 0.0, 360.0, Plain, |a| a.color_grading.midtones.hue),
    scalar!("ColorGradeMidtoneSat", "colorGrading.midtones.saturation", 0.0, 100.0, Plain, |a| a
        .color_grading
        .midtones
        .saturation),
    scalar!("ColorGradeMidtoneLum", "colorGrading.midtones.luminance", -100.0, 100.0, Signed, |a| a
        .color_grading
        .midtones
        .luminance),
    scalar!("SplitToningHighlightHue", "colorGrading.highlights.hue", 0.0, 360.0, Plain, |a| a
        .color_grading
        .highlights
        .hue),
    scalar!("SplitToningHighlightSaturation", "colorGrading.highlights.saturation", 0.0, 100.0, Plain, |a| a
        .color_grading
        .highlights
        .saturation),
    scalar!("ColorGradeHighlightLum", "colorGrading.highlights.luminance", -100.0, 100.0, Signed, |a| a
        .color_grading
        .highlights
        .luminance),
    scalar!("ColorGradeGlobalHue", "colorGrading.global.hue", 0.0, 360.0, Plain, |a| a.color_grading.global.hue),
    scalar!("ColorGradeGlobalSat", "colorGrading.global.saturation", 0.0, 100.0, Plain, |a| a
        .color_grading
        .global
        .saturation),
    scalar!("ColorGradeGlobalLum", "colorGrading.global.luminance", -100.0, 100.0, Signed, |a| a
        .color_grading
        .global
        .luminance),
    scalar!("ColorGradeBlending", "colorGrading.blending", 0.0, 100.0, Plain, |a| a.color_grading.blending),
    scalar!("SplitToningBalance", "colorGrading.balance", -100.0, 100.0, Signed, |a| a.color_grading.balance),
    // Calibration.
    scalar!("RedHue", "calibration.red.hue", -100.0, 100.0, Signed, |a| a.calibration.red.hue),
    scalar!("RedSaturation", "calibration.red.saturation", -100.0, 100.0, Signed, |a| a.calibration.red.saturation),
    scalar!("GreenHue", "calibration.green.hue", -100.0, 100.0, Signed, |a| a.calibration.green.hue),
    scalar!("GreenSaturation", "calibration.green.saturation", -100.0, 100.0, Signed, |a| a
        .calibration
        .green
        .saturation),
    scalar!("BlueHue", "calibration.blue.hue", -100.0, 100.0, Signed, |a| a.calibration.blue.hue),
    scalar!("BlueSaturation", "calibration.blue.saturation", -100.0, 100.0, Signed, |a| a.calibration.blue.saturation),
    scalar!("ShadowTint", "calibration.shadowTint", -100.0, 100.0, Signed, |a| a.calibration.shadow_tint),
    // Detail.
    scalar!("Sharpness", "detail.sharpening.amount", 0.0, 150.0, Plain, |a| a.detail.sharpening.amount),
    scalar!("SharpenRadius", "detail.sharpening.radius", 0.5, 3.0, SignedDecimal, |a| a.detail.sharpening.radius),
    scalar!("SharpenDetail", "detail.sharpening.detail", 0.0, 100.0, Plain, |a| a.detail.sharpening.detail),
    scalar!("SharpenEdgeMasking", "detail.sharpening.masking", 0.0, 100.0, Plain, |a| a.detail.sharpening.masking),
    scalar!("LuminanceSmoothing", "detail.noiseReduction.luminance", 0.0, 100.0, Plain, |a| a
        .detail
        .noise_reduction
        .luminance),
    scalar!("LuminanceNoiseReductionDetail", "detail.noiseReduction.luminanceDetail", 0.0, 100.0, Plain, |a| a
        .detail
        .noise_reduction
        .luminance_detail),
    scalar!("LuminanceNoiseReductionContrast", "detail.noiseReduction.luminanceContrast", 0.0, 100.0, Plain, |a| a
        .detail
        .noise_reduction
        .luminance_contrast),
    scalar!("ColorNoiseReduction", "detail.noiseReduction.color", 0.0, 100.0, Plain, |a| a
        .detail
        .noise_reduction
        .color),
    scalar!("ColorNoiseReductionDetail", "detail.noiseReduction.colorDetail", 0.0, 100.0, Plain, |a| a
        .detail
        .noise_reduction
        .color_detail),
    scalar!("ColorNoiseReductionSmoothness", "detail.noiseReduction.colorSmoothness", 0.0, 100.0, Plain, |a| a
        .detail
        .noise_reduction
        .color_smoothness),
    // Effects.
    scalar!("PostCropVignetteAmount", "effects.vignette.amount", -100.0, 100.0, Signed, |a| a.effects.vignette.amount),
    scalar!("PostCropVignetteMidpoint", "effects.vignette.midpoint", 0.0, 100.0, Plain, |a| a
        .effects
        .vignette
        .midpoint),
    scalar!("PostCropVignetteRoundness", "effects.vignette.roundness", -100.0, 100.0, Signed, |a| a
        .effects
        .vignette
        .roundness),
    scalar!("PostCropVignetteFeather", "effects.vignette.feather", 0.0, 100.0, Plain, |a| a.effects.vignette.feather),
    scalar!("PostCropVignetteHighlightContrast", "effects.vignette.highlights", 0.0, 100.0, Plain, |a| a
        .effects
        .vignette
        .highlights),
    scalar!("GrainAmount", "effects.grain.amount", 0.0, 100.0, Plain, |a| a.effects.grain.amount),
    scalar!("GrainSize", "effects.grain.size", 0.0, 100.0, Plain, |a| a.effects.grain.size),
    scalar!("GrainFrequency", "effects.grain.roughness", 0.0, 100.0, Plain, |a| a.effects.grain.roughness),
    // Black & White mixer.
    scalar!("GrayMixerRed", "blackAndWhite.mixer.red", -100.0, 100.0, Signed, |a| a.black_and_white.mixer.red),
    scalar!("GrayMixerOrange", "blackAndWhite.mixer.orange", -100.0, 100.0, Signed, |a| a.black_and_white.mixer.orange),
    scalar!("GrayMixerYellow", "blackAndWhite.mixer.yellow", -100.0, 100.0, Signed, |a| a.black_and_white.mixer.yellow),
    scalar!("GrayMixerGreen", "blackAndWhite.mixer.green", -100.0, 100.0, Signed, |a| a.black_and_white.mixer.green),
    scalar!("GrayMixerAqua", "blackAndWhite.mixer.aqua", -100.0, 100.0, Signed, |a| a.black_and_white.mixer.aqua),
    scalar!("GrayMixerBlue", "blackAndWhite.mixer.blue", -100.0, 100.0, Signed, |a| a.black_and_white.mixer.blue),
    scalar!("GrayMixerPurple", "blackAndWhite.mixer.purple", -100.0, 100.0, Signed, |a| a.black_and_white.mixer.purple),
    scalar!("GrayMixerMagenta", "blackAndWhite.mixer.magenta", -100.0, 100.0, Signed, |a| a
        .black_and_white
        .mixer
        .magenta),
    // Crop (fractions of the un-oriented image; angle in degrees).
    scalar!("CropTop", "crop.top", 0.0, 1.0, Plain, |a| a.crop.top),
    scalar!("CropLeft", "crop.left", 0.0, 1.0, Plain, |a| a.crop.left),
    scalar!("CropBottom", "crop.bottom", 0.0, 1.0, Plain, |a| a.crop.bottom),
    scalar!("CropRight", "crop.right", 0.0, 1.0, Plain, |a| a.crop.right),
    scalar!("CropAngle", "crop.angle", -45.0, 45.0, Plain, |a| a.crop.angle),
];

/// Boolean properties (`"True"` / `"False"`; decode also accepts `1`/`0`, case-insensitive).
pub const PARITY_BOOLS: &[(&str, &str)] =
    &[("HasCrop", "crop.enabled"), ("ConvertToGrayscale", "blackAndWhite.enabled")];

/// `crs:PostCropVignetteStyle` `1` / `2` / `3` <-> `effects.vignette.style`.
pub const VIGNETTE_STYLE: &str = "PostCropVignetteStyle";

/// Point curves: `rdf:Seq` of `"x, y"` items (integers as Lightroom writes them; decimals are
/// accepted and written with shortest round-trip formatting).
pub const CRS_CURVES: &[(&str, &str)] = &[
    ("ToneCurvePV2012", "toneCurve.point.master"),
    ("ToneCurvePV2012Red", "toneCurve.point.red"),
    ("ToneCurvePV2012Green", "toneCurve.point.green"),
    ("ToneCurvePV2012Blue", "toneCurve.point.blue"),
];
/// Written with the curves: `"Linear"` when the master curve is the identity, else `"Custom"`
/// (Lightroom's preset names like "Medium Contrast" are not modelled; the points are).
pub const CURVE_NAME: &str = "ToneCurveName2012";

/// `crs:CameraProfile` <-> `profile.cameraProfile` (scalar). The look is the `<crs:Look>`
/// struct (`Name`, `Amount`, `UUID`, `SupportsAmount`, `SupportsMonochrome`,
/// `SupportsOutputReferred`, `Group` (rdf:Alt), `Parameters` (nested description with the
/// look's settings)). Write rule (rust-engine-dev, needs struct support in `packet`): only when
/// the catalog's profile differs from the sidecar's (compare cameraProfile, look uuid, amount):
/// set `crs:CameraProfile`, remove `crs:CameraProfileDigest` (Lightroom recomputes it), and
/// replace `<crs:Look>` with the struct copied from the installed look profile (plus its
/// `crs:Table_<md5>` only if the look is not an Adobe-installed one); `look: null` removes
/// `<crs:Look>`. Otherwise both stay byte-for-byte.
pub const CAMERA_PROFILE: &str = "CameraProfile";

/// Property edit for an `rdf:Seq` property (`items: None` removes it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeqEdit {
    pub ns: &'static str,
    pub name: &'static str,
    pub items: Option<Vec<String>>,
}

/// What `decode` can ask a packet for. Only *top-level* properties of the packet (never the
/// look's nested `crs:Parameters`).
pub trait CrsSource {
    /// Attribute- or element-form scalar value.
    fn scalar(&self, ns: &str, name: &str) -> Option<String>;
    /// `rdf:Seq` / `rdf:Bag` item values.
    fn seq(&self, ns: &str, name: &str) -> Option<Vec<String>>;
    /// Any top-level property with this name (scalar, list or struct).
    fn has(&self, ns: &str, name: &str) -> bool;
    /// The `<crs:Look>` struct as settings (`None` if absent *or not parsed yet*).
    /// rust-engine-dev: implement in `packet` (nested struct reader).
    fn look(&self) -> Option<LookSettings>;
}

/// A source over plain getters (tests, simple callers): no lists, no structs.
pub struct ScalarSource<'a>(pub &'a dyn Fn(&str, &str) -> Option<String>);

impl CrsSource for ScalarSource<'_> {
    fn scalar(&self, ns: &str, name: &str) -> Option<String> {
        (self.0)(ns, name)
    }
    fn seq(&self, _: &str, _: &str) -> Option<Vec<String>> {
        None
    }
    fn has(&self, ns: &str, name: &str) -> bool {
        (self.0)(ns, name).is_some()
    }
    fn look(&self) -> Option<LookSettings> {
        None
    }
}

/// `"x, y"` items for a curve.
pub fn format_curve(curve: &[CurvePoint]) -> Vec<String> {
    curve
        .iter()
        .map(|p| format!("{}, {}", format_num(p[0], NumFormat::Plain), format_num(p[1], NumFormat::Plain)))
        .collect()
}

/// Parses `"x, y"` items; `Err` on malformed items or an invalid curve.
pub fn parse_curve(name: &str, items: &[String]) -> Result<Vec<CurvePoint>, String> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let mut parts = item.split(',').map(str::trim);
        let (Some(x), Some(y), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(format!("crs:{name}: bad curve point {item:?}"));
        };
        out.push([parse_num(name, x)?, parse_num(name, y)?]);
    }
    PointCurves::validate_curve(&format!("crs:{name}"), &out)?;
    Ok(out)
}

fn curves_of(p: &PointCurves) -> [&Vec<CurvePoint>; 4] {
    [&p.master, &p.red, &p.green, &p.blue]
}

fn curves_of_mut(p: &mut PointCurves) -> [&mut Vec<CurvePoint>; 4] {
    [&mut p.master, &mut p.red, &mut p.green, &mut p.blue]
}

/// Scalar edits for the v9 groups (numbers, booleans, vignette style). Part of [`encode`].
pub fn encode_parity(adj: &ParametricAdjustments) -> Vec<PropertyEdit> {
    let crs = |name: &str, value: String| PropertyEdit { ns: CRS_NS, name: name.to_owned(), value: Some(value) };
    let mut out: Vec<PropertyEdit> =
        PARITY_SCALARS.iter().map(|f| crs(f.name, format_num((f.get)(adj), f.format))).collect();
    let b = |v: bool| if v { "True".to_owned() } else { "False".to_owned() };
    out.push(crs("HasCrop", b(adj.crop.enabled)));
    out.push(crs("ConvertToGrayscale", b(adj.black_and_white.enabled)));
    let style = match adj.effects.vignette.style {
        VignetteStyle::HighlightPriority => "1",
        VignetteStyle::ColorPriority => "2",
        VignetteStyle::PaintOverlay => "3",
    };
    out.push(crs(VIGNETTE_STYLE, style.to_owned()));
    out
}

/// Curve edits (four `rdf:Seq`s) + `crs:ToneCurveName2012`. Not part of [`encode`]: the
/// sidecar writer (`xmp::desired`) puts them into `packet::Desired::seqs` / `develop`.
pub fn encode_curves(adj: &ParametricAdjustments) -> (Vec<SeqEdit>, PropertyEdit) {
    let p = &adj.tone_curve.point;
    let seqs = CRS_CURVES
        .iter()
        .zip(curves_of(p))
        .map(|((name, _), curve)| SeqEdit { ns: CRS_NS, name, items: Some(format_curve(curve)) })
        .collect();
    let curve_name = if PointCurves::is_identity(&p.master) { "Linear" } else { "Custom" };
    (seqs, PropertyEdit { ns: CRS_NS, name: CURVE_NAME.to_owned(), value: Some(curve_name.to_owned()) })
}

fn parse_bool(name: &str, raw: &str) -> Result<bool, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(format!("crs:{name}: bad boolean {raw:?}")),
    }
}

/// Fills the v9 groups of `adj` from `src` (missing properties keep `adj`'s values, which
/// are the format's defaults). Part of [`decode_source`].
pub fn decode_parity(src: &dyn CrsSource, adj: &mut ParametricAdjustments) -> Result<(), String> {
    let crs = |name: &str| src.scalar(CRS_NS, name).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
    for f in PARITY_SCALARS {
        if let Some(raw) = crs(f.name) {
            (f.set)(adj, parse_num(f.name, &raw)?.clamp(f.lo, f.hi));
        }
    }
    // Legacy split toning (no Color Grading yet): Lightroom upgrades it with blending 100.
    if crs("ColorGradeBlending").is_none()
        && [
            "SplitToningShadowHue",
            "SplitToningShadowSaturation",
            "SplitToningHighlightHue",
            "SplitToningHighlightSaturation",
        ]
        .iter()
        .any(|n| crs(n).is_some())
    {
        adj.color_grading.blending = 100.0;
    }
    if let Some(raw) = crs("HasCrop") {
        adj.crop.enabled = parse_bool("HasCrop", &raw)?;
    }
    if let Some(raw) = crs("ConvertToGrayscale") {
        adj.black_and_white.enabled = parse_bool("ConvertToGrayscale", &raw)?;
    }
    if let Some(raw) = crs(VIGNETTE_STYLE) {
        adj.effects.vignette.style = match parse_num(VIGNETTE_STYLE, &raw)?.round() as i32 {
            2 => VignetteStyle::ColorPriority,
            3 => VignetteStyle::PaintOverlay,
            _ => VignetteStyle::HighlightPriority,
        };
    }
    // Inconsistent values from other tools: fall back to defaults rather than failing.
    let pc = &mut adj.tone_curve.parametric;
    if !(pc.shadow_split < pc.midtone_split && pc.midtone_split < pc.highlight_split) {
        let d = ParametricCurve::default();
        (pc.shadow_split, pc.midtone_split, pc.highlight_split) = (d.shadow_split, d.midtone_split, d.highlight_split);
    }
    if !(adj.crop.left < adj.crop.right && adj.crop.top < adj.crop.bottom) {
        adj.crop = CropSettings::default();
    }

    let point = &mut adj.tone_curve.point;
    for ((name, _), slot) in CRS_CURVES.iter().zip(curves_of_mut(point)) {
        if let Some(items) = src.seq(CRS_NS, name) {
            *slot = parse_curve(name, &items)?;
        }
    }

    decode_profile(src, adj)
}

/// Adobe's "Adobe Raw" looks (names and UUIDs as installed by Camera Raw / DNG Converter;
/// stable across versions). Before Lightroom 7.3 these were DCP names, so a legacy
/// `crs:CameraProfile="Adobe Vivid"` without `<crs:Look>` means DCP "Adobe Standard" + this look.
pub const ADOBE_RAW_LOOKS: &[(&str, &str)] = &[
    ("Adobe Color", "B952C231111CD8E0ECCF14B86BAA7077"),
    ("Adobe Landscape", "6F9C877E84273F4E8271E6B91BEB36A1"),
    ("Adobe Monochrome", "0CFE8F8AB5F63B2A73CE0B0077D20817"),
    ("Adobe Neutral", "1E8E067A11CD44394A3C36A327BB34D1"),
    ("Adobe Portrait", "D6496412E06A83789C499DF9540AA616"),
    ("Adobe Vivid", "EA1DE074F188405965EF399C72C221D9"),
];

/// `crs:CameraProfile` value Lightroom uses for non-RAW files ("no camera profile").
pub const EMBEDDED_PROFILE: &str = "Embedded";
/// `crs:CameraProfileDigest`: MD5 of the DCP, recomputed by Lightroom when absent.
pub const CAMERA_PROFILE_DIGEST: &str = "CameraProfileDigest";
/// The look struct property.
pub const LOOK: &str = "Look";

fn truncate_name(s: &str) -> String {
    s.trim().chars().take(ProfileSettings::MAX_NAME).collect()
}

/// Profile from a packet (rust-engine-dev, together with the `<crs:Look>` writer; call it
/// from [`decode_parity`] then). Rules:
/// - `crs:CameraProfile` -> `cameraProfile` (truncated to `ProfileSettings::MAX_NAME`);
///   absent -> keep `adj`'s (the format default). `"Embedded"` (non-RAW) -> `null`.
/// - `<crs:Look>` parsed (`CrsSource::look`) -> `look`; no `<crs:Look>` but a
///   `crs:CameraProfile` (pre-2018 sidecars, bare DCP choices like "Camera ST") -> `null`.
///   An unparsable `<crs:Look>` keeps `adj`'s look.
/// - Legacy names that were DCPs before Lightroom 7.3 ("Adobe Color", "Adobe Vivid", ... as
///   `crs:CameraProfile` without a look) map to "Adobe Standard" + the look of that name
///   ([`ADOBE_RAW_LOOKS`]).
pub fn decode_profile(src: &dyn CrsSource, adj: &mut ParametricAdjustments) -> Result<(), String> {
    let camera = src.scalar(CRS_NS, CAMERA_PROFILE).map(|v| truncate_name(&v)).filter(|v| !v.is_empty());
    let has_look = src.has(CRS_NS, LOOK);
    if camera.is_none() && !has_look {
        return Ok(());
    }
    let look = src.look();
    if let Some(name) = &camera {
        let legacy = ADOBE_RAW_LOOKS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name));
        match (legacy, has_look) {
            (Some((n, uuid)), false) => {
                adj.profile.camera_profile = Some(ProfileSettings::ADOBE_STANDARD.into());
                adj.profile.look = Some(LookSettings { name: (*n).into(), uuid: (*uuid).into(), amount: 1.0 });
                return Ok(());
            }
            _ if name.eq_ignore_ascii_case(EMBEDDED_PROFILE) => adj.profile.camera_profile = None,
            _ => adj.profile.camera_profile = Some(name.clone()),
        }
    }
    match look {
        Some(l) => adj.profile.look = Some(l),
        None if has_look => {}
        None => adj.profile.look = None,
    }
    Ok(())
}

/// The profile a packet records, `None` when it has neither `crs:CameraProfile` nor
/// `<crs:Look>` (Lightroom then applies its default).
pub fn current_profile(src: &dyn CrsSource) -> Option<ProfileSettings> {
    if !src.has(CRS_NS, CAMERA_PROFILE) && !src.has(CRS_NS, LOOK) {
        return None;
    }
    let mut adj = ParametricAdjustments::default();
    decode_profile(src, &mut adj).ok()?;
    Some(adj.profile)
}

/// Profile edits for a write (rust-engine-dev): see [`CAMERA_PROFILE`] for the rule. `current`
/// is the sidecar's decoded profile (`None` = no sidecar / nothing recorded).
/// Scalar part only: `crs:CameraProfile` (set, or removed for `null`) and removal of
/// `crs:CameraProfileDigest` when the camera profile changes. The `<crs:Look>` struct is
/// handled by [`look_change`] (applied by `packet::merge`).
pub fn encode_profile(adj: &ParametricAdjustments, current: Option<&ProfileSettings>) -> Vec<PropertyEdit> {
    let want = &adj.profile;
    let crs = |name: &str, value: Option<String>| PropertyEdit { ns: CRS_NS, name: name.to_owned(), value };
    let unchanged = match current {
        Some(c) => c.camera_profile == want.camera_profile,
        // Nothing recorded = Lightroom's default; only write a different choice.
        None => *want == ProfileSettings::default() || (want.camera_profile.is_none() && want.look.is_none()),
    };
    if unchanged {
        return Vec::new();
    }
    vec![crs(CAMERA_PROFILE, want.camera_profile.clone()), crs(CAMERA_PROFILE_DIGEST, None)]
}

/// Replaces RAW defaults that `decode_source` assumed for properties the packet does not
/// carry with `format`'s defaults (non-RAW: sharpening amount, colour NR, profile).
pub fn overlay_format_defaults(src: &dyn CrsSource, adj: &mut ParametricAdjustments, format: ImageFormat) {
    if format.is_raw() {
        return;
    }
    let d = ParametricAdjustments::defaults_for(format);
    if !src.has(CRS_NS, "Sharpness") {
        adj.detail.sharpening.amount = d.detail.sharpening.amount;
    }
    if !src.has(CRS_NS, "ColorNoiseReduction") {
        adj.detail.noise_reduction.color = d.detail.noise_reduction.color;
    }
    if current_profile(src).is_none() {
        adj.profile = d.profile;
    }
}

/// What to do with the sidecar's `<crs:Look>` for a profile write.
#[derive(Debug, Clone, PartialEq)]
pub enum LookChange {
    Keep,
    /// Same look (UUID), new `crs:Amount`.
    Amount(f32),
    /// Replace (or create) the struct for the wanted look.
    Replace,
    Remove,
}

/// `<crs:Look>` edit for writing `want` over a sidecar recording `current` (`has_look`: the
/// sidecar has a `<crs:Look>` element, parsable or not).
pub fn look_change(want: &ProfileSettings, current: Option<&ProfileSettings>, has_look: bool) -> LookChange {
    let Some(cur) = current else {
        return match &want.look {
            _ if *want == ProfileSettings::default() => LookChange::Keep,
            Some(_) => LookChange::Replace,
            None => LookChange::Keep,
        };
    };
    match (&cur.look, &want.look) {
        (_, None) if has_look => LookChange::Remove,
        (_, None) => LookChange::Keep,
        (Some(a), Some(b)) if a.uuid == b.uuid => {
            if a.amount == b.amount {
                LookChange::Keep
            } else {
                LookChange::Amount(b.amount)
            }
        }
        (_, Some(_)) => LookChange::Replace,
    }
}

/// `crs:` features found in a packet that Sieve preserves but does not render, as
/// `RawImageEntry.developWarnings` (stored at every XMP read).
pub fn unsupported_warnings(src: &dyn CrsSource) -> Vec<DevelopWarning> {
    let crs = |name: &str| src.scalar(CRS_NS, name).map(|v| v.trim().to_owned());
    let nonzero =
        |name: &str| crs(name).and_then(|v| v.trim_start_matches('+').parse::<f32>().ok()).is_some_and(|v| v != 0.0);
    let mut out = Vec::new();
    let mut push = |code: DevelopWarningCode, detail: Option<String>| out.push(DevelopWarning { code, detail });

    let mask_groups = src.seq(CRS_NS, "MaskGroupBasedCorrections").map(|v| v.len());
    let legacy_local = ["GradientBasedCorrections", "CircularGradientBasedCorrections", "PaintBasedCorrections"]
        .iter()
        .any(|n| src.has(CRS_NS, n));
    if mask_groups.is_some_and(|n| n > 0) || legacy_local {
        push(DevelopWarningCode::MasksUnsupported, mask_groups.map(|n| n.to_string()));
    }
    if src.has(CRS_NS, "RetouchAreas") || src.has(CRS_NS, "RetouchInfo") {
        push(DevelopWarningCode::RetouchUnsupported, None);
    }
    let lens = [
        "LensProfileEnable",
        "AutoLateralCA",
        "DefringePurpleAmount",
        "DefringeGreenAmount",
        "LensManualDistortionAmount",
        "VignetteAmount",
    ]
    .iter()
    .any(|n| nonzero(n));
    if lens {
        push(DevelopWarningCode::LensCorrectionsUnsupported, None);
    }
    let transform = [
        "PerspectiveUpright",
        "PerspectiveVertical",
        "PerspectiveHorizontal",
        "PerspectiveRotate",
        "PerspectiveAspect",
        "PerspectiveX",
        "PerspectiveY",
    ]
    .iter()
    .any(|n| nonzero(n))
        || crs("PerspectiveScale").and_then(|v| v.parse::<f32>().ok()).is_some_and(|v| v != 100.0);
    if transform {
        push(DevelopWarningCode::TransformUnsupported, None);
    }
    let legacy_pv = crs("ProcessVersion").and_then(|v| v.parse::<f32>().ok()).is_some_and(|v| v < MIN_PROCESS_VERSION);
    if legacy_pv && (src.has(CRS_NS, "Exposure") || src.has(CRS_NS, "Exposure2012")) {
        push(DevelopWarningCode::LegacyProcessVersion, crs("ProcessVersion"));
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::ipc::types::ColorWheel;

    /// Applies edits to a property map (what a sidecar would hold).
    fn apply(map: &mut HashMap<(String, String), String>, edits: &[PropertyEdit]) {
        for e in edits {
            let key = (e.ns.to_owned(), e.name.clone());
            match &e.value {
                Some(v) => {
                    map.insert(key, v.clone());
                }
                None => {
                    map.remove(&key);
                }
            }
        }
    }

    fn decode_map(map: &HashMap<(String, String), String>) -> Result<Option<ParametricAdjustments>, String> {
        decode(&|ns: &str, name: &str| map.get(&(ns.to_owned(), name.to_owned())).cloned())
    }

    /// xorshift64*.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        /// Uniform in lo..=hi, sometimes an exact integer / zero / bound.
        fn val(&mut self, lo: f32, hi: f32) -> f32 {
            let u = (self.next() >> 40) as f32 / (1u64 << 24) as f32;
            match self.next() % 6 {
                0 => 0.0,
                1 => (lo + (hi - lo) * u).round(),
                2 => {
                    if self.next().is_multiple_of(2) {
                        lo
                    } else {
                        hi
                    }
                }
                _ => lo + (hi - lo) * u,
            }
        }
    }

    fn random_adj(rng: &mut Rng) -> ParametricAdjustments {
        let mut a = ParametricAdjustments {
            exposure: rng.val(-5.0, 5.0),
            white_balance: if rng.next().is_multiple_of(2) {
                WhiteBalance::AsShot
            } else {
                WhiteBalance::Custom {
                    temperature_k: rng.val(2000.0, 50000.0).max(2000.0),
                    tint: rng.val(-150.0, 150.0),
                }
            },
            ..Default::default()
        };
        for s in [
            &mut a.contrast,
            &mut a.highlights,
            &mut a.shadows,
            &mut a.whites,
            &mut a.blacks,
            &mut a.texture,
            &mut a.clarity,
            &mut a.dehaze,
            &mut a.vibrance,
            &mut a.saturation,
        ] {
            *s = rng.val(-100.0, 100.0);
        }
        for group in [&mut a.hsl.hue, &mut a.hsl.saturation, &mut a.hsl.luminance] {
            for s in channels_mut(group) {
                *s = rng.val(-100.0, 100.0);
            }
        }
        if rng.next().is_multiple_of(2) {
            a.lut = Some(LutRef { id: format!("lut-{:08x}", rng.next() as u32), amount: rng.val(0.0, 100.0) });
        }
        // v9 scalars (curves / profile need Seq / struct access: covered by packet tests).
        for f in PARITY_SCALARS {
            (f.set)(&mut a, rng.val(f.lo, f.hi).clamp(f.lo, f.hi));
        }
        let pc = &mut a.tone_curve.parametric;
        let mut splits = [pc.shadow_split, pc.midtone_split, pc.highlight_split];
        splits.sort_by(f32::total_cmp);
        if splits[0] < splits[1] && splits[1] < splits[2] {
            (pc.shadow_split, pc.midtone_split, pc.highlight_split) = (splits[0], splits[1], splits[2]);
        } else {
            *pc = ParametricCurve {
                shadows: pc.shadows,
                darks: pc.darks,
                lights: pc.lights,
                highlights: pc.highlights,
                ..Default::default()
            };
        }
        let c = &mut a.crop;
        (c.left, c.right) = (c.left.min(c.right), c.left.max(c.right));
        (c.top, c.bottom) = (c.top.min(c.bottom), c.top.max(c.bottom));
        if !(c.left < c.right && c.top < c.bottom) {
            *c = CropSettings { angle: c.angle, ..Default::default() };
        }
        c.enabled = rng.next().is_multiple_of(2);
        a.black_and_white.enabled = rng.next().is_multiple_of(2);
        a.effects.vignette.style = VignetteStyle::ALL[(rng.next() % 3) as usize];
        a.validate().unwrap();
        a
    }

    #[test]
    fn parity_formats_match_lightroom() {
        assert_eq!(format_num(1.0, NumFormat::SignedDecimal), "+1.0");
        assert_eq!(format_num(0.8, NumFormat::SignedDecimal), "+0.8");
        assert_eq!(format_num(20.0, NumFormat::Plain), "20");
        assert_eq!(format_num(-0.322254, NumFormat::Plain), "-0.322254");
        assert_eq!(format_num(-5.0, NumFormat::Signed), "-5");
        let mut adj = ParametricAdjustments::default();
        adj.tone_curve.parametric.lights = 20.0;
        adj.color_grading.midtones.hue = 185.0;
        let edits = encode(&adj);
        let get = |n: &str| edits.iter().find(|e| e.name == n).and_then(|e| e.value.clone());
        assert_eq!(get("ParametricLights").as_deref(), Some("+20"));
        assert_eq!(get("ParametricShadowSplit").as_deref(), Some("25"));
        assert_eq!(get("ColorGradeMidtoneHue").as_deref(), Some("185"));
        assert_eq!(get("ColorGradeBlending").as_deref(), Some("50"));
        assert_eq!(get("Sharpness").as_deref(), Some("40"));
        assert_eq!(get("SharpenRadius").as_deref(), Some("+1.0"));
        assert_eq!(get("HasCrop").as_deref(), Some("False"));
        assert_eq!(get("PostCropVignetteStyle").as_deref(), Some("1"));
        // Every table entry is written exactly once, names are unique.
        let mut names: Vec<&str> = PARITY_SCALARS.iter().map(|f| f.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), PARITY_SCALARS.len());
        for f in PARITY_SCALARS {
            assert_eq!(edits.iter().filter(|e| e.name == f.name).count(), 1, "{}", f.name);
        }
    }

    #[test]
    fn parity_read_rules_and_curves() {
        let from = |pairs: &[(&str, &str)]| -> ParametricAdjustments {
            let map: HashMap<(String, String), String> =
                pairs.iter().map(|(k, v)| ((CRS_NS.to_owned(), (*k).to_owned()), (*v).to_owned())).collect();
            decode_map(&map).unwrap().unwrap()
        };
        // Values from the user's Lightroom 15.4 sidecars.
        let a = from(&[
            ("Exposure2012", "-0.87"),
            ("ParametricDarks", "-15"),
            ("ParametricShadowSplit", "15"),
            ("ParametricMidtoneSplit", "35"),
            ("SplitToningShadowHue", "30"),
            ("SplitToningShadowSaturation", "2"),
            ("ColorGradeMidtoneHue", "185"),
            ("ColorGradeBlending", "100"),
            ("RedSaturation", "+20"),
            ("BlueHue", "-10"),
            ("Sharpness", "48"),
            ("SharpenRadius", "+1.0"),
            ("LuminanceSmoothing", "24"),
            ("HasCrop", "True"),
            ("CropLeft", "0.1"),
            ("CropRight", "0.9"),
            ("CropAngle", "-1.865361"),
            ("ConvertToGrayscale", "False"),
        ]);
        assert_eq!((a.tone_curve.parametric.darks, a.tone_curve.parametric.shadow_split), (-15.0, 15.0));
        assert_eq!(a.color_grading.shadows, ColorWheel { hue: 30.0, saturation: 2.0, luminance: 0.0 });
        assert_eq!((a.color_grading.midtones.hue, a.color_grading.blending), (185.0, 100.0));
        assert_eq!((a.calibration.red.saturation, a.calibration.blue.hue), (20.0, -10.0));
        assert_eq!((a.detail.sharpening.amount, a.detail.noise_reduction.luminance), (48.0, 24.0));
        assert!(a.crop.enabled && a.crop.angle == -1.865361 && a.crop.right == 0.9);
        assert!(a.validate().is_ok());
        // Legacy split toning (no ColorGradeBlending): blending 100.
        assert_eq!(from(&[("Exposure2012", "0"), ("SplitToningHighlightHue", "40")]).color_grading.blending, 100.0);
        assert_eq!(from(&[("Exposure2012", "0")]).color_grading.blending, 50.0);
        // Unordered splits / inverted crop fall back to defaults.
        let b =
            from(&[("Exposure2012", "0"), ("ParametricShadowSplit", "80"), ("CropLeft", "0.9"), ("CropRight", "0.1")]);
        assert_eq!(b.tone_curve.parametric, ParametricCurve::default());
        assert_eq!(b.crop, CropSettings::default());
        assert!(decode_map(
            &[
                ((CRS_NS.to_owned(), "Exposure2012".to_owned()), "0".to_owned()),
                ((CRS_NS.to_owned(), "HasCrop".to_owned()), "maybe".to_owned())
            ]
            .into_iter()
            .collect()
        )
        .is_err());

        // Curves.
        let items: Vec<String> = ["0, 14", "44, 46", "106, 110", "255, 252"].iter().map(|s| s.to_string()).collect();
        let c = parse_curve("ToneCurvePV2012", &items).unwrap();
        assert_eq!(c, vec![[0.0, 14.0], [44.0, 46.0], [106.0, 110.0], [255.0, 252.0]]);
        assert_eq!(format_curve(&c), items);
        assert!(parse_curve("ToneCurvePV2012", &["0, 0".to_owned()]).is_err());
        assert!(parse_curve("ToneCurvePV2012", &["0, 0".to_owned(), "x, 1".to_owned()]).is_err());
        let mut adj = ParametricAdjustments::default();
        adj.tone_curve.point.red = c.clone();
        let (seqs, name) = encode_curves(&adj);
        assert_eq!(seqs.len(), 4);
        assert_eq!(seqs[1].items.as_deref(), Some(items.as_slice()));
        assert_eq!(name.value.as_deref(), Some("Linear"), "master is the identity");
    }

    /// A source with lists (what `packet` provides).
    struct MapSource {
        scalars: HashMap<String, String>,
        seqs: HashMap<String, Vec<String>>,
    }

    impl CrsSource for MapSource {
        fn scalar(&self, ns: &str, name: &str) -> Option<String> {
            (ns == CRS_NS).then(|| self.scalars.get(name).cloned()).flatten()
        }
        fn seq(&self, ns: &str, name: &str) -> Option<Vec<String>> {
            (ns == CRS_NS).then(|| self.seqs.get(name).cloned()).flatten()
        }
        fn has(&self, ns: &str, name: &str) -> bool {
            self.scalar(ns, name).is_some() || self.seq(ns, name).is_some()
        }
        fn look(&self) -> Option<LookSettings> {
            None
        }
    }

    #[test]
    fn curves_round_trip_and_unsupported_warnings() {
        let mut adj = ParametricAdjustments::default();
        adj.tone_curve.point.master = vec![[0.0, 14.0], [44.0, 46.0], [255.0, 252.0]];
        adj.tone_curve.point.blue = vec![[0.0, 0.0], [28.0, 18.0], [255.0, 255.0]];
        let mut src = MapSource { scalars: HashMap::new(), seqs: HashMap::new() };
        for e in encode(&adj) {
            if e.ns == CRS_NS {
                if let Some(v) = e.value {
                    src.scalars.insert(e.name, v);
                }
            }
        }
        let (seqs, _) = encode_curves(&adj);
        for s in seqs {
            src.seqs.insert(s.name.to_owned(), s.items.unwrap());
        }
        assert_eq!(decode_source(&src).unwrap().unwrap(), adj);
        assert!(unsupported_warnings(&src).is_empty(), "neutral lens/transform values are not warnings");

        src.seqs.insert("MaskGroupBasedCorrections".into(), vec![String::new(); 3]);
        src.seqs.insert("RetouchAreas".into(), vec![String::new()]);
        src.scalars.insert("PerspectiveUpright".into(), "1".into());
        src.scalars.insert("PerspectiveScale".into(), "100".into());
        src.scalars.insert("LensProfileEnable".into(), "0".into());
        let codes: Vec<(DevelopWarningCode, Option<String>)> =
            unsupported_warnings(&src).into_iter().map(|w| (w.code, w.detail)).collect();
        assert_eq!(
            codes,
            [
                (DevelopWarningCode::MasksUnsupported, Some("3".into())),
                (DevelopWarningCode::RetouchUnsupported, None),
                (DevelopWarningCode::TransformUnsupported, None),
            ]
        );
    }

    #[test]
    fn round_trip_is_lossless() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut map = HashMap::new();
        for i in 0..2000 {
            let adj = random_adj(&mut rng);
            apply(&mut map, &encode(&adj));
            let back = decode_map(&map).unwrap().unwrap();
            assert_eq!(back, adj, "iteration {i}");
        }
        // Neutral round trips too (a reset must reach the sidecar).
        apply(&mut map, &encode(&ParametricAdjustments::default()));
        assert_eq!(decode_map(&map).unwrap().unwrap(), ParametricAdjustments::default());
        assert!(!map.contains_key(&(CRS_NS.to_owned(), "Temperature".to_owned())));
        assert!(!map.contains_key(&(SIEVE_NS.to_owned(), "LutId".to_owned())));
    }

    #[test]
    fn lightroom_formatting() {
        assert_eq!(format_signed(0.35), "+0.35");
        assert_eq!(format_signed(-0.0), "0");
        assert_eq!(format_signed(15.0), "+15");
        assert_eq!(format_signed(-7.5), "-7.5");
        let adj = ParametricAdjustments {
            exposure: 0.35,
            contrast: -12.0,
            white_balance: WhiteBalance::Custom { temperature_k: 5500.0, tint: 10.0 },
            ..Default::default()
        };
        let edits = encode(&adj);
        let get = |n: &str| edits.iter().find(|e| e.name == n).and_then(|e| e.value.clone());
        assert_eq!(get("Exposure2012").as_deref(), Some("+0.35"));
        assert_eq!(get("Contrast2012").as_deref(), Some("-12"));
        assert_eq!(get("Temperature").as_deref(), Some("5500"));
        assert_eq!(get("Tint").as_deref(), Some("+10"));
        assert_eq!(get("WhiteBalance").as_deref(), Some("Custom"));
        assert_eq!(get("ProcessVersion").as_deref(), Some("11.0"));
        assert_eq!(get("HasSettings").as_deref(), Some("True"));
        assert_eq!(get("HueAdjustmentMagenta").as_deref(), Some("0"));
        // Every owned property is covered.
        for (name, _) in CRS_FIELDS {
            assert!(edits.iter().any(|e| e.name == *name), "{name}");
        }
        assert_eq!(edits.iter().filter(|e| e.name.contains("Adjustment")).count(), 24);
    }

    #[test]
    fn read_rules() {
        let from = |pairs: &[(&str, &str)]| -> Result<Option<ParametricAdjustments>, String> {
            let map: HashMap<(String, String), String> =
                pairs.iter().map(|(k, v)| ((CRS_NS.to_owned(), (*k).to_owned()), (*v).to_owned())).collect();
            decode_map(&map)
        };
        // No owned crs: nothing to import (other crs: like sharpening do not count).
        assert_eq!(from(&[("Sharpness", "40"), ("ProcessVersion", "11.0")]).unwrap(), None);
        // PV2010 not imported.
        assert_eq!(from(&[("ProcessVersion", "5.7"), ("Exposure", "+1.0"), ("Contrast2012", "10")]).unwrap(), None);
        // Lightroom sidecar: missing values neutral, clamping, presets.
        let a = from(&[
            ("ProcessVersion", "15.4"),
            ("WhiteBalance", "Daylight"),
            ("Temperature", "5500"),
            ("Tint", "+10"),
            ("Exposure2012", "+6.00"),
            ("Shadows2012", "+35"),
            ("HueAdjustmentOrange", "-4"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(a.white_balance, WhiteBalance::Custom { temperature_k: 5500.0, tint: 10.0 });
        assert_eq!((a.exposure, a.shadows, a.hsl.hue.orange, a.contrast), (5.0, 35.0, -4.0, 0.0));
        for (wb, t, n) in [("Tungsten", 2850.0, 0.0), ("Fluorescent", 3800.0, 21.0), ("Shade", 7500.0, 10.0)] {
            let a = from(&[("WhiteBalance", wb)]).unwrap().unwrap();
            assert_eq!(a.white_balance, WhiteBalance::Custom { temperature_k: t, tint: n });
        }
        assert_eq!(from(&[("WhiteBalance", "Auto")]).unwrap().unwrap().white_balance, WhiteBalance::AsShot);
        let a = from(&[("WhiteBalance", "Custom"), ("Temperature", "90000"), ("Tint", "-3")]).unwrap().unwrap();
        assert_eq!(a.white_balance, WhiteBalance::Custom { temperature_k: 50000.0, tint: -3.0 });
        assert!(from(&[("Contrast2012", "lots")]).is_err());
        // Invalid LUT ids are dropped.
        let map: HashMap<(String, String), String> = [
            ((CRS_NS.to_owned(), "Exposure2012".to_owned()), "0".to_owned()),
            ((SIEVE_NS.to_owned(), "LutId".to_owned()), "../x".to_owned()),
        ]
        .into_iter()
        .collect();
        assert_eq!(decode_map(&map).unwrap().unwrap().lut, None);
    }
}
