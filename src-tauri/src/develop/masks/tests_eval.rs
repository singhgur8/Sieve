//! Evaluation, geometry, planes and cache tests (synthetic), plus the ignored real-data check
//! on the user's Lightroom sidecars.

use super::*;
use crate::ipc::types::{
    orient_point, AiTarget, BrushMask, BrushStroke, ColorRange, ColorSample, DevelopWarningCode, LinearMask,
    LocalAdjustments, LocalColor, LuminanceRange, RadialMask, UnsupportedMask,
};

fn id(n: u32) -> String {
    format!("{n:032X}")
}

fn comp(n: u32, shape: MaskShape) -> MaskComponent {
    MaskComponent {
        id: id(100 + n),
        name: String::new(),
        active: true,
        mode: MaskBlendMode::Add,
        inverted: false,
        opacity: 1.0,
        shape,
    }
}

fn group(n: u32, components: Vec<MaskComponent>) -> MaskGroup {
    MaskGroup {
        id: id(n),
        name: String::new(),
        active: true,
        amount: 1.0,
        adjustments: LocalAdjustments::default(),
        components,
    }
}

/// Un-oriented `w x h` sensor rendered 1:1 (orientation 1, no crop).
fn geom(w: u32, h: u32) -> MaskGeometry {
    MaskGeometry {
        sensor_width: w,
        sensor_height: h,
        orientation: 1,
        crop: CropSettings::default(),
        region: None,
        width: w,
        height: h,
    }
}

fn at(p: &[f32], w: u32, x: u32, y: u32) -> f32 {
    p[(y * w + x) as usize]
}

