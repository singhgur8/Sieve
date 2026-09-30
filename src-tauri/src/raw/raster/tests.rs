use std::path::{Path, PathBuf};

use super::*;
use crate::raw::icc::test_support::rgb_profile;
use crate::raw::tiff::build::{tiff as build_tiff, IfdSpec, Val};

pub(crate) mod fixtures {
    //! Synthetic non-RAW files for raster/ingest/export tests.
    use super::*;

    /// EXIF TIFF: make/model/orientation/date + `ColorSpace` (+ interop index when given).
    pub fn exif(orientation: u16, color_space: u16, interop: Option<&str>) -> Vec<u8> {
        let mut exif_tags = vec![
            (0x9003, Val::Ascii("2026:09:18 17:45:46".into())),
            (0x9291, Val::Ascii("50".into())),
            (0x829A, Val::Rational(1, 250)),
            (0x829D, Val::Rational(28, 10)),
            (0x8827, Val::Short(vec![400])),
            (0xA001, Val::Short(vec![color_space])),
            (0xA434, Val::Ascii("XF23mmF1.4 R LM WR".into())),
        ];
        let mut ifds = vec![
            IfdSpec {
                tags: vec![
                    (0x010F, Val::Ascii("FUJIFILM".into())),
                    (0x0110, Val::Ascii("X-T5".into())),
                    (0x0112, Val::Short(vec![orientation])),
                    (0x8769, Val::IfdRef(1)),
                ],
                next: None,
            },
            IfdSpec::default(),
        ];
        if let Some(i) = interop {
            exif_tags.push((0xA005, Val::IfdRef(2)));
            ifds.push(IfdSpec { tags: vec![(0x0001, Val::Ascii(i.into()))], next: None });
        }
        ifds[1].tags = exif_tags;
        build_tiff(&ifds, &[]).0
    }

    fn app(marker: u8, body: &[u8]) -> Vec<u8> {
        let mut v = vec![0xFF, marker];
        v.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
        v.extend_from_slice(body);
        v
    }

    /// Inserts APP segments after SOI.
    pub fn with_segments(jpeg: &[u8], segs: &[Vec<u8>]) -> Vec<u8> {
        let mut out = jpeg[..2].to_vec();
        for s in segs {
            out.extend_from_slice(s);
        }
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    pub fn exif_segment(tiff: &[u8]) -> Vec<u8> {
        app(0xE1, &[b"Exif\0\0".as_slice(), tiff].concat())
    }

    pub fn icc_segment(icc: &[u8]) -> Vec<u8> {
        app(0xE2, &[b"ICC_PROFILE\0\x01\x01".as_slice(), icc].concat())
    }

    pub fn xmp_segment(xmp: &str) -> Vec<u8> {
        app(0xE1, &[XMP_APP1, xmp.as_bytes()].concat())
    }

    /// A smooth RGB gradient (every 8-bit code present across the width).
    pub fn gradient(w: u32, h: u32) -> Vec<u8> {
        let mut px = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                px.extend_from_slice(&[(x * 255 / (w - 1)) as u8, (y * 255 / (h - 1).max(1)) as u8, 128]);
            }
        }
        px
    }

    /// Camera-style sRGB JPEG (EXIF ColorSpace 1, orientation) with optional XMP.
    pub fn camera_jpeg(w: u32, h: u32, orientation: u16, xmp: Option<&str>) -> Vec<u8> {
        let base = crate::raw::turbo::encode_rgb_444(&gradient(w, h), w, h, 95).unwrap();
        let mut segs = vec![exif_segment(&exif(orientation, 1, None))];
        if let Some(x) = xmp {
            segs.push(xmp_segment(x));
        }
        with_segments(&base, &segs)
    }

    pub fn packet(body: &str) -> String {
        format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
             <rdf:Description rdf:about=\"\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" \
             xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" {body}/></rdf:RDF></x:xmpmeta>"
        )
    }

    pub fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }
}

