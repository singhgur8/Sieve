//! Acceptance numbers of the baseline edit's light normalization (roadmap Phase 10, "Light
//! normalization engine").
//!
//! ```text
//! cargo run --release --example baseline_eval -- --out ../test-data/baseline-eval \
//!     [--per-scene 16] [--timing 2500] [--raws DIR] [--no-xmp]
//! ```
//! 1. Synthetic shoot (`develop::baseline::synth`): a dark church, a bright outdoor set, a
//!    tungsten reception with window-lit frames (mixed light), a back-lit silhouette set and a
//!    low-key portrait set, bursts of 4 with known exposure / white-balance shifts, 16-bit
//!    PNGs with an 18 % grey card.
//! 2. Anchor = an outdoor frame with a preset-like look and the user's light: Auto + 0.30 EV,
//!    15 mired warmer, tint +4, contrast +12, highlights -15, shadows +10, blacks -5.
//! 3. The engine (`compute` with the real `DevelopMeter`) vs plain copy-paste of the anchor's
//!    settings: every frame rendered at 384 px; per scene the spread (SD, range) of the frame
//!    mean L*, of the grey card L* and of the card's a*b* (WB), and the card's distance to the
//!    anchor's card (the anchor's look target).
//! 4. Look keys byte-identical to the anchor's, never keys untouched, determinism (two runs).
//! 5. XMP round trip: a scratch catalog, `run_pipeline`, `XmpSync::write_images`, exiftool.
//! 6. Timing: `compute` over `--timing` synthetic 384 px JPEG previews (+ `--raws`: every RAW).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use sieve_lib::db::baseline::PhotoEditState;
use sieve_lib::develop::baseline::synth::{self, SceneKind, SynthFrame};
use sieve_lib::develop::baseline::{self as engine, DevelopMeter, LightMeter, PhotoInput};
use sieve_lib::develop::{DevelopCache, DevelopConfig, SourceImage};
use sieve_lib::ipc::types::*;
use sieve_lib::lut::LutLibrary;

const W: u32 = 384;
const H: u32 = 256;

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn flag(name: &str) -> bool {
    std::env::args().any(|a| a == name)
}

fn look() -> ParametricAdjustments {
    let mut a = ParametricAdjustments::defaults_for(ImageFormat::Png);
    a.vibrance = 15.0;
    a.saturation = -6.0;
    a.clarity = 8.0;
    a.texture = 5.0;
    a.hsl.saturation.orange = -12.0;
    a.hsl.luminance.orange = 8.0;
    a.hsl.hue.green = 20.0;
    a.hsl.saturation.blue = -20.0;
    a.tone_curve.parametric.shadows = 12.0;
    a.tone_curve.parametric.highlights = -8.0;
    a.color_grading.shadows.hue = 210.0;
    a.color_grading.shadows.saturation = 10.0;
    a.color_grading.highlights.hue = 45.0;
    a.color_grading.highlights.saturation = 8.0;
    a.effects.grain.amount = 15.0;
    a.effects.vignette.amount = -10.0;
    a.detail.sharpening.amount = 50.0;
    a
}

/// The anchor's offset from its Auto (what "the user" did on top of Auto).
fn user_offset() -> LightOffset {
    LightOffset {
        exposure: 0.3,
        contrast: 12.0,
        highlights: -15.0,
        shadows: 10.0,
        whites: 0.0,
        blacks: -5.0,
        temperature_mired: -15.0,
        tint: 4.0,
    }
}

fn input(id: ImageId, path: PathBuf, f: Option<&SynthFrame>, format: ImageFormat) -> PhotoInput {
    PhotoInput {
        image_id: id,
        src: SourceImage { id, path, orientation: None },
        format,
        current: ParametricAdjustments::defaults_for(format),
        scene_id: f.map(|f| f.scene_id),
        burst_group_id: f.map(|f| f.burst_id),
        captured_at_ms: f.map(|f| f.captured_at_ms),
        faces: f.map(|f| f.faces.clone()),
        state: PhotoEditState::Unedited,
        missing: false,
        exposure_ev: f.map(|f| f.exposure_ev),
    }
}

