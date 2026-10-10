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
/// Catalog row id of a style group (preset/profile library folder, IPC v14).
pub type StyleGroupId = i64;
/// Catalog row id of a profile in the style library (IPC v14).
pub type StyleProfileId = i64;
/// Catalog row id of an undoable multi-image edit (IPC v14).
pub type EditBatchId = i64;
/// Catalog row id of a project (one shoot: its source folder(s), IPC v14).
pub type ProjectId = i64;

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
pub(crate) use string_enum;

// Masks / local adjustments (Phase 7c, IPC v10) live in `ipc/masks.rs`.
pub use super::masks::*;
// Target-count culling (Phase 9, IPC v20) lives in `ipc/target.rs`.
pub use super::target::*;

// ---------------------------------------------------------------------------
// Camera / file identity
// ---------------------------------------------------------------------------

string_enum! {
    /// Supported file formats: RAW containers in pipeline priority order, then non-RAW
    /// ("raster", display-referred) sources (Phase 7b). Renamed from `RawFormat` in IPC v9
    /// (same wire values for the RAW variants).
    pub enum ImageFormat {
        /// Sony ARW (TIFF-based).
        Arw => "arw",
        /// Fujifilm RAF (X-Trans or Bayer).
        Raf => "raf",
        /// Canon CR3 (ISO-BMFF based).
        Cr3 => "cr3",
        /// JPEG (`.jpg`, `.jpeg`, `.jpe`), incl. camera JPEGs next to RAWs.
        Jpeg => "jpeg",
        /// HEIF/HEIC (`.heic`, `.heif`, `.hif`: Canon/Fuji/iPhone HEIF).
        Heic => "heic",
        /// TIFF (`.tif`, `.tiff`), 8/16-bit.
        Tiff => "tiff",
        /// PNG, 8/16-bit.
        Png => "png",
    }
}

/// Pre-v9 name of [`ImageFormat`], kept for Rust call sites.
pub type RawFormat = ImageFormat;

impl ImageFormat {
    /// The RAW formats (LibRaw demosaic; `<stem>.xmp` sidecars).
    pub const RAW: &'static [ImageFormat] = &[ImageFormat::Arw, ImageFormat::Raf, ImageFormat::Cr3];

    /// `true` for camera RAW containers, `false` for raster (display-referred) sources.
    pub fn is_raw(self) -> bool {
        Self::RAW.contains(&self)
    }

    /// Maps a file extension (case-insensitive, without the dot) to a format.
    pub fn from_extension(ext: &str) -> Option<ImageFormat> {
        match ext.to_ascii_lowercase().as_str() {
            "arw" => Some(ImageFormat::Arw),
            "raf" => Some(ImageFormat::Raf),
            "cr3" => Some(ImageFormat::Cr3),
            "jpg" | "jpeg" | "jpe" => Some(ImageFormat::Jpeg),
            "heic" | "heif" | "hif" => Some(ImageFormat::Heic),
            "tif" | "tiff" => Some(ImageFormat::Tiff),
            "png" => Some(ImageFormat::Png),
            _ => None,
        }
    }

    /// Non-RAW formats that become the companion of a same-stem RAW in the same directory
    /// (`ImportOptions.pairJpegWithRaw`). TIFF/PNG are usually derived files and never pair.
    pub fn pairs_with_raw(self) -> bool {
        matches!(self, ImageFormat::Jpeg | ImageFormat::Heic)
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
    /// v19.2: body serial number from EXIF (`BodySerialNumber`, else DNG `CameraSerialNumber`),
    /// read at import / thumbnail re-extraction and backfilled in the background for photos
    /// imported before v19.2 (`null` until then, or when the file has none). Tells two bodies
    /// of the same model apart (`CameraBody`, Edit Capture Time > sync cameras).
    #[serde(default)]
    pub serial: Option<String>,
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
    ///
    /// v19: this is the **corrected** capture time (Lightroom's "Edit Capture Time"), used by
    /// sorting, burst grouping, scenes, filters, export naming and project date ranges. It
    /// equals `originalCapturedAtMs` unless `captureTimeSource` is `sidecar` or `user`.
    pub captured_at_ms: Option<i64>,
    /// v19: capture time read from the file itself (EXIF `DateTimeOriginal` + sub-seconds),
    /// same "naive" ms convention. Never changed by `edit_capture_time` or sidecar reads.
    /// `null` when the file has none (or its metadata was not extracted yet).
    #[serde(default)]
    pub original_captured_at_ms: Option<i64>,
    /// v19: where `capturedAtMs` comes from.
    #[serde(default)]
    pub capture_time_source: CaptureTimeSource,
    pub iso: Option<u32>,
    #[specta(type = Option<Number>)]
    pub shutter_seconds: Option<f64>,
    #[specta(type = Option<Number>)]
    pub aperture: Option<f32>,
    #[specta(type = Option<Number>)]
    pub focal_length_mm: Option<f32>,
    pub lens: Option<String>,
}

string_enum! {
    /// Origin of `CaptureMeta.capturedAtMs` (IPC v19, `images.capture_time_source`).
    #[derive(Default)]
    pub enum CaptureTimeSource {
        /// The file's own EXIF time (`originalCapturedAtMs`).
        #[default]
        Exif => "exif",
        /// A corrected time read from the XMP sidecar (`exif:DateTimeOriginal`, else
        /// `photoshop:DateCreated`) that differs from the file's EXIF time, e.g. after
        /// Lightroom's "Edit Capture Time".
        Sidecar => "sidecar",
        /// Corrected in Sieve (`edit_capture_time`); written to the sidecar.
        User => "user",
    }
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
    /// Engine's suggested flag. Only unrecoverable defects suggest `reject`; burst non-keepers are
    /// capped below the keeper (never `pick`) but not rejected.
    /// Never written to `RawImageEntry.pick` except through `apply_suggestions`.
    pub suggested_pick: PickFlag,
    /// Why the engine suggests what it does, most important first (v18; filled by the culling
    /// engine for every non-`pick` suggestion and every auto tag; may be empty, e.g. for
    /// scores written before v18 until the next analysis / rescore). Stored as JSON with the
    /// score (`quality_scores.reasons_json`).
    pub reasons: Vec<SuggestionReason>,
}

string_enum! {
    /// Category of a [`SuggestionReason`] (v18). Tag-like kinds match the `CullTag` of the
    /// same name.
    pub enum SuggestionReasonKind {
        /// Eyes closed (`blink`).
        Blink => "blink",
        /// The face (or, without a usable face, the frame) is not sharp (`missed_focus`).
        MissedFocus => "missed_focus",
        MotionBlur => "motion_blur",
        /// Intentional blur, not a defect (`creative_blur`).
        CreativeBlur => "creative_blur",
        Underexposed => "underexposed",
        Overexposed => "overexposed",
        /// A better frame of the same burst exists (`relatedImageId` = the burst keeper).
        DuplicateBurst => "duplicate_burst",
        /// Low overall score without a single dominant defect.
        LowScore => "low_score",
        /// Anything else (the text says what).
        Other => "other",
    }
}

/// One human-readable reason behind a suggestion (v18), e.g.
/// `{kind: "blink", text: "Eyes closed"}` or
/// `{kind: "duplicate_burst", text: "Duplicate in burst (keeper DSC0123)", relatedImageId: 42}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionReason {
    pub kind: SuggestionReasonKind,
    /// Short, user-facing, sentence case, no trailing period.
    pub text: String,
    /// Another image the reason refers to (the burst keeper for `duplicate_burst`).
    #[serde(default)]
    pub related_image_id: Option<ImageId>,
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
    /// Share of a subject face's skin that is blown (every channel >= 250) above which the frame is
    /// `overexposed`; a frame more than 60% blown also counts. 0..=1.
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
    /// Re-measure every image of this project (v14).
    Project { project_id: ProjectId },
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
    /// Who set an image's pick / reject flag (v18; `images.pick_origin`).
    pub enum PickOrigin {
        /// The user: flag commands, undo/redo of the user's flags, sidecar reads (a flag
        /// found in an XMP file counts as a person's decision). Flags set before v18 count as
        /// `user`.
        User => "user",
        /// `apply_suggestions` ("Auto"), not changed by the user since.
        Auto => "auto",
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
    pub format: ImageFormat,
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
    /// Who set `pick` (v18); `null` when `pick = unflagged`.
    pub pick_origin: Option<PickOrigin>,
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
    /// RAW only: absolute path of the same-stem camera JPEG/HEIC paired with this RAW at
    /// import (`ImportOptions.pairJpegWithRaw`); that file is not a catalog image of its own.
    /// Never modified, never exported (Phase 7b).
    pub companion_path: Option<String>,
    /// Sidecar develop settings Sieve preserves but does not render (masks, retouch, lens
    /// corrections, transforms, legacy process version), found at the last XMP read. Empty
    /// when none / never read. Editor-time warnings (Adobe Look, source colour) are in
    /// `DevelopInfo.warnings` (Phase 7b).
    pub develop_warnings: Vec<DevelopWarning>,
    /// Unix ms when the original was first found missing at its `path` (moved, renamed,
    /// drive disconnected) by a render, develop info, export, sidecar write, thumbnail
    /// extraction or re-import; `null` = present (or not checked since). Cleared by the next
    /// successful access, a re-import that finds it, or `relocate_folder` (IPC v13).
    pub missing_since_ms: Option<i64>,
    /// Rendered previews of the photo's develop settings (IPC v19.1): what Library grid,
    /// Loupe, filmstrip and scenes show for an edited photo instead of the embedded
    /// thumbnail. `null` when unedited (`hasEdits = false`) or not rendered yet (the
    /// `editedPreviewChanged` event follows once it is). May briefly lag the newest edit.
    pub edited_preview: Option<EditedPreview>,
}

/// Cached renders of one photo's develop settings (IPC v19.1), served by the `sieve://`
/// scheme from the app cache dir (never next to the photos). URLs are content-addressed
/// (image id + settings hash): a new edit gives new URLs, so they may be cached forever.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EditedPreview {
    /// Grid thumbnail, long edge 512 px, orientation and crop applied.
    pub thumb_url: String,
    /// Loupe / Develop placeholder, long edge 2048 px, orientation and crop applied.
    pub preview_url: String,
}

// ---------------------------------------------------------------------------
// XMP sidecars (Phase 4)
// ---------------------------------------------------------------------------

/// Per-image XMP sidecar state (`<basename>.xmp` next to a RAW; `<file name>.xmp`, e.g.
/// `IMG_1.JPG.xmp`, next to a non-RAW source: see `xmp::sidecar_path`).
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
    /// A sidecar existed at the last successful write or read (import reads existing
    /// sidecars) (v18; `ImageQuery.metadata.hasSidecar`). `false` = never synced, or no
    /// sidecar was found then.
    pub has_sidecar: bool,
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
/// Since IPC v14 the UI presents LUTs as profiles (`StyleProfileKind::Lut`, profile browser
/// with an Amount slider); this field is where a LUT profile lives in the adjustments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LutRef {
    /// `LutInfo.id` (library file stem). A missing LUT renders as if absent and sets
    /// `RenderedPreview.lutMissing`.
    pub id: LutId,
    /// Amount 0..=200 (100 = full LUT output; v14: above 100 extrapolates
    /// `in + (lut - in) * amount / 100`, clamped to the output range, like Lightroom's
    /// profile Amount). Was 0..=100 before v14.
    #[specta(type = Number)]
    pub amount: f32,
}

// ---------------------------------------------------------------------------
// Lightroom develop parity (Phase 7b, IPC v9). Every group maps 1:1 to `crs:` properties
// (table in `xmp/crs.rs` and docs/architecture.md). Defaults are Lightroom's defaults for a
// RAW file, so an untouched group renders and writes exactly what Lightroom would.
// ---------------------------------------------------------------------------

/// Tone-curve control point `[input, output]`, both 0..=255 on Lightroom's curve axes
/// (display-referred encoded values).
pub type CurvePoint = [f32; 2];

/// Point curves: `crs:ToneCurvePV2012` (+ `Red` / `Green` / `Blue`). Each curve has
/// 2..=[`PointCurves::MAX_POINTS`] points with strictly increasing input; endpoints may move
/// (e.g. `[0, 14]` lifts the blacks). Identity = `[[0, 0], [255, 255]]`. Interpolation is
/// Lightroom's (monotone cubic through the points), evaluated by the engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PointCurves {
    /// Applied to R, G and B alike.
    #[specta(type = Vec<[Number; 2]>)]
    pub master: Vec<CurvePoint>,
    #[specta(type = Vec<[Number; 2]>)]
    pub red: Vec<CurvePoint>,
    #[specta(type = Vec<[Number; 2]>)]
    pub green: Vec<CurvePoint>,
    #[specta(type = Vec<[Number; 2]>)]
    pub blue: Vec<CurvePoint>,
}

impl PointCurves {
    pub const MAX_POINTS: usize = 32;
    pub const IDENTITY: [CurvePoint; 2] = [[0.0, 0.0], [255.0, 255.0]];

    pub fn identity_curve() -> Vec<CurvePoint> {
        Self::IDENTITY.to_vec()
    }

    pub fn is_identity(curve: &[CurvePoint]) -> bool {
        curve == Self::IDENTITY
    }

    fn curves(&self) -> [(&'static str, &Vec<CurvePoint>); 4] {
        [("master", &self.master), ("red", &self.red), ("green", &self.green), ("blue", &self.blue)]
    }

    /// 2..=MAX_POINTS finite points within 0..=255, inputs strictly increasing.
    pub fn validate_curve(name: &str, curve: &[CurvePoint]) -> Result<(), String> {
        if !(2..=Self::MAX_POINTS).contains(&curve.len()) {
            return Err(format!("{name} has {} points (2..={} allowed)", curve.len(), Self::MAX_POINTS));
        }
        for p in curve {
            if !p.iter().all(|v| v.is_finite() && (0.0..=255.0).contains(v)) {
                return Err(format!("{name} point {p:?} is outside 0..=255"));
            }
        }
        if curve.windows(2).any(|w| w[1][0] <= w[0][0]) {
            return Err(format!("{name} inputs must be strictly increasing"));
        }
        Ok(())
    }

    /// Point-wise when both curves have the same number of points, else the nearer side.
    fn lerp_curve(a: &[CurvePoint], b: &[CurvePoint], t: f32) -> Vec<CurvePoint> {
        if a.len() == b.len() {
            a.iter().zip(b).map(|(p, q)| [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t]).collect()
        } else if t >= 0.5 {
            b.to_vec()
        } else {
            a.to_vec()
        }
    }

    fn lerp(a: &Self, b: &Self, t: f32) -> Self {
        Self {
            master: Self::lerp_curve(&a.master, &b.master, t),
            red: Self::lerp_curve(&a.red, &b.red, t),
            green: Self::lerp_curve(&a.green, &b.green, t),
            blue: Self::lerp_curve(&a.blue, &b.blue, t),
        }
    }
}

impl Default for PointCurves {
    fn default() -> Self {
        Self {
            master: Self::identity_curve(),
            red: Self::identity_curve(),
            green: Self::identity_curve(),
            blue: Self::identity_curve(),
        }
    }
}

/// Parametric ("region") tone curve: `crs:Parametric*`. Region amounts -100..=100; splits
/// 0..=100 with `shadowSplit < midtoneSplit < highlightSplit` (Lightroom defaults 25/50/75).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ParametricCurve {
    #[specta(type = Number)]
    pub shadows: f32,
    #[specta(type = Number)]
    pub darks: f32,
    #[specta(type = Number)]
    pub lights: f32,
    #[specta(type = Number)]
    pub highlights: f32,
    #[specta(type = Number)]
    pub shadow_split: f32,
    #[specta(type = Number)]
    pub midtone_split: f32,
    #[specta(type = Number)]
    pub highlight_split: f32,
}

impl Default for ParametricCurve {
    fn default() -> Self {
        Self {
            shadows: 0.0,
            darks: 0.0,
            lights: 0.0,
            highlights: 0.0,
            shadow_split: 25.0,
            midtone_split: 50.0,
            highlight_split: 75.0,
        }
    }
}

/// Tone Curve panel. Order in the pipeline: parametric curve, then the point curves
/// (master, then per channel), both on display-referred values after the base tone.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ToneCurve {
    pub parametric: ParametricCurve,
    pub point: PointCurves,
}

/// One Color Grading wheel. `hue` 0..=360 degrees, `saturation` 0..=100,
/// `luminance` -100..=100.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ColorWheel {
    #[specta(type = Number)]
    pub hue: f32,
    #[specta(type = Number)]
    pub saturation: f32,
    #[specta(type = Number)]
    pub luminance: f32,
}

impl ColorWheel {
    /// Hue along the shorter arc; for 0 < t < 1 a side with saturation 0 (hue meaningless)
    /// takes the other side's hue. `t <= 0` / `t >= 1` return `a` / `b` exactly.
    fn lerp(a: &Self, b: &Self, t: f32) -> Self {
        if t <= 0.0 {
            return *a;
        }
        if t >= 1.0 {
            return *b;
        }
        let l = |x: f32, y: f32| x + (y - x) * t;
        let hue = if a.saturation == 0.0 {
            b.hue
        } else if b.saturation == 0.0 {
            a.hue
        } else {
            let mut d = (b.hue - a.hue) % 360.0;
            if d > 180.0 {
                d -= 360.0;
            } else if d < -180.0 {
                d += 360.0;
            }
            (a.hue + d * t).rem_euclid(360.0)
        };
        Self { hue, saturation: l(a.saturation, b.saturation), luminance: l(a.luminance, b.luminance) }
    }
}

/// Color Grading panel (Lightroom 10+; legacy Split Toning maps onto it: shadow/highlight
/// hue+saturation *are* `crs:SplitToning*`, balance is `crs:SplitToningBalance`).
/// `blending` 0..=100 (default 50; a legacy split-toning sidecar without
/// `crs:ColorGradeBlending` reads as 100), `balance` -100..=100.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ColorGrading {
    pub shadows: ColorWheel,
    pub midtones: ColorWheel,
    pub highlights: ColorWheel,
    pub global: ColorWheel,
    #[specta(type = Number)]
    pub blending: f32,
    #[specta(type = Number)]
    pub balance: f32,
}

impl Default for ColorGrading {
    fn default() -> Self {
        Self {
            shadows: ColorWheel::default(),
            midtones: ColorWheel::default(),
            highlights: ColorWheel::default(),
            global: ColorWheel::default(),
            blending: 50.0,
            balance: 0.0,
        }
    }
}

/// Hue / saturation shift of one camera primary, each -100..=100.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PrimaryCalibration {
    #[specta(type = Number)]
    pub hue: f32,
    #[specta(type = Number)]
    pub saturation: f32,
}

/// Calibration panel (`crs:RedHue` ... `crs:ShadowTint`), all -100..=100. Applied to the
/// camera -> working-space matrix (primaries), before every other colour operation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CameraCalibration {
    pub red: PrimaryCalibration,
    pub green: PrimaryCalibration,
    pub blue: PrimaryCalibration,
    /// Green (-) / magenta (+) tint of the shadows.
    #[specta(type = Number)]
    pub shadow_tint: f32,
}

