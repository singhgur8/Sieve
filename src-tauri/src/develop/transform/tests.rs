use super::*;
use crate::ipc::types::UprightSolution;

const ASPECT: f64 = 1.5;

fn settings() -> TransformSettings {
    TransformSettings::default()
}

/// Lightroom's `UprightTransform_1` (Auto) and `_3` (Level) of a 6240 x 4160 Fuji frame
/// from the user's sidecars, whose `CropConstrainToWarp = 1` crop is below.
const LR_AUTO: &str = "0.966440195,-0.033575436,0.033575436,0.075437230,0.964770064,-0.020098923,0.000009403,\
                       0.000018298,0.999990597";
const LR_LEVEL: &str = "1.075273612,-0.037365889,-0.018953861,0.084073248,1.075273612,-0.079673430,0.000000000,\
                        0.000000000,1.000000000";
const LR_CROP: CropSettings =
    CropSettings { enabled: true, top: 0.049755, left: 0.031145, bottom: 0.947094, right: 0.928483, angle: 0.0 };

fn lr_solution(raw: &str, mode: UprightMode) -> UprightSolution {
    let m = from_lightroom(&parse_matrix(raw).unwrap()).unwrap();
    UprightSolution { mode, matrix: m.to_vec(), rotation_deg: rotation_deg(&m), crs: Vec::new() }
}

#[test]
fn neutral_settings_have_no_warp() {
    assert!(homography(&settings(), ASPECT).is_none());
    let adj = ParametricAdjustments::default();
    assert_eq!(Geometry::of(&adj, 6000, 4000), Geometry { crop: adj.crop, warp: None });
    // A solution for another mode than the current one is not rendered.
    let t = TransformSettings {
        upright: UprightMode::Vertical,
        solution: Some(lr_solution(LR_LEVEL, UprightMode::Level)),
        ..settings()
    };
    assert!(homography(&t, ASPECT).is_none());
}

/// A rectangular facade photographed by a camera pitched up by `alpha` (35 mm-equivalent
/// lens, centred): its vertical edges converge towards the top. Vertical = -alpha / 30 * 100
/// maps them back to parallel vertical lines.
#[test]
fn keystone_maps_converging_rectangle_to_parallel_lines() {
    let alpha = 12f64.to_radians();
    let f = 35.0 / 36.0 * ASPECT;
    // Normalized source point of a world point (X, Y, Z) seen by a camera pitched up.
    let project = |x: f64, y: f64, z: f64| {
        let (s, c) = alpha.sin_cos();
        // Camera coordinates (y down): rotate the world by the pitch.
        let (yc, zc) = (c * y + s * z, -s * y + c * z);
        let (u, v) = (f * x / zc, f * yc / zc);
        (u / ASPECT + 0.5, v + 0.5)
    };
    // Facade corners (y up in the world is -y here), centred in view after the pitch.
    let z = 4.0;
    let y_mid = -z * alpha.tan();
    let corners = [(-0.8, y_mid - 0.9), (0.8, y_mid - 0.9), (0.8, y_mid + 0.9), (-0.8, y_mid + 0.9)]
        .map(|(x, y)| project(x, y, z));
    let (tl, tr, br, bl) = (corners[0], corners[1], corners[2], corners[3]);
    assert!(tr.0 - tl.0 < br.0 - bl.0 - 0.02, "source verticals converge upwards: {corners:?}");

    let t = TransformSettings { vertical: -(alpha.to_degrees() / MAX_KEYSTONE_DEG * 100.0) as f32, ..settings() };
    let to_src = homography(&t, ASPECT).unwrap();
    let to_corr = invert(&to_src).unwrap();
    let c = corners.map(|(x, y)| apply(&to_corr, x, y).unwrap());
    // Left and right edges vertical, i.e. parallel.
    assert!((c[0].0 - c[3].0).abs() < 2e-3, "left edge vertical: {c:?}");
    assert!((c[1].0 - c[2].0).abs() < 2e-3, "right edge vertical: {c:?}");
    // Horizontal edges stay horizontal.
    assert!((c[0].1 - c[1].1).abs() < 1e-9 && (c[2].1 - c[3].1).abs() < 1e-9);
    // The frame centre stays put.
    let (cx, cy) = apply(&to_src, 0.5, 0.5).unwrap();
    assert!((cx - 0.5).abs() < 1e-9 && (cy - 0.5).abs() < 1e-9);

    // Horizontal (yaw) behaves the same way about the vertical axis.
    let th = TransformSettings { horizontal: 40.0, ..settings() };
    let m = invert(&homography(&th, ASPECT).unwrap()).unwrap();
    let left = apply(&m, 0.1, 0.9).unwrap().1 - apply(&m, 0.1, 0.1).unwrap().1;
    let right = apply(&m, 0.9, 0.9).unwrap().1 - apply(&m, 0.9, 0.1).unwrap().1;
    assert!(right > left, "positive Horizontal enlarges the right side ({left} vs {right})");
}

