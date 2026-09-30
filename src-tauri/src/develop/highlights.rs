//! Highlight reconstruction for raw-clipped pixels (Camera Raw's behaviour, measured on
//! real frames with `tools/acr-oracle`).
//!
//! The sensor clips each raw channel at its white level. After white balance the channels
//! clip at different levels (green usually first, at 1; red / blue at their multipliers), so
//! simply clipping everything at 1 discards valid red / blue data and leaves partly clipped
//! pixels with a wrong hue (the green / magenta arcs around blown skies), and clipped areas
//! render grey at negative exposure. Instead, each clipped channel is rebuilt from the
//! pixel's unclipped channels and the local *chromaticity* of the unclipped highlights
//! around the clipped area (a coarse field filled into the clipped regions by push-pull
//! interpolation), with smooth weights over the last few percent below the clip level:
//!
//! `v_c <- v_c + w_c * (max(v_c, s * chroma_c) - v_c)`, `s` = the pixel's brightness
//! measured on its unclipped channels (at least what the clipped ones prove).
//!
//! The field depends on the image content only (not on the view), so region renders sample
//! the whole frame's field (`pipeline::ToneContext`).

use rayon::prelude::*;

/// Raw fraction of the white level where a channel starts to count as clipped / is clipped.
pub const CLIP_LO: f32 = 0.90;
pub const CLIP_HI: f32 = 0.985;
/// Long edge of the chromaticity grid (cells).
const GRID: usize = 64;
/// Raw brightness (max channel, fraction of white) from which unclipped pixels vote for
/// the chromaticity of nearby highlights.
const VOTE_LO: f32 = 0.25;
const VOTE_HI: f32 = 0.6;

/// TEMP experiment knobs (`SIEVE_K="name=v,..."`).
pub fn knob(name: &str, default: f32) -> f32 {
    static K: std::sync::OnceLock<Vec<(String, f32)>> = std::sync::OnceLock::new();
    let k = K.get_or_init(|| {
        std::env::var("SIEVE_K")
            .unwrap_or_default()
            .split(',')
            .filter_map(|kv| kv.split_once('=').map(|(a, b)| (a.to_owned(), b.parse().unwrap_or(0.0))))
            .collect()
    });
    k.iter().find(|(a, _)| a == name).map_or(default, |(_, v)| *v)
}

#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Clip weight of a raw fraction (0 = valid, 1 = clipped).
#[inline]
pub fn clip_weight(raw: f32) -> f32 {
    smoothstep(CLIP_LO, CLIP_HI, raw)
}

/// Chromaticity (white-balanced camera RGB normalized to sum 1) of the highlights around
/// clipped areas, on a coarse grid over a frame (normalized coordinates).
#[derive(Debug, Clone)]
pub struct HighlightField {
    w: usize,
    h: usize,
    data: Vec<[f32; 3]>,
}

impl HighlightField {
    /// The field of an interleaved camera RGB16 image (`pixels`, white = 65535) with
    /// white-balance multipliers `mul` (WB value = raw fraction * mul); `None` when nothing
    /// is near the clip level.
    pub fn compute(pixels: &[u16], w: usize, h: usize, mul: [f32; 3]) -> Option<HighlightField> {
        if w == 0 || h == 0 {
            return None;
        }
        let f = w.max(h).div_ceil(GRID).max(1);
        let (gw, gh) = (w.div_ceil(f), h.div_ceil(f));
        let lo = (CLIP_LO * 65535.0) as u16;
        // Per grid row: (sum of chroma * weight, weight, any clipped).
        let rows: Vec<(Vec<[f32; 4]>, bool)> = (0..gh)
            .into_par_iter()
            .map(|gy| {
                let mut acc = vec![[0.0f32; 4]; gw];
                let mut clipped = false;
                for y in gy * f..((gy + 1) * f).min(h) {
                    let row = &pixels[y * w * 3..(y + 1) * w * 3];
                    for (x, p) in row.as_chunks::<3>().0.iter().enumerate() {
                        let mx = p[0].max(p[1]).max(p[2]);
                        if mx >= lo {
                            clipped = true;
                        }
                        let raw = p.map(|c| f32::from(c) / 65535.0);
                        let rmax = raw[0].max(raw[1]).max(raw[2]);
                        let vote = smoothstep(VOTE_LO, VOTE_HI, rmax) * (1.0 - clip_weight(rmax));
                        if vote <= 0.0 {
                            continue;
                        }
                        let v = [raw[0] * mul[0], raw[1] * mul[1], raw[2] * mul[2]];
                        let s = v[0] + v[1] + v[2];
                        if s <= 1e-6 {
                            continue;
                        }
                        let a = &mut acc[x / f];
                        let k = vote / s;
                        a[0] += v[0] * k;
                        a[1] += v[1] * k;
                        a[2] += v[2] * k;
                        a[3] += vote;
                    }
                }
                (acc, clipped)
            })
            .collect();
        if !rows.iter().any(|(_, c)| *c) {
            return None;
        }
        let cells: Vec<[f32; 4]> = rows.into_iter().flat_map(|(a, _)| a).collect();
        // Confident when ~10% of a cell's pixels voted.
        let full = (f * f) as f32 * 0.1;
        let data = push_pull(cells, gw, gh, full);
        Some(HighlightField { w: gw, h: gh, data })
    }

