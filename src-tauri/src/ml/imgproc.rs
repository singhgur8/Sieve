//! Pixel-level measurements on decoded previews: luma planes, blur/sharpness metrics,
//! exposure statistics, DCT perceptual hash and the affine crop used by the landmark
//! model. Pure functions over plain buffers (unit tested; no ML).

use crate::ipc::types::ExposureStats;

/// Single-channel float image (luma 0..=255), row-major.
#[derive(Debug, Clone, PartialEq)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub px: Vec<f32>,
}

impl Gray {
    pub fn new(w: usize, h: usize) -> Self {
        Self { w, h, px: vec![0.0; w * h] }
    }

    #[inline]
    pub fn at(&self, x: usize, y: usize) -> f32 {
        self.px[y * self.w + x]
    }
}

#[inline]
fn luma_of(p: &[u8; 3]) -> f32 {
    0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
}

/// Luma of an RGB region `[x0, x1) x [y0, y1)` (clamped to the image).
pub fn luma_region(rgb: &[u8], w: usize, h: usize, x0: usize, y0: usize, x1: usize, y1: usize) -> Gray {
    let (x1, y1) = (x1.min(w), y1.min(h));
    let (x0, y0) = (x0.min(x1), y0.min(y1));
    let mut g = Gray::new(x1 - x0, y1 - y0);
    for y in y0..y1 {
        let row = &rgb[(y * w + x0) * 3..(y * w + x1) * 3];
        let out = &mut g.px[(y - y0) * g.w..(y - y0 + 1) * g.w];
        for (o, p) in out.iter_mut().zip(row.as_chunks::<3>().0) {
            *o = luma_of(p);
        }
    }
    g
}

/// Luma of the whole RGB image, downsampled by an integer box factor `k` (>= 1).
pub fn luma_box(rgb: &[u8], w: usize, h: usize, k: usize) -> Gray {
    let k = k.max(1);
    let (dw, dh) = (w / k, h / k);
    let mut g = Gray::new(dw, dh);
    let norm = 1.0 / (k * k) as f32;
    let mut acc = vec![0.0f32; dw];
    for dy in 0..dh {
        acc.iter_mut().for_each(|a| *a = 0.0);
        for y in dy * k..dy * k + k {
            let row = &rgb[y * w * 3..(y * w + dw * k) * 3];
            for (i, p) in row.as_chunks::<3>().0.iter().enumerate() {
                acc[i / k] += luma_of(p);
            }
        }
        for (o, a) in g.px[dy * dw..(dy + 1) * dw].iter_mut().zip(&acc) {
            *o = a * norm;
        }
    }
    g
}

/// Integer box downsample of a gray image (`k >= 1`).
pub fn downsample(g: &Gray, k: usize) -> Gray {
    if k <= 1 {
        return g.clone();
    }
    let (dw, dh) = (g.w / k, g.h / k);
    let mut out = Gray::new(dw, dh);
    let norm = 1.0 / (k * k) as f32;
    for dy in 0..dh {
        for dx in 0..dw {
            let mut s = 0.0;
            for y in dy * k..dy * k + k {
                for x in dx * k..dx * k + k {
                    s += g.at(x, y);
                }
            }
            out.px[dy * dw + dx] = s * norm;
        }
    }
    out
}

/// Sub-image `[x0, x1) x [y0, y1)` (clamped).
pub fn crop(g: &Gray, x0: usize, y0: usize, x1: usize, y1: usize) -> Gray {
    let (x1, y1) = (x1.min(g.w), y1.min(g.h));
    let (x0, y0) = (x0.min(x1), y0.min(y1));
    let mut out = Gray::new(x1 - x0, y1 - y0);
    for y in y0..y1 {
        out.px[(y - y0) * out.w..(y - y0 + 1) * out.w].copy_from_slice(&g.px[y * g.w + x0..y * g.w + x1]);
    }
    out
}

/// Half-width of the re-blur kernel of the Crete-Roffet metric (9 taps).
const BLUR_R: isize = 4;

