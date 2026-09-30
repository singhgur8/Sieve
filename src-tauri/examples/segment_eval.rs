//! AI-mask evaluation (Phase 7c prep): runs Select Subject, Select Sky and People (+ parts)
//! from `ml::segment` over sample previews and writes overlay visualizations.
//!
//! ```text
//! scripts/fetch-models.sh
//! cargo run --release --example segment_eval -- [options]
//!   --src DIR     input JPEGs (default test-data/segment-src; 2048 px previews extracted from
//!                 the READ-ONLY sample shoots with exiftool)
//!   --out DIR     overlays (default test-data/segment-check)
//!   --cpu         CPU execution provider only (default: CoreML where the model compiles)
//!   --only LIST   comma list of subject,sky,people (default all)
//!   --no-images   timings only
//! ```
//! Outputs per image: `<name>_subject.jpg` (subject over magenta), `<name>_sky.jpg` (sky over
//! magenta), `<name>_people.jpg` (one colour per instance), `<name>_parts.jpg` (hair yellow,
//! face skin cyan, body skin salmon, clothes blue, sclera white, iris green, brows magenta,
//! lips red, teeth orange). Prints per-step timings and medians (first call = model load).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use sieve_lib::ml::segment::{Mask, RgbImage, SegmentConfig, Segmenter};
use sieve_lib::raw::turbo;

const ROOT: &str = "/Users/gurjotsingh/Documents/GitHub/Sieve/test-data";

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn composite_over_magenta(rgb: &[u8], w: usize, h: usize, m: &Mask) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let a = m.at(x, y);
            let p = (y * w + x) * 3;
            let bg = [255.0, 0.0, 255.0];
            for c in 0..3 {
                out[p + c] = (rgb[p + c] as f32 * a + bg[c] * (1.0 - a)) as u8;
            }
        }
    }
    out
}

/// Dimmed image with coloured masks blended on top (later masks win where they overlap).
fn overlay(rgb: &[u8], w: usize, layers: &[(&Mask, [f32; 3])], opacity: f32) -> Vec<u8> {
    let mut out: Vec<f32> = rgb.iter().map(|&v| v as f32 * 0.45).collect();
    for (m, col) in layers {
        for y in m.y0..m.y0 + m.height {
            for x in m.x0..m.x0 + m.width {
                let a = m.at(x, y) * opacity;
                if a <= 0.0 {
                    continue;
                }
                let p = (y * w + x) * 3;
                for c in 0..3 {
                    // Keep some image detail under the colour so edges are judgeable.
                    let base = rgb[p + c] as f32 * 0.35 + col[c] * 0.65;
                    out[p + c] = out[p + c] * (1.0 - a) + base * a;
                }
            }
        }
    }
    out.iter().map(|&v| v.clamp(0.0, 255.0) as u8).collect()
}

const PALETTE: [[f32; 3]; 8] = [
    [255.0, 80.0, 80.0],
    [80.0, 160.0, 255.0],
    [255.0, 220.0, 60.0],
    [80.0, 230.0, 120.0],
    [230.0, 120.0, 255.0],
    [255.0, 150.0, 40.0],
    [60.0, 230.0, 230.0],
    [200.0, 200.0, 200.0],
];

fn save(out: &Path, name: &str, suffix: &str, px: &[u8], w: usize, h: usize) {
    let jpg = turbo::encode_rgb(px, w as u32, h as u32, 88).expect("encode");
    std::fs::write(out.join(format!("{name}_{suffix}.jpg")), jpg).expect("write overlay");
}