#[test]
fn rotate_aspect_scale_offset() {
    // Rotate +5: the image turns clockwise, i.e. a point right of the centre moves down.
    let m = invert(&homography(&TransformSettings { rotate: 5.0, ..settings() }, ASPECT).unwrap()).unwrap();
    let (_, y) = apply(&m, 0.9, 0.5).unwrap();
    assert!(y > 0.5);
    assert!(
        (rotation_deg(&homography(&TransformSettings { rotate: 5.0, ..settings() }, ASPECT).unwrap()) + 5.0).abs()
            < 1e-4
    );
    // Scale 50: the image shrinks about the centre.
    let m = invert(&homography(&TransformSettings { scale: 50.0, ..settings() }, ASPECT).unwrap()).unwrap();
    let (x, y) = apply(&m, 1.0, 1.0).unwrap();
    assert!((x - 0.75).abs() < 1e-9 && (y - 0.75).abs() < 1e-9);
    // Aspect +100: wider, area kept.
    let m = invert(&homography(&TransformSettings { aspect: 100.0, ..settings() }, ASPECT).unwrap()).unwrap();
    let (x, y) = apply(&m, 1.0, 1.0).unwrap();
    assert!(((x - 0.5) * (y - 0.5) - 0.25).abs() < 1e-9 && x > 1.0);
    // Offsets: +X right, +Y up, a quarter frame at 100.
    let m = invert(&homography(&TransformSettings { offset_x: 100.0, offset_y: 100.0, ..settings() }, ASPECT).unwrap())
        .unwrap();
    let (x, y) = apply(&m, 0.5, 0.5).unwrap();
    assert!((x - 0.75).abs() < 1e-9 && (y - 0.25).abs() < 1e-9);
}

#[test]
fn lightroom_matrices_are_source_to_corrected() {
    // Lightroom's constrained crop touches the source border on every side through the
    // inverse of its matrix (and would leave the source through the matrix itself).
    let t = TransformSettings {
        upright: UprightMode::Auto,
        solution: Some(lr_solution(LR_AUTO, UprightMode::Auto)),
        constrain_crop: true,
        ..settings()
    };
    let to_src = homography(&t, ASPECT).unwrap();
    let corners = crop_corners(&LR_CROP, ASPECT).map(|(x, y)| apply(&to_src, x, y).unwrap());
    let mut touching = 0;
    for (x, y) in corners {
        assert!((-1e-3..=1.001).contains(&x) && (-1e-3..=1.001).contains(&y), "{corners:?}");
        touching += usize::from(x.min(y).min(1.0 - x).min(1.0 - y) < 2e-3);
    }
    assert!(touching >= 3, "the largest frame touches the border: {corners:?}");
    // The constrained crop already fits: unchanged.
    assert_eq!(constrain_crop(&LR_CROP, Some(&to_src), ASPECT), LR_CROP);
    // From the default (whole frame) crop: Lightroom's size (largest 3:2 frame) within 0.5%.
    let c = constrain_crop(&CropSettings::default(), Some(&to_src), ASPECT);
    let lr = f64::from(LR_CROP.right - LR_CROP.left);
    assert!(c.enabled && (f64::from(c.right - c.left) / lr - 1.0).abs() < 5e-3, "{c:?}");
    assert!((f64::from(c.bottom - c.top) / f64::from(LR_CROP.bottom - LR_CROP.top) - 1.0).abs() < 5e-3);
    // Level: ~3 degrees, the image turned clockwise (negative = clockwise).
    let level = lr_solution(LR_LEVEL, UprightMode::Level);
    assert!((level.rotation_deg + 3.0).abs() < 0.1, "{}", level.rotation_deg);
}

