//! DNG Camera Profile (`.dcp`) parsing (rust-engine-dev). A DCP is a TIFF-like file with
//! magic `IIRC` (little-endian, 0x4352 instead of 42) or `MMCR`, whose IFD0 carries the DNG
//! profile tags (DNG spec 1.6, chapter 6 "Camera Profiles"):
//! `UniqueCameraModel` 0xC614, `ProfileName` 0xC6F8, `CalibrationIlluminant1/2` 0xC65A/0xC65B,
//! `ColorMatrix1/2` 0xC621/0xC622, `ForwardMatrix1/2` 0xC714/0xC715,
//! `ProfileHueSatMapDims` 0xC6F9, `ProfileHueSatMapData1/2` 0xC6FA/0xC6FB,
//! `ProfileHueSatMapEncoding` 0xC7A3, `ProfileLookTableDims` 0xC725,
//! `ProfileLookTableData` 0xC726, `ProfileLookTableEncoding` 0xC7A4,
//! `ProfileToneCurve` 0xC6FC, `BaselineExposureOffset` 0xC7A5, `DefaultBlackRender` 0xC7A6,
//! `ProfileEmbedPolicy` 0xC6FD, `ProfileCalibrationSignature` 0xC6F4.
//! A DNG file's IFD0 carries the same tags (with the TIFF magic 42), so [`Dcp::parse`]
//! accepts both. Parsed matrices are row-major 3x3 (3-colour cameras).
//!
//! The colour maths the pipeline needs lives here too: illuminant interpolation
//! ([`Dcp::illuminant_weight`]), the camera -> XYZ (D50) matrix of a white balance
//! ([`Dcp::camera_to_pcs`], DNG spec "Mapping Camera Color Space to CIE XYZ Space"), the
//! camera neutral <-> white xy conversions and the HSV table evaluation ([`HsvTable::apply`],
//! DNG SDK `RefBaselineHueSatMap`).

/// One HSV table: `ProfileHueSatMap*` or `ProfileLookTable` (DNG spec "ProfileHueSatMapData").
#[derive(Debug, Clone, PartialEq)]
pub struct HsvTable {
    pub hue_divisions: u32,
    pub sat_divisions: u32,
    /// 1 = 2.5D table (no value dimension).
    pub val_divisions: u32,
    /// `hue_div * sat_div * val_div` entries of (hue shift degrees, sat scale, val scale),
    /// in DNG order (value outermost, then hue, then saturation).
    pub data: Vec<[f32; 3]>,
    /// `*Encoding` tag: 0 = linear, 1 = sRGB gamma for the value axis.
    pub srgb_gamma: bool,
}

/// A parsed DCP. Illuminants use EXIF LightSource codes (17 = Standard A, 21 = D65, ...).
#[derive(Debug, Clone, PartialEq)]
pub struct Dcp {
    pub unique_camera_model: String,
    pub profile_name: String,
    pub calibration_illuminant1: u16,
    pub calibration_illuminant2: Option<u16>,
    pub color_matrix1: [[f32; 3]; 3],
    pub color_matrix2: Option<[[f32; 3]; 3]>,
    pub forward_matrix1: Option<[[f32; 3]; 3]>,
    pub forward_matrix2: Option<[[f32; 3]; 3]>,
    pub hue_sat_map1: Option<HsvTable>,
    pub hue_sat_map2: Option<HsvTable>,
    pub look_table: Option<HsvTable>,
    /// `ProfileToneCurve` points (x, y in 0..=1); `None` = Adobe's default ACR3 curve.
    pub tone_curve: Option<Vec<[f32; 2]>>,
    pub baseline_exposure_offset: f32,
    /// `DefaultBlackRender`: 0 = auto, 1 = none.
    pub default_black_render: u32,
}

// ---------------------------------------------------------------------------
// TIFF reading
// ---------------------------------------------------------------------------

const TAG_UNIQUE_CAMERA_MODEL: u16 = 0xC614;
const TAG_COLOR_MATRIX1: u16 = 0xC621;
const TAG_COLOR_MATRIX2: u16 = 0xC622;
const TAG_CALIBRATION_ILLUMINANT1: u16 = 0xC65A;
const TAG_CALIBRATION_ILLUMINANT2: u16 = 0xC65B;
const TAG_PROFILE_NAME: u16 = 0xC6F8;
const TAG_HSM_DIMS: u16 = 0xC6F9;
const TAG_HSM_DATA1: u16 = 0xC6FA;
const TAG_HSM_DATA2: u16 = 0xC6FB;
const TAG_TONE_CURVE: u16 = 0xC6FC;
const TAG_FORWARD_MATRIX1: u16 = 0xC714;
const TAG_FORWARD_MATRIX2: u16 = 0xC715;
const TAG_LOOK_DIMS: u16 = 0xC725;
const TAG_LOOK_DATA: u16 = 0xC726;
const TAG_HSM_ENCODING: u16 = 0xC7A3;
const TAG_LOOK_ENCODING: u16 = 0xC7A4;
const TAG_BASELINE_EXPOSURE_OFFSET: u16 = 0xC7A5;
const TAG_DEFAULT_BLACK_RENDER: u16 = 0xC7A6;

/// Upper bound on any table (entries), so malformed files cannot allocate unboundedly.
const MAX_TABLE_ENTRIES: u64 = 1 << 22;

struct Reader<'a> {
    b: &'a [u8],
    le: bool,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    tag: u16,
    typ: u16,
    count: u32,
    /// Absolute offset of the value bytes (inline values point into the entry).
    at: usize,
}

