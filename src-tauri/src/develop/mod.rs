//! Develop engine (Phase 5): preview rendering from a cached half-size linear RAW decode,
//! edit history, presets. Owned by rust-engine-dev.
//!
//! The architect fixed only the surface used by `ipc::commands` and `lib.rs`:
//! [`DevelopConfig`], [`SourceImage`], [`DevelopCache`] (`new`, `config`, `ticket`,
//! `render`, `info`, `prefetch`, `encoded`), [`render_url`], [`handle_protocol`],
//! [`RENDER_SCHEME`], and the SQL seams [`history`] / [`presets`]. The compute split
//! ([`source`], [`pipeline`], [`wb`]) is internal.
//!
//! Contract (details in `docs/architecture.md`, "Editor"):
//! - Source: LibRaw `half_size = 1` decode of the RAW (one pixel per 2x2 Bayer quad, or the
//!   X-Trans equivalent), *unscaled camera RGB, no white balance, linear, 16-bit*, plus the
//!   camera's as-shot multipliers and colour matrices. Cached per image in an LRU bounded by
//!   bytes (default 1 GiB, `SIEVE_DEVELOP_CACHE_MB`), together with up to
//!   [`PREPARED_PER_IMAGE`] "prepared" inputs (region crop + resample + orientation, u16) for
//!   the sizes the UI asks for, so a slider render only runs the pipeline + JPEG encode.
//! - Pipeline: see [`pipeline`]. Orientation applied; `region` crops before resampling.
//! - Latest-wins: [`DevelopCache::ticket`] is taken on the async side in arrival order;
//!   [`DevelopCache::render`] returns `Ok(None)` without rendering (or after, discarding the
//!   result) when a newer ticket exists for the same (image, slot). At most one render per
//!   key runs at a time. The encoded JPEG of the newest finished render per key is kept in
//!   memory (bounded to [`MAX_ENCODED`] keys) and served by [`handle_protocol`].
//! - Prefetch: one background thread decodes queued sources (newest request replaces the
//!   queue); concurrent decodes of the same image are coalesced.

pub mod auto;
pub mod baseline;
pub mod batches;
pub mod camera;
pub mod cancel;
pub mod edited;
pub mod fastmath;
pub mod highlights;
pub mod history;
pub mod local;
mod local_tone_data;
pub mod masks;
mod param_data;
pub mod parity;
pub mod pipeline;
pub mod presets;
pub mod source;
pub mod sync_delta;
pub mod tone;
mod tone_data;
pub mod transform;
pub mod wb;

use std::collections::{HashMap, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use tauri::http;

use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::{
    CameraCalibration, CropSettings, DevelopInfo, DevelopWarning, DevelopWarningCode, ImageId, NormPoint, NormRect,
    ParametricAdjustments, ProfileSettings, RenderOptions, RenderSlot, RenderedPreview, WhiteBalance,
    WhiteBalanceValues,
};
use crate::lut::LutLibrary;
use crate::profiles::ProfileLibrary;

use camera::Profile;
use source::{LinearImage, Prepared, SourceMeta};
use transform::Geometry;

/// Tone contexts kept per image (white balance / calibration / profile variants: toggling
/// between a few, e.g. before/after or presets, stays cached; ~1 MB each).
const TONE_CONTEXTS: usize = 4;

/// Custom URI scheme serving rendered previews (registered in `lib.rs`).
pub const RENDER_SCHEME: &str = "sieve";
/// JPEG quality of served previews (4:4:4).
pub const JPEG_QUALITY: u8 = 90;
/// JPEG quality of draft renders (slider drags; 4:2:0).
pub const DRAFT_JPEG_QUALITY: u8 = 75;
/// Prepared (cropped/resampled) inputs kept per cached image.
pub const PREPARED_PER_IMAGE: usize = 3;
/// Encoded renders kept for the protocol handler (oldest dropped first).
pub const MAX_ENCODED: usize = 24;

/// Resolved at startup by `lib.rs`.
#[derive(Clone)]
pub struct DevelopConfig {
    /// Upper bound of decoded sources + working copies kept in memory.
    pub cache_bytes: u64,
    /// AI matte cache for masks (Phase 7c); `None` = AI mask components render empty.
    pub mask_cache: Option<masks::MaskCache>,
}

impl DevelopConfig {
    pub const DEFAULT_CACHE_MB: u64 = 1024;
}

impl Default for DevelopConfig {
    fn default() -> Self {
        DevelopConfig { cache_bytes: Self::DEFAULT_CACHE_MB * 1024 * 1024, mask_cache: None }
    }
}

impl std::fmt::Debug for DevelopConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DevelopConfig")
            .field("cache_bytes", &self.cache_bytes)
            .field("mask_cache", &self.mask_cache.as_ref().map(|c| c.config().clone()))
            .finish()
    }
}

/// What the engine needs to know about an image (resolved from the catalog by the command).
#[derive(Debug, Clone, PartialEq)]
pub struct SourceImage {
    pub id: ImageId,
    pub path: PathBuf,
    /// EXIF orientation 1..=8 (`None` = 1).
    pub orientation: Option<u8>,
}

impl SourceImage {
    fn orientation(&self) -> u8 {
        self.orientation.filter(|o| (1..=8).contains(o)).unwrap_or(1)
    }
}

/// Claim to render the newest state of one (image, slot) stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderTicket {
    pub image_id: ImageId,
    pub slot: RenderSlot,
    pub seq: u32,
}

/// Renders at most this long run as drafts (slider drags): cheaper LUT, no luminance NR.
pub const DRAFT_EDGE: u32 = 1024;

/// Identity of a prepared input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PrepKey {
    orientation: u8,
    max_edge: u32,
    region: Option<[u32; 4]>,
    /// Effective crop (enabled flag + rect + angle bits); `None` = whole frame.
    crop: Option<[u32; 5]>,
    /// Transform warp bits.
    warp: Option<[u64; 9]>,
}

impl PrepKey {
    fn new(orientation: u8, max_edge: u32, region: Option<NormRect>, geo: &Geometry) -> Self {
        let region = region.map(|r| [r.x.to_bits(), r.y.to_bits(), r.width.to_bits(), r.height.to_bits()]);
        let (crop, warp) = geo.key();
        PrepKey { orientation, max_edge, region, crop, warp }
    }
}

/// A decoded source and its prepared inputs.
struct Entry {
    path: PathBuf,
    image: LinearImage,
    meta: SourceMeta,
    prepared: Mutex<Vec<(PrepKey, Arc<Prepared>)>>,
    /// Last resolved profile (resolution reads the sidecar when a look is not installed).
    profile: Mutex<Option<(ProfileSettings, Arc<Profile>)>>,
    /// Uncropped [`pipeline::TONE_GRID`] px source for the local tone context.
    tone_src: Mutex<Option<Arc<Prepared>>>,
    /// Last uncropped local tone context and the settings it depends on.
    /// Most recently used tone contexts (newest last, at most [`TONE_CONTEXTS`]).
    tone_ctx: Mutex<Vec<(ToneKey, Arc<pipeline::ToneContext>)>>,
}

/// What the uncropped local tone context depends on besides the source.
type ToneKey = (WhiteBalance, CameraCalibration, ProfileSettings);

impl Entry {
    fn bytes(&self) -> u64 {
        let prepared: usize = lock(&self.prepared).iter().map(|(_, p)| p.bytes()).sum();
        (self.image.bytes() + prepared) as u64
    }

    /// Prepared input for the key (computed and cached if needed).
    fn prepared(&self, orientation: u8, geo: &Geometry, region: Option<NormRect>, max_edge: u32) -> Arc<Prepared> {
        let key = PrepKey::new(orientation, max_edge, region, geo);
        {
            let mut list = lock(&self.prepared);
            if let Some(i) = list.iter().position(|(k, _)| *k == key) {
                let hit = list.remove(i);
                let p = hit.1.clone();
                list.push(hit);
                return p;
            }
        }
        let p = Arc::new(source::prepare_geo(&self.image, orientation, geo, region, max_edge));
        let mut list = lock(&self.prepared);
        if list.len() >= PREPARED_PER_IMAGE {
            list.remove(0);
        }
        list.push((key, p.clone()));
        p
    }

    /// Geometry `adjustments` render with on this source (effective crop + Transform warp).
    fn geometry(&self, adjustments: &ParametricAdjustments) -> Geometry {
        Geometry::of(adjustments, self.image.full_width, self.image.full_height)
    }

