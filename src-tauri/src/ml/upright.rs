//! Upright (Lightroom Transform panel): straight-line detection, vanishing points and the
//! perspective solve behind `auto_upright` (IPC v19). Owned by vision-ml-dev; the warp /
//! render and the `crs:` I/O live in `develop` / `xmp` (rust-engine-dev).
//!
//! Pipeline (all on a ~1024 px render of the photo, orientation applied, uncropped and
//! untransformed, see [`solve_rgb8`]):
//! 1. [`detect_segments`]: an LSD-style line segment detector (Grompone von Gioi et al.):
//!    2x2 gradient on a lightly smoothed luminance plane, pixels ordered by gradient
//!    magnitude, region growing over pixels whose level-line angle agrees within 22.5
//!    degrees, a weighted principal-axis rectangle fit, and acceptance by length, thinness
//!    and aligned-pixel density (the density test plays the role of LSD's a-contrario
//!    NFA: random texture almost never forms a long thin region of aligned gradients).
//! 2. Vanishing points in camera-normalized coordinates (pinhole, principal point at the
//!    centre, focal length from the 35 mm equivalent): RANSAC over pairs of the longest
//!    candidate segments + weighted least squares (smallest eigenvector of
//!    `sum w l l^T`). A vanishing point needs 2+ distinct inlier segments and enough
//!    total inlier length; horizontal ones must be ~orthogonal (3D) to the vertical one.
//! 3. The solve is a virtual camera rotation `K R K^-1` (+ a shear for Full / Guided):
//!    - Level: in-plane roll only. From a consistent horizon (near-horizontal segments
//!      agreeing within 1 degree) or from the vertical vanishing point direction when the
//!      verticals are the stronger evidence (architecture).
//!    - Vertical: roll + pitch so the vertical vanishing point goes to infinity straight
//!      down: verticals become parallel and vertical.
//!    - Full: Vertical + yaw (and a vertical-preserving shear) so the dominant horizontal
//!      vanishing point goes to infinity sideways: horizontals become level too.
//!    - Auto: Vertical with the pitch damped above 10 degrees (capped at 20) and, only for a
//!      well-supported facade, a gentle yaw (damped above 3 degrees, capped at 6), then the
//!      whole solve is scaled back
//!      until no image edge is stretched more than 1.6x (never over-corrects).
//!    - Guided: from the user's 2..=4 guides (each guide is vertical or horizontal by its
//!      angle): 2+ vertical guides fix the vertical vanishing point exactly, 2+ horizontal
//!      guides the horizontal one, a single guide only its rotation.
//! 4. The source->corrected map is re-centred (the image centre stays at the centre) and
//!    scaled to keep the image area, then inverted (corrected -> source) and expressed in
//!    the normalized **sensor frame** (un-oriented), as `UprightSolution.matrix` defines.
//!
//! `UprightSolution.rotationDeg` is the in-plane (roll) part, positive = the content is
//! turned counter-clockwise as displayed. "Auto straighten" for the crop tool is
//! `auto_upright(id, "level", adjustments)`; the crop angle is
//! [`crop_angle_for_rotation`] of it (= `rotationDeg` for orientations 1/3/6/8).

use crate::ipc::types::{CameraMake, NormPoint, ParametricAdjustments, UprightGuide, UprightMode, UprightSolution};

/// Long edge of the detection render.
pub const DETECT_EDGE: u32 = 1024;

/// 35 mm equivalent focal length assumed when the photo has none (Lightroom's default
/// is close: ~35 mm).
pub const DEFAULT_FOCAL_35MM: f64 = 35.0;

/// Region-growing angle tolerance (LSD's 22.5 degrees).
const TAU: f32 = std::f32::consts::PI / 8.0;

/// One detected straight segment, oriented image pixels (y down).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

impl Segment {
    pub fn length(&self) -> f64 {
        (self.x2 - self.x1).hypot(self.y2 - self.y1)
    }

    /// Direction angle in (-90, 90] degrees, y down (positive = descends to the right).
    pub fn angle_deg(&self) -> f64 {
        let mut a = (self.y2 - self.y1).atan2(self.x2 - self.x1).to_degrees();
        if a <= -90.0 {
            a += 180.0;
        } else if a > 90.0 {
            a -= 180.0;
        }
        a
    }
}

/// Detected segments and the frame they were found in.
#[derive(Debug, Clone)]
pub struct LineSet {
    pub width: u32,
    pub height: u32,
    pub segments: Vec<Segment>,
}

/// Luminance (0..=1) of interleaved RGB8.
pub fn luminance(rgb: &[u8], w: u32, h: u32) -> Vec<f32> {
    let n = (w * h) as usize;
    let mut out = Vec::with_capacity(n);
    for px in rgb.as_chunks::<3>().0.iter().take(n) {
        out.push((0.299 * f32::from(px[0]) + 0.587 * f32::from(px[1]) + 0.114 * f32::from(px[2])) / 255.0);
    }
    out
}

/// Separable [1 4 6 4 1] / 16 blur with clamped borders.
fn smooth(src: &[f32], w: usize, h: usize) -> Vec<f32> {
    const K: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    let mut tmp = vec![0.0f32; w * h];
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        for x in 0..w {
            let mut s = 0.0;
            for (k, kv) in K.iter().enumerate() {
                let xx = (x as isize + k as isize - 2).clamp(0, w as isize - 1) as usize;
                s += kv * row[xx];
            }
            tmp[y * w + x] = s;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for (k, kv) in K.iter().enumerate() {
                let yy = (y as isize + k as isize - 2).clamp(0, h as isize - 1) as usize;
                s += kv * tmp[yy * w + x];
            }
            out[y * w + x] = s;
        }
    }
    out
}

fn ang_diff(a: f32, b: f32) -> f32 {
    let mut d = (a - b).abs();
    if d > std::f32::consts::PI {
        d = 2.0 * std::f32::consts::PI - d;
    }
    d
}

