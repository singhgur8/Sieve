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
        /// `crs:MaskGroupBasedCorrections` (local adjustments / AI masks, Phase 7c):
        /// preserved in the sidecar, not rendered yet. `detail` = number of mask groups.
        MasksUnsupported => "masks_unsupported",
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
    /// else "Other".
    pub group: String,
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
    /// are unavailable for non-RAW sources).
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
    /// Directories scanned (for the "no Adobe profiles found" hint).
    pub search_dirs: Vec<String>,
}

/// Parametric develop settings. Field names and ranges mirror Adobe Camera Raw
/// Process 2012+ (`crs:` XMP namespace) so XMP export is a 1:1 mapping.
/// Phase 7c (masking) will add `masks: Vec<MaskGroup>` (`#[serde(default)]`, group `masks`),
/// each with a *local* parameter set mirroring Lightroom's `crs:Local*` (exposure, contrast,
/// highlights, shadows, whites, blacks, temperature/tint deltas, texture, clarity, dehaze,
/// saturation, hue, sharpness, luminance noise, moire, defringe, toning colour, curve
/// refine saturation); those reuse this struct's ranges and names, not a second model.
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
            check("lut.amount", lut.amount, 0.0, 100.0)?;
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
                AdjustmentField::Vignette => self.effects.vignette = src.effects.vignette,
                AdjustmentField::Grain => self.effects.grain = src.effects.grain,
                AdjustmentField::BlackAndWhite => self.black_and_white = src.black_and_white,
                AdjustmentField::Crop => self.crop = src.crop,
                AdjustmentField::Profile => self.profile = src.profile.clone(),
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
    }
}

impl AdjustmentField {
    /// Every group except `crop` (Lightroom's Sync/Copy default: crops are per-frame).
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
            tone_curve: ToneCurve::default(),
            color_grading: ColorGrading::default(),
            calibration: CameraCalibration::default(),
            detail: DetailAdjustments::default(),
            effects: EffectsAdjustments::default(),
            black_and_white: BlackAndWhite::default(),
            crop: CropSettings::default(),
            profile: ProfileSettings::default(),
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

/// Result of `apply_suggestions`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ApplySuggestionsResult {
    /// Images whose rating/pick were set from the suggestions.
    pub applied: u32,
    /// Images left alone: unanalyzed, or (with `onlyUnset`) already flagged or rated.
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
}

/// Small per-catalog UI preferences. Every field is optional so the struct can grow;
/// `set_ui_prefs` replaces the whole value (read-modify-write from the frontend).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", default)]
pub struct UiPrefs {
    /// Folder last chosen in the export dialog (absolute path).
    pub last_export_folder: Option<String>,
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
        assert_eq!(AdjustmentField::DEFAULT_SYNC.len() + 1, AdjustmentField::ALL.len());

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
}
