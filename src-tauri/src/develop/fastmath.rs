//! Fast `log2` / `exp2` / sRGB transfer for the per-render tables (3D LUT nodes, profile
//! HueSatMap lookups) and per-pixel LUT coordinates: polynomial approximations with errors
//! ~1e-6, far below 16-bit output precision, several times faster than libm's `powf`.

/// log2 of a positive normal f32 (exponent bits + a degree-6 fit of the mantissa;
/// max error ~3e-6).
#[inline(always)]
pub fn log2(x: f32) -> f32 {
    let bits = x.to_bits();
    let e = ((bits >> 23) & 0xff) as i32 - 127;
    let m = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000) - 1.0;
    let p = m
        * (1.442_502_3
            + m * (-0.717_677_4 + m * (0.455_789_04 + m * (-0.274_933_76 + m * (0.119_482_85 + m * -0.025_163_25)))));
    e as f32 + p
}

/// 2^x for x in about [-126, 127] (max relative error ~2e-7).
#[inline(always)]
pub fn exp2(x: f32) -> f32 {
    let x = x.clamp(-126.0, 127.0);
    let fl = x.floor();
    let f = x - fl;
    let p = 1.0 + f * (0.693_153_6 + f * (0.240_144_18 + f * (0.055_858_58 + f * (0.008_948_437 + f * 0.001_895_211))));
    f32::from_bits(((fl as i32 + 127) as u32) << 23) * p
}

/// sRGB encoding (IEC 61966-2-1) of linear `v` (clamped at 0 below).
#[inline(always)]
pub fn srgb_encode(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        12.92 * v.max(0.0)
    } else {
        1.055 * exp2(log2(v) * (1.0 / 2.4)) - 0.055
    }
}

/// Inverse of [`srgb_encode`].
#[inline(always)]
pub fn srgb_decode(e: f32) -> f32 {
    if e <= 0.040_45 {
        e / 12.92
    } else {
        exp2(log2((e + 0.055) / 1.055) * 2.4)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn accurate_against_libm() {
        let (mut wl, mut we, mut ws, mut wd) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        let mut x = 1e-6f32;
        while x < 1e4 {
            wl = wl.max((super::log2(x) - x.log2()).abs());
            x *= 1.0007;
        }
        let mut t = -30.0f32;
        while t < 30.0 {
            we = we.max((super::exp2(t) / t.exp2() - 1.0).abs());
            t += 0.0013;
        }
        for i in 0..=100_000 {
            let v = i as f32 / 100_000.0;
            let e = if v <= 0.003_130_8 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
            ws = ws.max((super::srgb_encode(v) - e).abs());
            let d = if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
            wd = wd.max((super::srgb_decode(v) - d).abs());
        }
        assert!(wl < 1e-5, "log2 {wl}");
        assert!(we < 1e-6, "exp2 {we}");
        assert!(ws < 5e-6 && wd < 5e-6, "srgb {ws} {wd}");
    }
}
