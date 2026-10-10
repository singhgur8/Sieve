//! Baseline edit contract (Phase 10, IPC v21). Re-exported from [`crate::ipc::types`]; same
//! conventions (camelCase fields, snake_case enum values identical to the catalog's).
//!
//! The user picks a preset, edits one photo of the shoot (the **anchor**) and asks Sieve to
//! "edit the rest the same way", then finishes in Lightroom. Every develop group is either:
//! - **look**: copied as-is from the anchor (which carries the preset), e.g. profile, HSL,
//!   color grading, tone curve, sharpening, grain, LUT;
//! - **light**: computed per photo = that photo's own Auto (auto tone + auto white balance)
//!   plus the anchor's offset from its own Auto, smoothed across a burst / scene, never
//!   brightening silhouettes and deliberate low-key frames blindly (they are flagged);
//! - **never**: photo-specific geometry and local work, never copied (crop, transform, masks).
//!
//! The partition is one table, [`BASELINE_PARTITION`], over [`AdjustmentField`] with the
//! `crs:` keys each group owns (exported to TS as `BASELINE_PARTITION`). Everything else in this
//! module is the run around it:
//! - `preview_baseline`: before / after settings for a sample of photos spread across scenes,
//!   nothing written.
//! - `run_baseline`: background job (activity kind `baseline_edit`, one `baselineRunFinished`)
//!   writing every photo in scope as **one** edit batch (kind `baseline`, label
//!   [`BASELINE_LABEL`]); `undo_edit_batch(run.batch.batchId)` takes it back.
//! - Per-photo result ([`BaselinePhotoResult`]): applied / flagged (written, needs a look) /
//!   skipped (already edited) / anchor / failed, with reasons.
//! - Provenance ([`BaselineProvenance`], table `baseline_provenance`): which run wrote a photo's
//!   settings; a photo is "on the baseline" while its history cursor is still the entry the
//!   baseline wrote. Any later edit makes it `user_edited`; a re-run (e.g. after the anchor
//!   changed) updates unedited photos and photos still on the baseline, and skips the rest
//!   unless `replaceEdited`.
//!
//! Output is Lightroom-native: the settings land in the XMP sidecars as `crs:` values through
//! the existing auto-sync path (see `docs/architecture.md`, "Baseline edit").

use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

use super::types::{
    string_enum, AdjustmentField, BurstGroupId, EditBatchId, EditBatchInfo, ImageId, ParametricAdjustments, PresetId,
    ProjectId, SceneId, WhiteBalance,
};

/// Catalog row id of a baseline run (IPC v21).
pub type BaselineRunId = i64;

/// History label of every entry a baseline run writes (`EditSource::Baseline`).
pub const BASELINE_LABEL: &str = "Baseline Edit";
/// Default `BaselinePreviewOptions.sampleCount` (the UI's before / after grid).
pub const DEFAULT_BASELINE_SAMPLES: u32 = 12;
/// Largest accepted `BaselinePreviewOptions.sampleCount` / `imageIds` length.
pub const MAX_BASELINE_SAMPLES: u32 = 48;

// ---------------------------------------------------------------------------
// Look / light partition
// ---------------------------------------------------------------------------

string_enum! {
    /// What a baseline run does with a develop group (IPC v21, [`BASELINE_PARTITION`]).
    pub enum SettingClass {
        /// Copied as-is from the anchor (preset colours, detail, effects).
        Look => "look",
        /// Computed per photo: its own Auto + the anchor's offset from its own Auto.
        Light => "light",
        /// Photo-specific: the photo keeps its own value (never copied).
        Never => "never",
    }
}

/// One row of [`BASELINE_PARTITION`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartitionRow {
    pub field: AdjustmentField,
    pub class: SettingClass,
    /// `crs:` local names the group owns (as in `xmp::crs` and imported presets). A trailing
    /// `*` is a prefix (e.g. `HueAdjustment*` = the 8 bands). `sieve:` keys are written with
    /// their prefix (`sieve:LutId`).
    pub crs: &'static [&'static str],
    /// Why, for the docs / UI.
    pub note: &'static str,
}