fn type_size(typ: u16) -> Option<usize> {
    Some(match typ {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        _ => return None,
    })
}

impl<'a> Reader<'a> {
    fn u16(&self, at: usize) -> Result<u16, String> {
        let s = self.b.get(at..at + 2).ok_or("truncated")?;
        Ok(if self.le { u16::from_le_bytes([s[0], s[1]]) } else { u16::from_be_bytes([s[0], s[1]]) })
    }

    fn u32(&self, at: usize) -> Result<u32, String> {
        let s = self.b.get(at..at + 4).ok_or("truncated")?;
        let a = [s[0], s[1], s[2], s[3]];
        Ok(if self.le { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) })
    }

    fn u64(&self, at: usize) -> Result<u64, String> {
        let s = self.b.get(at..at + 8).ok_or("truncated")?;
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        Ok(if self.le { u64::from_le_bytes(a) } else { u64::from_be_bytes(a) })
    }

    /// Header -> IFD0 entries.
    fn ifd0(bytes: &'a [u8]) -> Result<(Reader<'a>, Vec<Entry>), String> {
        let head = bytes.get(0..8).ok_or("not a DCP/TIFF: too short")?;
        let le = match &head[0..2] {
            b"II" => true,
            b"MM" => false,
            _ => return Err("not a DCP/TIFF: bad byte order".into()),
        };
        let r = Reader { b: bytes, le };
        let magic = r.u16(2)?;
        // DCP: "IIRC" (0x4352 read little-endian) / "MMCR"; DNG/TIFF: 42.
        if magic != 0x4352 && magic != 42 {
            return Err(format!("not a DCP/TIFF: magic {magic:#x}"));
        }
        let off = r.u32(4)? as usize;
        let n = r.u16(off)? as usize;
        if n > 4096 {
            return Err(format!("implausible IFD entry count {n}"));
        }
        let mut entries = Vec::with_capacity(n);
        for i in 0..n {
            let e = off + 2 + i * 12;
            let tag = r.u16(e)?;
            let typ = r.u16(e + 2)?;
            let count = r.u32(e + 4)?;
            let Some(sz) = type_size(typ) else { continue };
            let total = sz as u64 * u64::from(count);
            let at = if total <= 4 { e + 8 } else { r.u32(e + 8)? as usize };
            if (at as u64).saturating_add(total) > bytes.len() as u64 {
                return Err(format!("tag {tag:#x} points outside the file"));
            }
            entries.push(Entry { tag, typ, count, at });
        }
        Ok((r, entries))
    }

    /// Value `i` of an entry as f64 (numeric types only).
    fn num(&self, e: &Entry, i: usize) -> Result<f64, String> {
        let sz = type_size(e.typ).ok_or("bad type")?;
        let at = e.at + i * sz;
        Ok(match e.typ {
            1 | 7 => f64::from(*self.b.get(at).ok_or("truncated")?),
            6 => f64::from(*self.b.get(at).ok_or("truncated")? as i8),
            3 => f64::from(self.u16(at)?),
            8 => f64::from(self.u16(at)? as i16),
            4 => f64::from(self.u32(at)?),
            9 => f64::from(self.u32(at)? as i32),
            5 => {
                let (n, d) = (self.u32(at)?, self.u32(at + 4)?);
                if d == 0 {
                    0.0
                } else {
                    f64::from(n) / f64::from(d)
                }
            }
            10 => {
                let (n, d) = (self.u32(at)? as i32, self.u32(at + 4)? as i32);
                if d == 0 {
                    0.0
                } else {
                    f64::from(n) / f64::from(d)
                }
            }
            11 => f64::from(f32::from_bits(self.u32(at)?)),
            12 => f64::from_bits(self.u64(at)?),
            _ => return Err("bad type".into()),
        })
    }

    fn nums(&self, e: &Entry) -> Result<Vec<f64>, String> {
        if u64::from(e.count) > MAX_TABLE_ENTRIES * 3 {
            return Err(format!("tag {:#x}: too many values", e.tag));
        }
        (0..e.count as usize).map(|i| self.num(e, i)).collect()
    }

    fn string(&self, e: &Entry) -> String {
        let bytes = &self.b[e.at..e.at + e.count as usize];
        let end = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
        String::from_utf8_lossy(&bytes[..end]).trim().to_owned()
    }
}

fn find(entries: &[Entry], tag: u16) -> Option<&Entry> {
    entries.iter().find(|e| e.tag == tag)
}

fn matrix(r: &Reader, entries: &[Entry], tag: u16) -> Result<Option<[[f32; 3]; 3]>, String> {
    let Some(e) = find(entries, tag) else { return Ok(None) };
    let v = r.nums(e)?;
    if v.len() != 9 {
        return Err(format!("tag {tag:#x}: {} values (3-colour profiles only)", v.len()));
    }
    Ok(Some([
        [v[0] as f32, v[1] as f32, v[2] as f32],
        [v[3] as f32, v[4] as f32, v[5] as f32],
        [v[6] as f32, v[7] as f32, v[8] as f32],
    ]))
}

