//! Face embedding: insightface ArcFace R50 (`w600k_r50.onnx`, buffalo_l pack, 512-d) on a
//! 112x112 crop aligned with the standard ArcFace similarity transform from the 5 SCRFD
//! keypoints. See `src-tauri/models/README.md` ("w600k_r50.onnx").

use std::path::Path;

use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use ort::session::Session;
use ort::value::TensorRef;

use crate::ipc::types::NormRect;
use crate::ml::imgproc::{warp_rgb_chw, Affine};
use crate::ml::models::{self, Detection, FaceDetector, Provider, DET_SIZE};
use crate::ml::FaceMetrics;
use crate::raw::turbo;

/// File name in the culling models directory.
pub const EMBED_MODEL: &str = "w600k_r50.onnx";
/// Stored in `face_embeddings.model_version`: model + alignment / preprocessing revision.
/// Bump the suffix when the crop or normalisation changes (old embeddings are recomputed).
pub const EMBED_MODEL_VERSION: &str = "arcface-w600k_r50@1";
/// Network input (square).
pub const EMBED_SIZE: usize = 112;
/// Embedding length.
pub const EMBED_DIM: usize = 512;
/// insightface `arcface_dst`: where the 5 keypoints (left eye, right eye, nose, left / right
/// mouth corner; image left / right) land in the 112x112 crop.
pub const ARCFACE_DST: [[f32; 2]; 5] =
    [[38.2946, 51.6963], [73.5318, 51.5014], [56.0252, 71.7366], [41.5493, 92.3655], [70.7299, 92.2041]];
/// Refuse absurd previews (as the analysis does).
const MAX_PIXELS: u64 = 64_000_000;
/// A re-detected face matches the analysed face when the boxes overlap this much.
const MATCH_IOU: f32 = 0.45;

/// Least-squares similarity (rotation, uniform scale, translation; no reflection) mapping
/// `src` onto `dst` (closed-form 2-D Umeyama). Returns `(a, b, tx, ty)` for
/// `dst = [a -b; b a] * src + t`.
pub fn similarity(src: &[[f32; 2]; 5], dst: &[[f32; 2]; 5]) -> (f32, f32, f32, f32) {
    let mean = |p: &[[f32; 2]; 5]| {
        let (sx, sy) = p.iter().fold((0.0, 0.0), |(x, y), q| (x + q[0], y + q[1]));
        (sx / 5.0, sy / 5.0)
    };
    let (msx, msy) = mean(src);
    let (mdx, mdy) = mean(dst);
    let (mut dot, mut cross, mut var) = (0.0f32, 0.0f32, 0.0f32);
    for (s, d) in src.iter().zip(dst) {
        let (sx, sy, dx, dy) = (s[0] - msx, s[1] - msy, d[0] - mdx, d[1] - mdy);
        dot += sx * dx + sy * dy;
        cross += sx * dy - sy * dx;
        var += sx * sx + sy * sy;
    }
    let var = var.max(1e-6);
    let (a, b) = (dot / var, cross / var);
    (a, b, mdx - (a * msx - b * msy), mdy - (b * msx + a * msy))
}

/// Crop transform (output pixel -> source pixel) of the aligned 112x112 face, i.e. the
/// inverse of the keypoints -> template similarity.
pub fn align(kps: &[[f32; 2]; 5]) -> Affine {
    let (a, b, tx, ty) = similarity(kps, &ARCFACE_DST);
    let k = (a * a + b * b).max(1e-12);
    // inverse of [a -b; b a] is [a b; -b a] / k
    let (ia, ib, ic, id) = (a / k, b / k, -b / k, a / k);
    Affine { a: ia, b: ib, tx: -(ia * tx + ib * ty), c: ic, d: id, ty: -(ic * tx + id * ty) }
}

/// How usable a face is for identity, 0..=1, from the analysis measurements: size (ArcFace
/// needs ~40 px faces), pose (|yaw|; profiles embed poorly), blur and detector confidence.
/// `preview_h` = preview height in pixels.
pub fn face_quality(f: &FaceMetrics, preview_h: u32) -> f32 {
    let ramp = |v: f32, lo: f32, hi: f32| ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
    let h_px = f.bbox.height * preview_h as f32;
    let size = ramp(h_px, 24.0, 72.0);
    let pose = 1.0 - ramp(f.yaw.abs(), 0.3, 0.8);
    let blur = 0.4 + 0.6 * ramp(f.face_sharpness, 0.0, 0.45);
    let det = 0.5 + 0.5 * ramp(f.detection_score, 0.5, 0.8);
    let q = size * pose * blur * det;
    if q.is_finite() {
        q
    } else {
        0.0
    }
}

