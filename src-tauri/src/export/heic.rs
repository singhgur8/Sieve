//! HEIC via macOS ImageIO (`CGImageDestination`, `public.heic`): 8-bit HEVC in a HEIF
//! container with the ICC profile (colour space of the `CGImage`), the lossy quality and
//! the XMP packet (`CGImageMetadataCreateFromXMPData`). EXIF-only fields travel in the XMP
//! packet; ImageIO writes no separate EXIF item for our images.
//!
//! Minimal hand-written FFI; every CF object is released through [`Cf`].

use std::ffi::c_void;
use std::path::Path;

#[cfg(target_os = "macos")]
mod ffi {
    use std::ffi::{c_char, c_void};

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

    pub const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    pub const K_CF_NUMBER_FLOAT64: CFIndex = 6;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        pub static kCFTypeDictionaryKeyCallBacks: CFDictionaryKeyCallBacks;
        pub static kCFTypeDictionaryValueCallBacks: CFDictionaryValueCallBacks;
        pub fn CFRelease(cf: CFTypeRef);
        pub fn CFDataCreate(alloc: CFTypeRef, bytes: *const u8, len: CFIndex) -> CFTypeRef;
        pub fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const c_char, encoding: u32) -> CFTypeRef;
        pub fn CFNumberCreate(alloc: CFTypeRef, typ: CFIndex, value: *const c_void) -> CFTypeRef;
        pub fn CFDictionaryCreate(
            alloc: CFTypeRef,
            keys: *const CFTypeRef,
            values: *const CFTypeRef,
            n: CFIndex,
            kcb: *const CFDictionaryKeyCallBacks,
            vcb: *const CFDictionaryValueCallBacks,
        ) -> CFTypeRef;
        pub fn CFURLCreateFromFileSystemRepresentation(
            alloc: CFTypeRef,
            buf: *const u8,
            len: CFIndex,
            is_dir: u8,
        ) -> CFTypeRef;
        pub fn CFArrayGetCount(a: CFTypeRef) -> CFIndex;
        pub fn CFArrayGetValueAtIndex(a: CFTypeRef, i: CFIndex) -> CFTypeRef;
        pub fn CFEqual(a: CFTypeRef, b: CFTypeRef) -> u8;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        pub fn CGColorSpaceCreateWithICCData(data: CFTypeRef) -> CFTypeRef;
        pub fn CGDataProviderCreateWithCFData(data: CFTypeRef) -> CFTypeRef;
        pub fn CGImageCreate(
            width: usize,
            height: usize,
            bits_per_component: usize,
            bits_per_pixel: usize,
            bytes_per_row: usize,
            space: CFTypeRef,
            bitmap_info: u32,
            provider: CFTypeRef,
            decode: *const f64,
            should_interpolate: bool,
            intent: i32,
        ) -> CFTypeRef;
    }

    #[link(name = "ImageIO", kind = "framework")]
    extern "C" {
        pub static kCGImageDestinationLossyCompressionQuality: CFTypeRef;
        pub fn CGImageDestinationCopyTypeIdentifiers() -> CFTypeRef;
        pub fn CGImageDestinationCreateWithURL(
            url: CFTypeRef,
            typ: CFTypeRef,
            count: usize,
            options: CFTypeRef,
        ) -> CFTypeRef;
        pub fn CGImageDestinationAddImageAndMetadata(
            dest: CFTypeRef,
            image: CFTypeRef,
            metadata: CFTypeRef,
            options: CFTypeRef,
        );
        pub fn CGImageDestinationFinalize(dest: CFTypeRef) -> bool;
        pub fn CGImageMetadataCreateFromXMPData(data: CFTypeRef) -> CFTypeRef;
    }
}

/// Owned CF object (released on drop). Null = absent.
#[cfg(target_os = "macos")]
struct Cf(ffi::CFTypeRef);

#[cfg(target_os = "macos")]
impl Cf {
    fn new(p: ffi::CFTypeRef, what: &str) -> Result<Self, String> {
        if p.is_null() {
            Err(format!("ImageIO: {what} failed"))
        } else {
            Ok(Cf(p))
        }
    }

    fn string(s: &str) -> Result<Self, String> {
        let c = std::ffi::CString::new(s).map_err(|_| "NUL in string".to_owned())?;
        // SAFETY: c is a valid NUL-terminated string.
        Cf::new(
            unsafe { ffi::CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), ffi::K_CF_STRING_ENCODING_UTF8) },
            "string",
        )
    }

    fn data(bytes: &[u8]) -> Result<Self, String> {
        // SAFETY: CFDataCreate copies `bytes`.
        Cf::new(unsafe { ffi::CFDataCreate(std::ptr::null(), bytes.as_ptr(), bytes.len() as ffi::CFIndex) }, "data")
    }
}

#[cfg(target_os = "macos")]
impl Drop for Cf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: we own one reference.
            unsafe { ffi::CFRelease(self.0) }
        }
    }
}

const HEIC_UTI: &str = "public.heic";

/// `Ok` if this system's ImageIO can encode HEIC, else why not.
pub fn probe() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let uti = Cf::string(HEIC_UTI)?;
        // SAFETY: returns a +1 CFArray of CFStrings (Copy rule).
        let types = Cf::new(unsafe { ffi::CGImageDestinationCopyTypeIdentifiers() }, "type list")?;
        // SAFETY: types is a live CFArray; values are borrowed.
        let found = unsafe {
            (0..ffi::CFArrayGetCount(types.0))
                .any(|i| ffi::CFEqual(ffi::CFArrayGetValueAtIndex(types.0, i), uti.0) != 0)
        };
        if found {
            Ok(())
        } else {
            Err("HEIC encoder not available on this Mac (ImageIO has no public.heic destination)".into())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("HEIC export needs macOS ImageIO".into())
    }
}

