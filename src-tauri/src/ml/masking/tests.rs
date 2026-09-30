//! Seam tests with synthetic models, an in-memory matte store and a synthetic source loader;
//! `#[ignore]` real-model tests over `test-data/segment-src` (run with
//! `cargo test --release real_ -- --ignored --nocapture`).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::*;
use crate::develop::masks::MaskCacheConfig;
use crate::ipc::error::ErrorKind;

const FULL: NormRect = NormRect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 };

/// Selects a fixed rectangle (in the input frame); background = its inverse.
struct StubModel {
    id: String,
    families: Vec<AiTargetKind>,
    calls: AtomicUsize,
    delay: Duration,
}

impl StubModel {
    fn new(id: &str, families: &[AiTargetKind]) -> Arc<Self> {
        Arc::new(Self { id: id.into(), families: families.to_vec(), calls: AtomicUsize::new(0), delay: Duration::ZERO })
    }
}

impl SegmentModel for StubModel {
    fn id(&self) -> &str {
        &self.id
    }
    fn families(&self) -> &[AiTargetKind] {
        &self.families
    }
    fn run(&self, request: &SegmentRequest, input: &SegmentInput) -> Result<AlphaMask, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        assert_eq!(input.rgb.len(), (input.width * input.height * 3) as usize);
        let bounds = NormRect { x: 0.25, y: 0.25, width: 0.5, height: 0.5 };
        Ok(match request.target {
            AiTarget::Background => {
                let mut data = vec![255u8; 16];
                data[5] = 0;
                data[6] = 0;
                data[9] = 0;
                data[10] = 0;
                AlphaMask { width: 4, height: 4, bounds: FULL, data }
            }
            _ => AlphaMask { width: 2, height: 2, bounds, data: vec![255; 4] },
        })
    }
}

struct StubPeople;

impl PeopleDetector for StubPeople {
    fn people(&self, _: &SegmentInput) -> Result<Vec<PersonSummary>, String> {
        // Sensor frame: A at the top, B at the bottom.
        let p = |y: f32| PersonSummary {
            bbox: NormRect { x: 0.1, y, width: 0.3, height: 0.2 },
            face: Some(NormRect { x: 0.2, y, width: 0.1, height: 0.1 }),
            reference_point: NormPoint { x: 0.25, y: y + 0.05 },
            features_available: true,
        };
        Ok(vec![p(0.1), p(0.7)])
    }
}

#[derive(Default)]
struct MemStore {
    rows: Mutex<Vec<(ImageId, String, String, String, CachedMatte)>>,
    puts: AtomicUsize,
}

impl MatteStore for MemStore {
    fn find(&self, image_id: ImageId, kind: &str, model: &str, input: &str) -> AppResult<Option<CachedMatte>> {
        Ok(lock(&self.rows)
            .iter()
            .rev()
            .find(|r| r.0 == image_id && r.1 == kind && r.2 == model && r.3 == input)
            .map(|r| r.4.clone()))
    }
    fn put(&self, image_id: ImageId, matte: &NewMatte, mask: &AlphaMask) -> AppResult<AiMaskInfo> {
        let n = self.puts.fetch_add(1, Ordering::SeqCst);
        assert_eq!(matte.origin, AiMaskOrigin::Sieve);
        assert!(matte.digest.is_none());
        let m = CachedMatte {
            digest: format!("{n:032X}"),
            width: mask.width,
            height: mask.height,
            bounds: mask.bounds,
            coverage: coverage(mask),
        };
        lock(&self.rows).push((
            image_id,
            matte.kind.clone(),
            matte.model_version.clone(),
            matte.input_digest.clone().expect("input digest"),
            m.clone(),
        ));
        Ok(AiMaskInfo {
            digest: m.digest,
            target: matte.target.clone(),
            reference_point: matte.reference_point,
            origin: matte.origin,
            model_version: matte.model_version.clone(),
            width: m.width,
            height: m.height,
            bounds: m.bounds,
            coverage: m.coverage,
        })
    }
}

