//! AI masks for local adjustments (Phase 7c prototype; no IPC surface yet): Select Subject /
//! Background, Select Sky and People (per-person instances plus face/skin/hair/clothes/eye/
//! brow/lip/teeth parts). Models, licenses, tensor layouts and measured quality/timings are in
//! `src-tauri/models/README.md` ("Segmentation models").
//!
//! | Mask | Model (license) | Input | EP |
//! |---|---|---|---|
//! | subject / background | BiRefNet_lite (MIT) | 1024x1024 | CPU (CoreML rejects the export) |
//! | sky | skyseg U2-Net (MIT) | 320x320 | CPU (CoreML slower) |
//! | person boxes | YOLOX-m COCO (Apache-2.0) | 640x640 letterbox | CoreML |
//! | person instances | EfficientSAM-Ti (Apache-2.0), box + face-point prompts | 1024x1024 | encoder CoreML, decoder CPU |
//! | hair / face skin / body skin / clothes | MediaPipe selfie multiclass (Apache-2.0) | 256x256 per person/head crop | CPU |
//! | eyes / iris / brows / lips / teeth | MediaPipe FaceMesh V2 landmarks (Apache-2.0, `models.rs`) | polygons | CPU |
//!
//! Every network output is upsampled to image resolution with a fast *colour guided filter*
//! (He et al.): the filter coefficients are solved at <= [`WORK_EDGE`] px over the mask's region
//! and applied with the full-resolution image as guide, which snaps soft low-res masks to real
//! edges (hair strands, tree/sky boundaries) without a dedicated matting model.
//!
//! Masks are returned as [`Mask`]: an f32 alpha plane (0..1) over a region of interest in
//! image pixels, so per-person part masks cost only their bounding box.

use std::path::{Path, PathBuf};
use std::time::Instant;

use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use ort::ep;
use ort::session::builder::{GraphOptimizationLevel, SessionBuilder};
use ort::session::Session;
use ort::value::TensorRef;

use super::models::{nms, Detection, Models, Provider, DET_SIZE};

pub const SUBJECT_MODEL: &str = "birefnet_lite.onnx";
pub const SKY_MODEL: &str = "skyseg.onnx";
pub const PERSON_MODEL: &str = "yolox_m.onnx";
pub const SAM_ENCODER_MODEL: &str = "efficientsam_ti_encoder.onnx";
pub const SAM_DECODER_MODEL: &str = "efficientsam_ti_decoder.onnx";
pub const PARTS_MODEL: &str = "selfie_multiclass_256x256.onnx";

const SUBJECT_SIZE: usize = 1024;
const SKY_SIZE: usize = 320;
const PERSON_SIZE: usize = 640;
const SAM_SIZE: usize = 1024;
const PARTS_SIZE: usize = 256;
/// Long edge of the working resolution for guided refinement and SAM mask decoding.
pub const WORK_EDGE: usize = 1024;

const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];

/// YOLOX person score (objectness x class) and NMS IoU.
const PERSON_SCORE: f32 = 0.35;
const PERSON_NMS: f32 = 0.5;
/// Persons / synthetic persons smaller than this fraction of the image height are ignored.
const MIN_PERSON_H: f32 = 0.04;
/// Faces used as person seeds / for FaceMesh parts must be at least this tall (fraction of h).
const MIN_SEED_FACE: f32 = 0.012;
const MIN_MESH_FACE_PX: f32 = 40.0;
const MAX_PEOPLE: usize = 24;

// MediaPipe FaceMesh (468 + 10 iris) contours, from `face_mesh_connections.py`.
const LIPS_OUTER: [usize; 20] =
    [61, 146, 91, 181, 84, 17, 314, 405, 321, 375, 291, 409, 270, 269, 267, 0, 37, 39, 40, 185];
const LIPS_INNER: [usize; 20] =
    [78, 95, 88, 178, 87, 14, 317, 402, 318, 324, 308, 415, 310, 311, 312, 13, 82, 81, 80, 191];
const EYE_R: [usize; 16] = [33, 7, 163, 144, 145, 153, 154, 155, 133, 173, 157, 158, 159, 160, 161, 246];
const EYE_L: [usize; 16] = [263, 249, 390, 373, 374, 380, 381, 382, 362, 398, 384, 385, 386, 387, 388, 466];
const BROW_R: [usize; 10] = [70, 63, 105, 66, 107, 55, 65, 52, 53, 46];
const BROW_L: [usize; 10] = [300, 293, 334, 296, 336, 285, 295, 282, 283, 276];
/// Iris centre followed by its 4 ring points.
const IRIS_R: [usize; 5] = [468, 469, 470, 471, 472];
const IRIS_L: [usize; 5] = [473, 474, 475, 476, 477];

/// Borrowed interleaved RGB8 image.
#[derive(Clone, Copy)]
pub struct RgbImage<'a> {
    pub data: &'a [u8],
    pub width: usize,
    pub height: usize,
}