use fixtures::*;

fn srgb_linear(code: u8) -> u16 {
    icc::Trc::Srgb.lut(8)[code as usize]
}

#[test]
fn srgb_jpeg_extract_decode_and_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let xmp = packet("xmp:Rating=\"4\" crs:Exposure2012=\"+0.5\"");
    let jpeg = camera_jpeg(64, 48, 6, Some(&xmp));
    let p = write(dir.path(), "DSCF1.JPG", &jpeg);

    let mut buf = Vec::new();
    let x = extract(&p, ImageFormat::Jpeg, &mut buf).unwrap();
    assert!(matches!(x.preview, Ok(Preview::Embedded)), "sRGB JPEG is its own preview");
    assert_eq!(buf, jpeg);
    assert_eq!(x.meta.make, Some(CameraMake::Fujifilm));
    assert_eq!(x.meta.model.as_deref(), Some("X-T5"));
    assert_eq!(x.meta.sensor_layout, None, "no CFA for developed files");
    assert_eq!(x.meta.orientation, Some(6));
    assert_eq!((x.meta.width, x.meta.height), (Some(64), Some(48)));
    assert_eq!(x.meta.iso, Some(400));
    assert!(x.meta.captured_at_ms.is_some());

    // Full decode: linear sRGB of exactly what TurboJPEG decodes.
    let img = decode_linear(&p, ImageFormat::Jpeg, None).unwrap();
    assert_eq!((img.width, img.height, img.bit_depth), (64, 48, 8));
    assert_eq!(img.color_space, SourceColorSpace::Srgb);
    assert_eq!(img.orientation, Some(6));
    let decoded = turbo::decode_rgb(&jpeg, u32::MAX, u64::MAX).unwrap().pixels;
    let expect: Vec<u16> = decoded.iter().map(|&c| srgb_linear(c)).collect();
    assert_eq!(img.pixels, expect);
    // ... and back to 8-bit sRGB exactly (the display-referred neutral round trip).
    assert_eq!(to_srgb8(&img), decoded);

    let li = to_linear_image(img);
    assert!(li.display_referred);
    assert_eq!(li.source_color, Some(SourceColorSpace::Srgb));
    assert_eq!(li.color.as_shot(), [1.0; 3]);
    assert_eq!(li.color.rgb_cam, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);

    // Embedded XMP (read-only) and EXIF for export.
    let got = embedded_xmp(&p, ImageFormat::Jpeg).unwrap().unwrap();
    assert!(got.contains("crs:Exposure2012=\"+0.5\""));
    let dirs = exif_dirs(&p, ImageFormat::Jpeg).unwrap();
    assert!(dirs.ifd0.iter().any(|t| t.tag == 0x010F));
    assert!(dirs.exif.iter().any(|t| t.tag == 0x9003));
    assert_eq!(crate::raw::exif_dirs(&p).unwrap(), dirs);
}

#[test]
fn downscale_is_an_area_average_in_linear_light() {
    let dir = tempfile::tempdir().unwrap();
    // 2x2 checkerboard blocks of 0 and 255: the linear average is 0.5 (sRGB ~188), not 128.
    let (w, h) = (64u32, 32u32);
    let px: Vec<u8> = (0..w * h).flat_map(|i| if (i % w + i / w) % 2 == 0 { [0u8; 3] } else { [255u8; 3] }).collect();
    let png_bytes = crate::raw::png::test_support::encode(w, h, None, Some(&px), None, None, None);
    let p = write(dir.path(), "a.png", &png_bytes);
    let img = decode_linear(&p, ImageFormat::Png, Some(16)).unwrap();
    assert_eq!((img.width, img.height, img.full_width, img.full_height), (16, 8, 64, 32));
    assert_eq!(img.color_space, SourceColorSpace::Unknown(None), "untagged PNG is assumed sRGB");
    assert!(img.color_space.assumed_detail().is_some());
    for v in &img.pixels {
        assert!((i32::from(*v) - 32768).abs() <= 1, "{v}");
    }
    // Non-integer ratio still conserves the mean.
    let img = decode_linear(&p, ImageFormat::Png, Some(20)).unwrap();
    assert_eq!((img.width, img.height), (20, 10));
    let mean = img.pixels.iter().map(|&v| f64::from(v)).sum::<f64>() / img.pixels.len() as f64;
    assert!((mean - 32767.5).abs() < 400.0, "{mean}");
}

