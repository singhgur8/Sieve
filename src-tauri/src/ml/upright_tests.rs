use super::*;
use crate::develop::parity::{crop_geometry, orientation_map};
use crate::ipc::types::CropSettings;

const W: u32 = 1024;
const H: u32 = 683;

/// Deterministic hash noise in -0.5..0.5.
fn hash_noise(x: u32, y: u32) -> f32 {
    let mut v = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77);
    v ^= v >> 15;
    v = v.wrapping_mul(0x2C1B_3C6D);
    v ^= v >> 12;
    (v & 0xFFFF) as f32 / 65535.0 - 0.5
}

/// RGB8 image from a supersampled (3x3) intensity function of centred pixel coordinates.
fn render(w: u32, h: u32, f: impl Fn(f64, f64) -> f32) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for sy in 0..3 {
                for sx in 0..3 {
                    let px = f64::from(x) + (f64::from(sx) + 0.5) / 3.0 - f64::from(w) / 2.0;
                    let py = f64::from(y) + (f64::from(sy) + 0.5) / 3.0 - f64::from(h) / 2.0;
                    s += f(px, py);
                }
            }
            let v = (s / 9.0 + 0.03 * hash_noise(x, y)).clamp(0.0, 1.0);
            let b = (v * 255.0).round() as u8;
            out.extend_from_slice(&[b, b, b]);
        }
    }
    out
}

/// Sea + sky with a horizon through the centre descending right by `deg` (y down).
fn horizon_image(deg: f64) -> Vec<u8> {
    let (s, c) = deg.to_radians().sin_cos();
    render(W, H, move |x, y| {
        // Signed distance below the horizon line through the centre.
        let d = -s * x + c * y;
        if d < 0.0 {
            0.8 - 0.1 * (y / f64::from(H)) as f32
        } else {
            0.35 + 0.02 * ((x * 0.05).sin() as f32)
        }
    })
}

fn level_rotation(rgb: &[u8]) -> Option<f32> {
    let out = solve_rgb8(rgb, W, H, 1, UprightMode::Level, &[], None);
    out.solution.map(|s| s.rotation_deg)
}

#[test]
fn level_recovers_tilted_horizon() {
    for deg in [3.7, -2.2, 0.8, -7.5] {
        let rgb = horizon_image(deg);
        let rot = level_rotation(&rgb).unwrap_or_else(|| panic!("no level solution for {deg}"));
        assert!((f64::from(rot) - deg).abs() <= 0.3, "horizon {deg}: rotation {rot}");
    }
}

#[test]
fn level_matrix_makes_horizon_horizontal() {
    let deg = 4.0f64;
    let rgb = horizon_image(deg);
    let out = solve_rgb8(&rgb, W, H, 1, UprightMode::Level, &[], None);
    let sol = out.solution.expect("solution");
    assert_eq!(sol.matrix.len(), 9);
    let inv = invert_matrix(&sol.matrix).unwrap();
    // Two horizon points (sensor normalized) map to the same corrected row.
    let (s, c) = deg.to_radians().sin_cos();
    let pt = |t: f64| ((f64::from(W) / 2.0 + c * t) / f64::from(W), (f64::from(H) / 2.0 + s * t) / f64::from(H));
    let (a, b) = (pt(-300.0), pt(300.0));
    let (ax, ay) = map_point(&inv, a.0, a.1);
    let (bx, by) = map_point(&inv, b.0, b.1);
    let ang = ((by - ay) * f64::from(H)).atan2((bx - ax) * f64::from(W)).to_degrees();
    assert!(ang.abs() < 0.3, "corrected horizon at {ang} deg");
    // The centre stays at the centre.
    let (cx, cy) = map_point(&sol.matrix, 0.5, 0.5);
    assert!((cx - 0.5).abs() < 1e-6 && (cy - 0.5).abs() < 1e-6);
}

/// Synthetic building: a facade at depth 12 seen by a camera rotated by `r` (world ->
/// camera), focal `f` px. Returns the image and the projection of a world point.
struct Building {
    r: M3,
    f: f64,
}

impl Building {
    fn project(&self, x: f64, y: f64) -> (f64, f64) {
        let c = apply(&self.r, [x, y, 12.0]);
        (self.f * c[0] / c[2], self.f * c[1] / c[2])
    }

