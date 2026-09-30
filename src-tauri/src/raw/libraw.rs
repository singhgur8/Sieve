//! Minimal hand-written FFI to the LibRaw C API (thread-safe `libraw_r`, linked
//! dynamically by `build.rs`). Only opaque handles and accessor functions are used, so
//! no struct layouts beyond `libraw_processed_image_t`'s fixed header are mirrored.
//!
//! Phase 2 uses it as a thumbnail fallback; Phases 5/6 decode linear camera RGB with
//! `unpack` + `dcraw_process` (half size for the editor, full demosaic for export).

use std::ffi::{c_char, c_int, c_uint, c_ushort, CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

#[repr(C)]
struct LibrawData {
    _private: [u8; 0],
}

/// `libraw_processed_image_t` header; pixel/JPEG data follows `data_size` bytes from `data`.
#[repr(C)]
struct ProcessedImage {
    typ: c_int,
    height: c_ushort,
    width: c_ushort,
    colors: c_ushort,
    bits: c_ushort,
    data_size: c_uint,
    data: [u8; 1],
}

const LIBRAW_IMAGE_JPEG: c_int = 1;
const LIBRAW_IMAGE_BITMAP: c_int = 2;

extern "C" {
    fn libraw_init(flags: c_uint) -> *mut LibrawData;
    fn libraw_open_file(lr: *mut LibrawData, path: *const c_char) -> c_int;
    fn libraw_unpack_thumb(lr: *mut LibrawData) -> c_int;
    fn libraw_dcraw_make_mem_thumb(lr: *mut LibrawData, errc: *mut c_int) -> *mut ProcessedImage;
    fn libraw_dcraw_clear_mem(img: *mut ProcessedImage);
    fn libraw_close(lr: *mut LibrawData);
    fn libraw_strerror(errorcode: c_int) -> *const c_char;
    fn libraw_version() -> *const c_char;
    fn libraw_unpack(lr: *mut LibrawData) -> c_int;
    fn libraw_dcraw_process(lr: *mut LibrawData) -> c_int;
    fn libraw_dcraw_make_mem_image(lr: *mut LibrawData, errc: *mut c_int) -> *mut ProcessedImage;
    // native/libraw_shim.c
    fn sieve_lr_set_linear(lr: *mut LibrawData, half_size: c_int);
    fn sieve_lr_get_color(lr: *mut LibrawData, out: *mut ShimColor);
    fn sieve_lr_set_white(lr: *mut LibrawData, white: c_uint);
    fn sieve_lr_copy_rgb16(lr: *mut LibrawData, out: *mut u16, cap: usize, w: *mut c_int, h: *mut c_int) -> c_int;
}

/// Mirror of `sieve_lr_color_t` in `native/libraw_shim.c`.
#[repr(C)]
struct ShimColor {
    cam_mul: [f32; 4],
    pre_mul: [f32; 4],
    rgb_cam: [[f32; 4]; 3],
    cam_xyz: [[f32; 3]; 4],
    black: c_uint,
    maximum: c_uint,
    width: c_int,
    height: c_int,
    flip: c_int,
    colors: c_int,
    filters: c_uint,
    make: [c_char; 64],
    model: [c_char; 64],
    fuji_expo_shift: f32,
    dng_baseline_exposure: f32,
    linear_max: c_uint,
    wb_daylight: [c_int; 4],
    wb_d65: [c_int; 4],
}

impl Default for ShimColor {
    fn default() -> Self {
        // SAFETY: plain-old-data C struct (floats, integers, char arrays); all-zero is valid.
        unsafe { std::mem::zeroed() }
    }
}

fn c_name(b: &[c_char; 64]) -> String {
    let bytes: Vec<u8> = b.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).trim().to_owned()
}

