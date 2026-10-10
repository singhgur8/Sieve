//! Upright evaluation (Phase 8d): runs `ml::upright` on real frames (read in place, never
//! written), reports per-mode rotation / timings, compares Level with Lightroom's own
//! Upright solve when the sidecar has one (`crs:UprightTransform_3` = Level), and writes
//! before / after PNGs for a visual check.
//!
//! ```text
//! cargo run --release --example upright_eval -- [--out DIR] [--list FILE_OF_PATHS] FILE...
//! ```
//! Each PNG: top-left the render with the detected segments (red = near-vertical, green =
//! other), then Level, Vertical and Auto (512 px each).

use std::path::PathBuf;
use std::time::Instant;

use sieve_lib::develop::parity::orientation_map;
use sieve_lib::develop::{DevelopCache, DevelopConfig, SourceImage};
use sieve_lib::ipc::types::{ParametricAdjustments, UprightMode};
use sieve_lib::lut::LutLibrary;
use sieve_lib::ml::upright;
use sieve_lib::raw;

fn lr_transforms(xmp: &str) -> Vec<(usize, [f64; 9])> {
    let mut out = Vec::new();
    for i in 0..6 {
        let key = format!("crs:UprightTransform_{i}=\"");
        if let Some(p) = xmp.find(&key) {
            let rest = &xmp[p + key.len()..];
            let end = rest.find('"').unwrap_or(0);
            let v: Vec<f64> = rest[..end].split(',').filter_map(|s| s.trim().parse().ok()).collect();
            if v.len() == 9 {
                let mut m = [0.0; 9];
                m.copy_from_slice(&v);
                out.push((i, m));
            }
        }
    }
    out
}

fn sample(img: &[u8], w: u32, h: u32, x: f64, y: f64) -> [u8; 3] {
    if x < 0.0 || y < 0.0 || x > f64::from(w - 1) || y > f64::from(h - 1) {
        return [40, 40, 40];
    }
    let (x0, y0) = (x.floor() as u32, y.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - f64::from(x0), y - f64::from(y0));
    let px = |xx: u32, yy: u32, c: usize| f64::from(img[((yy * w + xx) * 3) as usize + c]);
    let mut o = [0u8; 3];
    for (c, v) in o.iter_mut().enumerate() {
        let a = px(x0, y0, c) * (1.0 - fx) + px(x1, y0, c) * fx;
        let b = px(x0, y1, c) * (1.0 - fx) + px(x1, y1, c) * fx;
        *v = (a * (1.0 - fy) + b * fy).round() as u8;
    }
    o
}

/// Corrected oriented image from a sensor matrix (corrected -> source).
fn warp(img: &[u8], w: u32, h: u32, o: u8, m: &[f64]) -> Vec<u8> {
    let om = orientation_map(o);
    // Sensor normalized -> oriented normalized (invert the affine).
    let det = om[0] * om[4] - om[1] * om[3];
    let inv = |sx: f64, sy: f64| {
        let (dx, dy) = (sx - om[2], sy - om[5]);
        ((om[4] * dx - om[1] * dy) / det, (-om[3] * dx + om[0] * dy) / det)
    };
    let mut out = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = ((f64::from(x) + 0.5) / f64::from(w), (f64::from(y) + 0.5) / f64::from(h));
            let (su, sv) = (om[0] * u + om[1] * v + om[2], om[3] * u + om[4] * v + om[5]);
            let (tu, tv) = upright::map_point(m, su, sv);
            let (ou, ov) = inv(tu, tv);
            out.extend_from_slice(&sample(img, w, h, ou * f64::from(w) - 0.5, ov * f64::from(h) - 0.5));
        }
    }
    // Light grid to judge verticals.
    for gx in 1..8 {
        let x = gx * w / 8;
        for y in 0..h {
            let i = ((y * w + x) * 3) as usize;
            out[i] = out[i].saturating_add(60);
            out[i + 1] = out[i + 1].saturating_add(60);
        }
    }
    out
}

