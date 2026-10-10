//! Full-resolution develop for export: decode -> resample (linear light, Lanczos-3, the
//! preview's own resampler) ->
//! orientation -> the shared parametric pipeline (`develop::pipeline::Stages`, identical to
//! the preview) -> output colour space -> output sharpening -> quantize.
//!
//! The decode lives in `develop::source::decode_full` next to the preview's half-size
//! decode (shared LibRaw FFI and settings) and is re-exported here.
//!
//! Colour rule (in `pipeline::render_output`): without a LUT, linear Rec.2020 goes straight
//! to the target space (keeps P3 / Adobe RGB gamut); with a LUT, the LUT runs on
//! sRGB-encoded values as in the preview and the result is converted to the target space.
//!
//! Memory (per image, output `n` px, source `m` px): the decode (`6m` bytes after LibRaw is
//! released) is dropped once resampled; the pipeline holds its u16 input and output (`12n`),
//! sharpening one f32 plane (`4n`), 8-bit quantization a `3n` copy. Full-size frames with
//! orientation 1 skip the resample and use the decode directly.

use std::borrow::Cow;
use std::path::Path;

use rayon::prelude::*;

use super::color;
use crate::develop::camera::Profile;
use crate::develop::pipeline::{self, Quality, RenderInput, View};
use crate::develop::source::{self, ColorInfo, LinearImage};
use crate::develop::transform::Geometry;
use crate::ipc::error::AppResult;
use crate::ipc::types::{
    BitDepth, CropSettings, ExportSettings, OutputSharpening, ParametricAdjustments, ResizeMode, ResizeOptions,
    SharpenAmount, SharpenMedia,
};
use crate::lut::Lut;

/// Developed, resized, sharpened, quantized pixels ready for the encoder.
#[derive(Debug, Clone)]
pub struct ExportImage {
    pub width: u32,
    pub height: u32,
    /// Interleaved RGB in the target colour space's transfer curve, orientation applied.
    pub pixels: ExportPixels,
}

#[derive(Debug, Clone)]
pub enum ExportPixels {
    Rgb8(Vec<u8>),
    Rgb16(Vec<u16>),
}

/// Blocking full-resolution decode: LibRaw full demosaic with the same settings as the
/// preview's half-size decode (camera RGB, no WB, linear, 16-bit, `highlight = 0`,
/// `user_flip = 0`) except `half_size = 0` and `user_qual = 3` (AHD for Bayer; LibRaw runs
/// its 3-pass Markesteijn interpolation for X-Trans at that quality). Releases LibRaw's
/// buffers before returning. `full_width/full_height` equal `width/height`.
pub fn decode_full(path: &Path) -> AppResult<LinearImage> {
    source::decode_full(path)
}

/// Output pixel size for a full-resolution (orientation-corrected) `full` size:
/// - `none` -> `full`;
/// - `long_edge{px}` / `short_edge{px}` -> that edge = px, other edge rounded to nearest;
/// - `megapixels{mp}` -> scale = sqrt(mp * 1e6 / (w * h)), both edges rounded;
/// - `width_height{w, h}` -> largest size with the same aspect that fits in w x h;
/// - `dont_enlarge` -> never larger than `full` (returns `full` when the target is larger);
/// - every edge >= 1.
pub fn output_size(full: (u32, u32), resize: &ResizeOptions) -> (u32, u32) {
    let (w, h) = (full.0.max(1), full.1.max(1));
    let (fw, fh) = (f64::from(w), f64::from(h));
    let round = |v: f64| (v.round() as u32).max(1);
    let out = match resize.mode {
        ResizeMode::None => (w, h),
        ResizeMode::LongEdge { px } => {
            if w >= h {
                (px, round(fh * f64::from(px) / fw))
            } else {
                (round(fw * f64::from(px) / fh), px)
            }
        }
        ResizeMode::ShortEdge { px } => {
            if w <= h {
                (px, round(fh * f64::from(px) / fw))
            } else {
                (round(fw * f64::from(px) / fh), px)
            }
        }
        ResizeMode::Megapixels { mp } => {
            let s = (f64::from(mp) * 1e6 / (fw * fh)).sqrt();
            (round(fw * s), round(fh * s))
        }
        ResizeMode::WidthHeight { width, height } => {
            let s = (f64::from(width) / fw).min(f64::from(height) / fh);
            (round(fw * s).min(width.max(1)), round(fh * s).min(height.max(1)))
        }
    };
    let out = (out.0.max(1), out.1.max(1));
    if resize.dont_enlarge && (out.0 > w || out.1 > h) {
        (w, h)
    } else {
        out
    }
}

