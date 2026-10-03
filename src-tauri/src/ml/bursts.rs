//! Burst grouping: frames shot in quick succession that look alike.

use std::collections::HashSet;

use super::imgproc::hamming;
use super::scoring::FACE_SOFT;
use super::{Burst, BurstFrame};
use crate::ipc::types::{ImageId, PickFlag, QualityScore, SuggestionReason, SuggestionReasonKind};

/// Clusters `frames` (sorted by `captured_at_ms`; sorted here defensively) into chains
/// where each frame is within `window_ms` of the previous one *and* its pHash is within
/// `max_hash_distance` bits of the previous frame's. Chaining (rather than comparing with
/// the first frame) follows slow pans and zooms through a burst. Groups of one are
/// dropped. Keeper = highest `overall` (earliest on ties).
pub fn group_bursts(frames: &[BurstFrame], window_ms: u32, max_hash_distance: u32) -> Vec<Burst> {
    let mut sorted: Vec<BurstFrame> = frames.to_vec();
    sorted.sort_by_key(|f| (f.captured_at_ms, f.id));
    let mut out = Vec::new();
    let mut cur: Vec<BurstFrame> = Vec::new();
    let flush = |cur: &mut Vec<BurstFrame>, out: &mut Vec<Burst>| {
        if cur.len() >= 2 {
            let keeper = cur.iter().fold(cur[0], |best, f| if f.overall > best.overall { *f } else { best });
            out.push(Burst { members: cur.iter().map(|f| f.id).collect(), keeper: keeper.id });
        }
        cur.clear();
    };
    for f in sorted {
        if let Some(prev) = cur.last() {
            let gap = f.captured_at_ms - prev.captured_at_ms;
            if gap > window_ms as i64 || hamming(prev.phash, f.phash) > max_hash_distance {
                flush(&mut cur, &mut out);
            }
        }
        cur.push(f);
    }
    flush(&mut cur, &mut out);
    out
}

/// Honours user-chosen keepers (`set_burst_keeper`): if any member of `burst` is in `pins`,
/// the pinned member with the highest `overall` (earliest on ties) becomes the keeper.
pub fn apply_pins(burst: &mut Burst, pins: &HashSet<ImageId>, overall: impl Fn(ImageId) -> f32) {
    let mut best: Option<(ImageId, f32)> = None;
    for &m in burst.members.iter().filter(|m| pins.contains(m)) {
        let o = overall(m);
        if best.is_none_or(|(_, b)| o > b) {
            best = Some((m, o));
        }
    }
    if let Some((id, _)) = best {
        burst.keeper = id;
    }
}

/// Lowers a burst non-keeper's suggestions: never a pick, and at most one star below the
/// keeper (`keeper_rating`). It is *not* rejected for being a duplicate: photographers
/// often keep several frames of a moment (on a real proposal shoot 38% of burst
/// non-keepers were kept), so only its own hard defects can make it a reject.
/// `duplicate_burst` stays a tag for filtering / collapsing.
pub fn demote(q: &mut QualityScore, keeper_rating: u8) {
    if q.suggested_pick == PickFlag::Pick {
        q.suggested_pick = PickFlag::Unflagged;
    }
    q.suggested_rating = q.suggested_rating.min(keeper_rating.saturating_sub(1));
}

/// Sharpness gap (0..=1 metric) above which "that one is sharper" is said.
const SHARPER_BY: f32 = 0.05;
/// Eye-openness gap above which "eyes are more open there" is said.
const EYES_BY: f32 = 0.1;

/// The `duplicate_burst` reason of a burst non-keeper (`related_image_id` = the keeper),
/// naming the keeper (`keeper_name`, e.g. its file name) and, where a component clearly
/// differs, why it was chosen: "Similar to DSC0123 in this burst — that one is sharper".
pub fn duplicate_reason(
    member: &QualityScore,
    keeper: &QualityScore,
    keeper_id: ImageId,
    keeper_name: &str,
    pinned: bool,
) -> SuggestionReason {
    let gap = |k: Option<f32>, m: Option<f32>| k.zip(m).map(|(k, m)| k - m);
    let why = if pinned {
        "you chose that one as the best of the burst"
    } else if gap(keeper.face_sharpness, member.face_sharpness).is_some_and(|d| d >= SHARPER_BY)
        || (keeper.face_sharpness.is_none() && keeper.global_sharpness - member.global_sharpness >= SHARPER_BY)
    {
        "that one is sharper"
    } else if gap(keeper.eyes_open, member.eyes_open).is_some_and(|d| d >= EYES_BY) {
        "eyes are more open in that one"
    } else if keeper.overall > member.overall {
        "that one scored higher"
    } else {
        "Sieve chose that one as the best of the burst"
    };
    SuggestionReason {
        kind: SuggestionReasonKind::DuplicateBurst,
        text: format!("Similar to {keeper_name} in this burst \u{2014} {why}"),
        related_image_id: Some(keeper_id),
    }
}

