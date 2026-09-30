//! Phase 8: Select Sky runs on the upright image; its matte comes back in the sensor frame.

use super::*;
use crate::develop::masks::MaskCacheConfig;

struct UprightSky(Mutex<Vec<(u32, u32, u8)>>);

impl SegmentModel for UprightSky {
    fn id(&self) -> &str {
        "sky@test"
    }
    fn families(&self) -> &[AiTargetKind] {
        &[AiTargetKind::Sky]
    }
    fn run(&self, _: &SegmentRequest, input: &SegmentInput) -> Result<AlphaMask, String> {
        lock(&self.0).push((input.width, input.height, input.orientation));
        // Sensor pixel (0, 0) is red; orientation 6 puts it at the displayed top-right.
        let red = input.rgb.chunks(3).position(|p| p[0] == 255).unwrap();
        assert_eq!((red % input.width as usize, red / input.width as usize), (input.width as usize - 1, 0));
        // Top half of the (upright) input.
        let bounds = NormRect { x: 0.0, y: 0.0, width: 1.0, height: 0.5 };
        Ok(AlphaMask { width: 1, height: 1, bounds, data: vec![255] })
    }
}

#[derive(Default)]
struct Store(Mutex<Vec<NormRect>>);

impl MatteStore for Store {
    fn find(&self, _: ImageId, _: &str, _: &str, _: &str) -> AppResult<Option<CachedMatte>> {
        Ok(None)
    }
    fn put(&self, _: ImageId, matte: &NewMatte, mask: &AlphaMask) -> AppResult<AiMaskInfo> {
        lock(&self.0).push(mask.bounds);
        Ok(AiMaskInfo {
            digest: "0".repeat(32),
            target: matte.target.clone(),
            reference_point: matte.reference_point,
            origin: matte.origin,
            model_version: matte.model_version.clone(),
            width: mask.width,
            height: mask.height,
            bounds: mask.bounds,
            coverage: coverage(mask),
        })
    }
}

#[test]
fn sky_model_sees_the_upright_image() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sky.onnx");
    std::fs::write(&file, b"x").unwrap();
    let model = Arc::new(UprightSky(Mutex::new(Vec::new())));
    let store = Arc::new(Store::default());
    let parts = SegmenterParts {
        models: vec![RegisteredModel { model: model.clone(), required: vec![file], parts: Vec::new(), people: None }],
        store: store.clone(),
        loader: Arc::new(|_: &SourceImage, _: Option<&Path>| {
            let mut rgb = vec![128u8; 8 * 6 * 3];
            rgb[0] = 255;
            Ok(SourcePixels { width: 8, height: 6, rgb })
        }),
    };
    let config = SegmenterConfig { models_dir: dir.path().into(), catalog_path: dir.path().join("c.sqlite") };
    let mattes =
        MaskCache::new(MaskCacheConfig { catalog_path: config.catalog_path.clone(), cache_dir: dir.path().into() });
    let seg = Segmenter::with_parts(config, mattes, parts);
    let path = dir.path().join("img.arw");
    std::fs::write(&path, b"raw").unwrap();
    let src = SourceImage { id: 1, path, orientation: Some(6) };
    let req = AiMaskRequest { target: AiTarget::Sky, reference_point: None, force: false };
    let info = seg.compute(&src, None, &req).unwrap();
    assert_eq!(*lock(&model.0), vec![(6, 8, 6)], "upright 6x8 input, orientation 6");
    // Displayed top half = the sensor's left half for orientation 6.
    let want = unorient_rect(NormRect { x: 0.0, y: 0.0, width: 1.0, height: 0.5 }, 6);
    assert!((want.width - 0.5).abs() < 1e-6 && (want.height - 1.0).abs() < 1e-6, "{want:?}");
    let got = info.bounds;
    assert!((got.x - want.x).abs() < 1e-6 && (got.width - want.width).abs() < 1e-6, "{got:?} vs {want:?}");
    assert_eq!(lock(&store.0).len(), 1);
    // Other families keep the sensor frame (orientation 1 input).
    let seg_input = SourcePixels { width: 8, height: 6, rgb: vec![0; 144] };
    let (_, w, h) = orient_pixels(&seg_input.rgb, 8, 6, 3, 1);
    assert_eq!((w, h), (8, 6));
}
