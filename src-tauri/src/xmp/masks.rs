//! `crs:MaskGroupBasedCorrections` <-> `ParametricAdjustments.masks` (Phase 7c, IPC v10).
//! Owned by rust-engine-dev; the architect fixed the mapping tables and signatures below
//! (the tables are contract data: names, scales, value codes). Works on packet text so it can
//! run as a pass after `packet::merge` without touching `packet` internals.
//!
//! # Lightroom format (from the user's 394 sidecars, Lightroom Classic 8.1 / ACR 17.1)
//! ```text
//! <crs:MaskGroupBasedCorrections><rdf:Seq>
//!  <rdf:li><rdf:Description crs:What="Correction" crs:CorrectionAmount="1"
//!     crs:CorrectionActive="true" crs:CorrectionName="Cool Soft" crs:CorrectionSyncID="<32 hex>"
//!     crs:LocalExposure2012="0" ... crs:LocalClarity2012="-0.195091" crs:LocalTemperature="-0.198124"
//!     crs:LocalTexture="-0.149141" crs:LocalCurveRefineSaturation="100" (+ PV2010 Local* = 0)>
//!   <crs:CorrectionMasks><rdf:Seq>
//!    <rdf:li crs:What="Mask/Image" crs:MaskActive="true" crs:MaskName="Subject 1"
//!       crs:MaskBlendMode="0" crs:MaskInverted="false" crs:MaskSyncID="<32 hex>" crs:MaskValue="1"
//!       crs:MaskVersion="1" crs:MaskSubType="1" crs:ReferencePoint="0.136719 0.363636"
//!       crs:InputDigest="<32 hex>" crs:InputDigestVersion="2" crs:MaskDigest="<32 hex>"
//!       crs:WholeImageArea="0/1,0/1,1920/1,2880/1" crs:Origin="0,296" crs:ModelVersion="251659306"/>
//!   </rdf:Seq></crs:CorrectionMasks>
//!  </rdf:Description></rdf:li>
//! </rdf:Seq></crs:MaskGroupBasedCorrections>
//! ```
//! - Only the **top-level** property is the image's state. Copies inside `<crs:Preset>`
//!   `<crs:Parameters>` (45 in the user's files: which Adaptive preset was applied, with
//!   `crs:ErrorReason`, no digest, `ReferencePoint="0.5 0.5"`) are never read or written.
//! - AI matte bitmap = top-level attribute `crs:Table_<MaskDigest>` (the digest is the
//!   attribute name). Value: Adobe base-85 text (alphabet [`TABLE_ALPHABET`], 5 chars ->
//!   one little-endian u32, least-significant digit first, short last group) -> bytes:
//!   16-byte header (u32 LE `2, 1, 0, 0` on all 50 user mattes) + a little-endian TIFF with
//!   one IFD: 8-bit, 1 sample, photometric 1 (min-is-black), one tile = whole image,
//!   compression 52546 (JPEG XL, DNG 1.7). The tile is a bare JXL codestream (`FF 0A`);
//!   macOS ImageIO decodes it (verified: `sips` -> soft 8-bit subject matte). Unlike look
//!   tables (`profiles::table`) there is no zlib layer.
//! - Placement: `crs:WholeImageArea` = "top,left,bottom,right" as rationals in *matte pixel
//!   space* (e.g. 0,0,1920,2880 for a 7008x4672 sensor: the sensor frame scaled to 2880 on
//!   the long edge); `crs:Origin` = "x,y" of the tile's top-left in that space (verified: y +
//!   height = 1920 exactly on a bottom-touching matte). Matte bounds in the sensor frame =
//!   `(x / W, y / H, w / W, h / H)` with W, H from WholeImageArea. Un-oriented (verified on
//!   an orientation-8 frame: matte is sideways).
//! - Other `crs:Table_*` attributes belong to retouch (`pm_patch`, `pm_patch_mask`,
//!   `pm_patch_variation`, `IngestInfo`; 115 in the user's files) or looks: never touched.
//! - Brush (`Mask/Paint`, seen only inside `crs:RetouchAreas` in the user's files): attributes
//!   `Radius Flow CenterWeight MaskValue`, child `crs:Dabs` `rdf:Seq` of `"d x y"` items (1322)
//!   and one `"r <radius>"` item; li wraps an `rdf:Description`. `Radius` is a fraction of the
//!   sensor width (verified against `crs:pm_target_*` pixel boxes on 12 strokes).
//!
//! # Read ([`read`])
//! Groups/components map 1:1 (ids = SyncIDs). Local scalars via [`LOCAL_SCALARS`] (UI =
//! XMP / scale, clamped to the UI range); missing -> default. `MaskBlendMode` via
//! [`BLEND_MODES`]; AI targets via [`AI_SUBTYPES`] / [`PERSON_PARTS`] (unknown codes ->
//! `AiTarget::Other`); ranges from `crs:CorrectionRangeMask` (Type luminance/colour; depth or
//! unparsable -> `MaskShape::Unsupported`). Brush: consecutive `Mask/Paint` items with the
//! same `MaskName` (or no name/SyncID after a Paint item) form one component; each item is one
//! stroke; an `r` dab item starts a new stroke with that radius; `CenterWeight` -> `feather =
//! 1 - CenterWeight`; `MaskValue` 0 -> erase stroke (provisional, no Lightroom-authored brush
//! mask in the sample set). Every `Mask/Image` with a digest and a matching table yields a
//! [`LightroomMatte`]; the read path stores it in `MaskCache` (origin `lightroom`) and sets
//! `AiMask.digest`. A digest without a table -> digest `None` (recompute).
//! The read path imports masks like other develop settings ("Read from XMP" history entry)
//! and clears `images.masks_pending_import`; images flagged by migration 0010 get their masks
//! imported once (XmpSync catch-up at launch + `read_xmp`), merged into the *current*
//! adjustments (only `masks` replaced), never replacing other Sieve edits.
//!
//! # Write ([`apply`])
//! - Never for images with `masks_pending_import = 1` (would delete unimported Lightroom masks).
//! - If `masks` equals what [`read`] returns for the existing packet -> no change (bytes kept).
//! - Else the top-level element is regenerated from `masks` (empty -> element removed):
//!   items whose SyncID exists in the old packet keep all their unmodelled attributes /
//!   children (e.g. `MaskVersion`, `ModelVersion`, `InputDigest*`, `WholeImageArea`, `Origin`,
//!   PV2010 `Local*`, unknown future properties, `Unsupported` components verbatim); an
//!   unchanged component is re-emitted byte-for-byte. AI components whose target / reference
//!   point / digest changed drop `MaskDigest`, `InputDigest*`, `WholeImageArea`, `Origin`,
//!   `ModelVersion`.
//! - AI components are written with a `MaskDigest` only when it is the Lightroom matte read
//!   from this sidecar. Sieve-computed mattes are **not** written as tables: the component is
//!   written in the form Lightroom uses for presets (`What`, `MaskSubType`,
//!   `MaskSubCategoryID`, `ReferencePoint`, no digest), which Lightroom recomputes with its own
//!   model on open (as it does for Adaptive presets / synced masks). This keeps sidecars small
//!   and never feeds Lightroom a bitmap from another model under its digest scheme.
//! - `crs:Table_<d>` attributes whose `d` was a `MaskDigest` in the old packet and is no
//!   longer referenced are removed; all other tables are kept byte-for-byte.
//! - New groups also write the PV2010 `Local*` properties as 0 and
//!   `LocalCurveRefineSaturation`, like Lightroom; numbers in Lightroom's style (up to 6
//!   decimals, no trailing zeros).