struct Fixture {
    seg: Segmenter,
    model: Arc<StubModel>,
    store: Arc<MemStore>,
    loads: Arc<AtomicUsize>,
    dir: tempfile::TempDir,
}

fn fixture(delay: Duration, parts_file_present: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let present = dir.path().join("present.onnx");
    std::fs::write(&present, b"x").unwrap();
    let missing = dir.path().join("sky.onnx");
    let model = Arc::new(StubModel {
        id: "stub@1".into(),
        families: vec![AiTargetKind::Subject, AiTargetKind::Background, AiTargetKind::People],
        calls: AtomicUsize::new(0),
        delay,
    });
    let sky = StubModel::new("sky@1", &[AiTargetKind::Sky]);
    let store = Arc::new(MemStore::default());
    let loads = Arc::new(AtomicUsize::new(0));
    let l = loads.clone();
    let parts_file = if parts_file_present { present.clone() } else { missing.clone() };
    let parts = SegmenterParts {
        models: vec![
            RegisteredModel {
                model: model.clone(),
                required: vec![present.clone()],
                parts: vec![(PersonPart::Hair, vec![present.clone()]), (PersonPart::Lips, vec![parts_file])],
                people: Some(Arc::new(StubPeople)),
            },
            RegisteredModel { model: sky, required: vec![missing], parts: Vec::new(), people: None },
        ],
        store: store.clone(),
        loader: Arc::new(move |_: &SourceImage, _: Option<&Path>| {
            l.fetch_add(1, Ordering::SeqCst);
            Ok(SourcePixels { width: 8, height: 6, rgb: vec![128; 8 * 6 * 3] })
        }),
    };
    let config = SegmenterConfig { models_dir: dir.path().into(), catalog_path: dir.path().join("c.sqlite") };
    let mattes = MaskCache::new(MaskCacheConfig { catalog_path: config.catalog_path.clone(), cache_dir: dir.path().into() });
    Fixture { seg: Segmenter::with_parts(config, mattes, parts), model, store, loads, dir }
}

fn source(f: &Fixture, id: ImageId) -> SourceImage {
    let path = f.dir.path().join(format!("img{id}.arw"));
    if !path.exists() {
        std::fs::write(&path, b"raw bytes").unwrap();
    }
    SourceImage { id, path, orientation: Some(6) }
}

fn request(target: AiTarget) -> AiMaskRequest {
    AiMaskRequest { target, reference_point: None, force: false }
}

#[test]
fn capabilities_follow_model_files() {
    let f = fixture(Duration::ZERO, false);
    let caps = f.seg.capabilities();
    assert_eq!(caps.ai.len(), AiTargetKind::ALL.len());
    let get = |k: AiTargetKind| caps.ai.iter().find(|c| c.kind == k).unwrap().clone();
    assert!(get(AiTargetKind::Subject).available);
    assert_eq!(get(AiTargetKind::Subject).model.as_deref(), Some("stub@1"));
    let sky = get(AiTargetKind::Sky);
    assert!(!sky.available && sky.model.is_none());
    assert!(sky.reason.unwrap().contains("sky.onnx"), "reason names the missing file");
    let land = get(AiTargetKind::Landscape);
    assert!(!land.available && land.reason.unwrap().contains("landscape"));
    assert!(!get(AiTargetKind::Object).available);
    // Hair's file exists, lips' does not.
    assert_eq!(caps.person_parts, vec![PersonPart::Hair]);
    assert!(caps.landscape.is_empty());
    assert_eq!(f.seg.model_version(AiTargetKind::Background).as_deref(), Some("stub@1"));
    assert_eq!(f.seg.model_version(AiTargetKind::Sky), None);
}

