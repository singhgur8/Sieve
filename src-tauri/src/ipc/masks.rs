//! Local adjustments / masking contract (Phase 7c, IPC v10). Re-exported from
//! [`crate::ipc::types`]; same conventions (camelCase fields, snake_case enum values).
//!
//! The model mirrors Lightroom's `crs:MaskGroupBasedCorrections` so the XMP mapping is 1:1
//! (`xmp/masks.rs`, table in `docs/architecture.md` "Masks"):
//! - a [`MaskGroup`] is one entry of the Masks panel ("Mask 1") = one `crs:What="Correction"`
//!   item: a local adjustment set ([`LocalAdjustments`]) applied through a mask built from
//!   [`MaskComponent`]s (`crs:CorrectionMasks`), combined in order with add / subtract /
//!   intersect;
//! - every geometric value is in Lightroom's **sensor frame**: normalized 0..=1 of the
//!   *un-oriented, uncropped* image (x = column / width, y = row / height, origin top-left of
//!   the stored sensor image; EXIF orientation *not* applied, crop *not* applied). Verified on
//!   the user's sidecars: AI mattes of orientation-8 frames are stored sideways, and brush
//!   dabs map to `crs:pm_whole_image_*` pixel boxes. Use [`orient_point`] /
//!   [`unorient_point`] (TS `orientPoint` / `unorientPoint`) to convert to/from the displayed
//!   (oriented) frame; crop mapping is on top of that (`CropSettings` is in the same frame).
//! - isotropic lengths (brush radius) are fractions of the sensor-frame **width** (verified:
//!   `crs:Radius` x width reproduces Lightroom's patch boxes on 12 strokes, 3 cameras).
//!
//! Masks live in `ParametricAdjustments.masks` (field mask `masks`), so history, undo,
//! presets, copy/paste and sync cover them like any other group.

use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

use super::types::{string_enum, NormPoint, NormRect, PointCurves};

// ---------------------------------------------------------------------------
// Limits
// ---------------------------------------------------------------------------

/// Upper bounds enforced by [`validate_masks`] (TS `MASK_LIMITS`).
pub struct MaskLimits;

impl MaskLimits {
    /// Mask groups per image (Lightroom's limit is 100).
    pub const MAX_GROUPS: usize = 100;
    /// Components per group.
    pub const MAX_COMPONENTS: usize = 64;
    /// Brush dabs per image, all strokes of all groups (bounds the per-frame IPC payload:
    /// ~40 bytes of JSON per dab).
    pub const MAX_DABS: usize = 200_000;
    /// Colour-range samples per component (Lightroom allows 5).
    pub const MAX_COLOR_SAMPLES: usize = 5;
    /// Group / component names, in chars.
    pub const MAX_NAME: usize = 64;
}

// ---------------------------------------------------------------------------
// Local adjustment set
// ---------------------------------------------------------------------------

/// Colour tint of a local adjustment (Lightroom's "Color" swatch):
/// `crs:LocalToningHue` (degrees, plain) + `crs:LocalToningSaturation` (0..=1 in XMP).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LocalColor {
    /// 0..=360 degrees.
    #[specta(type = Number)]
    pub hue: f32,
    /// 0..=100 (0 = no tint).
    #[specta(type = Number)]
    pub saturation: f32,
}

/// Per-mask adjustment set: Lightroom's local sliders in **UI units** (what the panel shows).
/// Values are *offsets added to the global settings* where the mask is 1 (Lightroom's model),
/// scaled by the mask weight and `MaskGroup.amount`. All default to 0 (no effect) except
/// `curveRefineSaturation` (100) and `toneCurve` (identity).
///
/// XMP (`crs:` on the Correction item; scale = XMP / UI, see `xmp/masks.rs` `LOCAL_SCALARS`):
/// | field | property | range (UI) | XMP scale |
/// |---|---|---|---|
/// | temperature | LocalTemperature | -100..=100 | 1/100 |
/// | tint | LocalTint | -100..=100 | 1/100 |
/// | exposure | LocalExposure2012 | -4..=4 EV | 1/4 (provisional) |
/// | contrast, highlights, shadows, whites, blacks | Local{Contrast,Highlights,Shadows,Whites,Blacks}2012 | -100..=100 | 1/100 |
/// | texture, dehaze | LocalTexture, LocalDehaze | -100..=100 | 1/100 |
/// | clarity | LocalClarity2012 | -100..=100 | 1/100 |
/// | hue | LocalHue | -180..=180 deg | 1/180 (provisional) |
/// | saturation | LocalSaturation | -100..=100 | 1/100 |
/// | sharpness | LocalSharpness | -100..=100 | 1/100 |
/// | noise | LocalLuminanceNoise | -100..=100 | 1/100 |
/// | moire | LocalMoire | -100..=100 | 1/100 |
/// | defringe | LocalDefringe | -100..=100 | 1/100 |
/// | color | LocalToningHue / LocalToningSaturation | 0..=360 / 0..=100 | 1 / 1/100 |
/// | curveRefineSaturation | LocalCurveRefineSaturation | 0..=100 | 1 |
/// | toneCurve | MainCurve / RedCurve / GreenCurve / BlueCurve (`rdf:Seq` "x, y") | as `PointCurves` | 1 |
/// Legacy PV2010 properties (`LocalExposure`, `LocalBrightness`, `LocalContrast`,
/// `LocalClarity`, `LocalToning*` pre-2012 ...) are written as 0 for new groups and
/// preserved for existing ones.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LocalAdjustments {
    #[specta(type = Number)]
    pub temperature: f32,
    #[specta(type = Number)]
    pub tint: f32,
    /// EV offset, -4..=4.
    #[specta(type = Number)]
    pub exposure: f32,
    #[specta(type = Number)]
    pub contrast: f32,
    #[specta(type = Number)]
    pub highlights: f32,
    #[specta(type = Number)]
    pub shadows: f32,
    #[specta(type = Number)]
    pub whites: f32,
    #[specta(type = Number)]
    pub blacks: f32,
    #[specta(type = Number)]
    pub texture: f32,
    #[specta(type = Number)]
    pub clarity: f32,
    #[specta(type = Number)]
    pub dehaze: f32,
    /// Degrees, -180..=180.
    #[specta(type = Number)]
    pub hue: f32,
    #[specta(type = Number)]
    pub saturation: f32,
    #[specta(type = Number)]
    pub sharpness: f32,
    /// Luminance noise reduction offset.
    #[specta(type = Number)]
    pub noise: f32,
    #[specta(type = Number)]
    pub moire: f32,
    #[specta(type = Number)]
    pub defringe: f32,
    pub color: LocalColor,
    /// 0..=100, default 100 (Lightroom's curve "Refine Saturation").
    #[specta(type = Number)]
    pub curve_refine_saturation: f32,
    /// Per-mask point curves (identity = no change).
    #[serde(default)]
    pub tone_curve: PointCurves,
}

