//! Output colour spaces and their ICC profiles.
//!
//! The profiles are generated here (no third-party profile bytes): ICC v2.1 display-class
//! matrix/TRC profiles with D50-adapted (Bradford) colorants, a D50 media white point and
//! `curv` tone curves (a 1024-entry table for the sRGB curve, a single gamma for Adobe RGB).
//! This is the same structure as the widely used compact v2 profiles, so every
//! colour-managed reader handles them. Descriptions: "sRGB IEC61966-2.1", "Display P3",
//! "Adobe RGB (1998) compatible". Released as CC0 (see `docs/decisions.md`).

use std::sync::OnceLock;

use crate::develop::pipeline::{OutputSpace, Transfer};
use crate::develop::wb;
use crate::ipc::types::ExportColorSpace;

/// Display P3 primaries (DCI-P3 primaries, D65 white, sRGB transfer).
pub const P3_PRIMARIES: [[f64; 2]; 3] = [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]];
/// Adobe RGB (1998) primaries (D65 white, gamma 563/256).
pub const ADOBE_PRIMARIES: [[f64; 2]; 3] = [[0.64, 0.33], [0.21, 0.71], [0.15, 0.06]];
/// Adobe RGB (1998) gamma, as stored in its ICC `curv` (u8Fixed8 0x0233).
pub const ADOBE_GAMMA: f32 = 563.0 / 256.0;

/// The pipeline's output space for `cs` (built once).
pub fn output_space(cs: ExportColorSpace) -> &'static OutputSpace {
    static SRGB: OnceLock<OutputSpace> = OnceLock::new();
    static P3: OnceLock<OutputSpace> = OnceLock::new();
    static ADOBE: OnceLock<OutputSpace> = OnceLock::new();
    match cs {
        ExportColorSpace::Srgb => SRGB.get_or_init(OutputSpace::srgb),
        ExportColorSpace::DisplayP3 => P3.get_or_init(|| OutputSpace::from_primaries(P3_PRIMARIES, Transfer::Srgb)),
        ExportColorSpace::AdobeRgb => {
            ADOBE.get_or_init(|| OutputSpace::from_primaries(ADOBE_PRIMARIES, Transfer::Gamma(ADOBE_GAMMA)))
        }
    }
}

/// Profile description (`desc`) of the embedded profile.
pub fn profile_description(cs: ExportColorSpace) -> &'static str {
    match cs {
        ExportColorSpace::Srgb => "sRGB IEC61966-2.1",
        ExportColorSpace::DisplayP3 => "Display P3",
        ExportColorSpace::AdobeRgb => "Adobe RGB (1998) compatible",
    }
}

/// ICC profile bytes for `cs` (generated once).
pub fn icc_profile(cs: ExportColorSpace) -> &'static [u8] {
    static SRGB: OnceLock<Vec<u8>> = OnceLock::new();
    static P3: OnceLock<Vec<u8>> = OnceLock::new();
    static ADOBE: OnceLock<Vec<u8>> = OnceLock::new();
    let cell = match cs {
        ExportColorSpace::Srgb => &SRGB,
        ExportColorSpace::DisplayP3 => &P3,
        ExportColorSpace::AdobeRgb => &ADOBE,
    };
    cell.get_or_init(|| {
        let space = output_space(cs);
        build_icc(profile_description(cs), &space.to_xyz, space.transfer)
    })
}

/// ICC PCS illuminant (D50) as encoded in s15Fixed16 by every ICC profile.
const D50: [f64; 3] = [0.9642, 1.0, 0.8249];

const BRADFORD: [[f64; 3]; 3] = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];

fn mat_mul(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

fn mat_vec(a: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| (0..3).map(|k| a[i][k] * v[k]).sum())
}

/// Bradford adaptation from white `src` (XYZ) to D50.
fn bradford_to_d50(src: [f64; 3]) -> [[f64; 3]; 3] {
    let s = mat_vec(&BRADFORD, src);
    let d = mat_vec(&BRADFORD, D50);
    let diag = [[d[0] / s[0], 0.0, 0.0], [0.0, d[1] / s[1], 0.0], [0.0, 0.0, d[2] / s[2]]];
    let inv = wb::invert3(&BRADFORD).expect("invertible");
    mat_mul(&inv, &mat_mul(&diag, &BRADFORD))
}