#[test]
fn lightroom_solution_from_crs() {
    let crs = vec![
        CrsProperty { name: "UprightTransform_1".into(), value: LR_AUTO.into() },
        CrsProperty { name: "UprightTransform_3".into(), value: LR_LEVEL.into() },
    ];
    let s = lightroom_solution(&crs, UprightMode::Level).unwrap();
    assert_eq!(s.mode, UprightMode::Level);
    assert_eq!(s.crs, crs);
    assert!(lightroom_solution(&crs, UprightMode::Vertical).is_none());
    assert!(lightroom_solution(&crs, UprightMode::Off).is_none());
}

/// Rotating the image by theta with Constrain Crop keeps the largest frame of the photo's
/// aspect inside the rotated image: area within 0.5% of 1 / (cos + aspect sin)^2.
#[test]
fn constrain_keeps_max_inscribed_rect() {
    for deg in [1.0f32, 3.5, 7.0, 10.0, -6.0] {
        let t = TransformSettings { rotate: deg, constrain_crop: true, ..settings() };
        let adj = ParametricAdjustments { transform: t, ..Default::default() };
        let g = Geometry::of(&adj, 6000, 4000);
        let (w, h) = (f64::from(g.crop.right - g.crop.left), f64::from(g.crop.bottom - g.crop.top));
        let th = f64::from(deg).abs().to_radians();
        let s = 1.0 / (th.cos() + ASPECT * th.sin());
        assert!(((w * h) / (s * s) - 1.0).abs() < 5e-3, "{deg}: {w} x {h} vs {s}");
        assert!((w / h - 1.0).abs() < 1e-3, "keeps the aspect");
        // No empty pixels: every corner maps inside the source.
        for (x, y) in crop_corners(&g.crop, ASPECT) {
            let (sx, sy) = g.to_source(x, y).unwrap();
            assert!((-1e-6..=1.0 + 1e-6).contains(&sx) && (-1e-6..=1.0 + 1e-6).contains(&sy));
        }
    }
    // A user crop already inside is left alone; one leaving the image shrinks in place.
    let t = TransformSettings { rotate: 4.0, constrain_crop: true, ..settings() };
    let inner = CropSettings { enabled: true, left: 0.3, top: 0.3, right: 0.6, bottom: 0.6, angle: 0.0 };
    let adj = ParametricAdjustments { transform: t.clone(), crop: inner, ..Default::default() };
    assert_eq!(Geometry::of(&adj, 6000, 4000).crop, inner);
    let corner = CropSettings { enabled: true, left: 0.0, top: 0.0, right: 0.4, bottom: 0.4, angle: 0.0 };
    let adj = ParametricAdjustments { transform: t, crop: corner, ..Default::default() };
    let c = Geometry::of(&adj, 6000, 4000).crop;
    assert!(c.right - c.left < 0.4 && c.left > 0.0 && c.top > 0.0 && c.right < 0.45, "{c:?}");
}

/// v19.3 `get_transform_bounds`: no warp -> no quad / constrained crop; a crop-tool render
/// (crop disabled, Constrain Crop off) keeps the whole warped frame (R1-3 step 1).
#[test]
fn bounds_identity_is_null() {
    let adj = ParametricAdjustments::default();
    assert_eq!(bounds(&adj, 6000, 4000, 1), TransformBounds { valid_quad: None, constrained_crop: None });
    let t = TransformSettings { vertical: -40.0, constrain_crop: false, ..settings() };
    let adj = ParametricAdjustments { transform: t, ..Default::default() };
    let g = Geometry::of(&adj, 6000, 4000);
    assert!(g.warp.is_some());
    assert!(!g.crop.enabled, "crop tool frame is the full warped frame: {:?}", g.crop);
}