fn hsv_table(
    r: &Reader,
    entries: &[Entry],
    dims_tag: u16,
    data_tag: u16,
    enc_tag: u16,
) -> Result<Option<HsvTable>, String> {
    let (Some(d), Some(e)) = (find(entries, dims_tag), find(entries, data_tag)) else { return Ok(None) };
    let dims = r.nums(d)?;
    if dims.len() != 3 {
        return Err(format!("tag {dims_tag:#x}: expected 3 dimensions"));
    }
    let (h, s, v) = (dims[0] as u32, dims[1] as u32, dims[2].max(1.0) as u32);
    let n = u64::from(h) * u64::from(s) * u64::from(v);
    if h < 1 || s < 2 || n > MAX_TABLE_ENTRIES || u64::from(e.count) != n * 3 {
        return Err(format!("tag {data_tag:#x}: dimensions {h}x{s}x{v} do not match {} values", e.count));
    }
    let vals = r.nums(e)?;
    let data = vals.as_chunks::<3>().0.iter().map(|c| [c[0] as f32, c[1] as f32, c[2] as f32]).collect();
    let srgb_gamma = match find(entries, enc_tag) {
        Some(x) => r.num(x, 0)? as u32 == 1,
        None => false,
    };
    Ok(Some(HsvTable { hue_divisions: h, sat_divisions: s, val_divisions: v, data, srgb_gamma }))
}

impl Dcp {
    /// Parses a DCP file's bytes (a DNG's IFD0 works too).
    pub fn parse(bytes: &[u8]) -> Result<Dcp, String> {
        let (r, entries) = Reader::ifd0(bytes)?;
        let unique_camera_model = find(&entries, TAG_UNIQUE_CAMERA_MODEL).map(|e| r.string(e)).unwrap_or_default();
        let profile_name = find(&entries, TAG_PROFILE_NAME).map(|e| r.string(e)).unwrap_or_default();
        let color_matrix1 = matrix(&r, &entries, TAG_COLOR_MATRIX1)?.ok_or("no ColorMatrix1")?;
        let color_matrix2 = matrix(&r, &entries, TAG_COLOR_MATRIX2)?;
        let ill = |tag| -> Result<Option<u16>, String> {
            find(&entries, tag).map(|e| r.num(e, 0).map(|v| v as u16)).transpose()
        };
        let calibration_illuminant1 = ill(TAG_CALIBRATION_ILLUMINANT1)?.unwrap_or(21);
        let calibration_illuminant2 = if color_matrix2.is_some() { ill(TAG_CALIBRATION_ILLUMINANT2)? } else { None };
        let forward_matrix1 = matrix(&r, &entries, TAG_FORWARD_MATRIX1)?;
        let forward_matrix2 = matrix(&r, &entries, TAG_FORWARD_MATRIX2)?;
        let hue_sat_map1 = hsv_table(&r, &entries, TAG_HSM_DIMS, TAG_HSM_DATA1, TAG_HSM_ENCODING)?;
        let hue_sat_map2 = hsv_table(&r, &entries, TAG_HSM_DIMS, TAG_HSM_DATA2, TAG_HSM_ENCODING)?;
        let look_table = hsv_table(&r, &entries, TAG_LOOK_DIMS, TAG_LOOK_DATA, TAG_LOOK_ENCODING)?;
        let tone_curve = match find(&entries, TAG_TONE_CURVE) {
            Some(e) => {
                let v = r.nums(e)?;
                if v.len() < 4 || v.len() % 2 != 0 {
                    return Err("ProfileToneCurve: odd or too few values".into());
                }
                Some(v.as_chunks::<2>().0.iter().map(|p| [p[0] as f32, p[1] as f32]).collect())
            }
            None => None,
        };
        let baseline_exposure_offset = match find(&entries, TAG_BASELINE_EXPOSURE_OFFSET) {
            Some(e) => r.num(e, 0)? as f32,
            None => 0.0,
        };
        let default_black_render = match find(&entries, TAG_DEFAULT_BLACK_RENDER) {
            Some(e) => r.num(e, 0)? as u32,
            None => 0,
        };
        Ok(Dcp {
            unique_camera_model,
            profile_name,
            calibration_illuminant1,
            calibration_illuminant2,
            color_matrix1,
            color_matrix2,
            forward_matrix1,
            forward_matrix2,
            hue_sat_map1,
            hue_sat_map2,
            look_table,
            tone_curve,
            baseline_exposure_offset,
            default_black_render,
        })
    }

    /// Reads only `UniqueCameraModel` and `ProfileName` (library index scan).
    pub fn peek_names(bytes: &[u8]) -> Result<(String, String), String> {
        let (r, entries) = Reader::ifd0(bytes)?;
        let model = find(&entries, TAG_UNIQUE_CAMERA_MODEL).map(|e| r.string(e)).ok_or("no UniqueCameraModel")?;
        let name = find(&entries, TAG_PROFILE_NAME).map(|e| r.string(e)).unwrap_or_default();
        Ok((model, name))
    }

    /// Weight of illuminant 1 (0..=1) for a white point of correlated colour temperature
    /// `temperature_k` (inverse-mired interpolation between the two calibration illuminants).
    pub fn illuminant_weight(&self, temperature_k: f32) -> f32 {
        let Some(i2) = self.calibration_illuminant2 else { return 1.0 };
        let (t1, t2) = (illuminant_temperature(self.calibration_illuminant1), illuminant_temperature(i2));
        if t1 <= 0.0 || t2 <= 0.0 || (t1 - t2).abs() < 1e-3 {
            return 1.0;
        }
        let t = f64::from(temperature_k);
        // Weight of the *lower* temperature illuminant, then mapped back to illuminant 1.
        let (lo, hi, one_is_lo) = if t1 < t2 { (t1, t2, true) } else { (t2, t1, false) };
        let g_lo = if t <= lo {
            1.0
        } else if t >= hi {
            0.0
        } else {
            (1.0 / t - 1.0 / hi) / (1.0 / lo - 1.0 / hi)
        };
        (if one_is_lo { g_lo } else { 1.0 - g_lo }) as f32
    }

