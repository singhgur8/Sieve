//! Baseline edit engine (Phase 10, IPC v21): one preset + one edited photo -> the whole shoot.
//!
//! Owned by rust-engine-dev (light normalization, clamps, smoothing, WB per lighting) with
//! vision-ml-dev (low-key / silhouette detection). The architect fixed the surface used by
//! `ipc::commands` and `lib.rs`: [`BaselineEdit`] (`new`, `is_running`, `start`, `cancel`),
//! [`BaselineJob`], [`preview`] and [`run_pipeline`]; the stages below are the seams the tests
//! use. **The bodies are a stub**: look keys are copied, light keys are set to the photo's plain
//! Auto (no anchor offset, no smoothing, no low-key flags), which is enough to run end to end.
//!
//! Model (roadmap Phase 10, decisions.md 2026-10-10; partition: [`BASELINE_PARTITION`]):
//! - **Look** = the anchor's settings for every `look` group, copied as-is
//!   ([`compose`]; the anchor carries the preset, see `db::baseline::look_source`).
//! - **Light** (exposure, contrast, highlights, shadows, whites, blacks, white balance) =
//!   the photo's own Auto measured **with the look applied** ([`LightMeter::auto_light`]) +
//!   the anchor's offset from its own Auto ([`measure_anchor`], [`LightOffset`]), clamped to
//!   the slider ranges and rounded like Lightroom (exposure 0.05 EV, others integers, Kelvin
//!   whole). Target behaviour for the real engine:
//!   1. per photo ([`photo_light`]): Auto + offset; Auto failing -> the anchor's light values,
//!      flagged `auto_failed`;
//!   2. low-key / silhouette ([`low_key`], vision-ml-dev): deliberate dark frames and
//!      back-lit silhouettes keep (most of) their own brightness instead of being lifted to
//!      Auto: flagged `low_key` / `silhouette` (still written, "needs review");
//!   3. smoothing ([`smooth`]): photos of one burst (`burst_group_id`), and of one scene
//!      (`scene_id`) within a short time, get consistent light (no flicker through a
//!      sequence): e.g. a robust (median) exposure / WB per burst with each frame's own
//!      deviation damped, scene-level WB per lighting (mixed indoor / outdoor scenes keep
//!      separate WB clusters, flagged `mixed_light` when uncertain);
//!   4. clamps ([`clamp_light`]): out-of-range values clamped and flagged `clamped`.
//!
//!   Deterministic: same catalog + settings -> same values (order by capture time, then id).
//! - **Never**: crop, transform, masks keep the photo's own values (unmodelled `crs:` keys such
//!   as spot removal stay untouched in the sidecar).
//! - Scope / skip: `db::baseline::scope_photos`; edited photos are `skipped_edited` unless
//!   `replaceEdited`; photos on an earlier baseline and unedited photos are written; the anchor
//!   is never written.
//!
//! Acceptance (roadmap "Light normalization engine"): on sample scenes the rendered frames'
//! mean luma and neutral-grey WB vary far less across a scene than a plain copy of the
//! anchor's settings and stay within tolerance of the anchor's look; look keys byte-identical
//! to the anchor's on every photo; timings for 2,500 photos recorded.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rusqlite::Connection;
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use super::{DevelopCache, SourceImage};
use crate::db::baseline::{self as store, BaselineDraft, PhotoEditState, ScopePhoto};
use crate::ipc::activity::activities;
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::events::{ActivityKind, ActivityState, BaselineRunFinished};
use crate::ipc::types::*;

/// Version of the engine (stored in `BaselineRun.engineVersion`). Bump when results change.
pub const ENGINE_VERSION: &str = "baseline-stub-1";

/// The Auto sliders the light stage uses (Lightroom's Basic Auto minus vibrance / saturation,
/// which are look).
pub const LIGHT_TONE_FIELDS: &[AdjustmentField] = &[
    AdjustmentField::Exposure,
    AdjustmentField::Contrast,
    AdjustmentField::Highlights,
    AdjustmentField::Shadows,
    AdjustmentField::Whites,
    AdjustmentField::Blacks,
];

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

/// What the engine knows about one photo.
#[derive(Debug, Clone, PartialEq)]
pub struct PhotoInput {
    pub image_id: ImageId,
    pub src: SourceImage,
    pub format: ImageFormat,
    /// Current settings (the never groups are kept from these).
    pub current: ParametricAdjustments,
    pub scene_id: Option<SceneId>,
    pub burst_group_id: Option<BurstGroupId>,
    pub captured_at_ms: Option<i64>,
    /// Analysis face boxes (oriented preview frame), `None` when not analysed.
    pub faces: Option<Vec<NormRect>>,
    pub state: PhotoEditState,
    /// The original is missing (`RawImageEntry.missingSinceMs`).
    pub missing: bool,
}