fn look_bytes(a: &ParametricAdjustments) -> String {
    let mut l = ParametricAdjustments::default();
    l.copy_fields(a, &fields_of_class(SettingClass::Look));
    serde_json::to_string(&l).unwrap()
}

fn never_bytes(a: &ParametricAdjustments) -> String {
    let mut l = ParametricAdjustments::default();
    l.copy_fields(a, &fields_of_class(SettingClass::Never));
    serde_json::to_string(&l).unwrap()
}

#[derive(Default, Clone)]
struct Metrics {
    frame_l: Vec<f32>,
    card: Vec<[f32; 3]>,
}

fn sd(v: &[f32]) -> f32 {
    let n = v.len().max(1) as f32;
    let m = v.iter().sum::<f32>() / n;
    (v.iter().map(|x| (x - m).powi(2)).sum::<f32>() / n).sqrt()
}

fn range(v: &[f32]) -> f32 {
    v.iter().cloned().fold(f32::MIN, f32::max) - v.iter().cloned().fold(f32::MAX, f32::min)
}

fn render_metrics(
    cache: &DevelopCache,
    luts: &LutLibrary,
    p: &PhotoInput,
    adj: &ParametricAdjustments,
    f: &SynthFrame,
) -> (f32, Option<[f32; 3]>) {
    let r = cache.render_image(&p.src, adj, None, W, luts).expect("render");
    let (w, h) = (r.image.width, r.image.height);
    let frame = synth::lab_mean(&r.image.rgb, w, h, None)[0];
    let card = f.card.as_ref().map(|c| synth::lab_mean(&r.image.rgb, w, h, Some(c)));
    (frame, card)
}

