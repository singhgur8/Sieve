//! On-demand face boxes for Auto tone on photos the analysis has not reached yet.
//!
//! `develop::auto::auto_tone` protects skin with the analysis' face boxes; without them it
//! falls back to a bounded skin-colour estimate, which protects skin less. [`AutoFaces`]
//! closes that gap: it runs the culling SCRFD detector (same model, same input size, same
//! "considered" thresholds as the analysis, [`face_boxes`]) on the image's neutral render
//! (format defaults, [`DET_SIZE`] long edge, orientation applied: the frame the analysis
//! boxes live in), plus a half-scale pass for close-ups, pads the boxes ([`BOX_PAD`]) and
//! caches them per image.
//!
//! - Only the detector session is loaded (not the landmark / eye / mesh models), lazily on
//!   first use; if the model file is missing or fails to load, [`AutoFaces::detect`] returns
//!   `None` from then on (callers keep the skin-colour estimate) and nothing blocks.
//! - One session, behind a mutex (render + both passes: p50 29 ms, p95 33 ms on a decoded
//!   source; first call + model load ~0.25 s).
//! - Held by `DevelopCache` (`DevelopCache::with_auto_faces`), so the `auto_tone` command and
//!   the style model's Auto anchor (`ml::style::auto_tone_baseline`) share the session and
//!   the cache; `DevelopCache::forget_sources` drops cached boxes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use super::models::{nms, Detection, FaceDetector, DET_MODEL, DET_SIZE};
use super::scoring::MIN_CONSIDER_SCORE;
use crate::develop::{DevelopCache, SourceImage};
use crate::ipc::types::{ImageFormat, ImageId, NormRect, ParametricAdjustments};
use crate::lut::LutLibrary;

/// "Considered" face height (share of the frame height): the Wedding / Portrait
/// `CullThresholds::min_face_size` default the analysis uses.
pub const MIN_FACE: f32 = 0.04;
/// Detected boxes grow by this share (about their centre) before Auto tone uses them: boxes
/// from the neutral render jitter by a few pixels against the analysis' (preview) boxes, and
/// the skin guard only sees the central 70 % of a box. On the 202 face frames of
/// `examples/auto_eval.rs --no-faces --on-demand`, frames with > 0.3 % skin clipped (measured
/// in independently detected boxes) 5 -> 1 (0.33 %), Whites vs Camera Raw Auto 8.8 -> 10.4.
pub const BOX_PAD: f32 = 0.10;
/// Cached images (cleared wholesale when exceeded; each entry is a few boxes).
pub const MAX_CACHED: usize = 4096;

/// path, orientation and boxes of a detected image.
type CachedBoxes = (PathBuf, Option<u8>, Vec<NormRect>);

enum DetectorState {
    Unloaded,
    Ready(Box<FaceDetector>),
    Unavailable,
}

struct Inner {
    models_dir: PathBuf,
    detector: Mutex<DetectorState>,
    /// image -> (path, orientation, boxes): a different path / orientation is a miss.
    cache: Mutex<HashMap<ImageId, CachedBoxes>>,
}

