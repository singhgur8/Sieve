//! EXIF values the Library Metadata panel reads from the file on demand (`get_image_metadata`,
//! IPC v19): 35 mm focal length, exposure compensation, flash, body serial, GPS. Read through
//! the same IFD0 / EXIF / GPS directories the exporter copies (`raw::exif_dirs`: ARW TIFF,
//! RAF embedded EXIF, CR3 CMT boxes, raster APP1), a few hundred KB at most.

use std::path::Path;

use super::tiff::{ExifDirs, RawTag};
use crate::ipc::types::GpsLocation;

/// File-derived fields of `ImageMetadata` (`None` = absent / unreadable).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileExif {
    pub focal_length_35mm: Option<f32>,
    pub exposure_compensation_ev: Option<f32>,
    pub flash_fired: Option<bool>,
    pub camera_serial: Option<String>,
    pub gps: Option<GpsLocation>,
}

const FOCAL_35MM: u16 = 0xA405;
const EXPOSURE_BIAS: u16 = 0x9204;
const FLASH: u16 = 0x9209;
const BODY_SERIAL: u16 = 0xA431;
/// Canon's IFD0 / older bodies: `SerialNumber` in IFD0 (0xC62F is DNG's camera serial).
const DNG_SERIAL: u16 = 0xC62F;
const GPS_LAT_REF: u16 = 1;
const GPS_LAT: u16 = 2;
const GPS_LON_REF: u16 = 3;
const GPS_LON: u16 = 4;
const GPS_ALT_REF: u16 = 5;
const GPS_ALT: u16 = 6;

/// Reads the panel's EXIF values of `path` (empty on any error: the panel shows what it has).
pub fn read(path: &Path) -> FileExif {
    super::exif_dirs(path).map(|d| from_dirs(&d)).unwrap_or_default()
}

pub fn from_dirs(d: &ExifDirs) -> FileExif {
    let exif = |t| find(&d.exif, t).or_else(|| find(&d.ifd0, t));
    FileExif {
        focal_length_35mm: exif(FOCAL_35MM).and_then(uint).filter(|v| *v > 0).map(|v| v as f32),
        exposure_compensation_ev: exif(EXPOSURE_BIAS)
            .and_then(|t| rationals(t).first().copied())
            .and_then(|(n, dd)| (dd != 0).then(|| (n as f64 / dd as f64) as f32))
            .map(|v| (v * 100.0).round() / 100.0),
        flash_fired: exif(FLASH).and_then(uint).map(|v| v & 1 == 1),
        camera_serial: exif(BODY_SERIAL).or_else(|| find(&d.ifd0, DNG_SERIAL)).and_then(ascii),
        gps: gps(&d.gps),
    }
}

fn find(tags: &[RawTag], tag: u16) -> Option<&RawTag> {
    tags.iter().find(|t| t.tag == tag)
}