fn main() {
    let out = PathBuf::from(arg("--out").unwrap_or_else(|| "../test-data/baseline-eval".into()));
    let per_scene: usize = arg("--per-scene").and_then(|v| v.parse().ok()).unwrap_or(16);
    let timing: usize = arg("--timing").and_then(|v| v.parse().ok()).unwrap_or(2500);
    let frames_dir = out.join("frames");
    std::fs::create_dir_all(&frames_dir).unwrap();

    // 1. Synthetic shoot.
    let shoot = synth::shoot(per_scene, 7);
    let mut photos = Vec::new();
    for (i, f) in shoot.iter().enumerate() {
        let path = frames_dir.join(format!("f{:04}_{}.png", i, f.kind.name().replace(' ', "_")));
        synth::write_png(&path, W, H, &synth::render(f, W, H)).unwrap();
        photos.push(input(i as ImageId + 1, path, Some(f), ImageFormat::Png));
    }
    // A crop and a straighten on two frames: never copied.
    photos[3].current.crop.enabled = true;
    photos[3].current.crop.left = 0.1;
    photos[20].current.transform.rotate = 1.5;
    let cache = DevelopCache::new(DevelopConfig::default());
    let meter = DevelopMeter { cache: cache.clone() };
    let luts = LutLibrary::new(out.join("luts"));

    // 2. Anchor: an outdoor frame.
    let ai = shoot.iter().position(|f| f.kind == SceneKind::BrightOutdoor && f.index == 5).unwrap();
    let look0 = look();
    let auto_a = meter.measure(&photos[ai], &look0).unwrap().auto;
    let anchor_light = user_offset().add_to(&auto_a);
    let (anchor_light, _) = engine::clamp_light(&anchor_light);
    let anchor_settings = anchor_light.apply_to(&look0);
    photos[ai].current = anchor_settings.clone();
    photos[ai].state = PhotoEditState::Edited;
    let anchor = engine::measure_anchor(&meter, &photos[ai], &anchor_settings).unwrap();
    println!(
        "anchor #{}: auto exposure {:+.2} EV, {:.0} K / {:+.0}; light {:+.2} EV, {:.0} K / {:+.0}; offset {:+.2} EV, {:+.1} mired, tint {:+.1}",
        anchor.image_id,
        anchor.auto.exposure,
        anchor.auto.temperature_k,
        anchor.auto.tint,
        anchor.light.exposure,
        anchor.light.temperature_k,
        anchor.light.tint,
        anchor.offset.exposure,
        anchor.offset.temperature_mired,
        anchor.offset.tint
    );
    let settings = BaselineSettings {
        anchor_id: anchor.image_id,
        preset_id: None,
        scope: BaselineScope::All,
        replace_edited: false,
    };

    // 3. Engine.
    let never = AtomicBool::new(false);
    let t = Instant::now();
    let drafts = engine::compute(
        &meter,
        &anchor_settings,
        &anchor,
        Some(&photos[ai]),
        &photos,
        &settings,
        &never,
        &mut |_, _| {},
    )
    .unwrap();
    println!("engine on {} frames: {:.2} s", photos.len(), t.elapsed().as_secs_f32());
    let drafts2 = engine::compute(
        &meter,
        &anchor_settings,
        &anchor,
        Some(&photos[ai]),
        &photos,
        &settings,
        &never,
        &mut |_, _| {},
    )
    .unwrap();
    let deterministic = serde_json::to_string(&drafts.iter().map(|d| (&d.result, &d.adjustments)).collect::<Vec<_>>())
        .unwrap()
        == serde_json::to_string(&drafts2.iter().map(|d| (&d.result, &d.adjustments)).collect::<Vec<_>>()).unwrap();

    // Anchor target.
    let (anchor_frame_l, anchor_card) = render_metrics(&cache, &luts, &photos[ai], &anchor_settings, &shoot[ai]);
    let anchor_card = anchor_card.unwrap();
    println!(
        "anchor render: frame L* {anchor_frame_l:.1}, card L* {:.1} a* {:+.1} b* {:+.1}",
        anchor_card[0], anchor_card[1], anchor_card[2]
    );

    // Metrics per scene (the tungsten reception's window-lit frames as their own row).
    let anchor_look = look_bytes(&anchor_settings);
    let (mut look_ok, mut look_bad, mut never_ok, mut never_bad) = (0, 0, 0, 0);
    type Key = (SceneKind, bool);
    let mut copy: BTreeMap<Key, Metrics> = BTreeMap::new();
    let mut base: BTreeMap<Key, Metrics> = BTreeMap::new();
    let mut model: BTreeMap<Key, Metrics> = BTreeMap::new();
    // Baseline vs its per-photo model target (Auto + offset): |dL* frame|, card dab.
    let mut to_model: BTreeMap<Key, Vec<(f32, f32)>> = BTreeMap::new();
    let mut flags: BTreeMap<SceneKind, BTreeMap<String, usize>> = BTreeMap::new();
    let mut exposures: BTreeMap<SceneKind, Vec<(f32, f32)>> = BTreeMap::new();
    for (i, (p, d)) in photos.iter().zip(&drafts).enumerate() {
        let f = &shoot[i];
        let key = (f.kind, f.daylight);
        let Some(adj) = &d.adjustments else { continue };
        if look_bytes(adj) == anchor_look {
            look_ok += 1;
        } else {
            look_bad += 1;
        }
        if never_bytes(adj) == never_bytes(&p.current) {
            never_ok += 1;
        } else {
            never_bad += 1;
        }
        for r in &d.result.reasons {
            *flags.entry(f.kind).or_default().entry(format!("{:?}", r.kind)).or_default() += 1;
        }
        let light = d.result.light.unwrap();
        exposures.entry(f.kind).or_default().push((light.exposure, d.result.auto.map_or(f32::NAN, |a| a.exposure)));
        let kept_dark = d
            .result
            .reasons
            .iter()
            .any(|r| matches!(r.kind, BaselineReasonKind::LowKey | BaselineReasonKind::Silhouette));
        let pasted = engine::compose(&p.current, &anchor_settings, &anchor.light);
        let (fl, card) = render_metrics(&cache, &luts, p, adj, f);
        let (cl, ccard) = render_metrics(&cache, &luts, p, &pasted, f);
        let m = base.entry(key).or_default();
        m.frame_l.push(fl);
        m.card.extend(card);
        let m = copy.entry(key).or_default();
        m.frame_l.push(cl);
        m.card.extend(ccard);
        // The per-photo model target: Auto + offset (without the Auto lift for kept-dark
        // frames), before smoothing.
        if let Some(auto) = d.result.auto {
            let base_light = if kept_dark { engine::without_lift(&auto) } else { auto };
            let (l, _) = engine::clamp_light(&anchor.offset.add_to(&base_light));
            let a = engine::compose(&p.current, &anchor_settings, &l);
            let (al, acard) = render_metrics(&cache, &luts, p, &a, f);
            let m = model.entry(key).or_default();
            m.frame_l.push(al);
            m.card.extend(acard);
            // Auto WB fell back to the file's as-shot (6500 K / 0 for these PNGs): no WB target.
            let wb_fallback = auto.temperature_k == 6500.0 && auto.tint == 0.0;
            let dab = match (card, acard, wb_fallback) {
                (Some(b), Some(a), false) => (b[1] - a[1]).hypot(b[2] - a[2]),
                _ => f32::NAN,
            };
            to_model.entry(key).or_default().push(((fl - al).abs(), dab));
        }
        if flag("--debug") {
            println!(
                "  #{:<3} {:<20} {:?} light {:+.2} EV {:.0} K {:+.0} | auto {:?} | {:?} | card b {:?} c {:?}",
                p.image_id,
                f.kind.name(),
                d.result.outcome,
                light.exposure,
                light.temperature_k,
                light.tint,
                d.result.auto.map(|a| (a.exposure, a.temperature_k, a.tint)),
                d.result.reasons.iter().map(|r| format!("{:?}", r.kind)).collect::<Vec<_>>(),
                card.map(|c| c.map(|v| (v * 10.0).round() / 10.0)),
                ccard.map(|c| c.map(|v| (v * 10.0).round() / 10.0))
            );
        }
    }

    println!("\nper scene (frames written; anchor excluded): copy-paste of the anchor's settings (c) vs baseline (b)");
    println!(
        "{:<26} {:>3} | {:>15} {:>15} | {:>15} {:>15} | {:>15} | {:>15} {:>15} | {:>13}",
        "scene",
        "n",
        "frameL* SD c/b",
        "frameL* rng c/b",
        "cardL* SD c/b",
        "cardL* rng c/b",
        "card ab SD c/b",
        "|dL*| anchor c/b",
        "|dab| anchor c/b",
        "b-target dL/ab"
    );
    let ab_sd = |m: &Metrics| {
        let a: Vec<f32> = m.card.iter().map(|c| c[1]).collect();
        let bb: Vec<f32> = m.card.iter().map(|c| c[2]).collect();
        (sd(&a).powi(2) + sd(&bb).powi(2)).sqrt()
    };
    let card_l = |m: &Metrics| m.card.iter().map(|c| c[0]).collect::<Vec<f32>>();
    let to_anchor = |m: &Metrics| {
        let n = m.card.len().max(1) as f32;
        let dl = m.card.iter().map(|c| (c[0] - anchor_card[0]).abs()).sum::<f32>() / n;
        let dab = m.card.iter().map(|c| (c[1] - anchor_card[1]).hypot(c[2] - anchor_card[2])).sum::<f32>() / n;
        (dl, dab)
    };
    for (key, b) in &base {
        let c = &copy[key];
        let (cdl, cdab) = to_anchor(c);
        let (bdl, bdab) = to_anchor(b);
        let has_card = !b.card.is_empty();
        let fmt2 = |x: f32, y: f32| if has_card { format!("{x:>6.2} / {y:<6.2}") } else { format!("{:>15}", "-") };
        let tm = to_model.get(key).cloned().unwrap_or_default();
        let n = tm.len().max(1) as f32;
        let tdl = tm.iter().map(|t| t.0).sum::<f32>() / n;
        let finite: Vec<f32> = tm.iter().map(|t| t.1).filter(|v| v.is_finite()).collect();
        let tab = finite.iter().sum::<f32>() / finite.len().max(1) as f32;
        let name = format!("{}{}", key.0.name(), if key.1 { " (window light)" } else { "" });
        println!(
            "{:<26} {:>3} | {:>6.2} / {:<6.2} {:>6.2} / {:<6.2} | {} {} | {} | {} {} | {:>5.2} / {:<5.2}",
            name,
            b.frame_l.len(),
            sd(&c.frame_l),
            sd(&b.frame_l),
            range(&c.frame_l),
            range(&b.frame_l),
            fmt2(sd(&card_l(c)), sd(&card_l(b))),
            fmt2(range(&card_l(c)), range(&card_l(b))),
            fmt2(ab_sd(c), ab_sd(b)),
            fmt2(cdl, bdl),
            fmt2(cdab, bdab),
            tdl,
            if has_card { tab } else { f32::NAN },
        );
    }
    if flag("--debug") {
        for (key, m) in &model {
            println!(
                "  model target {:?}: frame L* SD {:.2}, card L* SD {:.2}, card ab SD {:.2}",
                key,
                sd(&m.frame_l),
                sd(&card_l(m)),
                ab_sd(m)
            );
        }
    }
    println!("\nflags per scene:");
    for (k, f) in &flags {
        println!("  {:<20} {:?}", k.name(), f);
    }
    println!("\nexposure written (baseline) / Auto per scene:");
    for (k, e) in &exposures {
        let s: Vec<String> = e.iter().map(|(l, a)| format!("{l:+.2}({a:+.2})")).collect();
        println!("  {:<20} {}", k.name(), s.join(" "));
    }
    println!(
        "\nlook keys identical to the anchor: {look_ok} / {} | never keys untouched: {never_ok} / {} | deterministic: {deterministic}",
        look_ok + look_bad,
        never_ok + never_bad
    );

    if !flag("--no-xmp") {
        xmp_round_trip(&out, &shoot, &photos, ai, &anchor_settings, &cache);
    }

    // 6. Timing.
    if timing > 0 {
        timing_run(&out, timing);
    }
    if let Some(raws) = arg("--raws") {
        raw_run(Path::new(&raws));
    }
}

