//! Minimal TIFF/EXIF IFD reader.
//!
//! Used for Sony ARW (a TIFF container), the EXIF block inside Fuji's embedded JPEG,
//! and Canon CR3 `CMT1`/`CMT2` boxes (each a standalone TIFF). Only the tags ingest
//! needs are interpreted; everything is bounds-checked and capped so malformed files
//! produce errors, never panics or unbounded allocations.

use std::collections::HashSet;

use super::meta::MetaBuilder;
use super::source::ByteSource;

/// Location of an embedded JPEG inside the container, in source coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JpegRef {
    pub offset: u64,
    pub len: u64,
}

/// Result of walking every IFD of a TIFF.
#[derive(Debug, Default)]
pub struct TiffScan {
    pub meta: MetaBuilder,
    /// Every embedded JPEG candidate (thumbnail, preview, full-size), unvalidated.
    pub jpegs: Vec<JpegRef>,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    tag: u16,
    typ: u16,
    count: u32,
    /// Inline value or offset, in file byte order.
    raw: [u8; 4],
}

struct Ifd {
    entries: Vec<Entry>,
    next: u32,
}

impl Ifd {
    fn get(&self, tag: u16) -> Option<&Entry> {
        self.entries.iter().find(|e| e.tag == tag)
    }
}

// Tags.
const COMPRESSION: u16 = 0x0103;
const MAKE: u16 = 0x010F;
const MODEL: u16 = 0x0110;
const STRIP_OFFSETS: u16 = 0x0111;
const ORIENTATION: u16 = 0x0112;
const STRIP_BYTE_COUNTS: u16 = 0x0117;
const DATE_TIME: u16 = 0x0132;
const SUB_IFDS: u16 = 0x014A;
const JPEG_OFFSET: u16 = 0x0201;
const JPEG_LENGTH: u16 = 0x0202;
const EXPOSURE_TIME: u16 = 0x829A;
const F_NUMBER: u16 = 0x829D;
const EXIF_IFD: u16 = 0x8769;
const ISO: u16 = 0x8827;
const RECOMMENDED_EXPOSURE_INDEX: u16 = 0x8832;
const ISO_SPEED: u16 = 0x8833;
const DATE_TIME_ORIGINAL: u16 = 0x9003;
const DATE_TIME_DIGITIZED: u16 = 0x9004;
const FOCAL_LENGTH: u16 = 0x920A;
const SUB_SEC_TIME: u16 = 0x9290;
const SUB_SEC_TIME_ORIGINAL: u16 = 0x9291;
const SUB_SEC_TIME_DIGITIZED: u16 = 0x9292;
const PIXEL_X_DIMENSION: u16 = 0xA002;
const PIXEL_Y_DIMENSION: u16 = 0xA003;
const LENS_MODEL: u16 = 0xA434;

const MAX_ENTRIES: u16 = 1024;
const MAX_IFDS: usize = 64;
const MAX_STRING: u32 = 1024;
const MAX_ARRAY: u32 = 64;

pub struct Tiff<'a, S: ByteSource + ?Sized> {
    src: &'a S,
    le: bool,
    first_ifd: u32,
}

impl<'a, S: ByteSource + ?Sized> Tiff<'a, S> {
    /// Parses the 8-byte TIFF header at offset 0 of `src`.
    pub fn new(src: &'a S) -> Result<Self, String> {
        let mut h = [0u8; 8];
        src.read_at(0, &mut h).map_err(|e| format!("TIFF header: {e}"))?;
        let le = match &h[..4] {
            b"II*\0" => true,
            b"MM\0*" => false,
            _ => return Err("not a TIFF (bad byte-order mark)".into()),
        };
        let first_ifd = if le {
            u32::from_le_bytes([h[4], h[5], h[6], h[7]])
        } else {
            u32::from_be_bytes([h[4], h[5], h[6], h[7]])
        };
        Ok(Self { src, le, first_ifd })
    }