/// Adds a burst non-keeper's `duplicate_burst` reason in importance order: after the
/// defects (and after everything that made it a reject on its own), before a low score
/// and notes.
pub fn add_reason(q: &mut QualityScore, reason: SuggestionReason) {
    let after_defects = |k: SuggestionReasonKind| {
        matches!(k, SuggestionReasonKind::CreativeBlur | SuggestionReasonKind::Other)
            || (k == SuggestionReasonKind::LowScore && q.suggested_pick != PickFlag::Reject)
    };
    let at = q.reasons.iter().position(|r| after_defects(r.kind)).unwrap_or(q.reasons.len());
    q.reasons.insert(at, reason);
}

/// Makes a soft-face reason ([`FACE_SOFT`]) specific within a burst: "Face is soft
/// (sharpness 18% of the burst's best)", `best_face` = the sharpest primary face of the burst.
pub fn annotate_soft_face(q: &mut QualityScore, best_face: f32) {
    let Some(s) = q.face_sharpness else { return };
    if best_face <= 0.0 || s >= best_face {
        return;
    }
    let share = (s.max(0.0) / best_face * 100.0).round() as u32;
    for r in &mut q.reasons {
        if r.kind == SuggestionReasonKind::MissedFocus && r.text == FACE_SOFT {
            r.text = format!("{FACE_SOFT} (sharpness {share}% of the burst's best)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::ExposureStats;

    fn f(id: i64, t: i64, phash: u64, overall: f32) -> BurstFrame {
        BurstFrame { id, captured_at_ms: t, phash, overall }
    }

    #[test]
    fn groups_by_gap_and_hash_with_chaining() {
        let a = 0u64;
        let near = 0b111u64; // 3 bits from a
        let far = u64::MAX; // 64 bits
        let frames = vec![
            f(1, 0, a, 0.5),
            f(2, 1000, near, 0.9),            // chained
            f(3, 2400, near | 0b111000, 0.4), // 3 bits from 2, 6 from 1: chained
            f(4, 3000, far, 0.8),             // looks different -> new chain
            f(5, 3500, far, 0.7),             // with 4
            f(6, 9000, far, 0.9),             // too late
            f(7, 9100, a, 0.9),               // different
        ];
        let bursts = group_bursts(&frames, 1500, 4);
        assert_eq!(bursts, vec![Burst { members: vec![1, 2, 3], keeper: 2 }, Burst { members: vec![4, 5], keeper: 4 }]);
        // Tighter window splits the first chain.
        let bursts = group_bursts(&frames, 1200, 4);
        assert_eq!(bursts[0].members, vec![1, 2]);
        // Hash distance 0 only groups identical hashes.
        assert_eq!(group_bursts(&frames, 1500, 0), vec![Burst { members: vec![4, 5], keeper: 4 }]);
    }

    #[test]
    fn unsorted_input_and_ties() {
        let frames = vec![f(2, 100, 0, 0.5), f(1, 0, 0, 0.5)];
        assert_eq!(group_bursts(&frames, 1500, 0), vec![Burst { members: vec![1, 2], keeper: 1 }]);
        assert!(group_bursts(&[], 1500, 10).is_empty());
        assert!(group_bursts(&[f(1, 0, 0, 1.0)], 1500, 10).is_empty());
    }

    #[test]
    fn demote_caps_below_keeper_and_never_rejects() {
        let q = |pick, stars| QualityScore {
            overall: 0.8,
            face_sharpness: None,
            global_sharpness: 0.8,
            eyes_open: None,
            composition: None,
            face_count: 0,
            exposure: ExposureStats { clipped_highlights_pct: 0.0, clipped_shadows_pct: 0.0, mean_luma: 0.5 },
            model_version: String::new(),
            suggested_rating: stars,
            suggested_pick: pick,
            reasons: Vec::new(),
        };
        let mut a = q(PickFlag::Pick, 5);
        demote(&mut a, 4);
        assert_eq!((a.suggested_pick, a.suggested_rating), (PickFlag::Unflagged, 3));
        let mut b = q(PickFlag::Unflagged, 2);
        demote(&mut b, 5);
        assert_eq!((b.suggested_pick, b.suggested_rating), (PickFlag::Unflagged, 2), "own rating already lower");
        let mut c = q(PickFlag::Reject, 0);
        demote(&mut c, 3);
        assert_eq!(c.suggested_pick, PickFlag::Reject, "own hard defect stays a reject");
    }

    fn quality(overall: f32, face: Option<f32>, global: f32, eyes: Option<f32>) -> QualityScore {
        QualityScore {
            overall,
            face_sharpness: face,
            global_sharpness: global,
            eyes_open: eyes,
            composition: None,
            face_count: u32::from(face.is_some()),
            exposure: ExposureStats { clipped_highlights_pct: 0.0, clipped_shadows_pct: 0.0, mean_luma: 0.5 },
            model_version: String::new(),
            suggested_rating: 3,
            suggested_pick: PickFlag::Unflagged,
            reasons: Vec::new(),
        }
    }

    fn reason(kind: SuggestionReasonKind, text: &str) -> SuggestionReason {
        SuggestionReason { kind, text: text.to_string(), related_image_id: None }
    }

    #[test]
    fn duplicate_reason_points_at_the_keeper_and_says_why() {
        let keeper = quality(0.9, Some(0.8), 0.8, Some(1.0));
        let r = duplicate_reason(&quality(0.7, Some(0.5), 0.8, Some(1.0)), &keeper, 42, "DSC0123", false);
        assert_eq!(r.kind, SuggestionReasonKind::DuplicateBurst);
        assert_eq!(r.related_image_id, Some(42));
        assert_eq!(r.text, "Similar to DSC0123 in this burst \u{2014} that one is sharper");
        let r = duplicate_reason(&quality(0.7, Some(0.79), 0.8, Some(0.5)), &keeper, 42, "DSC0123", false);
        assert!(r.text.ends_with("eyes are more open in that one"), "{}", r.text);
        let r = duplicate_reason(&quality(0.85, Some(0.79), 0.8, Some(1.0)), &keeper, 42, "DSC0123", false);
        assert!(r.text.ends_with("that one scored higher"), "{}", r.text);
        // No faces: whole-frame sharpness.
        let r = duplicate_reason(&quality(0.7, None, 0.5, None), &quality(0.9, None, 0.8, None), 7, "A", false);
        assert!(r.text.ends_with("that one is sharper"), "{}", r.text);
        // User-chosen keeper (even if it is the softer frame).
        let r = duplicate_reason(&keeper, &quality(0.5, Some(0.4), 0.5, None), 3, "DSC0001", true);
        assert_eq!(r.text, "Similar to DSC0001 in this burst \u{2014} you chose that one as the best of the burst");
        assert_eq!(r.related_image_id, Some(3));
    }

    #[test]
    fn duplicate_reason_order() {
        let dup = || reason(SuggestionReasonKind::DuplicateBurst, "dup");
        let order = |q: &QualityScore| q.reasons.iter().map(|r| r.kind).collect::<Vec<_>>();
        use SuggestionReasonKind as K;
        // Former pick: the duplicate is the only reason.
        let mut q = quality(0.9, None, 0.8, None);
        add_reason(&mut q, dup());
        assert_eq!(order(&q), vec![K::DuplicateBurst]);
        // Defects first, then the duplicate, then a low score and notes.
        q.reasons = vec![
            reason(K::Blink, "Eyes closed"),
            reason(K::LowScore, "Low overall score"),
            reason(K::CreativeBlur, "Shallow"),
        ];
        add_reason(&mut q, dup());
        assert_eq!(order(&q), vec![K::Blink, K::DuplicateBurst, K::LowScore, K::CreativeBlur]);
        // A reject keeps what rejected it first.
        q.suggested_pick = PickFlag::Reject;
        q.reasons = vec![reason(K::LowScore, "Very low overall score"), reason(K::CreativeBlur, "Shallow")];
        add_reason(&mut q, dup());
        assert_eq!(order(&q), vec![K::LowScore, K::DuplicateBurst, K::CreativeBlur]);
    }

    #[test]
    fn soft_face_reason_is_relative_to_the_burst() {
        let mut q = quality(0.3, Some(0.18), 0.8, None);
        q.reasons = vec![reason(SuggestionReasonKind::MissedFocus, FACE_SOFT)];
        annotate_soft_face(&mut q, 1.0);
        assert_eq!(q.reasons[0].text, "Face is soft (sharpness 18% of the burst's best)");
        // The sharpest face of the burst, or other texts: unchanged.
        let mut best = quality(0.3, Some(0.3), 0.8, None);
        best.reasons = vec![reason(SuggestionReasonKind::MissedFocus, FACE_SOFT)];
        annotate_soft_face(&mut best, 0.3);
        assert_eq!(best.reasons[0].text, FACE_SOFT);
    }

    #[test]
    fn pins_override_keeper() {
        let overall = |id: ImageId| [0.0, 0.9, 0.2, 0.5, 0.5][id as usize];
        let mut b = Burst { members: vec![1, 2, 3, 4], keeper: 1 };
        apply_pins(&mut b, &HashSet::new(), overall);
        assert_eq!(b.keeper, 1, "no pins: unchanged");
        apply_pins(&mut b, &HashSet::from([2, 9]), overall);
        assert_eq!(b.keeper, 2);
        apply_pins(&mut b, &HashSet::from([2, 3, 4]), overall);
        assert_eq!(b.keeper, 3, "best pinned, earliest on ties");
    }
}
