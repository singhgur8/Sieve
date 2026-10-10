//! Transform / Upright render check over real RAWs + their Lightroom sidecars (read-only).
//!
//! ```text
//! cargo run --release --example transform_check -- <out_dir> <raw>...
//! ```
//! For each RAW: reads the sidecar's develop settings, renders 2048 px previews with the
//! sidecar's Transform and without it (JPEGs into `out_dir`), reports the Upright rotation and
//! the constrained crop, and times warm 2048 renders (p50 / p95) with and without the warp, plus
//! renders that re-prepare (a Transform slider moving).

use std::path::PathBuf;
use std::time::Instant;

use sieve_lib::develop::{transform, DevelopCache, DevelopConfig, SourceImage};
use sieve_lib::ipc::types::{ParametricAdjustments, RenderOptions, RenderSlot, TransformSettings};
use sieve_lib::lut::LutLibrary;
use sieve_lib::{raw, xmp};

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(&args[0]);
    std::fs::create_dir_all(&out).unwrap();
    let luts = LutLibrary::new(out.join("luts"));
    let cache = DevelopCache::new(DevelopConfig { cache_bytes: 2048 << 20, mask_cache: None });
    let opts = RenderOptions { max_edge: 2048, slot: RenderSlot::Main, region: None };
    let (mut with, mut without, mut reprep) = (Vec::new(), Vec::new(), Vec::new());
    for (i, p) in args[1..].iter().enumerate() {
        let path = PathBuf::from(p);
        let format = raw::format_from_extension(&path).unwrap();
        let mut buf = Vec::new();
        let ex = raw::extract(&path, format, &mut buf).expect("extract");
        let src =
            SourceImage { id: i as i64 + 1, path: path.clone(), orientation: ex.meta.orientation.map(|o| o as u8) };
        let text = std::fs::read_to_string(xmp::resolve_sidecar(&path)).expect("sidecar");
        let adj = xmp::packet::parse_for(&text, format).unwrap().develop.expect("develop settings");
        let t = &adj.transform;
        let info = cache.info(&src).unwrap();
        let (sw, sh) = if src.orientation.unwrap_or(1) >= 5 {
            (info.full_height, info.full_width)
        } else {
            (info.full_width, info.full_height)
        };
        let geo = transform::Geometry::of(&adj, sw, sh);
        println!(
            "{}: upright {:?} rotation {:.2} deg (solution {}), constrain {}, crop {:?} -> effective {:?}",
            path.file_name().unwrap().to_string_lossy(),
            t.upright,
            t.solution.as_ref().map_or(0.0, |s| s.rotation_deg),
            t.solution.is_some(),
            t.constrain_crop,
            adj.crop,
            geo.crop
        );
        let plain = ParametricAdjustments { transform: TransformSettings::default(), ..adj.clone() };
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        for (name, a) in [("lr_transform", &adj), ("no_transform", &plain)] {
            let r = cache.render(cache.ticket(src.id, RenderSlot::Main), &src, a, &opts, &luts).unwrap().unwrap();
            let bytes = cache.encoded(src.id, RenderSlot::Main, r.seq).unwrap();
            std::fs::write(out.join(format!("{stem}_{name}.jpg")), bytes.as_slice()).unwrap();
            println!("  {name}: {}x{}", r.width, r.height);
        }
        // Warm renders: exposure varies (prepared input cached).
        for round in 0..8 {
            for (a, sink) in [(&adj, &mut with), (&plain, &mut without)] {
                let mut v = a.clone();
                v.exposure += 0.01 * round as f32;
                let t0 = Instant::now();
                cache.render(cache.ticket(src.id, RenderSlot::Main), &src, &v, &opts, &luts).unwrap().unwrap();
                sink.push(t0.elapsed().as_secs_f64() * 1000.0);
            }
        }
        // A Transform slider moving: every render re-prepares with a new warp.
        for k in 0..6 {
            let mut v = adj.clone();
            v.transform.vertical = -5.0 - k as f32;
            let t0 = Instant::now();
            cache.render(cache.ticket(src.id, RenderSlot::Main), &src, &v, &opts, &luts).unwrap().unwrap();
            reprep.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
    }
    println!(
        "warm 2048: with transform p50 {:.1} ms p95 {:.1} | without p50 {:.1} ms p95 {:.1} | transform slider (re-prepare) p50 {:.1} ms p95 {:.1}",
        pct(&mut with, 0.5),
        pct(&mut with, 0.95),
        pct(&mut without, 0.5),
        pct(&mut without, 0.95),
        pct(&mut reprep, 0.5),
        pct(&mut reprep, 0.95)
    );
}
