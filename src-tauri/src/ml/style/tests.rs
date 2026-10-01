use std::path::Path;
use std::sync::atomic::AtomicBool;

use super::*;
use crate::ipc::error::ErrorKind;
use crate::ml::style_model::tests::ctx;
use crate::scene::MatchImage;

/// Writes a 96x64 RGB PNG: a horizontal ramp around `level` with a slight warm cast.
fn write_png(path: &Path, level: f32) {
    let (w, h) = (96u32, 64u32);
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    for _y in 0..h {
        for x in 0..w {
            let v = level * (0.5 + x as f32 / w as f32);
            data.extend([
                (v * 1.05).clamp(0.0, 255.0) as u8,
                v.clamp(0.0, 255.0) as u8,
                (v * 0.9).clamp(0.0, 255.0) as u8,
            ]);
        }
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&data).unwrap();
}

/// Brightness of image `i` (1-based) of the synthetic shoot.
fn level(i: i64) -> f32 {
    30.0 + ((i * 37) % 160) as f32
}

/// The synthetic user: a preset (vibrance, green saturation) + exposure that brings every
/// frame to the same brightness.
fn user_edit(i: i64) -> ParametricAdjustments {
    let mut a = ParametricAdjustments::defaults_for(ImageFormat::Png);
    a.vibrance = 15.0;
    a.hsl.saturation.green = -30.0;
    a.exposure = ((110.0 / level(i)).log2() * 20.0).round() / 20.0;
    a
}

/// Catalog with `n` PNGs (ids 1..=n, one minute apart); `edited` of them carry the user's
/// edit.
fn catalog(dir: &Path, n: i64, edited: i64) -> StyleModel {
    let path = dir.join("cat.sqlite");
    let conn = db::open(&path).unwrap();
    conn.execute("INSERT INTO folders (id, path, added_at) VALUES (1, ?1, 0)", [dir.to_string_lossy()]).unwrap();
    for i in 1..=n {
        let file = dir.join(format!("f{i:03}.png"));
        write_png(&file, level(i));
        conn.execute(
            "INSERT INTO images (id, folder_id, path, file_name, format, camera_make, camera_model, file_size,
                                 file_mtime_ms, imported_at, captured_at_ms)
             VALUES (?1, 1, ?2, ?3, 'png', 'other', 'Body', 1, 0, 0, ?4)",
            params![i, file.to_string_lossy(), format!("f{i:03}.png"), i * 60_000],
        )
        .unwrap();
        if i <= edited {
            repo::save_adjustments(&conn, i, &user_edit(i)).unwrap();
        }
    }
    StyleModel::new(StyleModelConfig { catalog_path: path })
}

fn cache() -> DevelopCache {
    DevelopCache::new(DevelopConfig { cache_bytes: 64 << 20, mask_cache: None })
}

fn luts() -> LutLibrary {
    LutLibrary::new(PathBuf::from("/nonexistent"))
}

#[derive(Default)]
struct Control {
    cancel: AtomicBool,
    phases: Mutex<Vec<(StyleTrainPhase, u32, u32)>>,
}