impl Default for LocalAdjustments {
    fn default() -> Self {
        Self {
            temperature: 0.0,
            tint: 0.0,
            exposure: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
            texture: 0.0,
            clarity: 0.0,
            dehaze: 0.0,
            hue: 0.0,
            saturation: 0.0,
            sharpness: 0.0,
            noise: 0.0,
            moire: 0.0,
            defringe: 0.0,
            color: LocalColor::default(),
            curve_refine_saturation: 100.0,
            tone_curve: PointCurves::default(),
        }
    }
}

impl LocalAdjustments {
    /// Every slider at its default (the group has no visible effect).
    pub fn is_neutral(&self) -> bool {
        *self == Self::default()
    }

    pub fn validate(&self, path: &str) -> Result<(), String> {
        check(&format!("{path}.exposure"), self.exposure, -4.0, 4.0)?;
        for (name, v) in [
            ("temperature", self.temperature),
            ("tint", self.tint),
            ("contrast", self.contrast),
            ("highlights", self.highlights),
            ("shadows", self.shadows),
            ("whites", self.whites),
            ("blacks", self.blacks),
            ("texture", self.texture),
            ("clarity", self.clarity),
            ("dehaze", self.dehaze),
            ("saturation", self.saturation),
            ("sharpness", self.sharpness),
            ("noise", self.noise),
            ("moire", self.moire),
            ("defringe", self.defringe),
        ] {
            check(&format!("{path}.{name}"), v, -100.0, 100.0)?;
        }
        check(&format!("{path}.hue"), self.hue, -180.0, 180.0)?;
        check(&format!("{path}.color.hue"), self.color.hue, 0.0, 360.0)?;
        check(&format!("{path}.color.saturation"), self.color.saturation, 0.0, 100.0)?;
        check(&format!("{path}.curveRefineSaturation"), self.curve_refine_saturation, 0.0, 100.0)?;
        let c = &self.tone_curve;
        for (name, curve) in [("master", &c.master), ("red", &c.red), ("green", &c.green), ("blue", &c.blue)] {
            PointCurves::validate_curve(&format!("{path}.toneCurve.{name}"), curve)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Groups and components
// ---------------------------------------------------------------------------

/// One Masks-panel entry ("Mask 1"): `crs:What="Correction"` in
/// `crs:MaskGroupBasedCorrections`. Groups apply in list order (later groups see earlier
/// groups' results only through the shared per-pixel parameter sums; Lightroom's model).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MaskGroup {
    /// `crs:CorrectionSyncID`: 32 upper-case hex digits, unique within the image. Kept
    /// when read from Lightroom (the XMP writer matches items by it to preserve unmodelled
    /// attributes); new groups get a random one (TS `newMaskId()`).
    pub id: String,
    /// `crs:CorrectionName` ("Mask 1", "Cool Soft"); may be empty.
    pub name: String,
    /// `crs:CorrectionActive` (the panel's visibility toggle). Inactive groups do not render.
    pub active: bool,
    /// `crs:CorrectionAmount`, 0..=2 (1 = 100 %): scales every local slider of the group.
    #[specta(type = Number)]
    pub amount: f32,
    pub adjustments: LocalAdjustments,
    /// Combined in order; the first component's `mode` is ignored (it starts the mask).
    /// Empty = the group has no area (renders nothing).
    pub components: Vec<MaskComponent>,
}

impl MaskGroup {
    /// A copy suitable for another image (paste / sync / presets): AI components drop their
    /// `digest` (the bitmap belongs to the source image; the target recomputes it, as in
    /// Lightroom). Everything else, including ids, is kept.
    pub fn transferable(&self) -> MaskGroup {
        let mut g = self.clone();
        for c in &mut g.components {
            if let MaskShape::Ai(ai) = &mut c.shape {
                ai.digest = None;
            }
        }
        g
    }
}

string_enum! {
    /// How a component combines with the mask built so far (`crs:MaskBlendMode`; value
    /// mapping in `xmp/masks.rs` `BLEND_MODES`: 0 = add verified on the user's files, the
    /// others provisional).
    pub enum MaskBlendMode {
        /// `max(acc, c)` (Lightroom "Add").
        Add => "add",
        /// `acc * (1 - c)` (Lightroom "Subtract").
        Subtract => "subtract",
        /// `acc * c` (Lightroom "Intersect").
        Intersect => "intersect",
    }
}

/// One mask component ("Subject 1", "Brush 1"): an item of `crs:CorrectionMasks`.
/// Its value at a pixel is `opacity * (inverted ? 1 - shape : shape)`, then combined with
/// the group's mask by `mode`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MaskComponent {
    /// `crs:MaskSyncID` (32 upper-case hex, unique within the image).
    pub id: String,
    /// `crs:MaskName`; may be empty (Lightroom names them "<Kind> <n>").
    pub name: String,
    /// `crs:MaskActive`. Inactive components are skipped.
    pub active: bool,
    pub mode: MaskBlendMode,
    /// `crs:MaskInverted`.
    pub inverted: bool,
    /// `crs:MaskValue`, 0..=1.
    #[specta(type = Number)]
    pub opacity: f32,
    pub shape: MaskShape,
}

/// Component geometry / selection. Coordinates are in the sensor frame (module docs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MaskShape {
    /// `Mask/Paint` items (one per stroke).
    Brush(BrushMask),
    /// `Mask/Gradient`.
    Linear(LinearMask),
    /// `Mask/CircularGradient`.
    Radial(RadialMask),
    /// `Mask/RangeMask` luminance range.
    Luminance(LuminanceRange),
    /// `Mask/RangeMask` colour range.
    Color(ColorRange),
    /// `Mask/Image` (AI selection).
    Ai(AiMask),
    /// A Lightroom component Sieve does not model (depth range, future kinds). Preserved in
    /// the sidecar byte-for-byte (matched by `MaskComponent.id`), contributes nothing to the
    /// render and raises `masks_unsupported`. Cannot be created by the frontend.
    Unsupported(UnsupportedMask),
}

