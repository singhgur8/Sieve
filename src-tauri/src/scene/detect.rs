//! Scene grouping (pure). Owned by vision-ml-dev.

use super::DetectFrame;
use crate::ipc::types::{ImageId, SceneDetectOptions};

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
    let _ = (frames, options);
    todo!("vision-ml-dev: time-gap + feature-similarity segmentation")
}
