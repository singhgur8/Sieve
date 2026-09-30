//! Camera colour setup of a render: which camera profile (DCP) and look apply to a source,
//! and the white-balance-dependent camera -> working-space transform (DNG model).
//!
//! Working space: linear ProPhoto RGB (ROMM primaries, D50), the space Adobe's HSV tables are
//! defined in. A neutral at the sensor's clip level maps to (1, 1, 1).

use std::path::Path;
use std::sync::Arc;

use crate::ipc::types::{
    CameraCalibration, DevelopWarning, DevelopWarningCode, ProfileSettings, WhiteBalance, WhiteBalanceValues,
};
use crate::profiles::dcp::{self, HsvTable};
use crate::profiles::{CameraKey, Dcp, LookProfile, ProfileLibrary};

use super::source::{ColorInfo, SourceMeta};
use super::wb;

/// XYZ (D50) -> linear ProPhoto.
pub const XYZ_TO_PROPHOTO: [[f64; 3]; 3] =
    [[1.345_943_3, -0.255_607_5, -0.051_111_8], [-0.544_598_9, 1.508_167_3, 0.020_535_1], [0.0, 0.0, 1.211_812_8]];
/// Linear ProPhoto -> XYZ (D50).
pub const PROPHOTO_TO_XYZ: [[f64; 3]; 3] =
    [[0.797_674_9, 0.135_191_7, 0.031_353_4], [0.288_040_2, 0.711_874_1, 0.000_085_7], [0.0, 0.0, 0.825_210_0]];

/// The profile/look resolved for one source + `ProfileSettings`.
#[derive(Debug, Clone, Default)]
pub struct Profile {
    pub dcp: Option<Arc<Dcp>>,
    pub look: Option<Arc<LookProfile>>,
    /// `LookSettings.amount` (0..=2).
    pub look_amount: f32,
    /// Raw baseline exposure + DCP `BaselineExposureOffset` (EV); 0 for display-referred.
    pub baseline_ev: f32,
    pub display_referred: bool,
    /// Missing profile/look.
    pub warnings: Vec<DevelopWarning>,
}

impl Profile {
    /// Plain scene-referred profile (LibRaw matrix, no look, `baseline_ev`).
    pub fn matrix(baseline_ev: f32) -> Self {
        Profile { baseline_ev, ..Default::default() }
    }
}

fn camera_key(meta: &SourceMeta) -> CameraKey {
    CameraKey { format: meta.format, make: meta.make.clone(), model: meta.model.clone() }
}

/// Resolves `settings` for a source. `sidecar` = the source's XMP sidecar path (a look
/// that is not installed may be embedded there).
pub fn resolve(meta: &SourceMeta, settings: &ProfileSettings, lib: &ProfileLibrary, sidecar: Option<&Path>) -> Profile {
    let mut warnings = Vec::new();
    let key = camera_key(meta);
    let dcp = if meta.display_referred {
        None
    } else {
        match settings.camera_profile.as_deref() {
            Some(name) => {
                let d = lib.dcp(&key, name);
                if d.is_none() {
                    warnings.push(DevelopWarning {
                        code: DevelopWarningCode::ProfileUnavailable,
                        detail: Some(name.to_owned()),
                    });
                }
                d
            }
            None => None,
        }
    };
    let (look, look_amount) = match &settings.look {
        Some(l) => {
            let found = lib.look(&l.uuid).or_else(|| {
                let text = std::fs::read_to_string(sidecar?).ok()?;
                let embedded = LookProfile::from_sidecar(&text).ok()??;
                (embedded.uuid == l.uuid.to_ascii_uppercase() && !embedded.tables.is_empty())
                    .then(|| Arc::new(embedded))
            });
            let usable = found.filter(|lp| !meta.display_referred || lp.supports_output_referred);
            if usable.is_none() {
                warnings
                    .push(DevelopWarning { code: DevelopWarningCode::LookUnavailable, detail: Some(l.name.clone()) });
            }
            (usable, l.amount.clamp(0.0, 2.0))
        }
        None => (None, 1.0),
    };
    let baseline_ev = if meta.display_referred {
        0.0
    } else {
        meta.baseline_exposure
            .unwrap_or_else(|| crate::profiles::raw_baseline_exposure(meta.make.as_deref(), meta.model.as_deref()))
            + dcp.as_ref().map_or(0.0, |d| d.baseline_exposure_offset)
    };
    Profile { dcp, look, look_amount, baseline_ev, display_referred: meta.display_referred, warnings }
}