/// Brush strokes of one brush component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BrushMask {
    pub strokes: Vec<BrushStroke>,
}

/// One stroke = one `Mask/Paint` item: constant brush settings + a dab polyline.
/// XMP: `crs:Radius`, `crs:Flow`, `crs:CenterWeight` (= 1 - feather, provisional),
/// `crs:MaskValue` (density; 0 for erase strokes, provisional), `crs:Dabs` = `rdf:Seq` of
/// `"d <x> <y>"` (and rarely `"r <radius>"`, a radius change for the following dabs; the
/// reader splits the stroke there).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BrushStroke {
    /// Fraction of the sensor-frame width, > 0 and <= 1 (verified scale).
    #[specta(type = Number)]
    pub radius: f32,
    /// 0..=1: fraction of the stroke's opacity each dab adds (Lightroom "Flow").
    #[specta(type = Number)]
    pub flow: f32,
    /// 0..=1: soft edge width as a fraction of the radius (Lightroom "Feather" / 100).
    #[specta(type = Number)]
    pub feather: f32,
    /// 0..=1: maximum opacity this stroke can reach (Lightroom "Density" / 100).
    #[specta(type = Number)]
    pub density: f32,
    /// Eraser stroke: removes `density` from the brush component instead of adding.
    pub erase: bool,
    /// Lightroom "Auto Mask": confine the stroke to edges similar to the colour under the
    /// dabs (edge-aware refine at evaluation time).
    pub auto_mask: bool,
    /// Dab centres (sensor frame). Coordinates may lie slightly outside 0..=1.
    pub dabs: Vec<NormPoint>,
}

/// Linear gradient: 0 % effect at `zero`, 100 % at `full`, linear ramp between, constant
/// beyond (`crs:ZeroX/ZeroY/FullX/FullY`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LinearMask {
    pub zero: NormPoint,
    pub full: NormPoint,
}

/// Radial gradient (`crs:Top/Left/Bottom/Right` = ellipse bounds before rotation,
/// `crs:Angle`, `crs:Midpoint`, `crs:Roundness`, `crs:Feather`, `crs:Flipped`). Bounds are
/// in the sensor frame and may extend beyond 0..=1.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RadialMask {
    #[specta(type = Number)]
    pub top: f32,
    #[specta(type = Number)]
    pub left: f32,
    #[specta(type = Number)]
    pub bottom: f32,
    #[specta(type = Number)]
    pub right: f32,
    /// Rotation in degrees, -360..=360 (sensor frame; add the orientation's rotation to show it).
    #[specta(type = Number)]
    pub angle: f32,
    /// 0..=100 (Lightroom default 50).
    #[specta(type = Number)]
    pub midpoint: f32,
    /// -100..=100 (default 0).
    #[specta(type = Number)]
    pub roundness: f32,
    /// 0..=100: soft edge width (default 50).
    #[specta(type = Number)]
    pub feather: f32,
    /// Legacy "effect outside" flag (`crs:Flipped`); new masks use `MaskComponent.inverted`.
    pub flipped: bool,
}