/// The look / light / never partition of every [`AdjustmentField`] (exactly one row each, in
/// `AdjustmentField::ALL` order; checked by tests). `crs:` keys of develop settings Sieve does
/// not model (lens corrections, spot removal `RetouchInfo`, red eye, legacy local corrections)
/// are never touched by a baseline run: they stay as they are in each photo's sidecar.
///
/// Notes on choices (decisions.md 2026-10-10, "Contract v21"):
/// - `vibrance` / `saturation` are **look** although Lightroom's Auto sets them: they are part
///   of a preset's colour; the light Auto uses only the six tone sliders + white balance.
/// - `white_balance` is **light** (per lighting); the anchor's taste ("warmer than Auto")
///   carries over as an offset in mireds + tint.
/// - A preset's own light keys (e.g. `Exposure2012`) reach the photos only through the anchor's
///   offset: the anchor carries them, its offset from Auto includes them.
pub const BASELINE_PARTITION: &[PartitionRow] = &[
    row(
        AdjustmentField::WhiteBalance,
        SettingClass::Light,
        &["WhiteBalance", "Temperature", "Tint"],
        "Auto WB + anchor offset (mireds, tint)",
    ),
    row(AdjustmentField::Exposure, SettingClass::Light, &["Exposure2012"], "Auto + anchor offset"),
    row(AdjustmentField::Contrast, SettingClass::Light, &["Contrast2012"], "Auto + anchor offset"),
    row(AdjustmentField::Highlights, SettingClass::Light, &["Highlights2012"], "Auto + anchor offset"),
    row(AdjustmentField::Shadows, SettingClass::Light, &["Shadows2012"], "Auto + anchor offset"),
    row(AdjustmentField::Whites, SettingClass::Light, &["Whites2012"], "Auto + anchor offset"),
    row(AdjustmentField::Blacks, SettingClass::Light, &["Blacks2012"], "Auto + anchor offset"),
    row(AdjustmentField::Texture, SettingClass::Look, &["Texture"], "presence"),
    row(AdjustmentField::Clarity, SettingClass::Look, &["Clarity2012"], "presence"),
    row(AdjustmentField::Dehaze, SettingClass::Look, &["Dehaze"], "presence"),
    row(AdjustmentField::Vibrance, SettingClass::Look, &["Vibrance"], "preset colour (not part of the light Auto)"),
    row(AdjustmentField::Saturation, SettingClass::Look, &["Saturation"], "preset colour (not part of the light Auto)"),
    row(AdjustmentField::HslHue, SettingClass::Look, &["HueAdjustment*"], "color mixer"),
    row(AdjustmentField::HslSaturation, SettingClass::Look, &["SaturationAdjustment*"], "color mixer"),
    row(AdjustmentField::HslLuminance, SettingClass::Look, &["LuminanceAdjustment*"], "color mixer"),
    row(
        AdjustmentField::Lut,
        SettingClass::Look,
        &["sieve:LutId", "sieve:LutAmount"],
        "Sieve LUT (not read by Lightroom)",
    ),
    row(
        AdjustmentField::ToneCurve,
        SettingClass::Look,
        &["Parametric*", "ToneCurvePV2012*", "ToneCurveName2012"],
        "tone curve",
    ),
    row(AdjustmentField::ColorGrading, SettingClass::Look, &["SplitToning*", "ColorGrade*"], "color grading"),
    row(
        AdjustmentField::Calibration,
        SettingClass::Look,
        &["RedHue", "RedSaturation", "GreenHue", "GreenSaturation", "BlueHue", "BlueSaturation", "ShadowTint"],
        "calibration",
    ),
    row(
        AdjustmentField::Sharpening,
        SettingClass::Look,
        &["Sharpness", "SharpenRadius", "SharpenDetail", "SharpenEdgeMasking"],
        "detail",
    ),
    row(
        AdjustmentField::NoiseReduction,
        SettingClass::Look,
        &["LuminanceSmoothing", "LuminanceNoiseReduction*", "ColorNoiseReduction*"],
        "detail",
    ),
    row(AdjustmentField::Vignette, SettingClass::Look, &["PostCropVignette*"], "effects"),
    row(AdjustmentField::Grain, SettingClass::Look, &["Grain*"], "effects"),
    row(AdjustmentField::BlackAndWhite, SettingClass::Look, &["ConvertToGrayscale", "GrayMixer*"], "treatment"),
    row(AdjustmentField::Crop, SettingClass::Never, &["Crop*", "HasCrop"], "per-frame geometry"),
    row(
        AdjustmentField::Profile,
        SettingClass::Look,
        &["CameraProfile", "CameraProfileDigest", "Look"],
        "profile + creative look",
    ),
    row(
        AdjustmentField::Masks,
        SettingClass::Never,
        &["MaskGroupBasedCorrections"],
        "local adjustments belong to one frame",
    ),
    row(AdjustmentField::NoiseReductionLuminance, SettingClass::Look, &[], "subset of noise_reduction"),
    row(AdjustmentField::NoiseReductionColor, SettingClass::Look, &[], "subset of noise_reduction"),
    row(AdjustmentField::ProcessVersion, SettingClass::Look, &["ProcessVersion"], "same process as the anchor"),
    row(
        AdjustmentField::Transform,
        SettingClass::Never,
        &["Perspective*", "Upright*", "CropConstrainToWarp"],
        "per-frame geometry (Upright / Transform)",
    ),
];

