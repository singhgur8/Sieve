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

pub mod camera;
pub mod history;
mod param_data;
pub mod masks;
pub mod parity;
pub mod pipeline;
pub mod presets;
pub mod source;
pub mod tone;
mod tone_data;
pub mod wb;

use std::collections::{HashMap, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use tauri::http;

use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::{
    CropSettings, DevelopInfo, DevelopWarning, DevelopWarningCode, ImageId, NormRect, ParametricAdjustments,
    ProfileSettings, RenderOptions, RenderSlot, RenderedPreview,
};
use crate::lut::LutLibrary;
use crate::profiles::ProfileLibrary;

use camera::Profile;
use source::{LinearImage, Prepared, SourceMeta};

/// Custom URI scheme serving rendered previews (registered in `lib.rs`).
pub const RENDER_SCHEME: &str = "sieve";
/// JPEG quality of served previews (4:4:4).
pub const JPEG_QUALITY: u8 = 90;
/// Prepared (cropped/resampled) inputs kept per cached image.
pub const PREPARED_PER_IMAGE: usize = 3;
/// Encoded renders kept for the protocol handler (oldest dropped first).
pub const MAX_ENCODED: usize = 24;

/// Resolved at startup by `lib.rs`.
#[derive(Debug, Clone)]
pub struct DevelopConfig {
    /// Upper bound of decoded sources + working copies kept in memory.
    pub cache_bytes: u64,
}

impl DevelopConfig {
    pub const DEFAULT_CACHE_MB: u64 = 1024;
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
    /// Crop (enabled flag + rect + angle bits); `None` = whole frame.
    crop: Option<[u32; 5]>,
}

impl PrepKey {
    fn new(orientation: u8, max_edge: u32, region: Option<NormRect>, crop: &CropSettings) -> Self {
        let region = region.map(|r| [r.x.to_bits(), r.y.to_bits(), r.width.to_bits(), r.height.to_bits()]);
        let crop = crop.enabled.then(|| [crop.top, crop.left, crop.bottom, crop.right, crop.angle].map(f32::to_bits));
        PrepKey { orientation, max_edge, region, crop }
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
}

impl Entry {
    fn bytes(&self) -> u64 {
        let prepared: usize = lock(&self.prepared).iter().map(|(_, p)| p.bytes()).sum();
        (self.image.bytes() + prepared) as u64
    }

    /// Prepared input for the key (computed and cached if needed).
    fn prepared(&self, orientation: u8, crop: &CropSettings, region: Option<NormRect>, max_edge: u32) -> Arc<Prepared> {
        let key = PrepKey::new(orientation, max_edge, region, crop);
        {
            let mut list = lock(&self.prepared);
            if let Some(i) = list.iter().position(|(k, _)| *k == key) {
                let hit = list.remove(i);
                let p = hit.1.clone();
                list.push(hit);
                return p;
            }
        }
        let p = Arc::new(source::prepare(&self.image, orientation, crop, region, max_edge));
        let mut list = lock(&self.prepared);
        if list.len() >= PREPARED_PER_IMAGE {
            list.remove(0);
        }
        list.push((key, p.clone()));
        p
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

    fn input<'a>(
        &'a self,
        prepared: &'a Prepared,
        profile: &'a Profile,
        seed: ImageId,
        quality: pipeline::Quality,
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
        }
    }
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
    latest: Arc<Mutex<HashMap<(ImageId, RenderSlot), u32>>>,
    inner: Arc<Inner>,
}

impl DevelopCache {
    pub fn new(config: DevelopConfig) -> Self {
        // Scan the installed Adobe profiles in the background so the first render does not
        // wait for it.
        let _ = std::thread::Builder::new().name("profile-scan".into()).spawn(|| ProfileLibrary::shared().warm());
        Self {
            config,
            latest: Arc::new(Mutex::new(HashMap::new())),
            inner: Arc::new(Inner {
                lru: Mutex::new(Lru::default()),
                decoding: Mutex::new(HashMap::new()),
                rendering: Mutex::new(HashMap::new()),
                encoded: Mutex::new((HashMap::new(), VecDeque::new())),
                prefetch: Mutex::new(Prefetch::default()),
            }),
        }
    }

    pub fn config(&self) -> &DevelopConfig {
        &self.config
    }

    /// Issues the next ticket for (image, slot). Call on the async side, before any
    /// blocking work, so tickets follow request arrival order.
    pub fn ticket(&self, image_id: ImageId, slot: RenderSlot) -> RenderTicket {
        let mut latest = self.latest.lock().unwrap_or_else(|e| e.into_inner());
        let seq = latest.entry((image_id, slot)).or_insert(0);
        *seq = seq.wrapping_add(1);
        RenderTicket { image_id, slot, seq: *seq }
    }

    /// `ticket` is still the newest for its (image, slot).
    pub fn is_current(&self, ticket: RenderTicket) -> bool {
        let latest = self.latest.lock().unwrap_or_else(|e| e.into_inner());
        latest.get(&(ticket.image_id, ticket.slot)) == Some(&ticket.seq)
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
        let (image, meta) = source::decode_half_size_meta(&src.path)?;
        let entry = Arc::new(Entry {
            path: src.path.clone(),
            image,
            meta,
            prepared: Mutex::new(Vec::new()),
            profile: Mutex::new(None),
        });
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
        let key = (ticket.image_id, ticket.slot);
        let render_lock = lock(&self.inner.rendering).entry(key).or_default().clone();
        let _guard = lock(&render_lock);
        if !self.is_current(ticket) {
            return Ok(None);
        }
        let entry = self.entry(src)?;
        let prepared = entry.prepared(src.orientation(), &adjustments.crop, options.region, options.max_edge);
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
        let input = entry.input(&prepared, &profile, src.id, quality_for(options.max_edge));
        let img = pipeline::render(&input, adjustments, lut.as_deref());
        if !self.is_current(ticket) {
            return Ok(None);
        }
        let jpeg = crate::raw::turbo::encode_rgb_444(&img.rgb, img.width, img.height, JPEG_QUALITY)
            .map_err(AppError::internal)?;
        drop(img.rgb);
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
        Ok(DevelopInfo { image_id: src.id, as_shot, source_width, source_height, full_width, full_height, warnings })
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
                match catch_unwind(AssertUnwindSafe(|| this.entry(&next))) {
                    Ok(Err(e)) => eprintln!("develop prefetch {}: {}", next.path.display(), e.message),
                    Err(_) => eprintln!("develop prefetch {}: panicked", next.path.display()),
                    Ok(Ok(_)) => {}
                }
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
        let prepared = entry.prepared(src.orientation(), &adjustments.crop, region, max_edge);
        self.evict(src.id);
        let lut = match &adjustments.lut {
            Some(l) => luts.load(&l.id)?,
            None => None,
        };
        let lut_missing = adjustments.lut.is_some() && lut.is_none();
        let profile = entry.profile(&adjustments.profile);
        let input = entry.input(&prepared, &profile, src.id, quality_for(max_edge));
        let image = pipeline::render(&input, adjustments, lut.as_deref());
        let as_shot = camera::as_shot_values(&entry.image.color, &profile);
        Ok(RenderedPixels { image, lut_missing, as_shot })
    }
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

    #[test]
    fn tickets_are_latest_wins_per_key() {
        let cache = DevelopCache::new(DevelopConfig { cache_bytes: 0 });
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

    fn get(cache: &DevelopCache, uri: &str) -> http::Response<Vec<u8>> {
        let req = http::Request::builder().uri(uri).body(Vec::new()).unwrap();
        handle_protocol(cache, &req)
    }

    #[test]
    fn protocol_serves_newest_encoded_render() {
        let cache = DevelopCache::new(DevelopConfig { cache_bytes: 0 });
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
        let cache = DevelopCache::new(DevelopConfig { cache_bytes: 1 << 30 });
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
        let cache = DevelopCache::new(DevelopConfig { cache_bytes: 150 << 20 });
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
