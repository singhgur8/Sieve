//! Non-RAW ("raster") sources, Phase 7b: JPEG, HEIC/HEIF, TIFF, PNG.
//!
//! Rules (docs/architecture.md "Non-RAW sources"):
//! - Originals are never modified (no embedded-XMP writes, no re-encoding). Edits go to a
//!   `<file name>.xmp` sidecar (`xmp::sidecar_path`, e.g. `IMG_1.JPG.xmp`).
//! - Ingest ([`extract`]): EXIF (make/model/lens/capture time/exposure/orientation) and a
//!   preview for the thumbnail pipeline. An sRGB (or untagged) JPEG is its own preview
//!   (`Preview::Embedded`); everything else is decoded, colour-managed from its ICC profile
//!   and delivered as 8-bit sRGB (`Preview::Libraw(Thumb::Rgb)`) at <= `PREVIEW_EDGE`:
//!   JPEG/PNG through [`decode_linear`], HEIC/TIFF through ImageIO's thumbnail path.
//!   `sensor_layout` stays unknown; `camera_make` comes from EXIF, else `other`.
//! - Develop ([`decode_linear`]): decode (TurboJPEG, `png`, ImageIO for HEIC/TIFF) ->
//!   linearize with the profile's own TRCs (`raw::icc`) -> 16-bit linear light in the
//!   source's primaries (generic matrix profiles are converted to linear ProPhoto) ->
//!   area downscale in linear light -> [`to_linear_image`] (display-referred, WB = 1).
//! - Export uses `decode_linear(path, format, None)` (full size) instead of LibRaw
//!   (`develop::source::decode_full` dispatches by extension).
//! - Embedded XMP ([`embedded_xmp`]) is read-only input when no sidecar exists.
//!
//! Memory: one decoded frame (8 or 16 bit) plus the linear output; the downscale streams
//! source rows per output row (no full-size float buffer).

use std::path::Path;
use std::sync::OnceLock;

use rayon::prelude::*;

use crate::develop::source::{ColorInfo, LinearImage};
use crate::ipc::types::{CameraMake, ImageFormat};

use super::icc::{self, Profile};
use super::meta::MetaBuilder;
use super::preview::PREVIEW_EDGE;
use super::source::{ByteSource, FileSource};
use super::{heif, imageio, jpeg, libraw, png, tiff, turbo, Extracted, Preview};

/// Colour encoding of a decoded raster source, identified from its ICC profile
/// (`Unknown` = no/unsupported profile: decoded as sRGB, `DevelopWarningCode::SourceColorAssumed`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceColorSpace {
    Srgb,
    DisplayP3,
    AdobeRgb,
    /// ProPhoto / ROMM RGB (16-bit TIFFs from Lightroom/Photoshop).
    ProPhoto,
    /// Profile description if there was one.
    Unknown(Option<String>),
}

impl SourceColorSpace {
    /// Detail for `DevelopWarningCode::SourceColorAssumed`, or `None` when the colour
    /// encoding is known.
    pub fn assumed_detail(&self) -> Option<String> {
        match self {
            SourceColorSpace::Unknown(desc) => Some(
                desc.clone().map_or_else(|| "no colour profile; assumed sRGB".into(), |d| format!("{d}; assumed sRGB")),
            ),
            _ => None,
        }
    }
}

/// Linear-light decode of a raster file (orientation *not* applied, like RAW decodes).
#[derive(Debug, Clone)]
pub struct RasterImage {
    pub width: u32,
    pub height: u32,
    /// Interleaved RGB, linear light, 0..=65535, in `color_space` primaries.
    pub pixels: Vec<u16>,
    pub color_space: SourceColorSpace,
    /// Bits per sample in the file (8 or 16).
    pub bit_depth: u8,
    /// Full-size dimensions of the file (equal to `width/height` unless downscaled).
    pub full_width: u32,
    pub full_height: u32,
    /// EXIF orientation 1..=8 from the file, if any.
    pub orientation: Option<u8>,
}

// ---------------------------------------------------------------------------
// Encoded decode (samples in the file's own encoding)
// ---------------------------------------------------------------------------