/// Shared handle (cheap to clone).
#[derive(Clone)]
pub struct AutoFaces {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for AutoFaces {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutoFaces").field("models_dir", &self.inner.models_dir).finish()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl AutoFaces {
    /// Nothing is loaded until the first [`Self::detect`].
    pub fn new(models_dir: impl Into<PathBuf>) -> Self {
        AutoFaces {
            inner: Arc::new(Inner {
                models_dir: models_dir.into(),
                detector: Mutex::new(DetectorState::Unloaded),
                cache: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn models_dir(&self) -> &Path {
        &self.inner.models_dir
    }

    /// The cached boxes of `src`, if detected before.
    pub fn cached(&self, src: &SourceImage) -> Option<Vec<NormRect>> {
        let cache = lock(&self.inner.cache);
        let (path, orientation, boxes) = cache.get(&src.id)?;
        (*path == src.path && *orientation == src.orientation).then(|| boxes.clone())
    }

    /// Drops cached boxes of `ids` (`None` = all).
    pub fn forget(&self, ids: Option<&[ImageId]>) {
        let mut cache = lock(&self.inner.cache);
        match ids {
            Some(ids) => ids.iter().for_each(|id| {
                cache.remove(id);
            }),
            None => cache.clear(),
        }
    }

    /// Loads the detector on first use; `false` = unavailable (now and from then on).
    fn ensure_loaded(&self) -> bool {
        let mut state = lock(&self.inner.detector);
        if matches!(*state, DetectorState::Unloaded) {
            let path = self.inner.models_dir.join(DET_MODEL);
            *state = if !path.is_file() {
                DetectorState::Unavailable
            } else {
                match FaceDetector::load(&self.inner.models_dir) {
                    Ok(d) => DetectorState::Ready(Box::new(d)),
                    Err(e) => {
                        eprintln!("[auto faces] detector unavailable: {e}");
                        DetectorState::Unavailable
                    }
                }
            };
        }
        matches!(*state, DetectorState::Ready(_))
    }

    /// Face boxes of `src` as the analysis would weigh them ([`face_boxes`]), normalized in
    /// the oriented, uncropped frame; cached per image. `None` when the detector is
    /// unavailable (model missing / failed to load) or the render or inference fails.
    /// Blocking (decodes the source if `cache` does not hold it).
    pub fn detect(&self, cache: &DevelopCache, src: &SourceImage) -> Option<Vec<NormRect>> {
        if let Some(boxes) = self.cached(src) {
            return Some(boxes);
        }
        // Load first: no model, no render.
        if !self.ensure_loaded() {
            return None;
        }
        let format = src
            .path
            .extension()
            .and_then(|e| e.to_str())
            .and_then(ImageFormat::from_extension)
            .unwrap_or(ImageFormat::Jpeg);
        let neutral = ParametricAdjustments::defaults_for(format);
        let luts = LutLibrary::new(std::env::temp_dir().join("sieve-auto-faces-no-luts"));
        let r = match cache.render_image(src, &neutral, None, DET_SIZE as u32, &luts) {
            Ok(r) => r.image,
            Err(e) => {
                eprintln!("[auto faces] render of image {}: {}", src.id, e.message);
                return None;
            }
        };
        let (w, h) = (r.width as usize, r.height as usize);
        if w == 0 || h == 0 || w > DET_SIZE || h > DET_SIZE || r.rgb.len() < w * h * 3 {
            return None;
        }
        // Second pass at half scale (as `ml::segment`'s face detection): SCRFD misses faces
        // that fill most of the frame (close-ups).
        let (hw, hh, half) = half_size(&r.rgb, w, h);
        let dets = {
            let mut state = lock(&self.inner.detector);
            let DetectorState::Ready(det) = &mut *state else { return None };
            let run = det.detect(&r.rgb, w, h, 1.0).and_then(|mut d| {
                d.extend(det.detect(&half, hw, hh, 2.0)?);
                Ok(d)
            });
            match run {
                Ok(d) => nms(d, 0.4),
                Err(e) => {
                    eprintln!("[auto faces] image {}: {e}", src.id);
                    return None;
                }
            }
        };
        let boxes: Vec<NormRect> = face_boxes(&dets, w, h).iter().map(padded).collect();
        let mut c = lock(&self.inner.cache);
        if c.len() >= MAX_CACHED && !c.contains_key(&src.id) {
            c.clear();
        }
        c.insert(src.id, (src.path.clone(), src.orientation, boxes.clone()));
        Some(boxes)
    }
}

/// `b` grown by [`BOX_PAD`] about its centre. May extend past the frame by a few percent:
/// Auto tone clips its skin region to the frame (the region stays centred on the face).
fn padded(b: &NormRect) -> NormRect {
    let (cx, cy) = (b.x + b.width / 2.0, b.y + b.height / 2.0);
    let (w, h) = (b.width * (1.0 + BOX_PAD), b.height * (1.0 + BOX_PAD));
    NormRect { x: cx - w / 2.0, y: cy - h / 2.0, width: w, height: h }
}

/// 2x2 box downscale of interleaved RGB8 (odd edges dropped).
fn half_size(rgb: &[u8], w: usize, h: usize) -> (usize, usize, Vec<u8>) {
    let (hw, hh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = vec![0u8; hw * hh * 3];
    for y in 0..hh {
        for x in 0..hw {
            for c in 0..3 {
                let at = |xx: usize, yy: usize| u16::from(rgb[((yy.min(h - 1)) * w + xx.min(w - 1)) * 3 + c]);
                let sum = at(2 * x, 2 * y) + at(2 * x + 1, 2 * y) + at(2 * x, 2 * y + 1) + at(2 * x + 1, 2 * y + 1);
                out[(y * hw + x) * 3 + c] = ((sum + 2) / 4) as u8;
            }
        }
    }
    (hw, hh, out)
}

/// Normalized box (clamped to the frame), as `ml::metrics` stores analysis boxes.
fn norm_rect(b: &[f32; 4], w: usize, h: usize) -> NormRect {
    let (w, h) = (w as f32, h as f32);
    let x0 = b[0].clamp(0.0, w) / w;
    let y0 = b[1].clamp(0.0, h) / h;
    let x1 = b[2].clamp(0.0, w) / w;
    let y1 = b[3].clamp(0.0, h) / h;
    NormRect { x: x0, y: y0, width: (x1 - x0).max(0.0), height: (y1 - y0).max(0.0) }
}

/// The analysis' selection (`develop::auto::face_boxes` over `FaceInfo`): faces it would
/// consider (height >= [`MIN_FACE`], score >= [`MIN_CONSIDER_SCORE`]), else every detection
/// with score >= 0.6; largest first.
pub fn face_boxes(dets: &[Detection], w: usize, h: usize) -> Vec<NormRect> {
    let mut all: Vec<(NormRect, f32)> = dets.iter().map(|d| (norm_rect(&d.bbox, w, h), d.score)).collect();
    all.sort_by(|a, b| b.0.height.total_cmp(&a.0.height));
    let considered: Vec<NormRect> =
        all.iter().filter(|(r, s)| r.height >= MIN_FACE && *s >= MIN_CONSIDER_SCORE).map(|(r, _)| *r).collect();
    if !considered.is_empty() {
        return considered;
    }
    all.iter().filter(|(_, s)| *s >= 0.6).map(|(r, _)| *r).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det(bbox: [f32; 4], score: f32) -> Detection {
        Detection { bbox, score, kps: [[0.0; 2]; 5] }
    }

    #[test]
    fn selection_matches_the_analysis() {
        // 640x480: a 100 px tall face (20.8 %) and a 10 px one (2 %).
        let dets = [det([10.0, 10.0, 30.0, 20.0], 0.9), det([100.0, 100.0, 180.0, 200.0], 0.7)];
        let b = face_boxes(&dets, 640, 480);
        assert_eq!(b.len(), 1);
        assert!((b[0].height - 100.0 / 480.0).abs() < 1e-6);
        // Only small faces: every confident one counts.
        let small = [det([10.0, 10.0, 30.0, 20.0], 0.9), det([50.0, 50.0, 60.0, 60.0], 0.55)];
        assert_eq!(face_boxes(&small, 640, 480).len(), 1);
        // A large but unsure face is not considered.
        assert!(face_boxes(&[det([0.0, 0.0, 200.0, 200.0], 0.55)], 640, 480).is_empty());
        // Clamped to the frame.
        let c = face_boxes(&[det([-20.0, -10.0, 100.0, 100.0], 0.9)], 640, 480);
        assert_eq!((c[0].x, c[0].y), (0.0, 0.0));
    }

    #[test]
    fn padding_grows_about_the_centre() {
        let p = padded(&NormRect { x: 0.4, y: 0.4, width: 0.2, height: 0.1 });
        assert!((p.width - 0.22).abs() < 1e-6 && (p.height - 0.11).abs() < 1e-6);
        assert!((p.x + p.width / 2.0 - 0.5).abs() < 1e-6 && (p.y + p.height / 2.0 - 0.45).abs() < 1e-6);
    }

    #[test]
    fn missing_models_fall_back_without_rendering() {
        let faces = AutoFaces::new(std::env::temp_dir().join("sieve-no-such-models"));
        let cache = DevelopCache::new(crate::develop::DevelopConfig { cache_bytes: 0, mask_cache: None });
        // No model: `None` without rendering (the source does not exist either).
        let src = SourceImage { id: 7, path: PathBuf::from("/nonexistent/a.jpg"), orientation: None };
        assert_eq!(faces.detect(&cache, &src), None);
        assert_eq!(faces.cached(&src), None);
    }

    /// Real detector on the sample previews (skipped without `det_10g.onnx` / test data;
    /// `SIEVE_MODELS`, `SIEVE_TEST_DATA` override the checkout's `models/`, `test-data/`).
    #[test]
    fn detects_and_caches_on_sample_previews() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let models = std::env::var_os("SIEVE_MODELS").map(PathBuf::from).unwrap_or_else(|| root.join("models"));
        let data = std::env::var_os("SIEVE_TEST_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("..").join("test-data"))
            .join("segment-src");
        if !models.join(DET_MODEL).is_file() || !data.is_dir() {
            eprintln!("skipped: no models / test-data");
            return;
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(&data)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("jpg")))
            .collect();
        files.sort();
        let faces = AutoFaces::new(&models);
        let cache = DevelopCache::new(crate::develop::DevelopConfig { cache_bytes: 256 << 20, mask_cache: None })
            .with_auto_faces(faces.clone());
        let mut with_faces = 0;
        for (i, path) in files.iter().enumerate() {
            let src = SourceImage { id: i as ImageId + 1, path: path.clone(), orientation: None };
            let boxes = crate::develop::auto::resolve_faces(&cache, &src, None).expect("detector available");
            with_faces += usize::from(!boxes.is_empty());
            // Detections are clamped to the frame before padding.
            for b in &boxes {
                let m = BOX_PAD / 2.0 + 1e-5;
                assert!(b.x >= -m * b.width && b.y >= -m * b.height, "{b:?}");
                assert!(b.x + b.width <= 1.0 + m * b.width && b.y + b.height <= 1.0 + m * b.height, "{b:?}");
            }
            assert_eq!(faces.cached(&src), Some(boxes.clone()));
            // The analysis' faces win, even "analysed, no faces".
            assert_eq!(crate::develop::auto::resolve_faces(&cache, &src, Some(Vec::new())), Some(Vec::new()));
            // Another orientation is another frame: not served from the cache.
            assert_eq!(faces.cached(&SourceImage { orientation: Some(6), ..src.clone() }), None);
        }
        assert!(with_faces * 2 >= files.len(), "faces on {with_faces} of {} sample previews", files.len());
        faces.forget(None);
        assert_eq!(faces.cached(&SourceImage { id: 1, path: files[0].clone(), orientation: None }), None);
    }
}