/// Capture sharpening (Detail panel). `amount` 0..=150, `radius` 0.5..=3.0 px (at full
/// resolution; the engine scales it for reduced-size previews), `detail` 0..=100,
/// `masking` 0..=100. Lightroom RAW defaults 40 / 1.0 / 25 / 0; non-RAW default amount 0.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Sharpening {
    #[specta(type = Number)]
    pub amount: f32,
    #[specta(type = Number)]
    pub radius: f32,
    #[specta(type = Number)]
    pub detail: f32,
    #[specta(type = Number)]
    pub masking: f32,
}

impl Default for Sharpening {
    fn default() -> Self {
        Self { amount: 40.0, radius: 1.0, detail: 25.0, masking: 0.0 }
    }
}

/// Noise reduction (Detail panel), all 0..=100. Lightroom RAW defaults: luminance 0,
/// luminanceDetail 50, luminanceContrast 0, color 25, colorDetail 50, colorSmoothness 50;
/// non-RAW default color 0.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NoiseReduction {
    #[specta(type = Number)]
    pub luminance: f32,
    #[specta(type = Number)]
    pub luminance_detail: f32,
    #[specta(type = Number)]
    pub luminance_contrast: f32,
    #[specta(type = Number)]
    pub color: f32,
    #[specta(type = Number)]
    pub color_detail: f32,
    #[specta(type = Number)]
    pub color_smoothness: f32,
}

impl Default for NoiseReduction {
    fn default() -> Self {
        Self {
            luminance: 0.0,
            luminance_detail: 50.0,
            luminance_contrast: 0.0,
            color: 25.0,
            color_detail: 50.0,
            color_smoothness: 50.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DetailAdjustments {
    pub sharpening: Sharpening,
    pub noise_reduction: NoiseReduction,
}

string_enum! {
    /// Post-crop vignette style (`crs:PostCropVignetteStyle` 1 / 2 / 3).
    pub enum VignetteStyle {
        HighlightPriority => "highlight_priority",
        ColorPriority => "color_priority",
        PaintOverlay => "paint_overlay",
    }
}

/// Post-crop vignette (Effects panel), relative to the cropped frame. `amount` -100..=100
/// (negative darkens), `midpoint` 0..=100 (50), `roundness` -100..=100 (0), `feather`
/// 0..=100 (50), `highlights` 0..=100 (0; highlight/colour priority only).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PostCropVignette {
    #[specta(type = Number)]
    pub amount: f32,
    #[specta(type = Number)]
    pub midpoint: f32,
    #[specta(type = Number)]
    pub roundness: f32,
    #[specta(type = Number)]
    pub feather: f32,
    #[specta(type = Number)]
    pub highlights: f32,
    pub style: VignetteStyle,
}

impl Default for PostCropVignette {
    fn default() -> Self {
        Self {
            amount: 0.0,
            midpoint: 50.0,
            roundness: 0.0,
            feather: 50.0,
            highlights: 0.0,
            style: VignetteStyle::HighlightPriority,
        }
    }
}

/// Film grain (Effects panel), all 0..=100: `amount` (0), `size` (25), `roughness` (50,
/// `crs:GrainFrequency`). Deterministic per image (seeded by the image id), scaled with the
/// output size so previews and exports match.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Grain {
    #[specta(type = Number)]
    pub amount: f32,
    #[specta(type = Number)]
    pub size: f32,
    #[specta(type = Number)]
    pub roughness: f32,
}

impl Default for Grain {
    fn default() -> Self {
        Self { amount: 0.0, size: 25.0, roughness: 50.0 }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EffectsAdjustments {
    pub vignette: PostCropVignette,
    pub grain: Grain,
}

/// Black & White treatment (`crs:ConvertToGrayscale` + `crs:GrayMixer*`, -100..=100 per
/// band). Independent of monochrome looks (Adobe Monochrome, B&W creative profiles), which
/// convert through their own tables.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BlackAndWhite {
    pub enabled: bool,
    pub mixer: HslChannels,
}

/// Crop + straighten, in Lightroom's `crs:` semantics so sidecars round-trip:
/// `top/left/bottom/right` 0..=1 are fractions of the *un-oriented* image (before EXIF
/// orientation, as ACR stores them) with `left < right`, `top < bottom`; `angle` -45..=45
/// degrees (`crs:CropAngle`). `enabled` = `crs:HasCrop`; disabled renders the full frame.
/// When enabled, renders (`RenderedPreview` size, `RenderOptions.region`, histogram,
/// scene stats) and exports cover the cropped, straightened frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CropSettings {
    pub enabled: bool,
    #[specta(type = Number)]
    pub top: f32,
    #[specta(type = Number)]
    pub left: f32,
    #[specta(type = Number)]
    pub bottom: f32,
    #[specta(type = Number)]
    pub right: f32,
    #[specta(type = Number)]
    pub angle: f32,
}

impl Default for CropSettings {
    fn default() -> Self {
        Self { enabled: false, top: 0.0, left: 0.0, bottom: 1.0, right: 1.0, angle: 0.0 }
    }
}

string_enum! {
    /// Lightroom Transform panel "Upright" mode (IPC v19). `crs:PerspectiveUpright` stores it
    /// as an integer: 0 off, 1 auto, 2 full, 3 level, 4 vertical, 5 guided (ExifTool's crs
    /// table; [`UprightMode::crs_value`] / [`UprightMode::from_crs`]).
    #[derive(Default)]
    pub enum UprightMode {
        #[default]
        Off => "off",
        /// Balanced level + vertical + aspect correction.
        Auto => "auto",
        /// Horizon / dominant horizontal lines level (rotation only).
        Level => "level",
        /// Level + converging verticals.
        Vertical => "vertical",
        /// Level + vertical + horizontal perspective.
        Full => "full",
        /// From the user's guide lines (`TransformSettings.guides`, 2..=4).
        Guided => "guided",
    }
}

impl UprightMode {
    /// `crs:PerspectiveUpright` value.
    pub fn crs_value(self) -> u8 {
        match self {
            UprightMode::Off => 0,
            UprightMode::Auto => 1,
            UprightMode::Full => 2,
            UprightMode::Level => 3,
            UprightMode::Vertical => 4,
            UprightMode::Guided => 5,
        }
    }

    /// Inverse of [`Self::crs_value`]; unknown values -> `None`.
    pub fn from_crs(v: i64) -> Option<UprightMode> {
        Some(match v {
            0 => UprightMode::Off,
            1 => UprightMode::Auto,
            2 => UprightMode::Full,
            3 => UprightMode::Level,
            4 => UprightMode::Vertical,
            5 => UprightMode::Guided,
            _ => return None,
        })
    }
}

/// One Guided Upright guide line (v19), endpoints in the **sensor frame** (normalized 0..=1
/// of the un-oriented, uncropped image, like masks and `crs:UprightFourSegments_N`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UprightGuide {
    pub start: NormPoint,
    pub end: NormPoint,
}

/// A `crs:` property kept verbatim (name without the `crs:` prefix, value as written).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CrsProperty {
    pub name: String,
    pub value: String,
}

/// The perspective correction an Upright mode solved for one photo (v19): from
/// `auto_upright` (Sieve's line detection) or read from a Lightroom sidecar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UprightSolution {
    /// Mode it was solved for. Rendered only while it equals `TransformSettings.upright`.
    pub mode: UprightMode,
    /// Row-major 3x3 homography (9 finite values) mapping a point of the corrected frame to
    /// the source, both normalized 0..=1 in the sensor frame (un-oriented, before crop); the
    /// manual sliders apply on top. Identity = `[1,0,0, 0,1,0, 0,0,1]`.
    #[specta(type = Vec<Number>)]
    pub matrix: Vec<f64>,
    /// In-plane rotation part of the solve, degrees (positive = counter-clockwise), e.g. the
    /// horizon angle `level` corrected. Informational (UI readout, tests).
    #[specta(type = Number)]
    pub rotation_deg: f32,
    /// Lightroom's own Upright state read from the sidecar, kept verbatim and written back
    /// unchanged while `mode` is still the sidecar's (`UprightVersion`, `UprightTransform_0..5`,
    /// `UprightTransformCount`, `UprightFocalMode`, `UprightFocalLength35mm`,
    /// `UprightCenterMode`, `UprightCenterNormX/Y`, `UprightPreview`, `UprightDependentDigest`,
    /// `UprightGuidedDependentDigest`, `UprightFourSegments*`). Empty when solved by Sieve.
    pub crs: Vec<CrsProperty>,
}

impl UprightSolution {
    pub const IDENTITY: [f64; 9] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
}

/// Lightroom Transform panel (IPC v19). Field <-> `crs:` mapping (Process 2012+; `xmp/crs.rs`
/// implements it, rust-engine-dev):
///
/// | Field | crs property | Range / format |
/// |---|---|---|
/// | `upright` | `PerspectiveUpright` | 0..=5, see [`UprightMode`] |
/// | `guides` | `UprightFourSegmentsCount` + `UprightFourSegments_0..3` | 0..=4 lines |
/// | `vertical` | `PerspectiveVertical` | -100..=100, signed integer |
/// | `horizontal` | `PerspectiveHorizontal` | -100..=100, signed integer |
/// | `rotate` | `PerspectiveRotate` | -10..=10 degrees, signed, 1 decimal |
/// | `aspect` | `PerspectiveAspect` | -100..=100, signed integer |
/// | `scale` | `PerspectiveScale` | 50..=150 (default 100), integer |
/// | `offsetX` / `offsetY` | `PerspectiveX` / `PerspectiveY` | -100..=100, signed, 2 decimals |
/// | `constrainCrop` | `CropConstrainToWarp` | 0 / 1 |
/// | `solution.crs` | `UprightVersion`, `UprightTransform_*`, `UprightFocal*`, `UprightCenter*`, `UprightPreview`, `Upright*DependentDigest` | verbatim |
///
/// Default = Lightroom's (off, sliders 0, scale 100, constrain off) and neutral. Renders
/// (preview, export) apply `solution` (when its mode is current) then the sliders, before
/// the crop; with `constrainCrop` the crop is limited to the warped image area.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TransformSettings {
    pub upright: UprightMode,
    /// Guided Upright lines (at most 4; Guided needs 2+ to have an effect).
    #[serde(default)]
    pub guides: Vec<UprightGuide>,
    #[specta(type = Number)]
    pub vertical: f32,
    #[specta(type = Number)]
    pub horizontal: f32,
    /// Degrees.
    #[specta(type = Number)]
    pub rotate: f32,
    #[specta(type = Number)]
    pub aspect: f32,
    /// Percent.
    #[specta(type = Number)]
    pub scale: f32,
    #[specta(type = Number)]
    pub offset_x: f32,
    #[specta(type = Number)]
    pub offset_y: f32,
    pub constrain_crop: bool,
    /// Solved Upright correction (`auto_upright`, or read from the sidecar); `null` for
    /// `off` or not solved yet.
    #[serde(default)]
    pub solution: Option<UprightSolution>,
}

impl Default for TransformSettings {
    fn default() -> Self {
        Self {
            upright: UprightMode::Off,
            guides: Vec::new(),
            vertical: 0.0,
            horizontal: 0.0,
            rotate: 0.0,
            aspect: 0.0,
            scale: 100.0,
            offset_x: 0.0,
            offset_y: 0.0,
            constrain_crop: false,
            solution: None,
        }
    }
}

impl TransformSettings {
    pub const MAX_GUIDES: usize = 4;

    /// Returns a description of the first invalid value, if any.
    pub fn validate(&self) -> Result<(), String> {
        fn check(name: &str, v: f32, min: f32, max: f32) -> Result<(), String> {
            if v.is_finite() && (min..=max).contains(&v) {
                Ok(())
            } else {
                Err(format!("transform.{name} = {v} is outside {min}..={max}"))
            }
        }
        check("vertical", self.vertical, -100.0, 100.0)?;
        check("horizontal", self.horizontal, -100.0, 100.0)?;
        check("rotate", self.rotate, -10.0, 10.0)?;
        check("aspect", self.aspect, -100.0, 100.0)?;
        check("scale", self.scale, 50.0, 150.0)?;
        check("offsetX", self.offset_x, -100.0, 100.0)?;
        check("offsetY", self.offset_y, -100.0, 100.0)?;
        if self.guides.len() > Self::MAX_GUIDES {
            return Err(format!("transform.guides: at most {} guides", Self::MAX_GUIDES));
        }
        for g in &self.guides {
            for p in [g.start, g.end] {
                if !(p.x.is_finite() && p.y.is_finite() && (0.0..=1.0).contains(&p.x) && (0.0..=1.0).contains(&p.y)) {
                    return Err("transform.guides points must lie within 0..=1".into());
                }
            }
        }
        if let Some(sol) = &self.solution {
            if sol.matrix.len() != 9 || sol.matrix.iter().any(|v| !v.is_finite()) {
                return Err("transform.solution.matrix must hold 9 finite numbers".into());
            }
            if !sol.rotation_deg.is_finite() {
                return Err("transform.solution.rotationDeg must be finite".into());
            }
            for p in &sol.crs {
                let name_ok = !p.name.is_empty()
                    && p.name.len() <= 64
                    && p.name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
                if !name_ok || p.value.len() > 4096 {
                    return Err(format!("transform.solution.crs: invalid property {:?}", p.name));
                }
            }
        }
        Ok(())
    }
}

/// A Look / creative profile (`<crs:Look>`): Adobe Raw looks (Adobe Color, Adobe Monochrome,
/// ...), creative profiles (Artistic, B&W, Modern, Vintage) and third-party looks. Resolved
/// at render time by `uuid` from the installed look profiles (`profiles::ProfileLibrary`,
/// `/Library/Application Support/Adobe/CameraRaw/Settings/**/*.xmp` with
/// `crs:PresetType="Look"`), else from the image's sidecar (`crs:Look/crs:Parameters` +
/// `crs:Table_<md5>`); if neither is available the look is skipped and
/// `DevelopWarningCode::LookUnavailable` is reported.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LookSettings {
    /// `crs:Look/crs:Name`, e.g. "Adobe Color" (display only).
    pub name: String,
    /// `crs:Look/crs:UUID` (32 upper-case hex digits): the library key.
    pub uuid: String,
    /// `crs:Look/crs:Amount`, 0..=2 (1 = 100%); only adjustable when the look supports it
    /// (`LookProfileInfo.supportsAmount`), else 1.
    #[specta(type = Number)]
    pub amount: f32,
}

impl LookSettings {
    /// Lightroom's default look for RAW files ("Adobe Color", UUID stable across ACR versions).
    pub fn adobe_color() -> Self {
        Self { name: "Adobe Color".into(), uuid: "B952C231111CD8E0ECCF14B86BAA7077".into(), amount: 1.0 }
    }

    pub fn is_valid_uuid(uuid: &str) -> bool {
        uuid.len() == 32 && uuid.bytes().all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
    }
}

/// Profile (Lightroom's Profile browser): base camera profile (DCP) + optional look.
/// Default = Lightroom's RAW default "Adobe Color" (= DCP "Adobe Standard" + look Adobe
/// Color); non-RAW default = no camera profile, no look.
/// Rendering (see `profiles` module docs): the DCP named `cameraProfile` for the image's
/// camera is located in the installed Adobe CameraProfiles (read at runtime, never bundled);
/// when missing, Sieve's LibRaw-matrix base stands in (`DevelopWarningCode::ProfileUnavailable`).
/// Copy/paste group `profile`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSettings {
    /// `crs:CameraProfile`: DCP `ProfileName`, e.g. "Adobe Standard", "Camera ST",
    /// "Camera Standard". `null` = no camera profile (non-RAW "Embedded"; for RAW the
    /// LibRaw-matrix base). 1..=128 chars.
    pub camera_profile: Option<String>,
    pub look: Option<LookSettings>,
}

impl ProfileSettings {
    pub const ADOBE_STANDARD: &'static str = "Adobe Standard";
    pub const MAX_NAME: usize = 128;

    /// Nothing applied (non-RAW default).
    pub fn none() -> Self {
        Self { camera_profile: None, look: None }
    }

    fn lerp(a: &Self, b: &Self, t: f32) -> Self {
        match (&a.look, &b.look) {
            (Some(x), Some(y)) if x.uuid == y.uuid && a.camera_profile == b.camera_profile => Self {
                camera_profile: a.camera_profile.clone(),
                look: Some(LookSettings { amount: x.amount + (y.amount - x.amount) * t, ..x.clone() }),
            },
            _ => {
                if t >= 0.5 {
                    b.clone()
                } else {
                    a.clone()
                }
            }
        }
    }
}

impl Default for ProfileSettings {
    /// Lightroom's RAW default ("Adobe Color").
    fn default() -> Self {
        Self { camera_profile: Some(Self::ADOBE_STANDARD.into()), look: Some(LookSettings::adobe_color()) }
    }
}

string_enum! {
    /// Why a render may differ from Lightroom's (Phase 7b).
    pub enum DevelopWarningCode {
        /// The camera profile (DCP) `detail` is not installed for this camera: Sieve's
        /// LibRaw-matrix base colour stands in. Installing Adobe DNG Converter (free) fixes it.
        ProfileUnavailable => "profile_unavailable",
        /// The look `detail` is neither installed nor embedded in the sidecar: rendered
        /// without it.
        LookUnavailable => "look_unavailable",
        /// Local corrections Sieve cannot render (v10: `MaskShape::Unsupported` components
        /// such as depth ranges, legacy pre-2021 `crs:GradientBasedCorrections` /
        /// `CircularGradientBasedCorrections` / `PaintBasedCorrections`): preserved in the
        /// sidecar. `detail` = number of affected components / corrections. (Until the v10
        /// mask reader lands, every `crs:MaskGroupBasedCorrections`; `detail` = group count.)
        MasksUnsupported => "masks_unsupported",
        /// AI mask components without a matte for this image (pasted/synced masks, or the
        /// model for that kind is not installed): rendered as empty until `computeAiMask`
        /// succeeds. `detail` = number of components (v10).
        AiMaskNeedsUpdate => "ai_mask_needs_update",
        /// `crs:RetouchAreas` (spot heal/clone): preserved, not rendered.
        RetouchUnsupported => "retouch_unsupported",
        /// Lens profile / chromatic aberration / defringe / manual distortion or lens
        /// vignetting: preserved, not rendered.
        LensCorrectionsUnsupported => "lens_corrections_unsupported",
        /// Upright / manual perspective transform: preserved, not rendered.
        TransformUnsupported => "transform_unsupported",
        /// Pre-2012 process version: develop settings not imported.
        LegacyProcessVersion => "legacy_process_version",
        /// Non-RAW source without a recognised ICC profile: decoded as sRGB.
        /// `detail` = profile description, if any.
        SourceColorAssumed => "source_color_assumed",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DevelopWarning {
    pub code: DevelopWarningCode,
    /// Short specifics for display (look/profile name, count), if any.
    pub detail: Option<String>,
}

/// A camera profile (DCP) installed for an image's camera.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CameraProfileInfo {
    /// DCP `ProfileName` = `crs:CameraProfile` value, e.g. "Adobe Standard", "Camera ST".
    pub name: String,
    /// Profile browser group: "Adobe Raw" (Adobe Standard), "Camera Matching" (`Camera/<model>/`),
    /// else "Other"; for imported DCPs (v14) the style group's name.
    pub group: String,
    /// Style-library profile this entry comes from (imported with `import_style_folder`, v14);
    /// `null` = installed by Adobe software.
    pub style_id: Option<StyleProfileId>,
}

/// A look / creative profile installed on this Mac (Lightroom's Profile browser entries
/// other than bare DCPs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LookProfileInfo {
    /// `crs:UUID` (= `LookSettings.uuid`).
    pub uuid: String,
    /// `crs:Name`, e.g. "Adobe Color", "Artistic 01".
    pub name: String,
    /// `crs:Group` (e.g. "Profiles" for Adobe Raw, "Artistic", "B&W", "Modern", "Vintage"),
    /// else the containing directory name.
    pub group: String,
    /// `crs:SupportsAmount`: the Amount slider (0..=200%) applies.
    pub supports_amount: bool,
    /// Converts to monochrome (`crs:ConvertToGrayscale` in the look).
    pub monochrome: bool,
    /// `crs:CameraProfile` the look is built on (selecting the look sets
    /// `ProfileSettings.cameraProfile` to it); `null` = keeps the current camera profile.
    pub camera_profile: Option<String>,
    /// Usable for this image (`crs:CameraModelRestriction` empty or matching; RAW-only looks
    /// are unavailable for non-RAW sources; v14: imported file still readable).
    pub available: bool,
    /// Style-library profile this entry comes from (imported creative profile, v14); `null` =
    /// installed by Adobe software. For imported looks `group` is the style group's name.
    pub style_id: Option<StyleProfileId>,
}