enum Samples {
    U8(Vec<u8>),
    U16(Vec<u16>),
}

/// Decoded pixels before linearization.
struct Encoded {
    width: u32,
    height: u32,
    samples: Samples,
    /// Profile used to linearize (sRGB for unknown/unsupported).
    profile: Profile,
    /// What we report (Unknown when assumed).
    reported: SourceColorSpace,
    bit_depth: u8,
    full_width: u32,
    full_height: u32,
    orientation: Option<u8>,
}

/// Upper bound for header segments read from a JPEG (EXIF/ICC/XMP incl. extended XMP).
const MAX_JPEG_HEADER: u64 = 32 << 20;
/// Largest frame we decode (guards memory on corrupt headers).
const MAX_PIXELS: u64 = 400_000_000;

/// The JPEG's marker segments from SOI up to SOS (no entropy-coded data).
fn jpeg_header(src: &(impl ByteSource + ?Sized)) -> Result<Vec<u8>, String> {
    let mut pos = 2u64;
    let mut two = [0u8; 2];
    src.read_at(0, &mut two).map_err(|e| format!("JPEG: {e}"))?;
    if two != [0xFF, 0xD8] {
        return Err("not a JPEG".into());
    }
    loop {
        let mut m = [0u8; 4];
        if src.read_at(pos, &mut m).is_err() || m[0] != 0xFF {
            break;
        }
        if m[1] == 0xFF {
            pos += 1;
            continue;
        }
        if matches!(m[1], 0xDA | 0xD9) || pos > MAX_JPEG_HEADER {
            break;
        }
        let len = u16::from_be_bytes([m[2], m[3]]) as u64;
        pos += 2 + len.max(2);
    }
    src.read_vec(0, pos.min(src.len()) as usize).map_err(|e| format!("JPEG: {e}"))
}

/// Profile of a JPEG: embedded ICC, else EXIF colour signalling (sRGB / DCF Adobe RGB),
/// else assumed sRGB. Returns `(profile used, reported space)`.
fn jpeg_profile(header: &[u8]) -> (Profile, SourceColorSpace) {
    if let Some(bytes) = icc::from_jpeg(header) {
        return profile_from_icc(&bytes);
    }
    let signal = jpeg::exif_tiff(header).and_then(|t| tiff::Tiff::new(t).ok()).map(|t| t.color_signal());
    match signal {
        Some(s) if s.interop_index.as_deref() == Some("R03") && s.color_space != Some(1) => {
            (Profile::adobe_rgb(), SourceColorSpace::AdobeRgb)
        }
        Some(s) if s.color_space == Some(1) => (Profile::srgb(), SourceColorSpace::Srgb),
        _ => (Profile::srgb(), SourceColorSpace::Unknown(None)),
    }
}

fn profile_from_icc(bytes: &[u8]) -> (Profile, SourceColorSpace) {
    match icc::parse(bytes) {
        Ok(p) => {
            let reported = match &p.space {
                // Generic matrix profiles are converted to ProPhoto during linearization.
                SourceColorSpace::Unknown(_) => SourceColorSpace::ProPhoto,
                s => s.clone(),
            };
            (p, reported)
        }
        Err(u) => (Profile::srgb(), SourceColorSpace::Unknown(Some(icc::describe(&u)))),
    }
}

fn meta_from_exif(tiff_bytes: Option<&[u8]>) -> MetaBuilder {
    tiff_bytes.and_then(|t| tiff::scan_bytes(t, false).ok()).map(|s| s.meta).unwrap_or_default()
}

fn decode_jpeg_bytes(bytes: &[u8], min_long_edge: Option<u32>) -> Result<Encoded, String> {
    let (profile, reported) = jpeg_profile(bytes);
    let (fw, fh) = match jpeg::header(bytes) {
        jpeg::Header::Frame { width, height } => (u32::from(width), u32::from(height)),
        _ => return Err("JPEG: no decodable 8-bit frame".into()),
    };
    let orientation = jpeg::exif_tiff(bytes)
        .and_then(|t| tiff::scan_bytes(t, false).ok())
        .and_then(|s| s.meta.orientation)
        .filter(|o| (1..=8).contains(o))
        .map(|o| o as u8);
    let d = turbo::decode_rgb(bytes, min_long_edge.unwrap_or(u32::MAX), MAX_PIXELS)?;
    Ok(Encoded {
        width: d.width,
        height: d.height,
        samples: Samples::U8(d.pixels),
        profile,
        reported,
        bit_depth: 8,
        full_width: fw,
        full_height: fh,
        orientation,
    })
}

