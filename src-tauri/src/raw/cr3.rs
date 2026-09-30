//! Canon CR3: ISO-BMFF. Metadata lives in `moov/uuid(Canon)/CMT1..CMT2` (standalone
//! TIFFs: IFD0 and EXIF tags). Previews: full-size JPEG = first `trak`'s only sample
//! (`stsz`/`co64`), a ~1620 px JPEG in the top-level `uuid(PRVW)` box, and a 160 px
//! `THMB` inside the Canon uuid.

use super::meta::MetaBuilder;
use super::source::ByteSource;
use super::tiff::{self, JpegRef};
use super::Container;

const CANON_UUID: [u8; 16] =
    [0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a, 0x48];
const PREVIEW_UUID: [u8; 16] =
    [0xea, 0xf4, 0x2b, 0x5e, 0x1c, 0x98, 0x4b, 0x88, 0xb9, 0xfb, 0xb7, 0xdc, 0x40, 0x6e, 0x4d, 0x16];

const MAX_BOXES: usize = 256;
const MAX_CMT: u64 = 1 << 20;

#[derive(Debug, Clone, Copy)]
struct BoxHdr {
    typ: [u8; 4],
    uuid: Option<[u8; 16]>,
    /// Payload range (after header and uuid).
    start: u64,
    end: u64,
}

fn children(src: &(impl ByteSource + ?Sized), start: u64, end: u64) -> Vec<BoxHdr> {
    let mut out = Vec::new();
    let mut pos = start;
    while pos + 8 <= end && out.len() < MAX_BOXES {
        let mut h = [0u8; 8];
        if src.read_at(pos, &mut h).is_err() {
            break;
        }
        let size32 = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
        let typ = [h[4], h[5], h[6], h[7]];
        let (size, mut hdr_len) = match size32 {
            0 => (end - pos, 8),
            1 => {
                let mut l = [0u8; 8];
                if src.read_at(pos + 8, &mut l).is_err() {
                    break;
                }
                (u64::from_be_bytes(l), 16)
            }
            n => (n, 8),
        };
        if size < hdr_len || pos.checked_add(size).is_none_or(|e| e > end) {
            break;
        }
        let mut uuid = None;
        if &typ == b"uuid" {
            let mut u = [0u8; 16];
            if size < hdr_len + 16 || src.read_at(pos + hdr_len, &mut u).is_err() {
                break;
            }
            uuid = Some(u);
            hdr_len += 16;
        }
        out.push(BoxHdr { typ, uuid, start: pos + hdr_len, end: pos + size });
        pos += size;
    }
    out
}

fn find<'a>(boxes: &'a [BoxHdr], typ: &[u8; 4]) -> Option<&'a BoxHdr> {
    boxes.iter().find(|b| &b.typ == typ)
}

fn find_uuid<'a>(boxes: &'a [BoxHdr], uuid: &[u8; 16]) -> Option<&'a BoxHdr> {
    boxes.iter().find(|b| b.uuid.as_ref() == Some(uuid))
}

/// Locates a JPEG (SOI) inside a small header-prefixed box payload.
fn jpeg_in_box(src: &(impl ByteSource + ?Sized), b: &BoxHdr) -> Option<JpegRef> {
    let probe_len = (b.end - b.start).min(64) as usize;
    let probe = src.read_vec(b.start, probe_len).ok()?;
    let soi = probe.windows(3).position(|w| w == [0xFF, 0xD8, 0xFF])? as u64;
    Some(JpegRef { offset: b.start + soi, len: b.end - b.start - soi })
}