/// Single-matrix pseudo profile from the source's `ColorInfo` (LibRaw `cam_xyz`, a D65
/// `ColorMatrix`; for raster sources XYZ -> source primaries).
pub fn matrix_dcp(color: &ColorInfo) -> Dcp {
    Dcp {
        unique_camera_model: String::new(),
        profile_name: String::new(),
        calibration_illuminant1: 21,
        calibration_illuminant2: None,
        color_matrix1: color.xyz_to_cam,
        color_matrix2: None,
        forward_matrix1: None,
        forward_matrix2: None,
        hue_sat_map1: None,
        hue_sat_map2: None,
        look_table: None,
        tone_curve: None,
        baseline_exposure_offset: 0.0,
        default_black_render: 0,
    }
}

/// Per-render camera transform.
#[derive(Debug, Clone)]
pub struct ColorSetup {
    /// White-balance multipliers: w = min(camera * mul, 1) per channel (min(mul) = 1).
    pub mul: [f32; 3],
    /// White-balanced camera -> linear ProPhoto (calibration folded in).
    pub m: [[f32; 3]; 3],
    /// Profile HueSatMap for this white balance.
    pub hsm: Option<HsvTable>,
    /// White point temperature (K).
    pub temperature: f32,
}

fn neutral_of_multipliers(mul: [f32; 3]) -> Option<[f64; 3]> {
    if !mul.iter().all(|v| v.is_finite() && *v > 0.0) {
        return None;
    }
    let n = mul.map(|v| 1.0 / f64::from(v));
    let mx = n[0].max(n[1]).max(n[2]);
    Some(n.map(|v| v / mx))
}

/// White point (xy) and camera neutral (max 1) of a white balance setting.
fn white(color: &ColorInfo, d: &Dcp, wbs: &WhiteBalance) -> ((f64, f64), [f64; 3]) {
    let cc = color.calibration.map(f64::from);
    match *wbs {
        WhiteBalance::AsShot => {
            let n = neutral_of_multipliers(color.as_shot()).unwrap_or([1.0; 3]);
            (d.neutral_to_xy(n, cc), n)
        }
        WhiteBalance::Custom { temperature_k, tint } => {
            let t = f64::from(temperature_k.clamp(wb::MIN_TEMP, wb::MAX_TEMP));
            let n = f64::from(tint.clamp(wb::MIN_TINT, wb::MAX_TINT));
            let xy = wb::xy_for(t, n);
            (xy, d.xy_to_neutral(xy, cc))
        }
    }
}

/// Camera transform for a render.
pub fn color_setup(color: &ColorInfo, profile: &Profile, wbs: &WhiteBalance, cal: &CameraCalibration) -> ColorSetup {
    let fallback;
    let d: &Dcp = match &profile.dcp {
        Some(d) => d,
        None => {
            fallback = matrix_dcp(color);
            &fallback
        }
    };
    let (xy, neutral) = white(color, d, wbs);
    let (to_pcs, g) = d.camera_to_pcs(xy, neutral, color.calibration.map(f64::from));
    // Matrix on white-balanced values: camera = diag(neutral) * w.
    let mut m_w = to_pcs;
    for row in m_w.iter_mut() {
        for (j, v) in row.iter_mut().enumerate() {
            *v *= neutral[j];
        }
    }
    let pp = dcp::mul_mm(&XYZ_TO_PROPHOTO, &m_w);
    let calm = super::parity::calibration_matrix(cal).map(|r| r.map(f64::from));
    let m = dcp::mul_mm(&calm, &pp);
    let ok = m.iter().flatten().all(|v| v.is_finite());
    let m = if ok { m.map(|r| r.map(|v| v as f32)) } else { [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] };
    let mul = neutral.map(|v| (1.0 / v.max(1e-6)) as f32);
    let (temperature, _) = wb::temp_tint_for(xy.0, xy.1);
    let hsm = profile.dcp.as_ref().and_then(|d| d.hue_sat_map(g)).filter(|t| !t.is_identity());
    ColorSetup { mul, m, hsm, temperature: temperature as f32 }
}

