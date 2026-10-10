//! Transform / Upright geometry (IPC v19): the warp between the *corrected* frame (what the
//! crop and the output see) and the source, both normalized 0..=1 in the un-oriented sensor
//! frame, and the crop limits that go with it.
//!
//! Composition (corrected -> source), Lightroom's order: the manual sliders apply on top of
//! the Upright solution, so a corrected point goes back through the sliders first, then the
//! solution: `T = S * M^-1`, where `S = solution.matrix` (Upright frame -> source) and `M` the
//! forward slider warp (Upright frame -> corrected). The crop (`develop::parity::crop_geometry`)
//! is expressed in the corrected frame, which has the source's size and aspect.
//!
//! Lightroom's `crs:UprightTransform_N` matrices map source -> corrected (verified on the
//! user's sidecars: with `CropConstrainToWarp = 1` every corner of Lightroom's crop maps
//! exactly onto the source border through the inverse), so `solution.matrix` is their
//! inverse ([`from_lightroom`]).
//!
//! Manual sliders (forward, in pixel units centred on the frame, height = 1; Lightroom's exact
//! formulas are not published, these follow its visible behaviour, see `docs/decisions.md`):
//! - Vertical `v`: camera pitch `phi = -v / 100 * 30 deg` through a 35 mm-equivalent lens
//!   (focal = 35/36 of the long edge); negative widens the top (fixes verticals converging
//!   upwards, a camera tilted up), the centre stays put.
//! - Horizontal `h`: yaw `psi = h / 100 * 30 deg`; positive enlarges the right side.
//! - Rotate `r` degrees: positive turns the image clockwise.
//! - Aspect `a`: area-preserving stretch, `x * 2^(a/200)`, `y / 2^(a/200)` (positive = wider).
//! - Scale `s` %: zoom about the centre.
//! - Offset X / Y: `+-100` shifts the image by a quarter of the frame (positive = right / up).

use crate::ipc::types::{
    CropSettings, CrsProperty, ParametricAdjustments, TransformBounds, TransformSettings, UprightMode,
};

/// Row-major 3x3 homography.
pub type Mat3 = [f64; 9];

pub const IDENTITY: Mat3 = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

/// Largest pitch / yaw of the Vertical / Horizontal sliders at +-100, degrees.
pub const MAX_KEYSTONE_DEG: f64 = 30.0;
/// Shift of Offset X / Y at +-100, fraction of the frame.
pub const MAX_OFFSET: f64 = 0.25;

pub fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut out = [0.0; 9];
    for r in 0..3 {
        for c in 0..3 {
            out[r * 3 + c] = (0..3).map(|k| a[r * 3 + k] * b[k * 3 + c]).sum();
        }
    }
    out
}

pub fn invert(m: &Mat3) -> Option<Mat3> {
    let [a, b, c, d, e, f, g, h, i] = *m;
    let co = [e * i - f * h, -(d * i - f * g), d * h - e * g];
    let det = a * co[0] + b * co[1] + c * co[2];
    if !det.is_finite() || det.abs() < 1e-12 {
        return None;
    }
    let inv = [
        co[0],
        -(b * i - c * h),
        b * f - c * e,
        co[1],
        a * i - c * g,
        -(a * f - c * d),
        co[2],
        -(a * h - b * g),
        a * e - b * d,
    ];
    Some(inv.map(|v| v / det))
}

/// Applies `m` to `(x, y)`; `None` behind the projection centre.
#[inline]
pub fn apply(m: &Mat3, x: f64, y: f64) -> Option<(f64, f64)> {
    let w = m[6] * x + m[7] * y + m[8];
    if w <= 1e-12 {
        return None;
    }
    Some(((m[0] * x + m[1] * y + m[2]) / w, (m[3] * x + m[4] * y + m[5]) / w))
}

/// Normalizes so `m[8] = 1` (when possible).
fn normalized(m: Mat3) -> Mat3 {
    if m[8].abs() > 1e-12 {
        m.map(|v| v / m[8])
    } else {
        m
    }
}

fn is_identity(m: &Mat3) -> bool {
    let n = normalized(*m);
    n.iter().zip(IDENTITY.iter()).all(|(a, b)| (a - b).abs() < 1e-12)
}

