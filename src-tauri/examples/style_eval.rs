//! Style learning evaluation (Phase 8b "Style learning / auto edit").
//!
//! ```text
//! cargo run --release --example style_eval -- [--folder DIR] [--out DIR] [--size PX]
//!     [--train-frac 0.6] [--split camera|time] [--rebuild] [--limit N] [--linear-only]
//!     [--save-sheets N] [--sheet-stems A,B]
//! ```
//! - `--folder`: RAWs + Lightroom `<stem>.xmp` sidecars, read in place and never written
//!   (default: `$SIEVE_SAMPLE_XMP_DIR`, then the user's proposal shoot).
//! - Dataset (`<out>/dataset.json`, reused unless `--rebuild`): every RAW's embedded preview
//!   -> scene features -> scenes (Phase 7 `detect::group`, default options); every edited
//!   frame (sidecar with develop settings, `is_style_sample`) -> a neutral render at
//!   `STATS_MAX_EDGE` -> `FrameContext` + the user's settings.
//! - Split: capture-time order, first `--train-frac` train, the rest held out (a later part
//!   of the shoot, i.e. later scenes / light): per camera (default) or one global cut.
//! - Held-out frames are rendered at `--size` (default 768) with: the user's settings (masks
//!   removed: the plain pipeline cannot evaluate Lightroom mattes here), the prediction,
//!   "no edit" (format defaults), a reference Auto tone (see [`auto_tone`]; Sieve has no
//!   Auto yet), and "preset" (the camera template + training-mean sliders). The user's crop
//!   is applied to all so only colour/tone differ. Reports CIEDE2000 mean vs the user's
//!   render (3x3 box filter, every 3rd pixel as `parity_eval`) and slider MAE per group.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use sieve_lib::develop::{camera, pipeline, source};
use sieve_lib::ipc::types::{ImageFormat, ImageStats, ParametricAdjustments, SceneDetectOptions};
use sieve_lib::ml::style_model::{
    self, features, slider_values, targets, FrameContext, RenderFeatures, SceneContext, StyleSample, TrainOptions,
};
use sieve_lib::profiles::ProfileLibrary;
use sieve_lib::raw;
use sieve_lib::scene::{self, detect, DetectFrame, SceneFeatures};
use sieve_lib::xmp;

const NM: usize = 6;
const DEFAULT_FOLDER: &str = "/Users/gurjotsingh/Pictures/Jasmit Natalie Proposal/10060918";

#[derive(Serialize, Deserialize, Clone)]
struct Entry {
    path: PathBuf,
    format: ImageFormat,
    captured_at_ms: Option<i64>,
    scene: i64,
    context: FrameContext,
    user: ParametricAdjustments,
    has_masks: bool,
    black_and_white: bool,
    /// The user's settings rendered at STATS_MAX_EDGE without crop/masks, measured.
    #[serde(default)]
    edited: Option<RenderFeatures>,
}

#[derive(Serialize, Deserialize)]
struct Dataset {
    folder: PathBuf,
    raws: usize,
    scenes: usize,
    entries: Vec<Entry>,
}

// ---------------------------------------------------------------------------------------
// Colour difference (as parity_eval).