/// Colour metadata of a RAW as LibRaw sees it (read right after `open_file`).
#[derive(Debug, Clone, PartialEq)]
pub struct ColorData {
    /// As-shot white balance multipliers (R, G, B, G2); zeros if unrecorded.
    pub cam_mul: [f32; 4],
    /// Daylight multipliers derived from the colour matrix.
    pub pre_mul: [f32; 4],
    /// White-balanced camera RGB -> linear sRGB (D65); rows sum to 1.
    pub rgb_cam: [[f32; 3]; 3],
    /// XYZ (D65-referred Adobe `ColorMatrix`) -> camera RGB; zeros if unknown.
    pub cam_xyz: [[f32; 3]; 3],
    /// Full-size output dimensions (before rotation).
    pub width: u32,
    pub height: u32,
    /// LibRaw's orientation code (0, 3, 5, 6).
    pub flip: i32,
    pub colors: i32,
    /// CFA pattern code (9 = X-Trans).
    pub filters: u32,
    /// LibRaw's normalized make / model ("Sony" / "ILCE-7M4"; empty if unknown).
    pub make: String,
    pub model: String,
    /// Fujifilm `RawExposureBias` (EV; 0 if none) and a DNG's `BaselineExposure`.
    pub fuji_expo_shift: f32,
    pub dng_baseline_exposure: f32,
    /// Camera white-balance presets (R, G, B, G2 coefficients; zeros if absent): Daylight
    /// (EXIF light source 1) and D65 (21).
    pub wb_daylight: [i32; 4],
    pub wb_d65: [i32; 4],
}

/// Linear 16-bit camera RGB (no white balance, black-subtracted, white level = 65535),
/// interleaved RGB, not rotated.
#[derive(Debug, Clone)]
pub struct LinearRgb16 {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u16>,
    pub color: ColorData,
}

/// Decodes `path` with LibRaw into linear camera RGB (`half_size`: one pixel per CFA quad).
pub fn decode_linear(path: &Path, half_size: bool) -> Result<LinearRgb16, String> {
    let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|_| "path contains NUL".to_owned())?;
    let h = Handle::new()?;
    // SAFETY: h.0 is a live handle; c_path outlives the call.
    let rc = unsafe { libraw_open_file(h.0, c_path.as_ptr()) };
    if rc != 0 {
        return Err(err(rc));
    }
    let mut c = ShimColor::default();
    // SAFETY: file opened; the shim only reads/writes plain fields of the live handle.
    unsafe {
        sieve_lr_get_color(h.0, &mut c);
        sieve_lr_set_linear(h.0, c_int::from(half_size));
    }
    // SAFETY: opened above.
    let rc = unsafe { libraw_unpack(h.0) };
    if rc != 0 {
        return Err(err(rc));
    }
    if let Some(white) = adobe_white_level(&c) {
        // SAFETY: unpacked handle; sets a processing parameter only.
        unsafe { sieve_lr_set_white(h.0, white) };
    }
    // SAFETY: unpacked above.
    let rc = unsafe { libraw_dcraw_process(h.0) };
    if rc != 0 {
        return Err(err(rc));
    }
    let color = color_data(&c);
    // Fast path: copy LibRaw's processed image straight into our buffer (no intermediate
    // full-size mem image; peak memory = raw + image + ours).
    let (mut w, mut hgt): (c_int, c_int) = (0, 0);
    // SAFETY: processed above; a null `out` only queries the size.
    if unsafe { sieve_lr_copy_rgb16(h.0, std::ptr::null_mut(), 0, &mut w, &mut hgt) } == 0 && w > 0 && hgt > 0 {
        let mut pixels = vec![0u16; w as usize * hgt as usize * 3];
        // SAFETY: `pixels` holds w*h*3 values, as passed in `cap`.
        let rc = unsafe { sieve_lr_copy_rgb16(h.0, pixels.as_mut_ptr(), pixels.len(), &mut w, &mut hgt) };
        if rc == 0 {
            return Ok(LinearRgb16 { width: w as u32, height: hgt as u32, pixels, color });
        }
    }
    let mut code: c_int = 0;
    // SAFETY: processed above; the buffer is freed with dcraw_clear_mem below.
    let img = unsafe { libraw_dcraw_make_mem_image(h.0, &mut code) };
    if img.is_null() {
        return Err(err(code));
    }
    // SAFETY: img is non-null; `data` holds `data_size` bytes allocated by LibRaw.
    let result = unsafe {
        let r = &*img;
        let (w, hgt) = (r.width as usize, r.height as usize);
        if r.typ != LIBRAW_IMAGE_BITMAP || r.colors != 3 || r.bits != 16 {
            Err(format!("LibRaw: unexpected develop image (type {}, {} colors, {} bits)", r.typ, r.colors, r.bits))
        } else if (r.data_size as usize) < w * hgt * 6 {
            Err("LibRaw: short develop image".to_owned())
        } else {
            let mut pixels = vec![0u16; w * hgt * 3];
            std::ptr::copy_nonoverlapping(
                std::ptr::addr_of!(r.data).cast::<u8>(),
                pixels.as_mut_ptr().cast::<u8>(),
                pixels.len() * 2,
            );
            Ok(LinearRgb16 { width: w as u32, height: hgt as u32, pixels, color })
        }
    };
    // SAFETY: img came from dcraw_make_mem_image and is freed once.
    unsafe { libraw_dcraw_clear_mem(img) };
    result
}