/// Directional sharpness (Crete-Roffet "no-reference perceptual blur", inverted).
///
/// Re-blurs the image with a 9-tap box along direction `(dx, dy)` and measures how much
/// neighbour variation that removed: a sharp image loses most of its variation, an
/// already blurred one hardly changes. Returns `(sharpness 0..=1, mean |gradient|)`;
/// the ratio is contrast-invariant, the mean gradient says whether there was any
/// texture to judge at all.
pub fn directional_sharpness(g: &Gray, dx: isize, dy: isize) -> (f32, f32) {
    let (w, h) = (g.w as isize, g.h as isize);
    let m = BLUR_R + 1;
    let (xs, xe) = (m * dx.abs(), w - m * dx.abs());
    let (ys, ye) = (m * dy.abs(), h - m * dy.abs());
    if xe <= xs || ye <= ys {
        return (0.0, 0.0);
    }
    let blur = |x: isize, y: isize| -> f32 {
        let mut s = 0.0;
        for t in -BLUR_R..=BLUR_R {
            s += g.px[((y + t * dy) * w + x + t * dx) as usize];
        }
        s / (2 * BLUR_R + 1) as f32
    };
    let (mut s_f, mut s_v, mut n) = (0.0f64, 0.0f64, 0usize);
    for y in ys..ye {
        for x in xs..xe {
            let f0 = g.px[(y * w + x) as usize];
            let f1 = g.px[((y - dy) * w + x - dx) as usize];
            let d_f = (f0 - f1).abs();
            let d_b = (blur(x, y) - blur(x - dx, y - dy)).abs();
            s_f += d_f as f64;
            s_v += (d_f - d_b).max(0.0) as f64;
            n += 1;
        }
    }
    if s_f <= 0.0 {
        return (0.0, 0.0);
    }
    ((s_v / s_f) as f32, (s_f / n as f64) as f32)
}

/// Horizontal + vertical sharpness using running sums (fast path for whole tiles).
/// Returns `(sharp_h, sharp_v, mean |gradient|)`.
pub fn hv_sharpness(g: &Gray) -> (f32, f32, f32) {
    let (w, h) = (g.w, g.h);
    let k = (2 * BLUR_R + 1) as usize;
    if w <= k + 1 || h <= k + 1 {
        return (0.0, 0.0, 0.0);
    }
    let inv = 1.0 / k as f32;
    // Horizontal: box blur each row.
    let (mut sf_h, mut sv_h) = (0.0f64, 0.0f64);
    let mut b = vec![0.0f32; w];
    for y in 0..h {
        let row = &g.px[y * w..(y + 1) * w];
        let mut s: f32 = row[..k].iter().sum();
        b[k / 2] = s * inv;
        for x in k / 2 + 1..w - k / 2 {
            s += row[x + k / 2] - row[x - k / 2 - 1];
            b[x] = s * inv;
        }
        for x in k / 2 + 1..w - k / 2 {
            let d_f = (row[x] - row[x - 1]).abs();
            let d_b = (b[x] - b[x - 1]).abs();
            sf_h += d_f as f64;
            sv_h += (d_f - d_b).max(0.0) as f64;
        }
    }
    // Vertical: box blur columns via running column sums.
    let (mut sf_v, mut sv_v) = (0.0f64, 0.0f64);
    let mut col = vec![0.0f32; w];
    for y in 0..k {
        for (c, v) in col.iter_mut().zip(&g.px[y * w..(y + 1) * w]) {
            *c += v;
        }
    }
    let mut prev = col.iter().map(|c| c * inv).collect::<Vec<f32>>();
    for y in k / 2 + 1..h - k / 2 {
        let add = &g.px[(y + k / 2) * w..(y + k / 2 + 1) * w];
        let sub = &g.px[(y - k / 2 - 1) * w..(y - k / 2) * w];
        let cur = &g.px[y * w..(y + 1) * w];
        let up = &g.px[(y - 1) * w..y * w];
        for x in 0..w {
            col[x] += add[x] - sub[x];
            let bx = col[x] * inv;
            let d_f = (cur[x] - up[x]).abs();
            let d_b = (bx - prev[x]).abs();
            sf_v += d_f as f64;
            sv_v += (d_f - d_b).max(0.0) as f64;
            prev[x] = bx;
        }
    }
    let n = ((w - k) * h + (h - k) * w) as f64;
    let sh = if sf_h > 0.0 { (sv_h / sf_h) as f32 } else { 0.0 };
    let sv = if sf_v > 0.0 { (sv_v / sf_v) as f32 } else { 0.0 };
    (sh, sv, ((sf_h + sf_v) / n) as f32)
}

