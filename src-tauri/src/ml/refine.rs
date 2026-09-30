//! Guided refinement of Sieve AI mattes at render time (Phase 7c).
//!
//! Sieve stores AI mattes (`mask_cache` origin `sieve`) as the model's **unrefined, low-res
//! output** plus its placement in the sensor frame ([`AlphaMask`] with `bounds`): 1024x1024
//! for subject/background, 320x320 for sky, the person/object ROI at <= 1536 px for people and
//! objects. That unit is resolution independent. Edges are snapped to the image at the
//! resolution actually rendered (a 2048 px preview, a 1:1 region, a 60 MP export) with a
//! fast colour guided filter (He, Sun & Tang 2013) whose guide is the render itself:
//!
//! ```text
//! // once per render (sensor-frame guide, e.g. the uncropped render un-oriented with
//! // `unorient_rgb`, or any axis-aligned sensor-frame region of it):
//! let guide = Guide { width, height, rgb: &rgb, region };
//! let stats = GuideStats::new(&guide, params.radius);         // shared by every matte
//! // per Sieve matte (params from its cache kind; `None` = sample it as is):
//! if let Some(params) = RefineParams::for_kind(&kind) {
//!     let refined = refine_with(&stats, &matte, &guide, params);  // AlphaMask at guide density
//! }
//! ```
//!
//! [`refine`] is the one-shot form. All functions are pure (no I/O, no models) and
//! deterministic. Lightroom mattes (origin `lightroom`) are already final: do not refine them.
//!
//! Cost (M3 Max, 2048 px guide): stats ~15 ms once per guide; per matte ~10-25 ms (solve on
//! a <= 1024 px grid restricted to the matte's bounds, apply at guide resolution).

use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use rayon::prelude::*;

use crate::develop::masks::AlphaMask;
use crate::ipc::types::NormRect;

/// Long edge of the grid the filter coefficients are solved on.
pub const WORK_EDGE: usize = 1024;

/// Guided-filter parameters of one matte kind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RefineParams {
    /// Window radius as a fraction of the *full frame's* long edge (resolution independent).
    /// Mattes refined against one [`GuideStats`] must share it.
    pub radius: f32,
    /// Regularisation on [0,1] intensities: larger = smoother, follows the matte more.
    pub eps: f32,
    /// S-curve around 0.5 after filtering (> 1 counters the filter's softening).
    pub gamma: f32,
}

/// Person parts drawn from FaceMesh polygons (already sharp, anti-aliased; not refined).
const FEATURE_PARTS: [&str; 5] = ["eyebrows", "eye_sclera", "iris_pupil", "lips", "teeth"];

impl RefineParams {
    /// BiRefNet is already sharp: small window, no contrast.
    pub const SUBJECT: RefineParams = RefineParams { radius: 0.004, eps: 1e-4, gamma: 1.0 };
    /// 320 px network output: wide window.
    pub const SKY: RefineParams = RefineParams { radius: 0.012, eps: 1e-4, gamma: 1.5 };
    /// People instances and objects (SAM at 1024 px).
    pub const PERSON: RefineParams = RefineParams { radius: 0.004, eps: 1e-4, gamma: 1.3 };
    /// Hair / skin / clothes (256 px parser tiles): smoother.
    pub const PARTS: RefineParams = RefineParams { radius: 0.004, eps: 5e-4, gamma: 1.3 };

    /// Parameters for a Sieve matte of `kind` (`AiMask::cache_kind`); `None` = use the matte
    /// as is (facial features only, landscape/other kinds, unknown kinds).
    pub fn for_kind(kind: &str) -> Option<RefineParams> {
        let base = kind.split('@').next().unwrap_or(kind);
        match base {
            "subject" | "background" => Some(Self::SUBJECT),
            "sky" => Some(Self::SKY),
            "object" => Some(Self::PERSON),
            _ => {
                let parts = base.strip_prefix("people:")?;
                if parts == "person" {
                    return Some(Self::PERSON);
                }
                let all_features = parts.split('+').all(|p| FEATURE_PARTS.contains(&p));
                (!all_features).then_some(Self::PARTS)
            }
        }
    }
}

/// Render image used as the guide: interleaved sRGB8 in the **sensor frame** (un-oriented)
/// covering `region` (normalized sensor-frame rect; the whole frame = `0,0,1,1`).
#[derive(Clone, Copy)]
pub struct Guide<'a> {
    pub width: u32,
    pub height: u32,
    pub rgb: &'a [u8],
    pub region: NormRect,
}