    fn u16(&self, b: [u8; 2]) -> u16 {
        if self.le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        }
    }

    fn u32(&self, b: [u8; 4]) -> u32 {
        if self.le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        }
    }

    fn read_ifd(&self, offset: u32) -> Result<Ifd, String> {
        let mut n = [0u8; 2];
        self.src.read_at(offset as u64, &mut n).map_err(|e| format!("IFD at {offset}: {e}"))?;
        let count = self.u16(n);
        if count == 0 || count > MAX_ENTRIES {
            return Err(format!("IFD at {offset}: implausible entry count {count}"));
        }
        let table = self
            .src
            .read_vec(offset as u64 + 2, count as usize * 12 + 4)
            .map_err(|e| format!("IFD at {offset}: {e}"))?;
        let entries = table
            .as_chunks::<12>()
            .0
            .iter()
            .map(|c| Entry {
                tag: self.u16([c[0], c[1]]),
                typ: self.u16([c[2], c[3]]),
                count: self.u32([c[4], c[5], c[6], c[7]]),
                raw: [c[8], c[9], c[10], c[11]],
            })
            .collect();
        let t = &table[count as usize * 12..];
        Ok(Ifd { entries, next: self.u32([t[0], t[1], t[2], t[3]]) })
    }

    fn type_size(typ: u16) -> Option<u32> {
        Some(match typ {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 | 13 => 4,
            5 | 10 | 12 => 8,
            _ => return None,
        })
    }

    /// The entry's value bytes (inline or at its offset), capped at `max_count` items.
    fn value_bytes(&self, e: &Entry, max_count: u32) -> Option<Vec<u8>> {
        let size = Self::type_size(e.typ)?;
        let count = e.count.min(max_count);
        let total = size.checked_mul(count)? as usize;
        let full = size.checked_mul(e.count)?;
        if full <= 4 {
            Some(e.raw[..total].to_vec())
        } else {
            self.src.read_vec(self.u32(e.raw) as u64, total).ok()
        }
    }

    /// Unsigned integer values (BYTE/SHORT/LONG/IFD), capped at `MAX_ARRAY`.
    fn uints(&self, e: &Entry) -> Vec<u32> {
        let Some(b) = self.value_bytes(e, MAX_ARRAY) else { return Vec::new() };
        match e.typ {
            1 | 7 => b.iter().map(|&v| v as u32).collect(),
            3 | 8 => b.as_chunks::<2>().0.iter().map(|c| self.u16(*c) as u32).collect(),
            4 | 9 | 13 => b.as_chunks::<4>().0.iter().map(|c| self.u32(*c)).collect(),
            _ => Vec::new(),
        }
    }

    fn uint(&self, e: &Entry) -> Option<u32> {
        self.uints(e).first().copied()
    }

    fn rational(&self, e: &Entry) -> Option<f64> {
        let b = self.value_bytes(e, 1)?;
        if b.len() < 8 {
            return None;
        }
        let (n, d) = (self.u32([b[0], b[1], b[2], b[3]]), self.u32([b[4], b[5], b[6], b[7]]));
        let (n, d) = match e.typ {
            5 => (n as f64, d as f64),
            10 => (n as i32 as f64, d as i32 as f64),
            _ => return None,
        };
        (d != 0.0).then(|| n / d).filter(|v| v.is_finite())
    }

    fn string(&self, e: &Entry) -> Option<String> {
        if e.typ != 2 && e.typ != 7 {
            return None;
        }
        let b = self.value_bytes(e, MAX_STRING)?;
        let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        let s = String::from_utf8_lossy(&b[..end]).trim().to_owned();
        (!s.is_empty()).then_some(s)
    }

    /// Applies the capture-metadata tags present in `ifd` (IFD0 or EXIF tags; the tag
    /// numbers don't overlap). First value seen wins.
    fn apply_meta(&self, ifd: &Ifd, m: &mut MetaBuilder) {
        for e in &ifd.entries {
            match e.tag {
                MAKE => set(&mut m.make, self.string(e)),
                MODEL => set(&mut m.model, self.string(e)),
                ORIENTATION => set(&mut m.orientation, self.uint(e).map(|v| v as u16)),
                DATE_TIME => set(&mut m.date_modified, self.string(e)),
                EXPOSURE_TIME => set(&mut m.exposure_time, self.rational(e)),
                F_NUMBER => set(&mut m.f_number, self.rational(e)),
                ISO => set(&mut m.iso, self.uint(e)),
                RECOMMENDED_EXPOSURE_INDEX => set(&mut m.recommended_exposure_index, self.uint(e)),
                ISO_SPEED => set(&mut m.iso_speed, self.uint(e)),
                DATE_TIME_ORIGINAL => set(&mut m.date_original, self.string(e)),
                DATE_TIME_DIGITIZED => set(&mut m.date_digitized, self.string(e)),
                FOCAL_LENGTH => set(&mut m.focal_length, self.rational(e)),
                SUB_SEC_TIME => set(&mut m.subsec_modified, self.string(e)),
                SUB_SEC_TIME_ORIGINAL => set(&mut m.subsec_original, self.string(e)),
                SUB_SEC_TIME_DIGITIZED => set(&mut m.subsec_digitized, self.string(e)),
                PIXEL_X_DIMENSION => set(&mut m.pixel_width, self.uint(e)),
                PIXEL_Y_DIMENSION => set(&mut m.pixel_height, self.uint(e)),
                LENS_MODEL => set(&mut m.lens, self.string(e)),
                _ => {}
            }
        }
    }

    fn collect_jpegs(&self, ifd: &Ifd, out: &mut Vec<JpegRef>) {
        if let (Some(o), Some(l)) = (ifd.get(JPEG_OFFSET), ifd.get(JPEG_LENGTH)) {
            if let (Some(offset), Some(len)) = (self.uint(o), self.uint(l)) {
                out.push(JpegRef { offset: offset as u64, len: len as u64 });
            }
        }
        // Some bodies store a JPEG as a single "strip" with Compression 6/7. Raw data
        // can look the same (lossless JPEG), so candidates are validated later.
        let compression = ifd.get(COMPRESSION).and_then(|e| self.uint(e));
        if matches!(compression, Some(6 | 7)) {
            if let (Some(o), Some(l)) = (ifd.get(STRIP_OFFSETS), ifd.get(STRIP_BYTE_COUNTS)) {
                let (offs, lens) = (self.uints(o), self.uints(l));
                if offs.len() == 1 && lens.len() == 1 {
                    out.push(JpegRef { offset: offs[0] as u64, len: lens[0] as u64 });
                }
            }
        }
    }

    /// Walks IFD0 and its chain (IFD1, IFD2, ...), their SubIFDs, and IFD0's EXIF IFD.
    /// `jpegs` controls whether embedded-JPEG candidates are collected (otherwise only
    /// IFD0 + EXIF are read).
    pub fn scan(&self, jpegs: bool) -> Result<TiffScan, String> {
        let mut out = TiffScan::default();
        let mut visited = HashSet::new();
        let mut offset = self.first_ifd;
        let mut index = 0;
        while offset != 0 && visited.len() < MAX_IFDS && visited.insert(offset) {
            let ifd = match self.read_ifd(offset) {
                Ok(ifd) => ifd,
                // IFD0 is mandatory; the rest of the chain is best-effort.
                Err(e) if index == 0 => return Err(e),
                Err(_) => break,
            };
            if index == 0 {
                self.apply_meta(&ifd, &mut out.meta);
                if let Some(exif) = ifd.get(EXIF_IFD).and_then(|e| self.uint(e)) {
                    if visited.insert(exif) {
                        if let Ok(exif_ifd) = self.read_ifd(exif) {
                            self.apply_meta(&exif_ifd, &mut out.meta);
                        }
                    }
                }
            }
            if !jpegs {
                break;
            }
            self.collect_jpegs(&ifd, &mut out.jpegs);
            self.walk_sub_ifds(&ifd, 1, &mut visited, &mut out.jpegs);
            offset = ifd.next;
            index += 1;
        }
        Ok(out)
    }

    fn walk_sub_ifds(&self, ifd: &Ifd, depth: u8, visited: &mut HashSet<u32>, out: &mut Vec<JpegRef>) {
        let Some(e) = ifd.get(SUB_IFDS) else { return };
        if depth > 3 {
            return;
        }
        for offset in self.uints(e).into_iter().take(8) {
            if offset == 0 || visited.len() >= MAX_IFDS || !visited.insert(offset) {
                continue;
            }
            if let Ok(sub) = self.read_ifd(offset) {
                self.collect_jpegs(&sub, out);
                self.walk_sub_ifds(&sub, depth + 1, visited, out);
            }
        }
    }
}