#[test]
fn icc_profiles_select_primaries_and_trc() {
    let dir = tempfile::tempdir().unwrap();
    // Display P3 JPEG: pure red stays P3 red in linear light; the preview is sRGB-converted.
    let p3 = rgb_profile("Display P3", icc::known_d50(&SourceColorSpace::DisplayP3).unwrap(), None);
    let red: Vec<u8> = (0..16 * 16).flat_map(|_| [255u8, 0, 0]).collect();
    let base = turbo::encode_rgb_444(&red, 16, 16, 100).unwrap();
    let jpeg = with_segments(&base, &[icc_segment(&p3)]);
    let p = write(dir.path(), "p3.jpg", &jpeg);
    let img = decode_linear(&p, ImageFormat::Jpeg, None).unwrap();
    assert_eq!(img.color_space, SourceColorSpace::DisplayP3);
    let c = &img.pixels[8 * 16 * 3 + 8 * 3..][..3];
    assert!(c[0] > 64000 && c[1] < 300 && c[2] < 300, "{c:?}");
    let li = to_linear_image(img.clone());
    let rows: Vec<f32> = li.color.rgb_cam.iter().map(|r| r.iter().sum()).collect();
    assert!(rows.iter().all(|s| (s - 1.0).abs() < 1e-5), "{rows:?}");
    // P3 red is outside sRGB: red saturates, green/blue clip at 0.
    let s = to_srgb8(&img);
    assert_eq!(&s[8 * 16 * 3 + 8 * 3..][..3], &[255, 0, 0]);
    let mut buf = Vec::new();
    let x = extract(&p, ImageFormat::Jpeg, &mut buf).unwrap();
    assert!(matches!(x.preview, Ok(Preview::Libraw(libraw::Thumb::Rgb { width: 16, height: 16, .. }))));

    // 16-bit Adobe RGB PNG: gamma 563/256 linearization, primaries kept.
    let adobe =
        rgb_profile("Adobe RGB (1998)", icc::known_d50(&SourceColorSpace::AdobeRgb).unwrap(), Some(563.0 / 256.0));
    let px16: Vec<u16> = (0..8 * 8).flat_map(|i| [(i * 1000) as u16, 32768, 65535]).collect();
    let png_bytes = crate::raw::png::test_support::encode(8, 8, Some(&px16), None, Some(&adobe), None, None);
    let p = write(dir.path(), "adobe.png", &png_bytes);
    let img = decode_linear(&p, ImageFormat::Png, None).unwrap();
    assert_eq!((img.color_space.clone(), img.bit_depth), (SourceColorSpace::AdobeRgb, 16));
    let expect = |v: u16| ((f64::from(v) / 65535.0).powf(563.0 / 256.0) * 65535.0).round() as u16;
    assert_eq!(&img.pixels[..3], &[expect(0), expect(32768), 65535]);
    assert_eq!(img.pixels[3 * 5], expect(5000));

    // A generic matrix profile is converted to linear ProPhoto (white stays white).
    let odd = [[0.6, 0.2, 0.1644], [0.3, 0.65, 0.05], [0.0, 0.05, 0.7749]];
    let custom = rgb_profile("Custom Monitor", odd, Some(2.0));
    let white: Vec<u8> = vec![255; 4 * 4 * 3];
    let png_bytes = crate::raw::png::test_support::encode(4, 4, None, Some(&white), Some(&custom), None, None);
    let p = write(dir.path(), "custom.png", &png_bytes);
    let img = decode_linear(&p, ImageFormat::Png, None).unwrap();
    assert_eq!(img.color_space, SourceColorSpace::ProPhoto);
    // The custom white is XYZ(0.9644, 1.0, 0.8249) = D50: ProPhoto (1,1,1).
    assert!(img.pixels[..3].iter().all(|&v| v > 65000), "{:?}", &img.pixels[..3]);

    // Unsupported profile: assumed sRGB with the description as detail.
    let mut broken = custom.clone();
    broken[16..20].copy_from_slice(b"CMYK");
    let base = turbo::encode_rgb_444(&red, 16, 16, 100).unwrap();
    let p = write(dir.path(), "cmyk-icc.jpg", &with_segments(&base, &[icc_segment(&broken)]));
    let img = decode_linear(&p, ImageFormat::Jpeg, None).unwrap();
    assert!(matches!(&img.color_space, SourceColorSpace::Unknown(Some(d)) if d.contains("Custom Monitor")));
}

