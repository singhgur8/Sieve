//! Auto tone / Auto WB evaluation (Phase 8b): on the user's Lightroom-edited frames, runs
//! `develop::auto` with the frame's own other settings (curve, grading, profile, WB ...) and
//! compares the eight Auto sliders (and Temp/Tint) with the user's values; reports skin
//! clipping of the Auto result vs the user's edit. Sources are read in place, never written.
//!
//! ```text
//! cargo run --release --example auto_eval -- [--folder DIR] [--count N] [--models DIR]
//! ```
//! - `--folder` (default the Proposal shoot): every RAW/JPEG with a `<stem>.xmp` carrying
//!   develop settings (up to `--count`, spread evenly).
//! - `--models` (default `~/Library/Application Support/com.sieve.app/models`): SCRFD face
//!   detector for the face boxes (as the analysis would provide); faces are detected on the
//!   user's render. Without models, Auto runs without faces (skin-colour guard only).

use std::path::{Path, PathBuf};
use std::time::Instant;

use sieve_lib::develop::{auto, DevelopCache, DevelopConfig, SourceImage};
use sieve_lib::ipc::types::{AdjustmentField, NormRect, ParametricAdjustments, WhiteBalance};
use sieve_lib::lut::LutLibrary;
use sieve_lib::ml::segment::{RgbImage, SegmentConfig, SegmentEngine};
use sieve_lib::raw;
use sieve_lib::xmp;

const NAMES: [&str; 8] =
    ["Exposure", "Contrast", "Highlights", "Shadows", "Whites", "Blacks", "Vibrance", "Saturation"];

fn sliders(a: &ParametricAdjustments) -> [f32; 8] {
    [a.exposure, a.contrast, a.highlights, a.shadows, a.whites, a.blacks, a.vibrance, a.saturation]
}

#[derive(Default)]
struct Acc {
    n: usize,
    abs: [f64; 8],
    bias: [f64; 8],
    zero_abs: [f64; 8],
}

impl Acc {
    fn add(&mut self, auto: [f32; 8], user: [f32; 8]) {
        self.n += 1;
        for i in 0..8 {
            let d = f64::from(auto[i] - user[i]);
            self.abs[i] += d.abs();
            self.bias[i] += d;
            self.zero_abs[i] += f64::from(user[i]).abs();
        }
    }
    fn print(&self, title: &str) {
        let n = self.n.max(1) as f64;
        println!("\n{title}: {} frames", self.n);
        println!("{:<11} {:>10} {:>10} {:>12}", "slider", "mean|A-U|", "bias A-U", "mean|0-U|");
        for (i, name) in NAMES.iter().enumerate() {
            let (a, b, z) = (self.abs[i] / n, self.bias[i] / n, self.zero_abs[i] / n);
            println!("{name:<11} {a:>10.2} {b:>+10.2} {z:>12.2}");
        }
    }
}

