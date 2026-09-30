//! Local adjustments inside the develop pipeline (Phase 7c): the per-pixel side of the
//! `develop::masks` seam. [`LocalOps`] is built once per render from the render's
//! [`LocalPlanes`] (mask weights x group values, UI units) and called by `pipeline::develop`
//! at the stages below; each local value is an offset added to the global slider (Lightroom's
//! additive model). Everything here runs only for masked renders.
//!
//! | Stage (pipeline)                  | Local parameters                                      |
//! |-----------------------------------|-------------------------------------------------------|
//! | A. after camera -> ProPhoto       | Temperature, Tint ([`LocalOps::apply_white_balance`])  |
//! | C. local operators                | Shadows, Highlights (at the edge-aware base), Clarity, Texture, Dehaze ([`LocalOps::tone_local`], [`LocalOps::clarity`], ...) |
//! | D. before the pointwise LUT       | Exposure, Contrast, Whites, Blacks (input gain), Hue, Saturation ([`LocalOps::pointwise`]) |
//! | D. after the LUT (point curves)   | per-group curves + colour ([`LocalOps::blend`])         |
//! | E. after capture sharpening       | Sharpness, Noise ([`LocalOps::apply_detail`])           |
//!
//! Tone sliders act in the global chain as `ev -> G(ev)` (`tone::ToneModel::global`). A local
//! offset `s` of one slider is applied *before* the chain's LUT as an input gain
//! `d(ev) = G0^-1(G_s(ev)) - ev`, so the LUT (built for the global sliders) then yields exactly
//! `G_s(ev)`: Lightroom's slider response (highlight shoulder included) for every local value,
//! tabulated over (s, ev). Several local tone sliders add their gains.
//!
//! Temperature / Tint: +100 shifts the white balance by [`LOCAL_TEMP_MIRED`] mired (warmer)
//! / [`LOCAL_TINT`] tint units (magenta); applied as the colour matrix that re-white-balances
//! the pixel (`M' diag(mul'/mul) M^-1`), interpolated in the local value (provisional scale,
//! see `docs/decisions.md`). Moire and Defringe are not rendered (no global equivalent).

use rayon::prelude::*;

use crate::ipc::types::{ParametricAdjustments, WhiteBalance};

use super::camera::{self, ColorSetup, Profile, PROPHOTO_TO_XYZ};
use super::masks::{LocalParam, LocalPlanes, PreparedBlends};
use super::parity::{self, Working, PROPHOTO_Y};
use super::source::ColorInfo;
use super::tone::{ToneModel, ToneSliders};
use super::wb;

/// Mired shift of a +100 local Temperature (negative mired = warmer).
pub const LOCAL_TEMP_MIRED: f64 = 50.0;
/// Tint units of a +100 local Tint.
pub const LOCAL_TINT: f64 = 40.0;

/// Pipeline scale of the local operators (as `pipeline::develop` scales the global sliders).
const CLARITY_SCALE: f32 = 0.6 / 100.0;
const TEXTURE_SCALE: f32 = 0.8 / 100.0;
const DEHAZE_SCALE: f32 = 1.0 / 100.0;

/// Function of (local slider value, EV) sampled on a grid, bilinear.
struct SliderTable {
    s0: f32,
    s_step: f32,
    rows: usize,
    e0: f32,
    e_step: f32,
    cols: usize,
    data: Vec<f32>,
}