impl PhotoInput {
    /// From a scope row + its current settings and faces.
    pub fn new(p: &ScopePhoto, current: ParametricAdjustments, faces: Option<Vec<NormRect>>) -> Self {
        let e = &p.entry;
        PhotoInput {
            image_id: e.id,
            src: SourceImage { id: e.id, path: PathBuf::from(&e.path), orientation: e.orientation },
            format: e.format,
            current,
            scene_id: e.scene_id,
            burst_group_id: e.burst_group_id,
            captured_at_ms: e.capture.captured_at_ms,
            faces,
            state: p.state,
            missing: e.missing_since_ms.is_some(),
        }
    }
}

/// Reads the inputs of `photos` (current settings + analysis faces).
pub fn photo_inputs(conn: &Connection, photos: &[ScopePhoto]) -> AppResult<Vec<PhotoInput>> {
    photos
        .iter()
        .map(|p| {
            let current = crate::db::repo::get_adjustments(conn, p.entry.id)?;
            let faces = super::auto::analysis_faces(conn, p.entry.id)?.map(|f| super::auto::face_boxes(&f));
            Ok(PhotoInput::new(p, current, faces))
        })
        .collect()
}

/// Measures Auto. [`DevelopMeter`] in the app; synthetic meters in tests.
pub trait LightMeter: Sync {
    /// Lightroom-style Auto light of `photo` rendered with `look` (its light groups ignored):
    /// the six [`LIGHT_TONE_FIELDS`] + auto white balance (always custom).
    fn auto_light(&self, photo: &PhotoInput, look: &ParametricAdjustments) -> AppResult<LightValues>;
    /// The camera's as-shot temperature / tint (to resolve an `as_shot` anchor).
    fn as_shot(&self, photo: &PhotoInput) -> AppResult<WhiteBalanceValues>;
}

/// [`LightMeter`] over the develop cache (`develop::auto`).
#[derive(Clone)]
pub struct DevelopMeter {
    pub cache: DevelopCache,
}

impl LightMeter for DevelopMeter {
    fn auto_light(&self, photo: &PhotoInput, look: &ParametricAdjustments) -> AppResult<LightValues> {
        let faces = super::auto::resolve_faces(&self.cache, &photo.src, photo.faces.clone());
        let tone =
            super::auto::auto_tone_with_faces(&self.cache, &photo.src, look, LIGHT_TONE_FIELDS, faces.as_deref())?;
        let wb = super::auto::auto_white_balance(&self.cache, &photo.src, look)?;
        Ok(LightValues {
            exposure: tone.exposure.unwrap_or(0.0),
            contrast: tone.contrast.unwrap_or(0.0),
            highlights: tone.highlights.unwrap_or(0.0),
            shadows: tone.shadows.unwrap_or(0.0),
            whites: tone.whites.unwrap_or(0.0),
            blacks: tone.blacks.unwrap_or(0.0),
            temperature_k: wb.temperature_k,
            tint: wb.tint,
        })
    }

    fn as_shot(&self, photo: &PhotoInput) -> AppResult<WhiteBalanceValues> {
        self.cache
            .info(&photo.src)?
            .as_shot
            .ok_or_else(|| AppError::invalid("the camera's as-shot white balance is unknown"))
    }
}

// ---------------------------------------------------------------------------
// Stages
// ---------------------------------------------------------------------------

/// `current` with every look group copied from `look` and the light groups set to `light`;
/// never groups (crop, transform, masks) keep `current`'s values.
pub fn compose(
    current: &ParametricAdjustments,
    look: &ParametricAdjustments,
    light: &LightValues,
) -> ParametricAdjustments {
    let mut out = current.clone();
    out.copy_fields(look, &fields_of_class(SettingClass::Look));
    light.apply_to(&out)
}

/// The anchor's light values (as-shot white balance resolved), its Auto with its look, and the
/// offset. Stub: as specified (the offset is computed; [`photo_light`] does not use it yet).
pub fn measure_anchor(
    meter: &dyn LightMeter,
    anchor: &PhotoInput,
    look: &ParametricAdjustments,
) -> AppResult<BaselineAnchor> {
    let auto = meter.auto_light(anchor, look)?;
    let light = match LightValues::of(look) {
        Some(v) => v,
        None => {
            let wb = meter.as_shot(anchor)?;
            LightValues::of(&ParametricAdjustments {
                white_balance: WhiteBalance::Custom { temperature_k: wb.temperature_k, tint: wb.tint },
                ..look.clone()
            })
            .unwrap_or(auto)
        }
    };
    Ok(BaselineAnchor { image_id: anchor.image_id, light, auto, offset: LightOffset::between(&light, &auto) })
}

/// Light of one photo before smoothing.
#[derive(Debug, Clone, PartialEq)]
pub struct PhotoLight {
    pub auto: Option<LightValues>,
    pub light: LightValues,
    pub reasons: Vec<BaselineReason>,
}

