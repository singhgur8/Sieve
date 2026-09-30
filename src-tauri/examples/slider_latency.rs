//! Slider path latency: IPC -> render -> protocol, end to end without the webview.
//!
//! ```text
//! cargo run --release --example slider_latency -- [raw_file] [frames]
//! ```
//! `raw_file` defaults to the first ARW in `$SIEVE_SAMPLES` / "/Users/gurjotsingh/Pictures/test RAWS"
//! (only read; a clone goes into a temp folder that is imported into a scratch catalog).
//! Simulates `frames` (default 60) consecutive slider frames of an exposure drag, the way the
//! `render_preview` command and the `sieve://` handler run them:
//! 1. IPC: the invoke payload (adjustments + options) is deserialized (serde_json);
//! 2. source: the image's path/orientation is resolved: **legacy** = lock the catalog
//!    connection + `repo::get_images` (what every frame did before Phase 8), **cached** =
//!    `DevelopCache::source` (the Phase 8 path; one catalog lookup, then memory);
//! 3. render: ticket + `DevelopCache::render` (draft 1024 px while dragging, as the UI does);
//! 4. protocol: `handle_protocol` serves the JPEG (`sieve://localhost/render/<id>/main?v=<seq>`);
//! 5. the `RenderedPreview` result is serialized back.
//!
//! Each mode runs twice: idle catalog, and with a background writer holding the catalog
//! lock for batches (a 500-image rating write in a loop, like a batch action or XMP
//! refresh on the command connection) to show what the per-frame catalog query cost under
//! contention. Prints p50/p95/max per stage and end to end.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rusqlite::Connection;
use sieve_lib::db::{self, repo};
use sieve_lib::develop::{self, DevelopCache, DevelopConfig, SourceImage};
use sieve_lib::ipc::types::{ImportOptions, ParametricAdjustments, RenderOptions, RenderSlot};
use sieve_lib::lut::LutLibrary;

const DEFAULT_SAMPLES: &str = "/Users/gurjotsingh/Pictures/test RAWS";

fn pct(v: &[f64], p: f64) -> f64 {
    let mut v = v.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

#[derive(Default)]
struct Stages {
    ipc: Vec<f64>,
    source: Vec<f64>,
    render: Vec<f64>,
    protocol: Vec<f64>,
    total: Vec<f64>,
    jpeg_bytes: usize,
}

fn report(name: &str, s: &Stages) {
    let row = |label: &str, v: &[f64]| {
        println!("  {label:<9} p50 {:>7.3}  p95 {:>7.3}  max {:>7.3} ms", pct(v, 0.5), pct(v, 0.95), pct(v, 1.0));
    };
    println!("{name} ({} frames, JPEG {} KB)", s.total.len(), s.jpeg_bytes / 1024);
    row("ipc", &s.ipc);
    row("source", &s.source);
    row("render", &s.render);
    row("protocol", &s.protocol);
    row("total", &s.total);
}

fn first_raw(dir: &std::path::Path) -> Option<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("arw")))
        .collect();
    v.sort();
    v.into_iter().next()
}

#[allow(clippy::too_many_arguments)]
fn run(
    frames: usize,
    cached: bool,
    catalog: &Arc<Mutex<Connection>>,
    cache: &DevelopCache,
    luts: &LutLibrary,
    id: i64,
    draft: u32,
) -> Stages {
    let mut s = Stages::default();
    cache.forget_sources(None);
    for f in 0..frames {
        let t0 = Instant::now();
        // 1. IPC payload in.
        let adj = ParametricAdjustments { exposure: -1.0 + 2.0 * f as f32 / frames as f32, ..Default::default() };
        let payload =
            serde_json::to_string(&(adj, RenderOptions { max_edge: draft, slot: RenderSlot::Main, region: None }))
                .unwrap();
        let t = Instant::now();
        let (adj, options): (ParametricAdjustments, RenderOptions) = serde_json::from_str(&payload).unwrap();
        adj.validate().unwrap();
        options.validate().unwrap();
        s.ipc.push(ms(t));
        // 2. Source resolution.
        let t = Instant::now();
        let ticket = cache.ticket(id, options.slot);
        let src = match cache.source(id).filter(|_| cached) {
            Some(src) => src,
            None => {
                let conn = catalog.lock().unwrap();
                let e = repo::get_images(&conn, &[id]).unwrap().remove(0);
                let src = SourceImage { id: e.id, path: PathBuf::from(e.path), orientation: e.orientation };
                if cached {
                    cache.remember_source(src.clone());
                }
                src
            }
        };
        s.source.push(ms(t));
        // 3. Render.
        let t = Instant::now();
        let out = cache.render(ticket, &src, &adj, &options, luts).unwrap().expect("current ticket");
        s.render.push(ms(t));
        // 4. Protocol fetch + 5. result out.
        let t = Instant::now();
        let url = out.url.replace("sieve://localhost", "");
        let req = tauri::http::Request::builder().uri(format!("sieve://localhost{url}")).body(Vec::new()).unwrap();
        let resp = develop::handle_protocol(cache, &req);
        assert_eq!(resp.status(), 200);
        s.jpeg_bytes = resp.body().len();
        let _json = serde_json::to_string(&out).unwrap();
        s.protocol.push(ms(t));
        s.total.push(ms(t0));
    }
    s
}