/// Inside test for a clockwise (y down) convex quad.
fn inside_quad(q: &[(f64, f64)], p: (f64, f64)) -> bool {
    (0..q.len()).all(|i| {
        let (a, b) = (q[i], q[(i + 1) % q.len()]);
        // Tolerance: the crop is stored as f32.
        (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0) >= -1e-6
    })
}

/// Keystone: the quad is the frame corners forwarded through the slider warp, clockwise;
/// widening the top (Vertical < 0) pulls the bottom corners inwards.
#[test]
fn bounds_keystone_quad_forwards_frame_corners() {
    let t = TransformSettings { vertical: -50.0, ..settings() };
    let adj = ParametricAdjustments { transform: t.clone(), ..Default::default() };
    let q = bounds(&adj, 6000, 4000, 1).valid_quad.expect("warp has an outline");
    let fwd = sliders_forward(&t, ASPECT);
    let expected: Vec<(f64, f64)> =
        [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].iter().map(|&(x, y)| apply(&fwd, x, y).unwrap()).collect();
    assert_eq!(q.len(), 4);
    for (a, b) in q.iter().zip(&expected) {
        assert!((a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9, "{q:?} vs {expected:?}");
    }
    assert!(q[3].0 > 0.01 && q[2].0 < 0.99, "bottom corners pulled in: {q:?}");
    assert!(inside_quad(&q, (0.5, 0.5)), "clockwise: {q:?}");
    // Orientation 6 (90 CW for display): the same outline, displayed: un-oriented (x, y) ->
    // display (1 - y, x).
    let q6 = bounds(&adj, 6000, 4000, 6).valid_quad.unwrap();
    assert!(inside_quad(&q6, (0.5, 0.5)), "clockwise after orientation: {q6:?}");
    for p in &expected {
        let d = (1.0 - p.1, p.0);
        assert!(q6.iter().any(|r| (r.0 - d.0).abs() < 1e-9 && (r.1 - d.1).abs() < 1e-9), "{d:?} in {q6:?}");
    }
}

/// The constrained crop lies inside the quad (and the frame), for a disabled crop, a user
/// crop leaving the warped image, and a straightened crop.
#[test]
fn bounds_constrained_crop_inside_quad() {
    let crops = [
        CropSettings::default(),
        CropSettings { enabled: true, left: 0.0, top: 0.3, right: 0.5, bottom: 1.0, angle: 0.0 },
        CropSettings { enabled: true, left: 0.05, top: 0.05, right: 0.95, bottom: 0.95, angle: 3.0 },
    ];
    let transforms = [
        TransformSettings { vertical: -50.0, ..settings() },
        TransformSettings { horizontal: 30.0, rotate: 4.0, ..settings() },
        TransformSettings {
            upright: UprightMode::Level,
            solution: Some(lr_solution(LR_LEVEL, UprightMode::Level)),
            ..settings()
        },
    ];
    for t in &transforms {
        for crop in &crops {
            let adj = ParametricAdjustments { transform: t.clone(), crop: *crop, ..Default::default() };
            let b = bounds(&adj, 6000, 4000, 1);
            let (q, c) = (b.valid_quad.unwrap(), b.constrained_crop.unwrap());
            // Unchanged (possibly disabled) when the frame already lies inside the warped image.
            assert!(c.enabled || c == *crop);
            for p in crop_corners(&c, ASPECT) {
                assert!(inside_quad(&q, p), "{p:?} outside {q:?} ({t:?}, {crop:?})");
                assert!((-1e-6..=1.0 + 1e-6).contains(&p.0) && (-1e-6..=1.0 + 1e-6).contains(&p.1));
            }
            // Same as what a render with Constrain Crop on uses.
            let on = ParametricAdjustments {
                transform: TransformSettings { constrain_crop: true, ..t.clone() },
                ..adj.clone()
            };
            assert_eq!(Geometry::of(&on, 6000, 4000).crop, c);
        }
    }
}