const fn row(
    field: AdjustmentField,
    class: SettingClass,
    crs: &'static [&'static str],
    note: &'static str,
) -> PartitionRow {
    PartitionRow { field, class, crs, note }
}

/// The class of `field` per [`BASELINE_PARTITION`].
pub fn setting_class(field: AdjustmentField) -> SettingClass {
    BASELINE_PARTITION.iter().find(|r| r.field == field).map(|r| r.class).unwrap_or(SettingClass::Never)
}

/// Groups of `class` in `AdjustmentField::ALL` order. Look fields include the noise-reduction
/// subsets (harmless with `copy_fields`: the full group is copied too).
pub fn fields_of_class(class: SettingClass) -> Vec<AdjustmentField> {
    AdjustmentField::ALL.iter().copied().filter(|f| setting_class(*f) == class).collect()
}

/// The class of a `crs:` local name (or `sieve:`-prefixed name) per [`BASELINE_PARTITION`]:
/// exact names first, then the longest matching prefix. `None` = a key Sieve does not model
/// (left alone by a baseline run).
pub fn crs_key_class(name: &str) -> Option<SettingClass> {
    let mut best: Option<(usize, SettingClass)> = None;
    for r in BASELINE_PARTITION {
        for key in r.crs {
            if let Some(prefix) = key.strip_suffix('*') {
                if name.starts_with(prefix) && best.is_none_or(|(n, _)| prefix.len() > n) {
                    best = Some((prefix.len(), r.class));
                }
            } else if *key == name {
                return Some(r.class);
            }
        }
    }
    best.map(|(_, c)| c)
}

/// One row of the partition for the frontend (`BASELINE_PARTITION` constant in TS).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SettingPartitionEntry {
    pub field: AdjustmentField,
    pub class: SettingClass,
    pub crs_keys: Vec<String>,
    pub note: String,
}

