//! RAW file handling: identification (extension + magic bytes) and, for ingest,
//! embedded-preview + EXIF extraction.
//!
//! Extraction parses containers directly (fast path, reads ~1 MB of a 25+ MB file):
//! - ARW: TIFF IFDs (`tiff`), JPEGs via JPEGInterchangeFormat in IFD0/IFD1/IFD2/SubIFDs.
//! - RAF: fixed header -> embedded JPEG (EXIF in its APP1) + RAF directory (`raf`).
//! - CR3: ISO-BMFF boxes, CMT1/CMT2 TIFFs, full-size JPEG track, PRVW, THMB (`cr3`).
//!
//! Orientation always comes from the container metadata (ARW IFD0, CR3 CMT1, RAF EXIF),
//! never from the embedded JPEG itself (Sony previews carry no usable Orientation).
//! If no usable embedded JPEG is found, LibRaw's `unpack_thumb` is the fallback.

pub mod cr3;
pub mod jpeg;
pub mod libraw;
pub mod meta;
pub mod preview;
pub mod raf;
pub mod source;
pub mod tiff;
pub mod turbo;

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::ipc::types::{CameraMake, RawFormat, SensorLayout};
use meta::{ImageMeta, MetaBuilder};
use preview::PREVIEW_EDGE;
use source::{ByteSource, FileSource};
use tiff::JpegRef;

/// Maps a file extension (case-insensitive) to a supported format.
pub fn format_from_extension(path: &Path) -> Option<RawFormat> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "arw" => Some(RawFormat::Arw),
        "raf" => Some(RawFormat::Raf),
        "cr3" => Some(RawFormat::Cr3),
        _ => None,
    }
}

/// Checks the file header matches the container the extension claims.
pub fn header_matches(format: RawFormat, header: &[u8]) -> bool {
    match format {
        // Sony ARW is little-endian TIFF.
        RawFormat::Arw => header.starts_with(b"II*\0"),
        RawFormat::Raf => header.starts_with(b"FUJIFILMCCD-RAW"),
        // Canon CR3 is ISO-BMFF: 4-byte box size, then `ftyp` with brand `crx `.
        RawFormat::Cr3 => header.get(4..12) == Some(b"ftypcrx ".as_slice()),
    }
}

/// Returns the format if `path` has a supported extension *and* a valid header.
/// `Ok(None)` = not a RAW we handle; `Err` = RAW extension but bad/unreadable header.
pub fn identify(path: &Path) -> Result<Option<RawFormat>, String> {
    let Some(format) = format_from_extension(path) else {
        return Ok(None);
    };
    let mut header = [0u8; 16];
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let n = file.read(&mut header).map_err(|e| e.to_string())?;
    if header_matches(format, &header[..n]) {
        Ok(Some(format))
    } else {
        Err(format!("{} header does not match .{}", path.display(), format.as_str()))
    }
}

/// Camera make implied by the container, before EXIF is read.
pub fn default_make(format: RawFormat) -> CameraMake {
    match format {
        RawFormat::Arw => CameraMake::Sony,
        RawFormat::Raf => CameraMake::Fujifilm,
        RawFormat::Cr3 => CameraMake::Canon,
    }
}

/// Sensor layout implied by the container. Fuji ships both X-Trans and Bayer
/// bodies, so it stays `Unknown` until the model is read in Phase 2.
pub fn default_sensor_layout(format: RawFormat) -> SensorLayout {
    match format {
        RawFormat::Arw | RawFormat::Cr3 => SensorLayout::Bayer,
        RawFormat::Raf => SensorLayout::Unknown,
    }
}

/// What a container parser found: raw metadata and embedded JPEG candidates.
#[derive(Debug, Default)]
pub struct Container {
    pub meta: MetaBuilder,
    pub jpegs: Vec<JpegRef>,
}

/// Parses the container for `format`.
pub fn parse_container(src: &(impl ByteSource + ?Sized), format: RawFormat) -> Result<Container, String> {
    match format {
        RawFormat::Arw => {
            let scan = tiff::Tiff::new(src)?.scan(true)?;
            Ok(Container { meta: scan.meta, jpegs: scan.jpegs })
        }
        RawFormat::Raf => raf::parse(src),
        RawFormat::Cr3 => cr3::parse(src),
    }
}

/// Raw EXIF directories (IFD0, EXIF, GPS) of a RAW, values little-endian, for copying into
/// exported files. ARW: the file's own IFDs; RAF: the embedded JPEG's EXIF; CR3: CMT1/2/4.
pub fn exif_dirs(path: &Path) -> Result<tiff::ExifDirs, String> {
    let format = format_from_extension(path).ok_or_else(|| format!("{}: not a supported RAW", path.display()))?;
    let src = FileSource::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    match format {
        RawFormat::Arw => tiff::Tiff::new(&src)?.exif_dirs(),
        RawFormat::Raf => raf::exif_dirs(&src),
        RawFormat::Cr3 => cr3::exif_dirs(&src),
    }
}

/// Upper bound for an embedded JPEG we are willing to read.
const MAX_JPEG_BYTES: u64 = 64 << 20;
/// Prefix read to validate a JPEG candidate and find its frame size.
const JPEG_PROBE: usize = 64 * 1024;

/// A validated embedded JPEG.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddedJpeg {
    pub at: JpegRef,
    pub width: u16,
    pub height: u16,
}

