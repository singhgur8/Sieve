//! Colour helpers shared by scene features and render statistics (vision-ml-dev):
//! sRGB decoding, Rec.709 luminance, Oklab, CIE xy.

use std::sync::OnceLock;

/// Rec.709 / sRGB luminance weights.
pub const LUMA_709: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// Floor of luminance in log statistics (`ImageStats::log_mean_luma`).
pub const MIN_LUMA: f32 = 1.0 / 16384.0;

/// sRGB EOTF of every 8-bit code value.
pub fn srgb_to_linear_table() -> &'static [f32; 256] {
    static T: OnceLock<[f32; 256]> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = [0.0f32; 256];
        for (i, v) in t.iter_mut().enumerate() {
            let c = i as f64 / 255.0;
            let l = if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
            *v = l as f32;
        }
        t
    })
}

/// sRGB OETF (linear 0..=1 -> encoded 0..=1).
pub fn srgb_encode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

#[inline]
pub fn luma(rgb: [f32; 3]) -> f32 {
    LUMA_709[0] * rgb[0] + LUMA_709[1] * rgb[1] + LUMA_709[2] * rgb[2]
}

/// Oklab `[L, a, b]` of a linear sRGB colour.
#[inline]
pub fn oklab(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb.map(|c| c.max(0.0));
    let l = 0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// CIE 1931 xy of a linear sRGB colour (D65 white for black / invalid input).
pub fn xy(rgb: [f32; 3]) -> (f32, f32) {
    let [r, g, b] = rgb.map(|c| f64::from(c.max(0.0)));
    let x = 0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b;
    let z = 0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b;
    let sum = x + y + z;
    if !(sum.is_finite() && sum > 1e-12) {
        return (0.3127, 0.3290);
    }
    ((x / sum) as f32, (y / sum) as f32)
}

/// Finite or `fallback`.
#[inline]
pub fn finite(v: f32, fallback: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oklab_reference_values() {
        let w = oklab([1.0, 1.0, 1.0]);
        assert!((w[0] - 1.0).abs() < 1e-3 && w[1].abs() < 1e-3 && w[2].abs() < 1e-3, "{w:?}");
        // Björn Ottosson's reference: sRGB red -> (0.628, 0.225, 0.126).
        let r = oklab([1.0, 0.0, 0.0]);
        assert!((r[0] - 0.628).abs() < 2e-3 && (r[1] - 0.2249).abs() < 2e-3 && (r[2] - 0.1258).abs() < 2e-3);
        let (x, y) = xy([0.5, 0.5, 0.5]);
        assert!((x - 0.3127).abs() < 1e-3 && (y - 0.3290).abs() < 1e-3);
        let t = srgb_to_linear_table();
        assert_eq!(t[0], 0.0);
        assert!((t[255] - 1.0).abs() < 1e-6 && (t[128] - 0.2158).abs() < 1e-3);
        assert!((srgb_encode(t[200]) * 255.0 - 200.0).abs() < 0.01);
    }
}