fn decode_png(path: &Path) -> Result<Encoded, String> {
    let src = FileSource::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let m = png::scan(&src)?;
    drop(src);
    let (profile, reported) = match &m.icc {
        Some(bytes) => profile_from_icc(bytes),
        None if m.srgb => (Profile::srgb(), SourceColorSpace::Srgb),
        None => (Profile::srgb(), SourceColorSpace::Unknown(None)),
    };
    let orientation = meta_from_exif(m.exif.as_deref()).orientation.filter(|o| (1..=8).contains(o)).map(|o| o as u8);
    let d = png::decode(path)?;
    let samples = match d.samples {
        png::Samples::U8(v) => Samples::U8(v),
        png::Samples::U16(v) => Samples::U16(v),
    };
    Ok(Encoded {
        width: d.width,
        height: d.height,
        samples,
        profile,
        reported,
        bit_depth: d.bit_depth,
        full_width: d.width,
        full_height: d.height,
        orientation,
    })
}

/// Orientation of an ImageIO source: EXIF inside the file, else ImageIO's (HEIF irot/imir).
fn imageio_orientation(path: &Path, format: ImageFormat) -> Option<u8> {
    let exif = match format {
        ImageFormat::Heic => FileSource::open(path).ok().and_then(|s| heif::parse(&s).ok()).and_then(|h| {
            let from_exif = meta_from_exif(h.exif.as_deref()).orientation.map(|o| o as u8);
            from_exif.or(h.orientation)
        }),
        ImageFormat::Tiff => FileSource::open(path)
            .ok()
            .and_then(|s| tiff::Tiff::new(&s).ok().and_then(|t| t.scan(false).ok()))
            .and_then(|s| s.meta.orientation)
            .map(|o| o as u8),
        _ => None,
    };
    exif.filter(|o| (1..=8).contains(o)).or_else(|| imageio::properties(path).ok().and_then(|p| p.orientation))
}

fn decode_imageio(path: &Path, format: ImageFormat) -> Result<Encoded, String> {
    let n = imageio::decode_native(path)?;
    let (profile, reported) = if n.converted_to_srgb {
        (Profile::srgb(), SourceColorSpace::Srgb)
    } else {
        match &n.icc {
            Some(bytes) => profile_from_icc(bytes),
            None => (Profile::srgb(), SourceColorSpace::Unknown(None)),
        }
    };
    let samples = match n.samples {
        imageio::Samples::U8(v) => Samples::U8(v),
        imageio::Samples::U16(v) => Samples::U16(v),
    };
    let bit_depth = if matches!(samples, Samples::U16(_)) { 16 } else { 8 };
    Ok(Encoded {
        width: n.width,
        height: n.height,
        samples,
        profile,
        reported,
        bit_depth,
        full_width: n.width,
        full_height: n.height,
        orientation: imageio_orientation(path, format),
    })
}

fn decode_encoded(path: &Path, format: ImageFormat, min_long_edge: Option<u32>) -> Result<Encoded, String> {
    match format {
        ImageFormat::Jpeg => {
            let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
            // CMYK / 12-bit / lossless JPEGs: let ImageIO handle them.
            decode_jpeg_bytes(&bytes, min_long_edge)
                .or_else(|e| decode_imageio(path, format).map_err(|e2| format!("{}: {e}; {e2}", path.display())))
        }
        ImageFormat::Png => decode_png(path)
            .or_else(|e| decode_imageio(path, format).map_err(|e2| format!("{}: {e}; {e2}", path.display()))),
        ImageFormat::Heic | ImageFormat::Tiff => {
            decode_imageio(path, format).map_err(|e| format!("{}: {e}", path.display()))
        }
        f => Err(format!("{}: {} is a RAW format", path.display(), f.as_str())),
    }
}