/// A `.cube` LUT offered as a profile in the profile browser (IPC v14). Selecting it sets
/// `ParametricAdjustments.lut = { id: lutId, amount: 100 }` (see `StyleProfile`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LutProfileInfo {
    pub style_id: StyleProfileId,
    pub lut_id: LutId,
    pub name: String,
    /// Style group name (source folder name, or "LUTs" for the pre-v14 LUT library).
    pub group: String,
    /// The library copy exists.
    pub available: bool,
}

/// Profile browser contents for one image (`list_profiles`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProfileCatalog {
    pub image_id: ImageId,
    /// Adobe unique camera model the DCPs were matched on (e.g. "Sony ILCE-7M4"); `null` for
    /// non-RAW sources or unknown cameras.
    pub camera_model: Option<String>,
    /// DCPs for this camera (empty for non-RAW sources or when none are installed).
    pub camera_profiles: Vec<CameraProfileInfo>,
    pub looks: Vec<LookProfileInfo>,
    /// `.cube` LUTs of the style library, as profiles (v14; every group, library order).
    pub luts: Vec<LutProfileInfo>,
    /// Directories scanned (for the "no Adobe profiles found" hint).
    pub search_dirs: Vec<String>,
}

/// Parametric develop settings. Field names and ranges mirror Adobe Camera Raw
/// Process 2012+ (`crs:` XMP namespace) so XMP export is a 1:1 mapping.
/// Local adjustments (Phase 7c, IPC v10) are `masks`: mask groups with per-mask parameter
/// sets mirroring Lightroom's `crs:MaskGroupBasedCorrections` (see `ipc/masks.rs`).
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
    // Phase 7b (IPC v9) Lightroom parity groups. `#[serde(default)]` so JSON written before
    // v9 (and callers built against older bindings) still deserializes; TS sees them as
    // optional (`completeAdjustments` in `src/ipc/index.ts` fills them in). The backend always
    // sends them.
    #[serde(default)]
    pub tone_curve: ToneCurve,
    #[serde(default)]
    pub color_grading: ColorGrading,
    #[serde(default)]
    pub calibration: CameraCalibration,
    #[serde(default)]
    pub detail: DetailAdjustments,
    #[serde(default)]
    pub effects: EffectsAdjustments,
    #[serde(default)]
    pub black_and_white: BlackAndWhite,
    #[serde(default)]
    pub crop: CropSettings,
    /// Camera profile + look (see [`ProfileSettings`]).
    #[serde(default)]
    pub profile: ProfileSettings,
    /// Local adjustments (IPC v10): Masks-panel groups in Lightroom's order (see
    /// [`MaskGroup`]). Empty = none. `#[serde(default)]` like the v9 groups.
    #[serde(default)]
    pub masks: Vec<MaskGroup>,
    /// Transform panel (IPC v19): Upright + manual perspective sliders (see
    /// [`TransformSettings`]). `#[serde(default)]` like the v9 groups.
    #[serde(default)]
    pub transform: TransformSettings,
}

impl ParametricAdjustments {
    pub const PROCESS_VERSION: u32 = 1;

    /// Neutral ("unedited") settings for a source format: Lightroom's defaults. RAW = `default()`
    /// (incl. profile "Adobe Color"); non-RAW sources (already rendered, sharpened and
    /// noise-reduced in camera) default to sharpening amount 0, color noise reduction 0 and no
    /// profile, as in Lightroom.
    pub fn defaults_for(format: ImageFormat) -> ParametricAdjustments {
        let mut adj = ParametricAdjustments::default();
        if !format.is_raw() {
            adj.detail.sharpening.amount = 0.0;
            adj.detail.noise_reduction.color = 0.0;
            adj.profile = ProfileSettings::none();
        }
        adj
    }

    /// Equal to [`Self::defaults_for`] `format` (drives `hasEdits` / `adjustments.neutral`).
    pub fn is_neutral_for(&self, format: ImageFormat) -> bool {
        *self == Self::defaults_for(format)
    }

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
            check("lut.amount", lut.amount, 0.0, 200.0)?;
        }

        // Phase 7b groups.
        let pc = &self.tone_curve.parametric;
        for (name, v) in [
            ("toneCurve.parametric.shadows", pc.shadows),
            ("toneCurve.parametric.darks", pc.darks),
            ("toneCurve.parametric.lights", pc.lights),
            ("toneCurve.parametric.highlights", pc.highlights),
        ] {
            check(name, v, -100.0, 100.0)?;
        }
        for (name, v) in [
            ("toneCurve.parametric.shadowSplit", pc.shadow_split),
            ("toneCurve.parametric.midtoneSplit", pc.midtone_split),
            ("toneCurve.parametric.highlightSplit", pc.highlight_split),
        ] {
            check(name, v, 0.0, 100.0)?;
        }
        if !(pc.shadow_split < pc.midtone_split && pc.midtone_split < pc.highlight_split) {
            return Err("toneCurve.parametric splits must satisfy shadowSplit < midtoneSplit < highlightSplit".into());
        }
        for (name, curve) in self.tone_curve.point.curves() {
            PointCurves::validate_curve(&format!("toneCurve.point.{name}"), curve)?;
        }

        let cg = &self.color_grading;
        for (name, w) in [
            ("shadows", &cg.shadows),
            ("midtones", &cg.midtones),
            ("highlights", &cg.highlights),
            ("global", &cg.global),
        ] {
            check(&format!("colorGrading.{name}.hue"), w.hue, 0.0, 360.0)?;
            check(&format!("colorGrading.{name}.saturation"), w.saturation, 0.0, 100.0)?;
            check(&format!("colorGrading.{name}.luminance"), w.luminance, -100.0, 100.0)?;
        }
        check("colorGrading.blending", cg.blending, 0.0, 100.0)?;
        check("colorGrading.balance", cg.balance, -100.0, 100.0)?;

        let cal = &self.calibration;
        for (name, p) in [("red", &cal.red), ("green", &cal.green), ("blue", &cal.blue)] {
            check(&format!("calibration.{name}.hue"), p.hue, -100.0, 100.0)?;
            check(&format!("calibration.{name}.saturation"), p.saturation, -100.0, 100.0)?;
        }
        check("calibration.shadowTint", cal.shadow_tint, -100.0, 100.0)?;

        let s = &self.detail.sharpening;
        check("detail.sharpening.amount", s.amount, 0.0, 150.0)?;
        check("detail.sharpening.radius", s.radius, 0.5, 3.0)?;
        check("detail.sharpening.detail", s.detail, 0.0, 100.0)?;
        check("detail.sharpening.masking", s.masking, 0.0, 100.0)?;
        let nr = &self.detail.noise_reduction;
        for (name, v) in [
            ("luminance", nr.luminance),
            ("luminanceDetail", nr.luminance_detail),
            ("luminanceContrast", nr.luminance_contrast),
            ("color", nr.color),
            ("colorDetail", nr.color_detail),
            ("colorSmoothness", nr.color_smoothness),
        ] {
            check(&format!("detail.noiseReduction.{name}"), v, 0.0, 100.0)?;
        }

        let v = &self.effects.vignette;
        check("effects.vignette.amount", v.amount, -100.0, 100.0)?;
        check("effects.vignette.midpoint", v.midpoint, 0.0, 100.0)?;
        check("effects.vignette.roundness", v.roundness, -100.0, 100.0)?;
        check("effects.vignette.feather", v.feather, 0.0, 100.0)?;
        check("effects.vignette.highlights", v.highlights, 0.0, 100.0)?;
        let g = &self.effects.grain;
        check("effects.grain.amount", g.amount, 0.0, 100.0)?;
        check("effects.grain.size", g.size, 0.0, 100.0)?;
        check("effects.grain.roughness", g.roughness, 0.0, 100.0)?;

        for v in self.black_and_white.mixer.values() {
            check("blackAndWhite.mixer", v, -100.0, 100.0)?;
        }

        let c = &self.crop;
        for (name, v) in [("top", c.top), ("left", c.left), ("bottom", c.bottom), ("right", c.right)] {
            check(&format!("crop.{name}"), v, 0.0, 1.0)?;
        }
        check("crop.angle", c.angle, -45.0, 45.0)?;
        if !(c.left < c.right && c.top < c.bottom) {
            return Err("crop must satisfy left < right and top < bottom".into());
        }

        if let Some(name) = &self.profile.camera_profile {
            if name.trim().is_empty() || name.chars().count() > ProfileSettings::MAX_NAME {
                return Err(format!("profile.cameraProfile must be 1..={} chars", ProfileSettings::MAX_NAME));
            }
        }
        if let Some(look) = &self.profile.look {
            check("profile.look.amount", look.amount, 0.0, 2.0)?;
            if !LookSettings::is_valid_uuid(&look.uuid) {
                return Err(format!("profile.look.uuid {:?} is not 32 upper-case hex digits", look.uuid));
            }
            if look.name.chars().count() > ProfileSettings::MAX_NAME {
                return Err(format!("profile.look.name must be at most {} chars", ProfileSettings::MAX_NAME));
            }
        }
        validate_masks(&self.masks)?;
        self.transform.validate()?;
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
                AdjustmentField::ToneCurve => self.tone_curve = src.tone_curve.clone(),
                AdjustmentField::ColorGrading => self.color_grading = src.color_grading,
                AdjustmentField::Calibration => self.calibration = src.calibration,
                AdjustmentField::Sharpening => self.detail.sharpening = src.detail.sharpening,
                AdjustmentField::NoiseReduction => self.detail.noise_reduction = src.detail.noise_reduction,
                AdjustmentField::NoiseReductionLuminance => {
                    let (d, s) = (&mut self.detail.noise_reduction, &src.detail.noise_reduction);
                    d.luminance = s.luminance;
                    d.luminance_detail = s.luminance_detail;
                    d.luminance_contrast = s.luminance_contrast;
                }
                AdjustmentField::NoiseReductionColor => {
                    let (d, s) = (&mut self.detail.noise_reduction, &src.detail.noise_reduction);
                    d.color = s.color;
                    d.color_detail = s.color_detail;
                    d.color_smoothness = s.color_smoothness;
                }
                AdjustmentField::ProcessVersion => self.process_version = src.process_version,
                AdjustmentField::Vignette => self.effects.vignette = src.effects.vignette,
                AdjustmentField::Grain => self.effects.grain = src.effects.grain,
                AdjustmentField::BlackAndWhite => self.black_and_white = src.black_and_white,
                AdjustmentField::Crop => self.crop = src.crop,
                AdjustmentField::Profile => self.profile = src.profile.clone(),
                // AI mattes belong to the source image: the target recomputes them.
                AdjustmentField::Masks => self.masks = src.masks.iter().map(MaskGroup::transferable).collect(),
                AdjustmentField::Transform => self.transform = src.transform.clone(),
            }
        }
    }

    /// Equal to the RAW defaults (`default()`). For a specific image use
    /// [`Self::is_neutral_for`] (non-RAW sources have different Detail defaults).
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
    /// - v9 groups: numbers linear; colour-wheel hues along the shorter arc (a wheel with
    ///   saturation 0 takes the other side's hue); point curves point-wise when both have the
    ///   same number of points, else the nearer side's curve; booleans, `vignette.style`,
    ///   `crop` unless both enabled: the nearer side's; `profile`: look amount linear when both
    ///   sides have the same camera profile and look, else the nearer side's.
    /// - v10 `masks`: the nearer side's (never interpolated).
    /// - v19 `transform`: the nearer side's (per-frame geometry, never interpolated).
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
            tone_curve: {
                let (pa, pb) = (&a.tone_curve.parametric, &b.tone_curve.parametric);
                ToneCurve {
                    parametric: ParametricCurve {
                        shadows: l(pa.shadows, pb.shadows),
                        darks: l(pa.darks, pb.darks),
                        lights: l(pa.lights, pb.lights),
                        highlights: l(pa.highlights, pb.highlights),
                        shadow_split: l(pa.shadow_split, pb.shadow_split),
                        midtone_split: l(pa.midtone_split, pb.midtone_split),
                        highlight_split: l(pa.highlight_split, pb.highlight_split),
                    },
                    point: PointCurves::lerp(&a.tone_curve.point, &b.tone_curve.point, t),
                }
            },
            color_grading: {
                let (ga, gb) = (&a.color_grading, &b.color_grading);
                ColorGrading {
                    shadows: ColorWheel::lerp(&ga.shadows, &gb.shadows, t),
                    midtones: ColorWheel::lerp(&ga.midtones, &gb.midtones, t),
                    highlights: ColorWheel::lerp(&ga.highlights, &gb.highlights, t),
                    global: ColorWheel::lerp(&ga.global, &gb.global, t),
                    blending: l(ga.blending, gb.blending),
                    balance: l(ga.balance, gb.balance),
                }
            },
            calibration: {
                let (ca, cb) = (&a.calibration, &b.calibration);
                let p = |x: &PrimaryCalibration, y: &PrimaryCalibration| PrimaryCalibration {
                    hue: l(x.hue, y.hue),
                    saturation: l(x.saturation, y.saturation),
                };
                CameraCalibration {
                    red: p(&ca.red, &cb.red),
                    green: p(&ca.green, &cb.green),
                    blue: p(&ca.blue, &cb.blue),
                    shadow_tint: l(ca.shadow_tint, cb.shadow_tint),
                }
            },
            detail: {
                let (sa, sb) = (&a.detail.sharpening, &b.detail.sharpening);
                let (na, nb) = (&a.detail.noise_reduction, &b.detail.noise_reduction);
                DetailAdjustments {
                    sharpening: Sharpening {
                        amount: l(sa.amount, sb.amount),
                        radius: l(sa.radius, sb.radius),
                        detail: l(sa.detail, sb.detail),
                        masking: l(sa.masking, sb.masking),
                    },
                    noise_reduction: NoiseReduction {
                        luminance: l(na.luminance, nb.luminance),
                        luminance_detail: l(na.luminance_detail, nb.luminance_detail),
                        luminance_contrast: l(na.luminance_contrast, nb.luminance_contrast),
                        color: l(na.color, nb.color),
                        color_detail: l(na.color_detail, nb.color_detail),
                        color_smoothness: l(na.color_smoothness, nb.color_smoothness),
                    },
                }
            },
            effects: {
                let (va, vb) = (&a.effects.vignette, &b.effects.vignette);
                let (ga, gb) = (&a.effects.grain, &b.effects.grain);
                EffectsAdjustments {
                    vignette: PostCropVignette {
                        amount: l(va.amount, vb.amount),
                        midpoint: l(va.midpoint, vb.midpoint),
                        roundness: l(va.roundness, vb.roundness),
                        feather: l(va.feather, vb.feather),
                        highlights: l(va.highlights, vb.highlights),
                        style: if near_b { vb.style } else { va.style },
                    },
                    grain: Grain {
                        amount: l(ga.amount, gb.amount),
                        size: l(ga.size, gb.size),
                        roughness: l(ga.roughness, gb.roughness),
                    },
                }
            },
            black_and_white: BlackAndWhite {
                enabled: if near_b { b.black_and_white.enabled } else { a.black_and_white.enabled },
                mixer: HslChannels::lerp(&a.black_and_white.mixer, &b.black_and_white.mixer, t),
            },
            crop: {
                let (ca, cb) = (&a.crop, &b.crop);
                if ca.enabled && cb.enabled {
                    CropSettings {
                        enabled: true,
                        top: l(ca.top, cb.top),
                        left: l(ca.left, cb.left),
                        bottom: l(ca.bottom, cb.bottom),
                        right: l(ca.right, cb.right),
                        angle: l(ca.angle, cb.angle),
                    }
                } else if near_b {
                    *cb
                } else {
                    *ca
                }
            },
            profile: ProfileSettings::lerp(&a.profile, &b.profile, t),
            masks: if near_b { b.masks.clone() } else { a.masks.clone() },
            transform: if near_b { b.transform.clone() } else { a.transform.clone() },
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
        /// `toneCurve` (parametric + point curves) (v9).
        ToneCurve => "tone_curve",
        /// `colorGrading` (all wheels, blending, balance) (v9).
        ColorGrading => "color_grading",
        /// `calibration` (v9).
        Calibration => "calibration",
        /// `detail.sharpening` (v9).
        Sharpening => "sharpening",
        /// `detail.noiseReduction` (v9).
        NoiseReduction => "noise_reduction",
        /// `effects.vignette` (post-crop) (v9).
        Vignette => "vignette",
        /// `effects.grain` (v9).
        Grain => "grain",
        /// `blackAndWhite` (treatment + mixer) (v9).
        BlackAndWhite => "black_and_white",
        /// `crop` (v9). Not in [`AdjustmentField::DEFAULT_SYNC`].
        Crop => "crop",
        /// `profile` (camera profile + look) (v9).
        Profile => "profile",
        /// `masks` (all mask groups; AI mattes are recomputed on the target) (v10). Not in
        /// [`AdjustmentField::DEFAULT_SYNC`].
        Masks => "masks",
        /// `detail.noiseReduction` luminance, luminanceDetail, luminanceContrast only (v14,
        /// Lightroom's "Luminance Noise Reduction" copy item). Subset of `noise_reduction`.
        NoiseReductionLuminance => "noise_reduction_luminance",
        /// `detail.noiseReduction` color, colorDetail, colorSmoothness only (v14, "Color
        /// Noise Reduction"). Subset of `noise_reduction`.
        NoiseReductionColor => "noise_reduction_color",
        /// `processVersion` (v14, Lightroom's "Process Version" copy item).
        ProcessVersion => "process_version",
        /// `transform` (v19: Upright mode, guides, solved Upright values and the manual
        /// Transform sliders; Lightroom's "Upright Mode" + "Upright Transforms" + "Transform
        /// Adjustments" copy items). Not in [`AdjustmentField::DEFAULT_SYNC`] (per-frame
        /// geometry, like `crop`).
        Transform => "transform",
    }
}

