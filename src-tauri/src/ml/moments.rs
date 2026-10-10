//! Moments and shot types (Phase 9, IPC v20): frames of the same scene + people grouped across
//! the shoot, each classified couple / group / detail / candid / other.
//!
//! Owned by vision-ml-dev. The architect fixed the surface used by `ml::selection`'s pipeline:
//! [`detect_moments`] returning [`MomentPlan`]. [`group_moments`] / [`classify_shot`] are the
//! suggested pure seam; internals and the frame features are free to change.
//!
//! Contract (docs/architecture.md, "Target-count culling"):
//! - A moment = time gap + visual similarity (phash / scene features) + same people; moments
//!   may span bursts and scenes but never projects.
//! - Shot type per frame: group = >= 3 faces posed; couple = the main pair dominant (portrait:
//!   the subject); detail = no face, a sharp salient object; candid = faces not posed; else
//!   other. The moment's type is the dominant one.
//! - Also produce the per-frame signals the selection needs ("visible face" score: frontal,
//!   size, eyes; back-of-head / no-face penalty; detail focus on the salient object) in
//!   [`FrameSignals`].

use std::sync::atomic::AtomicBool;

use rusqlite::Connection;

use crate::db::target::MomentDraft;
use crate::ipc::error::AppResult;
use crate::ipc::types::{ImageId, PersonId, ProjectId, ShootType, ShotType};

/// Per-frame signals for the selection (extend freely).
#[derive(Debug, Clone, PartialEq)]
pub struct FrameSignals {
    pub image_id: ImageId,
    pub moment_key: Option<u32>,
    pub shot_type: ShotType,
    pub person_ids: Vec<PersonId>,
    /// 0..=1: a subject's face clearly visible (frontal, large enough, eyes open).
    pub visible_face: f32,
    /// Detail shots: focus is on the salient object.
    pub detail_in_focus: Option<bool>,
}

/// [`detect_moments`] result.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MomentPlan {
    pub moments: Vec<MomentDraft>,
    pub frames: Vec<FrameSignals>,
}

/// Groups the project's analysed photos into moments and classifies them, using the people
/// stored by `ml::identity` (`face_embeddings.person_id`, roles via `db::target::people_roles`).
/// Stub: no moments.
pub fn detect_moments(
    _conn: &Connection,
    _project_id: ProjectId,
    _shoot_type: ShootType,
    _cancel: &AtomicBool,
) -> AppResult<MomentPlan> {
    Ok(MomentPlan::default())
}

/// Input of [`group_moments`] (extend freely).
#[derive(Debug, Clone, PartialEq)]
pub struct MomentFrame {
    pub image_id: ImageId,
    pub captured_at_ms: Option<i64>,
    pub phash: Option<u64>,
    pub person_ids: Vec<PersonId>,
    pub face_count: u32,
}

/// Pure grouping of frames (capture order) into moments. Stub: no moments.
pub fn group_moments(_frames: &[MomentFrame]) -> Vec<MomentDraft> {
    Vec::new()
}

/// Pure shot-type classification of one frame. Stub: `other`.
pub fn classify_shot(_frame: &MomentFrame, _main: &[PersonId]) -> ShotType {
    ShotType::Other
}