// ---------------------------------------------------------------------------
// Linearization + area downscale
// ---------------------------------------------------------------------------

/// Linear ProPhoto (D50) from XYZ D50.
fn prophoto_from_xyz_d50() -> [[f64; 3]; 3] {
    icc::invert(&icc::known_d50(&SourceColorSpace::ProPhoto).expect("known")).expect("invertible")
}

/// Size with long edge <= `edge` (never upscales).
fn fit(w: u32, h: u32, edge: u32) -> (u32, u32) {
    let long = w.max(h);
    if long <= edge {
        return (w, h);
    }
    let s = edge as f64 / long as f64;
    (((w as f64 * s).round() as u32).max(1), ((h as f64 * s).round() as u32).max(1))
}

/// Box (area) filter taps: output `i` covers source `[i*scale, (i+1)*scale)`.
struct AreaTaps {
    start: Vec<usize>,
    weights: Vec<Vec<f32>>,
}

fn area_taps(src: usize, dst: usize) -> AreaTaps {
    let scale = src as f64 / dst as f64;
    let mut start = Vec::with_capacity(dst);
    let mut weights = Vec::with_capacity(dst);
    for i in 0..dst {
        let (a, b) = (i as f64 * scale, ((i + 1) as f64 * scale).min(src as f64));
        let first = a.floor() as usize;
        let last = (b.ceil() as usize).clamp(first + 1, src);
        let w: Vec<f32> = (first..last)
            .map(|k| {
                let lo = (k as f64).max(a);
                let hi = ((k + 1) as f64).min(b);
                ((hi - lo).max(0.0) / (b - a)) as f32
            })
            .collect();
        start.push(first);
        weights.push(w);
    }
    AreaTaps { start, weights }
}

/// Per-channel linearization LUTs + optional matrix into the output primaries.
struct Linearizer {
    luts: [Vec<u16>; 3],
    matrix: Option<[[f32; 3]; 3]>,
}

impl Linearizer {
    fn new(profile: &Profile, bits: u8) -> Self {
        let luts = [0, 1, 2].map(|c| profile.trc[c].lut(bits));
        let matrix = match (&profile.space, profile.to_xyz_d50) {
            (SourceColorSpace::Unknown(_), Some(m)) if !profile.gray => {
                let to_pro = icc::mat_mul(&prophoto_from_xyz_d50(), &m);
                Some(to_pro.map(|r| r.map(|v| v as f32)))
            }
            _ => None,
        };
        Linearizer { luts, matrix }
    }

    /// Linear values (0..=65535 as f32) of one encoded pixel.
    #[inline]
    fn pixel(&self, codes: [usize; 3]) -> [f32; 3] {
        let v = [0, 1, 2].map(|c| f32::from(self.luts[c][codes[c]]));
        match &self.matrix {
            None => v,
            Some(m) => [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2]),
        }
    }
}

fn to_u16(v: f32) -> u16 {
    v.round().clamp(0.0, 65535.0) as u16
}