fn draw_segments(img: &mut [u8], w: u32, h: u32, segs: &[upright::Segment]) {
    for s in segs {
        let vertical = 90.0 - s.angle_deg().abs() <= 35.0;
        let col = if vertical { [255, 0, 0] } else { [0, 255, 0] };
        let n = s.length().ceil() as usize + 1;
        for k in 0..=n {
            let t = k as f64 / n as f64;
            let (x, y) = (s.x1 + (s.x2 - s.x1) * t, s.y1 + (s.y2 - s.y1) * t);
            if x >= 0.0 && y >= 0.0 && (x as u32) < w && (y as u32) < h {
                let i = (((y as u32) * w + x as u32) * 3) as usize;
                img[i..i + 3].copy_from_slice(&col);
            }
        }
    }
}

fn downscale(img: &[u8], w: u32, h: u32, f: u32) -> (Vec<u8>, u32, u32) {
    let (dw, dh) = (w / f, h / f);
    let mut out = Vec::with_capacity((dw * dh * 3) as usize);
    for y in 0..dh {
        for x in 0..dw {
            for c in 0..3 {
                let mut s = 0u32;
                for yy in 0..f {
                    for xx in 0..f {
                        s += u32::from(img[(((y * f + yy) * w + x * f + xx) * 3) as usize + c]);
                    }
                }
                out.push((s / (f * f)) as u8);
            }
        }
    }
    (out, dw, dh)
}

