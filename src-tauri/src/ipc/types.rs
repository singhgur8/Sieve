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
/// Catalog row id of a develop preset.
pub type PresetId = i64;
/// Catalog row id of an adjustment history entry.
pub type HistoryEntryId = i64;
/// Catalog row id of a scene (Phase 7).
pub type SceneId = i64;
/// LUT library id: the `.cube` file stem in the LUT directory (`[a-z0-9-]+`).
pub type LutId = String;

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
    /// Sidecar sync state of the XMP-mapped values (rating, pick, label, tags, develop settings).
    pub xmp: XmpSyncState,
    /// Scene (lighting scenario) this image belongs to, if any (Phase 7).
    pub scene_id: Option<SceneId>,
    /// This image is a graded anchor of its scene.
    pub is_scene_anchor: bool,
}

// ---------------------------------------------------------------------------
// XMP sidecars (Phase 4)
// ---------------------------------------------------------------------------

/// Per-image XMP sidecar state (`<basename>.xmp` next to the RAW).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct XmpSyncState {
    /// Rating/pick/label/tags or develop settings (crs:) changed in the catalog since the
    /// sidecar was last written.
    pub dirty: bool,
    /// Unix ms when catalog and sidecar last agreed (write or read); `None` = never synced.
    pub synced_at_ms: Option<i64>,
    /// Reason of the last failed write/read; `None` after a success.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct XmpFailure {
    pub image_id: ImageId,
    pub reason: String,
}

/// Result of `write_xmp` / `read_xmp`. Per-image file errors do not fail the batch;
/// they are listed in `failed` (and stored in `XmpSyncState.error`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct XmpSyncReport {
    /// Sidecars written (`write_xmp`) or read (`read_xmp`).
    pub succeeded: u32,
    /// `read_xmp`: images without a sidecar. `write_xmp`: always 0.
    pub skipped: u32,
    pub failed: Vec<XmpFailure>,
    /// Images whose catalog rating/pick/label or develop settings changed as a result (reads only);
    /// refetch them with `get_images`.
    pub changed: Vec<ImageId>,
}

/// Catalog-wide XMP sync snapshot.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct XmpStatus {
    /// Images with unwritten changes.
    pub dirty: u32,
    /// Images whose last write/read failed.
    pub failed: u32,
    /// The auto-sync writer is currently working.
    pub running: bool,
    /// Same as `CatalogState.xmpAutoSync`.
    pub auto_sync: bool,
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

    fn lerp(a: &Self, b: &Self, t: f32) -> Self {
        let l = |x: f32, y: f32| x + (y - x) * t;
        Self {
            red: l(a.red, b.red),
            orange: l(a.orange, b.orange),
            yellow: l(a.yellow, b.yellow),
            green: l(a.green, b.green),
            aqua: l(a.aqua, b.aqua),
            blue: l(a.blue, b.blue),
            purple: l(a.purple, b.purple),
            magenta: l(a.magenta, b.magenta),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HslAdjustments {
    pub hue: HslChannels,
    pub saturation: HslChannels,
    pub luminance: HslChannels,
}

/// A `.cube` LUT from the LUT library, applied after the parametric stage
/// (on display-referred sRGB-encoded values, before output encoding).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LutRef {
    /// `LutInfo.id` (library file stem). A missing LUT renders as if absent and sets
    /// `RenderedPreview.lutMissing`.
    pub id: LutId,
    /// Blend amount 0..=100 (100 = full LUT output).
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
            if !is_valid_lut_id(&lut.id) {
                return Err(format!("lut.id {:?} is not a valid LUT id", lut.id));
            }
            check("lut.amount", lut.amount, 0.0, 100.0)?;
        }
        Ok(())
    }

    /// Copies the groups in `fields` from `src` into `self`, leaving other groups as-is.
    /// The semantics of every fields mask (presets, paste, sync).
    pub fn copy_fields(&mut self, src: &ParametricAdjustments, fields: &[AdjustmentField]) {
        for field in fields {
            match field {
                AdjustmentField::WhiteBalance => self.white_balance = src.white_balance,
                AdjustmentField::Exposure => self.exposure = src.exposure,
                AdjustmentField::Contrast => self.contrast = src.contrast,
                AdjustmentField::Highlights => self.highlights = src.highlights,
                AdjustmentField::Shadows => self.shadows = src.shadows,
                AdjustmentField::Whites => self.whites = src.whites,
                AdjustmentField::Blacks => self.blacks = src.blacks,
                AdjustmentField::Texture => self.texture = src.texture,
                AdjustmentField::Clarity => self.clarity = src.clarity,
                AdjustmentField::Dehaze => self.dehaze = src.dehaze,
                AdjustmentField::Vibrance => self.vibrance = src.vibrance,
                AdjustmentField::Saturation => self.saturation = src.saturation,
                AdjustmentField::HslHue => self.hsl.hue = src.hsl.hue,
                AdjustmentField::HslSaturation => self.hsl.saturation = src.hsl.saturation,
                AdjustmentField::HslLuminance => self.hsl.luminance = src.hsl.luminance,
                AdjustmentField::Lut => self.lut = src.lut.clone(),
            }
        }
    }

    /// All values neutral (an unedited image renders identically).
    pub fn is_neutral(&self) -> bool {
        *self == Self::default()
    }

    /// Interpolates `a` (t = 0) -> `b` (t = 1); `t` is clamped to 0..=1. Defines the scene-match
    /// strength slider (`lerp(base, full, strength)`) and two-anchor blending. Mirrored by
    /// `lerpAdjustments` in `src/ipc/index.ts`; keep both in sync.
    /// - Numeric sliders and HSL bands: linear.
    /// - White balance: both `custom` -> temperature linear in mireds (1e6 / K), tint linear;
    ///   otherwise the nearer side's value (`t < 0.5` -> `a`).
    /// - LUT: same id on both sides -> amount linear; otherwise the nearer side's LUT.
    /// - `processVersion`: `a`'s.
    pub fn lerp(a: &ParametricAdjustments, b: &ParametricAdjustments, t: f32) -> ParametricAdjustments {
        let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
        let l = |x: f32, y: f32| x + (y - x) * t;
        let near_b = t >= 0.5;
        let white_balance = match (a.white_balance, b.white_balance) {
            (
                WhiteBalance::Custom { temperature_k: ta, tint: ia },
                WhiteBalance::Custom { temperature_k: tb, tint: ib },
            ) => {
                let mired = l(1.0e6 / ta, 1.0e6 / tb);
                WhiteBalance::Custom { temperature_k: (1.0e6 / mired).clamp(2000.0, 50000.0), tint: l(ia, ib) }
            }
            (x, y) => {
                if near_b {
                    y
                } else {
                    x
                }
            }
        };
        let lut = match (&a.lut, &b.lut) {
            (Some(x), Some(y)) if x.id == y.id => Some(LutRef { id: x.id.clone(), amount: l(x.amount, y.amount) }),
            (x, y) => {
                if near_b {
                    y.clone()
                } else {
                    x.clone()
                }
            }
        };
        ParametricAdjustments {
            process_version: a.process_version,
            white_balance,
            exposure: l(a.exposure, b.exposure),
            contrast: l(a.contrast, b.contrast),
            highlights: l(a.highlights, b.highlights),
            shadows: l(a.shadows, b.shadows),
            whites: l(a.whites, b.whites),
            blacks: l(a.blacks, b.blacks),
            texture: l(a.texture, b.texture),
            clarity: l(a.clarity, b.clarity),
            dehaze: l(a.dehaze, b.dehaze),
            vibrance: l(a.vibrance, b.vibrance),
            saturation: l(a.saturation, b.saturation),
            hsl: HslAdjustments {
                hue: HslChannels::lerp(&a.hsl.hue, &b.hsl.hue, t),
                saturation: HslChannels::lerp(&a.hsl.saturation, &b.hsl.saturation, t),
                luminance: HslChannels::lerp(&a.hsl.luminance, &b.hsl.luminance, t),
            },
            lut,
        }
    }
}