/// Linearizes `enc` into `dw x dh` (area filter in linear light when smaller).
fn linearize(enc: &Encoded, dw: u32, dh: u32) -> Vec<u16> {
    let (sw, sh) = (enc.width as usize, enc.height as usize);
    let (dw, dh) = (dw as usize, dh as usize);
    let bits = match enc.samples {
        Samples::U8(_) => 8,
        Samples::U16(_) => 16,
    };
    let lin = Linearizer::new(&enc.profile, bits);
    let code = |i: usize| -> usize {
        match &enc.samples {
            Samples::U8(s) => s[i] as usize,
            Samples::U16(s) => s[i] as usize,
        }
    };
    let mut out = vec![0u16; dw * dh * 3];
    if (dw, dh) == (sw, sh) {
        out.par_chunks_mut(dw * 3).enumerate().for_each(|(y, row)| {
            for x in 0..dw {
                let i = (y * sw + x) * 3;
                let p = lin.pixel([code(i), code(i + 1), code(i + 2)]);
                row[x * 3..x * 3 + 3].copy_from_slice(&p.map(to_u16));
            }
        });
        return out;
    }
    let tx = area_taps(sw, dw);
    let ty = area_taps(sh, dh);
    out.par_chunks_mut(dw * 3).enumerate().for_each_init(
        || (vec![0.0f32; sw * 3], vec![0.0f32; dw * 3]),
        |(src_row, acc), (y, out_row)| {
            acc.iter_mut().for_each(|v| *v = 0.0);
            for (k, &wy) in ty.weights[y].iter().enumerate() {
                if wy == 0.0 {
                    continue;
                }
                let sy = ty.start[y] + k;
                for x in 0..sw {
                    let i = (sy * sw + x) * 3;
                    let p = lin.pixel([code(i), code(i + 1), code(i + 2)]);
                    src_row[x * 3..x * 3 + 3].copy_from_slice(&p);
                }
                for x in 0..dw {
                    let mut s = [0.0f32; 3];
                    for (j, &wx) in tx.weights[x].iter().enumerate() {
                        let p = &src_row[(tx.start[x] + j) * 3..(tx.start[x] + j) * 3 + 3];
                        s[0] += wx * p[0];
                        s[1] += wx * p[1];
                        s[2] += wx * p[2];
                    }
                    for c in 0..3 {
                        acc[x * 3 + c] += wy * s[c];
                    }
                }
            }
            for (o, a) in out_row.iter_mut().zip(acc.iter()) {
                *o = to_u16(*a);
            }
        },
    );
    out
}

fn finish(enc: Encoded, max_edge: Option<u32>) -> RasterImage {
    let (dw, dh) = match max_edge {
        Some(m) => fit(enc.width, enc.height, m),
        None => (enc.width, enc.height),
    };
    let pixels = linearize(&enc, dw, dh);
    RasterImage {
        width: dw,
        height: dh,
        pixels,
        color_space: enc.reported.clone(),
        bit_depth: enc.bit_depth,
        full_width: enc.full_width,
        full_height: enc.full_height,
        orientation: enc.orientation,
    }
}

/// Develop/export: decodes `path` to linear light. `max_edge` = downscale (box/area filter
/// in linear light) so the long edge is <= `max_edge` (the editor's cached source, like the
/// RAW half-size decode: use 4096); `None` = full size (export).
pub fn decode_linear(path: &Path, format: ImageFormat, max_edge: Option<u32>) -> Result<RasterImage, String> {
    let enc = decode_encoded(path, format, max_edge)?;
    Ok(finish(enc, max_edge))
}

/// Linear RGB of `space` -> XYZ D65 (unknown spaces were decoded as sRGB).
fn space_to_xyz_d65(space: &SourceColorSpace) -> [[f64; 3]; 3] {
    icc::known_d65(space).unwrap_or_else(|| icc::known_d65(&SourceColorSpace::Srgb).expect("known"))
}

fn xyz_d65_to_srgb() -> [[f64; 3]; 3] {
    icc::invert(&icc::known_d65(&SourceColorSpace::Srgb).expect("known")).expect("invertible")
}

/// Source primaries -> linear sRGB (D65), rows normalized to sum to 1.
pub fn rgb_to_srgb(space: &SourceColorSpace) -> [[f64; 3]; 3] {
    let mut m = icc::mat_mul(&xyz_d65_to_srgb(), &space_to_xyz_d65(space));
    for row in m.iter_mut() {
        let s: f64 = row.iter().sum();
        if s.abs() > 1e-9 {
            row.iter_mut().for_each(|v| *v /= s);
        }
    }
    if matches!(space, SourceColorSpace::Srgb | SourceColorSpace::Unknown(_)) {
        m = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    }
    m
}

