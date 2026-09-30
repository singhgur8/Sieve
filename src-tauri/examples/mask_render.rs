//! Mask rendering check (Phase 7c): renders the user's Lightroom-masked frames (read in
//! place, never written) with and without their masks through the shared pipeline, writes
//! `<stem>.mask.jpg`, `<stem>.nomask.jpg` and `<stem>.sheet.jpg` (no masks | masks | mask
//! weight | amplified difference | Camera Raw reference) and reports CIEDE2000 statistics.
//!
//! ```text
//! cargo run --release --example mask_render -- --out DIR [--folder DIR] [--size PX]
//!     [--limit N] [--reference DIR] [--reference-nomask DIR] [--only-ref] [--xmp-dir DIR]
//! ```
//! `--reference` = folder with `<stem>.ref.jpg` Camera Raw renders (the full-size preview of
//! a DNG made by Adobe DNG Converter from the RAW + sidecar, `tools/acr-oracle/dng_list.sh`).
//! Lightroom mattes are decoded from the sidecar tables (no catalog needed).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use sieve_lib::develop::masks::{evaluate, AlphaMask, LocalPlanes, MaskGeometry, MatteSource};
use sieve_lib::develop::{camera, pipeline, source};
use sieve_lib::ipc::types::{AiMask, ParametricAdjustments};
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

/// In-memory mattes (the sidecar's decoded Lightroom tables).
struct Mattes(HashMap<String, Arc<AlphaMask>>);

impl MatteSource for Mattes {
    fn matte(&self, _: i64, ai: &AiMask) -> Option<Arc<AlphaMask>> {
        self.0.get(ai.digest.as_deref()?).cloned()
    }
}

fn to_img(r: &pipeline::RenderedImage) -> Img {
    Img { w: r.width as usize, h: r.height as usize, px: r.rgb.clone() }
}

/// CIEDE2000 mean / p95 of `a` vs `b` (same size) over pixels where `sel` holds.
fn delta_e(a: &Img, b: &Img, sel: &dyn Fn(usize) -> bool) -> (f64, f64, usize) {
    let (pa, pb) = (box3(a), box3(b));
    let mut d: Vec<f64> =
        (0..pa.len()).step_by(3).filter(|&i| sel(i)).map(|i| de2000(lab(pa[i]), lab(pb[i]))).collect();
    if d.is_empty() {
        return (0.0, 0.0, 0);
    }
    let n = d.len();
    let mean = d.iter().sum::<f64>() / n as f64;
    (mean, pct(&mut d, 0.95), n)
}

/// Mean Lab of `a` minus mean Lab of `b` over `sel`, and the ratio of their local contrast
/// (std of L* minus its 9x9 box mean) over `sel`.
fn local_effect(a: &Img, b: &Img, sel: &dyn Fn(usize) -> bool) -> ([f64; 3], f64) {
    let lab_of = |im: &Img| -> Vec<[f64; 3]> {
        im.px.chunks(3).map(|p| lab([f32::from(p[0]), f32::from(p[1]), f32::from(p[2])])).collect()
    };
    let (la, lb) = (lab_of(a), lab_of(b));
    let (w, h) = (a.w, a.h);
    let hp = |l: &[[f64; 3]], i: usize| {
        let (x, y) = (i % w, i / w);
        let (mut s, mut n) = (0.0, 0.0);
        for yy in y.saturating_sub(4)..(y + 5).min(h) {
            for xx in x.saturating_sub(4)..(x + 5).min(w) {
                s += l[yy * w + xx][0];
                n += 1.0;
            }
        }
        l[i][0] - s / n
    };
    let (mut d, mut n) = ([0.0f64; 3], 0.0f64);
    let (mut va, mut vb) = (0.0f64, 0.0f64);
    for i in (0..la.len()).step_by(7).filter(|&i| sel(i)) {
        for c in 0..3 {
            d[c] += la[i][c] - lb[i][c];
        }
        va += hp(&la, i).powi(2);
        vb += hp(&lb, i).powi(2);
        n += 1.0;
    }
    (d.map(|v| v / n.max(1.0)), (va / vb.max(1e-9)).sqrt())
}