    /// Camera profile + look for `settings` (cached for the last settings).
    fn profile(&self, settings: &ProfileSettings) -> Arc<Profile> {
        if let Some((s, p)) = lock(&self.profile).as_ref() {
            if s == settings {
                return p.clone();
            }
        }
        let sidecar = crate::xmp::sidecar_path(&self.path);
        let p = Arc::new(camera::resolve(&self.meta, settings, &ProfileLibrary::shared(), Some(&sidecar)));
        *lock(&self.profile) = Some((settings.clone(), p.clone()));
        p
    }

    /// The format's default profile (for `info`).
    fn default_profile(&self) -> Arc<Profile> {
        self.profile(&ParametricAdjustments::defaults_for(self.meta.format).profile)
    }

    /// Neutral render of the whole image in the sensor frame (guide for refining Sieve AI
    /// mattes, `develop::masks::render`).
    fn sensor_guide(&self) -> masks::render::SensorGuide {
        let prep = source::prepare(&self.image, 1, &CropSettings::default(), None, 2048);
        let profile = self.default_profile();
        let mut adj = ParametricAdjustments::defaults_for(self.meta.format);
        adj.masks.clear();
        let input = pipeline::RenderInput {
            frame_long_edge: prep.frame_long_edge,
            view: prep.view,
            quality: pipeline::Quality::Draft,
            ..pipeline::RenderInput::simple(prep.width, prep.height, &prep.pixels, &self.image.color, &profile)
        };
        let img = pipeline::render(&input, &adj, None);
        masks::render::SensorGuide { width: img.width, height: img.height, rgb: img.rgb }
    }

    fn input<'a>(
        &'a self,
        prepared: &'a Prepared,
        profile: &'a Profile,
        seed: ImageId,
        quality: pipeline::Quality,
        tone: Option<&'a pipeline::ToneContext>,
    ) -> pipeline::RenderInput<'a> {
        pipeline::RenderInput {
            width: prepared.width,
            height: prepared.height,
            pixels: &prepared.pixels,
            color: &self.image.color,
            frame_long_edge: prepared.frame_long_edge,
            view: prepared.view,
            profile,
            seed: seed as u64,
            quality,
            tone,
        }
    }

    /// Local tone + highlight-reconstruction context of the whole uncropped source (so
    /// drafts, previews, zoomed regions and exports adapt and rebuild clipped highlights
    /// alike, local Shadows/Highlights of mask groups included); cached per white balance /
    /// calibration / profile.
    fn tone_context(
        &self,
        orientation: u8,
        adjustments: &ParametricAdjustments,
        geo: &Geometry,
        profile: &Profile,
    ) -> Option<pipeline::ToneContext> {
        let key: ToneKey = (adjustments.white_balance, adjustments.calibration, adjustments.profile.clone());
        let cached = {
            let mut list = lock(&self.tone_ctx);
            let hit = list.iter().position(|(k, _)| *k == key);
            hit.map(|i| {
                let e = list.remove(i);
                let c = e.1.clone();
                list.push(e);
                c
            })
        };
        let base = match cached {
            Some(c) => c,
            None => {
                let whole = lock(&self.tone_src)
                    .get_or_insert_with(|| {
                        Arc::new(source::prepare(&self.image, 1, &CropSettings::default(), None, pipeline::TONE_GRID))
                    })
                    .clone();
                let c = Arc::new(pipeline::tone_context_uncropped(&whole, &self.image, adjustments, profile));
                let mut list = lock(&self.tone_ctx);
                if list.len() >= TONE_CONTEXTS {
                    list.remove(0);
                }
                list.push((key, c.clone()));
                c
            }
        };
        let (w, h) = (self.image.width, self.image.height);
        Some((*base).clone().with_geometry(geo, w, h, orientation))
    }
}

/// Half-size decode of an original with user-facing errors: a missing/moved file is
/// `file_missing` ("Original file is missing ..."), a decoder failure is `decode_failed`.
fn decode_source(path: &std::path::Path) -> AppResult<(LinearImage, SourceMeta)> {
    crate::raw::access::require_original(path)?;
    source::decode_half_size_meta(path).map_err(|e| match e.kind {
        crate::ipc::error::ErrorKind::Internal => {
            let prefix = format!("{}: ", path.display());
            crate::raw::access::decode_failed(path, e.message.strip_prefix(&prefix).unwrap_or(&e.message))
        }
        _ => e,
    })
}

/// Decodes `src` into a fresh (uncached) entry.
fn load_entry(src: &SourceImage) -> AppResult<Entry> {
    let (image, meta) = decode_source(&src.path)?;
    Ok(Entry {
        path: src.path.clone(),
        image,
        meta,
        prepared: Mutex::new(Vec::new()),
        profile: Mutex::new(None),
        tone_src: Mutex::new(None),
        tone_ctx: Mutex::new(Vec::new()),
    })
}

/// Rayon pool of interactive preview renders: leaves two cores to the UI (WebView main thread, compositor), so a
/// slider drag does not starve the page it is dragged in. Min 2 threads.
fn preview_pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        rayon::ThreadPoolBuilder::new()
            .num_threads(cores.saturating_sub(2).max(2))
            .thread_name(|i| format!("develop-preview-{i}"))
            .build()
            .expect("preview thread pool")
    })
}

/// Background work (prefetch decode): at most half the cores, so it never competes with a render.
fn background_pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        rayon::ThreadPoolBuilder::new()
            .num_threads((cores / 4).max(2))
            .thread_name(|i| format!("develop-background-{i}"))
            .build()
            .expect("background thread pool")
    })
}

fn quality_for(max_edge: u32) -> pipeline::Quality {
    if max_edge <= DRAFT_EDGE {
        pipeline::Quality::Draft
    } else {
        pipeline::Quality::Preview
    }
}

#[derive(Default)]
struct Lru {
    map: HashMap<ImageId, (Arc<Entry>, u64)>,
    clock: u64,
}

#[derive(Default)]
struct Prefetch {
    queue: VecDeque<SourceImage>,
    running: bool,
}

type SlotKey = (ImageId, RenderSlot);
/// Newest ticket seq per stream; shared with running renders so they can abort ([`cancel`]).
type LatestMap = HashMap<SlotKey, Arc<AtomicU32>>;
/// Newest JPEG per key `(seq, bytes)` + insertion order (for bounding).
type EncodedStore = (HashMap<SlotKey, (u32, Arc<Vec<u8>>)>, VecDeque<SlotKey>);

struct Inner {
    lru: Mutex<Lru>,
    /// Per-image decode locks (coalesce concurrent decodes of one image).
    decoding: Mutex<HashMap<ImageId, Arc<Mutex<()>>>>,
    /// Per-(image, slot) render locks (at most one render per key).
    rendering: Mutex<HashMap<SlotKey, Arc<Mutex<()>>>>,
    /// Newest finished JPEG per key: (seq, bytes), plus insertion order for bounding.
    encoded: Mutex<EncodedStore>,
    prefetch: Mutex<Prefetch>,
    /// Evaluated mask weights of recent renders (Phase 7c).
    weights: masks::render::WeightCache,
    /// Edited-preview cache (IPC v19.1), once enabled by the app.
    edited: OnceLock<edited::EditedPreviews>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Managed Tauri state: decoded-source LRU, latest-wins bookkeeping and the last encoded
/// render per (image, slot). Cheap to clone.
#[derive(Clone)]
pub struct DevelopCache {
    config: DevelopConfig,
    /// Newest ticket issued per (image, slot).
    latest: Arc<Mutex<LatestMap>>,
    /// Catalog facts (path, orientation) of recently edited images, so slider renders skip
    /// the catalog (and its lock) entirely. Filled by the commands; see [`Self::source`].
    sources: Arc<Mutex<HashMap<ImageId, SourceImage>>>,
    inner: Arc<Inner>,
    /// On-demand face detection for Auto tone on unanalysed photos ([`Self::with_auto_faces`]).
    auto_faces: Option<crate::ml::auto_faces::AutoFaces>,
}

/// Remembered [`SourceImage`]s (cleared wholesale when exceeded; each is ~100 bytes).
pub const MAX_SOURCES: usize = 4096;

impl DevelopCache {
    pub fn new(config: DevelopConfig) -> Self {
        // Scan the installed Adobe profiles in the background so the first render does not
        // wait for it, and register the style library's imported looks / DCPs (IPC v14; the
        // catalog path is known through the mask cache in the app).
        let catalog_path = config.mask_cache.as_ref().map(|m| m.config().catalog_path.clone());
        let _ = std::thread::Builder::new().name("profile-scan".into()).spawn(move || {
            ProfileLibrary::shared().warm();
            if let Some(path) = catalog_path {
                // Tests: don't replace the registry mid-way through a style-import test.
                #[cfg(test)]
                let _registry = crate::styles::test_registry_guard();
                let registered = masks::open_catalog_read_only(&path)
                    .and_then(|conn| crate::styles::register_imported_profiles(&conn));
                if let Err(e) = registered {
                    eprintln!("style library profiles: {}", e.message);
                }
            }
        });
        Self {
            config,
            latest: Arc::new(Mutex::new(HashMap::new())),
            sources: Arc::new(Mutex::new(HashMap::new())),
            inner: Arc::new(Inner {
                lru: Mutex::new(Lru::default()),
                decoding: Mutex::new(HashMap::new()),
                rendering: Mutex::new(HashMap::new()),
                encoded: Mutex::new((HashMap::new(), VecDeque::new())),
                prefetch: Mutex::new(Prefetch::default()),
                weights: masks::render::WeightCache::default(),
                edited: OnceLock::new(),
            }),
            auto_faces: None,
        }
    }