/// The camera's as-shot white balance in Lightroom's temperature/tint scale (through the
/// profile's colour matrices when a DCP is resolved). `None` if the file has none.
pub fn as_shot_values(color: &ColorInfo, profile: &Profile) -> Option<WhiteBalanceValues> {
    let mul = color.as_shot_mul?;
    if profile.display_referred {
        return Some(WhiteBalanceValues { temperature_k: 6500.0, tint: 0.0 });
    }
    values_of_multipliers(mul, color, profile)
}

/// Temperature/tint of camera multipliers (R, G, B; > 0) through the profile's colour
/// matrices when a DCP is resolved, else LibRaw's `cam_xyz` ([`wb::values_for`]). The path
/// [`as_shot_values`] uses; also the white balance picker (`sample_white_balance`).
pub fn values_of_multipliers(mul: [f32; 3], color: &ColorInfo, profile: &Profile) -> Option<WhiteBalanceValues> {
    match &profile.dcp {
        Some(d) => {
            let n = neutral_of_multipliers(mul)?;
            let (x, y) = d.neutral_to_xy(n, color.calibration.map(f64::from));
            let (t, tint) = wb::temp_tint_for(x, y);
            Some(WhiteBalanceValues {
                temperature_k: (t as f32).clamp(wb::MIN_TEMP, wb::MAX_TEMP),
                tint: (tint as f32).clamp(wb::MIN_TINT, wb::MAX_TINT),
            })
        }
        None => Some(wb::values_for(mul, &color.xyz_to_cam)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn srgb_like() -> ColorInfo {
        // Camera = linear sRGB primaries (XYZ D65 -> sRGB).
        ColorInfo {
            as_shot_mul: Some([1.0, 1.0, 1.0]),
            daylight_mul: [1.0; 3],
            rgb_cam: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            xyz_to_cam: [
                [3.240_454_2, -1.537_138_5, -0.498_531_4],
                [-0.969_266, 1.876_010_8, 0.041_556],
                [0.055_643_4, -0.204_025_9, 1.057_225_2],
            ],
            calibration: [1.0; 3],
        }
    }

    #[test]
    fn neutral_maps_to_prophoto_white() {
        let c = srgb_like();
        let s = color_setup(&c, &Profile::matrix(0.0), &WhiteBalance::AsShot, &CameraCalibration::default());
        assert!((s.mul.iter().cloned().fold(f32::MAX, f32::min) - 1.0).abs() < 1e-6);
        let w = s.m.map(|r| r.iter().sum::<f32>());
        for v in w {
            assert!((v - 1.0).abs() < 1e-3, "{w:?}");
        }
        assert!((s.temperature - 6504.0).abs() < 60.0, "{}", s.temperature);
        let v = as_shot_values(&c, &Profile::matrix(0.0)).unwrap();
        assert!((v.temperature_k - 6504.0).abs() < 60.0);
        // Warmer custom WB -> blue gain up relative to red.
        let warm = color_setup(
            &c,
            &Profile::matrix(0.0),
            &WhiteBalance::Custom { temperature_k: 3000.0, tint: 0.0 },
            &CameraCalibration::default(),
        );
        assert!(warm.mul[2] / warm.mul[0] > s.mul[2] / s.mul[0]);
    }
}