/// EXIF colour signalling used when a raster file has no ICC profile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ColorSignal {
    /// EXIF `ColorSpace` (0xA001): 1 = sRGB, 0xFFFF = uncalibrated.
    pub color_space: Option<u32>,
    /// Interoperability IFD `InteroperabilityIndex` ("R98" = sRGB, "R03" = Adobe RGB).
    pub interop_index: Option<String>,
}

const EXIF_COLOR_SPACE: u16 = 0xA001;
const INTEROP_IFD: u16 = 0xA005;
const INTEROP_INDEX: u16 = 0x0001;

impl<S: ByteSource + ?Sized> Tiff<'_, S> {
    /// Raw bytes of an IFD0 tag of any type (e.g. XMP, tag 700), up to `max` bytes.
    pub fn ifd0_tag_bytes(&self, tag: u16, max: u32) -> Result<Option<Vec<u8>>, String> {
        let ifd = self.read_ifd(self.first_ifd)?;
        let Some(e) = ifd.get(tag) else { return Ok(None) };
        let size = Self::type_size(e.typ).ok_or("unknown TIFF type")?;
        if size.checked_mul(e.count).is_none_or(|t| t > max) {
            return Err(format!("tag {tag} too large"));
        }
        Ok(self.value_bytes(e, e.count))
    }

    /// EXIF `ColorSpace` and the interoperability index (see [`ColorSignal`]).
    pub fn color_signal(&self) -> ColorSignal {
        let mut out = ColorSignal::default();
        let Ok(ifd0) = self.read_ifd(self.first_ifd) else { return out };
        let Some(exif) = ifd0.get(EXIF_IFD).and_then(|e| self.uint(e)).and_then(|o| self.read_ifd(o).ok()) else {
            return out;
        };
        out.color_space = exif.get(EXIF_COLOR_SPACE).and_then(|e| self.uint(e));
        out.interop_index = exif
            .get(INTEROP_IFD)
            .and_then(|e| self.uint(e))
            .and_then(|o| self.read_ifd(o).ok())
            .and_then(|i| i.get(INTEROP_INDEX).and_then(|e| self.string(e)));
        out
    }
}

