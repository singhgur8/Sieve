//! macOS ImageIO decoding for HEIC/HEIF and TIFF sources (Phase 7b), plus a
//! colour-managed thumbnail path for previews.
//!
//! - [`decode_native`]: the primary image drawn into a bitmap context *in its own colour
//!   space* (CoreGraphics does not convert when source and destination spaces are the same
//!   object), 8 or 16 bits per sample, plus the space's ICC data. Colour management then
//!   happens in `raw::raster` with our own ICC reader, so JPEG/PNG (decoded without
//!   ImageIO) and HEIC/TIFF share one linearization. Non-RGB/grey sources (CMYK, Lab,
//!   indexed) are converted to sRGB by ColorSync instead (`converted_to_srgb`).
//! - [`thumbnail_srgb`]: `CGImageSourceCreateThumbnailAtIndex` (never upscaled, no
//!   orientation transform) drawn into an 8-bit sRGB context.
//! - [`properties`]: pixel size and orientation of the primary image.
//!
//! Minimal hand-written FFI; every CF object is released through [`Cf`].

use std::path::Path;

/// Decoded samples in the image's own colour space.
#[derive(Debug, Clone, PartialEq)]
pub enum Samples {
    U8(Vec<u8>),
    U16(Vec<u16>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Native {
    pub width: u32,
    pub height: u32,
    /// Interleaved RGB (grey sources are expanded).
    pub samples: Samples,
    /// ICC profile of the image's colour space (None for device spaces).
    pub icc: Option<Vec<u8>>,
    /// Pixels were converted to sRGB by ColorSync (unsupported source model).
    pub converted_to_srgb: bool,
    /// Bits per component in the file (8, 10, 12, 16, ...).
    pub file_bits: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Properties {
    pub width: u32,
    pub height: u32,
    pub orientation: Option<u8>,
}

#[cfg(target_os = "macos")]
mod ffi {
    use std::ffi::c_void;

    pub type CFTypeRef = *const c_void;
    pub type CFIndex = isize;

    #[repr(C)]
    pub struct CFDictionaryKeyCallBacks {
        _opaque: [u8; 0],
    }
    #[repr(C)]
    pub struct CFDictionaryValueCallBacks {
        _opaque: [u8; 0],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct CGPoint {
        pub x: f64,
        pub y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct CGSize {
        pub width: f64,
        pub height: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct CGRect {
        pub origin: CGPoint,
        pub size: CGSize,
    }

    pub const K_CF_NUMBER_SINT64: CFIndex = 4;
    /// `kCGImageAlphaNone`, `kCGImageAlphaNoneSkipLast`, `kCGBitmapByteOrder16Little`.
    pub const ALPHA_NONE: u32 = 0;
    pub const ALPHA_NONE_SKIP_LAST: u32 = 5;
    pub const BYTE_ORDER_16_LITTLE: u32 = 1 << 12;
    /// `CGColorSpaceModel`.
    pub const MODEL_MONOCHROME: i32 = 0;
    pub const MODEL_RGB: i32 = 1;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        pub static kCFTypeDictionaryKeyCallBacks: CFDictionaryKeyCallBacks;
        pub static kCFTypeDictionaryValueCallBacks: CFDictionaryValueCallBacks;
        pub static kCFBooleanTrue: CFTypeRef;
        pub static kCFBooleanFalse: CFTypeRef;
        pub fn CFRelease(cf: CFTypeRef);
        pub fn CFNumberCreate(alloc: CFTypeRef, typ: CFIndex, value: *const c_void) -> CFTypeRef;
        pub fn CFNumberGetValue(n: CFTypeRef, typ: CFIndex, value: *mut c_void) -> bool;
        pub fn CFNumberGetTypeID() -> usize;
        pub fn CFGetTypeID(cf: CFTypeRef) -> usize;
        pub fn CFDictionaryCreate(
            alloc: CFTypeRef,
            keys: *const CFTypeRef,
            values: *const CFTypeRef,
            n: CFIndex,
            kcb: *const CFDictionaryKeyCallBacks,
            vcb: *const CFDictionaryValueCallBacks,
        ) -> CFTypeRef;
        pub fn CFDictionaryGetValue(d: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
        pub fn CFURLCreateFromFileSystemRepresentation(
            alloc: CFTypeRef,
            buf: *const u8,
            len: CFIndex,
            is_dir: u8,
        ) -> CFTypeRef;
        pub fn CFDataGetLength(d: CFTypeRef) -> CFIndex;
        pub fn CFDataGetBytePtr(d: CFTypeRef) -> *const u8;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        pub static kCGColorSpaceSRGB: CFTypeRef;
        pub fn CGColorSpaceCreateWithName(name: CFTypeRef) -> CFTypeRef;
        pub fn CGColorSpaceGetModel(space: CFTypeRef) -> i32;
        pub fn CGColorSpaceCopyICCData(space: CFTypeRef) -> CFTypeRef;
        pub fn CGImageGetWidth(image: CFTypeRef) -> usize;
        pub fn CGImageGetHeight(image: CFTypeRef) -> usize;
        pub fn CGImageGetBitsPerComponent(image: CFTypeRef) -> usize;
        pub fn CGImageGetColorSpace(image: CFTypeRef) -> CFTypeRef;
        pub fn CGBitmapContextCreate(
            data: *mut c_void,
            width: usize,
            height: usize,
            bits_per_component: usize,
            bytes_per_row: usize,
            space: CFTypeRef,
            bitmap_info: u32,
        ) -> CFTypeRef;
        pub fn CGContextDrawImage(ctx: CFTypeRef, rect: CGRect, image: CFTypeRef);
        pub fn CGContextSetBlendMode(ctx: CFTypeRef, mode: i32);
    }

    #[link(name = "ImageIO", kind = "framework")]
    extern "C" {
        pub static kCGImagePropertyPixelWidth: CFTypeRef;
        pub static kCGImagePropertyPixelHeight: CFTypeRef;
        pub static kCGImagePropertyOrientation: CFTypeRef;
        pub static kCGImageSourceCreateThumbnailFromImageAlways: CFTypeRef;
        pub static kCGImageSourceThumbnailMaxPixelSize: CFTypeRef;
        pub static kCGImageSourceCreateThumbnailWithTransform: CFTypeRef;
        pub static kCGImageSourceShouldCache: CFTypeRef;
        pub fn CGImageSourceCreateWithURL(url: CFTypeRef, options: CFTypeRef) -> CFTypeRef;
        pub fn CGImageSourceGetCount(src: CFTypeRef) -> usize;
        pub fn CGImageSourceGetPrimaryImageIndex(src: CFTypeRef) -> usize;
        pub fn CGImageSourceCreateImageAtIndex(src: CFTypeRef, index: usize, options: CFTypeRef) -> CFTypeRef;
        pub fn CGImageSourceCreateThumbnailAtIndex(src: CFTypeRef, index: usize, options: CFTypeRef) -> CFTypeRef;
        pub fn CGImageSourceCopyPropertiesAtIndex(src: CFTypeRef, index: usize, options: CFTypeRef) -> CFTypeRef;
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::ffi::*;
    use super::*;
    use std::ffi::c_void;

    /// Owned CF object (released on drop).
    pub struct Cf(pub CFTypeRef);

    impl Cf {
        pub fn new(p: CFTypeRef, what: &str) -> Result<Self, String> {
            if p.is_null() {
                Err(format!("ImageIO: {what} failed"))
            } else {
                Ok(Cf(p))
            }
        }
    }

    impl Drop for Cf {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: we own one reference.
                unsafe { CFRelease(self.0) }
            }
        }
    }

    fn number(v: i64) -> Result<Cf, String> {
        // SAFETY: v outlives the call; CFNumber copies it.
        Cf::new(
            unsafe { CFNumberCreate(std::ptr::null(), K_CF_NUMBER_SINT64, (&v as *const i64).cast::<c_void>()) },
            "number",
        )
    }

    fn dict(pairs: &[(CFTypeRef, CFTypeRef)]) -> Result<Cf, String> {
        let keys: Vec<CFTypeRef> = pairs.iter().map(|p| p.0).collect();
        let values: Vec<CFTypeRef> = pairs.iter().map(|p| p.1).collect();
        // SAFETY: keys/values are live CF objects; the dictionary retains them.
        Cf::new(
            unsafe {
                CFDictionaryCreate(
                    std::ptr::null(),
                    keys.as_ptr(),
                    values.as_ptr(),
                    keys.len() as CFIndex,
                    &kCFTypeDictionaryKeyCallBacks,
                    &kCFTypeDictionaryValueCallBacks,
                )
            },
            "dictionary",
        )
    }

    /// Opens an image source (no decode yet) and its primary index.
    pub fn source(path: &Path) -> Result<(Cf, usize), String> {
        use std::os::unix::ffi::OsStrExt;
        let bytes = path.as_os_str().as_bytes();
        // SAFETY: bytes are valid for the call.
        let url = Cf::new(
            unsafe {
                CFURLCreateFromFileSystemRepresentation(std::ptr::null(), bytes.as_ptr(), bytes.len() as CFIndex, 0)
            },
            "URL",
        )?;
        // SAFETY: url is live.
        let src = Cf::new(unsafe { CGImageSourceCreateWithURL(url.0, std::ptr::null()) }, "open image")?;
        // SAFETY: src is a live CGImageSource.
        let (count, primary) = unsafe { (CGImageSourceGetCount(src.0), CGImageSourceGetPrimaryImageIndex(src.0)) };
        if count == 0 {
            return Err(format!("{}: ImageIO found no image", path.display()));
        }
        Ok((src, primary.min(count - 1)))
    }

    fn dict_i64(d: CFTypeRef, key: CFTypeRef) -> Option<i64> {
        // SAFETY: d is a live CFDictionary; the value is borrowed.
        unsafe {
            let v = CFDictionaryGetValue(d, key);
            if v.is_null() || CFGetTypeID(v) != CFNumberGetTypeID() {
                return None;
            }
            let mut out: i64 = 0;
            CFNumberGetValue(v, K_CF_NUMBER_SINT64, (&mut out as *mut i64).cast::<c_void>()).then_some(out)
        }
    }

    pub fn properties(path: &Path) -> Result<Properties, String> {
        let (src, idx) = source(path)?;
        // SAFETY: src is live; returns +1 dictionary or null.
        let props = Cf::new(unsafe { CGImageSourceCopyPropertiesAtIndex(src.0, idx, std::ptr::null()) }, "properties")?;
        // SAFETY: the keys are ImageIO constants.
        let (w, h, o) = unsafe {
            (
                dict_i64(props.0, kCGImagePropertyPixelWidth),
                dict_i64(props.0, kCGImagePropertyPixelHeight),
                dict_i64(props.0, kCGImagePropertyOrientation),
            )
        };
        Ok(Properties {
            width: w.unwrap_or(0).clamp(0, u32::MAX as i64) as u32,
            height: h.unwrap_or(0).clamp(0, u32::MAX as i64) as u32,
            orientation: o.filter(|v| (1..=8).contains(v)).map(|v| v as u8),
        })
    }

    /// Draws `image` into a new bitmap context of `space`; returns interleaved samples.
    fn draw(image: CFTypeRef, space: CFTypeRef, gray: bool, wide: bool) -> Result<(u32, u32, Samples), String> {
        // SAFETY: image is a live CGImage.
        let (w, h) = unsafe { (CGImageGetWidth(image), CGImageGetHeight(image)) };
        if w == 0 || h == 0 || (w as u64) * (h as u64) > 400_000_000 {
            return Err(format!("ImageIO: implausible image size {w}x{h}"));
        }
        let comps = if gray { 1 } else { 4 };
        let info = match (gray, wide) {
            (true, false) => ALPHA_NONE,
            (true, true) => ALPHA_NONE | BYTE_ORDER_16_LITTLE,
            (false, false) => ALPHA_NONE_SKIP_LAST,
            (false, true) => ALPHA_NONE_SKIP_LAST | BYTE_ORDER_16_LITTLE,
        };
        let rect = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: w as f64, height: h as f64 } };
        let rgb_len = w * h * 3;
        if wide {
            let mut buf = vec![0u16; w * h * comps];
            // SAFETY: buf holds h rows of w*comps u16 and outlives the context.
            unsafe {
                let ctx = Cf::new(
                    CGBitmapContextCreate(buf.as_mut_ptr().cast(), w, h, 16, w * comps * 2, space, info),
                    "bitmap context",
                )?;
                CGContextSetBlendMode(ctx.0, 17); // kCGBlendModeCopy
                CGContextDrawImage(ctx.0, rect, image);
            }
            let out = if gray {
                buf.iter().flat_map(|&v| [v, v, v]).collect()
            } else {
                let mut out = Vec::with_capacity(rgb_len);
                for px in buf.as_chunks::<4>().0 {
                    out.extend_from_slice(&px[..3]);
                }
                out
            };
            Ok((w as u32, h as u32, Samples::U16(out)))
        } else {
            let mut buf = vec![0u8; w * h * comps];
            // SAFETY: as above, 8-bit samples.
            unsafe {
                let ctx = Cf::new(
                    CGBitmapContextCreate(buf.as_mut_ptr().cast(), w, h, 8, w * comps, space, info),
                    "bitmap context",
                )?;
                CGContextSetBlendMode(ctx.0, 17);
                CGContextDrawImage(ctx.0, rect, image);
            }
            let out = if gray {
                buf.iter().flat_map(|&v| [v, v, v]).collect()
            } else {
                // Compact RGBX -> RGB in place.
                for i in 0..w * h {
                    buf.copy_within(i * 4..i * 4 + 3, i * 3);
                }
                buf.truncate(rgb_len);
                buf.shrink_to_fit();
                buf
            };
            Ok((w as u32, h as u32, Samples::U8(out)))
        }
    }

    fn srgb_space() -> Result<Cf, String> {
        // SAFETY: kCGColorSpaceSRGB is a CoreGraphics constant.
        Cf::new(unsafe { CGColorSpaceCreateWithName(kCGColorSpaceSRGB) }, "sRGB colour space")
    }

    pub fn decode_native(path: &Path) -> Result<Native, String> {
        let (src, idx) = source(path)?;
        // SAFETY: kCFBooleanFalse is a constant.
        let opts = dict(&[(unsafe { kCGImageSourceShouldCache }, unsafe { kCFBooleanFalse })])?;
        // SAFETY: src/opts are live.
        let image = Cf::new(unsafe { CGImageSourceCreateImageAtIndex(src.0, idx, opts.0) }, "decode")?;
        drop(src);
        // SAFETY: image is live; the colour space is borrowed (Get rule).
        let (space, bits) = unsafe { (CGImageGetColorSpace(image.0), CGImageGetBitsPerComponent(image.0)) };
        let model = if space.is_null() { -1 } else { unsafe { CGColorSpaceGetModel(space) } };
        let wide = bits > 8;
        if model == MODEL_RGB || model == MODEL_MONOCHROME {
            // SAFETY: space is live; returns +1 CFData or null.
            let icc = unsafe {
                let d = CGColorSpaceCopyICCData(space);
                if d.is_null() {
                    None
                } else {
                    let d = Cf(d);
                    let n = CFDataGetLength(d.0).max(0) as usize;
                    Some(std::slice::from_raw_parts(CFDataGetBytePtr(d.0), n).to_vec())
                }
            };
            let (width, height, samples) = draw(image.0, space, model == MODEL_MONOCHROME, wide)?;
            Ok(Native { width, height, samples, icc, converted_to_srgb: false, file_bits: bits as u8 })
        } else {
            let srgb = srgb_space()?;
            let (width, height, samples) = draw(image.0, srgb.0, false, wide)?;
            Ok(Native { width, height, samples, icc: None, converted_to_srgb: true, file_bits: bits as u8 })
        }
    }

    pub fn thumbnail_srgb(path: &Path, max_edge: u32) -> Result<(u32, u32, Vec<u8>), String> {
        let (src, idx) = source(path)?;
        let size = number(i64::from(max_edge))?;
        // SAFETY: ImageIO/CF constants; values are live.
        let opts = unsafe {
            dict(&[
                (kCGImageSourceCreateThumbnailFromImageAlways, kCFBooleanTrue),
                (kCGImageSourceThumbnailMaxPixelSize, size.0),
                (kCGImageSourceCreateThumbnailWithTransform, kCFBooleanFalse),
                (kCGImageSourceShouldCache, kCFBooleanFalse),
            ])?
        };
        // SAFETY: src/opts are live.
        let thumb = Cf::new(unsafe { CGImageSourceCreateThumbnailAtIndex(src.0, idx, opts.0) }, "thumbnail")?;
        let srgb = srgb_space()?;
        let (w, h, samples) = draw(thumb.0, srgb.0, false, false)?;
        match samples {
            Samples::U8(px) => Ok((w, h, px)),
            Samples::U16(_) => Err("ImageIO: unexpected 16-bit thumbnail".into()),
        }
    }
}

/// Pixel size + orientation of the primary image (cheap: no decode).
pub fn properties(path: &Path) -> Result<Properties, String> {
    #[cfg(target_os = "macos")]
    {
        mac::properties(path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Err("ImageIO is macOS-only".into())
    }
}

/// Full-size decode in the image's own colour space (see module docs).
pub fn decode_native(path: &Path) -> Result<Native, String> {
    #[cfg(target_os = "macos")]
    {
        mac::decode_native(path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Err("ImageIO is macOS-only".into())
    }
}

/// 8-bit sRGB RGB thumbnail with long edge <= `max_edge` (orientation not applied).
pub fn thumbnail_srgb(path: &Path, max_edge: u32) -> Result<(u32, u32, Vec<u8>), String> {
    #[cfg(target_os = "macos")]
    {
        mac::thumbnail_srgb(path, max_edge)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (path, max_edge);
        Err("ImageIO is macOS-only".into())
    }
}
