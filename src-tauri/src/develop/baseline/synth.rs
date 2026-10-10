//! Synthetic scenes with known exposure / white-balance shifts for the baseline edit's tests
//! and `examples/baseline_eval.rs` (roadmap Phase 10 acceptance). Deterministic (seeded LCG).
//!
//! A frame is linear scene radiance = reflectance x illuminant (temperature / tint through
//! `wb::xy_for`, relative to D65 = the raster source's as-shot white) x the scene's light level
//! x 2^shift (the camera's exposure difference, also reported as EXIF-style `exposure_ev`),
//! plus emitters (windows, sky, candles), clipped and sRGB-encoded into a 16-bit PNG. Every
//! scene except the silhouette holds an 18 % grey card ([`SynthFrame::card`]), so a rendered
//! frame's exposure and white balance can be read off the card.

use std::path::Path;

use crate::ipc::types::NormRect;

/// The scene sets of the acceptance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SceneKind {
    /// Dim church interior (~3800 K), bright windows, candles, two faces.
    DarkChurch,
    /// Sunny park (~5600 K): sky, grass, white dress, dark suit.
    BrightOutdoor,
    /// Tungsten reception (~2900 K); every sixth frame lit by window daylight (mixed light).
    TungstenReception,
    /// Couple against a bright sky, exposed for the sky.
    BacklitSilhouette,
    /// Spot-lit portrait on black.
    LowKeyPortrait,
}

impl SceneKind {
    pub const ALL: [SceneKind; 5] = [
        SceneKind::DarkChurch,
        SceneKind::BrightOutdoor,
        SceneKind::TungstenReception,
        SceneKind::BacklitSilhouette,
        SceneKind::LowKeyPortrait,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SceneKind::DarkChurch => "dark church",
            SceneKind::BrightOutdoor => "bright outdoor",
            SceneKind::TungstenReception => "tungsten reception",
            SceneKind::BacklitSilhouette => "backlit silhouette",
            SceneKind::LowKeyPortrait => "low-key portrait",
        }
    }

    /// Nominal light level (linear multiplier) and camera EV of the scene.
    fn level(self) -> (f32, f32) {
        match self {
            SceneKind::DarkChurch => (0.22, -3.0),
            SceneKind::BrightOutdoor => (0.95, 7.0),
            SceneKind::TungstenReception => (0.45, -1.0),
            SceneKind::BacklitSilhouette => (0.55, 8.0),
            SceneKind::LowKeyPortrait => (0.9, 0.0),
        }
    }

    /// Illuminant (K, tint) and its frame-to-frame jitter.
    fn light(self) -> (f32, f32, f32, f32) {
        match self {
            SceneKind::DarkChurch => (3800.0, 2.0, 300.0, 4.0),
            SceneKind::BrightOutdoor => (5600.0, 4.0, 400.0, 4.0),
            SceneKind::TungstenReception => (2900.0, 0.0, 150.0, 3.0),
            SceneKind::BacklitSilhouette => (5800.0, 2.0, 200.0, 2.0),
            SceneKind::LowKeyPortrait => (4500.0, 0.0, 150.0, 2.0),
        }
    }

    /// Per-burst exposure shift range (EV) and within-burst jitter.
    fn shifts(self) -> (f32, f32) {
        match self {
            SceneKind::DarkChurch => (0.8, 0.15),
            SceneKind::BrightOutdoor => (0.7, 0.15),
            SceneKind::TungstenReception => (0.6, 0.15),
            SceneKind::BacklitSilhouette => (0.3, 0.1),
            SceneKind::LowKeyPortrait => (0.3, 0.1),
        }
    }
}