/// LSD-style line segment detection on a luminance plane (0..=1, `w x h`).
pub fn detect_segments(gray: &[f32], w: u32, h: u32) -> Vec<Segment> {
    let (w, h) = (w as usize, h as usize);
    if w < 8 || h < 8 || gray.len() < w * h {
        return Vec::new();
    }
    let img = smooth(gray, w, h);
    // Gradient on the (w-1) x (h-1) grid of 2x2 blocks (pixel centre at x + 0.5, y + 0.5).
    let (gw, gh) = (w - 1, h - 1);
    let mut mag = vec![0.0f32; gw * gh];
    let mut ang = vec![0.0f32; gw * gh];
    let mut max_mag = 0.0f32;
    for y in 0..gh {
        for x in 0..gw {
            let a = img[y * w + x];
            let b = img[y * w + x + 1];
            let c = img[(y + 1) * w + x];
            let d = img[(y + 1) * w + x + 1];
            let gx = (b + d - a - c) * 0.5;
            let gy = (c + d - a - b) * 0.5;
            let m = gx.hypot(gy);
            mag[y * gw + x] = m;
            // Level-line angle (perpendicular to the gradient), signed (polarity kept).
            ang[y * gw + x] = gx.atan2(-gy);
            max_mag = max_mag.max(m);
        }
    }
    // LSD's gradient threshold: q = 2 grey levels of quantization error over sin(tau).
    let rho = (2.0 / 255.0) / TAU.sin();
    if max_mag <= rho {
        return Vec::new();
    }
    // Pseudo-ordering by magnitude (1024 bins, strongest first).
    const BINS: usize = 1024;
    let mut bins: Vec<Vec<u32>> = vec![Vec::new(); BINS];
    for (i, &m) in mag.iter().enumerate() {
        if m > rho {
            let b = (((m - rho) / (max_mag - rho)) * (BINS - 1) as f32) as usize;
            bins[BINS - 1 - b.min(BINS - 1)].push(i as u32);
        }
    }
    let long = w.max(h) as f64;
    let min_len = (0.02 * long).max(12.0);
    let mut used = vec![false; gw * gh];
    let mut region: Vec<u32> = Vec::new();
    let mut segments = Vec::new();
    let grid = Grid { gw, gh, mag: &mag, ang: &ang, rho };
    for bin in &bins {
        for &seed in bin {
            let seed = seed as usize;
            if used[seed] {
                continue;
            }
            grid.grow(seed, TAU, None, &mut used, &mut region);
            if (region.len() as f64) < min_len {
                continue;
            }
            if let Some(s) = fit_rect(&region, gw, &mag, &ang, grid.mean_angle(&region), min_len) {
                segments.push(s);
                continue;
            }
            // LSD's refinement: regrow from the seed with half the tolerance around the
            // angle near the seed (rounded corners let the first region turn into an L).
            // The pixels left out are released so the other arm can seed its own line.
            let near: Vec<u32> = region
                .iter()
                .copied()
                .filter(|&p| {
                    let p = p as usize;
                    let (dx, dy) = ((p % gw) as isize - (seed % gw) as isize, (p / gw) as isize - (seed / gw) as isize);
                    dx * dx + dy * dy <= 9
                })
                .collect();
            let seed_ang = grid.mean_angle(&near);
            for &p in &region {
                used[p as usize] = false;
            }
            let first = std::mem::take(&mut region);
            grid.grow(seed, TAU * 0.5, Some(seed_ang), &mut used, &mut region);
            let ok = (region.len() as f64) >= min_len;
            let fitted = if ok { fit_refined(&mut region, seed, gw, &mag, &ang, min_len) } else { None };
            match fitted {
                Some(s) => segments.push(s),
                None => {
                    // Failed twice: retire the whole first region (LSD keeps it used).
                    for &p in &first {
                        used[p as usize] = true;
                    }
                }
            }
        }
    }
    merge_collinear(segments, long)
}

/// Gradient planes for region growing.
struct Grid<'a> {
    gw: usize,
    gh: usize,
    mag: &'a [f32],
    ang: &'a [f32],
    rho: f32,
}

impl Grid<'_> {
    fn mean_angle(&self, r: &[u32]) -> f32 {
        let (c, s) = r
            .iter()
            .fold((0.0f32, 0.0f32), |(c, s), &p| (c + self.ang[p as usize].cos(), s + self.ang[p as usize].sin()));
        s.atan2(c)
    }

    /// Grows a region of aligned pixels from `seed` (8-neighbours, `tol` around the running
    /// mean angle, or around `fixed` when given). Marks the pixels used.
    fn grow(&self, seed: usize, tol: f32, fixed: Option<f32>, used: &mut [bool], region: &mut Vec<u32>) {
        region.clear();
        region.push(seed as u32);
        used[seed] = true;
        let (mut sc, mut ss) = (self.ang[seed].cos(), self.ang[seed].sin());
        let mut reg_ang = fixed.unwrap_or(self.ang[seed]);
        let mut i = 0;
        while i < region.len() {
            let p = region[i] as usize;
            i += 1;
            let (px, py) = ((p % self.gw) as isize, (p / self.gw) as isize);
            for dy in -1..=1isize {
                for dx in -1..=1isize {
                    let (nx, ny) = (px + dx, py + dy);
                    if nx < 0 || ny < 0 || nx >= self.gw as isize || ny >= self.gh as isize {
                        continue;
                    }
                    let n = ny as usize * self.gw + nx as usize;
                    if used[n] || self.mag[n] <= self.rho || ang_diff(self.ang[n], reg_ang) > tol {
                        continue;
                    }
                    used[n] = true;
                    region.push(n as u32);
                    if fixed.is_none() {
                        sc += self.ang[n].cos();
                        ss += self.ang[n].sin();
                        reg_ang = ss.atan2(sc);
                    }
                }
            }
        }
    }
}

/// [`fit_rect`], and when the region fails (it merged with a neighbouring structure),
/// LSD's radius reduction: keep shrinking the region around its seed and refit.
fn fit_refined(
    region: &mut Vec<u32>,
    seed: usize,
    gw: usize,
    mag: &[f32],
    ang: &[f32],
    min_len: f64,
) -> Option<Segment> {
    let mean_ang = |r: &[u32]| {
        let (c, s) =
            r.iter().fold((0.0f32, 0.0f32), |(c, s), &p| (c + ang[p as usize].cos(), s + ang[p as usize].sin()));
        s.atan2(c)
    };
    if let Some(s) = fit_rect(region, gw, mag, ang, mean_ang(region), min_len) {
        return Some(s);
    }
    let (sx, sy) = ((seed % gw) as f64, (seed / gw) as f64);
    let dist2 = |p: u32| {
        let p = p as usize;
        let (dx, dy) = ((p % gw) as f64 - sx, (p / gw) as f64 - sy);
        dx * dx + dy * dy
    };
    let mut radius = region.iter().map(|&p| dist2(p)).fold(0.0, f64::max).sqrt();
    for _ in 0..8 {
        radius *= 0.75;
        if 2.0 * radius < min_len {
            return None;
        }
        let r2 = radius * radius;
        region.retain(|&p| dist2(p) <= r2);
        if (region.len() as f64) < min_len {
            return None;
        }
        if let Some(s) = fit_rect(region, gw, mag, ang, mean_ang(region), min_len) {
            return Some(s);
        }
    }
    None
}