/// Guide statistics on the solve grid (window means and colour covariance), computed once
/// per guide and radius and shared by every matte refined against it.
pub struct GuideStats {
    region: NormRect,
    guide_w: usize,
    guide_h: usize,
    /// Solve grid.
    ww: usize,
    wh: usize,
    radius: f32,
    r: usize,
    /// Guide channels on the solve grid, 0..1.
    chan: [Vec<f32>; 3],
    mean: [Vec<f32>; 3],
    /// rr, rg, rb, gg, gb, bb (without eps).
    cov: [Vec<f32>; 6],
}

impl GuideStats {
    pub fn new(guide: &Guide, radius: f32) -> GuideStats {
        let (gw, gh) = (guide.width.max(1) as usize, guide.height.max(1) as usize);
        let k = (WORK_EDGE as f32 / gw.max(gh) as f32).min(1.0);
        let (ww, wh) = (((gw as f32 * k).round() as usize).max(1), ((gh as f32 * k).round() as usize).max(1));
        let small = if (ww, wh) == (gw, gh) {
            guide.rgb[..gw * gh * 3].to_vec()
        } else {
            resize_rgb(guide.rgb, gw, gh, ww, wh)
        };
        let n = ww * wh;
        let chan: [Vec<f32>; 3] = std::array::from_fn(|c| (0..n).map(|i| small[i * 3 + c] as f32 / 255.0).collect());
        // Frame long edge in solve-grid pixels.
        let frame_w = ww as f32 / guide.region.width.max(1e-6);
        let frame_h = wh as f32 / guide.region.height.max(1e-6);
        let r = ((radius * frame_w.max(frame_h)).round() as usize).clamp(1, (ww.max(wh) / 2).max(1));
        let mean: [Vec<f32>; 3] = std::array::from_fn(|c| box_mean(&chan[c], ww, wh, r));
        const PAIRS: [(usize, usize); 6] = [(0, 0), (0, 1), (0, 2), (1, 1), (1, 2), (2, 2)];
        let cov: [Vec<f32>; 6] = std::array::from_fn(|k| {
            let (a, b) = PAIRS[k];
            let prod: Vec<f32> = chan[a].iter().zip(&chan[b]).map(|(x, y)| x * y).collect();
            let mut m = box_mean(&prod, ww, wh, r);
            m.iter_mut().enumerate().for_each(|(i, v)| *v -= mean[a][i] * mean[b][i]);
            m
        });
        GuideStats { region: guide.region, guide_w: gw, guide_h: gh, ww, wh, radius, r, chan, mean, cov }
    }

    /// The radius these statistics were computed for.
    pub fn radius(&self) -> f32 {
        self.radius
    }

    fn fits(&self, guide: &Guide, radius: f32) -> bool {
        self.radius == radius
            && self.region == guide.region
            && self.guide_w == guide.width as usize
            && self.guide_h == guide.height as usize
    }
}

/// One-shot refinement: [`GuideStats::new`] + [`refine_with`].
pub fn refine(alpha: &AlphaMask, guide: &Guide, params: RefineParams) -> AlphaMask {
    refine_with(&GuideStats::new(guide, params.radius), alpha, guide, params)
}