fn s15f16(v: f64) -> [u8; 4] {
    ((v * 65536.0).round() as i32).to_be_bytes()
}

fn xyz_tag(xyz: [f64; 3]) -> Vec<u8> {
    let mut t = b"XYZ \0\0\0\0".to_vec();
    for v in xyz {
        t.extend_from_slice(&s15f16(v));
    }
    t
}

fn desc_tag(text: &str) -> Vec<u8> {
    // textDescriptionType (ICC v2): ASCII + empty Unicode + empty ScriptCode.
    let mut t = b"desc\0\0\0\0".to_vec();
    t.extend_from_slice(&(text.len() as u32 + 1).to_be_bytes());
    t.extend_from_slice(text.as_bytes());
    t.push(0);
    t.extend_from_slice(&[0; 4]); // Unicode language code
    t.extend_from_slice(&[0; 4]); // Unicode count
    t.extend_from_slice(&[0; 2]); // ScriptCode code
    t.push(0); // ScriptCode count
    t.extend_from_slice(&[0; 67]);
    t
}

fn text_tag(text: &str) -> Vec<u8> {
    let mut t = b"text\0\0\0\0".to_vec();
    t.extend_from_slice(text.as_bytes());
    t.push(0);
    t
}

fn curv_tag(transfer: Transfer) -> Vec<u8> {
    let mut t = b"curv\0\0\0\0".to_vec();
    match transfer {
        Transfer::Gamma(g) => {
            t.extend_from_slice(&1u32.to_be_bytes());
            t.extend_from_slice(&((f64::from(g) * 256.0).round() as u16).to_be_bytes());
        }
        Transfer::Srgb => {
            const N: u32 = 1024;
            t.extend_from_slice(&N.to_be_bytes());
            for i in 0..N {
                let v = transfer.decode(i as f32 / (N - 1) as f32);
                t.extend_from_slice(&((f64::from(v) * 65535.0).round().clamp(0.0, 65535.0) as u16).to_be_bytes());
            }
        }
    }
    t
}