/// [`BASELINE_PARTITION`] as serializable rows.
pub fn baseline_partition() -> Vec<SettingPartitionEntry> {
    BASELINE_PARTITION
        .iter()
        .map(|r| SettingPartitionEntry {
            field: r.field,
            class: r.class,
            crs_keys: r.crs.iter().map(|s| (*s).to_owned()).collect(),
            note: r.note.to_owned(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Light values
// ---------------------------------------------------------------------------

/// The light settings of one photo (absolute slider values; white balance always custom).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LightValues {
    /// EV, -5..=5.
    #[specta(type = Number)]
    pub exposure: f32,
    /// -100..=100 (the next five too).
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
    /// Kelvin, 2000..=50000.
    #[specta(type = Number)]
    pub temperature_k: f32,
    /// -150..=150.
    #[specta(type = Number)]
    pub tint: f32,
}

impl LightValues {
    /// The light values of `adj`; `None` when its white balance is `as_shot` (resolve the
    /// camera's as-shot temperature / tint first, like `sync_delta`).
    pub fn of(adj: &ParametricAdjustments) -> Option<LightValues> {
        match adj.white_balance {
            WhiteBalance::AsShot => None,
            WhiteBalance::Custom { temperature_k, tint } => Some(LightValues {
                exposure: adj.exposure,
                contrast: adj.contrast,
                highlights: adj.highlights,
                shadows: adj.shadows,
                whites: adj.whites,
                blacks: adj.blacks,
                temperature_k,
                tint,
            }),
        }
    }

    /// `adj` with the six tone sliders and a custom white balance set to these values.
    pub fn apply_to(&self, adj: &ParametricAdjustments) -> ParametricAdjustments {
        let mut out = adj.clone();
        out.exposure = self.exposure;
        out.contrast = self.contrast;
        out.highlights = self.highlights;
        out.shadows = self.shadows;
        out.whites = self.whites;
        out.blacks = self.blacks;
        out.white_balance = WhiteBalance::Custom { temperature_k: self.temperature_k, tint: self.tint };
        out
    }
}

/// The anchor's offset from its own Auto (`anchor - auto(anchor)`): what the user changed
/// relative to Auto, carried over to every photo.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LightOffset {
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
    /// Mireds (1e6 / K) added to the photo's Auto: negative = warmer than Auto.
    #[specta(type = Number)]
    pub temperature_mired: f32,
    #[specta(type = Number)]
    pub tint: f32,
}

impl LightOffset {
    /// `light - auto`.
    pub fn between(light: &LightValues, auto: &LightValues) -> LightOffset {
        LightOffset {
            exposure: light.exposure - auto.exposure,
            contrast: light.contrast - auto.contrast,
            highlights: light.highlights - auto.highlights,
            shadows: light.shadows - auto.shadows,
            whites: light.whites - auto.whites,
            blacks: light.blacks - auto.blacks,
            temperature_mired: mired(light.temperature_k) - mired(auto.temperature_k),
            tint: light.tint - auto.tint,
        }
    }

    /// `auto + self`, unclamped (the engine clamps and rounds).
    pub fn add_to(&self, auto: &LightValues) -> LightValues {
        LightValues {
            exposure: auto.exposure + self.exposure,
            contrast: auto.contrast + self.contrast,
            highlights: auto.highlights + self.highlights,
            shadows: auto.shadows + self.shadows,
            whites: auto.whites + self.whites,
            blacks: auto.blacks + self.blacks,
            temperature_k: 1e6 / (mired(auto.temperature_k) + self.temperature_mired).max(1.0),
            tint: auto.tint + self.tint,
        }
    }
}

fn mired(k: f32) -> f32 {
    1e6 / k.max(1.0)
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// Which photos of the project a baseline run edits (IPC v21).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BaselineScope {
    /// The project's keepers under `CatalogState.keeperRule` (after "Pick the best N" +
    /// Apply, the delivery set). Default.
    Keepers,
    /// Every photo of the project.
    All,
    /// These photos (must belong to the project; unknown -> `not_found`, another project ->
    /// `invalid_argument`).
    Selection { ids: Vec<ImageId> },
}

/// What to run (IPC v21; `preview_baseline` / `run_baseline`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BaselineSettings {
    /// The photo whose look is copied and whose offset from Auto carries over (must belong to
    /// the project). Never written by the run.
    pub anchor_id: ImageId,
    /// The preset chosen in step 1 (style library / `list_presets` id; unknown -> `not_found`),
    /// `null` = none. Provenance, and the look source while the anchor has no edits yet (the
    /// preset resolved on the anchor, `styles::resolve_preset`); once the anchor is edited its
    /// current settings are the look (they carry the preset).
    pub preset_id: Option<PresetId>,
    pub scope: BaselineScope,
    /// Also replace photos that already have edits (`false` = skip them: outcome
    /// `skipped_edited`). Photos still on an earlier baseline and unedited photos are always
    /// (re)written.
    pub replace_edited: bool,
}

/// Options of `preview_baseline` (`null` = defaults).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", default)]
pub struct BaselinePreviewOptions {
    /// How many photos to sample, spread across scenes (and bursts) in capture order;
    /// 1..=[`MAX_BASELINE_SAMPLES`], default [`DEFAULT_BASELINE_SAMPLES`].
    pub sample_count: u32,
    /// Preview exactly these photos instead (in this order; must be in the run's scope);
    /// `null` = sample.
    pub image_ids: Option<Vec<ImageId>>,
}

impl Default for BaselinePreviewOptions {
    fn default() -> Self {
        Self { sample_count: DEFAULT_BASELINE_SAMPLES, image_ids: None }
    }
}

// ---------------------------------------------------------------------------
// Per-photo results
// ---------------------------------------------------------------------------

string_enum! {
    /// What a baseline run did with one photo (IPC v21).
    pub enum BaselineOutcome {
        /// Written: look copied, light = its Auto + the anchor's offset.
        Applied => "applied",
        /// Written, but needs a look (`reasons`): e.g. low-key / silhouette frames are not
        /// brightened to Auto, or Auto failed and the anchor's light was copied. Shows as
        /// "needs review" (`ImageEditState.needsReview`) until `mark_reviewed` or an edit.
        Flagged => "flagged",
        /// Not written: the photo already had edits (not from a baseline) and
        /// `replaceEdited` was off.
        SkippedEdited => "skipped_edited",
        /// The anchor itself (never written).
        Anchor => "anchor",
        /// Not written: the photo could not be read (`reasons[0].text` says why).
        Failed => "failed",
    }
}

string_enum! {
    /// Why a photo was flagged / failed (IPC v21).
    pub enum BaselineReasonKind {
        /// Deliberately dark frame: kept darker than Auto would make it.
        LowKey => "low_key",
        /// Back-lit subject against a bright background: not brightened blindly.
        Silhouette => "silhouette",
        /// Auto tone / WB could not be computed: the anchor's light values were used.
        AutoFailed => "auto_failed",
        /// Mixed light (e.g. window + tungsten): white balance is uncertain.
        MixedLight => "mixed_light",
        /// A light value hit its slider limit.
        Clamped => "clamped",
        /// Original missing / unreadable (outcome `failed`).
        Unreadable => "unreadable",
        Other => "other",
    }
}

/// One reason of a [`BaselinePhotoResult`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BaselineReason {
    pub kind: BaselineReasonKind,
    /// User-facing, e.g. "Dark on purpose: kept darker than Auto".
    pub text: String,
}