    /// `ColorMatrix` (XYZ -> camera) interpolated with illuminant-1 weight `g`.
    pub fn color_matrix(&self, g: f32) -> [[f64; 3]; 3] {
        lerp_matrix(&self.color_matrix1, self.color_matrix2.as_ref(), g)
    }

    /// `ForwardMatrix` (white-balanced camera -> XYZ D50) interpolated with weight `g`.
    pub fn forward_matrix(&self, g: f32) -> Option<[[f64; 3]; 3]> {
        let fm1 = self.forward_matrix1.as_ref()?;
        Some(lerp_matrix(fm1, self.forward_matrix2.as_ref(), g))
    }

    /// `ProfileHueSatMap` interpolated with weight `g` (tables must share dimensions).
    pub fn hue_sat_map(&self, g: f32) -> Option<HsvTable> {
        match (&self.hue_sat_map1, &self.hue_sat_map2) {
            (Some(a), Some(b)) if a.data.len() == b.data.len() && g < 1.0 => {
                if g <= 0.0 {
                    return Some(b.clone());
                }
                let data = a
                    .data
                    .iter()
                    .zip(&b.data)
                    .map(|(p, q)| {
                        [p[0] * g + q[0] * (1.0 - g), p[1] * g + q[1] * (1.0 - g), p[2] * g + q[2] * (1.0 - g)]
                    })
                    .collect();
                Some(HsvTable { data, ..a.clone() })
            }
            (Some(a), _) => Some(a.clone()),
            (None, Some(b)) => Some(b.clone()),
            (None, None) => None,
        }
    }

    /// White xy of a camera neutral (camera RGB of a neutral, e.g. `1 / multipliers`), found
    /// by iterating the illuminant interpolation (DNG SDK `dng_color_spec::NeutralToXY`).
    ///
    /// `cc`: the raw file's per-unit `CameraCalibration` diagonal (reference camera ->
    /// this camera; `[1; 3]` if none): the DNG model's XYZ -> camera is `CC * CM`.
    pub fn neutral_to_xy(&self, neutral: [f64; 3], cc: [f64; 3]) -> (f64, f64) {
        let mut xy = D50_XY;
        for _ in 0..30 {
            let (t, _) = crate::develop::wb::temp_tint_for(xy.0, xy.1);
            let g = self.illuminant_weight(t as f32);
            let Some(inv) = invert3(&self.calibrated_matrix(g, cc)) else { return D50_XY };
            let xyz = mul_mv(&inv, neutral);
            let sum = xyz[0] + xyz[1] + xyz[2];
            if !(sum.is_finite() && sum.abs() > 1e-12) {
                return D50_XY;
            }
            let next = (xyz[0] / sum, xyz[1] / sum);
            if (next.0 - xy.0).abs() + (next.1 - xy.1).abs() < 1e-7 {
                return next;
            }
            xy = next;
        }
        xy
    }

    /// Camera neutral (max component 1) for a white point xy.
    pub fn xy_to_neutral(&self, xy: (f64, f64), cc: [f64; 3]) -> [f64; 3] {
        let (t, _) = crate::develop::wb::temp_tint_for(xy.0, xy.1);
        let g = self.illuminant_weight(t as f32);
        let n = mul_mv(&self.calibrated_matrix(g, cc), xy_to_xyz(xy));
        let m = n[0].max(n[1]).max(n[2]);
        if m > 1e-12 && n.iter().all(|v| v.is_finite() && *v > 0.0) {
            n.map(|v| v / m)
        } else {
            [1.0; 3]
        }
    }

    /// Camera RGB -> XYZ (D50) for white point `xy` whose camera neutral is `neutral`
    /// (max 1), per the DNG spec: with forward matrices `FM * diag(1 / neutral)`, else
    /// `Bradford(white -> D50) * inverse(CM)` normalized so the neutral maps to D50 with
    /// Y = 1. Also returns the illuminant-1 weight used.
    /// A diagonal `cc` cancels out of the forward-matrix path
    /// (`FM * diag(1 / inv(CC) n) * inv(CC) = FM * diag(1 / n)`).
    pub fn camera_to_pcs(&self, xy: (f64, f64), neutral: [f64; 3], cc: [f64; 3]) -> ([[f64; 3]; 3], f32) {
        let (t, _) = crate::develop::wb::temp_tint_for(xy.0, xy.1);
        let g = self.illuminant_weight(t as f32);
        if let Some(fm) = self.forward_matrix(g) {
            let fm = normalize_forward(fm);
            let mut m = fm;
            for row in m.iter_mut() {
                for (j, v) in row.iter_mut().enumerate() {
                    *v /= neutral[j].max(1e-9);
                }
            }
            return (m, g);
        }
        (color_matrix_to_pcs(&self.calibrated_matrix(g, cc), xy), g)
    }

    /// `diag(cc) * ColorMatrix(g)`.
    pub fn calibrated_matrix(&self, g: f32, cc: [f64; 3]) -> [[f64; 3]; 3] {
        let mut m = self.color_matrix(g);
        for (row, c) in m.iter_mut().zip(cc) {
            row.iter_mut().for_each(|v| *v *= c);
        }
        m
    }
}

