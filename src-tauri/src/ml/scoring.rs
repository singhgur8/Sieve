//! Pure scoring: [`ImageMetrics`] + [`CullThresholds`] + [`ShootType`] -> scores, per-face
//! flags, auto tags and suggested rating/pick. No ML, no I/O (rescore path).
//!
//! Face roles (calibrated on `test-data/labels.json`, see `docs/decisions.md`):
//! - **considered**: box height at least `minFaceSize` x preview height and detector
//!   score at least [`MIN_CONSIDER_SCORE`]. Smaller faces (crowds, faces inside picture frames) are
//!   reported but never drive tags or scores.
//! - **subjects**: considered faces at least [`SUBJECT_REL`] x the largest considered
//!   face's height (drops background people in wide shots).
//! - **primary**: the *sharpest* subject (ties: larger). If any subject is sharp the
//!   photographer nailed focus on someone, so soft secondary faces (shallow depth of
//!   field, bokeh) are `creative_blur`, never `missed_focus`.
//! - **eye state** of a frontal subject about as sharp as the primary (blurred background
//!   faces give unreliable eyes), see [`eye_state`]: low EAR alone cannot tell closed eyes
//!   from a downcast gaze or a smiling squint, so `Closed` needs three agreeing signals:
//!   both eyes' EAR below `blinkEar`, the eye-state CNN saying closed, and laugh evidence
//!   on the face (wide or open mouth, cheeks pushing the lower lids up). Low EAR without
//!   that is `Undetermined` (typically looking down): no tag, no score penalty, never a
//!   reject. The CNN saying open overrides a low EAR (squint).
//! - **blink**: with `requireAllEyesOpen` any `Closed` judged subject tags the image,
//!   otherwise only the primary. Laughing blinks only soften the suggestion.
//! - **focus faces**: subjects at least [`FOCUS_MIN_FACE`] tall and not cut off by the
//!   frame edge. `missed_focus` needs a frontal focus face and *no* sharp subject;
//!   without focus faces the whole-frame rule decides (detail shots, wide scenes).

use super::{AutoTag, FaceMetrics, ImageMetrics, Scored, MODEL_VERSION};
use crate::ipc::types::{CullTag, CullThresholds, FaceInfo, PickFlag, QualityScore, ShootType};

/// Minimum detector confidence for a face to be judged.
pub const MIN_CONSIDER_SCORE: f32 = 0.6;
/// Subjects are at least this share of the largest considered face's height.
pub const SUBJECT_REL: f32 = 0.35;
/// A subject face may be this much softer (whole-face crop) than the primary and still
/// be judged for blinks.
pub const BLINK_SHARPNESS_SLACK: f32 = 0.1;
/// Eye-state CNN: mean open probability below this means the eyes look closed.
pub const EYE_OPEN_PROB: f32 = 0.5;
/// Laugh evidence from the mouth (a region independent of the eye landmarks, which are
/// unreliable on closed / lowered eyes): inner-lip gap / mouth width, or mouth width /
/// outer eye-corner span (broad smile).
pub const LAUGH_MOUTH_OPEN: f32 = 0.2;
pub const LAUGH_MOUTH_WIDTH: f32 = 0.64;
/// Suggested-star cap for a frame with a blink (a blink is never an automatic reject:
/// see `docs/decisions.md`) and for a laughing blink (open or very wide mouth).
pub const BLINK_MAX_STARS: u8 = 2;
pub const LAUGH_BLINK_MAX_STARS: u8 = 3;
pub const SOFT_BLINK_MOUTH_WIDTH: f32 = 0.7;
/// Faces must be at least this share of the frame height to judge focus on
/// (~70 px on the 2048 px preview); smaller faces are scene elements.
pub const FOCUS_MIN_FACE: f32 = 0.05;
/// Faces darker than this (mean luma of the face crop) show no detail to judge focus on.
pub const FOCUS_MIN_FACE_LUMA: f32 = 0.12;
/// Head pitch (degrees, 3D pose from FaceMesh) at or above which the face is turned down:
/// a lowered gaze then looks exactly like closed eyes, so the eyes are not judged.
pub const HEAD_PITCH_DOWN: f32 = 15.0;
/// Openness reported for eyes the CNN calls open despite a low EAR (smiling squint).
pub const SQUINT_OPENNESS: f32 = 0.8;
/// Directional anisotropy above which a soft subject is `motion_blur`.
pub const MOTION_ANISOTROPY: f32 = 0.35;
/// Median tile sharpness below which an in-focus frame is bokeh-heavy (`creative_blur`).
pub const BOKEH_TILE_P50: f32 = 0.35;
/// `overall` needed for 2, 3, 4 and 5 suggested stars (1 below). Spread so that on a
/// typical wedding set roughly the top sixth gets 5 stars.
pub const STAR_CUTS: [f32; 4] = [0.5, 0.65, 0.78, 0.88];
/// Soft faces at least this share of the primary's height count as co-subjects for
/// `creative_blur` (e.g. the groom behind the bride); smaller soft faces are background.
pub const CREATIVE_REL: f32 = 0.6;
/// A secondary subject this much softer than the primary is deliberate shallow focus.
pub const CREATIVE_SHARPNESS_GAP: f32 = 0.15;
/// Eye-openness stand-in when eyes matter (Wedding/Portrait) but no frontal subject
/// face could be judged (backs, profiles, detail shots): keeps them below clean portraits.
pub const UNJUDGED_EYES: f32 = 0.6;