/// Luminance range: selects pixels whose luminance (CIE L* / 100 of the image after global
/// white balance and exposure, before local adjustments) lies in `low..=high`, fading to 0
/// at `featherLow` / `featherHigh`. `0 <= featherLow <= low <= high <= featherHigh <= 1`.
/// XMP: `Mask/RangeMask` with `crs:CorrectionRangeMask` (`Type` luminance, `LumRange`
/// "featherLow low high featherHigh", `LumFeather` = smoothness / 100; provisional).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LuminanceRange {
    #[specta(type = Number)]
    pub feather_low: f32,
    #[specta(type = Number)]
    pub low: f32,
    #[specta(type = Number)]
    pub high: f32,
    #[specta(type = Number)]
    pub feather_high: f32,
    /// 0..=100: edge smoothing of the selection (Lightroom "Smoothness", default 50).
    #[specta(type = Number)]
    pub smoothness: f32,
}

/// Colour range: pixels whose colour is close to any sample (1..=5 samples).
/// XMP: `Mask/RangeMask` + `crs:CorrectionRangeMask` (`Type` colour, `ColorAmount` =
/// amount / 100, `PointModels` / `AreaModels`; provisional).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ColorRange {
    pub samples: Vec<ColorSample>,
    /// 0..=100: selection width (Lightroom "Refine", default 50).
    #[specta(type = Number)]
    pub amount: f32,
}

/// One eyedropper sample (point, or a dragged rectangle when `area` is set).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ColorSample {
    /// Sensor frame.
    pub point: NormPoint,
    /// Dragged sample area (sensor frame), if any.
    pub area: Option<NormRect>,
    /// Lightroom's stored colour model for this sample (the raw `PointModels` / `AreaModels`
    /// item), kept verbatim so an unmodified sample round-trips; Sieve re-derives the colour
    /// from `point`/`area` when rendering and writes `null` samples without a model.
    pub lightroom_model: Option<String>,
}

string_enum! {
    /// Body parts of an AI "People" selection (Lightroom's list; empty = entire person).
    pub enum PersonPart {
        FaceSkin => "face_skin",
        BodySkin => "body_skin",
        Eyebrows => "eyebrows",
        EyeSclera => "eye_sclera",
        IrisPupil => "iris_pupil",
        Lips => "lips",
        Teeth => "teeth",
        Hair => "hair",
        Clothes => "clothes",
    }
}

string_enum! {
    /// Landscape categories of an AI "Landscape" selection (Lightroom 13; sky is `AiTarget::Sky`).
    pub enum LandscapeCategory {
        Water => "water",
        Vegetation => "vegetation",
        Mountains => "mountains",
        Architecture => "architecture",
        NaturalGround => "natural_ground",
        ArtificialGround => "artificial_ground",
    }
}

string_enum! {
    /// AI selection families, for capability reporting (`MaskCapabilities.ai`).
    pub enum AiTargetKind {
        Subject => "subject",
        Sky => "sky",
        Background => "background",
        People => "people",
        Object => "object",
        Landscape => "landscape",
    }
}

/// What an AI component selects (`Mask/Image`, `crs:MaskSubType` + `crs:MaskSubCategoryID`;
/// value mapping in `xmp/masks.rs` `AI_SUBTYPES`: subject = 1 verified on 50 of the user's
/// sidecars, the rest provisional).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum AiTarget {
    /// Main subject(s) ("Select Subject").
    Subject,
    Sky,
    /// Everything but the subject ("Select Background").
    Background,
    /// A person (identified by `AiMask.referencePoint`; `null` = all people) or some of
    /// their parts (empty = entire person).
    People {
        parts: Vec<PersonPart>,
    },
    /// The object inside `region` (sensor frame), Lightroom "Objects".
    Object {
        region: NormRect,
    },
    Landscape {
        category: LandscapeCategory,
    },
    /// A Lightroom AI kind Sieve does not model: rendered only from Lightroom's own bitmap
    /// (when present), preserved on write.
    Other {
        sub_type: i32,
        sub_category: Option<i32>,
    },
}

impl AiTarget {
    pub fn family(&self) -> Option<AiTargetKind> {
        Some(match self {
            AiTarget::Subject => AiTargetKind::Subject,
            AiTarget::Sky => AiTargetKind::Sky,
            AiTarget::Background => AiTargetKind::Background,
            AiTarget::People { .. } => AiTargetKind::People,
            AiTarget::Object { .. } => AiTargetKind::Object,
            AiTarget::Landscape { .. } => AiTargetKind::Landscape,
            AiTarget::Other { .. } => return None,
        })
    }
}

/// AI selection. The pixels are a matte computed once per image (Sieve's models) or read
/// from Lightroom's sidecar bitmap, stored in the mask cache (`mask_cache`, migration 0010)
/// and referenced by `digest`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AiMask {
    pub target: AiTarget,
    /// Sensor-frame point identifying the instance (`crs:ReferencePoint`): which person for
    /// `people`, the clicked subject otherwise. `null` = all instances.
    pub reference_point: Option<NormPoint>,
    /// Mask-cache key of the matte this component uses (`crs:MaskDigest` for Lightroom
    /// bitmaps; 32 upper-case hex). `null` = not computed for this image yet: the renderer
    /// uses a cached Sieve matte for (image, [`AiMask::cache_kind`]) if one exists, else the
    /// component is empty and `ai_mask_needs_update` is reported. Set it from
    /// `computeAiMask(...).digest`.
    pub digest: Option<String>,
}

