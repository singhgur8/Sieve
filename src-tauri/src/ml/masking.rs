//! AI masks (Phase 7c, IPC v10): the seam between the masking contract and the segmentation
//! models. Owned by vision-ml-dev; the architect fixed the surface below. Model selection,
//! pre/post-processing and the concrete [`SegmentModel`] implementations
//! (`ml/segment_models.rs` over the engine in `ml/segment.rs`) are the owner's.
//!
//! Contract the implementation must honour:
//! - Output mattes are **sensor-frame** [`AlphaMask`]s (un-oriented, uncropped; see
//!   `ipc::masks`), 8-bit, soft edges (Lightroom mattes are soft 8-bit too). They may be
//!   cropped to the selection's bounding box (`AlphaMask.bounds`).
//! - `compute` is blocking (called on the blocking pool by `compute_ai_mask` and by export for
//!   masks without a matte); concurrent requests for the same (image, kind) are coalesced;
//!   `is_computing` reports them. Results go to `develop::masks::MaskCache::put` with origin
//!   `sieve`, `model_version` = [`SegmentModel::id`], `input_digest` = [`source_fingerprint`].
//! - Families (`AiTargetKind`): subject, sky, background (= 1 - subject matte), people
//!   (instances by reference point; parts via a face/human parsing model), object (box
//!   prompt), landscape. Unavailable families are reported by [`Segmenter::capabilities`]
//!   with a reason; never fail app start because a model is missing.
//! - Models are permissively licensed (record licence + source in `docs/decisions.md`),
//!   loaded lazily from `<models_dir>` on first use, CoreML EP where possible.
//!
//! # Implementation (vision-ml-dev)
//! - **Input**: the *develop source* (the same half-size LibRaw / raster decode the editor
//!   renders), rendered with neutral adjustments at [`SOURCE_EDGE`] px on the long edge
//!   **without orientation** (`develop::source::prepare(.., orientation 1, ..)`), so mattes
//!   are in the sensor frame by construction and align with every render. If the source
//!   cannot be decoded, the catalog's 2048 px preview (oriented) is un-oriented instead. The
//!   last [`SOURCE_CACHE`] decoded inputs are kept, so subject + sky + people on one image
//!   decode once.
//! - **Stored unit**: the model's *unrefined* low-res output plus its region (1024x1024 for
//!   subject/background, 320x320 for sky, the person / object ROI at <= 1024-1536 px for
//!   people and objects; facial features at input resolution). Edge refinement happens at
//!   evaluation against the render (`ml::refine::refine`, guided filter; parameters from
//!   `RefineParams::for_kind(cache_kind)`), so one stored matte serves previews, 1:1 zoom
//!   and full-resolution export. This is below the ~1500 px hint for subject/sky on purpose:
//!   the edges come from the guide at render resolution, not from the stored bitmap.
//! - **Cache**: `mask_cache` rows of (image, `AiMask::cache_kind`, model id) whose
//!   `input_digest` equals the source's current [`source_fingerprint`]; `force` recomputes.
//!   In memory the engine also keeps the last image's subject output (background reuses
//!   it), SAM embedding (people + objects) and person instances / parts, so the People
//!   picker (`detect_people`) followed by `compute_ai_mask(people)` runs detection once.
//! - **Concurrency**: one computation per (image, kind); later identical requests wait for
//!   the first and share its result. Model sessions sit behind one engine lock (each model
//!   uses all cores; running two at once would not be faster).
//! - Progress / cancel: the v10 contract defines neither for `compute_ai_mask` (it resolves
//!   when done; `list_masks` reports `computing`). Nothing is emitted.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Instant;

use rusqlite::{params, Connection, OptionalExtension};

use crate::develop::masks::{AlphaMask, MaskCache, NewMatte};
use crate::develop::SourceImage;
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::{
    orient_point, AiCapability, AiMask, AiMaskInfo, AiMaskOrigin, AiMaskRequest, AiTarget, AiTargetKind,
    DetectedPerson, ImageId, MaskCapabilities, NormPoint, NormRect, ParametricAdjustments, PersonPart,
};

use super::refine::{oriented_index, unorient_pixels};