impl SliderTable {
    /// `row(s, e)` over `rows` values of `s` in `s_range` x `cols` EVs in `e_range`.
    fn build<'b>(
        s_range: (f32, f32),
        rows: usize,
        e_range: (f32, f32),
        cols: usize,
        row: impl Fn(f32) -> Box<dyn Fn(f32) -> f32 + 'b> + Sync,
    ) -> SliderTable {
        let s_step = (s_range.1 - s_range.0) / (rows - 1) as f32;
        let e_step = (e_range.1 - e_range.0) / (cols - 1) as f32;
        let data: Vec<f32> = (0..rows)
            .into_par_iter()
            .flat_map_iter(|r| {
                let g = row(s_range.0 + r as f32 * s_step);
                (0..cols).map(move |c| g(e_range.0 + c as f32 * e_step))
            })
            .collect();
        SliderTable { s0: s_range.0, s_step, rows, e0: e_range.0, e_step, cols, data }
    }

    #[inline]
    fn eval(&self, s: f32, e: f32) -> f32 {
        let fs = ((s - self.s0) / self.s_step).clamp(0.0, (self.rows - 1) as f32);
        let fe = ((e - self.e0) / self.e_step).clamp(0.0, (self.cols - 1) as f32);
        let (r0, c0) = ((fs as usize).min(self.rows - 2), (fe as usize).min(self.cols - 2));
        let (ts, te) = (fs - r0 as f32, fe - c0 as f32);
        let at = |r: usize, c: usize| self.data[r * self.cols + c];
        let a = at(r0, c0) + (at(r0, c0 + 1) - at(r0, c0)) * te;
        let b = at(r0 + 1, c0) + (at(r0 + 1, c0 + 1) - at(r0 + 1, c0)) * te;
        a + (b - a) * ts
    }
}

/// Monotone inverse of `g` over `[lo, hi]` (sampled; flat parts resolve to their first EV).
struct Inverse {
    xs: Vec<f32>,
    ys: Vec<f32>,
}

impl Inverse {
    fn new(g: impl Fn(f32) -> f32, lo: f32, hi: f32, n: usize) -> Inverse {
        let xs: Vec<f32> = (0..n).map(|i| lo + (hi - lo) * i as f32 / (n - 1) as f32).collect();
        let mut ys: Vec<f32> = xs.iter().map(|&x| g(x)).collect();
        for i in 1..n {
            if ys[i] < ys[i - 1] {
                ys[i] = ys[i - 1];
            }
        }
        Inverse { xs, ys }
    }

    fn eval(&self, y: f32) -> f32 {
        let n = self.ys.len();
        if y <= self.ys[0] {
            return self.xs[0] + (y - self.ys[0]);
        }
        if y >= self.ys[n - 1] {
            return self.xs[n - 1] + (y - self.ys[n - 1]);
        }
        let k = self.ys.partition_point(|&v| v <= y).clamp(1, n - 1);
        let (y0, y1) = (self.ys[k - 1], self.ys[k]);
        let t = if y1 > y0 { (y - y0) / (y1 - y0) } else { 0.0 };
        self.xs[k - 1] + (self.xs[k] - self.xs[k - 1]) * t
    }
}

/// EV domain of the tone tables (pre-exposure scene luminance; raw clip = 0).
const EV_RANGE: (f32, f32) = (-18.0, 3.0);
const EV_COLS: usize = 168;

/// `ToneSliders` with `param` offset by `s` (UI units).
fn with_offset(base: ToneSliders, param: LocalParam, s: f32) -> ToneSliders {
    let mut t = base;
    match param {
        LocalParam::Exposure => t.exposure += s,
        LocalParam::Contrast => t.contrast = (t.contrast + s).clamp(-100.0, 100.0),
        LocalParam::Whites => t.whites = (t.whites + s).clamp(-100.0, 100.0),
        LocalParam::Blacks => t.blacks = (t.blacks + s).clamp(-100.0, 100.0),
        LocalParam::Shadows => t.shadows = (t.shadows + s).clamp(-100.0, 100.0),
        LocalParam::Highlights => t.highlights = (t.highlights + s).clamp(-100.0, 100.0),
        _ => {}
    }
    t
}

/// Slider-value grid of a local parameter: (range, rows).
fn s_grid(planes: &LocalPlanes, param: LocalParam) -> ((f32, f32), usize) {
    let b = planes.bound(param).max(1e-3);
    if param == LocalParam::Exposure {
        let b = b.min(10.0);
        ((-b, b), ((b * 8.0).ceil() as usize * 2 + 1).clamp(3, 161))
    } else {
        let b = b.min(400.0);
        ((-b, b), ((b / 6.25).ceil() as usize * 2 + 1).clamp(3, 129))
    }
}