/// A raw TIFF/EXIF tag with its value bytes normalized to little-endian (for re-emitting
/// in exported files).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawTag {
    pub tag: u16,
    pub typ: u16,
    pub count: u32,
    pub data: Vec<u8>,
}

/// The directories exported metadata is copied from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExifDirs {
    /// IFD0 (camera make/model, artist, copyright, date).
    pub ifd0: Vec<RawTag>,
    /// EXIF sub-IFD (exposure, lens, capture time).
    pub exif: Vec<RawTag>,
    /// GPS sub-IFD.
    pub gps: Vec<RawTag>,
}

const GPS_IFD: u16 = 0x8825;
/// Values larger than this (maker notes, thumbnails) are never copied.
const MAX_RAW_TAG_BYTES: u32 = 64 * 1024;

impl<S: ByteSource + ?Sized> Tiff<'_, S> {
    /// Every entry of `ifd` with a known type and a value <= 64 KiB, values little-endian.
    fn raw_tags(&self, ifd: &Ifd) -> Vec<RawTag> {
        ifd.entries
            .iter()
            .filter_map(|e| {
                let size = Self::type_size(e.typ)?;
                let total = size.checked_mul(e.count)?;
                if total > MAX_RAW_TAG_BYTES {
                    return None;
                }
                let mut data = self.value_bytes(e, e.count)?;
                if !self.le {
                    let unit = match e.typ {
                        3 | 8 => 2,
                        4 | 9 | 11 | 13 | 5 | 10 => 4,
                        12 => 8,
                        _ => 1,
                    };
                    if unit > 1 {
                        data.chunks_exact_mut(unit).for_each(<[u8]>::reverse);
                    }
                }
                Some(RawTag { tag: e.tag, typ: e.typ, count: e.count, data })
            })
            .collect()
    }

    /// Raw tags of IFD0 only (e.g. a CR3 `CMT` box, whose IFD0 holds one directory).
    pub fn ifd0_tags(&self) -> Result<Vec<RawTag>, String> {
        Ok(self.raw_tags(&self.read_ifd(self.first_ifd)?))
    }

    /// IFD0 plus its EXIF and GPS sub-IFDs (missing/corrupt sub-IFDs are empty).
    pub fn exif_dirs(&self) -> Result<ExifDirs, String> {
        let ifd0 = self.read_ifd(self.first_ifd)?;
        let sub = |tag: u16| -> Vec<RawTag> {
            ifd0.get(tag)
                .and_then(|e| self.uint(e))
                .filter(|&o| o != 0 && o != self.first_ifd)
                .and_then(|o| self.read_ifd(o).ok())
                .map(|ifd| self.raw_tags(&ifd))
                .unwrap_or_default()
        };
        Ok(ExifDirs { ifd0: self.raw_tags(&ifd0), exif: sub(EXIF_IFD), gps: sub(GPS_IFD) })
    }
}

