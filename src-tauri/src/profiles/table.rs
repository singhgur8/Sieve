//! Adobe "big table" strings: the `crs:Table_<MD5>` attribute values in look profiles and
//! sidecars (rust-engine-dev). Reference: Adobe DNG SDK `dng_big_table.cpp`
//! (`dng_big_table::DecodeFromString`, `dng_look_table`, `dng_rgb_table`), verified against
//! the installed Adobe look profiles:
//! - ASCII text in a base-85 alphabet (Z85 with the XML-unsafe `&<>` replaced by `` `'| ``);
//!   each 5 characters encode one little-endian u32, least-significant digit first; a short
//!   final group of `n + 1` characters encodes `n` bytes.
//! - Decoded bytes: u32 LE uncompressed length, then a zlib stream.
//! - Decompressed: a little-endian stream `u32 type` (0 = HSV look table, 1 = RGB table),
//!   `u32 version`, then
//!   - look table: `u32 hue, sat, val divisions`, `hue*sat*val` x (f32 hue shift, sat scale,
//!     val scale) in DNG order, `u32 encoding` (0 linear, 1 sRGB), version >= 2: `f64 min
//!     amount, f64 max amount`;
//!   - RGB table: `u32 dimensions` (1 or 3), `u32 divisions`, samples as u16 offsets from the
//!     identity (`index * 0xFFFF / (divisions - 1)`, rounded), 3D in r-g-b loop order (blue
//!     fastest), then `u32 primaries` (0 sRGB, 1 Adobe RGB, 2 ProPhoto, 3 P3, 4 Rec.2020),
//!     `u32 gamma` (0 linear, 1 sRGB, 2 1.8, 3 2.2, 4 Rec.2020), `u32 gamut` (0 clip,
//!     1 extend), `f64 min amount, f64 max amount`.
//! - The attribute name's MD5 is the MD5 of the decompressed stream; a table whose digest
//!   differs is rejected.
//!
//! Tables are never written by Sieve; sidecar `crs:Table_*` attributes stay byte-for-byte.

use std::io::Read;

use super::dcp::HsvTable;

/// A 3D RGB table (`crs:RGBTable`), sampled in its own encoding.
#[derive(Debug, Clone, PartialEq)]
pub struct RgbTable {
    /// Samples per axis.
    pub divisions: u32,
    /// 1 = a per-channel 1D curve (`divisions` samples), 3 = a 3D table.
    pub dims: u8,
    /// 3D: `divisions^3` RGB samples in 0..=1, index `(r * div + g) * div + b` (blue
    /// fastest). 1D: `divisions` samples.
    pub samples: Vec<[f32; 3]>,
    /// Colour space / transfer the table is defined in (SDK `dng_rgb_table::primaries_*`,
    /// `gamma_*`), mapped by the pipeline.
    pub primaries: u32,
    pub gamma: u32,
    /// 0 = clip to the table's gamut, 1 = extend.
    pub gamut: u32,
    /// Blend range for `LookSettings.amount` (SDK `fMinAmount` / `fMaxAmount`).
    pub min_amount: f32,
    pub max_amount: f32,
}

/// A decoded table value.
#[derive(Debug, Clone, PartialEq)]
pub enum BigTable {
    Look(HsvTable),
    Rgb(RgbTable),
}