/// Per-render local adjustment state (see the module docs).
pub struct LocalOps<'a> {
    planes: &'a LocalPlanes,
    /// (param, input-EV gain table) for exposure / contrast / whites / blacks.
    tone_in: Vec<(LocalParam, SliderTable)>,
    /// Global tone sliders of the render (exposure includes the baseline).
    tone: ToneSliders,
    /// (param, local-delta table over the base luminance) for shadows / highlights; built by
    /// [`Self::set_key`].
    tone_local: Vec<(LocalParam, SliderTable)>,
    /// White balance deltas: (temperature +, temperature -, tint +, tint -) as `C - I` per
    /// unit (100) of local value, in linear ProPhoto.
    wb: Option<[[[f32; 3]; 3]; 4]>,
    color: bool,
    blends: PreparedBlends,
    lab: &'static parity::Oklab,
}

impl<'a> LocalOps<'a> {
    /// `adj` = the effective adjustments of the render (look parameters merged), `setup` =
    /// its colour setup.
    pub fn new(
        planes: &'a LocalPlanes,
        adj: &ParametricAdjustments,
        color: &ColorInfo,
        profile: &Profile,
        setup: &ColorSetup,
    ) -> LocalOps<'a> {
        let tone = ToneSliders {
            exposure: adj.exposure + profile.baseline_ev,
            contrast: adj.contrast,
            highlights: adj.highlights,
            shadows: adj.shadows,
            whites: adj.whites,
            blacks: adj.blacks,
        };
        let global = ToneSliders { shadows: 0.0, highlights: 0.0, ..tone };
        let tone_in = [LocalParam::Exposure, LocalParam::Contrast, LocalParam::Whites, LocalParam::Blacks]
            .into_iter()
            .filter(|&p| planes.has(p))
            .map(|p| {
                let g0 = ToneModel::new(global);
                let inv = Inverse::new(|e| g0.global(e), EV_RANGE.0 - 12.0, EV_RANGE.1 + 12.0, 2048);
                let (range, rows) = s_grid(planes, p);
                let inv = &inv;
                let table = SliderTable::build(range, rows, EV_RANGE, EV_COLS, |s| {
                    let gs = ToneModel::new(with_offset(global, p, s));
                    Box::new(move |e: f32| (inv.eval(gs.global(e)) - e).clamp(-12.0, 12.0))
                });
                (p, table)
            })
            .collect();
        let wb = (planes.has(LocalParam::Temperature) || planes.has(LocalParam::Tint))
            .then(|| wb_deltas(adj, color, profile, setup))
            .flatten();
        LocalOps {
            planes,
            tone_in,
            tone,
            tone_local: Vec::new(),
            wb,
            color: planes.has(LocalParam::Hue) || planes.has(LocalParam::Saturation),
            blends: planes.prepare_blends(),
            lab: parity::Oklab::get(),
        }
    }

    /// Shadows/Highlights (or Clarity, Texture, Dehaze) need the pipeline's local fields.
    pub fn needs(&self, param: LocalParam) -> bool {
        self.planes.has(param)
    }

    /// Positive local dehaze somewhere (needs the haze field).
    pub fn needs_haze(&self) -> bool {
        self.planes.has(LocalParam::Dehaze)
    }

    /// Local Shadows/Highlights need the edge-aware base luminance.
    pub fn needs_base(&self) -> bool {
        self.planes.has(LocalParam::Shadows) || self.planes.has(LocalParam::Highlights)
    }

    /// Image key / white (as `ToneModel::with_key`): builds the Shadows/Highlights tables.
    pub fn set_key(&mut self, key: f32, white: f32) {
        let base = ToneSliders { exposure: 0.0, ..self.tone };
        let g0 = ToneModel::new(base).with_key(key, white);
        let g0 = &g0;
        self.tone_local = [LocalParam::Shadows, LocalParam::Highlights]
            .into_iter()
            .filter(|&p| self.planes.has(p))
            .map(|p| {
                let (range, rows) = s_grid(self.planes, p);
                let table = SliderTable::build(range, rows, (-16.0, 4.0), 160, |s| {
                    let m = ToneModel::new(with_offset(base, p, s)).with_key(key, white);
                    Box::new(move |e: f32| m.local_delta(e) - g0.local_delta(e))
                });
                (p, table)
            })
            .collect();
    }

    /// Extra Shadows/Highlights EV delta at pixel `i` for base luminance `m` (EV).
    #[inline]
    pub fn tone_local(&self, i: usize, m: f32) -> f32 {
        let mut d = 0.0;
        for (p, t) in &self.tone_local {
            let s = self.planes.value(*p, i);
            if s != 0.0 {
                d += t.eval(s, m);
            }
        }
        d
    }

    /// Clarity amount offset (pipeline scale) at pixel `i`.
    #[inline]
    pub fn clarity(&self, i: usize) -> f32 {
        self.planes.value(LocalParam::Clarity, i) * CLARITY_SCALE
    }

    /// Texture amount offset (pipeline scale) at pixel `i`.
    #[inline]
    pub fn texture(&self, i: usize) -> f32 {
        self.planes.value(LocalParam::Texture, i) * TEXTURE_SCALE
    }

    /// Dehaze amount offset (pipeline scale) at pixel `i`.
    #[inline]
    pub fn dehaze(&self, i: usize) -> f32 {
        self.planes.value(LocalParam::Dehaze, i) * DEHAZE_SCALE
    }

    /// Anything to do in stage C.
    pub fn any_local_operator(&self) -> bool {
        self.needs_base()
            || self.planes.has(LocalParam::Clarity)
            || self.planes.has(LocalParam::Texture)
            || self.planes.has(LocalParam::Dehaze)
    }

    /// Stage A: local white balance on linear ProPhoto (interleaved, `w` px per row).
    pub fn apply_white_balance(&self, rgb: &mut [f32], w: usize) {
        let Some(d) = &self.wb else { return };
        let planes = self.planes;
        rgb.par_chunks_mut(w * 3).enumerate().for_each(|(y, row)| {
            for (x, p) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let i = y * w + x;
                let t = planes.value(LocalParam::Temperature, i) / 100.0;
                let n = planes.value(LocalParam::Tint, i) / 100.0;
                if t == 0.0 && n == 0.0 {
                    continue;
                }
                let v = *p;
                let mut out = v;
                for (amount, m) in
                    [(t, if t >= 0.0 { &d[0] } else { &d[1] }), (n, if n >= 0.0 { &d[2] } else { &d[3] })]
                {
                    if amount != 0.0 {
                        let a = amount.abs();
                        for k in 0..3 {
                            out[k] += a * (m[k][0] * v[0] + m[k][1] * v[1] + m[k][2] * v[2]);
                        }
                    }
                }
                *p = out.map(|c| c.max(0.0));
            }
        });
    }

    /// Stage D, before the LUT: local exposure/contrast/whites/blacks as an input gain, then
    /// hue/saturation (Oklab), on linear ProPhoto (pre-exposure).
    #[inline]
    pub fn pointwise(&self, v: [f32; 3], i: usize) -> [f32; 3] {
        let mut v = v;
        if !self.tone_in.is_empty() {
            let y = parity_dot(PROPHOTO_Y, v);
            if y > 1e-12 {
                let ev = y.log2();
                let mut d = 0.0;
                for (p, t) in &self.tone_in {
                    let s = self.planes.value(*p, i);
                    if s != 0.0 {
                        d += t.eval(s, ev);
                    }
                }
                if d != 0.0 {
                    let g = d.clamp(-12.0, 12.0).exp2();
                    v = v.map(|c| c * g);
                }
            }
        }
        if self.color {
            let hue = self.planes.value(LocalParam::Hue, i);
            let sat = self.planes.value(LocalParam::Saturation, i);
            if hue != 0.0 || sat != 0.0 {
                v = self.hue_sat(v, hue, sat);
            }
        }
        v
    }

    fn hue_sat(&self, v: [f32; 3], hue_deg: f32, sat: f32) -> [f32; 3] {
        let [l, a, b] = self.lab.from_prophoto(v.map(|c| c.max(0.0)));
        let k = (1.0 + sat / 100.0).max(0.0);
        let (sn, cs) = hue_deg.to_radians().sin_cos();
        let (a2, b2) = ((a * cs - b * sn) * k, (a * sn + b * cs) * k);
        let mut out = self.lab.to_prophoto([l, a2, b2]);
        let mn = out[0].min(out[1]).min(out[2]);
        if mn < 0.0 {
            let yo = parity_dot(PROPHOTO_Y, out).max(0.0);
            let t = yo / (yo - mn);
            out = out.map(|c| yo + (c - yo) * t);
        }
        out
    }

    /// Stage D, after the LUT (global point curves): per-group curves and colour, on
    /// display-linear ProPhoto.
    #[inline]
    pub fn blend(&self, v: [f32; 3], i: usize) -> [f32; 3] {
        if self.blends.is_empty() {
            v
        } else {
            self.blends.apply(i, v)
        }
    }

    /// Stage E: local Sharpness (+ sharpen, - soften) and Noise (+ smooth luminance) on the
    /// encoded output. `scale` = input px per full-resolution frame px.
    pub fn apply_detail(&self, img: &mut Working, scale: f32) {
        let sharp = self.planes.has(LocalParam::Sharpness);
        let noise = self.planes.has(LocalParam::Noise);
        if !sharp && !noise {
            return;
        }
        let (w, h) = (img.width, img.height);
        const LW: [f32; 3] = [0.2126, 0.7152, 0.0722];
        let lum: Vec<f32> = img.rgb.par_chunks(3).map(|p| LW[0] * p[0] + LW[1] * p[1] + LW[2] * p[2]).collect();
        let s = scale.clamp(0.05, 1.0);
        let fine = sharp.then(|| parity::blur(&lum, w, h, (1.0 * s).max(0.35)));
        let soft = (sharp || noise).then(|| parity::blur(&lum, w, h, (2.5 * s).max(0.5)));
        let planes = self.planes;
        img.rgb.par_chunks_mut(w * 3).enumerate().for_each(|(y, row)| {
            for (x, p) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let i = y * w + x;
                let l = lum[i];
                let mut d = 0.0f32;
                if let Some(f) = &fine {
                    let a = planes.value(LocalParam::Sharpness, i) / 100.0;
                    if a > 0.0 {
                        d += (a * 1.2 * (l - f[i])).clamp(-0.1, 0.1);
                    } else if a < 0.0 {
                        if let Some(sf) = &soft {
                            d += -a * (sf[i] - l);
                        }
                    }
                }
                if let (true, Some(sf)) = (noise, &soft) {
                    let a = (planes.value(LocalParam::Noise, i) / 100.0).clamp(0.0, 2.0);
                    if a > 0.0 {
                        d += a * 0.6 * (sf[i] - l);
                    }
                }
                if d != 0.0 {
                    *p = p.map(|c| (c + d).clamp(0.0, 1.0));
                }
            }
        });
    }
}