/// Test helper: the same decode through `dcraw_make_mem_image` (the pre-Phase 6 path).
#[cfg(test)]
fn decode_linear_mem_image(path: &Path, half_size: bool) -> Result<Vec<u16>, String> {
    let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|_| "path contains NUL".to_owned())?;
    let h = Handle::new()?;
    // SAFETY: as in `decode_linear`.
    unsafe {
        if libraw_open_file(h.0, c_path.as_ptr()) != 0 {
            return Err("open".into());
        }
        sieve_lr_set_linear(h.0, c_int::from(half_size));
        if libraw_unpack(h.0) != 0 || libraw_dcraw_process(h.0) != 0 {
            return Err("process".into());
        }
        let mut code: c_int = 0;
        let img = libraw_dcraw_make_mem_image(h.0, &mut code);
        if img.is_null() {
            return Err(err(code));
        }
        let r = &*img;
        let n = r.width as usize * r.height as usize * 3;
        let mut pixels = vec![0u16; n];
        std::ptr::copy_nonoverlapping(std::ptr::addr_of!(r.data).cast::<u8>(), pixels.as_mut_ptr().cast::<u8>(), n * 2);
        libraw_dcraw_clear_mem(img);
        Ok(pixels)
    }
}

/// White level Adobe's raw pipeline uses when it differs from LibRaw's: for Canon, Camera
/// Raw / DNG Converter clip at the sensor's highlight linearity limit (e.g. 13660 for the
/// EOS M6 Mark II, LibRaw's `linear_max`) rather than the ADC maximum, so the same raw
/// value is ~0.3 EV brighter there. Other makes keep LibRaw's level (Sony's linearity
/// limit, 15360, is *not* what Adobe uses; its DNGs keep 16383).
fn adobe_white_level(c: &ShimColor) -> Option<c_uint> {
    let canon = c_name(&c.make).eq_ignore_ascii_case("canon");
    (canon && c.linear_max > 1024 && c.linear_max < c.maximum).then_some(c.linear_max)
}

fn color_data(c: &ShimColor) -> ColorData {
    let mut rgb_cam = [[0.0; 3]; 3];
    let mut cam_xyz = [[0.0; 3]; 3];
    for i in 0..3 {
        rgb_cam[i].copy_from_slice(&c.rgb_cam[i][..3]);
        cam_xyz[i] = c.cam_xyz[i];
    }
    ColorData {
        cam_mul: c.cam_mul,
        pre_mul: c.pre_mul,
        rgb_cam,
        cam_xyz,
        width: c.width.max(0) as u32,
        height: c.height.max(0) as u32,
        flip: c.flip,
        colors: c.colors,
        filters: c.filters,
        make: c_name(&c.make),
        model: c_name(&c.model),
        fuji_expo_shift: if c.fuji_expo_shift.is_finite() { c.fuji_expo_shift } else { 0.0 },
        dng_baseline_exposure: if c.dng_baseline_exposure.is_finite() { c.dng_baseline_exposure } else { 0.0 },
        wb_daylight: c.wb_daylight,
        wb_d65: c.wb_d65,
    }
}

/// LibRaw version string, e.g. "0.21.4-Release".
pub fn version() -> String {
    // SAFETY: returns a pointer to a static NUL-terminated string.
    unsafe { CStr::from_ptr(libraw_version()) }.to_string_lossy().into_owned()
}

fn err(code: c_int) -> String {
    // SAFETY: libraw_strerror returns a static string for any code.
    let msg = unsafe { CStr::from_ptr(libraw_strerror(code)) }.to_string_lossy();
    format!("LibRaw: {msg} ({code})")
}

/// Owned LibRaw handle; closed on drop.
struct Handle(*mut LibrawData);

