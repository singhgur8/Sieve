//! Little-endian TIFF writing: IFD serialization (EXIF blobs for JPEG/PNG/WebP) and
//! baseline RGB TIFF files (8/16-bit, none/LZW/Deflate with horizontal predictor).
//!
//! Own writer rather than the `tiff` crate: exports need an EXIF sub-IFD, a GPS sub-IFD,
//! ICC (34675) and XMP (700) in IFD0, and strips compressed in parallel and streamed to
//! disk (bounded memory for 16-bit 61 MP files). Strips are written first, the IFDs last.

use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};

use rayon::prelude::*;

use crate::ipc::types::TiffCompression;
use crate::raw::tiff::RawTag;

pub const T_BYTE: u16 = 1;
pub const T_ASCII: u16 = 2;
pub const T_SHORT: u16 = 3;
pub const T_LONG: u16 = 4;
pub const T_RATIONAL: u16 = 5;
pub const T_UNDEFINED: u16 = 7;

// Tags written by the exporter.
pub const IMAGE_WIDTH: u16 = 256;
pub const IMAGE_LENGTH: u16 = 257;
pub const BITS_PER_SAMPLE: u16 = 258;
pub const COMPRESSION: u16 = 259;
pub const PHOTOMETRIC: u16 = 262;
pub const IMAGE_DESCRIPTION: u16 = 270;
pub const MAKE: u16 = 271;
pub const MODEL: u16 = 272;
pub const STRIP_OFFSETS: u16 = 273;
pub const ORIENTATION: u16 = 274;
pub const SAMPLES_PER_PIXEL: u16 = 277;
pub const ROWS_PER_STRIP: u16 = 278;
pub const STRIP_BYTE_COUNTS: u16 = 279;
pub const X_RESOLUTION: u16 = 282;
pub const Y_RESOLUTION: u16 = 283;
pub const PLANAR_CONFIG: u16 = 284;
pub const RESOLUTION_UNIT: u16 = 296;
pub const SOFTWARE: u16 = 305;
pub const DATE_TIME: u16 = 306;
pub const ARTIST: u16 = 315;
pub const PREDICTOR: u16 = 317;
pub const SAMPLE_FORMAT: u16 = 339;
pub const XMP: u16 = 700;
pub const COPYRIGHT: u16 = 33432;
pub const EXIF_IFD: u16 = 34665;
pub const ICC_PROFILE: u16 = 34675;
pub const GPS_IFD: u16 = 34853;
pub const EXIF_VERSION: u16 = 0x9000;
pub const COLOR_SPACE: u16 = 0xA001;
pub const PIXEL_X_DIMENSION: u16 = 0xA002;
pub const PIXEL_Y_DIMENSION: u16 = 0xA003;

pub fn ascii(tag: u16, s: &str) -> RawTag {
    let mut data: Vec<u8> = s.bytes().filter(|&b| b != 0).collect();
    data.push(0);
    RawTag { tag, typ: T_ASCII, count: data.len() as u32, data }
}

pub fn short(tag: u16, v: &[u16]) -> RawTag {
    RawTag { tag, typ: T_SHORT, count: v.len() as u32, data: v.iter().flat_map(|x| x.to_le_bytes()).collect() }
}

pub fn long(tag: u16, v: &[u32]) -> RawTag {
    RawTag { tag, typ: T_LONG, count: v.len() as u32, data: v.iter().flat_map(|x| x.to_le_bytes()).collect() }
}

pub fn rational(tag: u16, num: u32, den: u32) -> RawTag {
    RawTag { tag, typ: T_RATIONAL, count: 1, data: [num.to_le_bytes(), den.to_le_bytes()].concat() }
}

pub fn undefined(tag: u16, bytes: &[u8]) -> RawTag {
    RawTag { tag, typ: T_UNDEFINED, count: bytes.len() as u32, data: bytes.to_vec() }
}

pub fn bytes(tag: u16, bytes: &[u8]) -> RawTag {
    RawTag { tag, typ: T_BYTE, count: bytes.len() as u32, data: bytes.to_vec() }
}

/// IFD0 plus optional EXIF / GPS sub-IFDs (pointer tags are added on serialization).
#[derive(Debug, Clone, Default)]
pub struct Dirs {
    pub ifd0: Vec<RawTag>,
    pub exif: Vec<RawTag>,
    pub gps: Vec<RawTag>,
}

fn pad2(n: usize) -> usize {
    n + (n & 1)
}

