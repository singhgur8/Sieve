//! IPC contract types shared between the Rust backend and the TypeScript frontend.
//!
//! Rust is the source of truth: `src/ipc/bindings.ts` is generated from these types.
//! Conventions:
//! - struct fields are `camelCase` on the wire;
//! - enum values are `snake_case` strings, identical to what the catalog DB stores;
//! - IDs and unix-ms timestamps are `i64` and exported as TS `number` (all < 2^53);
//! - floats carry `#[specta(type = Number)]` so TS sees `number`, not `number | null`
//!   (specta's default, because serde_json writes NaN as null). Never send NaN.
//!
//! Changing anything here is a contract change: log it in `docs/ipc-changelog.md`.

use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

/// Catalog row id of an image.
pub type ImageId = i64;
/// Catalog row id of an imported folder.
pub type FolderId = i64;
/// Catalog row id of a burst group.
pub type BurstGroupId = i64;

/// Declares a fieldless enum that round-trips through the same snake_case string
/// on the wire (serde) and in SQLite (`as_str` / `parse`).
macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident { $($(#[$vmeta:meta])* $variant:ident => $s:literal),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
        pub enum $name {
            $($(#[$vmeta])* #[serde(rename = $s)] $variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $s),+ }
            }

            pub fn parse(s: &str) -> Option<Self> {
                match s { $($s => Some($name::$variant),)+ _ => None }
            }
        }
    };
}

// ---------------------------------------------------------------------------
// Camera / file identity
// ---------------------------------------------------------------------------

string_enum! {
    /// Supported RAW containers, in pipeline priority order.
    pub enum RawFormat {
        /// Sony ARW (TIFF-based).
        Arw => "arw",
        /// Fujifilm RAF (X-Trans or Bayer).
        Raf => "raf",
        /// Canon CR3 (ISO-BMFF based).
        Cr3 => "cr3",
    }
}

string_enum! {
    pub enum CameraMake {
        Sony => "sony",
        Fujifilm => "fujifilm",
        Canon => "canon",
        Other => "other",
    }
}

string_enum! {
    /// Colour filter array layout; selects the demosaic algorithm.
    /// Fuji bodies can be either, so this is `unknown` until metadata is read.
    pub enum SensorLayout {
        Bayer => "bayer",
        XTrans => "x_trans",
        Unknown => "unknown",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CameraInfo {
    pub make: CameraMake,
    pub model: Option<String>,
    pub sensor_layout: SensorLayout,
}

/// EXIF capture metadata. All optional: populated by the ingest pipeline (Phase 2),
/// in the same pass that extracts the embedded preview.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CaptureMeta {
    /// Capture time in ms, including sub-seconds (`SubSecTimeOriginal`), needed for
    /// burst grouping. EXIF `DateTimeOriginal` is camera-local wall-clock time with no
    /// zone, so it is stored as that wall-clock time *interpreted as UTC* ("naive" ms):
    /// display with `timeZone: "UTC"`; differences between frames are exact.
    pub captured_at_ms: Option<i64>,
    pub iso: Option<u32>,
    #[specta(type = Option<Number>)]
    pub shutter_seconds: Option<f64>,
    #[specta(type = Option<Number>)]
    pub aperture: Option<f32>,
    #[specta(type = Option<Number>)]
    pub focal_length_mm: Option<f32>,
    pub lens: Option<String>,
}

/// Where the embedded preview for an image stands. Pixels are JPEG files under
/// `<cacheDir>/thumbs/`; the frontend loads them with `convertFileSrc(path)`
/// (Tauri asset protocol). Paths are absolute. Orientation is already applied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "status", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum ThumbnailState {
    /// Registered, not yet extracted (or queued for regeneration).
    Pending,
    Ready {
        /// Grid thumbnail, long edge 512 px.
        path: String,
        /// Loupe preview, long edge 2048 px (smaller if the embedded JPEG is smaller).
        /// `None` if only the thumbnail could be produced.
        preview_path: Option<String>,
        /// Pixel size of the thumbnail at `path` (after orientation).
        width: u32,
        height: u32,
    },
    /// Extraction failed; `reason` is a human-readable message.
    Failed { reason: String },
}

// ---------------------------------------------------------------------------
// Culling
// ---------------------------------------------------------------------------

string_enum! {
    /// Granular reason a frame may be culled (or deliberately kept).
    pub enum CullTag {
        Blink => "blink",
        MissedFocus => "missed_focus",
        MotionBlur => "motion_blur",
        /// Intentional blur (panning, bokeh-heavy) — not a defect.
        CreativeBlur => "creative_blur",
        Underexposed => "underexposed",
        Overexposed => "overexposed",
        DuplicateBurst => "duplicate_burst",
    }
}

string_enum! {
    pub enum TagSource {
        /// Emitted by the culling engine.
        Auto => "auto",
        /// Applied by the user.
        User => "user",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CullTagEntry {
    pub tag: CullTag,
    pub source: TagSource,
    /// 0..=1. User tags are always 1.
    #[specta(type = Number)]
    pub confidence: f32,
    /// The user dismissed this auto tag. Kept (not deleted) so re-analysis
    /// does not resurrect it and filters stay non-destructive.
    pub suppressed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExposureStats {
    /// Share of pixels at the highlight clip point, 0..=1.
    #[specta(type = Number)]
    pub clipped_highlights_pct: f32,
    /// Share of pixels at the shadow clip point, 0..=1.
    #[specta(type = Number)]
    pub clipped_shadows_pct: f32,
    /// Mean luminance, 0..=1.
    #[specta(type = Number)]
    pub mean_luma: f32,
}

/// Culling-engine scores. All scores are normalized to 0..=1, higher is better.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct QualityScore {
    #[specta(type = Number)]
    pub overall: f32,
    /// Laplacian variance on the primary face crop; `None` when no face found.
    #[specta(type = Option<Number>)]
    pub face_sharpness: Option<f32>,
    #[specta(type = Number)]
    pub global_sharpness: f32,
    /// Eye-openness (from EAR) of the least-open detected face; `None` when no face.
    #[specta(type = Option<Number>)]
    pub eyes_open: Option<f32>,
    #[specta(type = Option<Number>)]
    pub composition: Option<f32>,
    pub face_count: u32,
    pub exposure: ExposureStats,
    /// Identifies the model/algorithm set, so scores can be recomputed on upgrade.
    pub model_version: String,
    /// Engine's suggested star rating 0..=5. Never written to `RawImageEntry.rating`
    /// except through `apply_suggestions`.
    pub suggested_rating: u8,
    /// Engine's suggested flag (burst non-keepers and hard defects lean `reject`).
    /// Never written to `RawImageEntry.pick` except through `apply_suggestions`.
    pub suggested_pick: PickFlag,
}

/// Axis-aligned rectangle in normalized preview coordinates: 0..=1 of the preview's
/// width/height, origin top-left, orientation already applied (same frame the UI shows).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NormRect {
    #[specta(type = Number)]
    pub x: f32,
    #[specta(type = Number)]
    pub y: f32,
    #[specta(type = Number)]
    pub width: f32,
    #[specta(type = Number)]
    pub height: f32,
}

/// Point in normalized preview coordinates (see [`NormRect`]).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NormPoint {
    #[specta(type = Number)]
    pub x: f32,
    #[specta(type = Number)]
    pub y: f32,
}

/// One detected face, for the loupe's face-crop zoom and per-face diagnostics.
/// Raw measurements (`ear`, `sharpness`) are threshold-independent; the derived flags
/// (`eyesOpen`, `blink`, `inFocus`) reflect the thresholds at the last (re)score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FaceInfo {
    pub bbox: NormRect,
    /// Eye centres from the detector keypoints; image-left and image-right.
    pub left_eye: NormPoint,
    pub right_eye: NormPoint,
    /// Detector confidence 0..=1.
    #[specta(type = Number)]
    pub detection_score: f32,
    /// Eye Aspect Ratio of the less-open eye; `None` if landmarks were unusable.
    #[specta(type = Option<Number>)]
    pub ear: Option<f32>,
    /// Eye openness 0..=1 derived from `ear`; `None` if `ear` is `None`.
    #[specta(type = Option<Number>)]
    pub eyes_open: Option<f32>,
    /// Normalized sharpness 0..=1 of the eye region (face crop if eyes unusable).
    #[specta(type = Number)]
    pub sharpness: f32,
    /// Eyes closed per `CullThresholds.blinkEar`.
    pub blink: bool,
    /// `sharpness >= CullThresholds.faceSharpnessMin`.
    pub in_focus: bool,
    /// The face that drives `QualityScore.faceSharpness` (largest / most central).
    pub primary: bool,
    /// Face is large enough to be judged (`CullThresholds.minFaceSize`); smaller faces
    /// are reported but ignored for tags and scores.
    pub considered: bool,
}

/// Relative weights of the score components in `QualityScore.overall`.
/// Non-negative; normalized by their sum (all-zero is invalid).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ScoreWeights {
    #[specta(type = Number)]
    pub eyes_open: f32,
    #[specta(type = Number)]
    pub face_sharpness: f32,
    #[specta(type = Number)]
    pub global_sharpness: f32,
    #[specta(type = Number)]
    pub exposure: f32,
    #[specta(type = Number)]
    pub composition: f32,
}