impl AiMask {
    /// `mask_cache.kind`: identifies what was segmented, independent of model/version, e.g.
    /// `subject`, `people:face_skin+lips@0.4123,0.2211`, `object@0.1000,0.2000,0.3000,0.4000`,
    /// `landscape:water`, `other:7:3`.
    pub fn cache_kind(&self) -> String {
        let at = |p: &NormPoint| format!("@{:.4},{:.4}", p.x, p.y);
        match &self.target {
            AiTarget::Subject => "subject".to_owned(),
            AiTarget::Sky => "sky".to_owned(),
            AiTarget::Background => "background".to_owned(),
            AiTarget::People { parts } => {
                let mut names: Vec<&str> = parts.iter().map(|p| p.as_str()).collect();
                names.sort_unstable();
                names.dedup();
                let parts = if names.is_empty() { "person".to_owned() } else { names.join("+") };
                format!("people:{parts}{}", self.reference_point.as_ref().map(at).unwrap_or_default())
            }
            AiTarget::Object { region } => {
                format!("object@{:.4},{:.4},{:.4},{:.4}", region.x, region.y, region.width, region.height)
            }
            AiTarget::Landscape { category } => format!("landscape:{}", category.as_str()),
            AiTarget::Other { sub_type, sub_category } => match sub_category {
                Some(c) => format!("other:{sub_type}:{c}"),
                None => format!("other:{sub_type}"),
            },
        }
    }
}

/// See [`MaskShape::Unsupported`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UnsupportedMask {
    /// `crs:What` of the item (e.g. `Mask/RangeMask` with a depth range).
    pub what: String,
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

fn check(name: &str, v: f32, min: f32, max: f32) -> Result<(), String> {
    if v.is_finite() && (min..=max).contains(&v) {
        Ok(())
    } else {
        Err(format!("{name} = {v} is outside {min}..={max}"))
    }
}

/// 32 upper-case hex digits (Lightroom sync ids and MD5 digests).
pub fn is_mask_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
}

fn check_point(name: &str, p: &NormPoint, lo: f32, hi: f32) -> Result<(), String> {
    check(&format!("{name}.x"), p.x, lo, hi)?;
    check(&format!("{name}.y"), p.y, lo, hi)
}

fn check_rect(name: &str, r: &NormRect) -> Result<(), String> {
    let ok = [r.x, r.y, r.width, r.height].iter().all(|v| v.is_finite())
        && r.x >= 0.0
        && r.y >= 0.0
        && r.width > 0.0
        && r.height > 0.0
        && r.x + r.width <= 1.0 + 1e-4
        && r.y + r.height <= 1.0 + 1e-4;
    if ok {
        Ok(())
    } else {
        Err(format!("{name} must lie within 0..=1 with positive size"))
    }
}

fn check_name(name: &str, v: &str) -> Result<(), String> {
    if v.chars().count() > MaskLimits::MAX_NAME {
        return Err(format!("{name} must be at most {} chars", MaskLimits::MAX_NAME));
    }
    Ok(())
}

