//! Personal style model ("Auto edit (my style)", IPC v14): the catalog / command side of
//! [`crate::ml::style_model`] (pure learning code). Contract by the architect; bodies:
//! vision-ml-dev.
//!
//! - Training data: every catalog image whose stored settings are a style sample
//!   ([`style_model::is_style_sample`]: something beyond crop / masks differs from the format's
//!   defaults) **and** the user's own ([`trainable`]: edit source `user` or `sidecar`, incl.
//!   edits read from Lightroom sidecars; baseline / auto-style / scene-apply / pasted
//!   settings are machine-made and excluded, v21). Features per image
//!   ([`style_model::FrameContext`]): one neutral render (`ParametricAdjustments::defaults_for`
//!   the format) through `DevelopCache::render_image` at `scene::STATS_MAX_EDGE` measured by
//!   [`RenderFeatures::from_render`], the as-shot white balance of that render and Sieve's
//!   Auto tone of the frame ([`auto_tone_baseline`], the per-frame anchor the tone sliders are
//!   predicted relative to; cached in `style_features` under [`FEATURES_KEY`], stale when the
//!   image's preview was re-extracted or the analysis found faces since); for training
//!   samples also the user's settings rendered (no crop / masks) and measured, the stage-B
//!   output the exposure refinement aims at (cached with a key of those settings),
//!   EXIF from the catalog, the stored preview `scene_features` and the scene's mean
//!   ([`SceneContext`]); the scene id is the cross-validation group.
//! - [`StyleModel::start_training`] runs on a background thread with its own catalog
//!   connection and a private `DevelopCache` (never the editor's), emits `StyleModelProgress`
//!   (throttled, phases features -> fit -> validate) and exactly one `StyleModelFinished`; one
//!   run at a time. Validation: the last [`HOLDOUT_FRAC`] of every camera's edits (capture
//!   time, at most [`HOLDOUT_MAX`]) are held out, a model trained on the rest is rendered
//!   against the user's settings (masks off, the user's crop on every method) with CIEDE2000,
//!   next to "no edit" and Auto tone (the same Auto as the Develop "Auto" button and
//!   `examples/style_eval.rs`: format defaults, as-shot WB, the analysis faces); the stored
//!   model is then fit on every sample.
//! - Parallel LibRaw decodes: every decoding worker here runs OpenMP single-threaded
//!   ([`single_threaded_openmp`]) - concurrent decodes with a full OpenMP team each stall on
//!   `copy_bayer`'s critical section (docs/decisions.md 2026-09-30) - and concurrency is
//!   bounded ([`MAX_DECODE_THREADS`]).
//! - `predict` returns, per image, its current adjustments with [`PREDICTED_FIELDS`] replaced
//!   (crop, masks and other per-frame groups kept) by [`style_model::StyleModel::predict_refined`]
//!   (regression + exposure refinement, renders at [`style_model::REFINE_EDGE`]);
//!   `invalid_argument` when no model is trained ("Train the style model first").
//! - `apply_style_prediction` (command) = `predict` + `develop::batches::commit_recorded`
//!   (label `LABEL_STYLE`, kind `StylePrediction`), so it is undoable as one batch.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rayon::prelude::*;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use crate::db::{self, now_ms, repo};
use crate::develop::{DevelopCache, DevelopConfig, SourceImage};
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::events::{StyleModelFinished, StyleModelProgress};
use crate::ipc::types::*;
use crate::lut::LutLibrary;
use crate::ml::style_model::{
    self, eval, AutoAnchor, FrameContext, RenderFeatures, SceneContext, StyleSample, TrainOptions, MIN_SAMPLES,
};
use crate::scene::{self, SceneFeatures};

/// Current feature + model family (`StyleModelStatus.modelVersion`; rows of another version
/// are ignored: retrain).
pub const MODEL_VERSION: &str = "style-gbt@2";
/// `style_features.version` of the cached per-image features.
pub const FEATURES_KEY: &str = "style-features-v2";
/// Training needs at least this many edited photos.
pub const MIN_EXAMPLES: u32 = MIN_SAMPLES as u32;
/// Groups the model predicts (everything but per-frame geometry and local masks).
pub const PREDICTED_FIELDS: &[AdjustmentField] = AdjustmentField::DEFAULT_SYNC;

/// Share of each camera's edits (latest by capture time) held out for validation.
pub const HOLDOUT_FRAC: f64 = 0.2;
/// At most this many held-out frames are rendered (evenly spaced in capture order).
pub const HOLDOUT_MAX: usize = 40;
/// Fewer held-out frames than this: no validation.
pub const HOLDOUT_MIN: usize = 5;
/// Long edge of the validation renders.
pub const VALIDATION_EDGE: u32 = 512;
/// Decoded-source budget of the training's private develop cache.
pub const TRAIN_CACHE_BYTES: u64 = 768 << 20;
/// Parallel decodes in training / batch prediction.
pub const MAX_DECODE_THREADS: usize = 8;
/// Stored models kept (newest first); older rows are deleted after a training.
const KEEP_MODELS: i64 = 2;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone)]
pub struct StyleModelConfig {
    pub catalog_path: PathBuf,
}

#[derive(Default)]
struct Run {
    training: bool,
    cancelled: bool,
    progress: Option<f32>,
    error: Option<String>,
}

type Loaded = Option<(i64, Arc<style_model::StyleModel>)>;