fn main() {
    let mut args = std::env::args().skip(1);
    let raw = args.next().map(PathBuf::from).unwrap_or_else(|| {
        let dir = std::env::var_os("SIEVE_SAMPLES").map(PathBuf::from).unwrap_or_else(|| DEFAULT_SAMPLES.into());
        first_raw(&dir).expect("no ARW sample found")
    });
    let frames: usize = args.next().and_then(|v| v.parse().ok()).unwrap_or(60);

    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("shoot");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::copy(&raw, folder.join(raw.file_name().unwrap())).unwrap();
    let catalog_path = tmp.path().join("catalog.sqlite");
    let mut conn = db::open(&catalog_path).unwrap();
    repo::import_folder(&mut conn, &folder, &ImportOptions::raw_only(false)).unwrap();
    // Background noise for the contention runs: 2000 extra rows to write ratings on.
    conn.execute_batch(
        "WITH RECURSIVE s(x) AS (SELECT 2 UNION ALL SELECT x + 1 FROM s WHERE x < 2001)
         INSERT INTO images (id, folder_id, path, file_name, format, camera_make, file_size, file_mtime_ms, imported_at)
         SELECT x, 1, '/nowhere/' || x || '.arw', x || '.arw', 'arw', 'sony', 1, 0, 0 FROM s;",
    )
    .unwrap();
    let id: i64 = conn.query_row("SELECT id FROM images ORDER BY id LIMIT 1", [], |r| r.get(0)).unwrap();
    let catalog = Arc::new(Mutex::new(conn));

    let cache = DevelopCache::new(DevelopConfig::default());
    let luts = LutLibrary::new(tmp.path().join("luts"));
    let src = {
        let c = catalog.lock().unwrap();
        let e = repo::get_image(&c, id).unwrap();
        SourceImage { id, path: PathBuf::from(e.path), orientation: e.orientation }
    };
    // Warm: decode + first render (not part of the slider path).
    let t = Instant::now();
    let warm = RenderOptions { max_edge: 1024, slot: RenderSlot::Main, region: None };
    cache.render(cache.ticket(id, RenderSlot::Main), &src, &ParametricAdjustments::default(), &warm, &luts).unwrap();
    println!("sample {} (cold decode + first render {:.0} ms)", raw.display(), ms(t));

    for contended in [false, true] {
        let stop = Arc::new(AtomicBool::new(false));
        let writer = contended.then(|| {
            let (catalog, stop) = (catalog.clone(), stop.clone());
            std::thread::spawn(move || {
                let ids: Vec<i64> = (2..502).collect();
                let mut n = 0u8;
                let mut held = Vec::new();
                while !stop.load(Ordering::SeqCst) {
                    let t = Instant::now();
                    {
                        let mut c = catalog.lock().unwrap();
                        repo::set_rating(&mut c, &ids, n % 6).unwrap();
                    }
                    held.push(ms(t));
                    n = n.wrapping_add(1);
                    std::thread::sleep(Duration::from_millis(5));
                }
                held
            })
        });
        for (label, cached) in [("legacy (catalog query per frame)", false), ("cached source (Phase 8)", true)] {
            let s = run(frames, cached, &catalog, &cache, &luts, id, 1024);
            report(&format!("{label}{}", if contended { ", catalog busy" } else { "" }), &s);
        }
        if let Some(w) = writer {
            stop.store(true, Ordering::SeqCst);
            let held = w.join().unwrap();
            println!("  (background writer: {} batches, lock held p50 {:.2} ms)", held.len(), pct(&held, 0.5));
        }
    }
    let s = run(frames, true, &catalog, &cache, &luts, id, 2048);
    report("cached source, 2048 px (release render)", &s);

    // Protocol body copy (Tauri's responder takes an owned `Cow<'static, [u8]>`).
    let bytes = cache.encoded(id, RenderSlot::Main, 0).unwrap();
    let t = Instant::now();
    for _ in 0..100 {
        std::hint::black_box(bytes.as_ref().clone());
    }
    println!("protocol body copy of {} KB: {:.1} us", bytes.len() / 1024, ms(t) * 10.0);
}