/// Refines `alpha` (a low-res Sieve matte, sensor-frame `bounds`) against `guide`. Returns a
/// matte at the guide's pixel density covering `alpha.bounds` ∩ `guide.region` (pixel
/// aligned to the guide; a 1x1 zero matte when they do not overlap). `stats` must belong to
/// `guide` and `params.radius` (recomputed otherwise).
pub fn refine_with(stats: &GuideStats, alpha: &AlphaMask, guide: &Guide, params: RefineParams) -> AlphaMask {
    if !stats.fits(guide, params.radius) {
        return refine_with(&GuideStats::new(guide, params.radius), alpha, guide, params);
    }
    let reg = guide.region;
    let (gw, gh) = (stats.guide_w, stats.guide_h);
    let b = alpha.bounds;
    // Output rect in guide pixels.
    let to_gx = |x: f32| (x - reg.x) / reg.width * gw as f32;
    let to_gy = |y: f32| (y - reg.y) / reg.height * gh as f32;
    // Snap within 1e-3 px so float noise does not add a column.
    let ox0 = (to_gx(b.x) + 1e-3).floor().max(0.0) as usize;
    let oy0 = (to_gy(b.y) + 1e-3).floor().max(0.0) as usize;
    let ox1 = ((to_gx(b.x + b.width) - 1e-3).ceil().max(0.0) as usize).min(gw);
    let oy1 = ((to_gy(b.y + b.height) - 1e-3).ceil().max(0.0) as usize).min(gh);
    if ox1 <= ox0 || oy1 <= oy0 || alpha.width == 0 || alpha.height == 0 {
        return AlphaMask { width: 1, height: 1, bounds: b, data: vec![0] };
    }
    let (ow, oh) = (ox1 - ox0, oy1 - oy0);
    let out_bounds = NormRect {
        x: reg.x + ox0 as f32 / gw as f32 * reg.width,
        y: reg.y + oy0 as f32 / gh as f32 * reg.height,
        width: ow as f32 / gw as f32 * reg.width,
        height: oh as f32 / gh as f32 * reg.height,
    };

    // Solve window on the work grid (output rect + filter support).
    let (ww, wh, r) = (stats.ww, stats.wh, stats.r);
    let (sx, sy) = (ww as f32 / gw as f32, wh as f32 / gh as f32);
    let pad = 2 * r + 2;
    let wx0 = ((ox0 as f32 * sx).floor() as usize).saturating_sub(pad);
    let wy0 = ((oy0 as f32 * sy).floor() as usize).saturating_sub(pad);
    let wx1 = ((ox1 as f32 * sx).ceil() as usize + pad).min(ww);
    let wy1 = ((oy1 as f32 * sy).ceil() as usize + pad).min(wh);
    let (vw, vh) = (wx1 - wx0, wy1 - wy0);
    let n = vw * vh;

    // Matte on the window (bilinear from its own grid, 0 outside its bounds).
    let p: Vec<f32> = (0..n)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((wx0 + i % vw) as f32 + 0.5, (wy0 + i / vw) as f32 + 0.5);
            let nx = reg.x + x / ww as f32 * reg.width;
            let ny = reg.y + y / wh as f32 * reg.height;
            sample_alpha(alpha, nx, ny)
        })
        .collect();
    let at = |i: usize| (wy0 + i / vw) * ww + wx0 + i % vw;
    let m_p = box_mean(&p, vw, vh, r);
    let m_ip: [Vec<f32>; 3] = std::array::from_fn(|c| {
        let prod: Vec<f32> = (0..n).map(|i| stats.chan[c][at(i)] * p[i]).collect();
        box_mean(&prod, vw, vh, r)
    });
    let eps = params.eps;
    let mut a = vec![0.0f32; 3 * n];
    let mut bb = vec![0.0f32; n];
    for i in 0..n {
        let g = at(i);
        let mi = [stats.mean[0][g], stats.mean[1][g], stats.mean[2][g]];
        let cv = [m_ip[0][i] - mi[0] * m_p[i], m_ip[1][i] - mi[1] * m_p[i], m_ip[2][i] - mi[2] * m_p[i]];
        let c = &stats.cov;
        let (rr, rg, rb, gg, gb, bl) = (c[0][g] + eps, c[1][g], c[2][g], c[3][g] + eps, c[4][g], c[5][g] + eps);
        let inv = [
            gg * bl - gb * gb,
            gb * rb - rg * bl,
            rg * gb - gg * rb,
            rr * bl - rb * rb,
            rg * rb - rr * gb,
            rr * gg - rg * rg,
        ];
        let det = rr * inv[0] + rg * inv[1] + rb * inv[2];
        let (ar, ag, ab) = if det.abs() > 1e-12 {
            (
                (inv[0] * cv[0] + inv[1] * cv[1] + inv[2] * cv[2]) / det,
                (inv[1] * cv[0] + inv[3] * cv[1] + inv[4] * cv[2]) / det,
                (inv[2] * cv[0] + inv[4] * cv[1] + inv[5] * cv[2]) / det,
            )
        } else {
            (0.0, 0.0, 0.0)
        };
        a[i] = ar;
        a[n + i] = ag;
        a[2 * n + i] = ab;
        bb[i] = m_p[i] - ar * mi[0] - ag * mi[1] - ab * mi[2];
    }
    let ma: [Vec<f32>; 3] = std::array::from_fn(|c| box_mean(&a[c * n..(c + 1) * n], vw, vh, r));
    let mb = box_mean(&bb, vw, vh, r);

    // Apply q = a . I + b at guide resolution.
    let gamma = params.gamma;
    let mut data = vec![0u8; ow * oh];
    data.par_chunks_mut(ow).enumerate().for_each(|(y, row)| {
        let gy = oy0 + y;
        let v = (gy as f32 + 0.5) * sy - 0.5 - wy0 as f32;
        for (x, out) in row.iter_mut().enumerate() {
            let gx = ox0 + x;
            let u = (gx as f32 + 0.5) * sx - 0.5 - wx0 as f32;
            let px = &guide.rgb[(gy * gw + gx) * 3..(gy * gw + gx) * 3 + 3];
            let mut q = sample(&mb, vw, vh, u, v);
            for c in 0..3 {
                q += sample(&ma[c], vw, vh, u, v) * (px[c] as f32 / 255.0);
            }
            let q = q.clamp(0.0, 1.0);
            let q = if gamma != 1.0 { contrast(q, gamma) } else { q };
            *out = (q * 255.0 + 0.5) as u8;
        }
    });
    AlphaMask { width: ow as u32, height: oh as u32, bounds: out_bounds, data }
}