fn close(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

fn pt(x: f32, y: f32) -> NormPoint {
    NormPoint { x, y }
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

#[test]
fn geometry_maps_pixel_centres_through_orientation() {
    let g = MaskGeometry { width: 100, height: 50, ..geom(7008, 3504) };
    let p = g.sensor_point(0.0, 0.0);
    assert!(close(p.x, 0.005, 1e-6) && close(p.y, 0.01, 1e-6), "{p:?}");
    assert!(close(g.sensor_width_px(), 100.0, 1e-3));
    // Orientation 8 (displayed = sensor rotated 90 deg counter-clockwise): the displayed
    // top-left corner is the sensor's top-right corner.
    let g8 = MaskGeometry { orientation: 8, width: 50, height: 100, ..geom(7008, 3504) };
    let p = g8.sensor_point(0.0, 0.0);
    assert!(close(p.x, 0.995, 1e-3) && close(p.y, 0.01, 1e-3), "{p:?}");
    for o in 1..=8u8 {
        let (w, h) = if o >= 5 { (50, 100) } else { (100, 50) };
        let g = MaskGeometry { orientation: o, width: w, height: h, ..geom(7008, 3504) };
        // Every output pixel's sensor point, oriented back, is the output position.
        for (px, py) in [(0.0, 0.0), (w as f32 - 1.0, 3.0), (10.0, h as f32 - 1.0)] {
            let s = g.sensor_point(px, py);
            let d = orient_point(s, o);
            assert!(close(d.x, (px + 0.5) / w as f32, 1e-5) && close(d.y, (py + 0.5) / h as f32, 1e-5), "o {o}");
        }
        assert!(close(g.sensor_width_px(), 100.0, 1e-3), "o {o}: {}", g.sensor_width_px());
    }
}

#[test]
fn geometry_applies_crop_region_and_straighten() {
    let crop = CropSettings { enabled: true, left: 0.1, top: 0.2, right: 0.6, bottom: 0.7, angle: 0.0 };
    let (cw, ch, _) = crop_frame(&crop, 6000, 4000, 1);
    assert_eq!((cw, ch), (3000, 2000));
    let g = MaskGeometry { crop, width: 300, height: 200, ..geom(6000, 4000) };
    let p = g.sensor_point(-0.5, -0.5);
    assert!(close(p.x, 0.1, 1e-6) && close(p.y, 0.2, 1e-6));
    let p = g.sensor_point(299.5, 199.5);
    assert!(close(p.x, 0.6, 1e-6) && close(p.y, 0.7, 1e-6));
    // Brush radius scale: 300 output px for 3000 sensor px.
    assert!(close(g.sensor_width_px(), 600.0, 1e-2));
    // Region = right half of the cropped frame.
    let r =
        MaskGeometry { region: Some(NormRect { x: 0.5, y: 0.0, width: 0.5, height: 1.0 }), width: 150, ..g.clone() };
    let p = r.sensor_point(-0.5, -0.5);
    assert!(close(p.x, 0.35, 1e-6) && close(p.y, 0.2, 1e-6));
    // Straighten keeps the centre and the given corners.
    let s = MaskGeometry { crop: CropSettings { angle: 5.0, ..crop }, ..g.clone() };
    let (cw, ch, m) = crop_frame(&s.crop, 6000, 4000, 1);
    let (sn, cs) = 5f64.to_radians().sin_cos();
    assert_eq!((cw, ch), ((3000.0 * cs + 2000.0 * sn).round() as u32, (-3000.0 * sn + 2000.0 * cs).round() as u32));
    let c = |x: f64, y: f64| (m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5]);
    let (x0, y0) = c(0.0, 0.0);
    let (x1, y1) = c(1.0, 1.0);
    assert!((x0 - 0.1).abs() < 1e-3 && (y0 - 0.2).abs() < 1e-3 && (x1 - 0.6).abs() < 1e-3 && (y1 - 0.7).abs() < 1e-3);
    let (mx, my) = c(0.5, 0.5);
    assert!((mx - 0.35).abs() < 1e-6 && (my - 0.45).abs() < 1e-6);
    // Crop with orientation 6: the output is portrait.
    let (cw6, ch6, _) = crop_frame(&crop, 6000, 4000, 6);
    assert_eq!((cw6, ch6), (2000, 3000));
}

// ---------------------------------------------------------------------------
// Mattes
// ---------------------------------------------------------------------------

fn matte() -> AlphaMask {
    // 2 x 2 matte covering the right half of the frame.
    AlphaMask {
        width: 2,
        height: 2,
        bounds: NormRect { x: 0.5, y: 0.0, width: 0.5, height: 1.0 },
        data: vec![0, 255, 255, 255],
    }
}

#[test]
fn alpha_mask_samples_bilinearly_inside_its_bounds() {
    let m = matte();
    assert_eq!(m.sample(pt(0.25, 0.5)), 0.0, "outside bounds");
    assert_eq!(m.sample(pt(0.5 + 0.125, 0.25)), 0.0, "pixel centre (0, 0)");
    assert_eq!(m.sample(pt(0.875, 0.25)), 1.0);
    let mid = m.sample(pt(0.75, 0.5));
    assert!(close(mid, 0.75, 1e-6), "{mid}");
    assert!(close(m.coverage(), 0.5 * 0.75, 1e-6));
}

struct OneMatte(Arc<AlphaMask>);

impl MatteSource for OneMatte {
    fn matte(&self, _: ImageId, ai: &AiMask) -> Option<Arc<AlphaMask>> {
        (ai.target == AiTarget::Subject).then(|| self.0.clone())
    }
}

#[test]
fn ai_component_samples_the_matte_and_background_inverts_subject() {
    let g = geom(40, 20);
    let src = OneMatte(Arc::new(matte()));
    let ai = |target| comp(1, MaskShape::Ai(AiMask { target, reference_point: None, digest: None }));
    let p = evaluate_component(&ai(AiTarget::Subject), &g, 1, &src, None).unwrap();
    assert_eq!(at(&p, 40, 5, 10), 0.0);
    assert_eq!(at(&p, 40, 38, 18), 1.0);
    let b = evaluate_component(&ai(AiTarget::Background), &g, 1, &src, None).unwrap();
    assert_eq!(at(&b, 40, 5, 10), 1.0);
    assert_eq!(at(&b, 40, 38, 18), 0.0);
    assert!(evaluate_component(&ai(AiTarget::Sky), &g, 1, &src, None).is_none());
    assert!(evaluate_component(&ai(AiTarget::Subject), &g, 1, &NoMattes, None).is_none());
}

// ---------------------------------------------------------------------------
// Shapes
// ---------------------------------------------------------------------------

fn stroke(dabs: Vec<NormPoint>) -> BrushStroke {
    BrushStroke { radius: 0.1, flow: 1.0, feather: 0.0, density: 1.0, erase: false, auto_mask: false, dabs }
}

#[test]
fn brush_dabs_respect_radius_feather_flow_density_and_erase() {
    let g = geom(200, 100);
    let eval = |strokes: Vec<BrushStroke>| {
        evaluate_component(&comp(1, MaskShape::Brush(BrushMask { strokes })), &g, 1, &NoMattes, None).unwrap()
    };
    // Radius 0.1 of the width = 20 px around (100, 50).
    let hard = eval(vec![stroke(vec![pt(100.5 / 200.0, 50.5 / 100.0)])]);
    assert_eq!(at(&hard, 200, 100, 50), 1.0);
    assert_eq!(at(&hard, 200, 115, 50), 1.0);
    assert_eq!(at(&hard, 200, 125, 50), 0.0);
    assert_eq!(at(&hard, 200, 100, 80), 0.0);
    // Feather: soft falloff, 0.5 at the middle of the soft band.
    let soft = eval(vec![BrushStroke { feather: 1.0, ..stroke(vec![pt(100.5 / 200.0, 50.5 / 100.0)]) }]);
    assert_eq!(at(&soft, 200, 100, 50), 1.0);
    assert!(close(at(&soft, 200, 110, 50), 0.5, 0.01), "{}", at(&soft, 200, 110, 50));
    // Flow and density.
    let half = eval(vec![BrushStroke { flow: 0.5, ..stroke(vec![pt(0.5025, 0.505)]) }]);
    assert!(close(at(&half, 200, 100, 50), 0.5, 1e-5));
    let two = eval(vec![BrushStroke { flow: 0.5, ..stroke(vec![pt(0.5025, 0.505), pt(0.5025, 0.505)]) }]);
    assert!(close(at(&two, 200, 100, 50), 0.75, 1e-5), "flow builds up");
    let dens = eval(vec![BrushStroke { density: 0.4, ..stroke(vec![pt(0.5025, 0.505), pt(0.5025, 0.505)]) }]);
    assert!(close(at(&dens, 200, 100, 50), 0.4, 1e-5), "never above density");
    // Erase removes what was painted.
    let erased = eval(vec![
        stroke(vec![pt(0.4, 0.505), pt(0.6, 0.505)]),
        BrushStroke { erase: true, ..stroke(vec![pt(0.6, 0.505)]) },
    ]);
    assert_eq!(at(&erased, 200, 80, 50), 1.0);
    assert_eq!(at(&erased, 200, 120, 50), 0.0);
    // Resolution independence: same stroke at half resolution covers the same area.
    let g2 = MaskGeometry { width: 100, height: 50, ..g.clone() };
    let small = evaluate_component(
        &comp(1, MaskShape::Brush(BrushMask { strokes: vec![stroke(vec![pt(0.5, 0.5)])] })),
        &g2,
        1,
        &NoMattes,
        None,
    )
    .unwrap();
    assert_eq!(at(&small, 100, 57, 25), 1.0);
    assert_eq!(at(&small, 100, 62, 25), 0.0);
}

#[test]
fn linear_gradient_ramps_from_zero_to_full() {
    let g = geom(100, 100);
    let l = comp(1, MaskShape::Linear(LinearMask { zero: pt(0.0, 0.2), full: pt(0.0, 0.6) }));
    let p = evaluate_component(&l, &g, 1, &NoMattes, None).unwrap();
    assert_eq!(at(&p, 100, 50, 5), 0.0);
    assert!(close(at(&p, 100, 50, 40), 0.5 + 0.5 / 40.0, 1e-4), "{}", at(&p, 100, 50, 40));
    assert_eq!(at(&p, 100, 50, 90), 1.0);
    assert_eq!(at(&p, 100, 0, 90), at(&p, 100, 99, 90), "perpendicular to the direction");
    // Inverted + opacity.
    let inv = MaskComponent { inverted: true, opacity: 0.5, ..l };
    let q = evaluate_component(&inv, &g, 1, &NoMattes, None).unwrap();
    assert_eq!(at(&q, 100, 50, 5), 0.5);
    assert_eq!(at(&q, 100, 50, 90), 0.0);
}

fn radial(angle: f32, feather: f32) -> RadialMask {
    RadialMask {
        top: 0.25,
        left: 0.1,
        bottom: 0.75,
        right: 0.9,
        angle,
        midpoint: 50.0,
        roundness: 0.0,
        feather,
        flipped: false,
    }
}

#[test]
fn radial_gradient_is_an_ellipse_with_feather_angle_and_flip() {
    let g = geom(100, 100);
    let ev = |r: RadialMask| evaluate_component(&comp(1, MaskShape::Radial(r)), &g, 1, &NoMattes, None).unwrap();
    let hard = ev(radial(0.0, 0.0));
    assert_eq!(at(&hard, 100, 50, 50), 1.0);
    assert_eq!(at(&hard, 100, 85, 50), 1.0, "wide ellipse: inside along x");
    assert_eq!(at(&hard, 100, 50, 80), 0.0, "outside along y");
    let soft = ev(radial(0.0, 100.0));
    assert!(at(&soft, 100, 50, 50) > 0.99);
    let v = at(&soft, 100, 70, 50);
    assert!(v > 0.1 && v < 0.9, "{v}");
    // Rotated 90 degrees: tall ellipse.
    let rot = ev(radial(90.0, 0.0));
    assert_eq!(at(&rot, 100, 50, 85), 1.0);
    assert_eq!(at(&rot, 100, 80, 50), 0.0);
    let flip = ev(RadialMask { flipped: true, ..radial(0.0, 0.0) });
    assert_eq!(at(&flip, 100, 50, 50), 0.0);
    assert_eq!(at(&flip, 100, 50, 95), 1.0);
}

fn guide_lab(w: u32, h: u32, f: impl Fn(u32, u32) -> [f32; 3]) -> Vec<[f32; 3]> {
    (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| f(x, y)).collect()
}

#[test]
fn luminance_and_color_ranges_read_the_guide() {
    let g = geom(100, 10);
    // L* ramps 0..99 left to right; left half red, right half blue.
    let lab = guide_lab(100, 10, |x, _| if x < 50 { [x as f32, 60.0, 40.0] } else { [x as f32, 20.0, -60.0] });
    let guide = RangeGuide { width: 100, height: 10, lab: &lab };
    let lum = comp(
        1,
        MaskShape::Luminance(LuminanceRange {
            feather_low: 0.2,
            low: 0.4,
            high: 0.6,
            feather_high: 0.6,
            smoothness: 0.0,
        }),
    );
    let p = evaluate_component(&lum, &g, 1, &NoMattes, Some(&guide)).unwrap();
    assert_eq!(at(&p, 100, 10, 5), 0.0);
    assert!(close(at(&p, 100, 30, 5), 0.5, 1e-5));
    assert_eq!(at(&p, 100, 50, 5), 1.0);
    assert_eq!(at(&p, 100, 61, 5), 0.0, "hard upper edge");
    assert!(evaluate_component(&lum, &g, 1, &NoMattes, None).is_none(), "needs a guide");
    let col = comp(
        2,
        MaskShape::Color(ColorRange {
            samples: vec![ColorSample { point: pt(0.2, 0.5), area: None, lightroom_model: None }],
            amount: 50.0,
        }),
    );
    let c = evaluate_component(&col, &g, 1, &NoMattes, Some(&guide)).unwrap();
    assert!(at(&c, 100, 20, 5) > 0.99);
    assert!(at(&c, 100, 25, 5) > 0.9, "similar colour, close luminance");
    assert!(at(&c, 100, 80, 5) < 0.01, "other hue");
    let smooth = comp(
        1,
        MaskShape::Luminance(LuminanceRange {
            feather_low: 0.4,
            low: 0.4,
            high: 0.6,
            feather_high: 0.6,
            smoothness: 100.0,
        }),
    );
    let g2 = geom(400, 4);
    let lab2 = guide_lab(400, 4, |x, _| [x as f32 / 4.0, 0.0, 0.0]);
    let s = evaluate_component(&smooth, &g2, 1, &NoMattes, Some(&RangeGuide { width: 400, height: 4, lab: &lab2 }))
        .unwrap();
    assert!(at(&s, 400, 159, 2) > 0.0 && at(&s, 400, 159, 2) < 1.0, "edges are smoothed: {}", at(&s, 400, 159, 2));
}

/// Smoothness follows image edges (guided filter): a selection bounded by a strong edge stays
/// crisp at the edge while a hard luminance threshold inside a flat noisy area is softened;
/// no rectangular blocks (a box blur spreads a lone selected pixel into a square).
#[test]
fn range_smoothing_is_edge_aware_and_block_free() {
    let (w, h) = (200u32, 40u32);
    let g = geom(w, h);
    // Dark left half (L 20), bright right half (L 80), plus one bright pixel on the left.
    let lab =
        guide_lab(w, h, |x, y| if x >= 100 || (x == 40 && y == 20) { [80.0, 0.0, 0.0] } else { [20.0, 0.0, 0.0] });
    let guide = RangeGuide { width: w, height: h, lab: &lab };
    let lum = |smoothness: f32| {
        comp(
            1,
            MaskShape::Luminance(LuminanceRange {
                feather_low: 0.5,
                low: 0.5,
                high: 1.0,
                feather_high: 1.0,
                smoothness,
            }),
        )
    };
    let s = evaluate_component(&lum(100.0), &g, 1, &NoMattes, Some(&guide)).unwrap();
    assert!(
        at(&s, w, 98, 5) < 0.05 && at(&s, w, 101, 5) > 0.95,
        "edge kept: {} {}",
        at(&s, w, 98, 5),
        at(&s, w, 101, 5)
    );
    // The isolated pixel does not grow into a square plateau.
    let plateau = (35..46).flat_map(|x| (15..26).map(move |y| (x, y))).filter(|&(x, y)| at(&s, w, x, y) > 0.2).count();
    assert!(plateau <= 1, "{plateau} px around the lone pixel");
    // Hard edge (empty feather): a one-L* ramp outside the range, full weight inside.
    let r = LuminanceRange { feather_low: 0.5, low: 0.5, high: 1.0, feather_high: 1.0, smoothness: 0.0 };
    assert_eq!(luminance_weight(0.5, &r), 1.0);
    assert!(close(luminance_weight(0.495, &r), 0.5, 1e-4));
    assert_eq!(luminance_weight(0.489, &r), 0.0);
}

/// The overlay guide resamples in linear light: enlarging interpolates (no 2x2 duplicates),
/// shrinking averages.
#[test]
fn overlay_guide_resamples_smoothly() {
    // 4x1 ramp of sRGB values -> 8x1.
    let rgb: Vec<u8> = [0u8, 80, 160, 240].iter().flat_map(|&v| [v, v, v]).collect();
    let up = guide_from_srgb8(&rgb, 4, 1, 8, 1);
    let l: Vec<f32> = up.iter().map(|p| p[0]).collect();
    for k in 1..8 {
        assert!(l[k] >= l[k - 1], "monotone {l:?}");
    }
    let distinct = l.windows(2).filter(|p| (p[0] - p[1]).abs() > 1e-3).count();
    assert!(distinct >= 5, "interpolated, not duplicated: {l:?}");
    let down = guide_from_srgb8(&rgb, 4, 1, 2, 1);
    let mid = |a: u8, b: u8| {
        let lin = |v: u8| {
            let x = f32::from(v) / 255.0;
            if x <= 0.04045 {
                x / 12.92
            } else {
                ((x + 0.055) / 1.055).powf(2.4)
            }
        };
        linear_srgb_to_lab((lin(a) + lin(b)) / 2.0, (lin(a) + lin(b)) / 2.0, (lin(a) + lin(b)) / 2.0)[0]
    };
    assert!(close(down[0][0], mid(0, 80), 0.05) && close(down[1][0], mid(160, 240), 0.05), "{down:?}");
    // Same size: exact.
    assert_eq!(
        guide_from_srgb8(&rgb, 4, 1, 4, 1)[2],
        linear_srgb_to_lab(
            {
                let x: f32 = 160.0 / 255.0;
                ((x + 0.055) / 1.055).powf(2.4)
            },
            {
                let x: f32 = 160.0 / 255.0;
                ((x + 0.055) / 1.055).powf(2.4)
            },
            {
                let x: f32 = 160.0 / 255.0;
                ((x + 0.055) / 1.055).powf(2.4)
            },
        )
    );
}

// ---------------------------------------------------------------------------
// Combining and groups
// ---------------------------------------------------------------------------

#[test]
fn combine_modes() {
    let mut a = vec![0.2, 0.8, 1.0];
    combine(&mut a, &[0.5, 0.5, 0.0], MaskBlendMode::Add);
    assert_eq!(a, vec![0.5, 0.8, 1.0]);
    combine(&mut a, &[0.5, 1.0, 0.0], MaskBlendMode::Subtract);
    assert_eq!(a, vec![0.25, 0.0, 1.0]);
    combine(&mut a, &[0.5, 1.0, 0.5], MaskBlendMode::Intersect);
    assert_eq!(a, vec![0.125, 0.0, 0.5]);
}

#[test]
fn evaluate_combines_components_in_order_and_reports_warnings() {
    let g = geom(100, 100);
    let lin = comp(1, MaskShape::Linear(LinearMask { zero: pt(0.0, 0.0), full: pt(0.0, 1.0) }));
    let mut sub = comp(2, MaskShape::Radial(radial(0.0, 0.0)));
    sub.mode = MaskBlendMode::Subtract;
    let unsupported = comp(3, MaskShape::Unsupported(UnsupportedMask { what: "Mask/Depth".into() }));
    let missing = comp(4, MaskShape::Ai(AiMask { target: AiTarget::Sky, reference_point: None, digest: None }));
    let mut off = comp(5, MaskShape::Linear(LinearMask { zero: pt(0.0, 1.0), full: pt(0.0, 0.0) }));
    off.active = false;
    let mut grp = group(1, vec![unsupported, lin, sub, missing, off]);
    grp.amount = 1.5;
    let mut inactive =
        group(2, vec![comp(9, MaskShape::Linear(LinearMask { zero: pt(0.0, 0.0), full: pt(0.0, 1.0) }))]);
    inactive.active = false;
    let w = evaluate(&[grp, inactive, group(3, vec![])], &g, 1, &NoMattes, None);
    assert_eq!(w.groups.len(), 3);
    assert!(w.groups[1].is_none() && w.groups[2].is_none());
    let p = w.groups[0].as_ref().unwrap();
    // The linear ramp starts the mask (unsupported first component ignored), the ellipse is
    // cut out, amount 1.5 scales.
    assert_eq!(at(p, 100, 50, 50), 0.0);
    assert!(close(at(p, 100, 5, 95), 1.5 * 0.955, 1e-3), "{}", at(p, 100, 5, 95));
    let codes: Vec<_> = w.warnings.iter().map(|w| (w.code, w.detail.clone())).collect();
    assert_eq!(
        codes,
        vec![
            (DevelopWarningCode::MasksUnsupported, Some("1".into())),
            (DevelopWarningCode::AiMaskNeedsUpdate, Some("1".into()))
        ]
    );
}

#[test]
fn a_few_masks_evaluate_fast_at_preview_size() {
    let g = MaskGeometry { width: 2048, height: 1365, ..geom(7008, 4672) };
    let big = AlphaMask {
        width: 1920,
        height: 1280,
        bounds: NormRect { x: 0.1, y: 0.1, width: 0.8, height: 0.8 },
        data: (0..1920 * 1280).map(|i| (i % 251) as u8).collect(),
    };
    struct Any(Arc<AlphaMask>);
    impl MatteSource for Any {
        fn matte(&self, _: ImageId, _: &AiMask) -> Option<Arc<AlphaMask>> {
            Some(self.0.clone())
        }
    }
    let src = Any(Arc::new(big));
    let groups = vec![
        group(
            1,
            vec![comp(1, MaskShape::Ai(AiMask { target: AiTarget::Subject, reference_point: None, digest: None }))],
        ),
        group(2, vec![comp(2, MaskShape::Linear(LinearMask { zero: pt(0.0, 0.0), full: pt(0.0, 0.5) }))]),
        group(3, vec![comp(3, MaskShape::Radial(radial(20.0, 50.0)))]),
    ];
    let _ = evaluate(&groups, &g, 1, &src, None); // warm the pool

    // Best of 5: a single wall-clock sample is at the mercy of the other tests competing for
    // the CPU (and the rayon pool) in a full `cargo test` run.
    let mut ms = f64::INFINITY;
    for _ in 0..5 {
        let t = std::time::Instant::now();
        let w = evaluate(&groups, &g, 1, &src, None);
        ms = ms.min(t.elapsed().as_secs_f64() * 1000.0);
        assert!(w.groups.iter().all(Option::is_some));
    }
    eprintln!("3 masks at 2048 px: {ms:.1} ms (best of 5)");
    // Debug builds are ~10x slower than release (and share the CPU with the other tests);
    // the budget is ~10 ms in release.
    assert!(ms < if cfg!(debug_assertions) { 1500.0 } else { 20.0 }, "{ms} ms");
}

// ---------------------------------------------------------------------------
// Local planes and blends
// ---------------------------------------------------------------------------

#[test]
fn local_planes_sum_weighted_group_values() {
    let g = geom(10, 1);
    let lin = |n| comp(n, MaskShape::Linear(LinearMask { zero: pt(0.0, 0.0), full: pt(1.0, 0.0) }));
    let mut a = group(1, vec![lin(1)]);
    a.adjustments.exposure = 1.0;
    let mut b = group(2, vec![lin(2)]);
    b.adjustments.exposure = -0.5;
    b.adjustments.clarity = 40.0;
    let neutral = group(3, vec![lin(3)]);
    let masks = vec![a, b, neutral.clone()];
    let w = evaluate(&masks, &g, 1, &NoMattes, None);
    let local = LocalPlanes::build(&masks, &w).unwrap();
    let e = local.get(LocalParam::Exposure).unwrap();
    let c = local.get(LocalParam::Clarity).unwrap();
    let wt = w.groups[0].as_ref().unwrap();
    for i in 0..10 {
        assert!(close(e[i], wt[i] * 0.5, 1e-6));
        assert!(close(c[i], wt[i] * 40.0, 1e-5));
    }
    assert!(local.get(LocalParam::Temperature).is_none());
    assert!(local.blends.is_empty());
    // Neutral groups only: nothing to do.
    let only = vec![neutral];
    assert!(LocalPlanes::build(&only, &evaluate(&only, &g, 1, &NoMattes, None)).is_none());
}

#[test]
fn group_blends_apply_curves_and_tint_by_weight() {
    let g = geom(4, 1);
    let lin = comp(1, MaskShape::Linear(LinearMask { zero: pt(0.0, 0.0), full: pt(1.0, 0.0) }));
    let mut grp = group(1, vec![lin]);
    grp.adjustments.tone_curve.master = vec![[0.0, 0.0], [128.0, 200.0], [255.0, 255.0]];
    let masks = vec![grp.clone()];
    let w = evaluate(&masks, &g, 1, &NoMattes, None);
    let local = LocalPlanes::build(&masks, &w).unwrap();
    assert_eq!(local.blends.len(), 1);
    let mut rgb = vec![[0.2f32, 0.2, 0.2]; 4];
    apply_group_blends(&mut rgb, &local);
    // Weight grows left to right, so does the brightening.
    assert!(rgb[0][0] > 0.2 && rgb[3][0] > rgb[0][0], "{rgb:?}");
    assert!(close(rgb[3][0], rgb[3][1], 1e-6) && close(rgb[3][1], rgb[3][2], 1e-6), "neutral stays neutral");
    // Tint: warm colour pushes red above blue.
    let mut tint = grp;
    tint.adjustments.tone_curve = PointCurves::default();
    tint.adjustments.color = LocalColor { hue: 30.0, saturation: 80.0 };
    let masks = vec![tint];
    let local = LocalPlanes::build(&masks, &evaluate(&masks, &g, 1, &NoMattes, None)).unwrap();
    let mut rgb = vec![[0.2f32, 0.2, 0.2]; 4];
    apply_group_blends(&mut rgb, &local);
    assert!(rgb[3][0] > rgb[3][2]);
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

fn catalog() -> (tempfile::TempDir, PathBuf, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.sqlite");
    let conn = crate::db::open(&path).unwrap();
    conn.execute_batch(
        "INSERT INTO folders (id, path, added_at) VALUES (1, '/f', 0);
         INSERT INTO images (id, folder_id, path, file_name, format, camera_make, file_size, file_mtime_ms, imported_at)
         VALUES (1, 1, '/f/a.arw', 'a.arw', 'arw', 'sony', 1, 0, 0), (2, 1, '/f/b.arw', 'b.arw', 'arw', 'sony', 1, 0, 0);",
    )
    .unwrap();
    (dir, path, conn)
}

#[test]
fn cache_puts_resolves_loads_and_sweeps() {
    let (dir, path, conn) = catalog();
    let cache = MaskCache::new(MaskCacheConfig { catalog_path: path, cache_dir: dir.path().join("cache") });
    let lr = NewMatte {
        kind: "subject".into(),
        target: AiTarget::Subject,
        reference_point: Some(pt(0.1, 0.2)),
        origin: AiMaskOrigin::Lightroom,
        digest: Some("E71A59AFC894F4F898F72751E30113DA".into()),
        model_version: "lr:251659306".into(),
        input_digest: None,
    };
    let info = cache.put(&conn, 1, &lr, &matte()).unwrap();
    assert_eq!(info.digest, "E71A59AFC894F4F898F72751E30113DA");
    assert_eq!((info.width, info.height, info.origin), (2, 2, AiMaskOrigin::Lightroom));
    assert!(close(info.coverage, 0.375, 1e-6));
    assert!(cache.masks_dir().join("1/E71A59AFC894F4F898F72751E30113DA.png").exists());
    // Sieve mattes are keyed by the PNG's MD5 and resolved by kind (+ model version).
    let sieve = NewMatte {
        origin: AiMaskOrigin::Sieve,
        digest: None,
        model_version: "m@1".into(),
        input_digest: Some("F".into()),
        ..lr.clone()
    };
    let s1 = cache.put(&conn, 1, &sieve, &AlphaMask { data: vec![1, 2, 3, 4], ..matte() }).unwrap();
    assert!(crate::ipc::types::is_mask_id(&s1.digest));
    let subject = AiMask { target: AiTarget::Subject, reference_point: None, digest: None };
    assert_eq!(cache.resolve(&conn, 1, &subject, Some("m@1")).unwrap().unwrap().digest, s1.digest);
    assert_eq!(cache.resolve(&conn, 1, &subject, Some("m@2")).unwrap(), None);
    assert_eq!(cache.resolve(&conn, 2, &subject, None).unwrap(), None);
    let explicit = AiMask { digest: Some(info.digest.clone()), ..subject.clone() };
    assert_eq!(cache.resolve(&conn, 1, &explicit, None).unwrap().unwrap().origin, AiMaskOrigin::Lightroom);
    // Load: from a fresh cache (no LRU) through the PNG file, bounds included.
    let fresh = MaskCache::new(cache.config().clone());
    let loaded = fresh.load(1, &info.digest).unwrap().unwrap();
    assert_eq!(*loaded, matte());
    assert_eq!(fresh.load(1, "00000000000000000000000000000000").unwrap(), None);
    // MatteSource: explicit digest, then newest Sieve matte of the kind (own connection).
    assert_eq!(*fresh.matte(1, &explicit).unwrap(), matte());
    assert_eq!(fresh.matte(1, &subject).unwrap().data, vec![1, 2, 3, 4]);
    assert!(fresh.matte(2, &subject).is_none());
    // A newer model supersedes the unreferenced old Sieve matte; orphan files are removed.
    std::thread::sleep(std::time::Duration::from_millis(5));
    let s2 = cache
        .put(
            &conn,
            1,
            &NewMatte { model_version: "m@2".into(), ..sieve.clone() },
            &AlphaMask { data: vec![9, 9, 9, 9], ..matte() },
        )
        .unwrap();
    std::fs::create_dir_all(cache.masks_dir().join("7")).unwrap();
    std::fs::write(cache.masks_dir().join("7/ABC.png"), b"x").unwrap();
    let removed = cache.sweep(&conn).unwrap();
    assert_eq!(removed, 2, "old sieve matte + orphan");
    assert!(!cache.masks_dir().join(format!("1/{}.png", s1.digest)).exists());
    assert!(cache.masks_dir().join(format!("1/{}.png", s2.digest)).exists());
    assert!(cache.masks_dir().join("1/E71A59AFC894F4F898F72751E30113DA.png").exists());
    assert!(!cache.masks_dir().join("7").exists());
    // Deleting the image cascades the rows; the sweep then removes its files.
    conn.execute("DELETE FROM images WHERE id = 1", []).unwrap();
    assert_eq!(cache.sweep(&conn).unwrap(), 2);
    assert!(!cache.masks_dir().join("1").exists());
}

#[test]
fn png_round_trip_keeps_bounds_and_pixels() {
    let m = AlphaMask {
        width: 3,
        height: 2,
        bounds: NormRect { x: 0.123_456_78, y: 0.2, width: 0.3, height: 0.456 },
        data: vec![0, 1, 2, 250, 254, 255],
    };
    let bytes = cache::encode_png(&m).unwrap();
    assert_eq!(cache::decode_png(&bytes).unwrap(), m);
}

// ---------------------------------------------------------------------------
// Real data (read-only source; writes only under test-data/)
// ---------------------------------------------------------------------------

const REAL_DIR: &str = "/Users/gurjotsingh/Pictures/Jasmit Natalie Proposal";
/// Overlays and round-trip copies (gitignored `test-data/` of the main checkout).
const OUT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../test-data/mask-check");

fn sidecars() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = walkdir::WalkDir::new(REAL_DIR)
        .into_iter()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("xmp")))
        .map(|e| e.path().to_path_buf())
        .collect();
    out.sort();
    out
}