/// What a run did (or a preview would do) with one photo (IPC v21).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BaselinePhotoResult {
    pub image_id: ImageId,
    pub outcome: BaselineOutcome,
    /// Most important first; empty for `applied` / `anchor`.
    pub reasons: Vec<BaselineReason>,
    pub scene_id: Option<SceneId>,
    pub burst_group_id: Option<BurstGroupId>,
    /// The photo's own Auto (with the look applied); `null` when not computed (skipped /
    /// failed / Auto failed).
    pub auto: Option<LightValues>,
    /// The light values written (Auto + offset, smoothed, clamped); `null` when not written.
    pub light: Option<LightValues>,
    /// v21.1: where the photo stands now (`get_baseline_results` only, derived on read like
    /// `BaselineProvenance.state`): `on_baseline`, `user_edited` (changed since, or kept by
    /// `undo_edit_batch(…, {keepLaterEdits: true})`) or `undone`. `null` for outcomes that
    /// wrote nothing, for photos a later run rewrote, and in `preview_baseline`.
    pub state: Option<BaselineState>,
}

/// One photo of a preview: its result and the settings before / after (nothing written).
/// Render `after` with `render_preview(imageId, after, …)` in slot `preview` (or `before` for
/// the left side; the edited thumbnail shows the current settings too).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BaselineSample {
    pub photo: BaselinePhotoResult,
    pub before: ParametricAdjustments,
    /// What the run would write (= `before` when the outcome writes nothing).
    pub after: ParametricAdjustments,
}

/// The anchor as the engine sees it (IPC v21).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BaselineAnchor {
    pub image_id: ImageId,
    /// The anchor's light values (its as-shot white balance resolved).
    pub light: LightValues,
    /// The anchor's own Auto (with its look).
    pub auto: LightValues,
    /// `light - auto`: carried over to every photo.
    pub offset: LightOffset,
}

/// Counts of a planned run (`preview_baseline`): known without computing any Auto.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BaselinePlanCounts {
    /// Photos in scope (incl. the anchor when in scope).
    pub in_scope: u32,
    /// Photos the run would write (unedited + on baseline + edited when `replaceEdited`).
    pub to_write: u32,
    /// Of `toWrite`: photos still on an earlier baseline (updated by a re-run).
    pub on_baseline: u32,
    /// Photos with their own edits: skipped, or replaced when `replaceEdited`.
    pub edited: u32,
}

