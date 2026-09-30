//! Mask evaluation on an output grid (formulas in the parent module docs). Everything is
//! computed per output pixel from the sensor-frame geometry, so any resolution / region /
//! crop gives the same mask; rows are processed in parallel (rayon).

use rayon::prelude::*;

use super::{AlphaMask, GroupWeights, MaskGeometry, MatteSource, RangeGuide};
use crate::ipc::types::{
    AiMask, AiTarget, BrushMask, ColorRange, CropSettings, DevelopWarning, DevelopWarningCode, ImageId, LinearMask,
    LuminanceRange, MaskBlendMode, MaskComponent, MaskGroup, MaskShape, NormRect, RadialMask,
};

/// Rows per parallel work item.
const BAND: usize = 16;

/// Affine map `(px, py)` (output pixel indices; integers hit pixel centres) -> sensor-frame
/// normalized coordinates: `u = a px + b py + c`, `v = d px + e py + f`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Affine {
    #[inline]
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (self.a * x + self.b * y + self.c, self.d * x + self.e * y + self.f)
    }

    pub fn inverse(&self) -> Option<Affine> {
        let det = self.a * self.e - self.b * self.d;
        if det.abs() < 1e-18 {
            return None;
        }
        let (a, b, d, e) = (self.e / det, -self.b / det, -self.d / det, self.a / det);
        Some(Affine { a, b, c: -(a * self.c + b * self.f), d, e, f: -(d * self.c + e * self.f) })
    }
}

/// Oriented normalized (x, y) -> un-oriented normalized (u, v) as `[a, b, c, d, e, f]`
/// (the inverse of `ipc::types::orient_point`).
pub fn orientation_map(o: u8) -> [f64; 6] {
    match o {
        2 => [-1.0, 0.0, 1.0, 0.0, 1.0, 0.0],
        3 => [-1.0, 0.0, 1.0, 0.0, -1.0, 1.0],
        4 => [1.0, 0.0, 0.0, 0.0, -1.0, 1.0],
        5 => [0.0, 1.0, 0.0, 1.0, 0.0, 0.0],
        6 => [0.0, 1.0, 0.0, -1.0, 0.0, 1.0],
        7 => [0.0, -1.0, 1.0, -1.0, 0.0, 1.0],
        8 => [0.0, -1.0, 1.0, 1.0, 0.0, 0.0],
        _ => [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
    }
}

/// Crop frame of `crop` on a `sw x sh` (un-oriented) image: oriented output size in source
/// px and the map from oriented normalized crop-frame coordinates to un-oriented normalized
/// source coordinates. Same semantics as `develop::parity::crop_geometry` (Lightroom:
/// `left/top` and `right/bottom` are the rotated frame's corners in the source).
pub fn crop_frame(crop: &CropSettings, sw: u32, sh: u32, orientation: u8) -> (u32, u32, [f64; 6]) {
    let o = if (1..=8).contains(&orientation) { orientation } else { 1 };
    let (sw, sh) = (f64::from(sw.max(1)), f64::from(sh.max(1)));
    let (l, t, r, b, angle) = if crop.enabled {
        let l = f64::from(crop.left).clamp(0.0, 1.0);
        let r = f64::from(crop.right).clamp(0.0, 1.0);
        let t = f64::from(crop.top).clamp(0.0, 1.0);
        let b = f64::from(crop.bottom).clamp(0.0, 1.0);
        if r - l > 1e-6 && b - t > 1e-6 {
            (l, t, r, b, f64::from(crop.angle).clamp(-45.0, 45.0))
        } else {
            (0.0, 0.0, 1.0, 1.0, 0.0)
        }
    } else {
        (0.0, 0.0, 1.0, 1.0, 0.0)
    };
    let (cx, cy) = ((l + r) / 2.0 * sw, (t + b) / 2.0 * sh);
    let (dx, dy) = ((r - l) * sw, (b - t) * sh);
    let (sn, cs) = angle.to_radians().sin_cos();
    let (fw, fh) = ((dx * cs + dy * sn).max(1.0), (-dx * sn + dy * cs).max(1.0));
    let uv = [
        cs * fw / sw,
        -sn * fh / sw,
        (cx - cs * fw * 0.5 + sn * fh * 0.5) / sw,
        sn * fw / sh,
        cs * fh / sh,
        (cy - sn * fw * 0.5 - cs * fh * 0.5) / sh,
    ];
    let om = orientation_map(o);
    let m = [
        uv[0] * om[0] + uv[1] * om[3],
        uv[0] * om[1] + uv[1] * om[4],
        uv[0] * om[2] + uv[1] * om[5] + uv[2],
        uv[3] * om[0] + uv[4] * om[3],
        uv[3] * om[1] + uv[4] * om[4],
        uv[3] * om[2] + uv[4] * om[5] + uv[5],
    ];
    let (w, h) = (fw.round().max(1.0) as u32, fh.round().max(1.0) as u32);
    let (w, h) = if o >= 5 { (h, w) } else { (w, h) };
    (w, h, m)
}

/// Output pixel -> sensor frame for `g`.
pub fn geometry_affine(g: &MaskGeometry) -> Affine {
    let (_, _, m) = crop_frame(&g.crop, g.sensor_width, g.sensor_height, g.orientation);
    let r = g.region.unwrap_or(NormRect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 });
    let sx = f64::from(r.width) / f64::from(g.width.max(1));
    let sy = f64::from(r.height) / f64::from(g.height.max(1));
    let (ox, oy) = (f64::from(r.x) + 0.5 * sx, f64::from(r.y) + 0.5 * sy);
    Affine {
        a: m[0] * sx,
        b: m[1] * sy,
        c: m[0] * ox + m[1] * oy + m[2],
        d: m[3] * sx,
        e: m[4] * sy,
        f: m[3] * ox + m[4] * oy + m[5],
    }
}

