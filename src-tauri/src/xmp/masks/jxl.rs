//! Decodes a JPEG XL codestream (Lightroom matte tile) to 8-bit grey through macOS ImageIO,
//! which supports JPEG XL from macOS 14. Minimal FFI from memory (`CGImageSourceCreateWithData`).

/// Oldest macOS whose ImageIO decodes JPEG XL.
pub const MIN_MACOS_MAJOR: u32 = 14;

/// Major version of the running macOS (`kern.osproductversion`), if known.
pub fn macos_major() -> Option<u32> {
    #[cfg(target_os = "macos")]
    {
        let name = c"kern.osproductversion";
        let mut buf = [0u8; 64];
        let mut len = buf.len();
        // SAFETY: buf/len describe a valid writable buffer; name is NUL-terminated.
        let rc = unsafe {
            libc::sysctlbyname(name.as_ptr(), buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0)
        };
        if rc != 0 {
            return None;
        }
        let s = std::str::from_utf8(&buf[..len]).ok()?.trim_end_matches('\0');
        s.split('.').next()?.trim().parse().ok()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Whether this system can decode Lightroom mattes (`Err` = why not).
pub fn supported() -> Result<(), String> {
    match macos_major() {
        Some(v) if v >= MIN_MACOS_MAJOR => Ok(()),
        Some(v) => Err(format!("JPEG XL mattes need macOS {MIN_MACOS_MAJOR} or newer (this is macOS {v})")),
        None => Err("JPEG XL mattes need macOS ImageIO".into()),
    }
}

/// Grey 8-bit pixels `(width, height, data)` of an image ImageIO can read from memory.
pub fn decode_gray8(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    supported()?;
    #[cfg(target_os = "macos")]
    {
        mac::decode_gray8(bytes)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = bytes;
        Err("ImageIO is macOS only".into())
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;

    type CFTypeRef = *const c_void;
    type CFIndex = isize;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGSize {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }

    const MODEL_MONOCHROME: i32 = 0;
    /// `kCGImageAlphaNone`.
    const ALPHA_NONE: u32 = 0;
    /// `kCGBlendModeCopy`.
    const BLEND_COPY: i32 = 17;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: CFTypeRef);
        fn CFDataCreate(alloc: CFTypeRef, bytes: *const u8, len: CFIndex) -> CFTypeRef;
        fn CFDataGetLength(d: CFTypeRef) -> CFIndex;
        fn CFDataGetBytePtr(d: CFTypeRef) -> *const u8;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGColorSpaceCreateDeviceGray() -> CFTypeRef;
        fn CGColorSpaceGetModel(space: CFTypeRef) -> i32;
        fn CGImageGetWidth(image: CFTypeRef) -> usize;
        fn CGImageGetHeight(image: CFTypeRef) -> usize;
        fn CGImageGetBitsPerComponent(image: CFTypeRef) -> usize;
        fn CGImageGetBitsPerPixel(image: CFTypeRef) -> usize;
        fn CGImageGetBytesPerRow(image: CFTypeRef) -> usize;
        fn CGImageGetColorSpace(image: CFTypeRef) -> CFTypeRef;
        fn CGImageGetDataProvider(image: CFTypeRef) -> CFTypeRef;
        fn CGDataProviderCopyData(provider: CFTypeRef) -> CFTypeRef;
        fn CGBitmapContextCreate(
            data: *mut c_void,
            width: usize,
            height: usize,
            bits_per_component: usize,
            bytes_per_row: usize,
            space: CFTypeRef,
            bitmap_info: u32,
        ) -> CFTypeRef;
        fn CGContextDrawImage(ctx: CFTypeRef, rect: CGRect, image: CFTypeRef);
        fn CGContextSetBlendMode(ctx: CFTypeRef, mode: i32);
    }

    #[link(name = "ImageIO", kind = "framework")]
    extern "C" {
        fn CGImageSourceCreateWithData(data: CFTypeRef, options: CFTypeRef) -> CFTypeRef;
        fn CGImageSourceGetCount(src: CFTypeRef) -> usize;
        fn CGImageSourceCreateImageAtIndex(src: CFTypeRef, index: usize, options: CFTypeRef) -> CFTypeRef;
    }

    struct Cf(CFTypeRef);

    impl Cf {
        fn new(p: CFTypeRef, what: &str) -> Result<Cf, String> {
            if p.is_null() {
                Err(format!("ImageIO: {what} failed"))
            } else {
                Ok(Cf(p))
            }
        }
    }

    impl Drop for Cf {
        fn drop(&mut self) {
            // SAFETY: we own one reference.
            unsafe { CFRelease(self.0) }
        }
    }

    pub fn decode_gray8(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
        // SAFETY: bytes are valid for the call; CFData copies them.
        let data = Cf::new(unsafe { CFDataCreate(std::ptr::null(), bytes.as_ptr(), bytes.len() as CFIndex) }, "data")?;
        // SAFETY: data is a live CFData.
        let src = Cf::new(unsafe { CGImageSourceCreateWithData(data.0, std::ptr::null()) }, "open matte")?;
        // SAFETY: src is a live image source.
        if unsafe { CGImageSourceGetCount(src.0) } == 0 {
            return Err("ImageIO cannot read the matte (JPEG XL unsupported?)".into());
        }
        // SAFETY: index 0 exists.
        let image = Cf::new(unsafe { CGImageSourceCreateImageAtIndex(src.0, 0, std::ptr::null()) }, "decode matte")?;
        // SAFETY: image is a live CGImage.
        let (w, h, bpc, bpp, stride, space) = unsafe {
            (
                CGImageGetWidth(image.0),
                CGImageGetHeight(image.0),
                CGImageGetBitsPerComponent(image.0),
                CGImageGetBitsPerPixel(image.0),
                CGImageGetBytesPerRow(image.0),
                CGImageGetColorSpace(image.0),
            )
        };
        if w == 0 || h == 0 {
            return Err("empty matte".into());
        }
        // SAFETY: space is borrowed from the image (may be null).
        let mono = !space.is_null() && unsafe { CGColorSpaceGetModel(space) } == MODEL_MONOCHROME;
        if mono && bpc == 8 && bpp == 8 && stride >= w {
            // Raw decoded samples, no colour conversion.
            // SAFETY: the provider is borrowed from the image; the copied data is owned.
            let provider = unsafe { CGImageGetDataProvider(image.0) };
            if !provider.is_null() {
                if let Ok(copied) = Cf::new(unsafe { CGDataProviderCopyData(provider) }, "matte data") {
                    // SAFETY: copied is a live CFData.
                    let (ptr, len) = unsafe { (CFDataGetBytePtr(copied.0), CFDataGetLength(copied.0) as usize) };
                    if !ptr.is_null() && len >= stride * (h - 1) + w {
                        // SAFETY: ptr..ptr+len is valid while `copied` lives.
                        let all = unsafe { std::slice::from_raw_parts(ptr, len) };
                        let mut out = Vec::with_capacity(w * h);
                        for row in all.chunks(stride).take(h) {
                            out.extend_from_slice(&row[..w]);
                        }
                        return Ok((w as u32, h as u32, out));
                    }
                }
            }
        }
        // Draw into a grey context (the image's own grey space when it has one, so no
        // tone conversion happens).
        let own;
        let target = if mono {
            space
        } else {
            // SAFETY: creates an owned colour space.
            own = Cf::new(unsafe { CGColorSpaceCreateDeviceGray() }, "grey space")?;
            own.0
        };
        let mut out = vec![0u8; w * h];
        // SAFETY: out is w*h bytes with stride w; the context does not outlive it.
        let ctx = Cf::new(
            unsafe { CGBitmapContextCreate(out.as_mut_ptr().cast(), w, h, 8, w, target, ALPHA_NONE) },
            "grey context",
        )?;
        let rect = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: w as f64, height: h as f64 } };
        // SAFETY: ctx and image are live.
        unsafe {
            CGContextSetBlendMode(ctx.0, BLEND_COPY);
            CGContextDrawImage(ctx.0, rect, image.0);
        }
        drop(ctx);
        Ok((w as u32, h as u32, out))
    }
}