/// `solution.matrix` from a Lightroom `crs:UprightTransform_N` value (source -> corrected).
pub fn from_lightroom(source_to_corrected: &Mat3) -> Option<Mat3> {
    invert(source_to_corrected).map(normalized)
}

/// Parses a `crs:UprightTransform_N` value (`"a,b,c,d,e,f,g,h,i"`).
pub fn parse_matrix(s: &str) -> Option<Mat3> {
    let v: Vec<f64> = s.split(',').map(|p| p.trim().parse::<f64>().ok()).collect::<Option<_>>()?;
    let m: Mat3 = v.try_into().ok()?;
    m.iter().all(|x| x.is_finite()).then_some(m)
}

/// In-plane rotation of a corrected -> source matrix, degrees, positive = the image turns
/// counter-clockwise on screen (aspect-free: in normalized coordinates the rotation part is
/// `[c, -s h/w; s w/h, c]`, so `s^2 = -m01 * m10`).
pub fn rotation_deg(corrected_to_source: &Mat3) -> f32 {
    let m = normalized(*corrected_to_source);
    let s2 = (-m[1] * m[3]).max(0.0);
    let s = s2.sqrt() * m[3].signum();
    // corrected -> source turning by +theta (clockwise on screen, y down) shows the image
    // turned counter-clockwise.
    s.atan2(m[0]).to_degrees() as f32
}

/// The Upright solution of `t` when it applies (`solution.mode == upright`, mode not off).
pub fn solution_matrix(t: &TransformSettings) -> Option<Mat3> {
    let sol = t.solution.as_ref()?;
    if t.upright == UprightMode::Off || sol.mode != t.upright {
        return None;
    }
    let m: Mat3 = sol.matrix.clone().try_into().ok()?;
    (m.iter().all(|v| v.is_finite()) && invert(&m).is_some()).then_some(m)
}

/// A Lightroom solution for `mode` from the verbatim `crs:` Upright state (every mode's
/// matrix is stored, `UprightTransform_<crs value>`), e.g. to switch modes on a photo Lightroom
/// already solved without detecting lines again. `None` when absent / identity for a mode
/// other than off.
pub fn lightroom_solution(crs: &[CrsProperty], mode: UprightMode) -> Option<crate::ipc::types::UprightSolution> {
    if mode == UprightMode::Off {
        return None;
    }
    let key = format!("UprightTransform_{}", mode.crs_value());
    let raw = crs.iter().find(|p| p.name == key)?;
    let m = from_lightroom(&parse_matrix(&raw.value)?)?;
    Some(crate::ipc::types::UprightSolution {
        mode,
        matrix: m.to_vec(),
        rotation_deg: rotation_deg(&m),
        crs: crs.to_vec(),
    })
}

/// Forward slider warp (Upright frame -> corrected frame), normalized coordinates of a frame
/// with `aspect = width / height`. Identity for neutral sliders.
pub fn sliders_forward(t: &TransformSettings, aspect: f64) -> Mat3 {
    let aspect = if aspect.is_finite() && aspect > 0.0 { aspect } else { 1.0 };
    // Normalized -> centred pixel units (height 1).
    let n = [aspect, 0.0, -aspect / 2.0, 0.0, 1.0, -0.5, 0.0, 0.0, 1.0];
    let n_inv = [1.0 / aspect, 0.0, 0.5, 0.0, 1.0, 0.5, 0.0, 0.0, 1.0];
    let f = 35.0 / 36.0 * aspect.max(1.0);
    let mut m = IDENTITY;

    // Keystone: K R K^-1 with K = diag(f, f, 1) (centred), then re-centred.
    let phi = -(f64::from(t.vertical) / 100.0 * MAX_KEYSTONE_DEG).to_radians();
    let psi = (f64::from(t.horizontal) / 100.0 * MAX_KEYSTONE_DEG).to_radians();
    if phi != 0.0 || psi != 0.0 {
        let (sp, cp) = phi.sin_cos();
        let (sy, cy) = psi.sin_cos();
        // Rays (X/f, Y/f, 1) rotated by R_y(psi) * R_x(phi).
        let rx = [1.0, 0.0, 0.0, 0.0, cp, -sp, 0.0, sp, cp];
        let ry = [cy, 0.0, sy, 0.0, 1.0, 0.0, -sy, 0.0, cy];
        let r = mul(&ry, &rx);
        let k = [f, 0.0, 0.0, 0.0, f, 0.0, 0.0, 0.0, 1.0];
        let k_inv = [1.0 / f, 0.0, 0.0, 0.0, 1.0 / f, 0.0, 0.0, 0.0, 1.0];
        let mut kr = mul(&k, &mul(&r, &k_inv));
        if let Some((cx, cy)) = apply(&kr, 0.0, 0.0) {
            kr = mul(&[1.0, 0.0, -cx, 0.0, 1.0, -cy, 0.0, 0.0, 1.0], &kr);
        }
        m = mul(&kr, &m);
    }
    if t.rotate != 0.0 {
        let (s, c) = f64::from(t.rotate).to_radians().sin_cos();
        m = mul(&[c, -s, 0.0, s, c, 0.0, 0.0, 0.0, 1.0], &m);
    }
    if t.aspect != 0.0 {
        let k = 2f64.powf(f64::from(t.aspect) / 200.0);
        m = mul(&[k, 0.0, 0.0, 0.0, 1.0 / k, 0.0, 0.0, 0.0, 1.0], &m);
    }
    if t.scale != 100.0 {
        let s = f64::from(t.scale) / 100.0;
        m = mul(&[s, 0.0, 0.0, 0.0, s, 0.0, 0.0, 0.0, 1.0], &m);
    }
    if t.offset_x != 0.0 || t.offset_y != 0.0 {
        let dx = f64::from(t.offset_x) / 100.0 * MAX_OFFSET * aspect;
        let dy = -f64::from(t.offset_y) / 100.0 * MAX_OFFSET;
        m = mul(&[1.0, 0.0, dx, 0.0, 1.0, dy, 0.0, 0.0, 1.0], &m);
    }
    normalized(mul(&n_inv, &mul(&m, &n)))
}