    pub fn config(&self) -> &DevelopConfig {
        &self.config
    }

    /// Enables on-demand face detection for Auto tone (`develop::auto::resolve_faces`).
    pub fn with_auto_faces(mut self, faces: crate::ml::auto_faces::AutoFaces) -> Self {
        self.auto_faces = Some(faces);
        self
    }

    /// The on-demand face detector, if enabled.
    pub fn auto_faces(&self) -> Option<&crate::ml::auto_faces::AutoFaces> {
        self.auto_faces.as_ref()
    }

    /// Starts the edited-preview cache (IPC v19.1; files in `config.dir`, renders through this
    /// cache with `luts`, changes reported to `emit`). Idempotent: later calls return the
    /// first instance.
    pub fn enable_edited_previews(
        &self,
        config: edited::EditedConfig,
        luts: LutLibrary,
        emit: edited::Emitter,
    ) -> edited::EditedPreviews {
        self.inner.edited.get_or_init(|| edited::EditedPreviews::start(config, luts, emit, self.clone())).clone()
    }

    /// The edited-preview cache, if enabled.
    pub fn edited(&self) -> Option<&edited::EditedPreviews> {
        self.inner.edited.get()
    }

    /// The remembered catalog facts of image `id` (see [`Self::remember_source`]).
    pub fn source(&self, id: ImageId) -> Option<SourceImage> {
        lock(&self.sources).get(&id).cloned()
    }

    /// Remembers `src` for [`Self::source`]. Callers only remember images whose metadata is
    /// settled (thumbnail extraction finished: orientation known).
    pub fn remember_source(&self, src: SourceImage) {
        let mut map = lock(&self.sources);
        if map.len() >= MAX_SOURCES && !map.contains_key(&src.id) {
            map.clear();
        }
        map.insert(src.id, src);
    }

    /// Forgets remembered sources: `Some(ids)` (e.g. thumbnails regenerated: orientation
    /// may change) or all (`None`, e.g. after an import re-registered files).
    pub fn forget_sources(&self, ids: Option<&[ImageId]>) {
        if let Some(f) = &self.auto_faces {
            f.forget(ids);
        }
        let mut map = lock(&self.sources);
        match ids {
            Some(ids) => ids.iter().for_each(|id| {
                map.remove(id);
            }),
            None => map.clear(),
        }
    }

    /// Forgets everything held in memory for images removed from the catalog
    /// (`remove_project`; ids can be reused by the next import): decoded sources and their
    /// working copies, the last encoded renders and tickets, decode/render locks, queued
    /// prefetches, remembered sources, evaluated mask weights and decoded mattes (+ matte
    /// files).
    pub fn forget_images(&self, ids: &[ImageId]) {
        if ids.is_empty() {
            return;
        }
        self.forget_sources(Some(ids));
        lock(&self.inner.lru).map.retain(|id, _| !ids.contains(id));
        {
            let mut enc = lock(&self.inner.encoded);
            enc.0.retain(|(id, _), _| !ids.contains(id));
            enc.1.retain(|(id, _)| !ids.contains(id));
        }
        lock(&self.latest).retain(|(id, _), _| !ids.contains(id));
        lock(&self.inner.decoding).retain(|id, _| !ids.contains(id));
        lock(&self.inner.rendering).retain(|(id, _), _| !ids.contains(id));
        lock(&self.inner.prefetch).queue.retain(|s| !ids.contains(&s.id));
        self.inner.weights.forget(ids);
        if let Some(m) = &self.config.mask_cache {
            m.forget_images(ids);
        }
        if let Some(e) = self.edited() {
            e.forget(ids);
        }
    }

    /// Issues the next ticket for (image, slot). Call on the async side, before any
    /// blocking work, so tickets follow request arrival order.
    pub fn ticket(&self, image_id: ImageId, slot: RenderSlot) -> RenderTicket {
        let mut latest = self.latest.lock().unwrap_or_else(|e| e.into_inner());
        let cell = latest.entry((image_id, slot)).or_default();
        // Also what running renders of this stream poll to abort mid-loop ([`cancel`]).
        let seq = cell.load(Ordering::Relaxed).wrapping_add(1);
        cell.store(seq, Ordering::Relaxed);
        RenderTicket { image_id, slot, seq }
    }

    /// `ticket` is still the newest for its (image, slot).
    pub fn is_current(&self, ticket: RenderTicket) -> bool {
        let latest = self.latest.lock().unwrap_or_else(|e| e.into_inner());
        latest.get(&(ticket.image_id, ticket.slot)).is_some_and(|c| c.load(Ordering::Relaxed) == ticket.seq)
    }

    /// Cached entry, touching its LRU stamp.
    fn cached(&self, src: &SourceImage) -> Option<Arc<Entry>> {
        let mut lru = lock(&self.inner.lru);
        lru.clock += 1;
        let now = lru.clock;
        match lru.map.get_mut(&src.id) {
            Some((e, stamp)) if e.path == src.path => {
                *stamp = now;
                Some(e.clone())
            }
            _ => None,
        }
    }

    /// Decoded source for `src` (decoding and caching it if needed).
    fn entry(&self, src: &SourceImage) -> AppResult<Arc<Entry>> {
        if let Some(e) = self.cached(src) {
            return Ok(e);
        }
        let decode_lock = lock(&self.inner.decoding).entry(src.id).or_default().clone();
        let _guard = lock(&decode_lock);
        if let Some(e) = self.cached(src) {
            return Ok(e);
        }
        let entry = match load_entry(src) {
            Ok(e) => Arc::new(e),
            Err(e) => {
                lock(&self.inner.decoding).remove(&src.id);
                return Err(e);
            }
        };
        {
            let mut lru = lock(&self.inner.lru);
            lru.clock += 1;
            let now = lru.clock;
            lru.map.insert(src.id, (entry.clone(), now));
        }
        self.evict(src.id);
        lock(&self.inner.decoding).remove(&src.id);
        Ok(entry)
    }

    /// Drops least recently used entries (never `keep`) until within the byte budget.
    fn evict(&self, keep: ImageId) {
        let mut lru = lock(&self.inner.lru);
        loop {
            let total: u64 = lru.map.values().map(|(e, _)| e.bytes()).sum();
            if total <= self.config.cache_bytes {
                return;
            }
            let victim = lru.map.iter().filter(|(id, _)| **id != keep).min_by_key(|(_, (_, s))| *s).map(|(id, _)| *id);
            match victim {
                Some(id) => {
                    lru.map.remove(&id);
                }
                None => return,
            }
        }
    }