/// `inverse(cm)` adapted from the white `xy` to D50 (Bradford), scaled so the camera neutral
/// of `xy` (normalized to max 1) maps to D50 white (Y = 1). Camera values are *not* white
/// balanced.
pub fn color_matrix_to_pcs(cm: &[[f64; 3]; 3], xy: (f64, f64)) -> [[f64; 3]; 3] {
    let Some(inv) = invert3(cm) else { return IDENTITY };
    let ca = bradford(xy_to_xyz(xy), xy_to_xyz(D50_XY));
    let m = mul_mm(&ca, &inv);
    // Scale: the camera neutral normalized to max 1 (the saturation of the brightest channel
    // on a white) -> Y = 1, as with forward matrices.
    let neutral = mul_mv(cm, xy_to_xyz(xy));
    let mx = neutral[0].max(neutral[1]).max(neutral[2]);
    let neutral = if mx > 1e-12 { neutral.map(|v| v / mx) } else { neutral };
    let y = mul_mv(&m, neutral)[1];
    if y.is_finite() && y.abs() > 1e-12 {
        m.map(|r| r.map(|v| v / y))
    } else {
        m
    }
}

/// Forward matrices map a white-balanced camera white to D50; rescale rows so they do
/// exactly (DNG SDK `NormalizeForwardMatrix`).
fn normalize_forward(fm: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let white = mul_mv(&fm, [1.0; 3]);
    let d50 = xy_to_xyz(D50_XY);
    let mut out = fm;
    for (i, row) in out.iter_mut().enumerate() {
        if white[i].abs() > 1e-9 {
            let s = d50[i] / white[i];
            row.iter_mut().for_each(|v| *v *= s);
        }
    }
    out
}

fn lerp_matrix(m1: &[[f32; 3]; 3], m2: Option<&[[f32; 3]; 3]>, g: f32) -> [[f64; 3]; 3] {
    let g = f64::from(g.clamp(0.0, 1.0));
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            let a = f64::from(m1[i][j]);
            out[i][j] = match m2 {
                Some(m2) => a * g + f64::from(m2[i][j]) * (1.0 - g),
                None => a,
            };
        }
    }
    out
}

/// Correlated colour temperature of an EXIF LightSource code (DNG SDK table); 0 = unknown.
pub fn illuminant_temperature(code: u16) -> f64 {
    match code {
        1 | 4 | 9 | 18 | 20 => 5500.0, // daylight, flash, fine weather, B, D55
        2 | 15 => 3450.0,              // fluorescent, white fluorescent
        3 | 17 => 2850.0,              // tungsten, standard A
        10 | 19 => 6500.0,             // cloudy, C
        11 | 22 => 7500.0,             // shade, D75
        12 => 6430.0,                  // daylight fluorescent
        13 => 5000.0,                  // day white fluorescent
        14 => 4150.0,                  // cool white fluorescent
        21 => 6504.0,                  // D65
        23 => 5003.0,                  // D50
        24 => 3200.0,                  // ISO studio tungsten
        _ => 0.0,
    }
}

// ---------------------------------------------------------------------------
// Small matrix helpers (f64, row-major)
// ---------------------------------------------------------------------------

