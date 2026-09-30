//! White balance conversions. Temperature/tint follow Adobe's DNG model (Robertson
//! isotherms in CIE 1960 uv, tint = offset along the isotherm normal, Lightroom scale:
//! tint 1 unit = 1/3000 uv, positive = magenta), so values read from Lightroom sidecars
//! land close to Lightroom's rendering.
//!
//! Approximation (documented): Adobe interpolates two colour matrices (StdA / D65) by
//! temperature; LibRaw exposes one (`cam_xyz`, the D65 `ColorMatrix2` for most cameras),
//! so neutral points far from daylight (tungsten) are a few hundred kelvin off Lightroom's
//! numbers. The camera neutral for an illuminant xy is `cam_xyz * XYZ(x, y, Y = 1)`; the
//! multipliers are its reciprocal normalized to G = 1. `values_for` inverts that path.

use crate::ipc::types::WhiteBalanceValues;

pub const MIN_TEMP: f32 = 2000.0;
pub const MAX_TEMP: f32 = 50000.0;
pub const MIN_TINT: f32 = -150.0;
pub const MAX_TINT: f32 = 150.0;

const TINT_SCALE: f64 = -3000.0;

/// Robertson isotherms: (mired, u, v, slope t).
const TEMP_TABLE: [(f64, f64, f64, f64); 31] = [
    (0.0, 0.18006, 0.26352, -0.24341),
    (10.0, 0.18066, 0.26589, -0.25479),
    (20.0, 0.18133, 0.26846, -0.26876),
    (30.0, 0.18208, 0.27119, -0.28539),
    (40.0, 0.18293, 0.27407, -0.30470),
    (50.0, 0.18388, 0.27709, -0.32675),
    (60.0, 0.18494, 0.28021, -0.35156),
    (70.0, 0.18611, 0.28342, -0.37915),
    (80.0, 0.18740, 0.28668, -0.40955),
    (90.0, 0.18880, 0.28997, -0.44278),
    (100.0, 0.19032, 0.29326, -0.47888),
    (125.0, 0.19462, 0.30141, -0.58204),
    (150.0, 0.19962, 0.30921, -0.70471),
    (175.0, 0.20525, 0.31647, -0.84901),
    (200.0, 0.21142, 0.32312, -1.0182),
    (225.0, 0.21807, 0.32909, -1.2168),
    (250.0, 0.22511, 0.33439, -1.4512),
    (275.0, 0.23247, 0.33904, -1.7298),
    (300.0, 0.24010, 0.34308, -2.0637),
    (325.0, 0.24702, 0.34655, -2.4681),
    (350.0, 0.25591, 0.34951, -2.9641),
    (375.0, 0.26400, 0.35200, -3.5814),
    (400.0, 0.27218, 0.35407, -4.3633),
    (425.0, 0.28039, 0.35577, -5.3762),
    (450.0, 0.28863, 0.35714, -6.7262),
    (475.0, 0.29685, 0.35823, -8.5955),
    (500.0, 0.30505, 0.35907, -11.324),
    (525.0, 0.31320, 0.35968, -15.628),
    (550.0, 0.32129, 0.36011, -23.325),
    (575.0, 0.32931, 0.36038, -40.770),
    (600.0, 0.33724, 0.36051, -116.45),
];

/// CIE xy chromaticity of temperature (K) / tint (Lightroom units), DNG SDK algorithm.
pub fn xy_for(temperature_k: f64, tint: f64) -> (f64, f64) {
    let r = 1.0e6 / temperature_k;
    let offset = tint * (1.0 / TINT_SCALE);
    for i in 0..30 {
        if r < TEMP_TABLE[i + 1].0 || i == 29 {
            let (r0, u0, v0, t0) = TEMP_TABLE[i];
            let (r1, u1, v1, t1) = TEMP_TABLE[i + 1];
            let f = (r1 - r) / (r1 - r0);
            let mut u = u0 * f + u1 * (1.0 - f);
            let mut v = v0 * f + v1 * (1.0 - f);
            let (len0, len1) = ((1.0 + t0 * t0).sqrt(), (1.0 + t1 * t1).sqrt());
            let (uu0, vv0) = (1.0 / len0, t0 / len0);
            let (uu1, vv1) = (1.0 / len1, t1 / len1);
            let (mut uu, mut vv) = (uu0 * f + uu1 * (1.0 - f), vv0 * f + vv1 * (1.0 - f));
            let len = (uu * uu + vv * vv).sqrt();
            uu /= len;
            vv /= len;
            u += uu * offset;
            v += vv * offset;
            let d = u - 4.0 * v + 2.0;
            return (1.5 * u / d, v / d);
        }
    }
    unreachable!()
}