/// Tag/score thresholds for one `ShootType`. Defaults come from
/// `ml::thresholds::default_thresholds`; user overrides are stored per shoot type.
/// Sharpness values use the same normalized 0..=1 scale as `QualityScore`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CullThresholds {
    /// EAR below this means the eye is closed (`blink`). 0..=1; open eyes are ~0.24.
    #[specta(type = Number)]
    pub blink_ear: f32,
    /// Group shots: `blink` if *any* considered face blinks (else only the primary face).
    pub require_all_eyes_open: bool,
    /// Faces whose box height is below this share of the preview height are ignored. 0..=1.
    #[specta(type = Number)]
    pub min_face_size: f32,
    /// Primary-face sharpness below this means `missed_focus`. 0..=1.
    #[specta(type = Number)]
    pub face_sharpness_min: f32,
    /// Global sharpness below this (no usable face) means `missed_focus`/`motion_blur`. 0..=1.
    #[specta(type = Number)]
    pub global_sharpness_min: f32,
    /// Mean luma below this means `underexposed`. 0..=1.
    #[specta(type = Number)]
    pub underexposed_mean_luma: f32,
    /// Share of shadow-clipped pixels above this means `underexposed`. 0..=1.
    #[specta(type = Number)]
    pub underexposed_clip_pct: f32,
    /// Share of highlight-clipped pixels above this means `overexposed`. 0..=1.
    #[specta(type = Number)]
    pub overexposed_clip_pct: f32,
    /// Max Hamming distance (0..=64) between 64-bit perceptual hashes of frames in one burst.
    pub burst_hash_distance: u32,
    /// `overall` at or above this suggests `pick`. 0..=1.
    #[specta(type = Number)]
    pub pick_min_overall: f32,
    /// `overall` below this suggests `reject`. 0..=1, `<= pickMinOverall`.
    #[specta(type = Number)]
    pub reject_max_overall: f32,
    pub weights: ScoreWeights,
}