fn write_png(path: &std::path::Path, tiles: &[(Vec<u8>, u32, u32)]) {
    let (tw, th) = (tiles[0].1, tiles[0].2);
    let (cw, ch) = if tiles.len() == 1 { (tw, th) } else { (tw * 2, th * 2) };
    let mut canvas = vec![0u8; (cw * ch * 3) as usize];
    for (i, (img, w, h)) in tiles.iter().enumerate() {
        let (ox, oy) = ((i as u32 % 2) * tw, (i as u32 / 2) * th);
        for y in 0..(*h).min(th) {
            for x in 0..(*w).min(tw) {
                let s = ((y * w + x) * 3) as usize;
                let d = (((oy + y) * cw + ox + x) * 3) as usize;
                canvas[d..d + 3].copy_from_slice(&img[s..s + 3]);
            }
        }
    }
    let f = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), cw, ch);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&canvas).unwrap();
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    if v.is_empty() {
        return 0.0;
    }
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut out_dir: Option<PathBuf> = None;
    let mut files = Vec::new();
    while let Some(a) = args.next() {
        if a == "--out" {
            out_dir = args.next().map(PathBuf::from);
        } else if a == "--list" {
            let list = std::fs::read_to_string(args.next().expect("--list FILE")).unwrap();
            files.extend(list.lines().filter(|l| !l.trim().is_empty()).map(PathBuf::from));
        } else {
            files.push(PathBuf::from(a));
        }
    }
    if let Some(d) = &out_dir {
        std::fs::create_dir_all(d).unwrap();
    }
    let cache = DevelopCache::new(DevelopConfig { cache_bytes: 1 << 30, mask_cache: None });
    let luts = LutLibrary::new(std::env::temp_dir().join("sieve-upright-eval-luts"));
    let (mut t_render, mut t_solve) = (Vec::new(), Vec::new());
    println!(
        "{:<14} {:>5} {:>8} {:>8} {:>8} {:>8} {:>8}  messages",
        "file", "segs", "level", "LR lvl", "vert", "auto", "full"
    );
    for (i, path) in files.iter().enumerate() {
        let Some(fmt) = raw::format_from_extension(path) else { continue };
        let mut buf = Vec::new();
        let orientation = raw::extract(path, fmt, &mut buf).ok().and_then(|e| e.meta.orientation).map(|o| o as u8);
        let src = SourceImage { id: i as i64 + 1, path: path.clone(), orientation };
        let o = orientation.unwrap_or(1);
        let adj = upright::detection_adjustments(&ParametricAdjustments::defaults_for(fmt));
        // Warm the decode (the app keeps it cached while editing), then time a render.
        if cache.render_image(&src, &adj, None, 256, &luts).is_err() {
            println!("{}: decode failed", path.display());
            continue;
        }
        let t0 = Instant::now();
        let r = cache.render_image(&src, &adj, None, upright::DETECT_EDGE, &luts).unwrap().image;
        t_render.push(t0.elapsed().as_secs_f64() * 1e3);
        let mut res = Vec::new();
        for mode in [UprightMode::Level, UprightMode::Vertical, UprightMode::Auto, UprightMode::Full] {
            let t = Instant::now();
            let out = upright::solve_rgb8(&r.rgb, r.width, r.height, o, mode, &[], None);
            t_solve.push(t.elapsed().as_secs_f64() * 1e3);
            res.push(out);
        }
        println!("    {}", upright::diagnose(&r.rgb, r.width, r.height, None));
        let xmp = std::fs::read_to_string(path.with_extension("xmp")).unwrap_or_default();
        let lr = lr_transforms(&xmp);
        // Sensor-frame pixel-space rotation of a normalized sensor matrix.
        let (sw, sh) = if o >= 5 { (r.height, r.width) } else { (r.width, r.height) };
        let aspect = f64::from(sh) / f64::from(sw);
        let px_rot = |m: &[f64]| (m[3] * aspect).atan2(m[0]).to_degrees();
        let lr_level = lr.iter().find(|(k, _)| *k == 3).map(|(_, m)| px_rot(m));
        let fmt_rot = |o: &upright::UprightOutcome| {
            o.solution.as_ref().map_or("-".to_owned(), |s| format!("{:+.2}", s.rotation_deg))
        };
        let msgs: Vec<String> = res.iter().filter_map(|o| o.message.clone()).collect();
        println!(
            "{:<14} {:>5} {:>8} {:>8} {:>8} {:>8} {:>8}  {}",
            path.file_name().unwrap().to_string_lossy(),
            res[0].segments,
            fmt_rot(&res[0]),
            lr_level.map_or("-".to_owned(), |a| format!("{a:+.2}")),
            fmt_rot(&res[1]),
            fmt_rot(&res[2]),
            fmt_rot(&res[3]),
            msgs.join("; ")
        );
        if !lr.is_empty() {
            for (k, m) in &lr {
                if *k == 0 || *k == 5 {
                    continue;
                }
                let ours = match k {
                    1 => &res[2],
                    2 => &res[3],
                    3 => &res[0],
                    _ => &res[1],
                };
                println!(
                    "    LR T{k}: sensor rot {:+.2} persp ({:+.5}, {:+.5}) | ours {}",
                    px_rot(m),
                    m[6],
                    m[7],
                    ours.solution.as_ref().map_or("-".to_owned(), |s| format!(
                        "sensor rot {:+.2} persp ({:+.5}, {:+.5}) | inverse rot {:+.2}",
                        px_rot(&s.matrix),
                        s.matrix[6],
                        s.matrix[7],
                        upright::invert_matrix(&s.matrix).map_or(0.0, |i| px_rot(&i))
                    ))
                );
            }
        }
        if let Some(d) = &out_dir {
            let mut before = r.rgb.clone();
            let segs = upright::detect_segments(&upright::luminance(&r.rgb, r.width, r.height), r.width, r.height);
            draw_segments(&mut before, r.width, r.height, &segs);
            write_png(
                &d.join(format!("{}_lines.png", path.file_stem().unwrap().to_string_lossy())),
                &[(before.clone(), r.width, r.height)],
            );
            let mut tiles = vec![downscale(&before, r.width, r.height, 2)];
            for k in [0usize, 1, 2] {
                let m = res[k]
                    .solution
                    .as_ref()
                    .map_or(vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0], |s| s.matrix.clone());
                let wimg = warp(&r.rgb, r.width, r.height, o, &m);
                tiles.push(downscale(&wimg, r.width, r.height, 2));
            }
            let name = path.file_stem().unwrap().to_string_lossy().to_string();
            write_png(&d.join(format!("{name}.png")), &tiles);
        }
    }
    println!(
        "\nrender 1024 px (decoded source cached): p50 {:.1} ms p90 {:.1} ms",
        percentile(&mut t_render.clone(), 0.5),
        percentile(&mut t_render.clone(), 0.9)
    );
    println!(
        "detect + solve per mode: p50 {:.1} ms p90 {:.1} ms",
        percentile(&mut t_solve.clone(), 0.5),
        percentile(&mut t_solve.clone(), 0.9)
    );
}