/// Scratch catalog: a few church frames + the anchor; `run_pipeline`; sidecars; exiftool.
fn xmp_round_trip(
    out: &Path,
    shoot: &[SynthFrame],
    photos: &[PhotoInput],
    ai: usize,
    anchor_settings: &ParametricAdjustments,
    cache: &DevelopCache,
) {
    use sieve_lib::db;
    let dir = out.join("xmp");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut picks: Vec<usize> =
        shoot.iter().enumerate().filter(|(_, f)| f.kind == SceneKind::DarkChurch).map(|(i, _)| i).take(4).collect();
    picks.push(ai);
    let catalog = dir.join("catalog.sqlite");
    let mut conn = db::open(&catalog).unwrap();
    conn.execute_batch("INSERT INTO projects (id, name, shoot_type, created_at) VALUES (1, 'p', 'wedding', 0);")
        .unwrap();
    conn.execute("INSERT INTO folders (id, path, added_at, project_id) VALUES (1, ?1, 0, 1)", [dir.to_string_lossy()])
        .unwrap();
    for &i in &picks {
        let name = photos[i].src.path.file_name().unwrap().to_string_lossy().to_string();
        let path = dir.join(&name);
        std::fs::copy(&photos[i].src.path, &path).unwrap();
        conn.execute(
            "INSERT INTO images (id, folder_id, path, file_name, format, camera_make, sensor_layout, file_size,
                                 file_mtime_ms, imported_at, captured_at_ms)
             VALUES (?1, 1, ?2, ?3, 'png', 'other', 'bayer', 1, 0, 0, ?4)",
            rusqlite::params![photos[i].image_id, path.to_string_lossy(), name, shoot[i].captured_at_ms],
        )
        .unwrap();
    }
    let anchor_id = photos[ai].image_id;
    sieve_lib::develop::history::commit(&mut conn, anchor_id, anchor_settings, "Exposure").unwrap();
    let settings = BaselineSettings { anchor_id, preset_id: None, scope: BaselineScope::All, replace_edited: false };
    let run_id = sieve_lib::db::baseline::begin_run(&conn, 1, &settings, engine::ENGINE_VERSION).unwrap();
    let job = engine::BaselineJob { run_id, project_id: 1, settings };
    let meter = DevelopMeter { cache: cache.clone() };
    let never = AtomicBool::new(false);
    let o = engine::run_pipeline(&mut conn, &meter, &job, &never, &mut |_, _, _| {}).unwrap();
    sieve_lib::db::baseline::finish_run(&conn, run_id, BaselineRunState::Finished, o.message.as_deref()).unwrap();
    println!("\nXMP round trip: {}", o.message.unwrap_or_default());
    drop(conn);
    let sync = sieve_lib::xmp::XmpSync::new(sieve_lib::xmp::XmpSyncConfig { catalog_path: catalog.clone() });
    let ids: Vec<ImageId> = picks.iter().map(|&i| photos[i].image_id).collect();
    let report = sync.write_images(&ids).unwrap();
    println!("sidecars written: {} (failed {:?})", report.succeeded, report.failed);
    let conn = db::open(&catalog).unwrap();
    let first = photos[picks[0]].image_id;
    let stored = sieve_lib::db::repo::get_adjustments(&conn, first).unwrap();
    let sidecar = sieve_lib::xmp::sidecar_path(&dir.join(photos[picks[0]].src.path.file_name().unwrap()));
    // Every crs: tag exiftool reads (`-G1 -s`: group + tag names), the light keys first.
    let out = std::process::Command::new("exiftool").args(["-G1", "-s", "-XMP-crs:all"]).arg(&sidecar).output();
    match out {
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stdout).to_string();
            let light = [
                "Exposure2012",
                "Contrast2012",
                "Highlights2012",
                "Shadows2012",
                "Whites2012",
                "Blacks2012",
                "WhiteBalance",
                "ColorTemperature",
                "Tint",
                "IncrementalTemperature",
                "IncrementalTint",
            ];
            let is_light =
                |l: &str| light.iter().any(|k| l.split(':').next().unwrap_or("").contains(&format!(" {k} ")));
            println!("exiftool -G1 -s -XMP-crs:all {} ({} tags):", sidecar.display(), text.lines().count());
            for l in text.lines().filter(|l| is_light(l)) {
                println!("  {l}");
            }
            println!("  ... look keys:");
            for l in text.lines().filter(|l| !is_light(l)) {
                println!("  {l}");
            }
        }
        Err(e) => println!("exiftool unavailable: {e}"),
    }
    println!(
        "catalog: Exposure2012 {:+.2} Contrast2012 {:+.0} Highlights2012 {:+.0} Shadows2012 {:+.0} Whites2012 {:+.0} Blacks2012 {:+.0} WB {:?}",
        stored.exposure, stored.contrast, stored.highlights, stored.shadows, stored.whites, stored.blacks, stored.white_balance
    );
}