/// Sensor px per output px (the map is a similarity up to rounding).
pub fn sensor_px_per_output_px(g: &MaskGeometry, t: &Affine) -> f64 {
    let (sw, sh) = (f64::from(g.sensor_width.max(1)), f64::from(g.sensor_height.max(1)));
    let sx = ((t.a * sw).powi(2) + (t.d * sh).powi(2)).sqrt();
    let sy = ((t.b * sw).powi(2) + (t.e * sh).powi(2)).sqrt();
    ((sx + sy) / 2.0).max(1e-9)
}

struct Grid {
    w: usize,
    h: usize,
    t: Affine,
    inv: Option<Affine>,
    /// Sensor size in px.
    sw: f64,
    sh: f64,
    /// Output px per sensor px.
    scale: f64,
}

impl Grid {
    fn new(g: &MaskGeometry) -> Grid {
        let t = geometry_affine(g);
        Grid {
            w: g.width as usize,
            h: g.height as usize,
            t,
            inv: t.inverse(),
            sw: f64::from(g.sensor_width.max(1)),
            sh: f64::from(g.sensor_height.max(1)),
            scale: 1.0 / sensor_px_per_output_px(g, &t),
        }
    }

    /// Fills a plane row by row with `f(px, py, row)`.
    fn plane(&self, f: impl Fn(usize, &mut [f32]) + Sync) -> Vec<f32> {
        let mut out = vec![0.0f32; self.w * self.h];
        if self.w == 0 {
            return out;
        }
        out.par_chunks_mut(self.w).enumerate().for_each(|(y, row)| f(y, row));
        out
    }

    /// A function linear in the sensor pixel coordinates `k0 * sx + k1 * sy + k2`, as
    /// coefficients over output pixels `(alpha px + beta py + gamma)`.
    fn linear(&self, k0: f64, k1: f64, k2: f64) -> (f64, f64, f64) {
        let t = &self.t;
        (
            k0 * t.a * self.sw + k1 * t.d * self.sh,
            k0 * t.b * self.sw + k1 * t.e * self.sh,
            k0 * t.c * self.sw + k1 * t.f * self.sh + k2,
        )
    }

