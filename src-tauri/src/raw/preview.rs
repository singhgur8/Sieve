//! Embedded preview -> cache JPEGs: DCT-scaled decode, resize, orientation, encode.
//!
//! Memory: all intermediate pixels live in a per-thread [`Work`] whose buffers are
//! reused from image to image, so steady-state allocation is ~zero and memory is
//! proportional to the number of pool threads, not to the number of files. The
//! decoded image is just above the preview edge (TurboJPEG's n/8 DCT scaling picks the
//! smallest size >= 2048 px). Nothing here spawns threads.

use std::fs;
use std::path::Path;

use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};

use super::{jpeg, turbo};

/// Long edge of the grid thumbnail.
pub const THUMB_EDGE: u32 = 512;
/// Long edge of the loupe preview.
pub const PREVIEW_EDGE: u32 = 2048;
pub const JPEG_QUALITY: u8 = 85;
/// Refuse to decode absurd frames (guards memory on corrupt headers).
const MAX_PIXELS: u64 = 120_000_000;
/// Scratch buffers larger than this are released after use (outlier images).
const KEEP_BYTES: usize = 48 << 20;

/// 8-bit interleaved RGB.
#[derive(Debug, Clone, PartialEq)]
pub struct Rgb {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Borrowed 8-bit interleaved RGB.
#[derive(Debug, Clone, Copy)]
pub struct View<'a> {
    pub width: u32,
    pub height: u32,
    pub pixels: &'a [u8],
}

impl Rgb {
    pub fn view(&self) -> View<'_> {
        View { width: self.width, height: self.height, pixels: &self.pixels }
    }
}

/// Source pixels for a preview.
pub enum Source<'a> {
    Jpeg(&'a [u8]),
    Rgb(View<'a>),
}

/// Sizes of the written files (after orientation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rendered {
    pub thumb: (u32, u32),
    pub preview: (u32, u32),
}

/// Reusable per-thread buffers for [`render`].
pub struct Work {
    resizer: Resizer,
    /// Full-size ping-pong buffers (decode / resize / orient).
    a: Vec<u8>,
    b: Vec<u8>,
    thumb: Vec<u8>,
}

impl Default for Work {
    fn default() -> Self {
        Self { resizer: Resizer::new(), a: Vec::new(), b: Vec::new(), thumb: Vec::new() }
    }
}

impl Work {
    /// Drops buffers that grew beyond the steady-state size.
    fn trim(&mut self) {
        for b in [&mut self.a, &mut self.b, &mut self.thumb] {
            if b.capacity() > KEEP_BYTES {
                *b = Vec::new();
            }
        }
    }
}

/// Decodes a JPEG, using DCT scaling so the long edge is the smallest supported size
/// that is still >= `min_long_edge` (or full size if the image is smaller).
pub fn decode_jpeg(bytes: &[u8], min_long_edge: u32) -> Result<Rgb, String> {
    let mut pixels = Vec::new();
    let (width, height) = decode_into(bytes, min_long_edge, &mut pixels)?;
    Ok(Rgb { width, height, pixels })
}

fn decode_into(bytes: &[u8], min_long_edge: u32, out: &mut Vec<u8>) -> Result<(u32, u32), String> {
    match jpeg::header(bytes) {
        jpeg::Header::Frame { .. } => turbo::decode_rgb_into(bytes, min_long_edge, MAX_PIXELS, out),
        _ => Err("embedded JPEG is not a decodable 8-bit frame".into()),
    }
}

/// Size with long edge `edge` (never upscales).
fn fit_size(width: u32, height: u32, edge: u32) -> (u32, u32) {
    let long = width.max(height);
    if long <= edge {
        return (width, height);
    }
    let scale = edge as f64 / long as f64;
    (((width as f64 * scale).round() as u32).max(1), ((height as f64 * scale).round() as u32).max(1))
}

/// Resamples `src` so its long edge is at most `edge`, into `out`. Returns the size.
fn fit_into(src: View, edge: u32, resizer: &mut Resizer, out: &mut Vec<u8>) -> Result<(u32, u32), String> {
    let (dw, dh) = fit_size(src.width, src.height, edge);
    out.clear();
    if (dw, dh) == (src.width, src.height) {
        out.extend_from_slice(&src.pixels[..dw as usize * dh as usize * 3]);
        return Ok((dw, dh));
    }
    out.resize(dw as usize * dh as usize * 3, 0);
    let src_img = ImageRef::new(src.width, src.height, src.pixels, PixelType::U8x3).map_err(|e| e.to_string())?;
    let mut dst = Image::from_slice_u8(dw, dh, out.as_mut_slice(), PixelType::U8x3).map_err(|e| e.to_string())?;
    let opts = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::CatmullRom));
    resizer.resize(&src_img, &mut dst, &opts).map_err(|e| format!("resize: {e}"))?;
    Ok((dw, dh))
}