/// Corrected -> source homography of `t` for a sensor of `aspect` (`None` = no warp).
pub fn homography(t: &TransformSettings, aspect: f64) -> Option<Mat3> {
    let s = solution_matrix(t);
    let fwd = sliders_forward(t, aspect);
    let sliders = if is_identity(&fwd) { None } else { invert(&fwd) };
    let total = match (s, sliders) {
        (None, None) => return None,
        (Some(s), None) => s,
        (None, Some(m)) => m,
        (Some(s), Some(m)) => mul(&s, &m),
    };
    let total = normalized(total);
    (!is_identity(&total)).then_some(total)
}

/// Geometry of a render: the effective crop (in the corrected frame) and the warp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    pub crop: CropSettings,
    /// Corrected -> source homography (`None` = the corrected frame is the source).
    pub warp: Option<Mat3>,
}

impl From<&CropSettings> for Geometry {
    fn from(crop: &CropSettings) -> Self {
        Geometry { crop: *crop, warp: None }
    }
}

impl From<CropSettings> for Geometry {
    fn from(crop: CropSettings) -> Self {
        Geometry { crop, warp: None }
    }
}

impl From<&Geometry> for Geometry {
    fn from(g: &Geometry) -> Self {
        *g
    }
}

impl Geometry {
    /// The geometry `adj` renders with on a sensor of `sensor_w x sensor_h` (any scale; only
    /// the aspect matters): warp from `adj.transform`, crop limited to the warped image with
    /// `transform.constrainCrop` ([`constrain_crop`]).
    pub fn of(adj: &ParametricAdjustments, sensor_w: u32, sensor_h: u32) -> Geometry {
        let aspect = f64::from(sensor_w.max(1)) / f64::from(sensor_h.max(1));
        let warp = homography(&adj.transform, aspect);
        let crop =
            if adj.transform.constrain_crop { constrain_crop(&adj.crop, warp.as_ref(), aspect) } else { adj.crop };
        Geometry { crop, warp }
    }

    /// Cache key bits.
    pub fn key(&self) -> (Option<[u32; 5]>, Option<[u64; 9]>) {
        let c = &self.crop;
        (
            c.enabled.then(|| [c.top, c.left, c.bottom, c.right, c.angle].map(f32::to_bits)),
            self.warp.map(|m| m.map(f64::to_bits)),
        )
    }

    /// Corrected-frame point -> source point (normalized).
    #[inline]
    pub fn to_source(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        match &self.warp {
            Some(m) => apply(m, x, y),
            None => Some((x, y)),
        }
    }
}