/// Result of `preview_baseline` (IPC v21). Nothing is written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BaselinePreview {
    pub project_id: ProjectId,
    pub settings: BaselineSettings,
    pub anchor: BaselineAnchor,
    /// Spread across scenes, capture order (or `imageIds` order).
    pub samples: Vec<BaselineSample>,
    pub counts: BaselinePlanCounts,
    /// Engine version that computed it (`BaselineRun.engineVersion`).
    pub engine_version: String,
}

// ---------------------------------------------------------------------------
// Runs
// ---------------------------------------------------------------------------

string_enum! {
    /// Lifecycle of a baseline run (IPC v21).
    pub enum BaselineRunState {
        Running => "running",
        /// Written (one batch); per-photo results stored.
        Finished => "finished",
        /// Nothing written; `message` says why.
        Failed => "failed",
        /// Stopped by `cancel_baseline` (or the app quit) before writing: nothing written.
        Cancelled => "cancelled",
    }
}

/// Outcome counts of a run (sums to `total`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BaselineCounts {
    pub total: u32,
    pub applied: u32,
    pub flagged: u32,
    pub skipped_edited: u32,
    pub anchor: u32,
    pub failed: u32,
}

/// One `run_baseline` (IPC v21). `get_baseline_run` returns the project's latest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BaselineRun {
    pub id: BaselineRunId,
    pub project_id: ProjectId,
    pub settings: BaselineSettings,
    pub state: BaselineRunState,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
    /// User-facing summary or error ("Edited 742 photos; 18 need a look; 40 already edited
    /// were skipped").
    pub message: Option<String>,
    pub engine_version: String,
    /// Known once the anchor was measured (`null` while starting, or failed before it).
    pub anchor: Option<BaselineAnchor>,
    /// Zero until finished.
    pub counts: BaselineCounts,
    /// The edit batch it wrote (kind `baseline`): undo with `undo_edit_batch(batch.batchId)`
    /// while `batch.undoable`, or `undo_edit_batch(batch.batchId, {keepLaterEdits: true})`
    /// ("Undo the rest", v21.1) while `batch.undoneAtMs` is null. `null` = nothing written.
    pub batch: Option<EditBatchInfo>,
    /// v21.1: where the photos this run wrote stand now (derived on read; all zero until it
    /// finished). After an undo `message` is replaced by the undo summary ("Undone: the
    /// photos are back to how they were", or "… 1 photo you changed since was kept").
    pub live: BaselineLiveCounts,
}

/// Where the photos a run wrote stand now (IPC v21.1, `BaselineRun.live`), per
/// [`BaselineProvenance`] state. Photos a later run rewrote count for that run only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BaselineLiveCounts {
    /// Photos this run wrote that no later run rewrote (`onBaseline + userEdited + undone`).
    pub written: u32,
    /// Still carrying what the run wrote (a re-run updates them).
    pub on_baseline: u32,
    /// Of `onBaseline`: written as flagged and not reviewed / edited yet ("7 need a look").
    pub needs_look: u32,
    /// Changed since (an edit, a paste, another batch), incl. photos `keepLaterEdits` kept.
    pub user_edited: u32,
    /// Put back by undoing the run's batch.
    pub undone: u32,
}

/// `EditPlan.baseline` (IPC v21.1): the project's baseline in the Edit step's terms (keepers
/// under `CatalogState.keeperRule`). Present while the latest run is finished and its batch is
/// not undone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EditPlanBaseline {
    pub run_id: BaselineRunId,
    pub batch: EditBatchInfo,
    pub anchor_id: ImageId,
    pub preset_id: Option<PresetId>,
    /// `EditPlan.keeperIds.length` ("of 43 keepers").
    pub keepers: u32,
    /// Keepers whose current settings are a baseline's (`editSource` = `baseline`): "42".
    pub on_baseline: u32,
    /// Of `onBaseline`: flagged and not reviewed yet ("7 need a look").
    pub needs_look: u32,
    /// Keepers this run wrote that were changed since (`BaselineState::UserEdited`).
    pub edited_since: u32,
}

