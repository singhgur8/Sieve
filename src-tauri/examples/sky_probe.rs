//! Select Sky probe (Phase 8): the sky matte variants on upright images, for visual review.
//!
//! ```text
//! cargo run --release --example sky_probe -- --out DIR [--jpegs DIR] [--raws DIR]
//! ```
//! `--jpegs`: upright previews (e.g. `test-data/segment-src`); `--raws`: RAWs (read only,
//! rendered neutral like the app's model input, then oriented upright). Writes
//! `<name>.photo.jpg` and `<name>.sky.jpg` (final matte, [`SegmentEngine::sky_raw`]) plus
//! `<name>.global.jpg` (the 320x320 whole-image pass alone) and prints sky coverage.

use std::path::PathBuf;
use std::time::Instant;

use sieve_lib::develop::SourceImage;
use sieve_lib::ml::masking::{load_source, orient_pixels};
use sieve_lib::ml::segment::{LowRes, RgbImage, SegmentConfig, SegmentEngine};
use sieve_lib::raw::{self, turbo};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn gray(lr: &LowRes, w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let v = (lr.at(x as f32 + 0.5, y as f32 + 0.5).clamp(0.0, 1.0) * 255.0) as u8;
            out[(y * w + x) * 3..(y * w + x) * 3 + 3].fill(v);
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(arg(&args, "--out").expect("--out"));
    std::fs::create_dir_all(&out).unwrap();
    let models_dir = std::env::var("SIEVE_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models"));
    let mut seg = SegmentEngine::new(SegmentConfig::new(models_dir));
    let mut inputs: Vec<(String, Vec<u8>, usize, usize)> = Vec::new();
    let list = |d: &str, ext: &str| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(d)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)))
            .collect();
        v.sort();
        v
    };
    if let Some(d) = arg(&args, "--jpegs") {
        for p in list(&d, "jpg") {
            let dec = turbo::decode_rgb(&std::fs::read(&p).unwrap(), u32::MAX, 64_000_000).unwrap();
            let name = format!("{}_jpg", p.file_stem().unwrap().to_string_lossy());
            inputs.push((name, dec.pixels, dec.width as usize, dec.height as usize));
        }
    }
    if let Some(d) = arg(&args, "--raws") {
        for p in list(&d, "arw") {
            let f = raw::format_from_extension(&p).unwrap();
            let mut buf = Vec::new();
            let o = raw::extract(&p, f, &mut buf).ok().and_then(|x| x.meta.orientation).unwrap_or(1) as u8;
            let src = SourceImage { id: 1, path: p.clone(), orientation: Some(o) };
            let s = load_source(&src, None).unwrap();
            let (px, w, h) = orient_pixels(&s.rgb, s.width as usize, s.height as usize, 3, o);
            inputs.push((format!("{}_raw", p.file_stem().unwrap().to_string_lossy()), px, w, h));
        }
    }
    for (name, px, w, h) in &inputs {
        let img = RgbImage { data: px, width: *w, height: *h };
        let t = Instant::now();
        let global = seg.sky_network(img).unwrap();
        let tg = t.elapsed().as_millis();
        let t = Instant::now();
        let fin = seg.sky_raw(img).unwrap();
        let tf = t.elapsed().as_millis();
        let cov = |lr: &LowRes| lr.data.iter().sum::<f32>() / lr.data.len() as f32;
        println!("{name} {w}x{h}: global cov {:.3} ({tg} ms) | final cov {:.3} ({tf} ms)", cov(&global), cov(&fin));
        // Panels at <= 800 px long edge.
        let k = 800.0 / (*w.max(h)) as f32;
        let (pw, ph) = ((*w as f32 * k) as usize, (*h as f32 * k) as usize);
        let small = |lr: &LowRes| {
            let scaled = LowRes { roi: [0, 0, pw, ph], width: lr.width, height: lr.height, data: lr.data.clone() };
            gray(&scaled, pw, ph)
        };
        let mut photo = vec![0u8; pw * ph * 3];
        for y in 0..ph {
            for x in 0..pw {
                let (sx, sy) = (((x as f32 + 0.5) / k) as usize, ((y as f32 + 0.5) / k) as usize);
                let s = (sy.min(h - 1) * w + sx.min(w - 1)) * 3;
                photo[(y * pw + x) * 3..(y * pw + x) * 3 + 3].copy_from_slice(&px[s..s + 3]);
            }
        }
        let save = |tag: &str, rgb: &[u8]| {
            let j = turbo::encode_rgb(rgb, pw as u32, ph as u32, 88).unwrap();
            std::fs::write(out.join(format!("{name}.{tag}.jpg")), j).unwrap();
        };
        save("photo", &photo);
        save("global", &small(&global));
        save("sky", &small(&fin));
    }
}
