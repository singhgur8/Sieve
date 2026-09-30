//! Develop render benchmark over real RAWs (read-only).
//!
//! ```text
//! cargo run --release --example render_bench -- [folder] [count] [--check <out_dir>]
//! ```
//! `folder` defaults to `$SIEVE_SAMPLES`, then "/Users/gurjotsingh/Pictures/test RAWS".
//! Reports cold decode ms, warm 2048 px render ms (several slider variations, p50/p95),
//! the stage split (pipeline vs JPEG encode), region (1:1 detail) render ms and the
//! first-render prepare cost. `--check` writes sample renders (neutral, +1 EV, 3200 K, a
//! LUT, strong HSL, ...) plus the camera's embedded JPEG for comparison.

use std::path::PathBuf;
use std::time::Instant;

use sieve_lib::develop::{pipeline, source, DevelopCache, DevelopConfig, SourceImage};
use sieve_lib::ipc::types::{
    HslAdjustments, HslChannels, LutRef, NormRect, ParametricAdjustments, RenderOptions, RenderSlot, WhiteBalance,
};
use sieve_lib::lut::LutLibrary;
use sieve_lib::raw;

const DEFAULT_SAMPLES: &str = "/Users/gurjotsingh/Pictures/test RAWS";

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if v.is_empty() {
        return f64::NAN;
    }
    let i = ((v.len() as f64 - 1.0) * p).round() as usize;
    v[i]
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// A 17^3 "warm film" LUT: gentle S-curve, warm highlights, teal shadows.
fn film_cube() -> String {
    let n = 17;
    let mut s = String::from("TITLE \"Sieve bench warm film\"\nLUT_3D_SIZE 17\n");
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let c = [r, g, b].map(|v| v as f32 / (n - 1) as f32);
                let l = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
                let s_curve = |x: f32| x + 0.15 * (x - 0.5) * (1.0 - (2.0 * x - 1.0).abs());
                let warm = (l - 0.5) * 0.08;
                let out = [s_curve(c[0]) + warm, s_curve(c[1]) + warm * 0.3, s_curve(c[2]) - warm];
                s.push_str(&format!(
                    "{:.6} {:.6} {:.6}\n",
                    out[0].clamp(0.0, 1.0),
                    out[1].clamp(0.0, 1.0),
                    out[2].clamp(0.0, 1.0)
                ));
            }
        }
    }
    s
}

fn variations(lut: &str) -> Vec<(&'static str, ParametricAdjustments)> {
    let d = ParametricAdjustments::default;
    vec![
        ("neutral", d()),
        ("exposure_plus1", ParametricAdjustments { exposure: 1.0, ..d() }),
        ("exposure_minus1", ParametricAdjustments { exposure: -1.0, ..d() }),
        (
            "wb_3200k",
            ParametricAdjustments { white_balance: WhiteBalance::Custom { temperature_k: 3200.0, tint: 0.0 }, ..d() },
        ),
        (
            "wb_7500k_tint20",
            ParametricAdjustments { white_balance: WhiteBalance::Custom { temperature_k: 7500.0, tint: 20.0 }, ..d() },
        ),
        ("contrast_hl_sh", ParametricAdjustments { contrast: 40.0, highlights: -70.0, shadows: 60.0, ..d() }),
        ("whites_blacks", ParametricAdjustments { whites: 30.0, blacks: -40.0, ..d() }),
        ("clarity_texture", ParametricAdjustments { clarity: 50.0, texture: 40.0, ..d() }),
        ("dehaze", ParametricAdjustments { dehaze: 40.0, ..d() }),
        ("vibrance_sat", ParametricAdjustments { vibrance: 50.0, saturation: 15.0, ..d() }),
        (
            "hsl_strong",
            ParametricAdjustments {
                hsl: HslAdjustments {
                    hue: HslChannels { orange: -30.0, green: 60.0, blue: -40.0, ..Default::default() },
                    saturation: HslChannels { blue: 60.0, green: -80.0, aqua: 40.0, ..Default::default() },
                    luminance: HslChannels { blue: -60.0, orange: 25.0, ..Default::default() },
                },
                ..d()
            },
        ),
        ("lut_film", ParametricAdjustments { lut: Some(LutRef { id: lut.to_owned(), amount: 100.0 }), ..d() }),
        (
            "everything",
            ParametricAdjustments {
                exposure: 0.3,
                contrast: 20.0,
                highlights: -40.0,
                shadows: 30.0,
                whites: 10.0,
                blacks: -10.0,
                texture: 15.0,
                clarity: 20.0,
                dehaze: 10.0,
                vibrance: 25.0,
                saturation: 5.0,
                white_balance: WhiteBalance::Custom { temperature_k: 5200.0, tint: 5.0 },
                hsl: HslAdjustments {
                    luminance: HslChannels { orange: 10.0, ..Default::default() },
                    ..Default::default()
                },
                lut: Some(LutRef { id: lut.to_owned(), amount: 60.0 }),
                ..d()
            },
        ),
    ]
}