/// Per-photo light. Target: `auto + anchor.offset` ([`LightOffset::add_to`]), low-key /
/// silhouette handling ([`low_key`]), Auto failure -> the anchor's light + `auto_failed`.
/// **Stub**: plain Auto (no offset).
pub fn photo_light(
    meter: &dyn LightMeter,
    photo: &PhotoInput,
    look: &ParametricAdjustments,
    anchor: &BaselineAnchor,
) -> PhotoLight {
    match meter.auto_light(photo, look) {
        Ok(auto) => {
            let mut reasons = Vec::new();
            if let Some(r) = low_key(photo, &auto) {
                reasons.push(r);
            }
            PhotoLight { auto: Some(auto), light: auto, reasons }
        }
        Err(e) => PhotoLight {
            auto: None,
            light: anchor.light,
            reasons: vec![BaselineReason {
                kind: BaselineReasonKind::AutoFailed,
                text: format!("Auto could not be computed ({}); used the anchor's light", e.message),
            }],
        },
    }
}

/// Deliberate low-key frame / silhouette (vision-ml-dev): `Some` keeps the frame from being
/// brightened blindly and flags it. **Stub**: never.
pub fn low_key(_photo: &PhotoInput, _auto: &LightValues) -> Option<BaselineReason> {
    None
}

/// Consistent light across bursts / scenes (rust-engine-dev): adjusts `lights[i]` (same order
/// as `photos`) in place, may add reasons (`mixed_light`). **Stub**: no change.
pub fn smooth(_photos: &[&PhotoInput], _lights: &mut [PhotoLight]) {}

/// Clamps to the slider ranges and rounds like Lightroom; `true` when something was clamped.
pub fn clamp_light(v: &LightValues) -> (LightValues, bool) {
    use super::wb::{MAX_TEMP, MAX_TINT, MIN_TEMP, MIN_TINT};
    let mut clamped = false;
    let mut c = |x: f32, lo: f32, hi: f32| {
        let x = if x.is_finite() { x } else { 0.0 };
        if x < lo || x > hi {
            clamped = true;
        }
        x.clamp(lo, hi)
    };
    let out = LightValues {
        exposure: (c(v.exposure, -5.0, 5.0) * 20.0).round() / 20.0,
        contrast: c(v.contrast, -100.0, 100.0).round(),
        highlights: c(v.highlights, -100.0, 100.0).round(),
        shadows: c(v.shadows, -100.0, 100.0).round(),
        whites: c(v.whites, -100.0, 100.0).round(),
        blacks: c(v.blacks, -100.0, 100.0).round(),
        temperature_k: c(v.temperature_k, MIN_TEMP, MAX_TEMP).round(),
        tint: c(v.tint, MIN_TINT, MAX_TINT).round(),
    };
    (out, clamped)
}

/// Indices of up to `n` photos spread across scenes in capture order: scenes (photos without a
/// scene form one group) share `n` round-robin; within a scene the picks are evenly spaced over
/// one frame per burst first, then the other frames. Sorted, deterministic.
pub fn pick_samples(photos: &[PhotoInput], n: usize) -> Vec<usize> {
    let mut groups: Vec<(Option<SceneId>, Vec<usize>)> = Vec::new();
    for (i, p) in photos.iter().enumerate() {
        match groups.iter_mut().find(|(k, _)| *k == p.scene_id) {
            Some((_, v)) => v.push(i),
            None => groups.push((p.scene_id, vec![i])),
        }
    }
    let mut quota = vec![0usize; groups.len()];
    let mut left = n.min(photos.len());
    while left > 0 {
        for (q, (_, g)) in quota.iter_mut().zip(&groups) {
            if left > 0 && *q < g.len() {
                *q += 1;
                left -= 1;
            }
        }
    }
    let spread = |list: &[usize], q: usize| -> Vec<usize> {
        (0..q).map(|j| list[((2 * j + 1) * list.len()) / (2 * q)]).collect()
    };
    let mut out = Vec::new();
    for ((_, g), q) in groups.iter().zip(quota) {
        let (mut firsts, mut rest) = (Vec::new(), Vec::new());
        for &i in g {
            let b = photos[i].burst_group_id;
            if b.is_none() || !firsts.iter().any(|&j: &usize| photos[j].burst_group_id == b) {
                firsts.push(i);
            } else {
                rest.push(i);
            }
        }
        if q <= firsts.len() {
            out.extend(spread(&firsts, q));
        } else {
            out.extend(firsts.iter().copied());
            out.extend(spread(&rest, q - firsts.len()));
        }
    }
    out.sort_unstable();
    out
}

