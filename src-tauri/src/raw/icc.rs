//! Minimal ICC profile reader for raster sources (Phase 7b): matrix/TRC RGB and gray
//! profiles (v2 and v4), which covers sRGB, Display P3, Adobe RGB, ProPhoto and the
//! "generic" monitor/working-space profiles found in JPEG/TIFF/PNG/HEIC files.
//! LUT-only profiles (`A2B0` without colorants, CMYK, Lab) are reported as unsupported;
//! the caller then assumes sRGB (`DevelopWarningCode::SourceColorAssumed`).
//!
//! Colorants are PCS (D50) values; well-known spaces are recognised by their Bradford-
//! adapted colorants so their pixels can stay in the space's own primaries.

use super::raster::SourceColorSpace;

/// One tone reproduction curve (encoded 0..=1 -> linear 0..=1).
#[derive(Debug, Clone, PartialEq)]
pub enum Trc {
    /// `y = x^g` (`g = 1` is linear).
    Gamma(f64),
    /// ICC `para` function type 0..=4 with its parameters `[g, a, b, c, d, e, f]`.
    Parametric(u16, [f64; 7]),
    /// Sampled curve, evenly spaced over 0..=1.
    Table(Vec<f64>),
    /// IEC 61966-2-1 (used when the profile is implied, e.g. untagged or EXIF-sRGB files).
    Srgb,
}

impl Trc {
    /// Adobe RGB (1998): gamma 563/256.
    pub const ADOBE_RGB: Trc = Trc::Gamma(563.0 / 256.0);
    /// ProPhoto (ROMM) TRC as shipped in ICC profiles: gamma 1.8.
    pub const PROPHOTO: Trc = Trc::Gamma(1.8);

    pub fn eval(&self, x: f64) -> f64 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Trc::Gamma(g) => x.powf(*g),
            Trc::Srgb => {
                if x <= 0.04045 {
                    x / 12.92
                } else {
                    ((x + 0.055) / 1.055).powf(2.4)
                }
            }
            Trc::Parametric(kind, p) => {
                let [g, a, b, c, d, e, f] = *p;
                let pow = |v: f64| if v > 0.0 { v.powf(g) } else { 0.0 };
                match kind {
                    0 => pow(x),
                    1 => {
                        if a != 0.0 && x >= -b / a {
                            pow(a * x + b)
                        } else {
                            0.0
                        }
                    }
                    2 => {
                        if a != 0.0 && x >= -b / a {
                            pow(a * x + b) + c
                        } else {
                            c
                        }
                    }
                    3 => {
                        if x >= d {
                            pow(a * x + b)
                        } else {
                            c * x
                        }
                    }
                    _ => {
                        if x >= d {
                            pow(a * x + b) + e
                        } else {
                            c * x + f
                        }
                    }
                }
                .clamp(0.0, 1.0)
            }
            Trc::Table(t) => match t.len() {
                0 => x,
                1 => t[0],
                n => {
                    let pos = x * (n - 1) as f64;
                    let i = (pos.floor() as usize).min(n - 2);
                    let frac = pos - i as f64;
                    t[i] + (t[i + 1] - t[i]) * frac
                }
            },
        }
    }

    /// Lookup table from `2^bits` encoded codes to linear 0..=65535.
    pub fn lut(&self, bits: u8) -> Vec<u16> {
        let n = 1usize << bits;
        let max = (n - 1) as f64;
        (0..n).map(|i| (self.eval(i as f64 / max) * 65535.0).round().clamp(0.0, 65535.0) as u16).collect()
    }
}

/// A parsed, usable profile.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    /// Identified space (primaries), or `Unknown(description)` for other matrix profiles.
    pub space: SourceColorSpace,
    /// Linear RGB -> XYZ (D50 PCS); columns are the colorants. Gray profiles: `None`.
    pub to_xyz_d50: Option<[[f64; 3]; 3]>,
    /// Per-channel TRCs (R, G, B); gray profiles repeat `kTRC`.
    pub trc: [Trc; 3],
    pub gray: bool,
    pub description: Option<String>,
}

impl Profile {
    /// The implied profile of an untagged / EXIF-sRGB file.
    pub fn srgb() -> Self {
        Profile {
            space: SourceColorSpace::Srgb,
            to_xyz_d50: Some(known_d50(&SourceColorSpace::Srgb).expect("sRGB is known")),
            trc: [Trc::Srgb, Trc::Srgb, Trc::Srgb],
            gray: false,
            description: None,
        }
    }