    /// Blocking (call from the blocking pool). Renders `adjustments` for `src` per
    /// `options` (already validated), stores the JPEG for [`Self::encoded`] and returns
    /// the metadata with `url = render_url(id, slot, seq)`. `Ok(None)` when superseded by a
    /// newer ticket for the same (image, slot). Decodes (and caches) the source if needed.
    /// A LUT id not in `luts` renders without the LUT and sets `lutMissing`.
    pub fn render(
        &self,
        ticket: RenderTicket,
        src: &SourceImage,
        adjustments: &ParametricAdjustments,
        options: &RenderOptions,
        luts: &LutLibrary,
    ) -> AppResult<Option<RenderedPreview>> {
        let started = Instant::now();
        if !self.is_current(ticket) {
            return Ok(None);
        }
        if let Some(e) = self.edited() {
            e.note_interactive();
        }
        let key = (ticket.image_id, ticket.slot);
        let render_lock = lock(&self.inner.rendering).entry(key).or_default().clone();
        let _guard = lock(&render_lock);
        if !self.is_current(ticket) {
            return Ok(None);
        }
        let cell = lock(&self.latest).get(&key).cloned();
        // Everything heavy runs on the dedicated preview pool; only the uncached final stages are cancellable.
        let rendered = preview_pool().install(|| -> AppResult<Option<(pipeline::RenderedImage, bool)>> {
            let entry = self.entry(src)?;
            let geo = entry.geometry(adjustments);
            let prepared = entry.prepared(src.orientation(), &geo, options.region, options.max_edge);
            self.evict(src.id);
            let lut = match &adjustments.lut {
                Some(l) => luts.load(&l.id)?,
                None => None,
            };
            let lut_missing = adjustments.lut.is_some() && lut.is_none();
            if !self.is_current(ticket) {
                return Ok(None);
            }
            let profile = entry.profile(&adjustments.profile);
            let tone = entry.tone_context(src.orientation(), adjustments, &geo, &profile);
            let input = entry.input(&prepared, &profile, src.id, quality_for(options.max_edge), tone.as_ref());
            let (local, _) = self.local_planes(&entry, src, adjustments, &geo, &input, options.region);
            let run = || pipeline::render_masked(&input, adjustments, lut.as_deref(), local.as_ref());
            let mut img = match cell {
                Some(c) => cancel::scope(c, ticket.seq, run),
                None => run(),
            };
            if let Some(c) = &prepared.coverage {
                source::fill_outside_rgb8(&mut img.rgb, c);
            }
            Ok(Some((img, lut_missing)))
        })?;
        let Some((img, lut_missing)) = rendered else {
            return Ok(None);
        };
        if !self.is_current(ticket) {
            return Ok(None);
        }
        let jpeg = if quality_for(options.max_edge) == pipeline::Quality::Draft {
            crate::raw::turbo::encode_rgb(&img.rgb, img.width, img.height, DRAFT_JPEG_QUALITY)
        } else {
            crate::raw::turbo::encode_rgb_444(&img.rgb, img.width, img.height, JPEG_QUALITY)
        }
        .map_err(AppError::internal)?;
        // A settled full-quality render of the whole frame becomes the edited preview if these
        // are the stored settings (edited cache, IPC v19.1); otherwise it is dropped here.
        let settled = ticket.slot == RenderSlot::Main
            && options.region.is_none()
            && quality_for(options.max_edge) == pipeline::Quality::Preview;
        match self.edited() {
            Some(e) if settled => e.note_settled(
                ticket.image_id,
                adjustments,
                crate::raw::preview::Rgb { width: img.width, height: img.height, pixels: img.rgb },
            ),
            _ => drop(img.rgb),
        }
        if !self.is_current(ticket) {
            return Ok(None);
        }
        self.store_encoded(key, ticket.seq, jpeg);
        Ok(Some(RenderedPreview {
            image_id: ticket.image_id,
            slot: ticket.slot,
            seq: ticket.seq,
            url: render_url(ticket.image_id, ticket.slot, ticket.seq),
            width: img.width,
            height: img.height,
            histogram: img.histogram,
            render_ms: started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32,
            lut_missing,
        }))
    }

    fn store_encoded(&self, key: (ImageId, RenderSlot), seq: u32, jpeg: Vec<u8>) {
        let mut guard = lock(&self.inner.encoded);
        let (map, order) = &mut *guard;
        if map.get(&key).is_some_and(|(s, _)| *s > seq) {
            return;
        }
        map.insert(key, (seq, Arc::new(jpeg)));
        order.retain(|k| *k != key);
        order.push_back(key);
        while order.len() > MAX_ENCODED {
            if let Some(old) = order.pop_front() {
                map.remove(&old);
            }
        }
    }

    /// Blocking. As-shot WB and sizes; decodes (and caches) the source if needed.
    pub fn info(&self, src: &SourceImage) -> AppResult<DevelopInfo> {
        let entry = self.entry(src)?;
        let img = &entry.image;
        let o = src.orientation();
        let (source_width, source_height) = source::oriented_size(img.width, img.height, o);
        let (full_width, full_height) = source::oriented_size(img.full_width, img.full_height, o);
        let profile = entry.default_profile();
        let as_shot = camera::as_shot_values(&img.color, &profile);
        let mut warnings = profile.warnings.clone();
        if let Some(detail) = img.source_color.as_ref().and_then(|c| c.assumed_detail()) {
            warnings.push(DevelopWarning { code: DevelopWarningCode::SourceColorAssumed, detail: Some(detail) });
        }
        warnings.extend(self.mask_warning(src.id));
        Ok(DevelopInfo { image_id: src.id, as_shot, source_width, source_height, full_width, full_height, warnings })
    }

    /// Crop-tool bounds of the live `adjustments` (IPC v19.3): pure geometry on the develop
    /// source's aspect (decodes the source if not cached, as `info`).
    pub fn transform_bounds(
        &self,
        src: &SourceImage,
        adjustments: &ParametricAdjustments,
    ) -> AppResult<crate::ipc::types::TransformBounds> {
        let entry = self.entry(src)?;
        let img = &entry.image;
        Ok(transform::bounds(adjustments, img.full_width, img.full_height, src.orientation()))
    }

    /// Warms the cache for `sources` in the background (e.g. filmstrip neighbours) and
    /// returns immediately. Lower priority than `render`; must never panic.
    pub fn prefetch(&self, sources: Vec<SourceImage>) {
        let mut pf = lock(&self.inner.prefetch);
        pf.queue = sources.into_iter().collect();
        if pf.running || pf.queue.is_empty() {
            return;
        }
        pf.running = true;
        drop(pf);
        let this = self.clone();
        let spawned = std::thread::Builder::new().name("develop-prefetch".into()).spawn(move || loop {
            let next = {
                let mut pf = lock(&this.inner.prefetch);
                match pf.queue.pop_front() {
                    Some(s) => s,
                    None => {
                        pf.running = false;
                        return;
                    }
                }
            };
            if this.cached(&next).is_none() {
                match catch_unwind(AssertUnwindSafe(|| background_pool().install(|| this.entry(&next)))) {
                    Ok(Err(e)) => eprintln!("develop prefetch {}: {}", next.path.display(), e.message),
                    Err(_) => eprintln!("develop prefetch {}: panicked", next.path.display()),
                    Ok(Ok(_)) => {}
                }
            }
            // Prerender the neighbour's edited preview (no-op when unedited or current).
            if let Some(e) = this.edited() {
                e.ensure(&[next.id], true);
            }
        });
        if spawned.is_err() {
            lock(&self.inner.prefetch).running = false;
        }
    }

    /// Encoded JPEG of the newest finished render for (image, slot) if its seq >= `min_seq`.
    pub fn encoded(&self, image_id: ImageId, slot: RenderSlot, min_seq: u32) -> Option<Arc<Vec<u8>>> {
        let guard = lock(&self.inner.encoded);
        guard.0.get(&(image_id, slot)).filter(|(seq, _)| *seq >= min_seq).map(|(_, b)| b.clone())
    }

    /// Images currently decoded in memory (tests/bench).
    pub fn cached_count(&self) -> usize {
        lock(&self.inner.lru).map.len()
    }

    /// Bytes held by decoded sources and prepared inputs (tests/bench).
    pub fn cached_bytes(&self) -> u64 {
        lock(&self.inner.lru).map.values().map(|(e, _)| e.bytes()).sum()
    }

    /// Blocking. Renders `adjustments` (already validated) through the same source + pipeline
    /// as [`Self::render`] and returns the pixels instead of an encoded JPEG: no tickets, no
    /// latest-wins, nothing stored for the protocol. Used by scene matching / render stats
    /// (Phase 7), so measurements see exactly what the editor shows. Also returns
    /// `lut_missing` and the as-shot white balance (as in [`Self::info`]).
    pub fn render_image(
        &self,
        src: &SourceImage,
        adjustments: &ParametricAdjustments,
        region: Option<NormRect>,
        max_edge: u32,
        luts: &LutLibrary,
    ) -> AppResult<RenderedPixels> {
        let entry = self.entry(src)?;
        let geo = entry.geometry(adjustments);
        let prepared = entry.prepared(src.orientation(), &geo, region, max_edge);
        self.evict(src.id);
        let lut = match &adjustments.lut {
            Some(l) => luts.load(&l.id)?,
            None => None,
        };
        let lut_missing = adjustments.lut.is_some() && lut.is_none();
        let profile = entry.profile(&adjustments.profile);
        let tone = entry.tone_context(src.orientation(), adjustments, &geo, &profile);
        let input = entry.input(&prepared, &profile, src.id, quality_for(max_edge), tone.as_ref());
        let (local, _) = self.local_planes(&entry, src, adjustments, &geo, &input, region);
        let mut image = pipeline::render_masked(&input, adjustments, lut.as_deref(), local.as_ref());
        if let Some(c) = &prepared.coverage {
            source::fill_outside_rgb8(&mut image.rgb, c);
        }
        let as_shot = camera::as_shot_values(&entry.image.color, &profile);
        Ok(RenderedPixels { image, lut_missing, as_shot })
    }