/// Wraps a raster decode as the pipeline's input: "camera RGB" = source primaries,
/// `as_shot_mul = daylight_mul = [1, 1, 1]`, `rgb_cam` = source primaries -> linear sRGB
/// (D65), `xyz_to_cam` = XYZ (D65) -> source primaries; marked display-referred (module docs).
pub fn to_linear_image(img: RasterImage) -> LinearImage {
    let to_xyz = space_to_xyz_d65(&img.color_space);
    let xyz_to_cam = icc::invert(&to_xyz).expect("invertible").map(|r| r.map(|v| v as f32));
    let rgb_cam = rgb_to_srgb(&img.color_space).map(|r| r.map(|v| v as f32));
    LinearImage {
        width: img.width,
        height: img.height,
        pixels: img.pixels,
        color: ColorInfo {
            as_shot_mul: Some([1.0; 3]),
            daylight_mul: [1.0; 3],
            rgb_cam,
            xyz_to_cam,
            calibration: [1.0; 3],
        },
        full_width: img.full_width,
        full_height: img.full_height,
        display_referred: true,
        source_color: Some(img.color_space),
    }
}

// ---------------------------------------------------------------------------
// Preview (8-bit sRGB) helpers
// ---------------------------------------------------------------------------

/// Linear 0..=65535 -> sRGB-encoded 8-bit.
fn srgb_encode_lut() -> &'static [u8] {
    static LUT: OnceLock<Vec<u8>> = OnceLock::new();
    LUT.get_or_init(|| {
        (0..=65535u32)
            .map(|i| {
                let v = i as f64 / 65535.0;
                let e = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
                (e * 255.0).round().clamp(0.0, 255.0) as u8
            })
            .collect()
    })
}

/// 8-bit sRGB rendition of a linear raster (gamut-clipped).
pub fn to_srgb8(img: &RasterImage) -> Vec<u8> {
    let m = rgb_to_srgb(&img.color_space).map(|r| r.map(|v| v as f32));
    let identity = matches!(img.color_space, SourceColorSpace::Srgb | SourceColorSpace::Unknown(_));
    let lut = srgb_encode_lut();
    let mut out = vec![0u8; img.pixels.len()];
    out.par_chunks_mut(img.width as usize * 3).zip(img.pixels.par_chunks(img.width as usize * 3)).for_each(|(o, i)| {
        for (op, ip) in o.as_chunks_mut::<3>().0.iter_mut().zip(i.as_chunks::<3>().0) {
            if identity {
                for c in 0..3 {
                    op[c] = lut[ip[c] as usize];
                }
            } else {
                let v = [f32::from(ip[0]), f32::from(ip[1]), f32::from(ip[2])];
                for c in 0..3 {
                    let s = m[c][0] * v[0] + m[c][1] * v[1] + m[c][2] * v[2];
                    op[c] = lut[to_u16(s) as usize];
                }
            }
        }
    });
    out
}

// ---------------------------------------------------------------------------
// Ingest
// ---------------------------------------------------------------------------

fn finish_meta(mut b: MetaBuilder, size: Option<(u32, u32)>, orientation: Option<u8>) -> super::meta::ImageMeta {
    if let Some(s) = size.filter(|(w, h)| *w > 0 && *h > 0) {
        b.sensor_size = Some(s);
    }
    if b.orientation.is_none_or(|o| !(1..=8).contains(&o)) {
        b.orientation = orientation.map(u16::from);
    }
    let mut meta = b.finish();
    // A developed file has no CFA; keep the catalog's `unknown`.
    meta.sensor_layout = None;
    if meta.make.is_none() {
        meta.make = Some(CameraMake::Other);
    }
    meta
}

fn rgb_preview(img: &RasterImage) -> Preview {
    Preview::Libraw(libraw::Thumb::Rgb { width: img.width, height: img.height, pixels: to_srgb8(img) })
}

