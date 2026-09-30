//! PNG sources (Phase 7b): chunk scan for metadata (`IHDR` size, `eXIf`, XMP in
//! `iTXt`/`tEXt`/`zTXt` `XML:com.adobe.xmp`, `iCCP`, `sRGB`) and a full decode to 8/16-bit
//! samples with the `png` crate (palette/grey expanded, alpha dropped).

use std::io::Read;
use std::path::Path;

use super::source::ByteSource;

/// Metadata chunks of a PNG.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct PngMeta {
    pub width: u32,
    pub height: u32,
    pub exif: Option<Vec<u8>>,
    pub xmp: Option<String>,
    pub icc: Option<Vec<u8>>,
    /// An `sRGB` chunk is present.
    pub srgb: bool,
}

const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const XMP_KEYWORD: &[u8] = b"XML:com.adobe.xmp";
/// Metadata chunks larger than this are skipped.
const MAX_CHUNK: usize = 16 << 20;

fn inflate(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(data).take(MAX_CHUNK as u64).read_to_end(&mut out).ok()?;
    Some(out)
}

/// Scans the chunk list, reading only metadata chunk payloads (pixels are skipped).
pub fn scan(src: &(impl ByteSource + ?Sized)) -> Result<PngMeta, String> {
    let mut sig = [0u8; 8];
    if src.read_at(0, &mut sig).is_err() || &sig != SIGNATURE {
        return Err("not a PNG".into());
    }
    let mut m = PngMeta::default();
    let mut pos = 8u64;
    let mut hdr = [0u8; 8];
    for _ in 0..100_000 {
        if src.read_at(pos, &mut hdr).is_err() {
            break;
        }
        let len = u32::from_be_bytes(hdr[..4].try_into().unwrap_or_default()) as usize;
        let typ: [u8; 4] = hdr[4..8].try_into().unwrap_or_default();
        let data_at = pos + 8;
        pos = data_at + len as u64 + 4;
        let wanted = matches!(&typ, b"IHDR" | b"eXIf" | b"sRGB" | b"iCCP" | b"iTXt" | b"tEXt" | b"zTXt");
        if &typ == b"IEND" {
            break;
        }
        if !wanted || len > MAX_CHUNK {
            continue;
        }
        let Ok(data) = src.read_vec(data_at, len) else { break };
        let data = data.as_slice();
        let is_xmp = data.starts_with(XMP_KEYWORD) && data.get(XMP_KEYWORD.len()) == Some(&0);
        match &typ {
            b"IHDR" if len >= 8 => {
                m.width = u32::from_be_bytes(data[..4].try_into().unwrap_or_default());
                m.height = u32::from_be_bytes(data[4..8].try_into().unwrap_or_default());
            }
            b"eXIf" => m.exif = Some(data.to_vec()),
            b"sRGB" => m.srgb = true,
            b"iCCP" => {
                if let Some(n) = data.iter().position(|&c| c == 0) {
                    // name \0 method(0 = zlib) data
                    m.icc = data.get(n + 2..).and_then(inflate);
                }
            }
            b"iTXt" if is_xmp => {
                let rest = &data[XMP_KEYWORD.len() + 1..];
                let (Some(&compressed), Some(_method)) = (rest.first(), rest.get(1)) else { continue };
                let rest = &rest[2..];
                // language tag \0 translated keyword \0 text
                let Some(a) = rest.iter().position(|&c| c == 0) else { continue };
                let Some(b) = rest[a + 1..].iter().position(|&c| c == 0) else { continue };
                let text = &rest[a + 1 + b + 1..];
                let text = if compressed == 1 { inflate(text) } else { Some(text.to_vec()) };
                m.xmp = text.map(|t| String::from_utf8_lossy(&t).into_owned());
            }
            b"tEXt" if is_xmp => {
                m.xmp = Some(String::from_utf8_lossy(&data[XMP_KEYWORD.len() + 1..]).into_owned());
            }
            b"zTXt" if is_xmp => {
                m.xmp = data
                    .get(XMP_KEYWORD.len() + 2..)
                    .and_then(inflate)
                    .map(|t| String::from_utf8_lossy(&t).into_owned());
            }
            _ => {}
        }
    }
    if m.width == 0 || m.height == 0 {
        return Err("PNG without IHDR".into());
    }
    Ok(m)
}

/// Decoded PNG samples: interleaved RGB (grey is expanded to RGB), 8 or 16 bits.
pub enum Samples {
    U8(Vec<u8>),
    U16(Vec<u16>),
}

pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub samples: Samples,
    pub bit_depth: u8,
}