    /// Chromaticity at normalized coordinates `u`, `v` (0..=1) of the field's frame.
    #[inline]
    pub fn sample(&self, u: f32, v: f32) -> [f32; 3] {
        let gx = (u * self.w as f32 - 0.5).clamp(0.0, (self.w - 1) as f32);
        let gy = (v * self.h as f32 - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (gx as usize, gy as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = (gx - x0 as f32, gy - y0 as f32);
        let d = |x: usize, y: usize| self.data[y * self.w + x];
        let (a, b, c, e) = (d(x0, y0), d(x1, y0), d(x0, y1), d(x1, y1));
        [0, 1, 2].map(|k| {
            let t = a[k] + (b[k] - a[k]) * fx;
            let s = c[k] + (e[k] - c[k]) * fx;
            t + (s - t) * fy
        })
    }
}

/// Fills a sparse grid of weighted sums (`[sum0, sum1, sum2, weight]`) by push-pull: a
/// pyramid of 2x2 sums, then each level blends its own mean (confidence = weight / `full`,
/// capped at 1) with the coarser level upsampled. Returns normalized means (sum 1).
fn push_pull(cells: Vec<[f32; 4]>, w: usize, h: usize, full: f32) -> Vec<[f32; 3]> {
    let mut levels = vec![(cells, w, h)];
    while {
        let (_, lw, lh) = levels.last().unwrap();
        *lw > 1 || *lh > 1
    } {
        let (src, lw, lh) = levels.last().unwrap();
        let (nw, nh) = (lw.div_ceil(2), lh.div_ceil(2));
        let mut dst = vec![[0.0f32; 4]; nw * nh];
        for y in 0..*lh {
            for x in 0..*lw {
                let s = src[y * lw + x];
                let d = &mut dst[(y / 2) * nw + x / 2];
                for k in 0..4 {
                    d[k] += s[k];
                }
            }
        }
        levels.push((dst, nw, nh));
    }
    let neutral = [1.0 / 3.0; 3];
    let mean = |c: [f32; 4]| -> Option<[f32; 3]> {
        (c[3] > 1e-6).then(|| {
            let s = (c[0] + c[1] + c[2]).max(1e-9);
            [c[0] / s, c[1] / s, c[2] / s]
        })
    };
    // Coarsest: its own mean (or neutral when nothing voted anywhere).
    let (top, _, _) = levels.last().unwrap();
    let mut up: Vec<[f32; 3]> = vec![mean(top[0]).unwrap_or(neutral)];
    let (mut uw, mut uh) = (1usize, 1usize);
    let mut level_full = full;
    // Weight per level scales with its cell area.
    let n_levels = levels.len();
    let fulls: Vec<f32> = (0..n_levels)
        .map(|i| {
            let f = level_full;
            level_full *= 4.0;
            let _ = i;
            f
        })
        .collect();
    for i in (0..n_levels - 1).rev() {
        let (cells, lw, lh) = &levels[i];
        let mut out = vec![neutral; lw * lh];
        for y in 0..*lh {
            for x in 0..*lw {
                // Bilinear sample of the coarser level at this cell's centre.
                let gx = ((x as f32 + 0.5) / 2.0 - 0.5).clamp(0.0, (uw - 1) as f32);
                let gy = ((y as f32 + 0.5) / 2.0 - 0.5).clamp(0.0, (uh - 1) as f32);
                let (x0, y0) = (gx as usize, gy as usize);
                let (x1, y1) = ((x0 + 1).min(uw - 1), (y0 + 1).min(uh - 1));
                let (fx, fy) = (gx - x0 as f32, gy - y0 as f32);
                let c = [0, 1, 2].map(|k| {
                    let a = up[y0 * uw + x0][k] + (up[y0 * uw + x1][k] - up[y0 * uw + x0][k]) * fx;
                    let b = up[y1 * uw + x0][k] + (up[y1 * uw + x1][k] - up[y1 * uw + x0][k]) * fx;
                    a + (b - a) * fy
                });
                let cell = cells[y * lw + x];
                out[y * lw + x] = match mean(cell) {
                    Some(m) => {
                        let a = (cell[3] / fulls[i]).min(1.0);
                        [0, 1, 2].map(|k| c[k] + (m[k] - c[k]) * a)
                    }
                    None => c,
                };
            }
        }
        up = out;
        (uw, uh) = (*lw, *lh);
    }
    up
}

/// Reconstructs a white-balanced camera pixel `v` (unclipped; raw fraction `v / mul`) from
/// its unclipped channels and the local highlight chromaticity `chroma` (sum 1).
#[inline]
pub fn reconstruct(v: [f32; 3], mul: [f32; 3], chroma: [f32; 3]) -> [f32; 3] {
    let w = [0, 1, 2].map(|c| clip_weight(v[c] / mul[c]));
    if w[0] == 0.0 && w[1] == 0.0 && w[2] == 0.0 {
        return v;
    }
    let ch = chroma.map(|c| c.max(1e-4));
    // Brightness from the unclipped channels, faded out as the last one clips.
    let (mut num, mut den, mut valid) = (0.0f32, 0.0f32, 0.0f32);
    let mut bound = 0.0f32;
    for c in 0..3 {
        let u = 1.0 - w[c];
        num += u * v[c];
        den += u * ch[c];
        valid += u;
        bound = bound.max(w[c] * v[c] / ch[c]);
    }
    let s_u = if den > 1e-6 { num / den * (valid / 0.05).min(1.0) } else { 0.0 };
    let s = s_u.max(bound);
    let o = [0, 1, 2].map(|c| {
        let est = (s * ch[c]).max(v[c]);
        v[c] + (est - v[c]) * w[c]
    });
    let d = knob("desat", 0.0) * w[0].max(w[1]).max(w[2]);
    let m = (o[0] + o[1] + o[2]) / 3.0;
    o.map(|c| c + (m - c) * d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unclipped_pixels_are_untouched() {
        let v = [0.5, 0.8, 1.2];
        assert_eq!(reconstruct(v, [2.0, 1.0, 1.5], [0.3, 0.3, 0.4]), v);
    }

    #[test]
    fn clipped_green_is_rebuilt_from_red_and_blue() {
        // Sky: chroma r:g:b = 0.25:0.33:0.42, true value 3.0 * chroma; green clipped at 1.
        let chroma = [0.25, 0.33, 0.42];
        let mul = [2.0, 1.0, 1.6];
        let truth = chroma.map(|c| c * 3.6);
        let v = [truth[0], 1.0, truth[2]];
        let r = reconstruct(v, mul, chroma);
        assert!((r[1] - truth[1]).abs() < 1e-3, "{r:?} vs {truth:?}");
        assert_eq!(r[0], v[0]);
        assert_eq!(r[2], v[2]);
    }

    #[test]
    fn fully_clipped_pixel_takes_the_local_chroma_without_darkening() {
        let mul = [2.0, 1.0, 1.5];
        let chroma = [0.25, 0.33, 0.42];
        let r = reconstruct(mul, mul, chroma);
        for c in 0..3 {
            assert!(r[c] >= mul[c] - 1e-6);
        }
        // Hue of the neighbourhood (blue > green > red).
        assert!(r[2] / r[1] > 1.2 && r[1] / r[0] > 1.2, "{r:?}");
    }

    #[test]
    fn reconstruction_is_continuous_across_the_clip_ramp() {
        let mul = [2.0, 1.0, 1.5];
        let chroma = [0.25, 0.33, 0.42];
        let mut last: Option<[f32; 3]> = None;
        for i in 0..=400 {
            let g = 0.8 + 0.2 * i as f32 / 400.0;
            let r = reconstruct([0.9, g, 1.3], mul, chroma);
            if let Some(l) = last {
                for c in 0..3 {
                    assert!((r[c] - l[c]).abs() < 0.01, "jump at g={g}: {l:?} -> {r:?}");
                }
            }
            last = Some(r);
        }
    }

    #[test]
    fn field_fills_clipped_area_with_border_chroma() {
        // 256x256: blue sky ramp whose centre clips in green; border chroma known.
        let (w, h) = (256usize, 256usize);
        let mul = [2.0f32, 1.0, 1.5];
        let chroma = [0.25f32, 0.33, 0.42];
        let mut px = vec![0u16; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let d = ((x as f32 - 128.0).powi(2) + (y as f32 - 128.0).powi(2)).sqrt() / 128.0;
                let s = 4.0 * (1.0 - 0.6 * d).max(0.2);
                for c in 0..3 {
                    let raw = (s * chroma[c] / mul[c]).min(1.0);
                    px[(y * w + x) * 3 + c] = (raw * 65535.0) as u16;
                }
            }
        }
        let f = HighlightField::compute(&px, w, h, mul).expect("clipped");
        let c = f.sample(0.5, 0.5);
        for k in 0..3 {
            assert!((c[k] - chroma[k]).abs() < 0.02, "{c:?}");
        }
        assert!(HighlightField::compute(&[1000u16; 48], 4, 4, mul).is_none());
    }
}