impl AdjustmentField {
    /// Every group except `crop` and `masks` (per-frame geometry; Lightroom's Sync/Copy
    /// default leaves them unchecked).
    /// Default of `MatchOptions.copyFields`; TS mirror `DEFAULT_SYNC_FIELDS`.
    pub const DEFAULT_SYNC: &'static [AdjustmentField] = &[
        AdjustmentField::WhiteBalance,
        AdjustmentField::Exposure,
        AdjustmentField::Contrast,
        AdjustmentField::Highlights,
        AdjustmentField::Shadows,
        AdjustmentField::Whites,
        AdjustmentField::Blacks,
        AdjustmentField::Texture,
        AdjustmentField::Clarity,
        AdjustmentField::Dehaze,
        AdjustmentField::Vibrance,
        AdjustmentField::Saturation,
        AdjustmentField::HslHue,
        AdjustmentField::HslSaturation,
        AdjustmentField::HslLuminance,
        AdjustmentField::Lut,
        AdjustmentField::ToneCurve,
        AdjustmentField::ColorGrading,
        AdjustmentField::Calibration,
        AdjustmentField::Sharpening,
        AdjustmentField::NoiseReduction,
        AdjustmentField::Vignette,
        AdjustmentField::Grain,
        AdjustmentField::BlackAndWhite,
        AdjustmentField::Profile,
        AdjustmentField::ProcessVersion,
    ];

    /// Sliders `auto_tone` can set (Lightroom's Basic "Auto": tone + presence) (v14).
    pub const AUTO_TONE: &'static [AdjustmentField] = &[
        AdjustmentField::Exposure,
        AdjustmentField::Contrast,
        AdjustmentField::Highlights,
        AdjustmentField::Shadows,
        AdjustmentField::Whites,
        AdjustmentField::Blacks,
        AdjustmentField::Vibrance,
        AdjustmentField::Saturation,
    ];

    /// `paste_previous` with `fields = null` (v14): everything but `masks` (AI mattes and
    /// brush strokes belong to one frame). Lightroom's "Previous" copies all settings.
    pub const PASTE_PREVIOUS: &'static [AdjustmentField] = &[
        AdjustmentField::WhiteBalance,
        AdjustmentField::Exposure,
        AdjustmentField::Contrast,
        AdjustmentField::Highlights,
        AdjustmentField::Shadows,
        AdjustmentField::Whites,
        AdjustmentField::Blacks,
        AdjustmentField::Texture,
        AdjustmentField::Clarity,
        AdjustmentField::Dehaze,
        AdjustmentField::Vibrance,
        AdjustmentField::Saturation,
        AdjustmentField::HslHue,
        AdjustmentField::HslSaturation,
        AdjustmentField::HslLuminance,
        AdjustmentField::Lut,
        AdjustmentField::ToneCurve,
        AdjustmentField::ColorGrading,
        AdjustmentField::Calibration,
        AdjustmentField::Sharpening,
        AdjustmentField::NoiseReduction,
        AdjustmentField::Vignette,
        AdjustmentField::Grain,
        AdjustmentField::BlackAndWhite,
        AdjustmentField::Crop,
        AdjustmentField::Profile,
        AdjustmentField::ProcessVersion,
        AdjustmentField::Transform,
    ];
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
        /// Mask overlays (`render_mask_overlay`, v10): grayscale JPEG mattes. Not accepted
        /// by `render_preview`.
        Mask => "mask",
        /// Navigator panel + preset/profile hover previews (v14): independent of `main`, so a
        /// hover never supersedes the loupe render.
        Navigator => "navigator",
        /// Temporary previews on the main image (v19, `render_preview_variant`): hover preset
        /// preview and press-and-hold "without this panel". Independent of `main`, so going
        /// back to the edit is just showing the last `main` URL again (no re-render).
        Preview => "preview",
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
        if self.slot == RenderSlot::Mask {
            return Err("slot \"mask\" is reserved for render_mask_overlay".into());
        }
        Self::validate_geometry(self.max_edge, self.region)
    }

    /// `maxEdge` / `region` rules shared with `MaskOverlayOptions`.
    pub fn validate_geometry(max_edge: u32, region: Option<NormRect>) -> Result<(), String> {
        if !(Self::MIN_EDGE..=Self::MAX_EDGE).contains(&max_edge) {
            return Err(format!("maxEdge = {} is outside {}..={}", max_edge, Self::MIN_EDGE, Self::MAX_EDGE));
        }
        if let Some(r) = region {
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
    /// Non-RAW sources (v9): always `{6500, 0}` (the decoded pixels are treated as white
    /// balanced for D65; a custom temperature/tint is applied relative to that).
    pub as_shot: Option<WhiteBalanceValues>,
    /// Size of the cached develop source (half-size demosaic), orientation applied.
    pub source_width: u32,
    pub source_height: u32,
    /// Full sensor output size, orientation applied (Phase 6 export size). Uncropped.
    pub full_width: u32,
    pub full_height: u32,
    /// Why the render may differ from Lightroom's (v9): `RawImageEntry.developWarnings`
    /// (sidecar features not rendered) + the stored `profile` resolved against the installed
    /// profiles (`profile_unavailable`, `look_unavailable`) + source facts
    /// (`source_color_assumed`). Recomputed per call.
    pub warnings: Vec<DevelopWarning>,
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
    /// v19: the preset last applied to this photo (`apply_preset`) while its settings still
    /// carry it: `null` once any setting the preset owns (its `fields`) differs from what the
    /// apply produced (a slider change, another preset, undo past the apply...). Redo back
    /// to the apply highlights it again. Drives the preset browser's highlight.
    #[serde(default)]
    pub applied_preset_id: Option<PresetId>,
}

/// Adjustments + history after an undo/redo/jump.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EditState {
    pub adjustments: ParametricAdjustments,
    pub history: AdjustmentHistory,
}

/// A develop preset: a user preset saved in Sieve (`save_preset`, group
/// [`USER_PRESETS_GROUP_ID`]) or one imported from Lightroom (`import_style_folder`, v14).
/// Applying a Sieve preset copies `adjustments` restricted to `fields`; applying an imported
/// preset sets exactly the `crs:` settings it contains (`settingKeys`, Lightroom semantics).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    pub id: PresetId,
    /// Unique (case-insensitive) within its group, 1..=100 chars.
    pub name: String,
    /// Defaults overlaid with the preset's settings (for imported presets: display only; use
    /// `resolve_preset` for what applying it to an image gives).
    pub adjustments: ParametricAdjustments,
    /// Non-empty; groups outside it are ignored when applying. For imported presets: the
    /// groups its `settingKeys` touch.
    pub fields: Vec<AdjustmentField>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    /// Style group (v14): [`USER_PRESETS_GROUP_ID`] for presets saved in Sieve.
    pub group_id: StyleGroupId,
    /// v14: `sieve` for presets saved in Sieve.
    pub source_format: StyleSourceFormat,
    /// `crs:` property names the preset sets (imported presets, v14; e.g. "Exposure2012",
    /// "ToneCurvePV2012", "Look"); empty for Sieve presets.
    pub setting_keys: Vec<String>,
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
            tone_curve: ToneCurve::default(),
            color_grading: ColorGrading::default(),
            calibration: CameraCalibration::default(),
            detail: DetailAdjustments::default(),
            effects: EffectsAdjustments::default(),
            black_and_white: BlackAndWhite::default(),
            crop: CropSettings::default(),
            profile: ProfileSettings::default(),
            masks: Vec::new(),
            transform: TransformSettings::default(),
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
        /// v20.1, Pick the best N: by target-run moment (moment start time, then moment id),
        /// within a moment by selection `score` (best first), then capture order. Photos
        /// without a moment come last, in capture order. Descending reverses the moment order
        /// only (the best frame of each moment stays first).
        TargetMoment => "target_moment",
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
    /// Who set the flag (v18.1; `null` = anyone). `user` = flagged (pick or reject) by the
    /// user (`RawImageEntry.pickOrigin` `user`, or a flag from before v18); `auto` = flagged
    /// by `apply_suggestions` and not changed since. Unflagged images never match, so with
    /// `picks = ["reject"]` and `auto` the query returns `CullSummary.rejectedAuto` images.
    #[serde(default)]
    pub pick_origin: Option<PickOrigin>,
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
    /// Only images whose original is missing (`missingSinceMs` set; IPC v13).
    #[serde(default)]
    pub missing_only: bool,
    pub folder_id: Option<FolderId>,
    /// Only images of this project (v14). Inside a project the UI always sets it; combined
    /// with `folderId` (AND) a folder of another project matches nothing.
    #[serde(default)]
    pub project_id: Option<ProjectId>,
    /// Only keepers under the catalog's `KeeperRule` (v15; the Edit / Export steps' grid).
    #[serde(default)]
    pub keepers_only: bool,
    /// Lightroom-style Library Filter "Metadata" constraints (v18; default: none).
    #[serde(default)]
    pub metadata: MetadataFilter,
    /// v19.2: only photos with a **pending** suggestion of this kind (`null` = no constraint):
    /// analysed, unflagged and 0 stars (what `apply_suggestions(onlyUnset = true)` would change),
    /// with `suggestedPick = reject` (`reject`), `pick` (`pick`), or no flag but
    /// `suggestedRating > 0` (`rating`). Counts: `CullSummary.suggested*Pending`,
    /// `FilterCounts.suggested*`.
    #[serde(default)]
    pub suggested: Option<PendingSuggestion>,
    /// v20: only photos whose target-run choice is one of these (empty = no constraint; photos
    /// without a selection row never match), e.g. `["not_sure", "set_aside"]` for pass 2.
    #[serde(default)]
    pub target_choices: Vec<TargetChoice>,
    /// v20.1: only photos whose first selection reason (`ImageSelection.reasons[0].kind`) is
    /// one of these (empty = no constraint; photos without a selection row never match).
    #[serde(default)]
    pub target_reason_kinds: Vec<TargetReasonKind>,
    /// v20.1: only photos in one of these second-look piles (`ImageSelection.pile`; empty =
    /// no constraint), e.g. `["not_sure"]` for the Second look's default pile.
    #[serde(default)]
    pub target_piles: Vec<TargetPile>,
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
            pick_origin: None,
            min_rating: None,
            max_rating: None,
            color_labels: Vec::new(),
            burst_group_id: None,
            scene_id: None,
            collapse_bursts: false,
            missing_only: false,
            folder_id: None,
            project_id: None,
            keepers_only: false,
            metadata: MetadataFilter::default(),
            suggested: None,
            target_choices: Vec::new(),
            target_reason_kinds: Vec::new(),
            target_piles: Vec::new(),
            sort: ImageSort::CaptureTime,
            sort_descending: false,
            offset: 0,
            limit: 200,
        }
    }
}

string_enum! {
    /// Kind of a pending culling suggestion (v19.2, `ImageQuery.suggested`).
    pub enum PendingSuggestion {
        /// Sieve suggests reject.
        Reject => "reject",
        /// Sieve suggests pick.
        Pick => "pick",
        /// No flag suggested, but stars (`suggestedRating > 0`).
        Rating => "rating",
    }
}

/// Inclusive numeric range (v18); a `null` bound is open. Images without the value never
/// match a range. Bounds are compared with a tiny tolerance (1e-6 relative), so a facet
/// value (`MetadataFilterOptions`) used as both bounds selects exactly that value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NumberRange {
    #[specta(type = Option<Number>)]
    pub min: Option<f64>,
    #[specta(type = Option<Number>)]
    pub max: Option<f64>,
}

/// Capture-time range (v18) in the naive ms of `CaptureMeta.capturedAtMs`: `fromMs` inclusive,
/// `toMs` exclusive (one day = `[dayStartMs, dayStartMs + 86_400_000)`); `null` = open.
/// Images without a capture time never match.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DateRange {
    pub from_ms: Option<i64>,
    pub to_ms: Option<i64>,
}

/// One camera body of the camera facet (v18): `model = null` = model unknown.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CameraFilter {
    pub make: CameraMake,
    pub model: Option<String>,
}

/// One camera body (v19.2): make + model + body serial. `model = null` = model unknown,
/// `serial = null` = serial unknown (not read yet, or the file has none). Matches exactly
/// (a `null` field matches only photos where that value is unknown).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CameraBody {
    pub make: CameraMake,
    pub model: Option<String>,
    pub serial: Option<String>,
}

/// Library Filter "Metadata" constraints (v18, `ImageQuery.metadata`, also accepted by
/// `get_filter_counts`). All fields optional on the wire; each set field narrows the result
/// (AND across fields, OR within a list). Values come from `get_metadata_filter_options`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", default)]
pub struct MetadataFilter {
    /// File type (Lightroom "File Type"): image's `format` is one of these.
    pub formats: Vec<ImageFormat>,
    /// File extension without the dot, case-insensitive (`"arw"`, `"jpg"`): `[A-Za-z0-9]{1,10}`
    /// each, else `invalid_argument`.
    pub extensions: Vec<String>,
    /// Camera body (make + model) is one of these.
    pub cameras: Vec<CameraFilter>,
    /// v19.2: camera body (make + model + serial, `CameraBody`) is one of these; the per-body
    /// refinement of `cameras` (two ILCE-7M4 bodies are two entries). Values come from
    /// `MetadataFilterOptions.bodies`.
    pub bodies: Vec<CameraBody>,
    /// Lens is one of these; `null` = lens unknown.
    pub lenses: Vec<Option<String>>,
    pub iso: Option<NumberRange>,
    /// Focal length in mm, compared at 0.1 mm (the facet's rounding).
    pub focal_length_mm: Option<NumberRange>,
    /// f-number, compared at 0.1 (the facet's rounding).
    pub aperture: Option<NumberRange>,
    /// Exposure time in seconds (1/250 s = 0.004).
    pub shutter_seconds: Option<NumberRange>,
    pub captured: Option<DateRange>,
    /// `true` = has develop edits (`RawImageEntry.hasEdits`), `false` = unedited.
    pub edited: Option<bool>,
    /// `true` = has an XMP sidecar (`XmpSyncState.hasSidecar`), `false` = none known.
    pub has_sidecar: Option<bool>,
}

impl MetadataFilter {
    /// No constraint set.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FormatCount {
    pub format: ImageFormat,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionCount {
    /// Lower case, without the dot.
    pub extension: String,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CameraCount {
    pub camera: CameraFilter,
    pub count: u32,
}

/// v19.2 `MetadataFilterOptions.bodies` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CameraBodyCount {
    pub body: CameraBody,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LensCount {
    /// `null` = unknown lens.
    pub lens: Option<String>,
    pub count: u32,
}

/// A distinct numeric value and its image count; `value = null` counts images without it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NumberCount {
    #[specta(type = Option<Number>)]
    pub value: Option<f64>,
    pub count: u32,
}

/// Images captured on one (naive, camera-local) day; `dayStartMs = null` counts images
/// without a capture time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DayCount {
    pub day_start_ms: Option<i64>,
    pub count: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct YesNoCount {
    pub yes: u32,
    pub no: u32,
}

/// `get_metadata_filter_options(query)` (v18): distinct values with image counts per facet.
/// Each facet is counted over the query's images with every constraint applied **except that
/// facet's own** (Lightroom's cascading columns), so a value's count is what selecting it
/// (alone) would show. Values with 0 images are omitted. Order: formats in `ImageFormat`
/// order; extensions, cameras, lenses by name (unknown last); numbers ascending (unknown
/// last); days ascending (unknown last).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MetadataFilterOptions {
    /// Images matching the query (all constraints).
    pub total: u32,
    pub formats: Vec<FormatCount>,
    pub extensions: Vec<ExtensionCount>,
    pub cameras: Vec<CameraCount>,
    /// v19.2: per body (make + model + serial), ignoring `metadata.bodies`; ordered like
    /// `cameras`, then by serial (unknown serial last). Label a body with its serial's last
    /// digits when two entries share make + model.
    #[serde(default)]
    pub bodies: Vec<CameraBodyCount>,
    pub lenses: Vec<LensCount>,
    pub isos: Vec<NumberCount>,
    /// Rounded to 0.1 mm.
    pub focal_lengths: Vec<NumberCount>,
    /// Rounded to 0.1.
    pub apertures: Vec<NumberCount>,
    /// Exact stored seconds.
    pub shutter_speeds: Vec<NumberCount>,
    pub capture_days: Vec<DayCount>,
    pub edited: YesNoCount,
    pub has_sidecar: YesNoCount,
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
    /// Also import non-RAW sources (JPEG, HEIC, TIFF, PNG) (v9). Default `false` (RAW only,
    /// the pre-v9 behaviour); the import dialog should offer it.
    #[serde(default)]
    pub include_non_raw: bool,
    /// With `includeNonRaw`: a JPEG/HEIC whose stem matches a RAW in the same directory
    /// (case-insensitive, e.g. `DSCF1234.RAF` + `DSCF1234.JPG`) is not imported as its own
    /// image but recorded as the RAW's `companionPath` (Lightroom's default "treat JPEG next to
    /// raw as separate photo" = off). `false` imports both as separate images. Default `true`.
    /// TIFF/PNG never pair.
    #[serde(default = "default_true")]
    pub pair_jpeg_with_raw: bool,
}

fn default_true() -> bool {
    true
}