    /// Sensor-normalized point -> output pixel coordinates (centres at integers).
    fn to_output(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let (px, py) = self.inv?.apply(f64::from(x), f64::from(y));
        Some((px as f32, py as f32))
    }
}

#[inline]
fn cosine_fall(t: f32) -> f32 {
    // 1 at t = 0, 0 at t = 1, smooth.
    0.5 * (1.0 + (std::f32::consts::PI * t.clamp(0.0, 1.0)).cos())
}

/// Dab falloff at normalized distance `d` (0 centre, 1 radius).
#[inline]
pub fn brush_falloff(d: f32, feather: f32) -> f32 {
    if d >= 1.0 {
        return 0.0;
    }
    let inner = 1.0 - feather;
    if d <= inner || feather <= 0.0 {
        1.0
    } else {
        cosine_fall((d - inner) / feather)
    }
}

/// CIE Lab distance with luminance down-weighted (colour-range / auto-mask similarity).
#[inline]
fn lab_dist2(p: [f32; 3], q: [f32; 3]) -> f32 {
    let dl = (p[0] - q[0]) * 0.5;
    let da = p[1] - q[1];
    let db = p[2] - q[2];
    dl * dl + da * da + db * db
}

fn guide_at(guide: &RangeGuide, x: f32, y: f32) -> Option<[f32; 3]> {
    let (gx, gy) = (x.round(), y.round());
    if gx < 0.0 || gy < 0.0 || gx >= guide.width as f32 || gy >= guide.height as f32 {
        return None;
    }
    guide.lab.get(gy as usize * guide.width as usize + gx as usize).copied()
}

/// Mean Lab over the output pixels of a sensor-frame rectangle (clipped to the grid).
fn guide_mean(grid: &Grid, guide: &RangeGuide, area: &NormRect) -> Option<[f32; 3]> {
    let corners = [
        (area.x, area.y),
        (area.x + area.width, area.y),
        (area.x, area.y + area.height),
        (area.x + area.width, area.y + area.height),
    ];
    let pts: Vec<(f32, f32)> = corners.iter().filter_map(|&(x, y)| grid.to_output(x, y)).collect();
    if pts.len() != 4 {
        return None;
    }
    let x0 = pts.iter().map(|p| p.0).fold(f32::MAX, f32::min).round().max(0.0) as usize;
    let x1 = pts.iter().map(|p| p.0).fold(f32::MIN, f32::max).round().min(guide.width as f32 - 1.0);
    let y0 = pts.iter().map(|p| p.1).fold(f32::MAX, f32::min).round().max(0.0) as usize;
    let y1 = pts.iter().map(|p| p.1).fold(f32::MIN, f32::max).round().min(guide.height as f32 - 1.0);
    if x1 < x0 as f32 || y1 < y0 as f32 {
        return None;
    }
    let (x1, y1) = (x1 as usize, y1 as usize);
    let step = (((x1 - x0 + 1) * (y1 - y0 + 1)) as f32 / 4096.0).sqrt().ceil().max(1.0) as usize;
    let mut sum = [0f64; 3];
    let mut n = 0f64;
    for y in (y0..=y1).step_by(step) {
        for x in (x0..=x1).step_by(step) {
            let p = guide.lab[y * guide.width as usize + x];
            for k in 0..3 {
                sum[k] += f64::from(p[k]);
            }
            n += 1.0;
        }
    }
    (n > 0.0).then(|| [(sum[0] / n) as f32, (sum[1] / n) as f32, (sum[2] / n) as f32])
}