/// Validates `ParametricAdjustments.masks` (ranges, ids, limits). Called by
/// `ParametricAdjustments::validate`.
pub fn validate_masks(masks: &[MaskGroup]) -> Result<(), String> {
    if masks.len() > MaskLimits::MAX_GROUPS {
        return Err(format!("{} mask groups (at most {})", masks.len(), MaskLimits::MAX_GROUPS));
    }
    let mut ids = std::collections::HashSet::new();
    let mut dabs = 0usize;
    for (gi, g) in masks.iter().enumerate() {
        let gp = format!("masks[{gi}]");
        if !is_mask_id(&g.id) {
            return Err(format!("{gp}.id {:?} is not 32 upper-case hex digits", g.id));
        }
        if !ids.insert(g.id.as_str()) {
            return Err(format!("{gp}.id {} is not unique", g.id));
        }
        check_name(&format!("{gp}.name"), &g.name)?;
        check(&format!("{gp}.amount"), g.amount, 0.0, 2.0)?;
        g.adjustments.validate(&format!("{gp}.adjustments"))?;
        if g.components.len() > MaskLimits::MAX_COMPONENTS {
            return Err(format!("{gp} has {} components (at most {})", g.components.len(), MaskLimits::MAX_COMPONENTS));
        }
        for (ci, c) in g.components.iter().enumerate() {
            let cp = format!("{gp}.components[{ci}]");
            if !is_mask_id(&c.id) {
                return Err(format!("{cp}.id {:?} is not 32 upper-case hex digits", c.id));
            }
            if !ids.insert(c.id.as_str()) {
                return Err(format!("{cp}.id {} is not unique", c.id));
            }
            check_name(&format!("{cp}.name"), &c.name)?;
            check(&format!("{cp}.opacity"), c.opacity, 0.0, 1.0)?;
            match &c.shape {
                MaskShape::Brush(b) => {
                    for (si, s) in b.strokes.iter().enumerate() {
                        let sp = format!("{cp}.strokes[{si}]");
                        if !(s.radius.is_finite() && s.radius > 0.0 && s.radius <= 1.0) {
                            return Err(format!("{sp}.radius = {} is outside (0, 1]", s.radius));
                        }
                        check(&format!("{sp}.flow"), s.flow, 0.0, 1.0)?;
                        check(&format!("{sp}.feather"), s.feather, 0.0, 1.0)?;
                        check(&format!("{sp}.density"), s.density, 0.0, 1.0)?;
                        if s.dabs.is_empty() {
                            return Err(format!("{sp} has no dabs"));
                        }
                        for (di, d) in s.dabs.iter().enumerate() {
                            check_point(&format!("{sp}.dabs[{di}]"), d, -1.0, 2.0)?;
                        }
                        dabs += s.dabs.len();
                    }
                }
                MaskShape::Linear(l) => {
                    check_point(&format!("{cp}.zero"), &l.zero, -10.0, 11.0)?;
                    check_point(&format!("{cp}.full"), &l.full, -10.0, 11.0)?;
                    if l.zero == l.full {
                        return Err(format!("{cp}: zero and full must differ"));
                    }
                }
                MaskShape::Radial(r) => {
                    for (name, v) in [("top", r.top), ("left", r.left), ("bottom", r.bottom), ("right", r.right)] {
                        check(&format!("{cp}.{name}"), v, -10.0, 11.0)?;
                    }
                    if !(r.left < r.right && r.top < r.bottom) {
                        return Err(format!("{cp} must satisfy left < right and top < bottom"));
                    }
                    check(&format!("{cp}.angle"), r.angle, -360.0, 360.0)?;
                    check(&format!("{cp}.midpoint"), r.midpoint, 0.0, 100.0)?;
                    check(&format!("{cp}.roundness"), r.roundness, -100.0, 100.0)?;
                    check(&format!("{cp}.feather"), r.feather, 0.0, 100.0)?;
                }
                MaskShape::Luminance(l) => {
                    for (name, v) in [
                        ("featherLow", l.feather_low),
                        ("low", l.low),
                        ("high", l.high),
                        ("featherHigh", l.feather_high),
                    ] {
                        check(&format!("{cp}.{name}"), v, 0.0, 1.0)?;
                    }
                    if !(l.feather_low <= l.low && l.low <= l.high && l.high <= l.feather_high) {
                        return Err(format!("{cp} must satisfy featherLow <= low <= high <= featherHigh"));
                    }
                    check(&format!("{cp}.smoothness"), l.smoothness, 0.0, 100.0)?;
                }
                MaskShape::Color(cr) => {
                    if cr.samples.is_empty() || cr.samples.len() > MaskLimits::MAX_COLOR_SAMPLES {
                        return Err(format!("{cp} needs 1..={} samples", MaskLimits::MAX_COLOR_SAMPLES));
                    }
                    for (si, s) in cr.samples.iter().enumerate() {
                        check_point(&format!("{cp}.samples[{si}].point"), &s.point, 0.0, 1.0)?;
                        if let Some(a) = &s.area {
                            check_rect(&format!("{cp}.samples[{si}].area"), a)?;
                        }
                    }
                    check(&format!("{cp}.amount"), cr.amount, 0.0, 100.0)?;
                }
                MaskShape::Ai(ai) => {
                    if let Some(d) = &ai.digest {
                        if !is_mask_id(d) {
                            return Err(format!("{cp}.digest {d:?} is not 32 upper-case hex digits"));
                        }
                    }
                    if let Some(p) = &ai.reference_point {
                        check_point(&format!("{cp}.referencePoint"), p, 0.0, 1.0)?;
                    }
                    if let AiTarget::Object { region } = &ai.target {
                        check_rect(&format!("{cp}.target.region"), region)?;
                    }
                }
                MaskShape::Unsupported(u) => {
                    if u.what.trim().is_empty() {
                        return Err(format!("{cp}.what must not be empty"));
                    }
                }
            }
        }
    }
    if dabs > MaskLimits::MAX_DABS {
        return Err(format!("{dabs} brush dabs (at most {})", MaskLimits::MAX_DABS));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------

/// Sensor frame -> displayed (EXIF-oriented) frame, normalized coordinates.
/// `orientation` is EXIF 1..=8 (anything else = 1). TS mirror `orientPoint`.
pub fn orient_point(p: NormPoint, orientation: u8) -> NormPoint {
    let (u, v) = (p.x, p.y);
    let (x, y) = match orientation {
        2 => (1.0 - u, v),
        3 => (1.0 - u, 1.0 - v),
        4 => (u, 1.0 - v),
        5 => (v, u),
        6 => (1.0 - v, u),
        7 => (1.0 - v, 1.0 - u),
        8 => (v, 1.0 - u),
        _ => (u, v),
    };
    NormPoint { x, y }
}

/// Displayed (EXIF-oriented) frame -> sensor frame; inverse of [`orient_point`].
/// TS mirror `unorientPoint`.
pub fn unorient_point(p: NormPoint, orientation: u8) -> NormPoint {
    let (x, y) = (p.x, p.y);
    let (u, v) = match orientation {
        2 => (1.0 - x, y),
        3 => (1.0 - x, 1.0 - y),
        4 => (x, 1.0 - y),
        5 => (y, x),
        6 => (y, 1.0 - x),
        7 => (1.0 - y, 1.0 - x),
        8 => (1.0 - y, x),
        _ => (x, y),
    };
    NormPoint { x: u, y: v }
}

// ---------------------------------------------------------------------------
// Commands: AI masks, overlays, capabilities
// ---------------------------------------------------------------------------

string_enum! {
    /// Where a cached matte came from (`mask_cache.origin`).
    pub enum AiMaskOrigin {
        /// Decoded from the sidecar's `crs:Table_<MaskDigest>` (Lightroom's own model).
        Lightroom => "lightroom",
        /// Computed by Sieve's segmentation models.
        Sieve => "sieve",
    }
}

/// A cached AI matte.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AiMaskInfo {
    /// Put this into `AiMask.digest`.
    pub digest: String,
    pub target: AiTarget,
    pub reference_point: Option<NormPoint>,
    pub origin: AiMaskOrigin,
    /// Sieve model id (`SegmentModel::id`) or `lr:<crs:ModelVersion>`.
    pub model_version: String,
    /// Stored bitmap size in px.
    pub width: u32,
    pub height: u32,
    /// Placement of the bitmap in the sensor frame (Lightroom crops mattes to their
    /// bounding box: `crs:Origin` / `crs:WholeImageArea`).
    pub bounds: NormRect,
    /// Mean matte value over the whole frame, 0..=1 (0 = nothing found).
    #[specta(type = Number)]
    pub coverage: f32,
}

/// `compute_ai_mask` input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AiMaskRequest {
    pub target: AiTarget,
    /// Sensor frame; for `people` from `detectPeople(...)[i].referencePoint`.
    pub reference_point: Option<NormPoint>,
    /// Recompute even when a matte for this (image, kind, current model) is cached.
    pub force: bool,
}

