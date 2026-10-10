//! Face identity (Phase 9, IPC v20): face embeddings and per-project clustering into people.
//!
//! Owned by vision-ml-dev. The architect fixed the surface used by `ml::selection`'s pipeline:
//! [`update_people`] and its [`IdentityOutcome`]. [`cluster_people`] / [`suggest_roles`] are the
//! suggested pure seam (deterministic tests on synthetic embeddings); internals are free.
//!
//! Contract (docs/architecture.md, "Target-count culling"):
//! - Faces = `image_analysis.faces_json` entries (`get_faces` order); embeddings are stored per
//!   (image, face index) with `db::target::upsert_embedding` (f32 LE BLOB + `dim`,
//!   L2-normalised, `model_version`, the face box it came from); stale ones (box moved /
//!   other model) are recomputed. Embed from the 2048 px preview like the analysis worker.
//! - Clusters are stored with `db::target::replace_people`; a cluster continuing an existing
//!   person sets `prior_id` (match by centroid) so person ids, and with them the user's
//!   answers (`people.user_role`), survive re-runs.
//! - Main subject: the most frequent pair of faces appearing together (frequency, size,
//!   centrality); Portrait: the single most frequent face. Other recurring people get
//!   `ask = true` ("Is this person important?"). Guests seen once or twice: `other`, no ask.
//! - Without a model (not installed / not chosen yet) return `people: None`: the stored people
//!   stay as they are and the selection runs without people.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use rusqlite::Connection;

use crate::db::target::{PersonDraft, StoredEmbedding};
use crate::ipc::error::AppResult;
use crate::ipc::types::{PersonId, PersonRole, ProjectId, ShootType};

/// What [`update_people`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IdentityOutcome {
    /// Model of the embeddings now stored; `None` = face identity unavailable.
    pub model_version: Option<String>,
    /// Faces embedded in this run.
    pub embedded: u32,
    /// The project's people (to store with `db::target::replace_people`); `None` = leave the
    /// stored people unchanged.
    pub people: Option<Vec<PersonDraft>>,
    /// User-facing note for the run message (e.g. "Face recognition is not available yet").
    pub message: Option<String>,
}

/// An existing person, for matching new clusters to old ids.
#[derive(Debug, Clone, PartialEq)]
pub struct PriorPerson {
    pub id: PersonId,
    pub centroid: Vec<f32>,
    pub user_role: Option<PersonRole>,
}

/// Embeds the project's faces that lack a current embedding and clusters them into people.
/// `progress(done, total)` may be called often (the caller throttles). Checks `cancel`
/// between images. Stub: face identity is not available yet.
pub fn update_people(
    _conn: &mut Connection,
    _project_id: ProjectId,
    _shoot_type: ShootType,
    _models_dir: &Path,
    _cancel: &AtomicBool,
    _progress: &mut dyn FnMut(u32, Option<u32>),
) -> AppResult<IdentityOutcome> {
    Ok(IdentityOutcome {
        model_version: None,
        embedded: 0,
        people: None,
        message: Some("Face recognition is not available yet".to_owned()),
    })
}

/// Clusters `faces` into people, continuing `prior` people where centroids match. Stub: no
/// clusters.
pub fn cluster_people(_faces: &[StoredEmbedding], _prior: &[PriorPerson]) -> Vec<PersonDraft> {
    Vec::new()
}

/// Sets `suggested_role` / `ask` of `people` (main pair or portrait subject, recurring people
/// to ask about). Stub: leaves them as they are.
pub fn suggest_roles(_people: &mut [PersonDraft], _shoot_type: ShootType) {}