/// Eye openness 0..=1 from the EAR of the more-open eye: 0.45 at the blink threshold,
/// 1.0 at 1.6x the threshold.
pub fn openness(ear_open: f32, blink_ear: f32) -> f32 {
    let b = blink_ear.max(1e-3);
    ((ear_open - 0.5 * b) / (1.1 * b)).clamp(0.0, 1.0)
}

/// EAR of the more-open eye (a blink closes both).
fn ear_open(f: &FaceMetrics) -> Option<f32> {
    match (f.ear_left, f.ear_right) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b).or(f.ear),
    }
}

/// Judged state of a face's eyes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EyeState {
    /// Openness 0..=1.
    Open(f32),
    /// Openness 0..=1 (below the blink threshold).
    Closed(f32),
    /// Looks closed by geometry, but could be a downcast gaze: not judged.
    Undetermined,
}

fn laugh_evidence(f: &FaceMetrics) -> bool {
    f.mouth_open.is_some_and(|v| v >= LAUGH_MOUTH_OPEN) || f.mouth_width.is_some_and(|v| v >= LAUGH_MOUTH_WIDTH)
}

/// Eye state of a face with landmarks (`None` without EAR). See the module docs.
pub fn eye_state(f: &FaceMetrics, blink_ear: f32) -> Option<EyeState> {
    let ear = ear_open(f)?;
    if ear >= blink_ear {
        return Some(EyeState::Open(openness(ear, blink_ear)));
    }
    Some(match f.eye_open_prob {
        Some(p) if p >= EYE_OPEN_PROB => EyeState::Open(SQUINT_OPENNESS),
        Some(_) if f.head_pitch.is_some_and(|p| p < HEAD_PITCH_DOWN) && laugh_evidence(f) => {
            EyeState::Closed(openness(ear, blink_ear))
        }
        _ => EyeState::Undetermined,
    })
}

/// Confidence from how far `value` is past `threshold` (relative), 0.5..=1.
fn margin_conf(value: f32, threshold: f32) -> f32 {
    let t = threshold.abs().max(1e-3);
    (0.5 + (value - threshold).abs() / t).clamp(0.5, 1.0)
}

/// Maps a sharpness onto 0..=1 around a threshold: 0 at `min - 0.15`, 1/3 at `min`,
/// 1.0 at `min + 0.3` (continuous over the range sharp frames actually span).
fn sharp_score(s: f32, min: f32) -> f32 {
    ((s - (min - 0.15)) / 0.45).clamp(0.0, 1.0)
}

fn exposure_score(m: &ImageMetrics) -> f32 {
    let e = &m.exposure;
    let luma = e.mean_luma;
    let mut s = 1.0;
    s -= (0.25 - luma).max(0.0) * 3.0;
    s -= (luma - 0.7).max(0.0) * 3.0;
    s -= e.clipped_highlights_pct * 5.0;
    s -= (e.clipped_shadows_pct - 0.05).max(0.0) * 2.0;
    s.clamp(0.0, 1.0)
}