#[test]
fn exif_adobe_rgb_without_icc() {
    let dir = tempfile::tempdir().unwrap();
    let base = turbo::encode_rgb_444(&gradient(16, 8), 16, 8, 90).unwrap();
    let jpeg = with_segments(&base, &[exif_segment(&exif(1, 0xFFFF, Some("R03")))]);
    let p = write(dir.path(), "DSC_ADOBE.JPG", &jpeg);
    let img = decode_linear(&p, ImageFormat::Jpeg, None).unwrap();
    assert_eq!(img.color_space, SourceColorSpace::AdobeRgb);
    let mut buf = Vec::new();
    let x = extract(&p, ImageFormat::Jpeg, &mut buf).unwrap();
    assert!(matches!(x.preview, Ok(Preview::Libraw(_))), "converted to sRGB for the preview");
}

#[test]
fn extended_xmp_is_merged() {
    let main =
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
        <rdf:Description rdf:about=\"\" xmlns:xmpNote=\"http://ns.adobe.com/xmp/note/\" \
        xmpNote:HasExtendedXMP=\"0123456789ABCDEF0123456789ABCDEF\"/></rdf:RDF></x:xmpmeta>";
    let ext = packet("crs:Exposure2012=\"+1.25\"");
    let guid = b"0123456789ABCDEF0123456789ABCDEF";
    let (a, b) = ext.as_bytes().split_at(40);
    let chunk = |off: usize, data: &[u8]| {
        let mut body = XMP_EXT_APP1.to_vec();
        body.extend_from_slice(guid);
        body.extend_from_slice(&(ext.len() as u32).to_be_bytes());
        body.extend_from_slice(&(off as u32).to_be_bytes());
        body.extend_from_slice(data);
        let mut v = vec![0xFF, 0xE1];
        v.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
        v.extend(body);
        v
    };
    let base = turbo::encode_rgb_444(&gradient(8, 8), 8, 8, 90).unwrap();
    // Chunks out of order on purpose.
    let jpeg = with_segments(&base, &[xmp_segment(main), chunk(40, b), chunk(0, a)]);
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "ext.jpg", &jpeg);
    let x = embedded_xmp(&p, ImageFormat::Jpeg).unwrap().unwrap();
    let v = crate::xmp::packet::parse(&x).unwrap();
    assert_eq!(v.develop.map(|d| d.exposure), Some(1.25));
    // No XMP at all.
    let p = write(dir.path(), "plain.jpg", &base);
    assert_eq!(embedded_xmp(&p, ImageFormat::Jpeg).unwrap(), None);
}