/// Long edge of the image the models see (sensor frame).
pub const SOURCE_EDGE: u32 = 2048;
/// Decoded model inputs kept in memory (most recent images).
pub const SOURCE_CACHE: usize = 2;

/// Image handed to a model: interleaved sRGB8, any orientation the model owner chooses
/// (`orientation` says which; 1 = sensor frame).
pub struct SegmentInput<'a> {
    pub width: u32,
    pub height: u32,
    pub rgb: &'a [u8],
    /// EXIF orientation of `rgb` relative to the sensor frame (1..=8).
    pub orientation: u8,
}

/// What to segment.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentRequest {
    pub target: AiTarget,
    /// Sensor frame (instance selection for people / subject).
    pub reference_point: Option<NormPoint>,
}

/// One segmentation model (or pipeline of models) for one or more families.
pub trait SegmentModel: Send + Sync {
    /// Stable id incl. version, stored as `mask_cache.model_version` (e.g.
    /// `"birefnet-lite-1024@1"`). Changing it invalidates cached Sieve mattes of its kinds.
    fn id(&self) -> &str;
    /// Families this model serves.
    fn families(&self) -> &[AiTargetKind];
    /// Runs the model; the matte is in the *input's* frame (`bounds` normalized to the input),
    /// the caller converts it to the sensor frame.
    fn run(&self, request: &SegmentRequest, input: &SegmentInput) -> Result<AlphaMask, String>;
}

/// A person for the People picker, in the *input's* frame (normalized).
#[derive(Debug, Clone, PartialEq)]
pub struct PersonSummary {
    pub bbox: NormRect,
    pub face: Option<NormRect>,
    /// A point on this person that selects it again in [`SegmentModel::run`].
    pub reference_point: NormPoint,
    /// Facial-feature parts (eyes, brows, lips, teeth) are available for this person (a face
    /// large enough for landmarks). Not in the v10 `DetectedPerson`; see the module docs.
    pub features_available: bool,
}

/// People detection for the picker (the people model implements it).
pub trait PeopleDetector: Send + Sync {
    fn people(&self, input: &SegmentInput) -> Result<Vec<PersonSummary>, String>;
}

/// A model plus the files it needs (availability) and, for people, the files each part needs.
#[derive(Clone)]
pub struct RegisteredModel {
    pub model: Arc<dyn SegmentModel>,
    pub required: Vec<PathBuf>,
    pub parts: Vec<(PersonPart, Vec<PathBuf>)>,
    pub people: Option<Arc<dyn PeopleDetector>>,
}

/// Cached-matte lookup + insert (the part of `MaskCache` the segmenter uses; an adapter so
/// the seam is testable without the catalog and independent of the cache internals).
pub trait MatteStore: Send + Sync {
    /// A `sieve` matte of (image, kind, model) computed from `input_digest`, if cached.
    fn find(
        &self,
        image_id: ImageId,
        kind: &str,
        model_version: &str,
        input_digest: &str,
    ) -> AppResult<Option<CachedMatte>>;
    fn put(&self, image_id: ImageId, matte: &NewMatte, mask: &AlphaMask) -> AppResult<AiMaskInfo>;
}

/// A `mask_cache` row (the columns `AiMaskInfo` needs besides target / reference point).
#[derive(Debug, Clone, PartialEq)]
pub struct CachedMatte {
    pub digest: String,
    pub width: u32,
    pub height: u32,
    pub bounds: NormRect,
    pub coverage: f32,
}

/// [`MatteStore`] over the catalog: reads `mask_cache` directly, writes via
/// [`MaskCache::put`] (PNG + row).
pub struct CatalogMatteStore {
    pub cache: MaskCache,
    pub catalog_path: PathBuf,
}

impl CatalogMatteStore {
    fn conn(&self) -> AppResult<Connection> {
        let conn = Connection::open(&self.catalog_path)?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Ok(conn)
    }
}