/// `compute` over `n` distinct synthetic 384 px JPEG previews (decode + measure + smooth).
fn timing_run(out: &Path, n: usize) {
    let dir = out.join("timing");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let per = n.div_ceil(4);
    let mut frames = Vec::new();
    for (k, kind) in
        [SceneKind::DarkChurch, SceneKind::BrightOutdoor, SceneKind::TungstenReception, SceneKind::BrightOutdoor]
            .iter()
            .enumerate()
    {
        frames.extend(synth::plan(
            *kind,
            k as i64 + 1,
            (k as i64 + 1) * 1000,
            per,
            4,
            k as i64 * 3_600_000,
            11 + k as u64,
        ));
    }
    frames.truncate(n);
    let t = Instant::now();
    let photos: Vec<PhotoInput> = frames
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let px16 = synth::render(f, W, H);
            let px8: Vec<u8> = px16.iter().map(|v| (v >> 8) as u8).collect();
            let path = dir.join(format!("t{i:05}.jpg"));
            std::fs::write(&path, sieve_lib::raw::turbo::encode_rgb_444(&px8, W, H, 90).unwrap()).unwrap();
            input(i as ImageId + 1, path, Some(f), ImageFormat::Jpeg)
        })
        .collect();
    println!("\ntiming: wrote {} previews in {:.1} s", photos.len(), t.elapsed().as_secs_f32());
    let cache = DevelopCache::new(DevelopConfig::default());
    let meter = DevelopMeter { cache };
    let mut look0 = look();
    look0.white_balance = WhiteBalance::Custom { temperature_k: 5600.0, tint: 5.0 };
    let anchor = engine::measure_anchor(&meter, &photos[1], &look0).unwrap();
    let settings = BaselineSettings { anchor_id: 1, preset_id: None, scope: BaselineScope::All, replace_edited: false };
    let never = AtomicBool::new(false);
    let mut last = 0;
    let t = Instant::now();
    let drafts =
        engine::compute(&meter, &look0, &anchor, Some(&photos[0]), &photos, &settings, &never, &mut |d, _| last = d)
            .unwrap();
    let s = t.elapsed().as_secs_f32();
    println!(
        "timing: {} photos ({} measured, {} threads): {:.1} s = {:.1} ms/photo, {:.0} photos/s",
        drafts.len(),
        last,
        engine::measure_threads(),
        s,
        1000.0 * s / drafts.len() as f32,
        drafts.len() as f32 / s
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Every RAW of `dir` as one scene: engine timing + values per frame.
fn raw_run(dir: &Path) {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| sieve_lib::raw::format_from_extension(p).is_some_and(|f| f.is_raw()))
        .collect();
    paths.sort();
    if paths.len() < 2 {
        println!("\nraws: fewer than 2 RAWs in {}", dir.display());
        return;
    }
    let photos: Vec<PhotoInput> = paths
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let format = sieve_lib::raw::format_from_extension(p).unwrap();
            // Unrelated cameras / subjects: no scene, no smoothing (per-photo Auto + offset).
            let mut x = input(i as ImageId + 1, p.clone(), None, format);
            if flag("--raws-one-scene") {
                x.scene_id = Some(1);
            }
            x
        })
        .collect();
    let cache = DevelopCache::new(DevelopConfig::default());
    let meter = DevelopMeter { cache: cache.clone() };
    let mut look0 = look();
    look0.profile = photos[0].current.profile.clone();
    let t = Instant::now();
    let auto0 = meter.measure(&photos[0], &look0).unwrap().auto;
    let light0 = engine::clamp_light(&user_offset().add_to(&auto0)).0;
    let anchor_settings = light0.apply_to(&ParametricAdjustments { ..look0.clone() });
    let anchor = engine::measure_anchor(&meter, &photos[0], &anchor_settings).unwrap();
    let settings = BaselineSettings { anchor_id: 1, preset_id: None, scope: BaselineScope::All, replace_edited: false };
    let never = AtomicBool::new(false);
    let drafts = engine::compute(
        &meter,
        &anchor_settings,
        &anchor,
        Some(&photos[0]),
        &photos,
        &settings,
        &never,
        &mut |_, _| {},
    )
    .unwrap();
    let s = t.elapsed().as_secs_f32();
    println!(
        "\nraws: {} files in {:.1} s ({:.0} ms/photo incl. half-size decode)",
        photos.len(),
        s,
        1000.0 * s / photos.len() as f32
    );
    let luts = LutLibrary::new(std::env::temp_dir().join("sieve-baseline-eval-luts"));
    let (mut cl, mut bl) = (Vec::new(), Vec::new());
    for (p, d) in photos.iter().zip(&drafts) {
        let name = p.src.path.file_name().unwrap().to_string_lossy();
        let Some(adj) = &d.adjustments else {
            println!("  {name}: {:?}", d.result.outcome);
            continue;
        };
        let pasted = engine::compose(&p.current, &anchor_settings, &anchor.light);
        let lb = cache.render_image(&p.src, adj, None, W, &luts).unwrap();
        let lc = cache.render_image(&p.src, &pasted, None, W, &luts).unwrap();
        let b = synth::lab_mean(&lb.image.rgb, lb.image.width, lb.image.height, None)[0];
        let c = synth::lab_mean(&lc.image.rgb, lc.image.width, lc.image.height, None)[0];
        cl.push(c);
        bl.push(b);
        let l = d.result.light.unwrap();
        println!(
            "  {name}: {:?} {:+.2} EV {:.0} K {:+.0} | frame L* copy {c:.1} baseline {b:.1} {:?}",
            d.result.outcome,
            l.exposure,
            l.temperature_k,
            l.tint,
            d.result.reasons.iter().map(|r| format!("{:?}", r.kind)).collect::<Vec<_>>()
        );
    }
    println!(
        "  frame L* SD copy {:.2} baseline {:.2}; range copy {:.2} baseline {:.2}",
        sd(&cl),
        sd(&bl),
        range(&cl),
        range(&bl)
    );
}