impl ImportOptions {
    /// RAW only, recursive or not (the pre-v9 behaviour).
    pub fn raw_only(recursive: bool) -> Self {
        Self { recursive, include_non_raw: false, pair_jpeg_with_raw: true }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub folder_id: FolderId,
    /// Project the folder belongs to (v14; a new folder gets a new project unless
    /// `import_folder` was given one).
    pub project_id: ProjectId,
    /// New images added to the catalog.
    pub added: u32,
    /// Supported files already in the catalog.
    pub skipped: u32,
    /// Files with a supported extension (RAW, or non-RAW when included) whose header did not
    /// match the format.
    pub invalid: u32,
    /// Existing XMP sidecars whose rating/pick/label were read into the catalog
    /// (new images, and unchanged images whose sidecar changed on disk).
    pub sidecars_read: u32,
    /// JPEG/HEIC siblings recorded as a RAW's `companionPath` instead of being added (v9).
    pub companions: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FolderEntry {
    pub id: FolderId,
    pub path: String,
    pub image_count: u32,
    /// Project this folder belongs to (v14; every folder belongs to exactly one).
    pub project_id: ProjectId,
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
    /// Images whose original is missing (`ImageQuery.missingOnly`; IPC v13).
    pub missing: u32,
    /// v19.2: pending suggestions (`ImageQuery.suggested`) in the counted images: reject /
    /// pick / stars only. Over a project without other constraints they equal
    /// `CullSummary.suggestedRejectPending` / `suggestedPickPending` / `suggestedRatingPending`.
    #[serde(default)]
    pub suggested_reject: u32,
    #[serde(default)]
    pub suggested_pick: u32,
    #[serde(default)]
    pub suggested_rating: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CatalogState {
    pub catalog_path: String,
    pub image_count: u32,
    /// Catalog default shoot type: new projects start with it (v14; each project has its own,
    /// `Project.shootType`, which is what culling scores with).
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
    /// Sidecars are written automatically (debounced) after rating/pick/label/tag and develop
    /// changes (`set_xmp_auto_sync`). Default **on** since v14 (migration 0012 turned it on for
    /// catalogs whose setting was never changed by the user).
    pub xmp_auto_sync: bool,
    /// Integrity of the catalog as found at launch, and its backups (IPC v13).
    pub health: CatalogHealth,
    /// Which images count as keepers (edit plan, export step, project counts) (v14;
    /// `set_keeper_rule`).
    pub keeper_rule: KeeperRule,
}

string_enum! {
    /// Catalog state found by the launch check (`db` module docs).
    pub enum CatalogHealthStatus {
        /// Healthy (or the check was skipped after a clean shutdown).
        Ok => "ok",
        /// The integrity check failed: the catalog was opened read-only; every write fails
        /// with `catalog_read_only`. Restore a backup (`restore_catalog_backup`).
        ReadOnly => "read_only",
        /// The file was not a database: it was moved aside and a new, empty catalog was
        /// created. A backup can be restored (`restore_catalog_backup`).
        Replaced => "replaced",
    }
}

/// `CatalogState.health` (IPC v13).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CatalogHealth {
    pub status: CatalogHealthStatus,
    /// User-facing explanation and remedy; `null` when `ok`.
    pub message: Option<String>,
    /// Automatic backups (`<catalog>.bak-N`), newest first.
    pub backups: Vec<CatalogBackup>,
    /// A backup was staged by `restore_catalog_backup`; it replaces the catalog at the next
    /// launch ("Relaunch to finish"). Changes made until then are lost.
    pub restore_pending: bool,
}

impl Default for CatalogHealth {
    fn default() -> Self {
        Self { status: CatalogHealthStatus::Ok, message: None, backups: Vec::new(), restore_pending: false }
    }
}

/// One automatic catalog backup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CatalogBackup {
    /// 1 = newest; the argument of `restore_catalog_backup`.
    pub index: u32,
    pub path: String,
    /// When the backup was taken (file modification time).
    pub created_at_ms: i64,
    pub size_bytes: u64,
}

/// Result of `relocate_folder` (IPC v13).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RelocateResult {
    /// Images of the folder found in the new location (paths repointed, missing flag
    /// cleared).
    pub matched: u32,
    /// Images not found there: their path is unchanged and they are flagged missing.
    pub still_missing: u32,
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
    /// The project every image of the job belongs to (v14; `null` = images of several
    /// projects, or a job from before v14). Drives the Export step's "Exported N".
    pub project_id: Option<ProjectId>,
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
        /// `apply_scene_edit` / `apply_all_edited_scenes` (v14); `total` = target images.
        Apply => "apply",
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
    /// Default (v9): `AdjustmentField::DEFAULT_SYNC` (everything but `crop`).
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
            copy_fields: AdjustmentField::DEFAULT_SYNC.to_vec(),
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

// ---------------------------------------------------------------------------
// UX additions (IPC v8)
// ---------------------------------------------------------------------------

/// Which suggestions `apply_suggestions` copies (v19.2; `null` = all). A suggested `pick` flag
/// is copied with `picks`, a suggested `reject` with `rejects`, a suggested "no flag" (which
/// clears a flag, only possible with `onlyUnset = false`) only with both; `suggestedRating`
/// is copied with `stars`. Anything not selected stays as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionKinds {
    pub picks: bool,
    pub rejects: bool,
    pub stars: bool,
}

impl Default for SuggestionKinds {
    fn default() -> Self {
        Self { picks: true, rejects: true, stars: true }
    }
}

/// Result of `apply_suggestions`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ApplySuggestionsResult {
    /// Images whose rating and/or pick changed (v18.1: an image already matching its
    /// suggestion is not counted, it counts as skipped).
    pub applied: u32,
    /// Images left alone: unanalyzed, already matching their suggestion, or (with
    /// `onlyUnset`) already flagged or rated.
    pub skipped: u32,
}

/// The user's culling values of one image, for a frontend culling undo stack:
/// `get_cull_snapshot` before a change, `restore_cull_snapshot` to undo it.
/// Tags are not included (undo a tag change with the inverse `set_user_tag`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CullSnapshot {
    pub image_id: ImageId,
    /// 0..=5.
    pub rating: u8,
    pub pick: PickFlag,
    pub color_label: Option<ColorLabel>,
    /// Who set `pick` (v18; `RawImageEntry.pickOrigin`). Restored with the flag, so undoing a
    /// user flag over an "Auto" reject brings back an `auto` reject. Missing / `null` restores
    /// as `user`.
    #[serde(default)]
    pub pick_origin: Option<PickOrigin>,
}

/// Small per-catalog UI preferences. Every field is optional so the struct can grow;
/// `set_ui_prefs` replaces the whole value (read-modify-write from the frontend).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", default)]
pub struct UiPrefs {
    /// Folder last chosen in the export dialog (absolute path).
    pub last_export_folder: Option<String>,
    /// Last selection in the Copy... dialog (v14; "remembers last choice"). `null` = default
    /// (`DEFAULT_SYNC_FIELDS`).
    pub copy_fields: Option<Vec<AdjustmentField>>,
    /// The one-time "how Sieve reads and merges XMP sidecars" explanation was shown (v14).
    pub xmp_explainer_seen: Option<bool>,
    /// Scene strip visible (v14 scenes toggle); `null` = shown.
    pub scene_strip_visible: Option<bool>,
}

// ---------------------------------------------------------------------------
// Model downloads (IPC v12)
// ---------------------------------------------------------------------------

/// Model group id accepted by `download_models`. Currently only `"segmentation"` (the AI-mask
/// models, ~560 MB); the culling models ship with the app.
pub const MODEL_GROUP_SEGMENTATION: &str = "segmentation";

/// One model file of a [`ModelGroupStatus`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ModelFileStatus {
    /// File name in the models directory (e.g. `birefnet_lite.onnx`).
    pub name: String,
    /// Present with the expected size (files are checksum-verified before they are moved into place).
    pub installed: bool,
    /// Download size in bytes.
    pub bytes: u64,
}

/// A downloadable set of models.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ModelGroupStatus {
    /// Pass to `download_models` (e.g. `"segmentation"`).
    pub id: String,
    /// User-facing name (e.g. "AI masking models").
    pub label: String,
    /// Every file installed.
    pub installed: bool,
    /// Sum of `files[].bytes`.
    pub bytes_total: u64,
    pub files: Vec<ModelFileStatus>,
}

/// `model_downloads_status()`: what is installed and what is downloading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ModelDownloadStatus {
    pub groups: Vec<ModelGroupStatus>,
    /// Group id of the download in flight (at most one at a time); `null` when idle.
    pub downloading: Option<String>,
}

// ---------------------------------------------------------------------------
// IPC v14 (Phase 8b): style library (presets + profiles), auto tone / WB, guided workflow,
// per-scene edit plan, edit batches, style model, Copy Settings groups.
// ---------------------------------------------------------------------------

/// Style group of the presets saved in Sieve (`save_preset`). Created by migration 0012.
pub const USER_PRESETS_GROUP_ID: StyleGroupId = 1;
/// Style group of the pre-v14 LUT library (`<app_data>/luts`, `import_lut`): LUT files not
/// imported through a folder appear here. Created by migration 0012.
pub const LUT_LIBRARY_GROUP_ID: StyleGroupId = 2;

string_enum! {
    /// Kind of a style group.
    pub enum StyleGroupKind {
        /// "User Presets" ([`USER_PRESETS_GROUP_ID`]): presets saved in Sieve. Not removable.
        User => "user",
        /// One source folder of an `import_style_folder` run. Removable.
        Imported => "imported",
        /// "LUTs" ([`LUT_LIBRARY_GROUP_ID`]): the pre-v14 LUT library. Not removable.
        Luts => "luts",
    }
}

string_enum! {
    /// What selecting a [`StyleProfile`] changes (see [`StyleProfile::apply_to`]).
    pub enum StyleProfileKind {
        /// Creative / look profile (`.xmp` with `crs:PresetType="Look"`: RGB/Look tables,
        /// optional Amount): sets `profile.look` (+ `profile.cameraProfile` when the look names
        /// one) and clears `lut`.
        Look => "look",
        /// Camera profile (`.dcp`): sets `profile.cameraProfile`, clears `profile.look` and `lut`.
        CameraProfile => "camera_profile",
        /// `.cube` LUT: sets `lut = { id, amount }`; the camera profile and look stay (a LUT
        /// expects a rendered image).
        Lut => "lut",
    }
}

string_enum! {
    /// File type a preset or profile was read from.
    pub enum StyleSourceFormat {
        /// Saved in Sieve (`save_preset`) or a LUT imported with `import_lut`.
        Sieve => "sieve",
        /// Lightroom / Camera Raw develop preset `.xmp` (`crs:PresetType="Normal"`).
        XmpPreset => "xmp_preset",
        /// Legacy Lightroom Classic develop preset `.lrtemplate` (Lua table `s = { ... value =
        /// { settings = { ... } } }`).
        Lrtemplate => "lrtemplate",
        /// Creative profile `.xmp` (`crs:PresetType="Look"`).
        XmpProfile => "xmp_profile",
        /// DNG camera profile `.dcp`.
        Dcp => "dcp",
        /// `.cube` LUT.
        Cube => "cube",
    }
}

/// A preset in the style library (summary; `resolve_preset` gives its effect on an image,
/// `apply_preset` applies it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StylePreset {
    /// `Preset.id` (same id space as `list_presets` / `apply_preset`).
    pub id: PresetId,
    pub group_id: StyleGroupId,
    /// `crs:Name` (imported), else the file name without extension; unique within the group
    /// (duplicates get " (2)", " (3)"...).
    pub name: String,
    pub source_format: StyleSourceFormat,
    /// File it was imported from (absolute; informational, the settings are stored in the
    /// catalog); `null` for Sieve presets.
    pub source_path: Option<String>,
    /// Groups the preset touches (drives the Copy/preset checkboxes and search).
    pub fields: Vec<AdjustmentField>,
    /// `crs:` properties it sets (empty for Sieve presets).
    pub setting_keys: Vec<String>,
    /// `crs:SupportsAmount` of the preset file (informational; v14 applies presets at 100%).
    pub supports_amount: bool,
    /// Settings found in the file that Sieve ignores (unsupported keys such as lens profiles,
    /// retouch, local corrections of pre-2021 presets), user-facing.
    pub warnings: Vec<String>,
}

/// A profile in the style library: an imported creative profile, camera profile or LUT.
/// Imported `.xmp` looks and `.dcp` files are **read in place** at `sourcePath` (never copied,
/// see `profiles` module docs); `.cube` files are copied into the LUT library (`lutId`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StyleProfile {
    pub id: StyleProfileId,
    pub group_id: StyleGroupId,
    pub kind: StyleProfileKind,
    /// Look `crs:Name`, DCP `ProfileName`, LUT `TITLE` or file name.
    pub name: String,
    pub source_format: StyleSourceFormat,
    /// Look/DCP: the file read in place; LUT: the library copy.
    pub source_path: String,
    /// The file is readable now (a removed drive or deleted folder makes it `false`; images
    /// using it render without it and report `look_unavailable` / `profile_unavailable` /
    /// `lutMissing`).
    pub available: bool,
    /// The Amount slider applies (looks with `crs:SupportsAmount`, every LUT); range 0..=200%.
    pub supports_amount: bool,
    /// Converts to monochrome (look with `crs:ConvertToGrayscale`).
    pub monochrome: bool,
    /// Look: `crs:CameraProfile` it is built on (selecting it sets `profile.cameraProfile`);
    /// DCP: its `ProfileName` (= `crs:CameraProfile` value). `null` for LUTs / looks without one.
    pub camera_profile: Option<String>,
    /// DCP `UniqueCameraModel` / look `crs:CameraModelRestriction` (Adobe model name, e.g.
    /// "Sony ILCE-7M4"); `null` = any camera. Per-image availability: `list_profiles(id)`.
    pub camera_model: Option<String>,
    /// Look: `crs:UUID` (= `LookSettings.uuid`).
    pub look_uuid: Option<String>,
    /// LUT: library id (= `LutRef.id`).
    pub lut_id: Option<LutId>,
}

impl StyleProfile {
    /// Default Amount in percent (0..=200) when a profile is selected.
    pub const DEFAULT_AMOUNT: f32 = 100.0;

    /// `adj` with this profile selected at `amount` percent (0..=200; ignored = 100 unless
    /// `supportsAmount`). Mirrored by `applyStyleProfile` in `src/ipc/index.ts` (hover preview
    /// renders use it); keep both in sync.
    /// - `look`: `profile.look = { name, uuid, amount: amount / 100 }`, `profile.cameraProfile =
    ///   cameraProfile` when set (else unchanged), `lut = null`.
    /// - `camera_profile`: `profile.cameraProfile = cameraProfile ?? name`, `profile.look = null`,
    ///   `lut = null`.
    /// - `lut`: `lut = { id: lutId, amount }`; `profile` unchanged.
    pub fn apply_to(&self, adj: &ParametricAdjustments, amount: f32) -> ParametricAdjustments {
        let mut out = adj.clone();
        let pct = if self.supports_amount && amount.is_finite() { amount.clamp(0.0, 200.0) } else { 100.0 };
        match self.kind {
            StyleProfileKind::Look => {
                if let Some(uuid) = &self.look_uuid {
                    out.profile.look =
                        Some(LookSettings { name: self.name.clone(), uuid: uuid.clone(), amount: pct / 100.0 });
                }
                if let Some(cp) = &self.camera_profile {
                    out.profile.camera_profile = Some(cp.clone());
                }
                out.lut = None;
            }
            StyleProfileKind::CameraProfile => {
                out.profile.camera_profile = Some(self.camera_profile.clone().unwrap_or_else(|| self.name.clone()));
                out.profile.look = None;
                out.lut = None;
            }
            StyleProfileKind::Lut => {
                if let Some(id) = &self.lut_id {
                    out.lut = Some(LutRef { id: id.clone(), amount: pct });
                }
            }
        }
        out
    }
}

/// A folder of presets/profiles (grouped by source folder name, Lightroom-style), the user
/// presets, or the LUT library. Catalog-wide: every project sees every group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StyleGroup {
    pub id: StyleGroupId,
    /// Source folder name ("User Presets" / "LUTs" for the built-in groups).
    pub name: String,
    pub kind: StyleGroupKind,
    /// Absolute source folder (imported groups; re-importing the same folder replaces the
    /// group's items); `null` for the built-in groups.
    pub source_path: Option<String>,
    pub imported_at_ms: Option<i64>,
    /// By name (case-insensitive).
    pub presets: Vec<StylePreset>,
    /// By name (case-insensitive).
    pub profiles: Vec<StyleProfile>,
}

/// `list_styles()`: every group. Order: "User Presets", imported groups by name, "LUTs".
/// Empty built-in groups are included (the UI may hide them).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StyleLibrary {
    pub groups: Vec<StyleGroup>,
}

/// A file `import_style_folder` did not import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StyleImportSkip {
    pub path: String,
    /// User-facing, e.g. "not a develop preset (External Editor preset)", "unreadable DCP",
    /// "invalid .cube: LUT_3D_SIZE missing".
    pub reason: String,
}

/// Result of `import_style_folder`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportStyleReport {
    /// The folder that was imported (absolute).
    pub root: String,
    /// Groups created or replaced (one per folder containing at least one importable file).
    pub group_ids: Vec<StyleGroupId>,
    pub presets: u32,
    pub profiles: u32,
    /// Files with a supported extension that could not be imported (other files are ignored
    /// silently).
    pub skipped: Vec<StyleImportSkip>,
}

/// Values of Lightroom's Basic "Auto" (`auto_tone`): absolute slider values for the
/// requested sliders, `null` for sliders not requested.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AutoToneValues {
    #[specta(type = Option<Number>)]
    pub exposure: Option<f32>,
    #[specta(type = Option<Number>)]
    pub contrast: Option<f32>,
    #[specta(type = Option<Number>)]
    pub highlights: Option<f32>,
    #[specta(type = Option<Number>)]
    pub shadows: Option<f32>,
    #[specta(type = Option<Number>)]
    pub whites: Option<f32>,
    #[specta(type = Option<Number>)]
    pub blacks: Option<f32>,
    #[specta(type = Option<Number>)]
    pub vibrance: Option<f32>,
    #[specta(type = Option<Number>)]
    pub saturation: Option<f32>,
}

impl AutoToneValues {
    /// `adj` with every non-null value set. Mirrored by `applyAutoTone` in `src/ipc/index.ts`.
    pub fn apply_to(&self, adj: &ParametricAdjustments) -> ParametricAdjustments {
        let mut out = adj.clone();
        let set = |slot: &mut f32, v: Option<f32>| {
            if let Some(v) = v {
                *slot = v;
            }
        };
        set(&mut out.exposure, self.exposure);
        set(&mut out.contrast, self.contrast);
        set(&mut out.highlights, self.highlights);
        set(&mut out.shadows, self.shadows);
        set(&mut out.whites, self.whites);
        set(&mut out.blacks, self.blacks);
        set(&mut out.vibrance, self.vibrance);
        set(&mut out.saturation, self.saturation);
        out
    }
}

string_enum! {
    /// Guided-workflow step of a project: the step bar Cull -> Edit -> Export (v14).
    pub enum WorkflowStep {
        Cull => "cull",
        Edit => "edit",
        Export => "export",
    }
}

string_enum! {
    /// How [`KeeperRule`] decides (v18).
    pub enum KeeperMode {
        /// Every photo that is not rejected is a keeper (picked or unflagged; stars and
        /// suggestions do not matter). The default since v18 (user decision 2026-10-03).
        NotRejected => "not_rejected",
        /// The pre-v18 rule: picks, unflagged photos rated `>= minRating`, and (with
        /// `useSuggestions`) untouched photos the engine suggests `pick`.
        PicksAndRatings => "picks_and_ratings",
    }
}

