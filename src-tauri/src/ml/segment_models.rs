//! Concrete [`SegmentModel`]s over the shared [`SegmentEngine`] (Phase 7c). All models share
//! one engine behind one lock: sessions load lazily on first use and per-image work (subject
//! output, SAM embedding, person instances and parts) is reused across requests.
//!
//! | Family | Model id | Files | Stored (unrefined) output |
//! |---|---|---|---|
//! | subject, background | `birefnet-lite-1024@1` | `birefnet_lite.onnx` | 1024x1024 over the frame (background = 1 - subject) |
//! | sky | `skyseg-u2net-320@1` | `skyseg.onnx` | 320x320 over the frame |
//! | people, object | `people-yoloxm-effsamti-selfiemc@1` | YOLOX-m, EfficientSAM-Ti, SCRFD + landmarks (faces), selfie multiclass (parts) | person / object ROI; parts on the person grid, features at input resolution |
//!
//! People: `referencePoint` selects one person (the one whose face contains the point, else
//! the instance with the highest probability there, else the nearest); `null` = all people.
//! Parts are computed for the selected person(s) only and only the groups asked for: the
//! parser (hair / skin / clothes) and FaceMesh (eyes, brows, lips, teeth; also used to cut
//! features out of face skin).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use super::masking::{PeopleDetector, PersonSummary, RegisteredModel, SegmentInput, SegmentModel, SegmentRequest};
use super::segment::{
    mask_lowres, union_all, Instance, LowRes, Mask, RgbImage, SegmentConfig, SegmentEngine, FACE_MODELS, PARTS_MODEL,
    PERSON_MODEL, SAM_DECODER_MODEL, SAM_ENCODER_MODEL, SKY_MODEL, SUBJECT_MODEL,
};
use crate::develop::masks::AlphaMask;
use crate::ipc::types::{AiTarget, AiTargetKind, NormPoint, NormRect, PersonPart};

pub const SUBJECT_ID: &str = "birefnet-lite-1024@1";
pub const SKY_ID: &str = "skyseg-u2net-320@1";
pub const PEOPLE_ID: &str = "people-yoloxm-effsamti-selfiemc@1";

/// Parts from the selfie-multiclass parser.
pub const REGION_PARTS: [PersonPart; 4] =
    [PersonPart::Hair, PersonPart::FaceSkin, PersonPart::BodySkin, PersonPart::Clothes];
/// Parts from FaceMesh polygons.
pub const FEATURE_PARTS: [PersonPart; 5] =
    [PersonPart::Eyebrows, PersonPart::EyeSclera, PersonPart::IrisPupil, PersonPart::Lips, PersonPart::Teeth];

/// Long-edge cap of a union of several people / parts.
const UNION_EDGE: usize = 2048;

pub type SharedEngine = Arc<Mutex<SegmentEngine>>;

fn lock(e: &SharedEngine) -> MutexGuard<'_, SegmentEngine> {
    e.lock().unwrap_or_else(|p| p.into_inner())
}

/// No I/O: sessions load on first use.
pub fn shared_engine(models_dir: &Path, coreml_cache: Option<PathBuf>) -> SharedEngine {
    let mut cfg = SegmentConfig::new(models_dir);
    cfg.coreml_cache = coreml_cache;
    Arc::new(Mutex::new(SegmentEngine::new(cfg)))
}

/// The installed models with their file requirements (no I/O).
pub fn registry(models_dir: &Path, coreml_cache: Option<PathBuf>) -> Vec<RegisteredModel> {
    let engine = shared_engine(models_dir, coreml_cache);
    let files = |names: &[&str]| names.iter().map(|n| models_dir.join(n)).collect::<Vec<_>>();
    let people = Arc::new(PeopleModel { engine: engine.clone() });
    let mut parts: Vec<(PersonPart, Vec<PathBuf>)> = REGION_PARTS
        .iter()
        .map(|&p| {
            // Face skin also uses FaceMesh to cut out eyes / brows / lips (optional).
            (p, files(&[PARTS_MODEL]))
        })
        .collect();
    parts.extend(FEATURE_PARTS.iter().map(|&p| (p, files(&FACE_MODELS))));
    vec![
        RegisteredModel {
            model: Arc::new(SubjectModel { engine: engine.clone() }),
            required: files(&[SUBJECT_MODEL]),
            parts: Vec::new(),
            people: None,
        },
        RegisteredModel {
            model: Arc::new(SkyModel { engine: engine.clone() }),
            required: files(&[SKY_MODEL]),
            parts: Vec::new(),
            people: None,
        },
        RegisteredModel {
            model: people.clone(),
            required: files(&[PERSON_MODEL, SAM_ENCODER_MODEL, SAM_DECODER_MODEL, FACE_MODELS[0], FACE_MODELS[1], FACE_MODELS[2], FACE_MODELS[3]]),
            parts,
            people: Some(people),
        },
    ]
}