#[inline]
fn parity_dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The render's white balance as temperature/tint.
fn base_wb(adj: &ParametricAdjustments, color: &ColorInfo, profile: &Profile, setup: &ColorSetup) -> (f64, f64) {
    match adj.white_balance {
        WhiteBalance::Custom { temperature_k, tint } => (f64::from(temperature_k), f64::from(tint)),
        WhiteBalance::AsShot => match camera::as_shot_values(color, profile) {
            Some(v) => (f64::from(v.temperature_k), f64::from(v.tint)),
            None => (f64::from(setup.temperature), 0.0),
        },
    }
}

fn mat3(m: &[[f32; 3]; 3]) -> [[f64; 3]; 3] {
    m.map(|r| r.map(f64::from))
}

/// `C - I` for local Temperature +-100 and Tint +-100 (module docs).
fn wb_deltas(
    adj: &ParametricAdjustments,
    color: &ColorInfo,
    profile: &Profile,
    setup: &ColorSetup,
) -> Option<[[[f32; 3]; 3]; 4]> {
    let (t0, n0) = base_wb(adj, color, profile, setup);
    let inv = wb::invert3(&mat3(&setup.m))?;
    let delta = |t: f64, n: f64| -> Option<[[f32; 3]; 3]> {
        let t = t.clamp(f64::from(wb::MIN_TEMP), f64::from(wb::MAX_TEMP)) as f32;
        let n = n.clamp(f64::from(wb::MIN_TINT), f64::from(wb::MAX_TINT)) as f32;
        let s2 =
            camera::color_setup(color, profile, &WhiteBalance::Custom { temperature_k: t, tint: n }, &adj.calibration);
        let mut m2 = mat3(&s2.m);
        for row in m2.iter_mut() {
            for (j, v) in row.iter_mut().enumerate() {
                *v *= f64::from(s2.mul[j] / setup.mul[j]);
            }
        }
        let c = crate::profiles::dcp::mul_mm(&m2, &inv);
        let mut d = [[0.0f32; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                d[i][j] = (c[i][j] - if i == j { 1.0 } else { 0.0 }) as f32;
            }
        }
        d.iter().flatten().all(|v| v.is_finite()).then_some(d)
    };
    let mired = 1e6 / t0.max(1000.0);
    let warm = 1e6 / (mired - LOCAL_TEMP_MIRED).max(10.0);
    let cool = 1e6 / (mired + LOCAL_TEMP_MIRED);
    Some([delta(warm, n0)?, delta(cool, n0)?, delta(t0, n0 + LOCAL_TINT)?, delta(t0, n0 - LOCAL_TINT)?])
}

