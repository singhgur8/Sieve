//! Mask evaluation for a render (Phase 7c pipeline integration, seam steps 1-4): resolves AI
//! mattes once per render (refining Sieve mattes against the image), evaluates the group
//! weights on the render grid with a small cache keyed by (image, geometry, components,
//! mattes, guide), and builds the [`LocalPlanes`] the pipeline consumes. Used by the preview
//! (`DevelopCache`) and the export (`export::develop`).

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use super::{cache, evaluate, needs_range_guide, AlphaMask, LocalPlanes, MaskCache, MaskGeometry, MatteSource};
use super::{GroupWeights, RangeGuide};
use crate::ipc::types::{
    AiMask, AiMaskOrigin, DevelopWarning, DevelopWarningCode, ImageId, LocalAdjustments, MaskGroup, MaskShape, NormRect,
};
use crate::ml::refine::{self, RefineParams};

/// A group contributes to the render (active, non-zero amount, components, non-neutral).
fn renders(g: &MaskGroup) -> bool {
    g.active && g.amount > 0.0 && g.components.iter().any(|c| c.active) && g.adjustments != LocalAdjustments::default()
}

/// Whether `masks` changes a render at all (cheap; no I/O).
pub fn any_rendered(masks: &[MaskGroup]) -> bool {
    masks.iter().any(renders)
}

/// Active AI components of rendered groups.
fn ai_components(masks: &[MaskGroup]) -> impl Iterator<Item = &AiMask> {
    masks.iter().filter(|g| renders(g)).flat_map(|g| &g.components).filter(|c| c.active).filter_map(|c| {
        match &c.shape {
            MaskShape::Ai(ai) => Some(ai),
            _ => None,
        }
    })
}

/// Resolution key of an AI component: its digest, else its Sieve cache kind.
fn ai_key(ai: &AiMask) -> String {
    match &ai.digest {
        Some(d) => format!("#{}", d.trim().to_ascii_uppercase()),
        None => ai.cache_kind(),
    }
}

/// Sensor-frame render of the whole image (sRGB8, un-oriented, uncropped): the guide Sieve
/// mattes are refined against (`ml::refine`).
pub struct SensorGuide {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// Refined Sieve mattes (process-wide, small): (image, digest) -> matte at guide density.
type RefinedLru = Mutex<VecDeque<((ImageId, String), Arc<AlphaMask>)>>;

fn refined_lru() -> &'static RefinedLru {
    static L: OnceLock<RefinedLru> = OnceLock::new();
    L.get_or_init(|| Mutex::new(VecDeque::new()))
}
const REFINED_KEEP: usize = 12;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The mattes of one render, resolved and loaded up front (so evaluation does no I/O).
pub struct ResolvedMattes {
    /// `ai_key` -> (digest, matte); `None` = no matte (renders empty, `needs_update`).
    mattes: HashMap<String, Option<(String, Arc<AlphaMask>)>>,
    /// Sieve mattes to refine: key -> (digest, params).
    to_refine: Vec<(String, String, RefineParams)>,
    image_id: ImageId,
}

impl ResolvedMattes {
    /// No mattes (AI components render empty).
    pub fn none(image_id: ImageId) -> Self {
        ResolvedMattes { mattes: HashMap::new(), to_refine: Vec::new(), image_id }
    }

    /// Resolves every AI component of the rendered groups of `masks` (digest -> that row;
    /// else the newest Sieve matte of its kind, any model version) and loads the bitmaps.
    pub fn resolve(cache: Option<&MaskCache>, image_id: ImageId, masks: &[MaskGroup]) -> Self {
        let mut out = Self::none(image_id);
        let keys: Vec<(String, AiMask)> = ai_components(masks).map(|ai| (ai_key(ai), ai.clone())).collect();
        if keys.is_empty() {
            return out;
        }
        let Some(cache) = cache else {
            out.mattes = keys.into_iter().map(|(k, _)| (k, None)).collect();
            return out;
        };
        let conn = cache::open_read_only(&cache.config().catalog_path).ok();
        for (key, ai) in keys {
            if out.mattes.contains_key(&key) {
                continue;
            }
            // Digest -> that matte; none (or not cached) -> the newest Sieve matte of the kind
            // (as `MaskCache::status` reports it).
            let row = conn.as_ref().and_then(|c| {
                let by_digest =
                    ai.digest.as_ref().and_then(|d| cache::by_digest(c, image_id, &d.trim().to_ascii_uppercase()).ok());
                by_digest.flatten().or_else(|| cache::newest_sieve(c, image_id, &ai.cache_kind(), None).ok().flatten())
            });
            let loaded = row.as_ref().and_then(|r| {
                let m = cache.load(image_id, &r.digest).ok().flatten()?;
                if r.origin == AiMaskOrigin::Sieve {
                    if let Some(p) = RefineParams::for_kind(&r.kind) {
                        out.to_refine.push((key.clone(), r.digest.clone(), p));
                    }
                }
                Some((r.digest.clone(), m))
            });
            out.mattes.insert(key, loaded);
        }
        out
    }

