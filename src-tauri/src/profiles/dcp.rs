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
//! The existing `raw::tiff` reader can be reused (it takes the magic as a parameter or a
//! small variant). Parsed matrices are row-major 3x3 (3-colour cameras).

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

impl Dcp {
    /// Parses a DCP file's bytes.
    pub fn parse(bytes: &[u8]) -> Result<Dcp, String> {
        let _ = bytes;
        todo!("rust-engine-dev: profiles::dcp::Dcp::parse")
    }

    /// Reads only `UniqueCameraModel` and `ProfileName` (library index scan).
    pub fn peek_names(bytes: &[u8]) -> Result<(String, String), String> {
        let _ = bytes;
        todo!("rust-engine-dev: profiles::dcp::Dcp::peek_names")
    }

    /// Weight of illuminant 1 (0..=1) for a white point of correlated colour temperature
    /// `temperature_k` (inverse-mired interpolation between the two calibration illuminants).
    pub fn illuminant_weight(&self, temperature_k: f32) -> f32 {
        let _ = temperature_k;
        todo!("rust-engine-dev: profiles::dcp::Dcp::illuminant_weight")
    }
}
