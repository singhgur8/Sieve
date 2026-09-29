//! RAW file identification. Decoding (LibRaw) arrives in Phase 2; this module only
//! decides which files are importable, from the extension plus header magic bytes.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::ipc::types::{CameraMake, RawFormat, SensorLayout};

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