    /// AI components without a matte.
    pub fn missing(&self, masks: &[MaskGroup]) -> usize {
        ai_components(masks).filter(|ai| !matches!(self.mattes.get(&ai_key(ai)), Some(Some(_)))).count()
    }

    /// Some Sieve matte still needs refining (call [`Self::refine`] with a guide).
    pub fn needs_guide(&self) -> bool {
        let lru = lock(refined_lru());
        self.to_refine.iter().any(|(_, d, _)| !lru.iter().any(|(k, _)| k.0 == self.image_id && &k.1 == d))
    }

    /// Replaces Sieve mattes by their refined versions (guided filter against `guide`, or
    /// the process cache of earlier refinements). `guide` is only called on a cache miss.
    pub fn refine(&mut self, guide: impl FnOnce() -> Option<SensorGuide>) {
        if self.to_refine.is_empty() {
            return;
        }
        let mut guide = Some(guide);
        let mut made: Option<Option<SensorGuide>> = None;
        for (key, digest, params) in std::mem::take(&mut self.to_refine) {
            let id = (self.image_id, digest.clone());
            let hit = lock(refined_lru()).iter().find(|(k, _)| *k == id).map(|(_, m)| m.clone());
            let refined = match hit {
                Some(m) => Some(m),
                None => {
                    let g = made.get_or_insert_with(|| guide.take().and_then(|f| f()));
                    let Some(g) = g.as_ref() else { continue };
                    let Some(Some((_, raw))) = self.mattes.get(&key) else { continue };
                    let full = NormRect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 };
                    let guide = refine::Guide { width: g.width, height: g.height, rgb: &g.rgb, region: full };
                    let m = Arc::new(refine::refine(raw, &guide, params));
                    let mut lru = lock(refined_lru());
                    lru.retain(|(k, _)| *k != id);
                    lru.push_back((id, m.clone()));
                    while lru.len() > REFINED_KEEP {
                        lru.pop_front();
                    }
                    Some(m)
                }
            };
            if let Some(m) = refined {
                self.mattes.insert(key, Some((digest, m)));
            }
        }
    }

    /// Identity of the resolved mattes (weight cache key).
    pub fn signature(&self) -> u64 {
        let mut v: Vec<(&String, Option<&String>)> =
            self.mattes.iter().map(|(k, m)| (k, m.as_ref().map(|(d, _)| d))).collect();
        v.sort();
        let mut h = DefaultHasher::new();
        v.hash(&mut h);
        h.finish()
    }
}

impl MatteSource for ResolvedMattes {
    fn matte(&self, _: ImageId, ai: &AiMask) -> Option<Arc<AlphaMask>> {
        self.mattes.get(&ai_key(ai)).and_then(|m| m.as_ref()).map(|(_, m)| m.clone())
    }
}

/// Weight cache key.
#[derive(Debug, Clone, PartialEq)]
struct WeightKey {
    image_id: ImageId,
    geom: MaskGeometry,
    components: u64,
    mattes: u64,
    guide: Option<u64>,
}

/// Evaluated group weights (shared planes) + evaluation warnings.
pub struct CachedWeights {
    pub groups: Vec<Option<Arc<Vec<f32>>>>,
    pub warnings: Vec<DevelopWarning>,
}

/// Last few evaluated weights (local slider drags only rebuild the planes).
#[derive(Default)]
pub struct WeightCache {
    entries: Mutex<VecDeque<(WeightKey, Arc<CachedWeights>)>>,
}

/// Entries kept (main + compare slots, a draft and a full size).
const WEIGHTS_KEEP: usize = 4;

/// Hash of what the weights depend on in `masks` (components, activity, amount; not the
/// local slider values).
fn components_hash(masks: &[MaskGroup]) -> u64 {
    let mut h = DefaultHasher::new();
    for g in masks {
        let r = renders(g);
        r.hash(&mut h);
        if !r {
            continue;
        }
        g.id.hash(&mut h);
        g.amount.to_bits().hash(&mut h);
        serde_json::to_string(&g.components).unwrap_or_default().hash(&mut h);
    }
    h.finish()
}