    /// Blocking. Full-quality render of the whole (cropped) frame for the edited-preview cache:
    /// uses the cached source if there is one, else decodes a temporary one that is *not*
    /// inserted into the LRU (a batch of hundreds must not evict what the user is editing);
    /// never touches the cached prepared inputs. A missing LUT renders without it.
    pub fn render_detached(
        &self,
        src: &SourceImage,
        adjustments: &ParametricAdjustments,
        max_edge: u32,
        luts: &LutLibrary,
    ) -> AppResult<pipeline::RenderedImage> {
        let entry = match self.cached(src) {
            Some(e) => e,
            None => Arc::new(load_entry(src)?),
        };
        let geo = entry.geometry(adjustments);
        let prepared = source::prepare_geo(&entry.image, src.orientation(), &geo, None, max_edge);
        let lut = match &adjustments.lut {
            Some(l) => luts.load(&l.id).ok().flatten(),
            None => None,
        };
        let profile = entry.profile(&adjustments.profile);
        let tone = entry.tone_context(src.orientation(), adjustments, &geo, &profile);
        let input = entry.input(&prepared, &profile, src.id, pipeline::Quality::Preview, tone.as_ref());
        let (local, _) = self.local_planes(&entry, src, adjustments, &geo, &input, None);
        let mut image = pipeline::render_masked(&input, adjustments, lut.as_deref(), local.as_ref());
        if let Some(c) = &prepared.coverage {
            source::fill_outside_rgb8(&mut image.rgb, c);
        }
        Ok(image)
    }
}

impl DevelopCache {
    /// Seam steps 1-4 (`develop::masks`) for a render of `input` (prepared from `src` with
    /// `adjustments.crop` and `region`): the local planes of `adjustments.masks` on the
    /// input's grid, weights cached across renders. `(None, [])` for unmasked edits.
    fn local_planes(
        &self,
        entry: &Entry,
        src: &SourceImage,
        adjustments: &ParametricAdjustments,
        geo: &Geometry,
        input: &pipeline::RenderInput,
        region: Option<NormRect>,
    ) -> (Option<masks::LocalPlanes>, Vec<DevelopWarning>) {
        if !masks::render::any_rendered(&adjustments.masks) {
            return (None, Vec::new());
        }
        let mut mattes =
            masks::render::ResolvedMattes::resolve(self.config.mask_cache.as_ref(), src.id, &adjustments.masks);
        if mattes.needs_guide() {
            mattes.refine(|| Some(entry.sensor_guide()));
        }
        let geom = masks::MaskGeometry::of(
            geo,
            entry.image.full_width,
            entry.image.full_height,
            src.orientation(),
            region,
            input.width,
            input.height,
        );
        masks::render::local_planes(
            &adjustments.masks,
            &geom,
            src.id,
            &mattes,
            || local::range_guide(input.pixels, input.color, input.profile, adjustments),
            masks::render::guide_key(adjustments),
            Some(&self.inner.weights),
        )
    }

    /// `ai_mask_needs_update` for the image's stored masks (`DevelopInfo.warnings`).
    fn mask_warning(&self, id: ImageId) -> Option<DevelopWarning> {
        let cache = self.config.mask_cache.as_ref()?;
        let conn = masks::open_catalog_read_only(&cache.config().catalog_path).ok()?;
        let adj = crate::db::repo::get_adjustments(&conn, id).ok()?;
        if !masks::render::any_rendered(&adj.masks) {
            return None;
        }
        let mattes = masks::render::ResolvedMattes::resolve(Some(cache), id, &adj.masks);
        masks::render::needs_update_warning(&adj.masks, &mattes)
    }

    /// Blocking. White balance picker: the temperature/tint that makes the 5x5 source-pixel
    /// neighbourhood around `point` (sensor frame: normalized, un-oriented, uncropped) neutral,
    /// through the colour matrices of `adjustments.profile` (same path as `DevelopInfo.asShot`).
    /// `invalid_argument` if the sample is clipped or too dark. Decodes the source if needed.
    pub fn sample_white_balance(
        &self,
        src: &SourceImage,
        point: NormPoint,
        adjustments: &ParametricAdjustments,
    ) -> AppResult<WhiteBalanceValues> {
        let entry = self.entry(src)?;
        self.evict(src.id);
        let mul = sample_multipliers(&entry.image, point)?;
        let profile = entry.profile(&adjustments.profile);
        camera::values_of_multipliers(mul, &entry.image.color, &profile)
            .ok_or_else(|| AppError::invalid("could not measure a white balance at this point"))
    }
}

/// Half-width of the white balance picker's sample window (5x5 source pixels).
pub const WB_SAMPLE_RADIUS: u32 = 2;
/// A sample pixel with any channel at or above this (white = 65535) counts as clipped.
pub const WB_CLIP_LEVEL: u16 = 64_200;
/// Mean channel values below this (about -12 EV) are too dark to measure.
pub const WB_MIN_LEVEL: f64 = 16.0;

/// Multipliers (R, G, B; G = 1) that neutralize the mean of the 5x5 window of `img`
/// (un-oriented source pixels) around `point` (sensor frame). The window is shifted inside
/// the image near edges. Errors (`invalid_argument`): point outside 0..=1, any sample pixel
/// clipped, any channel mean too dark.
pub fn sample_multipliers(img: &LinearImage, point: NormPoint) -> AppResult<[f32; 3]> {
    let inside = |v: f32| v.is_finite() && (0.0..=1.0).contains(&v);
    if !inside(point.x) || !inside(point.y) {
        return Err(AppError::invalid("white balance sample point must be within 0..=1"));
    }
    if img.width == 0 || img.height == 0 || img.pixels.len() < img.width as usize * img.height as usize * 3 {
        return Err(AppError::internal("develop source has no pixels"));
    }
    let span = |norm: f32, size: u32| {
        let size = i64::from(size);
        let r = i64::from(WB_SAMPLE_RADIUS);
        let c = ((f64::from(norm) * size as f64).floor() as i64).clamp(0, size - 1);
        let lo = (c - r).clamp(0, (size - 2 * r - 1).max(0));
        let hi = (lo + 2 * r).min(size - 1);
        (lo as usize, hi as usize)
    };
    let (x0, x1) = span(point.x, img.width);
    let (y0, y1) = span(point.y, img.height);
    let mut sum = [0.0f64; 3];
    let mut n = 0.0f64;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let i = (y * img.width as usize + x) * 3;
            let px = &img.pixels[i..i + 3];
            if px.iter().any(|&v| v >= WB_CLIP_LEVEL) {
                return Err(AppError::invalid(
                    "the sampled area is clipped (overexposed); pick a neutral grey or white that is not blown out",
                ));
            }
            for (s, &v) in sum.iter_mut().zip(px) {
                *s += f64::from(v);
            }
            n += 1.0;
        }
    }
    let mean = sum.map(|s| s / n);
    if mean.iter().any(|&m| m < WB_MIN_LEVEL) {
        return Err(AppError::invalid("the sampled area is too dark to measure; pick a brighter neutral"));
    }
    Ok([(mean[1] / mean[0]) as f32, 1.0, (mean[1] / mean[2]) as f32])
}

/// Result of [`DevelopCache::render_image`].
#[derive(Debug, Clone)]
pub struct RenderedPixels {
    /// 8-bit sRGB output, orientation applied (exactly what the preview JPEG encodes).
    pub image: pipeline::RenderedImage,
    pub lut_missing: bool,
    /// Camera as-shot white balance, `None` if the file has none.
    pub as_shot: Option<crate::ipc::types::WhiteBalanceValues>,
}

/// URL of a render in the webview. `sieve://localhost/...` on macOS/Linux,
/// `http://sieve.localhost/...` on Windows (WebView2 custom-scheme convention).
pub fn render_url(image_id: ImageId, slot: RenderSlot, seq: u32) -> String {
    let base = if cfg!(windows) {
        format!("http://{RENDER_SCHEME}.localhost")
    } else {
        format!("{RENDER_SCHEME}://localhost")
    };
    format!("{base}/render/{image_id}/{}?v={seq}", slot.as_str())
}