fn set<T>(slot: &mut Option<T>, value: Option<T>) {
    if slot.is_none() {
        *slot = value;
    }
}

/// Scans a TIFF held in memory (e.g. an EXIF APP1 payload or a CR3 CMT box).
pub fn scan_bytes(bytes: &[u8], jpegs: bool) -> Result<TiffScan, String> {
    Tiff::new(bytes)?.scan(jpegs)
}

// ---------------------------------------------------------------------------
// Test support: a tiny TIFF writer so container tests can build real files.
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod build {
    //! Little-endian TIFF builder for tests.

    /// A tag value.
    #[derive(Clone)]
    pub enum Val {
        Short(Vec<u16>),
        Long(Vec<u32>),
        Rational(u32, u32),
        Ascii(String),
        /// A LONG whose value is the absolute offset of another IFD (index into the
        /// builder's IFD list), patched at layout time.
        IfdRef(usize),
        /// LONG pointing at blob `n`; used for JPEG offsets.
        BlobOffset(usize),
    }

    /// One IFD: (tag, value) pairs; `next` = index of the next IFD in the chain.
    #[derive(Clone, Default)]
    pub struct IfdSpec {
        pub tags: Vec<(u16, Val)>,
        pub next: Option<usize>,
    }

    /// Lays out `ifds` (IFD 0 first) followed by `blobs`; returns (file, blob offsets).
    pub fn tiff(ifds: &[IfdSpec], blobs: &[Vec<u8>]) -> (Vec<u8>, Vec<u32>) {
        // Pass 1: compute IFD offsets and out-of-line data sizes.
        let mut ifd_offsets = Vec::new();
        let mut pos = 8u32;
        for ifd in ifds {
            ifd_offsets.push(pos);
            pos += 2 + 12 * ifd.tags.len() as u32 + 4;
            for (_, v) in &ifd.tags {
                let n = extra_len(v);
                pos += n + (n & 1);
            }
        }
        let mut blob_offsets = Vec::new();
        for b in blobs {
            blob_offsets.push(pos);
            pos += b.len() as u32;
        }

        // Pass 2: write.
        let mut out = b"II*\0".to_vec();
        out.extend_from_slice(&ifd_offsets.first().copied().unwrap_or(0).to_le_bytes());
        for (i, ifd) in ifds.iter().enumerate() {
            assert_eq!(out.len() as u32, ifd_offsets[i]);
            let mut tags = ifd.tags.clone();
            tags.sort_by_key(|(t, _)| *t);
            let mut extra = Vec::new();
            let extra_base = ifd_offsets[i] + 2 + 12 * tags.len() as u32 + 4;
            out.extend_from_slice(&(tags.len() as u16).to_le_bytes());
            for (tag, v) in &tags {
                out.extend_from_slice(&tag.to_le_bytes());
                let (typ, count, data): (u16, u32, Vec<u8>) = match v {
                    Val::Short(s) => (3, s.len() as u32, s.iter().flat_map(|x| x.to_le_bytes()).collect()),
                    Val::Long(l) => (4, l.len() as u32, l.iter().flat_map(|x| x.to_le_bytes()).collect()),
                    Val::Rational(n, d) => (5, 1, [n.to_le_bytes(), d.to_le_bytes()].concat()),
                    Val::Ascii(s) => {
                        let mut b = s.as_bytes().to_vec();
                        b.push(0);
                        (2, b.len() as u32, b)
                    }
                    Val::IfdRef(n) => (4, 1, ifd_offsets[*n].to_le_bytes().to_vec()),
                    Val::BlobOffset(n) => (4, 1, blob_offsets[*n].to_le_bytes().to_vec()),
                };
                out.extend_from_slice(&typ.to_le_bytes());
                out.extend_from_slice(&count.to_le_bytes());
                if data.len() <= 4 {
                    let mut inline = [0u8; 4];
                    inline[..data.len()].copy_from_slice(&data);
                    out.extend_from_slice(&inline);
                } else {
                    let off = extra_base + extra.len() as u32;
                    out.extend_from_slice(&off.to_le_bytes());
                    extra.extend_from_slice(&data);
                    if extra.len() & 1 == 1 {
                        extra.push(0);
                    }
                }
            }
            let next = ifd.next.map(|n| ifd_offsets[n]).unwrap_or(0);
            out.extend_from_slice(&next.to_le_bytes());
            out.extend_from_slice(&extra);
        }
        for b in blobs {
            out.extend_from_slice(b);
        }
        (out, blob_offsets)
    }

    fn extra_len(v: &Val) -> u32 {
        let n = match v {
            Val::Short(s) => s.len() as u32 * 2,
            Val::Long(l) => l.len() as u32 * 4,
            Val::Rational(..) => 8,
            Val::Ascii(s) => s.len() as u32 + 1,
            Val::IfdRef(_) | Val::BlobOffset(_) => 4,
        };
        if n > 4 {
            n
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::build::*;
    use super::*;

    #[test]
    fn reads_meta_and_jpegs_across_ifds() {
        let ifd0 = IfdSpec {
            tags: vec![
                (MAKE, Val::Ascii("SONY".into())),
                (MODEL, Val::Ascii("ILCE-7M4".into())),
                (ORIENTATION, Val::Short(vec![8])),
                (JPEG_OFFSET, Val::BlobOffset(0)),
                (JPEG_LENGTH, Val::Long(vec![3])),
                (EXIF_IFD, Val::IfdRef(1)),
                (SUB_IFDS, Val::IfdRef(3)),
            ],
            next: Some(2),
        };
        let exif = IfdSpec {
            tags: vec![
                (EXPOSURE_TIME, Val::Rational(1, 200)),
                (F_NUMBER, Val::Rational(14, 10)),
                (ISO, Val::Short(vec![125])),
                (DATE_TIME_ORIGINAL, Val::Ascii("2026:09:26 18:36:04".into())),
                (SUB_SEC_TIME_ORIGINAL, Val::Ascii("106".into())),
                (FOCAL_LENGTH, Val::Rational(850, 10)),
                (LENS_MODEL, Val::Ascii("85mm F1.4 DG DN | Art 020".into())),
                (PIXEL_X_DIMENSION, Val::Long(vec![4608])),
                (PIXEL_Y_DIMENSION, Val::Long(vec![3072])),
            ],
            next: None,
        };
        let ifd1 = IfdSpec {
            tags: vec![
                (MAKE, Val::Ascii("NOT-FIRST".into())),
                (JPEG_OFFSET, Val::BlobOffset(1)),
                (JPEG_LENGTH, Val::Long(vec![5])),
            ],
            next: None,
        };
        let sub = IfdSpec {
            tags: vec![
                (COMPRESSION, Val::Short(vec![7])),
                (STRIP_OFFSETS, Val::BlobOffset(2)),
                (STRIP_BYTE_COUNTS, Val::Long(vec![2])),
            ],
            next: None,
        };
        let (file, blobs) = tiff(&[ifd0, exif, ifd1, sub], &[vec![1, 2, 3], vec![4; 5], vec![9, 9]]);
        let scan = scan_bytes(&file, true).unwrap();
        let m = &scan.meta;
        assert_eq!(m.make.as_deref(), Some("SONY"));
        assert_eq!(m.model.as_deref(), Some("ILCE-7M4"));
        assert_eq!(m.orientation, Some(8));
        assert_eq!(m.exposure_time, Some(1.0 / 200.0));
        assert_eq!(m.f_number, Some(1.4));
        assert_eq!(m.iso, Some(125));
        assert_eq!(m.date_original.as_deref(), Some("2026:09:26 18:36:04"));
        assert_eq!(m.subsec_original.as_deref(), Some("106"));
        assert_eq!(m.focal_length, Some(85.0));
        assert_eq!(m.lens.as_deref(), Some("85mm F1.4 DG DN | Art 020"));
        assert_eq!((m.pixel_width, m.pixel_height), (Some(4608), Some(3072)));

        let mut j = scan.jpegs.clone();
        j.sort_by_key(|r| r.offset);
        assert_eq!(
            j,
            vec![
                JpegRef { offset: blobs[0] as u64, len: 3 },
                JpegRef { offset: blobs[1] as u64, len: 5 },
                JpegRef { offset: blobs[2] as u64, len: 2 },
            ]
        );

        // Metadata-only scans skip JPEG discovery.
        assert!(scan_bytes(&file, false).unwrap().jpegs.is_empty());
    }

    #[test]
    fn big_endian_and_garbage() {
        // MM header, 1 entry: Orientation = 6.
        let mut f = b"MM\0*\0\0\0\x08".to_vec();
        f.extend_from_slice(&[0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(scan_bytes(&f, true).unwrap().meta.orientation, Some(6));

        assert!(scan_bytes(b"nope", true).is_err());
        assert!(scan_bytes(b"II*\0\xff\xff\xff\x7f", true).is_err());
        // Self-referencing chain terminates.
        let mut cyc = b"II*\0\x08\0\0\0".to_vec();
        cyc.extend_from_slice(&[1, 0, 0x12, 0x01, 3, 0, 1, 0, 0, 0, 1, 0, 0, 0, 8, 0, 0, 0]);
        assert_eq!(scan_bytes(&cyc, true).unwrap().meta.orientation, Some(1));
    }
}