impl TrainControl for Control {
    fn progress(&self, phase: StyleTrainPhase, done: u32, total: u32) {
        lock(&self.phases).push((phase, done, total));
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

fn input(conn: &Connection, id: ImageId, adjustments: ParametricAdjustments) -> MatchImage {
    let e = repo::get_image(conn, id).unwrap();
    MatchImage {
        src: SourceImage { id, path: e.path.into(), orientation: e.orientation },
        captured_at_ms: e.capture.captured_at_ms,
        adjustments,
    }
}

#[test]
fn untrained_status_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let style = catalog(dir.path(), 4, 2);
    let conn = db::open(&style.config().catalog_path).unwrap();
    // A crop-only change is not a style sample.
    let mut crop_only = ParametricAdjustments::defaults_for(ImageFormat::Png);
    crop_only.crop.enabled = true;
    crop_only.crop.left = 0.2;
    repo::save_adjustments(&conn, 3, &crop_only).unwrap();
    let s = style.status(&conn).unwrap();
    assert_eq!(s.state, StyleModelState::Untrained);
    assert_eq!((s.available_examples, s.training_examples, s.min_examples), (2, 0, 20));
    assert_eq!((s.trained_at_ms, s.progress, s.validation), (None, None, None));
    assert_eq!(s.model_version, MODEL_VERSION);

    let e = style.predict(&cache(), &luts(), &[input(&conn, 4, crop_only.clone())]).unwrap_err();
    assert_eq!(e.kind, ErrorKind::InvalidArgument);
    let e = train_catalog(&style.config().catalog_path, &cache(), &luts(), &Control::default()).unwrap_err();
    assert_eq!(e.kind, ErrorKind::InvalidArgument);
    assert!(e.message.contains("at least 20"), "{}", e.message);
}

#[test]
fn trains_validates_stores_and_predicts_from_the_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let style = catalog(dir.path(), 32, 30);
    let (cache, luts) = (cache(), luts());
    let control = Control::default();
    let s = train_catalog(&style.config().catalog_path, &cache, &luts, &control).unwrap().expect("not cancelled");
    assert_eq!((s.samples, s.skipped, s.features_computed), (30, 0, 30));
    let v = s.validation.expect("validation");
    assert_eq!(v.held_out_images, 6);
    assert!(v.delta_e < v.no_edit_delta_e, "{v:?}");
    let phases = lock(&control.phases).clone();
    for p in [StyleTrainPhase::Features, StyleTrainPhase::Fit, StyleTrainPhase::Validate] {
        assert!(phases.iter().any(|(q, d, t)| *q == p && d == t), "{p:?} completes");
    }

    let conn = db::open(&style.config().catalog_path).unwrap();
    let st = style.status(&conn).unwrap();
    assert_eq!((st.state, st.training_examples, st.available_examples), (StyleModelState::Ready, 30, 30));
    assert_eq!(st.validation, Some(v));
    assert!(st.trained_at_ms.is_some());

    // Predict for an unedited frame: its crop and masks stay, the style is applied.
    let mut current = ParametricAdjustments::defaults_for(ImageFormat::Png);
    current.crop.enabled = true;
    current.crop.left = 0.25;
    let p = style.predict(&cache, &luts, &[input(&conn, 31, current.clone())]).unwrap();
    assert_eq!(p.len(), 1);
    let a = &p[0].adjustments;
    assert_eq!(a.crop, current.crop);
    assert_eq!(a.masks, current.masks);
    assert_eq!((a.vibrance, a.hsl.saturation.green), (15.0, -30.0));
    let want = user_edit(31).exposure;
    assert!((a.exposure - want).abs() < 0.35, "exposure {} vs {want}", a.exposure);
    assert_eq!(p[0].fields, PREDICTED_FIELDS.to_vec());
    assert!((0.0..=1.0).contains(&p[0].confidence));
    assert!(p[0].notes.iter().any(|n| n.contains("Scenes not detected")), "{:?}", p[0].notes);

    // Retrain: features come from the cache; older models are pruned.
    for _ in 0..2 {
        let s2 = train_catalog(&style.config().catalog_path, &cache, &luts, &Control::default()).unwrap().unwrap();
        assert_eq!(s2.features_computed, 0);
    }
    let rows: i64 = conn.query_row("SELECT COUNT(*) FROM style_models", [], |r| r.get(0)).unwrap();
    assert_eq!(rows, KEEP_MODELS);
    // The loaded model follows the newest row.
    let newest: i64 = conn.query_row("SELECT MAX(id) FROM style_models", [], |r| r.get(0)).unwrap();
    style.active_model(&conn).unwrap().unwrap();
    assert_eq!(lock(&style.loaded).as_ref().map(|l| l.0), Some(newest));
}

#[test]
fn cancelled_training_stores_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let style = catalog(dir.path(), 22, 22);
    let control = Control::default();
    control.cancel.store(true, Ordering::SeqCst);
    assert!(train_catalog(&style.config().catalog_path, &cache(), &luts(), &control).unwrap().is_none());
    let conn = db::open(&style.config().catalog_path).unwrap();
    let rows: i64 = conn.query_row("SELECT COUNT(*) FROM style_models", [], |r| r.get(0)).unwrap();
    assert_eq!(rows, 0);
    assert_eq!(style.status(&conn).unwrap().state, StyleModelState::Untrained);
}