impl MatteStore for CatalogMatteStore {
    fn find(
        &self,
        image_id: ImageId,
        kind: &str,
        model_version: &str,
        input_digest: &str,
    ) -> AppResult<Option<CachedMatte>> {
        let conn = self.conn()?;
        let row = conn
            .query_row(
                "SELECT digest, width, height, bounds_x, bounds_y, bounds_w, bounds_h, coverage, path
                   FROM mask_cache
                  WHERE image_id = ?1 AND kind = ?2 AND origin = 'sieve' AND model_version = ?3
                    AND input_digest = ?4
                  ORDER BY created_at DESC LIMIT 1",
                params![image_id, kind, model_version, input_digest],
                |r| {
                    Ok((
                        CachedMatte {
                            digest: r.get(0)?,
                            width: r.get(1)?,
                            height: r.get(2)?,
                            bounds: NormRect { x: r.get(3)?, y: r.get(4)?, width: r.get(5)?, height: r.get(6)? },
                            coverage: r.get(7)?,
                        },
                        r.get::<_, String>(8)?,
                    ))
                },
            )
            .optional()?;
        // A row whose PNG is gone (cache dir cleared) is a miss.
        Ok(row.and_then(|(m, path)| {
            let p = Path::new(&path);
            let abs = if p.is_absolute() { p.to_path_buf() } else { self.cache.config().cache_dir.join(p) };
            abs.is_file().then_some(m)
        }))
    }

    fn put(&self, image_id: ImageId, matte: &NewMatte, mask: &AlphaMask) -> AppResult<AiMaskInfo> {
        let conn = self.conn()?;
        self.cache.put(&conn, image_id, matte, mask)
    }
}

#[derive(Debug, Clone)]
pub struct SegmenterConfig {
    /// Same directory as the culling models (`SIEVE_MODELS`).
    pub models_dir: PathBuf,
    pub catalog_path: PathBuf,
}

/// Decoded model input (sensor frame, RGB8).
#[derive(Debug, Clone, PartialEq)]
pub struct SourcePixels {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// Produces the model input for an image (default: [`load_source`]).
pub type SourceLoader = dyn Fn(&SourceImage, Option<&Path>) -> AppResult<SourcePixels> + Send + Sync;

/// Everything the segmenter runs on (for [`Segmenter::with_parts`]).
pub struct SegmenterParts {
    pub models: Vec<RegisteredModel>,
    pub store: Arc<dyn MatteStore>,
    pub loader: Arc<SourceLoader>,
}

#[derive(Default)]
struct Flight {
    done: Mutex<Option<AppResult<AiMaskInfo>>>,
    cv: Condvar,
}

type SourceEntry = (ImageId, String, Arc<SourcePixels>);

struct Inner {
    models: Vec<RegisteredModel>,
    store: Arc<dyn MatteStore>,
    loader: Arc<SourceLoader>,
    flights: Mutex<HashMap<(ImageId, String), Arc<Flight>>>,
    sources: Mutex<VecDeque<SourceEntry>>,
    /// Per-image decode locks (coalesce concurrent decodes of one image).
    decoding: Mutex<HashMap<ImageId, Arc<Mutex<()>>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Managed state: model registry + compute coalescing. Cheap to clone.
#[derive(Clone)]
pub struct Segmenter {
    config: Arc<SegmenterConfig>,
    mattes: MaskCache,
    inner: Arc<Inner>,
}

impl Segmenter {
    /// No I/O and no model loading (called during app setup).
    pub fn new(config: SegmenterConfig, mattes: MaskCache) -> Self {
        let coreml_cache = mattes.config().cache_dir.join("coreml");
        let parts = SegmenterParts {
            models: super::segment_models::registry(&config.models_dir, Some(coreml_cache)),
            store: Arc::new(CatalogMatteStore { cache: mattes.clone(), catalog_path: config.catalog_path.clone() }),
            loader: Arc::new(load_source),
        };
        Self::with_parts(config, mattes, parts)
    }

