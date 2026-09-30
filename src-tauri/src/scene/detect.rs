//! Scene grouping (pure). Owned by vision-ml-dev.
//!
//! One pass over the frames in detection order deciding, for every consecutive pair, whether
//! a scene boundary lies between them:
//! - folder change: always a boundary;
//! - inside a burst's span (between its first and last member in input order): never a
//!   boundary (bursts are never split, even by a time gap longer than `max_gap_ms`, which
//!   can only happen when the gap option is below the burst window);
//! - capture-time gap > `max_gap_ms` (both frames have a time): boundary;
//! - otherwise appearance: the frame joins when [`features::similarity`] to the running
//!   group appearance (a recency-weighted mean of the members' features, so slow drift such
//!   as sunset light stays one scene while a cut to another light does not) or to the previous
//!   frame is >=
//!   `similarity`. A frame without a capture time needs that similarity (with
//!   `similarity = 0` it joins); a frame with a time but no features (or a group without
//!   features yet) joins by time only.

use std::collections::HashMap;

use super::{features, DetectFrame, SceneFeatures};
use crate::ipc::types::{ImageId, SceneDetectOptions};

/// Weight of a new member in the running group appearance (exponential moving average).
const RUNNING_WEIGHT: f32 = 0.35;

/// Contract. Partitions `frames` (in detection order, see [`DetectFrame`]) into scenes. Rules
/// the result must satisfy (tested by the caller's tests / QA):
/// - every frame in exactly one group; groups non-empty, members in input order;
/// - a group never spans folders;
/// - consecutive frames whose capture-time gap exceeds `options.max_gap_ms` are never in the
///   same group (frames without a capture time only join by similarity);
/// - members of one burst (`burst_group_id`) are never split;
/// - otherwise a frame joins the running group when its feature similarity to the group
///   (e.g. histogram intersection of luma + Oklab a/b histograms against the group's running
///   mean, or to the previous frame) is >= `options.similarity`; `similarity = 0` splits on
///   time gaps only; frames without features join by time only.
pub fn group(frames: &[DetectFrame], options: &SceneDetectOptions) -> Vec<Vec<ImageId>> {
    // Boundaries between i-1 and i that a burst forbids: first < i <= last of the burst's
    // members within one folder run.
    let mut span: HashMap<(i64, i64), (usize, usize)> = HashMap::new();
    for (i, f) in frames.iter().enumerate() {
        if let Some(b) = f.burst_group_id {
            span.entry((f.folder_id, b)).and_modify(|s| s.1 = i).or_insert((i, i));
        }
    }
    let mut locked = vec![false; frames.len()];
    for &(first, last) in span.values() {
        for l in locked.iter_mut().take(last + 1).skip(first + 1) {
            *l = true;
        }
    }

    let threshold = if options.similarity.is_finite() { options.similarity.clamp(0.0, 1.0) } else { 0.0 };
    let mut groups: Vec<Vec<ImageId>> = Vec::new();
    let mut running: Option<SceneFeatures> = None;
    for (i, f) in frames.iter().enumerate() {
        let boundary = match i.checked_sub(1).map(|p| &frames[p]) {
            None => true,
            Some(prev) if prev.folder_id != f.folder_id => true,
            Some(_) if locked[i] => false,
            Some(prev) => {
                let time_split = match (prev.captured_at_ms, f.captured_at_ms) {
                    (Some(a), Some(b)) => (b - a).abs() > i64::from(options.max_gap_ms),
                    _ => false,
                };
                time_split || !joins_by_appearance(f, prev, running.as_ref(), threshold)
            }
        };
        if boundary {
            groups.push(Vec::new());
            running = None;
        }
        if let Some(g) = groups.last_mut() {
            g.push(f.id);
        }
        if let Some(feat) = &f.features {
            running = Some(match &running {
                Some(r) => features::blend(r, feat, RUNNING_WEIGHT),
                None => feat.clone(),
            });
        }
    }
    groups
}