fn median(v: &[f64]) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    s[s.len() / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = PathBuf::from(arg(&args, "--src").unwrap_or_else(|| format!("{ROOT}/segment-src")));
    let out = PathBuf::from(arg(&args, "--out").unwrap_or_else(|| format!("{ROOT}/segment-check")));
    let cpu = args.iter().any(|a| a == "--cpu");
    let images = !args.iter().any(|a| a == "--no-images");
    let only = arg(&args, "--only").unwrap_or_else(|| "subject,sky,people".into());
    let want = |k: &str| only.split(',').any(|o| o == k);
    std::fs::create_dir_all(&out).expect("create out dir");
    let models_dir = std::env::var("SIEVE_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models"));
    if cpu {
        // Also moves the SCRFD face detector (`ml::models`) to the CPU EP.
        std::env::set_var("SIEVE_ML_CPU", "1");
    }
    let mut cfg = SegmentConfig::new(models_dir);
    cfg.coreml = !cpu;
    cfg.coreml_cache = Some(PathBuf::from(format!("{ROOT}/coreml-cache")));
    let mut seg = Segmenter::new(cfg);

    let mut files: Vec<PathBuf> = std::fs::read_dir(&src)
        .expect("read src dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("jpg")))
        .collect();
    files.sort();
    println!("{} images, EP: {}", files.len(), if cpu { "CPU" } else { "CoreML where supported" });

    // step -> per-image ms (first image separately: includes model load / CoreML compile).
    let mut steps: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut first: BTreeMap<String, f64> = BTreeMap::new();
    let mut record = |i: usize, key: String, v: f64| {
        if i == 0 {
            first.insert(key, v);
        } else {
            steps.entry(key).or_default().push(v);
        }
    };

    for (i, path) in files.iter().enumerate() {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let bytes = std::fs::read(path).expect("read jpeg");
        let d = turbo::decode_rgb(&bytes, u32::MAX, 64_000_000).expect("decode");
        let (w, h) = (d.width as usize, d.height as usize);
        let img = RgbImage { data: &d.pixels, width: w, height: h };
        let mut line = format!("{name} {w}x{h}:");

        if want("subject") {
            let t = Instant::now();
            let m = seg.subject(img).expect("subject");
            let total = t.elapsed().as_secs_f64() * 1000.0;
            record(i, "subject total".into(), total);
            for (k, v) in &seg.timings {
                record(i, format!("  {k}"), *v);
            }
            line += &format!(" subject {total:.0} ms (area {:.0}%)", 100.0 * m.area() / (w * h) as f32);
            if images {
                save(&out, &name, "subject", &composite_over_magenta(&d.pixels, w, h, &m), w, h);
            }
        }
        if want("sky") {
            let t = Instant::now();
            let m = seg.sky(img).expect("sky");
            let total = t.elapsed().as_secs_f64() * 1000.0;
            record(i, "sky total".into(), total);
            for (k, v) in &seg.timings {
                record(i, format!("  {k}"), *v);
            }
            line += &format!(" | sky {total:.0} ms (area {:.0}%)", 100.0 * m.area() / (w * h) as f32);
            if images {
                save(&out, &name, "sky", &composite_over_magenta(&d.pixels, w, h, &m), w, h);
            }
        }
        if want("people") {
            let t = Instant::now();
            let people = seg.people(img, true).expect("people");
            let total = t.elapsed().as_secs_f64() * 1000.0;
            record(i, "people total".into(), total);
            for (k, v) in &seg.timings {
                record(i, format!("  {k}"), *v);
            }
            let seeded = people.iter().filter(|p| p.from_face).count();
            let faces = people.iter().filter(|p| p.face.is_some()).count();
            line +=
                &format!(" | people {total:.0} ms ({} persons, {faces} with face, {seeded} face-seeded)", people.len());
            if images {
                let layers: Vec<(&Mask, [f32; 3])> =
                    people.iter().enumerate().map(|(k, p)| (&p.mask, PALETTE[k % PALETTE.len()])).collect();
                save(&out, &name, "people", &overlay(&d.pixels, w, &layers, 0.75), w, h);
                let mut parts: Vec<(&Mask, [f32; 3])> = Vec::new();
                for p in &people {
                    if let Some(pp) = &p.parts {
                        parts.push((&pp.clothes, [60.0, 60.0, 255.0]));
                        parts.push((&pp.body_skin, [255.0, 130.0, 110.0]));
                        parts.push((&pp.face_skin, [0.0, 220.0, 255.0]));
                        parts.push((&pp.hair, [255.0, 215.0, 0.0]));
                        parts.push((&pp.brows, [255.0, 0.0, 255.0]));
                        parts.push((&pp.sclera, [255.0, 255.0, 255.0]));
                        parts.push((&pp.iris, [0.0, 255.0, 0.0]));
                        parts.push((&pp.lips, [255.0, 0.0, 0.0]));
                        parts.push((&pp.teeth, [255.0, 140.0, 0.0]));
                    }
                }
                save(&out, &name, "parts", &overlay(&d.pixels, w, &parts, 0.8), w, h);
            }
        }
        println!("{line}");
    }
    println!("\nproviders: {:?}", seg.providers());
    println!("\nfirst image (includes model load / CoreML compile):");
    for (k, v) in &first {
        println!("  {k:<22} {v:>8.1} ms");
    }
    println!("\nmedian over the remaining {} images:", files.len().saturating_sub(1));
    for (k, v) in &steps {
        println!("  {k:<22} {:>8.1} ms", median(v));
    }
}