    fn image(&self) -> Vec<u8> {
        let rt = transpose(&self.r);
        let f = self.f;
        render(W, H, move |px, py| {
            let ray = apply(&rt, [px / f, py / f, 1.0]);
            if ray[2] <= 0.0 {
                return 0.85;
            }
            let t = 12.0 / ray[2];
            let (x, y) = (t * ray[0], t * ray[1]);
            if !(-4.0..=4.0).contains(&x) || y < -9.0 {
                return 0.85;
            }
            if y > 6.0 {
                return 0.3;
            }
            let fx = (x + 4.0).rem_euclid(1.0);
            let fy = (y + 9.0).rem_euclid(1.5) / 1.5;
            if (0.25..0.75).contains(&fx) && (0.2..0.7).contains(&fy) {
                0.12
            } else {
                0.5
            }
        })
    }

    /// Facade verticals (x = const) and horizontals (y = const) as source pixel segments.
    fn verticals(&self) -> Vec<((f64, f64), (f64, f64))> {
        [-4.0, -2.25, -0.75, 0.25, 1.75, 4.0].iter().map(|&x| (self.project(x, -6.0), self.project(x, 3.0))).collect()
    }

    fn horizontals(&self) -> Vec<((f64, f64), (f64, f64))> {
        [-6.0, -3.0, 0.0, 3.0].iter().map(|&y| (self.project(-3.5, y), self.project(3.5, y))).collect()
    }
}

fn map_centered(g: &M3, p: (f64, f64)) -> (f64, f64) {
    let q = apply(g, [p.0, p.1, 1.0]);
    (q[0] / q[2], q[1] / q[2])
}

/// Angle from vertical (degrees) of a source segment after `g`.
fn from_vertical(g: &M3, s: ((f64, f64), (f64, f64))) -> f64 {
    let a = map_centered(g, s.0);
    let b = map_centered(g, s.1);
    (b.0 - a.0).atan2(b.1 - a.1).to_degrees()
}

fn from_horizontal(g: &M3, s: ((f64, f64), (f64, f64))) -> f64 {
    let a = map_centered(g, s.0);
    let b = map_centered(g, s.1);
    (b.1 - a.1).atan2(b.0 - a.0).to_degrees()
}

fn wrap90(a: f64) -> f64 {
    let mut a = a;
    while a > 90.0 {
        a -= 180.0;
    }
    while a <= -90.0 {
        a += 180.0;
    }
    a
}

fn oriented_solve(rgb: &[u8], mode: UprightMode, f35: f64) -> Result<OrientedSolve, NoSolution> {
    let frame = Frame::new(W, H, Some(f35));
    let set = LineSet { width: W, height: H, segments: detect_segments(&luminance(rgb, W, H), W, H) };
    solve(&set, mode, &[], &frame)
}

fn focal(f35: f64) -> f64 {
    Frame::new(W, H, Some(f35)).focal_px
}

#[test]
fn vertical_corrects_converging_verticals() {
    let f35 = 24.0;
    let b = Building { r: mul(&rot_z(0.02), &rot_x(0.26)), f: focal(f35) };
    let rgb = b.image();
    let id: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let before: Vec<f64> = b.verticals().into_iter().map(|s| wrap90(from_vertical(&id, s))).collect();
    let spread_before =
        before.iter().cloned().fold(f64::MIN, f64::max) - before.iter().cloned().fold(f64::MAX, f64::min);
    assert!(spread_before > 8.0, "test building should converge: {before:?}");
    let s = oriented_solve(&rgb, UprightMode::Vertical, f35).expect("vertical solve");
    for seg in b.verticals() {
        let a = wrap90(from_vertical(&s.source_to_corrected, seg));
        assert!(a.abs() < 0.5, "vertical off by {a} deg");
    }
    // A focal length guess that is off still makes the verticals parallel.
    let s = oriented_solve(&rgb, UprightMode::Vertical, 35.0).expect("vertical solve, other focal");
    for seg in b.verticals() {
        let a = wrap90(from_vertical(&s.source_to_corrected, seg));
        assert!(a.abs() < 0.5, "vertical (f 35) off by {a} deg");
    }
}

#[test]
fn full_corrects_verticals_and_horizontals() {
    let f35 = 24.0;
    let b = Building { r: mul(&rot_x(0.2), &rot_y(0.3)), f: focal(f35) };
    let rgb = b.image();
    let s = oriented_solve(&rgb, UprightMode::Full, f35).expect("full solve");
    for seg in b.verticals() {
        let a = wrap90(from_vertical(&s.source_to_corrected, seg));
        assert!(a.abs() < 0.5, "vertical off by {a} deg");
    }
    for seg in b.horizontals() {
        let a = wrap90(from_horizontal(&s.source_to_corrected, seg));
        assert!(a.abs() < 0.5, "horizontal off by {a} deg");
    }
}