/// `[a-z0-9-]{1,64}`: safe as a file stem and inside XMP.
pub fn is_valid_lut_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

string_enum! {
    /// Groups of `ParametricAdjustments` selectable in a fields mask (Lightroom's
    /// "Copy Settings" / preset checkboxes). `ALL` selects everything.
    pub enum AdjustmentField {
        /// `whiteBalance` (mode + temperature + tint).
        WhiteBalance => "white_balance",
        Exposure => "exposure",
        Contrast => "contrast",
        Highlights => "highlights",
        Shadows => "shadows",
        Whites => "whites",
        Blacks => "blacks",
        Texture => "texture",
        Clarity => "clarity",
        Dehaze => "dehaze",
        Vibrance => "vibrance",
        Saturation => "saturation",
        /// `hsl.hue` (all 8 bands).
        HslHue => "hsl_hue",
        /// `hsl.saturation` (all 8 bands).
        HslSaturation => "hsl_saturation",
        /// `hsl.luminance` (all 8 bands).
        HslLuminance => "hsl_luminance",
        /// `lut` (reference + amount; copying a `null` removes the target's LUT).
        Lut => "lut",
    }
}

// ---------------------------------------------------------------------------
// Editor (Phase 5): preview render, develop info, history, presets, LUTs
// ---------------------------------------------------------------------------

string_enum! {
    /// Independent latest-wins render stream per image. A newer request in the same
    /// (image, slot) supersedes older ones; different slots never cancel each other.
    pub enum RenderSlot {
        /// The edited image in the loupe (slider feedback).
        Main => "main",
        /// Before/after view (the frontend sends the "before" adjustments, e.g. neutral).
        Before => "before",
        /// A zoomed region (`RenderOptions.region`), e.g. 1:1 loupe detail.
        Detail => "detail",
    }
}

/// How to render a preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RenderOptions {
    /// Long edge of the output in px, 64..=8192 (use CSS size x devicePixelRatio).
    /// Never upscaled beyond the develop source (`DevelopInfo.sourceWidth/Height`, or the
    /// region's share of it).
    pub max_edge: u32,
    pub slot: RenderSlot,
    /// Render only this part of the (orientation-corrected) frame; `null` = whole frame.
    pub region: Option<NormRect>,
}

impl RenderOptions {
    pub const MIN_EDGE: u32 = 64;
    pub const MAX_EDGE: u32 = 8192;

    pub fn validate(&self) -> Result<(), String> {
        if !(Self::MIN_EDGE..=Self::MAX_EDGE).contains(&self.max_edge) {
            return Err(format!("maxEdge = {} is outside {}..={}", self.max_edge, Self::MIN_EDGE, Self::MAX_EDGE));
        }
        if let Some(r) = self.region {
            let ok = [r.x, r.y, r.width, r.height].iter().all(|v| v.is_finite())
                && r.x >= 0.0
                && r.y >= 0.0
                && r.width > 0.0
                && r.height > 0.0
                && r.x + r.width <= 1.0 + 1e-4
                && r.y + r.height <= 1.0 + 1e-4;
            if !ok {
                return Err("region must lie within 0..=1 with positive size".into());
            }
        }
        Ok(())
    }
}

/// 256-bin histograms of the rendered output (8-bit sRGB-encoded values, what the user
/// sees). `luma` uses Rec.709 weights on the encoded values. Each vector has 256 entries;
/// every channel sums to `width * height` of the render.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Histogram {
    pub red: Vec<u32>,
    pub green: Vec<u32>,
    pub blue: Vec<u32>,
    pub luma: Vec<u32>,
}

impl Histogram {
    pub const BINS: usize = 256;
}

/// A finished preview render. Pixels are an in-memory JPEG (sRGB, quality ~90, 4:4:4)
/// served by the `sieve` URI scheme at `url`; set it as an `<img src>` directly (do not
/// pass it through `convertFileSrc`). The URL is unique per render (`?v=<seq>`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RenderedPreview {
    pub image_id: ImageId,
    pub slot: RenderSlot,
    /// Monotonic per (image, slot); larger = newer.
    pub seq: u32,
    /// `sieve://localhost/render/<imageId>/<slot>?v=<seq>` on macOS
    /// (`http://sieve.localhost/...` on Windows).
    pub url: String,
    /// Output pixel size (orientation applied).
    pub width: u32,
    pub height: u32,
    pub histogram: Histogram,
    /// Wall time of this request inside the backend (decode if uncached + pipeline + encode).
    pub render_ms: u32,
    /// `adjustments.lut` refers to a LUT not in the library; rendered without it.
    pub lut_missing: bool,
}

/// White balance as Lightroom shows it for RAW files.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct WhiteBalanceValues {
    /// Kelvin, 2000..=50000.
    #[specta(type = Number)]
    pub temperature_k: f32,
    /// -150..=150.
    #[specta(type = Number)]
    pub tint: f32,
}

/// Facts about an image's develop source, for initializing the editor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DevelopInfo {
    pub image_id: ImageId,
    /// The camera's as-shot white balance expressed as temperature/tint (from LibRaw's
    /// camera multipliers and colour matrix); `null` if the file has none. Seeds the
    /// Temp/Tint sliders when switching from `as_shot` to `custom`.
    pub as_shot: Option<WhiteBalanceValues>,
    /// Size of the cached develop source (half-size demosaic), orientation applied.
    pub source_width: u32,
    pub source_height: u32,
    /// Full sensor output size, orientation applied (Phase 6 export size).
    pub full_width: u32,
    pub full_height: u32,
}

/// One state in an image's edit history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: HistoryEntryId,
    /// e.g. "Exposure", "Paste Settings", "Preset: Warm", "Reset", "Read from XMP".
    /// The first entry of every history is "Original" (the state before the first edit).
    pub label: String,
    pub created_at_ms: i64,
}

/// Linear per-image edit history (oldest first) with a cursor. Undo/redo move the cursor
/// and make that entry's snapshot the image's adjustments; a new edit after an undo
/// discards the entries after the cursor.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AdjustmentHistory {
    pub image_id: ImageId,
    pub entries: Vec<HistoryEntry>,
    /// Entry whose snapshot is the current adjustments; `null` = never edited (no entries).
    pub current_entry_id: Option<HistoryEntryId>,
    pub can_undo: bool,
    pub can_redo: bool,
}

/// Adjustments + history after an undo/redo/jump.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EditState {
    pub adjustments: ParametricAdjustments,
    pub history: AdjustmentHistory,
}

/// A saved develop preset: applies `adjustments` restricted to `fields`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    pub id: PresetId,
    /// Unique (case-insensitive), 1..=100 chars.
    pub name: String,
    pub adjustments: ParametricAdjustments,
    /// Non-empty; groups outside it are ignored when applying.
    pub fields: Vec<AdjustmentField>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

string_enum! {
    pub enum LutKind {
        /// `LUT_1D_SIZE`: per-channel curves.
        Lut1d => "lut_1d",
        /// `LUT_3D_SIZE`: RGB cube.
        Lut3d => "lut_3d",
    }
}