#[test]
fn png_metadata_and_preview() {
    let dir = tempfile::tempdir().unwrap();
    let xmp = packet("xmp:Rating=\"3\"");
    let bytes = crate::raw::png::test_support::encode(
        40,
        20,
        None,
        Some(&gradient(40, 20)),
        None,
        Some(&exif(8, 1, None)),
        Some(&xmp),
    );
    let p = write(dir.path(), "shot.png", &bytes);
    let mut buf = Vec::new();
    let x = extract(&p, ImageFormat::Png, &mut buf).unwrap();
    assert_eq!(x.meta.orientation, Some(8));
    assert_eq!((x.meta.width, x.meta.height), (Some(40), Some(20)));
    assert!(matches!(x.preview, Ok(Preview::Libraw(libraw::Thumb::Rgb { width: 40, height: 20, .. }))));
    assert_eq!(embedded_xmp(&p, ImageFormat::Png).unwrap().as_deref(), Some(xmp.as_str()));
    assert!(!exif_dirs(&p, ImageFormat::Png).unwrap().ifd0.is_empty());
    // Untagged PNG without EXIF: make other, no capture time.
    let bytes = crate::raw::png::test_support::encode(4, 4, None, Some(&gradient(4, 4)), None, None, None);
    let p = write(dir.path(), "plain.png", &bytes);
    let x = extract(&p, ImageFormat::Png, &mut buf).unwrap();
    assert_eq!((x.meta.make, x.meta.captured_at_ms), (Some(CameraMake::Other), None));
    assert_eq!(exif_dirs(&p, ImageFormat::Png).unwrap(), tiff::ExifDirs::default());
}

#[cfg(target_os = "macos")]
#[test]
fn tiff_16bit_via_imageio_is_exact() {
    use crate::export::tiffw;
    use crate::ipc::types::TiffCompression;
    let dir = tempfile::tempdir().unwrap();
    let (w, h) = (48u32, 32u32);
    let px: Vec<u16> =
        (0..w * h).flat_map(|i| [((i * 37) % 65536) as u16, ((i * 911) % 65536) as u16, 40000]).collect();
    let pro = rgb_profile("ProPhoto RGB", icc::known_d50(&SourceColorSpace::ProPhoto).unwrap(), Some(1.8));
    let xmp = packet("xmp:Rating=\"5\"");
    let meta = tiffw::Dirs {
        ifd0: vec![
            tiffw::undefined(tiffw::ICC_PROFILE, &pro),
            tiffw::bytes(tiffw::XMP, xmp.as_bytes()),
            tiffw::ascii(tiffw::MAKE, "Canon"),
            tiffw::short(tiffw::ORIENTATION, &[3]),
        ],
        ..Default::default()
    };
    let p = dir.path().join("edit.tif");
    let f = std::fs::File::create(&p).unwrap();
    tiffw::write_tiff(f, w, h, tiffw::Samples::U16(&px), TiffCompression::Lzw, 300, &meta).unwrap();
    // The export writer always stores orientation 1; patch IFD0's entry to 3 (rotated 180).
    let mut bytes = std::fs::read(&p).unwrap();
    let le32 = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize;
    let ifd = le32(&bytes, 4);
    let n = u16::from_le_bytes([bytes[ifd], bytes[ifd + 1]]) as usize;
    let entry = (0..n).map(|i| ifd + 2 + i * 12).find(|&e| bytes[e..e + 2] == 274u16.to_le_bytes()).unwrap();
    bytes[entry + 8..entry + 10].copy_from_slice(&3u16.to_le_bytes());
    std::fs::write(&p, &bytes).unwrap();

    let img = decode_linear(&p, ImageFormat::Tiff, None).unwrap();
    assert_eq!((img.width, img.height, img.bit_depth), (w, h, 16));
    assert_eq!(img.color_space, SourceColorSpace::ProPhoto);
    assert_eq!(img.orientation, Some(3));
    let lut = icc::Trc::Gamma((1.8f64 * 256.0).round() / 256.0).lut(16);
    let expect: Vec<u16> = px.iter().map(|&v| lut[v as usize]).collect();
    let max_diff = img.pixels.iter().zip(&expect).map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs()).max().unwrap();
    assert!(max_diff <= 1, "ImageIO must not colour-convert when drawing into the image's own space ({max_diff})");

    let mut buf = Vec::new();
    let x = extract(&p, ImageFormat::Tiff, &mut buf).unwrap();
    assert_eq!(x.meta.make, Some(CameraMake::Canon));
    assert_eq!(x.meta.orientation, Some(3));
    assert_eq!((x.meta.width, x.meta.height), (Some(w), Some(h)));
    assert!(matches!(x.preview, Ok(Preview::Libraw(libraw::Thumb::Rgb { .. }))));
    assert!(embedded_xmp(&p, ImageFormat::Tiff).unwrap().unwrap().contains("xmp:Rating=\"5\""));
    assert!(exif_dirs(&p, ImageFormat::Tiff).unwrap().ifd0.iter().any(|t| t.tag == tiffw::MAKE));
}