fn rgb<'a>(input: &'a SegmentInput) -> Result<RgbImage<'a>, String> {
    let (w, h) = (input.width as usize, input.height as usize);
    if w == 0 || h == 0 || input.rgb.len() < w * h * 3 {
        return Err(format!("bad input image {w}x{h} ({} bytes)", input.rgb.len()));
    }
    Ok(RgbImage { data: input.rgb, width: w, height: h })
}

/// BiRefNet_lite (MIT): subject and background.
pub struct SubjectModel {
    engine: SharedEngine,
}

impl SegmentModel for SubjectModel {
    fn id(&self) -> &str {
        SUBJECT_ID
    }
    fn families(&self) -> &[AiTargetKind] {
        &[AiTargetKind::Subject, AiTargetKind::Background]
    }
    fn run(&self, request: &SegmentRequest, input: &SegmentInput) -> Result<AlphaMask, String> {
        let img = rgb(input)?;
        let lr = lock(&self.engine).subject_raw(img)?;
        let lr = match request.target {
            AiTarget::Background => lr.inverted(img.width, img.height),
            AiTarget::Subject => (*lr).clone(),
            ref t => return Err(format!("subject model cannot segment {t:?}")),
        };
        Ok(lr.to_alpha(img.width, img.height))
    }
}

/// skyseg U2-Net (MIT).
pub struct SkyModel {
    engine: SharedEngine,
}

impl SegmentModel for SkyModel {
    fn id(&self) -> &str {
        SKY_ID
    }
    fn families(&self) -> &[AiTargetKind] {
        &[AiTargetKind::Sky]
    }
    fn run(&self, request: &SegmentRequest, input: &SegmentInput) -> Result<AlphaMask, String> {
        if request.target != AiTarget::Sky {
            return Err(format!("sky model cannot segment {:?}", request.target));
        }
        let img = rgb(input)?;
        let lr = lock(&self.engine).sky_raw(img)?;
        Ok(lr.to_alpha(img.width, img.height))
    }
}

/// People (YOLOX-m + SCRFD + EfficientSAM-Ti + selfie multiclass + FaceMesh) and objects
/// (EfficientSAM-Ti box prompt).
pub struct PeopleModel {
    engine: SharedEngine,
}