/// A `.cube` file in the LUT library (`<app_data>/luts/<id>.cube`, `$SIEVE_LUTS`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LutInfo {
    pub id: LutId,
    /// `TITLE` from the file, else the imported file's name without extension.
    pub name: String,
    pub kind: LutKind,
    /// Entries per axis (2..=65 for 3D, 2..=65536 for 1D).
    pub size: u32,
    /// Absolute path of the library copy.
    pub path: String,
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
    /// Natural order of each key; `ImageQuery.sortDescending` reverses it.
    pub enum ImageSort {
        /// Oldest first; images without a capture time last. Ties by file name.
        CaptureTime => "capture_time",
        /// A..Z.
        FileName => "file_name",
        /// Best `QualityScore.overall` first; unscored images last.
        Quality => "quality",
        /// Most stars first; ties in capture order.
        Rating => "rating",
    }
}

/// Filter + page request for the grid. All filters are ANDed; empty lists / `null`
/// mean "no constraint". Suppressed tags never match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImageQuery {
    /// Combined with `tagMatch`.
    pub include_tags: Vec<CullTag>,
    /// Images carrying any of these are excluded.
    pub exclude_tags: Vec<CullTag>,
    pub tag_match: TagMatch,
    /// Image's pick flag is one of these (e.g. `["pick", "unflagged"]` hides rejects).
    pub picks: Vec<PickFlag>,
    /// Inclusive star range, 0..=5.
    pub min_rating: Option<u8>,
    pub max_rating: Option<u8>,
    /// Image's colour label is one of these.
    pub color_labels: Vec<ColorLabel>,
    pub burst_group_id: Option<BurstGroupId>,
    /// Only members of this scene.
    #[serde(default)]
    pub scene_id: Option<SceneId>,
    /// Hide burst members that are not their group's keeper (groups without a keeper
    /// show all members); images outside bursts are unaffected.
    pub collapse_bursts: bool,
    pub folder_id: Option<FolderId>,
    pub sort: ImageSort,
    /// Reverse the natural order of `sort` (images missing the key stay last).
    pub sort_descending: bool,
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
            picks: Vec::new(),
            min_rating: None,
            max_rating: None,
            color_labels: Vec::new(),
            burst_group_id: None,
            scene_id: None,
            collapse_bursts: false,
            folder_id: None,
            sort: ImageSort::CaptureTime,
            sort_descending: false,
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
    /// Existing XMP sidecars whose rating/pick/label were read into the catalog
    /// (new images, and unchanged images whose sidecar changed on disk).
    pub sidecars_read: u32,
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

/// Facet counts for the filter bar over one folder (or the whole catalog).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FilterCounts {
    pub total: u32,
    /// Non-suppressed tags; tags with no images are omitted.
    pub tags: Vec<TagCount>,
    pub picked: u32,
    pub rejected: u32,
    pub unflagged: u32,
    /// `ratings[n]` = images with exactly `n` stars; always 6 entries.
    pub ratings: Vec<u32>,
    pub burst_groups: u32,
    /// Burst members hidden by `ImageQuery.collapseBursts`.
    pub burst_non_keepers: u32,
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
    /// Root of the derived-file cache (`<app_cache_dir>` or `$SIEVE_CACHE`).
    /// Thumbnails/previews live in `<cacheDir>/thumbs/`.
    pub cache_dir: String,
    /// Analysis starts automatically after import / on launch (`set_auto_analyze`).
    pub auto_analyze: bool,
    /// Sidecars are written automatically after rating/pick/label/tag changes
    /// (`set_xmp_auto_sync`). Default off.
    pub xmp_auto_sync: bool,
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

// ---------------------------------------------------------------------------
// Export (Phase 6): presets, capabilities, jobs
// ---------------------------------------------------------------------------

/// Catalog row id of a user export preset; built-in presets have negative ids.
pub type ExportPresetId = i64;
/// Catalog row id of an export job (`export_jobs`).
pub type ExportJobId = i64;

string_enum! {
    /// Output file formats. `webp` / `heic` depend on encoders: check
    /// `get_export_capabilities()` before offering them.
    pub enum ExportFormatKind {
        Jpeg => "jpeg",
        Tiff => "tiff",
        Png => "png",
        Webp => "webp",
        Heic => "heic",
    }
}

impl ExportFormatKind {
    /// Lower-case file extension (without the dot) appended to expanded file names.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Tiff => "tif",
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Heic => "heic",
        }
    }
}

string_enum! {
    /// Bits per channel of the written file. On the wire: `"8"` / `"16"`.
    pub enum BitDepth {
        Eight => "8",
        Sixteen => "16",
    }
}

string_enum! {
    /// JPEG chroma subsampling. `444` keeps full colour resolution (larger files).
    pub enum ChromaSubsampling {
        Yuv444 => "444",
        Yuv422 => "422",
        Yuv420 => "420",
    }
}

string_enum! {
    pub enum TiffCompression {
        None => "none",
        Lzw => "lzw",
        /// Deflate (Adobe "ZIP").
        Zip => "zip",
    }
}

/// File format and its encoder options. Quality values are 0..=100 (Lightroom scale,
/// passed to the encoder as-is).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum ExportFormat {
    Jpeg {
        quality: u8,
        chroma_subsampling: ChromaSubsampling,
    },
    Tiff {
        bit_depth: BitDepth,
        compression: TiffCompression,
    },
    Png {
        bit_depth: BitDepth,
    },
    /// `quality` is ignored when `lossless`.
    Webp {
        quality: u8,
        lossless: bool,
    },
    /// 8-bit HEVC in a HEIF container (macOS ImageIO).
    Heic {
        quality: u8,
    },
}

impl ExportFormat {
    pub fn kind(&self) -> ExportFormatKind {
        match self {
            Self::Jpeg { .. } => ExportFormatKind::Jpeg,
            Self::Tiff { .. } => ExportFormatKind::Tiff,
            Self::Png { .. } => ExportFormatKind::Png,
            Self::Webp { .. } => ExportFormatKind::Webp,
            Self::Heic { .. } => ExportFormatKind::Heic,
        }
    }

    /// Bits per channel written (JPEG / WebP / HEIC are always 8).
    pub fn bit_depth(&self) -> BitDepth {
        match self {
            Self::Tiff { bit_depth, .. } | Self::Png { bit_depth } => *bit_depth,
            _ => BitDepth::Eight,
        }
    }
}

/// Output size. Sizes refer to the orientation-corrected image; the aspect ratio is always
/// preserved (no cropping). Rounding: the constrained edge is exact, the other rounds to nearest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum ResizeMode {
    /// Full resolution (`DevelopInfo.fullWidth x fullHeight`).
    None,
    /// Longer side = `px`.
    LongEdge { px: u32 },
    /// Shorter side = `px`.
    ShortEdge { px: u32 },
    /// Total pixels ~= `mp` x 1,000,000 (0.1..=200).
    Megapixels {
        #[specta(type = Number)]
        mp: f32,
    },
    /// Fit within a `width` x `height` box (literal: not rotated for portrait frames).
    WidthHeight { width: u32, height: u32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ResizeOptions {
    pub mode: ResizeMode,
    /// Never upscale: if the target is larger than full resolution, export full resolution.
    pub dont_enlarge: bool,
    /// Pixels per inch written to the file (EXIF/TIFF resolution, JFIF density, PNG `pHYs`),
    /// 1..=4800. Does not change pixel dimensions.
    pub resolution_ppi: u32,
}

string_enum! {
    /// Output colour space; its ICC profile is always embedded (sRGB IEC61966-2.1,
    /// Display P3, Adobe RGB (1998)).
    pub enum ExportColorSpace {
        Srgb => "srgb",
        DisplayP3 => "display_p3",
        AdobeRgb => "adobe_rgb",
    }
}

string_enum! {
    /// Lightroom output-sharpening target.
    pub enum SharpenMedia {
        Screen => "screen",
        Matte => "matte",
        Glossy => "glossy",
    }
}

string_enum! {
    pub enum SharpenAmount {
        Low => "low",
        Standard => "standard",
        High => "high",
    }
}

/// Output sharpening, applied after resizing to the output-encoded pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct OutputSharpening {
    pub media: SharpenMedia,
    pub amount: SharpenAmount,
}