/// Sharpness of a region in 4 directions (0°, 90°, 45°, 135°).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DirSharpness {
    /// Mean over directions, 0..=1.
    pub mean: f32,
    /// Best direction.
    pub max: f32,
    /// Worst direction.
    pub min: f32,
    /// Mean |gradient| (texture/contrast), luma levels per pixel.
    pub texture: f32,
}

impl DirSharpness {
    /// `(max - min) / max`: high when blur is directional (motion), ~0 for defocus.
    pub fn anisotropy(&self) -> f32 {
        if self.max > 0.0 {
            (self.max - self.min) / self.max
        } else {
            0.0
        }
    }
}

pub fn dir_sharpness(g: &Gray) -> DirSharpness {
    let dirs = [(1, 0), (0, 1), (1, 1), (1, -1)];
    let mut v = [0.0f32; 4];
    let mut tex = 0.0;
    for (i, &(dx, dy)) in dirs.iter().enumerate() {
        let (s, t) = directional_sharpness(g, dx, dy);
        v[i] = s;
        tex += t;
    }
    // Diagonal neighbours are sqrt(2) apart, which biases their raw value upwards;
    // rescale so a defocused (isotropic) region scores equal in all directions.
    v[2] /= DIAG_BIAS;
    v[3] /= DIAG_BIAS;
    let max = v.iter().cloned().fold(0.0, f32::max);
    let min = v.iter().cloned().fold(1.0, f32::min);
    DirSharpness { mean: v.iter().sum::<f32>() / 4.0, max, min, texture: tex / 4.0 }
}

/// Empirical diagonal/axis ratio of the metric on isotropic content.
const DIAG_BIAS: f32 = 0.93;

/// Exposure statistics on a luma plane (0..=255). Clip points: >= 250 and <= 5.
pub fn exposure(g: &Gray) -> ExposureStats {
    let n = g.px.len().max(1) as f32;
    let (mut hi, mut lo, mut sum) = (0u32, 0u32, 0.0f64);
    for &v in &g.px {
        if v >= 250.0 {
            hi += 1;
        }
        if v <= 5.0 {
            lo += 1;
        }
        sum += v as f64;
    }
    ExposureStats {
        clipped_highlights_pct: hi as f32 / n,
        clipped_shadows_pct: lo as f32 / n,
        mean_luma: (sum / n as f64 / 255.0) as f32,
    }
}

/// Area-average resize to `dw x dh` (downscale only; fine for hashing).
pub fn resize_area(g: &Gray, dw: usize, dh: usize) -> Gray {
    let mut out = Gray::new(dw, dh);
    for oy in 0..dh {
        let (y0, y1) = (oy * g.h / dh, ((oy + 1) * g.h / dh).max(oy * g.h / dh + 1).min(g.h));
        for ox in 0..dw {
            let (x0, x1) = (ox * g.w / dw, ((ox + 1) * g.w / dw).max(ox * g.w / dw + 1).min(g.w));
            let mut s = 0.0;
            for y in y0..y1 {
                s += g.px[y * g.w + x0..y * g.w + x1].iter().sum::<f32>();
            }
            out.px[oy * dw + ox] = s / ((y1 - y0) * (x1 - x0)).max(1) as f32;
        }
    }
    out
}

/// 64-bit DCT perceptual hash: 32x32 area-resized luma -> 2D DCT-II -> 8x8 lowest
/// frequencies -> bit set where the coefficient exceeds the median of the 63 AC terms.
pub fn phash(g: &Gray) -> u64 {
    const N: usize = 32;
    let small = resize_area(g, N, N);
    let mut cos = [[0.0f32; N]; 8];
    for (u, row) in cos.iter_mut().enumerate() {
        for (x, c) in row.iter_mut().enumerate() {
            *c = ((2 * x + 1) as f32 * u as f32 * std::f32::consts::PI / (2 * N) as f32).cos();
        }
    }
    // Rows: tmp[y][u] = sum_x f(x,y) cos(u,x)
    let mut tmp = [[0.0f32; 8]; N];
    for (y, row) in tmp.iter_mut().enumerate() {
        let src = &small.px[y * N..(y + 1) * N];
        for (u, t) in row.iter_mut().enumerate() {
            *t = src.iter().zip(&cos[u]).map(|(a, b)| a * b).sum();
        }
    }
    let mut coef = [0.0f32; 64];
    for v in 0..8 {
        for u in 0..8 {
            coef[v * 8 + u] = (0..N).map(|y| tmp[y][u] * cos[v][y]).sum();
        }
    }
    let mut ac: Vec<f32> = coef[1..].to_vec();
    ac.sort_by(|a, b| a.total_cmp(b));
    let median = (ac[31] + ac[32]) / 2.0;
    coef.iter().enumerate().fold(0u64, |h, (i, &c)| if c > median { h | (1 << i) } else { h })
}

pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// 2x3 affine map from *output* pixel coordinates to *source* coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine {
    pub a: f32,
    pub b: f32,
    pub tx: f32,
    pub c: f32,
    pub d: f32,
    pub ty: f32,
}

impl Affine {
    /// Similarity crop: output `size x size` centered on `(cx, cy)`, `scale` output px per
    /// source px, rotated so that source direction `angle` (radians) maps to +x.
    pub fn crop(cx: f32, cy: f32, scale: f32, angle: f32, size: f32) -> Self {
        let (s, c) = angle.sin_cos();
        let inv = 1.0 / scale;
        let half = size / 2.0;
        // src = center + R(angle) * (out - half) / scale
        Self {
            a: c * inv,
            b: -s * inv,
            tx: cx - (c * half - s * half) * inv,
            c: s * inv,
            d: c * inv,
            ty: cy - (s * half + c * half) * inv,
        }
    }

    #[inline]
    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (self.a * x + self.b * y + self.tx, self.c * x + self.d * y + self.ty)
    }
}

/// Bilinear warp of an RGB image into a planar (CHW) float buffer of `size x size`,
/// raw 0..=255 values; outside pixels are 0.
pub fn warp_rgb_chw(rgb: &[u8], w: usize, h: usize, m: &Affine, size: usize, out: &mut [f32]) {
    let plane = size * size;
    debug_assert!(out.len() >= plane * 3);
    for oy in 0..size {
        for ox in 0..size {
            let (sx, sy) = m.apply(ox as f32, oy as f32);
            let i = oy * size + ox;
            if sx < 0.0 || sy < 0.0 || sx > (w - 1) as f32 || sy > (h - 1) as f32 {
                out[i] = 0.0;
                out[plane + i] = 0.0;
                out[2 * plane + i] = 0.0;
                continue;
            }
            let (x0, y0) = (sx as usize, sy as usize);
            let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
            let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
            let p00 = (y0 * w + x0) * 3;
            let p01 = (y0 * w + x1) * 3;
            let p10 = (y1 * w + x0) * 3;
            let p11 = (y1 * w + x1) * 3;
            for ch in 0..3 {
                let top = rgb[p00 + ch] as f32 * (1.0 - fx) + rgb[p01 + ch] as f32 * fx;
                let bot = rgb[p10 + ch] as f32 * (1.0 - fx) + rgb[p11 + ch] as f32 * fx;
                out[ch * plane + i] = top * (1.0 - fy) + bot * fy;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random texture.
    fn noise(w: usize, h: usize, seed: u32) -> Gray {
        let mut s = seed;
        let mut g = Gray::new(w, h);
        for v in g.px.iter_mut() {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *v = (s >> 24) as f32;
        }
        g
    }

    /// Piecewise-constant 6 px blocks: hard edges like real detail.
    fn blocks(w: usize, h: usize, seed: u32) -> Gray {
        let n = noise(w / 6 + 1, h / 6 + 1, seed);
        let mut g = Gray::new(w, h);
        for y in 0..h {
            for x in 0..w {
                g.px[y * w + x] = n.at(x / 6, y / 6);
            }
        }
        g
    }

    fn box_blur(g: &Gray, rx: isize, ry: isize) -> Gray {
        let mut out = Gray::new(g.w, g.h);
        for y in 0..g.h as isize {
            for x in 0..g.w as isize {
                let mut s = 0.0;
                let mut n = 0.0;
                for dy in -ry..=ry {
                    for dx in -rx..=rx {
                        let (xx, yy) = ((x + dx).clamp(0, g.w as isize - 1), (y + dy).clamp(0, g.h as isize - 1));
                        s += g.px[yy as usize * g.w + xx as usize];
                        n += 1.0;
                    }
                }
                out.px[y as usize * g.w + x as usize] = s / n;
            }
        }
        out
    }

    #[test]
    fn sharpness_drops_with_blur_and_is_contrast_invariant() {
        let sharp = blocks(96, 96, 7);
        let blurred = box_blur(&sharp, 3, 3);
        let s = dir_sharpness(&sharp);
        let b = dir_sharpness(&blurred);
        assert!(s.mean > 0.6, "{s:?}");
        assert!(b.mean < s.mean - 0.3, "{b:?} vs {s:?}");
        let mut dim = sharp.clone();
        dim.px.iter_mut().for_each(|v| *v *= 0.25);
        assert!((dir_sharpness(&dim).mean - s.mean).abs() < 0.02);
        // Fast h/v path agrees with the generic one.
        let (h, v, _) = hv_sharpness(&blurred);
        assert!((h - directional_sharpness(&blurred, 1, 0).0).abs() < 0.05);
        assert!((v - directional_sharpness(&blurred, 0, 1).0).abs() < 0.05);
    }

    #[test]
    fn motion_blur_is_anisotropic_defocus_is_not() {
        let sharp = blocks(96, 96, 3);
        let motion = box_blur(&sharp, 6, 0);
        let defocus = box_blur(&sharp, 3, 3);
        let m = dir_sharpness(&motion);
        let d = dir_sharpness(&defocus);
        assert!(m.anisotropy() > 0.4, "{m:?}");
        assert!(d.anisotropy() + 0.2 < m.anisotropy(), "{d:?} vs {m:?}");
    }

    #[test]
    fn exposure_counts_clipping() {
        let mut g = Gray::new(10, 10);
        g.px[..10].iter_mut().for_each(|v| *v = 255.0);
        g.px[10..40].iter_mut().for_each(|v| *v = 128.0);
        let e = exposure(&g);
        assert!((e.clipped_highlights_pct - 0.1).abs() < 1e-6);
        assert!((e.clipped_shadows_pct - 0.6).abs() < 1e-6);
        assert!((e.mean_luma - (2550.0 + 30.0 * 128.0) / 100.0 / 255.0).abs() < 1e-4);
    }

    #[test]
    fn phash_is_stable_under_small_changes_and_differs_across_images() {
        let a = box_blur(&noise(256, 192, 1), 6, 6);
        let b = box_blur(&noise(256, 192, 2), 6, 6);
        let mut a2 = a.clone();
        a2.px.iter_mut().for_each(|v| *v = *v * 0.9 + 10.0); // exposure shift
        let (ha, hb, ha2) = (phash(&a), phash(&b), phash(&a2));
        assert!(hamming(ha, ha2) <= 2, "{}", hamming(ha, ha2));
        assert!(hamming(ha, hb) >= 16, "{}", hamming(ha, hb));
        assert_eq!(hamming(0b1011, 0b0001), 2);
    }

    #[test]
    fn luma_box_and_region() {
        let (w, h) = (4, 2);
        let rgb: Vec<u8> = (0..w * h).flat_map(|i| [i as u8 * 10; 3]).collect();
        let g = luma_box(&rgb, w, h, 2);
        assert_eq!((g.w, g.h), (2, 1));
        assert!((g.px[0] - (0.0 + 10.0 + 40.0 + 50.0) / 4.0).abs() < 1e-3);
        let r = luma_region(&rgb, w, h, 1, 0, 3, 2);
        assert_eq!((r.w, r.h), (2, 2));
        assert!((r.at(0, 1) - 50.0).abs() < 1e-3);
    }

    #[test]
    fn affine_crop_rotates_about_center() {
        // 90° crop: output +x follows source +y.
        let m = Affine::crop(50.0, 50.0, 1.0, std::f32::consts::FRAC_PI_2, 20.0);
        let (x, y) = m.apply(10.0, 10.0);
        assert!((x - 50.0).abs() < 1e-4 && (y - 50.0).abs() < 1e-4);
        let (x, y) = m.apply(15.0, 10.0);
        assert!((x - 50.0).abs() < 1e-4 && (y - 55.0).abs() < 1e-4);
        // Scale 2: 2 output px per source px.
        let m = Affine::crop(50.0, 50.0, 2.0, 0.0, 20.0);
        assert_eq!(m.apply(20.0, 10.0), (55.0, 50.0));
    }
}