/// One planned frame.
#[derive(Debug, Clone, PartialEq)]
pub struct SynthFrame {
    pub kind: SceneKind,
    /// Index within the scene.
    pub index: usize,
    pub scene_id: i64,
    pub burst_id: i64,
    pub captured_at_ms: i64,
    /// Exposure difference from the scene's nominal (EV).
    pub shift: f32,
    /// Camera exposure (EXIF-style, `baseline::camera_ev` units).
    pub exposure_ev: f32,
    /// The cast the frame carries: the scene light after the camera's own auto white balance
    /// ([`camera_residual_k`]), K / tint relative to D65.
    pub illuminant_k: f32,
    pub illuminant_tint: f32,
    /// Horizontal composition offset (share of the width).
    pub dx: f32,
    /// Lit by window daylight instead of the scene's light (tungsten reception only).
    pub daylight: bool,
    /// Grey card (normalized); `None` for the silhouette.
    pub card: Option<NormRect>,
    /// Face boxes (normalized), as the analysis would report them.
    pub faces: Vec<NormRect>,
}

/// Deterministic LCG in 0..1.
pub struct Lcg(u64);

impl Lcg {
    pub fn new(seed: u64) -> Self {
        Lcg(seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407))
    }
    pub fn unit(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / ((1u64 << 24) as f32)
    }
    /// Uniform in -1..1.
    pub fn sym(&mut self) -> f32 {
        2.0 * self.unit() - 1.0
    }
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> NormRect {
    NormRect { x, y, width: w, height: h }
}

/// `n` frames of `kind` as scene `scene_id`, bursts of `burst_len` (ids `burst_base + k`),
/// starting at `t0_ms`.
pub fn plan(
    kind: SceneKind,
    scene_id: i64,
    burst_base: i64,
    n: usize,
    burst_len: usize,
    t0_ms: i64,
    seed: u64,
) -> Vec<SynthFrame> {
    let mut rng = Lcg::new(seed ^ (scene_id as u64).wrapping_mul(0x9E37_79B9));
    let (_, ev0) = kind.level();
    let (k0, tint0, kj, tj) = kind.light();
    let (range, jitter) = kind.shifts();
    let mut out = Vec::with_capacity(n);
    let mut burst_shift = 0.0;
    let mut burst_dx = 0.0;
    for index in 0..n {
        let burst = index / burst_len.max(1);
        if index % burst_len.max(1) == 0 {
            burst_shift = range * rng.sym();
            burst_dx = 0.08 * rng.sym();
        }
        let shift = burst_shift + jitter * rng.sym();
        let daylight = kind == SceneKind::TungstenReception && index % 6 == 5;
        let (k, tint) = if daylight {
            (5500.0 + 200.0 * rng.sym(), 3.0 * rng.sym())
        } else {
            (k0 + kj * rng.sym(), tint0 + tj * rng.sym())
        };
        let dx = burst_dx + 0.02 * rng.sym();
        let (card, faces) = layout(kind, dx);
        out.push(SynthFrame {
            kind,
            index,
            scene_id,
            burst_id: burst_base + burst as i64,
            captured_at_ms: t0_ms + burst as i64 * 20_000 + (index % burst_len.max(1)) as i64 * 300,
            shift,
            exposure_ev: ev0 + shift,
            illuminant_k: camera_residual_k(k),
            illuminant_tint: CAMERA_AWB_KEEP * tint,
            dx,
            daylight,
            card,
            faces,
        });
    }
    out
}

/// Grey card and faces of a layout shifted by `dx`.
fn layout(kind: SceneKind, dx: f32) -> (Option<NormRect>, Vec<NormRect>) {
    match kind {
        SceneKind::DarkChurch => (
            Some(rect(0.08 + dx, 0.62, 0.1, 0.14)),
            vec![rect(0.40 + dx, 0.30, 0.08, 0.14), rect(0.55 + dx, 0.32, 0.08, 0.14)],
        ),
        SceneKind::BrightOutdoor => (
            Some(rect(0.78 + dx, 0.6, 0.1, 0.14)),
            vec![rect(0.36 + dx, 0.28, 0.08, 0.14), rect(0.52 + dx, 0.26, 0.08, 0.14)],
        ),
        SceneKind::TungstenReception => (
            Some(rect(0.10 + dx, 0.55, 0.1, 0.14)),
            vec![rect(0.42 + dx, 0.30, 0.09, 0.15), rect(0.60 + dx, 0.33, 0.08, 0.14)],
        ),
        SceneKind::BacklitSilhouette => (None, vec![rect(0.44 + dx, 0.30, 0.08, 0.13)]),
        SceneKind::LowKeyPortrait => (Some(rect(0.40 + dx, 0.70, 0.08, 0.1)), vec![rect(0.42 + dx, 0.22, 0.16, 0.26)]),
    }
}