string_enum! {
    /// What to do when a target file already exists on disk. Two images of one job that
    /// expand to the same name always get unique suffixes (never overwrite each other).
    pub enum CollisionPolicy {
        /// Append `-2`, `-3`, ... before the extension.
        UniqueSuffix => "unique_suffix",
        Overwrite => "overwrite",
        /// Leave the existing file; the image counts as `skipped`.
        Skip => "skip",
    }
}

/// Output file names. `template` grammar: literal text plus tokens in braces (see
/// [`parse_filename_template`]); the format's extension is appended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FileNaming {
    /// e.g. `"{filename}"`, `"Smith-Wedding-{seq:4}"`, `"{date:YYYYMMDD}_{filename}"`.
    pub template: String,
    /// First value of `{seq}` (0..=999999999).
    pub start_number: u32,
    pub collision: CollisionPolicy,
}

/// Where exported files go (`ExportSettings.subfolder` is appended in every case).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum ExportDestination {
    /// Ask at export time. Allowed in presets; the export dialog must replace it with
    /// `folder` before calling `export_images` / `plan_export` (they reject it).
    Choose,
    /// Absolute folder path; created (with the subfolder) if missing.
    Folder { path: String },
    /// Next to each RAW (its own folder).
    SourceFolder,
}

string_enum! {
    /// Which metadata is copied into exported files. Develop settings (`crs:`) and Sieve's
    /// culling tags (`Sieve|*`) are never exported. Orientation is always written as 1
    /// (pixels are rotated); the ICC profile and resolution are always present.
    pub enum MetadataInclude {
        /// EXIF of the RAW (camera, lens, exposure, capture time) + sidecar XMP/IPTC
        /// (creator, rights, title, description, rating, label; keywords per `includeKeywords`).
        All => "all",
        /// Copyright notice only (EXIF `Copyright` + `dc:rights`).
        CopyrightOnly => "copyright_only",
        /// Copyright + creator and IPTC creator contact info (`dc:creator`, EXIF `Artist`,
        /// `Iptc4xmpCore:CreatorContactInfo`).
        CopyrightAndContact => "copyright_and_contact",
        None => "none",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MetadataOptions {
    pub include: MetadataInclude,
    /// Strip GPS EXIF and IPTC location fields (only matters for `all`).
    pub remove_location: bool,
    /// Copy the sidecar's keywords (`dc:subject` / `lr:hierarchicalSubject`, minus `Sieve|*`).
    /// Only matters for `all`.
    pub include_keywords: bool,
    /// Overrides the copyright notice from the RAW/sidecar (ignored for `none`), <= 500 chars.
    pub copyright: Option<String>,
    /// Overrides the creator/artist (used by `all` and `copyright_and_contact`), <= 500 chars.
    pub creator: Option<String>,
}

/// Everything that defines an export (the body of an `ExportPreset`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExportSettings {
    pub format: ExportFormat,
    pub color_space: ExportColorSpace,
    pub resize: ResizeOptions,
    /// `null` = no output sharpening.
    pub sharpening: Option<OutputSharpening>,
    pub naming: FileNaming,
    pub destination: ExportDestination,
    /// Relative path appended to the destination (e.g. `"Smith Wedding/Web"`); `null` = none.
    /// `/`-separated; no `..`, `.`, empty components, `\` or `:`.
    pub subfolder: Option<String>,
    pub metadata: MetadataOptions,
}

impl ExportSettings {
    pub const MIN_EDGE_PX: u32 = 16;
    pub const MAX_EDGE_PX: u32 = 65_000;
    pub const MAX_TEXT: usize = 500;

    /// Returns a description of the first invalid value, if any. A `choose` destination
    /// passes (presets may store it); `export_images` / `plan_export` reject it separately.
    pub fn validate(&self) -> Result<(), String> {
        match &self.format {
            ExportFormat::Jpeg { quality, .. }
            | ExportFormat::Webp { quality, .. }
            | ExportFormat::Heic { quality }
                if *quality > 100 =>
            {
                return Err(format!("quality = {quality} is outside 0..=100"));
            }
            _ => {}
        }
        let edge = |name: &str, v: u32| {
            if (Self::MIN_EDGE_PX..=Self::MAX_EDGE_PX).contains(&v) {
                Ok(())
            } else {
                Err(format!("{name} = {v} is outside {}..={}", Self::MIN_EDGE_PX, Self::MAX_EDGE_PX))
            }
        };
        match &self.resize.mode {
            ResizeMode::None => {}
            ResizeMode::LongEdge { px } => edge("longEdge.px", *px)?,
            ResizeMode::ShortEdge { px } => edge("shortEdge.px", *px)?,
            ResizeMode::Megapixels { mp } => {
                if !(mp.is_finite() && (0.1..=200.0).contains(mp)) {
                    return Err(format!("megapixels = {mp} is outside 0.1..=200"));
                }
            }
            ResizeMode::WidthHeight { width, height } => {
                edge("width", *width)?;
                edge("height", *height)?;
            }
        }
        if !(1..=4800).contains(&self.resize.resolution_ppi) {
            return Err(format!("resolutionPpi = {} is outside 1..=4800", self.resize.resolution_ppi));
        }
        parse_filename_template(&self.naming.template)?;
        if self.naming.start_number > 999_999_999 {
            return Err("startNumber must be <= 999999999".into());
        }
        if let ExportDestination::Folder { path } = &self.destination {
            if !std::path::Path::new(path).is_absolute() {
                return Err(format!("destination folder {path:?} must be an absolute path"));
            }
        }
        if let Some(sub) = &self.subfolder {
            validate_subfolder(sub)?;
        }
        for (name, v) in [("copyright", &self.metadata.copyright), ("creator", &self.metadata.creator)] {
            if v.as_ref().is_some_and(|s| s.chars().count() > Self::MAX_TEXT) {
                return Err(format!("{name} is longer than {} characters", Self::MAX_TEXT));
            }
        }
        Ok(())
    }
}

fn validate_subfolder(sub: &str) -> Result<(), String> {
    let bad = |why: &str| Err(format!("subfolder {sub:?}: {why}"));
    if sub.is_empty() || sub.len() > 255 {
        return bad("must be 1..=255 bytes");
    }
    if sub.starts_with('/') || sub.contains('\\') || sub.contains(':') {
        return bad("must be a relative path using '/'");
    }
    for part in sub.split('/') {
        if part.trim().is_empty() || part == "." || part == ".." || part.chars().any(char::is_control) {
            return bad("empty, '.', '..' or control characters in a component");
        }
    }
    Ok(())
}

/// A parsed piece of a file-name template (not on the wire).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplatePart {
    /// Custom text, copied verbatim.
    Text(String),
    /// `{filename}`: the RAW's file name without extension (`DSC01234`).
    Filename,
    /// `{seq}` / `{seq:N}`: 0-based position in the export's `ids` + `startNumber`, zero-padded
    /// to `N` digits (1..=9; `{seq}` = no padding).
    Seq { digits: u8 },
    /// `{date}` / `{date:FMT}`: capture wall-clock time (the RAW's mtime, local, if unknown).
    /// `FMT` = `YYYY`, `YY`, `MM`, `DD`, `hh`, `mm`, `ss` and `-`, `_`, `.`, space; default `YYYYMMDD`.
    Date { format: String },
    /// `{rating}`: stars 0..=5.
    Rating,
    /// `{camera}`: camera model (else make; `Unknown`).
    Camera,
    /// `{folder}`: name of the RAW's parent folder.
    Folder,
    /// `{id}`: catalog image id.
    Id,
}