/// Writes 8-bit RGB as HEIC with the ICC profile, quality 0..=100 and optional XMP.
pub fn write(
    rgb: &[u8],
    w: u32,
    h: u32,
    quality: u8,
    icc: &[u8],
    xmp: Option<&str>,
    path: &Path,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let (wu, hu) = (w as usize, h as usize);
        if rgb.len() < wu * hu * 3 {
            return Err("HEIC: short pixel buffer".into());
        }
        let icc_data = Cf::data(icc)?;
        // SAFETY: icc_data is a live CFData.
        let space = Cf::new(unsafe { ffi::CGColorSpaceCreateWithICCData(icc_data.0) }, "colour space from ICC")?;
        let pixels = Cf::data(&rgb[..wu * hu * 3])?;
        // SAFETY: pixels is a live CFData (retained by the provider).
        let provider = Cf::new(unsafe { ffi::CGDataProviderCreateWithCFData(pixels.0) }, "data provider")?;
        // SAFETY: 8 bits/component, 24 bits/pixel, no alpha (kCGImageAlphaNone = 0), rows of w*3 bytes.
        let image = Cf::new(
            unsafe { ffi::CGImageCreate(wu, hu, 8, 24, wu * 3, space.0, 0, provider.0, std::ptr::null(), false, 0) },
            "CGImage",
        )?;
        let path_bytes = path.as_os_str().as_bytes();
        // SAFETY: path bytes are valid for the call.
        let url = Cf::new(
            unsafe {
                ffi::CFURLCreateFromFileSystemRepresentation(
                    std::ptr::null(),
                    path_bytes.as_ptr(),
                    path_bytes.len() as ffi::CFIndex,
                    0,
                )
            },
            "URL",
        )?;
        let uti = Cf::string(HEIC_UTI)?;
        // SAFETY: url/uti are live; one image.
        let dest = Cf::new(
            unsafe { ffi::CGImageDestinationCreateWithURL(url.0, uti.0, 1, std::ptr::null()) },
            "HEIC destination",
        )?;
        let q: f64 = f64::from(quality.min(100)) / 100.0;
        // SAFETY: q outlives the call; CFNumber copies it.
        let qnum = Cf::new(
            unsafe {
                ffi::CFNumberCreate(std::ptr::null(), ffi::K_CF_NUMBER_FLOAT64, (&q as *const f64).cast::<c_void>())
            },
            "number",
        )?;
        // SAFETY: the key is an ImageIO constant; values/keys are live CF objects.
        let options = Cf::new(
            unsafe {
                let keys = [ffi::kCGImageDestinationLossyCompressionQuality];
                let values = [qnum.0];
                ffi::CFDictionaryCreate(
                    std::ptr::null(),
                    keys.as_ptr(),
                    values.as_ptr(),
                    1,
                    &ffi::kCFTypeDictionaryKeyCallBacks,
                    &ffi::kCFTypeDictionaryValueCallBacks,
                )
            },
            "options",
        )?;
        let metadata = match xmp {
            Some(x) => {
                // ImageIO rejects `<?xpacket?>` wrappers: pass the bare `x:xmpmeta` element.
                let bare = match (x.find("<x:xmpmeta"), x.rfind("</x:xmpmeta>")) {
                    (Some(a), Some(b)) if b > a => &x[a..b + "</x:xmpmeta>".len()],
                    _ => x,
                };
                let d = Cf::data(bare.as_bytes())?;
                // SAFETY: d is live; null on unparsable XMP.
                let m = Cf(unsafe { ffi::CGImageMetadataCreateFromXMPData(d.0) });
                if m.0.is_null() {
                    return Err("ImageIO could not parse the XMP packet".into());
                }
                m
            }
            None => Cf(std::ptr::null()),
        };
        // SAFETY: all objects are live; metadata may be null.
        let ok = unsafe {
            ffi::CGImageDestinationAddImageAndMetadata(dest.0, image.0, metadata.0, options.0);
            ffi::CGImageDestinationFinalize(dest.0)
        };
        drop(dest);
        if !ok {
            return Err("ImageIO could not write the HEIC file".into());
        }
        std::fs::File::open(path).and_then(|f| f.sync_all()).map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (rgb, w, h, quality, icc, xmp, path);
        Err("HEIC export needs macOS ImageIO".into())
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn heic_round_trip_when_available() {
        if probe().is_err() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.heic");
        let (w, h) = (64u32, 48u32);
        let rgb: Vec<u8> = (0..w * h).flat_map(|i| [(i % 256) as u8, 120, 40]).collect();
        let icc = crate::export::color::icc_profile(crate::ipc::types::ExportColorSpace::DisplayP3);
        let opts = crate::ipc::types::MetadataOptions {
            include: crate::ipc::types::MetadataInclude::CopyrightOnly,
            remove_location: false,
            include_keywords: false,
            copyright: Some("(c) HEIC test".into()),
            creator: None,
        };
        let meta = crate::export::metadata::build(&Default::default(), &Default::default(), &opts);
        let p = std::env::var_os("SIEVE_HEIC_OUT").map(std::path::PathBuf::from).unwrap_or(p);
        write(&rgb, w, h, 80, icc, meta.xmp.as_deref(), &p).unwrap();
        let b = std::fs::read(&p).unwrap();
        assert!(b.len() > 100 && &b[4..8] == b"ftyp", "{:?}", &b[..16]);
        assert!(b.windows(13).any(|w| w == b"(c) HEIC test"), "XMP embedded");
    }
}