/// Plans every photo of `photos` (scope order). `None` when cancelled. `progress(done,
/// total)` after each measured photo.
pub fn compute(
    meter: &dyn LightMeter,
    look: &ParametricAdjustments,
    anchor: &BaselineAnchor,
    photos: &[PhotoInput],
    settings: &BaselineSettings,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u32, u32),
) -> Option<Vec<BaselineDraft>> {
    let result = |p: &PhotoInput, outcome: BaselineOutcome, reasons: Vec<BaselineReason>| BaselinePhotoResult {
        image_id: p.image_id,
        outcome,
        reasons,
        scene_id: p.scene_id,
        burst_group_id: p.burst_group_id,
        auto: None,
        light: None,
    };
    let mut drafts: Vec<Option<BaselineDraft>> = vec![None; photos.len()];
    let mut to_measure: Vec<usize> = Vec::new();
    for (i, p) in photos.iter().enumerate() {
        let skip = if p.image_id == settings.anchor_id {
            Some(result(p, BaselineOutcome::Anchor, Vec::new()))
        } else if p.state == PhotoEditState::Edited && !settings.replace_edited {
            Some(result(p, BaselineOutcome::SkippedEdited, Vec::new()))
        } else if p.missing {
            Some(result(
                p,
                BaselineOutcome::Failed,
                vec![BaselineReason { kind: BaselineReasonKind::Unreadable, text: "The original is missing".into() }],
            ))
        } else {
            None
        };
        match skip {
            Some(r) => drafts[i] = Some(BaselineDraft { result: r, adjustments: None }),
            None => to_measure.push(i),
        }
    }
    let total = to_measure.len() as u32;
    let mut lights = Vec::with_capacity(to_measure.len());
    for (n, &i) in to_measure.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            return None;
        }
        lights.push(photo_light(meter, &photos[i], look, anchor));
        progress(n as u32 + 1, total);
    }
    let measured: Vec<&PhotoInput> = to_measure.iter().map(|&i| &photos[i]).collect();
    smooth(&measured, &mut lights);
    for (&i, pl) in to_measure.iter().zip(lights) {
        let p = &photos[i];
        let (light, clamped) = clamp_light(&pl.light);
        let mut reasons = pl.reasons;
        if clamped {
            reasons.push(BaselineReason {
                kind: BaselineReasonKind::Clamped,
                text: "A light value hit its slider limit".into(),
            });
        }
        let flagged = reasons.iter().any(|r| r.kind != BaselineReasonKind::Clamped);
        let adjustments = compose(&p.current, look, &light);
        let mut r = result(p, if flagged { BaselineOutcome::Flagged } else { BaselineOutcome::Applied }, reasons);
        r.auto = pl.auto;
        r.light = Some(light);
        drafts[i] = Some(BaselineDraft { result: r, adjustments: Some(adjustments) });
    }
    Some(drafts.into_iter().flatten().collect())
}

// ---------------------------------------------------------------------------
// Preview + run
// ---------------------------------------------------------------------------

/// Everything a preview / run needs, read from the catalog.
pub struct Prepared {
    pub settings: BaselineSettings,
    pub look: ParametricAdjustments,
    pub anchor: PhotoInput,
    pub photos: Vec<PhotoInput>,
    pub counts: BaselinePlanCounts,
}

/// Resolves `settings` and reads the scope, the anchor and the look. Errors as
/// `db::baseline::resolve_settings`.
pub fn prepare(conn: &Connection, project_id: ProjectId, settings: &BaselineSettings) -> AppResult<Prepared> {
    let settings = store::resolve_settings(conn, project_id, settings)?;
    let look = store::look_source(conn, &settings)?;
    let scope = store::scope_photos(conn, project_id, &settings)?;
    let counts = store::plan_counts(&scope, &settings);
    let anchor_entry = crate::db::repo::get_images(conn, &[settings.anchor_id])?.remove(0);
    let anchor_state = store::photo_states(conn, &[settings.anchor_id])?;
    let anchor_row = ScopePhoto {
        entry: anchor_entry,
        state: anchor_state.get(&settings.anchor_id).copied().unwrap_or(PhotoEditState::Edited),
    };
    let anchor = photo_inputs(conn, std::slice::from_ref(&anchor_row))?.remove(0);
    let photos = photo_inputs(conn, &scope)?;
    Ok(Prepared { settings, look, anchor, photos, counts })
}