/// Ingest: metadata + preview pixels for a raster file (see module docs). Same error
/// contract as `raw::extract`: `Err` only if the file cannot be opened at all.
pub fn extract(path: &Path, format: ImageFormat, buf: &mut Vec<u8>) -> Result<Extracted, String> {
    match format {
        ImageFormat::Jpeg => {
            buf.clear();
            let bytes = std::fs::read(path).map_err(|e| format!("open {}: {e}", path.display()))?;
            buf.extend_from_slice(&bytes);
            drop(bytes);
            let exif = jpeg::exif_tiff(buf);
            let builder = meta_from_exif(exif);
            let size = match jpeg::header(buf) {
                jpeg::Header::Frame { width, height } => Some((u32::from(width), u32::from(height))),
                _ => None,
            };
            let (profile, _) = jpeg_profile(buf);
            let preview = if size.is_some() && (profile.is_srgb() || profile.gray) {
                Ok(Preview::Embedded)
            } else {
                // Wide-gamut or undecodable-by-TurboJPEG JPEG: decode + convert to sRGB.
                decode_jpeg_bytes(buf, Some(PREVIEW_EDGE))
                    .map(|enc| finish(enc, Some(PREVIEW_EDGE)))
                    .or_else(|_| {
                        decode_encoded(path, format, Some(PREVIEW_EDGE)).map(|e| finish(e, Some(PREVIEW_EDGE)))
                    })
                    .map(|img| rgb_preview(&img))
            };
            let size = size.or_else(|| imageio::properties(path).ok().map(|p| (p.width, p.height)));
            Ok(Extracted { meta: finish_meta(builder, size, None), preview })
        }
        ImageFormat::Png => {
            let src = FileSource::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
            let scanned = png::scan(&src);
            drop(src);
            let (builder, size) = match &scanned {
                Ok(m) => (meta_from_exif(m.exif.as_deref()), Some((m.width, m.height))),
                Err(_) => (MetaBuilder::default(), None),
            };
            let preview = decode_linear(path, format, Some(PREVIEW_EDGE)).map(|img| rgb_preview(&img));
            Ok(Extracted { meta: finish_meta(builder, size, None), preview })
        }
        ImageFormat::Tiff | ImageFormat::Heic => {
            let src = FileSource::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
            let (builder, heif_size, heif_orient) = if format == ImageFormat::Tiff {
                let b = tiff::Tiff::new(&src).ok().and_then(|t| t.scan(false).ok()).map(|s| s.meta);
                (b.unwrap_or_default(), None, None)
            } else {
                match heif::parse(&src) {
                    Ok(h) => (meta_from_exif(h.exif.as_deref()), h.size, h.orientation),
                    Err(_) => (MetaBuilder::default(), None, None),
                }
            };
            drop(src);
            let props = imageio::properties(path).ok();
            let size = props.map(|p| (p.width, p.height)).filter(|(w, h)| *w > 0 && *h > 0).or(heif_size);
            let orientation = props.and_then(|p| p.orientation).or(heif_orient);
            let preview = imageio::thumbnail_srgb(path, PREVIEW_EDGE)
                .map(|(width, height, pixels)| Preview::Libraw(libraw::Thumb::Rgb { width, height, pixels }));
            Ok(Extracted { meta: finish_meta(builder, size, orientation), preview })
        }
        f => Err(format!("{}: {} is a RAW format", path.display(), f.as_str())),
    }
}

/// Raw EXIF directories of a raster file for exported metadata (JPEG APP1, TIFF IFDs,
/// PNG `eXIf`, HEIC `Exif` item). Files without EXIF yield empty directories.
pub fn exif_dirs(path: &Path, format: ImageFormat) -> Result<tiff::ExifDirs, String> {
    let src = FileSource::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let from = |t: Option<&[u8]>| -> Result<tiff::ExifDirs, String> {
        match t {
            Some(t) => tiff::Tiff::new(t)?.exif_dirs(),
            None => Ok(tiff::ExifDirs::default()),
        }
    };
    match format {
        ImageFormat::Jpeg => {
            let header = jpeg_header(&src)?;
            from(jpeg::exif_tiff(&header))
        }
        ImageFormat::Tiff => tiff::Tiff::new(&src)?.exif_dirs(),
        ImageFormat::Png => from(png::scan(&src)?.exif.as_deref()),
        ImageFormat::Heic => from(heif::parse(&src)?.exif.as_deref()),
        f => Err(format!("{}: {} is a RAW format", path.display(), f.as_str())),
    }
}

// ---------------------------------------------------------------------------
// Embedded XMP
// ---------------------------------------------------------------------------

const XMP_APP1: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const XMP_EXT_APP1: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";
const XMP_TAG: u16 = 700;
const MAX_XMP: u32 = 32 << 20;

/// One extended-XMP packet being reassembled: GUID, full length, `(offset, chunk)`s.
type ExtendedXmp<'a> = ([u8; 32], u32, Vec<(u32, &'a [u8])>);