string_enum! {
    pub enum AiMaskState {
        /// The component's matte is cached and will render.
        Ready => "ready",
        /// No matte for this image yet (pasted/synced/preset mask, or Lightroom mask without
        /// a bitmap): call `computeAiMask` (Lightroom's "Update").
        NeedsUpdate => "needs_update",
        /// A `computeAiMask` for it is running.
        Computing => "computing",
        /// The model for this kind is not installed (`MaskCapabilities`); a Lightroom
        /// bitmap, if any, still renders (then the state is `ready`).
        Unavailable => "unavailable",
    }
}

/// Render status of one AI component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AiMaskStatus {
    pub group_id: String,
    pub component_id: String,
    pub state: AiMaskState,
    /// The matte that renders (when `ready`).
    pub info: Option<AiMaskInfo>,
}

/// `list_masks` result: the image's stored masks + AI status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MaskList {
    pub image_id: super::types::ImageId,
    /// Same as `getAdjustments(id).masks`.
    pub groups: Vec<MaskGroup>,
    /// One per AI component of `groups`, in order.
    pub ai: Vec<AiMaskStatus>,
}

/// A person found by `detect_people` (Lightroom's People thumbnails).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DetectedPerson {
    /// Sensor frame; use as `AiMaskRequest.referencePoint` / `AiMask.referencePoint`.
    pub reference_point: NormPoint,
    /// Person bounds in the *displayed* frame (orientation applied), like `FaceInfo.bbox`.
    pub bbox: NormRect,
    /// Face bounds (displayed frame), for the thumbnail, if a face was found.
    pub face: Option<NormRect>,
}

/// What to visualize with `render_mask_overlay`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MaskOverlayTarget {
    pub group_id: String,
    /// `null` = the group's combined mask (before `amount`); else this component alone
    /// (its `inverted` and `opacity` applied, blend mode not).
    pub component_id: Option<String>,
}

/// Overlay render options. The overlay covers exactly the frame `render_preview` produces
/// for the same `maxEdge` / `region` and adjustments (crop and orientation applied), so it
/// can be stacked on the preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MaskOverlayOptions {
    /// 64..=8192 (as `RenderOptions.maxEdge`).
    pub max_edge: u32,
    pub region: Option<NormRect>,
}

impl MaskOverlayOptions {
    pub fn validate(&self) -> Result<(), String> {
        super::types::RenderOptions::validate_geometry(self.max_edge, self.region)
    }
}

/// A rendered overlay: an 8-bit **grayscale JPEG** of the mask (white = 1) served on the
/// `mask` render slot (`sieve://localhost/render/<id>/mask?v=<seq>`, latest-wins per image).
/// Composite it as a luminance mask (CSS `mask-image` + `mask-mode: luminance`, or canvas)
/// over a colour layer for Lightroom's overlay styles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RenderedMaskOverlay {
    pub image_id: super::types::ImageId,
    pub seq: u32,
    pub url: String,
    pub width: u32,
    pub height: u32,
    /// Mean mask value of the rendered frame, 0..=1.
    #[specta(type = Number)]
    pub coverage: f32,
    pub render_ms: u32,
}

/// Availability of one AI selection family.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AiCapability {
    pub kind: AiTargetKind,
    pub available: bool,
    /// Model id when available.
    pub model: Option<String>,
    /// Why not, user-facing (e.g. "AI masking models are not installed. Download them from
    /// the Masks panel (~560 MB)."), when unavailable.
    pub reason: Option<String>,
}