/// Validates candidates and picks the cheapest one that still covers the preview size:
/// the smallest with long edge >= `PREVIEW_EDGE`, else the largest.
pub fn best_jpeg(src: &(impl ByteSource + ?Sized), candidates: &[JpegRef]) -> Option<EmbeddedJpeg> {
    let mut valid = Vec::new();
    for &at in candidates {
        if at.len < 4 || at.len > MAX_JPEG_BYTES || at.offset.checked_add(at.len).is_none_or(|e| e > src.len()) {
            continue;
        }
        let Ok(mut probe) = src.read_vec(at.offset, (at.len as usize).min(JPEG_PROBE)) else { continue };
        let mut hdr = jpeg::header(&probe);
        if hdr == jpeg::Header::Truncated && at.len as usize > probe.len() {
            let Ok(full) = src.read_vec(at.offset, at.len as usize) else { continue };
            probe = full;
            hdr = jpeg::header(&probe);
        }
        if let jpeg::Header::Frame { width, height } = hdr {
            valid.push(EmbeddedJpeg { at, width, height });
        }
    }
    let long = |j: &EmbeddedJpeg| j.width.max(j.height) as u32;
    valid
        .iter()
        .filter(|j| long(j) >= PREVIEW_EDGE)
        .min_by_key(|j| long(j))
        .or_else(|| valid.iter().max_by_key(|j| long(j)))
        .copied()
}

/// Where the preview pixels ended up.
pub enum Preview {
    /// The embedded JPEG was read into the caller's buffer.
    Embedded,
    /// LibRaw fallback output.
    Libraw(libraw::Thumb),
}

impl Preview {
    /// Borrows the pixels as a render source; `buf` is the buffer passed to [`extract`].
    pub fn source<'a>(&'a self, buf: &'a [u8]) -> preview::Source<'a> {
        match self {
            Preview::Embedded => preview::Source::Jpeg(buf),
            Preview::Libraw(libraw::Thumb::Jpeg(bytes)) => preview::Source::Jpeg(bytes),
            Preview::Libraw(libraw::Thumb::Rgb { width, height, pixels }) => {
                preview::Source::Rgb(preview::View { width: *width, height: *height, pixels })
            }
        }
    }
}

/// Everything ingest needs from one RAW file.
pub struct Extracted {
    pub meta: ImageMeta,
    /// Preview pixels, or why none could be obtained.
    pub preview: Result<Preview, String>,
}

/// Reads metadata and the best embedded preview from `path`. The embedded JPEG is read
/// into `jpeg_buf` (capacity reused across calls).
/// `Err` only if the file cannot be opened at all; parse problems degrade to
/// empty metadata and/or the LibRaw fallback for the preview.
pub fn extract(path: &Path, format: RawFormat, jpeg_buf: &mut Vec<u8>) -> Result<Extracted, String> {
    let src = FileSource::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let (builder, parse_err, jpeg) = match parse_container(&src, format) {
        Ok(c) => {
            let best = best_jpeg(&src, &c.jpegs);
            (c.meta, None, best)
        }
        Err(e) => (MetaBuilder::default(), Some(e), None),
    };
    let mut meta = builder.finish();
    if meta.make.is_none() {
        meta.make = Some(default_make(format));
    }

    let direct = match jpeg {
        Some(j) => {
            jpeg_buf.clear();
            jpeg_buf.resize(j.at.len as usize, 0);
            src.read_at(j.at.offset, jpeg_buf).map(|_| Preview::Embedded).map_err(|e| e.to_string())
        }
        None => Err(parse_err.unwrap_or_else(|| "no embedded JPEG preview".into())),
    };
    drop(src);
    let preview = direct.or_else(|direct_err| match libraw::thumbnail(path) {
        Ok(thumb) => Ok(Preview::Libraw(thumb)),
        Err(e) => Err(format!("{direct_err}; {e}")),
    });
    Ok(Extracted { meta, preview })
}

#[cfg(test)]
pub(crate) mod test_fixtures {
    use super::*;

    /// Minimal header bytes that pass `header_matches` for each format.
    pub fn stub_header(format: RawFormat) -> Vec<u8> {
        match format {
            RawFormat::Arw => b"II*\0\x08\0\0\0".to_vec(),
            RawFormat::Raf => b"FUJIFILMCCD-RAW 0201".to_vec(),
            RawFormat::Cr3 => b"\0\0\0\x18ftypcrx \0\0\0\x01".to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_fixtures::stub_header;
    use super::*;

    #[test]
    fn extension_is_case_insensitive() {
        assert_eq!(format_from_extension(Path::new("a/DSC0001.ARW")), Some(RawFormat::Arw));
        assert_eq!(format_from_extension(Path::new("DSCF1.raf")), Some(RawFormat::Raf));
        assert_eq!(format_from_extension(Path::new("IMG_1.Cr3")), Some(RawFormat::Cr3));
        assert_eq!(format_from_extension(Path::new("IMG_1.jpg")), None);
        assert_eq!(format_from_extension(Path::new("noext")), None);
    }

    #[test]
    fn magic_bytes() {
        for &f in RawFormat::ALL {
            assert!(header_matches(f, &stub_header(f)), "{f:?}");
        }
        assert!(!header_matches(RawFormat::Arw, b"MM\0*"));
        assert!(!header_matches(RawFormat::Raf, &stub_header(RawFormat::Arw)));
        assert!(!header_matches(RawFormat::Cr3, b"\0\0\0\x18ftypheic"));
        assert!(!header_matches(RawFormat::Cr3, b"short"));
    }

    #[test]
    fn identify_reads_files() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("a.cr3");
        let bad = dir.path().join("b.arw");
        let other = dir.path().join("c.xmp");
        std::fs::write(&good, stub_header(RawFormat::Cr3)).unwrap();
        std::fs::write(&bad, b"not a raw").unwrap();
        std::fs::write(&other, b"<x/>").unwrap();

        assert_eq!(identify(&good), Ok(Some(RawFormat::Cr3)));
        assert!(identify(&bad).is_err());
        assert_eq!(identify(&other), Ok(None));
    }
}
