//! Minimal FFI to the TurboJPEG 3 API (libjpeg-turbo >= 3.0, linked dynamically by
//! `build.rs`; Homebrew's `libraw` already depends on `jpeg-turbo`).
//!
//! Chosen over pure-Rust codecs for ingest because it has NEON SIMD on Apple Silicon,
//! DCT-domain scaling in 1/8 steps (decode a 4608 px preview straight to 2304 px), and
//! never spawns threads of its own, so memory stays proportional to the pool size.
//! Handles are cached per thread.

use std::cell::RefCell;
use std::ffi::{c_char, c_int, c_uchar, c_void, CStr};

use super::jpeg;

type Handle = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct ScalingFactor {
    num: c_int,
    denom: c_int,
}

const TJINIT_COMPRESS: c_int = 0;
const TJINIT_DECOMPRESS: c_int = 1;
const TJPARAM_QUALITY: c_int = 3;
const TJPARAM_SUBSAMP: c_int = 4;
const TJPARAM_JPEGWIDTH: c_int = 5;
const TJPARAM_JPEGHEIGHT: c_int = 6;
const TJPARAM_FASTDCT: c_int = 10;
const TJPARAM_MAXPIXELS: c_int = 24;
const TJPF_RGB: c_int = 0;
const TJSAMP_444: c_int = 0;
const TJSAMP_420: c_int = 2;
const TJERR_WARNING: c_int = 0;

extern "C" {
    fn tj3Init(init_type: c_int) -> Handle;
    fn tj3Destroy(handle: Handle);
    fn tj3GetErrorStr(handle: Handle) -> *mut c_char;
    fn tj3GetErrorCode(handle: Handle) -> c_int;
    fn tj3Set(handle: Handle, param: c_int, value: c_int) -> c_int;
    fn tj3Get(handle: Handle, param: c_int) -> c_int;
    fn tj3Free(buffer: *mut c_void);
    fn tj3DecompressHeader(handle: Handle, jpeg: *const c_uchar, size: usize) -> c_int;
    fn tj3SetScalingFactor(handle: Handle, factor: ScalingFactor) -> c_int;
    fn tj3Decompress8(
        handle: Handle,
        jpeg: *const c_uchar,
        size: usize,
        dst: *mut c_uchar,
        pitch: c_int,
        pixel_format: c_int,
    ) -> c_int;
    fn tj3Compress8(
        handle: Handle,
        src: *const c_uchar,
        width: c_int,
        pitch: c_int,
        height: c_int,
        pixel_format: c_int,
        jpeg: *mut *mut c_uchar,
        size: *mut usize,
    ) -> c_int;
}

struct Tj(Handle);

impl Tj {
    fn new(kind: c_int) -> Result<Self, String> {
        // SAFETY: plain constructor; null on failure.
        let h = unsafe { tj3Init(kind) };
        if h.is_null() {
            Err("TurboJPEG: init failed".into())
        } else {
            Ok(Self(h))
        }
    }

    fn error(&self, what: &str) -> String {
        // SAFETY: handle is live; returns a NUL-terminated string owned by the handle.
        let msg = unsafe { CStr::from_ptr(tj3GetErrorStr(self.0)) }.to_string_lossy();
        format!("TurboJPEG {what}: {msg}")
    }

    fn set(&self, param: c_int, value: c_int) -> Result<(), String> {
        // SAFETY: handle is live.
        if unsafe { tj3Set(self.0, param, value) } != 0 {
            return Err(self.error("set"));
        }
        Ok(())
    }
}

impl Drop for Tj {
    fn drop(&mut self) {
        // SAFETY: handle came from tj3Init and is destroyed once.
        unsafe { tj3Destroy(self.0) }
    }
}

thread_local! {
    static DECOMPRESSOR: RefCell<Option<Tj>> = const { RefCell::new(None) };
    static COMPRESSOR: RefCell<Option<Tj>> = const { RefCell::new(None) };
}

fn with_handle<T>(
    slot: &'static std::thread::LocalKey<RefCell<Option<Tj>>>,
    kind: c_int,
    f: impl FnOnce(&Tj) -> Result<T, String>,
) -> Result<T, String> {
    slot.with(|cell| {
        let mut guard = cell.borrow_mut();
        if guard.is_none() {
            *guard = Some(Tj::new(kind)?);
        }
        f(guard.as_ref().expect("initialised above"))
    })
}