    /// DCF "option file" Adobe RGB (EXIF InteropIndex `R03`) without an embedded profile.
    pub fn adobe_rgb() -> Self {
        Profile {
            space: SourceColorSpace::AdobeRgb,
            to_xyz_d50: known_d50(&SourceColorSpace::AdobeRgb),
            trc: [Trc::ADOBE_RGB, Trc::ADOBE_RGB, Trc::ADOBE_RGB],
            gray: false,
            description: None,
        }
    }

    /// Whether pixels decoded with this profile are already sRGB-encoded sRGB (the file can
    /// serve as its own preview).
    pub fn is_srgb(&self) -> bool {
        self.space == SourceColorSpace::Srgb && !self.gray
    }
}

/// CIE xy chromaticities.
pub const D65: [f64; 2] = [0.3127, 0.3290];
pub const D50: [f64; 2] = [0.3457, 0.3585];

pub const SRGB_PRIMARIES: [[f64; 2]; 3] = [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]];
pub const P3_PRIMARIES: [[f64; 2]; 3] = [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]];
pub const ADOBE_PRIMARIES: [[f64; 2]; 3] = [[0.64, 0.33], [0.21, 0.71], [0.15, 0.06]];
pub const PROPHOTO_PRIMARIES: [[f64; 2]; 3] = [[0.734699, 0.265301], [0.159597, 0.840403], [0.036598, 0.000105]];

/// Primaries and white point of a known space.
pub fn primaries(space: &SourceColorSpace) -> Option<([[f64; 2]; 3], [f64; 2])> {
    match space {
        SourceColorSpace::Srgb => Some((SRGB_PRIMARIES, D65)),
        SourceColorSpace::DisplayP3 => Some((P3_PRIMARIES, D65)),
        SourceColorSpace::AdobeRgb => Some((ADOBE_PRIMARIES, D65)),
        SourceColorSpace::ProPhoto => Some((PROPHOTO_PRIMARIES, D50)),
        SourceColorSpace::Unknown(_) => None,
    }
}

fn xy_to_xyz(xy: [f64; 2]) -> [f64; 3] {
    [xy[0] / xy[1], 1.0, (1.0 - xy[0] - xy[1]) / xy[1]]
}