fn respond(status: http::StatusCode, content_type: &str, body: Vec<u8>) -> http::Response<Vec<u8>> {
    let mut resp = http::Response::new(body);
    *resp.status_mut() = status;
    let h = resp.headers_mut();
    if let Ok(v) = http::HeaderValue::from_str(content_type) {
        h.insert(http::header::CONTENT_TYPE, v);
    }
    h.insert(http::header::CACHE_CONTROL, http::HeaderValue::from_static("no-store"));
    h.insert(http::header::ACCESS_CONTROL_ALLOW_ORIGIN, http::HeaderValue::from_static("*"));
    resp
}

/// `(image id, slot, min seq)` from `/render/<id>/<slot>` + `?v=<seq>` (missing v = 0).
fn parse_render_path(path: &str, query: Option<&str>) -> Option<(ImageId, RenderSlot, u32)> {
    let mut parts = path.trim_start_matches('/').split('/');
    if parts.next()? != "render" {
        return None;
    }
    let id: ImageId = parts.next()?.parse().ok()?;
    let slot = RenderSlot::parse(parts.next()?)?;
    if parts.next().is_some_and(|p| !p.is_empty()) {
        return None;
    }
    let mut seq = 0;
    for pair in query.unwrap_or("").split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == "v" {
            seq = v.parse().ok()?;
        }
    }
    Some((id, slot, seq))
}