/// `get_mask_capabilities` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MaskCapabilities {
    /// One entry per [`AiTargetKind`], in `AiTargetKind::ALL` order.
    pub ai: Vec<AiCapability>,
    /// People parts the installed models can separate (empty = entire person only).
    pub person_parts: Vec<PersonPart>,
    pub landscape: Vec<LandscapeCategory>,
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn id(n: u32) -> String {
        format!("{n:032X}")
    }

    fn group(components: Vec<MaskComponent>) -> MaskGroup {
        MaskGroup {
            id: id(1),
            name: "Mask 1".into(),
            active: true,
            amount: 1.0,
            adjustments: LocalAdjustments { exposure: 0.5, ..Default::default() },
            components,
        }
    }

    fn component(n: u32, shape: MaskShape) -> MaskComponent {
        MaskComponent {
            id: id(100 + n),
            name: String::new(),
            active: true,
            mode: MaskBlendMode::Add,
            inverted: false,
            opacity: 1.0,
            shape,
        }
    }

    fn subject() -> MaskShape {
        MaskShape::Ai(AiMask {
            target: AiTarget::Subject,
            reference_point: Some(NormPoint { x: 0.14, y: 0.36 }),
            digest: Some("E71A59AFC894F4F898F72751E30113DA".into()),
        })
    }

    #[test]
    fn valid_masks_pass_and_limits_fail() {
        let brush = MaskShape::Brush(BrushMask {
            strokes: vec![BrushStroke {
                radius: 0.0058,
                flow: 1.0,
                feather: 0.0,
                density: 1.0,
                erase: false,
                auto_mask: false,
                dabs: vec![NormPoint { x: 0.43, y: 0.59 }, NormPoint { x: 0.43, y: 0.58 }],
            }],
        });
        let radial = MaskShape::Radial(RadialMask {
            top: 0.2,
            left: 0.3,
            bottom: 0.6,
            right: 0.7,
            angle: 10.0,
            midpoint: 50.0,
            roundness: 0.0,
            feather: 50.0,
            flipped: false,
        });
        let lum = MaskShape::Luminance(LuminanceRange {
            feather_low: 0.1,
            low: 0.2,
            high: 0.8,
            feather_high: 0.9,
            smoothness: 50.0,
        });
        let g = group(vec![component(1, subject()), component(2, brush), component(3, radial), component(4, lum)]);
        validate_masks(std::slice::from_ref(&g)).unwrap();

        let mut bad = g.clone();
        bad.components[1].id = bad.id.clone();
        assert!(validate_masks(&[bad]).unwrap_err().contains("not unique"));
        let mut bad = g.clone();
        bad.id = "abc".into();
        assert!(validate_masks(&[bad]).is_err());
        let mut bad = g.clone();
        bad.adjustments.exposure = 4.5;
        assert!(validate_masks(&[bad]).unwrap_err().contains("exposure"));
        let mut bad = g.clone();
        bad.amount = 2.5;
        assert!(validate_masks(&[bad]).is_err());
        let mut bad = g;
        if let MaskShape::Luminance(l) = &mut bad.components[3].shape {
            l.low = 0.95;
        }
        assert!(validate_masks(&[bad]).unwrap_err().contains("featherLow <= low"));
    }

    #[test]
    fn wire_format_is_tagged_camel_case() {
        let c = component(1, subject());
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["mode"], "add");
        assert_eq!(v["shape"]["kind"], "ai");
        assert_eq!(v["shape"]["target"]["kind"], "subject");
        assert_eq!(v["shape"]["referencePoint"]["x"], serde_json::json!(0.14f32));
        let people = AiTarget::People { parts: vec![PersonPart::FaceSkin, PersonPart::IrisPupil] };
        let v = serde_json::to_value(&people).unwrap();
        assert_eq!(v, serde_json::json!({"kind": "people", "parts": ["face_skin", "iris_pupil"]}));
        let other = AiTarget::Other { sub_type: 7, sub_category: None };
        assert_eq!(
            serde_json::to_value(&other).unwrap(),
            serde_json::json!({"kind": "other", "subType": 7, "subCategory": null})
        );
        let back: MaskComponent = serde_json::from_value(serde_json::to_value(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn transferable_drops_ai_digests_only() {
        let g = group(vec![component(1, subject())]);
        let t = g.transferable();
        assert_eq!(t.id, g.id);
        match &t.components[0].shape {
            MaskShape::Ai(ai) => {
                assert_eq!(ai.digest, None);
                assert_eq!(ai.reference_point, Some(NormPoint { x: 0.14, y: 0.36 }));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn cache_kind_is_canonical() {
        let ai = |target, p| AiMask { target, reference_point: p, digest: None };
        assert_eq!(ai(AiTarget::Subject, None).cache_kind(), "subject");
        let p = Some(NormPoint { x: 0.41234, y: 0.22111 });
        let a = ai(AiTarget::People { parts: vec![PersonPart::Lips, PersonPart::FaceSkin, PersonPart::Lips] }, p);
        assert_eq!(a.cache_kind(), "people:face_skin+lips@0.4123,0.2211");
        assert_eq!(ai(AiTarget::People { parts: vec![] }, None).cache_kind(), "people:person");
        assert_eq!(ai(AiTarget::Other { sub_type: 7, sub_category: Some(3) }, None).cache_kind(), "other:7:3");
    }

    #[test]
    fn orientation_round_trips() {
        let p = NormPoint { x: 0.2, y: 0.7 };
        for o in 1..=8u8 {
            let q = unorient_point(orient_point(p, o), o);
            assert!((q.x - p.x).abs() < 1e-6 && (q.y - p.y).abs() < 1e-6, "orientation {o}");
        }
        // 6 = rotate 90 deg clockwise for display: sensor top-left -> displayed top-right.
        assert_eq!(orient_point(NormPoint { x: 0.0, y: 0.0 }, 6), NormPoint { x: 1.0, y: 0.0 });
        // 8 = rotate 90 deg counter-clockwise: sensor top-left -> displayed bottom-left.
        assert_eq!(orient_point(NormPoint { x: 0.0, y: 0.0 }, 8), NormPoint { x: 0.0, y: 1.0 });
    }

    #[test]
    fn local_defaults_are_neutral() {
        let d = LocalAdjustments::default();
        assert!(d.is_neutral());
        d.validate("x").unwrap();
        assert_eq!(d.curve_refine_saturation, 100.0);
    }
}