/// Linear sRGB of the illuminant (K, tint) relative to D65 (G = 1).
pub fn illuminant_rgb(k: f32, tint: f32) -> [f32; 3] {
    let (x, y) = super::super::wb::xy_for(f64::from(k), f64::from(tint));
    let (xx, zz) = (x / y, (1.0 - x - y) / y);
    let rgb =
        [3.2406 * xx - 1.5372 - 0.4986 * zz, -0.9689 * xx + 1.8758 + 0.0415 * zz, 0.0557 * xx - 0.2040 + 1.0570 * zz];
    let (xd, yd) = super::super::wb::xy_for(6504.0, 0.0);
    let (xxd, zzd) = (xd / yd, (1.0 - xd - yd) / yd);
    let d65 = [
        3.2406 * xxd - 1.5372 - 0.4986 * zzd,
        -0.9689 * xxd + 1.8758 + 0.0415 * zzd,
        0.0557 * xxd - 0.2040 + 1.0570 * zzd,
    ];
    let rel = [0, 1, 2].map(|c| (rgb[c] / d65[c]).max(0.01));
    [rel[0] / rel[1], 1.0, rel[2] / rel[1]].map(|v| v as f32)
}

fn inside(r: &NormRect, u: f32, v: f32) -> bool {
    u >= r.x && u < r.x + r.width && v >= r.y && v < r.y + r.height
}

fn in_ellipse(cx: f32, cy: f32, rx: f32, ry: f32, u: f32, v: f32) -> bool {
    let (a, b) = ((u - cx) / rx, (v - cy) / ry);
    a * a + b * b <= 1.0
}

const SKIN: [f32; 3] = [0.46, 0.32, 0.24];
const GREY: [f32; 3] = [0.18, 0.18, 0.18];

