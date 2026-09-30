use super::*;
use crate::ipc::types::{WhiteBalance, WhiteBalanceValues};

fn ctx(i: usize, make: &str) -> FrameContext {
    let luma = -4.0 + (i % 17) as f32 * 0.25;
    FrameContext {
        format: ImageFormat::Arw,
        make: Some(make.into()),
        model: Some("Body".into()),
        iso: Some(100 << (i % 5)),
        shutter_seconds: Some(1.0 / 250.0),
        aperture: Some(2.0),
        focal_length_mm: Some(35.0),
        as_shot: Some(WhiteBalanceValues { temperature_k: 4000.0 + (i % 7) as f32 * 300.0, tint: 3.0 }),
        render: RenderFeatures {
            log_mean_luma: luma,
            log_percentiles: [luma - 4.0, luma - 2.0, luma, luma + 1.0, luma + 2.0],
            clipped_highlights: 0.0,
            clipped_shadows: 0.0,
            mean_oklab: [0.5, 0.01, 0.02],
            neutral_ab: [0.0, (i % 3) as f32 * 0.01],
            neutral_coverage: 0.5,
            luma_hist: vec![1.0 / 16.0; 16],
            mean_chroma: 0.05,
            center_delta: 0.0,
        },
        preview: None,
        scene: None,
    }
}

/// A user whose preset sets HSL + vibrance, who brings every frame to the same brightness
/// and sets a fixed 5600 K.
fn user_edit(c: &FrameContext) -> ParametricAdjustments {
    let mut a = ParametricAdjustments::default();
    a.hsl.saturation.green = -40.0;
    a.vibrance = 20.0;
    a.exposure = ((-2.0 - c.render.log_mean_luma) * 0.8 * 100.0).round() / 100.0;
    a.white_balance = WhiteBalance::Custom { temperature_k: 5600.0, tint: 10.0 };
    a.crop.enabled = true;
    a.crop.left = 0.1;
    a
}

fn c_render(i: usize) -> RenderFeatures {
    ctx(i, "Sony").render
}

fn samples(n: usize) -> Vec<StyleSample> {
    (0..n)
        .map(|i| {
            let c = ctx(i, "Sony");
            StyleSample {
                adjustments: user_edit(&c),
                context: c,
                group: Some((i / 6) as i64),
                captured_at_ms: Some(i as i64 * 1000),
                edited: Some(RenderFeatures { log_mean_luma: -2.0, neutral_ab: [0.0, 0.0], ..c_render(i) }),
            }
        })
        .collect()
}

#[test]
fn learns_template_and_per_image_sliders() {
    let s = samples(120);
    let (model, report) = train(&s, &TrainOptions::default(), &|_| {}).unwrap();
    assert_eq!(report.samples, 120);
    for i in [3usize, 500, 1001] {
        let c = ctx(i, "Sony");
        let want = user_edit(&c);
        let p = model.predict(&c);
        assert!(p.known_camera);
        let a = p.adjustments;
        assert!((a.exposure - want.exposure).abs() < 0.1, "exposure {} vs {}", a.exposure, want.exposure);
        assert_eq!(a.hsl.saturation.green, -40.0);
        assert!((a.vibrance - 20.0).abs() < 1.0);
        let WhiteBalance::Custom { temperature_k, tint } = a.white_balance else { panic!("custom wb") };
        assert!((temperature_k - 5600.0).abs() < 200.0, "{temperature_k}");
        assert!((tint - 10.0).abs() < 1.5, "{tint}");
        assert!(!a.crop.enabled, "crop is never predicted");
        assert!(a.validate().is_ok());
    }
    let (cv, mean) = report.cv["exposure"];
    assert!(cv < mean * 0.5, "cv {cv} vs mean-predictor {mean}");
}

#[test]
fn unknown_camera_uses_the_global_template() {
    let (model, _) = train(&samples(40), &TrainOptions { linear_only: true, ..Default::default() }, &|_| {}).unwrap();
    let p = model.predict(&ctx(1, "Nikon"));
    assert!(!p.known_camera);
    assert_eq!(p.adjustments.hsl.saturation.green, -40.0);
}

#[test]
fn model_round_trips_through_json_and_rejects_other_versions() {
    let (model, _) = train(&samples(30), &TrainOptions::default(), &|_| {}).unwrap();
    let json = model.to_json();
    let back = StyleModel::from_json(&json).unwrap();
    assert_eq!(back.predict(&ctx(7, "Sony")).adjustments, model.predict(&ctx(7, "Sony")).adjustments);
    let old = json.replacen("\"version\":1", "\"version\":0", 1);
    assert_eq!(StyleModel::from_json(&old), Err(StyleError::Outdated));
    assert!(matches!(StyleModel::from_json("{"), Err(StyleError::InvalidModel(_))));
}

#[test]
fn too_few_samples_is_an_error() {
    let r = train(&samples(MIN_SAMPLES - 1), &TrainOptions::default(), &|_| {});
    assert_eq!(r.err(), Some(StyleError::Insufficient { have: MIN_SAMPLES - 1, min: MIN_SAMPLES }));
}

#[test]
fn crop_only_edits_are_not_style_samples() {
    let mut a = ParametricAdjustments::default();
    assert!(!is_style_sample(&a, ImageFormat::Arw));
    a.crop.enabled = true;
    assert!(!is_style_sample(&a, ImageFormat::Arw));
    a.exposure = 0.3;
    assert!(is_style_sample(&a, ImageFormat::Arw));
}

#[test]
fn target_encoding_round_trips() {
    let c = ctx(2, "Sony");
    let want = user_edit(&c);
    let mut a = targets::template_part(&want);
    let f = targets::TargetFrame::of(&c);
    for t in targets::TARGETS {
        t.set(&mut a, t.get(&want, &f), &f);
    }
    assert_eq!(a.exposure, want.exposure);
    assert_eq!(a.white_balance, want.white_balance);
    assert_eq!(a.hsl, want.hsl);
}

#[test]
fn slider_values_cover_the_groups() {
    let v = slider_values(&user_edit(&ctx(0, "Sony")), None);
    for g in ["whiteBalance", "tone", "presence", "hsl", "toneCurve", "colorGrading", "calibration", "detail"] {
        assert!(v.iter().any(|(gr, _, _)| gr == g), "{g}");
    }
}

#[test]
fn refine_solves_exposure_to_the_predicted_output_brightness() {
    let (model, _) = train(&samples(60), &TrainOptions { linear_only: true, ..Default::default() }, &|_| {}).unwrap();
    let c = ctx(5, "Sony");
    let reference = model.predict_output(&c).unwrap();
    assert!((reference.log_mean_luma + 2.0).abs() < 0.05, "{}", reference.log_mean_luma);
    let mut start = model.predict(&c).adjustments;
    start.exposure = 0.0;
    let mut renders = 0;
    let mut measure = |a: &ParametricAdjustments| -> crate::ipc::error::AppResult<ImageStats> {
        renders += 1;
        let mut s = reference.clone();
        s.log_mean_luma = c.render.log_mean_luma + a.exposure;
        s.white_balance = match a.white_balance {
            WhiteBalance::Custom { temperature_k, tint } => Some(WhiteBalanceValues { temperature_k, tint }),
            WhiteBalance::AsShot => c.as_shot,
        };
        Ok(s)
    };
    let out = model.refine(&c, &start, RefineGroups { exposure: true, ..Default::default() }, &mut measure).unwrap();
    let want = -2.0 - c.render.log_mean_luma;
    assert!((out.exposure - want).abs() < 0.1, "exposure {} want {want}", out.exposure);
    assert!(renders <= 8);
}
