//! JPEG marker-level helpers: frame size without decoding, EXIF APP1 lookup.

/// Outcome of scanning a JPEG's header markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Header {
    /// A baseline/extended/progressive DCT frame of this size.
    Frame { width: u16, height: u16 },
    /// Not a decodable preview (bad SOI, lossless/arithmetic frame, garbage).
    Invalid,
    /// The buffer ended before the frame header; read more bytes.
    Truncated,
}

/// Walks markers from SOI to the first SOF.
pub fn header(bytes: &[u8]) -> Header {
    if bytes.len() < 4 {
        return Header::Truncated;
    }
    if bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return Header::Invalid;
    }
    let mut pos = 2;
    loop {
        // Skip to the marker byte (allow fill 0xFF bytes).
        match bytes.get(pos) {
            None => return Header::Truncated,
            Some(0xFF) => {}
            Some(_) => return Header::Invalid,
        }
        while bytes.get(pos) == Some(&0xFF) {
            pos += 1;
        }
        let Some(&marker) = bytes.get(pos) else { return Header::Truncated };
        pos += 1;
        match marker {
            // Standalone markers (TEM, RSTn) carry no length.
            0x01 | 0xD0..=0xD7 => continue,
            0xD8 | 0xD9 | 0xDA | 0x00 => return Header::Invalid,
            _ => {}
        }
        let Some(len) = bytes.get(pos..pos + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as usize) else {
            return Header::Truncated;
        };
        if len < 2 {
            return Header::Invalid;
        }
        match marker {
            // SOF0 baseline, SOF1 extended, SOF2 progressive (Huffman): decodable.
            0xC0..=0xC2 => {
                let Some(f) = bytes.get(pos + 2..pos + 8) else { return Header::Truncated };
                let (precision, components) = (f[0], f[5]);
                let (height, width) = (u16::from_be_bytes([f[1], f[2]]), u16::from_be_bytes([f[3], f[4]]));
                // Only 8-bit grey / YCbCr previews are supported by the decoder path.
                let ok = width > 0 && height > 0 && precision == 8 && matches!(components, 1 | 3);
                return if ok { Header::Frame { width, height } } else { Header::Invalid };
            }
            // Lossless / arithmetic / hierarchical frames: raw data, not a preview.
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return Header::Invalid,
            _ => pos += len,
        }
    }
}

/// The TIFF payload of the first `APP1 Exif\0\0` segment, if any.
pub fn exif_tiff(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut pos = 2;
    loop {
        if *bytes.get(pos)? != 0xFF {
            return None;
        }
        let marker = *bytes.get(pos + 1)?;
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        let len = u16::from_be_bytes([*bytes.get(pos + 2)?, *bytes.get(pos + 3)?]) as usize;
        let body = bytes.get(pos + 4..pos + 2 + len)?;
        if marker == 0xE1 && body.starts_with(b"Exif\0\0") {
            return Some(&body[6..]);
        }
        pos += 2 + len;
    }
}

/// Marker segments `(marker, body)` from SOI up to (excluding) SOS, e.g. APP1/APP2.
pub fn segments(bytes: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
    let mut pos = if bytes.starts_with(&[0xFF, 0xD8]) { 2 } else { bytes.len() };
    std::iter::from_fn(move || {
        while bytes.get(pos) == Some(&0xFF) && bytes.get(pos + 1) == Some(&0xFF) {
            pos += 1;
        }
        if *bytes.get(pos)? != 0xFF {
            return None;
        }
        let marker = *bytes.get(pos + 1)?;
        if matches!(marker, 0xDA | 0xD9) {
            return None;
        }
        if matches!(marker, 0x01 | 0xD0..=0xD7) {
            pos += 2;
            return Some((marker, &bytes[0..0]));
        }
        let len = u16::from_be_bytes([*bytes.get(pos + 2)?, *bytes.get(pos + 3)?]) as usize;
        let body = bytes.get(pos + 4..pos + 2 + len.max(2))?;
        pos += 2 + len.max(2);
        Some((marker, body))
    })
}

#[cfg(test)]
pub(crate) mod test_support {
    /// Encodes a small RGB test JPEG with a distinct pattern per quadrant so tests can
    /// check orientation: top-left red, top-right green, bottom-left blue, bottom-right white.
    pub fn quadrant_jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut px = Vec::with_capacity(width as usize * height as usize * 3);
        for y in 0..height {
            for x in 0..width {
                let c = match (x < width / 2, y < height / 2) {
                    (true, true) => [255, 0, 0],
                    (false, true) => [0, 255, 0],
                    (true, false) => [0, 0, 255],
                    (false, false) => [255, 255, 255],
                };
                px.extend_from_slice(&c);
            }
        }
        crate::raw::turbo::encode_rgb(&px, width as u32, height as u32, 90).unwrap()
    }

    /// Inserts an `APP1 Exif` segment carrying `tiff` right after SOI.
    pub fn with_exif(jpeg: &[u8], tiff: &[u8]) -> Vec<u8> {
        let mut out = jpeg[..2].to_vec();
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
        out.extend_from_slice(b"Exif\0\0");
        out.extend_from_slice(tiff);
        out.extend_from_slice(&jpeg[2..]);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    #[test]
    fn frame_size_and_exif() {
        let j = quadrant_jpeg(64, 48);
        assert_eq!(header(&j), Header::Frame { width: 64, height: 48 });
        assert_eq!(header(&j[..10]), Header::Truncated);
        assert_eq!(header(b"\x00\x01\x02\x03"), Header::Invalid);
        // Lossless (SOF3) frames are raw data, not previews.
        assert_eq!(header(b"\xFF\xD8\xFF\xC3\x00\x0B\x08\x00\x10\x00\x10\x01"), Header::Invalid);

        assert_eq!(exif_tiff(&j), None);
        let tagged = with_exif(&j, b"II*\0\x08\0\0\0");
        assert_eq!(exif_tiff(&tagged), Some(b"II*\0\x08\0\0\0".as_slice()));
        assert_eq!(header(&tagged), Header::Frame { width: 64, height: 48 });
    }
}