/// Which images are keepers (Edit step scenes, Export step selection, cull summary). One
/// definition for the whole app ([`KeeperRule::is_keeper`], SQL mirror
/// `repo::keeper_predicate`, TS mirror `isKeeper`):
/// 1. rejected (by the user or by `apply_suggestions`) -> never;
/// 2. picked -> keeper;
/// 3. mode `not_rejected` (default): every other (unflagged) photo -> keeper;
/// 4. mode `picks_and_ratings`: unflagged and rated `>= minRating` -> keeper; unflagged with
///    0 stars and `useSuggestions` -> keeper iff the culling engine suggests `pick`
///    (`QualityScore.suggestedPick`; burst non-keepers are never suggested `pick`); otherwise
///    not a keeper.
///
/// `minRating` / `useSuggestions` are kept (and validated) in both modes, so switching back
/// to `picks_and_ratings` restores the user's thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct KeeperRule {
    /// v18. Default `not_rejected`.
    pub mode: KeeperMode,
    /// `picks_and_ratings` only. 1..=5. Default 1 (any star keeps, Lightroom convention).
    pub min_rating: u8,
    /// `picks_and_ratings` only. Default `true`.
    pub use_suggestions: bool,
}

impl Default for KeeperRule {
    fn default() -> Self {
        Self { mode: KeeperMode::NotRejected, min_rating: 1, use_suggestions: true }
    }
}

impl KeeperRule {
    /// A `picks_and_ratings` rule (the pre-v18 behaviour) with these thresholds.
    pub fn picks_and_ratings(min_rating: u8, use_suggestions: bool) -> Self {
        Self { mode: KeeperMode::PicksAndRatings, min_rating, use_suggestions }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(1..=5).contains(&self.min_rating) {
            return Err(format!("minRating = {} is outside 1..=5", self.min_rating));
        }
        Ok(())
    }

    /// The rule on the culling values (SQL mirror `repo::keeper_predicate`).
    pub fn is_keeper_values(&self, pick: PickFlag, rating: u8, suggested_pick: Option<PickFlag>) -> bool {
        match (pick, self.mode) {
            (PickFlag::Reject, _) => false,
            (PickFlag::Pick, _) => true,
            (PickFlag::Unflagged, KeeperMode::NotRejected) => true,
            (PickFlag::Unflagged, KeeperMode::PicksAndRatings) if rating >= self.min_rating => true,
            (PickFlag::Unflagged, KeeperMode::PicksAndRatings) => {
                rating == 0 && self.use_suggestions && suggested_pick == Some(PickFlag::Pick)
            }
        }
    }

    pub fn is_keeper(&self, e: &RawImageEntry) -> bool {
        self.is_keeper_values(e.pick, e.rating, e.quality.as_ref().map(|q| q.suggested_pick))
    }
}

/// How the keepers of a [`CullSummary`] are made up under its `keeperRule` (v18). The parts
/// are disjoint and add up to `CullSummary.keepers`:
/// - `not_rejected`: `picked + unflagged` (`starred = suggested = 0`);
/// - `picks_and_ratings`: `picked + starred + suggested` (`unflagged = 0`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct KeeperBreakdown {
    /// Picked photos (always keepers).
    pub picked: u32,
    /// `not_rejected`: unflagged photos (any stars).
    pub unflagged: u32,
    /// `picks_and_ratings`: unflagged photos rated `>= minRating`.
    pub starred: u32,
    /// `picks_and_ratings` with `useSuggestions`: unflagged 0-star photos the engine suggests
    /// `pick`.
    pub suggested: u32,
}

/// `get_cull_summary(projectId)` (v18): the Cull step readout "picked / unflagged / rejected /
/// keepers = formula". `total = picked + unflagged + rejected`; `keepers` equals the number of
/// images a `keepersOnly` query over the same scope returns.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CullSummary {
    pub total: u32,
    pub picked: u32,
    /// Of `picked`, flagged by `apply_suggestions` ("Auto") and not changed by the user since
    /// (`RawImageEntry.pickOrigin = auto`).
    pub picked_auto: u32,
    pub unflagged: u32,
    pub rejected: u32,
    /// Of `rejected`, rejected by the user (flag keys, sidecar reads, undo of a user flag).
    pub rejected_by_user: u32,
    /// Of `rejected`, rejected by `apply_suggestions` and not changed by the user since.
    pub rejected_auto: u32,
    /// Photos rated 1..=5 stars (any flag).
    pub starred: u32,
    pub keepers: u32,
    pub keeper_breakdown: KeeperBreakdown,
    /// The rule `keepers` was counted with (= `CatalogState.keeperRule`).
    pub keeper_rule: KeeperRule,
    /// What Apply suggestions would change with its default settings, part 1 (v18.1):
    /// untouched photos (analysed, unflagged **and** rated 0) the engine suggests `reject`.
    /// `apply_suggestions(every image in scope, onlyUnset = true)` changes exactly
    /// `suggestedRejectPending + suggestedPickPending + suggestedRatingPending` photos.
    pub suggested_reject_pending: u32,
    /// Part 2 (v18.1): untouched photos the engine suggests `pick`.
    pub suggested_pick_pending: u32,
    /// Part 3 (v18.1): untouched photos the engine suggests no flag for but stars
    /// (`suggestedRating > 0`). Untouched photos whose suggestion is "unflagged, 0 stars"
    /// would not change and are not counted anywhere.
    pub suggested_rating_pending: u32,
    /// Photos without a `QualityScore` yet (not analysed, or analysis failed).
    pub unanalyzed: u32,
}

string_enum! {
    /// Progress of one scene in the Edit step checklist.
    pub enum SceneEditStatus {
        /// The representative has no edits yet.
        ToEdit => "to_edit",
        /// The representative is edited; not applied to the rest of the scene yet.
        Edited => "edited",
        /// Applied, and the representative has not changed since.
        Applied => "applied",
        /// Applied, but the representative was edited again since (apply again).
        Outdated => "outdated",
        /// The scene was applied, but its representative has no edits any more (v17: reset,
        /// or its edit undone, after the apply). The members keep the look of the last apply
        /// (`appliedBatch` stays undoable when nothing blocks it). A to-do scene: edit the
        /// representative, then apply; not applied by `apply_all_edited_scenes`.
        Reset => "reset",
    }
}

string_enum! {
    /// Who chose a scene's representative.
    pub enum RepresentativeSource {
        /// Proposed by Sieve (best keeper with the most typical lighting of the scene).
        Auto => "auto",
        /// Chosen with `set_scene_representative`; kept while it is a keeper member.
        User => "user",
    }
}

/// One scene of the Edit step (`EditPlan.scenes`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SceneEditEntry {
    pub scene_id: SceneId,
    /// The scene's keepers, capture order (non-empty: scenes without keepers are omitted).
    pub image_ids: Vec<ImageId>,
    /// All members incl. non-keepers.
    pub member_count: u32,
    /// The frame to edit (a keeper of `imageIds`).
    pub representative_id: ImageId,
    pub representative_source: RepresentativeSource,
    /// User-facing, e.g. "Sharpest keeper, typical light for this scene" or "Chosen by you".
    pub representative_reason: String,
    /// The representative has edits (`hasEdits`).
    pub edited: bool,
    /// Time of the representative's latest history entry (`null` = never edited).
    pub edited_at_ms: Option<i64>,
    /// Last `apply_scene_edit` / `apply_all_edited_scenes` of this scene (`null` = never).
    pub applied_at_ms: Option<i64>,
    /// Checklist status of the representative's edit. Skipped scenes keep their status;
    /// read `skipped` first.
    pub status: SceneEditStatus,
    /// The user skipped this scene in the Edit step (`set_scene_skipped`, v15): it counts as
    /// done, nothing is copied to it, `apply_all_edited_scenes` leaves it alone.
    pub skipped: bool,
    /// A small scene: at most [`MINOR_SCENE_MAX_KEEPERS`] keepers (v15). The plan lists them
    /// last, folded.
    pub minor: bool,
    /// The representative's current settings came from the style model ("Auto edit (my
    /// style)") (v15).
    pub auto_edited: bool,
    /// Members whose current settings were written by an apply (`ImageEditState.editSource`
    /// = `scene_apply`), capture order (v15; non-keepers included when they were applied to).
    pub applied_ids: Vec<ImageId>,
    /// Keepers of this scene that need a look (`ImageEditState.needsReview`), capture order
    /// (v15).
    pub needs_review_ids: Vec<ImageId>,
    /// Applied scenes only (`appliedAtMs` set, not skipped, status not `reset` (v17)):
    /// keepers that the last apply did not cover (added to the scene or made keepers since),
    /// are not the representative, and have no edit of their own (`editSource` `none` or
    /// `auto_style`). Non-empty = "Apply to N new" (`apply_scene_edit`; already applied frames
    /// come out unchanged) (v15).
    pub unapplied_keeper_ids: Vec<ImageId>,
    /// The edit batch of this scene's last apply that changed something (v16): `null` when
    /// never applied, or that batch was undone (undoing it also clears the scene's applied
    /// state). Row "Undo apply" = `undo_edit_batch(appliedBatch.batchId)` while
    /// `appliedBatch.undoable`; persisted, so it survives leaving the workflow.
    pub applied_batch: Option<EditBatchInfo>,
}

/// Scenes with at most this many keepers are `SceneEditEntry.minor` (v15).
pub const MINOR_SCENE_MAX_KEEPERS: u32 = 2;

string_enum! {
    /// Where an image's current develop settings came from (v15). Derived from the history
    /// entry the image's edit history points at, so per-image undo / redo follow it.
    pub enum EditSource {
        /// Neutral settings (never edited, or reset).
        None => "none",
        /// The user's own edit: sliders and tools, presets, profiles, Auto Tone / WB.
        User => "user",
        /// Paste Settings, Sync Settings, Paste from Previous.
        Pasted => "pasted",
        /// "Auto edit (my style)" (`apply_style_prediction`).
        AutoStyle => "auto_style",
        /// Apply to Scene (`apply_scene_edit` / `apply_all_edited_scenes`) or Match Scene
        /// (`apply_scene_match`).
        SceneApply => "scene_apply",
        /// Settings that came from outside Sieve's history: read from the XMP sidecar (import,
        /// Read from XMP), e.g. edited in Lightroom.
        Sidecar => "sidecar",
    }
}

/// Per-photo workflow state (v15; `EditPlan.editStates`, `get_edit_states`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImageEditState {
    pub image_id: ImageId,
    pub edit_source: EditSource,
    /// The edit batch that wrote the current settings (`scene_apply` from an apply,
    /// `auto_style`); `null` otherwise. Undoable with `undo_edit_batch` while not undone.
    pub batch_id: Option<EditBatchId>,
    /// `scene_apply` from `apply_scene_edit`: the scene the settings were applied for (`null`
    /// once that scene was re-detected away, or for Match Scene).
    pub applied_scene_id: Option<SceneId>,
    /// The apply that wrote the current settings did not match this frame within tolerance
    /// (`SceneApplyOutcome.notConvergedIds`) and the user has not looked at it yet. Cleared
    /// by any later edit of the image (it no longer carries the applied settings) or
    /// `mark_reviewed`; comes back when a per-image undo returns to the applied settings.
    pub needs_review: bool,
    /// User-facing reason while `needsReview`, e.g. "Exposure did not match the
    /// representative".
    pub review_reason: Option<String>,
}

/// Scene counts of an `EditPlan` (v15). Status counts are over scenes that are not skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EditPlanCounts {
    pub scenes: u32,
    /// Scenes to do: status `to_edit` or `reset` (v17: `reset` scenes count here too).
    pub to_edit: u32,
    pub edited: u32,
    pub applied: u32,
    pub outdated: u32,
    /// Status `reset` (v17); included in `toEdit`.
    pub reset: u32,
    pub skipped: u32,
    /// Minor scenes (skipped or not).
    pub minor: u32,
    /// Keepers that need a look (`EditPlan.needsReviewIds.length`).
    pub needs_review: u32,
    /// Sum of `unappliedKeeperIds` over the scenes that are not skipped.
    pub unapplied_keepers: u32,
    /// `EditPlan.unassignedKeeperIds.length`.
    pub unassigned_keepers: u32,
}

/// `get_edit_plan(projectId)`: the Edit step of one project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EditPlan {
    pub project_id: ProjectId,
    pub keeper_rule: KeeperRule,
    /// Every keeper of the project, capture order (the Export step's default selection).
    pub keeper_ids: Vec<ImageId>,
    /// Keepers that belong to no scene. Non-empty = run `detect_scenes(null, projectId, null)`
    /// (or create scenes) and fetch the plan again.
    pub unassigned_keeper_ids: Vec<ImageId>,
    /// Scenes with at least one keeper in the project, capture order.
    pub scenes: Vec<SceneEditEntry>,
    /// The plan no longer covers every keeper (v15): keepers outside every scene
    /// (`unassignedKeeperIds`) or keepers added to an applied scene since its apply
    /// (`SceneEditEntry.unappliedKeeperIds`, scenes not skipped). "All scenes done" needs
    /// `!outdated` and every scene applied or skipped.
    pub outdated: bool,
    pub counts: EditPlanCounts,
    /// One per keeper (`keeperIds` order) (v15).
    pub edit_states: Vec<ImageEditState>,
    /// Keepers that need a look, capture order (v15).
    pub needs_review_ids: Vec<ImageId>,
    /// The newest edit batch (any kind) that is not undone and changed at least one image of
    /// the project (v16); `null` = none. Its `undoable` says whether `undo_edit_batch` would
    /// succeed now (linear undo: a later edit of its photos blocks it).
    pub latest_batch: Option<EditBatchInfo>,
}

/// Options of `apply_scene_edit` / `apply_all_edited_scenes`. `null` on the wire = default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SceneApplyOptions {
    /// Relative matching of every target to the representative (the representative is the
    /// single anchor of `match_scene`). Default `MatchOptions::default()` (exposure + WB
    /// matched, strength 1, `DEFAULT_SYNC` groups copied).
    pub match_options: MatchOptions,
    /// Also edit the scene's non-keepers (default `false`).
    pub include_non_keepers: bool,
    /// Leave targets alone whose adjustments the user changed after this scene's last apply
    /// (their current settings differ from what that apply wrote) (default `true`).
    pub skip_user_edited: bool,
    /// Frames to leave alone ("Apply with options" per-photo checkboxes) (v15; default
    /// empty). They count as covered by the apply (not `unappliedKeeperIds`). The
    /// representative and ids outside the scene are ignored.
    #[serde(default)]
    pub exclude_ids: Vec<ImageId>,
}

impl Default for SceneApplyOptions {
    fn default() -> Self {
        Self {
            match_options: MatchOptions::default(),
            include_non_keepers: false,
            skip_user_edited: true,
            exclude_ids: Vec::new(),
        }
    }
}

/// Per-scene result of an apply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SceneApplyOutcome {
    pub scene_id: SceneId,
    pub representative_id: ImageId,
    /// Targets whose adjustments changed.
    pub changed_ids: Vec<ImageId>,
    /// Targets left alone (`skipUserEdited`).
    pub skipped_ids: Vec<ImageId>,
    /// Targets left alone because they were in `SceneApplyOptions.excludeIds` (v15).
    pub excluded_ids: Vec<ImageId>,
    /// Targets whose match did not converge within tolerance (`MatchPreview.converged`); their
    /// settings were still applied. Show them for review.
    pub not_converged_ids: Vec<ImageId>,
    /// User-facing caveats (clamped slider, no neutral found, ...).
    pub notes: Vec<String>,
}

string_enum! {
    /// What produced an edit batch (v16).
    pub enum EditBatchKind {
        /// `apply_scene_edit` / `apply_all_edited_scenes`.
        SceneApply => "scene_apply",
        /// `apply_style_prediction` ("Auto edit (my style)").
        StylePrediction => "style_prediction",
        /// `paste_settings` / `sync_settings` / `paste_previous` (v19): Copy / Paste / Sync to
        /// an arbitrary selection.
        Paste => "paste",
        /// `sync_delta` (v19.2): one Auto Sync commit, the source photo's edit plus the same
        /// change on every target.
        Sync => "sync",
    }
}

/// Persisted state of one edit batch (v16; `SceneEditEntry.appliedBatch`,
/// `EditPlan.latestBatch`, `get_edit_batches`). Undo is linear: a batch can be undone only
/// while none of its photos has a history entry newer than the batch's own (a later batch or
/// a manual edit); undoing a later batch restores the photos to this batch's settings and
/// makes it undoable again. Per-image undo (Cmd+Z in Develop) of the later edit also does.
/// v17: a scene apply made from a representative whose settings this batch wrote (e.g. Auto
/// edit, then Apply to scene) is a later edit of this batch too: undo the apply first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EditBatchInfo {
    pub batch_id: EditBatchId,
    /// History label, e.g. "Apply to Scene", "Auto Edit (My Style)".
    pub label: String,
    pub kind: EditBatchKind,
    pub created_at_ms: i64,
    /// `null` = not undone.
    pub undone_at_ms: Option<i64>,
    /// Photos the batch changed.
    pub image_count: u32,
    /// Photos of the batch edited after it, or (v17) whose settings from this batch a later
    /// scene apply that is not undone was made from (the representative of an auto-edited
    /// scene) (0 when undone): `undo_edit_batch` refuses with `conflict` while this is > 0.
    pub conflict_count: u32,
    /// Not undone and `conflictCount == 0`: `undo_edit_batch` would succeed now.
    pub undoable: bool,
}

/// An undoable multi-image edit (`undo_edit_batch`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EditBatchResult {
    /// `null` when nothing changed (nothing to undo).
    pub batch_id: Option<EditBatchId>,
    /// History label of every entry of the batch, e.g. "Apply to Scene", "Auto Edit (My Style)".
    pub label: String,
    pub changed_ids: Vec<ImageId>,
}

/// Options of `sync_delta` (v19.2; `null` = defaults).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", default)]
pub struct SyncDeltaOptions {
    /// Changed groups applied **relatively** (the source's change is added to each target's
    /// own value) instead of copied. Only `exposure` (EV added, clamped to -5..=5) and
    /// `white_balance` (temperature shifted in mireds, tint added, clamped to the slider
    /// ranges; an `as_shot` target is resolved to its camera as-shot values first) can be
    /// relative; anything else -> `invalid_argument`. Default both. `[]` = copy everything
    /// (Lightroom's Auto Sync).
    pub relative: Vec<AdjustmentField>,
    /// Only consider these groups (`null` = every group except the per-frame ones). Groups in
    /// [`SyncDeltaOptions::NEVER_SYNCED`] are never synced even when listed.
    pub fields: Option<Vec<AdjustmentField>>,
    /// History label of the source's and the targets' entries (`null` = "Auto Sync"; 1..=100
    /// chars). The UI passes what it would pass to `save_adjustments` (e.g. "Exposure").
    pub label: Option<String>,
}

impl Default for SyncDeltaOptions {
    fn default() -> Self {
        Self { relative: Self::RELATIVE.to_vec(), fields: None, label: None }
    }
}

impl SyncDeltaOptions {
    /// Groups that can be (and by default are) applied relatively.
    pub const RELATIVE: &'static [AdjustmentField] = &[AdjustmentField::Exposure, AdjustmentField::WhiteBalance];
    /// Per-frame groups Auto Sync never copies (Lightroom: crop, masks, transform).
    pub const NEVER_SYNCED: &'static [AdjustmentField] =
        &[AdjustmentField::Crop, AdjustmentField::Masks, AdjustmentField::Transform];
    /// Default history label.
    pub const DEFAULT_LABEL: &'static str = "Auto Sync";
}