/// L2-normalises `v` in place; `false` when it has no length.
pub fn normalize(v: &mut [f32]) -> bool {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if !(n > 1e-6 && n.is_finite()) {
        return false;
    }
    v.iter_mut().for_each(|x| *x /= n);
    true
}

/// The ArcFace session plus reusable buffers. One per worker thread.
pub struct FaceEmbedder {
    session: Session,
    pub provider: Provider,
    input: Vec<f32>,
    region: Vec<u8>,
}

impl FaceEmbedder {
    pub fn load(models_dir: &Path) -> Result<Self, String> {
        let (session, provider) = models::build(&models_dir.join(EMBED_MODEL), ("None", 1))?;
        Ok(Self { session, provider, input: vec![0.0; 3 * EMBED_SIZE * EMBED_SIZE], region: Vec::new() })
    }

    /// Input / output names and types of the session (diagnostics).
    pub fn describe(&self) -> String {
        let fmt = |o: &[ort::value::Outlet]| {
            o.iter().map(|x| format!("{} {:?}", x.name(), x.dtype())).collect::<Vec<_>>().join(", ")
        };
        format!("inputs [{}] outputs [{}]", fmt(self.session.inputs()), fmt(self.session.outputs()))
    }

    /// L2-normalised 512-d embedding of the face with keypoints `kps` (source pixels) in the
    /// RGB image `rgb` (`w` x `h`).
    pub fn embed(&mut self, rgb: &[u8], w: usize, h: usize, kps: &[[f32; 2]; 5]) -> Result<Vec<f32>, String> {
        self.warp(rgb, w, h, &align(kps));
        // insightface ArcFaceONNX for w600k_r50: RGB, (x - 127.5) / 127.5.
        self.input.iter_mut().for_each(|v| *v = (*v - 127.5) / 127.5);
        let input = TensorRef::from_array_view(([1usize, 3, EMBED_SIZE, EMBED_SIZE], &self.input[..]))
            .map_err(|e| e.to_string())?;
        let outputs = self.session.run(ort::inputs![input]).map_err(|e| format!("face embedding: {e}"))?;
        let (_, out) = outputs[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
        if out.len() != EMBED_DIM {
            return Err(format!("face embedding: unexpected output size {}", out.len()));
        }
        let mut v = out.to_vec();
        if !normalize(&mut v) {
            return Err("face embedding: zero vector".into());
        }
        Ok(v)
    }

    /// Warps the aligned crop into `self.input` (raw 0..=255 CHW). Faces much larger than
    /// the crop are box-downsampled first so the bilinear warp does not alias.
    fn warp(&mut self, rgb: &[u8], w: usize, h: usize, m: &Affine) {
        let src_per_out = (m.a * m.a + m.c * m.c).sqrt();
        let k = src_per_out.floor() as usize;
        if k < 2 {
            warp_rgb_chw(rgb, w, h, m, EMBED_SIZE, &mut self.input);
            return;
        }
        // Source region covered by the crop.
        let s = EMBED_SIZE as f32;
        let corners = [m.apply(0.0, 0.0), m.apply(s, 0.0), m.apply(0.0, s), m.apply(s, s)];
        let fold =
            |f: fn(f32, f32) -> f32, init: f32, pick: fn(&(f32, f32)) -> f32| corners.iter().map(pick).fold(init, f);
        let x0 = (fold(f32::min, f32::MAX, |c| c.0).floor().max(0.0) as usize).min(w - 1);
        let y0 = (fold(f32::min, f32::MAX, |c| c.1).floor().max(0.0) as usize).min(h - 1);
        let x1 = (fold(f32::max, f32::MIN, |c| c.0).ceil().max(0.0) as usize + 1).min(w);
        let y1 = (fold(f32::max, f32::MIN, |c| c.1).ceil().max(0.0) as usize + 1).min(h);
        let (rw, rh) = ((x1 - x0) / k, (y1 - y0) / k);
        if rw < 2 || rh < 2 {
            warp_rgb_chw(rgb, w, h, m, EMBED_SIZE, &mut self.input);
            return;
        }
        self.region.clear();
        self.region.resize(rw * rh * 3, 0);
        let norm = (k * k) as u32;
        for ry in 0..rh {
            for rx in 0..rw {
                let mut acc = [0u32; 3];
                for yy in 0..k {
                    let row = ((y0 + ry * k + yy) * w + x0 + rx * k) * 3;
                    for px in rgb[row..row + k * 3].chunks_exact(3) {
                        acc[0] += px[0] as u32;
                        acc[1] += px[1] as u32;
                        acc[2] += px[2] as u32;
                    }
                }
                let o = (ry * rw + rx) * 3;
                for (dst, a) in self.region[o..o + 3].iter_mut().zip(acc) {
                    *dst = ((a + norm / 2) / norm) as u8;
                }
            }
        }
        // Region pixel j covers source pixels [x0 + jk, x0 + jk + k): centre x0 + jk + (k-1)/2.
        let kf = k as f32;
        let off = (kf - 1.0) / 2.0;
        let r = Affine {
            a: m.a / kf,
            b: m.b / kf,
            tx: (m.tx - x0 as f32 - off) / kf,
            c: m.c / kf,
            d: m.d / kf,
            ty: (m.ty - y0 as f32 - off) / kf,
        };
        warp_rgb_chw(&self.region, rw, rh, &r, EMBED_SIZE, &mut self.input);
    }
}

/// Detector + embedder for one worker thread, with decode / resize buffers.
pub struct FacePipeline {
    det: FaceDetector,
    pub embedder: FaceEmbedder,
    resizer: Resizer,
    rgb: Vec<u8>,
    small: Vec<u8>,
}

/// One face to embed: its index in `faces_json` and its analysed box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FaceRequest {
    pub face_index: u32,
    pub bbox: NormRect,
}