/// Full-size JPEG: the single sample of the first track.
fn trak_jpeg(src: &(impl ByteSource + ?Sized), trak: &BoxHdr) -> Option<JpegRef> {
    let mdia = *find(&children(src, trak.start, trak.end), b"mdia")?;
    let minf = *find(&children(src, mdia.start, mdia.end), b"minf")?;
    let stbl_boxes = {
        let stbl = *find(&children(src, minf.start, minf.end), b"stbl")?;
        children(src, stbl.start, stbl.end)
    };
    let stsz = find(&stbl_boxes, b"stsz")?;
    let s = src.read_vec(stsz.start, 16.min((stsz.end - stsz.start) as usize)).ok()?;
    let be = |i: usize| s.get(i..i + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let len = match be(4)? {
        0 => be(12)?, // per-sample table; first entry
        n => n,
    } as u64;
    let offset = if let Some(co64) = find(&stbl_boxes, b"co64") {
        let c = src.read_vec(co64.start + 8, 8).ok()?;
        u64::from_be_bytes(c.try_into().ok()?)
    } else {
        let stco = find(&stbl_boxes, b"stco")?;
        let c = src.read_vec(stco.start + 8, 4).ok()?;
        u32::from_be_bytes(c.try_into().ok()?) as u64
    };
    (len > 0).then_some(JpegRef { offset, len })
}

pub fn parse(src: &(impl ByteSource + ?Sized)) -> Result<Container, String> {
    let top = children(src, 0, src.len());
    match top.first() {
        Some(b) if &b.typ == b"ftyp" => {}
        _ => return Err("not a CR3 (no ftyp box)".into()),
    }
    let moov = find(&top, b"moov").ok_or("CR3 has no moov box")?;
    let moov_children = children(src, moov.start, moov.end);

    let mut meta = MetaBuilder::default();
    let mut jpegs = Vec::new();

    if let Some(canon) = find_uuid(&moov_children, &CANON_UUID) {
        let canon_children = children(src, canon.start, canon.end);
        for name in [b"CMT1", b"CMT2"] {
            let Some(b) = find(&canon_children, name) else { continue };
            if b.end - b.start > MAX_CMT {
                continue;
            }
            if let Ok(bytes) = src.read_vec(b.start, (b.end - b.start) as usize) {
                if let Ok(scan) = tiff::scan_bytes(&bytes, false) {
                    meta.merge(scan.meta);
                }
            }
        }
        if let Some(j) = find(&canon_children, b"THMB").and_then(|b| jpeg_in_box(src, b)) {
            jpegs.push(j);
        }
    }
    if let Some(j) = find(&moov_children, b"trak").and_then(|t| trak_jpeg(src, t)) {
        jpegs.push(j);
    }
    if let Some(prvw_uuid) = find_uuid(&top, &PREVIEW_UUID) {
        // uuid payload: 8 bytes of header, then a `PRVW` box.
        let inner = children(src, prvw_uuid.start + 8, prvw_uuid.end);
        if let Some(j) = find(&inner, b"PRVW").and_then(|b| jpeg_in_box(src, b)) {
            jpegs.push(j);
        }
    }
    if meta.make.is_none() {
        meta.make = Some("Canon".into());
    }
    Ok(Container { meta, jpegs })
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub fn bx(typ: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(typ);
        v.extend_from_slice(payload);
        v
    }

    pub fn uuid_box(uuid: &[u8; 16], payload: &[u8]) -> Vec<u8> {
        bx(b"uuid", &[uuid.as_slice(), payload].concat())
    }

    /// Builds a CR3 with CMT1/CMT2 TIFFs, a THMB, a PRVW and a full-size JPEG in `mdat`.
    pub fn cr3(cmt1: &[u8], cmt2: &[u8], thumb: &[u8], preview: &[u8], full: &[u8]) -> Vec<u8> {
        let ftyp = bx(b"ftyp", b"crx \0\0\0\x01crx isom");
        let thmb = bx(b"THMB", &[&[0u8, 0, 0, 0, 0, 160, 0, 120, 0, 0, 0, 0, 0, 1, 0, 0][..], thumb].concat());
        let canon = uuid_box(&CANON_UUID, &[bx(b"CMT1", cmt1), bx(b"CMT2", cmt2), thmb].concat());
        let prvw = bx(b"PRVW", &[&[0u8, 0, 0, 0, 0, 1, 6, 0x54, 4, 0x38, 0, 1, 0, 0, 0, 0][..], preview].concat());
        let prvw_uuid = uuid_box(&PREVIEW_UUID, &[&[0u8; 8][..], &prvw].concat());

        // moov size is independent of the mdat offset (co64 is fixed width), so lay out twice.
        let build_moov = |mdat_data_off: u64| {
            let stsz = bx(b"stsz", &[[0u8; 4], (full.len() as u32).to_be_bytes(), 1u32.to_be_bytes()].concat());
            let co64 = bx(b"co64", &[&[0u8; 4][..], &1u32.to_be_bytes(), &mdat_data_off.to_be_bytes()].concat());
            let stbl = bx(b"stbl", &[stsz, co64].concat());
            let trak = bx(b"trak", &bx(b"mdia", &bx(b"minf", &stbl)));
            bx(b"moov", &[canon.clone(), trak].concat())
        };
        let head_len = (ftyp.len() + build_moov(0).len() + prvw_uuid.len()) as u64;
        let moov = build_moov(head_len + 8);
        [ftyp, moov, prvw_uuid, bx(b"mdat", full)].concat()
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::ipc::types::{CameraMake, SensorLayout};
    use crate::raw::jpeg::test_support::quadrant_jpeg;
    use crate::raw::tiff::build::{tiff, IfdSpec, Val};

    #[test]
    fn parses_cmt_boxes_and_previews() {
        let (cmt1, _) = tiff(
            &[IfdSpec {
                tags: vec![
                    (0x010F, Val::Ascii("Canon".into())),
                    (0x0110, Val::Ascii("Canon EOS R5".into())),
                    (0x0112, Val::Short(vec![8])),
                ],
                next: None,
            }],
            &[],
        );
        let (cmt2, _) = tiff(
            &[IfdSpec {
                tags: vec![
                    (0x829A, Val::Rational(1, 1000)),
                    (0x8827, Val::Short(vec![800])),
                    (0x9003, Val::Ascii("2024:06:15 14:22:33".into())),
                    (0x9291, Val::Ascii("45".into())),
                    (0xA434, Val::Ascii("RF24-70mm F2.8 L IS USM".into())),
                    (0xA002, Val::Long(vec![8192])),
                    (0xA003, Val::Long(vec![5464])),
                ],
                next: None,
            }],
            &[],
        );
        let (thumb, preview, full) = (quadrant_jpeg(16, 12), quadrant_jpeg(40, 30), quadrant_jpeg(80, 60));
        let file = cr3(&cmt1, &cmt2, &thumb, &preview, &full);

        let c = parse(file.as_slice()).unwrap();
        let blobs: Vec<Vec<u8>> =
            c.jpegs.iter().map(|j| file.as_slice().read_vec(j.offset, j.len as usize).unwrap()).collect();
        assert_eq!(blobs, vec![thumb, full, preview]);

        let m = c.meta.finish();
        assert_eq!(m.make, Some(CameraMake::Canon));
        assert_eq!(m.model.as_deref(), Some("Canon EOS R5"));
        assert_eq!(m.sensor_layout, Some(SensorLayout::Bayer));
        assert_eq!(m.orientation, Some(8));
        assert_eq!(m.iso, Some(800));
        assert_eq!(m.shutter_seconds, Some(0.001));
        assert_eq!(m.lens.as_deref(), Some("RF24-70mm F2.8 L IS USM"));
        assert_eq!((m.width, m.height), (Some(8192), Some(5464)));
        assert_eq!(m.captured_at_ms.map(|t| t % 1000), Some(450));
    }

    #[test]
    fn rejects_non_bmff() {
        assert!(parse(b"\0\0\0\x08moov".as_slice()).is_err());
        assert!(parse(bx(b"ftyp", b"crx ").as_slice()).is_err(), "no moov");
    }
}