impl CullThresholds {
    /// Returns a description of the first invalid value, if any.
    pub fn validate(&self) -> Result<(), String> {
        fn unit(name: &str, v: f32) -> Result<(), String> {
            if v.is_finite() && (0.0..=1.0).contains(&v) {
                Ok(())
            } else {
                Err(format!("{name} = {v} is outside 0..=1"))
            }
        }
        for (name, v) in [
            ("blinkEar", self.blink_ear),
            ("minFaceSize", self.min_face_size),
            ("faceSharpnessMin", self.face_sharpness_min),
            ("globalSharpnessMin", self.global_sharpness_min),
            ("underexposedMeanLuma", self.underexposed_mean_luma),
            ("underexposedClipPct", self.underexposed_clip_pct),
            ("overexposedClipPct", self.overexposed_clip_pct),
            ("pickMinOverall", self.pick_min_overall),
            ("rejectMaxOverall", self.reject_max_overall),
        ] {
            unit(name, v)?;
        }
        if self.burst_hash_distance > 64 {
            return Err(format!("burstHashDistance = {} is outside 0..=64", self.burst_hash_distance));
        }
        if self.reject_max_overall > self.pick_min_overall {
            return Err("rejectMaxOverall must be <= pickMinOverall".into());
        }
        let w = self.weights;
        let ws = [w.eyes_open, w.face_sharpness, w.global_sharpness, w.exposure, w.composition];
        if ws.iter().any(|v| !v.is_finite() || *v < 0.0) {
            return Err("weights must be finite and >= 0".into());
        }
        if ws.iter().sum::<f32>() <= 0.0 {
            return Err("weights must not all be zero".into());
        }
        Ok(())
    }
}