pub const IDENTITY: [[f64; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
/// D50 white xy as the DNG SDK uses it (PCS white).
pub const D50_XY: (f64, f64) = (0.3457, 0.3585);

pub fn xy_to_xyz(xy: (f64, f64)) -> [f64; 3] {
    let y = xy.1.max(1e-9);
    [xy.0 / y, 1.0, (1.0 - xy.0 - xy.1) / y]
}

pub fn mul_mv(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

pub fn mul_mm(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

pub fn invert3(m: &[[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    crate::develop::wb::invert3(m)
}

/// Bradford chromatic adaptation from white `src` to white `dst` (XYZ).
pub fn bradford(src: [f64; 3], dst: [f64; 3]) -> [[f64; 3]; 3] {
    const B: [[f64; 3]; 3] = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];
    let Some(bi) = invert3(&B) else { return IDENTITY };
    let s = mul_mv(&B, src);
    let d = mul_mv(&B, dst);
    let mut diag = IDENTITY;
    for i in 0..3 {
        diag[i][i] = if s[i].abs() > 1e-12 { d[i] / s[i] } else { 1.0 };
    }
    mul_mm(&bi, &mul_mm(&diag, &B))
}

// ---------------------------------------------------------------------------
// HSV tables (DNG SDK RefBaselineHueSatMap)
// ---------------------------------------------------------------------------

/// RGB -> HSV with hue in 0..6 (DNG SDK `DNG_RGBtoHSV`).
#[inline]
pub fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let v = r.max(g).max(b);
    let gap = v - r.min(g).min(b);
    if gap > 0.0 {
        let mut h = if r == v {
            let h = (g - b) / gap;
            if h < 0.0 {
                h + 6.0
            } else {
                h
            }
        } else if g == v {
            2.0 + (b - r) / gap
        } else {
            4.0 + (r - g) / gap
        };
        if h >= 6.0 {
            h -= 6.0;
        }
        (h, gap / v, v)
    } else {
        (0.0, 0.0, v)
    }
}

/// HSV (hue 0..6) -> RGB (DNG SDK `DNG_HSVtoRGB`).
#[inline]
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    if s <= 0.0 {
        return [v, v, v];
    }
    let mut h = h;
    if h < 0.0 {
        h += 6.0;
    }
    if h >= 6.0 {
        h -= 6.0;
    }
    let i = h as i32;
    let f = h - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

fn srgb_encode(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn srgb_decode(e: f32) -> f32 {
    if e <= 0.040_45 {
        e / 12.92
    } else {
        ((e + 0.055) / 1.055).powf(2.4)
    }
}

impl HsvTable {
    /// Identity table (hue 0, scales 1).
    pub fn is_identity(&self) -> bool {
        self.data.iter().all(|d| d[0] == 0.0 && d[1] == 1.0 && d[2] == 1.0)
    }

    /// (hue shift degrees, sat scale, val scale) at HSV `(h 0..6, s 0..1, v_enc 0..1)`.
    #[inline]
    fn lookup(&self, h: f32, s: f32, v_enc: f32) -> [f32; 3] {
        let hd = self.hue_divisions as usize;
        let sd = self.sat_divisions as usize;
        let vd = self.val_divisions as usize;
        let h_scaled = h * (hd as f32 / 6.0);
        let s_scaled = s * (sd - 1) as f32;
        let mut h0 = h_scaled as usize;
        let s0 = (s_scaled as usize).min(sd.saturating_sub(2));
        let hf = h_scaled - h0 as f32;
        let sf = s_scaled - s0 as f32;
        let mut h1 = h0 + 1;
        if h0 >= hd - 1 {
            h0 = hd - 1;
            h1 = 0;
        }
        let hue_step = sd;
        let blend_hs = |base: usize| -> [f32; 3] {
            let e00 = &self.data[base + h0 * hue_step + s0];
            let e01 = &self.data[base + h1 * hue_step + s0];
            let e10 = &self.data[base + h0 * hue_step + s0 + 1];
            let e11 = &self.data[base + h1 * hue_step + s0 + 1];
            let mut out = [0.0f32; 3];
            for k in 0..3 {
                let a = e00[k] * (1.0 - hf) + e01[k] * hf;
                let b = e10[k] * (1.0 - hf) + e11[k] * hf;
                out[k] = a * (1.0 - sf) + b * sf;
            }
            out
        };
        if vd < 2 {
            return blend_hs(0);
        }
        let v_scaled = v_enc.clamp(0.0, 1.0) * (vd - 1) as f32;
        let v0 = (v_scaled as usize).min(vd - 2);
        let vf = v_scaled - v0 as f32;
        let val_step = hd * sd;
        let a = blend_hs(v0 * val_step);
        let b = blend_hs((v0 + 1) * val_step);
        [a[0] * (1.0 - vf) + b[0] * vf, a[1] * (1.0 - vf) + b[1] * vf, a[2] * (1.0 - vf) + b[2] * vf]
    }

    /// Applies the table to linear RGB (DNG SDK `RefBaselineHueSatMap`): hue shift, saturation
    /// scale (capped at 1), value scale in the table's value encoding. `amount` blends the
    /// modification (1 = full; looks with an amount slider).
    #[inline]
    pub fn apply(&self, rgb: [f32; 3], amount: f32) -> [f32; 3] {
        let [r, g, b] = rgb;
        let (h, s, v) = rgb_to_hsv(r.max(0.0), g.max(0.0), b.max(0.0));
        let v_enc = if self.srgb_gamma { srgb_encode(v.min(1.0)) } else { v };
        let [mut hue_shift, mut sat_scale, mut val_scale] = self.lookup(h, s, v_enc);
        if amount != 1.0 {
            hue_shift *= amount;
            sat_scale = 1.0 + (sat_scale - 1.0) * amount;
            val_scale = 1.0 + (val_scale - 1.0) * amount;
        }
        let h2 = h + hue_shift * (6.0 / 360.0);
        let s2 = (s * sat_scale).clamp(0.0, 1.0);
        let v2 = if self.srgb_gamma {
            if v <= 1.0 {
                srgb_decode((v_enc * val_scale).clamp(0.0, 1.0))
            } else {
                v * val_scale
            }
        } else {
            v * val_scale
        };
        hsv_to_rgb(h2, s2, v2)
    }
}

#[cfg(test)]
pub mod tests_support {
    use super::*;

    /// Minimal little-endian DCP writer for tests.
    pub fn synthetic_dcp(tags: &[(u16, u16, Vec<u8>, u32)]) -> Vec<u8> {
        let mut out = b"IIRC".to_vec();
        out.extend_from_slice(&8u32.to_le_bytes());
        let n = tags.len();
        let data_start = 8 + 2 + n * 12 + 4;
        let mut data = Vec::new();
        let mut ifd = Vec::new();
        ifd.extend_from_slice(&(n as u16).to_le_bytes());
        let mut sorted: Vec<_> = tags.iter().collect();
        sorted.sort_by_key(|t| t.0);
        for (tag, typ, bytes, count) in sorted {
            ifd.extend_from_slice(&tag.to_le_bytes());
            ifd.extend_from_slice(&typ.to_le_bytes());
            ifd.extend_from_slice(&count.to_le_bytes());
            if bytes.len() <= 4 {
                let mut v = bytes.clone();
                v.resize(4, 0);
                ifd.extend_from_slice(&v);
            } else {
                ifd.extend_from_slice(&((data_start + data.len()) as u32).to_le_bytes());
                data.extend_from_slice(bytes);
                if data.len() % 2 == 1 {
                    data.push(0);
                }
            }
        }
        ifd.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&ifd);
        out.extend_from_slice(&data);
        out
    }

    pub fn srational(v: &[f64]) -> Vec<u8> {
        v.iter()
            .flat_map(|x| {
                let n = (x * 10000.0).round() as i32;
                [n.to_le_bytes(), 10000i32.to_le_bytes()].concat()
            })
            .collect()
    }

    pub fn floats(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    pub fn longs(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    pub fn ascii(s: &str) -> (Vec<u8>, u32) {
        let mut b = s.as_bytes().to_vec();
        b.push(0);
        let n = b.len() as u32;
        (b, n)
    }

    pub fn sample() -> Vec<u8> {
        let (model, mn) = ascii("Sony ILCE-7M4");
        let (name, nn) = ascii("Adobe Standard");
        let cm1 = [0.8784, -0.4791, 0.1177, -0.3468, 1.0693, 0.3213, 0.0009, 0.0507, 0.7395];
        let cm2 = [0.746, -0.2365, -0.0588, -0.5687, 1.3442, 0.2474, -0.0624, 0.1156, 0.6584];
        let fm1 = [0.4743, 0.3796, 0.1104, 0.2023, 0.7673, 0.0304, 0.0553, 0.0008, 0.769];
        let fm2 = [0.5465, 0.2614, 0.1563, 0.3232, 0.6292, 0.0475, 0.1339, 0.0025, 0.6887];
        // 4 hues x 2 sats, 2.5D: hue shift +10 degrees everywhere in table 1, 0 in table 2.
        let hsm1: Vec<f32> = (0..8).flat_map(|_| [10.0, 1.0, 1.0]).collect();
        let hsm2: Vec<f32> = (0..8).flat_map(|_| [0.0, 1.0, 1.0]).collect();
        synthetic_dcp(&[
            (TAG_UNIQUE_CAMERA_MODEL, 2, model, mn),
            (TAG_PROFILE_NAME, 2, name, nn),
            (TAG_COLOR_MATRIX1, 10, srational(&cm1), 9),
            (TAG_COLOR_MATRIX2, 10, srational(&cm2), 9),
            (TAG_FORWARD_MATRIX1, 10, srational(&fm1), 9),
            (TAG_FORWARD_MATRIX2, 10, srational(&fm2), 9),
            (TAG_CALIBRATION_ILLUMINANT1, 3, 17u16.to_le_bytes().to_vec(), 1),
            (TAG_CALIBRATION_ILLUMINANT2, 3, 21u16.to_le_bytes().to_vec(), 1),
            (TAG_HSM_DIMS, 4, longs(&[4, 2, 1]), 3),
            (TAG_HSM_DATA1, 11, floats(&hsm1), 24),
            (TAG_HSM_DATA2, 11, floats(&hsm2), 24),
            (TAG_TONE_CURVE, 11, floats(&[0.0, 0.0, 0.5, 0.6, 1.0, 1.0]), 6),
            (TAG_BASELINE_EXPOSURE_OFFSET, 10, srational(&[-0.25]), 1),
            (TAG_DEFAULT_BLACK_RENDER, 4, longs(&[1]), 1),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::*;
    use super::*;

    #[test]
    fn parses_synthetic_dcp() {
        let bytes = sample();
        let d = Dcp::parse(&bytes).unwrap();
        assert_eq!(d.unique_camera_model, "Sony ILCE-7M4");
        assert_eq!(d.profile_name, "Adobe Standard");
        assert_eq!(Dcp::peek_names(&bytes).unwrap(), ("Sony ILCE-7M4".into(), "Adobe Standard".into()));
        assert_eq!((d.calibration_illuminant1, d.calibration_illuminant2), (17, Some(21)));
        assert!((d.color_matrix1[0][0] - 0.8784).abs() < 1e-4);
        assert!((d.forward_matrix2.unwrap()[2][2] - 0.6887).abs() < 1e-4);
        let hsm = d.hue_sat_map1.as_ref().unwrap();
        assert_eq!((hsm.hue_divisions, hsm.sat_divisions, hsm.val_divisions), (4, 2, 1));
        assert_eq!(hsm.data[3], [10.0, 1.0, 1.0]);
        assert_eq!(d.tone_curve.as_ref().unwrap()[1], [0.5, 0.6]);
        assert!((d.baseline_exposure_offset + 0.25).abs() < 1e-6);
        assert_eq!(d.default_black_render, 1);
        assert!(d.look_table.is_none());
        // Malformed input is an error, not a panic.
        assert!(Dcp::parse(&bytes[..20]).is_err());
        assert!(Dcp::parse(b"hello world, not a dcp").is_err());
        let mut bad = bytes.clone();
        bad[2] = 0x99;
        assert!(Dcp::parse(&bad).is_err());
    }

    #[test]
    fn illuminant_weight_is_inverse_mired() {
        let d = Dcp::parse(&sample()).unwrap();
        assert_eq!(d.illuminant_weight(2000.0), 1.0);
        assert_eq!(d.illuminant_weight(2850.0), 1.0);
        assert_eq!(d.illuminant_weight(6504.0), 0.0);
        assert_eq!(d.illuminant_weight(9000.0), 0.0);
        let mid = 1.0 / ((1.0 / 2850.0 + 1.0 / 6504.0) / 2.0);
        assert!((d.illuminant_weight(mid as f32) - 0.5).abs() < 1e-4);
        // Monotone decreasing.
        let mut last = 1.0;
        for t in (2900..6500).step_by(100) {
            let g = d.illuminant_weight(t as f32);
            assert!(g <= last);
            last = g;
        }
        // Interpolated tables and matrices.
        let hsm = d.hue_sat_map(0.5).unwrap();
        assert!((hsm.data[0][0] - 5.0).abs() < 1e-6);
        let fm = d.forward_matrix(0.25).unwrap();
        assert!((fm[0][0] - (0.4743 * 0.25 + 0.5465 * 0.75)).abs() < 1e-6);
    }

    #[test]
    fn forward_matrix_maps_neutral_to_d50() {
        let d = Dcp::parse(&sample()).unwrap();
        for (t, tint) in [(2850.0, 0.0), (4300.0, 5.0), (6500.0, -10.0)] {
            let xy = crate::develop::wb::xy_for(t, tint);
            let neutral = d.xy_to_neutral(xy, [1.0; 3]);
            assert!((neutral.iter().cloned().fold(0.0, f64::max) - 1.0).abs() < 1e-9);
            let back = d.neutral_to_xy(neutral, [1.0; 3]);
            assert!((back.0 - xy.0).abs() < 1e-5 && (back.1 - xy.1).abs() < 1e-5, "{xy:?} {back:?}");
            let (m, _) = d.camera_to_pcs(xy, neutral, [1.0; 3]);
            let w = mul_mv(&m, neutral);
            let d50 = xy_to_xyz(D50_XY);
            for i in 0..3 {
                assert!((w[i] - d50[i]).abs() < 1e-6, "{t}: {w:?}");
            }
            // Colour-matrix path agrees on the neutral.
            let cm = color_matrix_to_pcs(&d.color_matrix(d.illuminant_weight(t as f32)), xy);
            let w = mul_mv(&cm, neutral);
            for i in 0..3 {
                assert!((w[i] - d50[i]).abs() < 1e-6, "{t}: {w:?}");
            }
        }
    }

    #[test]
    fn hsv_round_trip_and_table_identity() {
        for rgb in [[0.2f32, 0.5, 0.9], [0.9, 0.1, 0.3], [0.4, 0.4, 0.4], [0.0, 0.0, 0.0], [0.7, 0.7, 0.1]] {
            let (h, s, v) = rgb_to_hsv(rgb[0], rgb[1], rgb[2]);
            let back = hsv_to_rgb(h, s, v);
            for k in 0..3 {
                assert!((back[k] - rgb[k]).abs() < 1e-5, "{rgb:?} {back:?}");
            }
        }
        let ident = HsvTable {
            hue_divisions: 6,
            sat_divisions: 3,
            val_divisions: 2,
            data: vec![[0.0, 1.0, 1.0]; 36],
            srgb_gamma: true,
        };
        assert!(ident.is_identity());
        let c = [0.3, 0.2, 0.05];
        let out = ident.apply(c, 1.0);
        for k in 0..3 {
            assert!((out[k] - c[k]).abs() < 1e-5);
        }
    }

    #[test]
    fn hsv_table_interpolates_between_nodes() {
        // 2.5D, 4 hue divisions (0, 90, 180, 270 deg), 2 sat divisions: hue shift = 20 deg at
        // hue node 1 (90 deg), 0 elsewhere; sat scale 1.5 at s = 1 nodes.
        let mut data = vec![[0.0f32, 1.0, 1.0]; 8];
        data[2] = [20.0, 1.0, 1.0]; // hue 1, sat 0
        data[3] = [20.0, 1.5, 1.0]; // hue 1, sat 1
        data[1] = [0.0, 1.5, 1.0];
        data[5] = [0.0, 1.5, 1.0];
        data[7] = [0.0, 1.5, 1.0];
        let t = HsvTable { hue_divisions: 4, sat_divisions: 2, val_divisions: 1, data, srgb_gamma: false };
        // Hue 45 deg (between node 0 and 1), s = 0.5: shift 10 deg, sat scale 1.25.
        let rgb = hsv_to_rgb(45.0 / 60.0, 0.5, 0.8);
        let out = t.apply(rgb, 1.0);
        let (h, s, v) = rgb_to_hsv(out[0], out[1], out[2]);
        assert!((h * 60.0 - 55.0).abs() < 0.01, "{}", h * 60.0);
        assert!((s - 0.625).abs() < 1e-4, "{s}");
        assert!((v - 0.8).abs() < 1e-5);
        // Half amount: half the shift.
        let out = t.apply(rgb, 0.5);
        let (h, _, _) = rgb_to_hsv(out[0], out[1], out[2]);
        assert!((h * 60.0 - 50.0).abs() < 0.01);
        // Hue wraps from the last node back to the first.
        let rgb = hsv_to_rgb(315.0 / 60.0, 0.5, 0.5);
        let out = t.apply(rgb, 1.0);
        let (h, _, _) = rgb_to_hsv(out[0], out[1], out[2]);
        assert!((h * 60.0 - 315.0).abs() < 0.01);
    }

    #[test]
    #[ignore = "needs Adobe DNG Converter / Camera Raw profiles installed"]
    fn parses_installed_adobe_standard() {
        let dir = std::path::Path::new(super::super::ProfileConfig::SYSTEM_DCP_DIR).join("Adobe Standard");
        let path = dir.join("Sony ILCE-7M4 Adobe Standard.dcp");
        let bytes = std::fs::read(&path).expect("installed DCP");
        let d = Dcp::parse(&bytes).unwrap();
        assert_eq!(d.unique_camera_model, "Sony ILCE-7M4");
        assert_eq!(d.profile_name, "Adobe Standard");
        let hsm = d.hue_sat_map1.as_ref().unwrap();
        assert_eq!((hsm.hue_divisions, hsm.sat_divisions, hsm.val_divisions), (90, 30, 1));
        let lt = d.look_table.as_ref().unwrap();
        assert_eq!((lt.hue_divisions, lt.sat_divisions, lt.val_divisions), (36, 8, 16));
        assert!(d.forward_matrix1.is_some() && d.forward_matrix2.is_some());
        assert_eq!((d.calibration_illuminant1, d.calibration_illuminant2), (17, Some(21)));
    }
}