fn lab(rgb: [f32; 3]) -> [f64; 3] {
    let lin = rgb.map(|c| {
        let e = f64::from(c) / 255.0;
        if e <= 0.04045 {
            e / 12.92
        } else {
            ((e + 0.055) / 1.055).powf(2.4)
        }
    });
    let x = (0.4124564 * lin[0] + 0.3575761 * lin[1] + 0.1804375 * lin[2]) / 0.95047;
    let y = 0.2126729 * lin[0] + 0.7151522 * lin[1] + 0.0721750 * lin[2];
    let z = (0.0193339 * lin[0] + 0.1191920 * lin[1] + 0.9503041 * lin[2]) / 1.08883;
    let f = |t: f64| if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn de2000(l1: [f64; 3], l2: [f64; 3]) -> f64 {
    let (lp1, a1, b1) = (l1[0], l1[1], l1[2]);
    let (lp2, a2, b2) = (l2[0], l2[1], l2[2]);
    let c1 = (a1 * a1 + b1 * b1).sqrt();
    let c2 = (a2 * a2 + b2 * b2).sqrt();
    let cm = (c1 + c2) / 2.0;
    let g = 0.5 * (1.0 - (cm.powi(7) / (cm.powi(7) + 25f64.powi(7))).sqrt());
    let (a1p, a2p) = (a1 * (1.0 + g), a2 * (1.0 + g));
    let (c1p, c2p) = ((a1p * a1p + b1 * b1).sqrt(), (a2p * a2p + b2 * b2).sqrt());
    let h = |b: f64, a: f64| {
        let v = b.atan2(a).to_degrees();
        if v < 0.0 {
            v + 360.0
        } else {
            v
        }
    };
    let (h1p, h2p) = (h(b1, a1p), h(b2, a2p));
    let dl = lp2 - lp1;
    let dc = c2p - c1p;
    let mut dh = h2p - h1p;
    if c1p * c2p != 0.0 {
        if dh > 180.0 {
            dh -= 360.0;
        } else if dh < -180.0 {
            dh += 360.0;
        }
    } else {
        dh = 0.0;
    }
    let dhh = 2.0 * (c1p * c2p).sqrt() * (dh.to_radians() / 2.0).sin();
    let lm = (lp1 + lp2) / 2.0;
    let cmp = (c1p + c2p) / 2.0;
    let hm = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * (hm - 30.0).to_radians().cos()
        + 0.24 * (2.0 * hm).to_radians().cos()
        + 0.32 * (3.0 * hm + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hm - 63.0).to_radians().cos();
    let dtheta = 30.0 * (-((hm - 275.0) / 25.0).powi(2)).exp();
    let rc = 2.0 * (cmp.powi(7) / (cmp.powi(7) + 25f64.powi(7))).sqrt();
    let sl = 1.0 + 0.015 * (lm - 50.0).powi(2) / (20.0 + (lm - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * cmp;
    let sh = 1.0 + 0.015 * cmp * t;
    let rt = -(2.0 * dtheta).to_radians().sin() * rc;
    ((dl / sl).powi(2) + (dc / sc).powi(2) + (dhh / sh).powi(2) + rt * (dc / sc) * (dhh / sh)).sqrt()
}

fn box3(px: &[u8], w: usize, h: usize) -> Vec<[f32; 3]> {
    let mut out = vec![[0.0f32; 3]; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f32; 3];
            let mut n = 0.0;
            for yy in y.saturating_sub(1)..(y + 2).min(h) {
                for xx in x.saturating_sub(1)..(x + 2).min(w) {
                    let p = &px[(yy * w + xx) * 3..];
                    for c in 0..3 {
                        acc[c] += f32::from(p[c]);
                    }
                    n += 1.0;
                }
            }
            out[y * w + x] = acc.map(|v| v / n);
        }
    }
    out
}

fn mean_de(a: &pipeline::RenderedImage, b: &pipeline::RenderedImage) -> f64 {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let (w, h) = (a.width as usize, a.height as usize);
    let (x, y) = (box3(&a.rgb, w, h), box3(&b.rgb, w, h));
    let d: Vec<f64> = x.iter().zip(&y).step_by(3).map(|(p, q)| de2000(lab(*p), lab(*q))).collect();
    d.iter().sum::<f64>() / d.len().max(1) as f64
}

// ---------------------------------------------------------------------------------------
// Rendering.

struct Decoded {
    img: source::LinearImage,
    meta: source::SourceMeta,
    orientation: u8,
    sidecar: PathBuf,
}

fn decode(path: &Path) -> Decoded {
    let format = raw::format_from_extension(path).expect("supported file");
    let mut buf = Vec::new();
    let ex = raw::extract(path, format, &mut buf).expect("extract");
    let orientation = ex.meta.orientation.map(|o| o as u8).unwrap_or(1);
    let (img, meta) = source::decode_half_size_meta(path).expect("decode");
    Decoded { img, meta, orientation, sidecar: xmp::sidecar_path(path) }
}

fn render(d: &Decoded, adj: &ParametricAdjustments, size: u32) -> pipeline::RenderedImage {
    let lib = ProfileLibrary::shared();
    let profile = camera::resolve(&d.meta, &adj.profile, &lib, Some(&d.sidecar));
    let prep = source::prepare(&d.img, d.orientation, &adj.crop, None, size);
    let tone = pipeline::tone_context(&d.img, d.orientation, adj, &profile);
    let input = pipeline::RenderInput {
        width: prep.width,
        height: prep.height,
        pixels: &prep.pixels,
        color: &d.img.color,
        frame_long_edge: prep.frame_long_edge,
        view: prep.view,
        profile: &profile,
        seed: 1,
        quality: pipeline::Quality::Preview,
        tone: Some(&tone),
    };
    pipeline::render(&input, adj, None)
}

fn as_shot(d: &Decoded, adj: &ParametricAdjustments) -> Option<sieve_lib::ipc::types::WhiteBalanceValues> {
    let profile = camera::resolve(&d.meta, &adj.profile, &ProfileLibrary::shared(), Some(&d.sidecar));
    camera::as_shot_values(&d.img.color, &profile)
}

/// Reference "Auto tone" baseline (Sieve has no Auto yet): Lightroom-style global
/// corrections from the neutral render, as-shot WB. Exposure by 3 secant steps on the
/// rendered log-mean luminance towards -2.5 (display middle grey), capped at +-2 EV, then Highlights /
/// Shadows / Whites / Blacks from the rendered percentiles, Vibrance +10.
fn auto_tone(d: &Decoded, format: ImageFormat) -> ParametricAdjustments {
    let mut adj = ParametricAdjustments::defaults_for(format);
    let measure = |a: &ParametricAdjustments| RenderFeatures::from_render(&render(d, a, 320));
    let target = -2.5f32;
    let (mut e0, mut f0) = (0.0f32, measure(&adj).log_mean_luma - target);
    let mut e1 = (-f0).clamp(-2.0, 2.0);
    for _ in 0..3 {
        adj.exposure = e1;
        let f1 = measure(&adj).log_mean_luma - target;
        if f1.abs() < 0.05 || (f1 - f0).abs() < 1e-4 {
            break;
        }
        let e2 = (e1 - f1 * (e1 - e0) / (f1 - f0)).clamp(-2.0, 2.0);
        (e0, f0, e1) = (e1, f1, e2);
    }
    adj.exposure = (e1 * 100.0).round() / 100.0;
    let m = measure(&adj);
    let enc = |lp: f32| {
        let y = lp.exp2();
        if y <= 0.003_130_8 {
            y * 12.92
        } else {
            1.055 * y.powf(1.0 / 2.4) - 0.055
        }
    };
    let [p1, p10, _, p90, p99] = m.log_percentiles.map(enc);
    adj.highlights = -(m.clipped_highlights * 400.0 + (p90 - 0.72).max(0.0) * 200.0).clamp(0.0, 70.0).round();
    adj.shadows = ((0.22 - p10).max(0.0) * 220.0).clamp(0.0, 60.0).round();
    adj.whites = ((0.93 - p99) * 150.0).clamp(-40.0, 40.0).round();
    adj.blacks = ((0.02 - p1) * 400.0).clamp(-40.0, 30.0).round();
    adj.vibrance = 10.0;
    adj
}

// ---------------------------------------------------------------------------------------
// Dataset.

fn build_dataset(folder: &Path, limit: usize) -> Dataset {
    let t = Instant::now();
    let mut raws: Vec<PathBuf> = std::fs::read_dir(folder)
        .expect("folder")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| raw::format_from_extension(p).is_some_and(|f| f.is_raw()))
        .collect();
    raws.sort();
    // 1. Previews -> scene features + EXIF, all frames.
    struct Frame {
        path: PathBuf,
        format: ImageFormat,
        meta: raw::meta::ImageMeta,
        features: Option<SceneFeatures>,
    }
    let mut frames: Vec<Frame> = raws
        .par_iter()
        .map(|p| {
            let format = raw::format_from_extension(p).unwrap();
            let mut buf = Vec::new();
            let ex = raw::extract(p, format, &mut buf).expect("extract");
            let features = match &ex.preview {
                Ok(raw::Preview::Embedded) => raw::turbo::decode_rgb(&buf, 256, u64::MAX)
                    .ok()
                    .map(|d| scene::features::from_rgb(&d.pixels, d.width as usize, d.height as usize)),
                _ => None,
            };
            Frame { path: p.clone(), format, meta: ex.meta, features }
        })
        .collect();
    frames.sort_by_key(|f| (f.meta.captured_at_ms.unwrap_or(i64::MAX), f.path.clone()));
    let detect_frames: Vec<DetectFrame> = frames
        .iter()
        .enumerate()
        .map(|(i, f)| DetectFrame {
            id: i as i64,
            folder_id: 1,
            captured_at_ms: f.meta.captured_at_ms,
            file_name: f.path.file_name().unwrap().to_string_lossy().into_owned(),
            burst_group_id: None,
            preview_path: None,
            features: f.features.clone(),
        })
        .collect();
    let groups = detect::group(&detect_frames, &SceneDetectOptions::default());
    let mut scene_of = vec![0i64; frames.len()];
    let mut contexts = Vec::new();
    for (s, g) in groups.iter().enumerate() {
        for &id in g {
            scene_of[id as usize] = s as i64;
        }
        let members: Vec<&Frame> = g.iter().map(|&id| &frames[id as usize]).collect();
        contexts.push(SceneContext::from_members(
            members.iter().filter_map(|f| f.features.as_ref()),
            members.iter().map(|f| features::ev100(f.meta.iso, f.meta.shutter_seconds, f.meta.aperture)),
        ));
    }
    println!("{} RAWs, {} scenes: previews + scenes in {:.1}s", frames.len(), groups.len(), t.elapsed().as_secs_f64());

    // 2. Edited frames -> neutral render features.
    let edited: Vec<(usize, ParametricAdjustments)> = frames
        .iter()
        .enumerate()
        .filter_map(|(i, f)| {
            let text = std::fs::read_to_string(xmp::sidecar_path(&f.path)).ok()?;
            let adj = xmp::packet::parse_for(&text, f.format).ok()?.develop?;
            style_model::is_style_sample(&adj, f.format).then_some((i, adj))
        })
        .take(limit)
        .collect();
    let t = Instant::now();
    let done = AtomicUsize::new(0);
    let entries: Vec<Entry> = edited
        .par_iter()
        .map(|(i, user)| {
            let f = &frames[*i];
            let d = decode(&f.path);
            let neutral = ParametricAdjustments::defaults_for(f.format);
            let render_features = RenderFeatures::from_render(&render(&d, &neutral, scene::STATS_MAX_EDGE));
            let mut plain = style_model::targets::strip_per_image(user);
            plain.masks.clear();
            let edited_stats = RenderFeatures::from_render(&render(&d, &plain, scene::STATS_MAX_EDGE));
            let context = FrameContext {
                format: f.format,
                make: d.meta.make.clone(),
                model: d.meta.model.clone(),
                iso: f.meta.iso,
                shutter_seconds: f.meta.shutter_seconds,
                aperture: f.meta.aperture,
                focal_length_mm: f.meta.focal_length_mm,
                as_shot: as_shot(&d, &neutral),
                render: render_features,
                preview: f.features.clone(),
                scene: contexts[scene_of[*i] as usize].clone(),
            };
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_multiple_of(50) {
                eprintln!("  features {n}/{}", edited.len());
            }
            Entry {
                path: f.path.clone(),
                format: f.format,
                captured_at_ms: f.meta.captured_at_ms,
                scene: scene_of[*i],
                context,
                has_masks: !user.masks.is_empty(),
                black_and_white: user.black_and_white.enabled,
                user: user.clone(),
                edited: Some(edited_stats),
            }
        })
        .collect();
    println!(
        "{} edited frames: neutral-render features in {:.1}s ({:.0} ms/frame wall)",
        entries.len(),
        t.elapsed().as_secs_f64(),
        t.elapsed().as_secs_f64() * 1000.0 / entries.len().max(1) as f64
    );
    Dataset { folder: folder.to_owned(), raws: frames.len(), scenes: groups.len(), entries }
}

