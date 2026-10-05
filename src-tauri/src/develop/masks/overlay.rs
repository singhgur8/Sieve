//! `render_mask_overlay`: the selected mask as a grayscale JPEG on the preview's grid.

use std::path::PathBuf;
use std::time::Instant;

use super::eval;
use super::{MaskCache, MaskGeometry, RangeGuide};
use crate::develop::{render_url, DevelopCache, RenderTicket, SourceImage};
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::{
    MaskOverlayOptions, MaskOverlayTarget, MaskShape, NormRect, ParametricAdjustments, RenderedMaskOverlay,
};
use crate::lut::LutLibrary;

/// JPEG quality of overlays.
pub const OVERLAY_QUALITY: u8 = 90;

/// Output size of a render of `region` (of the cropped, oriented frame `cw x ch` source px)
/// at most `max_edge` long, never upscaling (as `source::output_size`).
pub fn output_size(cw: u32, ch: u32, region: Option<NormRect>, max_edge: u32) -> (u32, u32) {
    let r = region.unwrap_or(NormRect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 });
    let (rw, rh) = (r.width.clamp(0.0, 1.0) * cw as f32, r.height.clamp(0.0, 1.0) * ch as f32);
    let long = rw.max(rh).max(1.0);
    let scale = (max_edge as f32 / long).min(1.0);
    (((rw * scale).round() as u32).max(1), ((rh * scale).round() as u32).max(1))
}

#[inline]
fn srgb8_to_linear(v: u8) -> f32 {
    let x = f32::from(v) / 255.0;
    if x <= 0.040_45 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
fn lab_f(t: f32) -> f32 {
    if t > 0.008_856 {
        t.cbrt()
    } else {
        7.787 * t + 16.0 / 116.0
    }
}

/// CIE Lab (D65) of an sRGB colour given as linear components.
pub fn linear_srgb_to_lab(r: f32, g: f32, b: f32) -> [f32; 3] {
    let x = (0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b) / 0.950_47;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b;
    let z = (0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b) / 1.088_83;
    let (fx, fy, fz) = (lab_f(x), lab_f(y), lab_f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// Lab guide on a `w x h` grid from an interleaved sRGB8 image. Resampled in linear light
/// when the sizes differ: area average when shrinking, bilinear when enlarging (a nearest
/// resample would turn range masks into blocks / aliased stairs).
pub fn guide_from_srgb8(rgb: &[u8], sw: u32, sh: u32, w: u32, h: u32) -> Vec<[f32; 3]> {
    use rayon::prelude::*;
    let lut: Vec<f32> = (0..=255u8).map(srgb8_to_linear).collect();
    let (sw, sh, w, h) = (sw as usize, sh as usize, w as usize, h as usize);
    let mut out = vec![[0.0f32; 3]; w * h];
    if sw == 0 || sh == 0 || rgb.len() < sw * sh * 3 {
        return out;
    }
    let px = |x: usize, y: usize| {
        let i = (y * sw + x) * 3;
        [lut[rgb[i] as usize], lut[rgb[i + 1] as usize], lut[rgb[i + 2] as usize]]
    };
    // Source footprint of output column / row `o` (source px, fractional).
    let (kx, ky) = (sw as f32 / w.max(1) as f32, sh as f32 / h.max(1) as f32);
    out.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            let c = if sw == w && sh == h {
                px(x, y)
            } else if kx > 1.0 || ky > 1.0 {
                // Area average over the footprint (integer bounds, at least one pixel).
                let x0 = ((x as f32 * kx) as usize).min(sw - 1);
                let x1 = (((x + 1) as f32 * kx).ceil() as usize).clamp(x0 + 1, sw);
                let y0 = ((y as f32 * ky) as usize).min(sh - 1);
                let y1 = (((y + 1) as f32 * ky).ceil() as usize).clamp(y0 + 1, sh);
                let mut acc = [0.0f32; 3];
                for yy in y0..y1 {
                    for xx in x0..x1 {
                        let p = px(xx, yy);
                        for k in 0..3 {
                            acc[k] += p[k];
                        }
                    }
                }
                let n = ((x1 - x0) * (y1 - y0)) as f32;
                acc.map(|a| a / n)
            } else {
                // Bilinear at the output pixel centre.
                let fx = ((x as f32 + 0.5) * kx - 0.5).clamp(0.0, (sw - 1) as f32);
                let fy = ((y as f32 + 0.5) * ky - 0.5).clamp(0.0, (sh - 1) as f32);
                let (x0, y0) = (fx as usize, fy as usize);
                let (x1, y1) = ((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1));
                let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
                let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
                [0, 1, 2].map(|k| (a[k] * (1.0 - tx) + b[k] * tx) * (1.0 - ty) + (c[k] * (1.0 - tx) + d[k] * tx) * ty)
            };
            *v = linear_srgb_to_lab(c[0], c[1], c[2]);
        }
    });
    out
}