/// Token names accepted in file-name templates (mirrored for UI help in `src/ipc/index.ts`).
pub const FILENAME_TOKENS: &[&str] = &["filename", "seq", "date", "rating", "camera", "folder", "id"];

/// Parses and validates a file-name template: 1..=200 chars, not blank; literal text must not
/// contain `/`, `\`, `:`, unmatched braces or control characters. Expanded token values are
/// sanitized by the exporter (`/ \ :` and control characters -> `_`), a leading `.` or space is
/// stripped from the final name, and names are truncated to 240 bytes before the extension.
pub fn parse_filename_template(template: &str) -> Result<Vec<TemplatePart>, String> {
    if template.trim().is_empty() || template.chars().count() > 200 {
        return Err("file name template must be 1..=200 characters".into());
    }
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut rest = template;
    while let Some(c) = rest.chars().next() {
        match c {
            '{' => {
                let end = rest.find('}').ok_or_else(|| format!("unclosed '{{' in template {template:?}"))?;
                if !text.is_empty() {
                    parts.push(TemplatePart::Text(std::mem::take(&mut text)));
                }
                parts.push(parse_template_token(&rest[1..end])?);
                rest = &rest[end + 1..];
            }
            '}' => return Err(format!("unmatched '}}' in template {template:?}")),
            '/' | '\\' | ':' => return Err(format!("{c:?} is not allowed in file names")),
            c if c.is_control() => return Err("control characters are not allowed in file names".into()),
            c => {
                text.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    if !text.is_empty() {
        parts.push(TemplatePart::Text(text));
    }
    Ok(parts)
}

fn parse_template_token(inner: &str) -> Result<TemplatePart, String> {
    let (name, arg) = match inner.split_once(':') {
        Some((n, a)) => (n, Some(a)),
        None => (inner, None),
    };
    let plain = |part: TemplatePart| match arg {
        None => Ok(part),
        Some(_) => Err(format!("token {{{name}}} takes no argument")),
    };
    match name {
        "filename" => plain(TemplatePart::Filename),
        "rating" => plain(TemplatePart::Rating),
        "camera" => plain(TemplatePart::Camera),
        "folder" => plain(TemplatePart::Folder),
        "id" => plain(TemplatePart::Id),
        "seq" => {
            let digits = match arg {
                None => 1,
                Some(a) => a
                    .parse::<u8>()
                    .ok()
                    .filter(|d| (1..=9).contains(d))
                    .ok_or_else(|| format!("{{seq:{a}}}: digits must be 1..=9"))?,
            };
            Ok(TemplatePart::Seq { digits })
        }
        "date" => {
            let format = arg.unwrap_or("YYYYMMDD");
            if format.is_empty() {
                return Err("{date:}: empty format".into());
            }
            let mut rest = format;
            while !rest.is_empty() {
                if let Some(t) = ["YYYY", "YY", "MM", "DD", "hh", "mm", "ss"].iter().find(|t| rest.starts_with(**t)) {
                    rest = &rest[t.len()..];
                } else if rest.starts_with(['-', '_', '.', ' ']) {
                    rest = &rest[1..];
                } else {
                    return Err(format!("{{date:{format}}}: use YYYY YY MM DD hh mm ss and - _ . space"));
                }
            }
            Ok(TemplatePart::Date { format: format.to_string() })
        }
        _ => Err(format!("unknown token {{{name}}} (known: {})", FILENAME_TOKENS.join(", "))),
    }
}

/// A named export configuration. Built-in presets (`builtIn`, negative ids) are read-only:
/// "save as" creates a user preset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExportPreset {
    pub id: ExportPresetId,
    /// Unique case-insensitively across built-in and user presets, 1..=100 chars (trimmed).
    pub name: String,
    pub built_in: bool,
    pub settings: ExportSettings,
    /// 0 for built-ins.
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl ExportPreset {
    /// The built-in presets, in display order (listed before user presets).
    pub fn builtins() -> Vec<ExportPreset> {
        let metadata_all = MetadataOptions {
            include: MetadataInclude::All,
            remove_location: false,
            include_keywords: true,
            copyright: None,
            creator: None,
        };
        let naming =
            FileNaming { template: "{filename}".into(), start_number: 1, collision: CollisionPolicy::UniqueSuffix };
        let preset = |id: ExportPresetId, name: &str, settings: ExportSettings| ExportPreset {
            id,
            name: name.into(),
            built_in: true,
            settings,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        vec![
            preset(
                -1,
                "Client JPEG full-res sRGB q90",
                ExportSettings {
                    format: ExportFormat::Jpeg { quality: 90, chroma_subsampling: ChromaSubsampling::Yuv444 },
                    color_space: ExportColorSpace::Srgb,
                    resize: ResizeOptions { mode: ResizeMode::None, dont_enlarge: true, resolution_ppi: 300 },
                    sharpening: None,
                    naming: naming.clone(),
                    destination: ExportDestination::Choose,
                    subfolder: None,
                    metadata: metadata_all.clone(),
                },
            ),
            preset(
                -2,
                "Web 2048 sRGB",
                ExportSettings {
                    format: ExportFormat::Jpeg { quality: 80, chroma_subsampling: ChromaSubsampling::Yuv420 },
                    color_space: ExportColorSpace::Srgb,
                    resize: ResizeOptions {
                        mode: ResizeMode::LongEdge { px: 2048 },
                        dont_enlarge: true,
                        resolution_ppi: 72,
                    },
                    sharpening: Some(OutputSharpening { media: SharpenMedia::Screen, amount: SharpenAmount::Standard }),
                    naming: naming.clone(),
                    destination: ExportDestination::Choose,
                    subfolder: None,
                    metadata: MetadataOptions {
                        include: MetadataInclude::CopyrightOnly,
                        remove_location: true,
                        include_keywords: false,
                        copyright: None,
                        creator: None,
                    },
                },
            ),
            preset(
                -3,
                "Print TIFF 16-bit Adobe RGB",
                ExportSettings {
                    format: ExportFormat::Tiff { bit_depth: BitDepth::Sixteen, compression: TiffCompression::Lzw },
                    color_space: ExportColorSpace::AdobeRgb,
                    resize: ResizeOptions { mode: ResizeMode::None, dont_enlarge: true, resolution_ppi: 300 },
                    sharpening: Some(OutputSharpening { media: SharpenMedia::Glossy, amount: SharpenAmount::Standard }),
                    naming,
                    destination: ExportDestination::Choose,
                    subfolder: None,
                    metadata: metadata_all,
                },
            ),
        ]
    }
}

/// Encoder availability for one format in this build / on this machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExportFormatInfo {
    pub kind: ExportFormatKind,
    pub available: bool,
    /// Why it is unavailable (e.g. "HEIC encoder not available"); `null` when available.
    pub reason: Option<String>,
    /// Bit depths the encoder writes (JPEG/WebP/HEIC `["8"]`, TIFF/PNG `["8", "16"]`).
    pub bit_depths: Vec<BitDepth>,
    /// EXIF/XMP metadata can be embedded. ICC profiles are embedded for every available format.
    pub supports_metadata: bool,
}

/// What the export engine can do here (`get_export_capabilities`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExportCapabilities {
    /// One entry per `ExportFormatKind`, in enum order.
    pub formats: Vec<ExportFormatInfo>,
    /// Upper bound of images developed concurrently (see `docs/architecture.md`, "Export").
    pub max_parallel: u32,
    /// Memory budget shared by concurrently developed images, MiB.
    pub memory_budget_mb: u32,
}

/// One file an export would write (`plan_export`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PlannedFile {
    pub image_id: ImageId,
    /// Final absolute path after the collision policy; `null` when it would be skipped.
    pub path: Option<String>,
    /// The template's path already exists on disk (before the collision policy).
    pub exists: bool,
}

/// Dry run of an export: resolved paths, no pixels. Disk state may change before it runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExportPlan {
    /// Resolved destination incl. subfolder; `null` for `source_folder` (one per RAW folder).
    pub output_dir: Option<String>,
    /// In `ids` order.
    pub files: Vec<PlannedFile>,
    /// How many `files` have `exists`.
    pub existing: u32,
}