/// What `analyze_images` should (re)process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum AnalysisScope {
    /// Every image with a ready preview that is unanalyzed or out of date (new import,
    /// re-extracted preview, older `modelVersion`). What auto-analysis runs.
    Pending,
    /// Re-measure these images even if up to date. Unknown ids fail with `not_found`.
    Images { ids: Vec<ImageId> },
    /// Re-measure every image in this folder.
    Folder { folder_id: FolderId },
    /// Re-measure the whole catalog.
    All,
    /// No ML: recompute tags, scores, suggestions and burst groups from stored
    /// measurements (after a shoot type / threshold / burst window change).
    Rescore,
}

/// Catalog-wide analysis snapshot, so the UI can restore progress after a reload.
/// `total = analyzed + failed + pending + waiting`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisStatus {
    pub total: u32,
    /// Up to date with the current model version and preview.
    pub analyzed: u32,
    /// Failed with the current model version (not retried until forced or re-extracted).
    pub failed: u32,
    /// Have a ready preview and need (re)analysis.
    pub pending: u32,
    /// No usable preview yet (thumbnail pending or failed).
    pub waiting: u32,
    /// The background worker is currently working.
    pub running: bool,
}

/// A cluster of near-identical frames shot in quick succession.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BurstGroup {
    pub id: BurstGroupId,
    pub started_at_ms: i64,
    pub ended_at_ms: i64,
    /// Best frame of the burst; the others carry `duplicate_burst`.
    pub keeper_image_id: Option<ImageId>,
    /// Members in capture order (at least 2).
    pub image_ids: Vec<ImageId>,
}

string_enum! {
    /// Shoot context; biases subject prioritization and tag thresholds.
    pub enum ShootType {
        Wedding => "wedding",
        Portrait => "portrait",
        Sports => "sports",
        Event => "event",
        Landscape => "landscape",
        General => "general",
    }
}

string_enum! {
    pub enum PickFlag {
        Pick => "pick",
        Reject => "reject",
        Unflagged => "unflagged",
    }
}

string_enum! {
    /// Lightroom-compatible colour labels (`xmp:Label`).
    pub enum ColorLabel {
        Red => "red",
        Yellow => "yellow",
        Green => "green",
        Blue => "blue",
        Purple => "purple",
    }
}

/// One RAW file in the catalog, with everything the grid and loupe need.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RawImageEntry {
    pub id: ImageId,
    pub folder_id: FolderId,
    pub path: String,
    pub file_name: String,
    pub format: RawFormat,
    pub camera: CameraInfo,
    pub capture: CaptureMeta,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// EXIF orientation 1..=8.
    pub orientation: Option<u8>,
    pub file_size: i64,
    pub file_mtime_ms: i64,
    pub thumbnail: ThumbnailState,
    /// Star rating 0..=5.
    pub rating: u8,
    pub pick: PickFlag,
    pub color_label: Option<ColorLabel>,
    pub burst_group_id: Option<BurstGroupId>,
    /// This image is its burst group's keeper.
    pub is_burst_keeper: bool,
    pub tags: Vec<CullTagEntry>,
    pub quality: Option<QualityScore>,
    pub has_edits: bool,
}

