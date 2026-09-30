//! Scene detection + scene matching acceptance run (Phase 7) over *copies* of sample RAWs.
//!
//! ```text
//! cargo run --release --example scene_eval -- [options]
//!   --raws DIR       folder of RAW copies (default test-data/scene-raws; never ~/Pictures)
//!   --work DIR       catalog + thumbnail cache (default test-data/scene-eval; reused if present)
//!   --sheets DIR     before/after contact sheets (default test-data/scene-check)
//!   --pick A,B,..    scenes to grade, each named by a member file name (default: built-in picks)
//!   --list           print the detected scenes and exit
//!   --calibrate      print d(neutral a,b)/d(mired, tint) and d(logMeanLuma)/d(EV) finite differences
//! ```
//! Per picked scene: 1-2 anchors get a clearly non-neutral grade, `match_images` matches the
//! rest, and every target is re-measured like `get_render_stats(target, preview.full)`:
//! pass = |logMeanLuma - reference| <= TOLERANCE_EV and |neutral.ab - reference| <=
//! TOLERANCE_AB. Prints per-scene pass rates, failures with reasons and ms per target (cold:
//! RAW decode included; warm: sources cached).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use sieve_lib::db::{self, repo};
use sieve_lib::develop::{DevelopCache, DevelopConfig, SourceImage};
use sieve_lib::ingest::{run_until_idle, IngestConfig, IngestSink};
use sieve_lib::ipc::events::{ImportProgress, ThumbnailFailed, ThumbnailReady};
use sieve_lib::ipc::types::{
    ImageStats, ImportOptions, MatchOptions, MatchPreview, ParametricAdjustments, SceneDetectOptions, WhiteBalance,
};
use sieve_lib::lut::LutLibrary;
use sieve_lib::raw::turbo;
use sieve_lib::scene::{self, detect, features, matching, stats, store, MatchImage};

const ROOT: &str = "/Users/gurjotsingh/Documents/GitHub/Sieve/test-data";
/// Default picks (a member file name per scene), chosen from `--list` + the previews:
/// bright ceremony/outdoor, warm reception, dark dance floor, portrait session, ...
const DEFAULT_PICKS: &str =
    "MON04770.ARW,MON04795.ARW,MON05024.ARW,MON05107.ARW,MON05241.ARW,MON05395.ARW,MON05471.ARW";