/// Decoded 8-bit RGB.
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Header-only size check.
pub fn dimensions(jpeg: &[u8]) -> Result<(u32, u32), String> {
    with_handle(&DECOMPRESSOR, TJINIT_DECOMPRESS, |tj| {
        // SAFETY: buffer pointer/len describe a live slice.
        if unsafe { tj3DecompressHeader(tj.0, jpeg.as_ptr(), jpeg.len()) } != 0 {
            return Err(tj.error("header"));
        }
        // SAFETY: header was read successfully.
        let (w, h) = unsafe { (tj3Get(tj.0, TJPARAM_JPEGWIDTH), tj3Get(tj.0, TJPARAM_JPEGHEIGHT)) };
        if w <= 0 || h <= 0 {
            return Err("TurboJPEG: invalid dimensions".into());
        }
        Ok((w as u32, h as u32))
    })
}

/// Smallest `n/8` scale whose long edge is still >= `min_long_edge` (1/1 if smaller).
pub fn scale_for(long_edge: u32, min_long_edge: u32) -> (u32, u32) {
    if long_edge <= min_long_edge {
        return (1, 1);
    }
    let num = (min_long_edge as u64 * 8).div_ceil(long_edge as u64).clamp(1, 8) as u32;
    // TurboJPEG matches factors exactly in reduced form (4/8 is listed as 1/2).
    let g = gcd(num, 8);
    (num / g, 8 / g)
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

fn scaled(dim: u32, (num, denom): (u32, u32)) -> u32 {
    (dim * num).div_ceil(denom)
}

/// Decodes to RGB, DCT-scaled so the long edge is the smallest n/8 size >= `min_long_edge`.
/// `max_pixels` bounds the *source* frame (guards memory on corrupt headers).
pub fn decode_rgb(jpeg: &[u8], min_long_edge: u32, max_pixels: u64) -> Result<Decoded, String> {
    let mut pixels = Vec::new();
    let (width, height) = decode_rgb_into(jpeg, min_long_edge, max_pixels, &mut pixels)?;
    Ok(Decoded { width, height, pixels })
}

/// Like [`decode_rgb`], but decodes into `out` (resized to fit, capacity reused) and
/// returns the output size.
pub fn decode_rgb_into(
    jpeg: &[u8],
    min_long_edge: u32,
    max_pixels: u64,
    out: &mut Vec<u8>,
) -> Result<(u32, u32), String> {
    with_handle(&DECOMPRESSOR, TJINIT_DECOMPRESS, |tj| {
        // SAFETY: buffer pointer/len describe a live slice.
        if unsafe { tj3DecompressHeader(tj.0, jpeg.as_ptr(), jpeg.len()) } != 0 {
            return Err(tj.error("header"));
        }
        // SAFETY: header was read successfully.
        let (w, h) = unsafe { (tj3Get(tj.0, TJPARAM_JPEGWIDTH), tj3Get(tj.0, TJPARAM_JPEGHEIGHT)) };
        if w <= 0 || h <= 0 {
            return Err("TurboJPEG: invalid dimensions".into());
        }
        let (w, h) = (w as u32, h as u32);
        // A truncated/garbage header can leave the handle reporting the previous
        // image's size; cross-check against our own marker parse.
        if jpeg::header(jpeg) != (jpeg::Header::Frame { width: w as u16, height: h as u16 }) {
            return Err("TurboJPEG: header mismatch (corrupt JPEG)".into());
        }
        if w as u64 * h as u64 > max_pixels {
            return Err(format!("JPEG too large ({w}x{h})"));
        }
        tj.set(TJPARAM_MAXPIXELS, 0)?;
        let factor = scale_for(w.max(h), min_long_edge);
        let sf = ScalingFactor { num: factor.0 as c_int, denom: factor.1 as c_int };
        // SAFETY: handle is live; reduced n/8 factors are always supported.
        if unsafe { tj3SetScalingFactor(tj.0, sf) } != 0 {
            return Err(tj.error("scale"));
        }
        let (dw, dh) = (scaled(w, factor), scaled(h, factor));
        out.clear();
        out.resize(dw as usize * dh as usize * 3, 0);
        // SAFETY: `out` holds dh rows of dw*3 bytes, matching pitch and the scaled size.
        let rc =
            unsafe { tj3Decompress8(tj.0, jpeg.as_ptr(), jpeg.len(), out.as_mut_ptr(), (dw * 3) as c_int, TJPF_RGB) };
        // Non-fatal warnings (e.g. a few corrupt MCUs) still produce a usable image.
        // SAFETY: handle is live.
        if rc != 0 && unsafe { tj3GetErrorCode(tj.0) } != TJERR_WARNING {
            return Err(tj.error("decode"));
        }
        Ok((dw, dh))
    })
}

/// Encodes interleaved RGB as a 4:2:0 JPEG.
pub fn encode_rgb(pixels: &[u8], width: u32, height: u32, quality: u8) -> Result<Vec<u8>, String> {
    encode_rgb_with(pixels, width, height, quality, |bytes| Ok(bytes.to_vec()))
}

/// Encodes and hands the JPEG bytes (owned by TurboJPEG) to `sink`, avoiding a copy.
pub fn encode_rgb_with<T>(
    pixels: &[u8],
    width: u32,
    height: u32,
    quality: u8,
    sink: impl FnOnce(&[u8]) -> Result<T, String>,
) -> Result<T, String> {
    encode_rgb_sub(pixels, width, height, quality, false, sink)
}

/// Encodes interleaved RGB as a 4:4:4 JPEG (no chroma subsampling; editor previews).
pub fn encode_rgb_444(pixels: &[u8], width: u32, height: u32, quality: u8) -> Result<Vec<u8>, String> {
    encode_rgb_sub(pixels, width, height, quality, true, |bytes| Ok(bytes.to_vec()))
}

fn encode_rgb_sub<T>(
    pixels: &[u8],
    width: u32,
    height: u32,
    quality: u8,
    full_chroma: bool,
    sink: impl FnOnce(&[u8]) -> Result<T, String>,
) -> Result<T, String> {
    if width == 0 || height == 0 || pixels.len() < width as usize * height as usize * 3 {
        return Err("TurboJPEG encode: bad buffer".into());
    }
    with_handle(&COMPRESSOR, TJINIT_COMPRESS, |tj| {
        tj.set(TJPARAM_QUALITY, quality.clamp(1, 100) as c_int)?;
        tj.set(TJPARAM_SUBSAMP, if full_chroma { TJSAMP_444 } else { TJSAMP_420 })?;
        tj.set(TJPARAM_FASTDCT, 1)?;
        let mut buf: *mut c_uchar = std::ptr::null_mut();
        let mut size: usize = 0;
        // SAFETY: src holds height rows of width*3 bytes; TurboJPEG allocates `buf`.
        let rc = unsafe {
            tj3Compress8(
                tj.0,
                pixels.as_ptr(),
                width as c_int,
                (width * 3) as c_int,
                height as c_int,
                TJPF_RGB,
                &mut buf,
                &mut size,
            )
        };
        let out = if rc == 0 && !buf.is_null() {
            // SAFETY: TurboJPEG wrote `size` bytes into `buf`, freed below after use.
            sink(unsafe { std::slice::from_raw_parts(buf, size) })
        } else {
            Err(tj.error("encode"))
        };
        if !buf.is_null() {
            // SAFETY: buffer was allocated by TurboJPEG.
            unsafe { tj3Free(buf.cast()) };
        }
        out
    })
}

/// `TJPF_GRAY` / `TJSAMP_GRAY`.
const TJPF_GRAY: c_int = 6;
const TJSAMP_GRAY: c_int = 3;

/// Encodes 8-bit greyscale as a single-component JPEG (mask overlays).
pub fn encode_gray(pixels: &[u8], width: u32, height: u32, quality: u8) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 || pixels.len() < width as usize * height as usize {
        return Err("TurboJPEG encode: bad buffer".into());
    }
    with_handle(&COMPRESSOR, TJINIT_COMPRESS, |tj| {
        tj.set(TJPARAM_QUALITY, quality.clamp(1, 100) as c_int)?;
        tj.set(TJPARAM_SUBSAMP, TJSAMP_GRAY)?;
        tj.set(TJPARAM_FASTDCT, 1)?;
        let mut buf: *mut c_uchar = std::ptr::null_mut();
        let mut size: usize = 0;
        // SAFETY: src holds height rows of width bytes; TurboJPEG allocates `buf`.
        let rc = unsafe {
            tj3Compress8(tj.0, pixels.as_ptr(), width as c_int, width as c_int, height as c_int, TJPF_GRAY, &mut buf, &mut size)
        };
        let out = if rc == 0 && !buf.is_null() {
            // SAFETY: TurboJPEG wrote `size` bytes into `buf`, freed below.
            Ok(unsafe { std::slice::from_raw_parts(buf, size) }.to_vec())
        } else {
            Err(tj.error("encode"))
        };
        if !buf.is_null() {
            // SAFETY: allocated by TurboJPEG.
            unsafe { tj3Free(buf.cast()) };
        }
        // Later RGB encodes set their own subsampling.
        out
    })
}