/// `preview_baseline` on prepared inputs: the anchor is measured, then the samples (spread
/// across scenes, or `options.imageIds`) are planned. Nothing is written. `imageIds` outside
/// the scope -> `invalid_argument`.
pub fn preview(
    meter: &dyn LightMeter,
    project_id: ProjectId,
    prepared: &Prepared,
    options: &BaselinePreviewOptions,
) -> AppResult<BaselinePreview> {
    let n = options.sample_count;
    if n == 0 || n > MAX_BASELINE_SAMPLES {
        return Err(AppError::invalid(format!("sampleCount must be 1..={MAX_BASELINE_SAMPLES}")));
    }
    let idx: Vec<usize> = match &options.image_ids {
        Some(ids) => {
            if ids.is_empty() || ids.len() > MAX_BASELINE_SAMPLES as usize {
                return Err(AppError::invalid(format!("imageIds must have 1..={MAX_BASELINE_SAMPLES} photos")));
            }
            ids.iter()
                .map(|id| {
                    prepared
                        .photos
                        .iter()
                        .position(|p| p.image_id == *id)
                        .ok_or_else(|| AppError::invalid(format!("image {id} is not in the baseline's scope")))
                })
                .collect::<AppResult<_>>()?
        }
        None => {
            // Prefer photos the run would write; fill with the rest.
            let writable: Vec<PhotoInput> = prepared
                .photos
                .iter()
                .filter(|p| p.image_id != prepared.settings.anchor_id)
                .filter(|p| p.state != PhotoEditState::Edited || prepared.settings.replace_edited)
                .cloned()
                .collect();
            pick_samples(&writable, n as usize)
                .into_iter()
                .filter_map(|i| prepared.photos.iter().position(|p| p.image_id == writable[i].image_id))
                .collect()
        }
    };
    let anchor = measure_anchor(meter, &prepared.anchor, &prepared.look)?;
    let sample: Vec<PhotoInput> = idx.iter().map(|&i| prepared.photos[i].clone()).collect();
    let never = AtomicBool::new(false);
    let drafts = compute(meter, &prepared.look, &anchor, &sample, &prepared.settings, &never, &mut |_, _| {})
        .unwrap_or_default();
    let samples = sample
        .iter()
        .zip(drafts)
        .map(|(p, d)| BaselineSample {
            after: d.adjustments.clone().unwrap_or_else(|| p.current.clone()),
            before: p.current.clone(),
            photo: d.result,
        })
        .collect();
    Ok(BaselinePreview {
        project_id,
        settings: prepared.settings.clone(),
        anchor,
        samples,
        counts: prepared.counts,
        engine_version: ENGINE_VERSION.to_owned(),
    })
}

/// One accepted `run_baseline` (the run row is already `running`).
#[derive(Debug, Clone, PartialEq)]
pub struct BaselineJob {
    pub run_id: BaselineRunId,
    pub project_id: ProjectId,
    /// Resolved.
    pub settings: BaselineSettings,
}

/// What [`run_pipeline`] produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PipelineOutcome {
    /// Stopped by `cancel` before anything was written.
    pub cancelled: bool,
    pub message: Option<String>,
    /// Photos written (for the XMP notify).
    pub changed: usize,
}

/// User-facing summary of a finished run.
pub fn summary(counts: &BaselineCounts) -> String {
    let written = counts.applied + counts.flagged;
    let mut m = format!("Edited {written} photo{}", if written == 1 { "" } else { "s" });
    if counts.flagged > 0 {
        m.push_str(&format!("; {} need a look", counts.flagged));
    }
    if counts.skipped_edited > 0 {
        m.push_str(&format!("; {} already edited were skipped", counts.skipped_edited));
    }
    if counts.failed > 0 {
        m.push_str(&format!("; {} could not be read", counts.failed));
    }
    m
}

/// The whole run on the worker's connection: prepare, measure the anchor (stored on the run),
/// plan every photo ([`compute`]), then write ([`store::store_results`]: one batch + results +
/// provenance). Nothing is written when cancelled before the last step.
pub fn run_pipeline(
    conn: &mut Connection,
    meter: &dyn LightMeter,
    job: &BaselineJob,
    cancel: &AtomicBool,
    report: &mut dyn FnMut(&str, u32, Option<u32>),
) -> AppResult<PipelineOutcome> {
    report("Preparing the baseline", 0, None);
    let prepared = prepare(conn, job.project_id, &job.settings)?;
    let anchor = measure_anchor(meter, &prepared.anchor, &prepared.look)?;
    store::set_anchor(conn, job.run_id, &anchor)?;
    let drafts = compute(meter, &prepared.look, &anchor, &prepared.photos, &prepared.settings, cancel, &mut |d, t| {
        report("Editing photos", d, Some(t))
    });
    let Some(drafts) = drafts else {
        return Ok(PipelineOutcome { cancelled: true, ..PipelineOutcome::default() });
    };
    if cancel.load(Ordering::SeqCst) {
        return Ok(PipelineOutcome { cancelled: true, ..PipelineOutcome::default() });
    }
    report("Saving", 0, None);
    let (batch, counts) = store::store_results(conn, job.run_id, &drafts)?;
    Ok(PipelineOutcome { cancelled: false, message: Some(summary(&counts)), changed: batch.changed_ids.len() })
}

// ---------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------

