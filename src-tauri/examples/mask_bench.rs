//! Masked render latency (Phase 7c): imports copies of masked frames (RAW + sidecar) into a
//! scratch catalog, reads their sidecars (masks + Lightroom mattes into the matte cache) and
//! times warm `DevelopCache::render` calls while a local slider is dragged, with and without
//! the mask.
//!
//! ```text
//! cargo run --release --example mask_bench -- --src DIR_WITH_COPIES --work DIR [--runs N]
//! ```
//! `--src` must hold *copies* (sidecars are read, never written, but the catalog is scratch).

use std::path::PathBuf;

use sieve_lib::db::{self, repo};
use sieve_lib::develop::masks::{MaskCache, MaskCacheConfig};
use sieve_lib::develop::{DevelopCache, DevelopConfig, SourceImage};
use sieve_lib::ipc::types::{ImportOptions, ParametricAdjustments, RenderOptions, RenderSlot};
use sieve_lib::lut::LutLibrary;
use sieve_lib::xmp::{XmpSync, XmpSyncConfig};

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let src_dir = PathBuf::from(arg("--src").expect("--src"));
    let work = PathBuf::from(arg("--work").expect("--work"));
    let runs: usize = arg("--runs").and_then(|v| v.parse().ok()).unwrap_or(30);
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let catalog = work.join("bench.sqlite");
    let ids: Vec<i64> = {
        let mut conn = db::open(&catalog).unwrap();
        let opts = ImportOptions { recursive: false, include_non_raw: false, pair_jpeg_with_raw: true };
        repo::import_folder(&mut conn, &src_dir, &opts).unwrap();
        let mut st = conn.prepare("SELECT id FROM images ORDER BY id").unwrap();
        let ids: Vec<i64> = st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        ids
    };
    let cache = MaskCache::new(MaskCacheConfig { catalog_path: catalog.clone(), cache_dir: work.join("cache") });
    let xmp = XmpSync::new(XmpSyncConfig { catalog_path: catalog.clone() }).with_mask_cache(cache.clone());
    let report = xmp.read_images(&ids).unwrap();
    println!("sidecars read: {} ({} failed)", report.succeeded, report.failed.len());
    let dev = DevelopCache::new(DevelopConfig { cache_bytes: 1 << 30, mask_cache: Some(cache) });
    let luts = LutLibrary::new(work.join("luts"));
    let conn = db::open(&catalog).unwrap();
    let (mut all_mask, mut all_plain, mut all_draft) = (Vec::new(), Vec::new(), Vec::new());
    for &id in &ids {
        let e = repo::get_image(&conn, id).unwrap();
        let src = SourceImage { id, path: PathBuf::from(&e.path), orientation: e.orientation };
        let adj = repo::get_adjustments(&conn, id).unwrap();
        assert!(!adj.masks.is_empty(), "{}: no masks imported", e.file_name);
        let info = dev.info(&src).unwrap();
        let render = |a: &ParametricAdjustments, edge: u32| {
            let t = dev.ticket(id, RenderSlot::Main);
            let o = RenderOptions { max_edge: edge, slot: RenderSlot::Main, region: None };
            dev.render(t, &src, a, &o, &luts).unwrap().unwrap().render_ms as f64
        };
        let cold = render(&adj, 2048);
        let first_masked = {
            // New weights (first render of this geometry) after the decode is warm.
            let mut a = adj.clone();
            a.crop.enabled = false;
            render(&a, 2047)
        };
        let (mut m, mut p, mut d) = (Vec::new(), Vec::new(), Vec::new());
        for k in 0..runs {
            // Dragging the mask's Clarity slider.
            let mut a = adj.clone();
            a.masks[0].adjustments.clarity = -30.0 + k as f32;
            m.push(render(&a, 2048));
            d.push(render(&a, 1024));
            let mut u = a.clone();
            u.masks.clear();
            p.push(render(&u, 2048));
        }
        println!(
            "{}: warnings {:?} | cold {cold:.0} ms, first masked (new geometry) {first_masked:.0} ms | 2048 masked p50 {:.0} p95 {:.0} ms, unmasked p50 {:.0} p95 {:.0} ms | draft 1024 masked p50 {:.0} p95 {:.0} ms",
            e.file_name,
            info.warnings.iter().map(|w| w.code.as_str()).collect::<Vec<_>>(),
            pct(&mut m.clone(), 0.5),
            pct(&mut m.clone(), 0.95),
            pct(&mut p.clone(), 0.5),
            pct(&mut p.clone(), 0.95),
            pct(&mut d.clone(), 0.5),
            pct(&mut d.clone(), 0.95),
        );
        all_mask.extend(m);
        all_plain.extend(p);
        all_draft.extend(d);
    }
    println!(
        "ALL: 2048 masked p50 {:.0} p95 {:.0} ms | unmasked p50 {:.0} p95 {:.0} ms | draft masked p50 {:.0} p95 {:.0} ms",
        pct(&mut all_mask.clone(), 0.5),
        pct(&mut all_mask, 0.95),
        pct(&mut all_plain.clone(), 0.5),
        pct(&mut all_plain, 0.95),
        pct(&mut all_draft.clone(), 0.5),
        pct(&mut all_draft, 0.95)
    );
}
