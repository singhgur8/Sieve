//! Appearance features of previews for scene detection. Owned by vision-ml-dev.

use std::path::Path;

use super::{DetectFrame, Progress, SceneFeatures};
use crate::ipc::types::ImageId;

/// Contract. Features of the 2048 px preview JPEG at `preview_path` (orientation applied;
/// downsample first, e.g. ~256 px, it only needs global statistics). Errors are human-readable.
pub fn compute(preview_path: &Path) -> Result<SceneFeatures, String> {
    let _ = preview_path;
    todo!("vision-ml-dev: decode preview (raw::turbo), luma + Oklab a/b histograms, log-mean luma")
}

/// Contract. Blocking; parallel (rayon). Computes features for every frame with
/// `features == None` and a `preview_path`, fills them in and returns the newly computed ones
/// (for [`super::store::save_features`]). Failures leave `features = None` (detection then
/// groups that frame by time only). Calls `progress(done, total)` over the frames computed.
pub fn compute_missing(frames: &mut [DetectFrame], progress: Progress) -> Vec<(ImageId, SceneFeatures)> {
    let _ = (frames, progress);
    todo!("vision-ml-dev: rayon over frames missing features -> compute()")
}