fn orientation(o: Option<u8>) -> u8 {
    o.filter(|o| (1..=8).contains(o)).unwrap_or(1)
}

/// Oriented output size of `src`'s cropped frame under `resize` (`crop`: the effective crop of
/// the render [`Geometry`]).
pub fn planned_size(
    src: &LinearImage,
    orientation_tag: Option<u8>,
    crop: &CropSettings,
    resize: &ResizeOptions,
) -> (u32, u32) {
    let o = orientation(orientation_tag);
    output_size(source::frame_size(src.width, src.height, o, crop), resize)
}

/// Lanczos-3 resample of interleaved RGB16 (linear light), rows in parallel: the same
/// resampler as the editor preview (`develop::source::resample`), so an export at the
/// preview's size sees identical pipeline input.
pub fn resample_lanczos(src: &[u16], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u16> {
    source::resample(src, sw as usize, sh as usize, [0.0, 0.0, f64::from(sw), f64::from(sh)], dw as usize, dh as usize)
}

/// Resampled export input: pixels + the coverage of the image (`source::Prepared::coverage`).
pub type PreparedOutput = (Vec<u16>, Option<Vec<u8>>);

/// Crops/straightens/warps `src` (unrotated) and resamples it to the oriented output size
/// `out` with orientation applied, like the editor preview (`source::prepare_sized`). `None`
/// when the decode can be used as is (no crop, no warp, same size, orientation 1).
pub fn prepare_output(
    src: &LinearImage,
    orientation_tag: Option<u8>,
    geo: &Geometry,
    out: (u32, u32),
) -> AppResult<Option<PreparedOutput>> {
    let o = orientation(orientation_tag);
    if !geo.crop.enabled && geo.warp.is_none() && o == 1 && out == (src.width, src.height) {
        return Ok(None);
    }
    let p = source::prepare_sized_geo(src, o, geo, None, out.0, out.1);
    Ok(Some((p.pixels, p.coverage)))
}

/// Output px per full-resolution frame px for an export of `size` from `src`.
pub fn export_scale(src: &LinearImage, orientation_tag: Option<u8>, crop: &CropSettings, size: (u32, u32)) -> f32 {
    let o = orientation(orientation_tag);
    let frame = source::frame_size(src.width, src.height, o, crop);
    let half = src.width as f32 / src.full_width.max(1) as f32;
    size.0 as f32 / frame.0.max(1) as f32 * half
}

/// Blocking: `src` (full decode) -> crop/straighten + resample to `output_size` in linear
/// light (orientation from EXIF `orientation`, 1..=8, applied) -> shared pipeline with
/// `profile` -> output colour space -> output sharpening (`settings.sharpening`,
/// radius/amount by media and output size, on the encoded luminance) -> quantize to
/// `settings.format.bit_depth()`. `lut` is the resolved `adjustments.lut`.
pub fn render_full(
    src: &LinearImage,
    orientation: Option<u8>,
    adjustments: &ParametricAdjustments,
    lut: Option<&Lut>,
    settings: &ExportSettings,
    profile: &Profile,
    seed: u64,
) -> AppResult<ExportImage> {
    let geo = Geometry::of(adjustments, src.full_width, src.full_height);
    let size = planned_size(src, orientation, &geo.crop, &settings.resize);
    let scale = export_scale(src, orientation, &geo.crop, size);
    let prepared = prepare_output(src, orientation, &geo, size)?;
    let (pixels, coverage): (Cow<[u16]>, _) = match prepared {
        Some((p, c)) => (Cow::Owned(p), c),
        None => (Cow::Borrowed(&src.pixels[..]), None),
    };
    let tone = crate::develop::pipeline::tone_context(src, orientation_code(orientation), adjustments, profile);
    let ctx = DevelopContext { profile, scale, seed, tone: Some(&tone), masks: None };
    let mut encoded = develop_prepared(&pixels, size, &src.color, adjustments, lut, settings, &ctx);
    drop(pixels);
    if let Some(c) = &coverage {
        source::fill_outside_rgb16(&mut encoded, c);
    }
    Ok(finish(encoded, size, settings))
}

/// Per-image context of an export develop.
pub struct DevelopContext<'a> {
    pub profile: &'a Profile,
    /// Output px per full-resolution frame px.
    pub scale: f32,
    /// Grain seed (the image id).
    pub seed: u64,
    /// Local tone context of the whole uncropped source (`pipeline::tone_context`).
    pub tone: Option<&'a crate::develop::pipeline::ToneContext>,
    /// Local adjustments evaluated on the output grid (`develop::masks::render`).
    pub masks: Option<&'a crate::develop::masks::LocalPlanes>,
}