/// (reflectance, emission) at (u, v) of `f`'s layout; emission is not lit by the illuminant.
fn sample(f: &SynthFrame, u: f32, v: f32, noise: f32) -> ([f32; 3], [f32; 3]) {
    let dx = f.dx;
    let none = [0.0; 3];
    if let Some(c) = &f.card {
        if inside(c, u, v) {
            return (GREY, none);
        }
    }
    for face in &f.faces {
        let (cx, cy) = (face.x + face.width / 2.0, face.y + face.height / 2.0);
        if in_ellipse(cx, cy, face.width * 0.45, face.height * 0.5, u, v) {
            return (SKIN.map(|s| s * (1.0 + 0.05 * noise)), none);
        }
    }
    let t = 1.0 + 0.04 * noise;
    match f.kind {
        SceneKind::DarkChurch => {
            // Windows (daylight emitters) and candles.
            if (inside(&rect(0.15 + dx, 0.06, 0.07, 0.24), u, v) || inside(&rect(0.75 + dx, 0.06, 0.07, 0.24), u, v))
                && f.kind == SceneKind::DarkChurch
            {
                return (none, [9.0, 9.5, 10.0]);
            }
            if in_ellipse(0.30 + dx, 0.5, 0.008, 0.02, u, v) || in_ellipse(0.68 + dx, 0.5, 0.008, 0.02, u, v) {
                return (none, [14.0, 10.0, 5.0]);
            }
            // Bodies (dark suits, a white dress) under the faces.
            if inside(&rect(0.38 + dx, 0.44, 0.12, 0.4), u, v) {
                return ([0.04, 0.04, 0.045].map(|x| x * t), none);
            }
            if inside(&rect(0.53 + dx, 0.46, 0.12, 0.4), u, v) {
                return ([0.75, 0.74, 0.72].map(|x| x * t), none);
            }
            if v > 0.72 {
                return ([0.07, 0.045, 0.03].map(|x| x * t), none); // pews
            }
            let wall = 0.16 + 0.10 * (1.0 - v);
            ([wall, wall * 0.99, wall * 0.97].map(|x| x * t), none)
        }
        SceneKind::BrightOutdoor => {
            if v < 0.3 {
                return (none, [0.55, 0.72, 1.0].map(|x| x * (1.0 + 0.2 * (0.3 - v))));
            }
            if inside(&rect(0.34 + dx, 0.42, 0.12, 0.45), u, v) {
                return ([0.03, 0.03, 0.035].map(|x| x * t), none);
            }
            if inside(&rect(0.50 + dx, 0.40, 0.13, 0.5), u, v) {
                return ([0.82, 0.81, 0.79].map(|x| x * t), none);
            }
            if v < 0.5 {
                return ([0.10, 0.16, 0.07].map(|x| x * t), none); // trees
            }
            ([0.09, 0.2, 0.06].map(|x| x * t), none) // grass
        }
        SceneKind::TungstenReception => {
            if inside(&rect(0.40 + dx, 0.45, 0.13, 0.4), u, v) {
                return ([0.04, 0.04, 0.045].map(|x| x * t), none);
            }
            if inside(&rect(0.58 + dx, 0.47, 0.12, 0.4), u, v) {
                return ([0.55, 0.2, 0.25].map(|x| x * t), none); // dress
            }
            if v > 0.75 {
                return ([0.72, 0.72, 0.70].map(|x| x * t), none); // table cloth
            }
            if in_ellipse(0.85 + dx, 0.15, 0.02, 0.03, u, v) {
                return (none, [16.0, 11.0, 5.0]); // lamp
            }
            let wall = 0.30 + 0.08 * v;
            ([wall * 1.05, wall, wall * 0.85].map(|x| x * t), none)
        }
        SceneKind::BacklitSilhouette => {
            // Two figures (unlit side) and dark ground against a bright sky.
            let figure = inside(&rect(0.40 + dx, 0.38, 0.16, 0.62), u, v)
                || in_ellipse(0.48 + dx, 0.36, 0.05, 0.08, u, v)
                || inside(&rect(0.57 + dx, 0.42, 0.12, 0.58), u, v)
                || in_ellipse(0.63 + dx, 0.38, 0.04, 0.07, u, v);
            if figure || v > 0.55 {
                return ([0.012, 0.011, 0.011].map(|x| x * t), none);
            }
            (none, [1.5, 1.45, 1.35].map(|x| x * (1.0 - 0.3 * v) * t))
        }
        SceneKind::LowKeyPortrait => {
            // Spot-lit shoulders, a rim highlight, black background.
            if inside(&rect(0.36 + dx, 0.48, 0.28, 0.5), u, v) {
                let lit = (1.0 - (u - 0.5 - dx).abs() * 4.0).clamp(0.0, 1.0);
                return ([0.25, 0.2, 0.18].map(|x| x * lit * t), none);
            }
            if in_ellipse(0.59 + dx, 0.36, 0.015, 0.15, u, v) {
                return (none, [6.0, 6.0, 6.0]);
            }
            ([0.006, 0.006, 0.007].map(|x| x * t), none)
        }
    }
}

/// The faces of a silhouette are on the unlit side.
fn light_factor(f: &SynthFrame, u: f32, v: f32) -> f32 {
    if f.kind == SceneKind::BacklitSilhouette && f.faces.iter().any(|r| inside(r, u, v)) {
        0.03
    } else {
        1.0
    }
}

/// sRGB-encoded 16-bit RGB of `f` at `w` x `h`.
pub fn render(f: &SynthFrame, w: u32, h: u32) -> Vec<u16> {
    let (level, _) = f.kind.level();
    let gain = level * 2f32.powf(f.shift);
    let illum = illuminant_rgb(f.illuminant_k, f.illuminant_tint);
    let mut rng = Lcg::new(0xC0FFEE ^ (f.index as u64) ^ ((f.scene_id as u64) << 20));
    let mut out = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
            let n = rng.sym();
            let (refl, emit) = sample(f, u, v, n);
            let lf = light_factor(f, u, v);
            for c in 0..3 {
                let lin = refl[c] * illum[c] * gain * lf + emit[c] * gain;
                let lin = (lin * (1.0 + 0.01 * n)).clamp(0.0, 1.0);
                let e = if lin <= 0.003_130_8 { 12.92 * lin } else { 1.055 * lin.powf(1.0 / 2.4) - 0.055 };
                out.push((e * 65535.0).round() as u16);
            }
        }
    }
    out
}