pub fn render_overlay(
    develop: &DevelopCache,
    mattes: &MaskCache,
    ticket: RenderTicket,
    src: &SourceImage,
    adjustments: &ParametricAdjustments,
    target: &MaskOverlayTarget,
    options: &MaskOverlayOptions,
) -> AppResult<Option<RenderedMaskOverlay>> {
    let started = Instant::now();
    if !develop.is_current(ticket) {
        return Ok(None);
    }
    let group = adjustments
        .masks
        .iter()
        .find(|g| g.id == target.group_id)
        .ok_or_else(|| AppError::invalid("unknown mask group"))?;
    let component = match &target.component_id {
        Some(id) => Some(
            group.components.iter().find(|c| &c.id == id).ok_or_else(|| AppError::invalid("unknown mask component"))?,
        ),
        None => None,
    };
    let info = develop.info(src)?;
    let o = src.orientation();
    let unorient = |w: u32, h: u32| if o >= 5 { (h, w) } else { (w, h) };
    let (sw, sh) = unorient(info.source_width, info.source_height);
    let (fw, fh) = unorient(info.full_width, info.full_height);
    let geo = crate::develop::transform::Geometry::of(adjustments, fw, fh);
    let (cw, ch, _) = eval::crop_frame(&geo.crop, sw, sh, o);
    let (width, height) = output_size(cw, ch, options.region, options.max_edge);
    let geom = MaskGeometry::of(&geo, fw, fh, o, options.region, width, height);
    let needs_guide = |s: &MaskShape| match s {
        MaskShape::Luminance(_) | MaskShape::Color(_) => true,
        MaskShape::Brush(b) => b.strokes.iter().any(|s| s.auto_mask),
        _ => false,
    };
    let wants_guide = match component {
        Some(c) => needs_guide(&c.shape),
        None => group.components.iter().filter(|c| c.active).any(|c| needs_guide(&c.shape)),
    };
    let lab = if wants_guide {
        // The image after the global settings (no masks, no LUT), on the overlay's grid.
        let mut adj = adjustments.clone();
        adj.masks.clear();
        adj.lut = None;
        let px = develop.render_image(src, &adj, options.region, options.max_edge, &LutLibrary::new(PathBuf::new()))?;
        Some(guide_from_srgb8(&px.image.rgb, px.image.width, px.image.height, width, height))
    } else {
        None
    };
    let guide = lab.as_deref().map(|lab| RangeGuide { width, height, lab });
    if !develop.is_current(ticket) {
        return Ok(None);
    }
    let plane = match component {
        Some(c) => eval::evaluate_component(c, &geom, src.id, mattes, guide.as_ref()),
        None => {
            let (mut a, mut b) = (0, 0);
            eval::group_mask(group, &geom, src.id, mattes, guide.as_ref(), &mut a, &mut b)
        }
    };
    let n = width as usize * height as usize;
    let bytes: Vec<u8> = match &plane {
        Some(p) => p.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect(),
        None => vec![0u8; n],
    };
    let coverage = plane
        .as_ref()
        .map_or(0.0, |p| (p.iter().map(|v| f64::from(v.clamp(0.0, 1.0))).sum::<f64>() / n.max(1) as f64) as f32);
    drop(plane);
    if !develop.is_current(ticket) {
        return Ok(None);
    }
    let jpeg = crate::raw::turbo::encode_gray(&bytes, width, height, OVERLAY_QUALITY).map_err(AppError::internal)?;
    if !develop.is_current(ticket) {
        return Ok(None);
    }
    develop.store_encoded((ticket.image_id, ticket.slot), ticket.seq, jpeg);
    Ok(Some(RenderedMaskOverlay {
        image_id: ticket.image_id,
        seq: ticket.seq,
        url: render_url(ticket.image_id, ticket.slot, ticket.seq),
        width,
        height,
        coverage,
        render_ms: started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32,
    }))
}