string_enum! {
    pub enum ExportJobState {
        /// Waiting for an earlier job (jobs run one at a time, in order).
        Queued => "queued",
        Running => "running",
        /// Every image was processed (some may have failed or been skipped).
        Completed => "completed",
        /// Stopped by `cancel_export`; files already written are kept.
        Cancelled => "cancelled",
        /// The app quit while the job was queued/running (not resumed).
        Interrupted => "interrupted",
    }
}

/// An image that could not be exported.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExportFailure {
    pub image_id: ImageId,
    /// Source RAW file name (for display).
    pub file_name: String,
    pub reason: String,
}

/// An export job (live or from history).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExportJob {
    pub id: ExportJobId,
    pub state: ExportJobState,
    /// Label passed to `export_images` (usually the preset name).
    pub preset_name: Option<String>,
    pub format: ExportFormatKind,
    pub total: u32,
    /// `succeeded + failed + skipped`.
    pub done: u32,
    pub succeeded: u32,
    pub failed: u32,
    /// Existing files left alone (`collision = skip`).
    pub skipped: u32,
    /// Resolved destination incl. subfolder; `null` for `source_folder`.
    pub output_dir: Option<String>,
    pub failures: Vec<ExportFailure>,
    pub created_at_ms: i64,
    pub finished_at_ms: Option<i64>,
}

// ---------------------------------------------------------------------------
// Scenes & scene matching (Phase 7)
// ---------------------------------------------------------------------------

string_enum! {
    /// How a scene's membership was decided.
    pub enum SceneMethod {
        /// Created by `detect_scenes` and not edited since; replaced by the next detection.
        Auto => "auto",
        /// Created or edited by the user (`create_scene`, `set_scene_members`, `merge_scenes`,
        /// `split_scene`); kept by `detect_scenes` unless `replaceManual`.
        Manual => "manual",
    }
}

string_enum! {
    /// Long-running scene operation reported by the `sceneProgress` event.
    pub enum SceneTask {
        Detect => "detect",
        Match => "match",
    }
}

/// A lighting scenario: consecutive frames shot under the same light, graded together from
/// 1-2 anchors. An image belongs to at most one scene.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Scene {
    pub id: SceneId,
    /// Folder of all members; `null` if they span folders (manual scenes only).
    pub folder_id: Option<FolderId>,
    /// Earliest / latest member capture time (`null` if no member has one).
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
    /// Members in capture order (images without a capture time last, by file name). Never empty.
    pub image_ids: Vec<ImageId>,
    /// Graded reference frames (0..=`Scene::MAX_ANCHORS`), a subset of `imageIds`, in capture order.
    pub anchor_ids: Vec<ImageId>,
    pub method: SceneMethod,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl Scene {
    /// At most this many anchors per scene / per `match_scene` call.
    pub const MAX_ANCHORS: usize = 2;
}

/// Parameters of `detect_scenes`. `null` on the wire = `SceneDetectOptions::default()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SceneDetectOptions {
    /// A capture-time gap longer than this always starts a new scene. 1000..=86_400_000 ms.
    pub max_gap_ms: u32,
    /// 0..=1. Minimum appearance similarity (tone + colour features of the preview) for a frame
    /// to stay in the running scene; lower = fewer, larger scenes. 0 = split on time gaps only.
    #[specta(type = Number)]
    pub similarity: f32,
    /// Also replace manual scenes in scope (default: they and their members are left alone).
    pub replace_manual: bool,
}

impl Default for SceneDetectOptions {
    fn default() -> Self {
        Self { max_gap_ms: 120_000, similarity: 0.7, replace_manual: false }
    }
}

impl SceneDetectOptions {
    pub fn validate(&self) -> Result<(), String> {
        if !(1000..=86_400_000).contains(&self.max_gap_ms) {
            return Err(format!("maxGapMs = {} is outside 1000..=86400000", self.max_gap_ms));
        }
        if !(self.similarity.is_finite() && (0.0..=1.0).contains(&self.similarity)) {
            return Err(format!("similarity = {} is outside 0..=1", self.similarity));
        }
        Ok(())
    }
}

/// Parameters of `match_scene`. Relative grading: every target starts from the anchor's
/// look (`base`) and gets corrections (`delta`) that cancel measured differences in
/// brightness / white point / tonal range, so it renders like the anchor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchOptions {
    /// Correct `exposure` so the target's rendered brightness matches the anchor's.
    pub match_exposure: bool,
    /// Correct temperature/tint so the target's rendered neutral matches the anchor's
    /// (the result is always `custom` white balance).
    pub match_white_balance: bool,
    /// Small `contrast` / `whites` / `blacks` corrections from luminance percentiles.
    pub match_tone: bool,
    /// 0..=1: share of the correction applied (`adjustments = lerp(base, full, strength)`);
    /// 0 = the anchor's settings copied verbatim (like Sync Settings).
    #[specta(type = Number)]
    pub strength: f32,
    /// Groups copied from the anchor into `base` (the target keeps its own values for the rest).
    /// The groups a `match*` flag corrects are always taken from the anchor.
    pub copy_fields: Vec<AdjustmentField>,
}

impl Default for MatchOptions {
    fn default() -> Self {
        Self {
            match_exposure: true,
            match_white_balance: true,
            match_tone: false,
            strength: 1.0,
            copy_fields: AdjustmentField::ALL.to_vec(),
        }
    }
}

impl MatchOptions {
    /// At most this many targets per `match_scene` call.
    pub const MAX_TARGETS: usize = 2000;

    pub fn validate(&self) -> Result<(), String> {
        if !(self.strength.is_finite() && (0.0..=1.0).contains(&self.strength)) {
            return Err(format!("strength = {} is outside 0..=1", self.strength));
        }
        Ok(())
    }

    /// Groups corrected by the enabled `match*` flags.
    pub fn matched_fields(&self) -> Vec<AdjustmentField> {
        let mut out = Vec::new();
        if self.match_white_balance {
            out.push(AdjustmentField::WhiteBalance);
        }
        if self.match_exposure {
            out.push(AdjustmentField::Exposure);
        }
        if self.match_tone {
            out.extend([AdjustmentField::Contrast, AdjustmentField::Whites, AdjustmentField::Blacks]);
        }
        out
    }
}