/// Result of `auto_light` (IPC v21.1): **the** light-only Auto of one photo
/// (`develop::auto::auto_light`: auto white balance first, then the six tone sliders measured
/// under it, faces resolved as in Develop; vibrance / saturation never). Identical to what a
/// baseline run measures for that photo with the same settings (`BaselineAnchor.auto` for the
/// anchor), so "Auto, then nudge" starts the anchor's offset at exactly zero.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AutoLightValues {
    /// Exposure, contrast, highlights, shadows, whites, blacks and a custom white balance.
    pub light: LightValues,
    /// `false`: auto white balance could not be estimated (too few neutral pixels); the
    /// camera's as-shot temperature / tint is returned, and the tone was measured under the
    /// photo's own white balance (as the baseline does).
    pub white_balance_estimated: bool,
}

string_enum! {
    /// Where a photo stands relative to the baseline that wrote it (IPC v21).
    pub enum BaselineState {
        /// Its current settings are what the baseline wrote (history cursor on that entry):
        /// a re-run updates it.
        OnBaseline => "on_baseline",
        /// Changed since (an edit, a paste, per-photo undo, another batch): a re-run skips it
        /// unless `replaceEdited`.
        UserEdited => "user_edited",
        /// The baseline batch was undone.
        Undone => "undone",
    }
}

/// Baseline provenance of one photo (IPC v21, table `baseline_provenance`): the last baseline
/// run that wrote its settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BaselineProvenance {
    pub image_id: ImageId,
    pub run_id: BaselineRunId,
    pub batch_id: EditBatchId,
    pub anchor_id: Option<ImageId>,
    pub preset_id: Option<PresetId>,
    pub applied_at_ms: i64,
    pub state: BaselineState,
    /// Written as flagged (needs a look) by that run.
    pub flagged: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_covers_every_field_once_in_order() {
        let fields: Vec<AdjustmentField> = BASELINE_PARTITION.iter().map(|r| r.field).collect();
        assert_eq!(fields, AdjustmentField::ALL.to_vec(), "every AdjustmentField exactly once, ALL order");
    }

    #[test]
    fn partition_matches_the_roadmap_model() {
        use AdjustmentField as F;
        let light = fields_of_class(SettingClass::Light);
        assert_eq!(
            light,
            vec![F::WhiteBalance, F::Exposure, F::Contrast, F::Highlights, F::Shadows, F::Whites, F::Blacks]
        );
        assert_eq!(fields_of_class(SettingClass::Never), vec![F::Crop, F::Masks, F::Transform]);
        for f in [F::Profile, F::HslHue, F::ColorGrading, F::ToneCurve, F::Calibration, F::Vibrance, F::Lut, F::Grain] {
            assert_eq!(setting_class(f), SettingClass::Look, "{f:?}");
        }
    }

    #[test]
    fn crs_keys_classify_by_exact_name_then_longest_prefix() {
        assert_eq!(crs_key_class("Exposure2012"), Some(SettingClass::Light));
        assert_eq!(crs_key_class("Temperature"), Some(SettingClass::Light));
        assert_eq!(crs_key_class("Saturation"), Some(SettingClass::Look));
        assert_eq!(crs_key_class("SaturationAdjustmentRed"), Some(SettingClass::Look));
        assert_eq!(crs_key_class("ToneCurvePV2012Red"), Some(SettingClass::Look));
        assert_eq!(crs_key_class("CropConstrainToWarp"), Some(SettingClass::Never));
        assert_eq!(crs_key_class("UprightTransform_0"), Some(SettingClass::Never));
        assert_eq!(crs_key_class("Look"), Some(SettingClass::Look));
        assert_eq!(crs_key_class("LensProfileEnable"), None);
        assert_eq!(crs_key_class("RetouchInfo"), None);
    }

    #[test]
    fn offset_round_trips() {
        let auto = LightValues { exposure: 0.4, temperature_k: 5000.0, tint: 5.0, ..LightValues::default() };
        let anchor = LightValues { exposure: 0.7, temperature_k: 5500.0, tint: 8.0, ..LightValues::default() };
        let off = LightOffset::between(&anchor, &auto);
        let back = off.add_to(&auto);
        assert!((back.exposure - 0.7).abs() < 1e-5);
        assert!((back.temperature_k - 5500.0).abs() < 0.5);
        assert!(off.temperature_mired < 0.0, "warmer than Auto = fewer mireds");
    }
}
