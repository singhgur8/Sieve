//! Encoders. JPEG via TurboJPEG (already linked; markers spliced in afterwards), TIFF via
//! the own writer in [`super::tiffw`], PNG via the `png` crate (iCCP / eXIf / iTXt written
//! as raw chunks), WebP via libwebp (vendored by `libwebp-sys`; simple-API bitstream
//! re-wrapped in an extended RIFF container with ICCP / EXIF / XMP chunks), HEIC via macOS
//! ImageIO when the system can encode it. Every file embeds the ICC profile of
//! `settings.color_space` and the resolution `settings.resize.resolution_ppi`.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::OnceLock;

use super::develop::{ExportImage, ExportPixels};
use super::heic;
use super::metadata::{ExifContainer, ExportMetadata};
use super::tiffw::{self, Samples};
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::{
    BitDepth, ChromaSubsampling, ExportColorSpace, ExportFormat, ExportFormatInfo, ExportFormatKind, ExportSettings,
};
use crate::raw::turbo::{self, Subsampling};

/// ICC profile bytes for `space` (generated v2.1 matrix/TRC profiles: sRGB IEC61966-2.1,
/// Display P3, Adobe RGB (1998)-compatible; see [`super::color`]).
pub fn icc_profile(space: ExportColorSpace) -> &'static [u8] {
    super::color::icc_profile(space)
}

/// WebP's maximum edge (VP8/VP8L bitstreams store 14-bit sizes).
pub const WEBP_MAX_EDGE: u32 = 16383;

/// Availability of every `ExportFormatKind`, in enum order (backs `get_export_capabilities`).
pub fn probe_formats() -> Vec<ExportFormatInfo> {
    static FORMATS: OnceLock<Vec<ExportFormatInfo>> = OnceLock::new();
    FORMATS
        .get_or_init(|| {
            ExportFormatKind::ALL
                .iter()
                .map(|&kind| {
                    let (available, reason, bit_depths) = match kind {
                        ExportFormatKind::Jpeg | ExportFormatKind::Webp => (true, None, vec![BitDepth::Eight]),
                        ExportFormatKind::Tiff | ExportFormatKind::Png => {
                            (true, None, vec![BitDepth::Eight, BitDepth::Sixteen])
                        }
                        ExportFormatKind::Heic => match heic::probe() {
                            Ok(()) => (true, None, vec![BitDepth::Eight]),
                            Err(why) => (false, Some(why), vec![BitDepth::Eight]),
                        },
                    };
                    ExportFormatInfo { kind, available, reason, bit_depths, supports_metadata: true }
                })
                .collect()
        })
        .clone()
}

fn io_err(path: &Path, e: std::io::Error) -> AppError {
    crate::raw::access::io_error(path, "write", &e)
}

fn rgb8(image: &ExportImage) -> AppResult<&[u8]> {
    match &image.pixels {
        ExportPixels::Rgb8(p) => Ok(p),
        ExportPixels::Rgb16(_) => Err(AppError::internal("8-bit pixels expected for this format")),
    }
}

