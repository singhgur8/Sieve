//! Default culling thresholds per shoot type. The numbers are calibration data owned by
//! vision-ml-dev (tune against `test-data/labels.json`); the architect only seeded them.
//! User overrides are stored per shoot type in `catalog_meta` and overlaid on these
//! (`db::repo::cull_thresholds`).

use crate::ipc::types::{CullThresholds, ScoreWeights, ShootType};

pub fn default_thresholds(shoot_type: ShootType) -> CullThresholds {
    let base = CullThresholds {
        blink_ear: 0.18,
        require_all_eyes_open: false,
        min_face_size: 0.05,
        face_sharpness_min: 0.35,
        global_sharpness_min: 0.25,
        underexposed_mean_luma: 0.12,
        underexposed_clip_pct: 0.30,
        overexposed_clip_pct: 0.05,
        burst_hash_distance: 12,
        pick_min_overall: 0.75,
        reject_max_overall: 0.30,
        weights: ScoreWeights {
            eyes_open: 1.0,
            face_sharpness: 1.0,
            global_sharpness: 1.0,
            exposure: 0.5,
            composition: 0.25,
        },
    };
    match shoot_type {
        // Eyes open + eye sharpness dominate; every face in a group shot must be open-eyed.
        ShootType::Wedding | ShootType::Portrait => CullThresholds {
            require_all_eyes_open: true,
            min_face_size: 0.04,
            weights: ScoreWeights {
                eyes_open: 3.0,
                face_sharpness: 3.0,
                global_sharpness: 0.5,
                exposure: 0.5,
                composition: 0.25,
            },
            ..base
        },
        // Peak action matters more than eyes; subjects are often motion-blurred on purpose.
        ShootType::Sports => CullThresholds {
            blink_ear: 0.12,
            global_sharpness_min: 0.30,
            burst_hash_distance: 16,
            weights: ScoreWeights {
                eyes_open: 0.5,
                face_sharpness: 1.5,
                global_sharpness: 2.0,
                exposure: 0.5,
                composition: 0.25,
            },
            ..base
        },
        ShootType::Event => CullThresholds { min_face_size: 0.03, ..base },
        // Faces are incidental; whole-frame sharpness and exposure drive the score.
        ShootType::Landscape => CullThresholds {
            min_face_size: 0.10,
            global_sharpness_min: 0.35,
            weights: ScoreWeights {
                eyes_open: 0.0,
                face_sharpness: 0.0,
                global_sharpness: 2.0,
                exposure: 1.5,
                composition: 0.5,
            },
            ..base
        },
        ShootType::General => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        for &s in ShootType::ALL {
            default_thresholds(s).validate().unwrap_or_else(|e| panic!("{s:?}: {e}"));
        }
    }
}