/// The sample shoot's typical Lightroom edit (AZA06589.xmp): custom WB, strong PV2012 tone,
/// HSL, parametric + point curves (master + RGB), split toning / colour grading,
/// calibration, luminance NR and sharpening, Adobe Color profile.
fn user_style() -> ParametricAdjustments {
    use sieve_lib::ipc::types::{ColorWheel, PrimaryCalibration};
    let mut a = ParametricAdjustments {
        white_balance: WhiteBalance::Custom { temperature_k: 6825.0, tint: 12.0 },
        exposure: -0.87,
        contrast: -59.0,
        highlights: -66.0,
        shadows: 30.0,
        whites: -18.0,
        blacks: 25.0,
        vibrance: 25.0,
        saturation: 5.0,
        hsl: HslAdjustments {
            hue: HslChannels {
                red: 15.0,
                yellow: -20.0,
                green: 10.0,
                aqua: 5.0,
                blue: -5.0,
                purple: 20.0,
                ..Default::default()
            },
            saturation: HslChannels {
                red: 10.0,
                orange: -10.0,
                yellow: -20.0,
                green: -40.0,
                aqua: 10.0,
                blue: -10.0,
                ..Default::default()
            },
            luminance: HslChannels { orange: -10.0, ..Default::default() },
        },
        ..Default::default()
    };
    let p = &mut a.tone_curve.parametric;
    (p.shadows, p.darks, p.lights, p.highlights) = (-5.0, -15.0, 20.0, -15.0);
    (p.shadow_split, p.midtone_split, p.highlight_split) = (15.0, 35.0, 75.0);
    let pc = &mut a.tone_curve.point;
    pc.master = vec![[0.0, 14.0], [44.0, 46.0], [106.0, 110.0], [255.0, 252.0]];
    pc.red = vec![[0.0, 0.0], [29.0, 21.0], [115.0, 133.0], [179.0, 195.0], [255.0, 255.0]];
    pc.green = vec![[0.0, 0.0], [28.0, 20.0], [115.0, 133.0], [181.0, 196.0], [255.0, 255.0]];
    pc.blue = vec![[0.0, 0.0], [28.0, 18.0], [117.0, 134.0], [179.0, 195.0], [255.0, 255.0]];
    let g = &mut a.color_grading;
    g.shadows = ColorWheel { hue: 30.0, saturation: 2.0, luminance: 0.0 };
    g.highlights = ColorWheel { hue: 30.0, saturation: 3.0, luminance: 0.0 };
    g.midtones = ColorWheel { hue: 185.0, saturation: 5.0, luminance: 0.0 };
    g.blending = 100.0;
    a.calibration.red = PrimaryCalibration { hue: 0.0, saturation: 20.0 };
    a.calibration.green = PrimaryCalibration { hue: 0.0, saturation: -25.0 };
    a.calibration.blue = PrimaryCalibration { hue: -10.0, saturation: 20.0 };
    a.detail.sharpening.amount = 20.0;
    a.detail.noise_reduction.luminance = 24.0;
    a.detail.noise_reduction.color = 0.0;
    a
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let check = args.iter().position(|a| a == "--check").map(|i| {
        let dir = PathBuf::from(args.get(i + 1).expect("--check <dir>"));
        args.drain(i..=i + 1);
        dir
    });
    let folder =
        args.first().cloned().or_else(|| std::env::var("SIEVE_SAMPLES").ok()).unwrap_or_else(|| DEFAULT_SAMPLES.into());
    let count: usize = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(6);

    let mut files: Vec<PathBuf> = std::fs::read_dir(&folder)
        .expect("read sample folder")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| raw::format_from_extension(p).is_some())
        .collect();
    files.sort();
    let step = (files.len() / count.max(1)).max(1);
    let picked: Vec<PathBuf> = files.iter().step_by(step).take(count).cloned().collect();
    println!(
        "LibRaw {} | {} of {} files from {folder} | rayon threads {}",
        raw::libraw::version(),
        picked.len(),
        files.len(),
        rayon::current_num_threads()
    );

    let tmp = tempfile_dir();
    let luts = LutLibrary::new(tmp.join("luts"));
    let cube = tmp.join("warm film.cube");
    std::fs::write(&cube, film_cube()).unwrap();
    let lut_id = luts.import(&cube).expect("import LUT").id;
    let mut vars = variations(&lut_id);
    vars.push(("user_style", user_style()));
    let mut user = Vec::new();
    let mut draft = Vec::new();

    let cache = DevelopCache::new(DevelopConfig { cache_bytes: 2048 << 20, mask_cache: None });
    let (mut decode, mut prepare, mut warm, mut pipe, mut enc, mut region) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    if let Some(dir) = &check {
        std::fs::create_dir_all(dir).unwrap();
    }

    for (i, path) in picked.iter().enumerate() {
        let format = raw::format_from_extension(path).unwrap();
        let mut buf = Vec::new();
        let ex = raw::extract(path, format, &mut buf).expect("extract");
        let orientation = ex.meta.orientation.map(|o| o as u8);
        let src = SourceImage { id: i as i64 + 1, path: path.clone(), orientation };
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();

        // Cold decode (through the cache, as the app does).
        let t = Instant::now();
        let info = cache.info(&src).expect("decode");
        let d = ms(t);
        decode.push(d);

        // First render at 2048: includes the resample/orient "prepare" step.
        let opts = RenderOptions { max_edge: 2048, slot: RenderSlot::Main, region: None };
        let t = Instant::now();
        let first =
            cache.render(cache.ticket(src.id, RenderSlot::Main), &src, &vars[0].1, &opts, &luts).unwrap().unwrap();
        prepare.push(ms(t));

        let mut per_image = Vec::new();
        for round in 0..3 {
            for (name, adj) in &vars {
                let t = Instant::now();
                let r = cache.render(cache.ticket(src.id, RenderSlot::Main), &src, adj, &opts, &luts).unwrap().unwrap();
                let e = ms(t);
                warm.push(e);
                per_image.push(e);
                if *name == "user_style" {
                    user.push(e);
                }
                if round == 0 {
                    if let Some(dir) = &check {
                        if i < 3 || *name == "neutral" {
                            let bytes = cache.encoded(src.id, RenderSlot::Main, r.seq).unwrap();
                            std::fs::write(dir.join(format!("{stem}_{name}.jpg")), bytes.as_slice()).unwrap();
                        }
                    }
                }
            }
        }

        // Stage split on the same prepared input (direct calls).
        let (img, meta) = source::decode_half_size_meta(path).unwrap();
        let profile = sieve_lib::develop::camera::resolve(
            &meta,
            &sieve_lib::ipc::types::ProfileSettings::default(),
            &sieve_lib::profiles::ProfileLibrary::shared(),
            None,
        );
        let prep = source::prepare(
            &img,
            orientation.unwrap_or(1),
            &sieve_lib::ipc::types::CropSettings::default(),
            None,
            2048,
        );
        for (_, adj) in &vars {
            let lut = adj.lut.as_ref().and_then(|l| luts.load(&l.id).unwrap());
            let input = pipeline::RenderInput {
                width: prep.width,
                height: prep.height,
                pixels: &prep.pixels,
                color: &img.color,
                frame_long_edge: prep.frame_long_edge,
                view: prep.view,
                profile: &profile,
                seed: 1,
                quality: pipeline::Quality::Preview,
                tone: None,
            };
            let t = Instant::now();
            let out = pipeline::render(&input, adj, lut.as_deref());
            pipe.push(ms(t));
            let t = Instant::now();
            let jpeg = raw::turbo::encode_rgb_444(&out.rgb, out.width, out.height, 90).unwrap();
            enc.push(ms(t));
            drop(jpeg);
        }
        drop(img);

        // Region (1:1 detail) renders, panning.
        for k in 0..6 {
            let r = NormRect { x: 0.1 * k as f32, y: 0.3, width: 0.3, height: 0.3 };
            let o = RenderOptions { max_edge: 4096, slot: RenderSlot::Detail, region: Some(r) };
            let t = Instant::now();
            cache.render(cache.ticket(src.id, RenderSlot::Detail), &src, &vars[5].1, &o, &luts).unwrap().unwrap();
            region.push(ms(t));
        }

        // Slider-drag drafts (1024 px) of the user-style edit.
        let dopts = RenderOptions { max_edge: 1024, slot: RenderSlot::Main, region: None };
        let us = user_style();
        for _ in 0..5 {
            let t = Instant::now();
            cache.render(cache.ticket(src.id, RenderSlot::Main), &src, &us, &dopts, &luts).unwrap().unwrap();
            draft.push(ms(t));
        }

        let mut pi = per_image.clone();
        println!(
            "{stem}: orient {:?} src {}x{} full {}x{} as-shot {:?} | decode {d:.0} ms | first 2048 {:.1} ms ({}x{}) | warm p50 {:.1} p95 {:.1} ms",
            orientation,
            info.source_width,
            info.source_height,
            info.full_width,
            info.full_height,
            info.as_shot.map(|w| (w.temperature_k.round(), w.tint.round())),
            prepare.last().unwrap(),
            first.width,
            first.height,
            pct(&mut pi.clone(), 0.5),
            pct(&mut pi, 0.95),
        );
        if let Some(dir) = &check {
            if let Ok(raw::Preview::Embedded) = &ex.preview {
                std::fs::write(dir.join(format!("{stem}_camera.jpg")), &buf).unwrap();
            }
        }
    }

    println!("\n== summary ({} images, {} warm renders) ==", picked.len(), warm.len());
    println!(
        "decode (cold, half-size)  p50 {:.0} ms  p95 {:.0} ms",
        pct(&mut decode.clone(), 0.5),
        pct(&mut decode, 0.95)
    );
    println!(
        "first render 2048 (prep)  p50 {:.1} ms  p95 {:.1} ms",
        pct(&mut prepare.clone(), 0.5),
        pct(&mut prepare, 0.95)
    );
    println!(
        "warm render 2048 (e2e)    p50 {:.1} ms  p95 {:.1} ms  max {:.1} ms",
        pct(&mut warm.clone(), 0.5),
        pct(&mut warm.clone(), 0.95),
        pct(&mut warm, 1.0)
    );
    println!(
        "user-style edit 2048      p50 {:.1} ms  p95 {:.1} ms  ({} renders: tone+HSL+curves+grading+calibration+NR+sharpening)",
        pct(&mut user.clone(), 0.5),
        pct(&mut user.clone(), 0.95),
        user.len()
    );
    println!(
        "user-style draft 1024     p50 {:.1} ms  p95 {:.1} ms",
        pct(&mut draft.clone(), 0.5),
        pct(&mut draft, 0.95)
    );
    println!("  pipeline only           p50 {:.1} ms  p95 {:.1} ms", pct(&mut pipe.clone(), 0.5), pct(&mut pipe, 0.95));
    println!("  JPEG encode (q90 4:4:4) p50 {:.1} ms  p95 {:.1} ms", pct(&mut enc.clone(), 0.5), pct(&mut enc, 0.95));
    println!(
        "region 1:1 (new crop)     p50 {:.1} ms  p95 {:.1} ms",
        pct(&mut region.clone(), 0.5),
        pct(&mut region, 0.95)
    );
    println!("cache: {} images, {} MB", cache.cached_count(), cache.cached_bytes() >> 20);
    let _ = std::fs::remove_dir_all(&tmp);
}

fn tempfile_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sieve-render-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