/// Blocking. Encodes `image` per `settings.format` with ICC, resolution and `metadata`, and
/// writes it to `path` (the caller passes a temp path and renames on success). The file is
/// fsynced. An unavailable format -> `invalid_argument`.
pub fn write_file(
    image: &ExportImage,
    settings: &ExportSettings,
    metadata: &ExportMetadata,
    path: &Path,
) -> AppResult<()> {
    let kind = settings.format.kind();
    if let Some(info) = probe_formats().into_iter().find(|f| f.kind == kind && !f.available) {
        return Err(AppError::invalid(
            info.reason.unwrap_or_else(|| format!("{} export is unavailable", kind.as_str())),
        ));
    }
    let (w, h) = (image.width, image.height);
    let ppi = settings.resize.resolution_ppi;
    let cs = settings.color_space;
    let icc = icc_profile(cs);
    let exif = || tiffw::exif_blob(&metadata.exif_dirs(w, h, ppi, cs, ExifContainer::Other));
    let bytes: Option<Vec<u8>> = match &settings.format {
        ExportFormat::Jpeg { quality, chroma_subsampling } => {
            let sub = match chroma_subsampling {
                ChromaSubsampling::Yuv444 => Subsampling::S444,
                ChromaSubsampling::Yuv422 => Subsampling::S422,
                ChromaSubsampling::Yuv420 => Subsampling::S420,
            };
            let jpeg_exif = |m: &ExportMetadata| tiffw::exif_blob(&m.exif_dirs(w, h, ppi, cs, ExifContainer::Jpeg));
            let exif = jpeg_exif(metadata);
            // An APP1 segment holds < 64 KiB: fall back to IFD0 only if the EXIF is huge.
            let exif = if exif.len() + 8 <= 65533 {
                exif
            } else {
                jpeg_exif(&ExportMetadata { exif: Vec::new(), gps: Vec::new(), ..metadata.clone() })
            };
            Some(
                turbo::encode_export(rgb8(image)?, w, h, *quality, sub, ppi, |jpeg| {
                    Ok(splice_jpeg(jpeg, &exif, metadata.xmp.as_deref(), icc))
                })
                .map_err(AppError::internal)?,
            )
        }
        ExportFormat::Png { .. } => Some(png(image, ppi, icc, &exif(), metadata.xmp.as_deref(), cs)?),
        ExportFormat::Webp { quality, lossless } => {
            Some(webp(rgb8(image)?, w, h, *quality, *lossless, icc, &exif(), metadata.xmp.as_deref())?)
        }
        ExportFormat::Heic { quality } => {
            heic::write(rgb8(image)?, w, h, *quality, icc, Some(&metadata.xmp_with_exif()), path).map_err(|e| {
                AppError::new(crate::ipc::error::ErrorKind::Io, format!("Could not write {}: {e}", path.display()))
            })?;
            None
        }
        ExportFormat::Tiff { compression, .. } => {
            let file = File::create(path).map_err(|e| io_err(path, e))?;
            let samples = match &image.pixels {
                ExportPixels::Rgb8(p) => Samples::U8(p),
                ExportPixels::Rgb16(p) => Samples::U16(p),
            };
            let dirs = metadata.tiff_dirs(cs, icc);
            let file = tiffw::write_tiff(file, w, h, samples, *compression, ppi, &dirs).map_err(|e| io_err(path, e))?;
            file.sync_all().map_err(|e| io_err(path, e))?;
            None
        }
    };
    if let Some(bytes) = bytes {
        let mut f = File::create(path).map_err(|e| io_err(path, e))?;
        f.write_all(&bytes).map_err(|e| io_err(path, e))?;
        f.sync_all().map_err(|e| io_err(path, e))?;
    }
    Ok(())
}

fn segment(out: &mut Vec<u8>, marker: u8, parts: &[&[u8]]) {
    let len: usize = 2 + parts.iter().map(|p| p.len()).sum::<usize>();
    debug_assert!(len <= 65535);
    out.extend_from_slice(&[0xFF, marker]);
    out.extend_from_slice(&(len as u16).to_be_bytes());
    for p in parts {
        out.extend_from_slice(p);
    }
}

const XMP_HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
/// ICC data per APP2 segment (65535 - 2 length - 14 header bytes).
const ICC_CHUNK: usize = 65519;