/// Bilinear sample of a matte at a normalized sensor-frame point (0 outside its bounds).
pub fn sample_alpha(m: &AlphaMask, x: f32, y: f32) -> f32 {
    let b = m.bounds;
    if m.width == 0 || m.height == 0 || x < b.x || y < b.y || x > b.x + b.width || y > b.y + b.height {
        return 0.0;
    }
    let u = (x - b.x) / b.width * m.width as f32 - 0.5;
    let v = (y - b.y) / b.height * m.height as f32 - 0.5;
    let (w, h) = (m.width as usize, m.height as usize);
    let u = u.clamp(0.0, (w - 1) as f32);
    let v = v.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (u as usize, v as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (u - x0 as f32, v - y0 as f32);
    let px = |x: usize, y: usize| m.data[y * w + x] as f32;
    let top = px(x0, y0) * (1.0 - fx) + px(x1, y0) * fx;
    let bot = px(x0, y1) * (1.0 - fx) + px(x1, y1) * fx;
    (top * (1.0 - fy) + bot * fy) / 255.0
}

/// Sensor-frame copy of an EXIF-oriented interleaved image (`w x h` = oriented size,
/// `channels` bytes per pixel). Returns the pixels and the sensor-frame size. Use it to turn
/// an oriented render into a [`Guide`].
pub fn unorient_pixels(px: &[u8], w: usize, h: usize, channels: usize, orientation: u8) -> (Vec<u8>, usize, usize) {
    let o = if (1..=8).contains(&orientation) { orientation } else { 1 };
    if o == 1 {
        return (px[..w * h * channels].to_vec(), w, h);
    }
    let (sw, sh) = if o >= 5 { (h, w) } else { (w, h) };
    let mut out = vec![0u8; sw * sh * channels];
    out.par_chunks_mut(sw * channels).enumerate().for_each(|(y, row)| {
        for x in 0..sw {
            let (dx, dy) = oriented_index(x, y, sw, sh, o);
            let s = (dy * w + dx) * channels;
            row[x * channels..(x + 1) * channels].copy_from_slice(&px[s..s + channels]);
        }
    });
    (out, sw, sh)
}

/// Displayed-frame pixel of sensor-frame pixel (`x`, `y`) of a `sw x sh` sensor image
/// (pixel-index form of `ipc::types::orient_point`).
pub fn oriented_index(x: usize, y: usize, sw: usize, sh: usize, orientation: u8) -> (usize, usize) {
    match orientation {
        2 => (sw - 1 - x, y),
        3 => (sw - 1 - x, sh - 1 - y),
        4 => (x, sh - 1 - y),
        5 => (y, x),
        6 => (sh - 1 - y, x),
        7 => (sh - 1 - y, sw - 1 - x),
        8 => (y, sw - 1 - x),
        _ => (x, y),
    }
}

/// S-curve around 0.5: `g > 1` pushes values towards 0 / 1.
pub(crate) fn contrast(v: f32, g: f32) -> f32 {
    if v <= 0.5 {
        0.5 * (2.0 * v).powf(g)
    } else {
        1.0 - 0.5 * (2.0 * (1.0 - v)).powf(g)
    }
}

fn resize_rgb(src: &[u8], w: usize, h: usize, dw: usize, dh: usize) -> Vec<u8> {
    let mut out = vec![0u8; dw * dh * 3];
    let ok = (|| -> Result<(), String> {
        let s = ImageRef::new(w as u32, h as u32, &src[..w * h * 3], PixelType::U8x3).map_err(|e| e.to_string())?;
        let mut d = Image::from_slice_u8(dw as u32, dh as u32, &mut out, PixelType::U8x3).map_err(|e| e.to_string())?;
        let opts = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Bilinear));
        Resizer::new().resize(&s, &mut d, &opts).map_err(|e| e.to_string())
    })();
    if ok.is_err() {
        // Nearest neighbour fallback (cannot fail).
        for y in 0..dh {
            for x in 0..dw {
                let (sx, sy) = (x * w / dw, y * h / dh);
                out[(y * dw + x) * 3..(y * dw + x) * 3 + 3]
                    .copy_from_slice(&src[(sy * w + sx) * 3..(sy * w + sx) * 3 + 3]);
            }
        }
    }
    out
}