struct Quiet;
impl IngestSink for Quiet {
    fn ready(&self, _: ThumbnailReady) {}
    fn failed(&self, e: ThumbnailFailed) {
        eprintln!("thumbnail failed: image {}: {}", e.image_id, e.reason);
    }
    fn progress(&self, _: ImportProgress) {}
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn ab_dist(a: &ImageStats, b: &ImageStats) -> f32 {
    (a.neutral.a - b.neutral.a).hypot(a.neutral.b - b.neutral.b)
}

/// The anchor look relative to the camera's as-shot white balance: warmer (x1.15 K, about
/// +800 K in daylight) and more magenta (+8 tint), +0.6 EV, contrast +15. A second anchor
/// (two-anchor scenes) gets a slightly different look so blending is exercised.
fn anchor_grade(as_shot: Option<(f32, f32)>, variant: usize) -> ParametricAdjustments {
    let (k, tint) = as_shot.unwrap_or((5500.0, 0.0));
    let (kf, dt, ev, c) = if variant == 0 { (1.15, 8.0, 0.6, 15.0) } else { (1.10, 5.0, 0.4, 10.0) };
    ParametricAdjustments {
        exposure: ev,
        contrast: c,
        white_balance: WhiteBalance::Custom { temperature_k: (k * kf).clamp(2000.0, 50000.0), tint: tint + dt },
        ..Default::default()
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let raws = PathBuf::from(arg(&args, "--raws").unwrap_or_else(|| format!("{ROOT}/scene-raws")));
    let work = PathBuf::from(arg(&args, "--work").unwrap_or_else(|| format!("{ROOT}/scene-eval")));
    let sheets = PathBuf::from(arg(&args, "--sheets").unwrap_or_else(|| format!("{ROOT}/scene-check")));
    if raws.canonicalize()?.starts_with("/Users/gurjotsingh/Pictures") {
        return Err("--raws must be a copy under test-data, not ~/Pictures".into());
    }
    std::fs::create_dir_all(&work)?;
    let config = IngestConfig { catalog_path: work.join("catalog.sqlite"), cache_dir: work.join("cache") };

    // 1. Catalog (import + thumbnails; reused when present).
    let mut conn = db::open(&config.catalog_path)?;
    let have: u32 = conn.query_row("SELECT COUNT(*) FROM images", [], |r| r.get(0))?;
    if have == 0 {
        let t = Instant::now();
        let s = repo::import_folder(&mut conn, &raws.canonicalize()?, &ImportOptions::raw_only(false))?;
        let running = AtomicBool::new(true);
        let st = run_until_idle(&config, &Quiet, &running)?;
        println!("imported {} ({} thumbnails, {} failed) in {:.1}s", s.added, st.done, st.failed, ms(t) / 1000.0);
    }

    // 2. Scene detection (same steps as the `detect_scenes` command).
    let mut options = SceneDetectOptions { replace_manual: true, ..Default::default() };
    if let Some(v) = arg(&args, "--similarity") {
        options.similarity = v.parse()?;
    }
    if let Some(v) = arg(&args, "--max-gap-ms") {
        options.max_gap_ms = v.parse()?;
    }
    let t = Instant::now();
    let mut frames = store::detection_frames(&conn, None, true)?;
    let computed = features::compute_missing(&mut frames, &|_, _| {});
    let feat_ms = ms(t);
    store::save_features(&mut conn, &computed)?;
    let t = Instant::now();
    let groups = detect::group(&frames, &options);
    let group_ms = ms(t);
    let scenes = store::replace_scenes(&mut conn, None, &groups, true)?;
    println!(
        "detected {} scenes over {} frames (features {} new in {feat_ms:.0} ms, grouping {group_ms:.2} ms)",
        scenes.len(),
        frames.len(),
        computed.len()
    );
    let name_of = |id: i64| frames.iter().find(|f| f.id == id).map(|f| f.file_name.clone()).unwrap_or_default();
    if let Some(dir) = arg(&args, "--detect-sheets") {
        detect_sheets(&frames, &groups, Path::new(&dir))?;
    }
    if args.iter().any(|a| a == "--frames") {
        for (i, f) in frames.iter().enumerate() {
            let prev = i.checked_sub(1).map(|p| &frames[p]);
            let sim = match (prev.and_then(|p| p.features.as_ref()), f.features.as_ref()) {
                (Some(a), Some(b)) => features::similarity(a, b),
                _ => f32::NAN,
            };
            let gap = prev.and_then(|p| Some((f.captured_at_ms? - p.captured_at_ms?) as f64 / 1000.0));
            let start = groups.iter().any(|g| g[0] == f.id);
            println!(
                "{}{} gap {:>7.1}s sim(prev) {sim:.3} logmean {:.2}",
                if start { "* " } else { "  " },
                f.file_name,
                gap.unwrap_or(f64::NAN),
                f.features.as_ref().map_or(f32::NAN, |x| x.log_mean_luma)
            );
        }
    }
    if args.iter().any(|a| a == "--list") {
        for s in &scenes {
            let first = frames.iter().find(|f| f.id == s.image_ids[0]);
            let lm = first.and_then(|f| f.features.as_ref()).map_or(f32::NAN, |f| f.log_mean_luma);
            println!(
                "scene {:>3}: {:>3} frames  {} .. {}  (first log-mean {lm:.2})",
                s.id,
                s.image_ids.len(),
                name_of(s.image_ids[0]),
                name_of(*s.image_ids.last().unwrap())
            );
        }
        return Ok(());
    }

    let cache = DevelopCache::new(DevelopConfig { cache_bytes: 6 << 30 });
    let luts = LutLibrary::new(work.join("luts"));

    if args.iter().any(|a| a == "--calibrate") {
        return calibrate(&conn, &cache, &luts, &frames.iter().map(|f| f.id).step_by(40).collect::<Vec<_>>());
    }

    // 3. Match the picked scenes.
    let picks = arg(&args, "--pick").unwrap_or_else(|| DEFAULT_PICKS.to_string());
    let mut totals = (0usize, 0usize);
    let mut sheet_count = 0;
    println!(
        "\n{:<14} {:>3} {:>2} {:>6} {:>9} {:>9} {:>8} {:>8} {:>9}",
        "scene", "tgt", "an", "pass", "cold ms/t", "warm ms/t", "max dEV", "max dab", "sync pass"
    );
    let mut failures: Vec<String> = Vec::new();
    for pick in picks.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let Some(scene) = scenes.iter().find(|s| s.image_ids.iter().any(|&id| name_of(id) == pick)) else {
            println!("{pick}: not found");
            continue;
        };
        let ids = &scene.image_ids;
        if ids.len() < 3 {
            println!("{pick}: scene has only {} frames, skipped", ids.len());
            continue;
        }
        let anchor_idx: Vec<usize> =
            if ids.len() >= 10 { vec![ids.len() / 5, ids.len() * 4 / 5] } else { vec![ids.len() / 3] };
        let mut inputs = store::match_inputs(&conn, ids)?;
        let mut anchors: Vec<MatchImage> = Vec::new();
        for (v, &i) in anchor_idx.iter().enumerate() {
            let info = cache.info(&inputs[i].src)?;
            inputs[i].adjustments = anchor_grade(info.as_shot.map(|w| (w.temperature_k, w.tint)), v);
            anchors.push(inputs[i].clone());
        }
        let targets: Vec<MatchImage> =
            inputs.iter().enumerate().filter(|(i, _)| !anchor_idx.contains(i)).map(|(_, m)| m.clone()).collect();
        let opts = MatchOptions::default();
        let t = Instant::now();
        let previews = matching::match_images(&cache, &luts, &anchors, &targets, &opts, &|_, _| {})?;
        let cold = ms(t) / targets.len() as f64;
        let t = Instant::now();
        let _ = matching::match_images(&cache, &luts, &anchors, &targets, &opts, &|_, _| {})?;
        let warm = ms(t) / targets.len() as f64;

        let mut pass = 0;
        let mut sync_pass = 0;
        let mut sync_stats = Vec::new();
        let (mut max_ev, mut max_ab) = (0.0f32, 0.0f32);
        for (p, tgt) in previews.iter().zip(&targets) {
            // Acceptance check exactly like get_render_stats(target, preview.full).
            let got = stats::render_stats(&cache, &luts, &tgt.src, &p.full, None)?;
            // Plain sync (anchor settings copied, strength 0) for comparison.
            let sync = stats::render_stats(&cache, &luts, &tgt.src, &p.base, None)?;
            if (sync.log_mean_luma - p.reference.log_mean_luma).abs() <= scene::TOLERANCE_EV
                && ab_dist(&sync, &p.reference) <= scene::TOLERANCE_AB
            {
                sync_pass += 1;
            }
            sync_stats.push(sync);
            let dev = (got.log_mean_luma - p.reference.log_mean_luma).abs();
            let dab = ab_dist(&got, &p.reference);
            max_ev = max_ev.max(dev);
            max_ab = max_ab.max(dab);
            if dev <= scene::TOLERANCE_EV && dab <= scene::TOLERANCE_AB {
                pass += 1;
            } else {
                failures.push(format!(
                    "{pick}: {} dEV {dev:.3} dab {dab:.4} (neutral coverage {:.3}, ref {:.3}; clipped hi {:.3}; delta {:+.2} EV {:+.0} K {:+.1} tint) {:?}",
                    name_of(p.target_id),
                    got.neutral.coverage,
                    p.reference.neutral.coverage,
                    got.clipped_highlights,
                    p.delta.exposure,
                    p.delta.temperature_k,
                    p.delta.tint,
                    p.notes
                ));
            }
        }
        totals.0 += pass;
        totals.1 += previews.len();
        println!(
            "{:<14} {:>3} {:>2} {:>5.0}% {:>9.1} {:>9.1} {:>8.3} {:>8.4} {:>8.0}%",
            pick,
            previews.len(),
            anchors.len(),
            100.0 * pass as f64 / previews.len() as f64,
            cold,
            warm,
            max_ev,
            max_ab,
            100.0 * sync_pass as f64 / previews.len() as f64,
        );
        print_details(&previews, &sync_stats, &name_of);
        if sheet_count < 2 || args.iter().any(|a| a == "--all-sheets") {
            std::fs::create_dir_all(&sheets)?;
            let out = sheets.join(format!("scene_{}.jpg", pick.trim_end_matches(".ARW")));
            contact_sheet(&cache, &luts, &anchors, &targets, &previews, &out)?;
            println!("  sheet: {}", out.display());
            sheet_count += 1;
        }
    }
    println!(
        "\noverall: {}/{} targets within tolerance ({:.1}%)",
        totals.0,
        totals.1,
        100.0 * totals.0 as f64 / totals.1.max(1) as f64
    );
    for f in &failures {
        println!("FAIL {f}");
    }
    Ok(())
}

fn print_details(previews: &[MatchPreview], sync: &[ImageStats], name_of: &dyn Fn(i64) -> String) {
    for (p, s) in previews.iter().zip(sync) {
        println!(
            "    {} sync {:+.2} EV / ab {:.3} -> {:+.3} EV / ab {:.4}  delta {:+.2} EV {:+5.0} K {:+5.1} tint  cov {:.2}{}",
            name_of(p.target_id),
            s.log_mean_luma - p.reference.log_mean_luma,
            ab_dist(s, &p.reference),
            p.predicted.log_mean_luma - p.reference.log_mean_luma,
            ab_dist(&p.predicted, &p.reference),
            p.delta.exposure,
            p.delta.temperature_k,
            p.delta.tint,
            p.predicted.neutral.coverage,
            if p.notes.is_empty() { String::new() } else { format!("  {:?}", p.notes) }
        );
    }
}

/// Preview thumbnails of every frame in detection order, 16 per row, 8 rows per sheet; a
/// coloured bar under each tile alternates per scene (a colour change = scene boundary).
fn detect_sheets(
    frames: &[scene::DetectFrame],
    groups: &[Vec<i64>],
    dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    const TILE: usize = 120;
    const BAR: usize = 8;
    const COLS: usize = 16;
    const ROWS: usize = 8;
    std::fs::create_dir_all(dir)?;
    let colours = [[230u8, 60, 60], [60, 200, 90], [70, 120, 240], [240, 200, 40]];
    let scene_of = |id: i64| groups.iter().position(|g| g.contains(&id)).unwrap_or(0);
    for (sheet, chunk) in frames.chunks(COLS * ROWS).enumerate() {
        let (w, h) = (COLS * (TILE + 4), ROWS * (TILE + BAR + 6));
        let mut rgb = vec![20u8; w * h * 3];
        for (k, f) in chunk.iter().enumerate() {
            let (cx, cy) = ((k % COLS) * (TILE + 4), (k / COLS) * (TILE + BAR + 6));
            if let Some(p) = &f.preview_path {
                let img = turbo::decode_rgb(&std::fs::read(p)?, 256, 200_000_000)?;
                let (iw, ih) = (img.width as usize, img.height as usize);
                let s = (iw.max(ih) as f64 / TILE as f64).max(1.0);
                let (tw, th) = ((iw as f64 / s) as usize, (ih as f64 / s) as usize);
                for y in 0..th {
                    for x in 0..tw {
                        let si =
                            (((y as f64 * s) as usize).min(ih - 1) * iw + ((x as f64 * s) as usize).min(iw - 1)) * 3;
                        let di = ((cy + y + (TILE - th) / 2) * w + cx + x + (TILE - tw) / 2) * 3;
                        rgb[di..di + 3].copy_from_slice(&img.pixels[si..si + 3]);
                    }
                }
            }
            let c = colours[scene_of(f.id) % colours.len()];
            for y in cy + TILE + 2..cy + TILE + 2 + BAR {
                for x in cx..cx + TILE {
                    let di = (y * w + x) * 3;
                    rgb[di..di + 3].copy_from_slice(&c);
                }
            }
        }
        let out = dir.join(format!("detect_{sheet:02}.jpg"));
        std::fs::write(&out, turbo::encode_rgb(&rgb, w as u32, h as u32, 85)?)?;
        println!("detect sheet: {}", out.display());
    }
    Ok(())
}

/// Rows: stored settings (as shot) / anchor look copied verbatim (strength 0) / matched
/// (strength 1). Anchors framed in orange.
fn contact_sheet(
    cache: &DevelopCache,
    luts: &LutLibrary,
    anchors: &[MatchImage],
    targets: &[MatchImage],
    previews: &[MatchPreview],
    out: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    const CELL: u32 = 300;
    const PAD: u32 = 6;
    let mut cols: Vec<(SourceImage, [ParametricAdjustments; 3], bool)> = Vec::new();
    for a in anchors {
        cols.push((
            a.src.clone(),
            [ParametricAdjustments::default(), a.adjustments.clone(), a.adjustments.clone()],
            true,
        ));
    }
    for (t, p) in targets.iter().zip(previews).take(8) {
        cols.push((t.src.clone(), [t.adjustments.clone(), p.base.clone(), p.full.clone()], false));
    }
    let (w, h) = ((CELL + PAD) * cols.len() as u32 + PAD, (CELL + PAD) * 3 + PAD);
    let mut rgb = vec![40u8; (w * h * 3) as usize];
    for (c, (src, adjs, is_anchor)) in cols.iter().enumerate() {
        for (r, adj) in adjs.iter().enumerate() {
            let img = cache.render_image(src, adj, None, CELL, luts)?.image;
            let x0 = PAD + c as u32 * (CELL + PAD) + (CELL - img.width) / 2;
            let y0 = PAD + r as u32 * (CELL + PAD) + (CELL - img.height) / 2;
            if *is_anchor {
                for y in y0.saturating_sub(3)..(y0 + img.height + 3).min(h) {
                    for x in x0.saturating_sub(3)..(x0 + img.width + 3).min(w) {
                        let i = ((y * w + x) * 3) as usize;
                        rgb[i..i + 3].copy_from_slice(&[255, 140, 0]);
                    }
                }
            }
            for y in 0..img.height {
                let s = (y * img.width * 3) as usize;
                let d = (((y0 + y) * w + x0) * 3) as usize;
                rgb[d..d + (img.width * 3) as usize].copy_from_slice(&img.rgb[s..s + (img.width * 3) as usize]);
            }
        }
    }
    std::fs::write(out, turbo::encode_rgb(&rgb, w, h, 88)?)?;
    Ok(())
}

/// Finite-difference slopes of the rendered statistics around each image's as-shot WB.
fn calibrate(
    conn: &rusqlite::Connection,
    cache: &DevelopCache,
    luts: &LutLibrary,
    ids: &[i64],
) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{:>6} {:>7} {:>6} {:>9} {:>9} {:>9} {:>9} {:>7}",
        "id", "K", "tint", "da/dmir", "db/dmir", "da/dtint", "db/dtint", "dL/dEV"
    );
    for m in store::match_inputs(conn, ids)? {
        let Some(asw) = cache.info(&m.src)?.as_shot else { continue };
        let at = |mired: f32, tint: f32, ev: f32| {
            let adj = ParametricAdjustments {
                exposure: ev,
                white_balance: WhiteBalance::Custom { temperature_k: 1.0e6 / mired, tint },
                ..Default::default()
            };
            stats::render_stats(cache, luts, &m.src, &adj, None)
        };
        let m0 = 1.0e6 / asw.temperature_k;
        let s0 = at(m0, asw.tint, 0.0)?;
        let sm = at(m0 + 10.0, asw.tint, 0.0)?;
        let st = at(m0, asw.tint + 10.0, 0.0)?;
        let se = at(m0, asw.tint, 0.5)?;
        println!(
            "{:>6} {:>7.0} {:>6.1} {:>9.5} {:>9.5} {:>9.5} {:>9.5} {:>7.3}",
            m.src.id,
            asw.temperature_k,
            asw.tint,
            (sm.neutral.a - s0.neutral.a) / 10.0,
            (sm.neutral.b - s0.neutral.b) / 10.0,
            (st.neutral.a - s0.neutral.a) / 10.0,
            (st.neutral.b - s0.neutral.b) / 10.0,
            (se.log_mean_luma - s0.log_mean_luma) / 0.5
        );
    }
    Ok(())
}