/// Inverse of [`xy_for`]: (temperature K, tint).
pub fn temp_tint_for(x: f64, y: f64) -> (f64, f64) {
    let d = 1.5 - x + 6.0 * y;
    let (u, v) = (2.0 * x / d, 3.0 * y / d);
    let (mut last_dt, mut last_du, mut last_dv) = (0.0, 0.0, 0.0);
    for i in 1..=30 {
        let (ri, ui, vi, ti) = TEMP_TABLE[i];
        let len = (1.0 + ti * ti).sqrt();
        let (mut du, mut dv) = (1.0 / len, ti / len);
        let (mut uu, mut vv) = (u - ui, v - vi);
        let mut dt = -uu * dv + vv * du;
        if dt <= 0.0 || i == 30 {
            if dt > 0.0 {
                dt = 0.0;
            }
            dt = -dt;
            let f = if i == 1 { 0.0 } else { dt / (last_dt + dt) };
            let (rp, up, vp, _) = TEMP_TABLE[i - 1];
            let temp = 1.0e6 / (rp * f + ri * (1.0 - f));
            uu = u - (up * f + ui * (1.0 - f));
            vv = v - (vp * f + vi * (1.0 - f));
            du = du * (1.0 - f) + last_du * f;
            dv = dv * (1.0 - f) + last_dv * f;
            let len = (du * du + dv * dv).sqrt();
            du /= len;
            dv /= len;
            let tint = (uu * du + vv * dv) * TINT_SCALE;
            return (temp, tint);
        }
        last_dt = dt;
        last_du = du;
        last_dv = dv;
    }
    unreachable!()
}