// ---------------------------------------------------------------------------
// Editing
// ---------------------------------------------------------------------------

/// White balance. `AsShot` uses the camera's recorded multipliers.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "mode", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum WhiteBalance {
    AsShot,
    /// `temperatureK` 2000..=50000, `tint` -150..=150 (Lightroom scale).
    Custom {
        #[specta(type = Number)]
        temperature_k: f32,
        #[specta(type = Number)]
        tint: f32,
    },
}

/// Per-colour-band slider values, -100..=100 (Lightroom HSL / Color Mixer bands).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HslChannels {
    #[specta(type = Number)]
    pub red: f32,
    #[specta(type = Number)]
    pub orange: f32,
    #[specta(type = Number)]
    pub yellow: f32,
    #[specta(type = Number)]
    pub green: f32,
    #[specta(type = Number)]
    pub aqua: f32,
    #[specta(type = Number)]
    pub blue: f32,
    #[specta(type = Number)]
    pub purple: f32,
    #[specta(type = Number)]
    pub magenta: f32,
}

impl HslChannels {
    fn values(&self) -> [f32; 8] {
        [self.red, self.orange, self.yellow, self.green, self.aqua, self.blue, self.purple, self.magenta]
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HslAdjustments {
    pub hue: HslChannels,
    pub saturation: HslChannels,
    pub luminance: HslChannels,
}

/// A 3D LUT (`.cube`) applied after the parametric stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LutRef {
    pub path: String,
    /// Blend amount 0..=100.
    #[specta(type = Number)]
    pub amount: f32,
}

/// Parametric develop settings. Field names and ranges mirror Adobe Camera Raw
/// Process 2012+ (`crs:` XMP namespace) so XMP export is a 1:1 mapping.
/// Stored JSON missing newer fields loads as neutral (see `db::repo::get_adjustments`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ParametricAdjustments {
    /// Bumped when slider semantics change; stored alongside the JSON.
    pub process_version: u32,
    pub white_balance: WhiteBalance,
    /// EV, -5..=5 (`crs:Exposure2012`).
    #[specta(type = Number)]
    pub exposure: f32,
    // The following are all -100..=100.
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
    #[specta(type = Number)]
    pub vibrance: f32,
    #[specta(type = Number)]
    pub saturation: f32,
    pub hsl: HslAdjustments,
    pub lut: Option<LutRef>,
}

impl ParametricAdjustments {
    pub const PROCESS_VERSION: u32 = 1;

    /// Returns a description of the first out-of-range value, if any.
    pub fn validate(&self) -> Result<(), String> {
        fn check(name: &str, v: f32, min: f32, max: f32) -> Result<(), String> {
            if v.is_finite() && (min..=max).contains(&v) {
                Ok(())
            } else {
                Err(format!("{name} = {v} is outside {min}..={max}"))
            }
        }

        if let WhiteBalance::Custom { temperature_k, tint } = self.white_balance {
            check("whiteBalance.temperatureK", temperature_k, 2000.0, 50000.0)?;
            check("whiteBalance.tint", tint, -150.0, 150.0)?;
        }
        check("exposure", self.exposure, -5.0, 5.0)?;
        for (name, v) in [
            ("contrast", self.contrast),
            ("highlights", self.highlights),
            ("shadows", self.shadows),
            ("whites", self.whites),
            ("blacks", self.blacks),
            ("texture", self.texture),
            ("clarity", self.clarity),
            ("dehaze", self.dehaze),
            ("vibrance", self.vibrance),
            ("saturation", self.saturation),
        ] {
            check(name, v, -100.0, 100.0)?;
        }
        for (band, channels) in [
            ("hsl.hue", &self.hsl.hue),
            ("hsl.saturation", &self.hsl.saturation),
            ("hsl.luminance", &self.hsl.luminance),
        ] {
            for v in channels.values() {
                check(band, v, -100.0, 100.0)?;
            }
        }
        if let Some(lut) = &self.lut {
            check("lut.amount", lut.amount, 0.0, 100.0)?;
        }
        Ok(())
    }
}