fn brush(b: &BrushMask, grid: &Grid, guide: Option<&RangeGuide>) -> Option<Vec<f32>> {
    struct Prepared {
        r: f32,
        feather: f32,
        flow: f32,
        density: f32,
        erase: bool,
        dabs: Vec<(f32, f32)>,
        /// Auto mask: Lab under each dab.
        refs: Option<Vec<Option<[f32; 3]>>>,
    }
    let strokes: Vec<Prepared> = b
        .strokes
        .iter()
        .map(|s| {
            // Keep sub-pixel brushes visible at small render sizes.
            let r = ((f64::from(s.radius) * grid.sw * grid.scale) as f32).max(0.75);
            let dabs: Vec<(f32, f32)> = s.dabs.iter().filter_map(|d| grid.to_output(d.x, d.y)).collect();
            let refs = match (s.auto_mask, guide) {
                (true, Some(g)) => Some(dabs.iter().map(|&(x, y)| guide_at(g, x, y)).collect()),
                _ => None,
            };
            Prepared { r, feather: s.feather, flow: s.flow, density: s.density, erase: s.erase, dabs, refs }
        })
        .collect();
    let (w, h) = (grid.w, grid.h);
    if w == 0 || h == 0 {
        return Some(Vec::new());
    }
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w * BAND).enumerate().for_each(|(bi, chunk)| {
        let y0 = bi * BAND;
        let rows = chunk.len() / w;
        let (by0, by1) = (y0 as f32, (y0 + rows) as f32 - 1.0);
        for s in &strokes {
            let r = s.r;
            let reach = r + 0.5;
            let inv_r = 1.0 / r;
            for (k, &(cx, cy)) in s.dabs.iter().enumerate() {
                if cy + reach < by0 || cy - reach > by1 || cx + reach < 0.0 || cx - reach > (w - 1) as f32 {
                    continue;
                }
                let xa = (cx - reach).floor().max(0.0) as usize;
                let xb = ((cx + reach).ceil() as usize).min(w - 1);
                let ya = ((cy - reach).floor().max(by0)) as usize;
                let yb = ((cy + reach).ceil().min(by1)) as usize;
                let reference = s.refs.as_ref().and_then(|r| r[k]);
                for py in ya..=yb {
                    let dy = py as f32 - cy;
                    let row = &mut chunk[(py - y0) * w..(py - y0 + 1) * w];
                    for (px, v) in row.iter_mut().enumerate().take(xb + 1).skip(xa) {
                        let dx = px as f32 - cx;
                        let dist = (dx * dx + dy * dy).sqrt();
                        if dist >= reach {
                            continue;
                        }
                        // Soft profile, plus one pixel of anti-aliasing at the rim.
                        let mut a = s.flow * brush_falloff(dist * inv_r, s.feather) * (reach - dist).min(1.0);
                        if let (Some(q), Some(g)) = (reference, guide) {
                            let p = g.lab[py * w + px];
                            a *= (-lab_dist2(p, q) / (2.0 * 12.0 * 12.0)).exp();
                        }
                        if a <= 0.0 {
                            continue;
                        }
                        if s.erase {
                            *v *= 1.0 - s.density * a;
                        } else if *v < s.density {
                            *v += (s.density - *v) * a;
                        }
                    }
                }
            }
        }
    });
    Some(out)
}

fn linear(l: &LinearMask, grid: &Grid) -> Vec<f32> {
    let (zx, zy) = (f64::from(l.zero.x) * grid.sw, f64::from(l.zero.y) * grid.sh);
    let (dx, dy) = (f64::from(l.full.x) * grid.sw - zx, f64::from(l.full.y) * grid.sh - zy);
    let len2 = (dx * dx + dy * dy).max(1e-12);
    let (al, be, ga) = grid.linear(dx / len2, dy / len2, -(zx * dx + zy * dy) / len2);
    grid.plane(|y, row| {
        let base = be * y as f64 + ga;
        for (x, v) in row.iter_mut().enumerate() {
            *v = ((al * x as f64 + base) as f32).clamp(0.0, 1.0);
        }
    })
}

