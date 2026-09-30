//! ONNX Runtime sessions: SCRFD-10GF face detector (`det_10g.onnx`) and the insightface
//! 106-point landmark model (`2d106det.onnx`). Tensor layouts and decoding follow
//! `src-tauri/models/README.md`.
//!
//! Execution provider: CoreML (MLProgram) with the free input dimensions pinned
//! (`"?"` -> [`DET_SIZE`], `"None"` -> 1) because CoreML needs static shapes; if the
//! CoreML session cannot be built the CPU EP is used and the reason is logged.

use std::path::Path;

use ort::ep;
use ort::session::builder::{GraphOptimizationLevel, SessionBuilder};
use ort::session::Session;
use ort::value::TensorRef;

use super::imgproc::{warp_rgb_chw, Affine};

pub const DET_MODEL: &str = "det_10g.onnx";
pub const LMK_MODEL: &str = "2d106det.onnx";
/// OpenVINO Open Model Zoo `open-closed-eye-0001` (Apache-2.0).
pub const EYE_MODEL: &str = "open_closed_eye.onnx";
pub const EYE_SIZE: usize = 32;
/// MediaPipe FaceMesh V2 (478 landmarks incl. iris; Apache-2.0), PINTO ONNX export.
pub const MESH_MODEL: &str = "face_landmarks_detector_1x3x256x256.onnx";
pub const MESH_SIZE: usize = 256;
/// Square detector input (multiple of 32).
pub const DET_SIZE: usize = 640;
/// Landmark model input.
pub const LMK_SIZE: usize = 192;
/// Detector confidence cut-off and NMS IoU (insightface defaults).
pub const DET_SCORE: f32 = 0.5;
const NMS_IOU: f32 = 0.4;
const STRIDES: [usize; 3] = [8, 16, 32];

/// Which execution provider a session ended up on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    CoreMl,
    Cpu,
}

/// One face from SCRFD, in source pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Detection {
    /// x0, y0, x1, y1
    pub bbox: [f32; 4],
    pub score: f32,
    /// left eye, right eye, nose, left mouth, right mouth (image left/right).
    pub kps: [[f32; 2]; 5],
}

pub struct Models {
    det: Session,
    lmk: Session,
    eye: Session,
    pub det_provider: Provider,
    pub lmk_provider: Provider,
    det_input: Vec<f32>,
    lmk_input: Vec<f32>,
    eye_input: Vec<f32>,
    mesh: Session,
    mesh_input: Vec<f32>,
}

fn base_builder() -> Result<SessionBuilder, String> {
    Session::builder()
        .and_then(|b| b.with_optimization_level(GraphOptimizationLevel::Level3).map_err(Into::into))
        // One image per pool thread; ORT's own thread pool would oversubscribe the cores.
        .and_then(|b| b.with_intra_threads(1).map_err(Into::into))
        .and_then(|b| b.with_inter_threads(1).map_err(Into::into))
        .map_err(|e| format!("session builder: {e}"))
}

/// CoreML first (unless `LUMENRAW_ML_CPU=1`), CPU fallback.
fn build(path: &Path, dim: (&str, i64)) -> Result<(Session, Provider), String> {
    if !path.is_file() {
        return Err(format!("model not found: {} (run scripts/fetch-models.sh)", path.display()));
    }
    let force_cpu = std::env::var("LUMENRAW_ML_CPU").is_ok_and(|v| v == "1");
    if !force_cpu {
        let coreml = ep::CoreML::default()
            .with_model_format(ep::coreml::ModelFormat::MLProgram)
            .with_static_input_shapes(true)
            .build()
            .error_on_failure();
        let attempt = base_builder().and_then(|b| {
            let b = b.with_dimension_override(dim.0, dim.1).map_err(|e| e.to_string())?;
            let mut b = b.with_execution_providers([coreml]).map_err(|e| e.to_string())?;
            b.commit_from_file(path).map_err(|e| e.to_string())
        });
        match attempt {
            Ok(s) => return Ok((s, Provider::CoreMl)),
            Err(e) => eprintln!("[analysis] CoreML unavailable for {}, using CPU: {e}", path.display()),
        }
    }
    let session = base_builder()?
        .with_dimension_override(dim.0, dim.1)
        .map_err(|e| e.to_string())?
        .commit_from_file(path)
        .map_err(|e| format!("load {}: {e}", path.display()))?;
    Ok((session, Provider::Cpu))
}