fn joins_by_appearance(f: &DetectFrame, prev: &DetectFrame, running: Option<&SceneFeatures>, threshold: f32) -> bool {
    if threshold <= 0.0 {
        return true;
    }
    let Some(a) = &f.features else {
        // No appearance to compare: frames with a time join by time; frames without one
        // cannot be placed.
        return f.captured_at_ms.is_some();
    };
    // Similar to the scene so far, or to the frame just before (the same light seen in a
    // new framing usually recurs in consecutive frames).
    let to_group = running.map(|g| features::similarity(a, g));
    let to_prev = prev.features.as_ref().map(|p| features::similarity(a, p));
    match (to_group, to_prev) {
        (None, None) => f.captured_at_ms.is_some(),
        (g, p) => g.unwrap_or(0.0).max(p.unwrap_or(0.0)) >= threshold,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feat(level: u8, warm: bool) -> SceneFeatures {
        let px: Vec<u8> = if warm { [level, level / 2 + level / 4, level / 3] } else { [level, level, level] }
            .into_iter()
            .cycle()
            .take(32 * 32 * 3)
            .collect();
        features::from_rgb(&px, 32, 32)
    }

    fn frame(id: i64, folder: i64, t: Option<i64>, burst: Option<i64>, f: Option<SceneFeatures>) -> DetectFrame {
        DetectFrame {
            id,
            folder_id: folder,
            captured_at_ms: t,
            file_name: format!("{id:04}.arw"),
            burst_group_id: burst,
            preview_path: None,
            features: f,
        }
    }

    fn opts(max_gap_ms: u32, similarity: f32) -> SceneDetectOptions {
        SceneDetectOptions { max_gap_ms, similarity, replace_manual: false }
    }

    fn check_partition(frames: &[DetectFrame], groups: &[Vec<ImageId>]) {
        let flat: Vec<ImageId> = groups.iter().flatten().copied().collect();
        assert_eq!(flat, frames.iter().map(|f| f.id).collect::<Vec<_>>(), "every frame once, in order");
        assert!(groups.iter().all(|g| !g.is_empty()));
    }

    #[test]
    fn splits_on_time_gaps_and_folders() {
        let grey = Some(feat(128, false));
        let frames = vec![
            frame(1, 1, Some(0), None, grey.clone()),
            frame(2, 1, Some(10_000), None, grey.clone()),
            frame(3, 1, Some(200_000), None, grey.clone()), // gap 190 s
            frame(4, 1, Some(201_000), None, grey.clone()),
            frame(5, 2, Some(202_000), None, grey.clone()), // other folder
        ];
        let g = group(&frames, &opts(120_000, 0.7));
        check_partition(&frames, &g);
        assert_eq!(g, vec![vec![1, 2], vec![3, 4], vec![5]]);
        // Gap exactly max_gap_ms: same scene.
        let g = group(&frames[..2], &opts(10_000, 0.0));
        assert_eq!(g, vec![vec![1, 2]]);
        assert!(group(&[], &opts(1000, 0.5)).is_empty());
    }

    #[test]
    fn splits_on_appearance_unless_similarity_zero() {
        let (grey, warm, dark) = (feat(128, false), feat(200, true), feat(25, false));
        let frames = vec![
            frame(1, 1, Some(0), None, Some(grey.clone())),
            frame(2, 1, Some(1000), None, Some(grey.clone())),
            frame(3, 1, Some(2000), None, Some(warm.clone())),
            frame(4, 1, Some(3000), None, Some(warm)),
            frame(5, 1, Some(4000), None, Some(dark)),
            frame(6, 1, Some(5000), None, None), // no features: joins by time
        ];
        let g = group(&frames, &opts(120_000, 0.7));
        check_partition(&frames, &g);
        assert_eq!(g, vec![vec![1, 2], vec![3, 4], vec![5, 6]]);
        assert_eq!(group(&frames, &opts(120_000, 0.0)), vec![vec![1, 2, 3, 4, 5, 6]]);
    }

    #[test]
    fn bursts_are_never_split() {
        let (grey, warm) = (feat(128, false), feat(200, true));
        let frames = vec![
            frame(1, 1, Some(0), None, Some(grey.clone())),
            frame(2, 1, Some(500), Some(7), Some(grey.clone())),
            frame(3, 1, Some(1000), Some(7), Some(warm.clone())), // looks different, same burst
            frame(4, 1, Some(1200), None, Some(warm.clone())),    // interleaved non-member
            frame(5, 1, Some(2900), Some(7), Some(warm.clone())), // gap 1.7 s > max_gap 1 s
            frame(6, 1, Some(9000), None, Some(warm)),
        ];
        let g = group(&frames, &opts(1000, 0.7));
        check_partition(&frames, &g);
        assert_eq!(g, vec![vec![1, 2, 3, 4, 5], vec![6]]);
    }

    #[test]
    fn frames_without_time_join_by_similarity_only() {
        let (grey, warm) = (feat(128, false), feat(200, true));
        let frames = vec![
            frame(1, 1, Some(0), None, Some(grey.clone())),
            frame(2, 1, None, None, Some(grey.clone())),
            frame(3, 1, None, None, Some(warm)),
            frame(4, 1, None, None, None),
        ];
        let g = group(&frames, &opts(120_000, 0.7));
        check_partition(&frames, &g);
        assert_eq!(g, vec![vec![1, 2], vec![3], vec![4]]);
        assert_eq!(group(&frames, &opts(120_000, 0.0)), vec![vec![1, 2, 3, 4]]);
    }
}