/// Joins collinear fragments of one straight edge (broken by noise or occluders): same
/// direction within 1 degree, both ends of the shorter one within 1.5 px of the longer's
/// line, gap at most 4% of the long image edge.
fn merge_collinear(mut segs: Vec<Segment>, long: f64) -> Vec<Segment> {
    segs.sort_by(|a, b| b.length().total_cmp(&a.length()));
    let max_gap = 0.04 * long;
    let mut i = 0;
    while i < segs.len() {
        let mut j = i + 1;
        while j < segs.len() {
            let (a, b) = (segs[i], segs[j]);
            let la = a.length();
            let (ux, uy) = ((a.x2 - a.x1) / la, (a.y2 - a.y1) / la);
            let dot_dir = ((b.x2 - b.x1) * ux + (b.y2 - b.y1) * uy).abs() / b.length();
            let perp = |x: f64, y: f64| ((x - a.x1) * uy - (y - a.y1) * ux).abs();
            if dot_dir < 1.0f64.to_radians().cos() || perp(b.x1, b.y1) > 1.5 || perp(b.x2, b.y2) > 1.5 {
                j += 1;
                continue;
            }
            let t = |x: f64, y: f64| (x - a.x1) * ux + (y - a.y1) * uy;
            let (tb1, tb2) = (t(b.x1, b.y1), t(b.x2, b.y2));
            let (bmin, bmax) = (tb1.min(tb2), tb1.max(tb2));
            let gap = (bmin - la).max(-bmax).max(0.0);
            if gap > max_gap {
                j += 1;
                continue;
            }
            // Refit through the four endpoints weighted by the segment lengths.
            let lb = b.length();
            let pts = [(a.x1, a.y1, la), (a.x2, a.y2, la), (b.x1, b.y1, lb), (b.x2, b.y2, lb)];
            let wsum: f64 = pts.iter().map(|p| p.2).sum();
            let cx = pts.iter().map(|p| p.0 * p.2).sum::<f64>() / wsum;
            let cy = pts.iter().map(|p| p.1 * p.2).sum::<f64>() / wsum;
            let (mut ixx, mut iyy, mut ixy) = (0.0, 0.0, 0.0);
            for p in &pts {
                let (dx, dy) = (p.0 - cx, p.1 - cy);
                ixx += p.2 * dx * dx;
                iyy += p.2 * dy * dy;
                ixy += p.2 * dx * dy;
            }
            let th = 0.5 * (2.0 * ixy).atan2(ixx - iyy);
            let (dx, dy) = (th.cos(), th.sin());
            let proj: Vec<f64> = pts.iter().map(|p| (p.0 - cx) * dx + (p.1 - cy) * dy).collect();
            let (lo, hi) =
                (proj.iter().cloned().fold(f64::MAX, f64::min), proj.iter().cloned().fold(f64::MIN, f64::max));
            segs[i] = Segment { x1: cx + lo * dx, y1: cy + lo * dy, x2: cx + hi * dx, y2: cy + hi * dy };
            segs.remove(j);
            j = i + 1;
        }
        i += 1;
    }
    segs
}

/// Principal-axis rectangle of a grown region; `None` unless it is a long, thin, dense line.
fn fit_rect(region: &[u32], gw: usize, mag: &[f32], ang: &[f32], reg_ang: f32, min_len: f64) -> Option<Segment> {
    let (mut sw, mut sx, mut sy) = (0.0f64, 0.0f64, 0.0f64);
    for &p in region {
        let p = p as usize;
        let m = f64::from(mag[p]);
        sw += m;
        sx += m * ((p % gw) as f64 + 0.5);
        sy += m * ((p / gw) as f64 + 0.5);
    }
    let (cx, cy) = (sx / sw, sy / sw);
    let (mut ixx, mut iyy, mut ixy) = (0.0f64, 0.0f64, 0.0f64);
    for &p in region {
        let p = p as usize;
        let m = f64::from(mag[p]);
        let dx = (p % gw) as f64 + 0.5 - cx;
        let dy = (p / gw) as f64 + 0.5 - cy;
        ixx += m * dx * dx;
        iyy += m * dy * dy;
        ixy += m * dx * dy;
    }
    let theta = 0.5 * (2.0 * ixy).atan2(ixx - iyy);
    let (dxv, dyv) = (theta.cos(), theta.sin());
    // The principal axis must agree with the level-line orientation (mod pi).
    let la = f64::from(reg_ang);
    let agree = (dxv * la.cos() + dyv * la.sin()).abs();
    if agree < f64::from(TAU).cos() {
        return None;
    }
    let (mut lmin, mut lmax, mut wmin, mut wmax) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for &p in region {
        let p = p as usize;
        let dx = (p % gw) as f64 + 0.5 - cx;
        let dy = (p / gw) as f64 + 0.5 - cy;
        let l = dx * dxv + dy * dyv;
        let wv = -dx * dyv + dy * dxv;
        lmin = lmin.min(l);
        lmax = lmax.max(l);
        wmin = wmin.min(wv);
        wmax = wmax.max(wv);
    }
    let length = lmax - lmin + 1.0;
    let width = wmax - wmin + 1.0;
    if length < min_len || width > (0.15 * length).max(4.0) {
        return None;
    }
    let density = region.len() as f64 / (length * width);
    if density < 0.6 {
        return None;
    }
    // Precision: most region pixels must be close to the region's mean angle.
    let close = region.iter().filter(|&&p| ang_diff(ang[p as usize], reg_ang) < TAU * 0.5).count();
    if (close as f64) < 0.6 * region.len() as f64 {
        return None;
    }
    Some(Segment { x1: cx + lmin * dxv, y1: cy + lmin * dyv, x2: cx + lmax * dxv, y2: cy + lmax * dyv })
}

// ---------------------------------------------------------------------------
// Small linear algebra.
// ---------------------------------------------------------------------------

type V3 = [f64; 3];
type M3 = [[f64; 3]; 3];

fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn norm(a: V3) -> V3 {
    let n = dot(a, a).sqrt();
    if n > 0.0 {
        [a[0] / n, a[1] / n, a[2] / n]
    } else {
        a
    }
}

fn mul(a: &M3, b: &M3) -> M3 {
    let mut r = [[0.0; 3]; 3];
    for (i, row) in r.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

fn apply(m: &M3, v: V3) -> V3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

fn inverse(m: &M3) -> Option<M3> {
    let c0 = cross(m[1], m[2]);
    let det = dot(m[0], c0);
    if det.abs() < 1e-15 || !det.is_finite() {
        return None;
    }
    let c1 = cross(m[2], m[0]);
    let c2 = cross(m[0], m[1]);
    // inverse = adj / det, adj columns are the cross products.
    Some([
        [c0[0] / det, c1[0] / det, c2[0] / det],
        [c0[1] / det, c1[1] / det, c2[1] / det],
        [c0[2] / det, c1[2] / det, c2[2] / det],
    ])
}

/// Eigenvector of the smallest eigenvalue of a symmetric 3x3 matrix (Jacobi).
fn smallest_eigvec(mut a: M3) -> V3 {
    let mut v: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..50 {
        let (mut p, mut q, mut big) = (0, 1, a[0][1].abs());
        for (i, j) in [(0usize, 2usize), (1, 2)] {
            if a[i][j].abs() > big {
                big = a[i][j].abs();
                p = i;
                q = j;
            }
        }
        if big < 1e-18 {
            break;
        }
        let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
        let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
        let t = if theta == 0.0 { 1.0 } else { t };
        let c = 1.0 / (t * t + 1.0).sqrt();
        let s = t * c;
        let mut j: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        j[p][p] = c;
        j[q][q] = c;
        j[p][q] = s;
        j[q][p] = -s;
        let jt = transpose(&j);
        a = mul(&mul(&jt, &a), &j);
        v = mul(&v, &j);
    }
    let k = (0..3).min_by(|&i, &j| a[i][i].total_cmp(&a[j][j])).unwrap_or(0);
    norm([v[0][k], v[1][k], v[2][k]])
}

fn transpose(m: &M3) -> M3 {
    [[m[0][0], m[1][0], m[2][0]], [m[0][1], m[1][1], m[2][1]], [m[0][2], m[1][2], m[2][2]]]
}

fn rot_z(a: f64) -> M3 {
    // Content turned counter-clockwise (as displayed, y down) by `a` radians.
    let (s, c) = a.sin_cos();
    [[c, s, 0.0], [-s, c, 0.0], [0.0, 0.0, 1.0]]
}

fn rot_x(b: f64) -> M3 {
    let (s, c) = b.sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, s], [0.0, -s, c]]
}