/// Bilinear sample with clamp-to-edge (pixel centres at integers).
#[inline]
pub(crate) fn sample(p: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x as usize, y as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let top = p[y0 * w + x0] * (1.0 - fx) + p[y0 * w + x1] * fx;
    let bot = p[y1 * w + x0] * (1.0 - fx) + p[y1 * w + x1] * fx;
    top * (1.0 - fy) + bot * fy
}

/// Mean over a `(2r+1)^2` window, clamped at the borders (divides by the valid count).
/// Rows in parallel.
pub(crate) fn box_mean(p: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0.0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
        let mut acc = vec![0.0f64; w + 1];
        for x in 0..w {
            acc[x + 1] = acc[x] + p[y * w + x] as f64;
        }
        for (x, o) in out.iter_mut().enumerate() {
            let (a, b) = (x.saturating_sub(r), (x + r + 1).min(w));
            *o = ((acc[b] - acc[a]) / (b - a) as f64) as f32;
        }
    });
    // Columns: running sums over rows, processed as whole rows for cache locality.
    let mut col = vec![0.0f64; w * (h + 1)];
    for y in 0..h {
        for x in 0..w {
            col[(y + 1) * w + x] = col[y * w + x] + tmp[y * w + x] as f64;
        }
    }
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, o)| {
        let (a, b) = (y.saturating_sub(r), (y + r + 1).min(h));
        let k = (b - a) as f64;
        for x in 0..w {
            o[x] = ((col[b * w + x] - col[a * w + x]) / k) as f32;
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: NormRect = NormRect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 };

    #[test]
    fn box_mean_matches_brute_force() {
        let (w, h, r) = (7, 5, 2);
        let p: Vec<f32> = (0..w * h).map(|i| ((i * 37) % 11) as f32).collect();
        let m = box_mean(&p, w, h, r);
        for y in 0..h {
            for x in 0..w {
                let (mut s, mut n) = (0.0, 0);
                for yy in y.saturating_sub(r)..(y + r + 1).min(h) {
                    for xx in x.saturating_sub(r)..(x + r + 1).min(w) {
                        s += p[yy * w + xx];
                        n += 1;
                    }
                }
                assert!((m[y * w + x] - s / n as f32).abs() < 1e-4);
            }
        }
    }

    fn edge_image(w: usize, h: usize, edge: usize) -> Vec<u8> {
        let mut data = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let p = (y * w + x) * 3;
                data[p..p + 3].copy_from_slice(if x < edge { &[120, 170, 230] } else { &[40, 110, 40] });
            }
        }
        data
    }

    /// A blurry 1/8-res matte of a sharp colour edge becomes sharp at guide resolution, and
    /// the same stored matte refines consistently at two render sizes.
    #[test]
    fn refine_snaps_to_edges_at_any_resolution() {
        // Low-res matte: 128 columns, column 50 straddles the edge at x = 0.3945.
        let (pw, ph) = (128u32, 4u32);
        let data: Vec<u8> = (0..pw * ph)
            .map(|i| match (i % pw).cmp(&50) {
                std::cmp::Ordering::Less => 255,
                std::cmp::Ordering::Equal => 128,
                std::cmp::Ordering::Greater => 0,
            })
            .collect();
        let alpha = AlphaMask { width: pw, height: ph, bounds: FULL, data };
        for (w, h) in [(1024usize, 32usize), (3072, 96)] {
            let edge = (w as f32 * 0.3945) as usize;
            let rgb = edge_image(w, h, edge);
            let guide = Guide { width: w as u32, height: h as u32, rgb: &rgb, region: FULL };
            let m = refine(&alpha, &guide, RefineParams::SKY);
            assert_eq!((m.width as usize, m.height as usize), (w, h));
            let row = &m.data[(h / 2) * w..(h / 2 + 1) * w];
            let d = w / 256;
            assert!(row[edge - 2 - d] > 230, "{w}: sky side {}", row[edge - 2 - d]);
            assert!(row[edge + 2 + d] < 25, "{w}: tree side {}", row[edge + 2 + d]);
        }
    }

    /// Mattes with bounds, guides covering a region: output is the intersection at guide
    /// density; shared stats give the same result as one-shot refinement.
    #[test]
    fn region_guides_and_shared_stats() {
        let (w, h) = (400usize, 200usize);
        let rgb = edge_image(w, h, 200);
        // Guide = right half of the frame.
        let region = NormRect { x: 0.5, y: 0.0, width: 0.5, height: 1.0 };
        let guide = Guide { width: w as u32, height: h as u32, rgb: &rgb, region };
        let alpha = AlphaMask {
            width: 10,
            height: 10,
            bounds: NormRect { x: 0.6, y: 0.2, width: 0.2, height: 0.5 },
            data: vec![255; 100],
        };
        let stats = GuideStats::new(&guide, RefineParams::PERSON.radius);
        let a = refine_with(&stats, &alpha, &guide, RefineParams::PERSON);
        let b = refine(&alpha, &guide, RefineParams::PERSON);
        assert_eq!(a, b);
        // 0.6..0.8 of the frame = guide px 80..240; 0.2..0.7 = rows 40..140.
        assert_eq!((a.width, a.height), (160, 100));
        assert!((a.bounds.x - 0.6).abs() < 1e-5 && (a.bounds.width - 0.2).abs() < 1e-5);
        assert!(a.data[50 * 160 + 80] > 240);
        // Disjoint: 1x1 zero.
        let far = AlphaMask { bounds: NormRect { x: 0.0, y: 0.0, width: 0.3, height: 0.3 }, ..alpha };
        assert_eq!(refine_with(&stats, &far, &guide, RefineParams::PERSON).data, vec![0]);
    }

    #[test]
    fn params_by_kind() {
        assert_eq!(RefineParams::for_kind("subject"), Some(RefineParams::SUBJECT));
        assert_eq!(RefineParams::for_kind("background"), Some(RefineParams::SUBJECT));
        assert_eq!(RefineParams::for_kind("sky"), Some(RefineParams::SKY));
        assert_eq!(RefineParams::for_kind("people:person@0.5000,0.5000"), Some(RefineParams::PERSON));
        assert_eq!(RefineParams::for_kind("people:face_skin+lips@0.5000,0.5000"), Some(RefineParams::PARTS));
        assert_eq!(RefineParams::for_kind("people:eye_sclera+iris_pupil"), None);
        assert_eq!(RefineParams::for_kind("object@0.1000,0.1000,0.2000,0.2000"), Some(RefineParams::PERSON));
        assert_eq!(RefineParams::for_kind("landscape:water"), None);
        assert_eq!(RefineParams::for_kind("other:7"), None);
    }

    #[test]
    fn unorient_matches_orient_point() {
        use crate::ipc::types::{orient_point, NormPoint};
        let (sw, sh) = (5usize, 3usize);
        let sensor: Vec<u8> = (0..(sw * sh) as u8).collect();
        for o in 1..=8u8 {
            let (dw, dh) = if o >= 5 { (sh, sw) } else { (sw, sh) };
            let mut disp = vec![0u8; sw * sh];
            for y in 0..sh {
                for x in 0..sw {
                    let p =
                        orient_point(NormPoint { x: (x as f32 + 0.5) / sw as f32, y: (y as f32 + 0.5) / sh as f32 }, o);
                    let (dx, dy) = ((p.x * dw as f32) as usize, (p.y * dh as f32) as usize);
                    assert_eq!(oriented_index(x, y, sw, sh, o), (dx, dy), "orientation {o}");
                    disp[dy * dw + dx] = sensor[y * sw + x];
                }
            }
            let (back, bw, bh) = unorient_pixels(&disp, dw, dh, 1, o);
            assert_eq!((bw, bh), (sw, sh));
            assert_eq!(back, sensor, "orientation {o}");
        }
    }
}
