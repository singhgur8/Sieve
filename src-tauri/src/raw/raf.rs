//! Fuji RAF: fixed big-endian header pointing at an embedded JPEG (whose APP1 carries
//! the camera EXIF) and a tag directory with the raw dimensions / CFA layout.

use super::jpeg;
use super::meta::MetaBuilder;
use super::source::ByteSource;
use super::tiff::{self, JpegRef};
use super::Container;

const HEADER_LEN: usize = 0x6C;
const JPEG_OFFSET: usize = 0x54;
const JPEG_LENGTH: usize = 0x58;
const META_OFFSET: usize = 0x5C;
const META_LENGTH: usize = 0x60;
const MODEL: std::ops::Range<usize> = 0x1C..0x3C;

// RAF directory tags.
const RAW_IMAGE_FULL_SIZE: u16 = 0x0100;
const RAW_IMAGE_CROPPED_SIZE: u16 = 0x0111;
const XTRANS_LAYOUT: u16 = 0x0131;

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

pub fn parse(src: &(impl ByteSource + ?Sized)) -> Result<Container, String> {
    let mut h = [0u8; HEADER_LEN];
    src.read_at(0, &mut h).map_err(|e| format!("RAF header: {e}"))?;
    if !h.starts_with(b"FUJIFILMCCD-RAW") {
        return Err("not a RAF (bad magic)".into());
    }
    let jpeg = JpegRef { offset: be32(&h, JPEG_OFFSET) as u64, len: be32(&h, JPEG_LENGTH) as u64 };

    let mut meta = MetaBuilder::default();
    // EXIF lives in the embedded JPEG's APP1 (< 64 KB, right after SOI).
    if jpeg.len > 0 {
        let head_len = jpeg.len.min(70 * 1024) as usize;
        if let Ok(head) = src.read_vec(jpeg.offset, head_len) {
            if let Some(exif) = jpeg::exif_tiff(&head) {
                if let Ok(scan) = tiff::scan_bytes(exif, false) {
                    meta = scan.meta;
                }
            }
        }
    }
    if meta.model.is_none() {
        let raw = &h[MODEL];
        let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
        let model = String::from_utf8_lossy(&raw[..end]).trim().to_owned();
        if !model.is_empty() {
            meta.model = Some(model);
        }
    }
    if meta.make.is_none() {
        meta.make = Some("FUJIFILM".into());
    }

    let (meta_off, meta_len) = (be32(&h, META_OFFSET) as u64, be32(&h, META_LENGTH) as u64);
    if (4..=1 << 20).contains(&meta_len) {
        if let Ok(dir) = src.read_vec(meta_off, meta_len as usize) {
            read_directory(&dir, &mut meta);
        }
    }

    Ok(Container { meta, jpegs: if jpeg.len > 0 { vec![jpeg] } else { Vec::new() } })
}