/// Relative luminance percentiles (linear, 0..=1) of a rendered image.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LumaPercentiles {
    #[specta(type = Number)]
    pub p1: f32,
    #[specta(type = Number)]
    pub p10: f32,
    #[specta(type = Number)]
    pub p50: f32,
    #[specta(type = Number)]
    pub p90: f32,
    #[specta(type = Number)]
    pub p99: f32,
}

/// A colour in Oklab (L 0..=1; a/b ~ -0.4..=0.4, 0 = neutral).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct OklabColor {
    #[specta(type = Number)]
    pub l: f32,
    #[specta(type = Number)]
    pub a: f32,
    #[specta(type = Number)]
    pub b: f32,
}

/// Estimated colour of neutral surfaces in a *rendered* image (render space: sRGB/D65).
/// A perfectly balanced render has `a = b = 0` (xy = D65 0.3127, 0.3290).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NeutralEstimate {
    /// CIE 1931 chromaticity of the estimate.
    #[specta(type = Number)]
    pub x: f32,
    #[specta(type = Number)]
    pub y: f32,
    /// Oklab a/b of the estimate (colour cast direction and size).
    #[specta(type = Number)]
    pub a: f32,
    #[specta(type = Number)]
    pub b: f32,
    /// 0..=1 share of pixels the estimate is based on; 0 = grey-world fallback (whole frame).
    #[specta(type = Number)]
    pub coverage: f32,
}

/// Measurements of an image rendered through the develop pipeline with given adjustments
/// (the same pixels the editor shows, LUT included), long edge `scene::STATS_MAX_EDGE`. The
/// basis of scene matching and its acceptance checks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImageStats {
    pub image_id: ImageId,
    /// Measured area (oriented, normalized); `null` = whole frame.
    pub region: Option<NormRect>,
    /// Pixel size measured.
    pub width: u32,
    pub height: u32,
    /// Mean relative luminance (Rec.709 Y of the linearized sRGB output), 0..=1.
    #[specta(type = Number)]
    pub mean_luma: f32,
    /// log2 of the geometric mean luminance (Y floored at 2^-14). Exposure differences in EV
    /// are differences of this value.
    #[specta(type = Number)]
    pub log_mean_luma: f32,
    pub percentiles: LumaPercentiles,
    /// Share of pixels with any 8-bit channel >= 254.
    #[specta(type = Number)]
    pub clipped_highlights: f32,
    /// Share of pixels with all 8-bit channels <= 1.
    #[specta(type = Number)]
    pub clipped_shadows: f32,
    /// Mean colour of the render.
    pub mean_oklab: OklabColor,
    pub neutral: NeutralEstimate,
    /// White balance the measured adjustments resolve to (`as_shot` -> the camera's values);
    /// `null` if as-shot and the file has none.
    pub white_balance: Option<WhiteBalanceValues>,
    /// The camera's as-shot white balance (as in `DevelopInfo.asShot`).
    pub as_shot: Option<WhiteBalanceValues>,
    /// The adjustments' LUT is missing from the library (measured without it).
    pub lut_missing: bool,
}

/// `full - base` of the corrected sliders (0 for groups not matched).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchDelta {
    /// EV.
    #[specta(type = Number)]
    pub exposure: f32,
    /// Kelvin (slider units).
    #[specta(type = Number)]
    pub temperature_k: f32,
    #[specta(type = Number)]
    pub tint: f32,
    #[specta(type = Number)]
    pub contrast: f32,
    #[specta(type = Number)]
    pub whites: f32,
    #[specta(type = Number)]
    pub blacks: f32,
}

/// Proposed grade for one target of `match_scene` (nothing is saved).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchPreview {
    pub target_id: ImageId,
    /// Anchor(s) this target was matched to: one, or both when the target lies between two
    /// anchors in capture time (blended).
    pub anchor_ids: Vec<ImageId>,
    /// Weight of `anchorIds[1]` when blended (0..=1), else 0.
    #[specta(type = Number)]
    pub anchor_weight: f32,
    /// Strength 0: the target's settings with the anchor's groups copied (white balance
    /// resolved to `custom` when matched).
    pub base: ParametricAdjustments,
    /// Strength 1: `base` plus the full correction.
    pub full: ParametricAdjustments,
    /// `ParametricAdjustments::lerp(base, full, options.strength)`; what `apply_scene_match`
    /// should receive unless the UI changes the strength (then use `lerpAdjustments`).
    pub adjustments: ParametricAdjustments,
    pub delta: MatchDelta,
    /// The anchor rendered with its own settings (blended when two anchors).
    pub reference: ImageStats,
    /// The target rendered with its current (stored) settings.
    pub before: ImageStats,
    /// The target rendered with `adjustments`.
    pub predicted: ImageStats,
    /// At strength 1 the target lands within `scene::TOLERANCE_EV` / `scene::TOLERANCE_AB`
    /// of the reference.
    pub converged: bool,
    /// Human-readable caveats (clamped slider, no neutral found, LUT missing...).
    pub notes: Vec<String>,
}