/// CIE Lab (D50) guide for range masks / brush auto-mask on the render grid: the input after
/// global white balance and exposure (+ baseline), through the base tone curve (so L* is
/// close to what the user sees), before any local adjustment. `pixels` = interleaved camera
/// RGB16 of the render input.
pub fn range_guide(pixels: &[u16], color: &ColorInfo, profile: &Profile, adj: &ParametricAdjustments) -> Vec<[f32; 3]> {
    let setup = camera::color_setup(color, profile, &adj.white_balance, &adj.calibration);
    let mul = setup.mul.map(|m| m / 65535.0);
    let m = setup.m;
    let gain = (adj.exposure + profile.baseline_ev).exp2();
    let display = profile.display_referred;
    let to_xyz = PROPHOTO_TO_XYZ.map(|r| r.map(|v| v as f32));
    const WHITE: [f32; 3] = [0.9642, 1.0, 0.8249];
    let f = |t: f32| if t > 0.008_856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
    pixels
        .par_chunks(3)
        .map(|p| {
            let c = [
                (f32::from(p[0]) * mul[0]).min(1.0),
                (f32::from(p[1]) * mul[1]).min(1.0),
                (f32::from(p[2]) * mul[2]).min(1.0),
            ];
            let mut v = [0, 1, 2].map(|k| (m[k][0] * c[0] + m[k][1] * c[1] + m[k][2] * c[2]).max(0.0) * gain);
            let y = parity_dot(PROPHOTO_Y, v);
            if !display && y > 1e-9 {
                let k = super::tone::base_curve(y) / y;
                v = v.map(|c| c * k);
            }
            let xyz = [0, 1, 2].map(|k| (to_xyz[k][0] * v[0] + to_xyz[k][1] * v[1] + to_xyz[k][2] * v[2]) / WHITE[k]);
            let (fx, fy, fz) = (f(xyz[0]), f(xyz[1]), f(xyz[2]));
            [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::masks::{evaluate, MaskGeometry, NoMattes};
    use super::super::pipeline::{self, RenderInput, RenderedImage, BASELINE_EV};
    use super::*;
    use crate::ipc::types::{
        CropSettings, LinearMask, LocalAdjustments, MaskBlendMode, MaskComponent, MaskGroup, MaskShape, NormPoint,
    };

    fn color() -> ColorInfo {
        ColorInfo {
            as_shot_mul: Some([2.0, 1.0, 1.5]),
            daylight_mul: [2.0, 1.0, 1.5],
            rgb_cam: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            xyz_to_cam: [
                [3.2404542, -1.5371385, -0.4985314],
                [-0.969266, 1.8760108, 0.041556],
                [0.0556434, -0.2040259, 1.0572252],
            ],
        }
    }

    fn plain() -> ParametricAdjustments {
        let mut a = ParametricAdjustments::default();
        a.detail.sharpening.amount = 0.0;
        a.detail.noise_reduction.color = 0.0;
        a.profile = crate::ipc::types::ProfileSettings::none();
        a
    }

    const W: u32 = 160;
    const H: u32 = 100;

    /// Grey ramps (top) and skin-like colour ramps (bottom), camera RGB.
    fn pixels() -> Vec<u16> {
        let mut px = Vec::new();
        for y in 0..H {
            for x in 0..W {
                let v = 0.02 + (x % 40) as f32 / 39.0 * 0.5;
                let rgb = if y < H / 2 { [v, v, v] } else { [v, v * 0.7, v * 0.5] };
                px.extend([rgb[0] / 2.0, rgb[1], rgb[2] / 1.5].map(|c| (c * 65535.0) as u16));
            }
        }
        px
    }

    fn group(zero_x: f32, full_x: f32, adjust: impl Fn(&mut LocalAdjustments)) -> MaskGroup {
        let mut adjustments = LocalAdjustments::default();
        adjust(&mut adjustments);
        MaskGroup {
            id: format!("{:032X}", 1),
            name: "Mask 1".into(),
            active: true,
            amount: 1.0,
            adjustments,
            components: vec![MaskComponent {
                id: format!("{:032X}", 2),
                name: String::new(),
                active: true,
                mode: MaskBlendMode::Add,
                inverted: false,
                opacity: 1.0,
                shape: MaskShape::Linear(LinearMask {
                    zero: NormPoint { x: zero_x, y: 0.5 },
                    full: NormPoint { x: full_x, y: 0.5 },
                }),
            }],
        }
    }

    fn render(adj: &ParametricAdjustments) -> RenderedImage {
        let px = pixels();
        let c = color();
        let p = Profile::matrix(BASELINE_EV);
        let input = RenderInput::simple(W, H, &px, &c, &p);
        let geom = MaskGeometry {
            sensor_width: W,
            sensor_height: H,
            orientation: 1,
            crop: CropSettings::default(),
            region: None,
            width: W,
            height: H,
        };
        let weights = evaluate(&adj.masks, &geom, 1, &NoMattes, None);
        let planes = LocalPlanes::build(&adj.masks, &weights);
        pipeline::render_masked(&input, adj, None, planes.as_ref())
    }

    fn px(img: &RenderedImage, x: u32, y: u32) -> [i32; 3] {
        let i = ((y * img.width + x) * 3) as usize;
        [0, 1, 2].map(|k| i32::from(img.rgb[i + k]))
    }

    fn max_diff(a: &RenderedImage, b: &RenderedImage, x0: u32, x1: u32) -> i32 {
        let mut d = 0;
        for y in 0..H {
            for x in x0..x1 {
                let (p, q) = (px(a, x, y), px(b, x, y));
                d = d.max((0..3).map(|k| (p[k] - q[k]).abs()).max().unwrap_or(0));
            }
        }
        d
    }

    fn mean_green(img: &RenderedImage, x0: u32, x1: u32) -> f32 {
        let mut s = 0.0;
        for y in 0..H {
            for x in x0..x1 {
                s += px(img, x, y)[1] as f32;
            }
        }
        s / ((x1 - x0) * H) as f32
    }

    #[test]
    fn local_exposure_applies_only_inside_the_mask() {
        let base = render(&plain());
        // Weight 0 left of x = 0.49, 1 right of 0.51.
        let masked = render(&ParametricAdjustments { masks: vec![group(0.49, 0.51, |a| a.exposure = 1.0)], ..plain() });
        assert_eq!(max_diff(&base, &masked, 0, W * 45 / 100), 0, "outside the mask: unchanged");
        let (b, m) = (mean_green(&base, W * 55 / 100, W), mean_green(&masked, W * 55 / 100, W));
        assert!(m > b + 15.0, "inside the mask: brighter ({b} -> {m})");
        // Inactive group / neutral group / zero amount: identical render.
        for g in [
            MaskGroup { active: false, ..group(0.49, 0.51, |a| a.exposure = 1.0) },
            group(0.49, 0.51, |_| {}),
            MaskGroup { amount: 0.0, ..group(0.49, 0.51, |a| a.exposure = 1.0) },
        ] {
            let r = render(&ParametricAdjustments { masks: vec![g], ..plain() });
            assert_eq!(max_diff(&base, &r, 0, W), 0);
        }
    }

    /// A mask covering the whole frame renders like the same global slider (Lightroom's
    /// additive model), within rounding.
    #[test]
    fn full_mask_matches_the_global_slider() {
        type Set = fn(&mut ParametricAdjustments, f32);
        type SetLocal = fn(&mut LocalAdjustments, f32);
        let cases: [(&str, Set, SetLocal, f32, i32); 9] = [
            ("exposure", |a, v| a.exposure = v, |a, v| a.exposure = v, 1.0, 1),
            ("exposure-", |a, v| a.exposure = v, |a, v| a.exposure = v, -1.5, 1),
            ("contrast", |a, v| a.contrast = v, |a, v| a.contrast = v, 60.0, 2),
            ("whites", |a, v| a.whites = v, |a, v| a.whites = v, -50.0, 2),
            ("blacks", |a, v| a.blacks = v, |a, v| a.blacks = v, 50.0, 2),
            ("shadows", |a, v| a.shadows = v, |a, v| a.shadows = v, 70.0, 2),
            ("highlights", |a, v| a.highlights = v, |a, v| a.highlights = v, -70.0, 2),
            ("clarity", |a, v| a.clarity = v, |a, v| a.clarity = v, 50.0, 1),
            ("texture", |a, v| a.texture = v, |a, v| a.texture = v, 100.0, 1),
        ];
        for (name, set, set_local, v, tol) in cases {
            let mut global = plain();
            set(&mut global, v);
            let g = render(&global);
            let local = ParametricAdjustments { masks: vec![group(-2.0, -1.0, |a| set_local(a, v))], ..plain() };
            let l = render(&local);
            let d = max_diff(&g, &l, 0, W);
            assert!(d <= tol, "{name}: max diff {d}");
            assert!(max_diff(&g, &render(&plain()), 0, W) > 0, "{name}: the slider changes the render");
        }
    }

    #[test]
    fn local_colour_temperature_tint_hue_and_saturation() {
        let base = render(&plain());
        let full = |f: fn(&mut LocalAdjustments)| {
            render(&ParametricAdjustments { masks: vec![group(-2.0, -1.0, f)], ..plain() })
        };
        let rb = |img: &RenderedImage| {
            let p = px(img, 30, 10);
            p[0] - p[2]
        };
        assert!(rb(&full(|a| a.temperature = 60.0)) > rb(&base) + 8, "warmer");
        assert!(rb(&full(|a| a.temperature = -60.0)) < rb(&base) - 8, "cooler");
        let g = |img: &RenderedImage| {
            let p = px(img, 30, 10);
            p[1] - (p[0] + p[2]) / 2
        };
        assert!(g(&full(|a| a.tint = 60.0)) < g(&base) - 4, "magenta");
        // Saturation: the colour row gets closer to grey.
        let sat = |img: &RenderedImage| {
            let p = px(img, 30, 80);
            p[0] - p[2]
        };
        assert!(sat(&full(|a| a.saturation = -100.0)) < sat(&base) / 3);
        assert!(sat(&full(|a| a.saturation = 60.0)) > sat(&base));
        assert_ne!(px(&full(|a| a.hue = 90.0), 30, 80), px(&base, 30, 80));
        // Greys stay grey under saturation.
        let p = px(&full(|a| a.saturation = 80.0), 30, 10);
        assert!((p[0] - p[2]).abs() <= 2, "{p:?}");
    }

    #[test]
    fn group_curve_and_detail_apply_by_weight() {
        let base = render(&plain());
        let lifted = render(&ParametricAdjustments {
            masks: vec![group(0.49, 0.51, |a| a.tone_curve.master = vec![[0.0, 60.0], [255.0, 255.0]])],
            ..plain()
        });
        assert_eq!(max_diff(&base, &lifted, 0, W * 45 / 100), 0);
        assert!(px(&lifted, W - 40, 10)[1] > px(&base, W - 40, 10)[1] + 20, "curve lifts the masked blacks");
        let soft =
            render(&ParametricAdjustments { masks: vec![group(0.49, 0.51, |a| a.sharpness = -100.0)], ..plain() });
        assert_eq!(max_diff(&base, &soft, 0, W * 40 / 100), 0);
        assert!(max_diff(&base, &soft, W * 60 / 100, W) > 0, "softened inside");
    }
}