/// Managed state.
#[derive(Clone)]
pub struct StyleModel {
    config: StyleModelConfig,
    run: Arc<Mutex<Run>>,
    /// Active model (`style_models` row id) as last loaded.
    loaded: Arc<Mutex<Loaded>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------------------
// OpenMP / decode pool.

/// Sets the OpenMP thread count of the *calling* thread's parallel regions to 1 (LibRaw's
/// `copy_bayer` and friends). Resolved at run time (`omp_set_num_threads` of the libomp
/// LibRaw links); a no-op when no OpenMP runtime is loaded. Call on threads that decode
/// concurrently with other threads.
pub fn single_threaded_openmp() {
    type SetNumThreads = unsafe extern "C" fn(i32);
    static F: OnceLock<Option<SetNumThreads>> = OnceLock::new();
    let f = F.get_or_init(|| {
        // SAFETY: dlsym with RTLD_DEFAULT and a NUL-terminated name; a non-null result is
        // libomp's `void omp_set_num_threads(int)`.
        unsafe {
            let p = libc::dlsym(libc::RTLD_DEFAULT, c"omp_set_num_threads".as_ptr());
            (!p.is_null()).then(|| std::mem::transmute::<*mut libc::c_void, SetNumThreads>(p))
        }
    });
    if let Some(f) = f {
        // SAFETY: plain C call with a valid argument.
        unsafe { f(1) }
    }
}

/// Bounded pool whose threads decode with single-threaded OpenMP.
pub fn decode_pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        let n = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, MAX_DECODE_THREADS);
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .thread_name(|i| format!("sieve-style-{i}"))
            .start_handler(|_| single_threaded_openmp())
            .build()
            .expect("style decode pool")
    })
}

// ---------------------------------------------------------------------------------------
// Catalog side.

/// What the model needs about one image, from the catalog.
#[derive(Debug, Clone)]
pub struct CatalogFrame {
    pub src: SourceImage,
    pub format: ImageFormat,
    pub make: String,
    pub model: Option<String>,
    pub capture: CaptureMeta,
    pub scene_id: Option<SceneId>,
    pub adjustments: ParametricAdjustments,
    /// Face boxes of the analysis (`develop::auto::face_boxes`); `None` = not analysed.
    pub faces: Option<Vec<NormRect>>,
}

impl CatalogFrame {
    pub fn load(conn: &Connection, id: ImageId, adjustments: Option<ParametricAdjustments>) -> AppResult<Self> {
        let e = repo::get_image(conn, id)?;
        let adjustments = match adjustments {
            Some(a) => a,
            None => repo::get_adjustments(conn, id)?,
        };
        let faces = crate::develop::auto::analysis_faces(conn, id)?.map(|f| crate::develop::auto::face_boxes(&f));
        Ok(CatalogFrame {
            src: SourceImage { id, path: e.path.into(), orientation: e.orientation },
            format: e.format,
            make: e.camera.make.as_str().to_owned(),
            model: e.camera.model,
            capture: e.capture,
            scene_id: e.scene_id,
            adjustments,
            faces,
        })
    }
}

/// Cached neutral-render features of one image (`style_features.features_json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredFeatures {
    pub render: RenderFeatures,
    pub as_shot: Option<WhiteBalanceValues>,
    /// Sieve's Auto tone of the frame ([`auto_tone_baseline`]).
    #[serde(default)]
    pub auto: Option<AutoAnchor>,
    /// The analysis' faces were known when `auto` was computed (features computed before
    /// the analysis are recomputed once it has run).
    #[serde(default)]
    pub faces_known: bool,
    /// `auto` of an unanalysed frame was computed with on-demand face detection tried
    /// (`develop::auto::resolve_faces`); older entries of unanalysed frames are recomputed.
    #[serde(default)]
    pub faces_on_demand: bool,
    /// The image's own settings (no crop / masks) rendered at `scene::STATS_MAX_EDGE` and
    /// measured (training samples: the stage-B output), with [`edited_key`] of those settings.
    #[serde(default)]
    pub edited: Option<RenderFeatures>,
    #[serde(default)]
    pub edited_key: Option<String>,
}

impl StoredFeatures {
    /// Still valid for `frame` (its faces did not become known since); `training` also
    /// needs the measured render of the frame's current settings.
    pub fn current_for(&self, frame: &CatalogFrame, training: bool) -> bool {
        self.auto.is_some()
            && (self.faces_known || (frame.faces.is_none() && self.faces_on_demand))
            && (!training || (self.edited.is_some() && self.edited_key.as_deref() == Some(&edited_key(frame))))
    }
}

