//! Head pose from FaceMesh V2 landmarks: rigid fit (Horn's quaternion method, least
//! squares over the 468 mesh vertices) of MediaPipe's canonical 3D face to the predicted
//! 3D landmarks, the same idea as MediaPipe's face-geometry module (orthographic, the
//! crop is small relative to the camera distance).

use super::canonical_face::CANONICAL_FACE;

/// Head rotation in degrees relative to the camera. `pitch > 0`: face turned down
/// (looking at the floor / at someone below), `yaw`: turned sideways, `roll`: in-plane.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct HeadPose {
    pub pitch: f32,
    pub yaw: f32,
    pub roll: f32,
}

/// `mesh`: FaceMesh landmarks (x right, y down, z = depth with smaller = closer, all in
/// the same pixel scale). At least the first 468 points are used.
pub fn head_pose(mesh: &[[f32; 3]]) -> HeadPose {
    let n = CANONICAL_FACE.len().min(mesh.len());
    if n < 3 {
        return HeadPose::default();
    }
    // Target in the canonical frame convention: y up, z towards the viewer.
    let b: Vec<[f64; 3]> = mesh[..n].iter().map(|p| [p[0] as f64, -p[1] as f64, -p[2] as f64]).collect();
    let a: Vec<[f64; 3]> = CANONICAL_FACE[..n].iter().map(|p| [p[0] as f64, p[1] as f64, p[2] as f64]).collect();
    let centroid = |v: &[[f64; 3]]| {
        let mut c = [0.0; 3];
        for p in v {
            for k in 0..3 {
                c[k] += p[k];
            }
        }
        c.map(|x| x / v.len() as f64)
    };
    let (ca, cb) = (centroid(&a), centroid(&b));
    // Cross-covariance S = sum (a - ca)(b - cb)^T.
    let mut s = [[0.0f64; 3]; 3];
    for (pa, pb) in a.iter().zip(&b) {
        for i in 0..3 {
            for j in 0..3 {
                s[i][j] += (pa[i] - ca[i]) * (pb[j] - cb[j]);
            }
        }
    }
    let [[sxx, sxy, sxz], [syx, syy, syz], [szx, szy, szz]] = s;
    let nmat = [
        [sxx + syy + szz, syz - szy, szx - sxz, sxy - syx],
        [syz - szy, sxx - syy - szz, sxy + syx, szx + sxz],
        [szx - sxz, sxy + syx, -sxx + syy - szz, syz + szy],
        [sxy - syx, szx + sxz, syz + szy, -sxx - syy + szz],
    ];
    let q = max_eigenvector(nmat);
    let r = quat_to_matrix(q);
    HeadPose {
        pitch: (-r[1][2]).atan2(r[2][2]).to_degrees() as f32,
        yaw: r[0][2].clamp(-1.0, 1.0).asin().to_degrees() as f32,
        roll: (-r[0][1]).atan2(r[0][0]).to_degrees() as f32,
    }
}

/// Rotation matrix of unit quaternion `(w, x, y, z)`.
fn quat_to_matrix([w, x, y, z]: [f64; 4]) -> [[f64; 3]; 3] {
    [
        [w * w + x * x - y * y - z * z, 2.0 * (x * y - w * z), 2.0 * (x * z + w * y)],
        [2.0 * (x * y + w * z), w * w - x * x + y * y - z * z, 2.0 * (y * z - w * x)],
        [2.0 * (x * z - w * y), 2.0 * (y * z + w * x), w * w - x * x - y * y + z * z],
    ]
}

/// Eigenvector of the largest eigenvalue of a symmetric 4x4 matrix (cyclic Jacobi).
fn max_eigenvector(mut a: [[f64; 4]; 4]) -> [f64; 4] {
    let mut v = [[0.0f64; 4]; 4];
    for (i, row) in v.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for _ in 0..50 {
        let off: f64 = (0..4)
            .flat_map(|i| (0..4).filter(move |&j| j != i).map(move |j| (i, j)))
            .map(|(i, j)| a[i][j] * a[i][j])
            .sum();
        if off < 1e-18 {
            break;
        }
        for p in 0..4 {
            for q in p + 1..4 {
                if a[p][q].abs() < 1e-30 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for row in a.iter_mut() {
                    let (akp, akq) = (row[p], row[q]);
                    row[p] = c * akp - s * akq;
                    row[q] = s * akp + c * akq;
                }
                let (rp, rq) = (a[p], a[q]);
                for k in 0..4 {
                    a[p][k] = c * rp[k] - s * rq[k];
                    a[q][k] = s * rp[k] + c * rq[k];
                }
                for row in v.iter_mut() {
                    let (vkp, vkq) = (row[p], row[q]);
                    row[p] = c * vkp - s * vkq;
                    row[q] = s * vkp + c * vkq;
                }
            }
        }
    }
    let best = (0..4).max_by(|&i, &j| a[i][i].total_cmp(&a[j][j])).unwrap_or(0);
    let q = [v[0][best], v[1][best], v[2][best], v[3][best]];
    let norm = q.iter().map(|x| x * x).sum::<f64>().sqrt().max(1e-12);
    q.map(|x| x / norm)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canonical face rotated by `pitch` about x (positive = chin towards the camera's
    /// down direction), yaw about y, scaled, shifted, in FaceMesh image conventions.
    fn posed(pitch: f32, yaw: f32) -> Vec<[f32; 3]> {
        let (sp, cp) = pitch.to_radians().sin_cos();
        let (sy, cy) = yaw.to_radians().sin_cos();
        CANONICAL_FACE
            .iter()
            .map(|&[x, y, z]| {
                // R = Ry(yaw) * Rx(pitch), Rx with the sign making the nose go down.
                let (y1, z1) = (y * cp - z * sp, y * sp + z * cp);
                let (x2, z2) = (x * cy + z1 * sy, -x * sy + z1 * cy);
                let (y2, x2) = (y1, x2);
                [x2 * 9.0 + 128.0, -y2 * 9.0 + 128.0, -z2 * 9.0]
            })
            .collect()
    }

    #[test]
    fn recovers_pitch_and_yaw() {
        let p = head_pose(&posed(0.0, 0.0));
        assert!(p.pitch.abs() < 0.5 && p.yaw.abs() < 0.5 && p.roll.abs() < 0.5, "{p:?}");
        let down = head_pose(&posed(-25.0, 0.0));
        let up = head_pose(&posed(25.0, 0.0));
        assert!((down.pitch.abs() - 25.0).abs() < 1.0 && (up.pitch.abs() - 25.0).abs() < 1.0);
        assert!(down.pitch.signum() != up.pitch.signum());
        let side = head_pose(&posed(0.0, 30.0));
        assert!((side.yaw.abs() - 30.0).abs() < 1.0 && side.pitch.abs() < 1.0, "{side:?}");
    }
}