// ---------------------------------------------------------------------------------------

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len().max(1) as f64
}

fn pct(v: &[f64], p: f64) -> f64 {
    let mut v = v.to_vec();
    v.sort_by(f64::total_cmp);
    v.get(((v.len() as f64 - 1.0) * p).round() as usize).copied().unwrap_or(0.0)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let flag = |k: &str| args.iter().any(|a| a == k);
    let folder = PathBuf::from(
        arg("--folder").or_else(|| std::env::var("SIEVE_SAMPLE_XMP_DIR").ok()).unwrap_or_else(|| DEFAULT_FOLDER.into()),
    );
    let out = PathBuf::from(arg("--out").unwrap_or_else(|| "../test-data/style-eval".into()));
    let size: u32 = arg("--size").and_then(|v| v.parse().ok()).unwrap_or(768);
    let train_frac: f64 = arg("--train-frac").and_then(|v| v.parse().ok()).unwrap_or(0.6);
    let limit: usize = arg("--limit").and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
    std::fs::create_dir_all(&out).expect("out dir");
    if out.canonicalize().unwrap().starts_with("/Users/gurjotsingh/Pictures") {
        panic!("--out must not be under ~/Pictures");
    }
    ProfileLibrary::shared().warm();

    let cache = out.join("dataset.json");
    let ds: Dataset = match std::fs::read(&cache).ok().filter(|_| !flag("--rebuild")) {
        Some(b)
            if serde_json::from_slice::<Dataset>(&b).is_ok_and(|d| d.entries.iter().all(|e| e.edited.is_some())) =>
        {
            serde_json::from_slice(&b).expect("dataset.json")
        }
        _ => {
            let ds = build_dataset(&folder, limit);
            std::fs::write(&cache, serde_json::to_vec(&ds).unwrap()).unwrap();
            ds
        }
    };
    let mut entries = ds.entries.clone();
    entries.sort_by_key(|e| (e.captured_at_ms.unwrap_or(i64::MAX), e.path.clone()));
    // Split: `--split camera` (default) = capture-time split within each camera (the first
    // `train_frac` of every body's edits train, its later frames are held out); `--split time`
    // = one global capture-time cut (bodies first used late in the shoot are unseen).
    let split = arg("--split").unwrap_or_else(|| "camera".into());
    let mut is_train = vec![false; entries.len()];
    if split == "time" {
        let n_train = ((entries.len() as f64) * train_frac).round() as usize;
        is_train.iter_mut().take(n_train).for_each(|v| *v = true);
    } else {
        let mut per: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, e) in entries.iter().enumerate() {
            per.entry(e.context.camera_key()).or_default().push(i);
        }
        for idx in per.values() {
            let n = ((idx.len() as f64) * train_frac).round() as usize;
            idx.iter().take(n).for_each(|&i| is_train[i] = true);
        }
    }
    let train_e: Vec<Entry> = entries.iter().zip(&is_train).filter(|(_, t)| **t).map(|(e, _)| e.clone()).collect();
    let test_e: Vec<Entry> = entries.iter().zip(&is_train).filter(|(_, t)| !**t).map(|(e, _)| e.clone()).collect();
    let (train_e, test_e) = (&train_e[..], &test_e[..]);
    let scenes_train: std::collections::BTreeSet<i64> = train_e.iter().map(|e| e.scene).collect();
    let shared = test_e.iter().filter(|e| scenes_train.contains(&e.scene)).count();
    println!(
        "\ndataset: {} RAWs, {} scenes, {} edited; split `{split}` by capture time: train {} / held-out {} ({} held-out frames share a scene with training)",
        ds.raws,
        ds.scenes,
        entries.len(),
        train_e.len(),
        test_e.len(),
        shared
    );

    let to_sample = |e: &Entry| StyleSample {
        context: e.context.clone(),
        adjustments: e.user.clone(),
        group: Some(e.scene),
        captured_at_ms: e.captured_at_ms,
        edited: e.edited.clone(),
    };
    let samples: Vec<StyleSample> = train_e.iter().map(to_sample).collect();
    let recency = match arg("--half-life").as_deref() {
        None | Some("off") => style_model::RecencyWeighting::Off,
        Some("auto") => style_model::RecencyWeighting::Auto,
        Some(v) => style_model::RecencyWeighting::HalfLife(v.parse().expect("--half-life")),
    };
    let opts = TrainOptions { linear_only: flag("--linear-only"), recency, ..Default::default() };
    let t = Instant::now();
    let (model, report) = style_model::train(&samples, &opts, &|_| {}).expect("train");
    let train_ms = t.elapsed().as_secs_f64() * 1000.0;
    let json = model.to_json();
    std::fs::write(out.join("style-model.json"), &json).unwrap();
    println!(
        "trained on {} samples ({} cameras) in {train_ms:.0} ms; model {} KB",
        report.samples,
        report.cameras,
        json.len() / 1024
    );
    println!("recency half-life: {:?} samples; encodings kept: {:?}", report.half_life, report.chosen);
    println!(
        "cross-validated (scene folds, training part, recency-weighted) MAE per encoding: model vs mean-predictor"
    );
    for (name, (m, b)) in &report.cv {
        println!("  {name:<20} {m:>8.3} vs {b:>8.3}");
    }
    println!("kept:");
    for tm in &model.targets {
        println!(
            "  {:<17} {:>8.3} vs {:>8.3}  (lambda {}, {} trees)",
            tm.name,
            tm.cv_mae,
            tm.cv_mae_mean,
            tm.lambda,
            tm.gbdt.trees.len()
        );
    }
    for tpl in &model.templates {
        println!("  template {:<28} support {}/{}", tpl.camera_key, tpl.support, tpl.samples);
    }

    // Prediction timing (features -> adjustments; the neutral render is timed below).
    let t = Instant::now();
    let reps = 1000;
    for i in 0..reps {
        std::hint::black_box(model.predict(&test_e[i % test_e.len()].context));
    }
    let predict_us = t.elapsed().as_secs_f64() * 1e6 / reps as f64;

    // "Preset" baseline: template + training-mean sliders.
    // Mean per slider: plain slider values, absolute WB (the user's typical temperature/tint).
    let plain: Vec<&targets::Target> = targets::TARGETS
        .iter()
        .filter(|t| matches!(t.name, "temperatureAbsMired" | "tintAbs") || (t.slot == t.name && t.slot != "tint"))
        .collect();
    let mean_sliders: Vec<f32> = plain
        .iter()
        .map(|t| {
            samples.iter().map(|s| t.get(&s.adjustments, &targets::TargetFrame::of(&s.context))).sum::<f32>()
                / samples.len() as f32
        })
        .collect();
    let preset = |ctx: &FrameContext| {
        let key = ctx.camera_key();
        let mut a =
            model.templates.iter().find(|t| t.camera_key == key).unwrap_or(&model.global_template).adjustments.clone();
        for (t, v) in plain.iter().zip(&mean_sliders) {
            t.set(&mut a, *v, &targets::TargetFrame::of(ctx));
        }
        a
    };

    // Held-out renders.
    const METHODS: [&str; NM] = ["stage A", "A+refine exp", "A+refine exp+wb", "no edit", "auto tone", "preset+mean"];
    struct Row {
        stem: String,
        format: ImageFormat,
        masks: bool,
        bw: bool,
        de: Vec<f64>,
        diag: [f64; 3],
        neutral_ms: f64,
        refine_ms: f64,
        adjs: Vec<ParametricAdjustments>,
        user: ParametricAdjustments,
        as_shot: Option<sieve_lib::ipc::types::WhiteBalanceValues>,
    }
    let t = Instant::now();
    let done = AtomicUsize::new(0);
    let sheets: usize = arg("--save-sheets").and_then(|v| v.parse().ok()).unwrap_or(0);
    let sheet_list = arg("--sheet-stems").unwrap_or_default();
    let sheet_stems: Vec<&str> = sheet_list.split(',').filter(|s| !s.is_empty()).collect();
    let rows: Vec<Row> = test_e
        .par_iter()
        .enumerate()
        .map(|(k, e)| {
            let d = decode(&e.path);
            let t_n = Instant::now();
            let neutral_small = render(&d, &ParametricAdjustments::defaults_for(e.format), scene::STATS_MAX_EDGE);
            let _ = RenderFeatures::from_render(&neutral_small);
            let neutral_ms = t_n.elapsed().as_secs_f64() * 1000.0;
            let mut user = e.user.clone();
            user.masks.clear();
            let with_crop = |mut a: ParametricAdjustments| {
                a.crop = e.user.crop;
                a
            };
            let stage_a = model.predict(&e.context).adjustments;
            let mut measure = |a: &ParametricAdjustments| -> sieve_lib::ipc::error::AppResult<ImageStats> {
                let img = render(&d, a, scene::STATS_MAX_EDGE);
                let mut st = scene::stats::measure(0, &img, None);
                st.white_balance = scene::stats::effective_white_balance(a, e.context.as_shot);
                st.as_shot = e.context.as_shot;
                Ok(st)
            };
            let t_r = Instant::now();
            let exp = style_model::RefineGroups { exposure: true, ..Default::default() };
            let exp_wb = style_model::RefineGroups { exposure: true, white_balance: true, tone: false };
            let refined = model.refine(&e.context, &stage_a, exp, &mut measure).expect("refine");
            let refine_ms = t_r.elapsed().as_secs_f64() * 1000.0;
            let refined_tone = model.refine(&e.context, &stage_a, exp_wb, &mut measure).expect("refine");
            let adjs = vec![
                with_crop(stage_a),
                with_crop(refined),
                with_crop(refined_tone),
                with_crop(ParametricAdjustments::defaults_for(e.format)),
                with_crop(auto_tone(&d, e.format)),
                with_crop(preset(&e.context)),
            ];
            let reference = render(&d, &user, size);
            let rendered: Vec<pipeline::RenderedImage> = adjs.iter().map(|a| render(&d, a, size)).collect();
            let de: Vec<f64> = rendered.iter().map(|r| mean_de(r, &reference)).collect();
            // Diagnostics: stage A with the user's WB / exposure / all basic tone.
            let mut diag = [0.0; 3];
            for (k, slot) in ["wb", "exposure", "tone"].iter().enumerate() {
                let mut a = adjs[0].clone();
                match *slot {
                    "wb" => a.white_balance = user.white_balance,
                    "exposure" => a.exposure = user.exposure,
                    _ => {
                        a.exposure = user.exposure;
                        a.contrast = user.contrast;
                        a.highlights = user.highlights;
                        a.shadows = user.shadows;
                        a.whites = user.whites;
                        a.blacks = user.blacks;
                    }
                }
                diag[k] = mean_de(&render(&d, &a, size), &reference);
            }
            let stem = e.path.file_stem().unwrap().to_string_lossy().into_owned();
            if k < sheets || sheet_stems.iter().any(|s| *s == stem) {
                let (w, h) = (reference.width as usize, reference.height as usize);
                // user | stage A | no edit | auto
                let mut px = vec![0u8; w * 4 * h * 3];
                for (c, img) in [&reference, &rendered[0], &rendered[3], &rendered[4]].iter().enumerate() {
                    for y in 0..h {
                        px[(y * w * 4 + c * w) * 3..(y * w * 4 + c * w + w) * 3]
                            .copy_from_slice(&img.rgb[y * w * 3..(y + 1) * w * 3]);
                    }
                }
                let j = raw::turbo::encode_rgb_444(&px, (w * 4) as u32, h as u32, 85).unwrap();
                std::fs::write(out.join(format!("{stem}.sheet.jpg")), j).unwrap();
            }
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_multiple_of(25) {
                eprintln!("  eval {n}/{}", test_e.len());
            }
            Row {
                stem,
                format: e.format,
                masks: e.has_masks,
                bw: e.black_and_white,
                de,
                diag,
                neutral_ms,
                refine_ms,
                adjs,
                user: e.user.clone(),
                as_shot: e.context.as_shot,
            }
        })
        .collect();
    println!("rendered {} held-out frames in {:.1}s", rows.len(), t.elapsed().as_secs_f64());

    println!("\n== Held-out dE2000 (vs the user's own settings rendered by Sieve), {} frames ==", rows.len());
    println!("  {:<14} {:>6} {:>6} {:>6}   stage A better on", "method", "mean", "median", "p90");
    for (m, name) in METHODS.iter().enumerate() {
        let v: Vec<f64> = rows.iter().map(|r| r.de[m]).collect();
        let w = rows.iter().filter(|r| r.de[0] < r.de[m]).count();
        let wins = if m == 0 { String::new() } else { format!("{w}/{}", rows.len()) };
        println!("  {:<14} {:>6.2} {:>6.2} {:>6.2}   {}", name, mean(&v), pct(&v, 0.5), pct(&v, 0.9), wins);
    }
    for (k, name) in ["user WB", "user exposure", "user basic tone"].iter().enumerate() {
        println!("  (diagnostic) stage A + {name}: {:.2}", mean(&rows.iter().map(|r| r.diag[k]).collect::<Vec<_>>()));
    }
    for (label, filt) in [
        ("ARW", Box::new(|r: &Row| r.format == ImageFormat::Arw) as Box<dyn Fn(&Row) -> bool>),
        ("CR3", Box::new(|r: &Row| r.format == ImageFormat::Cr3)),
        ("RAF", Box::new(|r: &Row| r.format == ImageFormat::Raf)),
        ("B&W edits", Box::new(|r: &Row| r.bw)),
        ("had masks", Box::new(|r: &Row| r.masks)),
    ] {
        let sel: Vec<&Row> = rows.iter().filter(|r| filt(r)).collect();
        if sel.is_empty() {
            continue;
        }
        let m: Vec<String> = (0..NM)
            .map(|k| format!("{} {:.2}", METHODS[k], mean(&sel.iter().map(|r| r.de[k]).collect::<Vec<_>>())))
            .collect();
        println!("  {label:<10} n={:<4} {}", sel.len(), m.join(" | "));
    }

    // Slider MAE per group.
    println!("\n== Held-out slider MAE per group (slider units; WB temperature in K) ==");
    let mut per: BTreeMap<(String, String), Vec<Vec<f64>>> = BTreeMap::new();
    for r in &rows {
        let truth: BTreeMap<(String, String), f32> =
            slider_values(&r.user, r.as_shot).into_iter().map(|(g, n, v)| ((g, n), v)).collect();
        for (m, a) in r.adjs.iter().enumerate() {
            for (g, n, v) in slider_values(a, r.as_shot) {
                if let Some(t) = truth.get(&(g.clone(), n.clone())) {
                    per.entry((g, n)).or_insert_with(|| vec![Vec::new(); NM])[m].push(f64::from((v - t).abs()));
                }
            }
        }
    }
    let mut groups: BTreeMap<String, Vec<Vec<f64>>> = BTreeMap::new();
    for ((g, _), v) in &per {
        let e = groups.entry(g.clone()).or_insert_with(|| vec![Vec::new(); NM]);
        for m in 0..NM {
            e[m].push(mean(&v[m]));
        }
    }
    let head: Vec<String> = METHODS.iter().map(|m| format!("{m:>14}")).collect();
    println!("  {:<14}{}", "group", head.join(""));
    for (g, v) in &groups {
        let cols: Vec<String> = v.iter().map(|c| format!("{:>14.2}", mean(c))).collect();
        println!("  {:<14}{}", g, cols.join(""));
    }
    println!("  per slider:");
    for ((g, n), v) in &per {
        let key = matches!(g.as_str(), "whiteBalance" | "tone" | "presence")
            || n == "detail.sharpening.amount"
            || n == "detail.noiseReduction.luminance";
        if key {
            let cols: Vec<String> = v.iter().map(|c| format!("{:>14.2}", mean(c))).collect();
            println!("    {:<36}{}", format!("{g}.{n}"), cols.join(""));
        }
    }

    let nm: Vec<f64> = rows.iter().map(|r| r.neutral_ms).collect();
    let rm: Vec<f64> = rows.iter().map(|r| r.refine_ms).collect();
    println!(
        "\n== Timing ==\n  train {train_ms:.0} ms ({} samples); predict {predict_us:.1} us/frame; neutral 640 px render + features {:.0} ms median; refine (exposure+WB solve) {:.0} ms median (parallel eval, {} threads, decode excluded)",
        samples.len(),
        pct(&nm, 0.5),
        pct(&rm, 0.5),
        rayon::current_num_threads()
    );
    let worst: Vec<String> = {
        let mut r: Vec<&Row> = rows.iter().collect();
        r.sort_by(|a, b| b.de[0].total_cmp(&a.de[0]));
        r.iter().take(8).map(|r| format!("{} {:.2} (no edit {:.2})", r.stem, r.de[0], r.de[3])).collect()
    };
    println!("  worst stage A: {}", worst.join(", "));
}
