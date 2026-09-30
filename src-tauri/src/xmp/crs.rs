//! Develop settings <-> XMP (Phase 5): `crs:` (Adobe Camera Raw Settings) 1:1 with
//! `ParametricAdjustments`, plus Sieve-only fields in the `sieve:` namespace.
//! The mapping table below is contract; the codec bodies belong to rust-engine-dev.
//!
//! Write (catalog -> sidecar), only for images that have an `adjustments` row (never add
//! or remove `crs:` for images not edited in Sieve):
//! - Set every property in [`CRS_FIELDS`] / [`CRS_HSL_BANDS`], plus `crs:ProcessVersion
//!   = "11.0"` and `crs:HasSettings = "True"`. Numbers are written signed with the shortest
//!   decimal that round-trips the f32 (`+15`, `-7.5`, `+0.7`; zero as `0`), so
//!   catalog -> XMP -> catalog is lossless for every valid value.
//! - `whiteBalance = as_shot` -> `crs:WhiteBalance = "As Shot"` and remove `crs:Temperature`
//!   / `crs:Tint`; `custom` -> `"Custom"` + both values.
//! - `lut` -> `sieve:LutId`, `sieve:LutAmount`; `null` -> remove both.
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
//! - `sieve:LutId` / `LutAmount` restore `lut` (kept even if the LUT is not in the
//!   library; the render reports `lutMissing`).
//! - Applied through `develop::history::commit(.., LABEL_READ_XMP)` only when the values
//!   differ from the catalog; such images are listed in `XmpSyncReport.changed`.
//! - A sidecar without owned `crs:` properties leaves the catalog's develop settings as-is.

use crate::develop::wb;
use crate::ipc::types::{is_valid_lut_id, HslChannels, LutRef, ParametricAdjustments, WhiteBalance};

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

/// Develop settings from a packet's properties; `get(ns, name)` returns the raw value.
/// `Ok(None)` when the packet carries no importable develop settings (see read rules).
/// `Err` when an owned property holds something that is not a number.
pub fn decode(get: &dyn Fn(&str, &str) -> Option<String>) -> Result<Option<ParametricAdjustments>, String> {
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
    Ok(Some(adj))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

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
        a.validate().unwrap();
        a
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