fn mat_vec(m: &[[f32; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    let mut out = [0.0; 3];
    for (i, row) in m.iter().enumerate() {
        out[i] = f64::from(row[0]) * v[0] + f64::from(row[1]) * v[1] + f64::from(row[2]) * v[2];
    }
    out
}

/// Inverse of a 3x3 matrix (`None` if singular).
pub fn invert3(m: &[[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 || !det.is_finite() {
        return None;
    }
    let inv = 1.0 / det;
    Some([
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv,
        ],
    ])
}

fn clamp_values(temp: f64, tint: f64) -> WhiteBalanceValues {
    let t = if temp.is_finite() { temp as f32 } else { 5500.0 };
    let n = if tint.is_finite() { tint as f32 } else { 0.0 };
    WhiteBalanceValues { temperature_k: t.clamp(MIN_TEMP, MAX_TEMP), tint: n.clamp(MIN_TINT, MAX_TINT) }
}

/// Camera multipliers (R, G, B; G = 1) that neutralize the given temperature/tint.
/// `xyz_to_cam` is LibRaw's `cam_xyz` (XYZ -> camera RGB).
pub fn multipliers_for(values: WhiteBalanceValues, xyz_to_cam: &[[f32; 3]; 3]) -> [f32; 3] {
    let temp = f64::from(values.temperature_k.clamp(MIN_TEMP, MAX_TEMP));
    let tint = f64::from(values.tint.clamp(MIN_TINT, MAX_TINT));
    let (x, y) = xy_for(temp, tint);
    let xyz = [x / y, 1.0, (1.0 - x - y) / y];
    let neutral = mat_vec(xyz_to_cam, xyz);
    if neutral.iter().any(|&n| !(n.is_finite() && n > 1e-9)) {
        return [1.0; 3];
    }
    let g = neutral[1];
    [(g / neutral[0]) as f32, 1.0, (g / neutral[2]) as f32]
}

/// Inverse of [`multipliers_for`]: the temperature/tint of the given multipliers.
pub fn values_for(multipliers: [f32; 3], xyz_to_cam: &[[f32; 3]; 3]) -> WhiteBalanceValues {
    let m: [[f64; 3]; 3] = xyz_to_cam.map(|r| r.map(f64::from));
    let Some(inv) = invert3(&m) else { return clamp_values(5500.0, 0.0) };
    if multipliers.iter().any(|&v| !(v.is_finite() && v > 0.0)) {
        return clamp_values(5500.0, 0.0);
    }
    let neutral = multipliers.map(|v| 1.0 / f64::from(v));
    let mut xyz = [0.0; 3];
    for (i, row) in inv.iter().enumerate() {
        xyz[i] = row[0] * neutral[0] + row[1] * neutral[1] + row[2] * neutral[2];
    }
    let sum = xyz[0] + xyz[1] + xyz[2];
    if !(sum.is_finite() && sum.abs() > 1e-12) {
        return clamp_values(5500.0, 0.0);
    }
    let (temp, tint) = temp_tint_for(xyz[0] / sum, xyz[1] / sum);
    clamp_values(temp, tint)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sony ILCE-7M3 `ColorMatrix2` (Adobe, D65) as LibRaw stores it.
    pub const A7M3: [[f32; 3]; 3] = [[0.7374, -0.2389, -0.0551], [-0.5435, 1.3162, 0.2519], [-0.1006, 0.1795, 0.6552]];

    #[test]
    fn xy_round_trip_and_known_points() {
        // D65 is ~6504 K with a slight green tint in Adobe's model; D50 ~5003 K.
        let (t, _) = temp_tint_for(0.3127, 0.3290);
        assert!((t - 6504.0).abs() < 40.0, "{t}");
        let (t, _) = temp_tint_for(0.3457, 0.3585);
        assert!((t - 5003.0).abs() < 40.0, "{t}");
        for temp in [2000.0, 2850.0, 4000.0, 5500.0, 6500.0, 10000.0, 25000.0, 50000.0] {
            for tint in [-150.0, -20.0, 0.0, 10.0, 150.0] {
                let (x, y) = xy_for(temp, tint);
                let (t2, n2) = temp_tint_for(x, y);
                assert!((t2 - temp).abs() / temp < 0.002, "{temp} {tint} -> {t2}");
                assert!((n2 - tint).abs() < 0.5, "{temp} {tint} -> {n2}");
            }
        }
    }

    #[test]
    fn multipliers_round_trip_through_camera_matrix() {
        for (temp, tint) in [(3200.0, 0.0), (5500.0, 10.0), (7500.0, -30.0), (2500.0, 40.0)] {
            let v = WhiteBalanceValues { temperature_k: temp, tint };
            let m = multipliers_for(v, &A7M3);
            assert_eq!(m[1], 1.0);
            let back = values_for(m, &A7M3);
            assert!((back.temperature_k - temp).abs() / temp < 0.005, "{temp}: {back:?}");
            assert!((back.tint - tint).abs() < 1.0, "{tint}: {back:?}");
        }
        // Warmer light (lower K) needs more blue gain, less red gain.
        let warm = multipliers_for(WhiteBalanceValues { temperature_k: 3000.0, tint: 0.0 }, &A7M3);
        let cool = multipliers_for(WhiteBalanceValues { temperature_k: 7000.0, tint: 0.0 }, &A7M3);
        assert!(warm[2] > cool[2] && warm[0] < cool[0], "{warm:?} {cool:?}");
        // Positive tint renders more magenta (the light is taken to be greener): higher R
        // and B gains relative to G, as in Lightroom.
        let mag = multipliers_for(WhiteBalanceValues { temperature_k: 5500.0, tint: 50.0 }, &A7M3);
        let grn = multipliers_for(WhiteBalanceValues { temperature_k: 5500.0, tint: -50.0 }, &A7M3);
        assert!(mag[0] > grn[0] && mag[2] > grn[2], "{mag:?} {grn:?}");
    }

    #[test]
    fn degenerate_inputs_do_not_panic() {
        let zero = [[0.0f32; 3]; 3];
        assert_eq!(multipliers_for(WhiteBalanceValues { temperature_k: 5000.0, tint: 0.0 }, &zero), [1.0; 3]);
        let v = values_for([0.0, 1.0, 1.0], &A7M3);
        assert!((MIN_TEMP..=MAX_TEMP).contains(&v.temperature_k));
        let v = values_for([2.0, 1.0, 1.5], &zero);
        assert_eq!(v.temperature_k, 5500.0);
    }
}