#[test]
fn compute_caches_by_kind_model_and_fingerprint() {
    let f = fixture(Duration::ZERO, true);
    let src = source(&f, 1);
    let a = f.seg.compute(&src, None, &request(AiTarget::Subject)).unwrap();
    assert_eq!(a.origin, AiMaskOrigin::Sieve);
    assert_eq!(a.model_version, "stub@1");
    assert_eq!((a.width, a.height), (2, 2));
    assert!((a.coverage - 0.25).abs() < 1e-6, "{}", a.coverage);
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 1);
    // Cached: no model run, no decode, same digest.
    let b = f.seg.compute(&src, None, &request(AiTarget::Subject)).unwrap();
    assert_eq!(b, a);
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.loads.load(Ordering::SeqCst), 1);
    // Another kind on the same image reuses the decoded source.
    let bg = f.seg.compute(&src, None, &request(AiTarget::Background)).unwrap();
    assert_ne!(bg.digest, a.digest);
    assert!((bg.coverage - 0.75).abs() < 1e-6);
    assert_eq!(f.loads.load(Ordering::SeqCst), 1);
    // force recomputes.
    let forced = f.seg.compute(&src, None, &AiMaskRequest { force: true, ..request(AiTarget::Subject) }).unwrap();
    assert_ne!(forced.digest, a.digest);
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 3);
    // Replaced original (size changes the fingerprint): recomputed and re-decoded.
    std::fs::write(&src.path, b"a different raw file").unwrap();
    let c = f.seg.compute(&src, None, &request(AiTarget::Subject)).unwrap();
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 4);
    assert_eq!(f.loads.load(Ordering::SeqCst), 2);
    assert_ne!(c.digest, forced.digest);
    // Reference points are part of the kind.
    let p = NormPoint { x: 0.3, y: 0.4 };
    let people = AiMaskRequest { reference_point: Some(p), ..request(AiTarget::People { parts: vec![] }) };
    let d = f.seg.compute(&src, None, &people).unwrap();
    assert_eq!(d.reference_point, Some(p));
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 5);
    let rows = lock(&f.store.rows);
    assert!(rows.iter().any(|r| r.1 == "people:person@0.3000,0.4000"));
}

#[test]
fn unavailable_requests_are_invalid() {
    let f = fixture(Duration::ZERO, false);
    let src = source(&f, 1);
    for target in [
        AiTarget::Sky,
        AiTarget::Landscape { category: crate::ipc::types::LandscapeCategory::Water },
        AiTarget::Object { region: FULL },
        AiTarget::Other { sub_type: 7, sub_category: None },
        AiTarget::People { parts: vec![PersonPart::Hair, PersonPart::Lips] },
    ] {
        let e = f.seg.compute(&src, None, &request(target.clone())).unwrap_err();
        assert_eq!(e.kind, ErrorKind::InvalidArgument, "{target:?}: {e}");
    }
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 0);
    // A missing original (and no preview) is not_found.
    let gone = SourceImage { id: 9, path: f.dir.path().join("gone.arw"), orientation: None };
    assert_eq!(f.seg.compute(&gone, None, &request(AiTarget::Subject)).unwrap_err().kind, ErrorKind::NotFound);
}

#[test]
fn concurrent_requests_are_coalesced() {
    let f = fixture(Duration::from_millis(300), true);
    let src = source(&f, 3);
    let kind = "subject";
    assert!(!f.seg.is_computing(3, kind));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let seg = f.seg.clone();
            let src = src.clone();
            std::thread::spawn(move || seg.compute(&src, None, &request(AiTarget::Subject)).unwrap())
        })
        .collect();
    std::thread::sleep(Duration::from_millis(100));
    assert!(f.seg.is_computing(3, kind));
    assert!(!f.seg.is_computing(3, "sky"));
    assert!(!f.seg.is_computing(4, kind));
    let results: Vec<AiMaskInfo> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert!(results.windows(2).all(|w| w[0] == w[1]));
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.store.puts.load(Ordering::SeqCst), 1);
    assert!(!f.seg.is_computing(3, kind));
    // Different images do not wait for each other's flight.
    let other = source(&f, 4);
    f.seg.compute(&other, None, &request(AiTarget::Subject)).unwrap();
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn detect_people_is_left_to_right_in_the_displayed_frame() {
    let f = fixture(Duration::ZERO, true);
    // Orientation 6 (rotate 90 CW for display): sensor top -> displayed right.
    let src = source(&f, 1);
    let people = f.seg.detect_people(&src, None).unwrap();
    assert_eq!(people.len(), 2);
    // B (sensor y = 0.7) is on the displayed left.
    assert!((people[0].reference_point.y - 0.75).abs() < 1e-6, "sensor-frame reference point");
    let b = people[0].bbox;
    // Displayed bbox = orient_rect(sensor): x = 1 - (y + h) = 0.1, y = 0.1.
    assert!((b.x - 0.1).abs() < 1e-5 && (b.y - 0.1).abs() < 1e-5, "{b:?}");
    assert!((b.width - 0.2).abs() < 1e-5 && (b.height - 0.3).abs() < 1e-5, "{b:?}");
    assert!(people[0].face.is_some());
    assert!(people[0].bbox.x < people[1].bbox.x);
}