/// Scales `img` so its long edge is at most `edge` (never upscales).
pub fn fit(img: &Rgb, edge: u32, resizer: &mut Resizer) -> Result<Rgb, String> {
    let mut pixels = Vec::new();
    let (width, height) = fit_into(img.view(), edge, resizer, &mut pixels)?;
    Ok(Rgb { width, height, pixels })
}

/// Applies EXIF orientation (1..=8) into `out` so the result displays upright.
fn orient_into(src: View, orientation: u16, out: &mut Vec<u8>) -> (u32, u32) {
    let (w, h) = (src.width as usize, src.height as usize);
    let swap = orientation >= 5;
    let (dw, dh) = if swap { (h, w) } else { (w, h) };
    out.clear();
    out.resize(w * h * 3, 0);
    if w == 0 || h == 0 {
        return (dw as u32, dh as u32);
    }
    // Source pixel index of destination (x, y) = base + x * dx + y * dy (pixels); hoisting
    // the orientation out of the loop and walking 64x64 destination tiles (so the strided
    // source reads of 90-degree rotations stay in cache) makes this ~4x faster than a
    // per-pixel match over whole rows (Phase 8 ingest profile).
    let (wi, hi) = (w as isize, h as isize);
    let (base, dx, dy): (isize, isize, isize) = match orientation {
        2 => (wi - 1, -1, wi),
        3 => (hi * wi - 1, -1, -wi),
        4 => ((hi - 1) * wi, 1, -wi),
        5 => (0, wi, 1),
        6 => ((hi - 1) * wi, -wi, 1),
        7 => ((hi - 1) * wi + wi - 1, -wi, -1),
        8 => (wi - 1, wi, -1),
        _ => (0, 1, wi),
    };
    const TILE: usize = 64;
    for ty in (0..dh).step_by(TILE) {
        for tx in (0..dw).step_by(TILE) {
            for y in ty..(ty + TILE).min(dh) {
                let row = base + y as isize * dy;
                let d0 = y * dw;
                for x in tx..(tx + TILE).min(dw) {
                    let s = (row + x as isize * dx) as usize * 3;
                    let d = (d0 + x) * 3;
                    out[d..d + 3].copy_from_slice(&src.pixels[s..s + 3]);
                }
            }
        }
    }
    (dw as u32, dh as u32)
}

/// Applies EXIF orientation (1..=8) so the result displays upright.
pub fn orient(img: Rgb, orientation: u16) -> Rgb {
    if !(2..=8).contains(&orientation) {
        return img;
    }
    let mut pixels = Vec::new();
    let (width, height) = orient_into(img.view(), orientation, &mut pixels);
    Rgb { width, height, pixels }
}

/// Encodes and writes via a temp file + rename so readers never see a partial JPEG.
fn write_jpeg(path: &Path, img: View) -> Result<(), String> {
    turbo::encode_rgb_with(img.pixels, img.width, img.height, JPEG_QUALITY, |bytes| {
        let tmp = path.with_extension("jpg.part");
        fs::write(&tmp, bytes).map_err(|e| format!("write {}: {e}", tmp.display()))?;
        fs::rename(&tmp, path).map_err(|e| format!("rename {}: {e}", path.display()))
    })
}

/// Produces the 2048 px preview and 512 px thumbnail for one image.
pub fn render(
    source: Source,
    orientation: u16,
    preview_path: &Path,
    thumb_path: &Path,
    work: &mut Work,
) -> Result<Rendered, String> {
    let result = render_inner(source, orientation, preview_path, thumb_path, work);
    work.trim();
    result
}

/// Which buffer currently holds the working image.
#[derive(Clone, Copy)]
enum Loc {
    A,
    B,
    External,
}

