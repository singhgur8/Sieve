//! Edited-preview cache bench (IPC v19.1) on one RAW.
//!
//! ```text
//! cargo run --release --example edited_bench -- path/to/one.ARW [repeats]
//! ```
//! The RAW is copied into a temp folder with a scratch catalog + cache (nothing else is read or written).
//! Prints
//! 1. time to first edited pixels when switching to an edited photo:
//!    - miss (no edited preview; what Develop did before: cold decode + 2048 px render + JPEG decode),
//!    - hit (sieve:// edited preview read + JPEG decode, what is painted first now);
//! 2. background regeneration after a commit (event latency, minus the 350 ms save debounce): cold source
//!    (detached decode + render) vs reuse of a settled Develop render;
//! 3. file sizes.

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sieve_lib::develop::edited::EditedConfig;
use sieve_lib::develop::{handle_protocol, DevelopCache, DevelopConfig, SourceImage};
use sieve_lib::ipc::types::{ImportOptions, ParametricAdjustments, RenderOptions, RenderSlot};
use sieve_lib::lut::LutLibrary;
use tauri::http;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn pct(v: &[f64], p: f64) -> f64 {
    let mut v = v.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

fn get(cache: &DevelopCache, url: &str) -> Vec<u8> {
    let path = url.split("localhost").nth(1).unwrap();
    let req = http::Request::builder().uri(format!("sieve://localhost{path}")).body(Vec::new()).unwrap();
    let resp = handle_protocol(cache, &req);
    assert_eq!(resp.status(), http::StatusCode::OK, "{url}");
    resp.into_body()
}

fn decode(jpeg: &[u8]) -> (u32, u32) {
    let d = sieve_lib::raw::turbo::decode_rgb(jpeg, 4096, u64::MAX).unwrap();
    (d.width, d.height)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let raw = PathBuf::from(args.next().expect("usage: edited_bench <raw> [repeats]"));
    let repeats: usize = args.next().and_then(|v| v.parse().ok()).unwrap_or(5);
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("shoot");
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join(raw.file_name().unwrap());
    std::fs::copy(&raw, &path).unwrap();
    let catalog = tmp.path().join("catalog.sqlite");
    let mut conn = sieve_lib::db::open(&catalog).unwrap();
    let opts = ImportOptions { recursive: false, include_non_raw: false, pair_jpeg_with_raw: false };
    sieve_lib::db::repo::import_folder(&mut conn, &folder, &opts).unwrap();
    conn.execute("UPDATE thumbnails SET status = 'ready', path = '/unused.jpg', width = 1, height = 1", []).unwrap();
    let id: i64 = conn.query_row("SELECT id FROM images", [], |r| r.get(0)).unwrap();
    let src = SourceImage { id, path: path.clone(), orientation: None };
    let luts = LutLibrary::new(tmp.path().join("luts"));
    let base = sieve_lib::db::repo::get_adjustments(&conn, id).unwrap();
    let edit = |k: usize| ParametricAdjustments { exposure: 0.4 + 0.01 * k as f32, contrast: 15.0, ..base.clone() };
    let main = RenderOptions { max_edge: 2048, slot: RenderSlot::Main, region: None };

    // 1a. Miss: a fresh cache each time (cold source), render 2048 + decode the JPEG.
    let mut miss = Vec::new();
    for k in 0..repeats {
        let cache = DevelopCache::new(DevelopConfig { cache_bytes: 1 << 30, mask_cache: None });
        let t = Instant::now();
        let r = cache.render(cache.ticket(id, RenderSlot::Main), &src, &edit(k), &main, &luts).unwrap().unwrap();
        let jpeg = cache.encoded(id, RenderSlot::Main, r.seq).unwrap();
        decode(&jpeg);
        miss.push(ms(t));
    }

    // The app's cache, with the background worker.
    let cache = DevelopCache::new(DevelopConfig { cache_bytes: 1 << 30, mask_cache: None });
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let ed = cache.enable_edited_previews(
        EditedConfig { dir: tmp.path().join("cache/edited"), catalog_path: catalog.clone(), max_bytes: 1 << 30 },
        luts.clone(),
        Arc::new(move |ev| {
            let _ = tx.lock().unwrap().send(ev);
        }),
    );
    let wait = |label: &str| {
        let t = Instant::now();
        let ev = rx.recv_timeout(Duration::from_secs(60)).unwrap_or_else(|_| panic!("{label}: no event"));
        (ev, t)
    };

    // 2a. Regeneration from a cold source (nothing decoded in this cache: detached decode + render).
    let mut cold = Vec::new();
    for k in 0..repeats {
        let t = Instant::now();
        sieve_lib::develop::history::commit(&mut conn, id, &edit(100 + k), &format!("Cold {k}")).unwrap();
        let (ev, _) = wait("cold");
        assert!(ev.preview.is_some());
        cold.push(ms(t) - 350.0);
    }
    // 2b. Reuse of a settled Develop render of the same settings.
    let mut settled = Vec::new();
    for k in 0..repeats {
        let a = edit(200 + k);
        cache.render(cache.ticket(id, RenderSlot::Main), &src, &a, &main, &luts).unwrap().unwrap();
        std::thread::sleep(Duration::from_millis(300)); // the worker waits for the UI to be idle (250 ms)
        let t = Instant::now();
        sieve_lib::develop::history::commit(&mut conn, id, &a, &format!("Settled {k}")).unwrap();
        let (ev, _) = wait("settled");
        assert!(ev.preview.is_some());
        settled.push(ms(t) - 350.0);
    }

    // 1b. Hit: the cached edited preview (read through the sieve:// handler) + JPEG decode.
    let (_, urls) = ed.current(id).unwrap();
    let mut hit = Vec::new();
    let mut size = (0, 0);
    let mut dims = (0, 0);
    for _ in 0..repeats.max(20) {
        let t = Instant::now();
        let bytes = get(&cache, &urls.preview_url);
        dims = decode(&bytes);
        hit.push(ms(t));
        size.0 = bytes.len();
    }
    size.1 = get(&cache, &urls.thumb_url).len();

    println!("RAW {} ({} repeats)", raw.display(), repeats);
    println!(
        "switch, time to first edited pixels: miss p50 {:.0} ms (min {:.0}, max {:.0}); hit p50 {:.1} ms (max {:.1})",
        pct(&miss, 0.5),
        pct(&miss, 0.0),
        pct(&miss, 1.0),
        pct(&hit, 0.5),
        pct(&hit, 1.0)
    );
    println!(
        "regeneration after commit (minus 350 ms debounce): cold source p50 {:.0} ms (max {:.0}); settled reuse p50 {:.0} ms (max {:.0})",
        pct(&cold, 0.5),
        pct(&cold, 1.0),
        pct(&settled, 0.5),
        pct(&settled, 1.0)
    );
    println!(
        "files: preview {}x{} {} KB, thumb {} KB; on disk {} KB",
        dims.0,
        dims.1,
        size.0 / 1024,
        size.1 / 1024,
        ed.bytes() / 1024
    );
}