impl FacePipeline {
    pub fn load(models_dir: &Path) -> Result<Self, String> {
        Ok(Self {
            det: FaceDetector::load(models_dir)?,
            embedder: FaceEmbedder::load(models_dir)?,
            resizer: Resizer::new(),
            rgb: Vec::new(),
            small: Vec::new(),
        })
    }

    /// Decodes the preview, re-runs SCRFD as the analysis does (640 px) for the 5 keypoints
    /// (not stored by the analysis), matches each requested face by box overlap and embeds
    /// it. `None` per face = no matching detection.
    pub fn embed_preview(&mut self, path: &Path, faces: &[FaceRequest]) -> Result<Vec<Option<Vec<f32>>>, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("read preview {}: {e}", path.display()))?;
        let (w, h) = turbo::decode_rgb_into(&bytes, u32::MAX, MAX_PIXELS, &mut self.rgb)?;
        let dets = self.detect_decoded(w as usize, h as usize)?;
        let (w, h) = (w as usize, h as usize);
        let mut out = Vec::with_capacity(faces.len());
        for f in faces {
            let want = [
                f.bbox.x * w as f32,
                f.bbox.y * h as f32,
                (f.bbox.x + f.bbox.width) * w as f32,
                (f.bbox.y + f.bbox.height) * h as f32,
            ];
            let best = dets
                .iter()
                .map(|d| (models::iou(&d.bbox, &want), d))
                .filter(|(o, _)| *o >= MATCH_IOU)
                .max_by(|a, b| a.0.total_cmp(&b.0));
            out.push(match best {
                Some((_, d)) => Some(self.embedder.embed(&self.rgb[..w * h * 3], w, h, &d.kps)?),
                None => None,
            });
        }
        Ok(out)
    }

    /// SCRFD on the decoded image in `self.rgb`, fitted to [`DET_SIZE`] like the analysis.
    fn detect_decoded(&mut self, w: usize, h: usize) -> Result<Vec<Detection>, String> {
        let long = w.max(h);
        let (sw, sh) = if long <= DET_SIZE {
            (w, h)
        } else {
            let s = DET_SIZE as f32 / long as f32;
            (((w as f32 * s).round() as usize).clamp(1, DET_SIZE), ((h as f32 * s).round() as usize).clamp(1, DET_SIZE))
        };
        self.small.resize(sw * sh * 3, 0);
        {
            let src = ImageRef::new(w as u32, h as u32, &self.rgb[..w * h * 3], PixelType::U8x3)
                .map_err(|e| e.to_string())?;
            let mut dst = Image::from_slice_u8(sw as u32, sh as u32, &mut self.small[..sw * sh * 3], PixelType::U8x3)
                .map_err(|e| e.to_string())?;
            let opts = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Bilinear));
            self.resizer.resize(&src, &mut dst, &opts).map_err(|e| format!("resize: {e}"))?;
        }
        let to_src = long as f32 / sw.max(sh) as f32;
        self.det.detect(&self.small[..sw * sh * 3], sw, sh, to_src)
    }

    /// Detections and embeddings of every face in an RGB image (evaluation helper).
    pub fn embed_all(&mut self, rgb: &[u8], w: usize, h: usize) -> Result<Vec<(Detection, Vec<f32>)>, String> {
        self.rgb.clear();
        self.rgb.extend_from_slice(&rgb[..w * h * 3]);
        let dets = self.detect_decoded(w, h)?;
        let mut out = Vec::with_capacity(dets.len());
        for d in dets {
            let v = self.embedder.embed(&self.rgb[..w * h * 3], w, h, &d.kps)?;
            out.push((d, v));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similarity_recovers_a_known_transform() {
        // dst = 0.5 * R(30deg) * src + (10, -4)
        let (s, c) = (30f32.to_radians().sin(), 30f32.to_radians().cos());
        let (a, b) = (0.5 * c, 0.5 * s);
        let src = [[100.0, 120.0], [160.0, 118.0], [130.0, 150.0], [105.0, 180.0], [158.0, 182.0]];
        let mut dst = [[0.0; 2]; 5];
        for (d, p) in dst.iter_mut().zip(&src) {
            *d = [a * p[0] - b * p[1] + 10.0, b * p[0] + a * p[1] - 4.0];
        }
        let (ea, eb, tx, ty) = similarity(&src, &dst);
        assert!((ea - a).abs() < 1e-4 && (eb - b).abs() < 1e-4);
        assert!((tx - 10.0).abs() < 1e-2 && (ty + 4.0).abs() < 1e-2);
    }

    #[test]
    fn align_maps_the_template_back_onto_the_keypoints() {
        // Keypoints = template scaled x3, rotated 90 degrees (an un-rotated portrait frame),
        // shifted.
        let kps: [[f32; 2]; 5] = ARCFACE_DST.map(|[x, y]| [500.0 - 3.0 * y, 200.0 + 3.0 * x]);
        let m = align(&kps);
        for (t, k) in ARCFACE_DST.iter().zip(&kps) {
            let (sx, sy) = m.apply(t[0], t[1]);
            assert!((sx - k[0]).abs() < 1e-2 && (sy - k[1]).abs() < 1e-2, "{t:?} -> ({sx}, {sy}) vs {k:?}");
        }
    }

    #[test]
    fn quality_prefers_large_frontal_sharp_faces() {
        let face = |h: f32, yaw: f32, sharp: f32| FaceMetrics {
            bbox: NormRect { x: 0.4, y: 0.3, width: h * 0.75, height: h },
            left_eye: crate::ipc::types::NormPoint { x: 0.45, y: 0.35 },
            right_eye: crate::ipc::types::NormPoint { x: 0.5, y: 0.35 },
            detection_score: 0.85,
            ear: None,
            sharpness: sharp,
            ear_left: None,
            ear_right: None,
            mouth_open: None,
            yaw,
            iod_px: 40.0,
            face_sharpness: sharp,
            eye_texture: 5.0,
            anisotropy: 0.1,
            frontal: true,
            truncated: false,
            eye_open_prob: None,
            head_pitch: None,
            head_yaw: None,
            mesh_ear: None,
            face_luma: 0.5,
            mouth_width: None,
            blown: 0.0,
        };
        let big = face_quality(&face(0.2, 0.0, 0.6), 1365);
        assert!(big > 0.95, "{big}");
        assert!(face_quality(&face(0.01, 0.0, 0.6), 1365) < 0.01, "tiny faces are junk");
        assert!(face_quality(&face(0.2, 0.9, 0.6), 1365) < 0.01, "profiles are junk");
        let soft = face_quality(&face(0.2, 0.0, 0.0), 1365);
        assert!((0.3..0.5).contains(&soft), "blur lowers but keeps: {soft}");
    }
}