/// Crop-tool bounds of `adj` on a sensor of `sensor_w x sensor_h` (un-oriented; only the
/// aspect matters) shown with EXIF `orientation` (IPC v19.3 `get_transform_bounds`).
pub fn bounds(adj: &ParametricAdjustments, sensor_w: u32, sensor_h: u32, orientation: u8) -> TransformBounds {
    let aspect = f64::from(sensor_w.max(1)) / f64::from(sensor_h.max(1));
    let Some(warp) = homography(&adj.transform, aspect) else {
        return TransformBounds { valid_quad: None, constrained_crop: None };
    };
    TransformBounds {
        valid_quad: warped_outline(&warp, orientation),
        constrained_crop: Some(constrain_crop(&adj.crop, Some(&warp), aspect)),
    }
}

/// The source rectangle's corners mapped into the corrected frame by the inverse of `warp`
/// (corrected -> source), then into the oriented display frame; clockwise on screen.
/// `None` when a corner has no finite image.
fn warped_outline(warp: &Mat3, orientation: u8) -> Option<Vec<(f64, f64)>> {
    let inv = invert(warp)?;
    let o = super::parity::orientation_map(if (1..=8).contains(&orientation) { orientation } else { 1 });
    let to_display = invert(&[o[0], o[1], o[2], o[3], o[4], o[5], 0.0, 0.0, 1.0])?;
    let mut pts = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
        .iter()
        .map(|&(x, y)| apply(&inv, x, y).and_then(|(cx, cy)| apply(&to_display, cx, cy)))
        .collect::<Option<Vec<_>>>()?;
    if pts.iter().any(|p| !p.0.is_finite() || !p.1.is_finite()) {
        return None;
    }
    // Shoelace in y-down coordinates: positive = clockwise on screen.
    let area: f64 = (0..4).map(|i| pts[i].0 * pts[(i + 1) % 4].1 - pts[(i + 1) % 4].0 * pts[i].1).sum();
    if area < 0.0 {
        pts.reverse();
    }
    Some(pts)
}

/// The four corners (TL, TR, BR, BL) of `crop`'s frame in the corrected frame, normalized,
/// plus its centre. Disabled crop = the whole frame.
fn crop_corners(crop: &CropSettings, aspect: f64) -> [(f64, f64); 4] {
    let g = super::parity::crop_geometry(crop, (aspect * 10_000.0).round() as u32, 10_000, 1);
    [g.map(0.0, 0.0), g.map(1.0, 0.0), g.map(1.0, 1.0), g.map(0.0, 1.0)]
}

/// Half-planes `a x + b y + c >= 0` (normalized coordinates, pixel-scaled so distances are
/// comparable) bounding the convex region a crop must stay in: the frame itself and, with a
/// warp, the image of the source rectangle.
fn region_planes(warp: Option<&Mat3>, aspect: f64) -> Vec<[f64; 3]> {
    let mut quads: Vec<[(f64, f64); 4]> = vec![[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]];
    if let Some(inv) = warp.and_then(invert) {
        let pts: Option<Vec<(f64, f64)>> =
            [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].iter().map(|&(x, y)| apply(&inv, x, y)).collect();
        if let Some(p) = pts {
            quads.push([p[0], p[1], p[2], p[3]]);
        }
    }
    let mut planes = Vec::new();
    for q in quads {
        let cx = q.iter().map(|p| p.0).sum::<f64>() / 4.0;
        let cy = q.iter().map(|p| p.1).sum::<f64>() / 4.0;
        for i in 0..4 {
            let (p, r) = (q[i], q[(i + 1) % 4]);
            // Line through p, r in pixel-scaled space (x * aspect).
            let (px, py, rx, ry) = (p.0 * aspect, p.1, r.0 * aspect, r.1);
            let (mut a, mut b) = (py - ry, rx - px);
            let len = (a * a + b * b).sqrt();
            if len < 1e-12 {
                continue;
            }
            a /= len;
            b /= len;
            let mut c = -(a * px + b * py);
            if a * cx * aspect + b * cy + c < 0.0 {
                (a, b, c) = (-a, -b, -c);
            }
            planes.push([a * aspect, b, c]);
        }
    }
    planes
}