/// Mean Lab distance, over `sel`, between the local effects `a - a0` (Sieve) and `b - b0`
/// (Camera Raw): how well the masks' effect matches, independent of the global difference.
fn effect_error(a: &Img, a0: &Img, b: &Img, b0: &Img, sel: &dyn Fn(usize) -> bool) -> f64 {
    let (pa, pa0, pb, pb0) = (box3(a), box3(a0), box3(b), box3(b0));
    let (mut s, mut n) = (0.0f64, 0.0f64);
    for i in (0..pa.len()).step_by(3).filter(|&i| sel(i)) {
        let (la, la0, lb, lb0) = (lab(pa[i]), lab(pa0[i]), lab(pb[i]), lab(pb0[i]));
        let d: f64 = (0..3).map(|c| ((la[c] - la0[c]) - (lb[c] - lb0[c])).powi(2)).sum();
        s += d.sqrt();
        n += 1.0;
    }
    s / n.max(1.0)
}

/// (ACR in-mask dLab, ACR contrast ratio, Sieve dLab, Sieve contrast ratio).
type Effect = ([f64; 3], f64, [f64; 3], f64);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let folder =
        PathBuf::from(arg("--folder").unwrap_or_else(|| "/Users/gurjotsingh/Pictures/Jasmit Natalie Proposal".into()));
    let out_dir = PathBuf::from(arg("--out").expect("--out DIR"));
    let size: u32 = arg("--size").and_then(|v| v.parse().ok()).unwrap_or(2048);
    let limit: usize = arg("--limit").and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
    let reference = arg("--reference").map(PathBuf::from);
    let reference_nomask = arg("--reference-nomask").map(PathBuf::from);
    // Only frames with a Camera Raw reference (fast calibration loops).
    let only_ref = args.iter().any(|a| a == "--only-ref");
    // Sidecars from this folder (`<stem>.xmp`, e.g. `local_variants` output) instead of the
    // RAWs' own.
    let xmp_dir = arg("--xmp-dir").map(PathBuf::from);
    let mut effects = Vec::new();
    let mut floors: Vec<f64> = Vec::new();
    let mut eff_errs: Vec<f64> = Vec::new();
    std::fs::create_dir_all(&out_dir).expect("output dir");
    let lib = ProfileLibrary::shared();
    lib.warm();

    let mut raws: Vec<PathBuf> = walkdir::WalkDir::new(&folder)
        .into_iter()
        .filter_map(Result::ok)
        .map(|e| e.path().to_path_buf())
        .filter(|p| raw::format_from_extension(p).is_some_and(|f| f.is_raw()))
        .collect();
    raws.sort();
    let mut rows = Vec::new();
    let mut n = 0;
    for path in &raws {
        if n >= limit {
            break;
        }
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        let sidecar = match &xmp_dir {
            Some(d) => d.join(format!("{stem}.xmp")),
            None => xmp::sidecar_path(path),
        };
        let Ok(text) = std::fs::read_to_string(&sidecar) else { continue };
        let Ok(Some(read)) = xmp::masks::read(&text) else { continue };
        if read.groups.is_empty() {
            continue;
        }
        if only_ref && !reference.as_ref().is_some_and(|d| d.join(format!("{stem}.ref.jpg")).is_file()) {
            continue;
        }
        n += 1;
        let format = raw::format_from_extension(path).unwrap();
        let mut adj = xmp::packet::parse_for(&text, format)
            .ok()
            .and_then(|v| v.develop)
            .unwrap_or_else(|| ParametricAdjustments::defaults_for(format));
        adj.masks = read.groups.clone();
        let mut mattes = HashMap::new();
        for m in &read.mattes {
            match xmp::masks::decode_matte(m) {
                Ok(a) => {
                    mattes.insert(m.digest.clone(), Arc::new(a));
                }
                Err(e) => eprintln!("{stem}: matte {}: {e}", m.digest),
            }
        }
        let mattes = Mattes(mattes);
        let mut buf = Vec::new();
        let ex = raw::extract(path, format, &mut buf).expect("extract");
        let o = ex.meta.orientation.map(|o| o as u8).unwrap_or(1);
        let (img, meta) = source::decode_half_size_meta(path).expect("decode");
        let profile = camera::resolve(&meta, &adj.profile, &lib, Some(&sidecar));
        let prep = source::prepare(&img, o, &adj.crop, None, size);
        // Whole-frame local tone context, as the editor and export compute it.
        let tone = pipeline::tone_context(&img, o, &adj, &profile);
        let input = pipeline::RenderInput {
            frame_long_edge: prep.frame_long_edge,
            view: prep.view,
            seed: n as u64,
            quality: pipeline::Quality::Preview,
            tone: Some(&tone),
            ..pipeline::RenderInput::simple(prep.width, prep.height, &prep.pixels, &img.color, &profile)
        };
        let geom = MaskGeometry {
            sensor_width: img.full_width,
            sensor_height: img.full_height,
            orientation: o,
            crop: adj.crop,
            region: None,
            width: prep.width,
            height: prep.height,
        };
        let t = Instant::now();
        let weights = evaluate(&adj.masks, &geom, 1, &mattes, None);
        let eval_ms = t.elapsed().as_secs_f64() * 1000.0;
        let planes = LocalPlanes::build(&adj.masks, &weights);
        let t = Instant::now();
        let with = pipeline::render_masked(&input, &adj, None, planes.as_ref());
        let masked_ms = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let without = pipeline::render(&input, &adj, None);
        let plain_ms = t.elapsed().as_secs_f64() * 1000.0;
        // Combined mask weight (max over groups) for the overlay and the masked-region stats.
        let npx = (prep.width * prep.height) as usize;
        let mut wmax = vec![0.0f32; npx];
        for g in weights.groups.iter().flatten() {
            for (o, v) in wmax.iter_mut().zip(g) {
                *o = o.max(*v);
            }
        }
        let coverage = wmax.iter().map(|v| v.min(1.0)).sum::<f32>() / npx as f32;
        let (wi, wo) = (to_img(&with), to_img(&without));
        let enc = |im: &Img| raw::turbo::encode_rgb_444(&im.px, im.w as u32, im.h as u32, 92).unwrap();
        std::fs::write(out_dir.join(format!("{stem}.mask.jpg")), enc(&wi)).unwrap();
        std::fs::write(out_dir.join(format!("{stem}.nomask.jpg")), enc(&wo)).unwrap();
        let matte_px: Vec<u8> = wmax.iter().flat_map(|v| [(v.min(1.0) * 255.0) as u8; 3]).collect();
        let matte = Img { w: wi.w, h: wi.h, px: matte_px };
        // Amplified difference (x4 around mid grey) shows where the masks acted.
        let diff = Img {
            w: wi.w,
            h: wi.h,
            px: wi
                .px
                .iter()
                .zip(&wo.px)
                .map(|(a, b)| (128 + (i32::from(*a) - i32::from(*b)) * 4).clamp(0, 255) as u8)
                .collect(),
        };
        let inside = |i: usize| wmax[i] > 0.5;
        let outside = |i: usize| wmax[i] < 0.02;
        let (dm_in, _, _) = delta_e(&wi, &wo, &inside);
        let (dm_out, _, _) = delta_e(&wi, &wo, &outside);
        let refimg = reference.as_ref().and_then(|dir| {
            let r = load_jpeg(&dir.join(format!("{stem}.ref.jpg")))?;
            let same = (r.w >= r.h) == (wi.w >= wi.h);
            Some(resize(&if same { r } else { orient(&r, o) }, wi.w, wi.h))
        });
        let mut parts: Vec<&Img> = vec![&wo, &wi, &matte, &diff];
        if let Some(r) = &refimg {
            parts.push(r);
        }
        let sh = sheet(&parts, 480);
        std::fs::write(out_dir.join(format!("{stem}.sheet.jpg")), enc(&sh)).unwrap();
        let g = &adj.masks[0].adjustments;
        let mut line = format!(
            "{stem}: o{o} groups {} cov {:.2} amount {:.2} clar {:+.0} tex {:+.0} temp {:+.0} contr {:+.0} exp {:+.2} | \
             eval {eval_ms:.0} ms render {masked_ms:.0}/{plain_ms:.0} ms | mask vs nomask dE in {dm_in:.2} out {dm_out:.3}",
            adj.masks.len(),
            coverage,
            adj.masks[0].amount,
            g.clarity,
            g.texture,
            g.temperature,
            g.contrast,
            g.exposure,
        );
        if let Some(r) = &refimg {
            let all = |_: usize| true;
            let (m_all, p_all, _) = delta_e(&wi, r, &all);
            let (n_all, q_all, _) = delta_e(&wo, r, &all);
            let (m_in, _, k) = delta_e(&wi, r, &inside);
            let (n_in, _, _) = delta_e(&wo, r, &inside);
            line += &format!(
                " | vs ACR: mask {m_all:.2}/{p_all:.2} nomask {n_all:.2}/{q_all:.2} (mean/p95); in-mask {m_in:.2} vs {n_in:.2} ({k} px)"
            );
            rows.push((m_all, n_all, m_in, n_in));
            let r0 = reference_nomask.as_ref().and_then(|dir| {
                let r = load_jpeg(&dir.join(format!("{stem}.ref.jpg")))?;
                let same = (r.w >= r.h) == (wi.w >= wi.h);
                Some(resize(&if same { r } else { orient(&r, o) }, wi.w, wi.h))
            });
            if let Some(r0) = &r0 {
                let (floor, _, _) = delta_e(&wo, r0, &inside);
                let eff = effect_error(&wi, &wo, r, r0, &inside);
                line += &format!(" | in-mask floor (no masks both) {floor:.2} effect error {eff:.2}");
                floors.push(floor);
                eff_errs.push(eff);
                let (acr, acr_c) = local_effect(r, r0, &inside);
                let (sv, sv_c) = local_effect(&wi, &wo, &inside);
                line += &format!(
                    " | in-mask effect ACR dLab [{:+.2} {:+.2} {:+.2}] contrast x{acr_c:.3}; Sieve [{:+.2} {:+.2} {:+.2}] x{sv_c:.3}",
                    acr[0], acr[1], acr[2], sv[0], sv[1], sv[2]
                );
                effects.push((acr, acr_c, sv, sv_c));
            }
        }
        println!("{line}");
    }
    if !rows.is_empty() {
        let k = rows.len() as f64;
        let avg = |f: fn(&(f64, f64, f64, f64)) -> f64| rows.iter().map(f).sum::<f64>() / k;
        println!(
            "SUMMARY {} frames vs ACR: dE mean with masks {:.2}, without {:.2}; in-mask {:.2} vs {:.2}",
            rows.len(),
            avg(|r| r.0),
            avg(|r| r.1),
            avg(|r| r.2),
            avg(|r| r.3)
        );
    }
    if !floors.is_empty() {
        let k = floors.len() as f64;
        println!(
            "FLOOR in-mask dE with no masks on either side: {:.2}; local effect error (Lab) {:.3}",
            floors.iter().sum::<f64>() / k,
            eff_errs.iter().sum::<f64>() / k
        );
    }
    if !effects.is_empty() {
        let k = effects.len() as f64;
        let m = |f: &dyn Fn(&Effect) -> f64| effects.iter().map(f).sum::<f64>() / k;
        println!(
            "EFFECT {} frames, in-mask mean: ACR dL {:+.2} da {:+.2} db {:+.2} contrast x{:.3} | Sieve dL {:+.2} da {:+.2} db {:+.2} contrast x{:.3}",
            effects.len(),
            m(&|e| e.0[0]),
            m(&|e| e.0[1]),
            m(&|e| e.0[2]),
            m(&|e| e.1),
            m(&|e| e.2[0]),
            m(&|e| e.2[1]),
            m(&|e| e.2[2]),
            m(&|e| e.3)
        );
    }
    println!("{n} masked frames rendered into {}", out_dir.display());
}