/// SHORT / LONG (values are little-endian in `RawTag`).
fn uint(t: &RawTag) -> Option<u32> {
    match t.typ {
        3 => t.data.get(..2).map(|b| u32::from(u16::from_le_bytes([b[0], b[1]]))),
        4 => t.data.get(..4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        1 | 7 => t.data.first().map(|&b| u32::from(b)),
        _ => None,
    }
}

/// RATIONAL / SRATIONAL values.
fn rationals(t: &RawTag) -> Vec<(i64, i64)> {
    if t.typ != 5 && t.typ != 10 {
        return Vec::new();
    }
    t.data
        .as_chunks::<8>()
        .0
        .iter()
        .map(|c| {
            let (n, d) = ([c[0], c[1], c[2], c[3]], [c[4], c[5], c[6], c[7]]);
            if t.typ == 10 {
                (i64::from(i32::from_le_bytes(n)), i64::from(i32::from_le_bytes(d)))
            } else {
                (i64::from(u32::from_le_bytes(n)), i64::from(u32::from_le_bytes(d)))
            }
        })
        .collect()
}

fn ascii(t: &RawTag) -> Option<String> {
    if t.typ != 2 && t.typ != 7 && t.typ != 1 {
        return None;
    }
    let end = t.data.iter().position(|&b| b == 0).unwrap_or(t.data.len());
    let s = String::from_utf8_lossy(&t.data[..end]).trim().to_owned();
    (!s.is_empty() && s.chars().any(|c| c.is_ascii_alphanumeric())).then_some(s)
}

/// Degrees from `(deg, min, sec)` rationals.
fn degrees(t: &RawTag) -> Option<f64> {
    let v = rationals(t);
    if v.is_empty() || v.iter().any(|(_, d)| *d == 0) {
        return None;
    }
    let f = |i: usize| v.get(i).map_or(0.0, |(n, d)| *n as f64 / *d as f64);
    Some(f(0) + f(1) / 60.0 + f(2) / 3600.0)
}

fn gps(tags: &[RawTag]) -> Option<GpsLocation> {
    let sign = |r: Option<&RawTag>, neg: char| match r.and_then(ascii) {
        Some(s) if s.starts_with(neg) => -1.0,
        _ => 1.0,
    };
    let lat = degrees(find(tags, GPS_LAT)?)? * sign(find(tags, GPS_LAT_REF), 'S');
    let lon = degrees(find(tags, GPS_LON)?)? * sign(find(tags, GPS_LON_REF), 'W');
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) || (lat == 0.0 && lon == 0.0) {
        return None;
    }
    let altitude_m =
        find(tags, GPS_ALT).and_then(|t| rationals(t).first().copied()).filter(|(_, d)| *d != 0).map(|(n, d)| {
            let below = find(tags, GPS_ALT_REF).and_then(uint) == Some(1);
            let a = n as f64 / d as f64;
            if below {
                -a
            } else {
                a
            }
        });
    Some(GpsLocation { latitude: lat, longitude: lon, altitude_m })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(tag: u16, typ: u16, data: Vec<u8>) -> RawTag {
        let size = match typ {
            3 => 2,
            4 => 4,
            5 | 10 => 8,
            _ => 1,
        };
        RawTag { tag, typ, count: (data.len() / size) as u32, data }
    }

    fn rat(v: &[(u32, u32)]) -> Vec<u8> {
        v.iter().flat_map(|(n, d)| n.to_le_bytes().into_iter().chain(d.to_le_bytes())).collect()
    }

    #[test]
    fn reads_panel_values() {
        let d = ExifDirs {
            ifd0: Vec::new(),
            exif: vec![
                tag(FOCAL_35MM, 3, 41u16.to_le_bytes().to_vec()),
                tag(EXPOSURE_BIAS, 10, [(-4i32).to_le_bytes(), 3i32.to_le_bytes()].concat()),
                tag(FLASH, 3, 0x10u16.to_le_bytes().to_vec()),
                tag(BODY_SERIAL, 2, b"61000657\0".to_vec()),
            ],
            gps: vec![
                tag(GPS_LAT_REF, 2, b"N\0".to_vec()),
                tag(GPS_LAT, 5, rat(&[(51, 1), (2, 1), (4500, 100)])),
                tag(GPS_LON_REF, 2, b"W\0".to_vec()),
                tag(GPS_LON, 5, rat(&[(114, 1), (3, 1), (0, 1)])),
                tag(GPS_ALT_REF, 1, vec![0]),
                tag(GPS_ALT, 5, rat(&[(10450, 10)])),
            ],
        };
        let f = from_dirs(&d);
        assert_eq!(f.focal_length_35mm, Some(41.0));
        assert_eq!(f.exposure_compensation_ev, Some(-1.33));
        assert_eq!(f.flash_fired, Some(false), "0x10 = off, did not fire");
        assert_eq!(f.camera_serial.as_deref(), Some("61000657"));
        let g = f.gps.unwrap();
        assert!((g.latitude - (51.0 + 2.0 / 60.0 + 45.0 / 3600.0)).abs() < 1e-9);
        assert!((g.longitude + (114.0 + 3.0 / 60.0)).abs() < 1e-9);
        assert_eq!(g.altitude_m, Some(1045.0));
        let fired = ExifDirs { exif: vec![tag(FLASH, 3, 0x19u16.to_le_bytes().to_vec())], ..Default::default() };
        assert_eq!(from_dirs(&fired).flash_fired, Some(true));
        assert_eq!(from_dirs(&ExifDirs::default()), FileExif::default());
    }

    /// The user's sample RAWs (read-only), when present.
    #[test]
    #[ignore = "needs ~/Pictures/Jasmit Natalie Proposal"]
    fn reads_real_files() {
        let dir =
            std::path::PathBuf::from(std::env::var("HOME").unwrap()).join("Pictures/Jasmit Natalie Proposal/10060918");
        let arw = read(&dir.join("AZA06414.ARW"));
        assert_eq!((arw.focal_length_35mm, arw.camera_serial.as_deref()), (Some(35.0), Some("06258214")));
        assert_eq!((arw.exposure_compensation_ev, arw.flash_fired), (Some(0.0), Some(false)));
        let raf = read(&dir.join("DSCF5910.RAF"));
        assert_eq!((raf.focal_length_35mm, raf.camera_serial.as_deref()), (Some(41.0), Some("61000657")));
        assert_eq!(raf.exposure_compensation_ev, Some(-1.33));
        let cr3 = read(&dir.join("IMG_5592.CR3"));
        eprintln!("{arw:?}\n{raf:?}\n{cr3:?}");
        assert!(cr3.focal_length_35mm.is_some() || cr3.camera_serial.is_some());
    }
}