/// Resolved at startup by `lib.rs`.
#[derive(Debug, Clone)]
pub struct BaselineConfig {
    /// Catalog file; the worker opens its own connection to it.
    pub catalog_path: PathBuf,
}

/// Managed Tauri state: the baseline worker (one run at a time, any project).
pub struct BaselineEdit {
    config: BaselineConfig,
    running: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

impl BaselineEdit {
    pub fn new(config: BaselineConfig) -> Self {
        Self { config, running: Arc::new(AtomicBool::new(false)), cancel: Arc::new(AtomicBool::new(false)) }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Starts `job` on a background thread and returns. Reports `activityEvent` kind
    /// `baseline_edit` and ends with exactly one `BaselineRunFinished`. A run in progress ->
    /// `invalid_argument`.
    pub fn start(&self, app: &AppHandle, job: BaselineJob) -> AppResult<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Err(AppError::invalid("A baseline edit is already running"));
        }
        self.cancel.store(false, Ordering::SeqCst);
        let (config, running, cancel, app) =
            (self.config.clone(), self.running.clone(), self.cancel.clone(), app.clone());
        let spawned = std::thread::Builder::new().name("baseline-edit".into()).spawn(move || {
            worker(&app, &config, &job, &cancel);
            running.store(false, Ordering::SeqCst);
        });
        if let Err(e) = spawned {
            self.running.store(false, Ordering::SeqCst);
            return Err(AppError::internal(e.to_string()));
        }
        Ok(())
    }

    /// Asks the running job to stop (nothing is written). No-op when idle.
    pub fn cancel(&self) {
        if self.is_running() {
            self.cancel.store(true, Ordering::SeqCst);
        }
    }
}