/// Sorted by tag, first occurrence of each tag kept.
fn normalize(tags: &mut Vec<RawTag>) {
    let mut seen = std::collections::HashSet::new();
    tags.retain(|t| seen.insert(t.tag));
    tags.sort_by_key(|t| t.tag);
}

fn ifd_size(tags: &[RawTag]) -> usize {
    2 + 12 * tags.len() + 4 + tags.iter().filter(|t| t.data.len() > 4).map(|t| pad2(t.data.len())).sum::<usize>()
}

/// Appends an IFD (entries, next = 0, then out-of-line values) that starts at absolute
/// offset `at`.
fn write_ifd(out: &mut Vec<u8>, at: u32, tags: &[RawTag]) {
    let start = out.len();
    out.extend_from_slice(&(tags.len() as u16).to_le_bytes());
    let mut value_off = at as usize + 2 + 12 * tags.len() + 4;
    let mut values = Vec::new();
    for t in tags {
        out.extend_from_slice(&t.tag.to_le_bytes());
        out.extend_from_slice(&t.typ.to_le_bytes());
        out.extend_from_slice(&t.count.to_le_bytes());
        if t.data.len() <= 4 {
            let mut inline = [0u8; 4];
            inline[..t.data.len()].copy_from_slice(&t.data);
            out.extend_from_slice(&inline);
        } else {
            out.extend_from_slice(&(value_off as u32).to_le_bytes());
            values.extend_from_slice(&t.data);
            if t.data.len() & 1 == 1 {
                values.push(0);
            }
            value_off += pad2(t.data.len());
        }
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&values);
    debug_assert_eq!(out.len() - start, ifd_size(tags));
}

/// Serializes `dirs` as IFD0 (at absolute offset `start`, even) followed by the EXIF and
/// GPS IFDs.
pub fn serialize_dirs(dirs: &Dirs, start: u32) -> Vec<u8> {
    let mut ifd0 = dirs.ifd0.clone();
    ifd0.retain(|t| t.tag != EXIF_IFD && t.tag != GPS_IFD);
    let mut exif = dirs.exif.clone();
    let mut gps = dirs.gps.clone();
    normalize(&mut exif);
    normalize(&mut gps);
    if !exif.is_empty() {
        ifd0.push(long(EXIF_IFD, &[0]));
    }
    if !gps.is_empty() {
        ifd0.push(long(GPS_IFD, &[0]));
    }
    normalize(&mut ifd0);
    let exif_at = start + ifd_size(&ifd0) as u32;
    let gps_at = exif_at + if exif.is_empty() { 0 } else { ifd_size(&exif) as u32 };
    for t in ifd0.iter_mut() {
        match t.tag {
            EXIF_IFD => *t = long(EXIF_IFD, &[exif_at]),
            GPS_IFD => *t = long(GPS_IFD, &[gps_at]),
            _ => {}
        }
    }
    let mut out = Vec::new();
    write_ifd(&mut out, start, &ifd0);
    if !exif.is_empty() {
        write_ifd(&mut out, exif_at, &exif);
    }
    if !gps.is_empty() {
        write_ifd(&mut out, gps_at, &gps);
    }
    out
}

/// A standalone EXIF TIFF (header + IFDs), as embedded in JPEG APP1 (after `Exif\0\0`),
/// PNG `eXIf` and WebP `EXIF`.
pub fn exif_blob(dirs: &Dirs) -> Vec<u8> {
    let mut out = b"II*\0\x08\0\0\0".to_vec();
    out.extend_from_slice(&serialize_dirs(dirs, 8));
    out
}