fn radial(r: &RadialMask, grid: &Grid) -> Vec<f32> {
    let (cx, cy) = (f64::from(r.left + r.right) / 2.0 * grid.sw, f64::from(r.top + r.bottom) / 2.0 * grid.sh);
    let ax = (f64::from(r.right - r.left) / 2.0 * grid.sw).max(1e-6);
    let ay = (f64::from(r.bottom - r.top) / 2.0 * grid.sh).max(1e-6);
    let (sn, cs) = f64::from(r.angle).to_radians().sin_cos();
    // Local ellipse coordinates, normalized by the semi-axes.
    let (xa, xb, xc) = grid.linear(cs / ax, sn / ax, -(cs * cx + sn * cy) / ax);
    let (ya, yb, yc) = grid.linear(-sn / ay, cs / ay, (sn * cx - cs * cy) / ay);
    let p = 2.0f32 * 2.0f32.powf(r.roundness / 100.0);
    let inner = (1.0 - r.feather / 100.0).clamp(0.0, 1.0);
    let g = (r.midpoint / 100.0).clamp(0.01, 0.99);
    let k = 0.5f32.ln() / g.ln();
    let flipped = r.flipped;
    grid.plane(|y, row| {
        let (bx, by) = (xb * y as f64 + xc, yb * y as f64 + yc);
        for (x, v) in row.iter_mut().enumerate() {
            let nx = (xa * x as f64 + bx) as f32;
            let ny = (ya * x as f64 + by) as f32;
            let rho = if (p - 2.0).abs() < 1e-6 {
                (nx * nx + ny * ny).sqrt()
            } else {
                (nx.abs().powf(p) + ny.abs().powf(p)).powf(1.0 / p)
            };
            let val = if rho <= inner {
                1.0
            } else if rho >= 1.0 {
                0.0
            } else {
                let t = (rho - inner) / (1.0 - inner).max(1e-6);
                cosine_fall(t.powf(k))
            };
            *v = if flipped { 1.0 - val } else { val };
        }
    })
}

/// Separable box blur (two passes) of `plane` with radius `r` px.
fn box_blur(plane: &mut [f32], w: usize, h: usize, r: usize) {
    if r == 0 || w == 0 || h == 0 {
        return;
    }
    let pass_rows = |src: &mut [f32], w: usize| {
        src.par_chunks_mut(w).for_each(|row| {
            let copy = row.to_vec();
            let mut acc: f32 = 0.0;
            let n = copy.len();
            // Running sum over [x - r, x + r], clamped at the edges.
            acc += copy[..=r.min(n - 1)].iter().sum::<f32>();
            let mut count = r.min(n - 1) + 1;
            for x in 0..n {
                row[x] = acc / count as f32;
                let add = x + r + 1;
                if add < n {
                    acc += copy[add];
                    count += 1;
                }
                if x >= r {
                    acc -= copy[x - r];
                    count -= 1;
                }
            }
        });
    };
    for _ in 0..2 {
        pass_rows(plane, w);
        let mut t = transpose(plane, w, h);
        pass_rows(&mut t, h);
        let back = transpose(&t, h, w);
        plane.copy_from_slice(&back);
    }
}

fn transpose(src: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(h).enumerate().for_each(|(x, col)| {
        for (y, v) in col.iter_mut().enumerate() {
            *v = src[y * w + x];
        }
    });
    out
}

/// Trapezoid over L*/100 (`featherLow -> low` up, `high -> featherHigh` down; linear ramps,
/// hard edges when a feather is empty).
#[inline]
pub fn luminance_weight(l: f32, r: &LuminanceRange) -> f32 {
    if l >= r.low && l <= r.high {
        1.0
    } else if l < r.low {
        if r.low - r.feather_low <= 1e-6 {
            0.0
        } else {
            ((l - r.feather_low) / (r.low - r.feather_low)).clamp(0.0, 1.0)
        }
    } else if r.feather_high - r.high <= 1e-6 {
        0.0
    } else {
        ((r.feather_high - l) / (r.feather_high - r.high)).clamp(0.0, 1.0)
    }
}