#[test]
fn stored_features_go_stale_when_the_preview_is_re_extracted() {
    let dir = tempfile::tempdir().unwrap();
    let style = catalog(dir.path(), 1, 0);
    let mut conn = db::open(&style.config().catalog_path).unwrap();
    let f = StoredFeatures {
        render: ctx(1, "Sony").render,
        as_shot: None,
        auto: Some(AutoAnchor { exposure: 0.5, ..Default::default() }),
        faces_known: false,
        faces_on_demand: true,
        edited: None,
        edited_key: None,
    };
    save_features(&mut conn, &[(1, f.clone()), (99, f.clone())]).unwrap(); // unknown id skipped
    assert_eq!(load_features(&conn, 1).unwrap(), Some(f));
    assert_eq!(load_features(&conn, 99).unwrap(), None);
    conn.execute(
        "INSERT INTO thumbnails (image_id, status, extracted_at) VALUES (1, 'ready', ?1)",
        [now_ms() + 60_000],
    )
    .unwrap();
    assert_eq!(load_features(&conn, 1).unwrap(), None);
}

#[test]
fn holdout_is_the_latest_share_of_each_camera() {
    let mk = |i: usize, make: &str| StyleSample {
        context: ctx(i, make),
        adjustments: ParametricAdjustments::default(),
        group: None,
        captured_at_ms: Some(i as i64),
        edited: None,
    };
    let mut s: Vec<StyleSample> = (0..40).map(|i| mk(i, "Sony")).collect();
    s.extend((40..50).map(|i| mk(i, "Canon")));
    let held = holdout_split(&s).unwrap();
    assert_eq!(held.iter().filter(|h| **h).count(), 10);
    assert!(held[32..40].iter().all(|h| *h) && !held[31]);
    assert!(held[48..50].iter().all(|h| *h) && !held[47]);
    // Too few left for training -> no validation.
    assert!(holdout_split(&s[..22]).is_none());
    // Capped at HOLDOUT_MAX.
    let big: Vec<StyleSample> = (0..1000).map(|i| mk(i, "Sony")).collect();
    assert_eq!(holdout_split(&big).unwrap().iter().filter(|h| **h).count(), HOLDOUT_MAX);
}

#[test]
fn openmp_limit_is_harmless_without_or_with_a_runtime() {
    single_threaded_openmp();
    single_threaded_openmp();
    assert!(decode_pool().current_num_threads() <= MAX_DECODE_THREADS);
}

#[test]
fn training_features_follow_the_frames_own_settings() {
    let dir = tempfile::tempdir().unwrap();
    let style = catalog(dir.path(), 1, 1);
    let conn = db::open(&style.config().catalog_path).unwrap();
    let mut frame = CatalogFrame::load(&conn, 1, None).unwrap();
    let f = neutral_features(&cache(), &luts(), &frame, true).unwrap();
    assert!(f.auto.is_some() && f.edited.is_some());
    assert!(f.current_for(&frame, true) && f.current_for(&frame, false));
    // Prediction features do not carry the edited render: not enough for training.
    let p = neutral_features(&cache(), &luts(), &frame, false).unwrap();
    assert!(p.current_for(&frame, false) && !p.current_for(&frame, true));
    // The user re-edits: the measured render is stale for training only.
    frame.adjustments.exposure += 0.5;
    assert!(!f.current_for(&frame, true) && f.current_for(&frame, false));
    // Computed before on-demand face detection existed (unanalysed frame): recompute.
    let old = StoredFeatures { faces_on_demand: false, ..p.clone() };
    assert!(p.faces_on_demand && !old.current_for(&frame, false));
    // Faces found by the analysis after the features were computed: recompute.
    frame.faces = Some(Vec::new());
    assert!(!f.current_for(&frame, false));
}