/// `crop` limited to the area the warped image covers (Lightroom "Constrain Crop"): unchanged
/// when it already fits; otherwise the largest frame with the same aspect and angle that
/// fits (no larger than `crop`), as close as possible to where `crop` was. A disabled crop
/// becomes the largest frame of the sensor's aspect (Lightroom's constrained default crop).
pub fn constrain_crop(crop: &CropSettings, warp: Option<&Mat3>, aspect: f64) -> CropSettings {
    let planes = region_planes(warp, aspect);
    let corners = crop_corners(crop, aspect);
    let inside = |p: (f64, f64)| planes.iter().all(|h| h[0] * p.0 + h[1] * p.1 + h[2] >= -1e-9);
    // Already fits (within a twentieth of a pixel: Lightroom's own constrained crops touch
    // the border).
    let fits_now = |p: (f64, f64)| planes.iter().all(|h| h[0] * p.0 + h[1] * p.1 + h[2] >= -1e-5);
    if corners.iter().all(|&c| fits_now(c)) {
        return *crop;
    }
    let cx = corners.iter().map(|p| p.0).sum::<f64>() / 4.0;
    let cy = corners.iter().map(|p| p.1).sum::<f64>() / 4.0;
    let d: Vec<(f64, f64)> = corners.iter().map(|p| (p.0 - cx, p.1 - cy)).collect();
    // Variables (px, py, s): corner_i = P + s d_i. Constraint rows: k . (px, py, s) + k0 >= 0.
    let mut rows: Vec<[f64; 4]> = Vec::new();
    for h in &planes {
        for di in &d {
            rows.push([h[0], h[1], h[0] * di.0 + h[1] * di.1, h[2]]);
        }
    }
    rows.push([0.0, 0.0, -1.0, 1.0]); // s <= 1
    rows.push([0.0, 0.0, 1.0, 0.0]); // s >= 0
    let feasible = |v: &[f64; 3]| rows.iter().all(|r| r[0] * v[0] + r[1] * v[1] + r[2] * v[2] + r[3] >= -1e-9);
    // Maximize s: enumerate vertices of the 3-variable polytope.
    let mut best: Option<[f64; 3]> = None;
    let n = rows.len();
    for i in 0..n {
        for j in i + 1..n {
            for k in j + 1..n {
                let Some(v) = solve3(&rows[i], &rows[j], &rows[k]) else { continue };
                if feasible(&v) && best.is_none_or(|b| v[2] > b[2] + 1e-12) {
                    best = Some(v);
                }
            }
        }
    }
    let Some(best) = best else { return *crop };
    let s = (best[2] * (1.0 - 1e-6)).max(0.0);
    // Slide from the optimum towards the original centre while it still fits.
    let fits = |px: f64, py: f64| d.iter().all(|di| inside((px + s * di.0, py + s * di.1)));
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    if fits(cx, cy) {
        lo = 1.0;
    } else {
        for _ in 0..40 {
            let t = (lo + hi) / 2.0;
            if fits(best[0] + t * (cx - best[0]), best[1] + t * (cy - best[1])) {
                lo = t;
            } else {
                hi = t;
            }
        }
    }
    let (px, py) = (best[0] + lo * (cx - best[0]), best[1] + lo * (cy - best[1]));
    // Back to CropSettings: (left, top) / (right, bottom) are the frame's TL / BR corners.
    let tl = (px + s * d[0].0, py + s * d[0].1);
    let br = (px + s * d[2].0, py + s * d[2].1);
    let clamp = |v: f64| v.clamp(0.0, 1.0) as f32;
    CropSettings {
        enabled: true,
        left: clamp(tl.0),
        top: clamp(tl.1),
        right: clamp(br.0),
        bottom: clamp(br.1),
        angle: if crop.enabled { crop.angle } else { 0.0 },
    }
}

/// Solves the 3x3 system of the equality versions of three constraint rows.
fn solve3(a: &[f64; 4], b: &[f64; 4], c: &[f64; 4]) -> Option<[f64; 3]> {
    let m = [a[0], a[1], a[2], b[0], b[1], b[2], c[0], c[1], c[2]];
    let inv = invert(&m)?;
    let rhs = [-a[3], -b[3], -c[3]];
    Some([
        inv[0] * rhs[0] + inv[1] * rhs[1] + inv[2] * rhs[2],
        inv[3] * rhs[0] + inv[4] * rhs[1] + inv[5] * rhs[2],
        inv[6] * rhs[0] + inv[7] * rhs[1] + inv[8] * rhs[2],
    ])
}

#[cfg(test)]
mod tests;