/// Inserts APP1 EXIF, APP1 XMP and APP2 ICC segments after SOI (+ JFIF APP0).
pub fn splice_jpeg(jpeg: &[u8], exif: &[u8], xmp: Option<&str>, icc: &[u8]) -> Vec<u8> {
    let mut at = 2;
    if jpeg.len() > 6 && jpeg[2] == 0xFF && jpeg[3] == 0xE0 {
        at = 4 + u16::from_be_bytes([jpeg[4], jpeg[5]]) as usize;
    }
    let mut out = Vec::with_capacity(jpeg.len() + exif.len() + icc.len() + 4096);
    out.extend_from_slice(&jpeg[..at]);
    if !exif.is_empty() && exif.len() + 8 <= 65535 {
        segment(&mut out, 0xE1, &[b"Exif\0\0", exif]);
    }
    if let Some(x) = xmp {
        if x.len() + XMP_HEADER.len() + 2 <= 65535 {
            segment(&mut out, 0xE1, &[XMP_HEADER, x.as_bytes()]);
        }
    }
    let chunks: Vec<&[u8]> = icc.chunks(ICC_CHUNK).collect();
    for (i, c) in chunks.iter().enumerate() {
        segment(&mut out, 0xE2, &[b"ICC_PROFILE\0", &[(i + 1) as u8, chunks.len() as u8], c]);
    }
    out.extend_from_slice(&jpeg[at..]);
    out
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    enc.write_all(data).expect("in-memory write");
    enc.finish().expect("in-memory write")
}

fn png(
    image: &ExportImage,
    ppi: u32,
    icc: &[u8],
    exif: &[u8],
    xmp: Option<&str>,
    cs: ExportColorSpace,
) -> AppResult<Vec<u8>> {
    let (w, h) = (image.width, image.height);
    let mut out = Vec::new();
    let perr = |e: png::EncodingError| AppError::internal(format!("PNG: {e}"));
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(match image.pixels {
            ExportPixels::Rgb8(_) => png::BitDepth::Eight,
            ExportPixels::Rgb16(_) => png::BitDepth::Sixteen,
        });
        enc.set_compression(png::Compression::Balanced);
        let ppm = (f64::from(ppi) / 0.0254).round() as u32;
        enc.set_pixel_dims(Some(png::PixelDimensions { xppu: ppm, yppu: ppm, unit: png::Unit::Meter }));
        let mut wr = enc.write_header().map_err(perr)?;
        let name = super::color::profile_description(cs);
        let iccp = [name.as_bytes(), &[0, 0], &zlib(icc)].concat();
        wr.write_chunk(png::chunk::ChunkType(*b"iCCP"), &iccp).map_err(perr)?;
        wr.write_chunk(png::chunk::ChunkType(*b"eXIf"), exif).map_err(perr)?;
        if let Some(x) = xmp {
            let itxt = [b"XML:com.adobe.xmp".as_slice(), &[0, 0, 0, 0, 0], x.as_bytes()].concat();
            wr.write_chunk(png::chunk::ChunkType(*b"iTXt"), &itxt).map_err(perr)?;
        }
        match &image.pixels {
            ExportPixels::Rgb8(p) => wr.write_image_data(p).map_err(perr)?,
            ExportPixels::Rgb16(p) => {
                // Stream big-endian rows (no full-size byte copy).
                let mut sw = wr.stream_writer().map_err(perr)?;
                let mut row = Vec::with_capacity(w as usize * 6);
                for r in p.chunks_exact(w as usize * 3) {
                    row.clear();
                    row.extend(r.iter().flat_map(|v| v.to_be_bytes()));
                    sw.write_all(&row).map_err(|e| AppError::internal(format!("PNG: {e}")))?;
                }
                sw.finish().map_err(perr)?;
            }
        }
        wr.finish().map_err(perr)?;
    }
    Ok(out)
}

fn riff_chunk(out: &mut Vec<u8>, fourcc: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(fourcc);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    if data.len() & 1 == 1 {
        out.push(0);
    }
}