/// Pixel data for [`write_tiff`].
pub enum Samples<'a> {
    U8(&'a [u8]),
    U16(&'a [u16]),
}

/// Rows per strip: ~256 KiB of uncompressed data (compressed in parallel).
fn rows_per_strip(row_bytes: usize) -> usize {
    (256 * 1024 / row_bytes.max(1)).max(1)
}

fn strip_bytes(samples: &Samples, w: usize, rows: std::ops::Range<usize>, predictor: bool) -> Vec<u8> {
    let n = w * 3;
    match samples {
        Samples::U8(px) => {
            let mut out = px[rows.start * n..rows.end * n].to_vec();
            if predictor {
                for row in out.chunks_exact_mut(n) {
                    for i in (3..n).rev() {
                        row[i] = row[i].wrapping_sub(row[i - 3]);
                    }
                }
            }
            out
        }
        Samples::U16(px) => {
            let src = &px[rows.start * n..rows.end * n];
            let mut out = Vec::with_capacity(src.len() * 2);
            for row in src.chunks_exact(n) {
                if predictor {
                    out.extend_from_slice(&row[0].to_le_bytes());
                    out.extend_from_slice(&row[1].to_le_bytes());
                    out.extend_from_slice(&row[2].to_le_bytes());
                    for i in 3..n {
                        out.extend_from_slice(&row[i].wrapping_sub(row[i - 3]).to_le_bytes());
                    }
                } else {
                    for v in row {
                        out.extend_from_slice(&v.to_le_bytes());
                    }
                }
            }
            out
        }
    }
}

fn compress(data: Vec<u8>, compression: TiffCompression) -> io::Result<Vec<u8>> {
    match compression {
        TiffCompression::None => Ok(data),
        TiffCompression::Lzw => weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8)
            .encode(&data)
            .map_err(|e| io::Error::other(format!("LZW: {e}"))),
        TiffCompression::Zip => {
            let mut enc =
                flate2::write::ZlibEncoder::new(Vec::with_capacity(data.len() / 2), flate2::Compression::new(6));
            enc.write_all(&data)?;
            enc.finish()
        }
    }
}

