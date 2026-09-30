//! Lightroom parity evaluation (Phase 7b): renders the user's edited frames with their
//! imported XMP settings through the shared develop pipeline and compares them with
//! Lightroom / Camera Raw renders.
//!
//! ```text
//! cargo run --release --example parity_eval -- [--list FILE | --folder DIR [--count N]]
//!     [--out DIR] [--size PX]
//! ```
//! - Sources are read in place and never written (RAW + `<stem>.xmp` sidecar).
//!   `--list` = one RAW path per line (default `../test-data/parity-list.txt`); `--folder`
//!   picks up to N edited RAWs (with a sidecar, no masks/retouch/upright) of each format.
//! - Output (default `../test-data/parity-out`): `<stem>.sieve.jpg` (the Sieve render,
//!   long edge `--size`, default 2048) and `<stem>.sheet.jpg` (camera JPEG | Sieve | reference).
//! - Reference: `SIEVE_LR_REFERENCE` = folder with Lightroom/Camera Raw renders named
//!   `<stem>.jpg` or `<stem>.ref.jpg` (as exported, or the full-size preview Adobe DNG
//!   Converter embeds in a DNG converted with the sidecar next to the RAW: those are stored
//!   without EXIF orientation, which is applied here when the reference is un-rotated).
//!   With a reference, reports CIEDE2000 mean / p95 per image (both images box-filtered
//!   3x3 after resizing the reference to the render's size, to discount resampling and
//!   sharpening differences), plus 16x16 block means (tone and colour only).

use std::path::{Path, PathBuf};
use std::time::Instant;

use sieve_lib::develop::{camera, pipeline, source};
use sieve_lib::ipc::types::{ImageFormat, ParametricAdjustments};
use sieve_lib::profiles::ProfileLibrary;
use sieve_lib::raw;
use sieve_lib::xmp;

struct Img {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

fn orient(img: &Img, o: u8) -> Img {
    if o <= 1 || o > 8 {
        return Img { w: img.w, h: img.h, px: img.px.clone() };
    }
    let (w, h) = (img.w, img.h);
    let (dw, dh) = if o >= 5 { (h, w) } else { (w, h) };
    let mut px = vec![0u8; dw * dh * 3];
    for y in 0..dh {
        for x in 0..dw {
            let (sx, sy) = match o {
                2 => (w - 1 - x, y),
                3 => (w - 1 - x, h - 1 - y),
                4 => (x, h - 1 - y),
                5 => (y, x),
                6 => (y, h - 1 - x),
                7 => (w - 1 - y, h - 1 - x),
                _ => (w - 1 - y, x),
            };
            px[(y * dw + x) * 3..(y * dw + x) * 3 + 3]
                .copy_from_slice(&img.px[(sy * w + sx) * 3..(sy * w + sx) * 3 + 3]);
        }
    }
    Img { w: dw, h: dh, px }
}

/// Area-average resize (downscaling) / nearest (upscaling).
fn resize(img: &Img, dw: usize, dh: usize) -> Img {
    let mut px = vec![0u8; dw * dh * 3];
    let (sx, sy) = (img.w as f64 / dw as f64, img.h as f64 / dh as f64);
    for y in 0..dh {
        let y0 = (y as f64 * sy) as usize;
        let y1 = (((y + 1) as f64 * sy) as usize).max(y0 + 1).min(img.h);
        for x in 0..dw {
            let x0 = (x as f64 * sx) as usize;
            let x1 = (((x + 1) as f64 * sx) as usize).max(x0 + 1).min(img.w);
            let mut acc = [0u32; 3];
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let p = &img.px[(yy * img.w + xx) * 3..];
                    for c in 0..3 {
                        acc[c] += u32::from(p[c]);
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            for c in 0..3 {
                px[(y * dw + x) * 3 + c] = ((acc[c] + n / 2) / n) as u8;
            }
        }
    }
    Img { w: dw, h: dh, px }
}

fn box3(img: &Img) -> Vec<[f32; 3]> {
    let (w, h) = (img.w, img.h);
    let mut out = vec![[0.0f32; 3]; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f32; 3];
            let mut n = 0.0;
            for yy in y.saturating_sub(1)..(y + 2).min(h) {
                for xx in x.saturating_sub(1)..(x + 2).min(w) {
                    let p = &img.px[(yy * w + xx) * 3..];
                    for c in 0..3 {
                        acc[c] += f32::from(p[c]);
                    }
                    n += 1.0;
                }
            }
            out[y * w + x] = acc.map(|v| v / n);
        }
    }
    out
}