#[allow(clippy::too_many_arguments)]
fn webp(
    rgb: &[u8],
    w: u32,
    h: u32,
    quality: u8,
    lossless: bool,
    icc: &[u8],
    exif: &[u8],
    xmp: Option<&str>,
) -> AppResult<Vec<u8>> {
    if w > WEBP_MAX_EDGE || h > WEBP_MAX_EDGE {
        return Err(AppError::invalid(format!(
            "WebP is limited to {WEBP_MAX_EDGE} px per edge ({w}x{h}); resize the export"
        )));
    }
    let mut ptr: *mut u8 = std::ptr::null_mut();
    // SAFETY: rgb holds h rows of w*3 bytes; libwebp allocates `ptr` (freed below).
    let size = unsafe {
        if lossless {
            libwebp_sys::WebPEncodeLosslessRGB(rgb.as_ptr(), w as i32, h as i32, (w * 3) as i32, &mut ptr)
        } else {
            libwebp_sys::WebPEncodeRGB(rgb.as_ptr(), w as i32, h as i32, (w * 3) as i32, f32::from(quality), &mut ptr)
        }
    };
    if ptr.is_null() || size == 0 {
        return Err(AppError::internal("WebP encode failed"));
    }
    // SAFETY: libwebp wrote `size` bytes at `ptr`.
    let simple = unsafe { std::slice::from_raw_parts(ptr, size) }.to_vec();
    // SAFETY: allocated by libwebp; freed once.
    unsafe { libwebp_sys::WebPFree(ptr.cast()) };
    if simple.len() < 12 || &simple[..4] != b"RIFF" || &simple[8..12] != b"WEBP" {
        return Err(AppError::internal("WebP encoder returned an unexpected container"));
    }
    let image_chunks = &simple[12..];
    let mut flags = 0x20u8; // ICC
    if !exif.is_empty() {
        flags |= 0x08;
    }
    if xmp.is_some() {
        flags |= 0x04;
    }
    let mut vp8x = vec![flags, 0, 0, 0];
    vp8x.extend_from_slice(&(w - 1).to_le_bytes()[..3]);
    vp8x.extend_from_slice(&(h - 1).to_le_bytes()[..3]);
    let mut body = b"WEBP".to_vec();
    riff_chunk(&mut body, b"VP8X", &vp8x);
    riff_chunk(&mut body, b"ICCP", icc);
    body.extend_from_slice(image_chunks);
    if !exif.is_empty() {
        riff_chunk(&mut body, b"EXIF", exif);
    }
    if let Some(x) = xmp {
        riff_chunk(&mut body, b"XMP ", x.as_bytes());
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::{
        CollisionPolicy, ExportDestination, FileNaming, MetadataInclude, MetadataOptions, ResizeMode, ResizeOptions,
        TiffCompression,
    };

    fn settings(format: ExportFormat) -> ExportSettings {
        ExportSettings {
            format,
            color_space: ExportColorSpace::DisplayP3,
            resize: ResizeOptions { mode: ResizeMode::None, dont_enlarge: true, resolution_ppi: 240 },
            sharpening: None,
            naming: FileNaming {
                template: "{filename}".into(),
                start_number: 1,
                collision: CollisionPolicy::UniqueSuffix,
            },
            destination: ExportDestination::Choose,
            subfolder: None,
            metadata: MetadataOptions {
                include: MetadataInclude::None,
                remove_location: false,
                include_keywords: false,
                copyright: None,
                creator: None,
            },
        }
    }

    fn image8() -> ExportImage {
        let (w, h) = (48u32, 32u32);
        let px = (0..w * h).flat_map(|i| [(i % 256) as u8, (i / 7 % 256) as u8, 90]).collect();
        ExportImage { width: w, height: h, pixels: ExportPixels::Rgb8(px) }
    }

    fn meta() -> ExportMetadata {
        ExportMetadata {
            ifd0: vec![tiffw::ascii(tiffw::COPYRIGHT, "(c) Test")],
            xmp: Some("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>".into()),
            ..Default::default()
        }
    }

    #[test]
    fn jpeg_carries_exif_xmp_icc() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jpg");
        let s = settings(ExportFormat::Jpeg { quality: 85, chroma_subsampling: ChromaSubsampling::Yuv420 });
        write_file(&image8(), &s, &meta(), &p).unwrap();
        let b = std::fs::read(&p).unwrap();
        assert_eq!(turbo::dimensions(&b).unwrap(), (48, 32));
        let exif = crate::raw::jpeg::exif_tiff(&b).expect("EXIF APP1");
        let m = crate::raw::tiff::Tiff::new(exif).unwrap().ifd0_tags().unwrap();
        assert!(m.iter().any(|t| t.tag == tiffw::ORIENTATION && t.data == [1, 0]));
        assert!(m.iter().any(|t| t.tag == tiffw::COPYRIGHT));
        assert!(b.windows(XMP_HEADER.len()).any(|w| w == XMP_HEADER));
        let icc = icc_profile(ExportColorSpace::DisplayP3);
        assert!(b.windows(12).any(|w| w == b"ICC_PROFILE\0"));
        assert!(b.windows(icc.len()).any(|w| w == icc));
    }

    #[test]
    fn png_and_webp_containers() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.png");
        write_file(&image8(), &settings(ExportFormat::Png { bit_depth: BitDepth::Eight }), &meta(), &p).unwrap();
        let b = std::fs::read(&p).unwrap();
        let d = png::Decoder::new(std::io::Cursor::new(&b));
        let r = d.read_info().unwrap();
        assert_eq!((r.info().width, r.info().height), (48, 32));
        assert!(r.info().icc_profile.is_some());
        assert!(b.windows(4).any(|w| w == b"eXIf") && b.windows(17).any(|w| w == b"XML:com.adobe.xmp"));

        // 16-bit PNG from 16-bit pixels.
        let img16 =
            ExportImage { width: 4, height: 2, pixels: ExportPixels::Rgb16((0..24).map(|i| i * 2000).collect()) };
        let p16 = dir.path().join("b.png");
        write_file(&img16, &settings(ExportFormat::Png { bit_depth: BitDepth::Sixteen }), &meta(), &p16).unwrap();
        let mut r = png::Decoder::new(std::io::BufReader::new(File::open(&p16).unwrap())).read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        r.next_frame(&mut buf).unwrap();
        assert_eq!(u16::from_be_bytes([buf[2], buf[3]]), 2000);

        let pw = dir.path().join("a.webp");
        write_file(&image8(), &settings(ExportFormat::Webp { quality: 80, lossless: false }), &meta(), &pw).unwrap();
        let b = std::fs::read(&pw).unwrap();
        assert_eq!(&b[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize, b.len() - 8);
        assert_eq!(&b[12..16], b"VP8X");
        for c in [b"ICCP", b"EXIF", b"XMP "] {
            assert!(b.windows(4).any(|w| w == c), "{c:?}");
        }
    }

    #[test]
    fn tiff_file_has_icc_and_xmp() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.tif");
        let img16 = ExportImage { width: 8, height: 4, pixels: ExportPixels::Rgb16(vec![1234; 8 * 4 * 3]) };
        let s = settings(ExportFormat::Tiff { bit_depth: BitDepth::Sixteen, compression: TiffCompression::Zip });
        write_file(&img16, &s, &meta(), &p).unwrap();
        let b = std::fs::read(&p).unwrap();
        let tags = crate::raw::tiff::Tiff::new(b.as_slice()).unwrap().ifd0_tags().unwrap();
        for t in [tiffw::ICC_PROFILE, tiffw::XMP, tiffw::COPYRIGHT, tiffw::ORIENTATION, tiffw::PREDICTOR] {
            assert!(tags.iter().any(|x| x.tag == t), "tag {t}");
        }
    }

    #[test]
    fn formats_probe_in_enum_order() {
        let f = probe_formats();
        assert_eq!(f.iter().map(|f| f.kind).collect::<Vec<_>>(), ExportFormatKind::ALL.to_vec());
        assert!(f[0].available && f[1].available && f[2].available);
        assert!(f.iter().filter(|f| !f.available).all(|f| f.reason.is_some()));
    }
}
