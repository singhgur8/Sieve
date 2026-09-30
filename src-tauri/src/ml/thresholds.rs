//! Default culling thresholds per shoot type. The numbers are calibration data owned by
//! vision-ml-dev (tune against `test-data/labels.json`); the architect only seeded them.
//! User overrides are stored per shoot type in `catalog_meta` and overlaid on these
//! (`db::repo::cull_thresholds`).

use crate::ipc::types::{CullThresholds, ScoreWeights, ShootType};

pub fn default_thresholds(shoot_type: ShootType) -> CullThresholds {
    // Calibrated on test-data/labels.json (103 wedding frames; see docs/decisions.md):
    // - blink_ear applies to the *more-open* eye (both eyes closed). Open eyes measure
    //   0.20-0.35, squints/laughs 0.15-0.20, downcast gaze 0.04-0.16, closed < 0.12.
    //   Low EAR is necessary, not sufficient: `scoring::eye_state` also needs the eye CNN,
    //   head pitch and a smiling mouth to agree (downcast gaze looks identical by EAR).
    // - face_sharpness_min: eye-region sharpness (worst direction, capped by detail, or a
    //   tight per-eye crop) of the sharpest face; sharp eyes 0.5-0.8, soft/motion 0.2-0.47.
    // - global_sharpness_min: 90th-percentile tile sharpness; in-focus frames 0.6-0.87.
    let base = CullThresholds {
        blink_ear: 0.15,
        require_all_eyes_open: false,
        min_face_size: 0.05,
        face_sharpness_min: 0.48,
        global_sharpness_min: 0.45,
        underexposed_mean_luma: 0.15,
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
            // ~1/3 of a wedding set suggested as picks (sample: 141/396).
            pick_min_overall: 0.8,
            weights: ScoreWeights {
                eyes_open: 3.0,
                face_sharpness: 3.0,
                global_sharpness: 0.5,
                // Low-key (first dance) frames are often intentional: exposure weighs little.
                exposure: 0.5,
                composition: 0.25,
            },
            ..base
        },
        // Peak action matters more than eyes; subjects are often motion-blurred on purpose.
        ShootType::Sports => CullThresholds {
            blink_ear: 0.12,
            global_sharpness_min: 0.50,
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
            global_sharpness_min: 0.55,
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