fn render_inner(
    source: Source,
    orientation: u16,
    preview_path: &Path,
    thumb_path: &Path,
    work: &mut Work,
) -> Result<Rendered, String> {
    // Two full-size buffers ping-pong: decode -> A, resize A -> B, orient B -> A.
    let Work { resizer, a, b, thumb } = work;

    let (ext, mut loc, (mut w, mut h)) = match source {
        Source::Jpeg(bytes) => (&[][..], Loc::A, decode_into(bytes, PREVIEW_EDGE, a)?),
        Source::Rgb(v) => {
            if v.pixels.len() < v.width as usize * v.height as usize * 3 {
                return Err("preview pixel buffer too small".into());
            }
            (v.pixels, Loc::External, (v.width, v.height))
        }
    };

    if w.max(h) > PREVIEW_EDGE {
        let (src, dst) = match loc {
            Loc::B => (&b[..], &mut *a),
            Loc::A => (&a[..], &mut *b),
            Loc::External => (ext, &mut *a),
        };
        (w, h) = fit_into(View { width: w, height: h, pixels: src }, PREVIEW_EDGE, resizer, dst)?;
        loc = match loc {
            Loc::B | Loc::External => Loc::A,
            Loc::A => Loc::B,
        };
    }
    if (2..=8).contains(&orientation) {
        let (src, dst) = match loc {
            Loc::B => (&b[..], &mut *a),
            Loc::A => (&a[..], &mut *b),
            Loc::External => (ext, &mut *a),
        };
        (w, h) = orient_into(View { width: w, height: h, pixels: src }, orientation, dst);
        loc = match loc {
            Loc::B | Loc::External => Loc::A,
            Loc::A => Loc::B,
        };
    }
    let upright = View {
        width: w,
        height: h,
        pixels: match loc {
            Loc::A => &a[..],
            Loc::B => &b[..],
            Loc::External => ext,
        },
    };
    write_jpeg(preview_path, upright)?;

    let (tw, th) = fit_into(upright, THUMB_EDGE, resizer, thumb)?;
    write_jpeg(thumb_path, View { width: tw, height: th, pixels: thumb })?;
    Ok(Rendered { thumb: (tw, th), preview: (w, h) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::jpeg::test_support::quadrant_jpeg;

    const RED: [u8; 3] = [255, 0, 0];
    const GREEN: [u8; 3] = [0, 255, 0];
    const BLUE: [u8; 3] = [0, 0, 255];
    const WHITE: [u8; 3] = [255, 255, 255];

    /// Colour near a corner (inset to avoid JPEG edge ringing), classified.
    pub(crate) fn corner(img: &Rgb, right: bool, bottom: bool) -> [u8; 3] {
        let x = if right { img.width - 1 - img.width / 8 } else { img.width / 8 } as usize;
        let y = if bottom { img.height - 1 - img.height / 8 } else { img.height / 8 } as usize;
        let i = (y * img.width as usize + x) * 3;
        let px = &img.pixels[i..i + 3];
        [px[0], px[1], px[2]].map(|c| if c > 127 { 255 } else { 0 })
    }

    fn quad(w: u32, h: u32) -> Rgb {
        decode_jpeg(&quadrant_jpeg(w as u16, h as u16), 10_000).unwrap()
    }

    /// The tiled strided implementation matches the per-pixel definition exactly (odd sizes
    /// spanning several tiles, every orientation).
    #[test]
    fn orientation_matches_reference_on_every_pixel() {
        let (w, h) = (131usize, 77usize);
        let pixels: Vec<u8> = (0..w * h * 3).map(|i| (i * 7 % 251) as u8).collect();
        let src = Rgb { width: w as u32, height: h as u32, pixels };
        for o in 1..=8u16 {
            let got = orient(src.clone(), o);
            let (dw, dh) = if o >= 5 { (h, w) } else { (w, h) };
            assert_eq!((got.width as usize, got.height as usize), (dw, dh));
            for y in 0..dh {
                for x in 0..dw {
                    let (sx, sy) = match o {
                        2 => (w - 1 - x, y),
                        3 => (w - 1 - x, h - 1 - y),
                        4 => (x, h - 1 - y),
                        5 => (y, x),
                        6 => (y, h - 1 - x),
                        7 => (w - 1 - y, h - 1 - x),
                        8 => (w - 1 - y, x),
                        _ => (x, y),
                    };
                    let s = (sy * w + sx) * 3;
                    let d = (y * dw + x) * 3;
                    assert_eq!(got.pixels[d..d + 3], src.pixels[s..s + 3], "orientation {o} at ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn orientation_all_eight() {
        let src = quad(64, 32);
        // (top-left, top-right, bottom-left, bottom-right) after orientation.
        let expect = [
            (1, [RED, GREEN, BLUE, WHITE]),
            (2, [GREEN, RED, WHITE, BLUE]),
            (3, [WHITE, BLUE, GREEN, RED]),
            (4, [BLUE, WHITE, RED, GREEN]),
            (5, [RED, BLUE, GREEN, WHITE]),
            (6, [BLUE, RED, WHITE, GREEN]),
            (7, [WHITE, GREEN, BLUE, RED]),
            (8, [GREEN, WHITE, RED, BLUE]),
        ];
        for (o, [tl, tr, bl, br]) in expect {
            let img = orient(src.clone(), o);
            let dims = if o >= 5 { (32, 64) } else { (64, 32) };
            assert_eq!((img.width, img.height), dims, "orientation {o}");
            assert_eq!(
                [
                    corner(&img, false, false),
                    corner(&img, true, false),
                    corner(&img, false, true),
                    corner(&img, true, true)
                ],
                [tl, tr, bl, br],
                "orientation {o}"
            );
        }
    }

    #[test]
    fn dct_scaled_decode_and_fit() {
        let j = quadrant_jpeg(1024, 512);
        let d = decode_jpeg(&j, 256).unwrap();
        assert_eq!((d.width, d.height), (256, 128), "2/8 scale is the smallest >= 256");
        let d = decode_jpeg(&j, 300).unwrap();
        assert_eq!((d.width, d.height), (384, 192), "3/8 scale");
        let mut r = Resizer::new();
        let f = fit(&d, 100, &mut r).unwrap();
        assert_eq!((f.width, f.height), (100, 50));
        assert_eq!(fit(&f, 500, &mut r).unwrap(), f, "never upscales");
        assert!(decode_jpeg(b"\xFF\xD8garbage", 10).is_err());
    }

    #[test]
    fn render_buffer_paths() {
        let dir = tempfile::tempdir().unwrap();
        let (p, t) = (dir.path().join("p.jpg"), dir.path().join("t.jpg"));
        let mut work = Work::default();
        let read = |path: &Path| decode_jpeg(&std::fs::read(path).unwrap(), 10_000).unwrap();

        // Small JPEG, no resize, rotated: decode buffer -> orient buffer.
        let r = render(Source::Jpeg(&quadrant_jpeg(600, 400)), 8, &p, &t, &mut work).unwrap();
        assert_eq!((r.preview, r.thumb), ((400, 600), (341, 512)));
        assert_eq!(corner(&read(&p), false, false), GREEN);

        // Large RGB (LibRaw fallback shape), resized + rotated 180: external -> A -> B.
        let big = decode_jpeg(&quadrant_jpeg(2600, 1300), 10_000).unwrap();
        let r = render(Source::Rgb(big.view()), 3, &p, &t, &mut work).unwrap();
        assert_eq!((r.preview, r.thumb), ((2048, 1024), (512, 256)));
        assert_eq!(corner(&read(&p), false, false), WHITE);
        assert_eq!(corner(&read(&t), true, true), RED);

        // Small RGB, upright: written straight from the external buffer.
        let small = decode_jpeg(&quadrant_jpeg(300, 200), 10_000).unwrap();
        let r = render(Source::Rgb(small.view()), 1, &p, &t, &mut work).unwrap();
        assert_eq!((r.preview, r.thumb), ((300, 200), (300, 200)));
        assert_eq!(corner(&read(&t), false, false), RED);

        let short = View { width: 300, height: 200, pixels: &small.pixels[..10] };
        assert!(render(Source::Rgb(short), 1, &p, &t, &mut work).is_err());
    }

    #[test]
    fn render_writes_oriented_files() {
        let dir = tempfile::tempdir().unwrap();
        let (p, t) = (dir.path().join("1_2048.jpg"), dir.path().join("1_512.jpg"));
        let j = quadrant_jpeg(3000, 2000);
        let r = render(Source::Jpeg(&j), 6, &p, &t, &mut Work::default()).unwrap();
        assert_eq!(r.preview, (1365, 2048));
        assert_eq!(r.thumb, (341, 512));
        let thumb = decode_jpeg(&std::fs::read(&t).unwrap(), 10_000).unwrap();
        assert_eq!((thumb.width, thumb.height), (341, 512));
        assert_eq!(corner(&thumb, true, false), RED, "rotated 90 CW: red moves to top-right");
        assert!(!dir.path().join("1_512.jpg.part").exists());
    }
}