/// Writes an RGB TIFF to `file`: strips first (compressed in parallel batches, streamed),
/// then IFD0 = technical tags + `meta.ifd0` (+ EXIF/GPS sub-IFDs), resolution in inches.
pub fn write_tiff(
    file: File,
    width: u32,
    height: u32,
    samples: Samples,
    compression: TiffCompression,
    ppi: u32,
    meta: &Dirs,
) -> io::Result<File> {
    let (w, h) = (width as usize, height as usize);
    let bps: u16 = match samples {
        Samples::U8(_) => 8,
        Samples::U16(_) => 16,
    };
    let row_bytes = w * 3 * usize::from(bps / 8);
    let rps = rows_per_strip(row_bytes);
    let strips = h.div_ceil(rps);
    let predictor = compression != TiffCompression::None;

    let mut out = BufWriter::with_capacity(1 << 20, file);
    out.write_all(b"II*\0\0\0\0\0")?;
    let mut pos: u64 = 8;
    let mut offsets = Vec::with_capacity(strips);
    let mut counts = Vec::with_capacity(strips);
    let batch = rayon::current_num_threads().max(1) * 2;
    for first in (0..strips).step_by(batch) {
        let last = (first + batch).min(strips);
        let done: Vec<io::Result<Vec<u8>>> = (first..last)
            .into_par_iter()
            .map(|s| {
                let rows = s * rps..((s + 1) * rps).min(h);
                compress(strip_bytes(&samples, w, rows, predictor), compression)
            })
            .collect();
        for data in done {
            let data = data?;
            offsets.push(pos);
            counts.push(data.len() as u64);
            out.write_all(&data)?;
            pos += data.len() as u64;
        }
    }
    if pos & 1 == 1 {
        out.write_all(&[0])?;
        pos += 1;
    }
    let too_big = || io::Error::other("TIFF larger than 4 GB (classic TIFF limit)");
    let to_u32 =
        |v: &[u64]| v.iter().map(|&x| u32::try_from(x).map_err(|_| too_big())).collect::<io::Result<Vec<u32>>>();
    let (offsets, counts) = (to_u32(&offsets)?, to_u32(&counts)?);
    let ifd_at = u32::try_from(pos).map_err(|_| too_big())?;

    let code = match compression {
        TiffCompression::None => 1,
        TiffCompression::Lzw => 5,
        TiffCompression::Zip => 8,
    };
    let mut ifd0 = vec![
        long(IMAGE_WIDTH, &[width]),
        long(IMAGE_LENGTH, &[height]),
        short(BITS_PER_SAMPLE, &[bps; 3]),
        short(COMPRESSION, &[code]),
        short(PHOTOMETRIC, &[2]),
        long(STRIP_OFFSETS, &offsets),
        short(ORIENTATION, &[1]),
        short(SAMPLES_PER_PIXEL, &[3]),
        long(ROWS_PER_STRIP, &[rps as u32]),
        long(STRIP_BYTE_COUNTS, &counts),
        rational(X_RESOLUTION, ppi, 1),
        rational(Y_RESOLUTION, ppi, 1),
        short(PLANAR_CONFIG, &[1]),
        short(RESOLUTION_UNIT, &[2]),
        short(SAMPLE_FORMAT, &[1; 3]),
    ];
    if predictor {
        ifd0.push(short(PREDICTOR, &[2]));
    }
    ifd0.extend(meta.ifd0.iter().cloned());
    let dirs = Dirs { ifd0, exif: meta.exif.clone(), gps: meta.gps.clone() };
    let ifds = serialize_dirs(&dirs, ifd_at);
    if u64::from(ifd_at) + ifds.len() as u64 > u64::from(u32::MAX) {
        return Err(too_big());
    }
    out.write_all(&ifds)?;
    let mut file = out.into_inner().map_err(|e| e.into_error())?;
    file.seek(SeekFrom::Start(4))?;
    file.write_all(&ifd_at.to_le_bytes())?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::tiff::Tiff;

    #[test]
    fn exif_blob_round_trips_through_the_reader() {
        let dirs = Dirs {
            ifd0: vec![ascii(MAKE, "SONY"), short(ORIENTATION, &[1]), ascii(COPYRIGHT, "(c) Me")],
            exif: vec![rational(0x829A, 1, 250), short(0x8827, &[400]), ascii(0x9003, "2024:06:15 14:22:33")],
            gps: vec![RawTag { tag: 1, typ: T_ASCII, count: 2, data: b"N\0".to_vec() }],
        };
        let blob = exif_blob(&dirs);
        let t = Tiff::new(blob.as_slice()).unwrap();
        let read = t.exif_dirs().unwrap();
        assert!(read.ifd0.iter().any(|t| t.tag == MAKE && t.data == b"SONY\0"));
        assert!(read.ifd0.iter().any(|t| t.tag == EXIF_IFD));
        assert_eq!(read.exif.len(), 3);
        assert_eq!(read.gps.len(), 1);
        let scan = t.scan(false).unwrap().meta.finish();
        assert_eq!(scan.iso, Some(400));
        assert_eq!(scan.shutter_seconds, Some(1.0 / 250.0));
    }

    #[test]
    fn tiff_files_decode_with_every_compression() {
        let (w, h) = (37u32, 23u32);
        let px16: Vec<u16> = (0..w * h * 3).map(|i| (i * 977 % 65536) as u16).collect();
        let px8: Vec<u8> = px16.iter().map(|v| (v >> 8) as u8).collect();
        let dir = tempfile::tempdir().unwrap();
        for c in [TiffCompression::None, TiffCompression::Lzw, TiffCompression::Zip] {
            for sixteen in [false, true] {
                let path = dir.path().join(format!("t{c:?}{sixteen}.tif"));
                let f = File::create(&path).unwrap();
                let samples = if sixteen { Samples::U16(&px16) } else { Samples::U8(&px8) };
                let meta = Dirs { ifd0: vec![ascii(SOFTWARE, "Sieve")], ..Default::default() };
                write_tiff(f, w, h, samples, c, 300, &meta).unwrap();
                // Decode strips back (predictor undone) and compare.
                let bytes = std::fs::read(&path).unwrap();
                let t = Tiff::new(bytes.as_slice()).unwrap();
                let d = t.ifd0_tags().unwrap();
                let get = |tag: u16| d.iter().find(|t| t.tag == tag).unwrap().clone();
                let longs = |t: RawTag| -> Vec<u32> {
                    t.data.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect()
                };
                let (offs, counts) = (longs(get(STRIP_OFFSETS)), longs(get(STRIP_BYTE_COUNTS)));
                let mut raw = Vec::new();
                for (o, n) in offs.iter().zip(&counts) {
                    let s = &bytes[*o as usize..(*o + *n) as usize];
                    let dec = match c {
                        TiffCompression::None => s.to_vec(),
                        TiffCompression::Lzw => {
                            weezl::decode::Decoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8).decode(s).unwrap()
                        }
                        TiffCompression::Zip => {
                            let mut v = Vec::new();
                            std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(s), &mut v).unwrap();
                            v
                        }
                    };
                    raw.extend(dec);
                }
                let n = w as usize * 3;
                if sixteen {
                    let mut v: Vec<u16> = raw.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
                    if c != TiffCompression::None {
                        for row in v.chunks_exact_mut(n) {
                            for i in 3..n {
                                row[i] = row[i].wrapping_add(row[i - 3]);
                            }
                        }
                    }
                    assert_eq!(v, px16, "{c:?} 16");
                } else {
                    if c != TiffCompression::None {
                        for row in raw.chunks_exact_mut(n) {
                            for i in 3..n {
                                row[i] = row[i].wrapping_add(row[i - 3]);
                            }
                        }
                    }
                    assert_eq!(raw, px8, "{c:?} 8");
                }
                assert!(d.iter().any(|t| t.tag == SOFTWARE));
            }
        }
    }
}