#[test]
fn auto_reduces_convergence_without_overcorrecting() {
    let f35 = 24.0;
    let b = Building { r: mul(&rot_z(0.03), &rot_x(0.3)), f: focal(f35) };
    let rgb = b.image();
    let id: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let spread = |g: &M3| {
        let a: Vec<f64> = b.verticals().into_iter().map(|s| wrap90(from_vertical(g, s))).collect();
        a.iter().cloned().fold(f64::MIN, f64::max) - a.iter().cloned().fold(f64::MAX, f64::min)
    };
    let s = oriented_solve(&rgb, UprightMode::Auto, f35).expect("auto solve");
    let (before, after) = (spread(&id), spread(&s.source_to_corrected));
    assert!(after < 0.5 * before, "auto: spread {before} -> {after}");
    let q = warped_corners(&s.source_to_corrected, f64::from(W), f64::from(H)).unwrap();
    assert!(max_stretch(&q, f64::from(W), f64::from(H)) <= 1.6 + 1e-6);
    // The roll is fully levelled: the centre vertical is vertical.
    let mid = b.verticals()[3];
    assert!(wrap90(from_vertical(&s.source_to_corrected, mid)).abs() < 1.5);
}

#[test]
fn guided_two_vertical_guides_become_vertical() {
    let b = Building { r: mul(&rot_z(-0.04), &rot_x(-0.22)), f: focal(28.0) };
    let guides_px = [b.verticals()[0], b.verticals()[5]];
    let to_norm = |p: (f64, f64)| NormPoint {
        x: ((p.0 + f64::from(W) / 2.0) / f64::from(W)) as f32,
        y: ((p.1 + f64::from(H) / 2.0) / f64::from(H)) as f32,
    };
    let guides: Vec<UprightGuide> =
        guides_px.iter().map(|s| UprightGuide { start: to_norm(s.0), end: to_norm(s.1) }).collect();
    // No image needed (Guided uses only the guides).
    let out = solve_rgb8(&[], W, H, 1, UprightMode::Guided, &guides, Some(28.0));
    let sol = out.solution.expect("guided solution");
    let inv = invert_matrix(&sol.matrix).unwrap();
    for g in &guides {
        let (ax, ay) = map_point(&inv, f64::from(g.start.x), f64::from(g.start.y));
        let (bx, by) = map_point(&inv, f64::from(g.end.x), f64::from(g.end.y));
        let a = ((bx - ax) * f64::from(W)).atan2((by - ay) * f64::from(H)).to_degrees();
        assert!(wrap90(a).abs() < 0.1, "guide off vertical by {a}");
    }
    // A single guide is not enough.
    let one = solve_rgb8(&[], W, H, 1, UprightMode::Guided, &guides[..1], Some(28.0));
    assert!(one.solution.is_none() && one.message.is_some());
}

/// Guides drawn in the sensor frame of a rotated (orientation 6) photo: the solve works in
/// the displayed frame and the matrix maps back to the sensor frame.
#[test]
fn guided_respects_orientation() {
    // Displayed frame is H x W (portrait) for orientation 6; sensor frame is W x H.
    let (dw, dh) = (H, W);
    let o = 6u8;
    let om = orientation_map(o);
    let to_sensor = |x: f64, y: f64| -> NormPoint {
        let (u, v) = (x / f64::from(dw), y / f64::from(dh));
        NormPoint { x: (om[0] * u + om[1] * v + om[2]) as f32, y: (om[3] * u + om[4] * v + om[5]) as f32 }
    };
    // Two converging (displayed) verticals.
    let guides = vec![
        UprightGuide { start: to_sensor(150.0, 100.0), end: to_sensor(110.0, 900.0) },
        UprightGuide { start: to_sensor(530.0, 100.0), end: to_sensor(575.0, 900.0) },
    ];
    let out = solve_rgb8(&[], dw, dh, o, UprightMode::Guided, &guides, None);
    let sol = out.solution.expect("solution");
    let inv = invert_matrix(&sol.matrix).unwrap();
    // Sensor -> displayed normalized: invert the orientation affine.
    let m = orient_to_sensor(o);
    let s2o = inverse(&m).unwrap();
    for g in &guides {
        let a = map_point(&inv, f64::from(g.start.x), f64::from(g.start.y));
        let b = map_point(&inv, f64::from(g.end.x), f64::from(g.end.y));
        let a = apply(&s2o, [a.0, a.1, 1.0]);
        let b = apply(&s2o, [b.0, b.1, 1.0]);
        let ang = ((b[0] - a[0]) * f64::from(dw)).atan2((b[1] - a[1]) * f64::from(dh)).to_degrees();
        assert!(wrap90(ang).abs() < 0.1, "displayed guide off vertical by {ang}");
    }
}