#[test]
fn orientation_helpers_round_trip() {
    let m = AlphaMask {
        width: 3,
        height: 2,
        bounds: NormRect { x: 0.1, y: 0.2, width: 0.3, height: 0.4 },
        data: vec![1, 2, 3, 4, 5, 6],
    };
    for o in 1..=8u8 {
        let back = unorient_matte(orient_matte(&m, o), o);
        assert_eq!(back.data, m.data, "orientation {o}");
        assert_eq!((back.width, back.height), (3, 2));
        let (b, r) = (back.bounds, m.bounds);
        assert!((b.x - r.x).abs() < 1e-6 && (b.y - r.y).abs() < 1e-6 && (b.width - r.width).abs() < 1e-6);
        assert!((unorient_rect(orient_rect(r, o), o).height - r.height).abs() < 1e-6);
    }
    let full = AlphaMask { width: 2, height: 1, bounds: FULL, data: vec![255, 0] };
    assert!((coverage(&full) - 0.5).abs() < 1e-6);
}

// ---------------------------------------------------------------------------------------
// Real models over test-data/segment-src (16 sample previews, sensor frame)
// ---------------------------------------------------------------------------------------

mod real {
    use super::*;
    use crate::ml::refine::{refine, Guide, RefineParams};
    use crate::ml::segment_models::registry;
    use crate::raw::turbo;

    const ROOT: &str = "/Users/gurjotsingh/Documents/GitHub/Sieve/test-data";
    /// Frames without visible sky (indoor, night, close-ups against walls/foliage).
    const NO_SKY: [&str; 6] = ["MON04829", "MON04849", "MON05151", "MON05322", "AZA06793", "IMG_5697"];

    fn models_dir() -> PathBuf {
        std::env::var("SIEVE_MODELS")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models"))
    }