fn rot_y(p: f64) -> M3 {
    let (s, c) = p.sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

// ---------------------------------------------------------------------------
// Lines in camera-normalized coordinates.
// ---------------------------------------------------------------------------

/// A segment as a homogeneous line in camera-normalized coordinates (centred, / f).
#[derive(Debug, Clone, Copy)]
struct Line {
    /// Unit 2D normal in (l[0], l[1]).
    l: V3,
    /// Midpoint, normalized.
    mid: V3,
    /// Length in pixels (weight).
    len: f64,
    /// Direction angle (-90, 90], degrees, y down.
    angle: f64,
}

fn to_line(s: &Segment, cx: f64, cy: f64, f: f64) -> Line {
    let a = [(s.x1 - cx) / f, (s.y1 - cy) / f, 1.0];
    let b = [(s.x2 - cx) / f, (s.y2 - cy) / f, 1.0];
    let mut l = cross(a, b);
    let n = l[0].hypot(l[1]);
    if n > 0.0 {
        l = [l[0] / n, l[1] / n, l[2] / n];
    }
    let mid = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5, 1.0];
    Line { l, mid, len: s.length(), angle: s.angle_deg() }
}

/// Angle (radians) between `line` and the line through its midpoint and `vp`.
fn vp_error(line: &Line, vp: V3) -> f64 {
    let t = cross(line.mid, vp);
    let n = t[0].hypot(t[1]);
    if n < 1e-12 {
        return 0.0;
    }
    let c = ((t[0] * line.l[0] + t[1] * line.l[1]) / n).abs().min(1.0);
    c.acos()
}

/// A vanishing point (unit direction in camera space) and its support.
#[derive(Debug, Clone, Copy)]
struct Vp {
    dir: V3,
    support: f64,
    count: usize,
}

/// RANSAC + least-squares vanishing point of `lines`. `accept` filters candidate points
/// (e.g. orthogonality to the vertical one).
fn find_vp(lines: &[Line], tol: f64, min_support: f64, min_spread: f64, accept: &dyn Fn(V3) -> bool) -> Option<Vp> {
    if lines.len() < 2 {
        return None;
    }
    let mut order: Vec<usize> = (0..lines.len()).collect();
    order.sort_by(|&a, &b| lines[b].len.total_cmp(&lines[a].len));
    order.truncate(60);
    let score = |vp: V3| -> f64 { lines.iter().filter(|l| vp_error(l, vp) < tol).map(|l| l.len).sum() };
    let mut best: Option<(f64, V3)> = None;
    for (i, &a) in order.iter().enumerate() {
        for &b in &order[i + 1..] {
            let vp = norm(cross(lines[a].l, lines[b].l));
            if !vp.iter().all(|v| v.is_finite()) || dot(vp, vp) < 0.5 || !accept(vp) {
                continue;
            }
            // Skip (nearly) collinear pairs: they pin no vanishing point.
            let d = (dot(lines[a].l, lines[b].mid)).abs();
            if d < min_spread {
                continue;
            }
            let s = score(vp);
            if best.is_none_or(|(bs, _)| s > bs) {
                best = Some((s, vp));
            }
        }
    }
    let (_, mut vp) = best?;
    // Least-squares refinement on the inliers (twice: inliers may change).
    for _ in 0..2 {
        let inl: Vec<&Line> = lines.iter().filter(|l| vp_error(l, vp) < tol).collect();
        if inl.len() < 2 {
            return None;
        }
        let mut m = [[0.0; 3]; 3];
        for l in &inl {
            for (i, row) in m.iter_mut().enumerate() {
                for (j, v) in row.iter_mut().enumerate() {
                    *v += l.len * l.l[i] * l.l[j];
                }
            }
        }
        let refined = smallest_eigvec(m);
        if !accept(refined) {
            break;
        }
        vp = refined;
    }
    let inl: Vec<&Line> = lines.iter().filter(|l| vp_error(l, vp) < tol).collect();
    let support: f64 = inl.iter().map(|l| l.len).sum();
    // Two lines alone must be long (a box edge pair is not a building).
    if inl.len() < 2 || support < min_support || (inl.len() < 3 && support < 2.0 * min_support) {
        return None;
    }
    // Inliers must not all lie on one line.
    let first = inl[0];
    let spread = inl.iter().map(|l| dot(first.l, l.mid).abs()).fold(0.0, f64::max);
    if spread < min_spread {
        return None;
    }
    Some(Vp { dir: vp, support, count: inl.len() })
}

/// Horizon roll from near-horizontal segments that agree within 1 degree: (angle deg,
/// support px). The agreeing group must be long enough and clearly dominate any other
/// group of near-horizontal segments (string lights, converging rooflines ...).
fn horizon_angle(lines: &[Line], width: f64) -> Option<(f64, f64)> {
    let cand: Vec<&Line> = lines.iter().filter(|l| l.angle.abs() <= 15.0).collect();
    let window =
        |a: f64, half: f64| -> f64 { cand.iter().filter(|o| (o.angle - a).abs() <= half).map(|o| o.len).sum() };
    let mut best = (0.0, 0.0);
    for c in &cand {
        let s = window(c.angle, 1.0);
        if s > best.0 {
            best = (s, c.angle);
        }
    }
    if best.0 <= 0.0 {
        return None;
    }
    let mean_of = |centre: f64, half: f64| {
        let (sw, sa) = cand
            .iter()
            .filter(|o| (o.angle - centre).abs() <= half)
            .fold((0.0, 0.0), |(sw, sa), o| (sw + o.len, sa + o.len * o.angle));
        (sa / sw, sw)
    };
    let (mean, _) = mean_of(best.1, 1.0);
    let (mean, support) = mean_of(mean, 0.6);
    let second = cand.iter().filter(|c| (c.angle - mean).abs() > 2.0).map(|c| window(c.angle, 1.0)).fold(0.0, f64::max);
    if support < 0.25 * width * (1.0 + mean.abs() / 5.0) || support < 1.5 * second {
        return None;
    }
    Some((mean, support))
}

/// Roll (radians, CCW correction) that makes the vertical direction `d` vertical.
fn roll_of_vertical(d: V3) -> f64 {
    let d = if d[1] < 0.0 { [-d[0], -d[1], -d[2]] } else { d };
    (-d[0]).atan2(d[1])
}

/// Pitch (radians) left after the roll.
fn pitch_of_vertical(d: V3) -> f64 {
    let d = if d[1] < 0.0 { [-d[0], -d[1], -d[2]] } else { d };
    d[2].atan2(d[0].hypot(d[1]))
}

// ---------------------------------------------------------------------------
// Solve.
// ---------------------------------------------------------------------------

/// Camera / frame of the detection render.
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    /// Oriented size in pixels.
    pub width: u32,
    pub height: u32,
    /// Focal length in pixels of this frame.
    pub focal_px: f64,
}