/// Key of the settings behind [`StoredFeatures::edited`]: FNV-1a of the settings without
/// crop / masks.
pub fn edited_key(frame: &CatalogFrame) -> String {
    let json = serde_json::to_string(&style_model::targets::strip_per_image(&frame.adjustments)).unwrap_or_default();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in json.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Cached features of every frame that are still current ([`StoredFeatures::current_for`]).
fn load_current(conn: &Connection, frames: &[CatalogFrame], training: bool) -> AppResult<Vec<Option<StoredFeatures>>> {
    frames.iter().map(|f| Ok(load_features(conn, f.src.id)?.filter(|s| s.current_for(f, training)))).collect()
}

/// Current cached features of `id` (`None` = missing, other version, or the preview was
/// re-extracted since).
pub fn load_features(conn: &Connection, id: ImageId) -> AppResult<Option<StoredFeatures>> {
    let json: Option<String> = conn
        .prepare_cached(
            "SELECT f.features_json FROM style_features f LEFT JOIN thumbnails t ON t.image_id = f.image_id
             WHERE f.image_id = ?1 AND f.version = ?2 AND (t.extracted_at IS NULL OR f.computed_at >= t.extracted_at)",
        )?
        .query_row(params![id, FEATURES_KEY], |r| r.get(0))
        .optional()?;
    Ok(json.and_then(|j| serde_json::from_str(&j).ok()))
}

pub fn save_features(conn: &mut Connection, items: &[(ImageId, StoredFeatures)]) -> AppResult<()> {
    let tx = conn.savepoint()?;
    let now = now_ms();
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO style_features (image_id, version, features_json, computed_at)
             SELECT ?1, ?2, ?3, ?4 WHERE EXISTS (SELECT 1 FROM images WHERE id = ?1)
             ON CONFLICT(image_id) DO UPDATE SET version = excluded.version,
                 features_json = excluded.features_json, computed_at = excluded.computed_at",
        )?;
        for (id, f) in items {
            stmt.execute(params![id, FEATURES_KEY, serde_json::to_string(f)?, now])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Neutral render of `frame` at `scene::STATS_MAX_EDGE`, measured, and the frame's Auto tone;
/// with `edited` also its own settings rendered and measured (blocking; decodes).
pub fn neutral_features(
    cache: &DevelopCache,
    luts: &LutLibrary,
    frame: &CatalogFrame,
    edited: bool,
) -> AppResult<StoredFeatures> {
    let neutral = ParametricAdjustments::defaults_for(frame.format);
    let px = cache.render_image(&frame.src, &neutral, None, scene::STATS_MAX_EDGE, luts)?;
    let auto = auto_tone_baseline(cache, luts, &frame.src, frame.format, frame.faces.as_deref())?;
    let (edited, edited_key) = if edited {
        let own = style_model::targets::strip_per_image(&frame.adjustments);
        let r = cache.render_image(&frame.src, &own, None, scene::STATS_MAX_EDGE, luts)?;
        (Some(RenderFeatures::from_render(&r.image)), Some(edited_key(frame)))
    } else {
        (None, None)
    };
    Ok(StoredFeatures {
        render: RenderFeatures::from_render(&px.image),
        as_shot: px.as_shot,
        auto: Some(AutoAnchor::from_adjustments(&auto)),
        faces_known: frame.faces.is_some(),
        faces_on_demand: frame.faces.is_none(),
        edited,
        edited_key,
    })
}

/// Stage-B measurement of `adjustments` on `frame` ([`style_model::StyleModel::refine`]):
/// uncropped render at [`style_model::REFINE_EDGE`], measured like the Phase 7 statistics.
fn refine_measure(
    cache: &DevelopCache,
    luts: &LutLibrary,
    frame: &CatalogFrame,
    adjustments: &ParametricAdjustments,
) -> AppResult<ImageStats> {
    let mut a = adjustments.clone();
    a.crop = Default::default();
    a.masks.clear();
    let px = cache.render_image(&frame.src, &a, None, style_model::REFINE_EDGE, luts)?;
    let mut st = scene::stats::measure(frame.src.id, &px.image, None);
    st.white_balance = scene::stats::effective_white_balance(&a, px.as_shot);
    st.as_shot = px.as_shot;
    Ok(st)
}

/// [`style_model::StyleModel::predict_refined`] for a catalog frame (blocking; renders).
pub fn predict_refined(
    model: &style_model::StyleModel,
    cache: &DevelopCache,
    luts: &LutLibrary,
    frame: &CatalogFrame,
    ctx: &FrameContext,
) -> AppResult<style_model::StylePrediction> {
    model.predict_refined(ctx, &mut |a| refine_measure(cache, luts, frame, a))
}

fn preview_features(conn: &Connection, id: ImageId) -> AppResult<Option<SceneFeatures>> {
    let json: Option<String> = conn
        .prepare_cached("SELECT features_json FROM scene_features WHERE image_id = ?1 AND version = ?2")?
        .query_row(params![id, scene::FEATURES_VERSION], |r| r.get(0))
        .optional()?;
    Ok(json.and_then(|j| serde_json::from_str(&j).ok()))
}

/// Mean appearance of scene `id` (all members, edited or not).
fn scene_context(conn: &Connection, id: SceneId) -> AppResult<Option<SceneContext>> {
    let mut stmt = conn.prepare_cached(
        "SELECT i.iso, i.shutter_s, i.aperture, f.features_json FROM images i
         LEFT JOIN scene_features f ON f.image_id = i.id AND f.version = ?2
         WHERE i.scene_id = ?1",
    )?;
    let rows = stmt.query_map(params![id, scene::FEATURES_VERSION], |r| {
        Ok((
            r.get::<_, Option<u32>>(0)?,
            r.get::<_, Option<f64>>(1)?,
            r.get::<_, Option<f32>>(2)?,
            r.get::<_, Option<String>>(3)?,
        ))
    })?;
    let mut feats = Vec::new();
    let mut evs = Vec::new();
    for row in rows {
        let (iso, t, n, json) = row?;
        evs.push(style_model::features::ev100(iso, t, n));
        if let Some(f) = json.and_then(|j| serde_json::from_str::<SceneFeatures>(&j).ok()) {
            feats.push(f);
        }
    }
    Ok(SceneContext::from_members(feats.iter(), evs))
}

/// Catalog lookups shared by the frames of one run (scene contexts memoized).
struct ContextBuilder<'c> {
    conn: &'c Connection,
    scenes: HashMap<SceneId, Option<SceneContext>>,
}

impl<'c> ContextBuilder<'c> {
    fn new(conn: &'c Connection) -> Self {
        Self { conn, scenes: HashMap::new() }
    }

    fn context(&mut self, frame: &CatalogFrame, features: &StoredFeatures) -> AppResult<FrameContext> {
        let scene = match frame.scene_id {
            Some(s) => match self.scenes.get(&s) {
                Some(c) => c.clone(),
                None => {
                    let c = scene_context(self.conn, s)?;
                    self.scenes.insert(s, c.clone());
                    c
                }
            },
            None => None,
        };
        Ok(FrameContext {
            format: frame.format,
            make: Some(frame.make.clone()),
            model: frame.model.clone(),
            iso: frame.capture.iso,
            shutter_seconds: frame.capture.shutter_seconds,
            aperture: frame.capture.aperture,
            focal_length_mm: frame.capture.focal_length_mm,
            as_shot: features.as_shot,
            render: features.render.clone(),
            preview: preview_features(self.conn, frame.src.id)?,
            scene,
            auto: features.auto,
            captured_at_ms: frame.capture.captured_at_ms,
        })
    }
}

/// The ids of `ids` (same order) whose current settings are the user's own: edit source `user`
/// (sliders, presets, Auto in Develop) or `sidecar` (read from XMP, e.g. edited in Lightroom).
/// Machine-made settings (baseline runs, "Auto edit (my style)", Apply to Scene, paste / sync)
/// never become training examples (v21).
pub fn trainable(conn: &Connection, ids: &[ImageId]) -> AppResult<Vec<ImageId>> {
    let states = crate::develop::batches::edit_states(conn, ids)?;
    Ok(states
        .into_iter()
        .filter(|s| matches!(s.edit_source, EditSource::User | EditSource::Sidecar))
        .map(|s| s.image_id)
        .collect())
}

/// Every style sample in the catalog (see the module docs), capture order.
pub fn style_sample_frames(conn: &Connection) -> AppResult<Vec<CatalogFrame>> {
    let ids: Vec<ImageId> = conn
        .prepare_cached(
            "SELECT a.image_id FROM adjustments a JOIN images i ON i.id = a.image_id WHERE a.neutral = 0
             ORDER BY i.captured_at_ms IS NULL, i.captured_at_ms, i.file_name, i.id",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let ids = trainable(conn, &ids)?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let f = CatalogFrame::load(conn, id, None)?;
        if style_model::is_style_sample(&f.adjustments, f.format) {
            out.push(f);
        }
    }
    Ok(out)
}

/// Number of style samples in the catalog (`StyleModelStatus.availableExamples`).
pub fn count_style_samples(conn: &Connection) -> AppResult<u32> {
    let mut stmt = conn.prepare_cached(
        "SELECT i.format, a.image_id FROM adjustments a JOIN images i ON i.id = a.image_id WHERE a.neutral = 0",
    )?;
    let rows: Vec<(String, ImageId)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    let keep: std::collections::HashSet<ImageId> =
        trainable(conn, &rows.iter().map(|(_, id)| *id).collect::<Vec<_>>())?.into_iter().collect();
    let mut n = 0;
    for (format, id) in rows.into_iter().filter(|(_, id)| keep.contains(id)) {
        let Some(format) = ImageFormat::parse(&format) else { continue };
        if style_model::is_style_sample(&repo::get_adjustments(conn, id)?, format) {
            n += 1;
        }
    }
    Ok(n)
}

/// Newest stored model of the current version: `(row id, trained_at, examples, validation)`.
type ModelRow = (i64, i64, u32, Option<String>);

fn latest_row(conn: &Connection) -> AppResult<Option<ModelRow>> {
    Ok(conn
        .query_row(
            "SELECT id, trained_at, examples, validation_json FROM style_models WHERE version = ?1
             ORDER BY id DESC LIMIT 1",
            [MODEL_VERSION],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?)
}

// ---------------------------------------------------------------------------------------
// Training.

/// Progress / cancellation of a training run.
pub trait TrainControl: Sync {
    fn progress(&self, phase: StyleTrainPhase, done: u32, total: u32);
    fn cancelled(&self) -> bool;
}

/// Result of [`train_catalog`].
#[derive(Debug, Clone)]
pub struct TrainSummary {
    pub model_row_id: i64,
    pub samples: u32,
    /// Edited images that could not be measured (e.g. missing originals).
    pub skipped: u32,
    pub features_computed: u32,
    pub validation: Option<StyleValidation>,
    pub features_ms: f64,
    pub fit_ms: f64,
    pub validate_ms: f64,
}

/// A sample with the catalog facts validation needs.
struct Prepared {
    frame: CatalogFrame,
    sample: StyleSample,
}

/// The whole training on the catalog at `catalog_path` (blocking): features -> fit ->
/// validate -> store. `Ok(None)` when cancelled (nothing stored). `cache` should be a private
/// cache (decodes every edited frame once).
pub fn train_catalog(
    catalog_path: &std::path::Path,
    cache: &DevelopCache,
    luts: &LutLibrary,
    control: &dyn TrainControl,
) -> AppResult<Option<TrainSummary>> {
    let mut conn = db::open(catalog_path)?;
    let frames = style_sample_frames(&conn)?;
    if frames.len() < MIN_SAMPLES {
        return Err(AppError::invalid(format!(
            "Edit at least {MIN_SAMPLES} photos first ({} edited so far).",
            frames.len()
        )));
    }

    // 1. Features (cached neutral-render statistics; missing ones rendered in parallel).
    let t = Instant::now();
    let mut stored = load_current(&conn, &frames, true)?;
    let missing: Vec<usize> = (0..frames.len()).filter(|&i| stored[i].is_none()).collect();
    let total = frames.len() as u32;
    let cached = total - missing.len() as u32;
    control.progress(StyleTrainPhase::Features, cached, total);
    let done = AtomicU32::new(cached);
    let computed: Vec<(usize, AppResult<StoredFeatures>)> = decode_pool().install(|| {
        missing
            .par_iter()
            .map(|&i| {
                if control.cancelled() {
                    return (i, Err(AppError::invalid("cancelled")));
                }
                let r = catch_unwind(AssertUnwindSafe(|| neutral_features(cache, luts, &frames[i], true)))
                    .unwrap_or_else(|_| Err(AppError::internal("feature render panicked")));
                let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                control.progress(StyleTrainPhase::Features, d, total);
                (i, r)
            })
            .collect()
    });
    if control.cancelled() {
        return Ok(None);
    }
    let mut to_save = Vec::new();
    let mut skipped = 0u32;
    for (i, r) in computed {
        match r {
            Ok(f) => {
                to_save.push((frames[i].src.id, f.clone()));
                stored[i] = Some(f);
            }
            Err(e) => {
                skipped += 1;
                eprintln!("[style] {}: {}", frames[i].src.path.display(), e.message);
            }
        }
    }
    save_features(&mut conn, &to_save)?;
    let features_ms = t.elapsed().as_secs_f64() * 1000.0;

    let mut builder = ContextBuilder::new(&conn);
    let mut prepared = Vec::with_capacity(frames.len());
    for (frame, feats) in frames.into_iter().zip(stored) {
        let Some(feats) = feats else { continue };
        let context = builder.context(&frame, &feats)?;
        let sample = StyleSample {
            context,
            adjustments: frame.adjustments.clone(),
            group: frame.scene_id,
            captured_at_ms: frame.capture.captured_at_ms,
            edited: feats.edited.clone(),
        };
        prepared.push(Prepared { frame, sample });
    }
    drop(builder);
    if prepared.len() < MIN_SAMPLES {
        return Err(AppError::invalid(format!(
            "Only {} of your edited photos could be read ({skipped} missing or unreadable); at least {MIN_SAMPLES} are needed.",
            prepared.len()
        )));
    }

    // 2. Fit on every sample.
    let t = Instant::now();
    let samples: Vec<StyleSample> = prepared.iter().map(|p| p.sample.clone()).collect();
    control.progress(StyleTrainPhase::Fit, 0, 100);
    let (model, _report) = style_model::train(&samples, &TrainOptions::default(), &|p| {
        control.progress(StyleTrainPhase::Fit, (p * 100.0).round() as u32, 100)
    })
    .map_err(|e| AppError::invalid(e.to_string()))?;
    let fit_ms = t.elapsed().as_secs_f64() * 1000.0;
    if control.cancelled() {
        return Ok(None);
    }

    // 3. Validate on held-out edits (a model trained without them).
    let t = Instant::now();
    let validation = validate(&prepared, cache, luts, control)?;
    if control.cancelled() {
        return Ok(None);
    }
    let validate_ms = t.elapsed().as_secs_f64() * 1000.0;

    // 4. Store.
    let validation_json = validation.map(|v| serde_json::to_string(&v)).transpose()?;
    let tx = conn.savepoint()?;
    tx.execute(
        "INSERT INTO style_models (version, trained_at, examples, model_blob, validation_json)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![MODEL_VERSION, model.trained_at_ms, model.sample_count, model.to_json().into_bytes(), validation_json],
    )?;
    let id = tx.last_insert_rowid();
    tx.execute(
        "DELETE FROM style_models WHERE id NOT IN (SELECT id FROM style_models ORDER BY id DESC LIMIT ?1)",
        [KEEP_MODELS],
    )?;
    tx.commit()?;
    Ok(Some(TrainSummary {
        model_row_id: id,
        samples: model.sample_count,
        skipped,
        features_computed: to_save.len() as u32,
        validation,
        features_ms,
        fit_ms,
        validate_ms,
    }))
}

/// Held-out split: the last [`HOLDOUT_FRAC`] (capture time) of each camera's samples, thinned
/// evenly to [`HOLDOUT_MAX`]. Returns `is_held_out` per sample, or `None` when either side
/// would be too small.
pub fn holdout_split(samples: &[StyleSample]) -> Option<Vec<bool>> {
    let mut per: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, s) in samples.iter().enumerate() {
        per.entry(s.context.camera_key()).or_default().push(i);
    }
    let mut held: Vec<usize> = Vec::new();
    for idx in per.values_mut() {
        idx.sort_by_key(|&i| (samples[i].captured_at_ms.unwrap_or(i64::MAX), i));
        let n = ((idx.len() as f64) * HOLDOUT_FRAC).round() as usize;
        held.extend(&idx[idx.len() - n.min(idx.len())..]);
    }
    held.sort_by_key(|&i| (samples[i].captured_at_ms.unwrap_or(i64::MAX), i));
    if held.len() > HOLDOUT_MAX {
        let n = held.len();
        held = (0..HOLDOUT_MAX).map(|k| held[k * n / HOLDOUT_MAX]).collect();
    }
    let train = samples.len() - held.len();
    if held.len() < HOLDOUT_MIN || train < MIN_SAMPLES {
        return None;
    }
    let mut out = vec![false; samples.len()];
    held.iter().for_each(|&i| out[i] = true);
    Some(out)
}

/// Sieve's Auto tone of a frame, as the Develop "Auto" button computes it on an unedited
/// photo: `develop::auto::auto_tone_with_faces` on the format defaults (as-shot WB) with the
/// analysis faces (`None` = not analysed: detected on demand, `develop::auto::resolve_faces`;
/// skin-coloured pixels stand in if the detector is unavailable); the reference auto
/// of [`eval::reference_auto_tone`] if it fails. The style model's per-frame anchor and the
/// "Auto tone" column of the validation (same as `examples/style_eval.rs`).
pub fn auto_tone_baseline(
    cache: &DevelopCache,
    luts: &LutLibrary,
    src: &SourceImage,
    format: ImageFormat,
    faces: Option<&[NormRect]>,
) -> AppResult<ParametricAdjustments> {
    let defaults = ParametricAdjustments::defaults_for(format);
    let faces = crate::develop::auto::resolve_faces(cache, src, faces.map(<[NormRect]>::to_vec));
    match crate::develop::auto::auto_tone_with_faces(
        cache,
        src,
        &defaults,
        AdjustmentField::AUTO_TONE,
        faces.as_deref(),
    ) {
        Ok(v) => Ok(v.apply_to(&defaults)),
        Err(_) => eval::reference_auto_tone(format, |a| {
            cache
                .render_image(src, a, None, eval::AUTO_MEASURE_EDGE, luts)
                .map(|p| RenderFeatures::from_render(&p.image))
        }),
    }
}

/// Mean ΔE2000 vs the user's render at `edge` long-edge pixels (validation:
/// [`VALIDATION_EDGE`]) of `[prediction, auto tone, no edit]` for one frame (masks off; the
/// user's crop applied to every method).
pub fn validation_errors(
    cache: &DevelopCache,
    luts: &LutLibrary,
    frame: &CatalogFrame,
    user: &ParametricAdjustments,
    predicted: &ParametricAdjustments,
    edge: u32,
) -> AppResult<[f64; 3]> {
    let mut reference = user.clone();
    reference.masks.clear();
    let with_crop = |a: &ParametricAdjustments| {
        let mut a = a.clone();
        a.crop = reference.crop;
        a.masks.clear();
        a
    };
    let auto = auto_tone_baseline(cache, luts, &frame.src, frame.format, frame.faces.as_deref())?;
    let render = |a: &ParametricAdjustments| cache.render_image(&frame.src, a, None, edge, luts).map(|p| p.image);
    let r = render(&reference)?;
    let mut out = [0.0; 3];
    for (k, a) in [predicted, &auto, &ParametricAdjustments::defaults_for(frame.format)].into_iter().enumerate() {
        out[k] = eval::mean_de(&render(&with_crop(a))?, &r)
            .ok_or_else(|| AppError::internal("validation renders differ in size"))?;
    }
    Ok(out)
}

fn validate(
    prepared: &[Prepared],
    cache: &DevelopCache,
    luts: &LutLibrary,
    control: &dyn TrainControl,
) -> AppResult<Option<StyleValidation>> {
    let samples: Vec<StyleSample> = prepared.iter().map(|p| p.sample.clone()).collect();
    let Some(held) = holdout_split(&samples) else { return Ok(None) };
    let train: Vec<StyleSample> = samples.iter().zip(&held).filter(|(_, h)| !**h).map(|(s, _)| s.clone()).collect();
    let test: Vec<&Prepared> = prepared.iter().zip(&held).filter(|(_, h)| **h).map(|(p, _)| p).collect();
    let total = test.len() as u32;
    control.progress(StyleTrainPhase::Validate, 0, total);
    let (model, _) =
        style_model::train(&train, &TrainOptions::default(), &|_| {}).map_err(|e| AppError::invalid(e.to_string()))?;
    let done = AtomicU32::new(0);
    let errors: Vec<Option<[f64; 3]>> = decode_pool().install(|| {
        test.par_iter()
            .map(|p| {
                if control.cancelled() {
                    return None;
                }
                let r = catch_unwind(AssertUnwindSafe(|| {
                    let predicted = predict_refined(&model, cache, luts, &p.frame, &p.sample.context)?.adjustments;
                    validation_errors(cache, luts, &p.frame, &p.frame.adjustments, &predicted, VALIDATION_EDGE)
                }));
                let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                control.progress(StyleTrainPhase::Validate, d, total);
                match r {
                    Ok(Ok(e)) => Some(e),
                    Ok(Err(e)) => {
                        eprintln!("[style] validation {}: {}", p.frame.src.path.display(), e.message);
                        None
                    }
                    Err(_) => None,
                }
            })
            .collect()
    });
    let ok: Vec<[f64; 3]> = errors.into_iter().flatten().collect();
    if ok.len() < HOLDOUT_MIN {
        return Ok(None);
    }
    let mean = |k: usize| (ok.iter().map(|e| e[k]).sum::<f64>() / ok.len() as f64) as f32;
    Ok(Some(StyleValidation {
        held_out_images: ok.len() as u32,
        delta_e: mean(0),
        auto_tone_delta_e: mean(1),
        no_edit_delta_e: mean(2),
    }))
}

// ---------------------------------------------------------------------------------------
// Prediction.

/// Predictions for `frames` with `model` (blocking; renders missing features in parallel with
/// `cache` and stores them).
pub fn predict_frames(
    conn: &mut Connection,
    model: &style_model::StyleModel,
    cache: &DevelopCache,
    luts: &LutLibrary,
    frames: &[CatalogFrame],
) -> AppResult<Vec<StylePrediction>> {
    let mut stored = load_current(conn, frames, false)?;
    let missing: Vec<usize> = (0..frames.len()).filter(|&i| stored[i].is_none()).collect();
    let computed: Vec<(usize, AppResult<StoredFeatures>)> = if missing.len() > 1 {
        decode_pool()
            .install(|| missing.par_iter().map(|&i| (i, neutral_features(cache, luts, &frames[i], false))).collect())
    } else {
        missing.iter().map(|&i| (i, neutral_features(cache, luts, &frames[i], false))).collect()
    };
    let mut to_save = Vec::new();
    for (i, r) in computed {
        let f = r?;
        to_save.push((frames[i].src.id, f.clone()));
        stored[i] = Some(f);
    }
    save_features(conn, &to_save)?;
    let mut builder = ContextBuilder::new(conn);
    let mut contexts = Vec::with_capacity(frames.len());
    for (frame, feats) in frames.iter().zip(stored) {
        let feats = feats.ok_or_else(|| AppError::internal("style features missing"))?;
        contexts.push(builder.context(frame, &feats)?);
    }
    drop(builder);
    let one = |(frame, ctx): (&CatalogFrame, &FrameContext)| predict_refined(model, cache, luts, frame, ctx);
    let predicted: Vec<AppResult<style_model::StylePrediction>> = if frames.len() > 1 {
        decode_pool().install(|| frames.par_iter().zip(contexts.par_iter()).map(one).collect())
    } else {
        frames.iter().zip(&contexts).map(one).collect()
    };
    let mut out = Vec::with_capacity(frames.len());
    for ((frame, ctx), p) in frames.iter().zip(&contexts).zip(predicted) {
        let p = p?;
        let ctx = ctx.clone();
        let mut adjustments = frame.adjustments.clone();
        adjustments.copy_fields(&p.adjustments, PREDICTED_FIELDS);
        let confidence = model.confidence(&ctx);
        let mut notes = Vec::new();
        if !p.known_camera {
            notes.push("No edits from this camera yet: your overall style was used.".to_owned());
        }
        if ctx.scene.is_none() {
            notes.push("Scenes not detected for this photo: predicted without scene context.".to_owned());
        }
        if confidence < 0.4 {
            notes.push("This photo is unlike the ones you edited: check the result.".to_owned());
        }
        out.push(StylePrediction {
            image_id: frame.src.id,
            adjustments,
            fields: PREDICTED_FIELDS.to_vec(),
            confidence,
            notes,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------
// Managed state.

struct AppControl<'a> {
    state: &'a StyleModel,
    app: &'a AppHandle,
    last: Mutex<(Option<Instant>, Option<StyleTrainPhase>)>,
}

impl TrainControl for AppControl<'_> {
    fn progress(&self, phase: StyleTrainPhase, done: u32, total: u32) {
        let overall = match phase {
            StyleTrainPhase::Features => 0.0,
            StyleTrainPhase::Fit => 0.6,
            StyleTrainPhase::Validate => 0.75,
        } + done as f32 / total.max(1) as f32
            * match phase {
                StyleTrainPhase::Features => 0.6,
                StyleTrainPhase::Fit => 0.15,
                StyleTrainPhase::Validate => 0.25,
            };
        lock(&self.state.run).progress = Some(overall.clamp(0.0, 1.0));
        let mut last = lock(&self.last);
        let due = last.1 != Some(phase) || done >= total || last.0.is_none_or(|t| t.elapsed() >= PROGRESS_INTERVAL);
        if due {
            *last = (Some(Instant::now()), Some(phase));
            drop(last);
            let _ = StyleModelProgress { phase, done, total }.emit(self.app);
        }
    }

    fn cancelled(&self) -> bool {
        lock(&self.state.run).cancelled
    }
}

impl StyleModel {
    pub fn new(config: StyleModelConfig) -> Self {
        Self { config, run: Arc::new(Mutex::new(Run::default())), loaded: Arc::new(Mutex::new(None)) }
    }

    pub fn config(&self) -> &StyleModelConfig {
        &self.config
    }

    /// Edited photos in the catalog (training candidates).
    pub fn available_examples(conn: &Connection) -> AppResult<u32> {
        count_style_samples(conn)
    }

    /// `style_model_status()`.
    pub fn status(&self, conn: &Connection) -> AppResult<StyleModelStatus> {
        let (training, progress, error) = {
            let run = lock(&self.run);
            (run.training, run.progress, run.error.clone())
        };
        let latest = latest_row(conn)?;
        let state = if training {
            StyleModelState::Training
        } else if error.is_some() {
            StyleModelState::Failed
        } else if latest.is_some() {
            StyleModelState::Ready
        } else {
            StyleModelState::Untrained
        };
        Ok(StyleModelStatus {
            state,
            model_version: MODEL_VERSION.into(),
            trained_at_ms: latest.as_ref().map(|l| l.1),
            training_examples: latest.as_ref().map_or(0, |l| l.2),
            available_examples: Self::available_examples(conn)?,
            min_examples: MIN_EXAMPLES,
            progress: if training { progress } else { None },
            error,
            validation: latest.and_then(|l| l.3).and_then(|j| serde_json::from_str(&j).ok()),
        })
    }

    /// Starts training in the background (see the module docs).
    pub fn start_training(&self, app: &AppHandle) -> AppResult<()> {
        {
            let mut run = lock(&self.run);
            if run.training {
                return Err(AppError::invalid("The style model is already training."));
            }
            *run = Run { training: true, cancelled: false, progress: Some(0.0), error: None };
        }
        let develop_config = app.state::<DevelopCache>().config().clone();
        let auto_faces = app.state::<DevelopCache>().auto_faces().cloned();
        let luts = app.state::<LutLibrary>().inner().clone();
        let (this, app) = (self.clone(), app.clone());
        let spawned = std::thread::Builder::new().name("style-train".into()).spawn(move || {
            this.run_training(&app, develop_config, auto_faces, &luts);
        });
        if let Err(e) = spawned {
            *lock(&self.run) = Run::default();
            return Err(AppError::internal(format!("could not start training: {e}")));
        }
        Ok(())
    }

    /// Private develop cache for training (never evicts the editor's working set).
    pub fn training_cache(develop_config: &DevelopConfig) -> DevelopCache {
        DevelopCache::new(DevelopConfig {
            cache_bytes: TRAIN_CACHE_BYTES.min(develop_config.cache_bytes.max(256 << 20)),
            mask_cache: None,
        })
    }

    fn run_training(
        &self,
        app: &AppHandle,
        develop_config: DevelopConfig,
        auto_faces: Option<super::auto_faces::AutoFaces>,
        luts: &LutLibrary,
    ) {
        single_threaded_openmp();
        let mut cache = Self::training_cache(&develop_config);
        // The editor's on-demand face detector (one session + cache shared with Auto tone).
        if let Some(f) = auto_faces {
            cache = cache.with_auto_faces(f);
        }
        let control = AppControl { state: self, app, last: Mutex::new((None, None)) };
        let result =
            catch_unwind(AssertUnwindSafe(|| train_catalog(&self.config.catalog_path, &cache, luts, &control)))
                .unwrap_or_else(|_| Err(AppError::internal("style training panicked")));
        let (ok, cancelled, error) = match result {
            Ok(Some(s)) => {
                eprintln!(
                    "[style] trained on {} photos ({} skipped): features {:.1}s ({} rendered), fit {:.1}s, validate {:.1}s; {:?}",
                    s.samples,
                    s.skipped,
                    s.features_ms / 1000.0,
                    s.features_computed,
                    s.fit_ms / 1000.0,
                    s.validate_ms / 1000.0,
                    s.validation
                );
                (true, false, None)
            }
            Ok(None) => (false, true, None),
            Err(e) => (false, false, Some(e.message)),
        };
        {
            let mut run = lock(&self.run);
            *run = Run { training: false, cancelled: false, progress: None, error: error.clone() };
        }
        let status =
            db::open(&self.config.catalog_path).and_then(|c| self.status(&c)).unwrap_or_else(|e| StyleModelStatus {
                state: StyleModelState::Failed,
                model_version: MODEL_VERSION.into(),
                trained_at_ms: None,
                training_examples: 0,
                available_examples: 0,
                min_examples: MIN_EXAMPLES,
                progress: None,
                error: Some(e.message),
                validation: None,
            });
        let _ = StyleModelFinished { ok, cancelled, error, status }.emit(app);
    }

    /// Stops a running training (no-op when idle).
    pub fn cancel(&self) {
        let mut run = lock(&self.run);
        if run.training {
            run.cancelled = true;
        }
    }

    /// The active model (`None` = never trained, or only by another model version).
    pub fn active_model(&self, conn: &Connection) -> AppResult<Option<Arc<style_model::StyleModel>>> {
        let Some((id, ..)) = latest_row(conn)? else { return Ok(None) };
        if let Some((loaded_id, m)) = lock(&self.loaded).as_ref() {
            if *loaded_id == id {
                return Ok(Some(m.clone()));
            }
        }
        let blob: Vec<u8> = conn.query_row("SELECT model_blob FROM style_models WHERE id = ?1", [id], |r| r.get(0))?;
        let text = String::from_utf8(blob).map_err(|e| AppError::internal(format!("style model: {e}")))?;
        let model = match style_model::StyleModel::from_json(&text) {
            Ok(m) => Arc::new(m),
            Err(style_model::StyleError::Outdated) => return Ok(None),
            Err(e) => return Err(AppError::internal(e.to_string())),
        };
        *lock(&self.loaded) = Some((id, model.clone()));
        Ok(Some(model))
    }

    /// Predictions for `inputs` (image + current adjustments), blocking.
    pub fn predict(
        &self,
        cache: &DevelopCache,
        luts: &LutLibrary,
        inputs: &[crate::scene::MatchImage],
    ) -> AppResult<Vec<StylePrediction>> {
        let mut conn = db::open(&self.config.catalog_path)?;
        let model = self
            .active_model(&conn)?
            .ok_or_else(|| AppError::invalid("Train the style model first (Auto edit needs your edited photos)."))?;
        let frames: Vec<CatalogFrame> = inputs
            .iter()
            .map(|m| CatalogFrame::load(&conn, m.src.id, Some(m.adjustments.clone())))
            .collect::<AppResult<_>>()?;
        predict_frames(&mut conn, &model, cache, luts, &frames)
    }
}

#[cfg(test)]
mod tests;