/// Builds an ICC v2.1 RGB display profile from `to_xyz` (linear RGB -> XYZ, D65).
fn build_icc(description: &str, to_xyz: &[[f64; 3]; 3], transfer: Transfer) -> Vec<u8> {
    let white = mat_vec(to_xyz, [1.0, 1.0, 1.0]);
    let adapted = mat_mul(&bradford_to_d50(white), to_xyz);
    let col = |j: usize| [adapted[0][j], adapted[1][j], adapted[2][j]];
    let curve = curv_tag(transfer);
    // (signature, data); tags sharing the same data point at one copy.
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", desc_tag(description)),
        (b"cprt", text_tag("No copyright (CC0). Generated by Sieve.")),
        (b"wtpt", xyz_tag(D50)),
        (b"rXYZ", xyz_tag(col(0))),
        (b"gXYZ", xyz_tag(col(1))),
        (b"bXYZ", xyz_tag(col(2))),
        (b"rTRC", curve.clone()),
        (b"gTRC", curve.clone()),
        (b"bTRC", curve),
    ];
    let table_len = 4 + 12 * tags.len();
    let mut data: Vec<u8> = Vec::new();
    let mut entries: Vec<(&[u8; 4], u32, u32)> = Vec::new();
    let mut placed: Vec<(Vec<u8>, u32)> = Vec::new();
    for (sig, bytes) in &tags {
        let offset = match placed.iter().find(|(b, _)| b == bytes) {
            Some((_, off)) => *off,
            None => {
                while !data.len().is_multiple_of(4) {
                    data.push(0);
                }
                let off = (128 + table_len + data.len()) as u32;
                data.extend_from_slice(bytes);
                placed.push((bytes.clone(), off));
                off
            }
        };
        entries.push((sig, offset, bytes.len() as u32));
    }
    while !data.len().is_multiple_of(4) {
        data.push(0);
    }
    let size = (128 + table_len + data.len()) as u32;

    let mut p = Vec::with_capacity(size as usize);
    p.extend_from_slice(&size.to_be_bytes());
    p.extend_from_slice(&[0; 4]); // preferred CMM
    p.extend_from_slice(&[0x02, 0x10, 0x00, 0x00]); // version 2.1
    p.extend_from_slice(b"mntr");
    p.extend_from_slice(b"RGB ");
    p.extend_from_slice(b"XYZ ");
    for v in [2026u16, 9, 29, 0, 0, 0] {
        p.extend_from_slice(&v.to_be_bytes());
    }
    p.extend_from_slice(b"acsp");
    p.extend_from_slice(b"APPL"); // primary platform
    p.extend_from_slice(&[0; 4]); // flags
    p.extend_from_slice(&[0; 4]); // manufacturer
    p.extend_from_slice(&[0; 4]); // model
    p.extend_from_slice(&[0; 8]); // attributes
    p.extend_from_slice(&0u32.to_be_bytes()); // perceptual intent
    for v in D50 {
        p.extend_from_slice(&s15f16(v));
    }
    p.extend_from_slice(&[0; 4]); // creator
    p.extend_from_slice(&[0; 44]); // profile ID (v4) + reserved
    debug_assert_eq!(p.len(), 128);
    p.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for (sig, off, len) in entries {
        p.extend_from_slice(sig);
        p.extend_from_slice(&off.to_be_bytes());
        p.extend_from_slice(&len.to_be_bytes());
    }
    p.extend_from_slice(&data);
    debug_assert_eq!(p.len(), size as usize);
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_xyz(p: &[u8], sig: &[u8; 4]) -> [f64; 3] {
        let n = u32::from_be_bytes(p[128..132].try_into().unwrap()) as usize;
        for i in 0..n {
            let e = &p[132 + i * 12..144 + i * 12];
            if &e[..4] == sig {
                let off = u32::from_be_bytes(e[4..8].try_into().unwrap()) as usize;
                assert_eq!(&p[off..off + 4], b"XYZ ");
                return [0, 1, 2].map(|k| {
                    f64::from(i32::from_be_bytes(p[off + 8 + k * 4..off + 12 + k * 4].try_into().unwrap())) / 65536.0
                });
            }
        }
        panic!("tag {sig:?} missing");
    }

    #[test]
    fn srgb_profile_matches_the_reference_colorants() {
        let p = icc_profile(ExportColorSpace::Srgb);
        assert_eq!(u32::from_be_bytes(p[0..4].try_into().unwrap()) as usize, p.len());
        assert_eq!(&p[36..40], b"acsp");
        // Reference: ICC's sRGB v2 colorants (D50-adapted).
        let r = read_xyz(p, b"rXYZ");
        let g = read_xyz(p, b"gXYZ");
        let b = read_xyz(p, b"bXYZ");
        for (got, want) in [(r, [0.4361, 0.2225, 0.0139]), (g, [0.3851, 0.7169, 0.0971]), (b, [0.1431, 0.0606, 0.7141])]
        {
            for k in 0..3 {
                assert!((got[k] - want[k]).abs() < 6e-4, "{got:?} vs {want:?}");
            }
        }
        // Colorants sum to the PCS white.
        for k in 0..3 {
            assert!((r[k] + g[k] + b[k] - D50[k]).abs() < 2e-3);
        }
    }

    #[test]
    fn profiles_are_distinct_and_described() {
        for cs in ExportColorSpace::ALL {
            let p = icc_profile(*cs);
            let desc = profile_description(*cs);
            assert!(p.windows(desc.len()).any(|w| w == desc.as_bytes()), "{desc}");
            assert_eq!(p.len() % 4, 0);
        }
        assert_ne!(icc_profile(ExportColorSpace::Srgb), icc_profile(ExportColorSpace::DisplayP3));
        // Adobe RGB: gamma curv with one entry = 0x0233.
        let a = icc_profile(ExportColorSpace::AdobeRgb);
        assert!(a.windows(14).any(|w| w == b"curv\0\0\0\0\0\0\0\x01\x02\x33"));
        // P3 red is outside sRGB: its linear Rec.2020 -> P3 matrix maps P3 red to (1, 0, 0).
        let p3 = output_space(ExportColorSpace::DisplayP3);
        let red_2020 = wb::invert3(&p3.from_rec2020).unwrap();
        let back = mat_vec(&p3.from_rec2020, [red_2020[0][0], red_2020[1][0], red_2020[2][0]]);
        assert!((back[0] - 1.0).abs() < 1e-9 && back[1].abs() < 1e-9);
    }
}