const TJPARAM_OPTIMIZE: c_int = 11;
const TJPARAM_XDENSITY: c_int = 20;
const TJPARAM_YDENSITY: c_int = 21;
const TJPARAM_DENSITYUNITS: c_int = 22;
const TJSAMP_422: c_int = 1;

thread_local! {
    /// Separate handle for exports: their density/quality settings never leak into the
    /// preview/ingest encoder.
    static EXPORT_COMPRESSOR: RefCell<Option<Tj>> = const { RefCell::new(None) };
}

/// JPEG chroma subsampling for [`encode_export`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subsampling {
    S444,
    S422,
    S420,
}

/// Export-quality encode of interleaved RGB8: accurate DCT, optimized Huffman tables, JFIF
/// density `ppi` dots per inch. The bytes (owned by TurboJPEG) are handed to `sink`.
pub fn encode_export<T>(
    pixels: &[u8],
    width: u32,
    height: u32,
    quality: u8,
    subsampling: Subsampling,
    ppi: u32,
    sink: impl FnOnce(&[u8]) -> Result<T, String>,
) -> Result<T, String> {
    if width == 0 || height == 0 || pixels.len() < width as usize * height as usize * 3 {
        return Err("TurboJPEG encode: bad buffer".into());
    }
    with_handle(&EXPORT_COMPRESSOR, TJINIT_COMPRESS, |tj| {
        tj.set(TJPARAM_QUALITY, quality.clamp(1, 100) as c_int)?;
        let samp = match subsampling {
            Subsampling::S444 => TJSAMP_444,
            Subsampling::S422 => TJSAMP_422,
            Subsampling::S420 => TJSAMP_420,
        };
        tj.set(TJPARAM_SUBSAMP, samp)?;
        tj.set(TJPARAM_FASTDCT, 0)?;
        tj.set(TJPARAM_OPTIMIZE, 1)?;
        let density = ppi.clamp(1, 65535) as c_int;
        tj.set(TJPARAM_DENSITYUNITS, 1)?;
        tj.set(TJPARAM_XDENSITY, density)?;
        tj.set(TJPARAM_YDENSITY, density)?;
        let mut buf: *mut c_uchar = std::ptr::null_mut();
        let mut size: usize = 0;
        // SAFETY: src holds height rows of width*3 bytes; TurboJPEG allocates `buf`.
        let rc = unsafe {
            tj3Compress8(
                tj.0,
                pixels.as_ptr(),
                width as c_int,
                (width * 3) as c_int,
                height as c_int,
                TJPF_RGB,
                &mut buf,
                &mut size,
            )
        };
        let out = if rc == 0 && !buf.is_null() {
            // SAFETY: TurboJPEG wrote `size` bytes into `buf`, freed below after use.
            sink(unsafe { std::slice::from_raw_parts(buf, size) })
        } else {
            Err(tj.error("encode"))
        };
        if !buf.is_null() {
            // SAFETY: buffer was allocated by TurboJPEG.
            unsafe { tj3Free(buf.cast()) };
        }
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_selection() {
        assert_eq!(scale_for(4608, 2048), (1, 2)); // -> 2304
        assert_eq!(scale_for(6000, 2048), (3, 8)); // -> 2250
        assert_eq!(scale_for(8192, 2048), (1, 4)); // -> 2048
        assert_eq!(scale_for(1616, 2048), (1, 1));
        assert_eq!(scale_for(100_000, 2048), (1, 8));
    }

    #[test]
    fn round_trip() {
        let (w, h) = (640u32, 480u32);
        let px: Vec<u8> = (0..w * h).flat_map(|i| [(i % 256) as u8, 128, 64]).collect();
        let jpeg = encode_rgb(&px, w, h, 90).unwrap();
        assert_eq!(dimensions(&jpeg).unwrap(), (w, h));
        let d = decode_rgb(&jpeg, 200, u64::MAX).unwrap();
        assert_eq!((d.width, d.height), (240, 180), "3/8 is the smallest >= 200");
        assert_eq!(d.pixels.len(), 240 * 180 * 3);
        assert!(decode_rgb(&jpeg, 200, 1000).is_err(), "max_pixels");
        assert!(decode_rgb(b"\xFF\xD8\xFF\xE0garbage", 10, u64::MAX).is_err());
        assert!(encode_rgb(&px[..10], w, h, 90).is_err());
    }
}