impl Models {
    pub fn load(models_dir: &Path) -> Result<Self, String> {
        let (det, det_provider) = build(&models_dir.join(DET_MODEL), ("?", DET_SIZE as i64))?;
        let (lmk, lmk_provider) = build(&models_dir.join(LMK_MODEL), ("None", 1))?;
        // 0.0014 GFLOP: CPU is faster than a CoreML round trip.
        let eye_path = models_dir.join(EYE_MODEL);
        if !eye_path.is_file() {
            return Err(format!("model not found: {} (run scripts/fetch-models.sh)", eye_path.display()));
        }
        let eye =
            base_builder()?.commit_from_file(&eye_path).map_err(|e| format!("load {}: {e}", eye_path.display()))?;
        // CPU: CoreML (MLProgram) rejects this export ("Required param 'pad' is
        // missing" on a depthwise conv); it only runs for possible blinks anyway.
        let mesh_path = models_dir.join(MESH_MODEL);
        if !mesh_path.is_file() {
            return Err(format!("model not found: {} (run scripts/fetch-models.sh)", mesh_path.display()));
        }
        let mesh =
            base_builder()?.commit_from_file(&mesh_path).map_err(|e| format!("load {}: {e}", mesh_path.display()))?;
        Ok(Self {
            det,
            lmk,
            eye,
            det_provider,
            lmk_provider,
            det_input: vec![0.0; 3 * DET_SIZE * DET_SIZE],
            lmk_input: vec![0.0; 3 * LMK_SIZE * LMK_SIZE],
            eye_input: vec![0.0; 3 * EYE_SIZE * EYE_SIZE],
            mesh,
            mesh_input: vec![0.0; 3 * MESH_SIZE * MESH_SIZE],
        })
    }