fn mean_l(img: &sieve_lib::develop::pipeline::RenderedImage, faces: &[NormRect]) -> (f64, Option<f64>) {
    let lin = |v: u8| {
        let e = f64::from(v) / 255.0;
        if e <= 0.04045 {
            e / 12.92
        } else {
            ((e + 0.055) / 1.055).powf(2.4)
        }
    };
    let l = |px: &[u8]| {
        let y = 0.2126 * lin(px[0]) + 0.7152 * lin(px[1]) + 0.0722 * lin(px[2]);
        if y > 216.0 / 24389.0 {
            116.0 * y.cbrt() - 16.0
        } else {
            y * 24389.0 / 27.0
        }
    };
    let (w, h) = (img.width as usize, img.height as usize);
    let all: f64 = img.rgb.as_chunks::<3>().0.iter().map(|p| l(p)).sum::<f64>() / (w * h) as f64;
    let mut s = 0.0;
    let mut n = 0usize;
    for f in faces {
        let (cx, cy) = (f.x + f.width / 2.0, f.y + f.height / 2.0);
        let x0 = ((cx - f.width * 0.35) * w as f32).max(0.0) as usize;
        let x1 = (((cx + f.width * 0.35) * w as f32) as usize).min(w);
        let y0 = ((cy - f.height * 0.35) * h as f32).max(0.0) as usize;
        let y1 = (((cy + f.height * 0.35) * h as f32) as usize).min(h);
        for y in y0..y1 {
            for x in x0..x1 {
                let i = (y * w + x) * 3;
                s += l(&img.rgb[i..i + 3]);
                n += 1;
            }
        }
    }
    (all, (n > 0).then(|| s / n as f64))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let folder =
        PathBuf::from(arg("--folder").unwrap_or_else(|| "/Users/gurjotsingh/Pictures/Jasmit Natalie Proposal".into()));
    let count: usize = arg("--count").and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
    let models = PathBuf::from(arg("--models").unwrap_or_else(|| {
        format!("{}/Library/Application Support/com.sieve.app/models", std::env::var("HOME").unwrap_or_default())
    }));

    // Edited frames: RAW/JPEG + sidecar with develop settings.
    let mut frames: Vec<(PathBuf, ParametricAdjustments, bool)> = Vec::new();
    let mut files: Vec<PathBuf> = walkdir::WalkDir::new(&folder)
        .into_iter()
        .filter_map(Result::ok)
        .map(|e| e.path().to_path_buf())
        .filter(|p| raw::format_from_extension(p).is_some())
        .collect();
    files.sort();
    for p in files {
        let sc = xmp::sidecar_path(&p);
        let Ok(text) = std::fs::read_to_string(&sc) else { continue };
        let fmt = raw::format_from_extension(&p).unwrap();
        let Ok(v) = xmp::packet::parse_for(&text, fmt) else { continue };
        if let Some(a) = v.develop {
            frames.push((p, a, text.contains("AutoToneDigest")));
        }
    }
    // --split fit|hold: even / odd frames (tune on one, report the other).
    match arg("--split").as_deref() {
        Some("fit") => frames = frames.into_iter().step_by(2).collect(),
        Some("hold") => frames = frames.into_iter().skip(1).step_by(2).collect(),
        _ => {}
    }
    let mut targets = auto::AutoParams::default();
    if let Some(sets) = arg("--set") {
        for kv in sets.split(',').filter(|s| !s.is_empty()) {
            let (k, v) = kv.split_once('=').expect("name=value");
            assert!(targets.set(k, v.parse().expect("number")), "unknown parameter {k}");
        }
    }
    if let Some(v) = arg("--key").and_then(|v| v.parse().ok()) {
        targets.key = v;
    }
    if let Some(v) = arg("--face").and_then(|v| v.parse().ok()) {
        targets.face = v;
    }
    if let Some(v) = arg("--keep").and_then(|v| v.parse().ok()) {
        targets.keep = v;
    }
    if let Some(v) = arg("--face-weight").and_then(|v| v.parse().ok()) {
        targets.face_weight = v;
    }
    let mut wb_opts = auto::WbOptions::default();
    if let Some(v) = arg("--wb-pow").and_then(|v| v.parse().ok()) {
        wb_opts.weight_pow = v;
    }
    if let Some(v) = arg("--wb-strength").and_then(|v| v.parse().ok()) {
        wb_opts.strength = v;
    }
    if args.iter().any(|a| a == "--wb-no-blue") {
        wb_opts.exclude_blue = true;
    }
    println!("targets {targets:?} wb {wb_opts:?}");
    // Camera Raw's own Auto (Adobe DNG Converter with crs:AutoTone + WhiteBalance Auto;
    // `test-data/lr-auto/make_oracle.sh`): stem -> resolved values.
    let lr_auto_map: serde_json::Map<String, serde_json::Value> = arg("--lr-auto")
        .map(|p| serde_json::from_str(&std::fs::read_to_string(&p).expect("--lr-auto file")).expect("json"))
        .unwrap_or_default();
    let lr_num = |v: &serde_json::Value, k: &str| -> Option<f32> {
        match v.get(k)? {
            serde_json::Value::Number(n) => n.as_f64().map(|x| x as f32),
            serde_json::Value::String(t) => t.trim_start_matches('+').parse().ok(),
            _ => None,
        }
    };
    let total = frames.len();
    if count < total {
        let step = total as f64 / count as f64;
        frames = (0..count).map(|i| frames[(i as f64 * step) as usize].clone()).collect();
    }
    println!(
        "{} edited frames ({} total), {} with Lightroom Auto history",
        frames.len(),
        total,
        frames.iter().filter(|f| f.2).count()
    );

    let cache = DevelopCache::new(DevelopConfig { cache_bytes: 1 << 30, mask_cache: None });
    let luts = LutLibrary::new(std::env::temp_dir().join("sieve-auto-eval-luts"));
    let mut seg = models.join("det_10g.onnx").is_file().then(|| SegmentEngine::new(SegmentConfig::new(&models)));
    if seg.is_none() {
        println!("(no face models under {}: skin-colour guard only)", models.display());
    }

    let (mut all, mut lr_auto, mut with_faces) = (Acc::default(), Acc::default(), Acc::default());
    let (mut wb_n, mut wb_temp, mut wb_tint, mut wb_asshot_temp, mut wb_asshot_tint) = (0usize, 0.0, 0.0, 0.0, 0.0);
    let (mut l_all_d, mut l_face_d, mut l_face_n) = (0.0f64, 0.0f64, 0usize);
    let (mut z_all_d, mut z_face_d) = (0.0f64, 0.0f64);
    let (mut u_all, mut u_face, mut a_all, mut a_face) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut wb_rows: Vec<(f32, f32, f32)> = Vec::new();
    let (mut skin_frames, mut auto_clip_frames, mut user_clip_frames) = (0usize, 0usize, 0usize);
    let (mut auto_clip_sum, mut user_clip_sum, mut auto_clip_max) = (0.0f64, 0.0f64, 0.0f32);
    let mut times = Vec::new();
    let (mut vs_lr, mut vs_lr_faces) = (Acc::default(), Acc::default());
    let (mut lr_wb_n, mut lr_wb_t, mut lr_wb_n_t, mut lr_asshot_t, mut lr_asshot_n) =
        (0usize, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let (mut lr_l, mut lr_lf, mut lr_lf_n, mut lr_z, mut lr_zf) = (0.0f64, 0.0f64, 0usize, 0.0f64, 0.0f64);
    let (
        mut lr_skin_n,
        mut lr_skin_sieve_over,
        mut lr_skin_lr_over,
        mut lr_skin_sieve_sum,
        mut lr_skin_lr_sum,
        mut lr_skin_sieve_max,
    ) = (0usize, 0usize, 0usize, 0.0f64, 0.0f64, 0.0f32);
    for (i, (path, user, lr)) in frames.iter().enumerate() {
        let fmt = raw::format_from_extension(path).unwrap();
        let mut buf = Vec::new();
        let orientation = raw::extract(path, fmt, &mut buf).ok().and_then(|e| e.meta.orientation).map(|o| o as u8);
        let src = SourceImage { id: i as i64 + 1, path: path.clone(), orientation };
        // Faces on the user's (uncropped) render, as the analysis boxes are uncropped.
        let mut probe_adj = user.clone();
        probe_adj.crop.enabled = false;
        let faces: Vec<NormRect> = match (&mut seg, cache.render_image(&src, &probe_adj, None, 1024, &luts)) {
            (Some(engine), Ok(r)) => {
                let img = &r.image;
                let (w, h) = (img.width as f32, img.height as f32);
                engine
                    .detect_faces(RgbImage { data: &img.rgb, width: img.width as usize, height: img.height as usize })
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|d| d.score >= 0.6 && (d.bbox[3] - d.bbox[1]) / h >= 0.04)
                    .map(|d| NormRect {
                        x: d.bbox[0] / w,
                        y: d.bbox[1] / h,
                        width: (d.bbox[2] - d.bbox[0]) / w,
                        height: (d.bbox[3] - d.bbox[1]) / h,
                    })
                    .collect()
            }
            (_, Err(e)) => {
                println!("{}: {}", path.display(), e.message);
                continue;
            }
            _ => Vec::new(),
        };
        let t = Instant::now();
        let a = match auto::auto_tone_opts(&cache, &src, user, AdjustmentField::AUTO_TONE, Some(&faces), targets) {
            Ok(a) => a,
            Err(e) => {
                println!("{}: {}", path.display(), e.message);
                continue;
            }
        };
        times.push(t.elapsed().as_secs_f64() * 1000.0);
        let auto_adj = a.apply_to(user);
        let av = sliders(&auto_adj);
        let uv = sliders(user);
        all.add(av, uv);
        if *lr {
            lr_auto.add(av, uv);
        }
        if !faces.is_empty() {
            with_faces.add(av, uv);
        }
        // White balance vs the user's.
        if let (Ok(wbv), WhiteBalance::Custom { temperature_k, tint }) =
            (auto::auto_white_balance_opts(&cache, &src, user, wb_opts), user.white_balance)
        {
            wb_n += 1;
            wb_rows.push((
                wbv.temperature_k,
                temperature_k,
                cache.info(&src).ok().and_then(|i| i.as_shot).map_or(0.0, |s| s.temperature_k),
            ));
            wb_temp += f64::from((wbv.temperature_k - temperature_k).abs());
            wb_tint += f64::from((wbv.tint - tint).abs());
            if let Ok(info) = cache.info(&src) {
                if let Some(s) = info.as_shot {
                    wb_asshot_temp += f64::from((s.temperature_k - temperature_k).abs());
                    wb_asshot_tint += f64::from((s.tint - tint).abs());
                }
            }
        }
        // Render-level: mean L* (global, faces) auto vs user; skin clipping.
        let mut ua = user.clone();
        ua.crop.enabled = false;
        let mut aa = auto_adj.clone();
        aa.crop.enabled = false;
        // Baseline: the eight sliders at 0 (no Auto, the user's other settings).
        let zero = sieve_lib::ipc::types::AutoToneValues {
            exposure: Some(0.0),
            contrast: Some(0.0),
            highlights: Some(0.0),
            shadows: Some(0.0),
            whites: Some(0.0),
            blacks: Some(0.0),
            vibrance: Some(0.0),
            saturation: Some(0.0),
        }
        .apply_to(&ua);
        if let (Ok(ru), Ok(ra), Ok(rz)) = (
            cache.render_image(&src, &ua, None, 768, &luts),
            cache.render_image(&src, &aa, None, 768, &luts),
            cache.render_image(&src, &zero, None, 768, &luts),
        ) {
            let (lu, fu) = mean_l(&ru.image, &faces);
            let (la, fa) = mean_l(&ra.image, &faces);
            let (lz, fz) = mean_l(&rz.image, &faces);
            if std::env::var_os("AUTO_EVAL_ROWS").is_some() {
                println!(
                    "ROW {:<14} faces {} exp user {:+.2} auto {:+.2} | L* user {:.1} auto {:.1} zero {:.1} | face user {:.1} auto {:.1}",
                    path.file_name().unwrap().to_string_lossy(),
                    faces.len(),
                    user.exposure,
                    auto_adj.exposure,
                    lu,
                    la,
                    lz,
                    fu.unwrap_or(0.0),
                    fa.unwrap_or(0.0)
                );
            }
            l_all_d += (la - lu).abs();
            z_all_d += (lz - lu).abs();
            u_all += lu;
            a_all += la;
            if let (Some(fu), Some(fa), Some(fz)) = (fu, fa, fz) {
                l_face_d += (fa - fu).abs();
                z_face_d += (fz - fu).abs();
                u_face += fu;
                a_face += fa;
                l_face_n += 1;
            }
        }
        let clip_a = auto::skin_clipped_fraction(&cache, &src, &aa, Some(&faces)).ok().flatten();
        let clip_u = auto::skin_clipped_fraction(&cache, &src, &ua, Some(&faces)).ok().flatten();
        if let (Some(ca), Some(cu)) = (clip_a, clip_u) {
            skin_frames += 1;
            auto_clip_sum += f64::from(ca);
            user_clip_sum += f64::from(cu);
            auto_clip_max = auto_clip_max.max(ca);
            auto_clip_frames += usize::from(ca > auto::SKIN_CLIP_MAX);
            if ca > auto::SKIN_CLIP_MAX {
                println!(
                    "   skin clip {:.2}% on {} (user {:.2}%), auto {:?}",
                    100.0 * ca,
                    path.display(),
                    100.0 * cu,
                    av
                );
            }
            user_clip_frames += usize::from(cu > auto::SKIN_CLIP_MAX);
        }
        if i % 25 == 0 {
            println!(
                "{:>4} {:<14} faces {} auto [{}] user [{}] {:.0} ms",
                i,
                path.file_name().unwrap().to_string_lossy(),
                faces.len(),
                av.iter().map(|v| format!("{v:.2}")).collect::<Vec<_>>().join(" "),
                uv.iter().map(|v| format!("{v:.2}")).collect::<Vec<_>>().join(" "),
                times.last().unwrap()
            );
        }
        // Against Camera Raw's own Auto, from Lightroom's defaults (WB = Camera Raw's Auto WB).
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        if let Some(lr) = lr_auto_map.get(&stem) {
            let g = |k: &str| lr_num(lr, k).unwrap_or(0.0);
            let lr_vals = [
                g("Exposure2012"),
                g("Contrast2012"),
                g("Highlights2012"),
                g("Shadows2012"),
                g("Whites2012"),
                g("Blacks2012"),
                g("Vibrance"),
                g("Saturation"),
            ];
            let mut base = ParametricAdjustments::defaults_for(fmt);
            base.white_balance = WhiteBalance::Custom { temperature_k: g("ColorTemperature"), tint: g("Tint") };
            if let Some(out) = std::env::var_os("AUTO_EVAL_FEATURES") {
                if let Ok((e, f)) = auto::exposure_features(&cache, &src, &base, Some(&faces), targets) {
                    use std::io::Write;
                    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(out).unwrap();
                    let fs: Vec<String> = f.iter().map(|v| format!("{v:.5}")).collect();
                    let ls: Vec<String> = lr_vals.iter().map(|v| format!("{v}")).collect();
                    writeln!(file, "{stem},{e},{},{}", fs.join(","), ls.join(",")).unwrap();
                }
            }
            if let Ok(sa) = auto::auto_tone_opts(&cache, &src, &base, AdjustmentField::AUTO_TONE, Some(&faces), targets)
            {
                let sieve_adj = sa.apply_to(&base);
                let sv = sliders(&sieve_adj);
                vs_lr.add(sv, lr_vals);
                if !faces.is_empty() {
                    vs_lr_faces.add(sv, lr_vals);
                }
                let mut lr_adj = base.clone();
                (lr_adj.exposure, lr_adj.contrast, lr_adj.highlights, lr_adj.shadows) =
                    (lr_vals[0], lr_vals[1], lr_vals[2], lr_vals[3]);
                (lr_adj.whites, lr_adj.blacks, lr_adj.vibrance, lr_adj.saturation) =
                    (lr_vals[4], lr_vals[5], lr_vals[6], lr_vals[7]);
                if let (Ok(r_lr), Ok(r_s), Ok(r_0)) = (
                    cache.render_image(&src, &lr_adj, None, 768, &luts),
                    cache.render_image(&src, &sieve_adj, None, 768, &luts),
                    cache.render_image(&src, &base, None, 768, &luts),
                ) {
                    let (a, af) = mean_l(&r_lr.image, &faces);
                    let (b, bf) = mean_l(&r_s.image, &faces);
                    let (z, zf) = mean_l(&r_0.image, &faces);
                    lr_l += (a - b).abs();
                    lr_z += (a - z).abs();
                    if let (Some(af), Some(bf), Some(zf)) = (af, bf, zf) {
                        lr_lf += (af - bf).abs();
                        lr_zf += (af - zf).abs();
                        lr_lf_n += 1;
                    }
                    if std::env::var_os("AUTO_EVAL_ROWS").is_some() {
                        println!(
                            "LR {:<14} faces {} sieve [{}] lr [{}] L* lr {:.1} sieve {:.1}",
                            stem,
                            faces.len(),
                            sv.iter().map(|v| format!("{v:.2}")).collect::<Vec<_>>().join(" "),
                            lr_vals.iter().map(|v| format!("{v:.2}")).collect::<Vec<_>>().join(" "),
                            a,
                            b
                        );
                    }
                }
                let cs = auto::skin_clipped_fraction(&cache, &src, &sieve_adj, Some(&faces)).ok().flatten();
                let cl = auto::skin_clipped_fraction(&cache, &src, &lr_adj, Some(&faces)).ok().flatten();
                if let (Some(cs), Some(cl)) = (cs, cl) {
                    lr_skin_n += 1;
                    lr_skin_sieve_sum += f64::from(cs);
                    lr_skin_lr_sum += f64::from(cl);
                    lr_skin_sieve_max = lr_skin_sieve_max.max(cs);
                    lr_skin_sieve_over += usize::from(cs > auto::SKIN_CLIP_MAX);
                    lr_skin_lr_over += usize::from(cl > auto::SKIN_CLIP_MAX);
                }
            }
            let defaults = ParametricAdjustments::defaults_for(fmt);
            if let Ok(w) = auto::auto_white_balance_opts(&cache, &src, &defaults, wb_opts) {
                lr_wb_n += 1;
                lr_wb_t += f64::from((w.temperature_k - g("ColorTemperature")).abs());
                lr_wb_n_t += f64::from((w.tint - g("Tint")).abs());
                if let Some(s) = cache.info(&src).ok().and_then(|i| i.as_shot) {
                    lr_asshot_t += f64::from((s.temperature_k - g("ColorTemperature")).abs());
                    lr_asshot_n += f64::from((s.tint - g("Tint")).abs());
                }
            }
        }
        cache.forget_sources(Some(&[src.id]));
    }
    if vs_lr.n > 0 {
        vs_lr.print("Sieve Auto vs Camera Raw Auto (defaults + Camera Raw's Auto WB; U = Camera Raw)");
        vs_lr_faces.print("  ... frames with faces");
        let n = vs_lr.n as f64;
        let fnn = lr_lf_n.max(1) as f64;
        println!(
            "Render mean |dL*| vs Camera Raw Auto (both rendered by Sieve): Sieve Auto global {:.2} / faces {:.2}; no Auto global {:.2} / faces {:.2} ({} with faces)",
            lr_l / n,
            lr_lf / fnn,
            lr_z / n,
            lr_zf / fnn,
            lr_lf_n
        );
        println!(
            "Skin clipping ({} frames): Sieve Auto mean {:.4}% max {:.3}% frames over {:.1}%: {}; Camera Raw Auto mean {:.4}% frames over: {}",
            lr_skin_n,
            100.0 * lr_skin_sieve_sum / lr_skin_n.max(1) as f64,
            100.0 * lr_skin_sieve_max,
            100.0 * auto::SKIN_CLIP_MAX,
            lr_skin_sieve_over,
            100.0 * lr_skin_lr_sum / lr_skin_n.max(1) as f64,
            lr_skin_lr_over
        );
        let w = lr_wb_n.max(1) as f64;
        println!(
            "Auto WB vs Camera Raw Auto WB ({} frames): Sieve |dTemp| {:.0} K |dTint| {:.1}; as-shot |dTemp| {:.0} K |dTint| {:.1}",
            lr_wb_n,
            lr_wb_t / w,
            lr_wb_n_t / w,
            lr_asshot_t / w,
            lr_asshot_n / w
        );
    }
    all.print("Auto vs the user's own values (all frames)");
    with_faces.print("Frames with detected faces");
    lr_auto.print("Frames where the user pressed Lightroom Auto (crs:AutoToneDigest)");
    let n = all.n.max(1) as f64;
    let fn_ = l_face_n.max(1) as f64;
    println!(
        "\nRender mean L* vs the user's render: Auto global {:.2} / faces {:.2}; no Auto (sliders 0) global {:.2} / faces {:.2} ({} frames with faces)",
        l_all_d / n,
        l_face_d / fn_,
        z_all_d / n,
        z_face_d / fn_,
        l_face_n
    );
    println!(
        "Mean render L*: user global {:.1} / faces {:.1}; Auto global {:.1} / faces {:.1}",
        u_all / n,
        u_face / fn_,
        a_all / n,
        a_face / fn_
    );
    for (a, u, s) in wb_rows.iter().take(12) {
        println!("   WB auto {a:.0} K  user {u:.0} K  as-shot {s:.0} K");
    }
    println!(
        "Skin clipping (share of skin pixels with a channel >= 250; {} frames with skin): auto mean {:.4}% max {:.3}%, frames over {:.1}%: auto {} / user {}; user mean {:.4}%",
        skin_frames,
        100.0 * auto_clip_sum / skin_frames.max(1) as f64,
        100.0 * auto_clip_max,
        100.0 * auto::SKIN_CLIP_MAX,
        auto_clip_frames,
        user_clip_frames,
        100.0 * user_clip_sum / skin_frames.max(1) as f64
    );
    let wn = wb_n.max(1) as f64;
    println!(
        "White balance vs user ({} frames): auto |dTemp| {:.0} K |dTint| {:.1}; as-shot |dTemp| {:.0} K |dTint| {:.1}",
        wb_n,
        wb_temp / wn,
        wb_tint / wn,
        wb_asshot_temp / wn,
        wb_asshot_tint / wn
    );
    times.sort_by(f64::total_cmp);
    if !times.is_empty() {
        println!("auto_tone time: p50 {:.0} ms, p95 {:.0} ms", times[times.len() / 2], times[times.len() * 95 / 100]);
    }
    let _ = Path::new("");
}