/// EXIF orientation code (1..=8) of an optional tag.
pub fn orientation_code(tag: Option<u8>) -> u8 {
    orientation(tag)
}

/// Neutral sensor-frame render of `src` (<= 2048 px): the guide Sieve AI mattes are refined
/// against (`develop::masks::render::ResolvedMattes::refine`).
pub fn sensor_guide(src: &LinearImage, profile: &Profile) -> crate::develop::masks::render::SensorGuide {
    let prep = source::prepare(src, 1, &CropSettings::default(), None, 2048);
    let input = RenderInput {
        frame_long_edge: prep.frame_long_edge,
        view: prep.view,
        quality: Quality::Draft,
        ..RenderInput::simple(prep.width, prep.height, &prep.pixels, &src.color, profile)
    };
    let img = pipeline::render(&input, &ParametricAdjustments::default(), None);
    crate::develop::masks::render::SensorGuide { width: img.width, height: img.height, rgb: img.rgb }
}

/// Pipeline + output colour space on prepared (cropped, oriented, output-size) camera RGB.
pub fn develop_prepared(
    pixels: &[u16],
    size: (u32, u32),
    color: &ColorInfo,
    adjustments: &ParametricAdjustments,
    lut: Option<&Lut>,
    settings: &ExportSettings,
    ctx: &DevelopContext,
) -> Vec<u16> {
    let input = RenderInput {
        width: size.0,
        height: size.1,
        pixels,
        color,
        frame_long_edge: size.0.max(size.1) as f32,
        view: View::whole(size.0, size.1, ctx.scale),
        profile: ctx.profile,
        seed: ctx.seed,
        quality: Quality::Export,
        tone: ctx.tone,
    };
    pipeline::render_output_masked(&input, adjustments, lut, color::output_space(settings.color_space), ctx.masks)
}

/// Output sharpening + quantization of the encoded 16-bit pipeline output.
pub fn finish(mut encoded: Vec<u16>, size: (u32, u32), settings: &ExportSettings) -> ExportImage {
    if let Some(s) = settings.sharpening {
        let (radius, amount) = sharpen_params(s, size, settings.resize.resolution_ppi);
        sharpen(&mut encoded, size.0 as usize, size.1 as usize, radius, amount);
    }
    let pixels = match settings.format.bit_depth() {
        BitDepth::Sixteen => ExportPixels::Rgb16(encoded),
        BitDepth::Eight => {
            let out: Vec<u8> = encoded.par_iter().map(|&v| ((u32::from(v) * 255 + 32767) / 65535) as u8).collect();
            drop(encoded);
            ExportPixels::Rgb8(out)
        }
    };
    ExportImage { width: size.0, height: size.1, pixels }
}