/// sRGB (0..255) -> CIELAB (D65).
fn lab(rgb: [f32; 3]) -> [f64; 3] {
    let lin = rgb.map(|c| {
        let e = f64::from(c) / 255.0;
        if e <= 0.04045 {
            e / 12.92
        } else {
            ((e + 0.055) / 1.055).powf(2.4)
        }
    });
    let x = (0.4124564 * lin[0] + 0.3575761 * lin[1] + 0.1804375 * lin[2]) / 0.95047;
    let y = 0.2126729 * lin[0] + 0.7151522 * lin[1] + 0.0721750 * lin[2];
    let z = (0.0193339 * lin[0] + 0.1191920 * lin[1] + 0.9503041 * lin[2]) / 1.08883;
    let f = |t: f64| if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIEDE2000 colour difference.
fn de2000(l1: [f64; 3], l2: [f64; 3]) -> f64 {
    let (lp1, a1, b1) = (l1[0], l1[1], l1[2]);
    let (lp2, a2, b2) = (l2[0], l2[1], l2[2]);
    let c1 = (a1 * a1 + b1 * b1).sqrt();
    let c2 = (a2 * a2 + b2 * b2).sqrt();
    let cm = (c1 + c2) / 2.0;
    let g = 0.5 * (1.0 - (cm.powi(7) / (cm.powi(7) + 25f64.powi(7))).sqrt());
    let (a1p, a2p) = (a1 * (1.0 + g), a2 * (1.0 + g));
    let (c1p, c2p) = ((a1p * a1p + b1 * b1).sqrt(), (a2p * a2p + b2 * b2).sqrt());
    let h = |b: f64, a: f64| {
        let v = b.atan2(a).to_degrees();
        if v < 0.0 {
            v + 360.0
        } else {
            v
        }
    };
    let (h1p, h2p) = (h(b1, a1p), h(b2, a2p));
    let dl = lp2 - lp1;
    let dc = c2p - c1p;
    let mut dh = h2p - h1p;
    if c1p * c2p != 0.0 {
        if dh > 180.0 {
            dh -= 360.0;
        } else if dh < -180.0 {
            dh += 360.0;
        }
    } else {
        dh = 0.0;
    }
    let dhh = 2.0 * (c1p * c2p).sqrt() * (dh.to_radians() / 2.0).sin();
    let lm = (lp1 + lp2) / 2.0;
    let cmp = (c1p + c2p) / 2.0;
    let hm = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * (hm - 30.0).to_radians().cos()
        + 0.24 * (2.0 * hm).to_radians().cos()
        + 0.32 * (3.0 * hm + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hm - 63.0).to_radians().cos();
    let dtheta = 30.0 * (-((hm - 275.0) / 25.0).powi(2)).exp();
    let rc = 2.0 * (cmp.powi(7) / (cmp.powi(7) + 25f64.powi(7))).sqrt();
    let sl = 1.0 + 0.015 * (lm - 50.0).powi(2) / (20.0 + (lm - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * cmp;
    let sh = 1.0 + 0.015 * cmp * t;
    let rt = -(2.0 * dtheta).to_radians().sin() * rc;
    ((dl / sl).powi(2) + (dc / sc).powi(2) + (dhh / sh).powi(2) + rt * (dc / sc) * (dhh / sh)).sqrt()
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

fn load_jpeg(path: &Path) -> Option<Img> {
    let bytes = std::fs::read(path).ok()?;
    let d = raw::turbo::decode_rgb(&bytes, u32::MAX, u64::MAX).ok()?;
    Some(Img { w: d.width as usize, h: d.height as usize, px: d.pixels })
}

fn sheet(parts: &[&Img], height: usize) -> Img {
    let scaled: Vec<Img> = parts
        .iter()
        .map(|p| {
            let w = (p.w as f64 * height as f64 / p.h as f64).round().max(1.0) as usize;
            resize(p, w, height)
        })
        .collect();
    let gap = 8;
    let w: usize = scaled.iter().map(|s| s.w).sum::<usize>() + gap * (scaled.len() - 1);
    let mut px = vec![24u8; w * height * 3];
    let mut x0 = 0;
    for s in &scaled {
        for y in 0..height {
            px[(y * w + x0) * 3..(y * w + x0 + s.w) * 3].copy_from_slice(&s.px[y * s.w * 3..(y + 1) * s.w * 3]);
        }
        x0 += s.w + gap;
    }
    Img { w, h: height, px }
}

fn auto_pick(folder: &Path, count: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut raws: Vec<PathBuf> = walkdir::WalkDir::new(folder)
        .into_iter()
        .filter_map(Result::ok)
        .map(|e| e.path().to_path_buf())
        .filter(|p| raw::format_from_extension(p).is_some_and(|f| f.is_raw()))
        .collect();
    raws.sort();
    for fmt in [ImageFormat::Arw, ImageFormat::Cr3, ImageFormat::Raf] {
        let mut n = 0;
        for p in &raws {
            if n >= count || raw::format_from_extension(p) != Some(fmt) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(xmp::sidecar_path(p)) else { continue };
            let skip =
                ["MaskGroupBasedCorrections", "RetouchAreas", "PaintBasedCorrections"].iter().any(|k| text.contains(k))
                    || text.contains("crs:PerspectiveUpright=\"1\"");
            if !skip && text.contains("crs:Exposure2012") {
                out.push(p.clone());
                n += 1;
            }
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let out_dir = PathBuf::from(arg("--out").unwrap_or_else(|| "../test-data/parity-out".into()));
    let size: u32 = arg("--size").and_then(|v| v.parse().ok()).unwrap_or(2048);
    let files: Vec<PathBuf> = if let Some(folder) = arg("--folder") {
        auto_pick(Path::new(&folder), arg("--count").and_then(|v| v.parse().ok()).unwrap_or(7))
    } else {
        let list = arg("--list").unwrap_or_else(|| "../test-data/parity-list.txt".into());
        std::fs::read_to_string(&list)
            .unwrap_or_else(|e| panic!("{list}: {e}"))
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
            .collect()
    };
    let reference = std::env::var_os("SIEVE_LR_REFERENCE").map(PathBuf::from);
    std::fs::create_dir_all(&out_dir).expect("output dir");
    let lib = ProfileLibrary::shared();
    lib.warm();

    let mut summary: Vec<(String, f64, f64, f64, String)> = Vec::new();
    for (i, path) in files.iter().enumerate() {
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        let format = raw::format_from_extension(path).expect("supported file");
        // SIEVE_XMP_DIR: read `<dir>/<stem>.xmp` instead of the sidecar (settings variants
        // extracted from edited DNG copies; the sources stay untouched).
        let sidecar = match std::env::var_os("SIEVE_XMP_DIR") {
            Some(d) => PathBuf::from(d).join(format!("{stem}.xmp")),
            None => xmp::sidecar_path(path),
        };
        let (adj, note) = match std::fs::read_to_string(&sidecar) {
            Ok(text) => match xmp::packet::parse_for(&text, format) {
                Ok(v) => match v.develop {
                    Some(a) => {
                        let w: Vec<String> = v.warnings.iter().map(|w| w.code.as_str().to_owned()).collect();
                        (a, w.join(","))
                    }
                    None => (ParametricAdjustments::defaults_for(format), "no develop settings".into()),
                },
                Err(e) => (ParametricAdjustments::defaults_for(format), format!("xmp error: {e}")),
            },
            Err(_) => (ParametricAdjustments::defaults_for(format), "no sidecar".into()),
        };
        // Debugging aid: PARITY_OVERRIDE="shadows=0,clarity=0,..." overrides basic sliders.
        let mut adj = adj;
        if let Ok(ov) = std::env::var("PARITY_OVERRIDE") {
            for kv in ov.split(',').filter(|s| !s.is_empty()) {
                let (k, v) = kv.split_once('=').expect("key=value");
                let v: f32 = v.parse().expect("number");
                match k {
                    "exposure" => adj.exposure = v,
                    "contrast" => adj.contrast = v,
                    "highlights" => adj.highlights = v,
                    "shadows" => adj.shadows = v,
                    "whites" => adj.whites = v,
                    "blacks" => adj.blacks = v,
                    "clarity" => adj.clarity = v,
                    "vibrance" => adj.vibrance = v,
                    "lnr" => adj.detail.noise_reduction.luminance = v,
                    "cnr" => adj.detail.noise_reduction.color = v,
                    "sharp" => adj.detail.sharpening.amount = v,
                    "curve0" => {
                        let splits = adj.tone_curve.parametric;
                        adj.tone_curve.parametric = sieve_lib::ipc::types::ParametricCurve {
                            shadows: 0.0,
                            darks: 0.0,
                            lights: 0.0,
                            highlights: 0.0,
                            ..splits
                        };
                        adj.tone_curve.point.master = sieve_lib::ipc::types::PointCurves::identity_curve();
                    }
                    _ => panic!("unknown override {k}"),
                }
            }
        }
        let mut buf = Vec::new();
        let ex = raw::extract(path, format, &mut buf).expect("extract");
        let o = ex.meta.orientation.map(|o| o as u8).unwrap_or(1);
        let t = Instant::now();
        let (img, meta) = source::decode_half_size_meta(path).expect("decode");
        let profile = camera::resolve(&meta, &adj.profile, &lib, Some(&sidecar));
        let prep = source::prepare(&img, o, &adj.crop, None, size);
        let tone = pipeline::tone_context(&img, o, &adj, &profile);
        let input = pipeline::RenderInput {
            width: prep.width,
            height: prep.height,
            pixels: &prep.pixels,
            color: &img.color,
            frame_long_edge: prep.frame_long_edge,
            view: prep.view,
            profile: &profile,
            seed: i as u64 + 1,
            quality: pipeline::Quality::Preview,
            tone: Some(&tone),
        };
        let t_render = Instant::now();
        let out = pipeline::render(&input, &adj, None);
        let render_ms = t_render.elapsed().as_secs_f64() * 1000.0;
        let jpeg = raw::turbo::encode_rgb_444(&out.rgb, out.width, out.height, 92).unwrap();
        std::fs::write(out_dir.join(format!("{stem}.sieve.jpg")), &jpeg).unwrap();
        let sieve = Img { w: out.width as usize, h: out.height as usize, px: out.rgb };
        let warn: Vec<&str> = profile.warnings.iter().map(|w| w.code.as_str()).collect();
        let look = adj.profile.look.as_ref().map_or("-".to_owned(), |l| l.name.clone());
        let mut line = format!(
            "{stem}: {}x{} base {:.2} cc {:.4?} profile {:?} look {look} dcp {} {} | total {:.0} ms render {render_ms:.0} ms {}{}",
            out.width,
            out.height,
            profile.baseline_ev,
            img.color.calibration,
            adj.profile.camera_profile.as_deref().unwrap_or("-"),
            profile.dcp.is_some(),
            if adj.crop.enabled { format!("crop {:.2}deg", adj.crop.angle) } else { String::new() },
            t.elapsed().as_secs_f64() * 1000.0,
            if warn.is_empty() { String::new() } else { format!(" warnings {warn:?}") },
            if note.is_empty() { String::new() } else { format!(" [{note}]") },
        );

        let camera_jpeg = match &ex.preview {
            Ok(raw::Preview::Embedded) => raw::turbo::decode_rgb(&buf, 1024, u64::MAX)
                .ok()
                .map(|d| orient(&Img { w: d.width as usize, h: d.height as usize, px: d.pixels }, o)),
            _ => None,
        };
        let refimg = reference.as_ref().and_then(|dir| {
            let r = load_jpeg(&dir.join(format!("{stem}.ref.jpg")))
                .or_else(|| load_jpeg(&dir.join(format!("{stem}.jpg"))))?;
            // Un-rotated reference (DNG preview): apply the orientation when the aspect says so.
            let same = (r.w >= r.h) == (sieve.w >= sieve.h);
            Some(if same { r } else { orient(&r, o) })
        });
        if let Some(r) = &refimg {
            let r = resize(r, sieve.w, sieve.h);
            if std::env::var_os("SIEVE_DUMP_EV").is_some() {
                // Scene log2 luminance before the local operators (f32 LE, render size).
                let ev = pipeline::scene_log_luminance(&input, &adj);
                let bytes: Vec<u8> = ev.iter().flat_map(|v| v.to_le_bytes()).collect();
                std::fs::write(out_dir.join(format!("{stem}.ev.f32")), bytes).unwrap();
                // Adaptation bases (fine, coarse) of the uncropped context at each pixel.
                let (w, h) = (input.width as usize, input.height as usize);
                let mut m1 = Vec::with_capacity(w * h * 4);
                let mut m2 = Vec::with_capacity(w * h * 4);
                for y in 0..h {
                    for x in 0..w {
                        let (f, c) = tone.bases((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                        m1.extend_from_slice(&f.to_le_bytes());
                        m2.extend_from_slice(&c.to_le_bytes());
                    }
                }
                std::fs::write(out_dir.join(format!("{stem}.m1.f32")), m1).unwrap();
                std::fs::write(out_dir.join(format!("{stem}.m2.f32")), m2).unwrap();
                let st = tone.stats;
                std::fs::write(
                    out_dir.join(format!("{stem}.stats.txt")),
                    format!(
                        "{} {} {} {} {} {} {}\n",
                        st.key,
                        st.p50,
                        st.p90,
                        st.p95,
                        st.p99,
                        st.white,
                        adj.exposure + profile.baseline_ev
                    ),
                )
                .unwrap();
            }
            if std::env::var_os("SIEVE_DUMP").is_some() {
                // Lossless copies for offline analysis (tools/acr-oracle).
                for (img, tag) in [(&r, "ref"), (&sieve, "sieve")] {
                    let mut ppm = format!("P6\n{} {}\n255\n", img.w, img.h).into_bytes();
                    ppm.extend_from_slice(&img.px);
                    std::fs::write(out_dir.join(format!("{stem}.{tag}.ppm")), ppm).unwrap();
                }
            }
            let (a, b) = (box3(&sieve), box3(&r));
            let mut d: Vec<f64> = a.iter().zip(&b).step_by(3).map(|(p, q)| de2000(lab(*p), lab(*q))).collect();
            let mean = d.iter().sum::<f64>() / d.len() as f64;
            let p95 = pct(&mut d, 0.95);
            // 16x16 block means.
            const B: usize = 16;
            let mut bd = Vec::new();
            for by in 0..sieve.h / B {
                for bx in 0..sieve.w / B {
                    let mean_of = |v: &[[f32; 3]]| {
                        let mut s = [0.0f32; 3];
                        for y in by * B..by * B + B {
                            for x in bx * B..bx * B + B {
                                let p = v[y * sieve.w + x];
                                for c in 0..3 {
                                    s[c] += p[c];
                                }
                            }
                        }
                        s.map(|c| c / (B * B) as f32)
                    };
                    bd.push(de2000(lab(mean_of(&a)), lab(mean_of(&b))));
                }
            }
            let bmean = bd.iter().sum::<f64>() / bd.len() as f64;
            line.push_str(&format!(" | dE2000 mean {mean:.2} p95 {p95:.2} | blocks mean {bmean:.2}"));
            summary.push((stem.clone(), mean, p95, bmean, format!("{format:?}")));
            let mut parts: Vec<&Img> = Vec::new();
            if let Some(c) = &camera_jpeg {
                parts.push(c);
            }
            parts.push(&sieve);
            parts.push(&r);
            let s = sheet(&parts, 900);
            let j = raw::turbo::encode_rgb_444(&s.px, s.w as u32, s.h as u32, 88).unwrap();
            std::fs::write(out_dir.join(format!("{stem}.sheet.jpg")), j).unwrap();
        } else {
            let mut parts: Vec<&Img> = Vec::new();
            if let Some(c) = &camera_jpeg {
                parts.push(c);
            }
            parts.push(&sieve);
            let s = sheet(&parts, 900);
            let j = raw::turbo::encode_rgb_444(&s.px, s.w as u32, s.h as u32, 88).unwrap();
            std::fs::write(out_dir.join(format!("{stem}.sheet.jpg")), j).unwrap();
        }
        println!("{line}");
    }
    if !summary.is_empty() {
        let n = summary.len() as f64;
        let mean = summary.iter().map(|s| s.1).sum::<f64>() / n;
        let p95 = summary.iter().map(|s| s.2).sum::<f64>() / n;
        let blocks = summary.iter().map(|s| s.3).sum::<f64>() / n;
        println!(
            "\n== {} images: dE2000 mean of means {mean:.2}, mean p95 {p95:.2}, block mean {blocks:.2} ==",
            summary.len()
        );
        for fmt in ["Arw", "Cr3", "Raf"] {
            let v: Vec<_> = summary.iter().filter(|s| s.4 == fmt).collect();
            if !v.is_empty() {
                let m = v.iter().map(|s| s.1).sum::<f64>() / v.len() as f64;
                println!("  {fmt}: {} images, mean {m:.2}", v.len());
            }
        }
    }
}