impl Frame {
    /// Focal length from a 35 mm equivalent (diagonal-based, like Lightroom).
    pub fn new(width: u32, height: u32, focal_35mm: Option<f64>) -> Frame {
        let f35 = focal_35mm.filter(|f| f.is_finite() && (8.0..=2000.0).contains(f)).unwrap_or(DEFAULT_FOCAL_35MM);
        let diag = f64::from(width).hypot(f64::from(height));
        Frame { width, height, focal_px: f35 * diag / 43.266_615 }
    }
}

/// An Upright solve in the oriented frame.
#[derive(Debug, Clone, PartialEq)]
pub struct OrientedSolve {
    /// Source -> corrected, oriented pixels centred on the image centre.
    pub source_to_corrected: [[f64; 3]; 3],
    /// Roll part, degrees, positive = content turned counter-clockwise.
    pub rotation_deg: f64,
}

/// Why no solution was produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoSolution(pub String);

struct Evidence {
    lines: Vec<Line>,
    vertical: Option<Vp>,
    horizon: Option<(f64, f64)>,
}

/// The best "parallel lines" explanation (vanishing point at infinity) of the lines near
/// `vp`, refined twice.
fn parallel_alternative(lines: &[Line], vp: V3, tol: f64) -> Option<Vp> {
    let mean_dir = |inl: &[&Line], reference: (f64, f64)| -> V3 {
        let (mut sx, mut sy) = (0.0, 0.0);
        for l in inl {
            let (mut dx, mut dy) = (-l.l[1], l.l[0]);
            if dx * reference.0 + dy * reference.1 < 0.0 {
                dx = -dx;
                dy = -dy;
            }
            sx += l.len * dx;
            sy += l.len * dy;
        }
        norm([sx, sy, 0.0])
    };
    let inl: Vec<&Line> = lines.iter().filter(|l| vp_error(l, vp) < tol).collect();
    let first = inl.first()?;
    let mut inf = mean_dir(&inl, (-first.l[1], first.l[0]));
    for _ in 0..2 {
        let near: Vec<&Line> = lines.iter().filter(|l| vp_error(l, inf) < tol).collect();
        if near.is_empty() {
            return None;
        }
        inf = mean_dir(&near, (inf[0], inf[1]));
    }
    let near: Vec<&Line> = lines.iter().filter(|l| vp_error(l, inf) < tol).collect();
    let support: f64 = near.iter().map(|l| l.len).sum();
    (near.len() >= 2).then_some(Vp { dir: inf, support, count: near.len() })
}

/// Line length a vertical vanishing point needs, growing with the correction it implies:
/// a quarter of the frame height for a straight photo, more for strong roll / keystone.
fn vertical_evidence_needed(v: V3, h: f64) -> f64 {
    let roll = roll_of_vertical(v).abs().to_degrees();
    let pitch = pitch_of_vertical(v).abs().to_degrees();
    0.25 * h * (1.0 + roll / 5.0 + pitch / 10.0)
}

/// Model selection for the vertical vanishing point: lines that are parallel in the image
/// (no keystone) are the default; a finite vanishing point is kept only when it explains
/// clearly more (15%+) line length than the best parallel direction and has the evidence
/// its correction needs. When the parallel model wins but the finite point was credible,
/// its in-plane direction (more exact for slightly converging lines) gives the roll.
fn select_vertical(lines: &[Line], finite: Vp, tol: f64, h: f64) -> Option<Vp> {
    let credible = |v: &Vp| v.support >= vertical_evidence_needed(v.dir, h);
    let parallel = parallel_alternative(lines, finite.dir, tol);
    let finite_ok = credible(&finite);
    match parallel {
        Some(p) if !finite_ok || p.support >= finite.support / 1.15 => {
            let p = if finite_ok {
                let r = roll_of_vertical(finite.dir);
                Vp { dir: [-r.sin(), r.cos(), 0.0], ..p }
            } else {
                p
            };
            credible(&p).then_some(p)
        }
        _ => finite_ok.then_some(finite),
    }
}

fn gather(set: &LineSet, f: f64) -> Evidence {
    let (w, h) = (f64::from(set.width), f64::from(set.height));
    let (cx, cy) = (w * 0.5, h * 0.5);
    let lines: Vec<Line> = set.segments.iter().map(|s| to_line(s, cx, cy, f)).collect();
    let verticals: Vec<Line> = lines.iter().copied().filter(|l| 90.0 - l.angle.abs() <= 35.0).collect();
    let spread = 0.04 * w.min(h) / f;
    let tol = 1.0f64.to_radians();
    let plausible = |v: V3| {
        // Within 15 degrees of upright in the image plane, tilted at most 35 degrees.
        let d = if v[1] < 0.0 { [-v[0], -v[1], -v[2]] } else { v };
        roll_of_vertical(d).abs() <= 15f64.to_radians() && pitch_of_vertical(d).abs() <= 35f64.to_radians()
    };
    let vertical = find_vp(&verticals, tol, 0.2 * h, spread, &plausible)
        .and_then(|v| select_vertical(&verticals, v, tol, h))
        .filter(|v| plausible(v.dir));
    let horizon = horizon_angle(&lines, w);
    Evidence { lines, vertical, horizon }
}

/// Dominant horizontal vanishing point, ~orthogonal to the vertical direction `dv`.
fn horizontal_vp(ev: &Evidence, dv: V3, frame: &Frame) -> Option<Vp> {
    let (w, h) = (f64::from(frame.width), f64::from(frame.height));
    let f = frame.focal_px;
    let cands: Vec<Line> =
        ev.lines.iter().copied().filter(|l| l.angle.abs() <= 50.0 && vp_error(l, dv) > 3f64.to_radians()).collect();
    let spread = 0.04 * w.min(h) / f;
    let tol = 1.0f64.to_radians();
    let orthogonal = |v: V3| dot(v, dv).abs() < 12f64.to_radians().sin();
    let finite = find_vp(&cands, tol, 0.3 * w, spread, &orthogonal)?;
    match parallel_alternative(&cands, finite.dir, tol) {
        Some(p) if p.support >= finite.support / 1.15 && orthogonal(p.dir) => Some(p),
        _ => Some(finite),
    }
}

fn k_mat(f: f64) -> M3 {
    [[f, 0.0, 0.0], [0.0, f, 0.0], [0.0, 0.0, 1.0]]
}

fn k_inv(f: f64) -> M3 {
    [[1.0 / f, 0.0, 0.0], [0.0, 1.0 / f, 0.0], [0.0, 0.0, 1.0]]
}

/// `K R K^-1`, then `post` (2D, pixels) — source -> corrected, centred pixels.
fn camera_homography(r: &M3, f: f64, post: &M3) -> M3 {
    mul(post, &mul(&k_mat(f), &mul(r, &k_inv(f))))
}

/// Shear `y' = y - k x` that levels a direction `d` (camera space, after rotation) without
/// touching verticals.
fn level_shear(d: V3) -> M3 {
    let k = if d[0].abs() > 1e-9 { d[1] / d[0] } else { 0.0 };
    let k = k.clamp(-0.5, 0.5);
    [[1.0, 0.0, 0.0], [-k, 1.0, 0.0], [0.0, 0.0, 1.0]]
}