mod jxl;
mod md5;
mod read;
mod table;
mod tree;
mod write;

#[cfg(test)]
mod tests_io;

use crate::develop::masks::AlphaMask;
use crate::ipc::types::{
    AiTarget, AiTargetKind, DevelopWarning, LandscapeCategory, LocalAdjustments, MaskBlendMode, MaskGroup, NormPoint,
    PersonPart,
};

/// Adobe big-table base-85 alphabet (DNG SDK `dng_big_table`), shared with `profiles::table`.
pub const TABLE_ALPHABET: &[u8; 85] =
    b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-:+=^!/*?`'|()[]{}@%$#";

/// TIFF compression code of Lightroom mattes (JPEG XL).
pub const MATTE_COMPRESSION_JXL: u16 = 52546;

/// How much a mapping is backed by evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    /// Confirmed on the user's Lightroom sidecars.
    Verified,
    /// Present in the user's sidecars with plausible values; scale from Lightroom convention.
    Observed,
    /// Lightroom convention, not present in the sample set; re-check with a Lightroom-authored
    /// sample before relying on it for writes.
    Provisional,
}

/// A local slider <-> `crs:` property: `xmp = ui * scale`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalScalar {
    pub property: &'static str,
    /// `LocalAdjustments` field (camelCase wire name).
    pub field: &'static str,
    pub scale: f32,
    pub evidence: Evidence,
}

const fn ls(property: &'static str, field: &'static str, scale: f32, evidence: Evidence) -> LocalScalar {
    LocalScalar { property, field, scale, evidence }
}

/// Scalar local sliders (curves: [`LOCAL_CURVES`]).
pub const LOCAL_SCALARS: &[LocalScalar] = &[
    ls("LocalTemperature", "temperature", 0.01, Evidence::Observed),
    ls("LocalTint", "tint", 0.01, Evidence::Observed),
    ls("LocalExposure2012", "exposure", 0.25, Evidence::Provisional),
    ls("LocalContrast2012", "contrast", 0.01, Evidence::Observed),
    ls("LocalHighlights2012", "highlights", 0.01, Evidence::Observed),
    ls("LocalShadows2012", "shadows", 0.01, Evidence::Observed),
    ls("LocalWhites2012", "whites", 0.01, Evidence::Observed),
    ls("LocalBlacks2012", "blacks", 0.01, Evidence::Observed),
    ls("LocalTexture", "texture", 0.01, Evidence::Observed),
    ls("LocalClarity2012", "clarity", 0.01, Evidence::Observed),
    ls("LocalDehaze", "dehaze", 0.01, Evidence::Observed),
    ls("LocalHue", "hue", 1.0 / 180.0, Evidence::Provisional),
    ls("LocalSaturation", "saturation", 0.01, Evidence::Observed),
    ls("LocalSharpness", "sharpness", 0.01, Evidence::Observed),
    ls("LocalLuminanceNoise", "noise", 0.01, Evidence::Observed),
    ls("LocalMoire", "moire", 0.01, Evidence::Observed),
    ls("LocalDefringe", "defringe", 0.01, Evidence::Observed),
    ls("LocalToningHue", "color.hue", 1.0, Evidence::Observed),
    ls("LocalToningSaturation", "color.saturation", 0.01, Evidence::Observed),
    ls("LocalCurveRefineSaturation", "curveRefineSaturation", 1.0, Evidence::Verified),
];

/// PV2010 local properties Lightroom still writes (as 0) on every Correction; preserved on
/// existing groups, written as 0 on new ones. Never read.
pub const LEGACY_LOCAL_ZEROS: &[&str] =
    &["LocalExposure", "LocalBrightness", "LocalContrast", "LocalClarity", "LocalSaturation"];

/// Per-mask point curves (`rdf:Seq` of `"x, y"`, like `crs:ToneCurvePV2012`), provisional.
pub const LOCAL_CURVES: &[(&str, &str)] =
    &[("MainCurve", "master"), ("RedCurve", "red"), ("GreenCurve", "green"), ("BlueCurve", "blue")];

/// `crs:MaskBlendMode` codes.
pub const BLEND_MODES: &[(i32, MaskBlendMode, Evidence)] = &[
    (0, MaskBlendMode::Add, Evidence::Verified),
    (1, MaskBlendMode::Subtract, Evidence::Provisional),
    (2, MaskBlendMode::Intersect, Evidence::Provisional),
];

/// `crs:MaskSubType` of `Mask/Image`. Background is written as subject (1) with
/// `MaskInverted` toggled and reads back as an inverted subject (provisional).
pub const AI_SUBTYPES: &[(i32, AiTargetKind, Evidence)] = &[
    (1, AiTargetKind::Subject, Evidence::Verified),
    (2, AiTargetKind::Sky, Evidence::Provisional),
    (3, AiTargetKind::People, Evidence::Provisional),
    (4, AiTargetKind::Object, Evidence::Provisional),
    (5, AiTargetKind::Landscape, Evidence::Provisional),
];

/// `crs:MaskSubCategoryID` of people masks (entire person = no part). Provisional.
pub const PERSON_PARTS: &[(i32, PersonPart)] = &[
    (2, PersonPart::FaceSkin),
    (3, PersonPart::BodySkin),
    (4, PersonPart::Eyebrows),
    (5, PersonPart::EyeSclera),
    (6, PersonPart::IrisPupil),
    (7, PersonPart::Lips),
    (8, PersonPart::Teeth),
    (9, PersonPart::Hair),
    (10, PersonPart::Clothes),
];

/// `crs:MaskSubCategoryID` of landscape masks. Provisional.
pub const LANDSCAPE_CATEGORIES: &[(i32, LandscapeCategory)] = &[
    (1, LandscapeCategory::Water),
    (2, LandscapeCategory::Vegetation),
    (3, LandscapeCategory::Mountains),
    (4, LandscapeCategory::Architecture),
    (5, LandscapeCategory::NaturalGround),
    (6, LandscapeCategory::ArtificialGround),
];

/// A Lightroom AI matte found in a sidecar (not decoded yet).
#[derive(Debug, Clone, PartialEq)]
pub struct LightroomMatte {
    /// `crs:MaskDigest` (= the `crs:Table_<digest>` attribute name).
    pub digest: String,
    /// `AiMask::cache_kind()` of the component.
    pub kind: String,
    pub target: AiTarget,
    pub reference_point: Option<NormPoint>,
    /// `crs:ModelVersion`.
    pub model_version: Option<String>,
    /// `crs:InputDigest`.
    pub input_digest: Option<String>,
    /// `crs:WholeImageArea` as (top, left, bottom, right) in matte pixel space.
    pub whole_area: [f64; 4],
    /// `crs:Origin` (x, y) in matte pixel space.
    pub origin: [f64; 2],
    /// Raw `crs:Table_<digest>` value (base-85 text).
    pub table: String,
}

/// Result of [`read`].
#[derive(Debug, Clone, PartialEq)]
pub struct MasksRead {
    /// The image's mask groups (AI digests set only where a table is present).
    pub groups: Vec<MaskGroup>,
    pub mattes: Vec<LightroomMatte>,
    /// `masks_unsupported` (unsupported components + legacy corrections) with counts.
    pub warnings: Vec<DevelopWarning>,
}

/// Reads the top-level `crs:MaskGroupBasedCorrections` (+ legacy `*BasedCorrections` for the
/// warning). `Ok(None)` when the packet has none of them.
pub fn read(packet: &str) -> Result<Option<MasksRead>, String> {
    let src = packet.strip_prefix('\u{feff}').unwrap_or(packet);
    let parsed = read::parse(src)?;
    if !parsed.any {
        return Ok(None);
    }
    Ok(Some(MasksRead { groups: parsed.models(), mattes: parsed.mattes, warnings: parsed.warnings }))
}

/// Base-85 text -> bytes (header included).
pub fn decode_table(value: &str) -> Result<Vec<u8>, String> {
    table::decode_base85(value)
}

/// Decodes a Lightroom matte (table -> TIFF -> JXL tile via ImageIO) and places it in the
/// sensor frame (`AlphaMask.bounds` from `origin` / `whole_area`).
pub fn decode_matte(matte: &LightroomMatte) -> Result<AlphaMask, String> {
    table::decode_matte(matte)
}

/// Rewrites the packet's masks per the write rules (module docs). Returns the packet
/// unchanged when `masks` equals what [`read`] yields.
pub fn apply(packet: &str, masks: &[MaskGroup]) -> Result<String, String> {
    write::apply(packet, masks)
}

/// Whether this Mac can decode Lightroom mattes (JPEG XL needs macOS 14+); `Err` = reason,
/// reported once as a warning by the read path.
pub fn mattes_supported() -> Result<(), String> {
    jxl::supported()
}

/// MD5 (32 upper-case hex) of `data`: Sieve matte digests.
pub fn md5_hex(data: &[u8]) -> String {
    md5::md5_hex(data)
}

/// `LocalAdjustments` scalar by wire name (`LOCAL_SCALARS.field`) with its UI range.
pub(crate) fn local_field_mut<'a>(a: &'a mut LocalAdjustments, field: &str) -> Option<(&'a mut f32, f32, f32)> {
    Some(match field {
        "temperature" => (&mut a.temperature, -100.0, 100.0),
        "tint" => (&mut a.tint, -100.0, 100.0),
        "exposure" => (&mut a.exposure, -4.0, 4.0),
        "contrast" => (&mut a.contrast, -100.0, 100.0),
        "highlights" => (&mut a.highlights, -100.0, 100.0),
        "shadows" => (&mut a.shadows, -100.0, 100.0),
        "whites" => (&mut a.whites, -100.0, 100.0),
        "blacks" => (&mut a.blacks, -100.0, 100.0),
        "texture" => (&mut a.texture, -100.0, 100.0),
        "clarity" => (&mut a.clarity, -100.0, 100.0),
        "dehaze" => (&mut a.dehaze, -100.0, 100.0),
        "hue" => (&mut a.hue, -180.0, 180.0),
        "saturation" => (&mut a.saturation, -100.0, 100.0),
        "sharpness" => (&mut a.sharpness, -100.0, 100.0),
        "noise" => (&mut a.noise, -100.0, 100.0),
        "moire" => (&mut a.moire, -100.0, 100.0),
        "defringe" => (&mut a.defringe, -100.0, 100.0),
        "color.hue" => (&mut a.color.hue, 0.0, 360.0),
        "color.saturation" => (&mut a.color.saturation, 0.0, 100.0),
        "curveRefineSaturation" => (&mut a.curve_refine_saturation, 0.0, 100.0),
        _ => return None,
    })
}

/// Read-only [`local_field_mut`] (0 for unknown names).
pub(crate) fn local_field(a: &LocalAdjustments, field: &str) -> f32 {
    match field {
        "temperature" => a.temperature,
        "tint" => a.tint,
        "exposure" => a.exposure,
        "contrast" => a.contrast,
        "highlights" => a.highlights,
        "shadows" => a.shadows,
        "whites" => a.whites,
        "blacks" => a.blacks,
        "texture" => a.texture,
        "clarity" => a.clarity,
        "dehaze" => a.dehaze,
        "hue" => a.hue,
        "saturation" => a.saturation,
        "sharpness" => a.sharpness,
        "noise" => a.noise,
        "moire" => a.moire,
        "defringe" => a.defringe,
        "color.hue" => a.color.hue,
        "color.saturation" => a.color.saturation,
        "curveRefineSaturation" => a.curve_refine_saturation,
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alphabet_has_85_distinct_ascii_chars_without_xml_specials() {
        let mut seen = std::collections::HashSet::new();
        for &c in TABLE_ALPHABET {
            assert!(c.is_ascii_graphic() && seen.insert(c));
            assert!(!b"<>&\"".contains(&c), "{}", c as char);
        }
        assert_eq!(seen.len(), 85);
    }

    #[test]
    fn tables_are_unique_and_cover_every_field() {
        let mut props = std::collections::HashSet::new();
        let mut fields = std::collections::HashSet::new();
        for s in LOCAL_SCALARS {
            assert!(props.insert(s.property), "{}", s.property);
            assert!(fields.insert(s.field), "{}", s.field);
            assert!(s.scale > 0.0);
        }
        // Every scalar of LocalAdjustments is mapped (curves are in LOCAL_CURVES).
        let wire = serde_json::to_value(crate::ipc::types::LocalAdjustments::default()).unwrap();
        for (k, v) in wire.as_object().unwrap() {
            match k.as_str() {
                "toneCurve" => {}
                "color" => {
                    for sub in v.as_object().unwrap().keys() {
                        assert!(fields.contains(format!("color.{sub}").as_str()), "color.{sub}");
                    }
                }
                _ => assert!(fields.contains(k.as_str()), "{k} is not mapped"),
            }
        }
        for (codes, n) in [
            (BLEND_MODES.iter().map(|b| b.0).collect::<Vec<_>>(), MaskBlendMode::ALL.len()),
            (PERSON_PARTS.iter().map(|b| b.0).collect(), PersonPart::ALL.len()),
            (LANDSCAPE_CATEGORIES.iter().map(|b| b.0).collect(), LandscapeCategory::ALL.len()),
        ] {
            let unique: std::collections::HashSet<_> = codes.iter().collect();
            assert_eq!((codes.len(), unique.len()), (n, n));
        }
        // Background has no subtype of its own (inverted subject).
        assert!(AI_SUBTYPES.iter().all(|s| s.1 != AiTargetKind::Background));
        assert_eq!(AI_SUBTYPES.len(), AiTargetKind::ALL.len() - 1);
    }
}