pub fn mat_mul(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

pub fn mat_vec(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

pub fn invert(m: &[[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 {
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

/// Linear RGB -> XYZ for `primaries` with white `white` (Y = 1).
pub fn rgb_to_xyz(primaries: [[f64; 2]; 3], white: [f64; 2]) -> [[f64; 3]; 3] {
    let cols = primaries.map(xy_to_xyz);
    let m = [0, 1, 2].map(|i| [cols[0][i], cols[1][i], cols[2][i]]);
    let s = mat_vec(&invert(&m).expect("independent primaries"), xy_to_xyz(white));
    [0, 1, 2].map(|i| [m[i][0] * s[0], m[i][1] * s[1], m[i][2] * s[2]])
}

const BRADFORD: [[f64; 3]; 3] = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];

/// Bradford chromatic adaptation XYZ(from) -> XYZ(to).
pub fn bradford(from: [f64; 2], to: [f64; 2]) -> [[f64; 3]; 3] {
    let s = mat_vec(&BRADFORD, xy_to_xyz(from));
    let d = mat_vec(&BRADFORD, xy_to_xyz(to));
    let scale = [[d[0] / s[0], 0.0, 0.0], [0.0, d[1] / s[1], 0.0], [0.0, 0.0, d[2] / s[2]]];
    mat_mul(&invert(&BRADFORD).expect("invertible"), &mat_mul(&scale, &BRADFORD))
}

/// Linear RGB of a known space -> XYZ D50 (the ICC colorant matrix).
pub fn known_d50(space: &SourceColorSpace) -> Option<[[f64; 3]; 3]> {
    let (p, w) = primaries(space)?;
    let m = rgb_to_xyz(p, w);
    Some(if w == D50 { m } else { mat_mul(&bradford(w, D50), &m) })
}

/// Linear RGB of a known space -> XYZ D65.
pub fn known_d65(space: &SourceColorSpace) -> Option<[[f64; 3]; 3]> {
    let (p, w) = primaries(space)?;
    let m = rgb_to_xyz(p, w);
    Some(if w == D65 { m } else { mat_mul(&bradford(w, D65), &m) })
}

const KNOWN: [SourceColorSpace; 4] =
    [SourceColorSpace::Srgb, SourceColorSpace::DisplayP3, SourceColorSpace::AdobeRgb, SourceColorSpace::ProPhoto];

/// Colorant tolerance for recognising a known space (s15Fixed16 rounding and the
/// slightly different adaptation matrices vendors used).
const COLORANT_TOL: f64 = 0.004;

fn identify(m: &[[f64; 3]; 3]) -> Option<SourceColorSpace> {
    KNOWN.iter().find_map(|s| {
        let k = known_d50(s)?;
        let close = (0..3).all(|i| (0..3).all(|j| (m[i][j] - k[i][j]).abs() <= COLORANT_TOL));
        close.then(|| s.clone())
    })
}

/// Why a profile cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported {
    pub description: Option<String>,
    pub reason: String,
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn s15(b: &[u8], at: usize) -> Option<f64> {
    Some(be32(b, at)? as i32 as f64 / 65536.0)
}

/// Parses ICC bytes. `Err` carries the description (if readable) for the warning detail.
pub fn parse(icc: &[u8]) -> Result<Profile, Unsupported> {
    let fail = |description: Option<String>, reason: &str| Unsupported { description, reason: reason.into() };
    if icc.len() < 132 || icc.get(36..40) != Some(b"acsp") {
        return Err(fail(None, "not an ICC profile"));
    }
    let class = &icc[16..20];
    let count = be32(icc, 128).unwrap_or(0).min(256) as usize;
    let mut tags: Vec<([u8; 4], &[u8])> = Vec::with_capacity(count);
    for i in 0..count {
        let at = 132 + i * 12;
        let (Some(sig), Some(off), Some(len)) = (icc.get(at..at + 4), be32(icc, at + 4), be32(icc, at + 8)) else {
            break;
        };
        let (off, len) = (off as usize, len as usize);
        if let Some(data) = off.checked_add(len).and_then(|end| icc.get(off..end)) {
            tags.push((sig.try_into().unwrap_or_default(), data));
        }
    }
    let tag = |sig: &[u8; 4]| tags.iter().find(|(s, _)| s == sig).map(|(_, d)| *d);
    let description = tag(b"desc").and_then(parse_text);
    let xyz = |sig: &[u8; 4]| -> Option<[f64; 3]> {
        let d = tag(sig)?;
        (d.get(..4)? == b"XYZ ").then_some(())?;
        Some([s15(d, 8)?, s15(d, 12)?, s15(d, 16)?])
    };
    let curve = |sig: &[u8; 4]| tag(sig).and_then(parse_curve);
    match class {
        b"RGB " => {
            let (Some(r), Some(g), Some(b)) = (xyz(b"rXYZ"), xyz(b"gXYZ"), xyz(b"bXYZ")) else {
                return Err(fail(description, "LUT-based RGB profile (no colorants)"));
            };
            let (Some(rt), Some(gt), Some(bt)) = (curve(b"rTRC"), curve(b"gTRC"), curve(b"bTRC")) else {
                return Err(fail(description, "RGB profile without TRCs"));
            };
            let m = [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]];
            if invert(&m).is_none() {
                return Err(fail(description, "degenerate colorants"));
            }
            let space = identify(&m).unwrap_or_else(|| SourceColorSpace::Unknown(description.clone()));
            Ok(Profile { space, to_xyz_d50: Some(m), trc: [rt, gt, bt], gray: false, description })
        }
        b"GRAY" => {
            let Some(k) = curve(b"kTRC") else {
                return Err(fail(description, "gray profile without kTRC"));
            };
            Ok(Profile {
                space: SourceColorSpace::Srgb,
                to_xyz_d50: None,
                trc: [k.clone(), k.clone(), k],
                gray: true,
                description,
            })
        }
        _ => Err(fail(description, "unsupported profile colour space")),
    }
}

fn parse_curve(d: &[u8]) -> Option<Trc> {
    match d.get(..4)? {
        b"curv" => {
            let n = be32(d, 8)? as usize;
            match n {
                0 => Some(Trc::Gamma(1.0)),
                1 => Some(Trc::Gamma(be16(d, 12)? as f64 / 256.0)),
                n if n <= 65536 => {
                    let t: Option<Vec<f64>> = (0..n).map(|i| be16(d, 12 + i * 2).map(|v| v as f64 / 65535.0)).collect();
                    Some(Trc::Table(t?))
                }
                _ => None,
            }
        }
        b"para" => {
            let kind = be16(d, 8)?;
            let nparams = match kind {
                0 => 1,
                1 => 3,
                2 => 4,
                3 => 5,
                4 => 7,
                _ => return None,
            };
            let mut p = [0.0; 7];
            // Defaults so unused parameters are harmless.
            p[1] = 1.0;
            for (i, slot) in p.iter_mut().enumerate().take(nparams) {
                *slot = s15(d, 12 + i * 4)?;
            }
            if kind == 0 {
                return Some(Trc::Gamma(p[0]));
            }
            Some(Trc::Parametric(kind, p))
        }
        _ => None,
    }
}

/// `desc` (v2 textDescriptionType) or `mluc` (v4) text.
fn parse_text(d: &[u8]) -> Option<String> {
    let s = match d.get(..4)? {
        b"desc" => {
            let n = be32(d, 8)? as usize;
            let bytes = d.get(12..12 + n)?;
            String::from_utf8_lossy(bytes).trim_end_matches('\0').to_owned()
        }
        b"mluc" => {
            let records = be32(d, 8)? as usize;
            if records == 0 {
                return None;
            }
            // First record (usually en-US).
            let len = be32(d, 16 + 4)? as usize;
            let off = be32(d, 16 + 8)? as usize;
            let raw = d.get(off..off + len)?;
            let units: Vec<u16> = raw.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
            String::from_utf16_lossy(&units).trim_end_matches('\0').to_owned()
        }
        b"text" => String::from_utf8_lossy(d.get(8..)?).trim_end_matches('\0').to_owned(),
        _ => return None,
    };
    let s = s.trim().to_owned();
    (!s.is_empty()).then_some(s)
}

/// Reassembles the ICC profile from JPEG `APP2 ICC_PROFILE\0` segments (in sequence order).
pub fn from_jpeg(jpeg: &[u8]) -> Option<Vec<u8>> {
    let mut parts: Vec<(u8, &[u8])> = Vec::new();
    for (marker, body) in super::jpeg::segments(jpeg) {
        if marker == 0xE2 && body.len() > 14 && body.starts_with(b"ICC_PROFILE\0") {
            parts.push((body[12], &body[14..]));
        }
    }
    if parts.is_empty() {
        return None;
    }
    parts.sort_by_key(|(seq, _)| *seq);
    Some(parts.into_iter().flat_map(|(_, b)| b.iter().copied()).collect())
}

/// Human-readable detail for an unsupported profile.
pub fn describe(u: &Unsupported) -> String {
    match &u.description {
        Some(d) => format!("{d}: {}", u.reason),
        None => u.reason.clone(),
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Builds small matrix/TRC ICC profiles for tests.

    fn s15(v: f64) -> [u8; 4] {
        ((v * 65536.0).round() as i32).to_be_bytes()
    }

    /// A v2 RGB profile with the given D50 colorant matrix and a single-gamma TRC
    /// (`gamma = None` writes a 1024-entry sRGB table).
    pub fn rgb_profile(desc: &str, m: [[f64; 3]; 3], gamma: Option<f64>) -> Vec<u8> {
        let mut tags: Vec<([u8; 4], Vec<u8>)> = Vec::new();
        let mut d = b"desc\0\0\0\0".to_vec();
        d.extend_from_slice(&((desc.len() + 1) as u32).to_be_bytes());
        d.extend_from_slice(desc.as_bytes());
        d.push(0);
        d.extend_from_slice(&[0u8; 79]);
        tags.push((*b"desc", d));
        let mut cprt = b"text\0\0\0\0".to_vec();
        cprt.extend_from_slice(b"test\0");
        tags.push((*b"cprt", cprt));
        let mut wtpt = b"XYZ \0\0\0\0".to_vec();
        for v in [0.9642, 1.0, 0.8249] {
            wtpt.extend_from_slice(&s15(v));
        }
        tags.push((*b"wtpt", wtpt));
        for (i, sig) in [b"rXYZ", b"gXYZ", b"bXYZ"].into_iter().enumerate() {
            let mut t = b"XYZ \0\0\0\0".to_vec();
            for row in &m {
                t.extend_from_slice(&s15(row[i]));
            }
            tags.push((*sig, t));
        }
        let curve = match gamma {
            Some(g) => {
                let mut c = b"curv\0\0\0\0".to_vec();
                c.extend_from_slice(&1u32.to_be_bytes());
                c.extend_from_slice(&((g * 256.0).round() as u16).to_be_bytes());
                c
            }
            None => {
                let mut c = b"curv\0\0\0\0".to_vec();
                c.extend_from_slice(&1024u32.to_be_bytes());
                for i in 0..1024 {
                    let v = super::Trc::Srgb.eval(i as f64 / 1023.0);
                    c.extend_from_slice(&((v * 65535.0).round() as u16).to_be_bytes());
                }
                c
            }
        };
        for sig in [b"rTRC", b"gTRC", b"bTRC"] {
            tags.push((*sig, curve.clone()));
        }
        let mut out = vec![0u8; 128];
        out[8..12].copy_from_slice(&[0x02, 0x10, 0, 0]);
        out[12..16].copy_from_slice(b"mntr");
        out[24..26].copy_from_slice(&2026u16.to_be_bytes());
        out[26..28].copy_from_slice(&1u16.to_be_bytes());
        out[28..30].copy_from_slice(&1u16.to_be_bytes());
        out[40..44].copy_from_slice(b"APPL");
        for (i, v) in [0.9642, 1.0, 0.8249].into_iter().enumerate() {
            out[68 + i * 4..72 + i * 4].copy_from_slice(&s15(v));
        }
        out[16..20].copy_from_slice(b"RGB ");
        out[20..24].copy_from_slice(b"XYZ ");
        out[36..40].copy_from_slice(b"acsp");
        out.extend_from_slice(&(tags.len() as u32).to_be_bytes());
        let offset = 128 + 4 + tags.len() * 12;
        let mut data = Vec::new();
        for (sig, t) in &tags {
            while !(offset + data.len()).is_multiple_of(4) {
                data.push(0);
            }
            out.extend_from_slice(sig);
            out.extend_from_slice(&((offset + data.len()) as u32).to_be_bytes());
            out.extend_from_slice(&(t.len() as u32).to_be_bytes());
            data.extend_from_slice(t);
        }
        out.extend_from_slice(&data);
        let len = out.len() as u32;
        out[..4].copy_from_slice(&len.to_be_bytes());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::rgb_profile;
    use super::*;

    #[test]
    fn known_spaces_are_identified() {
        for s in KNOWN {
            let m = known_d50(&s).unwrap();
            let p = parse(&rgb_profile("x", m, Some(2.2))).unwrap();
            assert_eq!(p.space, s);
            assert_eq!(p.trc[0], Trc::Gamma((2.2f64 * 256.0).round() / 256.0));
        }
        // sRGB D50 colorants as published in the IEC profile.
        let published = [[0.4361, 0.3851, 0.1431], [0.2225, 0.7169, 0.0606], [0.0139, 0.0971, 0.7141]];
        assert_eq!(parse(&rgb_profile("sRGB IEC61966-2.1", published, None)).unwrap().space, SourceColorSpace::Srgb);
        // Something else: a wide custom matrix.
        let odd = [[0.6, 0.2, 0.16], [0.3, 0.65, 0.05], [0.0, 0.05, 0.77]];
        let p = parse(&rgb_profile("Custom Monitor", odd, Some(2.0))).unwrap();
        assert_eq!(p.space, SourceColorSpace::Unknown(Some("Custom Monitor".into())));
        assert_eq!(p.description.as_deref(), Some("Custom Monitor"));
        assert!(parse(b"junk").is_err());
    }

    #[test]
    fn trc_evaluation() {
        assert!((Trc::Srgb.eval(0.5) - 0.214_041).abs() < 1e-5);
        // para type 3 sRGB.
        let p = Trc::Parametric(3, [2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045, 0.0, 0.0]);
        for i in 0..=20 {
            let x = i as f64 / 20.0;
            assert!((p.eval(x) - Trc::Srgb.eval(x)).abs() < 1e-6, "{x}");
        }
        let t = Trc::Table(vec![0.0, 0.25, 1.0]);
        assert!((t.eval(0.25) - 0.125).abs() < 1e-9);
        let lut = Trc::Srgb.lut(8);
        assert_eq!((lut[0], lut[255]), (0, 65535));
        assert!(lut.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn matrices() {
        // sRGB -> XYZ D65 matches the IEC matrix.
        let m = known_d65(&SourceColorSpace::Srgb).unwrap();
        assert!((m[0][0] - 0.4124).abs() < 1e-3 && (m[1][1] - 0.7152).abs() < 1e-3);
        // White maps to white.
        let w = mat_vec(&known_d65(&SourceColorSpace::ProPhoto).unwrap(), [1.0; 3]);
        let d65 = xy_to_xyz(D65);
        assert!((0..3).all(|i| (w[i] - d65[i]).abs() < 1e-3), "{w:?}");
    }
}