/// RAF directory: u32 count, then `(tag u16, size u16, data[size])` records.
fn read_directory(dir: &[u8], meta: &mut MetaBuilder) {
    let count = be32(dir, 0).min(512);
    let mut pos = 4usize;
    let (mut full, mut cropped) = (None, None);
    for _ in 0..count {
        let Some(hdr) = dir.get(pos..pos + 4) else { break };
        let tag = u16::from_be_bytes([hdr[0], hdr[1]]);
        let size = u16::from_be_bytes([hdr[2], hdr[3]]) as usize;
        let Some(data) = dir.get(pos + 4..pos + 4 + size) else { break };
        let dims = || {
            (data.len() >= 4).then(|| {
                let a = u16::from_be_bytes([data[0], data[1]]) as u32;
                let b = u16::from_be_bytes([data[2], data[3]]) as u32;
                // Stored as height, width; raw data is always landscape.
                (a.max(b), a.min(b))
            })
        };
        match tag {
            RAW_IMAGE_FULL_SIZE => full = dims(),
            RAW_IMAGE_CROPPED_SIZE => cropped = dims(),
            XTRANS_LAYOUT => meta.xtrans_hint = true,
            _ => {}
        }
        pos += 4 + size;
    }
    if meta.sensor_size.is_none() {
        meta.sensor_size = cropped.or(full).filter(|&(w, h)| w > 0 && h > 0);
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    /// Builds a RAF: header + directory + embedded JPEG.
    pub fn raf(model: &str, jpeg: &[u8], dir: &[(u16, Vec<u8>)]) -> Vec<u8> {
        let mut h = vec![0u8; super::HEADER_LEN];
        h[..16].copy_from_slice(b"FUJIFILMCCD-RAW ");
        h[16..20].copy_from_slice(b"0201");
        h[0x1C..0x1C + model.len()].copy_from_slice(model.as_bytes());
        h[0x3C..0x40].copy_from_slice(b"0100");

        let mut d = (dir.len() as u32).to_be_bytes().to_vec();
        for (tag, data) in dir {
            d.extend_from_slice(&tag.to_be_bytes());
            d.extend_from_slice(&(data.len() as u16).to_be_bytes());
            d.extend_from_slice(data);
        }
        let dir_off = h.len() as u32;
        let jpeg_off = dir_off + d.len() as u32;
        h[0x54..0x58].copy_from_slice(&jpeg_off.to_be_bytes());
        h[0x58..0x5C].copy_from_slice(&(jpeg.len() as u32).to_be_bytes());
        h[0x5C..0x60].copy_from_slice(&dir_off.to_be_bytes());
        h[0x60..0x64].copy_from_slice(&(d.len() as u32).to_be_bytes());
        h.extend_from_slice(&d);
        h.extend_from_slice(jpeg);
        h
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::raf;
    use super::*;
    use crate::ipc::types::{CameraMake, SensorLayout};
    use crate::raw::jpeg::test_support::{quadrant_jpeg, with_exif};
    use crate::raw::tiff::build::{tiff, IfdSpec, Val};

    #[test]
    fn parses_header_exif_and_directory() {
        let (exif, _) = tiff(
            &[
                IfdSpec {
                    tags: vec![
                        (0x010F, Val::Ascii("FUJIFILM".into())),
                        (0x0110, Val::Ascii("X-T5".into())),
                        (0x0112, Val::Short(vec![6])),
                        (0x8769, Val::IfdRef(1)),
                    ],
                    next: None,
                },
                IfdSpec {
                    tags: vec![(0x9003, Val::Ascii("2025:05:01 10:00:00".into())), (0x8827, Val::Short(vec![400]))],
                    next: None,
                },
            ],
            &[],
        );
        let jpeg = with_exif(&quadrant_jpeg(32, 16), &exif);
        let file = raf("X-T5", &jpeg, &[(0x0100, vec![0x14, 0x3A, 0x1E, 0x48]), (0x0131, vec![0; 36])]);

        let c = parse(file.as_slice()).unwrap();
        assert_eq!(c.jpegs, vec![JpegRef { offset: (file.len() - jpeg.len()) as u64, len: jpeg.len() as u64 }]);
        let m = c.meta.finish();
        assert_eq!(m.make, Some(CameraMake::Fujifilm));
        assert_eq!(m.model.as_deref(), Some("X-T5"));
        assert_eq!(m.orientation, Some(6));
        assert_eq!(m.iso, Some(400));
        assert_eq!(m.sensor_layout, Some(SensorLayout::XTrans));
        assert_eq!((m.width, m.height), (Some(0x1E48), Some(0x143A)));
        assert!(m.captured_at_ms.is_some());
    }

    #[test]
    fn model_from_header_when_exif_missing() {
        let file = raf("GFX100S", &quadrant_jpeg(8, 8), &[]);
        let m = parse(file.as_slice()).unwrap().meta.finish();
        assert_eq!(m.model.as_deref(), Some("GFX100S"));
        assert_eq!(m.sensor_layout, Some(SensorLayout::Bayer));
        assert!(parse(b"FUJIFILMCCD-RAW short".as_slice()).is_err());
    }
}