#[test]
fn no_lines_returns_no_solution_with_message() {
    // Fine noise.
    let noise: Vec<u8> = (0..W * H)
        .flat_map(|i| {
            let v = ((hash_noise(i % W, i / W) + 0.5) * 255.0) as u8;
            [v, v, v]
        })
        .collect();
    // Smooth sky gradient with a soft blob (cloud).
    let sky = render(W, H, |x, y| {
        let blob = (-(((x - 150.0).powi(2) + (y + 60.0).powi(2)) / 20000.0)).exp() as f32;
        0.55 + 0.25 * (-(y as f32) / H as f32) + 0.15 * blob
    });
    for (name, img) in [("noise", noise), ("sky", sky)] {
        for mode in [UprightMode::Level, UprightMode::Vertical, UprightMode::Auto, UprightMode::Full] {
            let out = solve_rgb8(&img, W, H, 1, mode, &[], None);
            assert!(out.solution.is_none(), "{name} {mode:?}: unexpected solution {:?}", out.solution);
            assert!(out.message.as_deref().is_some_and(|m| !m.is_empty()), "{name} {mode:?}: no message");
        }
    }
}

#[test]
fn detector_finds_a_straight_edge_precisely() {
    for deg in [0.0f64, 12.0, 47.0, 89.0, -30.0] {
        let (s, c) = deg.to_radians().sin_cos();
        let rgb = render(W, H, move |x, y| if -s * x + c * y < 0.0 { 0.7 } else { 0.3 });
        let segs = detect_segments(&luminance(&rgb, W, H), W, H);
        let best = segs.iter().max_by(|a, b| a.length().total_cmp(&b.length())).expect("a segment");
        let d = wrap90(best.angle_deg() - deg);
        assert!(d.abs() < 0.15, "edge {deg}: detected {}", best.angle_deg());
        assert!(best.length() > 300.0, "edge {deg}: length {}", best.length());
    }
}

/// "Auto straighten" for the crop tool: `crop.angle = crop_angle_for_rotation(rotationDeg,
/// orientation)` lines the crop frame's horizontal up with the displayed horizon, for every
/// EXIF orientation.
#[test]
fn crop_angle_matches_level_rotation_for_all_orientations() {
    let (sw, sh) = (6000u32, 4000u32);
    let phi = 3.2f64; // displayed horizon, descending right, degrees
    for o in 1..=8u8 {
        let (dw, dh) = if o >= 5 { (sh, sw) } else { (sw, sh) };
        let crop = CropSettings {
            enabled: true,
            left: 0.1,
            top: 0.1,
            right: 0.9,
            bottom: 0.9,
            angle: crop_angle_for_rotation(phi as f32, o),
        };
        let g = crop_geometry(&crop, sw, sh, o);
        // Output horizontal -> sensor pixels.
        let (ax, ay) = g.map(0.2, 0.5);
        let (bx, by) = g.map(0.8, 0.5);
        let out_dir = ((bx - ax) * f64::from(sw), (by - ay) * f64::from(sh));
        // Displayed horizon direction -> sensor pixels.
        let om = orientation_map(o);
        let (u, v) = (phi.to_radians().cos() / f64::from(dw), phi.to_radians().sin() / f64::from(dh));
        let hz = ((om[0] * u + om[1] * v) * f64::from(sw), (om[3] * u + om[4] * v) * f64::from(sh));
        let cross = out_dir.0 * hz.1 - out_dir.1 * hz.0;
        let ang = (cross / (out_dir.0.hypot(out_dir.1) * hz.0.hypot(hz.1))).asin().to_degrees();
        assert!(ang.abs() < 0.01, "orientation {o}: crop frame off the horizon by {ang} deg");
    }
}

#[test]
fn sensor_matrix_round_trips_orientation() {
    let frame = Frame::new(H, W, None);
    let s = OrientedSolve { source_to_corrected: rot_z(0.05), rotation_deg: 0.05f64.to_degrees() };
    for o in 1..=8u8 {
        let m = sensor_matrix(&s, &frame, o).unwrap();
        // Centre maps to centre in every orientation.
        let (x, y) = map_point(&m, 0.5, 0.5);
        assert!((x - 0.5).abs() < 1e-9 && (y - 0.5).abs() < 1e-9, "orientation {o}");
        assert!((m[8] - 1.0).abs() < 1e-12);
    }
}