/// Seam steps 2-4 for one render: evaluates (or reuses) the group weights of `masks` on
/// `geom` and builds the local planes. `guide` makes the range guide (Lab on the render grid)
/// when a range mask / auto-mask needs it; `guide_key` identifies its inputs (the global
/// settings). Returns the planes (`None` = nothing local) and the evaluation warnings
/// (`masks_unsupported`, `ai_mask_needs_update`).
pub fn local_planes(
    masks: &[MaskGroup],
    geom: &MaskGeometry,
    image_id: ImageId,
    mattes: &ResolvedMattes,
    guide: impl FnOnce() -> Vec<[f32; 3]>,
    guide_key: u64,
    cache: Option<&WeightCache>,
) -> (Option<LocalPlanes>, Vec<DevelopWarning>) {
    if !any_rendered(masks) {
        return (None, Vec::new());
    }
    let wants_guide = needs_range_guide(masks);
    let key = WeightKey {
        image_id,
        geom: geom.clone(),
        components: components_hash(masks),
        mattes: mattes.signature(),
        guide: wants_guide.then_some(guide_key),
    };
    let hit = cache.and_then(|c| lock(&c.entries).iter().find(|(k, _)| *k == key).map(|(_, w)| w.clone()));
    let weights = match hit {
        Some(w) => w,
        None => {
            let lab = wants_guide.then(guide);
            let range = lab.as_deref().map(|lab| RangeGuide { width: geom.width, height: geom.height, lab });
            let rendered: Vec<MaskGroup> = masks
                .iter()
                .map(|g| if renders(g) { g.clone() } else { MaskGroup { active: false, ..g.clone() } })
                .collect();
            let GroupWeights { groups, warnings, .. } = evaluate(&rendered, geom, image_id, mattes, range.as_ref());
            let w = Arc::new(CachedWeights { groups: groups.into_iter().map(|g| g.map(Arc::new)).collect(), warnings });
            if let Some(c) = cache {
                let mut e = lock(&c.entries);
                e.retain(|(k, _)| *k != key);
                e.push_back((key, w.clone()));
                while e.len() > WEIGHTS_KEEP {
                    e.pop_front();
                }
            }
            w
        }
    };
    let planes = LocalPlanes::from_shared(masks, &weights.groups, geom.width, geom.height);
    (planes, weights.warnings.clone())
}

/// `ai_mask_needs_update` for `masks` given resolved mattes (`DevelopInfo` warnings).
pub fn needs_update_warning(masks: &[MaskGroup], mattes: &ResolvedMattes) -> Option<DevelopWarning> {
    let n = mattes.missing(masks);
    (n > 0).then(|| DevelopWarning { code: DevelopWarningCode::AiMaskNeedsUpdate, detail: Some(n.to_string()) })
}

/// Hash of the global settings a range guide depends on.
pub fn guide_key(adj: &crate::ipc::types::ParametricAdjustments) -> u64 {
    let mut h = DefaultHasher::new();
    serde_json::to_string(&adj.white_balance).unwrap_or_default().hash(&mut h);
    serde_json::to_string(&adj.profile).unwrap_or_default().hash(&mut h);
    serde_json::to_string(&adj.calibration).unwrap_or_default().hash(&mut h);
    adj.exposure.to_bits().hash(&mut h);
    h.finish()
}

/// Export (and any render that must not show empty AI masks): computes the mattes of AI
/// components that have none with `segmenter` (blocking; cached like `compute_ai_mask`).
/// Returns per-component errors (unavailable model, decode failure); those render empty.
pub fn compute_missing(
    segmenter: &crate::ml::masking::Segmenter,
    src: &crate::develop::SourceImage,
    preview_path: Option<&std::path::Path>,
    masks: &[MaskGroup],
) -> Vec<String> {
    let resolved = ResolvedMattes::resolve(Some(segmenter.mattes()), src.id, masks);
    let mut done: Vec<String> = Vec::new();
    let mut errors = Vec::new();
    for ai in ai_components(masks) {
        if matches!(resolved.mattes.get(&ai_key(ai)), Some(Some(_))) {
            continue;
        }
        let kind = ai.cache_kind();
        if done.contains(&kind) {
            continue;
        }
        done.push(kind.clone());
        let request = crate::ipc::types::AiMaskRequest {
            target: ai.target.clone(),
            reference_point: ai.reference_point,
            force: false,
        };
        if let Err(e) = segmenter.compute(src, preview_path, &request) {
            errors.push(format!("{kind}: {}", e.message));
        }
    }
    errors
}