/// Main + extended XMP of a JPEG header. The extended packet's descriptions are merged
/// into the main packet's `rdf:RDF` so one parse sees every property.
fn jpeg_xmp(header: &[u8]) -> Option<String> {
    let mut main: Option<String> = None;
    // GUID -> (full length, chunks (offset, data)).
    let mut ext: Vec<ExtendedXmp<'_>> = Vec::new();
    for (marker, body) in jpeg::segments(header) {
        if marker != 0xE1 {
            continue;
        }
        if let Some(x) = body.strip_prefix(XMP_APP1) {
            if main.is_none() {
                main = Some(String::from_utf8_lossy(x).into_owned());
            }
        } else if let Some(x) = body.strip_prefix(XMP_EXT_APP1) {
            if x.len() < 40 {
                continue;
            }
            let guid: [u8; 32] = x[..32].try_into().ok()?;
            let full = u32::from_be_bytes(x[32..36].try_into().ok()?);
            let off = u32::from_be_bytes(x[36..40].try_into().ok()?);
            match ext.iter_mut().find(|e| e.0 == guid) {
                Some(e) => e.2.push((off, &x[40..])),
                None => ext.push((guid, full, vec![(off, &x[40..])])),
            }
        }
    }
    let mut main = main?;
    let wanted = main
        .find("HasExtendedXMP")
        .and_then(|i| main[i..].find(|c: char| c.is_ascii_hexdigit()).map(|j| i + j))
        .map(|i| main[i..].chars().take(32).collect::<String>());
    let chosen = ext.iter().find(|e| Some(String::from_utf8_lossy(&e.0).into_owned()) == wanted).or(ext.first());
    if let Some((_, full, chunks)) = chosen {
        let mut data = vec![0u8; (*full).min(MAX_XMP) as usize];
        for (off, bytes) in chunks {
            let (o, end) = (*off as usize, *off as usize + bytes.len());
            if end <= data.len() {
                data[o..end].copy_from_slice(bytes);
            }
        }
        let ext_text = String::from_utf8_lossy(&data).into_owned();
        main = merge_extended(&main, &ext_text).unwrap_or(main);
    }
    Some(main)
}

/// Inserts the `rdf:RDF` children of `ext` before `</rdf:RDF>` in `main`.
fn merge_extended(main: &str, ext: &str) -> Option<String> {
    let open = ext.find("<rdf:RDF")?;
    let body_start = open + ext[open..].find('>')? + 1;
    let body_end = ext.rfind("</rdf:RDF>")?;
    let close = main.rfind("</rdf:RDF>")?;
    let mut out = String::with_capacity(main.len() + (body_end - body_start));
    out.push_str(&main[..close]);
    out.push_str(ext.get(body_start..body_end)?);
    out.push_str(&main[close..]);
    Some(out)
}

/// The XMP packet embedded in the file, if any (JPEG APP1 `http://ns.adobe.com/xap/1.0/`
/// incl. extended XMP, TIFF tag 700, PNG `iTXt` `XML:com.adobe.xmp`, HEIC `mime` item).
/// Read-only: Sieve never writes into originals.
pub fn embedded_xmp(path: &Path, format: ImageFormat) -> Result<Option<String>, String> {
    let src = FileSource::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let text = match format {
        ImageFormat::Jpeg => jpeg_xmp(&jpeg_header(&src)?),
        ImageFormat::Tiff => tiff::Tiff::new(&src)?
            .ifd0_tag_bytes(XMP_TAG, MAX_XMP)?
            .map(|b| String::from_utf8_lossy(&b).trim_end_matches('\0').to_owned()),
        ImageFormat::Png => png::scan(&src)?.xmp,
        ImageFormat::Heic => heif::parse(&src)?.xmp.map(|b| String::from_utf8_lossy(&b).into_owned()),
        f => return Err(format!("{}: {} is a RAW format", path.display(), f.as_str())),
    };
    Ok(text.filter(|t| t.contains("xmpmeta") || t.contains("rdf:RDF")))
}

#[cfg(test)]
pub(crate) mod tests;