/// Index of the person `p` (image px) selects (module docs).
pub fn select_person(insts: &[Instance], x: f32, y: f32) -> Option<usize> {
    let face_hit = insts
        .iter()
        .enumerate()
        .filter_map(|(i, p)| p.face.map(|f| (i, f.bbox)))
        .filter(|(_, b)| x >= b[0] && x <= b[2] && y >= b[1] && y <= b[3])
        .min_by(|a, b| area(&a.1).total_cmp(&area(&b.1)));
    if let Some((i, _)) = face_hit {
        return Some(i);
    }
    let best = insts.iter().enumerate().map(|(i, p)| (i, p.alpha.at(x, y))).max_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((i, v)) = best {
        if v > 0.3 {
            return Some(i);
        }
    }
    insts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let (cx, cy) = ((p.bbox[0] + p.bbox[2]) * 0.5, (p.bbox[1] + p.bbox[3]) * 0.5);
            (i, (cx - x).powi(2) + (cy - y).powi(2))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

fn area(b: &[f32; 4]) -> f32 {
    (b[2] - b[0]).max(0.0) * (b[3] - b[1]).max(0.0)
}

/// A point on the person that [`select_person`] maps back to it: the face centre, else the
/// confident instance pixel nearest the instance's centroid.
fn reference_of(insts: &[Instance], idx: usize) -> (f32, f32) {
    let p = &insts[idx];
    if let Some(f) = p.face {
        return ((f.bbox[0] + f.bbox[2]) * 0.5, (f.bbox[1] + f.bbox[3]) * 0.5);
    }
    let a = &p.alpha;
    let [x0, y0, x1, y1] = a.roi;
    let (kx, ky) = ((x1 - x0) as f32 / a.width as f32, (y1 - y0) as f32 / a.height as f32);
    let pos = |i: usize| (x0 as f32 + ((i % a.width) as f32 + 0.5) * kx, y0 as f32 + ((i / a.width) as f32 + 0.5) * ky);
    let (mut sx, mut sy, mut sw) = (0.0f64, 0.0f64, 0.0f64);
    for (i, &v) in a.data.iter().enumerate() {
        let (x, y) = pos(i);
        sx += (x * v) as f64;
        sy += (y * v) as f64;
        sw += v as f64;
    }
    if sw <= 0.0 {
        return ((p.bbox[0] + p.bbox[2]) * 0.5, (p.bbox[1] + p.bbox[3]) * 0.5);
    }
    let (cx, cy) = ((sx / sw) as f32, (sy / sw) as f32);
    let mut best = (f32::MAX, (cx, cy));
    for (i, &v) in a.data.iter().enumerate() {
        if v > 0.6 {
            let (x, y) = pos(i);
            // Must also win the selection against everyone else.
            let d = (x - cx).powi(2) + (y - cy).powi(2);
            if d < best.0 && select_person(insts, x, y) == Some(idx) {
                best = (d, (x, y));
            }
        }
    }
    best.1
}

impl PeopleModel {
    /// Unrefined plane of `parts` (empty = whole person) for instance `idx`.
    fn person_plane(eng: &mut SegmentEngine, img: RgbImage, idx: usize, parts: &[PersonPart]) -> Result<LowRes, String> {
        let insts = eng.instances(img)?;
        let inst = &insts[idx];
        if parts.is_empty() {
            return Ok(inst.alpha.clone());
        }
        let want = |p: PersonPart| parts.contains(&p);
        let need_regions = REGION_PARTS.iter().any(|&p| want(p));
        let need_features = FEATURE_PARTS.iter().any(|&p| want(p)) || want(PersonPart::FaceSkin);
        let features = if need_features { eng.features(img, idx)? } else { None };
        let mut planes: Vec<LowRes> = Vec::new();
        if need_regions {
            let r = eng.region_planes(img, idx)?;
            let g = &r.grid;
            let mut acc = vec![0.0f32; g.data.len()];
            for (part, plane) in [
                (PersonPart::Hair, &r.hair),
                (PersonPart::BodySkin, &r.body_skin),
                (PersonPart::Clothes, &r.clothes),
            ] {
                if want(part) {
                    acc.iter_mut().zip(plane).for_each(|(a, v)| *a = a.max(*v));
                }
            }
            if want(PersonPart::FaceSkin) {
                let mut face = LowRes { data: r.face_skin.clone(), ..g.clone() };
                if let Some(f) = &features {
                    // Eyes, brows, lips and mouth are not skin.
                    let cut: Vec<&Mask> = vec![&f.eyes, &f.brows, &f.lips, &f.inner];
                    let [x0, y0, x1, y1] = g.roi;
                    let (kx, ky) = ((x1 - x0) as f32 / g.width as f32, (y1 - y0) as f32 / g.height as f32);
                    for (i, v) in face.data.iter_mut().enumerate() {
                        let x = x0 as f32 + ((i % g.width) as f32 + 0.5) * kx;
                        let y = y0 as f32 + ((i / g.width) as f32 + 0.5) * ky;
                        let m = cut.iter().map(|c| mask_at(c, x, y)).fold(0.0f32, f32::max);
                        *v *= 1.0 - m;
                    }
                }
                acc.iter_mut().zip(&face.data).for_each(|(a, v)| *a = a.max(*v));
            }
            planes.push(LowRes { data: acc, ..g.clone() });
        }
        if let Some(f) = &features {
            for (part, m) in [
                (PersonPart::Eyebrows, &f.brows),
                (PersonPart::EyeSclera, &f.sclera),
                (PersonPart::IrisPupil, &f.iris),
                (PersonPart::Lips, &f.lips),
                (PersonPart::Teeth, &f.teeth),
            ] {
                if want(part) && m.width > 0 && m.height > 0 {
                    planes.push(mask_lowres(m));
                }
            }
        }
        Ok(union_all(&planes, UNION_EDGE).unwrap_or_else(|| LowRes::empty(img.width, img.height)))
    }
}

/// Bilinear value of an image-pixel [`Mask`] at a continuous position.
fn mask_at(m: &Mask, x: f32, y: f32) -> f32 {
    if m.width == 0 || m.height == 0 {
        return 0.0;
    }
    mask_lowres(m).at(x, y)
}

impl SegmentModel for PeopleModel {
    fn id(&self) -> &str {
        PEOPLE_ID
    }
    fn families(&self) -> &[AiTargetKind] {
        &[AiTargetKind::People, AiTargetKind::Object]
    }
    fn run(&self, request: &SegmentRequest, input: &SegmentInput) -> Result<AlphaMask, String> {
        let img = rgb(input)?;
        let (w, h) = (img.width, img.height);
        let mut eng = lock(&self.engine);
        let lr = match &request.target {
            AiTarget::Object { region } => {
                let b = [region.x * w as f32, region.y * h as f32, (region.x + region.width) * w as f32, (region.y + region.height) * h as f32];
                eng.object_raw(img, b)?
            }
            AiTarget::People { parts } => {
                let insts = eng.instances(img)?;
                let chosen: Vec<usize> = match request.reference_point {
                    Some(p) => select_person(&insts, p.x * w as f32, p.y * h as f32).into_iter().collect(),
                    None => (0..insts.len()).collect(),
                };
                let mut planes = Vec::with_capacity(chosen.len());
                for idx in chosen {
                    planes.push(Self::person_plane(&mut eng, img, idx, parts)?);
                }
                union_all(&planes, UNION_EDGE).unwrap_or_else(|| LowRes::empty(w, h))
            }
            t => return Err(format!("people model cannot segment {t:?}")),
        };
        Ok(lr.to_alpha(w, h))
    }
}

impl PeopleDetector for PeopleModel {
    fn people(&self, input: &SegmentInput) -> Result<Vec<PersonSummary>, String> {
        let img = rgb(input)?;
        let (w, h) = (img.width as f32, img.height as f32);
        let norm = |b: [f32; 4]| NormRect {
            x: (b[0] / w).clamp(0.0, 1.0),
            y: (b[1] / h).clamp(0.0, 1.0),
            width: ((b[2] - b[0]) / w).clamp(0.0, 1.0),
            height: ((b[3] - b[1]) / h).clamp(0.0, 1.0),
        };
        let insts = lock(&self.engine).instances(img)?;
        Ok((0..insts.len())
            .map(|i| {
                let p = &insts[i];
                let (rx, ry) = reference_of(&insts, i);
                PersonSummary {
                    bbox: norm(p.alpha.tight_box(0.5).unwrap_or(p.bbox)),
                    face: p.face.map(|f| norm(f.bbox)),
                    reference_point: NormPoint { x: (rx / w).clamp(0.0, 1.0), y: (ry / h).clamp(0.0, 1.0) },
                    features_available: p.has_features(),
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ml::models::Detection;

    fn inst(bbox: [f32; 4], face: Option<[f32; 4]>) -> Instance {
        let roi = [bbox[0] as usize, bbox[1] as usize, bbox[2] as usize, bbox[3] as usize];
        Instance {
            bbox,
            score: 0.9,
            from_face: false,
            face: face.map(|b| Detection { bbox: b, score: 0.9, kps: [[0.0; 2]; 5] }),
            alpha: LowRes { roi, width: 4, height: 4, data: vec![1.0; 16] },
        }
    }

    #[test]
    fn person_selection_prefers_faces_then_alpha_then_distance() {
        // Two overlapping people; the point on B's face lies inside A's alpha too.
        let a = inst([0.0, 0.0, 100.0, 200.0], Some([30.0, 10.0, 70.0, 50.0]));
        let b = inst([60.0, 20.0, 160.0, 200.0], Some([90.0, 30.0, 120.0, 60.0]));
        let insts = vec![a, b];
        assert_eq!(select_person(&insts, 100.0, 40.0), Some(1));
        assert_eq!(select_person(&insts, 40.0, 150.0), Some(0));
        assert_eq!(select_person(&insts, 150.0, 150.0), Some(1));
        // Outside everyone: nearest centre.
        assert_eq!(select_person(&insts, 400.0, 100.0), Some(1));
        assert_eq!(select_person(&[], 1.0, 1.0), None);
        // Reference points round-trip.
        for i in 0..2 {
            let (x, y) = reference_of(&insts, i);
            assert_eq!(select_person(&insts, x, y), Some(i));
        }
        let no_face = vec![inst([0.0, 0.0, 100.0, 200.0], None), inst([60.0, 20.0, 160.0, 200.0], None)];
        for i in 0..2 {
            let (x, y) = reference_of(&no_face, i);
            assert_eq!(select_person(&no_face, x, y), Some(i), "person {i} at {x},{y}");
        }
    }

    #[test]
    fn registry_reports_files() {
        let reg = registry(Path::new("/nonexistent-models"), None);
        let fams: Vec<AiTargetKind> = reg.iter().flat_map(|m| m.model.families().to_vec()).collect();
        for f in [AiTargetKind::Subject, AiTargetKind::Background, AiTargetKind::Sky, AiTargetKind::People, AiTargetKind::Object] {
            assert!(fams.contains(&f), "{f:?}");
        }
        assert!(!fams.contains(&AiTargetKind::Landscape));
        let people = reg.iter().find(|m| m.people.is_some()).unwrap();
        assert_eq!(people.parts.len(), 9);
        assert!(people.required.iter().all(|p| p.starts_with("/nonexistent-models")));
    }
}