    fn samples() -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(format!("{ROOT}/segment-src"))
            .expect("test-data/segment-src")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("jpg")))
            .collect();
        v.sort();
        v
    }

    fn decode(path: &Path) -> SourcePixels {
        let d = turbo::decode_rgb(&std::fs::read(path).unwrap(), u32::MAX, 64_000_000).unwrap();
        SourcePixels { width: d.width, height: d.height, rgb: d.pixels }
    }

    /// Real registry, in-memory store, previews loaded as sensor-frame sources.
    fn real_segmenter() -> (Segmenter, Arc<MemStore>) {
        let dir = models_dir();
        let store = Arc::new(MemStore::default());
        let parts = SegmenterParts {
            models: registry(&dir, Some(PathBuf::from(format!("{ROOT}/coreml-cache")))),
            store: store.clone(),
            loader: Arc::new(|s: &SourceImage, _: Option<&Path>| Ok(decode(&s.path))),
        };
        let config = SegmenterConfig { models_dir: dir, catalog_path: PathBuf::from("/nonexistent/c.sqlite") };
        let mattes = MaskCache::new(MaskCacheConfig { catalog_path: config.catalog_path.clone(), cache_dir: "/tmp".into() });
        (Segmenter::with_parts(config, mattes, parts), store)
    }

    fn stored(store: &MemStore, digest: &str) -> CachedMatte {
        lock(&store.rows).iter().find(|r| r.4.digest == digest).unwrap().4.clone()
    }

    /// The low-res matte as the store would hold it: re-run the model to get the pixels
    /// (the in-memory store keeps only metadata), then refine against the preview.
    fn refined(seg: &Segmenter, target: AiTarget, rp: Option<NormPoint>, img: &SourcePixels) -> AlphaMask {
        let family = target.family().unwrap();
        let m = seg.model_for(family).unwrap().model.clone();
        let input = SegmentInput { width: img.width, height: img.height, rgb: &img.rgb, orientation: 1 };
        let kind = AiMask { target: target.clone(), reference_point: rp, digest: None }.cache_kind();
        let low = m.run(&SegmentRequest { target, reference_point: rp }, &input).unwrap();
        match RefineParams::for_kind(&kind) {
            Some(p) => {
                let guide = Guide { width: img.width, height: img.height, rgb: &img.rgb, region: FULL };
                refine(&low, &guide, p)
            }
            None => low,
        }
    }

    fn overlay(img: &SourcePixels, layers: &[(&AlphaMask, [f32; 3])]) -> Vec<u8> {
        let (w, h) = (img.width as usize, img.height as usize);
        let mut out: Vec<f32> = img.rgb.iter().map(|&v| v as f32 * 0.45).collect();
        for (m, col) in layers {
            for y in 0..h {
                for x in 0..w {
                    let a = crate::ml::refine::sample_alpha(m, (x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32) * 0.8;
                    if a <= 0.0 {
                        continue;
                    }
                    let p = (y * w + x) * 3;
                    for c in 0..3 {
                        let base = img.rgb[p + c] as f32 * 0.35 + col[c] * 0.65;
                        out[p + c] = out[p + c] * (1.0 - a) + base * a;
                    }
                }
            }
        }
        out.iter().map(|&v| v.clamp(0.0, 255.0) as u8).collect()
    }

    fn save(name: &str, suffix: &str, img: &SourcePixels, px: &[u8]) {
        let dir = PathBuf::from(format!("{ROOT}/segment-check/v2"));
        std::fs::create_dir_all(&dir).unwrap();
        let jpg = turbo::encode_rgb(px, img.width, img.height, 85).unwrap();
        std::fs::write(dir.join(format!("{name}_{suffix}.jpg")), jpg).unwrap();
    }

    fn median(v: &mut [f64]) -> f64 {
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(f64::NAN)
    }

    const PALETTE: [[f32; 3]; 6] =
        [[255.0, 80.0, 80.0], [80.0, 160.0, 255.0], [255.0, 220.0, 60.0], [80.0, 230.0, 120.0], [230.0, 120.0, 255.0], [60.0, 230.0, 230.0]];

    /// Subject non-empty everywhere, sky ~0 without sky, sane person counts, parts; timings
    /// (cold = first image incl. model load, warm = median over the rest, cached = hit);
    /// overlays in test-data/segment-check/v2.
    #[test]
    #[ignore = "needs models (scripts/fetch-models.sh) and test-data/segment-src"]
    fn real_masks_over_samples() {
        let (seg, store) = real_segmenter();
        let caps = seg.capabilities();
        assert!(caps.ai.iter().filter(|c| c.kind != AiTargetKind::Landscape).all(|c| c.available), "{caps:?}");
        assert_eq!(caps.person_parts.len(), 9);
        let files = samples();
        assert_eq!(files.len(), 16);
        let mut times: HashMap<&str, Vec<f64>> = HashMap::new();
        let mut cold: Vec<(&str, f64)> = Vec::new();
        let mut rec = |i: usize, k: &'static str, t: Instant| {
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            if i == 0 {
                cold.push((k, ms));
            } else {
                times.entry(k).or_default().push(ms);
            }
            ms
        };
        for (i, path) in files.iter().enumerate() {
            let name = path.file_stem().unwrap().to_string_lossy().to_string();
            let src = SourceImage { id: i as ImageId + 1, path: path.clone(), orientation: None };
            let img = decode(path);

            let t = Instant::now();
            let subject = seg.compute(&src, None, &request(AiTarget::Subject)).unwrap();
            let t_subject = rec(i, "subject", t);
            let t = Instant::now();
            let again = seg.compute(&src, None, &request(AiTarget::Subject)).unwrap();
            rec(i, "subject (cached)", t);
            assert_eq!(again.digest, subject.digest);
            let t = Instant::now();
            let bg = seg.compute(&src, None, &request(AiTarget::Background)).unwrap();
            rec(i, "background (after subject)", t);
            assert!((bg.coverage + subject.coverage - 1.0).abs() < 0.01);
            let t = Instant::now();
            let sky = seg.compute(&src, None, &request(AiTarget::Sky)).unwrap();
            rec(i, "sky", t);
            let t = Instant::now();
            let people = seg.detect_people(&src, None).unwrap();
            rec(i, "detect_people", t);
            let t = Instant::now();
            let everyone = seg.compute(&src, None, &request(AiTarget::People { parts: vec![] })).unwrap();
            rec(i, "people: all (after detect)", t);
            assert!(subject.coverage > 0.01, "{name}: empty subject ({})", subject.coverage);
            assert_eq!(stored(&store, &subject.digest).width, 1024);
            if NO_SKY.contains(&name.as_str()) {
                assert!(sky.coverage < 0.01, "{name}: sky coverage {}", sky.coverage);
            }
            let min_people = match name.as_str() {
                "MON05151" => 5,
                "MON05322" => 4,
                "IMG_5697" | "MON04849" | "DSCF5929" => 2,
                _ => 1,
            };
            assert!(people.len() >= min_people && people.len() <= 12, "{name}: {} people", people.len());
            assert!(everyone.coverage > 0.005, "{name}: people coverage {}", everyone.coverage);

            // One person: whole, then parts (regions, then features) - computed lazily.
            let first = people.iter().max_by(|a, b| (a.bbox.width * a.bbox.height).total_cmp(&(b.bbox.width * b.bbox.height))).unwrap();
            let rp = Some(first.reference_point);
            let t = Instant::now();
            let person =
                seg.compute(&src, None, &AiMaskRequest { reference_point: rp, ..request(AiTarget::People { parts: vec![] }) }).unwrap();
            rec(i, "person (after detect)", t);
            assert!(person.coverage > 0.002 && person.coverage <= everyone.coverage + 1e-3, "{name}: person {}", person.coverage);
            let skin = AiTarget::People { parts: vec![PersonPart::FaceSkin, PersonPart::BodySkin, PersonPart::Hair] };
            let t = Instant::now();
            let skin_info = seg.compute(&src, None, &AiMaskRequest { reference_point: rp, ..request(skin.clone()) }).unwrap();
            rec(i, "parts: face+body skin+hair", t);
            assert!(skin_info.coverage > 0.0 && skin_info.coverage < person.coverage + 1e-3, "{name}");
            let feats = AiTarget::People { parts: vec![PersonPart::EyeSclera, PersonPart::IrisPupil, PersonPart::Lips] };
            let t = Instant::now();
            let feat_info = seg.compute(&src, None, &AiMaskRequest { reference_point: rp, ..request(feats.clone()) }).unwrap();
            rec(i, "parts: eyes+lips", t);
            let t = Instant::now();
            let clothes = AiTarget::People { parts: vec![PersonPart::Clothes] };
            let cl_info = seg.compute(&src, None, &AiMaskRequest { reference_point: rp, ..request(clothes.clone()) }).unwrap();
            rec(i, "parts: clothes (planes cached)", t);
            println!(
                "{name} {}x{}: subject {:.1}% ({t_subject:.0} ms) sky {:.2}% people {} (all {:.1}%, picked {:.1}%, skin+hair {:.2}%, eyes+lips {:.3}%, clothes {:.1}%)",
                img.width,
                img.height,
                subject.coverage * 100.0,
                sky.coverage * 100.0,
                people.len(),
                everyone.coverage * 100.0,
                person.coverage * 100.0,
                skin_info.coverage * 100.0,
                feat_info.coverage * 100.0,
                cl_info.coverage * 100.0,
            );

            // Overlays (refined against the preview, as the evaluator would).
            let t = Instant::now();
            let sub = refined(&seg, AiTarget::Subject, None, &img);
            rec(i, "refine subject @2048", t);
            let skym = refined(&seg, AiTarget::Sky, None, &img);
            save(&name, "subject", &img, &overlay(&img, &[(&sub, [255.0, 0.0, 255.0])]));
            save(&name, "sky", &img, &overlay(&img, &[(&skym, [0.0, 200.0, 255.0])]));
            let persons: Vec<AlphaMask> = people
                .iter()
                .map(|p| refined(&seg, AiTarget::People { parts: vec![] }, Some(p.reference_point), &img))
                .collect();
            let layers: Vec<(&AlphaMask, [f32; 3])> = persons.iter().enumerate().map(|(k, m)| (m, PALETTE[k % 6])).collect();
            save(&name, "people", &img, &overlay(&img, &layers));
            let part = |parts: Vec<PersonPart>| refined(&seg, AiTarget::People { parts }, rp, &img);
            let (cl, hair, face, body) = (
                part(vec![PersonPart::Clothes]),
                part(vec![PersonPart::Hair]),
                part(vec![PersonPart::FaceSkin]),
                part(vec![PersonPart::BodySkin]),
            );
            let (sclera, iris, lips, brows, teeth) = (
                part(vec![PersonPart::EyeSclera]),
                part(vec![PersonPart::IrisPupil]),
                part(vec![PersonPart::Lips]),
                part(vec![PersonPart::Eyebrows]),
                part(vec![PersonPart::Teeth]),
            );
            let layers = [
                (&cl, [60.0, 60.0, 255.0]),
                (&body, [255.0, 130.0, 110.0]),
                (&face, [0.0, 220.0, 255.0]),
                (&hair, [255.0, 215.0, 0.0]),
                (&brows, [255.0, 0.0, 255.0]),
                (&sclera, [255.0, 255.0, 255.0]),
                (&iris, [0.0, 255.0, 0.0]),
                (&lips, [255.0, 0.0, 0.0]),
                (&teeth, [255.0, 140.0, 0.0]),
            ];
            save(&name, "parts", &img, &overlay(&img, &layers));
        }
        println!("\ncold (first image, includes model load / CoreML compile):");
        for (k, v) in &cold {
            println!("  {k:<30} {v:>8.1} ms");
        }
        println!("\nwarm (median over the other {} images):", files.len() - 1);
        let mut keys: Vec<_> = times.keys().copied().collect();
        keys.sort();
        for k in keys {
            println!("  {k:<30} {:>8.1} ms", median(times.get_mut(k).unwrap()));
        }
    }

    /// The default loader on a real RAW (develop source, sensor frame) + subject.
    #[test]
    #[ignore = "needs models and a RAW in test-data"]
    fn real_develop_source_subject() {
        let raw = walk(Path::new(ROOT)).into_iter().find(|p| {
            p.extension().is_some_and(|e| ["arw", "raf", "cr3"].contains(&e.to_ascii_lowercase().to_str().unwrap_or("")))
        });
        let Some(raw) = raw else {
            eprintln!("no RAW under {ROOT}; skipped");
            return;
        };
        let src = SourceImage { id: 1, path: raw.clone(), orientation: None };
        let t = Instant::now();
        let px = load_source(&src, None).unwrap();
        println!("{}: develop source {}x{} in {} ms", raw.display(), px.width, px.height, t.elapsed().as_millis());
        assert_eq!(px.width.max(px.height), SOURCE_EDGE);
        let (seg, _) = real_segmenter();
        let seg = Segmenter::with_parts(
            seg.config().clone(),
            seg.mattes().clone(),
            SegmenterParts {
                models: registry(&models_dir(), None),
                store: Arc::new(MemStore::default()),
                loader: Arc::new(load_source),
            },
        );
        let info = seg.compute(&src, None, &request(AiTarget::Subject)).unwrap();
        println!("subject coverage {:.1}%", info.coverage * 100.0);
        assert!(info.coverage > 0.01);
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    if !p.ends_with("segment-check") && !p.ends_with("coreml-cache") {
                        out.extend(walk(&p));
                    }
                } else {
                    out.push(p);
                }
            }
        }
        out
    }
}