#[cfg(target_os = "macos")]
#[test]
fn imageio_png_decode_matches_png_crate() {
    let dir = tempfile::tempdir().unwrap();
    let p3 = rgb_profile("Display P3", icc::known_d50(&SourceColorSpace::DisplayP3).unwrap(), None);
    let bytes = crate::raw::png::test_support::encode(32, 16, None, Some(&gradient(32, 16)), Some(&p3), None, None);
    let p = write(dir.path(), "p3.png", &bytes);
    let native = imageio::decode_native(&p).unwrap();
    let ours = crate::raw::png::decode(&p).unwrap();
    match (native.samples, ours.samples) {
        (imageio::Samples::U8(a), crate::raw::png::Samples::U8(b)) => assert_eq!(a, b),
        _ => panic!("expected 8-bit"),
    }
    assert!(native.icc.is_some());
    assert_eq!(icc::parse(&native.icc.unwrap()).unwrap().space, SourceColorSpace::DisplayP3);
}

#[cfg(target_os = "macos")]
#[test]
fn heic_via_imageio() {
    if crate::export::heic::probe().is_err() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (w, h) = (64u32, 48u32);
    let rgb = gradient(w, h);
    let icc_bytes = crate::export::color::icc_profile(crate::ipc::types::ExportColorSpace::DisplayP3);
    let xmp = packet("xmp:Rating=\"2\"");
    let p = dir.path().join("IMG_1.HEIC");
    crate::export::heic::write(&rgb, w, h, 95, icc_bytes, Some(&xmp), &p).unwrap();

    let img = decode_linear(&p, ImageFormat::Heic, None).unwrap();
    assert_eq!((img.width, img.height), (w, h));
    assert_eq!(img.color_space, SourceColorSpace::DisplayP3);
    // Lossy HEVC: compare in encoded space.
    let back = {
        let lut = srgb_encode_lut();
        img.pixels.iter().map(|&v| lut[v as usize]).collect::<Vec<u8>>()
    };
    let mean_err = back.iter().zip(&rgb).map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs()).sum::<i32>() as f64
        / rgb.len() as f64;
    assert!(mean_err < 3.0, "{mean_err}");

    let mut buf = Vec::new();
    let x = extract(&p, ImageFormat::Heic, &mut buf).unwrap();
    assert_eq!((x.meta.width, x.meta.height), (Some(w), Some(h)));
    assert!(matches!(x.preview, Ok(Preview::Libraw(libraw::Thumb::Rgb { width: 64, height: 48, .. }))));
    let got = embedded_xmp(&p, ImageFormat::Heic).unwrap();
    assert!(got.is_some_and(|x| x.contains("Rating")), "XMP item readable");
}