const ALPHABET: &[u8; 85] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-:+=^!/*?`'|()[]{}@%$#";

/// Largest decompressed table accepted (a 65^3 RGB table is ~1.6 MB).
const MAX_TABLE_BYTES: usize = 32 << 20;

fn base85_decode(value: &str) -> Result<Vec<u8>, String> {
    let mut index = [255u8; 256];
    for (i, &c) in ALPHABET.iter().enumerate() {
        index[c as usize] = i as u8;
    }
    let digits: Vec<u8> = value.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    let mut out = Vec::with_capacity(digits.len() / 5 * 4 + 4);
    for group in digits.chunks(5) {
        let mut v: u64 = 0;
        for &c in group.iter().rev() {
            let d = index[c as usize];
            if d == 255 {
                return Err(format!("invalid table character {:?}", c as char));
            }
            v = v * 85 + u64::from(d);
        }
        let n = if group.len() == 5 { 4 } else { group.len().saturating_sub(1) };
        if v > u64::from(u32::MAX) {
            return Err("table value overflows".into());
        }
        out.extend_from_slice(&(v as u32).to_le_bytes()[..n]);
    }
    Ok(out)
}

/// Decodes a `crs:Table_<md5>` value; `md5` (32 hex digits from the attribute name) is
/// checked against the table's fingerprint.
pub fn decode(value: &str, md5: &str) -> Result<BigTable, String> {
    let bytes = decode_bytes(value)?;
    let digest = hex(&md5_digest(&bytes));
    if !digest.eq_ignore_ascii_case(md5.trim()) {
        return Err(format!("table digest {digest} does not match {md5}"));
    }
    parse(&bytes)
}

/// Base-85 + zlib layer only (no digest check).
pub fn decode_bytes(value: &str) -> Result<Vec<u8>, String> {
    let raw = base85_decode(value)?;
    if raw.len() < 6 {
        return Err("table too short".into());
    }
    let len = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize;
    if len > MAX_TABLE_BYTES {
        return Err(format!("table of {len} bytes is too large"));
    }
    let mut out = Vec::with_capacity(len);
    flate2::read::ZlibDecoder::new(&raw[4..])
        .take(MAX_TABLE_BYTES as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| format!("table zlib: {e}"))?;
    if out.len() != len {
        return Err(format!("table length {} != declared {len}", out.len()));
    }
    Ok(out)
}

struct Stream<'a> {
    b: &'a [u8],
    at: usize,
}

impl Stream<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let s = self.b.get(self.at..self.at + n).ok_or("table truncated")?;
        self.at += n;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32, String> {
        let s = self.take(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn u16(&mut self) -> Result<u16, String> {
        let s = self.take(2)?;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }
    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn f64(&mut self) -> Result<f64, String> {
        let s = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        Ok(f64::from_le_bytes(a))
    }
    fn at_end(&self) -> bool {
        self.at >= self.b.len()
    }
}

/// Parses a decompressed table stream.
pub fn parse(bytes: &[u8]) -> Result<BigTable, String> {
    let mut s = Stream { b: bytes, at: 0 };
    let typ = s.u32()?;
    let version = s.u32()?;
    match typ {
        0 => {
            let (h, sd, v) = (s.u32()?, s.u32()?, s.u32()?);
            let n = u64::from(h) * u64::from(sd) * u64::from(v);
            if h < 1 || sd < 2 || v < 1 || n * 12 > bytes.len() as u64 {
                return Err(format!("look table dimensions {h}x{sd}x{v} do not fit"));
            }
            let mut data = Vec::with_capacity(n as usize);
            for _ in 0..n {
                data.push([s.f32()?, s.f32()?, s.f32()?]);
            }
            let encoding = if s.at_end() { 0 } else { s.u32()? };
            if version >= 2 && !s.at_end() {
                let _min = s.f64()?;
                let _max = s.f64()?;
            }
            Ok(BigTable::Look(HsvTable {
                hue_divisions: h,
                sat_divisions: sd,
                val_divisions: v,
                data,
                srgb_gamma: encoding == 1,
            }))
        }
        1 => {
            let dims = s.u32()?;
            let div = s.u32()?;
            if !(2..=256).contains(&div) || !(dims == 1 || dims == 3) {
                return Err(format!("RGB table dims {dims} / divisions {div} unsupported"));
            }
            let d = div as usize;
            let count = if dims == 3 { d * d * d } else { d };
            if count * 6 > bytes.len() {
                return Err("RGB table truncated".into());
            }
            let ident = |i: usize| ((i * 0xFFFF + (d - 1) / 2) / (d - 1)) as u16;
            let mut samples = Vec::with_capacity(count);
            let mut read = |o: [u16; 3]| -> Result<(), String> {
                let mut v = [0.0f32; 3];
                for (k, off) in o.iter().enumerate() {
                    v[k] = f32::from(s.u16()?.wrapping_add(*off)) / 65535.0;
                }
                samples.push(v);
                Ok(())
            };
            if dims == 3 {
                for r in 0..d {
                    for g in 0..d {
                        for b in 0..d {
                            read([ident(r), ident(g), ident(b)])?;
                        }
                    }
                }
            } else {
                for i in 0..d {
                    read([ident(i); 3])?;
                }
            }
            let primaries = s.u32()?;
            let gamma = s.u32()?;
            let gamut = s.u32()?;
            let (min_amount, max_amount) = if s.at_end() { (0.0, 1.0) } else { (s.f64()? as f32, s.f64()? as f32) };
            Ok(BigTable::Rgb(RgbTable {
                divisions: div,
                dims: dims as u8,
                samples,
                primaries,
                gamma,
                gamut,
                min_amount,
                max_amount,
            }))
        }
        _ => Err(format!("unknown table type {typ} (version {version})")),
    }
}

impl RgbTable {
    /// Trilinear lookup of encoded `rgb` (0..=1, clamped).
    pub fn eval(&self, rgb: [f32; 3]) -> [f32; 3] {
        let d = self.divisions as usize;
        let n = (d - 1) as f32;
        if self.dims == 1 {
            let mut out = [0.0; 3];
            for k in 0..3 {
                let t = rgb[k].clamp(0.0, 1.0) * n;
                let i = (t as usize).min(d - 2);
                let f = t - i as f32;
                out[k] = self.samples[i][k] * (1.0 - f) + self.samples[i + 1][k] * f;
            }
            return out;
        }
        let pos = rgb.map(|c| c.clamp(0.0, 1.0) * n);
        let i = pos.map(|p| (p as usize).min(d - 2));
        let f = [pos[0] - i[0] as f32, pos[1] - i[1] as f32, pos[2] - i[2] as f32];
        let at = |r: usize, g: usize, b: usize| &self.samples[(r * d + g) * d + b];
        let mut out = [0.0f32; 3];
        for (dr, wr) in [(0, 1.0 - f[0]), (1, f[0])] {
            for (dg, wg) in [(0, 1.0 - f[1]), (1, f[1])] {
                for (db, wb) in [(0, 1.0 - f[2]), (1, f[2])] {
                    let w = wr * wg * wb;
                    if w == 0.0 {
                        continue;
                    }
                    let s = at(i[0] + dr, i[1] + dg, i[2] + db);
                    out[0] += s[0] * w;
                    out[1] += s[1] * w;
                    out[2] += s[2] * w;
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// MD5 (RFC 1321), for table fingerprints.
// ---------------------------------------------------------------------------

pub fn md5_digest(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14,
        20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6,
        10, 15, 21,
    ];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4_294_967_296.0) as u32).collect();
    let (mut a0, mut b0, mut c0, mut d0) = (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());
    for chunk in msg.as_chunks::<64>().0 {
        let m: Vec<u32> = chunk.as_chunks::<4>().0.iter().map(|w| u32::from_le_bytes(*w)).collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base85_encode(bytes: &[u8]) -> String {
        let mut out = String::new();
        for chunk in bytes.chunks(4) {
            let mut w = [0u8; 4];
            w[..chunk.len()].copy_from_slice(chunk);
            let mut v = u32::from_le_bytes(w) as u64;
            let n = if chunk.len() == 4 { 5 } else { chunk.len() + 1 };
            for _ in 0..n {
                out.push(ALPHABET[(v % 85) as usize] as char);
                v /= 85;
            }
        }
        out
    }

    fn encode(stream: &[u8]) -> (String, String) {
        use std::io::Write;
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(stream).unwrap();
        let mut raw = (stream.len() as u32).to_le_bytes().to_vec();
        raw.extend(z.finish().unwrap());
        (base85_encode(&raw), hex(&md5_digest(stream)))
    }

    #[test]
    fn md5_known_vectors() {
        assert_eq!(hex(&md5_digest(b"")), "D41D8CD98F00B204E9800998ECF8427E");
        assert_eq!(hex(&md5_digest(b"abc")), "900150983CD24FB0D6963F7D28E17F72");
        let long = b"12345678901234567890123456789012345678901234567890123456789012345678901234567890";
        assert_eq!(hex(&md5_digest(long)), "57EDF4A22BE3C955AC49DA2E2107B67A");
    }

    #[test]
    fn decodes_synthetic_look_and_rgb_tables() {
        // Look table 2x2x1, version 1.
        let mut st = Vec::new();
        for v in [0u32, 1, 2, 2, 1] {
            st.extend_from_slice(&v.to_le_bytes());
        }
        for e in [[5.0f32, 1.1, 0.9], [0.0, 1.0, 1.0], [-5.0, 0.8, 1.2], [1.0, 1.0, 1.0]] {
            for x in e {
                st.extend_from_slice(&x.to_le_bytes());
            }
        }
        st.extend_from_slice(&1u32.to_le_bytes());
        let (value, md5) = encode(&st);
        match decode(&value, &md5).unwrap() {
            BigTable::Look(t) => {
                assert_eq!((t.hue_divisions, t.sat_divisions, t.val_divisions), (2, 2, 1));
                assert_eq!(t.data[2], [-5.0, 0.8, 1.2]);
                assert!(t.srgb_gamma);
            }
            other => panic!("{other:?}"),
        }
        // Wrong digest is rejected; corrupt characters too.
        assert!(decode(&value, "00000000000000000000000000000000").is_err());
        assert!(decode("~~~~~", &md5).is_err());

        // Identity RGB table 2^3 (all offsets 0) with a tweak on the last sample.
        let mut st = Vec::new();
        for v in [1u32, 1, 3, 2] {
            st.extend_from_slice(&v.to_le_bytes());
        }
        for i in 0..8 {
            // Last sample (1, 1, 1): green offset 0x8000 wraps to 0x7FFF (~0.5).
            let d: [u16; 3] = if i == 7 { [0, 0x8000, 0] } else { [0, 0, 0] };
            for x in d {
                st.extend_from_slice(&x.to_le_bytes());
            }
        }
        for v in [2u32, 1, 0] {
            st.extend_from_slice(&v.to_le_bytes());
        }
        st.extend_from_slice(&0.0f64.to_le_bytes());
        st.extend_from_slice(&2.0f64.to_le_bytes());
        let (value, md5) = encode(&st);
        match decode(&value, &md5).unwrap() {
            BigTable::Rgb(t) => {
                assert_eq!((t.dims, t.divisions, t.primaries, t.gamma, t.gamut), (3, 2, 2, 1, 0));
                assert_eq!(t.max_amount, 2.0);
                assert_eq!(t.samples[0], [0.0, 0.0, 0.0]);
                assert_eq!(t.samples[1], [0.0, 0.0, 1.0], "blue fastest");
                let mid = t.eval([0.5, 0.5, 0.5]);
                assert!((mid[0] - 0.5).abs() < 1e-6 && mid[1] < 0.5, "{mid:?}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    #[ignore = "needs Adobe Camera Raw look profiles installed"]
    fn decodes_installed_adobe_color_table() {
        let path = "/Library/Application Support/Adobe/CameraRaw/Settings/Adobe/Profiles/Adobe Raw/Adobe Color.xmp";
        let text = std::fs::read_to_string(path).unwrap();
        let md5 = "E1095149FDB39D7A057BAB208837E2E1";
        let key = format!("crs:Table_{md5}=\"");
        let start = text.find(&key).unwrap() + key.len();
        let end = start + text[start..].find('"').unwrap();
        match decode(&text[start..end], md5).unwrap() {
            BigTable::Look(t) => {
                assert_eq!((t.hue_divisions, t.sat_divisions, t.val_divisions), (36, 16, 16));
                assert_eq!(t.data.len(), 36 * 16 * 16);
            }
            other => panic!("{other:?}"),
        }
        let path = "/Library/Application Support/Adobe/CameraRaw/Settings/Adobe/Profiles/Artistic/Artistic 01.xmp";
        let text = std::fs::read_to_string(path).unwrap();
        let md5 = "BD510A12D9BF555B0328CD473B1392E0";
        let key = format!("crs:Table_{md5}=\"");
        let start = text.find(&key).unwrap() + key.len();
        let end = start + text[start..].find('"').unwrap();
        match decode(&text[start..end], md5).unwrap() {
            BigTable::Rgb(t) => {
                assert_eq!((t.dims, t.divisions), (3, 32));
                assert_eq!(t.samples.len(), 32 * 32 * 32);
            }
            other => panic!("{other:?}"),
        }
    }
}