fn worker(app: &AppHandle, config: &BaselineConfig, job: &BaselineJob, cancel: &AtomicBool) {
    let channel = format!("baseline-{}", job.run_id);
    let hub = activities(app);
    let mut report = |label: &str, done: u32, total: Option<u32>| {
        if let Some(h) = &hub {
            h.progress(&channel, ActivityKind::BaselineEdit, label, done, total);
        }
    };
    report("Baseline edit", 0, None);
    let result = match app.try_state::<DevelopCache>() {
        Some(cache) => {
            let meter = DevelopMeter { cache: cache.inner().clone() };
            crate::db::open(&config.catalog_path)
                .and_then(|mut conn| run_pipeline(&mut conn, &meter, job, cancel, &mut report))
        }
        None => Err(AppError::internal("develop cache unavailable")),
    };
    let (state, message) = match &result {
        Ok(o) if o.cancelled => (BaselineRunState::Cancelled, Some("Stopped; nothing was changed".to_owned())),
        Ok(o) => (BaselineRunState::Finished, o.message.clone()),
        Err(e) => (BaselineRunState::Failed, Some(e.message.clone())),
    };
    let run = crate::db::open(&config.catalog_path).and_then(|conn| {
        store::finish_run(&conn, job.run_id, state, message.as_deref())?;
        store::run_by_id(&conn, job.run_id, true)
    });
    if matches!(&result, Ok(o) if o.changed > 0) {
        if let Some(xmp) = app.try_state::<crate::xmp::XmpSync>() {
            xmp.notify(app);
        }
    }
    if let Some(h) = &hub {
        let activity_state = match state {
            BaselineRunState::Finished => ActivityState::Finished,
            BaselineRunState::Cancelled => ActivityState::Cancelled,
            _ => ActivityState::Error,
        };
        h.finish(&channel, activity_state, message.clone());
    }
    match run {
        Ok(run) => {
            let _ = BaselineRunFinished { run }.emit(app);
        }
        Err(e) => eprintln!("baseline edit: could not report the run: {}", e.message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    /// Auto = exposure by image id, 5000 K; image 4 fails.
    struct FakeMeter;
    impl LightMeter for FakeMeter {
        fn auto_light(&self, photo: &PhotoInput, _look: &ParametricAdjustments) -> AppResult<LightValues> {
            if photo.image_id == 4 {
                return Err(AppError::invalid("no"));
            }
            Ok(LightValues {
                exposure: photo.image_id as f32 * 0.1,
                temperature_k: 5000.0,
                tint: 3.0,
                ..Default::default()
            })
        }
        fn as_shot(&self, _photo: &PhotoInput) -> AppResult<WhiteBalanceValues> {
            Ok(WhiteBalanceValues { temperature_k: 5200.0, tint: 1.0 })
        }
    }

    fn seed() -> Connection {
        let conn = open_in_memory();
        conn.execute_batch(
            "INSERT INTO projects (id, name, shoot_type, created_at) VALUES (1, 'p', 'wedding', 0);
             INSERT INTO folders (id, path, added_at, project_id) VALUES (1, '/f', 0, 1);
             INSERT INTO images (id, folder_id, path, file_name, format, camera_make, sensor_layout,
                                 file_size, file_mtime_ms, imported_at, captured_at_ms)
             VALUES (1, 1, '/f/a.arw', 'a.arw', 'arw', 'sony', 'bayer', 1, 0, 0, 0),
                    (2, 1, '/f/b.arw', 'b.arw', 'arw', 'sony', 'bayer', 1, 0, 0, 1000),
                    (3, 1, '/f/c.arw', 'c.arw', 'arw', 'sony', 'bayer', 1, 0, 0, 2000),
                    (4, 1, '/f/d.arw', 'd.arw', 'arw', 'sony', 'bayer', 1, 0, 0, 3000),
                    (5, 1, '/f/e.arw', 'e.arw', 'arw', 'sony', 'bayer', 1, 0, 0, 4000);",
        )
        .unwrap();
        conn
    }

    /// The anchor (1): a look (HSL, grain, profile amount), its own light, a crop.
    fn edit_anchor(conn: &mut Connection) -> ParametricAdjustments {
        let mut a = ParametricAdjustments::default();
        a.hsl.saturation.orange = -20.0;
        a.effects.grain.amount = 25.0;
        a.vibrance = 15.0;
        a.exposure = 0.8;
        a.white_balance = WhiteBalance::Custom { temperature_k: 5600.0, tint: 6.0 };
        a.crop.enabled = true;
        a.crop.top = 0.1;
        crate::develop::history::commit(conn, 1, &a, "Exposure").unwrap();
        a
    }

    #[test]
    fn stub_copies_look_sets_auto_light_keeps_geometry_and_skips_edited() {
        let mut conn = seed();
        let anchor = edit_anchor(&mut conn);
        // Photo 5: the user's edit (skipped).
        let own = ParametricAdjustments { clarity: 40.0, ..Default::default() };
        crate::develop::history::commit(&mut conn, 5, &own, "Clarity").unwrap();

        let settings =
            BaselineSettings { anchor_id: 1, preset_id: None, scope: BaselineScope::All, replace_edited: false };
        let run_id = store::begin_run(&conn, 1, &settings, ENGINE_VERSION).unwrap();
        let job = BaselineJob { run_id, project_id: 1, settings: settings.clone() };
        let never = AtomicBool::new(false);
        let out = run_pipeline(&mut conn, &FakeMeter, &job, &never, &mut |_, _, _| {}).unwrap();
        assert!(!out.cancelled);
        store::finish_run(&conn, run_id, BaselineRunState::Finished, out.message.as_deref()).unwrap();

        let results = store::results(&conn, 1, None).unwrap();
        let outcome = |id: ImageId| results.iter().find(|r| r.image_id == id).unwrap().outcome;
        assert_eq!(outcome(1), BaselineOutcome::Anchor);
        assert_eq!(outcome(2), BaselineOutcome::Applied);
        assert_eq!(outcome(4), BaselineOutcome::Flagged, "auto failed -> anchor light, flagged");
        assert_eq!(outcome(5), BaselineOutcome::SkippedEdited);

        let p2 = crate::db::repo::get_adjustments(&conn, 2).unwrap();
        // Look identical to the anchor's.
        let mut look2 = ParametricAdjustments::default();
        look2.copy_fields(&p2, &fields_of_class(SettingClass::Look));
        let mut look1 = ParametricAdjustments::default();
        look1.copy_fields(&anchor, &fields_of_class(SettingClass::Look));
        assert_eq!(look1, look2);
        // Light = plain Auto (stub), crop not copied.
        assert!((p2.exposure - 0.2).abs() < 1e-6);
        assert_eq!(p2.white_balance, WhiteBalance::Custom { temperature_k: 5000.0, tint: 3.0 });
        assert!(!p2.crop.enabled);
        let p4 = crate::db::repo::get_adjustments(&conn, 4).unwrap();
        assert!((p4.exposure - 0.8).abs() < 1e-6, "anchor light on auto failure");
        assert_eq!(crate::db::repo::get_adjustments(&conn, 5).unwrap(), own);
        assert_eq!(crate::db::repo::get_adjustments(&conn, 1).unwrap(), anchor, "anchor untouched");
        // The anchor's offset was recorded.
        let run = store::get_run(&conn, 1, false).unwrap().unwrap();
        let a = run.anchor.unwrap();
        assert!((a.offset.exposure - (0.8 - 0.1)).abs() < 1e-5);
        assert!(a.offset.temperature_mired < 0.0);
        assert_eq!(run.counts.applied + run.counts.flagged, 3);
    }

    #[test]
    fn cancelled_run_writes_nothing_and_preview_writes_nothing() {
        let mut conn = seed();
        edit_anchor(&mut conn);
        let settings =
            BaselineSettings { anchor_id: 1, preset_id: None, scope: BaselineScope::All, replace_edited: false };
        let run_id = store::begin_run(&conn, 1, &settings, ENGINE_VERSION).unwrap();
        let job = BaselineJob { run_id, project_id: 1, settings: settings.clone() };
        let cancel = AtomicBool::new(true);
        assert!(run_pipeline(&mut conn, &FakeMeter, &job, &cancel, &mut |_, _, _| {}).unwrap().cancelled);
        assert!(crate::db::repo::get_adjustments(&conn, 2).unwrap().is_neutral());

        let prepared = prepare(&conn, 1, &settings).unwrap();
        let pv =
            preview(&FakeMeter, 1, &prepared, &BaselinePreviewOptions { sample_count: 2, image_ids: None }).unwrap();
        assert_eq!(pv.samples.len(), 2);
        assert!(pv.samples.iter().all(|s| s.photo.image_id != 1));
        assert_ne!(pv.samples[0].before, pv.samples[0].after);
        assert!(crate::db::repo::get_adjustments(&conn, 2).unwrap().is_neutral());
        let pinned = BaselinePreviewOptions { sample_count: 12, image_ids: Some(vec![3]) };
        assert_eq!(preview(&FakeMeter, 1, &prepared, &pinned).unwrap().samples[0].photo.image_id, 3);
        let bad = BaselinePreviewOptions { sample_count: 0, image_ids: None };
        assert!(preview(&FakeMeter, 1, &prepared, &bad).is_err());
    }

    #[test]
    fn compose_keeps_the_photos_geometry_and_masks() {
        let mut current = ParametricAdjustments::default();
        current.crop.enabled = true;
        current.crop.left = 0.2;
        current.transform.rotate = 2.0;
        current.clarity = 50.0;
        let mut look = ParametricAdjustments::default();
        look.crop.enabled = true;
        look.crop.top = 0.3;
        look.clarity = 10.0;
        look.exposure = 3.0;
        let light = LightValues { exposure: 0.25, temperature_k: 4800.0, ..Default::default() };
        let out = compose(&current, &look, &light);
        assert_eq!(out.crop, current.crop);
        assert_eq!(out.transform, current.transform);
        assert_eq!(out.clarity, 10.0, "look copied");
        assert_eq!(out.exposure, 0.25, "light from the engine, not the look");
    }

    #[test]
    fn samples_spread_across_scenes() {
        let photo = |id: ImageId, scene: Option<SceneId>, burst: Option<BurstGroupId>| PhotoInput {
            image_id: id,
            src: SourceImage { id, path: PathBuf::new(), orientation: None },
            format: ImageFormat::Arw,
            current: ParametricAdjustments::default(),
            scene_id: scene,
            burst_group_id: burst,
            captured_at_ms: Some(id),
            faces: None,
            state: PhotoEditState::Unedited,
            missing: false,
        };
        let photos: Vec<PhotoInput> = (0..30).map(|i| photo(i, Some(i / 10), Some(i / 2))).collect();
        let s = pick_samples(&photos, 6);
        assert_eq!(s.len(), 6);
        for scene in 0..3 {
            assert_eq!(s.iter().filter(|&&i| photos[i].scene_id == Some(scene)).count(), 2, "{s:?}");
        }
        assert_eq!(pick_samples(&photos, 100).len(), 30);
    }

    /// Every `crs:` key Sieve writes or imports is classified by the partition table the same
    /// way as the group the preset importer assigns it to.
    #[test]
    fn partition_agrees_with_the_crs_mapping() {
        use crate::styles::preset_file::field_of;
        use crate::xmp::crs;
        let mut keys: Vec<String> = Vec::new();
        keys.extend(crs::CRS_FIELDS.iter().map(|(n, _)| (*n).to_owned()));
        for (p, _) in crs::CRS_HSL_PREFIXES {
            for (b, _) in crs::CRS_HSL_BANDS {
                keys.push(format!("{p}{b}"));
            }
        }
        keys.extend(crs::PARITY_SCALARS.iter().map(|f| f.name.to_owned()));
        keys.extend(crs::PARITY_BOOLS.iter().map(|(n, _)| (*n).to_owned()));
        keys.extend(crs::CRS_CURVES.iter().map(|(n, _)| (*n).to_owned()));
        keys.extend(crs::TRANSFORM_SCALARS.iter().map(|(n, ..)| (*n).to_owned()));
        keys.extend([crs::VIGNETTE_STYLE, crs::PERSPECTIVE_UPRIGHT, crs::CAMERA_PROFILE, crs::LOOK].map(str::to_owned));
        for k in &keys {
            let class = crs_key_class(k).unwrap_or_else(|| panic!("{k} not in BASELINE_PARTITION"));
            if let Some(f) = field_of(k) {
                assert_eq!(class, setting_class(f), "{k} ({f:?})");
            }
        }
    }
}