fn luminance(r: &LuminanceRange, grid: &Grid, guide: &RangeGuide) -> Option<Vec<f32>> {
    if guide.width as usize != grid.w || guide.height as usize != grid.h {
        return None;
    }
    let w = grid.w;
    let mut out = grid.plane(|y, row| {
        for (x, v) in row.iter_mut().enumerate() {
            *v = luminance_weight(guide.lab[y * w + x][0] / 100.0, r).clamp(0.0, 1.0);
        }
    });
    let radius = (f64::from(r.smoothness) / 100.0 * 0.005 * grid.sw * grid.scale).round() as usize;
    box_blur(&mut out, grid.w, grid.h, radius);
    Some(out)
}

fn color(c: &ColorRange, grid: &Grid, guide: &RangeGuide) -> Option<Vec<f32>> {
    if guide.width as usize != grid.w || guide.height as usize != grid.h {
        return None;
    }
    let refs: Vec<[f32; 3]> = c
        .samples
        .iter()
        .filter_map(|s| match &s.area {
            Some(a) => guide_mean(grid, guide, a),
            None => {
                let (x, y) = grid.to_output(s.point.x, s.point.y)?;
                let r = 1.0;
                let area_of = |dx: f32, dy: f32| guide_at(guide, x + dx, y + dy);
                let pts: Vec<[f32; 3]> = [(0.0, 0.0), (-r, 0.0), (r, 0.0), (0.0, -r), (0.0, r)]
                    .iter()
                    .filter_map(|&(a, b)| area_of(a, b))
                    .collect();
                if pts.is_empty() {
                    return None;
                }
                let n = pts.len() as f32;
                Some([
                    pts.iter().map(|p| p[0]).sum::<f32>() / n,
                    pts.iter().map(|p| p[1]).sum::<f32>() / n,
                    pts.iter().map(|p| p[2]).sum::<f32>() / n,
                ])
            }
        })
        .collect();
    if refs.is_empty() {
        return None;
    }
    let sigma = 4.0 + c.amount / 100.0 * 36.0;
    let k = 1.0 / (2.0 * sigma * sigma);
    let w = grid.w;
    Some(grid.plane(|y, row| {
        for (x, v) in row.iter_mut().enumerate() {
            let p = guide.lab[y * w + x];
            let best = refs.iter().map(|q| lab_dist2(p, *q)).fold(f32::MAX, f32::min);
            *v = (-best * k).exp();
        }
    }))
}

fn sample_matte(m: &AlphaMask, grid: &Grid, invert: bool) -> Vec<f32> {
    let t = grid.t;
    grid.plane(|y, row| {
        let (bu, bv) = (t.b * y as f64 + t.c, t.e * y as f64 + t.f);
        for (x, v) in row.iter_mut().enumerate() {
            let u = t.a * x as f64 + bu;
            let w = t.d * x as f64 + bv;
            let s = m.sample_f64(u, w);
            *v = if invert { 1.0 - s } else { s };
        }
    })
}

fn ai(ai: &AiMask, grid: &Grid, image_id: ImageId, mattes: &dyn MatteSource) -> Option<Vec<f32>> {
    if let Some(m) = mattes.matte(image_id, ai) {
        return Some(sample_matte(&m, grid, false));
    }
    // Background without its own matte: the inverted subject matte.
    if ai.target == AiTarget::Background && ai.digest.is_none() {
        let subject = AiMask { target: AiTarget::Subject, reference_point: ai.reference_point, digest: None };
        if let Some(m) = mattes.matte(image_id, &subject) {
            return Some(sample_matte(&m, grid, true));
        }
    }
    None
}

/// The shape plane of `c` (before opacity / inversion).
fn shape_plane(
    c: &MaskComponent,
    grid: &Grid,
    image_id: ImageId,
    mattes: &dyn MatteSource,
    guide: Option<&RangeGuide>,
) -> Option<Vec<f32>> {
    match &c.shape {
        MaskShape::Brush(b) => brush(b, grid, guide),
        MaskShape::Linear(l) => Some(linear(l, grid)),
        MaskShape::Radial(r) => Some(radial(r, grid)),
        MaskShape::Luminance(l) => luminance(l, grid, guide?),
        MaskShape::Color(cr) => color(cr, grid, guide?),
        MaskShape::Ai(a) => ai(a, grid, image_id, mattes),
        MaskShape::Unsupported(_) => None,
    }
}