/// Corners of the source frame mapped by `g` (centred pixels); `None` when one falls
/// behind the virtual camera.
fn warped_corners(g: &M3, w: f64, h: f64) -> Option<[(f64, f64); 4]> {
    let mut out = [(0.0, 0.0); 4];
    for (i, (x, y)) in
        [(-w / 2.0, -h / 2.0), (w / 2.0, -h / 2.0), (w / 2.0, h / 2.0), (-w / 2.0, h / 2.0)].into_iter().enumerate()
    {
        let p = apply(g, [x, y, 1.0]);
        if p[2] <= 1e-6 {
            return None;
        }
        out[i] = (p[0] / p[2], p[1] / p[2]);
    }
    Some(out)
}

fn quad_area(q: &[(f64, f64); 4]) -> f64 {
    let mut a = 0.0;
    for i in 0..4 {
        let (x1, y1) = q[i];
        let (x2, y2) = q[(i + 1) % 4];
        a += x1 * y2 - x2 * y1;
    }
    a * 0.5
}

/// Largest edge stretch of the warped frame relative to the source edges (>= 1).
fn max_stretch(q: &[(f64, f64); 4], w: f64, h: f64) -> f64 {
    let mut worst = 1.0f64;
    for i in 0..4 {
        let (x1, y1) = q[i];
        let (x2, y2) = q[(i + 1) % 4];
        let src = if i % 2 == 0 { w } else { h };
        let r = (x2 - x1).hypot(y2 - y1) / src;
        worst = worst.max(r).max(1.0 / r.max(1e-9));
    }
    worst
}

/// Re-centres (source centre -> frame centre) and rescales (same area) a source->corrected
/// map; `None` if it folds or flips the frame.
fn normalize(g: M3, w: f64, h: f64) -> Option<M3> {
    let c = apply(&g, [0.0, 0.0, 1.0]);
    if c[2] <= 1e-9 {
        return None;
    }
    let (tx, ty) = (c[0] / c[2], c[1] / c[2]);
    let t = [[1.0, 0.0, -tx], [0.0, 1.0, -ty], [0.0, 0.0, 1.0]];
    let g1 = mul(&t, &g);
    let q = warped_corners(&g1, w, h)?;
    let area = quad_area(&q);
    if area <= 0.0 || !is_convex(&q) {
        return None;
    }
    let s = (w * h / area).sqrt();
    let g2 = mul(&[[s, 0.0, 0.0], [0.0, s, 0.0], [0.0, 0.0, 1.0]], &g1);
    let z = g2[2][2];
    if z.abs() < 1e-12 {
        return None;
    }
    let mut out = g2;
    for row in &mut out {
        for v in row.iter_mut() {
            *v /= z;
        }
    }
    Some(out)
}

fn is_convex(q: &[(f64, f64); 4]) -> bool {
    let mut sign = 0.0f64;
    for i in 0..4 {
        let (x1, y1) = q[i];
        let (x2, y2) = q[(i + 1) % 4];
        let (x3, y3) = q[(i + 2) % 4];
        let c = (x2 - x1) * (y3 - y2) - (y2 - y1) * (x3 - x2);
        if c.abs() < 1e-12 {
            continue;
        }
        if sign == 0.0 {
            sign = c.signum();
        } else if c.signum() != sign {
            return false;
        }
    }
    true
}

/// Lightroom-Auto style damping: full correction up to `knee` degrees, half beyond, capped.
fn damp(a: f64, knee: f64, cap: f64) -> f64 {
    let d = a.abs().to_degrees();
    let out = if d <= knee { d } else { (knee + 0.5 * (d - knee)).min(cap) };
    out.to_radians().copysign(a)
}

/// Solves `mode` from detected lines (and Guided `guides` in oriented pixels).
pub fn solve(set: &LineSet, mode: UprightMode, guides: &[Segment], frame: &Frame) -> Result<OrientedSolve, NoSolution> {
    let (w, h) = (f64::from(frame.width), f64::from(frame.height));
    let f = frame.focal_px;
    let id: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let finish = |r: M3, post: M3, roll: f64| -> Result<OrientedSolve, NoSolution> {
        let g = camera_homography(&r, f, &post);
        let g = normalize(g, w, h).ok_or_else(|| NoSolution("The correction would be too extreme".into()))?;
        let q = warped_corners(&g, w, h).ok_or_else(|| NoSolution("The correction would be too extreme".into()))?;
        if max_stretch(&q, w, h) > 4.0 {
            return Err(NoSolution("The correction would be too extreme".into()));
        }
        Ok(OrientedSolve { source_to_corrected: g, rotation_deg: roll.to_degrees() })
    };
    match mode {
        UprightMode::Off => Err(NoSolution("Upright is off".into())),
        UprightMode::Guided => solve_guided(guides, frame, finish),
        _ => {
            let ev = gather(set, f);
            let roll_v = ev.vertical.map(|v| roll_of_vertical(v.dir));
            // Horizon angle (y down, positive = descends right) = CCW correction.
            let roll_h = ev.horizon.map(|(a, _)| a.to_radians());
            let strong_vertical = ev.vertical.is_some_and(|v| v.count >= 3 && v.support >= 0.5 * h);
            // Horizon and verticals usually agree; when they do not, only a clearly
            // stronger side decides (else there is no reliable level).
            let level_roll = match (ev.horizon, ev.vertical) {
                (Some((_, sh)), Some(v)) => {
                    let (rh, rv) = (roll_h.unwrap_or(0.0), roll_v.unwrap_or(0.0));
                    if (rh - rv).abs() <= 3f64.to_radians() {
                        Some(if strong_vertical { rv } else { rh })
                    } else if strong_vertical && v.support >= 2.0 * sh {
                        Some(rv)
                    } else if sh >= 2.0 * v.support {
                        Some(rh)
                    } else {
                        None
                    }
                }
                (Some(_), None) => roll_h,
                (None, _) => roll_v,
            };
            match mode {
                UprightMode::Level => {
                    let roll = level_roll.ok_or_else(|| NoSolution("No level lines found".into()))?;
                    finish(rot_z(roll), id, roll)
                }
                UprightMode::Vertical => {
                    let Some(v) = ev.vertical else {
                        // No usable verticals: level only (as Auto / Full do).
                        let roll = level_roll.ok_or_else(|| NoSolution("No vertical lines found".into()))?;
                        return finish(rot_z(roll), id, roll);
                    };
                    let roll = roll_of_vertical(v.dir);
                    let pitch = pitch_of_vertical(v.dir);
                    finish(mul(&rot_x(pitch), &rot_z(roll)), id, roll)
                }
                UprightMode::Full | UprightMode::Auto => {
                    let auto = mode == UprightMode::Auto;
                    let Some(v) = ev.vertical else {
                        // Without verticals both fall back to leveling (as Lightroom does).
                        let roll = level_roll.ok_or_else(|| NoSolution("No straight lines found".into()))?;
                        return finish(rot_z(roll), id, roll);
                    };
                    let roll = roll_of_vertical(v.dir);
                    let pitch = pitch_of_vertical(v.dir);
                    let pitch_applied = if auto { damp(pitch, 10.0, 20.0) } else { pitch };
                    let rv = mul(&rot_x(pitch), &rot_z(roll));
                    let hv = horizontal_vp(&ev, v.dir, frame);
                    let (yaw, post) = match hv {
                        Some(hp) => {
                            let d = apply(&rv, hp.dir);
                            let d = if d[0] < 0.0 { [-d[0], -d[1], -d[2]] } else { d };
                            let yaw = d[2].atan2(d[0]);
                            if auto {
                                // Only a well-supported facade, gently (Lightroom's Auto
                                // rarely turns the view sideways).
                                let strong = hp.count >= 4 && hp.support >= 0.5 * w;
                                (if strong { damp(yaw, 3.0, 6.0) } else { 0.0 }, id)
                            } else {
                                let after = apply(&rot_y(yaw), d);
                                (yaw, level_shear(after))
                            }
                        }
                        None => (0.0, id),
                    };
                    if !auto {
                        return finish(mul(&rot_y(yaw), &rv), post, roll);
                    }
                    // Auto: scale the solve back until no edge stretches more than 1.6x.
                    let mut k = 1.0;
                    for _ in 0..12 {
                        let r = mul(&rot_y(yaw * k), &mul(&rot_x(pitch_applied * k), &rot_z(roll)));
                        let g = normalize(camera_homography(&r, f, &id), w, h);
                        let ok = g.and_then(|g| warped_corners(&g, w, h)).is_some_and(|q| max_stretch(&q, w, h) <= 1.6);
                        if ok {
                            return finish(r, id, roll);
                        }
                        k *= 0.8;
                    }
                    finish(rot_z(roll), id, roll)
                }
                UprightMode::Off | UprightMode::Guided => unreachable!(),
            }
        }
    }
}