    /// Explicit models / store / source loader (tests, benchmarks).
    pub fn with_parts(config: SegmenterConfig, mattes: MaskCache, parts: SegmenterParts) -> Self {
        Self {
            config: Arc::new(config),
            mattes,
            inner: Arc::new(Inner {
                models: parts.models,
                store: parts.store,
                loader: parts.loader,
                flights: Mutex::new(HashMap::new()),
                sources: Mutex::new(VecDeque::new()),
                decoding: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn config(&self) -> &SegmenterConfig {
        &self.config
    }

    pub fn mattes(&self) -> &MaskCache {
        &self.mattes
    }

    fn model_for(&self, family: AiTargetKind) -> Option<&RegisteredModel> {
        self.inner.models.iter().find(|m| m.model.families().contains(&family))
    }

    /// `Err(reason)` when the family cannot be computed now.
    fn available(&self, family: AiTargetKind) -> Result<&RegisteredModel, String> {
        let Some(m) = self.model_for(family) else {
            return Err(match family {
                AiTargetKind::Landscape => {
                    "no permissively licensed landscape segmentation model is installed".to_owned()
                }
                f => format!("no model for {}", f.as_str()),
            });
        };
        match m.required.iter().find(|p| !p.is_file()) {
            Some(p) => Err(format!(
                "model file {} not installed (run scripts/fetch-models.sh)",
                p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()
            )),
            None => Ok(m),
        }
    }

    /// Which families can be computed now (model files present), one entry per
    /// `AiTargetKind::ALL`, plus separable person parts / landscape categories.
    pub fn capabilities(&self) -> MaskCapabilities {
        let ai = AiTargetKind::ALL
            .iter()
            .map(|&kind| match self.available(kind) {
                Ok(m) => AiCapability { kind, available: true, model: Some(m.model.id().to_owned()), reason: None },
                Err(reason) => AiCapability { kind, available: false, model: None, reason: Some(reason) },
            })
            .collect();
        let person_parts = match self.available(AiTargetKind::People) {
            Ok(m) => m.parts.iter().filter(|(_, files)| files.iter().all(|f| f.is_file())).map(|(p, _)| *p).collect(),
            Err(_) => Vec::new(),
        };
        MaskCapabilities { ai, person_parts, landscape: Vec::new() }
    }

    /// Current model id for a family (`None` = unavailable).
    pub fn model_version(&self, family: AiTargetKind) -> Option<String> {
        self.available(family).ok().map(|m| m.model.id().to_owned())
    }

    /// Computes (or returns the cached) matte for `request` on `src`; blocking. Cached =
    /// a `sieve` row of (image, kind, current model) whose `input_digest` still matches,
    /// unless `request.force`. Errors: `invalid` for an unavailable family (message = reason).
    pub fn compute(
        &self,
        src: &SourceImage,
        preview_path: Option<&Path>,
        request: &AiMaskRequest,
    ) -> AppResult<AiMaskInfo> {
        let family = request
            .target
            .family()
            .ok_or_else(|| AppError::invalid("this Lightroom AI mask kind cannot be computed by Sieve"))?;
        let registered = self.available(family).map_err(AppError::invalid)?.clone();
        if let AiTarget::People { parts } = &request.target {
            let missing: Vec<&str> = parts
                .iter()
                .filter(|p| !registered.parts.iter().any(|(q, files)| q == *p && files.iter().all(|f| f.is_file())))
                .map(|p| p.as_str())
                .collect();
            if !missing.is_empty() {
                return Err(AppError::invalid(format!("person parts not available: {}", missing.join(", "))));
            }
        }
        let kind = AiMask { target: request.target.clone(), reference_point: request.reference_point, digest: None }
            .cache_kind();
        let key = (src.id, kind.clone());
        let (flight, leader) = {
            let mut flights = lock(&self.inner.flights);
            match flights.get(&key) {
                Some(f) => (f.clone(), false),
                None => {
                    let f = Arc::new(Flight::default());
                    flights.insert(key.clone(), f.clone());
                    (f, true)
                }
            }
        };
        if !leader {
            let mut done = lock(&flight.done);
            while done.is_none() {
                done = flight.cv.wait(done).unwrap_or_else(|e| e.into_inner());
            }
            return done.clone().expect("set");
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.compute_now(src, preview_path, request, &registered, &kind)
        }))
        .unwrap_or_else(|_| Err(AppError::internal("segmentation panicked")));
        lock(&self.inner.flights).remove(&key);
        *lock(&flight.done) = Some(result.clone());
        flight.cv.notify_all();
        result
    }

    fn compute_now(
        &self,
        src: &SourceImage,
        preview_path: Option<&Path>,
        request: &AiMaskRequest,
        registered: &RegisteredModel,
        kind: &str,
    ) -> AppResult<AiMaskInfo> {
        let started = Instant::now();
        let model = &registered.model;
        let version = model.id().to_owned();
        let fingerprint = fingerprint_of(src, preview_path)?;
        let info = |m: CachedMatte| AiMaskInfo {
            digest: m.digest,
            target: request.target.clone(),
            reference_point: request.reference_point,
            origin: AiMaskOrigin::Sieve,
            model_version: version.clone(),
            width: m.width,
            height: m.height,
            bounds: m.bounds,
            coverage: m.coverage,
        };
        if !request.force {
            if let Some(hit) = self.inner.store.find(src.id, kind, &version, &fingerprint)? {
                return Ok(info(hit));
            }
        }
        let source = self.source(src, preview_path, &fingerprint)?;
        let t_source = started.elapsed();
        // Select Sky runs on the upright image (the network is trained on upright photos; on
        // a sideways sensor frame it half-selects the sky). Other models see the sensor frame.
        let o = match request.target {
            AiTarget::Sky => src.orientation.filter(|o| (2..=8).contains(o)).unwrap_or(1),
            _ => 1,
        };
        let upright = (o != 1).then(|| orient_pixels(&source.rgb, source.width as usize, source.height as usize, 3, o));
        let input = match &upright {
            Some((rgb, w, h)) => SegmentInput { width: *w as u32, height: *h as u32, rgb, orientation: o },
            None => SegmentInput { width: source.width, height: source.height, rgb: &source.rgb, orientation: 1 },
        };
        let seg_request = SegmentRequest {
            target: orient_target(&request.target, input.orientation),
            reference_point: request.reference_point.map(|p| orient_point(p, input.orientation)),
        };
        let t = Instant::now();
        let matte = model.run(&seg_request, &input).map_err(AppError::internal)?;
        let t_model = t.elapsed();
        let matte = unorient_matte(matte, input.orientation);
        let new = NewMatte {
            kind: kind.to_owned(),
            target: request.target.clone(),
            reference_point: request.reference_point,
            origin: AiMaskOrigin::Sieve,
            digest: None,
            model_version: version.clone(),
            input_digest: Some(fingerprint),
        };
        let out = self.inner.store.put(src.id, &new, &matte)?;
        eprintln!(
            "[masking] image {} {kind}: {} ms (source {} ms, model {} ms, {}x{} matte)",
            src.id,
            started.elapsed().as_millis(),
            t_source.as_millis(),
            t_model.as_millis(),
            matte.width,
            matte.height
        );
        Ok(out)
    }

    /// Model input for `src` (cached per image + fingerprint; concurrent decodes coalesced).
    fn source(
        &self,
        src: &SourceImage,
        preview_path: Option<&Path>,
        fingerprint: &str,
    ) -> AppResult<Arc<SourcePixels>> {
        let cached = || {
            lock(&self.inner.sources).iter().find(|(id, fp, _)| *id == src.id && fp == fingerprint).map(|e| e.2.clone())
        };
        if let Some(s) = cached() {
            return Ok(s);
        }
        let decode_lock = lock(&self.inner.decoding).entry(src.id).or_default().clone();
        let _guard = lock(&decode_lock);
        if let Some(s) = cached() {
            return Ok(s);
        }
        let pixels = Arc::new((self.inner.loader)(src, preview_path)?);
        let mut sources = lock(&self.inner.sources);
        sources.retain(|(id, _, _)| *id != src.id);
        sources.push_back((src.id, fingerprint.to_owned(), pixels.clone()));
        while sources.len() > SOURCE_CACHE {
            sources.pop_front();
        }
        drop(sources);
        lock(&self.inner.decoding).remove(&src.id);
        Ok(pixels)
    }

    /// A `compute` for (image, `AiMask::cache_kind`) is running.
    pub fn is_computing(&self, image_id: ImageId, kind: &str) -> bool {
        lock(&self.inner.flights).keys().any(|(id, k)| *id == image_id && k == kind)
    }

    /// People in the image, left to right in the displayed frame. Runs the people pipeline
    /// (person boxes, faces, instances; cached for the following `compute`), so every
    /// listed person can be selected with its `referencePoint`. The culling face detections
    /// are not reused: they come from the oriented preview and lack person instances.
    pub fn detect_people(&self, src: &SourceImage, preview_path: Option<&Path>) -> AppResult<Vec<DetectedPerson>> {
        let registered = self.available(AiTargetKind::People).map_err(AppError::invalid)?;
        let detector = registered
            .people
            .clone()
            .ok_or_else(|| AppError::invalid("the installed people model cannot list people"))?;
        let fingerprint = fingerprint_of(src, preview_path)?;
        let source = self.source(src, preview_path, &fingerprint)?;
        let input = SegmentInput { width: source.width, height: source.height, rgb: &source.rgb, orientation: 1 };
        let people = detector.people(&input).map_err(AppError::internal)?;
        let o = src.orientation.filter(|o| (1..=8).contains(o)).unwrap_or(1);
        let to_sensor = |p: NormPoint| crate::ipc::types::unorient_point(p, input.orientation);
        let mut out: Vec<DetectedPerson> = people
            .into_iter()
            .map(|p| {
                let sensor_box = unorient_rect(p.bbox, input.orientation);
                let sensor_face = p.face.map(|f| unorient_rect(f, input.orientation));
                DetectedPerson {
                    reference_point: to_sensor(p.reference_point),
                    bbox: orient_rect(sensor_box, o),
                    face: sensor_face.map(|f| orient_rect(f, o)),
                }
            })
            .collect();
        out.sort_by(|a, b| (a.bbox.x + a.bbox.width / 2.0).total_cmp(&(b.bbox.x + b.bbox.width / 2.0)));
        Ok(out)
    }
}

/// `mask_cache.input_digest` for Sieve mattes: identifies the source pixels a matte was
/// computed from (upper-hex of file size + mtime; changes when the original is replaced).
pub fn source_fingerprint(path: &Path) -> std::io::Result<String> {
    let meta = std::fs::metadata(path)?;
    let mtime = meta.modified()?.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or_default();
    Ok(format!("{:016X}{:016X}", meta.len(), mtime as u64))
}

/// Fingerprint of the original, else of the preview (original offline).
fn fingerprint_of(src: &SourceImage, preview_path: Option<&Path>) -> AppResult<String> {
    source_fingerprint(&src.path)
        .or_else(|e| preview_path.map(source_fingerprint).unwrap_or(Err(e)))
        .map_err(|e| AppError::not_found(format!("{}: {e}", src.path.display())))
}

/// Default [`SourceLoader`]: the develop source rendered neutral at [`SOURCE_EDGE`] in the
/// sensor frame; falls back to the (oriented) catalog preview, un-oriented.
pub fn load_source(src: &SourceImage, preview_path: Option<&Path>) -> AppResult<SourcePixels> {
    use crate::develop::{pipeline, source};
    let decoded = source::decode_half_size(&src.path);
    match decoded {
        Ok(img) => {
            let crop = crate::ipc::types::CropSettings::default();
            let prep = source::prepare(&img, 1, &crop, None, SOURCE_EDGE);
            let profile = crate::develop::camera::Profile::matrix(0.0);
            let input = pipeline::RenderInput {
                frame_long_edge: prep.frame_long_edge,
                ..pipeline::RenderInput::simple(prep.width, prep.height, &prep.pixels, &img.color, &profile)
            };
            let out = pipeline::render(&input, &ParametricAdjustments::default(), None);
            Ok(SourcePixels { width: out.width, height: out.height, rgb: out.rgb })
        }
        Err(e) => {
            let Some(preview) = preview_path else { return Err(e) };
            let bytes = std::fs::read(preview)?;
            let d = crate::raw::turbo::decode_rgb(&bytes, SOURCE_EDGE, 400_000_000).map_err(AppError::internal)?;
            let o = src.orientation.filter(|o| (1..=8).contains(o)).unwrap_or(1);
            let (rgb, w, h) = unorient_pixels(&d.pixels, d.width as usize, d.height as usize, 3, o);
            Ok(SourcePixels { width: w as u32, height: h as u32, rgb })
        }
    }
}

fn orient_target(t: &AiTarget, orientation: u8) -> AiTarget {
    match t {
        AiTarget::Object { region } if orientation != 1 => {
            AiTarget::Object { region: orient_rect(*region, orientation) }
        }
        t => t.clone(),
    }
}

/// Sensor-frame rect -> displayed frame.
pub fn orient_rect(r: NormRect, orientation: u8) -> NormRect {
    let a = orient_point(NormPoint { x: r.x, y: r.y }, orientation);
    let b = orient_point(NormPoint { x: r.x + r.width, y: r.y + r.height }, orientation);
    NormRect { x: a.x.min(b.x), y: a.y.min(b.y), width: (a.x - b.x).abs(), height: (a.y - b.y).abs() }
}

/// Displayed-frame rect -> sensor frame.
pub fn unorient_rect(r: NormRect, orientation: u8) -> NormRect {
    use crate::ipc::types::unorient_point;
    let a = unorient_point(NormPoint { x: r.x, y: r.y }, orientation);
    let b = unorient_point(NormPoint { x: r.x + r.width, y: r.y + r.height }, orientation);
    NormRect { x: a.x.min(b.x), y: a.y.min(b.y), width: (a.x - b.x).abs(), height: (a.y - b.y).abs() }
}

/// A matte in an oriented input's frame -> sensor frame.
pub fn unorient_matte(m: AlphaMask, orientation: u8) -> AlphaMask {
    if !(2..=8).contains(&orientation) {
        return m;
    }
    let (data, w, h) = unorient_pixels(&m.data, m.width as usize, m.height as usize, 1, orientation);
    AlphaMask { width: w as u32, height: h as u32, bounds: unorient_rect(m.bounds, orientation), data }
}

/// Sensor-frame interleaved pixels (`channels` per pixel) -> displayed frame for EXIF
/// `orientation` (inverse of `refine::unorient_pixels`). Returns the pixels and their size.
pub fn orient_pixels(px: &[u8], w: usize, h: usize, channels: usize, orientation: u8) -> (Vec<u8>, usize, usize) {
    if !(2..=8).contains(&orientation) {
        return (px[..w * h * channels].to_vec(), w, h);
    }
    let (dw, dh) = if orientation >= 5 { (h, w) } else { (w, h) };
    let mut out = vec![0u8; dw * dh * channels];
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = oriented_index(x, y, w, h, orientation);
            let (s, d) = ((y * w + x) * channels, (dy * dw + dx) * channels);
            out[d..d + channels].copy_from_slice(&px[s..s + channels]);
        }
    }
    (out, dw, dh)
}

/// Sensor-frame matte -> oriented frame (inverse of [`unorient_matte`]; tests, overlays).
pub fn orient_matte(m: &AlphaMask, orientation: u8) -> AlphaMask {
    if !(2..=8).contains(&orientation) {
        return m.clone();
    }
    let (sw, sh) = (m.width as usize, m.height as usize);
    let (dw, dh) = if orientation >= 5 { (sh, sw) } else { (sw, sh) };
    let mut data = vec![0u8; sw * sh];
    for y in 0..sh {
        for x in 0..sw {
            let (dx, dy) = oriented_index(x, y, sw, sh, orientation);
            data[dy * dw + dx] = m.data[y * sw + x];
        }
    }
    AlphaMask { width: dw as u32, height: dh as u32, bounds: orient_rect(m.bounds, orientation), data }
}

/// Mean matte value over the whole frame (`AiMaskInfo.coverage`; the same definition as
/// `AlphaMask::coverage`).
pub fn coverage(m: &AlphaMask) -> f32 {
    if m.data.is_empty() {
        return 0.0;
    }
    let mean = m.data.iter().map(|&v| v as f64).sum::<f64>() / (m.data.len() as f64 * 255.0);
    (mean * (m.bounds.width * m.bounds.height).clamp(0.0, 1.0) as f64) as f32
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_sky;