/// Simple placement score of the primary face: centred-ish horizontally, eyes in the
/// upper half, not cut off at the top.
fn composition_score(f: &FaceMetrics) -> f32 {
    let cx = f.bbox.x + f.bbox.width / 2.0;
    let cy = f.bbox.y + f.bbox.height / 2.0;
    let mut s = 1.0 - (cx - 0.5).abs() * 0.6;
    s -= (cy - 0.55).max(0.0) * 1.5;
    s -= (0.12 - cy).max(0.0) * 3.0;
    if f.bbox.y <= 0.001 {
        s -= 0.2;
    }
    s.clamp(0.0, 1.0)
}

pub fn score(m: &ImageMetrics, t: &CullThresholds, shoot_type: ShootType) -> Scored {
    let n = m.faces.len();
    let considered: Vec<bool> =
        m.faces.iter().map(|f| f.bbox.height >= t.min_face_size && f.detection_score >= MIN_CONSIDER_SCORE).collect();
    let largest = m.faces.iter().zip(&considered).filter(|(_, &c)| c).map(|(f, _)| f.bbox.height).fold(0.0, f32::max);
    let subject: Vec<bool> = (0..n).map(|i| considered[i] && m.faces[i].bbox.height >= SUBJECT_REL * largest).collect();
    let primary: Option<usize> = (0..n).filter(|&i| subject[i]).max_by(|&a, &b| {
        let (fa, fb) = (&m.faces[a], &m.faces[b]);
        fa.sharpness.total_cmp(&fb.sharpness).then(fa.bbox.height.total_cmp(&fb.bbox.height))
    });
    let primary_sharp = primary.map(|p| m.faces[p].sharpness);
    // Blink eligibility compares whole-face sharpness: closed eyes have little eye-region
    // detail, so eye sharpness would exclude exactly the blinking faces.
    let primary_face_sharp = primary.map(|p| m.faces[p].face_sharpness);

    // Per-face flags.
    let mut faces = Vec::with_capacity(n);
    let mut blinking: Vec<usize> = Vec::new();
    let mut judged_open: Vec<f32> = Vec::new();
    for (i, f) in m.faces.iter().enumerate() {
        let state = eye_state(f, t.blink_ear);
        let judged = f.frontal
            && subject[i]
            && primary_face_sharp.is_some_and(|p| f.face_sharpness >= p - BLINK_SHARPNESS_SLACK)
            && (t.require_all_eyes_open || primary == Some(i));
        let blink = judged && matches!(state, Some(EyeState::Closed(_)));
        if blink {
            blinking.push(i);
        }
        let openness = match state {
            Some(EyeState::Open(o) | EyeState::Closed(o)) => Some(o),
            _ => None,
        };
        if judged {
            judged_open.extend(openness);
        }
        faces.push(FaceInfo {
            bbox: f.bbox,
            left_eye: f.left_eye,
            right_eye: f.right_eye,
            detection_score: f.detection_score,
            ear: f.ear,
            eyes_open: openness,
            sharpness: f.sharpness,
            blink,
            in_focus: f.sharpness >= t.face_sharpness_min,
            primary: primary == Some(i),
            considered: considered[i],
        });
    }

    let mut tags: Vec<AutoTag> = Vec::new();
    // Faces that can be judged for focus: big enough, inside the frame, lit enough to
    // show detail (silhouettes and deep shadow have none, whatever the focus).
    let judgeable = |f: &FaceMetrics| {
        !f.truncated && f.bbox.height >= FOCUS_MIN_FACE.max(t.min_face_size) && f.face_luma >= FOCUS_MIN_FACE_LUMA
    };
    let has_frontal_focus_face = (0..n).any(|i| subject[i] && m.faces[i].frontal && judgeable(&m.faces[i]));
    // Any judgeable sharp face (whatever its detector confidence or role) means the
    // photographer nailed focus on someone.
    let any_sharp_face = m.faces.iter().any(|f| judgeable(f) && f.sharpness >= t.face_sharpness_min);

    // Focus. Faces decide when a frontal focus face exists; otherwise the whole frame.
    // The primary is the sharpest subject, so any sharp subject means "in focus".
    let (missed_focus, motion) = match primary {
        Some(_) if any_sharp_face => (None, false),
        Some(p) if has_frontal_focus_face => {
            let f = &m.faces[p];
            let soft = f.sharpness < t.face_sharpness_min;
            (soft.then(|| margin_conf(f.sharpness, t.face_sharpness_min)), soft && f.anisotropy >= MOTION_ANISOTROPY)
        }
        _ => {
            let soft = m.global_sharpness < t.global_sharpness_min;
            (
                soft.then(|| margin_conf(m.global_sharpness, t.global_sharpness_min)),
                soft && m.tiles.anisotropy >= MOTION_ANISOTROPY,
            )
        }
    };
    if let Some(c) = missed_focus {
        tags.push(AutoTag { tag: CullTag::MissedFocus, confidence: c });
        if motion {
            tags.push(AutoTag { tag: CullTag::MotionBlur, confidence: 0.6 });
        }
    } else {
        // In focus: soft secondary faces or a mostly blurred frame are deliberate.
        let primary_h = primary.map_or(0.0, |p| m.faces[p].bbox.height);
        let soft_secondary = primary_sharp.is_some_and(|p| {
            (0..n).any(|i| {
                subject[i]
                    && m.faces[i].bbox.height >= CREATIVE_REL * primary_h
                    && primary != Some(i)
                    && !faces[i].in_focus
                    && m.faces[i].sharpness < p - CREATIVE_SHARPNESS_GAP
            })
        });
        let sharp_frame = m.global_sharpness >= t.global_sharpness_min;
        let bokeh = m.tiles.p50 < BOKEH_TILE_P50 && sharp_frame;
        // A soft face cut off by the frame in an otherwise sharp frame: detail shot.
        let soft_cropped = sharp_frame && (0..n).any(|i| considered[i] && m.faces[i].truncated && !faces[i].in_focus);
        let soft_secondary = soft_secondary || soft_cropped;
        if soft_secondary || bokeh {
            tags.push(AutoTag { tag: CullTag::CreativeBlur, confidence: if soft_secondary { 0.7 } else { 0.6 } });
        }
    }

    // Blink.
    let laughing = !blinking.is_empty()
        && blinking.iter().all(|&i| {
            let f = &m.faces[i];
            f.mouth_open.is_some_and(|mo| mo >= LAUGH_MOUTH_OPEN)
                || f.mouth_width.is_some_and(|mw| mw >= SOFT_BLINK_MOUTH_WIDTH)
        });
    if !blinking.is_empty() {
        let worst = blinking.iter().filter_map(|&i| ear_open(&m.faces[i])).fold(f32::MAX, f32::min);
        tags.push(AutoTag { tag: CullTag::Blink, confidence: margin_conf(worst, t.blink_ear) });
    }

    // Exposure.
    let e = &m.exposure;
    if e.mean_luma < t.underexposed_mean_luma || e.clipped_shadows_pct > t.underexposed_clip_pct {
        let c = if e.mean_luma < t.underexposed_mean_luma {
            margin_conf(e.mean_luma, t.underexposed_mean_luma)
        } else {
            margin_conf(e.clipped_shadows_pct, t.underexposed_clip_pct)
        };
        tags.push(AutoTag { tag: CullTag::Underexposed, confidence: c });
    }
    if e.clipped_highlights_pct > t.overexposed_clip_pct {
        tags.push(AutoTag {
            tag: CullTag::Overexposed,
            confidence: margin_conf(e.clipped_highlights_pct, t.overexposed_clip_pct),
        });
    }

    // Component scores (undetermined eyes are left out, never penalized).
    let eyes_open = (!judged_open.is_empty()).then(|| judged_open.iter().cloned().fold(1.0, f32::min));
    let face_sharpness = primary.map(|p| m.faces[p].sharpness);
    let composition = primary.map(|p| composition_score(&m.faces[p]));
    let w = t.weights;
    let mut parts: Vec<(f32, f32)> = vec![
        (w.global_sharpness, sharp_score(m.global_sharpness, t.global_sharpness_min)),
        (w.exposure, exposure_score(m)),
    ];
    let eyes_weighted = w.eyes_open > w.global_sharpness;
    match eyes_open {
        Some(v) => parts.push((w.eyes_open, v)),
        None if eyes_weighted => parts.push((w.eyes_open, UNJUDGED_EYES)),
        None => {}
    }
    if let Some(s) = face_sharpness {
        parts.push((w.face_sharpness, sharp_score(s, t.face_sharpness_min)));
    }
    if let Some(c) = composition {
        parts.push((w.composition, c));
    }
    let wsum: f32 = parts.iter().map(|p| p.0).sum();
    let mut overall = if wsum > 0.0 { parts.iter().map(|p| p.0 * p.1).sum::<f32>() / wsum } else { 0.5 };
    // Hard defects cap the score so they sort below clean frames.
    let eyes_matter = w.eyes_open > 0.0 && !matches!(shoot_type, ShootType::Landscape);
    if missed_focus.is_some() {
        overall = overall.min(0.35);
    }
    if !blinking.is_empty() && eyes_matter {
        overall = overall.min(if laughing { 0.6 } else { 0.4 });
    }
    let overall = overall.clamp(0.0, 1.0);

    // Suggestions. Exposure problems (often intentional low key) only withhold `pick`.
    let badly_exposed = tags.iter().any(|t| matches!(t.tag, CullTag::Underexposed | CullTag::Overexposed));
    let defect = missed_focus.is_some() || (!blinking.is_empty() && eyes_matter);
    // A blink lowers the stars but never rejects on its own: closed eyes cannot be told
    // apart from a lowered gaze with certainty in a single still.
    let hard_reject = missed_focus.is_some();
    let suggested_pick = if hard_reject || overall < t.reject_max_overall {
        PickFlag::Reject
    } else if !defect && !badly_exposed && overall >= t.pick_min_overall {
        PickFlag::Pick
    } else {
        PickFlag::Unflagged
    };
    let suggested_rating = if suggested_pick == PickFlag::Reject {
        0
    } else {
        let stars = STAR_CUTS.iter().filter(|&&c| overall >= c).count() as u8 + 1;
        if !blinking.is_empty() && eyes_matter {
            stars.min(if laughing { LAUGH_BLINK_MAX_STARS } else { BLINK_MAX_STARS })
        } else if defect {
            stars.min(3)
        } else {
            stars
        }
    };

    Scored {
        quality: QualityScore {
            overall,
            face_sharpness,
            global_sharpness: m.global_sharpness,
            eyes_open,
            composition,
            face_count: considered.iter().filter(|&&c| c).count() as u32,
            exposure: m.exposure.clone(),
            model_version: MODEL_VERSION.to_string(),
            suggested_rating,
            suggested_pick,
        },
        faces,
        tags,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ipc::types::{ExposureStats, NormPoint, NormRect};
    use crate::ml::thresholds::default_thresholds;
    use crate::ml::TileStats;

    /// A frontal face; `ear` < 0.15 gives a genuinely closed-eye face (CNN closed,
    /// broad smile as when laughing, lips together).
    pub(crate) fn face(x: f32, h: f32, sharp: f32, ear: f32) -> FaceMetrics {
        let closed = ear < 0.15;
        FaceMetrics {
            truncated: false,
            eye_open_prob: Some(if closed { 0.05 } else { 0.98 }),
            mouth_width: Some(if closed { 0.66 } else { 0.6 }),
            head_pitch: Some(5.0),
            head_yaw: Some(0.0),
            mesh_ear: Some(ear),
            bbox: NormRect { x, y: 0.3, width: h * 0.66, height: h },
            left_eye: NormPoint { x: x + 0.1 * h, y: 0.3 + 0.4 * h },
            right_eye: NormPoint { x: x + 0.5 * h, y: 0.3 + 0.4 * h },
            detection_score: 0.9,
            ear: Some(ear),
            sharpness: sharp,
            ear_left: Some(ear),
            ear_right: Some(ear),
            mouth_open: Some(0.05),
            yaw: 0.0,
            iod_px: 60.0,
            face_sharpness: sharp,
            eye_texture: 8.0,
            face_luma: 0.4,
            anisotropy: 0.2,
            frontal: true,
        }
    }

    pub(crate) fn metrics(faces: Vec<FaceMetrics>) -> ImageMetrics {
        ImageMetrics {
            width: 2048,
            height: 1365,
            faces,
            global_sharpness: 0.8,
            exposure: ExposureStats { clipped_highlights_pct: 0.0, clipped_shadows_pct: 0.0, mean_luma: 0.45 },
            phash: 0,
            tiles: TileStats { p90: 0.8, p50: 0.6, textured: 0.8, anisotropy: 0.05 },
        }
    }

    fn has(s: &Scored, tag: CullTag) -> bool {
        s.tags.iter().any(|t| t.tag == tag)
    }

    const W: ShootType = ShootType::Wedding;

    #[test]
    fn clean_portrait_is_a_pick() {
        let s = score(&metrics(vec![face(0.4, 0.15, 0.7, 0.28)]), &default_thresholds(W), W);
        assert!(s.tags.is_empty(), "{:?}", s.tags);
        assert_eq!(s.quality.suggested_pick, PickFlag::Pick);
        assert!(s.quality.suggested_rating >= 4);
        assert_eq!(s.quality.face_count, 1);
        assert!(s.faces[0].primary && s.faces[0].considered && s.faces[0].in_focus && !s.faces[0].blink);
        assert!(s.quality.eyes_open.unwrap() > 0.9);
    }

    #[test]
    fn blink_lowers_stars_but_never_rejects() {
        let t = default_thresholds(W);
        let s = score(&metrics(vec![face(0.4, 0.15, 0.7, 0.05)]), &t, W);
        assert!(has(&s, CullTag::Blink));
        assert_eq!(s.quality.suggested_pick, PickFlag::Unflagged);
        assert!(s.quality.suggested_rating <= BLINK_MAX_STARS);
        let mut laughing = face(0.4, 0.15, 0.7, 0.05);
        laughing.mouth_open = Some(0.4);
        let s = score(&metrics(vec![laughing]), &t, W);
        assert!(has(&s, CullTag::Blink));
        assert_ne!(s.quality.suggested_pick, PickFlag::Reject);
        assert!((1..=LAUGH_BLINK_MAX_STARS).contains(&s.quality.suggested_rating));
    }

    #[test]
    fn downcast_gaze_and_squints_are_not_blinks() {
        let t = default_thresholds(W);
        // Looking down: low EAR, CNN "closed", but no laugh evidence -> undetermined.
        let mut down = face(0.4, 0.15, 0.7, 0.05);
        down.mouth_width = Some(0.55);
        let s = score(&metrics(vec![down.clone()]), &t, W);
        assert!(!has(&s, CullTag::Blink) && !s.faces[0].blink);
        assert_eq!(s.faces[0].eyes_open, None);
        assert_eq!(s.quality.eyes_open, None);
        assert_ne!(s.quality.suggested_pick, PickFlag::Reject);
        // Partner looking down at a sharp open-eyed primary in a couple shot: no penalty.
        let s = score(&metrics(vec![face(0.2, 0.15, 0.72, 0.28), down]), &t, W);
        assert!(!has(&s, CullTag::Blink));
        assert_eq!(s.quality.suggested_pick, PickFlag::Pick);
        // Smiling squint: low EAR, but the CNN sees open eyes.
        let mut squint = face(0.4, 0.15, 0.7, 0.12);
        squint.eye_open_prob = Some(0.9);
        squint.mouth_width = Some(0.75);
        let s = score(&metrics(vec![squint]), &t, W);
        assert!(!has(&s, CullTag::Blink));
        assert_eq!(s.faces[0].eyes_open, Some(SQUINT_OPENNESS));
        // Head strongly turned down (face-down pose): not judged.
        let mut face_down = face(0.4, 0.15, 0.7, 0.05);
        face_down.head_pitch = Some(40.0);
        assert!(!has(&score(&metrics(vec![face_down]), &t, W), CullTag::Blink));
        // No CNN result: never a blink.
        let mut unknown = face(0.4, 0.15, 0.7, 0.05);
        unknown.eye_open_prob = None;
        assert!(!has(&score(&metrics(vec![unknown]), &t, W), CullTag::Blink));
    }

    #[test]
    fn cropped_and_small_faces_do_not_decide_focus() {
        let t = default_thresholds(W);
        // Jewellery detail: soft face cut off by the frame, sharp frame -> creative, not missed.
        let mut cropped = face(0.0, 0.45, 0.2, 0.28);
        cropped.truncated = true;
        let s = score(&metrics(vec![cropped]), &t, W);
        assert!(!has(&s, CullTag::MissedFocus));
        assert!(has(&s, CullTag::CreativeBlur));
        // Tiny soft face in a wide sharp scene: whole-frame rule, not missed.
        let s = score(&metrics(vec![face(0.4, 0.045, 0.3, 0.28)]), &t, W);
        assert!(!has(&s, CullTag::MissedFocus));
    }

    #[test]
    fn one_eye_open_is_not_a_blink_and_profiles_are_not_judged() {
        let t = default_thresholds(W);
        let mut wink = face(0.4, 0.15, 0.7, 0.05);
        wink.ear_right = Some(0.3);
        assert!(!has(&score(&metrics(vec![wink]), &t, W), CullTag::Blink));
        let mut profile = face(0.4, 0.15, 0.7, 0.05);
        profile.frontal = false;
        assert!(!has(&score(&metrics(vec![profile]), &t, W), CullTag::Blink));
    }

    #[test]
    fn group_shot_requires_all_subjects_open() {
        let faces = vec![face(0.2, 0.1, 0.7, 0.28), face(0.6, 0.1, 0.68, 0.05)];
        let wedding = score(&metrics(faces.clone()), &default_thresholds(W), W);
        assert!(has(&wedding, CullTag::Blink));
        assert!(wedding.faces[1].blink && !wedding.faces[1].primary);
        // General: only the primary (sharpest) face counts.
        let general = score(&metrics(faces), &default_thresholds(ShootType::General), ShootType::General);
        assert!(!has(&general, CullTag::Blink));
    }

    #[test]
    fn tiny_and_blurred_background_faces_do_not_drive_tags() {
        let t = default_thresholds(W);
        // Tiny closed-eye face (picture frame) + sharp main face.
        let s = score(&metrics(vec![face(0.4, 0.15, 0.7, 0.28), face(0.8, 0.02, 0.2, 0.02)]), &t, W);
        assert!(!has(&s, CullTag::Blink) && !has(&s, CullTag::MissedFocus));
        assert!(!s.faces[1].considered);
        // Out-of-focus groom behind a sharp bride, eyes closed: creative blur, no blink.
        let s = score(&metrics(vec![face(0.2, 0.15, 0.3, 0.02), face(0.6, 0.14, 0.72, 0.28)]), &t, W);
        assert!(s.faces[1].primary);
        assert!(!has(&s, CullTag::MissedFocus) && !has(&s, CullTag::Blink));
        assert!(has(&s, CullTag::CreativeBlur));
    }

    #[test]
    fn soft_subject_is_missed_focus_and_motion_when_directional() {
        let t = default_thresholds(W);
        let s = score(&metrics(vec![face(0.4, 0.15, 0.3, 0.28)]), &t, W);
        assert!(has(&s, CullTag::MissedFocus) && !has(&s, CullTag::MotionBlur));
        assert_eq!(s.quality.suggested_pick, PickFlag::Reject);
        assert!(s.quality.overall <= 0.35);
        let mut moving = face(0.4, 0.15, 0.3, 0.28);
        moving.anisotropy = 0.5;
        let s = score(&metrics(vec![moving]), &t, W);
        assert!(has(&s, CullTag::MissedFocus) && has(&s, CullTag::MotionBlur));
        // No faces: whole-frame sharpness decides.
        let mut m = metrics(vec![]);
        m.global_sharpness = 0.1;
        assert!(has(&score(&m, &t, W), CullTag::MissedFocus));
    }

    #[test]
    fn exposure_tags() {
        let t = default_thresholds(W);
        let mut m = metrics(vec![]);
        m.exposure.mean_luma = 0.05;
        let s = score(&m, &t, W);
        assert!(has(&s, CullTag::Underexposed));
        assert!(!s.quality.overall.is_nan());
        m.exposure = ExposureStats { clipped_highlights_pct: 0.2, clipped_shadows_pct: 0.0, mean_luma: 0.6 };
        assert!(has(&score(&m, &t, W), CullTag::Overexposed));
    }

    #[test]
    fn openness_scale() {
        assert!((openness(0.15, 0.15) - 0.45).abs() < 0.01);
        assert_eq!(openness(0.3, 0.15), 1.0);
        assert_eq!(openness(0.0, 0.15), 0.0);
    }
}