/// Result of `sync_delta` (v19.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncDeltaResult {
    /// One batch (kind `sync`) holding the source's edit and every changed target; `batchId =
    /// null` when nothing changed. `undo_edit_batch(batch.batchId)` reverts all of them.
    pub batch: EditBatchResult,
    /// The groups that differed between `before` and `after` and were synced (in
    /// `AdjustmentField` order); empty = nothing to sync.
    pub fields: Vec<AdjustmentField>,
    /// Of `fields`, those applied relatively.
    pub relative_fields: Vec<AdjustmentField>,
    /// Targets whose white balance was copied absolutely because their (or the source's)
    /// as-shot white balance could not be resolved (file missing / unreadable).
    pub absolute_wb_ids: Vec<ImageId>,
    /// The source's history after the commit (as `save_adjustments` returns it).
    pub history: AdjustmentHistory,
}

/// Result of `apply_scene_edit` / `apply_all_edited_scenes`: one batch for the whole call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ApplyScenesResult {
    pub batch: EditBatchResult,
    /// Scenes committed, in apply order. After a cancel only the scenes finished before it.
    pub scenes: Vec<SceneApplyOutcome>,
    /// `cancel_scene_apply` stopped the call (v15): scenes in `scenes` were applied (one
    /// batch); the scene being matched and the ones after it were not touched.
    pub cancelled: bool,
    /// `apply_all_edited_scenes` only (v17; always empty for `apply_scene_edit`, which fails
    /// instead): scenes it was going to apply but could not, plan order. Nothing was written
    /// to them; the other scenes were applied.
    pub skipped_scenes: Vec<SkippedScene>,
}

string_enum! {
    /// Why `apply_all_edited_scenes` left a scene out (v17).
    pub enum SceneSkipReason {
        /// The representative has no edits (edit it first).
        NotEdited => "not_edited",
        /// The scene has no keepers (any more).
        NoKeepers => "no_keepers",
        /// Reading or matching the scene failed (e.g. an original is missing or cannot be
        /// decoded); `message` says why.
        Failed => "failed",
    }
}

/// A scene `apply_all_edited_scenes` could not apply (v17).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkippedScene {
    pub scene_id: SceneId,
    pub reason: SceneSkipReason,
    /// User-facing, names the scene by its plan number, e.g. "Scene 2: edit its representative
    /// first, then apply." / "Scene 3: DSC01234.ARW is missing ...".
    pub message: String,
}

/// Result of `undo_edit_batch`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UndoBatchResult {
    /// Images put back to their settings before the batch (one "Undo <label>" history entry each).
    pub restored_ids: Vec<ImageId>,
    /// Images whose batch edit was already taken back with per-image undo (their history
    /// cursor is before the batch's entry): left alone. (Before v16 also images edited after
    /// the batch; since v16 those make the whole undo fail with `conflict`.)
    pub skipped_ids: Vec<ImageId>,
}

string_enum! {
    pub enum StyleModelState {
        /// No model yet (or not enough edited photos).
        Untrained => "untrained",
        Training => "training",
        Ready => "ready",
        /// The last training failed (`error`); a previous model, if any, stays in use
        /// (`trainedAtMs` set).
        Failed => "failed",
    }
}

string_enum! {
    /// Stage reported by `styleModelProgress`.
    pub enum StyleTrainPhase {
        /// Measuring the edited photos (develop-source statistics, scene context).
        Features => "features",
        Fit => "fit",
        /// Scoring on held-out edits.
        Validate => "validate",
    }
}

/// Held-out quality of the current style model (render ΔE2000 vs the user's own edits).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StyleValidation {
    pub held_out_images: u32,
    /// Mean ΔE2000 predicted vs user render.
    #[specta(type = Number)]
    pub delta_e: f32,
    /// Same for `auto_tone` (baseline).
    #[specta(type = Number)]
    pub auto_tone_delta_e: f32,
    /// Same for no edit (defaults).
    #[specta(type = Number)]
    pub no_edit_delta_e: f32,
}

/// `style_model_status()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StyleModelStatus {
    pub state: StyleModelState,
    /// Identifies features + model family, e.g. "style-gbt@1".
    pub model_version: String,
    /// Current model's training time (`null` = none).
    pub trained_at_ms: Option<i64>,
    /// Edited photos the current model learned from.
    pub training_examples: u32,
    /// Edited photos in the catalog now (training candidates).
    pub available_examples: u32,
    /// Training needs at least this many edited photos.
    pub min_examples: u32,
    /// 0..=1 while `training`, else `null`.
    #[specta(type = Option<Number>)]
    pub progress: Option<f32>,
    /// User-facing reason of the last failure.
    pub error: Option<String>,
    pub validation: Option<StyleValidation>,
}

/// `predict_style` result for one image (nothing is saved).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StylePrediction {
    pub image_id: ImageId,
    /// The image's current adjustments with the predicted `fields` replaced (crop, masks and
    /// other per-frame groups are kept). Render it for a preview; `apply_style_prediction`
    /// commits the same values.
    pub adjustments: ParametricAdjustments,
    /// Groups the model predicts.
    pub fields: Vec<AdjustmentField>,
    /// 0..=1 (distance of the frame to the training data).
    #[specta(type = Number)]
    pub confidence: f32,
    pub notes: Vec<String>,
}

/// One source folder of a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFolder {
    pub id: FolderId,
    /// Absolute path on disk.
    pub path: String,
    pub image_count: u32,
    /// The folder is on disk now. `false` = moved, renamed or on an unmounted drive: offer
    /// "Locate folder..." (`relocate_folder(id, newPath)`).
    pub exists: bool,
}

/// A project (one shoot): what the Projects home page card and the TopBar switcher show.
/// Counts are over all images of the project's folders.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: ProjectId,
    /// Defaults to the (first) folder's name; `rename_project`.
    pub name: String,
    /// Source folders, by path. Non-empty.
    pub folders: Vec<ProjectFolder>,
    /// Cover photo: the user's choice (`coverChosen`, `set_project_cover`), else automatic
    /// (best non-rejected photo: picked first, then most stars, then earliest capture);
    /// `null` for an empty project.
    pub cover_image_id: Option<ImageId>,
    /// `coverImageId` was chosen by the user.
    pub cover_chosen: bool,
    /// Ready grid thumbnail (512 px) of the cover, absolute path (asset protocol, like
    /// `ThumbnailState.ready.path`); `null` while pending / failed / no cover.
    pub cover_thumbnail_path: Option<String>,
    /// Culling profile of this shoot (`set_project_shoot_type`).
    pub shoot_type: ShootType,
    /// How readily culling suggests reject for this shoot (v19,
    /// `set_project_reject_strictness`; default `balanced`).
    pub reject_strictness: RejectStrictness,
    /// Guided-workflow step (`set_workflow_step`).
    pub workflow_step: WorkflowStep,
    pub created_at_ms: i64,
    /// Last `open_project` (`null` = never opened).
    pub last_opened_at_ms: Option<i64>,
    pub photo_count: u32,
    /// Keepers by `CatalogState.keeperRule` (same rule as the Edit step).
    pub keeper_count: u32,
    /// Photos with edits (`RawImageEntry.hasEdits`).
    pub edited_count: u32,
    pub picked_count: u32,
    pub rejected_count: u32,
    /// Photos whose original is missing (`RawImageEntry.missingSinceMs` set).
    pub missing_count: u32,
    /// Earliest / latest capture time of the project's photos (`null` = none known).
    pub captured_from_ms: Option<i64>,
    pub captured_to_ms: Option<i64>,
}

impl Project {
    /// Max length of a project name (characters, after trimming).
    pub const MAX_NAME_LEN: usize = 200;

    /// Trimmed name, or `invalid_argument`-style message when empty / too long / with
    /// control characters.
    pub fn validate_name(name: &str) -> Result<String, String> {
        let n = name.trim();
        if n.is_empty() {
            return Err("project name must not be empty".into());
        }
        if n.chars().count() > Self::MAX_NAME_LEN {
            return Err(format!("project name is longer than {} characters", Self::MAX_NAME_LEN));
        }
        if n.chars().any(char::is_control) {
            return Err("project name must not contain control characters".into());
        }
        Ok(n.to_owned())
    }
}

/// Result of `create_project`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CreateProjectResult {
    pub project: Project,
    /// The import that ran (new photos are `pending` until the ingest pipeline extracts them).
    pub import: ImportSummary,
    /// The folder was already in the catalog: its existing project was returned (and the folder
    /// re-scanned) instead of creating a new one; `name` / `shootType` were not applied.
    pub existing: bool,
}

/// Result of `remove_project`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RemoveProjectResult {
    /// Catalog images removed (their files, sidecars and exports are untouched).
    pub removed_images: u32,
    pub removed_folders: u32,
}

// ---------------------------------------------------------------------------
// IPC v19 (Phase 8d): capture time, per-photo metadata, Upright, preview variants, reject
// strictness
// ---------------------------------------------------------------------------

/// Earliest capture time `edit_capture_time` may produce (1900-01-01, naive ms).
pub const MIN_CAPTURE_TIME_MS: i64 = -2_208_988_800_000;
/// Latest capture time `edit_capture_time` may produce (2200-01-01, naive ms).
pub const MAX_CAPTURE_TIME_MS: i64 = 7_258_118_400_000;

/// How `edit_capture_time` changes the selected photos' corrected capture time (Lightroom's
/// Metadata > Edit Capture Time). Times are "naive" ms (see `CaptureMeta.capturedAtMs`).
/// Photos without a capture time are skipped unless the mode gives them one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum CaptureTimeEdit {
    /// "Shift by set number of hours" (any ms): every photo's time + `offsetMs`.
    Shift { offset_ms: i64 },
    /// "Adjust to a specified date and time": `referenceId` (one of the ids, the active
    /// photo) gets `capturedAtMs`; the other photos shift by the same offset. A reference
    /// without a capture time gets it and nothing else changes.
    SetExact { reference_id: ImageId, captured_at_ms: i64 },
    /// Sync two cameras: `referenceId` (a frame of the camera with the right clock) and
    /// `targetId` (a frame of the other camera taken at the same moment); the photos of
    /// `scope` shift by `reference - target`. Both need a capture time; they may be outside
    /// `ids`. v19.2 `scope` (optional, default `selected` = the v19 behaviour): `selected`
    /// moves `ids`; `body` / `model` move **every** photo of the target's project taken with
    /// the target's body (make + model + serial) / model (make + model), whatever the grid
    /// shows, and ignore `ids` (pass `[]`). With `body` / `model` the reference must not be
    /// in that set (`invalid_argument`: same camera).
    SyncCameras {
        reference_id: ImageId,
        target_id: ImageId,
        #[serde(default)]
        scope: CameraSyncScope,
    },
    /// "Revert capture time to original": back to the file's EXIF time
    /// (`originalCapturedAtMs`, source `exif`).
    Revert,
}

string_enum! {
    /// Which photos `CaptureTimeEdit::SyncCameras` moves (v19.2).
    #[derive(Default)]
    pub enum CameraSyncScope {
        /// The `ids` passed to `edit_capture_time` (v19 behaviour; "The selected photos").
        #[default]
        Selected => "selected",
        /// Every photo in the target's project from the target's body (make + model + serial;
        /// an unknown serial matches photos of that model with an unknown serial).
        Body => "body",
        /// Every photo in the target's project from the target's make + model (any serial).
        Model => "model",
    }
}

/// One photo's corrected capture time (undo of `edit_capture_time` via
/// `restore_capture_times`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CaptureTimeSnapshot {
    pub image_id: ImageId,
    pub captured_at_ms: Option<i64>,
    pub source: CaptureTimeSource,
}

/// Result of `edit_capture_time`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CaptureTimeEditResult {
    /// Photos whose corrected time changed (refetch with `get_images`; sort order, bursts and
    /// scene bounds follow).
    pub changed_ids: Vec<ImageId>,
    /// Photos left alone: no capture time to shift, or already at the target time.
    pub skipped_ids: Vec<ImageId>,
    /// Offset applied (`shift` / `set_exact` / `sync_cameras`); `null` for `revert`.
    pub offset_ms: Option<i64>,
    /// The changed photos' times before the edit: pass to `restore_capture_times` to undo.
    pub previous: Vec<CaptureTimeSnapshot>,
}

/// GPS position from the file's EXIF (WGS84 decimal degrees).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GpsLocation {
    /// -90..=90, north positive.
    #[specta(type = Number)]
    pub latitude: f64,
    /// -180..=180, east positive.
    #[specta(type = Number)]
    pub longitude: f64,
    /// Metres above sea level.
    #[specta(type = Option<Number>)]
    pub altitude_m: Option<f64>,
}

/// Everything the Library "Metadata" panel shows for one photo (`get_image_metadata`, v19).
/// Catalog facts plus a few EXIF values read from the file on demand (`null` when absent or
/// unreadable).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImageMetadata {
    pub image_id: ImageId,
    /// Absolute path of the original.
    pub path: String,
    pub file_name: String,
    /// Directory holding the original.
    pub folder_path: String,
    pub format: ImageFormat,
    /// Lower-case extension without the dot ("arw").
    pub extension: String,
    pub file_size: i64,
    pub file_mtime_ms: i64,
    /// Corrected capture time (`CaptureMeta.capturedAtMs`).
    pub captured_at_ms: Option<i64>,
    /// The file's own EXIF time (`CaptureMeta.originalCapturedAtMs`).
    pub original_captured_at_ms: Option<i64>,
    pub capture_time_source: CaptureTimeSource,
    pub camera: CameraInfo,
    pub lens: Option<String>,
    pub iso: Option<u32>,
    #[specta(type = Option<Number>)]
    pub shutter_seconds: Option<f64>,
    #[specta(type = Option<Number>)]
    pub aperture: Option<f32>,
    #[specta(type = Option<Number>)]
    pub focal_length_mm: Option<f32>,
    /// From the file (EXIF `FocalLengthIn35mmFormat`).
    #[specta(type = Option<Number>)]
    pub focal_length_35mm: Option<f32>,
    /// From the file (EXIF `ExposureBiasValue`), EV.
    #[specta(type = Option<Number>)]
    pub exposure_compensation_ev: Option<f32>,
    /// From the file (EXIF `Flash` bit 0).
    pub flash_fired: Option<bool>,
    /// From the file (EXIF `BodySerialNumber` / maker notes).
    pub camera_serial: Option<String>,
    /// Sensor pixel size (as in `RawImageEntry`).
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// EXIF orientation 1..=8.
    pub orientation: Option<u8>,
    /// From the file; `null` = no GPS data.
    pub gps: Option<GpsLocation>,
    /// Where the XMP sidecar is (or would be written).
    pub sidecar_path: String,
    /// The sidecar exists on disk now.
    pub sidecar_exists: bool,
    /// Paired camera JPEG/HEIC (`RawImageEntry.companionPath`).
    pub companion_path: Option<String>,
    /// The original is missing at `path` (`RawImageEntry.missingSinceMs` set).
    pub missing: bool,
}

/// Result of `auto_upright` (v19). Nothing is saved: the UI sets
/// `transform.upright = mode`, `transform.solution = solution` and commits one history entry
/// ("Upright: Auto"...).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UprightResult {
    pub mode: UprightMode,
    /// `null` when the photo has no usable lines for this mode (Lightroom then leaves the
    /// photo as it is); `message` says so.
    pub solution: Option<UprightSolution>,
    /// User-facing note, e.g. "No straight lines found for Vertical"; `null` on success.
    pub message: Option<String>,
}

/// Result of `get_transform_bounds` (v19.3): the Transform / Upright warp's outline for the
/// crop tool. Pure geometry of the live `adjustments` (no render).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TransformBounds {
    /// The warped image's outline in the *uncropped corrected frame as displayed* (EXIF
    /// orientation applied; fractions 0..=1 of a render with `crop.enabled = false`, the
    /// crop tool's frame): the four source corners mapped through the warp, clockwise on
    /// screen (y down). Points may lie outside 0..=1 (the warp pushes a corner past the frame
    /// edge); the photo's pixels cover the intersection of this quad and the frame, the rest
    /// renders white. `null` = no warp (the image covers the whole frame).
    #[specta(type = Option<Vec<(Number, Number)>>)]
    pub valid_quad: Option<Vec<(f64, f64)>>,
    /// What Constrain Crop makes of `adjustments.crop` (stored crop convention: un-oriented
    /// fractions + angle, as `CropSettings`): `adjustments.crop` unchanged when it already
    /// fits inside the warped image, else the largest frame of the same aspect and angle
    /// that fits; a disabled crop becomes the largest frame of the photo's aspect (enabled),
    /// or stays as it is (disabled) when the warp already covers the whole frame (e.g.
    /// Scale > 100).
    /// Computed whatever `transform.constrainCrop` says (it is what a render shows when that
    /// is on). `null` = no warp.
    pub constrained_crop: Option<CropSettings>,
}

/// A temporary variation of the live settings for `render_preview_variant` (v19). Nothing is
/// saved and no history entry is written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum PreviewVariant {
    /// What applying the preset would give (same as `resolve_preset`): hover preview on the
    /// main image.
    Preset { preset_id: PresetId },
    /// The live settings with these groups back at the photo's format defaults:
    /// press-and-hold a panel's "changed" dot to see the photo without that panel. Non-empty.
    WithoutFields { fields: Vec<AdjustmentField> },
}

string_enum! {
    /// How readily culling turns defects into reject suggestions (v19, per project,
    /// `set_project_reject_strictness`). Applied by the scorer (vision-ml-dev) on top of the
    /// shoot type's `CullThresholds`.
    #[derive(Default)]
    pub enum RejectStrictness {
        /// Only clear failures are suggested for reject.
        Conservative => "conservative",
        /// Default.
        #[default]
        Balanced => "balanced",
        /// Burst duplicates, any closed eyes on the main subject and soft focus are
        /// suggested for reject.
        Aggressive => "aggressive",
    }
}

/// One item of Lightroom's Copy Settings dialog (a checkbox).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CopySettingsItem {
    pub label: String,
    /// Fields copied when checked; empty when `supported` is false.
    pub fields: Vec<AdjustmentField>,
    /// `false`: Sieve preserves these settings in the sidecar but cannot copy them (shown
    /// disabled so the dialog matches Lightroom's).
    pub supported: bool,
}

/// A group of Lightroom's Copy Settings dialog (group checkbox = all its items).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CopySettingsGroup {
    pub id: String,
    pub label: String,
    pub items: Vec<CopySettingsItem>,
}