fn xmp_attr(text: &str, name: &str) -> Option<String> {
    let key = format!("{name}=\"");
    let i = text.find(&key)? + key.len();
    Some(text[i..i + text[i..].find('"')?].to_owned())
}

fn raw_for(xmp: &std::path::Path) -> Option<PathBuf> {
    ["ARW", "arw", "RAF", "raf", "CR3", "cr3"].iter().map(|e| xmp.with_extension(e)).find(|p| p.exists())
}

#[test]
#[ignore = "reads the user's Lightroom sidecars (macOS 14+, ImageIO JPEG XL)"]
fn real_lightroom_masks() {
    use crate::xmp::masks as xm;
    let out_dir = PathBuf::from(OUT_DIR);
    std::fs::create_dir_all(&out_dir).unwrap();
    let mut masked = Vec::new();
    let (mut decoded, mut failed, mut total_ms, mut groups_n) = (0, 0, 0.0f64, 0);
    for p in sidecars() {
        let text = std::fs::read_to_string(&p).unwrap();
        let Some(r) = xm::read(&text).unwrap() else { continue };
        if r.groups.is_empty() {
            continue;
        }
        crate::ipc::types::validate_masks(&r.groups).unwrap();
        groups_n += r.groups.len();
        for m in &r.mattes {
            let t = std::time::Instant::now();
            match xm::decode_matte(m) {
                Ok(a) => {
                    decoded += 1;
                    assert!(a.coverage() > 0.01, "{}: empty matte", p.display());
                    assert!(a.bounds.x >= -1e-3 && a.bounds.y >= -1e-3);
                    assert!(
                        a.bounds.x + a.bounds.width <= 1.001 && a.bounds.y + a.bounds.height <= 1.001,
                        "{:?}",
                        a.bounds
                    );
                }
                Err(e) => {
                    failed += 1;
                    eprintln!("{}: {e}", p.display());
                }
            }
            total_ms += t.elapsed().as_secs_f64() * 1000.0;
        }
        // Unchanged masks: byte-identical write.
        assert_eq!(xm::apply(&text, &r.groups).unwrap(), text, "{}", p.display());
        masked.push((p, text, r));
    }
    let n_mattes: usize = masked.iter().map(|m| m.2.mattes.len()).sum();
    eprintln!(
        "{} masked sidecars, {groups_n} groups, {n_mattes} mattes: {decoded} decoded, {failed} failed, {:.1} ms/matte",
        masked.len(),
        total_ms / n_mattes.max(1) as f64
    );
    assert_eq!(masked.len(), 50);
    assert_eq!((decoded, failed), (50, 0));

    // Overlays for 6 frames (orientation-8 frames first), on the embedded preview.
    let mut picks: Vec<&(PathBuf, String, xm::MasksRead)> =
        masked.iter().filter(|m| xmp_attr(&m.1, "tiff:Orientation").as_deref() == Some("8")).take(3).collect();
    picks.extend(
        masked.iter().filter(|m| xmp_attr(&m.1, "tiff:Orientation").as_deref() != Some("8")).take(6 - picks.len()),
    );
    for (p, text, r) in picks {
        let raw = raw_for(p).expect("RAW next to the sidecar");
        let format = crate::raw::format_from_extension(&raw).unwrap();
        let mut buf = Vec::new();
        let ex = crate::raw::extract(&raw, format, &mut buf).unwrap();
        let o = ex.meta.orientation.unwrap_or(1) as u8;
        let jpeg = match ex.preview.unwrap() {
            crate::raw::Preview::Embedded => buf.clone(),
            crate::raw::Preview::Libraw(crate::raw::libraw::Thumb::Jpeg(b)) => b,
            _ => panic!("no JPEG preview"),
        };
        let img = crate::raw::preview::decode_jpeg(&jpeg, 1024).unwrap();
        let img = crate::raw::preview::orient(img, u16::from(o));
        // Decoded mattes, served like the cache would.
        struct Mem(Vec<(String, Arc<AlphaMask>)>);
        impl MatteSource for Mem {
            fn matte(&self, _: ImageId, ai: &AiMask) -> Option<Arc<AlphaMask>> {
                let d = ai.digest.as_ref()?;
                self.0.iter().find(|(k, _)| k == d).map(|(_, m)| m.clone())
            }
        }
        let mem = Mem(r.mattes.iter().map(|m| (m.digest.clone(), Arc::new(xm::decode_matte(m).unwrap()))).collect());
        let (sw, sh) = (
            xmp_attr(text, "tiff:ImageWidth").and_then(|v| v.parse().ok()).unwrap_or(6000u32),
            xmp_attr(text, "tiff:ImageLength").and_then(|v| v.parse().ok()).unwrap_or(4000u32),
        );
        let g = MaskGeometry {
            sensor_width: sw,
            sensor_height: sh,
            orientation: o,
            crop: CropSettings::default(),
            region: None,
            width: img.width,
            height: img.height,
        };
        let t = std::time::Instant::now();
        let w = evaluate(&r.groups, &g, 1, &mem, None);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let plane = w.groups[0].as_ref().expect("group renders");
        let amount = r.groups[0].amount.max(1e-3);
        // Composite: photo, subject tinted red.
        let mut rgb = img.pixels.clone();
        for (i, v) in plane.iter().enumerate() {
            let a = (v / amount).clamp(0.0, 1.0) * 0.6;
            let px = &mut rgb[i * 3..i * 3 + 3];
            px[0] = (f32::from(px[0]) * (1.0 - a) + 255.0 * a) as u8;
            px[1] = (f32::from(px[1]) * (1.0 - a)) as u8;
            px[2] = (f32::from(px[2]) * (1.0 - a)) as u8;
        }
        let name = p.file_stem().unwrap().to_string_lossy().into_owned();
        let out = out_dir.join(format!("{name}_o{o}.jpg"));
        std::fs::write(&out, crate::raw::turbo::encode_rgb(&rgb, img.width, img.height, 85).unwrap()).unwrap();
        eprintln!("{} ({}x{}, orientation {o}): mask {ms:.1} ms -> {}", name, img.width, img.height, out.display());
    }

    // Round trip on a copy: change one group's local exposure.
    let (p, text, r) = &masked[0];
    let copy = out_dir.join(p.file_name().unwrap());
    std::fs::write(&copy, text).unwrap();
    let mut groups = r.groups.clone();
    groups[0].adjustments.exposure = 0.5;
    let written = xm::apply(text, &groups).unwrap();
    std::fs::write(&copy, &written).unwrap();
    let back = xm::read(&written).unwrap().unwrap();
    assert_eq!(back.groups, groups);
    assert_eq!(xmp_attr(&written, "crs:LocalExposure2012").as_deref(), Some("0.125"));
    // Everything else is preserved: every other top-level property, retouch and look tables.
    let before = crate::xmp::packet::top_level_properties(text).unwrap();
    let after = crate::xmp::packet::top_level_properties(&written).unwrap();
    assert_eq!(before.len(), after.len());
    for (b, a) in before.iter().zip(&after) {
        if b.1 != "MaskGroupBasedCorrections" {
            assert_eq!(b, a, "{}", b.1);
        }
    }
    eprintln!("round trip copy: {}", copy.display());
    // Retouch tables: sidecars with RetouchAreas keep every table when masks change.
    for (p, text, r) in masked.iter().filter(|m| m.1.contains("<crs:RetouchAreas>")).take(3) {
        let mut g = r.groups.clone();
        g[0].amount = 0.5;
        let w = xm::apply(text, &g).unwrap();
        let tables = |s: &str| s.matches("crs:Table_").count();
        assert_eq!(tables(&w), tables(text), "{}", p.display());
        g[0].components.clear();
        let w = xm::apply(text, &g).unwrap();
        assert_eq!(tables(&w), tables(text) - 1, "{}: only the matte table goes", p.display());
    }
}