/// Serves `GET /render/<imageId>/<slot>?v=<seq>` from [`DevelopCache::encoded`]:
/// 200 `image/jpeg` with `Cache-Control: no-store` and `Access-Control-Allow-Origin: *`;
/// 404 when nothing with seq >= v is stored; 400 on a malformed path. Runs on a
/// dedicated thread per request (see `lib.rs`); must not panic.
pub fn handle_protocol(cache: &DevelopCache, request: &http::Request<Vec<u8>>) -> http::Response<Vec<u8>> {
    let uri = request.uri();
    if let Some((id, hash, thumb)) = edited::parse_path(uri.path()) {
        return match cache.edited().and_then(|e| e.read(id, hash, thumb)) {
            Some(bytes) => {
                let mut resp = respond(http::StatusCode::OK, "image/jpeg", bytes);
                // Content-addressed (image id + settings hash): never changes.
                resp.headers_mut().insert(
                    http::header::CACHE_CONTROL,
                    http::HeaderValue::from_static("public, max-age=31536000, immutable"),
                );
                resp
            }
            None => respond(http::StatusCode::NOT_FOUND, "text/plain", b"edited preview not available".to_vec()),
        };
    }
    match parse_render_path(uri.path(), uri.query()) {
        None => respond(http::StatusCode::BAD_REQUEST, "text/plain", b"bad render path".to_vec()),
        Some((id, slot, seq)) => match cache.encoded(id, slot, seq) {
            Some(bytes) => respond(http::StatusCode::OK, "image/jpeg", bytes.as_ref().clone()),
            None => respond(http::StatusCode::NOT_FOUND, "text/plain", b"render not available".to_vec()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sony ILCE-7M3 `ColorMatrix2` (XYZ -> camera).
    const A7M3: [[f32; 3]; 3] = [[0.7374, -0.2389, -0.0551], [-0.5435, 1.3162, 0.2519], [-0.1006, 0.1795, 0.6552]];

    /// Uniform `w` x `h` source of camera RGB `rgb` with one pixel overridden.
    fn flat_source(w: u32, h: u32, rgb: [u16; 3], as_shot: [f32; 3], hot: Option<(u32, u32, [u16; 3])>) -> LinearImage {
        let mut pixels: Vec<u16> = (0..w * h).flat_map(|_| rgb).collect();
        if let Some((x, y, v)) = hot {
            let i = ((y * w + x) * 3) as usize;
            pixels[i..i + 3].copy_from_slice(&v);
        }
        LinearImage {
            width: w,
            height: h,
            pixels,
            color: source::ColorInfo {
                as_shot_mul: Some(as_shot),
                daylight_mul: [1.0; 3],
                rgb_cam: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                xyz_to_cam: A7M3,
                calibration: [1.0; 3],
            },
            full_width: w * 2,
            full_height: h * 2,
            display_referred: false,
            source_color: None,
        }
    }

    /// Camera RGB of a grey card lit by the illuminant the multipliers neutralize.
    fn grey_under(mul: [f32; 3], level: f32) -> [u16; 3] {
        mul.map(|m| (level / m).round() as u16)
    }

    #[test]
    fn wb_sample_of_as_shot_grey_returns_as_shot() {
        let as_shot = [2.1, 1.0, 1.6];
        let img = flat_source(40, 30, grey_under(as_shot, 20_000.0), as_shot, None);
        let mul = sample_multipliers(&img, NormPoint { x: 0.5, y: 0.5 }).unwrap();
        for (a, b) in mul.iter().zip(as_shot) {
            assert!((a - b).abs() < 1e-3, "{mul:?}");
        }
        // Matrix path and DCP path both match what DevelopInfo.asShot reports.
        let dcp = camera::Profile { dcp: Some(Arc::new(camera::matrix_dcp(&img.color))), ..Default::default() };
        for profile in [camera::Profile::matrix(0.0), dcp] {
            let want = camera::as_shot_values(&img.color, &profile).unwrap();
            let got = camera::values_of_multipliers(mul, &img.color, &profile).unwrap();
            assert!((got.temperature_k - want.temperature_k).abs() < 5.0, "{got:?} vs {want:?}");
            assert!((got.tint - want.tint).abs() < 0.5, "{got:?} vs {want:?}");
        }
    }

    #[test]
    fn wb_sample_recovers_a_known_cast() {
        // A grey card under 3200 K / +10 on a daylight-as-shot frame.
        let target = WhiteBalanceValues { temperature_k: 3200.0, tint: 10.0 };
        let cast = wb::multipliers_for(target, &A7M3);
        let img = flat_source(20, 20, grey_under(cast, 15_000.0), [2.0, 1.0, 1.4], None);
        // Corners: the window shifts inside the image.
        for p in [NormPoint { x: 0.0, y: 0.0 }, NormPoint { x: 1.0, y: 1.0 }, NormPoint { x: 0.3, y: 0.7 }] {
            let mul = sample_multipliers(&img, p).unwrap();
            let v = camera::values_of_multipliers(mul, &img.color, &camera::Profile::matrix(0.0)).unwrap();
            assert!((v.temperature_k - 3200.0).abs() < 15.0, "{v:?}");
            assert!((v.tint - 10.0).abs() < 1.0, "{v:?}");
        }
    }

    #[test]
    fn wb_sample_rejects_clipped_dark_and_out_of_range() {
        let grey = grey_under([2.0, 1.0, 1.5], 20_000.0);
        let hot = Some((10, 10, [30_000, 65_535, 20_000]));
        let img = flat_source(21, 21, grey, [2.0, 1.0, 1.5], hot);
        let e = sample_multipliers(&img, NormPoint { x: 0.5, y: 0.5 }).unwrap_err();
        assert_eq!(e.kind, crate::ipc::error::ErrorKind::InvalidArgument);
        assert!(e.message.contains("clipped"), "{}", e.message);
        // Away from the clipped pixel (outside its 5x5 window) is fine.
        assert!(sample_multipliers(&img, NormPoint { x: 0.05, y: 0.05 }).is_ok());
        let dark = flat_source(10, 10, [3, 5, 4], [2.0, 1.0, 1.5], None);
        let e = sample_multipliers(&dark, NormPoint { x: 0.5, y: 0.5 }).unwrap_err();
        assert!(e.message.contains("too dark"), "{}", e.message);
        for p in [NormPoint { x: -0.1, y: 0.5 }, NormPoint { x: 0.5, y: 1.2 }, NormPoint { x: f32::NAN, y: 0.5 }] {
            let e = sample_multipliers(&img, p).unwrap_err();
            assert_eq!(e.kind, crate::ipc::error::ErrorKind::InvalidArgument);
        }
    }

    /// End to end through the cache on a display-referred PNG (orientation 6): the point is
    /// in the un-oriented frame; grey neutralizes to ~D65, white is clipped.
    #[test]
    fn wb_sample_through_cache() {
        let (w, h) = (24u32, 12u32);
        let mut px = Vec::new();
        for _y in 0..h {
            for x in 0..w {
                px.extend_from_slice(&if x < w / 2 { [128u8, 128, 128] } else { [255, 255, 255] });
            }
        }
        let bytes = crate::raw::png::test_support::encode(w, h, None, Some(&px), None, None, None);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grey.png");
        std::fs::write(&path, bytes).unwrap();
        let cache = DevelopCache::new(DevelopConfig { cache_bytes: 64 << 20, mask_cache: None });
        let src = SourceImage { id: 1, path, orientation: Some(6) };
        let adj = ParametricAdjustments::defaults_for(crate::ipc::types::ImageFormat::Png);
        let v = cache.sample_white_balance(&src, NormPoint { x: 0.2, y: 0.5 }, &adj).unwrap();
        // sRGB grey = D65 in Adobe's temperature/tint model.
        let (t65, n65) = wb::temp_tint_for(0.3127, 0.3290);
        assert!((f64::from(v.temperature_k) - t65).abs() < 30.0, "{v:?} vs {t65}");
        assert!((f64::from(v.tint) - n65).abs() < 1.0, "{v:?} vs {n65}");
        let e = cache.sample_white_balance(&src, NormPoint { x: 0.8, y: 0.5 }, &adj).unwrap_err();
        assert!(e.message.contains("clipped"), "{}", e.message);
    }

    /// Phase 7c: renders through the cache apply masks in the displayed frame (orientation),
    /// and `info` reports AI components without a matte.
    #[test]
    fn masks_render_through_cache_and_info_reports_missing_mattes() {
        use crate::ipc::types::{
            AiMask, AiMaskOrigin, AiTarget, DevelopWarningCode, ImportOptions, LocalAdjustments, MaskBlendMode,
            MaskComponent, MaskGroup, MaskShape,
        };
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("src");
        std::fs::create_dir_all(&folder).unwrap();
        let (w, h) = (40u32, 20u32);
        let px: Vec<u8> = (0..w * h).flat_map(|_| [100u8, 100, 100]).collect();
        std::fs::write(
            folder.join("g.png"),
            crate::raw::png::test_support::encode(w, h, None, Some(&px), None, None, None),
        )
        .unwrap();
        let catalog = dir.path().join("cat.sqlite");
        let mut conn = crate::db::open(&catalog).unwrap();
        let opts = ImportOptions { recursive: false, include_non_raw: true, pair_jpeg_with_raw: true };
        crate::db::repo::import_folder(&mut conn, &folder, &opts).unwrap();
        let id: ImageId = conn.query_row("SELECT id FROM images", [], |r| r.get(0)).unwrap();
        let mattes = masks::MaskCache::new(masks::MaskCacheConfig {
            catalog_path: catalog.clone(),
            cache_dir: dir.path().join("cache"),
        });
        let cache = DevelopCache::new(DevelopConfig { cache_bytes: 64 << 20, mask_cache: Some(mattes.clone()) });
        let src = SourceImage { id, path: folder.join("g.png"), orientation: Some(6) };
        let digest = "0123456789ABCDEF0123456789ABCDEF".to_owned();
        let mut adj = ParametricAdjustments::defaults_for(crate::ipc::types::ImageFormat::Png);
        adj.masks = vec![MaskGroup {
            id: format!("{:032X}", 1),
            name: "Subject".into(),
            active: true,
            amount: 1.0,
            adjustments: LocalAdjustments { exposure: 1.5, ..Default::default() },
            components: vec![MaskComponent {
                id: format!("{:032X}", 2),
                name: String::new(),
                active: true,
                mode: MaskBlendMode::Add,
                inverted: false,
                opacity: 1.0,
                shape: MaskShape::Ai(AiMask {
                    target: AiTarget::Subject,
                    reference_point: None,
                    digest: Some(digest.clone()),
                }),
            }],
        }];
        crate::develop::history::commit(&mut conn, id, &adj, "Mask").unwrap();
        let luts = LutLibrary::new(PathBuf::from("/nonexistent"));
        let render = |a: &ParametricAdjustments| cache.render_image(&src, a, None, 512, &luts).unwrap().image;
        let plain = ParametricAdjustments { masks: Vec::new(), ..adj.clone() };
        let base = render(&plain);
        assert_eq!((base.width, base.height), (h, w), "orientation 6");
        let warned = |c: &DevelopCache| {
            c.info(&src).unwrap().warnings.iter().any(|w| w.code == DevelopWarningCode::AiMaskNeedsUpdate)
        };
        assert!(warned(&cache), "matte missing");
        assert_eq!(render(&adj).rgb, base.rgb, "missing matte renders empty");

        // Lightroom matte selecting the sensor's left half = the displayed top half (o = 6).
        let matte = masks::AlphaMask {
            width: w,
            height: h,
            bounds: NormRect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 },
            data: (0..w * h).map(|i| if i % w < w / 2 { 255 } else { 0 }).collect(),
        };
        let new = masks::NewMatte {
            kind: "subject".into(),
            target: AiTarget::Subject,
            reference_point: None,
            origin: AiMaskOrigin::Lightroom,
            digest: Some(digest),
            model_version: "lr:1".into(),
            input_digest: None,
        };
        mattes.put(&conn, id, &new, &matte).unwrap();
        assert!(!warned(&cache));
        let img = render(&adj);
        let g = |im: &pipeline::RenderedImage, x: u32, y: u32| im.rgb[((y * im.width + x) * 3 + 1) as usize];
        assert!(g(&img, 10, 5) > g(&base, 10, 5) + 30, "top (masked) brighter");
        assert_eq!(g(&img, 10, 35), g(&base, 10, 35), "bottom unchanged");
    }

    #[test]
    fn tickets_are_latest_wins_per_key() {
        let cache = DevelopCache::new(DevelopConfig { cache_bytes: 0, mask_cache: None });
        let a1 = cache.ticket(1, RenderSlot::Main);
        let b1 = cache.ticket(1, RenderSlot::Before);
        let a2 = cache.ticket(1, RenderSlot::Main);
        assert!(!cache.is_current(a1));
        assert!(cache.is_current(a2));
        assert!(cache.is_current(b1));
        assert!(a2.seq > a1.seq);
        assert!(cache.is_current(cache.ticket(2, RenderSlot::Main)));
    }

    #[test]
    fn render_url_format() {
        let url = render_url(42, RenderSlot::Detail, 7);
        assert!(url.ends_with("/render/42/detail?v=7"), "{url}");
    }

    /// Phase 8 error states: renders of a moved original / an undecodable file fail with
    /// actionable messages (no panic, nothing cached), and the decode lock is released.
    #[test]
    fn missing_and_undecodable_originals_have_actionable_errors() {
        use crate::ipc::error::ErrorKind;
        let dir = tempfile::tempdir().unwrap();
        let cache = DevelopCache::new(DevelopConfig::default());
        let luts = LutLibrary::new(dir.path().join("luts"));
        let opts = RenderOptions { max_edge: 512, slot: RenderSlot::Main, region: None };
        let adj = ParametricAdjustments::default();

        let gone = SourceImage { id: 1, path: dir.path().join("DSC0001.ARW"), orientation: None };
        let e = cache.render(cache.ticket(1, RenderSlot::Main), &gone, &adj, &opts, &luts).unwrap_err();
        assert_eq!(e.kind, ErrorKind::FileMissing);
        assert!(e.message.starts_with(crate::raw::access::MISSING_PREFIX), "{}", e.message);
        assert_eq!(cache.info(&gone).unwrap_err().kind, ErrorKind::FileMissing);

        let junk = dir.path().join("DSC0002.ARW");
        std::fs::write(&junk, b"II*\0not a raw at all").unwrap();
        let bad = SourceImage { id: 2, path: junk, orientation: None };
        let e = cache.render(cache.ticket(2, RenderSlot::Main), &bad, &adj, &opts, &luts).unwrap_err();
        assert_eq!(e.kind, ErrorKind::DecodeFailed);
        assert!(e.message.starts_with("Could not decode") && e.message.contains("damaged"), "{}", e.message);
        assert_eq!(cache.cached_count(), 0);
        assert!(lock(&cache.inner.decoding).is_empty(), "decode locks released on failure");
    }

    /// `remove_project`: everything held for removed images goes; other images stay.
    #[test]
    fn forget_images_drops_everything_held_for_them() {
        let dir = tempfile::tempdir().unwrap();
        let luts = LutLibrary::new(dir.path().join("luts"));
        let jpeg = crate::raw::turbo::encode_rgb_444(&[120u8; 64 * 48 * 3], 64, 48, 90).unwrap();
        let mattes = masks::MaskCache::new(masks::MaskCacheConfig {
            catalog_path: dir.path().join("cat.sqlite"),
            cache_dir: dir.path().join("cache"),
        });
        let cache = DevelopCache::new(DevelopConfig { cache_bytes: 64 << 20, mask_cache: Some(mattes.clone()) });
        let opts = RenderOptions { max_edge: 64, slot: RenderSlot::Main, region: None };
        let adj = ParametricAdjustments::defaults_for(crate::ipc::types::ImageFormat::Jpeg);
        for id in [1, 2] {
            let path = dir.path().join(format!("{id}.jpg"));
            std::fs::write(&path, &jpeg).unwrap();
            let src = SourceImage { id, path, orientation: None };
            cache.remember_source(src.clone());
            let t = cache.ticket(id, RenderSlot::Main);
            cache.render(t, &src, &adj, &opts, &luts).unwrap().unwrap();
            std::fs::create_dir_all(dir.path().join(format!("cache/masks/{id}"))).unwrap();
        }
        assert_eq!(cache.cached_count(), 2);
        cache.forget_images(&[1]);
        assert_eq!(cache.cached_count(), 1);
        assert!(cache.source(1).is_none() && cache.source(2).is_some());
        assert!(cache.encoded(1, RenderSlot::Main, 0).is_none());
        assert!(cache.encoded(2, RenderSlot::Main, 0).is_some());
        assert!(!lock(&cache.latest).keys().any(|(id, _)| *id == 1));
        assert!(!dir.path().join("cache/masks/1").exists() && dir.path().join("cache/masks/2").exists());
    }

    #[test]
    fn remembered_sources_are_bounded_and_forgotten() {
        let cache = DevelopCache::new(DevelopConfig::default());
        let src = |id: ImageId| SourceImage { id, path: PathBuf::from(format!("/x/{id}.arw")), orientation: Some(6) };
        assert_eq!(cache.source(1), None);
        cache.remember_source(src(1));
        cache.remember_source(src(2));
        assert_eq!(cache.source(1), Some(src(1)));
        cache.forget_sources(Some(&[1]));
        assert_eq!((cache.source(1), cache.source(2)), (None, Some(src(2))));
        cache.forget_sources(None);
        assert_eq!(cache.source(2), None);
        for id in 0..(MAX_SOURCES as i64 + 10) {
            cache.remember_source(src(id));
        }
        assert!(lock(&cache.sources).len() <= MAX_SOURCES);
    }

    fn get(cache: &DevelopCache, uri: &str) -> http::Response<Vec<u8>> {
        let req = http::Request::builder().uri(uri).body(Vec::new()).unwrap();
        handle_protocol(cache, &req)
    }

    #[test]
    fn protocol_serves_newest_encoded_render() {
        let cache = DevelopCache::new(DevelopConfig { cache_bytes: 0, mask_cache: None });
        assert_eq!(get(&cache, "sieve://localhost/render/5/main?v=1").status(), 404);
        cache.store_encoded((5, RenderSlot::Main), 3, vec![0xFF, 0xD8, 1]);
        let r = get(&cache, "sieve://localhost/render/5/main?v=3");
        assert_eq!(r.status(), 200);
        assert_eq!(r.body(), &vec![0xFF, 0xD8, 1]);
        assert_eq!(r.headers()["content-type"], "image/jpeg");
        assert_eq!(r.headers()["cache-control"], "no-store");
        assert_eq!(r.headers()["access-control-allow-origin"], "*");
        assert_eq!(get(&cache, "http://sieve.localhost/render/5/main?v=2").status(), 200, "older v is fine");
        assert_eq!(get(&cache, "sieve://localhost/render/5/main?v=4").status(), 404, "newer not rendered yet");
        assert_eq!(get(&cache, "sieve://localhost/render/5/before?v=1").status(), 404);
        // An older render finishing late never replaces a newer one.
        cache.store_encoded((5, RenderSlot::Main), 2, vec![9]);
        assert_eq!(get(&cache, "sieve://localhost/render/5/main").body(), &vec![0xFF, 0xD8, 1]);
        for bad in [
            "sieve://localhost/x/5/main",
            "sieve://localhost/render/abc/main",
            "sieve://localhost/render/5/side",
            "sieve://localhost/render/5/main?v=x",
            "sieve://localhost/render/5/main/extra",
        ] {
            assert_eq!(get(&cache, bad).status(), 400, "{bad}");
        }
        // Bounded.
        for i in 0..(MAX_ENCODED as i64 + 5) {
            cache.store_encoded((100 + i, RenderSlot::Main), 1, vec![1]);
        }
        assert!(cache.encoded(5, RenderSlot::Main, 0).is_none());
        assert!(cache.encoded(100 + MAX_ENCODED as i64 + 4, RenderSlot::Main, 0).is_some());
    }

    #[test]
    fn missing_file_is_an_error_not_a_panic() {
        let cache = DevelopCache::new(DevelopConfig { cache_bytes: 1 << 30, mask_cache: None });
        let src = SourceImage { id: 1, path: PathBuf::from("/nonexistent/x.arw"), orientation: None };
        assert!(cache.info(&src).is_err());
        let t = cache.ticket(1, RenderSlot::Main);
        let opts = RenderOptions { max_edge: 512, slot: RenderSlot::Main, region: None };
        let luts = LutLibrary::new(PathBuf::from("/nonexistent"));
        assert!(cache.render(t, &src, &ParametricAdjustments::default(), &opts, &luts).is_err());
        cache.prefetch(vec![src]);
    }

    fn sample_raws(n: usize) -> Vec<PathBuf> {
        let folder = std::env::var("SIEVE_SAMPLES").unwrap_or_else(|_| "/Users/gurjotsingh/Pictures/test RAWS".into());
        let mut v: Vec<PathBuf> = std::fs::read_dir(folder)
            .map(|d| {
                d.filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| crate::raw::format_from_extension(p).is_some())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v.truncate(n);
        v
    }

    /// Real RAWs (read-only): decode, info, renders, latest-wins, LRU bound.
    #[test]
    #[ignore = "needs sample RAWs ($SIEVE_SAMPLES)"]
    fn real_sample_render() {
        let raws = sample_raws(3);
        assert!(!raws.is_empty());
        let cache = DevelopCache::new(DevelopConfig { cache_bytes: 150 << 20, mask_cache: None });
        let luts = LutLibrary::new(PathBuf::from("/nonexistent"));
        for (i, path) in raws.iter().enumerate() {
            let src = SourceImage { id: i as i64 + 1, path: path.clone(), orientation: Some(8) };
            let info = cache.info(&src).unwrap();
            assert!(info.source_height > info.source_width, "orientation 8 swaps: {info:?}");
            // Half-size for Bayer data; Sony M/S-size RAWs are not CFA data (source = full).
            assert!(info.full_height >= info.source_height);
            let wb = info.as_shot.expect("as-shot WB");
            assert!((2500.0..9000.0).contains(&wb.temperature_k), "{wb:?}");
            let t = cache.ticket(src.id, RenderSlot::Main);
            let opts = RenderOptions { max_edge: 1024, slot: RenderSlot::Main, region: None };
            let adj = ParametricAdjustments {
                lut: Some(crate::ipc::types::LutRef { id: "gone".into(), amount: 50.0 }),
                ..Default::default()
            };
            let r = cache.render(t, &src, &adj, &opts, &luts).unwrap().unwrap();
            assert_eq!(r.width.max(r.height), 1024);
            assert!(r.height > r.width);
            assert!(r.lut_missing);
            assert_eq!(r.histogram.luma.iter().sum::<u32>(), r.width * r.height);
            let jpeg = cache.encoded(src.id, RenderSlot::Main, r.seq).unwrap();
            assert_eq!(crate::raw::turbo::dimensions(&jpeg).unwrap(), (r.width, r.height));
            // Superseded ticket renders nothing.
            let old = cache.ticket(src.id, RenderSlot::Main);
            let _new = cache.ticket(src.id, RenderSlot::Main);
            assert!(cache.render(old, &src, &adj, &opts, &luts).unwrap().is_none());
            // Region render.
            let region = NormRect { x: 0.25, y: 0.25, width: 0.25, height: 0.25 };
            let t = cache.ticket(src.id, RenderSlot::Detail);
            let d = cache
                .render(
                    t,
                    &src,
                    &adj,
                    &RenderOptions { max_edge: 4096, slot: RenderSlot::Detail, region: Some(region) },
                    &luts,
                )
                .unwrap()
                .unwrap();
            assert_eq!((d.width, d.height), (info.source_width / 4, info.source_height / 4));
            assert!(cache.cached_bytes() <= 150 << 20 || cache.cached_count() == 1);
        }
    }
}