/// The Copy... / Sync... / Save Preset dialog layout (Lightroom Classic's groups, in its order);
/// exported to TS as the constant `COPY_SETTINGS_GROUPS`. Every `AdjustmentField` except the
/// umbrella `noise_reduction` appears in exactly one item (test); `lut` is copied with `Profile`.
pub fn copy_settings_groups() -> Vec<CopySettingsGroup> {
    use AdjustmentField as F;
    let item = |label: &str, fields: &[AdjustmentField]| CopySettingsItem {
        label: label.into(),
        fields: fields.to_vec(),
        supported: true,
    };
    let unsupported = |label: &str| CopySettingsItem { label: label.into(), fields: Vec::new(), supported: false };
    let group = |id: &str, label: &str, items: Vec<CopySettingsItem>| CopySettingsGroup {
        id: id.into(),
        label: label.into(),
        items,
    };
    vec![
        group("white_balance", "White Balance", vec![item("White Balance", &[F::WhiteBalance])]),
        group(
            "basic_tone",
            "Basic Tone",
            vec![
                item("Exposure", &[F::Exposure]),
                item("Contrast", &[F::Contrast]),
                item("Highlights", &[F::Highlights]),
                item("Shadows", &[F::Shadows]),
                item("White Clipping", &[F::Whites]),
                item("Black Clipping", &[F::Blacks]),
            ],
        ),
        group("tone_curve", "Tone Curve", vec![item("Tone Curve", &[F::ToneCurve])]),
        group(
            "presence",
            "Presence",
            vec![
                item("Texture", &[F::Texture]),
                item("Clarity", &[F::Clarity]),
                item("Dehaze", &[F::Dehaze]),
                item("Vibrance", &[F::Vibrance]),
                item("Saturation", &[F::Saturation]),
            ],
        ),
        group(
            "color",
            "Color Adjustments",
            vec![
                item("Hue", &[F::HslHue]),
                item("Saturation", &[F::HslSaturation]),
                item("Luminance", &[F::HslLuminance]),
            ],
        ),
        group("color_grading", "Color Grading", vec![item("Color Grading", &[F::ColorGrading])]),
        group(
            "detail",
            "Detail",
            vec![
                item("Sharpening", &[F::Sharpening]),
                item("Luminance Noise Reduction", &[F::NoiseReductionLuminance]),
                item("Color Noise Reduction", &[F::NoiseReductionColor]),
            ],
        ),
        group(
            "treatment_profile",
            "Treatment & Profile",
            vec![item("Treatment & B&W Mix", &[F::BlackAndWhite]), item("Profile", &[F::Profile, F::Lut])],
        ),
        group(
            "lens_corrections",
            "Lens Corrections",
            vec![
                unsupported("Lens Profile Corrections"),
                unsupported("Chromatic Aberration"),
                unsupported("Lens Distortion"),
                unsupported("Lens Vignetting"),
            ],
        ),
        group("transform", "Transform", vec![item("Upright & Transform", &[F::Transform])]),
        group("effects", "Effects", vec![item("Post-Crop Vignetting", &[F::Vignette]), item("Grain", &[F::Grain])]),
        group("calibration", "Calibration", vec![item("Calibration", &[F::Calibration])]),
        group("masking", "Masking", vec![item("Masks", &[F::Masks])]),
        group("spot_removal", "Spot Removal", vec![unsupported("Spot Removal")]),
        group("crop", "Crop", vec![item("Crop, Straighten Angle & Aspect Ratio", &[F::Crop])]),
        group("process_version", "Process Version", vec![item("Process Version", &[F::ProcessVersion])]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn masked() -> ParametricAdjustments {
        let ai = MaskComponent {
            id: format!("{:032X}", 2),
            name: "Subject 1".into(),
            active: true,
            mode: MaskBlendMode::Add,
            inverted: false,
            opacity: 1.0,
            shape: MaskShape::Ai(AiMask {
                target: AiTarget::Subject,
                reference_point: None,
                digest: Some("E71A59AFC894F4F898F72751E30113DA".into()),
            }),
        };
        let group = MaskGroup {
            id: format!("{:032X}", 1),
            name: "Cool Soft".into(),
            active: true,
            amount: 1.0,
            adjustments: LocalAdjustments { clarity: -19.5, temperature: -19.8, texture: -14.9, ..Default::default() },
            components: vec![ai],
        };
        ParametricAdjustments { masks: vec![group], ..Default::default() }
    }

    #[test]
    fn v10_masks_validate_copy_lerp_and_old_json() {
        let m = masked();
        m.validate().unwrap();
        assert!(!m.is_neutral());
        // Copy/paste/sync: groups copied, AI digests dropped (target recomputes).
        let mut t = ParametricAdjustments::default();
        t.copy_fields(&m, AdjustmentField::DEFAULT_SYNC);
        assert!(t.masks.is_empty(), "DEFAULT_SYNC leaves masks alone");
        t.copy_fields(&m, &[AdjustmentField::Masks]);
        assert_eq!(t.masks.len(), 1);
        assert_eq!(t.masks[0].adjustments, m.masks[0].adjustments);
        assert!(matches!(&t.masks[0].components[0].shape, MaskShape::Ai(a) if a.digest.is_none()));
        // lerp: nearer side's masks.
        let d = ParametricAdjustments::default();
        assert!(ParametricAdjustments::lerp(&m, &d, 0.4).masks == m.masks);
        assert!(ParametricAdjustments::lerp(&m, &d, 0.6).masks.is_empty());
        // validate() covers masks.
        let mut bad = m.clone();
        bad.masks[0].adjustments.contrast = 150.0;
        assert!(bad.validate().unwrap_err().contains("masks[0].adjustments.contrast"));
        // JSON without `masks` (pre-v10 rows) reads as no masks; round trip keeps them.
        let mut v = serde_json::to_value(&d).unwrap();
        v.as_object_mut().unwrap().remove("masks");
        assert_eq!(serde_json::from_value::<ParametricAdjustments>(v).unwrap(), d);
        let back: ParametricAdjustments = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
        // The mask slot is reserved for overlays.
        let opts = RenderOptions { max_edge: 1024, slot: RenderSlot::Mask, region: None };
        assert!(opts.validate().is_err());
        assert!(MaskOverlayOptions { max_edge: 1024, region: None }.validate().is_ok());
        assert!(MaskOverlayOptions { max_edge: 10, region: None }.validate().is_err());
    }

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
        assert_eq!(json["copyFields"].as_array().unwrap().len(), AdjustmentField::DEFAULT_SYNC.len());
        assert!(!m.copy_fields.contains(&AdjustmentField::Crop), "crops are per-frame");
        assert_eq!(serde_json::to_value(SceneMethod::Manual).unwrap(), "manual");
        let q: ImageQuery = serde_json::from_value(serde_json::json!({
            "includeTags": [], "excludeTags": [], "tagMatch": "any", "picks": [], "minRating": null,
            "maxRating": null, "colorLabels": [], "burstGroupId": null, "collapseBursts": false,
            "folderId": null, "sort": "capture_time", "sortDescending": false, "offset": 0, "limit": 10
        }))
        .unwrap();
        assert_eq!(q.scene_id, None, "sceneId may be omitted by older callers");
    }

    /// A value in every v9 group, all valid.
    fn parity_edit() -> ParametricAdjustments {
        let mut a = ParametricAdjustments::default();
        a.tone_curve.parametric = ParametricCurve {
            shadows: -5.0,
            darks: -15.0,
            lights: 20.0,
            highlights: -15.0,
            shadow_split: 15.0,
            midtone_split: 35.0,
            highlight_split: 75.0,
        };
        a.tone_curve.point.master = vec![[0.0, 14.0], [44.0, 46.0], [106.0, 110.0], [255.0, 252.0]];
        a.tone_curve.point.red = vec![[0.0, 0.0], [29.0, 21.0], [115.0, 133.0], [255.0, 255.0]];
        a.color_grading.shadows = ColorWheel { hue: 30.0, saturation: 2.0, luminance: 0.0 };
        a.color_grading.midtones = ColorWheel { hue: 185.0, saturation: 5.0, luminance: -3.0 };
        a.color_grading.blending = 100.0;
        a.calibration.red.saturation = 20.0;
        a.calibration.blue.hue = -10.0;
        a.calibration.shadow_tint = 4.0;
        a.detail.sharpening = Sharpening { amount: 48.0, radius: 1.2, detail: 25.0, masking: 30.0 };
        a.detail.noise_reduction.luminance = 24.0;
        a.effects.vignette.amount = -12.0;
        a.effects.grain = Grain { amount: 10.0, size: 14.0, roughness: 66.0 };
        a.black_and_white = BlackAndWhite { enabled: true, mixer: HslChannels { red: 5.0, ..Default::default() } };
        a.crop = CropSettings { enabled: true, top: 0.05, left: 0.1, bottom: 0.9, right: 0.95, angle: -1.9 };
        a.profile = ProfileSettings { camera_profile: Some("Camera ST".into()), look: None };
        a
    }

    #[test]
    fn v9_parity_groups_validate_copy_and_default() {
        let d = ParametricAdjustments::default();
        assert!(d.validate().is_ok() && d.is_neutral());
        // Lightroom RAW defaults.
        assert_eq!(
            (d.detail.sharpening.amount, d.detail.sharpening.radius, d.detail.noise_reduction.color),
            (40.0, 1.0, 25.0)
        );
        assert_eq!(d.color_grading.blending, 50.0);
        assert_eq!(d.profile.look.as_ref().map(|l| l.name.as_str()), Some("Adobe Color"));
        assert!(PointCurves::is_identity(&d.tone_curve.point.master));
        // Non-RAW defaults differ only in Detail + profile.
        let j = ParametricAdjustments::defaults_for(ImageFormat::Jpeg);
        assert!(j.validate().is_ok() && j.is_neutral_for(ImageFormat::Heic) && !j.is_neutral());
        assert_eq!((j.detail.sharpening.amount, j.detail.noise_reduction.color), (0.0, 0.0));
        assert_eq!(j.profile, ProfileSettings::none());
        assert!(d.is_neutral_for(ImageFormat::Cr3) && !d.is_neutral_for(ImageFormat::Png));

        let e = parity_edit();
        assert!(e.validate().is_ok(), "{:?}", e.validate());
        let mut t = ParametricAdjustments::default();
        t.copy_fields(&e, AdjustmentField::DEFAULT_SYNC);
        assert_eq!(t.crop, CropSettings::default(), "DEFAULT_SYNC leaves the crop alone");
        t.copy_fields(&e, &[AdjustmentField::Crop]);
        assert_eq!(t, e, "ALL groups together cover every field");
        // v10: DEFAULT_SYNC = ALL minus crop and masks (v14: and the two noise-reduction
        // subsets, covered by the `noise_reduction` umbrella; v19: and transform).
        assert_eq!(AdjustmentField::DEFAULT_SYNC.len() + 5, AdjustmentField::ALL.len());
        assert!(!AdjustmentField::DEFAULT_SYNC.contains(&AdjustmentField::Masks));

        // Out-of-range / malformed values.
        let bad = |f: fn(&mut ParametricAdjustments)| {
            let mut a = ParametricAdjustments::default();
            f(&mut a);
            a.validate().is_err()
        };
        assert!(bad(|a| a.tone_curve.parametric.midtone_split = 10.0));
        assert!(bad(|a| a.tone_curve.point.blue = vec![[0.0, 0.0]]));
        assert!(bad(|a| a.tone_curve.point.green = vec![[0.0, 0.0], [0.0, 255.0]]));
        assert!(bad(|a| a.tone_curve.point.master = vec![[0.0, 0.0], [256.0, 255.0]]));
        assert!(bad(|a| a.color_grading.global.hue = 361.0));
        assert!(bad(|a| a.detail.sharpening.radius = 0.4));
        assert!(bad(|a| a.detail.sharpening.amount = 151.0));
        assert!(bad(|a| a.crop = CropSettings { enabled: true, left: 0.6, right: 0.4, ..Default::default() }));
        assert!(bad(|a| a.crop.angle = 46.0));
        assert!(bad(|a| a.profile.look = Some(LookSettings { uuid: "b952".into(), ..LookSettings::adobe_color() })));
        assert!(bad(|a| a.profile.camera_profile = Some(String::new())));
    }

    #[test]
    fn v9_json_without_new_groups_deserializes_to_defaults() {
        // A pre-v9 frontend / stored snapshot: no toneCurve, colorGrading, ... keys.
        let mut v = serde_json::to_value(ParametricAdjustments { exposure: 1.5, ..Default::default() }).unwrap();
        for k in ["toneCurve", "colorGrading", "calibration", "detail", "effects", "blackAndWhite", "crop", "profile"] {
            assert!(v.as_object_mut().unwrap().remove(k).is_some(), "{k}");
        }
        let a: ParametricAdjustments = serde_json::from_value(v).unwrap();
        assert_eq!(a, ParametricAdjustments { exposure: 1.5, ..Default::default() });
        // Wire names.
        let j = serde_json::to_value(parity_edit()).unwrap();
        assert_eq!(j["toneCurve"]["parametric"]["shadowSplit"], 15.0);
        assert_eq!(j["toneCurve"]["point"]["master"][0], serde_json::json!([0.0, 14.0]));
        assert_eq!(j["colorGrading"]["midtones"]["hue"], 185.0);
        assert_eq!(j["detail"]["noiseReduction"]["luminanceDetail"], 50.0);
        assert_eq!(j["effects"]["vignette"]["style"], "highlight_priority");
        assert_eq!(j["profile"]["cameraProfile"], "Camera ST");
        assert_eq!(j["blackAndWhite"]["enabled"], true);
        // Import options: new flags optional.
        let o: ImportOptions = serde_json::from_value(serde_json::json!({ "recursive": true })).unwrap();
        assert_eq!(o, ImportOptions::raw_only(true));
        assert!(o.pair_jpeg_with_raw && !o.include_non_raw);
    }

    #[test]
    fn v9_lerp_parity_groups() {
        let a = ParametricAdjustments::default();
        let b = parity_edit();
        assert_eq!(ParametricAdjustments::lerp(&a, &b, 0.0), a);
        assert_eq!(ParametricAdjustments::lerp(&a, &b, 1.0), b);
        let m = ParametricAdjustments::lerp(&a, &b, 0.5);
        assert!(m.validate().is_ok(), "{:?}", m.validate());
        assert_eq!(m.tone_curve.parametric.shadow_split, 20.0);
        assert_eq!(m.tone_curve.point.master, b.tone_curve.point.master, "4 vs 2 points: nearer side (t = 0.5 -> b)");
        assert_eq!(m.tone_curve.point.green, PointCurves::identity_curve());
        assert_eq!(m.detail.sharpening.amount, 44.0);
        assert!(m.crop.enabled && m.black_and_white.enabled && m.profile == b.profile);
        // Hue: shorter arc across 0/360; a zero-saturation wheel takes the other hue.
        let w = |h: f32, s: f32| ColorWheel { hue: h, saturation: s, luminance: 0.0 };
        let mid = ColorWheel::lerp(&w(350.0, 10.0), &w(30.0, 10.0), 0.5);
        assert!((mid.hue - 10.0).abs() < 1e-3, "{}", mid.hue);
        assert_eq!(ColorWheel::lerp(&w(0.0, 0.0), &w(200.0, 20.0), 0.25).hue, 200.0);
        // Same look: amount interpolates.
        let mut p = ParametricAdjustments::default();
        p.profile.look.as_mut().unwrap().amount = 0.0;
        let q = ParametricAdjustments::default();
        assert_eq!(ParametricAdjustments::lerp(&p, &q, 0.25).profile.look.unwrap().amount, 0.25);
    }

    #[test]
    fn v9_image_formats() {
        for &f in ImageFormat::ALL {
            assert_eq!(ImageFormat::parse(f.as_str()), Some(f));
        }
        assert_eq!(ImageFormat::from_extension("JPG"), Some(ImageFormat::Jpeg));
        assert_eq!(ImageFormat::from_extension("hif"), Some(ImageFormat::Heic));
        assert!(ImageFormat::Raf.is_raw() && !ImageFormat::Tiff.is_raw());
        assert!(ImageFormat::Jpeg.pairs_with_raw() && !ImageFormat::Png.pairs_with_raw());
    }

    #[test]
    fn v19_transform_validates_copies_and_maps_to_crs() {
        let d = ParametricAdjustments::default();
        assert_eq!(d.transform, TransformSettings::default());
        assert_eq!((d.transform.upright, d.transform.scale), (UprightMode::Off, 100.0));
        // Stored JSON without `transform` still loads (serde default).
        let mut v = serde_json::to_value(&d).unwrap();
        v.as_object_mut().unwrap().remove("transform");
        assert_eq!(serde_json::from_value::<ParametricAdjustments>(v).unwrap(), d);

        for m in UprightMode::ALL {
            assert_eq!(UprightMode::from_crs(m.crs_value() as i64), Some(*m));
        }
        assert_eq!(UprightMode::Full.crs_value(), 2);
        assert_eq!(UprightMode::Level.crs_value(), 3);
        assert_eq!(UprightMode::from_crs(6), None);

        let e = ParametricAdjustments {
            transform: TransformSettings {
                upright: UprightMode::Guided,
                guides: vec![UprightGuide { start: NormPoint { x: 0.1, y: 0.1 }, end: NormPoint { x: 0.1, y: 0.9 } }],
                vertical: -20.0,
                rotate: 1.5,
                scale: 90.0,
                constrain_crop: true,
                solution: Some(UprightSolution {
                    mode: UprightMode::Guided,
                    matrix: UprightSolution::IDENTITY.to_vec(),
                    rotation_deg: 0.5,
                    crs: vec![CrsProperty { name: "UprightVersion".into(), value: "151388160".into() }],
                }),
                ..TransformSettings::default()
            },
            ..ParametricAdjustments::default()
        };
        assert!(e.validate().is_ok(), "{:?}", e.validate());
        assert!(!e.is_neutral());
        let mut t = ParametricAdjustments::default();
        t.copy_fields(&e, AdjustmentField::DEFAULT_SYNC);
        assert_eq!(t.transform, TransformSettings::default(), "DEFAULT_SYNC leaves the transform alone");
        t.copy_fields(&e, &[AdjustmentField::Transform]);
        assert_eq!(t, e);
        assert!(AdjustmentField::PASTE_PREVIOUS.contains(&AdjustmentField::Transform));
        assert_eq!(ParametricAdjustments::lerp(&d, &e, 0.7).transform, e.transform);

        let bad = |f: fn(&mut TransformSettings)| {
            let mut a = ParametricAdjustments::default();
            f(&mut a.transform);
            a.validate().unwrap_err()
        };
        bad(|t| t.rotate = 10.5);
        bad(|t| t.scale = 40.0);
        bad(|t| t.offset_x = f32::NAN);
        bad(|t| {
            t.guides = vec![UprightGuide { start: NormPoint { x: 0.0, y: 0.0 }, end: NormPoint { x: 1.2, y: 0.0 } }]
        });
        bad(|t| {
            t.solution =
                Some(UprightSolution { mode: UprightMode::Auto, matrix: vec![1.0], rotation_deg: 0.0, crs: vec![] })
        });
        bad(|t| {
            t.solution = Some(UprightSolution {
                mode: UprightMode::Auto,
                matrix: UprightSolution::IDENTITY.to_vec(),
                rotation_deg: 0.0,
                crs: vec![CrsProperty { name: "bad name".into(), value: String::new() }],
            })
        });
    }
}