/// Unsharp-mask radius (gaussian sigma, output px) and amount for Lightroom-style output
/// sharpening. Screen: 0.6-1.0 px growing with the output size (web sizes need a finer
/// radius). Print (matte/glossy): radius scales with the print resolution (1 px at 300 ppi
/// for glossy, 1.25 px for matte, which loses more detail to ink spread); matte also needs
/// more amount. Low/standard/high scale the amount.
pub fn sharpen_params(s: OutputSharpening, size: (u32, u32), ppi: u32) -> (f32, f32) {
    let long = size.0.max(size.1) as f32;
    let print = (ppi as f32 / 300.0).clamp(0.5, 2.5);
    let (radius, base) = match s.media {
        SharpenMedia::Screen => (0.6 + 0.4 * ((long - 1200.0) / 4800.0).clamp(0.0, 1.0), 0.6),
        SharpenMedia::Glossy => (1.0 * print, 0.8),
        SharpenMedia::Matte => (1.25 * print, 1.1),
    };
    let scale = match s.amount {
        SharpenAmount::Low => 0.6,
        SharpenAmount::Standard => 1.0,
        SharpenAmount::High => 1.5,
    };
    (radius, base * scale)
}

/// Luminance unsharp mask on encoded RGB16: `L += amount * soft_threshold(L - blur(L))`,
/// the delta (limited to +-0.08 to avoid halos) added equally to R, G and B so hues do not
/// shift. One f32 plane of scratch memory.
pub fn sharpen(px: &mut [u16], w: usize, h: usize, sigma: f32, amount: f32) {
    if w == 0 || h == 0 || sigma <= 0.0 || amount <= 0.0 {
        return;
    }
    let r = (sigma * 3.0).ceil() as isize;
    let mut kernel: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = kernel.iter().sum();
    kernel.iter_mut().for_each(|k| *k /= sum);
    const LW: [f32; 3] = [0.2126 / 65535.0, 0.7152 / 65535.0, 0.0722 / 65535.0];
    let lum = |p: &[u16]| LW[0] * f32::from(p[0]) + LW[1] * f32::from(p[1]) + LW[2] * f32::from(p[2]);

    // Horizontal pass into `tmp`.
    let mut tmp = vec![0.0f32; w * h];
    tmp.par_chunks_mut(w).zip(px.par_chunks(w * 3)).for_each_init(
        || Vec::with_capacity(w),
        |l: &mut Vec<f32>, (out, row)| {
            l.clear();
            l.extend(row.as_chunks::<3>().0.iter().map(|p| lum(p)));
            for (x, o) in out.iter_mut().enumerate() {
                let mut acc = 0.0;
                for (k, kv) in kernel.iter().enumerate() {
                    let xi = (x as isize + k as isize - r).clamp(0, w as isize - 1) as usize;
                    acc += kv * l[xi];
                }
                *o = acc;
            }
        },
    );
    // Vertical pass + apply (reads `tmp` only; each row writes its own pixels).
    const THRESHOLD: f32 = 0.5 / 255.0;
    const LIMIT: f32 = 0.08;
    px.par_chunks_mut(w * 3).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let mut blur = 0.0;
            for (k, kv) in kernel.iter().enumerate() {
                let yi = (y as isize + k as isize - r).clamp(0, h as isize - 1) as usize;
                blur += kv * tmp[yi * w + x];
            }
            let p = &mut row[x * 3..x * 3 + 3];
            let d = lum(p) - blur;
            let d = d.signum() * (d.abs() - THRESHOLD).max(0.0);
            let delta = (amount * d).clamp(-LIMIT, LIMIT) * 65535.0;
            for c in p.iter_mut() {
                *c = (f32::from(*c) + delta).round().clamp(0.0, 65535.0) as u16;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::pipeline::render;
    use crate::ipc::types::{
        ChromaSubsampling, CollisionPolicy, ExportColorSpace, ExportDestination, ExportFormat, FileNaming,
        MetadataInclude, MetadataOptions,
    };

    fn prof() -> Profile {
        Profile::matrix(crate::develop::pipeline::BASELINE_EV)
    }

    fn opts(mode: ResizeMode, dont_enlarge: bool) -> ResizeOptions {
        ResizeOptions { mode, dont_enlarge, resolution_ppi: 300 }
    }

    #[test]
    fn output_sizes() {
        let l = (6000, 4000);
        let p = (4000, 6000);
        assert_eq!(output_size(l, &opts(ResizeMode::None, true)), l);
        assert_eq!(output_size(l, &opts(ResizeMode::LongEdge { px: 2048 }, true)), (2048, 1365));
        assert_eq!(output_size(p, &opts(ResizeMode::LongEdge { px: 2048 }, true)), (1365, 2048));
        assert_eq!(output_size(l, &opts(ResizeMode::ShortEdge { px: 1600 }, true)), (2400, 1600));
        assert_eq!(output_size(p, &opts(ResizeMode::ShortEdge { px: 1600 }, true)), (1600, 2400));
        let (w, h) = output_size(l, &opts(ResizeMode::Megapixels { mp: 6.0 }, true));
        assert_eq!((w, h), (3000, 2000));
        assert_eq!(output_size(l, &opts(ResizeMode::WidthHeight { width: 1000, height: 1000 }, true)), (1000, 667));
        assert_eq!(output_size(p, &opts(ResizeMode::WidthHeight { width: 1000, height: 1000 }, true)), (667, 1000));
        // Don't enlarge.
        assert_eq!(output_size(l, &opts(ResizeMode::LongEdge { px: 8000 }, true)), l);
        assert_eq!(output_size(l, &opts(ResizeMode::LongEdge { px: 8000 }, false)), (8000, 5333));
        assert_eq!(output_size(l, &opts(ResizeMode::Megapixels { mp: 100.0 }, true)), l);
        // Edges >= 1.
        assert_eq!(output_size((10000, 10), &opts(ResizeMode::LongEdge { px: 16 }, true)), (16, 1));
    }

    fn settings(color_space: ExportColorSpace, format: ExportFormat) -> ExportSettings {
        ExportSettings {
            format,
            color_space,
            resize: opts(ResizeMode::None, true),
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

    fn test_image(w: u32, h: u32) -> LinearImage {
        let mut pixels = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let v = x as f32 / (w - 1) as f32;
                let c = if y < h / 2 { [v, v, v] } else { [v, v * 0.4, v * 0.15] };
                pixels.extend(c.map(|c| (c * 0.5 * 65535.0) as u16));
            }
        }
        LinearImage {
            width: w,
            height: h,
            pixels,
            color: ColorInfo {
                as_shot_mul: Some([1.0; 3]),
                daylight_mul: [1.0; 3],
                rgb_cam: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                xyz_to_cam: [
                    [3.2404542, -1.5371385, -0.4985314],
                    [-0.969266, 1.8760108, 0.041556],
                    [0.0556434, -0.2040259, 1.0572252],
                ],
                calibration: [1.0; 3],
            },
            full_width: w,
            full_height: h,
            display_referred: false,
            source_color: None,
        }
    }

    /// WYSIWYG: an sRGB 8-bit export without sharpening at the preview's size equals the
    /// preview render of the same input within 1 level.
    #[test]
    fn srgb_export_matches_preview() {
        let src = test_image(320, 200);
        let adj = ParametricAdjustments {
            exposure: 0.4,
            contrast: 25.0,
            shadows: 30.0,
            vibrance: 20.0,
            ..Default::default()
        };
        let s = settings(
            ExportColorSpace::Srgb,
            ExportFormat::Jpeg { quality: 90, chroma_subsampling: ChromaSubsampling::Yuv444 },
        );
        let img = render_full(&src, Some(1), &adj, None, &s, &prof(), 1).unwrap();
        let ExportPixels::Rgb8(out) = img.pixels else { panic!("8-bit expected") };
        let p = prof();
        let mut input = RenderInput::simple(320, 200, &src.pixels, &src.color, &p);
        input.seed = 1;
        let preview = render(&input, &adj, None);
        let max = out.iter().zip(&preview.rgb).map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs()).max().unwrap();
        assert!(max <= 1, "max diff {max}");
    }

    #[test]
    fn wide_gamut_spaces_differ_and_greys_stay_neutral() {
        let src = test_image(64, 16);
        // No detail processing: the grey rows must not pick up chroma from the colour rows.
        let mut adj = ParametricAdjustments::default();
        adj.detail.sharpening.amount = 0.0;
        adj.detail.noise_reduction.color = 0.0;
        let tiff =
            ExportFormat::Tiff { bit_depth: BitDepth::Sixteen, compression: crate::ipc::types::TiffCompression::None };
        let get =
            |cs| match render_full(&src, None, &adj, None, &settings(cs, tiff.clone()), &prof(), 1).unwrap().pixels {
                ExportPixels::Rgb16(v) => v,
                _ => panic!(),
            };
        let (s, p3, adobe) =
            (get(ExportColorSpace::Srgb), get(ExportColorSpace::DisplayP3), get(ExportColorSpace::AdobeRgb));
        // Grey pixel (top half) equal in sRGB and P3 (same transfer), neutral in Adobe RGB
        // (16-bit tolerance: 16 = 0.06 8-bit levels).
        let i = (2 * 64 + 40) * 3;
        assert!((i32::from(s[i]) - i32::from(p3[i])).abs() <= 16);
        assert!(
            (i32::from(adobe[i]) - i32::from(adobe[i + 2])).abs() <= 16,
            "{:?} {:?}",
            &adobe[i..i + 3],
            &s[i..i + 3]
        );
        // Saturated orange (bottom half): smaller numbers in the wider gamuts.
        let j = (12 * 64 + 60) * 3;
        let sat = |v: &[u16]| i32::from(v[j]) - i32::from(v[j + 2]);
        assert!(sat(&p3) < sat(&s) && sat(&adobe) < sat(&s), "{} {} {}", sat(&s), sat(&p3), sat(&adobe));
    }

    #[test]
    fn orientation_and_resize_apply() {
        let src = test_image(300, 200);
        let mut s = settings(ExportColorSpace::Srgb, ExportFormat::Png { bit_depth: BitDepth::Eight });
        s.resize = opts(ResizeMode::LongEdge { px: 150 }, true);
        let img = render_full(&src, Some(6), &ParametricAdjustments::default(), None, &s, &prof(), 1).unwrap();
        assert_eq!((img.width, img.height), (100, 150));
        let ExportPixels::Rgb8(px) = img.pixels else { panic!() };
        assert_eq!(px.len(), 100 * 150 * 3);
        // Orientation 6: the source's left edge (dark) is now at the top.
        let top: u32 = px[..100 * 3].iter().map(|&v| u32::from(v)).sum();
        let bottom: u32 = px[px.len() - 100 * 3..].iter().map(|&v| u32::from(v)).sum();
        assert!(top < bottom, "{top} {bottom}");
    }

    #[test]
    fn sharpening_raises_edge_contrast_only() {
        let (w, h) = (64usize, 8usize);
        let mut px: Vec<u16> =
            (0..w * h).flat_map(|i| if i % w < 32 { [20000u16; 3] } else { [40000u16; 3] }).collect();
        let flat_before = px[3 * 5];
        sharpen(&mut px, w, h, 1.0, 1.0);
        assert_eq!(px[3 * 5], flat_before, "flat areas untouched");
        assert!(px[31 * 3] < 20000 && px[32 * 3] > 40000, "{} {}", px[31 * 3], px[32 * 3]);
        let (r, a) = sharpen_params(
            OutputSharpening { media: SharpenMedia::Screen, amount: SharpenAmount::Standard },
            (2048, 1365),
            72,
        );
        assert!(r > 0.5 && r < 1.0 && a > 0.0);
        let (rg, _) = sharpen_params(
            OutputSharpening { media: SharpenMedia::Glossy, amount: SharpenAmount::High },
            (6000, 4000),
            300,
        );
        assert!((rg - 1.0).abs() < 1e-6);
    }

    /// WYSIWYG on real RAWs: an sRGB 8-bit export at the preview's size (no sharpening)
    /// vs `render_preview`'s pipeline output (half-size decode + the same Lanczos-3
    /// resample). Where the half-size and full decodes coincide (non-CFA / sRAW-style files)
    /// the result must match within 2 levels per channel; for CFA files the demosaic
    /// differs (2x2 binning vs AHD), so only tone/colour are compared (16x16 block means).
    #[test]
    #[ignore = "needs sample RAWs ($SIEVE_SAMPLES)"]
    fn real_raw_export_matches_preview() {
        let folder = std::env::var("SIEVE_SAMPLES").unwrap_or_else(|_| "/Users/gurjotsingh/Pictures/test RAWS".into());
        let mut raws: Vec<_> = std::fs::read_dir(&folder)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| crate::raw::format_from_extension(p).is_some())
            .collect();
        raws.sort();
        let adj = ParametricAdjustments {
            exposure: 0.3,
            contrast: 15.0,
            shadows: 25.0,
            vibrance: 20.0,
            ..Default::default()
        };
        for raw in raws.iter().take(3) {
            let half = source::decode_half_size(raw).unwrap();
            let prep = source::prepare(&half, 1, &CropSettings::default(), None, 1024);
            let p = prof();
            // As the app: the whole-source local tone context (`DevelopCache`).
            let tone = crate::develop::pipeline::tone_context(&half, 1, &adj, &p);
            let input = RenderInput {
                width: prep.width,
                height: prep.height,
                pixels: &prep.pixels,
                color: &half.color,
                frame_long_edge: prep.frame_long_edge,
                view: prep.view,
                profile: &p,
                seed: 1,
                quality: crate::develop::pipeline::Quality::Preview,
                tone: Some(&tone),
            };
            let preview = render(&input, &adj, None);
            let full = decode_full(raw).unwrap();
            let mut s = settings(
                ExportColorSpace::Srgb,
                ExportFormat::Jpeg { quality: 90, chroma_subsampling: ChromaSubsampling::Yuv444 },
            );
            s.resize = opts(ResizeMode::LongEdge { px: 1024 }, true);
            let img = render_full(&full, Some(1), &adj, None, &s, &prof(), 1).unwrap();
            assert_eq!((img.width, img.height), (preview.width, preview.height));
            let ExportPixels::Rgb8(out) = img.pixels else { panic!() };
            let mut d: Vec<u8> = out.iter().zip(&preview.rgb).map(|(a, b)| a.abs_diff(*b)).collect();
            let mean = d.iter().map(|&v| f64::from(v)).sum::<f64>() / d.len() as f64;
            d.sort_unstable();
            let (p50, p99) = (d[d.len() / 2], d[d.len() * 99 / 100]);
            // 16x16 block means: remove the resampling-filter (edge) differences.
            const B: usize = 16;
            let (w, h) = (img.width as usize, img.height as usize);
            let block = |px: &[u8], bx: usize, by: usize, c: usize| -> f64 {
                let mut s = 0.0;
                for y in by * B..by * B + B {
                    for x in bx * B..bx * B + B {
                        s += f64::from(px[(y * w + x) * 3 + c]);
                    }
                }
                s / (B * B) as f64
            };
            let mut bd = Vec::new();
            for by in 0..h / B {
                for bx in 0..w / B {
                    for c in 0..3 {
                        bd.push((block(&out, bx, by, c) - block(&preview.rgb, bx, by, c)).abs());
                    }
                }
            }
            bd.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let bp99 = bd[bd.len() * 99 / 100];
            eprintln!(
                "{}: per pixel mean {mean:.3} p50 {p50} p99 {p99} max {} | 16x16 blocks p99 {bp99:.2} max {:.2}",
                raw.display(),
                d[d.len() - 1],
                bd[bd.len() - 1]
            );
            if (half.width, half.height) == (full.width, full.height) {
                assert!(d[d.len() - 1] <= 2, "same decode: max diff {}", d[d.len() - 1]);
            } else {
                assert!(mean <= 2.0 && bp99 <= 2.0, "mean {mean} block p99 {bp99}");
            }
        }
    }
}