fn solve_guided(
    guides: &[Segment],
    frame: &Frame,
    finish: impl Fn(M3, M3, f64) -> Result<OrientedSolve, NoSolution>,
) -> Result<OrientedSolve, NoSolution> {
    let (w, h) = (f64::from(frame.width), f64::from(frame.height));
    let f = frame.focal_px;
    let id: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let usable: Vec<&Segment> = guides.iter().filter(|g| g.length() >= 0.02 * w.max(h)).collect();
    if usable.len() < 2 {
        return Err(NoSolution("Draw at least 2 guides".into()));
    }
    let lines: Vec<Line> = usable.iter().map(|s| to_line(s, w * 0.5, h * 0.5, f)).collect();
    let (vert, horiz): (Vec<Line>, Vec<Line>) = lines.iter().partition(|l| l.angle.abs() > 45.0);
    let vp_of = |ls: &[Line]| -> Option<V3> {
        if ls.len() < 2 {
            return None;
        }
        let mut m = [[0.0; 3]; 3];
        for l in ls {
            for (i, row) in m.iter_mut().enumerate() {
                for (j, v) in row.iter_mut().enumerate() {
                    *v += l.l[i] * l.l[j];
                }
            }
        }
        let v = smallest_eigvec(m);
        v.iter().all(|x| x.is_finite()).then_some(v)
    };
    let dv = vp_of(&vert);
    let dh = vp_of(&horiz);
    // Vertical part: exact vertical VP, else the single vertical guide's rotation.
    let (rv, roll) = match (dv, vert.first()) {
        (Some(d), _) => {
            let roll = roll_of_vertical(d);
            (mul(&rot_x(pitch_of_vertical(d)), &rot_z(roll)), roll)
        }
        (None, Some(l)) => {
            // Direction of the guide, pointing down.
            let d = [-l.l[1], l.l[0], 0.0];
            let roll = roll_of_vertical(d);
            (rot_z(roll), roll)
        }
        (None, None) => {
            // Horizontal guides only.
            let Some(d) = dh else {
                return Err(NoSolution("Draw at least 2 guides".into()));
            };
            let d = if d[0] < 0.0 { [-d[0], -d[1], -d[2]] } else { d };
            // Roll the horizon level, then yaw the vanishing point to infinity.
            let roll = d[1].atan2(d[0]);
            let r1 = rot_z(roll);
            let d1 = apply(&r1, d);
            let yaw = d1[2].atan2(d1[0]);
            return finish(mul(&rot_y(yaw), &r1), id, roll);
        }
    };
    match (dh, horiz.first()) {
        (Some(d), _) => {
            // Horizontal vanishing point to infinity sideways (yaw), then level (shear).
            let d = apply(&rv, d);
            let d = if d[0] < 0.0 { [-d[0], -d[1], -d[2]] } else { d };
            let yaw = d[2].atan2(d[0]);
            let after = apply(&rot_y(yaw), d);
            finish(mul(&rot_y(yaw), &rv), level_shear(after), roll)
        }
        (None, Some(l)) => {
            // One horizontal guide: level it with a shear (verticals stay vertical). Lines
            // map by R^-T = R for a rotation.
            let lr = apply(&rv, l.l);
            finish(rv, level_shear([-lr[1], lr[0], 0.0]), roll)
        }
        (None, None) => finish(rv, id, roll),
    }
}

// ---------------------------------------------------------------------------
// Frames: oriented pixels <-> sensor normalized.
// ---------------------------------------------------------------------------

/// Oriented normalized -> sensor (un-oriented) normalized, as a 3x3 affine
/// (`develop::parity::orientation_map`).
fn orient_to_sensor(o: u8) -> M3 {
    let m = crate::develop::parity::orientation_map(o);
    [[m[0], m[1], m[2]], [m[3], m[4], m[5]], [0.0, 0.0, 1.0]]
}

/// Oriented normalized -> centred oriented pixels.
fn norm_to_px(w: f64, h: f64) -> M3 {
    [[w, 0.0, -w / 2.0], [0.0, h, -h / 2.0], [0.0, 0.0, 1.0]]
}

/// `UprightSolution.matrix` (corrected -> source, sensor frame normalized, row-major,
/// m[8] = 1) of an oriented solve.
pub fn sensor_matrix(solve: &OrientedSolve, frame: &Frame, orientation: u8) -> Option<[f64; 9]> {
    let (w, h) = (f64::from(frame.width), f64::from(frame.height));
    let c2s = inverse(&solve.source_to_corrected)?;
    let a = norm_to_px(w, h);
    let a_inv = inverse(&a)?;
    let o = orient_to_sensor(orientation);
    let o_inv = inverse(&o)?;
    let m = mul(&o, &mul(&a_inv, &mul(&c2s, &mul(&a, &o_inv))));
    let z = m[2][2];
    if z.abs() < 1e-12 {
        return None;
    }
    let out =
        [m[0][0] / z, m[0][1] / z, m[0][2] / z, m[1][0] / z, m[1][1] / z, m[1][2] / z, m[2][0] / z, m[2][1] / z, 1.0];
    out.iter().all(|v| v.is_finite()).then_some(out)
}