    /// MediaPipe FaceMesh V2 on the same upright, 1.5x face-box crop as the 106-point
    /// model (MediaPipe's own ROI is the eye-aligned detection box scaled 1.5x).
    /// Input RGB in `[0, 1]`, NCHW. Returns 478 `(x, y, z)` in crop pixels (z: depth in
    /// the same scale, smaller = closer) and the crop transform (crop -> source).
    pub fn face_mesh(
        &mut self,
        rgb: &[u8],
        w: usize,
        h: usize,
        det: &Detection,
    ) -> Result<(Vec<[f32; 3]>, Affine), String> {
        let [x0, y0, x1, y1] = det.bbox;
        let size = (x1 - x0).max(y1 - y0).max(1.0) * 1.5;
        let [l, r] = [det.kps[0], det.kps[1]];
        let angle = (r[1] - l[1]).atan2(r[0] - l[0]);
        let m = Affine::crop((x0 + x1) / 2.0, (y0 + y1) / 2.0, MESH_SIZE as f32 / size, angle, MESH_SIZE as f32);
        warp_rgb_chw(rgb, w, h, &m, MESH_SIZE, &mut self.mesh_input);
        self.mesh_input.iter_mut().for_each(|v| *v /= 255.0);
        let input = TensorRef::from_array_view(([1usize, 3, MESH_SIZE, MESH_SIZE], &self.mesh_input[..]))
            .map_err(|e| e.to_string())?;
        let outputs = self.mesh.run(ort::inputs![input]).map_err(|e| format!("face mesh: {e}"))?;
        let (_, out) = outputs[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
        if out.len() < 478 * 3 {
            return Err(format!("face mesh: unexpected output size {}", out.len()));
        }
        Ok((out[..478 * 3].as_chunks::<3>().0.to_vec(), m))
    }

    /// Probability that the eye in the square `side`-px box centred on `center` (in the
    /// upright landmark-crop coordinates of `crop`) is open. OMZ `open-closed-eye-0001`
    /// (MRL eye dataset, grey IR crops): input 32x32, `(x - 127) / 255`, 2-way softmax.
    /// The OMZ docs list the classes as `[open, closed]`, but on our previews index 1
    /// is ~1.0 for clearly open eyes and ~0.0 for shut ones, so index 1 = open.
    /// Fed grey (replicated to 3 channels) to match the IR training data.
    pub fn eye_open(
        &mut self,
        rgb: &[u8],
        w: usize,
        h: usize,
        crop: &Affine,
        center: [f32; 2],
        side: f32,
    ) -> Result<f32, String> {
        let k = side / EYE_SIZE as f32;
        let half = EYE_SIZE as f32 / 2.0;
        let (tx, ty) = crop.apply(center[0] - half * k, center[1] - half * k);
        let m = Affine { a: crop.a * k, b: crop.b * k, tx, c: crop.c * k, d: crop.d * k, ty };
        warp_rgb_chw(rgb, w, h, &m, EYE_SIZE, &mut self.eye_input);
        let plane = EYE_SIZE * EYE_SIZE;
        for i in 0..plane {
            let g =
                0.299 * self.eye_input[i] + 0.587 * self.eye_input[plane + i] + 0.114 * self.eye_input[2 * plane + i];
            let v = (g - 127.0) / 255.0;
            self.eye_input[i] = v;
            self.eye_input[plane + i] = v;
            self.eye_input[2 * plane + i] = v;
        }
        let input = TensorRef::from_array_view(([1usize, 3, EYE_SIZE, EYE_SIZE], &self.eye_input[..]))
            .map_err(|e| e.to_string())?;
        let outputs = self.eye.run(ort::inputs![input]).map_err(|e| format!("eye state: {e}"))?;
        let (_, out) = outputs[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
        if out.len() < 2 {
            return Err(format!("eye state: unexpected output size {}", out.len()));
        }
        Ok(out[1] / (out[0] + out[1]).max(1e-6))
    }

    /// Runs SCRFD on `small`, an RGB image whose long edge is <= [`DET_SIZE`]
    /// (letterboxed top-left). Coordinates are multiplied by `to_src` to map back.
    pub fn detect(&mut self, small: &[u8], w: usize, h: usize, to_src: f32) -> Result<Vec<Detection>, String> {
        let s = DET_SIZE;
        let plane = s * s;
        self.det_input.iter_mut().for_each(|v| *v = 0.0);
        for y in 0..h.min(s) {
            for x in 0..w.min(s) {
                let p = (y * w + x) * 3;
                let i = y * s + x;
                self.det_input[i] = (small[p] as f32 - 127.5) / 128.0;
                self.det_input[plane + i] = (small[p + 1] as f32 - 127.5) / 128.0;
                self.det_input[2 * plane + i] = (small[p + 2] as f32 - 127.5) / 128.0;
            }
        }
        let input = TensorRef::from_array_view(([1usize, 3, s, s], &self.det_input[..])).map_err(|e| e.to_string())?;
        let outputs = self.det.run(ort::inputs![input]).map_err(|e| format!("detector: {e}"))?;
        let mut dets = Vec::new();
        for (k, &stride) in STRIDES.iter().enumerate() {
            let (_, scores) = outputs[k].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
            let (_, boxes) = outputs[k + 3].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
            let (_, kps) = outputs[k + 6].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
            let cols = s / stride;
            for (i, &score) in scores.iter().enumerate() {
                if score < DET_SCORE {
                    continue;
                }
                let loc = i / 2;
                let (cx, cy) = (((loc % cols) * stride) as f32, ((loc / cols) * stride) as f32);
                let st = stride as f32;
                let b = &boxes[i * 4..i * 4 + 4];
                let bbox = [
                    (cx - b[0] * st) * to_src,
                    (cy - b[1] * st) * to_src,
                    (cx + b[2] * st) * to_src,
                    (cy + b[3] * st) * to_src,
                ];
                let kp = &kps[i * 10..i * 10 + 10];
                let mut pts = [[0.0f32; 2]; 5];
                for (j, p) in pts.iter_mut().enumerate() {
                    *p = [(cx + kp[2 * j] * st) * to_src, (cy + kp[2 * j + 1] * st) * to_src];
                }
                dets.push(Detection { bbox, score, kps: pts });
            }
        }
        Ok(nms(dets, NMS_IOU))
    }

    /// 106 landmarks for a face, in *crop* coordinates of the upright 192x192 crop
    /// (eyes horizontal), plus the crop transform (crop -> source).
    pub fn landmarks(
        &mut self,
        rgb: &[u8],
        w: usize,
        h: usize,
        det: &Detection,
    ) -> Result<([[f32; 2]; 106], Affine), String> {
        let [x0, y0, x1, y1] = det.bbox;
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let size = (x1 - x0).max(y1 - y0).max(1.0);
        let scale = LMK_SIZE as f32 / (size * 1.5);
        let [l, r] = [det.kps[0], det.kps[1]];
        let angle = (r[1] - l[1]).atan2(r[0] - l[0]);
        let m = Affine::crop(cx, cy, scale, angle, LMK_SIZE as f32);
        warp_rgb_chw(rgb, w, h, &m, LMK_SIZE, &mut self.lmk_input);
        let input = TensorRef::from_array_view(([1usize, 3, LMK_SIZE, LMK_SIZE], &self.lmk_input[..]))
            .map_err(|e| e.to_string())?;
        let outputs = self.lmk.run(ort::inputs![input]).map_err(|e| format!("landmarks: {e}"))?;
        let (_, out) = outputs[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
        if out.len() < 212 {
            return Err(format!("landmarks: unexpected output size {}", out.len()));
        }
        let half = LMK_SIZE as f32 / 2.0;
        let mut pts = [[0.0f32; 2]; 106];
        for (i, p) in pts.iter_mut().enumerate() {
            *p = [(out[2 * i] + 1.0) * half, (out[2 * i + 1] + 1.0) * half];
        }
        Ok((pts, m))
    }
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let ix = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let iy = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let inter = ix * iy;
    let area = |r: &[f32; 4]| (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0);
    let union = area(a) + area(b) - inter;
    if union > 0.0 {
        inter / union
    } else {
        0.0
    }
}

/// Greedy NMS, highest score first.
pub fn nms(mut dets: Vec<Detection>, thresh: f32) -> Vec<Detection> {
    dets.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut keep: Vec<Detection> = Vec::new();
    for d in dets {
        if keep.iter().all(|k| iou(&k.bbox, &d.bbox) <= thresh) {
            keep.push(d);
        }
    }
    keep
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det(bbox: [f32; 4], score: f32) -> Detection {
        Detection { bbox, score, kps: [[0.0; 2]; 5] }
    }

    #[test]
    fn nms_suppresses_overlaps() {
        let out = nms(
            vec![
                det([0.0, 0.0, 10.0, 10.0], 0.8),
                det([1.0, 1.0, 11.0, 11.0], 0.9),
                det([20.0, 20.0, 30.0, 30.0], 0.6),
            ],
            0.4,
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].score, 0.9);
        assert_eq!(out[1].score, 0.6);
    }
}