impl Default for ParametricAdjustments {
    fn default() -> Self {
        Self {
            process_version: Self::PROCESS_VERSION,
            white_balance: WhiteBalance::AsShot,
            exposure: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
            texture: 0.0,
            clarity: 0.0,
            dehaze: 0.0,
            vibrance: 0.0,
            saturation: 0.0,
            hsl: HslAdjustments::default(),
            lut: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Queries & catalog state
// ---------------------------------------------------------------------------

string_enum! {
    pub enum TagMatch {
        /// Image has at least one of `includeTags`.
        Any => "any",
        /// Image has every one of `includeTags`.
        All => "all",
    }
}

string_enum! {
    pub enum ImageSort {
        CaptureTime => "capture_time",
        FileName => "file_name",
        /// Best `QualityScore.overall` first; unscored images last.
        Quality => "quality",
    }
}

/// Filter + page request for the grid. Suppressed tags never match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImageQuery {
    pub include_tags: Vec<CullTag>,
    pub exclude_tags: Vec<CullTag>,
    pub tag_match: TagMatch,
    pub pick: Option<PickFlag>,
    pub min_rating: Option<u8>,
    pub burst_group_id: Option<BurstGroupId>,
    pub folder_id: Option<FolderId>,
    pub sort: ImageSort,
    pub offset: u32,
    /// Capped at [`ImageQuery::MAX_LIMIT`].
    pub limit: u32,
}

impl ImageQuery {
    pub const MAX_LIMIT: u32 = 1000;
}

impl Default for ImageQuery {
    fn default() -> Self {
        Self {
            include_tags: Vec::new(),
            exclude_tags: Vec::new(),
            tag_match: TagMatch::Any,
            pick: None,
            min_rating: None,
            burst_group_id: None,
            folder_id: None,
            sort: ImageSort::CaptureTime,
            offset: 0,
            limit: 200,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImagePage {
    pub items: Vec<RawImageEntry>,
    /// Total matches ignoring offset/limit.
    pub total: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportOptions {
    pub recursive: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub folder_id: FolderId,
    /// New images added to the catalog.
    pub added: u32,
    /// Supported files already in the catalog.
    pub skipped: u32,
    /// Files with a RAW extension whose header did not match the format.
    pub invalid: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FolderEntry {
    pub id: FolderId,
    pub path: String,
    pub image_count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TagCount {
    pub tag: CullTag,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CatalogState {
    pub catalog_path: String,
    pub image_count: u32,
    pub shoot_type: ShootType,
    /// Max gap between consecutive frames in one burst.
    pub burst_window_ms: u32,
    pub folders: Vec<FolderEntry>,
    /// Counts of non-suppressed tags, for the filter bar.
    pub tag_counts: Vec<TagCount>,
    /// Root of the derived-file cache (`<app_cache_dir>` or `$LUMENRAW_CACHE`).
    /// Thumbnails/previews live in `<cacheDir>/thumbs/`.
    pub cache_dir: String,
    /// Analysis starts automatically after import / on launch (`set_auto_analyze`).
    pub auto_analyze: bool,
}

/// Snapshot of thumbnail/metadata extraction, so the UI can restore its progress
/// display after a reload. Counts cover the whole catalog.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportStatus {
    pub total: u32,
    pub pending: u32,
    pub ready: u32,
    pub failed: u32,
    /// The background pipeline is currently working.
    pub running: bool,
}