/// Writes 16-bit RGB `pixels` as a PNG.
pub fn write_png(path: &Path, w: u32, h: u32, pixels: &[u16]) -> std::io::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Sixteen);
    let mut writer = enc.write_header().map_err(std::io::Error::other)?;
    let bytes: Vec<u8> = pixels.iter().flat_map(|v| v.to_be_bytes()).collect();
    writer.write_image_data(&bytes).map_err(std::io::Error::other)?;
    writer.finish().map_err(std::io::Error::other)
}

/// The standard acceptance shoot: `per_scene` frames of each [`SceneKind`] in bursts of 4,
/// scenes an hour apart (scene ids 1..=5, burst ids from 100 per scene).
pub fn shoot(per_scene: usize, seed: u64) -> Vec<SynthFrame> {
    let mut out = Vec::new();
    for (k, kind) in SceneKind::ALL.iter().enumerate() {
        let scene = k as i64 + 1;
        out.extend(plan(*kind, scene, scene * 100, per_scene, 4, 1_700_000_000_000 + scene * 3_600_000, seed));
    }
    out
}

/// Mean L*, a*, b* of the 8-bit sRGB `rgb` (w x h) inside `r` (inner 60 %), or the whole
/// frame when `None`.
pub fn lab_mean(rgb: &[u8], w: u32, h: u32, r: Option<&NormRect>) -> [f32; 3] {
    let (x0, x1, y0, y1) = match r {
        Some(r) => {
            let (ix, iy) = (r.width * 0.2, r.height * 0.2);
            (
                ((r.x + ix) * w as f32) as u32,
                ((r.x + r.width - ix) * w as f32) as u32,
                ((r.y + iy) * h as f32) as u32,
                ((r.y + r.height - iy) * h as f32) as u32,
            )
        }
        None => (0, w, 0, h),
    };
    let (mut s, mut n) = ([0.0f64; 3], 0usize);
    for y in y0..y1.min(h) {
        for x in x0..x1.min(w) {
            let i = ((y * w + x) * 3) as usize;
            let lab = srgb8_to_lab([rgb[i], rgb[i + 1], rgb[i + 2]]);
            for c in 0..3 {
                s[c] += f64::from(lab[c]);
            }
            n += 1;
        }
    }
    let n = n.max(1) as f64;
    s.map(|v| (v / n) as f32)
}

/// Share of the scene light's distance from D65 (mireds / tint) the camera's own auto white
/// balance leaves in the file: camera JPEGs of tungsten light stay warm, but far from a raw
/// 2900 K cast.
pub const CAMERA_AWB_KEEP: f32 = 0.4;

/// The cast a camera leaves of scene light `k` (K): [`CAMERA_AWB_KEEP`] of its mired distance
/// from D65.
pub fn camera_residual_k(k: f32) -> f32 {
    let d65 = 1e6 / 6504.0;
    1e6 / (d65 + CAMERA_AWB_KEEP * (1e6 / k - d65))
}

/// CIELAB (D65) of an 8-bit sRGB pixel.
pub fn srgb8_to_lab(px: [u8; 3]) -> [f32; 3] {
    let lin = px.map(|v| {
        let e = f32::from(v) / 255.0;
        if e <= 0.04045 {
            e / 12.92
        } else {
            ((e + 0.055) / 1.055).powf(2.4)
        }
    });
    let x = (0.4124 * lin[0] + 0.3576 * lin[1] + 0.1805 * lin[2]) / 0.95047;
    let y = 0.2126 * lin[0] + 0.7152 * lin[1] + 0.0722 * lin[2];
    let z = (0.0193 * lin[0] + 0.1192 * lin[1] + 0.9505 * lin[2]) / 1.08883;
    let f = |t: f32| if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}