/// Soft alpha (0..1) over `[x0, x0 + width) x [y0, y0 + height)` of the image; 0 outside.
#[derive(Debug, Clone, PartialEq)]
pub struct Mask {
    pub x0: usize,
    pub y0: usize,
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl Mask {
    pub fn zeros(x0: usize, y0: usize, width: usize, height: usize) -> Self {
        Self { x0, y0, width, height, data: vec![0.0; width * height] }
    }

    /// Alpha at image pixel `(x, y)`.
    #[inline]
    pub fn at(&self, x: usize, y: usize) -> f32 {
        if x < self.x0 || y < self.y0 || x >= self.x0 + self.width || y >= self.y0 + self.height {
            return 0.0;
        }
        self.data[(y - self.y0) * self.width + (x - self.x0)]
    }

    /// Full-image mask (`img_w x img_h`) with `1 - alpha` (e.g. background from subject).
    pub fn inverted(&self, img_w: usize, img_h: usize) -> Mask {
        let mut out = Mask { x0: 0, y0: 0, width: img_w, height: img_h, data: vec![1.0; img_w * img_h] };
        for y in 0..self.height {
            for x in 0..self.width {
                out.data[(y + self.y0) * img_w + x + self.x0] = 1.0 - self.data[y * self.width + x];
            }
        }
        out
    }

    /// Sum of alpha (pixel area).
    pub fn area(&self) -> f32 {
        self.data.iter().sum()
    }

    /// Full-image 8-bit alpha.
    pub fn to_u8(&self, img_w: usize, img_h: usize) -> Vec<u8> {
        let mut out = vec![0u8; img_w * img_h];
        for y in 0..self.height {
            let row = &self.data[y * self.width..(y + 1) * self.width];
            let o = (y + self.y0) * img_w + self.x0;
            for (d, &v) in out[o..o + self.width].iter_mut().zip(row) {
                *d = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
        }
        out
    }
}

/// One person: instance mask plus optional parts (same image coordinates).
#[derive(Debug, Clone)]
pub struct Person {
    /// x0, y0, x1, y1 in image pixels (detector box, or a face-derived box for `from_face`).
    pub bbox: [f32; 4],
    pub score: f32,
    /// True when no person box was detected and the instance was seeded from a face.
    pub from_face: bool,
    pub face: Option<Detection>,
    pub mask: Mask,
    pub parts: Option<PersonParts>,
}

#[derive(Debug, Clone)]
pub struct PersonParts {
    pub hair: Mask,
    pub face_skin: Mask,
    pub body_skin: Mask,
    pub clothes: Mask,
    /// Eye whites (eye opening minus iris).
    pub sclera: Mask,
    pub iris: Mask,
    pub brows: Mask,
    pub lips: Mask,
    pub teeth: Mask,
}

pub struct SegmentConfig {
    pub models_dir: PathBuf,
    /// Try CoreML for the models that compile on it (person detector, SAM encoder, SCRFD).
    pub coreml: bool,
    /// CoreML compiled-model cache (first compile of the SAM encoder takes ~9 s).
    pub coreml_cache: Option<PathBuf>,
    /// ORT intra-op threads for CPU sessions (0 = ORT default, all cores). Masks are computed
    /// one image at a time on demand, unlike the per-image culling pool.
    pub threads: usize,
}

impl SegmentConfig {
    pub fn new(models_dir: impl Into<PathBuf>) -> Self {
        Self { models_dir: models_dir.into(), coreml: true, coreml_cache: None, threads: 0 }
    }
}

/// Lazily loaded sessions; each mask type loads only what it needs.
pub struct Segmenter {
    cfg: SegmentConfig,
    subject: Option<Session>,
    sky: Option<Session>,
    person: Option<(Session, Provider)>,
    sam_enc: Option<(Session, Provider)>,
    sam_dec: Option<Session>,
    parts: Option<Session>,
    faces: Option<Models>,
    resizer: Resizer,
    /// Wall time per step of the last call, in ms.
    pub timings: Vec<(&'static str, f64)>,
}

fn base_builder(threads: usize) -> Result<SessionBuilder, String> {
    Session::builder()
        .and_then(|b| b.with_optimization_level(GraphOptimizationLevel::Level3).map_err(Into::into))
        .and_then(|b| b.with_intra_threads(threads).map_err(Into::into))
        .map_err(|e| format!("session builder: {e}"))
}

fn check(path: &Path) -> Result<(), String> {
    if path.is_file() {
        Ok(())
    } else {
        Err(format!("model not found: {} (run scripts/fetch-models.sh)", path.display()))
    }
}

fn cpu_session(cfg: &SegmentConfig, name: &str) -> Result<Session, String> {
    let path = cfg.models_dir.join(name);
    check(&path)?;
    base_builder(cfg.threads)?.commit_from_file(&path).map_err(|e| format!("load {}: {e}", path.display()))
}

/// CoreML (MLProgram, static shapes, pinned free dims) with CPU fallback.
fn coreml_session(cfg: &SegmentConfig, name: &str, dims: &[(&str, i64)]) -> Result<(Session, Provider), String> {
    let path = cfg.models_dir.join(name);
    check(&path)?;
    let with_dims = |mut b: SessionBuilder| -> Result<SessionBuilder, String> {
        for &(n, v) in dims {
            b = b.with_dimension_override(n, v).map_err(|e| e.to_string())?;
        }
        Ok(b)
    };
    let force_cpu = std::env::var("SIEVE_ML_CPU").is_ok_and(|v| v == "1");
    if cfg.coreml && !force_cpu {
        let mut coreml =
            ep::CoreML::default().with_model_format(ep::coreml::ModelFormat::MLProgram).with_static_input_shapes(true);
        if let Some(dir) = &cfg.coreml_cache {
            let _ = std::fs::create_dir_all(dir);
            coreml = coreml.with_model_cache_dir(dir.display());
        }
        let coreml = coreml.build().error_on_failure();
        let attempt = base_builder(cfg.threads).and_then(with_dims).and_then(|b| {
            let mut b = b.with_execution_providers([coreml]).map_err(|e| e.to_string())?;
            b.commit_from_file(&path).map_err(|e| e.to_string())
        });
        match attempt {
            Ok(s) => return Ok((s, Provider::CoreMl)),
            Err(e) => eprintln!("[segment] CoreML unavailable for {name}, using CPU: {e}"),
        }
    }
    let s = with_dims(base_builder(cfg.threads)?)?
        .commit_from_file(&path)
        .map_err(|e| format!("load {}: {e}", path.display()))?;
    Ok((s, Provider::Cpu))
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

impl Segmenter {
    pub fn new(cfg: SegmentConfig) -> Self {
        Self {
            cfg,
            subject: None,
            sky: None,
            person: None,
            sam_enc: None,
            sam_dec: None,
            parts: None,
            faces: None,
            resizer: Resizer::new(),
            timings: Vec::new(),
        }
    }

    /// Execution providers of the sessions loaded so far (for logs / benchmarks).
    pub fn providers(&self) -> Vec<(&'static str, Provider)> {
        let mut v = Vec::new();
        if self.subject.is_some() {
            v.push(("subject", Provider::Cpu));
        }
        if self.sky.is_some() {
            v.push(("sky", Provider::Cpu));
        }
        if let Some((_, p)) = &self.person {
            v.push(("person_det", *p));
        }
        if let Some((_, p)) = &self.sam_enc {
            v.push(("sam_encoder", *p));
        }
        if self.sam_dec.is_some() {
            v.push(("sam_decoder", Provider::Cpu));
        }
        if self.parts.is_some() {
            v.push(("parts", Provider::Cpu));
        }
        if let Some(m) = &self.faces {
            v.push(("face_det", m.det_provider));
        }
        v
    }

    /// Stretches `crop` (x, y, w, h in image px; whole image if `None`) to `dw x dh` RGB8.
    fn resize(&mut self, img: RgbImage, crop: Option<[f64; 4]>, dw: usize, dh: usize) -> Result<Vec<u8>, String> {
        let mut out = vec![0u8; dw * dh * 3];
        let src =
            ImageRef::new(img.width as u32, img.height as u32, img.data, PixelType::U8x3).map_err(|e| e.to_string())?;
        let mut dst =
            Image::from_slice_u8(dw as u32, dh as u32, &mut out, PixelType::U8x3).map_err(|e| e.to_string())?;
        let mut opts = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Bilinear));
        if let Some([x, y, w, h]) = crop {
            opts = opts.crop(x, y, w, h);
        }
        self.resizer.resize(&src, &mut dst, &opts).map_err(|e| format!("resize: {e}"))?;
        Ok(out)
    }

    // -----------------------------------------------------------------------------------
    // Subject / background
    // -----------------------------------------------------------------------------------

    /// Select Subject: salient foreground matte over the whole image (background = inverse).
    pub fn subject(&mut self, img: RgbImage) -> Result<Mask, String> {
        self.timings.clear();
        let t = Instant::now();
        if self.subject.is_none() {
            self.subject = Some(cpu_session(&self.cfg, SUBJECT_MODEL)?);
        }
        self.timings.push(("subject_load", ms(t)));
        let t = Instant::now();
        let small = self.resize(img, None, SUBJECT_SIZE, SUBJECT_SIZE)?;
        let input = to_nchw(&small, SUBJECT_SIZE * SUBJECT_SIZE, IMAGENET_MEAN, IMAGENET_STD, false);
        self.timings.push(("subject_pre", ms(t)));
        let t = Instant::now();
        let session = self.subject.as_mut().expect("loaded");
        let tensor = TensorRef::from_array_view(([1usize, 3, SUBJECT_SIZE, SUBJECT_SIZE], &input[..]))
            .map_err(|e| e.to_string())?;
        let outputs = session.run(ort::inputs![tensor]).map_err(|e| format!("subject: {e}"))?;
        let (_, out) = outputs[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
        if out.len() != SUBJECT_SIZE * SUBJECT_SIZE {
            return Err(format!("subject: unexpected output size {}", out.len()));
        }
        let p: Vec<f32> = out.iter().map(|&v| sigmoid(v)).collect();
        drop(outputs);
        self.timings.push(("subject_infer", ms(t)));
        let t = Instant::now();
        let roi = [0, 0, img.width, img.height];
        let m = self.refine(img, roi, &p, SUBJECT_SIZE, SUBJECT_SIZE, Refine::SUBJECT)?;
        self.timings.push(("subject_refine", ms(t)));
        Ok(m)
    }

    // -----------------------------------------------------------------------------------
    // Sky
    // -----------------------------------------------------------------------------------

    /// Select Sky over the whole image.
    pub fn sky(&mut self, img: RgbImage) -> Result<Mask, String> {
        self.timings.clear();
        let t = Instant::now();
        if self.sky.is_none() {
            self.sky = Some(cpu_session(&self.cfg, SKY_MODEL)?);
        }
        self.timings.push(("sky_load", ms(t)));
        let t = Instant::now();
        let small = self.resize(img, None, SKY_SIZE, SKY_SIZE)?;
        let input = to_nchw(&small, SKY_SIZE * SKY_SIZE, IMAGENET_MEAN, IMAGENET_STD, false);
        let session = self.sky.as_mut().expect("loaded");
        let tensor =
            TensorRef::from_array_view(([1usize, 3, SKY_SIZE, SKY_SIZE], &input[..])).map_err(|e| e.to_string())?;
        let outputs = session.run(ort::inputs![tensor]).map_err(|e| format!("sky: {e}"))?;
        // Output 0 is the fused U2-Net side output, already a sigmoid probability.
        let (_, out) = outputs[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
        if out.len() != SKY_SIZE * SKY_SIZE {
            return Err(format!("sky: unexpected output size {}", out.len()));
        }
        let p = out.to_vec();
        drop(outputs);
        self.timings.push(("sky_infer", ms(t)));
        let t = Instant::now();
        let m = self.refine(img, [0, 0, img.width, img.height], &p, SKY_SIZE, SKY_SIZE, Refine::SKY)?;
        self.timings.push(("sky_refine", ms(t)));
        Ok(m)
    }

    // -----------------------------------------------------------------------------------
    // People
    // -----------------------------------------------------------------------------------

    /// Person boxes from YOLOX-m (COCO class 0), in image pixels, NMS applied.
    pub fn detect_people(&mut self, img: RgbImage) -> Result<Vec<([f32; 4], f32)>, String> {
        if self.person.is_none() {
            self.person = Some(coreml_session(&self.cfg, PERSON_MODEL, &[])?);
        }
        let s = PERSON_SIZE;
        let ratio = (s as f32 / img.width as f32).min(s as f32 / img.height as f32);
        let (sw, sh) = (
            ((img.width as f32 * ratio).round() as usize).clamp(1, s),
            ((img.height as f32 * ratio).round() as usize).clamp(1, s),
        );
        let small = self.resize(img, None, sw, sh)?;
        // Official YOLOX preprocessing: BGR, raw 0..255, top-left letterbox padded with 114.
        let plane = s * s;
        let mut input = vec![114.0f32; 3 * plane];
        for y in 0..sh {
            for x in 0..sw {
                let p = (y * sw + x) * 3;
                let i = y * s + x;
                input[i] = small[p + 2] as f32;
                input[plane + i] = small[p + 1] as f32;
                input[2 * plane + i] = small[p] as f32;
            }
        }
        let (session, _) = self.person.as_mut().expect("loaded");
        let tensor = TensorRef::from_array_view(([1usize, 3, s, s], &input[..])).map_err(|e| e.to_string())?;
        let outputs = session.run(ort::inputs![tensor]).map_err(|e| format!("person detector: {e}"))?;
        let (_, out) = outputs[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
        let boxes = decode_yolox(out, s, PERSON_SCORE)?;
        Ok(boxes
            .into_iter()
            .map(|(b, sc)| {
                let b = [
                    (b[0] / ratio).clamp(0.0, img.width as f32),
                    (b[1] / ratio).clamp(0.0, img.height as f32),
                    (b[2] / ratio).clamp(0.0, img.width as f32),
                    (b[3] / ratio).clamp(0.0, img.height as f32),
                ];
                (b, sc)
            })
            .collect())
    }

    /// SCRFD faces (the culling detector, `models.rs`), largest first.
    pub fn detect_faces(&mut self, img: RgbImage) -> Result<Vec<Detection>, String> {
        if self.faces.is_none() {
            self.faces = Some(Models::load(&self.cfg.models_dir)?);
        }
        let long = img.width.max(img.height);
        let k = (DET_SIZE as f32 / long as f32).min(1.0);
        let (sw, sh) =
            (((img.width as f32 * k).round() as usize).max(1), ((img.height as f32 * k).round() as usize).max(1));
        let small = self.resize(img, None, sw, sh)?;
        let to_src = long as f32 / sw.max(sh) as f32;
        let mut dets = self.faces.as_mut().expect("loaded").detect(&small, sw, sh, to_src)?;
        // Second pass at half scale: SCRFD misses faces filling most of the frame (close-ups).
        let (hw, hh) = ((sw / 2).max(1), (sh / 2).max(1));
        let half = self.resize(img, None, hw, hh)?;
        dets.extend(self.faces.as_mut().expect("loaded").detect(&half, hw, hh, long as f32 / hw.max(hh) as f32)?);
        let mut dets = nms(dets, 0.4);
        dets.sort_by(|a, b| (b.bbox[3] - b.bbox[1]).total_cmp(&(a.bbox[3] - a.bbox[1])));
        Ok(dets)
    }

    /// People: one instance mask per person (optionally with parts), largest first.
    ///
    /// Instances come from YOLOX person boxes; faces without a box (close-ups, people seen from
    /// behind a partner) seed a synthetic box. Each instance is EfficientSAM prompted with its
    /// box plus the face centre as a positive point; overlaps go to the most confident
    /// instance, then each mask is guided-refined at full resolution.
    pub fn people(&mut self, img: RgbImage, with_parts: bool) -> Result<Vec<Person>, String> {
        self.timings.clear();
        let (w, h) = (img.width, img.height);
        let t = Instant::now();
        let boxes = self.detect_people(img)?;
        self.timings.push(("person_det", ms(t)));
        let t = Instant::now();
        let faces = self.detect_faces(img)?;
        self.timings.push(("face_det", ms(t)));

        let mut people = assign_faces(&boxes, &faces, w as f32, h as f32);
        if std::env::var("SIEVE_SEG_DEBUG").is_ok() {
            eprintln!("[segment] person boxes {boxes:?}");
            eprintln!("[segment] faces {:?}", faces.iter().map(|f| (f.bbox, f.score)).collect::<Vec<_>>());
        }
        people.truncate(MAX_PEOPLE);
        if people.is_empty() {
            return Ok(Vec::new());
        }

        // SAM image embedding (square stretch; prompts live in the working-res frame).
        let t = Instant::now();
        if self.sam_enc.is_none() {
            let dims = [("batch", 1), ("height", SAM_SIZE as i64), ("width", SAM_SIZE as i64)];
            self.sam_enc = Some(coreml_session(&self.cfg, SAM_ENCODER_MODEL, &dims)?);
        }
        if self.sam_dec.is_none() {
            self.sam_dec = Some(cpu_session(&self.cfg, SAM_DECODER_MODEL)?);
        }
        self.timings.push(("sam_load", ms(t)));
        let t = Instant::now();
        let sq = self.resize(img, None, SAM_SIZE, SAM_SIZE)?;
        let input: Vec<f32> = to_nchw(&sq, SAM_SIZE * SAM_SIZE, [0.0; 3], [1.0; 3], false);
        let (enc, _) = self.sam_enc.as_mut().expect("loaded");
        let tensor =
            TensorRef::from_array_view(([1usize, 3, SAM_SIZE, SAM_SIZE], &input[..])).map_err(|e| e.to_string())?;
        let outputs = enc.run(ort::inputs![tensor]).map_err(|e| format!("sam encoder: {e}"))?;
        let (_, emb) = outputs[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
        if emb.len() != 256 * 64 * 64 {
            return Err(format!("sam encoder: unexpected output size {}", emb.len()));
        }
        let emb = emb.to_vec();
        drop(outputs);
        self.timings.push(("sam_encode", ms(t)));

        // Decode each instance at the working resolution.
        let t = Instant::now();
        let ws = WORK_EDGE as f32 / w.max(h) as f32;
        let (ww, wh) = (((w as f32 * ws).round() as usize).max(1), ((h as f32 * ws).round() as usize).max(1));
        let (sx, sy) = (ww as f32 / w as f32, wh as f32 / h as f32);
        let mut logits: Vec<Vec<f32>> = Vec::with_capacity(people.len());
        let centre = |f: &Detection| ((f.bbox[0] + f.bbox[2]) * 0.5, (f.bbox[1] + f.bbox[3]) * 0.5);
        for (pi, p) in people.iter().enumerate() {
            let mut pts = vec![p.bbox[0] * sx, p.bbox[1] * sy, p.bbox[2] * sx, p.bbox[3] * sy];
            let mut labels = vec![2.0f32, 3.0];
            if let Some(f) = &p.face {
                let (cx, cy) = centre(f);
                pts.extend([cx * sx, cy * sy]);
                labels.push(1.0);
            }
            // Other people's faces inside this box are negative points: without them a box
            // around an embracing couple segments both.
            for (oi, o) in people.iter().enumerate() {
                if let (true, Some(f)) = (oi != pi, &o.face) {
                    let (cx, cy) = centre(f);
                    if cx > p.bbox[0] && cx < p.bbox[2] && cy > p.bbox[1] && cy < p.bbox[3] {
                        pts.extend([cx * sx, cy * sy]);
                        labels.push(0.0);
                    }
                }
            }
            let n = labels.len();
            let dec = self.sam_dec.as_mut().expect("loaded");
            let size = [wh as i64, ww as i64];
            let outputs = dec
                .run(ort::inputs![
                    TensorRef::from_array_view(([1usize, 256, 64, 64], &emb[..])).map_err(|e| e.to_string())?,
                    TensorRef::from_array_view(([1usize, 1, n, 2], &pts[..])).map_err(|e| e.to_string())?,
                    TensorRef::from_array_view(([1usize, 1, n], &labels[..])).map_err(|e| e.to_string())?,
                    TensorRef::from_array_view(([2usize], &size[..])).map_err(|e| e.to_string())?
                ])
                .map_err(|e| format!("sam decoder: {e}"))?;
            let (_, masks) = outputs[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
            let (_, iou) = outputs[1].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
            let plane = ww * wh;
            let k = iou.len().min(masks.len() / plane.max(1));
            if k == 0 {
                return Err("sam decoder: empty output".into());
            }
            let best = (0..k).max_by(|&a, &b| iou[a].total_cmp(&iou[b])).unwrap_or(0);
            let mut m = masks[best * plane..(best + 1) * plane].to_vec();
            // Keep the instance inside its (padded) prompt box: SAM sometimes leaks into a
            // neighbour of similar colour.
            let pad = 0.08 * (p.bbox[3] - p.bbox[1]).max(p.bbox[2] - p.bbox[0]);
            let (bx0, by0) = (((p.bbox[0] - pad) * sx).max(0.0), ((p.bbox[1] - pad) * sy).max(0.0));
            let (bx1, by1) = ((p.bbox[2] + pad) * sx, (p.bbox[3] + pad) * sy);
            for y in 0..wh {
                for x in 0..ww {
                    let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                    if fx < bx0 || fy < by0 || fx > bx1 || fy > by1 {
                        m[y * ww + x] = -20.0;
                    }
                }
            }
            if std::env::var("SIEVE_SEG_DEBUG").is_ok() {
                let fg = m.iter().filter(|&&v| v > 0.0).count();
                eprintln!(
                    "[segment] person {pi} box {:?} face {} labels {labels:?} iou {:?} best {best} fg {:.1}%",
                    p.bbox,
                    p.face.is_some(),
                    &iou[..k],
                    100.0 * fg as f32 / plane as f32
                );
            }
            logits.push(m);
        }
        self.timings.push(("sam_decode", ms(t)));

        // Exclusive ownership: a pixel claimed by several instances goes to the one with the
        // smallest mask. Overlaps are mostly a large prompt (an embrace, a group) swallowing
        // a partner, and the smaller instance is the more specific one; it is also usually the
        // one in front.
        let t = Instant::now();
        let plane = ww * wh;
        let sizes: Vec<usize> = logits.iter().map(|l| l.iter().filter(|&&v| v > 0.0).count()).collect();
        let mut owner = vec![usize::MAX; plane];
        for (i, v) in owner.iter_mut().enumerate() {
            let mut best = usize::MAX;
            for (k, l) in logits.iter().enumerate() {
                if l[i] > 0.0 && sizes[k] < best {
                    best = sizes[k];
                    *v = k;
                }
            }
        }
        let mut out = Vec::with_capacity(people.len());
        for (k, p) in people.iter().enumerate() {
            let prob: Vec<f32> = (0..plane)
                .map(|i| if owner[i] == k || owner[i] == usize::MAX { sigmoid(logits[k][i]) } else { 0.0 })
                .collect();
            let roi = roi_of(&prob, ww, wh, 0.05, w, h);
            let Some(roi) = roi else { continue };
            let (lr, lw, lh) = crop_plane(&prob, ww, wh, roi, w, h);
            let mask = self.refine(img, roi, &lr, lw, lh, Refine::PERSON)?;
            // Drop slivers: tiny masks, or boxes whose pixels were almost all won by others.
            if mask.area() < 0.0005 * (w * h) as f32 || mask.area() < 0.12 * area(&p.bbox) {
                continue;
            }
            out.push(Person { bbox: p.bbox, score: p.score, from_face: p.from_face, face: p.face, mask, parts: None });
        }
        self.timings.push(("person_refine", ms(t)));

        if with_parts {
            let t = Instant::now();
            for person in out.iter_mut() {
                person.parts = Some(self.person_parts(img, person)?);
            }
            self.timings.push(("parts", ms(t)));
        }
        Ok(out)
    }

    /// Selfie-multiclass probabilities (6 x 256 x 256, softmaxed) for an image crop.
    fn parts_probs(&mut self, img: RgbImage, crop: [usize; 4]) -> Result<Vec<f32>, String> {
        if self.parts.is_none() {
            self.parts = Some(cpu_session(&self.cfg, PARTS_MODEL)?);
        }
        let [x0, y0, x1, y1] = crop;
        let s = PARTS_SIZE;
        let small = self.resize(img, Some([x0 as f64, y0 as f64, (x1 - x0) as f64, (y1 - y0) as f64]), s, s)?;
        // NHWC RGB in [0, 1].
        let input: Vec<f32> = small.iter().map(|&v| v as f32 / 255.0).collect();
        let session = self.parts.as_mut().expect("loaded");
        let tensor = TensorRef::from_array_view(([1usize, s, s, 3], &input[..])).map_err(|e| e.to_string())?;
        let outputs = session.run(ort::inputs![tensor]).map_err(|e| format!("parts: {e}"))?;
        let (_, out) = outputs[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
        if out.len() != s * s * 6 {
            return Err(format!("parts: unexpected output size {}", out.len()));
        }
        // Logits (NHWC) -> planar softmax.
        let mut probs = vec![0.0f32; 6 * s * s];
        for i in 0..s * s {
            let l = &out[i * 6..i * 6 + 6];
            let m = l.iter().copied().fold(f32::MIN, f32::max);
            let e: Vec<f32> = l.iter().map(|&v| (v - m).exp()).collect();
            let sum: f32 = e.iter().sum();
            for c in 0..6 {
                probs[c * s * s + i] = e[c] / sum;
            }
        }
        Ok(probs)
    }

    fn person_parts(&mut self, img: RgbImage, p: &Person) -> Result<PersonParts, String> {
        let (w, h) = (img.width, img.height);
        let roi = [p.mask.x0, p.mask.y0, p.mask.x0 + p.mask.width, p.mask.y0 + p.mask.height];
        let [rx0, ry0, rx1, ry1] = roi;
        let (rw, rh) = (rx1 - rx0, ry1 - ry0);
        // Class planes over the ROI at working resolution. The selfie model expects roughly
        // square, upper-body framing, so tall (or wide) people are parsed in overlapping
        // near-square tiles; the head crop (finer hair / face skin) is blended in with a
        // higher weight. Tiles are feathered and normalised by the summed weight.
        let k = (WORK_EDGE as f32 / rw.max(rh) as f32).min(1.0);
        let (lw, lh) = (((rw as f32 * k).round() as usize).max(1), ((rh as f32 * k).round() as usize).max(1));
        let mut crops: Vec<([usize; 4], f32)> = tiles(roi).into_iter().map(|c| (c, 1.0)).collect();
        if let Some(f) = &p.face {
            let fs = (f.bbox[2] - f.bbox[0]).max(f.bbox[3] - f.bbox[1]);
            let (cx, cy) = ((f.bbox[0] + f.bbox[2]) * 0.5, (f.bbox[1] + f.bbox[3]) * 0.5 - 0.2 * fs);
            let half = 1.4 * fs;
            let hc = [
                (cx - half).max(0.0) as usize,
                (cy - half).max(0.0) as usize,
                ((cx + half) as usize).min(w),
                ((cy + half) as usize).min(h),
            ];
            // Only when the head is clearly smaller than the person crop (else it is the crop).
            if hc[2] > hc[0] + 8 && hc[3] > hc[1] + 8 && (hc[2] - hc[0]) * 3 < rw.max(rh) * 2 {
                crops.push((hc, 4.0));
            }
        }
        let n = lw * lh;
        let mut planes = vec![0.0f32; 6 * n];
        let mut wsum = vec![0.0f32; n];
        for (crop, prio) in crops {
            let probs = self.parts_probs(img, crop)?;
            let (cw, ch) = ((crop[2] - crop[0]) as f32, (crop[3] - crop[1]) as f32);
            let ss = PARTS_SIZE * PARTS_SIZE;
            for y in 0..lh {
                for x in 0..lw {
                    let u = (rx0 as f32 + (x as f32 + 0.5) / k - crop[0] as f32) / cw;
                    let v = (ry0 as f32 + (y as f32 + 0.5) / k - crop[1] as f32) / ch;
                    if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                        continue;
                    }
                    let edge = u.min(1.0 - u).min(v).min(1.0 - v);
                    let wgt = prio * (edge / 0.15).clamp(0.02, 1.0);
                    let (su, sv) = (u * PARTS_SIZE as f32 - 0.5, v * PARTS_SIZE as f32 - 0.5);
                    let i = y * lw + x;
                    for c in 0..6 {
                        planes[c * n + i] += wgt * sample(&probs[c * ss..(c + 1) * ss], PARTS_SIZE, PARTS_SIZE, su, sv);
                    }
                    wsum[i] += wgt;
                }
            }
        }
        for (i, &ws) in wsum.iter().enumerate() {
            if ws > 0.0 {
                (0..6).for_each(|c| planes[c * n + i] /= ws);
            } else {
                planes[i] = 1.0; // uncovered: background
            }
        }
        // Person alpha at the same working grid.
        let alpha: Vec<f32> = (0..n)
            .map(|i| {
                let (x, y) = (i % lw, i / lw);
                let ix = ((rx0 as f32 + (x as f32 + 0.5) / k) as usize).min(w - 1);
                let iy = ((ry0 as f32 + (y as f32 + 0.5) / k) as usize).min(h - 1);
                p.mask.at(ix, iy)
            })
            .collect();
        // Face skin far from the detected face is body skin (arms/hands get confused with
        // faces); with no face, trust the model.
        let near_face: Vec<f32> = match &p.face {
            Some(f) => {
                let fs = (f.bbox[2] - f.bbox[0]).max(f.bbox[3] - f.bbox[1]);
                let (cx, cy) = ((f.bbox[0] + f.bbox[2]) * 0.5, (f.bbox[1] + f.bbox[3]) * 0.5);
                (0..n)
                    .map(|i| {
                        let ix = rx0 as f32 + ((i % lw) as f32 + 0.5) / k;
                        let iy = ry0 as f32 + ((i / lw) as f32 + 0.5) / k;
                        let d = ((ix - cx).powi(2) + (iy - cy).powi(2)).sqrt() / fs;
                        1.0 - smoothstep(0.7, 1.1, d)
                    })
                    .collect()
            }
            None => vec![1.0; n],
        };
        let hair_p: Vec<f32> = (0..n).map(|i| planes[n + i] * alpha[i]).collect();
        let body_p: Vec<f32> =
            (0..n).map(|i| (planes[2 * n + i] + planes[3 * n + i] * (1.0 - near_face[i])) * alpha[i]).collect();
        let face_p: Vec<f32> = (0..n).map(|i| planes[3 * n + i] * near_face[i] * alpha[i]).collect();
        // Clothes also takes accessories (turbans, hats, bags) and the parts of the person
        // instance the parser calls background, so the parts cover the whole person.
        let clothes_p: Vec<f32> =
            (0..n).map(|i| (planes[4 * n + i] + planes[5 * n + i] + planes[i]) * alpha[i]).collect();
        let hair = self.refine(img, roi, &hair_p, lw, lh, Refine::PART)?;
        let body_skin = self.refine(img, roi, &body_p, lw, lh, Refine::PART)?;
        let mut face_skin = self.refine(img, roi, &face_p, lw, lh, Refine::PART)?;
        let clothes = self.refine(img, roi, &clothes_p, lw, lh, Refine::PART)?;

        // Facial features from FaceMesh polygons.
        let empty = || Mask::zeros(0, 0, 0, 0);
        let (mut sclera, mut iris, mut brows, mut lips, mut teeth) = (empty(), empty(), empty(), empty(), empty());
        if let Some(f) = p.face.filter(|f| f.bbox[3] - f.bbox[1] >= MIN_MESH_FACE_PX) {
            let models = self.faces.as_mut().expect("faces loaded by people()");
            let (mesh, m) = models.face_mesh(img.data, w, h, &f)?;
            let pts: Vec<[f32; 2]> = mesh
                .iter()
                .map(|q| {
                    let (x, y) = m.apply(q[0], q[1]);
                    [x, y]
                })
                .collect();
            let fs = (f.bbox[2] - f.bbox[0]).max(f.bbox[3] - f.bbox[1]);
            let froi = [
                (f.bbox[0] - 0.3 * fs).max(0.0) as usize,
                (f.bbox[1] - 0.3 * fs).max(0.0) as usize,
                ((f.bbox[2] + 0.3 * fs) as usize).min(w),
                ((f.bbox[3] + 0.3 * fs) as usize).min(h),
            ];
            let poly = |idx: &[usize]| -> Vec<[f32; 2]> { idx.iter().map(|&i| pts[i]).collect() };
            let eyes = union(&raster_poly(&poly(&EYE_R), froi), &raster_poly(&poly(&EYE_L), froi));
            let discs = union(&raster_disc(&pts, &IRIS_R, froi), &raster_disc(&pts, &IRIS_L, froi));
            iris = intersect(&discs, &eyes);
            sclera = subtract(&eyes, &iris);
            brows = union(&raster_poly(&poly(&BROW_R), froi), &raster_poly(&poly(&BROW_L), froi));
            let inner = raster_poly(&poly(&LIPS_INNER), froi);
            lips = subtract(&raster_poly(&poly(&LIPS_OUTER), froi), &inner);
            teeth = teeth_mask(img, &inner);
            for feat in [&eyes, &brows, &lips, &inner] {
                face_skin = subtract_into(face_skin, feat);
            }
        }
        Ok(PersonParts { hair, face_skin, body_skin, clothes, sclera, iris, brows, lips, teeth })
    }

    // -----------------------------------------------------------------------------------
    // Guided refinement
    // -----------------------------------------------------------------------------------

    /// Upsamples `p` (`pw x ph`, covering `roi` of the image) to a full-resolution [`Mask`]
    /// over `roi` with a fast colour guided filter.
    fn refine(
        &mut self,
        img: RgbImage,
        roi: [usize; 4],
        p: &[f32],
        pw: usize,
        ph: usize,
        r: Refine,
    ) -> Result<Mask, String> {
        let [x0, y0, x1, y1] = roi;
        let (rw, rh) = (x1 - x0, y1 - y0);
        let k = (WORK_EDGE as f32 / rw.max(rh) as f32).min(1.0);
        let (lw, lh) = (((rw as f32 * k).round() as usize).max(1), ((rh as f32 * k).round() as usize).max(1));
        let guide = self.resize(img, Some([x0 as f64, y0 as f64, rw as f64, rh as f64]), lw, lh)?;
        let p_lr = if (pw, ph) == (lw, lh) { p.to_vec() } else { resize_plane(p, pw, ph, lw, lh) };
        let radius = ((r.radius * lw.max(lh) as f32).round() as usize).max(1);
        let (ca, cb) = guided_coeffs(&guide, &p_lr, lw, lh, radius, r.eps);
        let mut out = Mask::zeros(x0, y0, rw, rh);
        let (fx, fy) = (lw as f32 / rw as f32, lh as f32 / rh as f32);
        for y in 0..rh {
            let v = (y as f32 + 0.5) * fy - 0.5;
            let row = ((y + y0) * img.width + x0) * 3;
            for x in 0..rw {
                let u = (x as f32 + 0.5) * fx - 0.5;
                let px = &img.data[row + x * 3..row + x * 3 + 3];
                let mut q = sample(&cb, lw, lh, u, v);
                for c in 0..3 {
                    q += sample(&ca[c * lw * lh..(c + 1) * lw * lh], lw, lh, u, v) * (px[c] as f32 / 255.0);
                }
                out.data[y * rw + x] = q.clamp(0.0, 1.0);
            }
        }
        if r.gamma != 1.0 {
            // Contrast around 0.5 to counter the guided filter's softening of confident masks.
            out.data.iter_mut().for_each(|v| *v = contrast(*v, r.gamma));
        }
        Ok(out)
    }
}

/// Guided-filter parameters: window radius as a fraction of the working long edge, and eps
/// (regularisation on [0,1] intensities: larger = smoother, follows the input mask more).
#[derive(Clone, Copy)]
struct Refine {
    radius: f32,
    eps: f32,
    gamma: f32,
}

impl Refine {
    const SUBJECT: Refine = Refine { radius: 0.004, eps: 1e-4, gamma: 1.0 };
    const SKY: Refine = Refine { radius: 0.012, eps: 1e-4, gamma: 1.5 };
    const PERSON: Refine = Refine { radius: 0.006, eps: 1e-4, gamma: 1.3 };
    const PART: Refine = Refine { radius: 0.008, eps: 5e-4, gamma: 1.3 };
}

/// S-curve around 0.5: `g > 1` pushes values towards 0 / 1.
fn contrast(v: f32, g: f32) -> f32 {
    if v <= 0.5 {
        0.5 * (2.0 * v).powf(g)
    } else {
        1.0 - 0.5 * (2.0 * (1.0 - v)).powf(g)
    }
}

/// Near-square, 25%-overlapping tiles covering `roi` along its long axis.
fn tiles(roi: [usize; 4]) -> Vec<[usize; 4]> {
    let [x0, y0, x1, y1] = roi;
    let (w, h) = (x1 - x0, y1 - y0);
    let (long, short) = (w.max(h), w.min(h));
    if long as f32 <= 1.3 * short as f32 {
        return vec![roi];
    }
    let side = ((short as f32 * 1.15) as usize).min(long);
    let n = ((long - side) as f32 / (0.75 * side as f32)).ceil() as usize + 1;
    (0..n)
        .map(|i| {
            let off = if n > 1 { (long - side) * i / (n - 1) } else { 0 };
            if h >= w {
                [x0, y0 + off, x1, y0 + off + side]
            } else {
                [x0 + off, y0, x0 + off + side, y1]
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// Person / face assignment
// ---------------------------------------------------------------------------------------

struct Candidate {
    bbox: [f32; 4],
    score: f32,
    from_face: bool,
    face: Option<Detection>,
}

/// Matches faces to person boxes and turns unmatched faces into synthetic person boxes.
/// Largest first.
///
/// A face fits a box when its centre is inside the box and in its upper 60%; the cost is how
/// far the face sits from where a head would be (top-centre of the box, in face heights /
/// box widths). Pairs are taken greedily by cost, so in an embrace the partner's box, which
/// also contains the face, does not steal it.
fn assign_faces(boxes: &[([f32; 4], f32)], faces: &[Detection], w: f32, h: f32) -> Vec<Candidate> {
    let mut cands: Vec<Candidate> = boxes
        .iter()
        .filter(|(b, _)| b[3] - b[1] >= MIN_PERSON_H * h)
        .map(|&(bbox, score)| Candidate { bbox, score, from_face: false, face: None })
        .collect();
    let faces: Vec<&Detection> = faces.iter().filter(|f| f.bbox[3] - f.bbox[1] >= MIN_SEED_FACE * h).collect();
    let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
    for (fi, f) in faces.iter().enumerate() {
        let (fx, fy) = ((f.bbox[0] + f.bbox[2]) * 0.5, (f.bbox[1] + f.bbox[3]) * 0.5);
        let fh = f.bbox[3] - f.bbox[1];
        for (ci, c) in cands.iter().enumerate() {
            let b = c.bbox;
            let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
            let fits = fx >= b[0] && fx <= b[2] && fy >= b[1] - 0.2 * fh && fy <= b[1] + 0.6 * bh && fh >= 0.05 * bh;
            if fits {
                let cost = (fy - (b[1] + 0.6 * fh)).abs() / fh + (fx - (b[0] + b[2]) * 0.5).abs() / bw.max(1.0);
                pairs.push((cost, fi, ci));
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut face_used = vec![false; faces.len()];
    for (_, fi, ci) in pairs {
        if !face_used[fi] && cands[ci].face.is_none() {
            cands[ci].face = Some(*faces[fi]);
            face_used[fi] = true;
        }
    }
    for (fi, f) in faces.iter().enumerate() {
        if face_used[fi] {
            continue;
        }
        let (fx, fh) = ((f.bbox[0] + f.bbox[2]) * 0.5, f.bbox[3] - f.bbox[1]);
        let s = (f.bbox[2] - f.bbox[0]).max(fh);
        let bbox = [
            (fx - 2.0 * s).max(0.0),
            (f.bbox[1] - 0.8 * s).max(0.0),
            (fx + 2.0 * s).min(w),
            (f.bbox[3] + 6.0 * s).min(h),
        ];
        cands.push(Candidate { bbox, score: f.score * 0.9, from_face: true, face: Some(**f) });
    }
    cands.sort_by(|a, b| area(&b.bbox).total_cmp(&area(&a.bbox)));
    cands
}

fn area(b: &[f32; 4]) -> f32 {
    (b[2] - b[0]).max(0.0) * (b[3] - b[1]).max(0.0)
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let ix = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let iy = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let inter = ix * iy;
    let u = area(a) + area(b) - inter;
    if u > 0.0 {
        inter / u
    } else {
        0.0
    }
}

/// Decodes the official YOLOX ONNX output (`[1, 8400, 85]` at 640: xywh offsets, objectness,
/// 80 class scores, sigmoids already applied) into class-0 (person) boxes in input pixels.
pub fn decode_yolox(out: &[f32], size: usize, min_score: f32) -> Result<Vec<([f32; 4], f32)>, String> {
    const C: usize = 85;
    let n: usize = [8, 16, 32].iter().map(|s| (size / s) * (size / s)).sum();
    if out.len() != n * C {
        return Err(format!("person detector: unexpected output size {}", out.len()));
    }
    let mut dets = Vec::new();
    let mut i = 0;
    for stride in [8usize, 16, 32] {
        let g = size / stride;
        for gy in 0..g {
            for gx in 0..g {
                let r = &out[i * C..(i + 1) * C];
                i += 1;
                let score = r[4] * r[5];
                if score < min_score {
                    continue;
                }
                let s = stride as f32;
                let (cx, cy) = ((r[0] + gx as f32) * s, (r[1] + gy as f32) * s);
                let (bw, bh) = (r[2].exp() * s, r[3].exp() * s);
                dets.push(([cx - bw / 2.0, cy - bh / 2.0, cx + bw / 2.0, cy + bh / 2.0], score));
            }
        }
    }
    dets.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut keep: Vec<([f32; 4], f32)> = Vec::new();
    for d in dets {
        if keep.iter().all(|k| iou(&k.0, &d.0) <= PERSON_NMS) {
            keep.push(d);
        }
    }
    Ok(keep)
}

// ---------------------------------------------------------------------------------------
// Pixel helpers
// ---------------------------------------------------------------------------------------

/// Interleaved RGB8 -> planar NCHW f32 `(v / 255 - mean) / std`.
fn to_nchw(rgb: &[u8], plane: usize, mean: [f32; 3], std: [f32; 3], bgr: bool) -> Vec<f32> {
    let mut out = vec![0.0f32; 3 * plane];
    for i in 0..plane {
        for c in 0..3 {
            let src = if bgr { 2 - c } else { c };
            out[c * plane + i] = (rgb[i * 3 + src] as f32 / 255.0 - mean[c]) / std[c];
        }
    }
    out
}

/// Bilinear sample with clamp-to-edge at continuous pixel coordinates (pixel centres at
/// integers).
#[inline]
fn sample(p: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x as usize, y as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let top = p[y0 * w + x0] * (1.0 - fx) + p[y0 * w + x1] * fx;
    let bot = p[y1 * w + x0] * (1.0 - fx) + p[y1 * w + x1] * fx;
    top * (1.0 - fy) + bot * fy
}

/// Bilinear resize of a float plane (half-pixel centres). Downscaling by more than 2x
/// box-averages first to avoid aliasing.
fn resize_plane(p: &[f32], w: usize, h: usize, dw: usize, dh: usize) -> Vec<f32> {
    let (fx, fy) = (w as f32 / dw as f32, h as f32 / dh as f32);
    let mut out = vec![0.0f32; dw * dh];
    for y in 0..dh {
        for x in 0..dw {
            let (cx, cy) = ((x as f32 + 0.5) * fx - 0.5, (y as f32 + 0.5) * fy - 0.5);
            out[y * dw + x] = if fx > 2.0 || fy > 2.0 {
                // Average a small grid of bilinear taps over the footprint.
                let (nx, ny) = (fx.ceil() as usize, fy.ceil() as usize);
                let mut s = 0.0;
                for j in 0..ny {
                    for i in 0..nx {
                        let sx = cx - fx / 2.0 + (i as f32 + 0.5) * fx / nx as f32;
                        let sy = cy - fy / 2.0 + (j as f32 + 0.5) * fy / ny as f32;
                        s += sample(p, w, h, sx, sy);
                    }
                }
                s / (nx * ny) as f32
            } else {
                sample(p, w, h, cx, cy)
            };
        }
    }
    out
}

/// Bounding ROI (image px, padded by `pad` of its size) of `prob > 0.1` on a `ww x wh` plane
/// covering the whole `w x h` image.
fn roi_of(prob: &[f32], ww: usize, wh: usize, pad: f32, w: usize, h: usize) -> Option<[usize; 4]> {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..wh {
        for x in 0..ww {
            if prob[y * ww + x] > 0.1 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x0 == usize::MAX {
        return None;
    }
    let (sx, sy) = (w as f32 / ww as f32, h as f32 / wh as f32);
    let (px, py) = (pad * (x1 - x0) as f32 * sx + 2.0 * sx, pad * (y1 - y0) as f32 * sy + 2.0 * sy);
    let r = [
        (x0 as f32 * sx - px).max(0.0) as usize,
        (y0 as f32 * sy - py).max(0.0) as usize,
        ((x1 as f32 * sx + px).ceil() as usize).min(w),
        ((y1 as f32 * sy + py).ceil() as usize).min(h),
    ];
    (r[2] > r[0] + 1 && r[3] > r[1] + 1).then_some(r)
}

/// Crops the image-space `roi` out of a whole-image `ww x wh` plane (at that plane's
/// resolution).
fn crop_plane(p: &[f32], ww: usize, wh: usize, roi: [usize; 4], w: usize, h: usize) -> (Vec<f32>, usize, usize) {
    let (sx, sy) = (ww as f32 / w as f32, wh as f32 / h as f32);
    let lw = (((roi[2] - roi[0]) as f32 * sx).round() as usize).max(1);
    let lh = (((roi[3] - roi[1]) as f32 * sy).round() as usize).max(1);
    let mut out = vec![0.0f32; lw * lh];
    let (kx, ky) = ((roi[2] - roi[0]) as f32 / lw as f32, (roi[3] - roi[1]) as f32 / lh as f32);
    for y in 0..lh {
        for x in 0..lw {
            let ix = roi[0] as f32 + (x as f32 + 0.5) * kx;
            let iy = roi[1] as f32 + (y as f32 + 0.5) * ky;
            out[y * lw + x] = sample(p, ww, wh, ix * sx - 0.5, iy * sy - 0.5);
        }
    }
    (out, lw, lh)
}

/// Mean over a `(2r+1)^2` window, clamped at the borders (divides by the valid count).
fn box_mean(p: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0.0f32; w * h];
    let mut row = vec![0.0f64; w + 1];
    for y in 0..h {
        for x in 0..w {
            row[x + 1] = row[x] + p[y * w + x] as f64;
        }
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r + 1).min(w));
            tmp[y * w + x] = ((row[b] - row[a]) / (b - a) as f64) as f32;
        }
    }
    let mut out = vec![0.0f32; w * h];
    let mut col = vec![0.0f64; h + 1];
    for x in 0..w {
        for y in 0..h {
            col[y + 1] = col[y] + tmp[y * w + x] as f64;
        }
        for y in 0..h {
            let (a, b) = (y.saturating_sub(r), (y + r + 1).min(h));
            out[y * w + x] = ((col[b] - col[a]) / (b - a) as f64) as f32;
        }
    }
    out
}

/// Colour guided filter coefficients (He, Sun & Tang 2013, eq. 19-21): returns the
/// box-averaged `a` (3 planes) and `b` so that `q = a . I + b`, with `I` in [0, 1].
fn guided_coeffs(guide: &[u8], p: &[f32], w: usize, h: usize, r: usize, eps: f32) -> (Vec<f32>, Vec<f32>) {
    let n = w * h;
    let ch: Vec<Vec<f32>> = (0..3).map(|c| (0..n).map(|i| guide[i * 3 + c] as f32 / 255.0).collect()).collect();
    let mean = |v: &[f32]| box_mean(v, w, h, r);
    let prod = |a: &[f32], b: &[f32]| -> Vec<f32> { a.iter().zip(b).map(|(x, y)| x * y).collect() };
    let m_i: Vec<Vec<f32>> = ch.iter().map(|c| mean(c)).collect();
    let m_p = mean(p);
    let m_ip: Vec<Vec<f32>> = ch.iter().map(|c| mean(&prod(c, p))).collect();
    // Covariance entries rr, rg, rb, gg, gb, bb.
    let pairs = [(0, 0), (0, 1), (0, 2), (1, 1), (1, 2), (2, 2)];
    let var: Vec<Vec<f32>> = pairs.iter().map(|&(a, b)| mean(&prod(&ch[a], &ch[b]))).collect();
    let mut a = vec![0.0f32; 3 * n];
    let mut b = vec![0.0f32; n];
    for i in 0..n {
        let mi = [m_i[0][i], m_i[1][i], m_i[2][i]];
        let cov = [m_ip[0][i] - mi[0] * m_p[i], m_ip[1][i] - mi[1] * m_p[i], m_ip[2][i] - mi[2] * m_p[i]];
        let s = |k: usize, x: usize, y: usize| var[k][i] - mi[x] * mi[y];
        let (rr, rg, rb, gg, gb, bb) =
            (s(0, 0, 0) + eps, s(1, 0, 1), s(2, 0, 2), s(3, 1, 1) + eps, s(4, 1, 2), s(5, 2, 2) + eps);
        // Inverse of the symmetric 3x3 via the adjugate.
        let inv = [
            gg * bb - gb * gb,
            gb * rb - rg * bb,
            rg * gb - gg * rb,
            rr * bb - rb * rb,
            rg * rb - rr * gb,
            rr * gg - rg * rg,
        ];
        let det = rr * inv[0] + rg * inv[1] + rb * inv[2];
        let (ar, ag, ab) = if det.abs() > 1e-12 {
            (
                (inv[0] * cov[0] + inv[1] * cov[1] + inv[2] * cov[2]) / det,
                (inv[1] * cov[0] + inv[3] * cov[1] + inv[4] * cov[2]) / det,
                (inv[2] * cov[0] + inv[4] * cov[1] + inv[5] * cov[2]) / det,
            )
        } else {
            (0.0, 0.0, 0.0)
        };
        a[i] = ar;
        a[n + i] = ag;
        a[2 * n + i] = ab;
        b[i] = m_p[i] - ar * mi[0] - ag * mi[1] - ab * mi[2];
    }
    let mut ma = Vec::with_capacity(3 * n);
    for c in 0..3 {
        ma.extend(mean(&a[c * n..(c + 1) * n]));
    }
    (ma, mean(&b))
}

/// Anti-aliased polygon coverage (4x4 supersampling, even-odd rule) over `roi`.
fn raster_poly(poly: &[[f32; 2]], roi: [usize; 4]) -> Mask {
    let [x0, y0, x1, y1] = roi;
    let mut m = Mask::zeros(x0, y0, x1 - x0, y1 - y0);
    if poly.len() < 3 {
        return m;
    }
    let (mut bx0, mut by0, mut bx1, mut by1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in poly {
        bx0 = bx0.min(p[0]);
        by0 = by0.min(p[1]);
        bx1 = bx1.max(p[0]);
        by1 = by1.max(p[1]);
    }
    let xs = (bx0.floor().max(x0 as f32) as usize, (bx1.ceil() as usize + 1).min(x1));
    let ys = (by0.floor().max(y0 as f32) as usize, (by1.ceil() as usize + 1).min(y1));
    const S: usize = 4;
    for y in ys.0..ys.1 {
        for x in xs.0..xs.1 {
            let mut hits = 0;
            for j in 0..S {
                for i in 0..S {
                    let px = x as f32 + (i as f32 + 0.5) / S as f32;
                    let py = y as f32 + (j as f32 + 0.5) / S as f32;
                    if inside(poly, px, py) {
                        hits += 1;
                    }
                }
            }
            m.data[(y - y0) * m.width + (x - x0)] = hits as f32 / (S * S) as f32;
        }
    }
    m
}

fn inside(poly: &[[f32; 2]], x: f32, y: f32) -> bool {
    let mut c = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > y) != (b[1] > y) && x < (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0] {
            c = !c;
        }
        j = i;
    }
    c
}

/// Iris disc: centre point plus the mean distance to its 4 ring points.
fn raster_disc(pts: &[[f32; 2]], idx: &[usize; 5], roi: [usize; 4]) -> Mask {
    let c = pts[idx[0]];
    let r =
        idx[1..].iter().map(|&i| ((pts[i][0] - c[0]).powi(2) + (pts[i][1] - c[1]).powi(2)).sqrt()).sum::<f32>() / 4.0;
    let poly: Vec<[f32; 2]> = (0..32)
        .map(|k| {
            let a = k as f32 / 32.0 * std::f32::consts::TAU;
            [c[0] + r * a.cos(), c[1] + r * a.sin()]
        })
        .collect();
    raster_poly(&poly, roi)
}

fn combine(a: &Mask, b: &Mask, f: impl Fn(f32, f32) -> f32) -> Mask {
    let mut out = a.clone();
    for y in 0..a.height {
        for x in 0..a.width {
            let i = y * a.width + x;
            out.data[i] = f(a.data[i], b.at(x + a.x0, y + a.y0));
        }
    }
    out
}

fn union(a: &Mask, b: &Mask) -> Mask {
    combine(a, b, f32::max)
}

fn intersect(a: &Mask, b: &Mask) -> Mask {
    combine(a, b, f32::min)
}

fn subtract(a: &Mask, b: &Mask) -> Mask {
    combine(a, b, |x, y| x * (1.0 - y))
}

fn subtract_into(a: Mask, b: &Mask) -> Mask {
    subtract(&a, b)
}

/// Teeth inside the inner-lip polygon: bright, low-saturation pixels relative to the mouth.
fn teeth_mask(img: RgbImage, inner: &Mask) -> Mask {
    let mut lum = Vec::new();
    for y in 0..inner.height {
        for x in 0..inner.width {
            if inner.data[y * inner.width + x] > 0.5 {
                let p = ((y + inner.y0) * img.width + x + inner.x0) * 3;
                let d = &img.data[p..p + 3];
                lum.push(0.299 * d[0] as f32 + 0.587 * d[1] as f32 + 0.114 * d[2] as f32);
            }
        }
    }
    let mut out = inner.clone();
    if lum.len() < 4 {
        out.data.iter_mut().for_each(|v| *v = 0.0);
        return out;
    }
    lum.sort_by(f32::total_cmp);
    let hi = lum[(lum.len() as f32 * 0.95) as usize].max(1.0);
    for y in 0..inner.height {
        for x in 0..inner.width {
            let i = y * inner.width + x;
            if inner.data[i] == 0.0 {
                continue;
            }
            let p = ((y + inner.y0) * img.width + x + inner.x0) * 3;
            let d = &img.data[p..p + 3];
            let (r, g, b) = (d[0] as f32, d[1] as f32, d[2] as f32);
            let l = (0.299 * r + 0.587 * g + 0.114 * b) / hi;
            let mx = r.max(g).max(b);
            let sat = if mx > 0.0 { (mx - r.min(g).min(b)) / mx } else { 0.0 };
            let bright = smoothstep(0.45, 0.7, l);
            let grey = 1.0 - smoothstep(0.3, 0.5, sat);
            out.data[i] *= bright * grey;
        }
    }
    out
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_mean_matches_brute_force() {
        let (w, h, r) = (7, 5, 2);
        let p: Vec<f32> = (0..w * h).map(|i| ((i * 37) % 11) as f32).collect();
        let m = box_mean(&p, w, h, r);
        for y in 0..h {
            for x in 0..w {
                let (mut s, mut n) = (0.0, 0);
                for yy in y.saturating_sub(r)..(y + r + 1).min(h) {
                    for xx in x.saturating_sub(r)..(x + r + 1).min(w) {
                        s += p[yy * w + xx];
                        n += 1;
                    }
                }
                assert!((m[y * w + x] - s / n as f32).abs() < 1e-4);
            }
        }
    }

    /// A blurry, upsampled low-res mask of a sharp colour edge becomes sharp at full
    /// resolution (bilinear alone gives 0.78 / 0.22 at the probe points).
    #[test]
    fn guided_refine_snaps_to_edges() {
        let (w, h) = (1024, 32);
        let edge = 404;
        let mut data = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let p = (y * w + x) * 3;
                // Blue sky left of the edge, green trees right of it.
                data[p..p + 3].copy_from_slice(if x < edge { &[120, 170, 230] } else { &[40, 110, 40] });
            }
        }
        let img = RgbImage { data: &data, width: w, height: h };
        // 1/8-res mask: column 50 straddles the edge (404 / 8 = 50.5) and is 0.5.
        let (pw, ph) = (w / 8, h / 8);
        let p: Vec<f32> = (0..pw * ph)
            .map(|i| match (i % pw).cmp(&50) {
                std::cmp::Ordering::Less => 1.0,
                std::cmp::Ordering::Equal => 0.5,
                std::cmp::Ordering::Greater => 0.0,
            })
            .collect();
        let mut seg = Segmenter::new(SegmentConfig::new("/nonexistent"));
        let m = seg.refine(img, [0, 0, w, h], &p, pw, ph, Refine::SKY).unwrap();
        assert_eq!((m.width, m.height), (w, h));
        let row = &m.data[16 * w..17 * w];
        assert!(row[edge - 4] > 0.9, "sky side {}", row[edge - 4]);
        assert!(row[edge + 4] < 0.1, "tree side {}", row[edge + 4]);
    }

    #[test]
    fn polygon_coverage_is_area() {
        let sq = [[2.0, 2.0], [12.0, 2.0], [12.0, 7.0], [2.0, 7.0]];
        let m = raster_poly(&sq, [0, 0, 16, 10]);
        assert!((m.area() - 50.0).abs() < 0.5, "{}", m.area());
        let tri = [[0.0, 0.0], [8.0, 0.0], [0.0, 7.0]];
        let m = raster_poly(&tri, [0, 0, 16, 10]);
        assert!((m.area() - 28.0).abs() < 1.0, "{}", m.area());
    }

    #[test]
    fn mask_ops_and_export() {
        let a = Mask { x0: 1, y0: 1, width: 2, height: 2, data: vec![1.0, 0.5, 0.0, 1.0] };
        assert_eq!(a.at(0, 0), 0.0);
        assert_eq!(a.at(2, 1), 0.5);
        let inv = a.inverted(4, 3);
        assert_eq!(inv.at(0, 0), 1.0);
        assert_eq!(inv.at(1, 1), 0.0);
        let u8s = a.to_u8(4, 3);
        assert_eq!(u8s[4 + 1], 255);
        assert_eq!(u8s[4 + 2], 128);
        let b = Mask { x0: 0, y0: 0, width: 4, height: 3, data: vec![0.5; 12] };
        assert_eq!(subtract(&a, &b).data, vec![0.5, 0.25, 0.0, 0.5]);
        assert_eq!(intersect(&a, &b).data, vec![0.5, 0.5, 0.0, 0.5]);
    }

    #[test]
    fn yolox_decodes_one_person() {
        let size = 64; // grids 8x8 + 4x4 + 2x2
        let n = 64 + 16 + 4;
        let mut out = vec![0.0f32; n * 85];
        // Anchor (gx=3, gy=2) at stride 8: centre (3.5, 2.5) * 8, size e^1 * 8.
        let i = 2 * 8 + 3;
        out[i * 85..i * 85 + 6].copy_from_slice(&[0.5, 0.5, 1.0, 1.0, 0.9, 0.8]);
        let dets = decode_yolox(&out, size, 0.3).unwrap();
        assert_eq!(dets.len(), 1);
        let (b, s) = dets[0];
        assert!((s - 0.72).abs() < 1e-6);
        let half = 1f32.exp() * 8.0 / 2.0;
        assert!((b[0] - (28.0 - half)).abs() < 1e-3 && (b[3] - (20.0 + half)).abs() < 1e-3);
    }

    /// IMG_5697-like embrace: the partner's (smaller) box also contains the man's face.
    #[test]
    fn faces_match_their_own_box_in_an_embrace() {
        let face = |b: [f32; 4]| Detection { bbox: b, score: 0.8, kps: [[0.0; 2]; 5] };
        let man = [66.0, 162.0, 1363.0, 2022.0];
        let woman = [2.0, 820.0, 1091.0, 2029.0];
        let faces = [face([457.0, 486.0, 985.0, 1233.0]), face([67.0, 942.0, 533.0, 1634.0])];
        let c = assign_faces(&[(man, 0.9), (woman, 0.88)], &faces, 1365.0, 2048.0);
        assert_eq!(c.len(), 2);
        let m = c.iter().find(|c| c.bbox == man).unwrap();
        let w = c.iter().find(|c| c.bbox == woman).unwrap();
        assert_eq!(m.face.unwrap().bbox[0], 457.0);
        assert_eq!(w.face.unwrap().bbox[0], 67.0);
    }

    #[test]
    fn faces_seed_missing_people() {
        let face = |x: f32, y: f32| Detection { bbox: [x, y, x + 40.0, y + 50.0], score: 0.9, kps: [[0.0; 2]; 5] };
        // One detected person with a face in its head region, one face without a box.
        let boxes = [([100.0, 100.0, 300.0, 700.0], 0.9)];
        let faces = [face(180.0, 120.0), face(600.0, 200.0)];
        let c = assign_faces(&boxes, &faces, 1000.0, 800.0);
        assert_eq!(c.len(), 2);
        let det = c.iter().find(|c| !c.from_face).unwrap();
        assert!(det.face.is_some());
        let seeded = c.iter().find(|c| c.from_face).unwrap();
        assert!(seeded.bbox[0] < 600.0 && seeded.bbox[2] > 640.0 && seeded.bbox[3] > 250.0);
    }
}