#[test]
fn heif_container_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let xmp = packet("xmp:Rating=\"1\"");
    let bytes = crate::raw::heif::test_support::heif(Some(&exif(1, 1, None)), Some(&xmp), (4032, 3024), 3);
    let p = write(dir.path(), "x.heic", &bytes);
    assert_eq!(embedded_xmp(&p, ImageFormat::Heic).unwrap().as_deref(), Some(xmp.as_str()));
    assert!(exif_dirs(&p, ImageFormat::Heic).unwrap().ifd0.iter().any(|t| t.tag == 0x0110));
}

/// Real camera JPEGs (read-only): exact linear round trip, EXIF, and how far the current
/// develop pipeline's neutral non-RAW render is from the original (printed; the pipeline
/// must honour `LinearImage::display_referred` for this to reach <= 1-2 levels).
#[test]
#[ignore = "needs sample JPEGs ($SIEVE_SAMPLE_JPEG_DIR)"]
fn real_jpegs_round_trip_and_neutral_render() {
    let dir = std::env::var("SIEVE_SAMPLE_JPEG_DIR")
        .unwrap_or_else(|_| "/Users/gurjotsingh/Pictures/Jasmit Natalie Proposal/Fujifilm".into());
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| crate::raw::format_from_extension(p) == Some(ImageFormat::Jpeg))
        .collect();
    files.sort();
    assert!(!files.is_empty());
    for p in files.iter().take(3) {
        let bytes = std::fs::read(p).unwrap();
        let img = decode_linear(p, ImageFormat::Jpeg, None).unwrap();
        let decoded = turbo::decode_rgb(&bytes, u32::MAX, u64::MAX).unwrap().pixels;
        assert_eq!(to_srgb8(&img), decoded, "{}", p.display());
        eprintln!(
            "{}: {}x{} {:?} orientation {:?}",
            p.display(),
            img.full_width,
            img.full_height,
            img.color_space,
            img.orientation
        );
        // Editor source + neutral render at 1024 px.
        let li = crate::develop::source::decode_half_size(p).unwrap();
        assert!(li.display_referred && li.width.max(li.height) <= 4096);
        let prep = crate::develop::source::prepare(&li, 1, &crate::ipc::types::CropSettings::default(), None, 1024);
        let profile = crate::develop::camera::Profile { display_referred: true, ..Default::default() };
        let input = crate::develop::pipeline::RenderInput {
            width: prep.width,
            height: prep.height,
            pixels: &prep.pixels,
            color: &li.color,
            frame_long_edge: prep.frame_long_edge,
            view: prep.view,
            profile: &profile,
            seed: 1,
            quality: crate::develop::pipeline::Quality::Preview,
        };
        let neutral = ParametricAdjustmentsExt::non_raw();
        let out = crate::develop::pipeline::render(&input, &neutral, None);
        let lut = srgb_encode_lut();
        let reference: Vec<u8> = prep.pixels.iter().map(|&v| lut[v as usize]).collect();
        let diffs: Vec<i32> =
            out.rgb.iter().zip(&reference).map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs()).collect();
        let mean = diffs.iter().sum::<i32>() as f64 / diffs.len() as f64;
        let within2 = diffs.iter().filter(|d| **d <= 2).count() as f64 / diffs.len() as f64;
        let max = *diffs.iter().max().unwrap();
        eprintln!("  neutral render vs original: mean |d| {mean:.2}, max {max}, <=2 levels {:.1}%", within2 * 100.0);
        // Display-referred sources: neutral settings reproduce the file (Phase 7b).
        assert!(mean <= 1.0 && max <= 2, "{}: mean {mean:.2} max {max}", p.display());
    }
}

struct ParametricAdjustmentsExt;
impl ParametricAdjustmentsExt {
    fn non_raw() -> crate::ipc::types::ParametricAdjustments {
        crate::ipc::types::ParametricAdjustments::defaults_for(ImageFormat::Jpeg)
    }
}

#[test]
fn raw_formats_are_rejected() {
    let p = Path::new("/nonexistent/a.ARW");
    assert!(decode_linear(p, ImageFormat::Arw, None).is_err());
}