/// Decodes the whole image (alpha dropped; palette/low-bit grey expanded to 8 bits).
pub fn decode(path: &Path) -> Result<Decoded, String> {
    let file = std::fs::File::open(path).map_err(|e| crate::raw::access::io_message(path, "read", &e))?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().map_err(|e| format!("PNG: {e}"))?;
    let (width, height) = (reader.info().width, reader.info().height);
    if u64::from(width) * u64::from(height) > 400_000_000 {
        return Err("PNG too large".into());
    }
    let size = reader.output_buffer_size().ok_or("PNG: output too large")?;
    let mut buf = vec![0u8; size];
    let frame = reader.next_frame(&mut buf).map_err(|e| format!("PNG: {e}"))?;
    let (color, depth) = (frame.color_type, frame.bit_depth);
    let channels = match color {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err("PNG: palette not expanded".into()),
    };
    let wide = depth == png::BitDepth::Sixteen;
    let n = width as usize * height as usize;
    let line = frame.line_size;
    let bpc = if wide { 2 } else { 1 };
    let pick = |x: usize, c: usize| -> usize {
        // Channel index of RGB component c for a pixel with `channels` samples.
        let src_c = if channels <= 2 { 0 } else { c };
        x * channels + src_c
    };
    let samples = if wide {
        let mut out = vec![0u16; n * 3];
        for y in 0..height as usize {
            let row = &buf[y * line..y * line + width as usize * channels * bpc];
            for x in 0..width as usize {
                for c in 0..3 {
                    let i = pick(x, c) * 2;
                    out[(y * width as usize + x) * 3 + c] = u16::from_be_bytes([row[i], row[i + 1]]);
                }
            }
        }
        Samples::U16(out)
    } else {
        let mut out = vec![0u8; n * 3];
        for y in 0..height as usize {
            let row = &buf[y * line..y * line + width as usize * channels];
            for x in 0..width as usize {
                for c in 0..3 {
                    out[(y * width as usize + x) * 3 + c] = row[pick(x, c)];
                }
            }
        }
        Samples::U8(out)
    };
    Ok(Decoded { width, height, samples, bit_depth: if wide { 16 } else { 8 } })
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::io::Write;

    fn chunk(typ: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut v = (data.len() as u32).to_be_bytes().to_vec();
        v.extend_from_slice(typ);
        v.extend_from_slice(data);
        let mut crc = flate2::Crc::new();
        crc.update(typ);
        crc.update(data);
        v.extend_from_slice(&crc.sum().to_be_bytes());
        v
    }

    /// Encodes RGB samples (8 or 16 bit) with optional ICC, eXIf and XMP (iTXt) chunks.
    pub fn encode(
        w: u32,
        h: u32,
        rgb16: Option<&[u16]>,
        rgb8: Option<&[u8]>,
        icc: Option<&[u8]>,
        exif: Option<&[u8]>,
        xmp: Option<&str>,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w, h);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(if rgb16.is_some() { png::BitDepth::Sixteen } else { png::BitDepth::Eight });
            let mut wr = enc.write_header().unwrap();
            let data: Vec<u8> = match (rgb16, rgb8) {
                (Some(s), _) => s.iter().flat_map(|v| v.to_be_bytes()).collect(),
                (None, Some(s)) => s.to_vec(),
                _ => panic!("no samples"),
            };
            wr.write_image_data(&data).unwrap();
        }
        // Insert extra chunks right after IHDR (8 + 25 bytes).
        let mut extra = Vec::new();
        if let Some(icc) = icc {
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            z.write_all(icc).unwrap();
            let mut d = b"test\0\0".to_vec();
            d.extend(z.finish().unwrap());
            extra.extend(chunk(b"iCCP", &d));
        }
        if let Some(e) = exif {
            extra.extend(chunk(b"eXIf", e));
        }
        if let Some(x) = xmp {
            let mut d = b"XML:com.adobe.xmp\0\0\0\0\0".to_vec();
            d.extend_from_slice(x.as_bytes());
            extra.extend(chunk(b"iTXt", &d));
        }
        let mut file = out[..33].to_vec();
        file.extend(extra);
        file.extend_from_slice(&out[33..]);
        file
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_and_decode() {
        let px: Vec<u16> = (0..6 * 4 * 3).map(|i| (i * 1000) as u16).collect();
        let bytes = test_support::encode(
            6,
            4,
            Some(&px),
            None,
            Some(b"ICCDATA"),
            Some(b"II*\0\x08\0\0\0"),
            Some("<x:xmpmeta/>"),
        );
        let m = scan(bytes.as_slice()).unwrap();
        assert_eq!((m.width, m.height), (6, 4));
        assert_eq!(m.icc.as_deref(), Some(b"ICCDATA".as_slice()));
        assert_eq!(m.exif.as_deref(), Some(b"II*\0\x08\0\0\0".as_slice()));
        assert_eq!(m.xmp.as_deref(), Some("<x:xmpmeta/>"));
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.png");
        std::fs::write(&p, &bytes).unwrap();
        let d = decode(&p).unwrap();
        assert_eq!((d.width, d.height, d.bit_depth), (6, 4, 16));
        match d.samples {
            Samples::U16(s) => assert_eq!(s, px),
            Samples::U8(_) => panic!("expected 16-bit"),
        }
        assert!(scan(b"nope".as_slice()).is_err());
    }
}