/// Guides (sensor normalized) -> oriented pixel segments.
pub fn guides_to_oriented(guides: &[UprightGuide], frame: &Frame, orientation: u8) -> Vec<Segment> {
    let (w, h) = (f64::from(frame.width), f64::from(frame.height));
    let Some(s2o) = inverse(&orient_to_sensor(orientation)) else {
        return Vec::new();
    };
    let map = |p: NormPoint| {
        let q = apply(&s2o, [f64::from(p.x), f64::from(p.y), 1.0]);
        (q[0] * w, q[1] * h)
    };
    guides
        .iter()
        .map(|g| {
            let (x1, y1) = map(g.start);
            let (x2, y2) = map(g.end);
            Segment { x1, y1, x2, y2 }
        })
        .collect()
}

/// `CropSettings.angle` that straightens like a Level solve's `rotation_deg` ("Auto
/// straighten" in the crop tool). The crop angle turns the crop frame clockwise in the
/// un-oriented source, so mirrored orientations (2, 4, 5, 7) flip the sign.
pub fn crop_angle_for_rotation(rotation_deg: f32, orientation: u8) -> f32 {
    let a = if matches!(orientation, 2 | 4 | 5 | 7) { -rotation_deg } else { rotation_deg };
    a.clamp(-45.0, 45.0)
}

/// 35 mm equivalent focal length from the catalog's focal length and a crop-factor guess
/// by body (Fujifilm X 1.5, Sony APS-C `ILCE-6xxx` / `NEX` / `ILCE-3500` 1.5, Canon
/// APS-C R7 / R10 / R50 / R100 1.6, else full frame). `None` without a focal length. Only
/// the aspect of a perspective solve depends on it (verticals become parallel anyway).
pub fn focal_35mm_estimate(make: CameraMake, model: Option<&str>, focal_mm: Option<f32>) -> Option<f64> {
    let f = f64::from(focal_mm.filter(|f| f.is_finite() && *f > 0.0)?);
    let m = model.unwrap_or("").to_ascii_uppercase();
    let crop = match make {
        CameraMake::Fujifilm if m.contains("GFX") => 0.79,
        CameraMake::Fujifilm => 1.5,
        CameraMake::Sony if m.starts_with("ILCE-6") || m.starts_with("NEX") || m.starts_with("ILCE-3") => 1.5,
        CameraMake::Canon if ["R7", "R10", "R50", "R100"].iter().any(|r| m.ends_with(&format!(" {r}")) || m == *r) => {
            1.6
        }
        _ => 1.0,
    };
    Some(f * crop)
}

/// The adjustments to render for line detection: the user's colour/tone (so edges look as
/// they do), without crop, transform, masks, LUT or effects.
pub fn detection_adjustments(a: &ParametricAdjustments) -> ParametricAdjustments {
    let mut d = a.clone();
    d.crop.enabled = false;
    d.transform = Default::default();
    d.masks.clear();
    d.lut = None;
    d.effects.grain.amount = 0.0;
    d.effects.vignette.amount = 0.0;
    d
}

/// One-line description of the evidence (horizon, vertical vanishing point) found on an
/// oriented RGB8 render, for evaluation tools.
pub fn diagnose(rgb: &[u8], width: u32, height: u32, focal_35mm: Option<f64>) -> String {
    let frame = Frame::new(width, height, focal_35mm);
    let set = LineSet { width, height, segments: detect_segments(&luminance(rgb, width, height), width, height) };
    let ev = gather(&set, frame.focal_px);
    let hz = ev.horizon.map_or("-".into(), |(a, s)| format!("{a:+.2} deg ({s:.0} px)"));
    let vp = ev.vertical.map_or("-".into(), |v| {
        format!(
            "roll {:+.2} pitch {:+.2} ({} lines, {:.0} px)",
            roll_of_vertical(v.dir).to_degrees(),
            pitch_of_vertical(v.dir).to_degrees(),
            v.count,
            v.support
        )
    });
    let hv = ev.vertical.and_then(|v| {
        let h = horizontal_vp(&ev, v.dir, &frame)?;
        let rv = mul(&rot_x(pitch_of_vertical(v.dir)), &rot_z(roll_of_vertical(v.dir)));
        let d = apply(&rv, h.dir);
        let d = if d[0] < 0.0 { [-d[0], -d[1], -d[2]] } else { d };
        Some(format!("yaw {:+.2} ({} lines, {:.0} px)", d[2].atan2(d[0]).to_degrees(), h.count, h.support))
    });
    format!("horizon {hz}; vertical vp {vp}; horizontal vp {}", hv.unwrap_or("-".into()))
}

/// Result of [`solve_rgb8`].
#[derive(Debug, Clone, PartialEq)]
pub struct UprightOutcome {
    pub solution: Option<UprightSolution>,
    pub message: Option<String>,
    /// Segments found (diagnostics).
    pub segments: usize,
}

/// Detects lines on an oriented RGB8 render (any size; ~[`DETECT_EDGE`] is what the
/// command uses) and solves `mode`. `guides` are in the sensor frame.
pub fn solve_rgb8(
    rgb: &[u8],
    width: u32,
    height: u32,
    orientation: u8,
    mode: UprightMode,
    guides: &[UprightGuide],
    focal_35mm: Option<f64>,
) -> UprightOutcome {
    let frame = Frame::new(width, height, focal_35mm);
    let segments = if mode == UprightMode::Guided || mode == UprightMode::Off {
        Vec::new()
    } else {
        detect_segments(&luminance(rgb, width, height), width, height)
    };
    let n = segments.len();
    let set = LineSet { width, height, segments };
    let guides_px = guides_to_oriented(guides, &frame, orientation);
    match solve(&set, mode, &guides_px, &frame) {
        Ok(s) => match sensor_matrix(&s, &frame, orientation) {
            Some(matrix) => UprightOutcome {
                solution: Some(UprightSolution {
                    mode,
                    matrix: matrix.to_vec(),
                    rotation_deg: s.rotation_deg as f32,
                    crs: Vec::new(),
                }),
                message: None,
                segments: n,
            },
            None => UprightOutcome {
                solution: None,
                message: Some("The correction would be too extreme".into()),
                segments: n,
            },
        },
        Err(NoSolution(m)) => UprightOutcome { solution: None, message: Some(m), segments: n },
    }
}

/// Maps a point of the corrected frame to the source with a sensor-frame `matrix`.
pub fn map_point(matrix: &[f64], x: f64, y: f64) -> (f64, f64) {
    let w = matrix[6] * x + matrix[7] * y + matrix[8];
    ((matrix[0] * x + matrix[1] * y + matrix[2]) / w, (matrix[3] * x + matrix[4] * y + matrix[5]) / w)
}

/// Inverse of a sensor-frame matrix (source -> corrected), row-major.
pub fn invert_matrix(matrix: &[f64]) -> Option<[f64; 9]> {
    if matrix.len() != 9 {
        return None;
    }
    let m = [[matrix[0], matrix[1], matrix[2]], [matrix[3], matrix[4], matrix[5]], [matrix[6], matrix[7], matrix[8]]];
    let i = inverse(&m)?;
    Some([i[0][0], i[0][1], i[0][2], i[1][0], i[1][1], i[1][2], i[2][0], i[2][1], i[2][2]])
}

#[cfg(test)]
#[path = "upright_tests.rs"]
mod tests;