pub fn evaluate_component(
    c: &MaskComponent,
    geom: &MaskGeometry,
    image_id: ImageId,
    mattes: &dyn MatteSource,
    guide: Option<&RangeGuide>,
) -> Option<Vec<f32>> {
    let grid = Grid::new(geom);
    component_on(c, &grid, image_id, mattes, guide)
}

fn component_on(
    c: &MaskComponent,
    grid: &Grid,
    image_id: ImageId,
    mattes: &dyn MatteSource,
    guide: Option<&RangeGuide>,
) -> Option<Vec<f32>> {
    let mut plane = shape_plane(c, grid, image_id, mattes, guide)?;
    let (op, inv) = (c.opacity.clamp(0.0, 1.0), c.inverted);
    if inv || op != 1.0 {
        plane.par_iter_mut().for_each(|v| {
            let s = if inv { 1.0 - *v } else { *v };
            *v = s * op;
        });
    }
    Some(plane)
}

pub fn combine(acc: &mut [f32], value: &[f32], mode: MaskBlendMode) {
    let n = acc.len().min(value.len());
    let (acc, value) = (&mut acc[..n], &value[..n]);
    match mode {
        MaskBlendMode::Add => acc.par_iter_mut().zip(value.par_iter()).for_each(|(a, v)| *a = a.max(*v)),
        MaskBlendMode::Subtract => acc.par_iter_mut().zip(value.par_iter()).for_each(|(a, v)| *a *= 1.0 - v),
        MaskBlendMode::Intersect => acc.par_iter_mut().zip(value.par_iter()).for_each(|(a, v)| *a *= v),
    }
}

/// Combined mask of `g` (before `amount`), counting unrenderable components. `None` = the
/// group has no renderable active component.
pub fn group_mask(
    g: &MaskGroup,
    geom: &MaskGeometry,
    image_id: ImageId,
    mattes: &dyn MatteSource,
    guide: Option<&RangeGuide>,
    unsupported: &mut usize,
    missing_ai: &mut usize,
) -> Option<Vec<f32>> {
    let grid = Grid::new(geom);
    let mut acc: Option<Vec<f32>> = None;
    for c in g.components.iter().filter(|c| c.active) {
        let plane = component_on(c, &grid, image_id, mattes, guide);
        let Some(plane) = plane else {
            match &c.shape {
                MaskShape::Unsupported(_) => *unsupported += 1,
                MaskShape::Ai(_) => *missing_ai += 1,
                _ => {}
            }
            continue;
        };
        match acc.as_mut() {
            None => acc = Some(plane),
            Some(a) => combine(a, &plane, c.mode),
        }
    }
    acc
}

pub fn evaluate(
    masks: &[MaskGroup],
    geom: &MaskGeometry,
    image_id: ImageId,
    mattes: &dyn MatteSource,
    guide: Option<&RangeGuide>,
) -> GroupWeights {
    let (mut unsupported, mut missing_ai) = (0usize, 0usize);
    let groups = masks
        .iter()
        .map(|g| {
            if !g.active || g.amount <= 0.0 {
                return None;
            }
            let mut m = group_mask(g, geom, image_id, mattes, guide, &mut unsupported, &mut missing_ai)?;
            if g.amount != 1.0 {
                let a = g.amount.clamp(0.0, 2.0);
                m.par_iter_mut().for_each(|v| *v *= a);
            }
            Some(m)
        })
        .collect();
    let mut warnings = Vec::new();
    if unsupported > 0 {
        warnings
            .push(DevelopWarning { code: DevelopWarningCode::MasksUnsupported, detail: Some(unsupported.to_string()) });
    }
    if missing_ai > 0 {
        warnings
            .push(DevelopWarning { code: DevelopWarningCode::AiMaskNeedsUpdate, detail: Some(missing_ai.to_string()) });
    }
    GroupWeights { width: geom.width, height: geom.height, groups, warnings }
}
