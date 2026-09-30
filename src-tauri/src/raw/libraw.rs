//! Minimal hand-written FFI to the LibRaw C API (thread-safe `libraw_r`, linked
//! dynamically by `build.rs`). Only opaque handles and accessor functions are used, so
//! no struct layouts beyond `libraw_processed_image_t`'s fixed header are mirrored.
//!
//! Phase 2 uses it as a thumbnail fallback; Phase 6 will add `unpack` +
//! `dcraw_process` + `dcraw_make_mem_image` through the same handle type.

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

    /// The fallback path on a real RAW (read-only): LibRaw's pick must be a valid JPEG.
    #[test]
    #[ignore = "needs sample RAWs ($LUMENRAW_SAMPLES)"]
    fn real_sample_thumbnail() {
        let folder =
            std::env::var("LUMENRAW_SAMPLES").unwrap_or_else(|_| "/Users/gurjotsingh/Pictures/test RAWS".into());
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
