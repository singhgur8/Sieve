//! Slider-drag render bench (Phase 8d): draft render time and cancellation latency on one RAW.
//!
//! ```text
//! cargo run --release --example drag_bench -- path/to/one.ARW [frames]
//! ```
//! Reads the file only (it is cloned into a temp folder). Prints
//! 1. draft (1024 px) and full (2048 px) render time p50 / p95 over `frames` exposure-sweep frames, and the JPEG size;
//! 2. cancellation latency: a render is started, a newer ticket for the same (image, slot) is issued 6 ms later;
//!    reported are the time until the superseded render returns `None` and until the newer render's result is back.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sieve_lib::develop::{DevelopCache, DevelopConfig, SourceImage};
use sieve_lib::ipc::types::{ParametricAdjustments, RenderOptions, RenderSlot};
use sieve_lib::lut::LutLibrary;

fn pct(v: &[f64], p: f64) -> f64 {
    let mut v = v.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    let mut args = std::env::args().skip(1);
    let raw = PathBuf::from(args.next().expect("usage: drag_bench <raw> [frames]"));
    let frames: usize = args.next().and_then(|v| v.parse().ok()).unwrap_or(40);
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(raw.file_name().unwrap());
    std::fs::copy(&raw, &path).unwrap();
    let cache = Arc::new(DevelopCache::new(DevelopConfig::default()));
    let luts = Arc::new(LutLibrary::new(tmp.path().join("luts")));
    let src = SourceImage { id: 1, path, orientation: Some(1) };
    let opts = |edge| RenderOptions { max_edge: edge, slot: RenderSlot::Main, region: None };
    let adj = |f: usize| ParametricAdjustments { exposure: -1.0 + 2.0 * (f % 50) as f32 / 50.0, ..Default::default() };

    let t = Instant::now();
    cache.render(cache.ticket(1, RenderSlot::Main), &src, &adj(0), &opts(1024), &luts).unwrap();
    println!("cold decode + first render {:.0} ms", ms(t));

    for (label, edge) in [("draft 1024", 1024u32), ("full 2048", 2048)] {
        let mut v = Vec::new();
        let mut bytes = 0;
        for f in 0..frames {
            let t = Instant::now();
            let out =
                cache.render(cache.ticket(1, RenderSlot::Main), &src, &adj(f), &opts(edge), &luts).unwrap().unwrap();
            v.push(ms(t));
            bytes = cache.encoded(1, RenderSlot::Main, out.seq).unwrap().len();
        }
        println!(
            "{label}: p50 {:.1}  p95 {:.1}  max {:.1} ms  (JPEG {} KB, {} frames)",
            pct(&v, 0.5),
            pct(&v, 0.95),
            pct(&v, 1.0),
            bytes / 1024,
            frames
        );
    }

    for (label, edge) in [("full 2048", 2048u32), ("draft 1024", 1024)] {
        let (mut stale, mut fresh) = (Vec::new(), Vec::new());
        for f in 0..20 {
            let a = cache.ticket(1, RenderSlot::Main);
            let (c, l, s) = (cache.clone(), luts.clone(), src.clone());
            let a_adj = adj(f);
            let t0 = Instant::now();
            let h = std::thread::spawn(move || {
                let r = c
                    .render(a, &s, &a_adj, &RenderOptions { max_edge: edge, slot: RenderSlot::Main, region: None }, &l)
                    .unwrap();
                (r.is_none(), t0.elapsed())
            });
            std::thread::sleep(Duration::from_millis(6));
            let t_b = Instant::now();
            let b = cache.ticket(1, RenderSlot::Main);
            let out = cache.render(b, &src, &adj(f + 25), &opts(edge), &luts).unwrap();
            assert!(out.is_some());
            fresh.push(ms(t_b));
            let (was_none, a_total) = h.join().unwrap();
            if was_none {
                // Time the superseded render ran after the newer ticket existed.
                stale.push((a_total.as_secs_f64() * 1000.0 - 6.0).max(0.0));
            }
        }
        if stale.is_empty() {
            println!("cancel {label}: superseded render never aborted");
        } else {
            println!(
                "cancel {label}: superseded render ran {:.1} ms (p50) / {:.1} (p95) after the newer ticket; newer result back after p50 {:.1} / p95 {:.1} ms ({} of 20 aborted)",
                pct(&stale, 0.5),
                pct(&stale, 0.95),
                pct(&fresh, 0.5),
                pct(&fresh, 0.95),
                stale.len()
            );
        }
    }
}