/// One image's adjustments to commit via `apply_scene_match`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchApplication {
    pub image_id: ImageId,
    pub adjustments: ParametricAdjustments,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_fields_copies_only_selected_groups() {
        let mut src = ParametricAdjustments {
            exposure: 1.0,
            contrast: 20.0,
            white_balance: WhiteBalance::Custom { temperature_k: 3200.0, tint: 5.0 },
            lut: Some(LutRef { id: "film-1".into(), amount: 50.0 }),
            ..Default::default()
        };
        src.hsl.hue.red = 10.0;
        src.hsl.luminance.blue = -30.0;

        let mut dst = ParametricAdjustments { shadows: 40.0, ..Default::default() };
        dst.copy_fields(&src, &[AdjustmentField::Exposure, AdjustmentField::HslHue, AdjustmentField::Lut]);
        assert_eq!(dst.exposure, 1.0);
        assert_eq!(dst.contrast, 0.0);
        assert_eq!(dst.shadows, 40.0);
        assert_eq!(dst.white_balance, WhiteBalance::AsShot);
        assert_eq!(dst.hsl.hue.red, 10.0);
        assert_eq!(dst.hsl.luminance.blue, 0.0);
        assert_eq!(dst.lut, src.lut);

        let mut all = ParametricAdjustments::default();
        all.copy_fields(&src, AdjustmentField::ALL);
        assert_eq!(all, src);
        assert!(!all.is_neutral());
        assert!(ParametricAdjustments::default().is_neutral());
    }

    #[test]
    fn lut_ids_and_render_options_validate() {
        assert!(is_valid_lut_id("kodak-portra-400-3f2a91c0"));
        let long = "x".repeat(65);
        for bad in ["", "Upper", "a/b", "../x", "a.cube", long.as_str()] {
            assert!(!is_valid_lut_id(bad), "{bad}");
        }
        let adj =
            ParametricAdjustments { lut: Some(LutRef { id: "../etc".into(), amount: 10.0 }), ..Default::default() };
        assert!(adj.validate().is_err());

        let ok = RenderOptions { max_edge: 2048, slot: RenderSlot::Main, region: None };
        assert!(ok.validate().is_ok());
        assert!(RenderOptions { max_edge: 10, ..ok.clone() }.validate().is_err());
        let region = NormRect { x: 0.5, y: 0.5, width: 0.6, height: 0.2 };
        assert!(RenderOptions { region: Some(region), ..ok.clone() }.validate().is_err());
        let region = NormRect { x: 0.25, y: 0.25, width: 0.5, height: 0.5 };
        assert!(RenderOptions { region: Some(region), ..ok }.validate().is_ok());
    }

    #[test]
    fn filename_templates_parse() {
        assert_eq!(
            parse_filename_template("Smith {date:YYYY-MM-DD}_{seq:4}-{filename}").unwrap(),
            vec![
                TemplatePart::Text("Smith ".into()),
                TemplatePart::Date { format: "YYYY-MM-DD".into() },
                TemplatePart::Text("_".into()),
                TemplatePart::Seq { digits: 4 },
                TemplatePart::Text("-".into()),
                TemplatePart::Filename,
            ]
        );
        assert_eq!(parse_filename_template("{date}").unwrap(), vec![TemplatePart::Date { format: "YYYYMMDD".into() }]);
        assert_eq!(parse_filename_template("{seq}").unwrap(), vec![TemplatePart::Seq { digits: 1 }]);
        for t in ["{rating}{camera}{folder}{id}", "Ünïcödé name"] {
            assert!(parse_filename_template(t).is_ok(), "{t}");
        }
        for bad in ["", "   ", "{nope}", "{filename", "a}b", "a/b", "a:b", "{seq:0}", "{seq:10}", "{date:Q}", "{id:1}"]
        {
            assert!(parse_filename_template(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn export_settings_validate_and_builtins_roundtrip() {
        let builtins = ExportPreset::builtins();
        assert_eq!(builtins.len(), 3);
        for p in &builtins {
            assert!(p.built_in && p.id < 0);
            p.settings.validate().unwrap();
            let json = serde_json::to_string(&p.settings).unwrap();
            assert_eq!(serde_json::from_str::<ExportSettings>(&json).unwrap(), p.settings);
        }
        let json = serde_json::to_value(&builtins[2].settings).unwrap();
        assert_eq!(json["format"], serde_json::json!({ "kind": "tiff", "bitDepth": "16", "compression": "lzw" }));
        assert_eq!(json["colorSpace"], "adobe_rgb");
        assert_eq!(json["destination"], serde_json::json!({ "kind": "choose" }));

        let base = builtins[0].settings.clone();
        let with = |f: &dyn Fn(&mut ExportSettings)| {
            let mut s = base.clone();
            f(&mut s);
            s.validate()
        };
        assert!(with(
            &|s| s.format = ExportFormat::Jpeg { quality: 101, chroma_subsampling: ChromaSubsampling::Yuv420 }
        )
        .is_err());
        assert!(with(&|s| s.resize.mode = ResizeMode::LongEdge { px: 8 }).is_err());
        assert!(with(&|s| s.resize.mode = ResizeMode::Megapixels { mp: f32::NAN }).is_err());
        assert!(with(&|s| s.resize.mode = ResizeMode::WidthHeight { width: 1920, height: 1080 }).is_ok());
        assert!(with(&|s| s.resize.resolution_ppi = 0).is_err());
        assert!(with(&|s| s.destination = ExportDestination::Folder { path: "relative/dir".into() }).is_err());
        assert!(with(&|s| s.destination = ExportDestination::Folder { path: "/tmp/out".into() }).is_ok());
        for bad in ["", "/abs", "a/../b", "a//b", "./a", "a\\b"] {
            assert!(with(&|s| s.subfolder = Some(bad.into())).is_err(), "{bad}");
        }
        assert!(with(&|s| s.subfolder = Some("Smith Wedding/Web".into())).is_ok());
        assert!(with(&|s| s.metadata.copyright = Some("x".repeat(501))).is_err());
        assert_eq!(ExportFormatKind::Tiff.extension(), "tif");
        assert_eq!(base.format.bit_depth(), BitDepth::Eight);
    }

    #[test]
    fn lerp_adjustments_semantics() {
        let a = ParametricAdjustments {
            exposure: 0.0,
            contrast: 10.0,
            white_balance: WhiteBalance::Custom { temperature_k: 4000.0, tint: 0.0 },
            lut: Some(LutRef { id: "film".into(), amount: 0.0 }),
            ..Default::default()
        };
        let mut b = ParametricAdjustments {
            exposure: 1.0,
            contrast: 30.0,
            white_balance: WhiteBalance::Custom { temperature_k: 8000.0, tint: 10.0 },
            lut: Some(LutRef { id: "film".into(), amount: 100.0 }),
            ..Default::default()
        };
        b.hsl.hue.red = 20.0;
        assert_eq!(ParametricAdjustments::lerp(&a, &b, 0.0), a);
        assert_eq!(ParametricAdjustments::lerp(&a, &b, 1.0), b);
        let m = ParametricAdjustments::lerp(&a, &b, 0.5);
        assert_eq!((m.exposure, m.contrast, m.hsl.hue.red), (0.5, 20.0, 10.0));
        // Mired midpoint of 4000 K (250) and 8000 K (125) = 187.5 mired = 5333.3 K.
        let WhiteBalance::Custom { temperature_k, tint } = m.white_balance else { panic!() };
        assert!((temperature_k - 5333.333).abs() < 0.01 && tint == 5.0, "{temperature_k}");
        assert_eq!(m.lut, Some(LutRef { id: "film".into(), amount: 50.0 }));
        // Clamped t; mixed WB modes / different LUTs take the nearer side.
        assert_eq!(ParametricAdjustments::lerp(&a, &b, 7.0), b);
        let c = ParametricAdjustments { white_balance: WhiteBalance::AsShot, lut: None, ..b.clone() };
        assert_eq!(ParametricAdjustments::lerp(&a, &c, 0.4).white_balance, a.white_balance);
        assert_eq!(ParametricAdjustments::lerp(&a, &c, 0.6).lut, None);
        assert!(ParametricAdjustments::lerp(&a, &b, 0.37).validate().is_ok());
    }

    #[test]
    fn scene_options_validate_and_wire_format() {
        assert!(SceneDetectOptions::default().validate().is_ok());
        assert!(SceneDetectOptions { max_gap_ms: 10, ..Default::default() }.validate().is_err());
        assert!(SceneDetectOptions { similarity: 1.5, ..Default::default() }.validate().is_err());
        let m = MatchOptions::default();
        assert!(m.validate().is_ok());
        assert_eq!(
            m.matched_fields(),
            vec![AdjustmentField::WhiteBalance, AdjustmentField::Exposure],
            "tone matching is off by default"
        );
        assert!(MatchOptions { strength: -0.1, ..Default::default() }.validate().is_err());
        assert!(MatchOptions { strength: f32::NAN, ..Default::default() }.validate().is_err());
        let json = serde_json::to_value(&m).unwrap();
        assert_eq!(json["matchWhiteBalance"], true);
        assert_eq!(json["copyFields"].as_array().unwrap().len(), AdjustmentField::ALL.len());
        assert_eq!(serde_json::to_value(SceneMethod::Manual).unwrap(), "manual");
        let q: ImageQuery = serde_json::from_value(serde_json::json!({
            "includeTags": [], "excludeTags": [], "tagMatch": "any", "picks": [], "minRating": null,
            "maxRating": null, "colorLabels": [], "burstGroupId": null, "collapseBursts": false,
            "folderId": null, "sort": "capture_time", "sortDescending": false, "offset": 0, "limit": 10
        }))
        .unwrap();
        assert_eq!(q.scene_id, None, "sceneId may be omitted by older callers");
    }
}
