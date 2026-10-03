//! Burst grouping: frames shot in quick succession that look alike.

use std::collections::HashSet;

use super::imgproc::hamming;
use super::{Burst, BurstFrame};
use crate::ipc::types::{ImageId, PickFlag, QualityScore};

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
