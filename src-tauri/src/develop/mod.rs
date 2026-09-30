//! Develop engine (Phase 5): preview rendering from a cached half-size linear RAW decode,
//! edit history, presets. Owned by rust-engine-dev.
//!
//! The architect fixed only the surface used by `ipc::commands` and `lib.rs`:
//! [`DevelopConfig`], [`SourceImage`], [`DevelopCache`] (`new`, `config`, `ticket`,
//! `render`, `info`, `prefetch`, `encoded`), [`render_url`], [`handle_protocol`],
//! [`RENDER_SCHEME`], and the SQL seams [`history`] / [`presets`]. The compute split
//! ([`source`], [`pipeline`], [`wb`]) is a suggestion; internals are free to change.
//!
//! Contract (details in `docs/architecture.md`, "Editor"):
//! - Source: LibRaw `half_size = 1` decode of the RAW (one pixel per 2x2 Bayer quad, or the
//!   X-Trans equivalent), *unscaled camera RGB, no white balance, linear, 16-bit*, plus the
//!   camera's as-shot multipliers, black/white levels and camera->XYZ matrix. Cached per
//!   image in an LRU bounded by bytes (default 1 GiB, `LUMENRAW_DEVELOP_CACHE_MB`), with a
//!   working-size f32 downsample kept alongside for the common loupe size. First decode
//!   ~300-800 ms; cached renders must meet the < 100 ms slider budget at 2048 px.
//! - Pipeline order: WB multipliers (AsShot = camera; Custom = temperature/tint -> xy ->
//!   camera neutral via the colour matrix) -> camera -> linear working space (Rec.2020 f32)
//!   -> exposure -> highlights/shadows/whites/blacks + contrast tone mapping -> texture /
//!   clarity / dehaze -> vibrance/saturation -> HSL (8 bands, Lightroom hue centres) ->
//!   sRGB-encode -> LUT (on encoded values, `amount` blend) -> 8-bit -> histogram -> JPEG.
//!   Orientation applied; `region` crops before resampling. rayon across tiles/rows.
//! - Latest-wins: [`DevelopCache::ticket`] is taken on the async side in arrival order;
//!   [`DevelopCache::render`] returns `Ok(None)` without rendering (or after, discarding the
//!   result) when a newer ticket exists for the same (image, slot). At most one render per
//!   key runs at a time. The encoded JPEG of the newest finished render per key is kept in
//!   memory and served by [`handle_protocol`].

pub mod history;
pub mod pipeline;
pub mod presets;
pub mod source;
pub mod wb;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tauri::http;

use crate::ipc::error::AppResult;
use crate::ipc::types::{DevelopInfo, ImageId, ParametricAdjustments, RenderOptions, RenderSlot, RenderedPreview};
use crate::lut::LutLibrary;

/// Custom URI scheme serving rendered previews (registered in `lib.rs`).
pub const RENDER_SCHEME: &str = "lumen";

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

/// Claim to render the newest state of one (image, slot) stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderTicket {
    pub image_id: ImageId,
    pub slot: RenderSlot,
    pub seq: u32,
}

/// Managed Tauri state: decoded-source LRU, latest-wins bookkeeping and the last encoded
/// render per (image, slot). Cheap to clone.
#[derive(Clone)]
pub struct DevelopCache {
    config: DevelopConfig,
    /// Newest ticket issued per (image, slot).
    latest: Arc<Mutex<HashMap<(ImageId, RenderSlot), u32>>>,
}

impl DevelopCache {
    pub fn new(config: DevelopConfig) -> Self {
        Self { config, latest: Arc::new(Mutex::new(HashMap::new())) }
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
        let _ = (ticket, src, adjustments, options, luts);
        todo!("rust-engine-dev: DevelopCache::render")
    }

    /// Blocking. As-shot WB and sizes; decodes (and caches) the source if needed.
    pub fn info(&self, src: &SourceImage) -> AppResult<DevelopInfo> {
        let _ = src;
        todo!("rust-engine-dev: DevelopCache::info")
    }

    /// Warms the cache for `sources` in the background (e.g. filmstrip neighbours) and
    /// returns immediately. Lower priority than `render`; must never panic.
    pub fn prefetch(&self, sources: Vec<SourceImage>) {
        let _ = sources;
        // rust-engine-dev: background decode. No-op until implemented (safe to call).
    }

    /// Encoded JPEG of the newest finished render for (image, slot) if its seq >= `min_seq`.
    pub fn encoded(&self, image_id: ImageId, slot: RenderSlot, min_seq: u32) -> Option<Arc<Vec<u8>>> {
        let _ = (image_id, slot, min_seq);
        todo!("rust-engine-dev: DevelopCache::encoded")
    }
}

/// URL of a render in the webview. `lumen://localhost/...` on macOS/Linux,
/// `http://lumen.localhost/...` on Windows (WebView2 custom-scheme convention).
pub fn render_url(image_id: ImageId, slot: RenderSlot, seq: u32) -> String {
    let base = if cfg!(windows) {
        format!("http://{RENDER_SCHEME}.localhost")
    } else {
        format!("{RENDER_SCHEME}://localhost")
    };
    format!("{base}/render/{image_id}/{}?v={seq}", slot.as_str())
}

/// Serves `GET /render/<imageId>/<slot>?v=<seq>` from [`DevelopCache::encoded`]:
/// 200 `image/jpeg` with `Cache-Control: no-store` and `Access-Control-Allow-Origin: *`;
/// 404 when nothing with seq >= v is stored; 400 on a malformed path. Runs on a
/// dedicated thread per request (see `lib.rs`); must not panic.
pub fn handle_protocol(cache: &DevelopCache, request: &http::Request<Vec<u8>>) -> http::Response<Vec<u8>> {
    let _ = (cache, request);
    todo!("rust-engine-dev: handle_protocol")
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
}