impl Handle {
    fn new() -> Result<Self, String> {
        // SAFETY: plain constructor; null means allocation failure.
        let p = unsafe { libraw_init(0) };
        if p.is_null() {
            Err("LibRaw: init failed".into())
        } else {
            Ok(Self(p))
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: self.0 came from libraw_init and is closed exactly once.
        unsafe { libraw_close(self.0) }
    }
}

/// A thumbnail as LibRaw provides it.
pub enum Thumb {
    Jpeg(Vec<u8>),
    /// 8-bit interleaved RGB.
    Rgb {
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    },
}

/// Extracts the camera's largest embedded thumbnail via `unpack_thumb`.
pub fn thumbnail(path: &Path) -> Result<Thumb, String> {
    let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|_| "path contains NUL".to_owned())?;
    let h = Handle::new()?;
    // SAFETY: h.0 is a live handle; c_path outlives the call.
    let rc = unsafe { libraw_open_file(h.0, c_path.as_ptr()) };
    if rc != 0 {
        return Err(err(rc));
    }
    // SAFETY: file opened successfully above.
    let rc = unsafe { libraw_unpack_thumb(h.0) };
    if rc != 0 {
        return Err(err(rc));
    }
    let mut code: c_int = 0;
    // SAFETY: thumbnail unpacked; the returned buffer is freed with dcraw_clear_mem below.
    let img = unsafe { libraw_dcraw_make_mem_thumb(h.0, &mut code) };
    if img.is_null() {
        return Err(err(code));
    }
    // SAFETY: img is non-null; `data` holds `data_size` bytes allocated by LibRaw.
    let result = unsafe {
        let r = &*img;
        let bytes = std::slice::from_raw_parts(std::ptr::addr_of!(r.data).cast::<u8>(), r.data_size as usize);
        match r.typ {
            LIBRAW_IMAGE_JPEG => Ok(Thumb::Jpeg(bytes.to_vec())),
            LIBRAW_IMAGE_BITMAP if r.colors == 3 && r.bits == 8 => {
                Ok(Thumb::Rgb { width: r.width as u32, height: r.height as u32, pixels: bytes.to_vec() })
            }
            t => Err(format!("LibRaw: unsupported thumbnail format (type {t}, {} colors, {} bits)", r.colors, r.bits)),
        }
    };
    // SAFETY: img came from dcraw_make_mem_thumb and is freed once.
    unsafe { libraw_dcraw_clear_mem(img) };
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_and_reports_errors() {
        assert!(version().starts_with("0."), "{}", version());
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.arw");
        std::fs::write(&p, b"not a raw file at all").unwrap();
        let e = thumbnail(&p).err().expect("garbage must fail");
        assert!(e.starts_with("LibRaw:"), "{e}");
    }

    /// The direct image copy equals `dcraw_make_mem_image`'s output (identity curve).
    #[test]
    #[ignore = "needs sample RAWs ($SIEVE_SAMPLES)"]
    fn direct_copy_matches_mem_image() {
        let folder = std::env::var("SIEVE_SAMPLES").unwrap_or_else(|_| "/Users/gurjotsingh/Pictures/test RAWS".into());
        let raw = std::fs::read_dir(&folder)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| crate::raw::format_from_extension(p).is_some())
            .expect("no RAW in sample folder");
        let a = decode_linear(&raw, true).unwrap();
        let b = decode_linear_mem_image(&raw, true).unwrap();
        assert_eq!(a.pixels.len(), b.len());
        let max = a.pixels.iter().zip(&b).map(|(x, y)| (i32::from(*x) - i32::from(*y)).abs()).max().unwrap();
        assert!(max <= 1, "max diff {max}");
    }

    /// The fallback path on a real RAW (read-only): LibRaw's pick must be a valid JPEG.
    #[test]
    #[ignore = "needs sample RAWs ($SIEVE_SAMPLES)"]
    fn real_sample_thumbnail() {
        let folder = std::env::var("SIEVE_SAMPLES").unwrap_or_else(|_| "/Users/gurjotsingh/Pictures/test RAWS".into());
        let raw = std::fs::read_dir(&folder)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| crate::raw::format_from_extension(p).is_some())
            .expect("no RAW in sample folder");
        match thumbnail(&raw).unwrap() {
            Thumb::Jpeg(bytes) => {
                assert!(matches!(crate::raw::jpeg::header(&bytes), crate::raw::jpeg::Header::Frame { .. }))
            }
            Thumb::Rgb { width, height, pixels } => assert_eq!(pixels.len(), width as usize * height as usize * 3),
        }
    }
}
