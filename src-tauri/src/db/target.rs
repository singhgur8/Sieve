//! Target-count culling storage (IPC v20, migration 0019): people, face embeddings, moments,
//! target runs and per-image selection, plus the user-side edits (swap / add / set choice /
//! undo), the suggestion overlay and the flag apply.
//!
//! Implemented by the architect (catalog plumbing only, no selection logic). The engines
//! (`ml::identity`, `ml::moments`, `ml::selection`, vision-ml-dev) produce [`PersonDraft`],
//! [`MomentDraft`] and [`SelectionDraft`] and store them through [`replace_people`] and
//! [`store_results`]; face embeddings go through [`upsert_embedding`] / [`project_embeddings`].
//!
//! Invariants:
//! - `quality_scores.suggested_pick` is the effective suggestion: [`overlay_sql`] of the
//!   selection row and `scored_pick` (the scorer's own). Every writer of either side calls
//!   [`overlay_suggestions`] / [`overlay_project`] (`ml::store::write_scored` does too).
//! - User decisions win: target edits and the user's own flags ([`note_user_flags`]) lock the
//!   row (`locked = 1`); [`store_results`] never replaces a locked row's choice; [`apply`]
//!   only changes unflagged photos and flags Sieve set (`pick_origin = 'auto'`).
//! - Target edits write the flags of the photos they touch as the user's (`pick` for the
//!   delivery set, a pick becomes `unflagged` outside it; origin `user`), at once (v20.1:
//!   keeping a delivered photo picks it too). [`apply`] flags the rest.
//! - v20.1: `covered_by` of every non-delivered row of a moment touched by an edit is
//!   recomputed from the frame similarities stored per run ([`store_similarities`]):
//!   the most similar delivered frame of the same moment (photos the user added included);
//!   without one, a still-delivered cover from another moment is kept, else cleared.
//! - v20.1: `origin` = who made the current choice (`engine` on store, `user` when a user edit
//!   changes the choice; a lock without a change keeps it).

use std::collections::{BTreeSet, HashMap};

use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::db::{now_ms, repo};
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::{
    Alternatives, ChoiceOrigin, CoveredBy, CullSnapshot, ImageId, ImageSelection, Moment, MomentId, NormRect,
    PeopleOverview, Person, PersonId, PersonRole, PickFlag, PickOrigin, ProjectId, ShootType, ShotType, ShotTypeCount,
    SimilarityTier, TargetApplyOptions, TargetApplyPlan, TargetApplyResult, TargetChoice, TargetCounts, TargetPile,
    TargetReason, TargetReasonKind, TargetRun, TargetRunSettings, TargetRunState, TargetSnapshot,
};

/// Faces shown per person (`Person.samples`).
pub const MAX_SAMPLES: usize = 6;

// ---------------------------------------------------------------------------
// Embedding storage format
// ---------------------------------------------------------------------------

/// `face_embeddings.embedding` / `people.centroid`: f32 little-endian.
pub fn encode_embedding(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// Inverse of [`encode_embedding`]; `None` when the blob is not `dim` f32 values.
pub fn decode_embedding(blob: &[u8], dim: usize) -> Option<Vec<f32>> {
    if dim == 0 || blob.len() != dim * 4 {
        return None;
    }
    Some(blob.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
}

/// One stored face embedding ([`project_embeddings`]).
#[derive(Debug, Clone, PartialEq)]
pub struct StoredEmbedding {
    pub image_id: ImageId,
    pub face_index: u32,
    pub model_version: String,
    pub bbox: NormRect,
    pub vector: Vec<f32>,
    pub quality: f32,
    pub person_id: Option<PersonId>,
}

/// Writes (or replaces) the embedding of face `face_index` of `image_id`. Empty or non-finite
/// vectors -> `invalid_argument`.
pub fn upsert_embedding(
    conn: &Connection,
    image_id: ImageId,
    face_index: u32,
    model_version: &str,
    bbox: &NormRect,
    vector: &[f32],
    quality: f32,
) -> AppResult<()> {
    if vector.is_empty() || vector.iter().any(|v| !v.is_finite()) || !quality.is_finite() {
        return Err(AppError::invalid("face embedding must be non-empty and finite"));
    }
    conn.prepare_cached(
        "INSERT INTO face_embeddings (image_id, face_index, model_version, dim, embedding, bbox_json, quality,
                                      person_id, computed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8)
         ON CONFLICT(image_id, face_index) DO UPDATE SET model_version = excluded.model_version,
             dim = excluded.dim, embedding = excluded.embedding, bbox_json = excluded.bbox_json,
             quality = excluded.quality, person_id = NULL, computed_at = excluded.computed_at",
    )?
    .execute(params![
        image_id,
        face_index,
        model_version,
        vector.len() as i64,
        encode_embedding(vector),
        serde_json::to_string(bbox)?,
        quality,
        now_ms()
    ])?;
    Ok(())
}

/// Every stored embedding of the project's photos (any model version), by image and face.
pub fn project_embeddings(conn: &Connection, project_id: ProjectId) -> AppResult<Vec<StoredEmbedding>> {
    let mut stmt = conn.prepare(
        "SELECT e.image_id, e.face_index, e.model_version, e.dim, e.embedding, e.bbox_json, e.quality, e.person_id
         FROM face_embeddings e JOIN images i ON i.id = e.image_id JOIN folders f ON f.id = i.folder_id
         WHERE f.project_id = ?1 ORDER BY e.image_id, e.face_index",
    )?;
    let rows = stmt.query_map([project_id], |r| {
        Ok((
            r.get::<_, ImageId>(0)?,
            r.get::<_, u32>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, Vec<u8>>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, f64>(6)?,
            r.get::<_, Option<PersonId>>(7)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (image_id, face_index, model_version, dim, blob, bbox, quality, person_id) = row?;
        let (Some(vector), Ok(bbox)) = (decode_embedding(&blob, dim as usize), serde_json::from_str(&bbox)) else {
            continue;
        };
        out.push(StoredEmbedding {
            image_id,
            face_index,
            model_version,
            bbox,
            vector,
            quality: quality as f32,
            person_id,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Engine output (written by the target-selection worker)
// ---------------------------------------------------------------------------

/// A person as clustered by `ml::identity`.
#[derive(Debug, Clone, PartialEq)]
pub struct PersonDraft {
    /// The existing person this cluster continues (keeps its id and the user's answer).
    pub prior_id: Option<PersonId>,
    pub suggested_role: PersonRole,
    pub ask: bool,
    /// Every face assigned to the person: (image, face index).
    pub faces: Vec<(ImageId, u32)>,
    /// Best faces first (<= [`MAX_SAMPLES`] are kept).
    pub samples: Vec<(ImageId, u32)>,
    /// Mean embedding (empty = none stored).
    pub centroid: Vec<f32>,
}

/// A moment as grouped by `ml::moments`.
#[derive(Debug, Clone, PartialEq)]
pub struct MomentDraft {
    /// Run-local key referenced by [`SelectionDraft::moment_key`].
    pub key: u32,
    pub shot_type: ShotType,
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
    pub representative_id: Option<ImageId>,
    pub person_ids: Vec<PersonId>,
}

/// One photo's selection as decided by `ml::selection`.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectionDraft {
    pub image_id: ImageId,
    pub choice: TargetChoice,
    pub moment_key: Option<u32>,
    pub shot_type: Option<ShotType>,
    pub alternative_of: Option<ImageId>,
    pub rank: Option<u32>,
    pub covered_by: Option<ImageId>,
    pub covered_similarity: Option<f32>,
    pub score: f32,
    pub reasons: Vec<TargetReason>,
}

/// A user decision the engine must keep ([`locked_choices`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedChoice {
    pub image_id: ImageId,
    pub choice: TargetChoice,
    pub alternative_of: Option<ImageId>,
    pub rank: Option<u32>,
}

/// Replaces the project's people with `drafts`. A draft with a `prior_id` of this project
/// keeps that row (id, `user_role`, `created_at`); other old people are deleted. Face
/// assignments (`face_embeddings.person_id`) of the project are rewritten from `faces`
/// (faces without a stored embedding are ignored). Returns the person id per draft.
pub fn replace_people(
    conn: &mut Connection,
    project_id: ProjectId,
    drafts: &[PersonDraft],
) -> AppResult<Vec<PersonId>> {
    let now = now_ms();
    let tx = conn.savepoint()?;
    let existing: BTreeSet<PersonId> = {
        let mut stmt = tx.prepare("SELECT id FROM people WHERE project_id = ?1")?;
        let ids = stmt.query_map([project_id], |r| r.get(0))?.collect::<Result<_, _>>()?;
        ids
    };
    tx.execute(
        "UPDATE face_embeddings SET person_id = NULL WHERE image_id IN
             (SELECT i.id FROM images i JOIN folders f ON f.id = i.folder_id WHERE f.project_id = ?1)",
        [project_id],
    )?;
    let mut ids = Vec::with_capacity(drafts.len());
    for d in drafts {
        let photos: BTreeSet<ImageId> = d.faces.iter().map(|f| f.0).collect();
        let samples: Vec<[i64; 2]> = d.samples.iter().take(MAX_SAMPLES).map(|&(i, f)| [i, f as i64]).collect();
        let samples = serde_json::to_string(&samples)?;
        let (centroid, dim) = if d.centroid.is_empty() {
            (None, None)
        } else {
            (Some(encode_embedding(&d.centroid)), Some(d.centroid.len() as i64))
        };
        let id = match d.prior_id.filter(|p| existing.contains(p) && !ids.contains(p)) {
            Some(id) => {
                tx.execute(
                    "UPDATE people SET suggested_role = ?2, ask = ?3, sample_faces_json = ?4, centroid = ?5, dim = ?6,
                         face_count = ?7, photo_count = ?8, updated_at = ?9 WHERE id = ?1",
                    params![
                        id,
                        d.suggested_role.as_str(),
                        d.ask,
                        samples,
                        centroid,
                        dim,
                        d.faces.len() as i64,
                        photos.len() as i64,
                        now
                    ],
                )?;
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO people (project_id, suggested_role, ask, sample_faces_json, centroid, dim, face_count,
                                         photo_count, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
                    params![
                        project_id,
                        d.suggested_role.as_str(),
                        d.ask,
                        samples,
                        centroid,
                        dim,
                        d.faces.len() as i64,
                        photos.len() as i64,
                        now
                    ],
                )?;
                tx.last_insert_rowid()
            }
        };
        {
            let mut assign =
                tx.prepare_cached("UPDATE face_embeddings SET person_id = ?3 WHERE image_id = ?1 AND face_index = ?2")?;
            for &(image, face) in &d.faces {
                assign.execute(params![image, face, id])?;
            }
        }
        ids.push(id);
    }
    for gone in existing.iter().filter(|e| !ids.contains(e)) {
        tx.execute("DELETE FROM people WHERE id = ?1", [gone])?;
    }
    tx.commit()?;
    Ok(ids)
}

/// Effective role of each person of the project (for the selection engine).
pub fn people_roles(conn: &Connection, project_id: ProjectId) -> AppResult<Vec<(PersonId, PersonRole)>> {
    let mut stmt =
        conn.prepare("SELECT id, COALESCE(user_role, suggested_role) FROM people WHERE project_id = ?1 ORDER BY id")?;
    let rows = stmt.query_map([project_id], |r| Ok((r.get(0)?, role_col(r, 1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The user's decisions in the project's selection (locked rows).
pub fn locked_choices(conn: &Connection, project_id: ProjectId) -> AppResult<Vec<LockedChoice>> {
    let mut stmt = conn.prepare(
        "SELECT image_id, choice, alternative_of, rank FROM target_selection
         WHERE project_id = ?1 AND locked = 1 ORDER BY image_id",
    )?;
    let rows = stmt.query_map([project_id], |r| {
        Ok(LockedChoice { image_id: r.get(0)?, choice: choice_col(r, 1)?, alternative_of: r.get(2)?, rank: r.get(3)? })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Replaces the stored frame similarities of the project's photos with `pairs` (`(a, b,
/// similarity)`, any order; pairs of the same moment, see `ml::selection::moment_similarities`).
/// Written by every run before [`store_results`]; [`refresh_covers`] reads them. Pairs naming
/// a photo outside the project, self pairs and non-finite values are skipped. Atomic.
pub fn store_similarities(
    conn: &mut Connection,
    project_id: ProjectId,
    pairs: &[(ImageId, ImageId, f32)],
) -> AppResult<()> {
    let tx = conn.savepoint()?;
    let members: BTreeSet<ImageId> = {
        let mut stmt =
            tx.prepare("SELECT i.id FROM images i JOIN folders f ON f.id = i.folder_id WHERE f.project_id = ?1")?;
        let ids = stmt.query_map([project_id], |r| r.get(0))?.collect::<Result<_, _>>()?;
        ids
    };
    tx.execute(
        "DELETE FROM target_similarity WHERE image_a IN
             (SELECT i.id FROM images i JOIN folders f ON f.id = i.folder_id WHERE f.project_id = ?1)",
        [project_id],
    )?;
    {
        let mut insert = tx.prepare_cached(
            "INSERT INTO target_similarity (image_a, image_b, similarity) VALUES (?1, ?2, ?3)
             ON CONFLICT(image_a, image_b) DO UPDATE SET similarity = excluded.similarity",
        )?;
        for &(a, b, sim) in pairs {
            if a == b || !sim.is_finite() || !members.contains(&a) || !members.contains(&b) {
                continue;
            }
            let sim = ((sim.clamp(0.0, 1.0) * 1000.0).round() / 1000.0) as f64;
            insert.execute(params![a.min(b), a.max(b), sim])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Stored similarity of two frames ([`store_similarities`]), if any.
fn pair_similarity(conn: &Connection, a: ImageId, b: ImageId) -> AppResult<Option<f32>> {
    Ok(conn
        .prepare_cached("SELECT similarity FROM target_similarity WHERE image_a = ?1 AND image_b = ?2")?
        .query_row([a.min(b), a.max(b)], |r| r.get::<_, f64>(0))
        .optional()?
        .map(|v| v as f32))
}

/// One row of a moment for [`refresh_covers`]: id, choice, cover, cover similarity, cover's choice.
type CoverRow = (ImageId, TargetChoice, Option<ImageId>, Option<f64>, Option<String>);

/// Recomputes `covered_by` / `covered_similarity` of every row of `moments` (see module docs):
/// delivered rows have none; a non-delivered row is covered by its most similar delivered
/// frame of the same moment (ties: lower id); without a stored similarity to any of them, a
/// cover that is still delivered is kept (another moment, or catalogs from before v20.1),
/// else cleared.
pub fn refresh_covers(conn: &Connection, moments: &BTreeSet<MomentId>) -> AppResult<()> {
    let mut rows_stmt = conn.prepare_cached(
        "SELECT s.image_id, s.choice, s.covered_by, s.covered_similarity, c.choice FROM target_selection s
         LEFT JOIN target_selection c ON c.image_id = s.covered_by WHERE s.moment_id = ?1 ORDER BY s.image_id",
    )?;
    let mut update = conn.prepare_cached(
        "UPDATE target_selection SET covered_by = ?2, covered_similarity = ?3 WHERE image_id = ?1
           AND (covered_by IS NOT ?2 OR covered_similarity IS NOT ?3)",
    )?;
    for &m in moments {
        let rows: Vec<CoverRow> = rows_stmt
            .query_map([m], |r| Ok((r.get(0)?, choice_col(r, 1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
            .collect::<Result<_, _>>()?;
        let delivered: Vec<ImageId> = rows.iter().filter(|r| r.1 == TargetChoice::Deliver).map(|r| r.0).collect();
        for (id, choice, cover, cover_sim, cover_choice) in &rows {
            let next: (Option<ImageId>, Option<f64>) = if *choice == TargetChoice::Deliver {
                (None, None)
            } else {
                let mut best: Option<(ImageId, f32)> = None;
                for &d in &delivered {
                    if let Some(sim) = pair_similarity(conn, *id, d)? {
                        if best.is_none_or(|(_, b)| sim > b) {
                            best = Some((d, sim));
                        }
                    }
                }
                match best {
                    Some((d, sim)) => (Some(d), Some(sim as f64)),
                    None if cover.is_some() && cover_choice.as_deref() == Some(TargetChoice::Deliver.as_str()) => {
                        (*cover, *cover_sim)
                    }
                    None => (None, None),
                }
            };
            update.execute(params![id, next.0, next.1])?;
        }
    }
    Ok(())
}

/// Moments of `ids` (rows without a moment are skipped).
fn moments_of(conn: &Connection, ids: impl IntoIterator<Item = ImageId>) -> AppResult<BTreeSet<MomentId>> {
    let mut stmt = conn.prepare_cached("SELECT moment_id FROM target_selection WHERE image_id = ?1")?;
    let mut out = BTreeSet::new();
    for id in ids {
        if let Some(Some(m)) = stmt.query_row([id], |r| r.get::<_, Option<MomentId>>(0)).optional()? {
            out.insert(m);
        }
    }
    Ok(out)
}

/// Stores a run's moments and selection: the project's moments are replaced; unlocked rows are
/// replaced by `drafts` (origin `engine`); a locked row keeps its choice / alternative / rank /
/// reasons / lock / origin and only takes the draft's moment, shot type and score (its cover is
/// then recomputed, [`refresh_covers`]). Drafts for photos outside the project
/// -> `invalid_argument`; a `moment_key` without a moment -> `invalid_argument`. Refreshes the
/// suggestion overlay of the whole project. Atomic.
pub fn store_results(
    conn: &mut Connection,
    project_id: ProjectId,
    moments: &[MomentDraft],
    drafts: &[SelectionDraft],
) -> AppResult<()> {
    let now = now_ms();
    let tx = conn.savepoint()?;
    tx.execute("DELETE FROM moments WHERE project_id = ?1", [project_id])?;
    let mut moment_ids: HashMap<u32, MomentId> = HashMap::new();
    for m in moments {
        tx.execute(
            "INSERT INTO moments (project_id, shot_type, started_at_ms, ended_at_ms, representative_id, person_ids_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                project_id,
                m.shot_type.as_str(),
                m.started_at_ms,
                m.ended_at_ms,
                m.representative_id,
                serde_json::to_string(&m.person_ids)?
            ],
        )?;
        moment_ids.insert(m.key, tx.last_insert_rowid());
    }
    let locked: BTreeSet<ImageId> = locked_choices(&tx, project_id)?.into_iter().map(|l| l.image_id).collect();
    tx.execute("DELETE FROM target_selection WHERE project_id = ?1 AND locked = 0", [project_id])?;
    {
        let mut in_project = tx.prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM images i JOIN folders f ON f.id = i.folder_id
                            WHERE i.id = ?1 AND f.project_id = ?2)",
        )?;
        let mut insert = tx.prepare_cached(
            "INSERT INTO target_selection (image_id, project_id, choice, moment_id, shot_type, alternative_of, rank,
                                           covered_by, covered_similarity, score, reasons_json, locked, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0, ?12)",
        )?;
        let mut update_locked = tx.prepare_cached(
            "UPDATE target_selection SET moment_id = ?2, shot_type = ?3, score = ?4, updated_at = ?5 WHERE image_id = ?1",
        )?;
        for d in drafts {
            if !in_project.query_row(params![d.image_id, project_id], |r| r.get::<_, bool>(0))? {
                return Err(AppError::invalid(format!("image {} is not in project {project_id}", d.image_id)));
            }
            let moment = match d.moment_key {
                Some(k) => Some(
                    *moment_ids
                        .get(&k)
                        .ok_or_else(|| AppError::invalid(format!("selection names unknown moment {k}")))?,
                ),
                None => None,
            };
            let shot = d.shot_type.map(|s| s.as_str());
            let score = finite_or_zero(d.score);
            if locked.contains(&d.image_id) {
                update_locked.execute(params![d.image_id, moment, shot, score, now])?;
            } else {
                insert.execute(params![
                    d.image_id,
                    project_id,
                    d.choice.as_str(),
                    moment,
                    shot,
                    d.alternative_of.filter(|_| d.choice == TargetChoice::Alternative),
                    d.rank.filter(|_| d.choice == TargetChoice::Alternative),
                    d.covered_by,
                    d.covered_similarity.map(finite_or_zero),
                    score,
                    serde_json::to_string(&d.reasons)?,
                    now
                ])?;
            }
        }
    }
    // Locked rows kept their cover from before the run: recompute their moments.
    let locked_moments = moments_of(&tx, locked.iter().copied())?;
    refresh_covers(&tx, &locked_moments)?;
    overlay_project(&tx, project_id)?;
    tx.commit()?;
    Ok(())
}

fn finite_or_zero(v: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// Runs
// ---------------------------------------------------------------------------

fn ensure_project(conn: &Connection, project_id: ProjectId) -> AppResult<()> {
    let ok: bool =
        conn.query_row("SELECT EXISTS (SELECT 1 FROM projects WHERE id = ?1)", [project_id], |r| r.get(0))?;
    if ok {
        Ok(())
    } else {
        Err(AppError::not_found(format!("project {project_id}")))
    }
}

/// `settings` with `shootType` resolved to the project's when `null`. Unknown project ->
/// `not_found`; invalid count -> `invalid_argument`.
pub fn resolve_settings(
    conn: &Connection,
    project_id: ProjectId,
    settings: &TargetRunSettings,
) -> AppResult<TargetRunSettings> {
    settings.validate().map_err(AppError::invalid)?;
    let shoot: Option<String> =
        conn.query_row("SELECT shoot_type FROM projects WHERE id = ?1", [project_id], |r| r.get(0)).optional()?;
    let shoot = shoot.ok_or_else(|| AppError::not_found(format!("project {project_id}")))?;
    Ok(TargetRunSettings {
        target_count: settings.target_count,
        shoot_type: Some(settings.shoot_type.or_else(|| ShootType::parse(&shoot)).unwrap_or(ShootType::General)),
    })
}

/// Marks the project's run `running` with `settings` (resolved). Keeps `applied_at`.
pub fn begin_run(conn: &Connection, project_id: ProjectId, settings: &TargetRunSettings) -> AppResult<()> {
    ensure_project(conn, project_id)?;
    let shoot = settings.shoot_type.unwrap_or(ShootType::General);
    conn.execute(
        "INSERT INTO target_runs (project_id, target_count, shoot_type, state, message, started_at)
         VALUES (?1, ?2, ?3, 'running', NULL, ?4)
         ON CONFLICT(project_id) DO UPDATE SET target_count = excluded.target_count,
             shoot_type = excluded.shoot_type, state = 'running', message = NULL,
             started_at = excluded.started_at, finished_at = NULL",
        params![project_id, settings.target_count, shoot.as_str(), now_ms()],
    )?;
    Ok(())
}

/// Ends the project's run with `state` and a user-facing `message`; `model_version` replaces
/// the stored one when given (a finished run).
pub fn finish_run(
    conn: &Connection,
    project_id: ProjectId,
    state: TargetRunState,
    message: Option<&str>,
    model_version: Option<&str>,
) -> AppResult<()> {
    conn.execute(
        "UPDATE target_runs SET state = ?2, message = ?3, model_version = COALESCE(?4, model_version), finished_at = ?5
         WHERE project_id = ?1",
        params![project_id, state.as_str(), message, model_version, now_ms()],
    )?;
    Ok(())
}

/// The project's latest run, `null` if never run. A `running` row while no worker runs
/// (`worker_running = false`, e.g. the app quit mid-run) reports `cancelled`. Unknown
/// project -> `not_found`.
pub fn get_run(conn: &Connection, project_id: ProjectId, worker_running: bool) -> AppResult<Option<TargetRun>> {
    ensure_project(conn, project_id)?;
    let row = conn
        .query_row(
            "SELECT target_count, shoot_type, state, message, model_version, started_at, finished_at, applied_at
             FROM target_runs WHERE project_id = ?1",
            [project_id],
            |r| {
                Ok((
                    r.get::<_, u32>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, Option<i64>>(6)?,
                    r.get::<_, Option<i64>>(7)?,
                ))
            },
        )
        .optional()?;
    let Some((target_count, shoot, state, mut message, model_version, started, finished, applied)) = row else {
        return Ok(None);
    };
    let mut state = TargetRunState::parse(&state).unwrap_or(TargetRunState::Failed);
    if state == TargetRunState::Running && !worker_running {
        state = TargetRunState::Cancelled;
        message.get_or_insert_with(|| "Stopped before it finished".to_owned());
    }
    Ok(Some(TargetRun {
        project_id,
        settings: TargetRunSettings { target_count, shoot_type: ShootType::parse(&shoot) },
        state,
        started_at_ms: started,
        finished_at_ms: finished,
        applied_at_ms: applied,
        message,
        model_version,
        counts: counts(conn, project_id)?,
        people_questions: open_questions(conn, project_id)?.len() as u32,
    }))
}

/// Counts over the project's selection rows.
pub fn counts(conn: &Connection, project_id: ProjectId) -> AppResult<TargetCounts> {
    let mut c = TargetCounts::default();
    let mut stmt = conn.prepare_cached(
        "SELECT choice, COUNT(*), COALESCE(SUM(locked), 0) FROM target_selection WHERE project_id = ?1 GROUP BY choice",
    )?;
    let rows = stmt.query_map([project_id], |r| Ok((choice_col(r, 0)?, r.get::<_, u32>(1)?, r.get::<_, u32>(2)?)))?;
    for row in rows {
        let (choice, n, locked) = row?;
        c.total += n;
        c.locked += locked;
        match choice {
            TargetChoice::Deliver => c.deliver = n,
            TargetChoice::Alternative => c.alternative = n,
            TargetChoice::NotSure => c.not_sure = n,
            TargetChoice::SetAside => c.set_aside = n,
        }
    }
    let mut stmt = conn.prepare_cached(
        "SELECT shot_type, COUNT(*), COALESCE(SUM(choice = 'deliver'), 0) FROM target_selection
         WHERE project_id = ?1 AND shot_type IS NOT NULL GROUP BY shot_type",
    )?;
    let mut per: Vec<ShotTypeCount> = stmt
        .query_map([project_id], |r| {
            Ok(ShotTypeCount { shot_type: shot_col(r, 0)?, total: r.get(1)?, deliver: r.get(2)? })
        })?
        .collect::<Result<_, _>>()?;
    per.sort_by_key(|s| ShotType::ALL.iter().position(|t| *t == s.shot_type));
    c.per_shot_type = per;
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT pile, COUNT(*) FROM (SELECT ({PILE_SQL}) AS pile FROM target_selection s WHERE s.project_id = ?1)
         WHERE pile IS NOT NULL GROUP BY pile"
    ))?;
    let rows = stmt.query_map([project_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?)))?;
    for row in rows {
        let (pile, n) = row?;
        match TargetPile::parse(&pile) {
            Some(TargetPile::NotSure) => c.piles.not_sure = n,
            Some(TargetPile::Similar) => c.piles.similar = n,
            Some(TargetPile::Weaker) => c.piles.weaker = n,
            Some(TargetPile::Defects) => c.piles.defects = n,
            None => {}
        }
    }
    Ok(c)
}

/// SQL expression of a selection row's second-look pile ([`TargetPile`]; row alias `s`).
pub const PILE_SQL: &str = "CASE WHEN s.choice = 'not_sure' THEN 'not_sure'
        WHEN s.choice <> 'set_aside' THEN NULL
        WHEN (SELECT pick FROM images WHERE id = s.image_id) = 'reject'
          OR (SELECT suggested_pick FROM quality_scores WHERE image_id = s.image_id) = 'reject' THEN 'defects'
        WHEN json_extract(s.reasons_json, '$[0].kind') IN ('near_duplicate', 'not_best_of_setup')
          OR (s.covered_by IS NOT NULL AND s.covered_similarity >= 0.7) THEN 'similar'
        ELSE 'weaker' END";

// ---------------------------------------------------------------------------
// People / moments reads
// ---------------------------------------------------------------------------

fn role_col(r: &Row, idx: usize) -> rusqlite::Result<PersonRole> {
    Ok(PersonRole::parse(&r.get::<_, String>(idx)?).unwrap_or(PersonRole::Unknown))
}

fn choice_col(r: &Row, idx: usize) -> rusqlite::Result<TargetChoice> {
    Ok(TargetChoice::parse(&r.get::<_, String>(idx)?).unwrap_or(TargetChoice::NotSure))
}

fn origin_col(r: &Row, idx: usize) -> rusqlite::Result<ChoiceOrigin> {
    Ok(ChoiceOrigin::parse(&r.get::<_, String>(idx)?).unwrap_or(ChoiceOrigin::Engine))
}

fn shot_col(r: &Row, idx: usize) -> rusqlite::Result<ShotType> {
    Ok(ShotType::parse(&r.get::<_, String>(idx)?).unwrap_or(ShotType::Other))
}

/// A padded square (in pixels) crop around `bbox`, inside the frame: 1.8x the face's larger
/// side, shifted to stay inside, shrunk only when the frame is smaller.
pub fn face_crop(bbox: &NormRect, aspect: f32) -> NormRect {
    let a = if aspect.is_finite() && aspect > 0.0 { aspect } else { 1.5 };
    let side = (bbox.width * a).max(bbox.height) * 1.8;
    let side = side.clamp(0.01, a.min(1.0));
    let cx = (bbox.x + bbox.width / 2.0) * a;
    let cy = bbox.y + bbox.height / 2.0;
    let left = (cx - side / 2.0).clamp(0.0, a - side);
    let top = (cy - side / 2.0).clamp(0.0, 1.0 - side);
    NormRect { x: left / a, y: top, width: side / a, height: side }
}

fn person_from_row(r: &Row) -> rusqlite::Result<(Person, String)> {
    let user_role: Option<String> = r.get(3)?;
    let suggested = role_col(r, 2)?;
    let person = Person {
        id: r.get(0)?,
        project_id: r.get(1)?,
        role: user_role.as_deref().and_then(PersonRole::parse).unwrap_or(suggested),
        role_confirmed: user_role.is_some(),
        suggested_role: suggested,
        ask: r.get(4)?,
        photo_count: r.get(6)?,
        face_count: r.get(7)?,
        samples: Vec::new(),
    };
    Ok((person, r.get(5)?))
}

const PERSON_COLS: &str =
    "id, project_id, suggested_role, user_role, ask, sample_faces_json, photo_count, face_count FROM people";

/// Resolves stored `[[imageId, faceIndex]]` samples to [`FaceSample`]s (faces no longer
/// detected are skipped).
fn samples(conn: &Connection, json: &str) -> AppResult<Vec<crate::ipc::types::FaceSample>> {
    let refs: Vec<(ImageId, u32)> = serde_json::from_str(json).unwrap_or_default();
    let mut out = Vec::new();
    let mut stmt = conn.prepare_cached(
        "SELECT a.faces_json, t.preview_path, t.width, t.height FROM images i
         LEFT JOIN image_analysis a ON a.image_id = i.id LEFT JOIN thumbnails t ON t.image_id = i.id
         WHERE i.id = ?1",
    )?;
    for (image_id, face_index) in refs.into_iter().take(MAX_SAMPLES) {
        let row = stmt
            .query_row([image_id], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<f64>>(2)?,
                    r.get::<_, Option<f64>>(3)?,
                ))
            })
            .optional()?;
        let Some((faces, preview, w, h)) = row else { continue };
        let faces: Vec<crate::ipc::types::FaceInfo> =
            faces.and_then(|f| serde_json::from_str(&f).ok()).unwrap_or_default();
        let Some(face) = faces.get(face_index as usize) else { continue };
        let aspect = match (w, h) {
            (Some(w), Some(h)) if w > 0.0 && h > 0.0 => (w / h) as f32,
            _ => 1.5,
        };
        out.push(crate::ipc::types::FaceSample {
            image_id,
            face_index,
            bbox: face.bbox,
            crop: face_crop(&face.bbox, aspect),
            image_aspect: aspect,
            preview_path: preview,
        });
    }
    Ok(out)
}

/// One person; unknown -> `not_found`.
pub fn person(conn: &Connection, id: PersonId) -> AppResult<Person> {
    let row = conn.query_row(&format!("SELECT {PERSON_COLS} WHERE id = ?1"), [id], person_from_row).optional()?;
    let (mut p, json) = row.ok_or_else(|| AppError::not_found(format!("person {id}")))?;
    p.samples = samples(conn, &json)?;
    Ok(p)
}

fn open_questions(conn: &Connection, project_id: ProjectId) -> AppResult<Vec<PersonId>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id FROM people WHERE project_id = ?1 AND ask = 1 AND user_role IS NULL
         ORDER BY photo_count DESC, id",
    )?;
    let ids = stmt.query_map([project_id], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(ids)
}

/// `list_people`. Unknown project -> `not_found`.
pub fn list_people(conn: &Connection, project_id: ProjectId) -> AppResult<PeopleOverview> {
    ensure_project(conn, project_id)?;
    let mut stmt = conn.prepare(&format!("SELECT {PERSON_COLS} WHERE project_id = ?1"))?;
    let rows: Vec<(Person, String)> = stmt.query_map([project_id], person_from_row)?.collect::<Result<_, _>>()?;
    let mut people = Vec::with_capacity(rows.len());
    for (mut p, json) in rows {
        p.samples = samples(conn, &json)?;
        people.push(p);
    }
    let order = |r: PersonRole| match r {
        PersonRole::Main => 0,
        PersonRole::Important => 1,
        _ => 2,
    };
    people.sort_by(|a, b| {
        order(a.role).cmp(&order(b.role)).then(b.photo_count.cmp(&a.photo_count)).then(a.id.cmp(&b.id))
    });
    let model_version: Option<String> = conn
        .query_row(
            "SELECT e.model_version FROM face_embeddings e JOIN images i ON i.id = e.image_id
             JOIN folders f ON f.id = i.folder_id WHERE f.project_id = ?1
             ORDER BY e.computed_at DESC LIMIT 1",
            [project_id],
            |r| r.get(0),
        )
        .optional()?;
    let message = match (&model_version, people.is_empty()) {
        (None, _) => Some("Face recognition is not available yet".to_owned()),
        (Some(_), true) => Some("No recurring people found".to_owned()),
        _ => None,
    };
    Ok(PeopleOverview { project_id, people, questions: open_questions(conn, project_id)?, model_version, message })
}

/// Sets (or, with `None`, clears) the user's answer for a person. Unknown -> `not_found`.
pub fn set_person_role(conn: &Connection, id: PersonId, role: Option<PersonRole>) -> AppResult<Person> {
    let n = conn.execute(
        "UPDATE people SET user_role = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, role.map(|r| r.as_str()), now_ms()],
    )?;
    if n == 0 {
        return Err(AppError::not_found(format!("person {id}")));
    }
    person(conn, id)
}

/// `list_moments`: capture order. Unknown project -> `not_found`.
pub fn list_moments(conn: &Connection, project_id: ProjectId) -> AppResult<Vec<Moment>> {
    ensure_project(conn, project_id)?;
    let mut stmt = conn.prepare(
        "SELECT id, shot_type, started_at_ms, ended_at_ms, representative_id, person_ids_json FROM moments
         WHERE project_id = ?1 ORDER BY started_at_ms IS NULL, started_at_ms, id",
    )?;
    let mut moments: Vec<Moment> = stmt
        .query_map([project_id], |r| {
            Ok(Moment {
                id: r.get(0)?,
                project_id,
                shot_type: shot_col(r, 1)?,
                started_at_ms: r.get(2)?,
                ended_at_ms: r.get(3)?,
                representative_id: r.get(4)?,
                person_ids: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or_default(),
                image_ids: Vec::new(),
                delivered_ids: Vec::new(),
                user_delivered_ids: Vec::new(),
            })
        })?
        .collect::<Result<_, _>>()?;
    let index: HashMap<MomentId, usize> = moments.iter().enumerate().map(|(i, m)| (m.id, i)).collect();
    let mut stmt = conn.prepare(
        "SELECT s.moment_id, s.image_id, s.choice, s.origin FROM target_selection s JOIN images i ON i.id = s.image_id
         WHERE s.project_id = ?1 AND s.moment_id IS NOT NULL
         ORDER BY i.captured_at_ms IS NULL, i.captured_at_ms, i.id",
    )?;
    let rows = stmt.query_map([project_id], |r| {
        Ok((r.get::<_, MomentId>(0)?, r.get::<_, ImageId>(1)?, choice_col(r, 2)?, origin_col(r, 3)?))
    })?;
    for row in rows {
        let (m, image, choice, origin) = row?;
        if let Some(&i) = index.get(&m) {
            moments[i].image_ids.push(image);
            if choice == TargetChoice::Deliver {
                moments[i].delivered_ids.push(image);
                if origin == ChoiceOrigin::User {
                    moments[i].user_delivered_ids.push(image);
                }
            }
        }
    }
    moments.retain(|m| !m.image_ids.is_empty());
    Ok(moments)
}

// ---------------------------------------------------------------------------
// Selection reads
// ---------------------------------------------------------------------------

// Column 13 is [`PILE_SQL`], spliced in by [`selection_sql`].
const SELECTION_SQL: &str = "SELECT s.image_id, s.choice, s.moment_id, s.shot_type, s.alternative_of, s.rank,
        s.covered_by, s.covered_similarity, s.score, s.reasons_json, s.locked,
        (SELECT json_group_array(DISTINCT e.person_id) FROM face_embeddings e
          WHERE e.image_id = s.image_id AND e.person_id IS NOT NULL),
        s.origin, (#PILE#), (SELECT c.moment_id FROM target_selection c WHERE c.image_id = s.covered_by)
     FROM target_selection s";

/// [`SELECTION_SQL`] followed by `tail` (a `WHERE` / `ORDER BY`).
fn selection_sql(tail: &str) -> String {
    format!("{} {tail}", SELECTION_SQL.replace("#PILE#", PILE_SQL))
}

fn selection_from_row(r: &Row) -> rusqlite::Result<ImageSelection> {
    let mut person_ids: Vec<PersonId> = serde_json::from_str(&r.get::<_, String>(11)?).unwrap_or_default();
    person_ids.sort_unstable();
    let moment_id: Option<MomentId> = r.get(2)?;
    let covered_by: Option<ImageId> = r.get(6)?;
    let covered_similarity = r.get::<_, Option<f64>>(7)?.map(|v| v as f32);
    let cover_moment: Option<MomentId> = r.get(14)?;
    let covered_tier = covered_by.map(|_| {
        SimilarityTier::of(covered_similarity.unwrap_or(0.0), moment_id.is_some() && moment_id == cover_moment)
    });
    Ok(ImageSelection {
        image_id: r.get(0)?,
        choice: choice_col(r, 1)?,
        moment_id,
        shot_type: r.get::<_, Option<String>>(3)?.as_deref().and_then(ShotType::parse),
        alternative_of: r.get(4)?,
        rank: r.get(5)?,
        covered_by,
        covered_similarity,
        covered_tier,
        score: r.get::<_, f64>(8)? as f32,
        reasons: serde_json::from_str(&r.get::<_, String>(9)?).unwrap_or_default(),
        locked: r.get(10)?,
        origin: origin_col(r, 12)?,
        pile: r.get::<_, Option<String>>(13)?.as_deref().and_then(TargetPile::parse),
        person_ids,
    })
}

/// The selection row of `id`, if any.
pub fn selection(conn: &Connection, id: ImageId) -> AppResult<Option<ImageSelection>> {
    Ok(conn.prepare_cached(&selection_sql("WHERE s.image_id = ?1"))?.query_row([id], selection_from_row).optional()?)
}

/// `get_image_selections`: rows of `ids` in the given order; ids without a row are omitted
/// (unknown images -> `not_found`).
pub fn selections(conn: &Connection, ids: &[ImageId]) -> AppResult<Vec<ImageSelection>> {
    let mut out = Vec::with_capacity(ids.len());
    for &id in ids {
        match selection(conn, id)? {
            Some(s) => out.push(s),
            None => ensure_image(conn, id)?,
        }
    }
    Ok(out)
}

fn ensure_image(conn: &Connection, id: ImageId) -> AppResult<()> {
    let ok: bool = conn.query_row("SELECT EXISTS (SELECT 1 FROM images WHERE id = ?1)", [id], |r| r.get(0))?;
    if ok {
        Ok(())
    } else {
        Err(AppError::not_found(format!("image {id}")))
    }
}

fn alternatives_of(conn: &Connection, delivered: ImageId) -> AppResult<Vec<ImageSelection>> {
    let mut stmt = conn.prepare_cached(&selection_sql(
        "WHERE s.alternative_of = ?1 AND s.choice = 'alternative' ORDER BY s.rank IS NULL, s.rank, s.image_id",
    ))?;
    let rows = stmt.query_map([delivered], selection_from_row)?.collect::<Result<_, _>>()?;
    Ok(rows)
}

/// `get_alternatives`. Unknown image -> `not_found`.
pub fn alternatives(conn: &Connection, id: ImageId) -> AppResult<Alternatives> {
    ensure_image(conn, id)?;
    let delivered = match selection(conn, id)? {
        Some(s) if s.choice == TargetChoice::Deliver => Some(s),
        Some(ImageSelection { choice: TargetChoice::Alternative, alternative_of: Some(p), .. }) => {
            selection(conn, p)?.filter(|s| s.choice == TargetChoice::Deliver)
        }
        _ => None,
    };
    let alternatives = match &delivered {
        Some(d) => alternatives_of(conn, d.image_id)?,
        None => Vec::new(),
    };
    Ok(Alternatives { image_id: id, delivered, alternatives })
}

fn file_stem(conn: &Connection, id: ImageId) -> AppResult<String> {
    let name: Option<String> =
        conn.query_row("SELECT file_name FROM images WHERE id = ?1", [id], |r| r.get(0)).optional()?;
    let name = name.unwrap_or_else(|| format!("photo {id}"));
    Ok(match name.rfind('.') {
        Some(i) if i > 0 => name[..i].to_owned(),
        _ => name,
    })
}

/// `get_covered_by`: `null` for delivered photos, photos without a row or without a delivered
/// similar photo. Unknown image -> `not_found`.
pub fn covered_by(conn: &Connection, id: ImageId) -> AppResult<Option<CoveredBy>> {
    ensure_image(conn, id)?;
    let Some(s) = selection(conn, id)? else { return Ok(None) };
    let Some(cover) = s.covered_by.filter(|_| s.choice != TargetChoice::Deliver) else { return Ok(None) };
    let Some(c) = selection(conn, cover)?.filter(|c| c.choice == TargetChoice::Deliver) else { return Ok(None) };
    let similarity = s.covered_similarity.unwrap_or(0.0);
    let same_moment = s.moment_id.is_some() && s.moment_id == c.moment_id;
    let tier = SimilarityTier::of(similarity, same_moment);
    let name = file_stem(conn, cover)?;
    Ok(Some(CoveredBy {
        image_id: id,
        covered_by_id: cover,
        text: covered_text(tier, &name, c.origin),
        covered_by_name: name,
        similarity,
        tier,
        same_moment,
        covered_by_origin: c.origin,
    }))
}

/// `CoveredBy.text`: "Almost identical to DSC0412 (kept)", "Similar to …", "Same moment as …",
/// "Looks like DSC0412 (kept, another moment)"; "kept" -> "you added it" for the user's photos.
pub fn covered_text(tier: SimilarityTier, name: &str, origin: ChoiceOrigin) -> String {
    let who = match origin {
        ChoiceOrigin::User => "you added it",
        ChoiceOrigin::Engine => "kept",
    };
    match tier {
        SimilarityTier::NearIdentical => format!("Almost identical to {name} ({who})"),
        SimilarityTier::VerySimilar => format!("Similar to {name} ({who})"),
        SimilarityTier::SameMoment => format!("Same moment as {name} ({who})"),
        SimilarityTier::AnotherMoment => format!("Looks like {name} ({who}, another moment)"),
    }
}

// ---------------------------------------------------------------------------
// Suggestion overlay + apply
// ---------------------------------------------------------------------------

/// SQL expression of the effective suggestion for a `quality_scores` row (see module docs).
pub const OVERLAY_SQL: &str =
    "CASE (SELECT t.choice FROM target_selection t WHERE t.image_id = quality_scores.image_id)
        WHEN 'deliver' THEN 'pick'
        WHEN 'alternative' THEN (CASE WHEN scored_pick = 'pick' THEN 'unflagged' ELSE scored_pick END)
        WHEN 'not_sure' THEN (CASE WHEN scored_pick = 'pick' THEN 'unflagged' ELSE scored_pick END)
        WHEN 'set_aside' THEN (CASE WHEN scored_pick = 'pick' THEN 'unflagged' ELSE scored_pick END)
        ELSE scored_pick END";

/// The effective suggestion of one image ([`OVERLAY_SQL`] in Rust).
pub fn overlay_sql(choice: Option<TargetChoice>, scored: PickFlag) -> PickFlag {
    match (choice, scored) {
        (None, s) => s,
        (Some(TargetChoice::Deliver), _) => PickFlag::Pick,
        (Some(_), PickFlag::Pick) => PickFlag::Unflagged,
        (Some(_), s) => s,
    }
}

/// Recomputes `suggested_pick` of `ids` from their selection row and `scored_pick`.
pub fn overlay_suggestions(conn: &Connection, ids: &[ImageId]) -> AppResult<()> {
    let mut stmt =
        conn.prepare_cached(&format!("UPDATE quality_scores SET suggested_pick = {OVERLAY_SQL} WHERE image_id = ?1"))?;
    for &id in ids {
        stmt.execute([id])?;
    }
    Ok(())
}

/// [`overlay_suggestions`] for every image of the project.
pub fn overlay_project(conn: &Connection, project_id: ProjectId) -> AppResult<()> {
    conn.execute(
        &format!(
            "UPDATE quality_scores SET suggested_pick = {OVERLAY_SQL} WHERE image_id IN
                 (SELECT i.id FROM images i JOIN folders f ON f.id = i.folder_id WHERE f.project_id = ?1)"
        ),
        [project_id],
    )?;
    Ok(())
}

/// What [`apply`] does to one photo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApplyStep {
    /// The user's flag: never touched.
    UserFlag,
    Unchanged,
    /// Write this flag (origin `auto`).
    Write(PickFlag),
}

/// Per photo of the project's selection: (image, step, would be rejected with `rejects: true`).
fn apply_steps(
    conn: &Connection,
    project_id: ProjectId,
    opts: TargetApplyOptions,
) -> AppResult<Vec<(ImageId, ApplyStep, bool)>> {
    let mut stmt = conn.prepare(
        "SELECT i.id, i.pick, i.pick_origin, q.suggested_pick FROM target_selection s
         JOIN images i ON i.id = s.image_id LEFT JOIN quality_scores q ON q.image_id = i.id
         WHERE s.project_id = ?1 ORDER BY i.id",
    )?;
    let rows = stmt.query_map([project_id], |r| {
        Ok((
            r.get::<_, ImageId>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, pick, origin, suggested) = row?;
        let pick = PickFlag::parse(&pick).unwrap_or(PickFlag::Unflagged);
        // Same rule as `ImageQuery.pickOrigin = user`: flagged and not by Sieve.
        if pick != PickFlag::Unflagged && origin.as_deref() != Some(PickOrigin::Auto.as_str()) {
            out.push((id, ApplyStep::UserFlag, false));
            continue;
        }
        let Some(suggested) = suggested.as_deref().and_then(PickFlag::parse) else {
            out.push((id, ApplyStep::Unchanged, false));
            continue;
        };
        let rejectable = suggested == PickFlag::Reject && pick != PickFlag::Reject;
        let next = match suggested {
            PickFlag::Reject if !opts.rejects && pick != PickFlag::Reject => PickFlag::Unflagged,
            s => s,
        };
        let step = if next == pick { ApplyStep::Unchanged } else { ApplyStep::Write(next) };
        out.push((id, step, rejectable));
    }
    Ok(out)
}

fn tally(steps: &[(ImageId, ApplyStep, bool)]) -> TargetApplyPlan {
    let mut p = TargetApplyPlan { total: steps.len() as u32, ..TargetApplyPlan::default() };
    for &(_, step, rejectable) in steps {
        if rejectable {
            p.rejectable += 1;
        }
        match step {
            ApplyStep::UserFlag => p.user_flagged += 1,
            ApplyStep::Unchanged => p.unchanged += 1,
            ApplyStep::Write(PickFlag::Pick) => p.picks += 1,
            ApplyStep::Write(PickFlag::Reject) => p.rejects += 1,
            ApplyStep::Write(PickFlag::Unflagged) => p.unflags += 1,
        }
    }
    p
}

/// `plan_target_apply`: what [`apply`] with `opts` would do now (nothing is written).
/// Unknown project -> `not_found`.
pub fn plan_apply(conn: &Connection, project_id: ProjectId, opts: TargetApplyOptions) -> AppResult<TargetApplyPlan> {
    ensure_project(conn, project_id)?;
    Ok(tally(&apply_steps(conn, project_id, opts)?))
}

/// `apply_target_selection`: writes the effective suggestion to the flags of the project's
/// photos that have a selection row and are unflagged or flagged by Sieve (`pick_origin =
/// 'auto'`); changed flags get origin `auto`. With `rejects: false` a `reject` suggestion is
/// not written (a Sieve pick on such a photo is cleared, a Sieve reject stays). The user's
/// flags and all stars are never touched. Returns the counts (same as [`plan_apply`]) and the
/// flags before (for the Cull undo stack). Stamps `target_runs.applied_at`. Unknown project
/// -> `not_found`.
pub fn apply(conn: &mut Connection, project_id: ProjectId, opts: TargetApplyOptions) -> AppResult<TargetApplyResult> {
    ensure_project(conn, project_id)?;
    let tx = conn.savepoint()?;
    let steps = apply_steps(&tx, project_id, opts)?;
    let plan = tally(&steps);
    let writes: Vec<(ImageId, PickFlag)> = steps
        .iter()
        .filter_map(|&(id, step, _)| match step {
            ApplyStep::Write(f) => Some((id, f)),
            _ => None,
        })
        .collect();
    let changed: Vec<ImageId> = writes.iter().map(|w| w.0).collect();
    let previous = repo::cull_snapshot(&tx, &changed)?;
    let previous_applied_at_ms: Option<i64> = tx
        .query_row("SELECT applied_at FROM target_runs WHERE project_id = ?1", [project_id], |r| r.get(0))
        .optional()?
        .flatten();
    {
        let mut stmt = tx.prepare_cached("UPDATE images SET pick = ?2, pick_origin = 'auto' WHERE id = ?1")?;
        for &(id, flag) in &writes {
            stmt.execute(params![id, flag.as_str()])?;
        }
    }
    let applied_at_ms = now_ms();
    tx.execute("UPDATE target_runs SET applied_at = ?2 WHERE project_id = ?1", params![project_id, applied_at_ms])?;
    tx.commit()?;
    Ok(TargetApplyResult {
        picks: plan.picks,
        rejects: plan.rejects,
        unflags: plan.unflags,
        unchanged: plan.unchanged,
        user_flagged: plan.user_flagged,
        changed,
        previous,
        applied_at_ms,
        previous_applied_at_ms,
    })
}

/// `restore_target_apply` (v20.2): undo / redo of [`apply`]. Writes the flags back like
/// `repo::restore_cull_snapshot` and sets `target_runs.applied_at` to `applied_at_ms` (undo:
/// the result's `previous_applied_at_ms`, `None` for a first apply; redo: its
/// `applied_at_ms`), in one transaction. Unknown project or image -> `not_found`, rating > 5
/// -> `invalid_argument`, nothing written. Returns the ids whose flags changed.
pub fn restore_apply(
    conn: &mut Connection,
    project_id: ProjectId,
    snapshots: &[CullSnapshot],
    applied_at_ms: Option<i64>,
) -> AppResult<Vec<ImageId>> {
    ensure_project(conn, project_id)?;
    let tx = conn.savepoint()?;
    let changed = repo::write_cull_snapshot(&tx, snapshots)?;
    tx.execute("UPDATE target_runs SET applied_at = ?2 WHERE project_id = ?1", params![project_id, applied_at_ms])?;
    tx.commit()?;
    Ok(changed)
}

// ---------------------------------------------------------------------------
// User edits
// ---------------------------------------------------------------------------

struct Row0 {
    project_id: ProjectId,
    choice: TargetChoice,
    alternative_of: Option<ImageId>,
}

fn row0(conn: &Connection, id: ImageId) -> AppResult<Row0> {
    ensure_image(conn, id)?;
    conn.prepare_cached("SELECT project_id, choice, alternative_of FROM target_selection WHERE image_id = ?1")?
        .query_row([id], |r| Ok(Row0 { project_id: r.get(0)?, choice: choice_col(r, 1)?, alternative_of: r.get(2)? }))
        .optional()?
        .ok_or_else(|| AppError::invalid(format!("image {id} is not part of the target selection")))
}

fn snapshot(conn: &Connection, id: ImageId) -> AppResult<TargetSnapshot> {
    let mut s = conn
        .prepare_cached(
            "SELECT choice, alternative_of, rank, covered_by, covered_similarity, locked, origin, reasons_json
             FROM target_selection WHERE image_id = ?1",
        )?
        .query_row([id], |r| {
            Ok(TargetSnapshot {
                image_id: id,
                choice: choice_col(r, 0)?,
                alternative_of: r.get(1)?,
                rank: r.get(2)?,
                covered_by: r.get(3)?,
                covered_similarity: r.get::<_, Option<f64>>(4)?.map(|v| v as f32),
                locked: r.get(5)?,
                origin: origin_col(r, 6)?,
                reasons: serde_json::from_str(&r.get::<_, String>(7)?).unwrap_or_default(),
                cull: crate::ipc::types::CullSnapshot {
                    image_id: id,
                    rating: 0,
                    pick: PickFlag::Unflagged,
                    color_label: None,
                    pick_origin: None,
                },
            })
        })?;
    s.cull = repo::cull_snapshot(conn, &[id])?.remove(0);
    Ok(s)
}

/// Ids an edit of `ids` may touch: themselves, their parents' alternatives, their own
/// alternatives and the rows they cover, plus every row of those rows' moments (covers are
/// recomputed per moment). Returns the ids and the moments.
fn affected(conn: &Connection, ids: &[ImageId]) -> AppResult<(BTreeSet<ImageId>, BTreeSet<MomentId>)> {
    let mut out: BTreeSet<ImageId> = ids.iter().copied().collect();
    let mut stmt = conn.prepare_cached(
        "SELECT image_id FROM target_selection WHERE alternative_of = ?1 OR covered_by = ?1
         OR (alternative_of IS NOT NULL AND alternative_of =
             (SELECT alternative_of FROM target_selection WHERE image_id = ?1))",
    )?;
    for &id in ids {
        for r in stmt.query_map([id], |r| r.get::<_, ImageId>(0))? {
            out.insert(r?);
        }
    }
    let moments = moments_of(conn, out.iter().copied())?;
    let mut members = conn.prepare_cached("SELECT image_id FROM target_selection WHERE moment_id = ?1")?;
    for &m in &moments {
        for r in members.query_map([m], |r| r.get::<_, ImageId>(0))? {
            out.insert(r?);
        }
    }
    Ok((out, moments))
}

/// Prepends a `user_choice` reason (replacing an older one).
fn set_user_reason(conn: &Connection, id: ImageId, text: String, related: Option<ImageId>) -> AppResult<()> {
    let json: String =
        conn.query_row("SELECT reasons_json FROM target_selection WHERE image_id = ?1", [id], |r| r.get(0))?;
    let mut reasons: Vec<TargetReason> = serde_json::from_str(&json).unwrap_or_default();
    reasons.retain(|r| r.kind != TargetReasonKind::UserChoice);
    reasons.insert(0, TargetReason { kind: TargetReasonKind::UserChoice, text, related_image_id: related });
    conn.execute(
        "UPDATE target_selection SET reasons_json = ?2 WHERE image_id = ?1",
        params![id, serde_json::to_string(&reasons)?],
    )?;
    Ok(())
}

/// Renumbers the alternatives of `parent` 1..n in their current order.
fn compact(conn: &Connection, parent: ImageId) -> AppResult<()> {
    let ids: Vec<ImageId> = conn
        .prepare_cached(
            "SELECT image_id FROM target_selection WHERE alternative_of = ?1 AND choice = 'alternative'
             ORDER BY rank IS NULL, rank, image_id",
        )?
        .query_map([parent], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut stmt = conn.prepare_cached("UPDATE target_selection SET rank = ?2 WHERE image_id = ?1")?;
    for (i, id) in ids.iter().enumerate() {
        stmt.execute(params![id, i as i64 + 1])?;
    }
    Ok(())
}

/// Moves `id` (any choice) to `choice` (not `alternative`), locked (origin `user` when the
/// choice changes). Leaving the delivery set:
/// its alternatives become `not_sure` and rows it covered lose the link. Leaving an
/// alternative strip: the strip is re-ranked.
fn move_to(conn: &Connection, id: ImageId, choice: TargetChoice) -> AppResult<()> {
    let r = row0(conn, id)?;
    if r.choice == TargetChoice::Deliver && choice != TargetChoice::Deliver {
        conn.execute(
            "UPDATE target_selection SET choice = 'not_sure', alternative_of = NULL, rank = NULL, updated_at = ?2
             WHERE alternative_of = ?1",
            params![id, now_ms()],
        )?;
        conn.execute(
            "UPDATE target_selection SET covered_by = NULL, covered_similarity = NULL WHERE covered_by = ?1",
            [id],
        )?;
    }
    conn.execute(
        "UPDATE target_selection SET choice = ?2, alternative_of = NULL, rank = NULL, locked = 1, updated_at = ?3,
             origin = CASE WHEN choice IS NOT ?2 THEN 'user' ELSE origin END,
             covered_by = CASE WHEN ?2 = 'deliver' THEN NULL ELSE covered_by END,
             covered_similarity = CASE WHEN ?2 = 'deliver' THEN NULL ELSE covered_similarity END
         WHERE image_id = ?1",
        params![id, choice.as_str(), now_ms()],
    )?;
    if let Some(p) = r.alternative_of.filter(|_| r.choice == TargetChoice::Alternative) {
        compact(conn, p)?;
    }
    Ok(())
}

/// Sets the flag as the user's; returns whether it changed.
fn write_user_flag(conn: &Connection, id: ImageId, pick: PickFlag) -> AppResult<bool> {
    Ok(conn.execute(
        "UPDATE images SET pick = ?2, pick_origin = 'user' WHERE id = ?1 AND pick IS NOT ?2",
        params![id, pick.as_str()],
    )? > 0)
}

/// Runs a target edit: snapshots the affected rows, applies `f`, refreshes their suggestion
/// overlay and returns what changed.
fn edit(
    conn: &mut Connection,
    ids: &[ImageId],
    f: impl FnOnce(&Connection, &mut Vec<ImageId>) -> AppResult<()>,
) -> AppResult<crate::ipc::types::TargetEditResult> {
    let tx = conn.savepoint()?;
    let mut project = None;
    for &id in ids {
        let p = row0(&tx, id)?.project_id;
        if project.is_some_and(|q| q != p) {
            return Err(AppError::invalid("photos of different projects"));
        }
        project = Some(p);
    }
    let project = project.ok_or_else(|| AppError::invalid("no photos given"))?;
    let (touched, moments) = affected(&tx, ids)?;
    let before: Vec<TargetSnapshot> = touched.iter().map(|&id| snapshot(&tx, id)).collect::<AppResult<_>>()?;
    let mut flags_changed = Vec::new();
    f(&tx, &mut flags_changed)?;
    // The edited photos may have changed moment membership of the delivery set (and a swap
    // may bring in a photo of another moment).
    let mut moments = moments;
    moments.extend(moments_of(&tx, ids.iter().copied())?);
    refresh_covers(&tx, &moments)?;
    let all: Vec<ImageId> = touched.iter().copied().collect();
    overlay_suggestions(&tx, &all)?;
    let mut changed = Vec::new();
    let mut previous = Vec::new();
    for b in before {
        let now = snapshot(&tx, b.image_id)?;
        let row_changed = (now.choice, now.alternative_of, now.rank, now.covered_by, now.locked, now.origin)
            != (b.choice, b.alternative_of, b.rank, b.covered_by, b.locked, b.origin)
            || now.covered_similarity != b.covered_similarity;
        if row_changed || now.cull != b.cull || ids.contains(&b.image_id) {
            if let Some(s) = selection(&tx, b.image_id)? {
                changed.push(s);
            }
            previous.push(b);
        }
    }
    let counts = counts(&tx, project)?;
    tx.commit()?;
    flags_changed.sort_unstable();
    flags_changed.dedup();
    Ok(crate::ipc::types::TargetEditResult { changed, flags_changed, previous, counts })
}

/// `swap_alternative`: `alternative_id` (any non-delivered photo of the project) takes the
/// place of `delivered_id` in the delivery set; `delivered_id` becomes its alternative #1 and
/// the old strip follows the new photo. Both locked; flags: the new photo picked, the old one
/// unflagged if it was picked (the user's). Not delivered / delivered alternative /
/// different projects / no row -> `invalid_argument`; unknown -> `not_found`.
pub fn swap(
    conn: &mut Connection,
    delivered_id: ImageId,
    alternative_id: ImageId,
) -> AppResult<crate::ipc::types::TargetEditResult> {
    if delivered_id == alternative_id {
        return Err(AppError::invalid("cannot swap a photo with itself"));
    }
    edit(conn, &[delivered_id, alternative_id], |tx, flags| {
        if row0(tx, delivered_id)?.choice != TargetChoice::Deliver {
            return Err(AppError::invalid(format!("image {delivered_id} is not in the delivery set")));
        }
        if row0(tx, alternative_id)?.choice == TargetChoice::Deliver {
            return Err(AppError::invalid(format!("image {alternative_id} is already in the delivery set")));
        }
        move_to(tx, alternative_id, TargetChoice::Deliver)?;
        // The old photo's strip and covered rows follow the new one (old photo first).
        let now = now_ms();
        tx.execute(
            "UPDATE target_selection SET alternative_of = ?2, rank = COALESCE(rank, 0) + 1, updated_at = ?3
             WHERE alternative_of = ?1 AND choice = 'alternative'",
            params![delivered_id, alternative_id, now],
        )?;
        tx.execute(
            "UPDATE target_selection SET covered_by = ?2 WHERE covered_by = ?1 AND image_id <> ?2",
            params![delivered_id, alternative_id],
        )?;
        tx.execute(
            "UPDATE target_selection SET choice = 'alternative', alternative_of = ?2, rank = 1, locked = 1,
                 origin = 'user', covered_by = ?2, covered_similarity = NULL, updated_at = ?3 WHERE image_id = ?1",
            params![delivered_id, alternative_id, now],
        )?;
        compact(tx, alternative_id)?;
        let (old, new) = (file_stem(tx, delivered_id)?, file_stem(tx, alternative_id)?);
        set_user_reason(tx, alternative_id, format!("You swapped this in for {old}"), Some(delivered_id))?;
        set_user_reason(tx, delivered_id, format!("You swapped this out for {new}"), Some(alternative_id))?;
        if write_user_flag(tx, alternative_id, PickFlag::Pick)? {
            flags.push(alternative_id);
        }
        let old_pick: String = tx.query_row("SELECT pick FROM images WHERE id = ?1", [delivered_id], |r| r.get(0))?;
        if old_pick == PickFlag::Pick.as_str() && write_user_flag(tx, delivered_id, PickFlag::Unflagged)? {
            flags.push(delivered_id);
        }
        Ok(())
    })
}

/// `add_alternative`: a non-delivered photo joins the delivery set as well (both kept),
/// locked, picked as the user's. Already delivered / no row -> `invalid_argument`.
pub fn add(conn: &mut Connection, id: ImageId) -> AppResult<crate::ipc::types::TargetEditResult> {
    edit(conn, &[id], |tx, flags| {
        if row0(tx, id)?.choice == TargetChoice::Deliver {
            return Err(AppError::invalid(format!("image {id} is already in the delivery set")));
        }
        move_to(tx, id, TargetChoice::Deliver)?;
        set_user_reason(tx, id, "You added this".to_owned(), None)?;
        if write_user_flag(tx, id, PickFlag::Pick)? {
            flags.push(id);
        }
        Ok(())
    })
}

/// `set_target_choice`: moves `ids` to `choice` (`deliver`, `not_sure`, `set_aside`; an
/// `alternative` needs a parent -> `invalid_argument`), locked. The flag follows as the user's
/// (v20.1 also when the choice is unchanged, e.g. Keep): `deliver` picks the photo, the other
/// choices unflag a pick (never reject). Leaving the delivery set: its alternatives become
/// `not_sure`.
pub fn set_choice(
    conn: &mut Connection,
    ids: &[ImageId],
    choice: TargetChoice,
) -> AppResult<crate::ipc::types::TargetEditResult> {
    if choice == TargetChoice::Alternative {
        return Err(AppError::invalid("use swap_alternative to make a photo an alternative"));
    }
    let ids: Vec<ImageId> = ids.iter().copied().collect::<BTreeSet<_>>().into_iter().collect();
    edit(conn, &ids, |tx, flags| {
        for &id in &ids {
            let was = row0(tx, id)?.choice;
            if was == choice {
                // Keep / confirm: lock (origin and reasons unchanged); the flag still follows.
                tx.execute("UPDATE target_selection SET locked = 1 WHERE image_id = ?1", [id])?;
            } else {
                move_to(tx, id, choice)?;
                let text = match choice {
                    TargetChoice::Deliver => "You added this",
                    TargetChoice::NotSure => "You marked this not sure",
                    _ => "You set this aside",
                };
                set_user_reason(tx, id, text.to_owned(), None)?;
            }
            let pick: String = tx.query_row("SELECT pick FROM images WHERE id = ?1", [id], |r| r.get(0))?;
            let next = match choice {
                TargetChoice::Deliver => Some(PickFlag::Pick),
                _ if pick == PickFlag::Pick.as_str() => Some(PickFlag::Unflagged),
                _ => None,
            };
            if let Some(next) = next {
                if write_user_flag(tx, id, next)? {
                    flags.push(id);
                }
            }
        }
        Ok(())
    })
}

/// `restore_target_snapshot`: writes the rows and flags back (undo / redo of a target edit),
/// all or nothing. Returns the ids whose row or flags changed.
pub fn restore(conn: &mut Connection, snapshots: &[TargetSnapshot]) -> AppResult<Vec<ImageId>> {
    let tx = conn.savepoint()?;
    let mut changed = BTreeSet::new();
    {
        let mut stmt = tx.prepare_cached(
            "UPDATE target_selection SET choice = ?2, alternative_of = ?3, rank = ?4, covered_by = ?5, locked = ?6,
                 updated_at = ?7, covered_similarity = ?8, origin = ?9, reasons_json = ?10
             WHERE image_id = ?1 AND (choice IS NOT ?2 OR alternative_of IS NOT ?3 OR rank IS NOT ?4
                                      OR covered_by IS NOT ?5 OR locked IS NOT ?6 OR covered_similarity IS NOT ?8
                                      OR origin IS NOT ?9 OR reasons_json IS NOT ?10)",
        )?;
        for s in snapshots {
            row0(&tx, s.image_id)?;
            let rank = s.rank.map(|r| r.max(1));
            let sim = s.covered_similarity.filter(|v| v.is_finite()).map(|v| v as f64);
            if stmt.execute(params![
                s.image_id,
                s.choice.as_str(),
                s.alternative_of,
                rank,
                s.covered_by,
                s.locked,
                now_ms(),
                sim,
                s.origin.as_str(),
                serde_json::to_string(&s.reasons)?
            ])? > 0
            {
                changed.insert(s.image_id);
            }
        }
    }
    {
        // Same statement as `repo::restore_cull_snapshot` (inside this transaction).
        let mut stmt = tx.prepare_cached(
            "UPDATE images SET rating = ?2, pick = ?3, color_label = ?4, pick_origin = ?5
             WHERE id = ?1 AND (rating IS NOT ?2 OR pick IS NOT ?3 OR color_label IS NOT ?4
                                OR (?3 <> 'unflagged' AND pick_origin IS NOT ?5))",
        )?;
        for s in snapshots.iter().map(|s| &s.cull) {
            if s.rating > 5 {
                return Err(AppError::invalid(format!("rating {} is outside 0..=5", s.rating)));
            }
            let label = s.color_label.map(|l| l.as_str());
            let origin = s.pick_origin.unwrap_or(PickOrigin::User).as_str();
            if stmt.execute(params![s.image_id, s.rating, s.pick.as_str(), label, origin])? > 0 {
                changed.insert(s.image_id);
            }
        }
    }
    let ids: Vec<ImageId> = snapshots.iter().map(|s| s.image_id).collect();
    overlay_suggestions(&tx, &ids)?;
    tx.commit()?;
    Ok(changed.into_iter().collect())
}

/// The user flagged `ids` (`set_pick`): their selection rows follow and lock, so re-runs and
/// [`apply`] keep the decision. pick -> `deliver`; reject -> `set_aside`; unflagged -> a
/// delivered photo becomes `not_sure`, others just lock. Ids without a row are ignored. Does
/// not write flags. Returns the ids whose row changed.
pub fn note_user_flags(conn: &mut Connection, ids: &[ImageId], pick: PickFlag) -> AppResult<Vec<ImageId>> {
    let tx = conn.savepoint()?;
    let mut changed = Vec::new();
    for &id in ids {
        let Some(r) = tx
            .prepare_cached("SELECT choice FROM target_selection WHERE image_id = ?1")?
            .query_row([id], |r| choice_col(r, 0))
            .optional()?
        else {
            continue;
        };
        let target = match (pick, r) {
            (PickFlag::Pick, _) => TargetChoice::Deliver,
            (PickFlag::Reject, _) => TargetChoice::SetAside,
            (PickFlag::Unflagged, TargetChoice::Deliver) => TargetChoice::NotSure,
            (PickFlag::Unflagged, other) => other,
        };
        if target == r {
            tx.execute("UPDATE target_selection SET locked = 1 WHERE image_id = ?1", [id])?;
            continue;
        }
        move_to(&tx, id, target)?;
        let text = match pick {
            PickFlag::Pick => "You picked this",
            PickFlag::Reject => "You rejected this",
            PickFlag::Unflagged => "You removed the flag",
        };
        set_user_reason(&tx, id, text.to_owned(), None)?;
        changed.push(id);
    }
    if !changed.is_empty() {
        let moments = moments_of(&tx, changed.iter().copied())?;
        refresh_covers(&tx, &moments)?;
    }
    overlay_suggestions(&tx, ids)?;
    tx.commit()?;
    Ok(changed)
}

/// `lock_target_choices`: locks the selection rows of `ids` as they are (choice, origin,
/// reasons and flags unchanged), so a re-run keeps them, e.g. the picks the user reviewed.
/// Ids without a row are ignored; unknown image -> `not_found`. Returns the ids newly locked.
pub fn lock(conn: &mut Connection, ids: &[ImageId]) -> AppResult<Vec<ImageId>> {
    let tx = conn.savepoint()?;
    let mut changed = Vec::new();
    {
        let mut stmt = tx.prepare_cached(
            "UPDATE target_selection SET locked = 1, updated_at = ?2 WHERE image_id = ?1 AND locked = 0",
        )?;
        for &id in ids {
            ensure_image(&tx, id)?;
            if stmt.execute(params![id, now_ms()])? > 0 && !changed.contains(&id) {
                changed.push(id);
            }
        }
    }
    tx.commit()?;
    Ok(changed)
}

/// Images of a project with a selection row whose choice is in `choices` (SQL fragment for
/// `ImageQuery.targetChoices`, column prefix `p`, e.g. `"i."`).
pub fn choices_clause(choices: &[TargetChoice], p: &str) -> String {
    let list = choices.iter().map(|c| format!("'{}'", c.as_str())).collect::<Vec<_>>().join(", ");
    format!("{p}id IN (SELECT image_id FROM target_selection WHERE choice IN ({list}))")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::ipc::types::{ImageQuery, TargetRunState};

    /// Project 1 (folder 1) with photos 1..=6, project 2 (folder 2) with photo 7. Every photo is
    /// analysed with the scorer suggesting `pick` for 1..=3, `reject` for 6 and nothing else.
    fn setup() -> Connection {
        let conn = open_in_memory();
        conn.execute_batch(
            "INSERT INTO projects (id, name, shoot_type, created_at) VALUES (1, 'Wedding', 'wedding', 0), (2, 'Other', 'portrait', 0);
             INSERT INTO folders (id, path, added_at, project_id) VALUES (1, '/w', 0, 1), (2, '/o', 0, 2);",
        )
        .unwrap();
        for id in 1..=7i64 {
            let folder = if id == 7 { 2 } else { 1 };
            let scored = match id {
                1..=3 => "pick",
                6 => "reject",
                _ => "unflagged",
            };
            conn.execute(
                "INSERT INTO images (id, folder_id, path, file_name, format, camera_make, file_size, file_mtime_ms,
                                     imported_at, captured_at_ms)
                 VALUES (?1, ?2, ?3, ?4, 'arw', 'sony', 1, 0, 0, ?5)",
                params![id, folder, format!("/x/DSC{id:04}.ARW"), format!("DSC{id:04}.ARW"), id * 1000],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO quality_scores (image_id, overall, global_sharpness, clipped_highlights_pct,
                                             clipped_shadows_pct, mean_luma, model_version, analyzed_at,
                                             suggested_pick, scored_pick)
                 VALUES (?1, 0.5, 0.5, 0, 0, 0.5, 'm', 0, ?2, ?2)",
                params![id, scored],
            )
            .unwrap();
        }
        conn
    }

    fn draft(id: ImageId, choice: TargetChoice) -> SelectionDraft {
        SelectionDraft {
            image_id: id,
            choice,
            moment_key: Some(1),
            shot_type: Some(ShotType::Couple),
            alternative_of: None,
            rank: None,
            covered_by: None,
            covered_similarity: None,
            score: 0.5,
            reasons: vec![],
        }
    }

    /// Moment 1 = photos 1..=5: 1 and 4 delivered, 2 / 3 alternatives of 1, 5 not sure
    /// (covered by 4); photo 6 set aside in no moment.
    fn store(conn: &mut Connection) {
        let moment = MomentDraft {
            key: 1,
            shot_type: ShotType::Couple,
            started_at_ms: Some(1000),
            ended_at_ms: Some(5000),
            representative_id: Some(1),
            person_ids: vec![],
        };
        let mut drafts = vec![draft(1, TargetChoice::Deliver), draft(4, TargetChoice::Deliver)];
        for (id, rank) in [(2, 1), (3, 2)] {
            drafts.push(SelectionDraft {
                alternative_of: Some(1),
                rank: Some(rank),
                covered_by: Some(1),
                covered_similarity: Some(0.9),
                ..draft(id, TargetChoice::Alternative)
            });
        }
        drafts.push(SelectionDraft {
            covered_by: Some(4),
            covered_similarity: Some(0.8),
            ..draft(5, TargetChoice::NotSure)
        });
        drafts.push(SelectionDraft { moment_key: None, shot_type: None, ..draft(6, TargetChoice::SetAside) });
        store_results(conn, 1, &[moment], &drafts).unwrap();
    }

    fn suggested(conn: &Connection, id: ImageId) -> String {
        conn.query_row("SELECT suggested_pick FROM quality_scores WHERE image_id = ?1", [id], |r| r.get(0)).unwrap()
    }

    fn flag(conn: &Connection, id: ImageId) -> (String, String) {
        conn.query_row("SELECT pick, pick_origin FROM images WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
    }

    #[test]
    fn embeddings_round_trip() {
        let conn = setup();
        let v = vec![0.25f32, -1.0, 3.5];
        assert_eq!(decode_embedding(&encode_embedding(&v), 3), Some(v.clone()));
        assert_eq!(decode_embedding(&encode_embedding(&v), 2), None);
        let bbox = NormRect { x: 0.1, y: 0.2, width: 0.1, height: 0.15 };
        upsert_embedding(&conn, 1, 0, "arcface-test", &bbox, &v, 0.9).unwrap();
        upsert_embedding(&conn, 7, 0, "arcface-test", &bbox, &v, 0.9).unwrap();
        assert!(upsert_embedding(&conn, 1, 1, "m", &bbox, &[], 0.5).is_err());
        assert!(upsert_embedding(&conn, 1, 1, "m", &bbox, &[f32::NAN], 0.5).is_err());
        let stored = project_embeddings(&conn, 1).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!((stored[0].image_id, stored[0].face_index, &stored[0].vector), (1, 0, &v));
    }

    #[test]
    fn store_overlay_counts_moments_alternatives_and_covered_by() {
        let mut conn = setup();
        // Photo 7 belongs to another project.
        assert_eq!(
            store_results(&mut conn, 1, &[], &[draft(7, TargetChoice::Deliver)]).unwrap_err().kind,
            crate::ipc::error::ErrorKind::InvalidArgument
        );
        store(&mut conn);
        // Overlay: delivered -> pick; others lose the scorer's pick; the defect reject stays.
        let s: Vec<String> = (1..=7).map(|id| suggested(&conn, id)).collect();
        assert_eq!(s, ["pick", "unflagged", "unflagged", "pick", "unflagged", "reject", "unflagged"]);
        let c = counts(&conn, 1).unwrap();
        assert_eq!((c.total, c.deliver, c.alternative, c.not_sure, c.set_aside, c.locked), (6, 2, 2, 1, 1, 0));
        assert_eq!(c.per_shot_type, vec![ShotTypeCount { shot_type: ShotType::Couple, total: 5, deliver: 2 }]);

        let m = list_moments(&conn, 1).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].image_ids.clone(), m[0].delivered_ids.clone()), (vec![1, 2, 3, 4, 5], vec![1, 4]));

        let a = alternatives(&conn, 3).unwrap();
        assert_eq!(a.delivered.unwrap().image_id, 1);
        assert_eq!(
            a.alternatives.iter().map(|s| (s.image_id, s.rank)).collect::<Vec<_>>(),
            [(2, Some(1)), (3, Some(2))]
        );
        assert!(alternatives(&conn, 5).unwrap().delivered.is_none());

        let cov = covered_by(&conn, 5).unwrap().unwrap();
        assert_eq!((cov.covered_by_id, cov.same_moment, cov.text.as_str()), (4, true, "Similar to DSC0004 (kept)"));
        assert!(covered_by(&conn, 1).unwrap().is_none());

        // Grid filter.
        let q = ImageQuery {
            project_id: Some(1),
            target_choices: vec![TargetChoice::NotSure, TargetChoice::SetAside],
            ..ImageQuery::default()
        };
        assert_eq!(repo::list_image_ids(&conn, &q).unwrap(), vec![5, 6]);

        // Run bookkeeping.
        let settings = resolve_settings(&conn, 1, &TargetRunSettings { target_count: 800, shoot_type: None }).unwrap();
        assert_eq!(settings.shoot_type, Some(ShootType::Wedding));
        assert!(resolve_settings(&conn, 1, &TargetRunSettings { target_count: 0, shoot_type: None }).is_err());
        assert!(get_run(&conn, 1, false).unwrap().is_none());
        begin_run(&conn, 1, &settings).unwrap();
        assert_eq!(get_run(&conn, 1, true).unwrap().unwrap().state, TargetRunState::Running);
        assert_eq!(get_run(&conn, 1, false).unwrap().unwrap().state, TargetRunState::Cancelled);
        finish_run(&conn, 1, TargetRunState::Finished, Some("Picked 2 of 6 photos"), Some("v1")).unwrap();
        let run = get_run(&conn, 1, false).unwrap().unwrap();
        assert_eq!((run.state, run.counts.deliver, run.model_version.as_str()), (TargetRunState::Finished, 2, "v1"));
    }

    #[test]
    fn swap_add_set_choice_and_undo() {
        let mut conn = setup();
        store(&mut conn);
        conn.execute("UPDATE images SET pick = 'pick', pick_origin = 'auto' WHERE id IN (1, 4)", []).unwrap();

        let r = swap(&mut conn, 1, 3).unwrap();
        let sel = |conn: &Connection, id| selection(conn, id).unwrap().unwrap();
        assert_eq!(sel(&conn, 3).choice, TargetChoice::Deliver);
        assert!(sel(&conn, 3).locked);
        let old = sel(&conn, 1);
        assert_eq!(
            (old.choice, old.alternative_of, old.rank, old.covered_by),
            (TargetChoice::Alternative, Some(3), Some(1), Some(3))
        );
        let two = sel(&conn, 2);
        assert_eq!((two.alternative_of, two.rank, two.covered_by), (Some(3), Some(2), Some(3)));
        assert_eq!(flag(&conn, 3), ("pick".into(), "user".into()));
        assert_eq!(flag(&conn, 1).0, "unflagged");
        assert_eq!(r.flags_changed, vec![1, 3]);
        assert_eq!((suggested(&conn, 3), suggested(&conn, 1)), ("pick".into(), "unflagged".into()));
        assert_eq!(r.counts.deliver, 2);
        assert!(swap(&mut conn, 2, 3).is_err(), "2 is not delivered");

        // Undo restores rows and flags.
        let changed = restore(&mut conn, &r.previous).unwrap();
        assert!(changed.contains(&1) && changed.contains(&3));
        assert_eq!(sel(&conn, 1).choice, TargetChoice::Deliver);
        assert_eq!((sel(&conn, 3).alternative_of, sel(&conn, 3).rank), (Some(1), Some(2)));
        assert_eq!(flag(&conn, 1), ("pick".into(), "auto".into()));
        assert_eq!(flag(&conn, 3).0, "unflagged");
        assert_eq!(suggested(&conn, 1), "pick");

        // Add both: 5 joins the delivery set.
        let r = add(&mut conn, 5).unwrap();
        assert_eq!((sel(&conn, 5).choice, sel(&conn, 5).covered_by), (TargetChoice::Deliver, None));
        assert_eq!(r.counts.deliver, 3);
        assert!(add(&mut conn, 5).is_err());

        // Removing 1 from the delivery set: its alternatives become not sure, its pick goes.
        let r = set_choice(&mut conn, &[1], TargetChoice::SetAside).unwrap();
        assert_eq!(sel(&conn, 1).choice, TargetChoice::SetAside);
        assert_eq!((sel(&conn, 2).choice, sel(&conn, 2).alternative_of), (TargetChoice::NotSure, None));
        assert_eq!(flag(&conn, 1).0, "unflagged");
        assert_eq!(r.counts.alternative, 0);
        assert!(set_choice(&mut conn, &[2], TargetChoice::Alternative).is_err());
        assert!(set_choice(&mut conn, &[7], TargetChoice::Deliver).is_err(), "7 has no selection row");
    }

    #[test]
    fn user_flags_lock_and_win_over_reruns_and_apply() {
        let mut conn = setup();
        store(&mut conn);
        // The user rejects delivered 4 and picks set-aside 6 (through set_pick).
        repo::set_pick(&mut conn, &[4], PickFlag::Reject).unwrap();
        note_user_flags(&mut conn, &[4], PickFlag::Reject).unwrap();
        repo::set_pick(&mut conn, &[6], PickFlag::Pick).unwrap();
        note_user_flags(&mut conn, &[6], PickFlag::Pick).unwrap();
        let sel = |conn: &Connection, id| selection(conn, id).unwrap().unwrap();
        assert_eq!((sel(&conn, 4).choice, sel(&conn, 4).locked), (TargetChoice::SetAside, true));
        assert_eq!(sel(&conn, 5).covered_by, None, "4 no longer covers 5");
        assert_eq!((sel(&conn, 6).choice, suggested(&conn, 6)), (TargetChoice::Deliver, "pick".into()));
        let locked = locked_choices(&conn, 1).unwrap();
        assert_eq!(
            locked.iter().map(|l| (l.image_id, l.choice)).collect::<Vec<_>>(),
            [(4, TargetChoice::SetAside), (6, TargetChoice::Deliver)]
        );

        // A re-run that would deliver 4 and set 6 aside keeps the user's choices.
        let moment = MomentDraft {
            key: 1,
            shot_type: ShotType::Group,
            started_at_ms: None,
            ended_at_ms: None,
            representative_id: None,
            person_ids: vec![],
        };
        store_results(
            &mut conn,
            1,
            &[moment],
            &[draft(4, TargetChoice::Deliver), draft(6, TargetChoice::SetAside), draft(2, TargetChoice::Deliver)],
        )
        .unwrap();
        assert_eq!(sel(&conn, 4).choice, TargetChoice::SetAside);
        assert_eq!(sel(&conn, 6).choice, TargetChoice::Deliver);
        assert_eq!(sel(&conn, 6).shot_type, Some(ShotType::Couple), "locked rows take the new moment data");
        assert!(selection(&conn, 1).unwrap().is_none(), "unlocked rows are replaced");

        // Apply: 2 (unflagged) becomes an auto pick, Sieve's stale auto pick on 5 is cleared,
        // the user's flags on 4 / 6 are untouched.
        conn.execute("UPDATE images SET pick = 'pick', pick_origin = 'auto' WHERE id = 5", []).unwrap();
        store_results(
            &mut conn,
            1,
            &[],
            &[
                SelectionDraft { moment_key: None, ..draft(4, TargetChoice::Deliver) },
                SelectionDraft { moment_key: None, ..draft(6, TargetChoice::SetAside) },
                SelectionDraft { moment_key: None, ..draft(2, TargetChoice::Deliver) },
                SelectionDraft { moment_key: None, ..draft(5, TargetChoice::SetAside) },
            ],
        )
        .unwrap();
        let plan = plan_apply(&conn, 1, TargetApplyOptions::default()).unwrap();
        let r = apply(&mut conn, 1, TargetApplyOptions::default()).unwrap();
        assert_eq!(flag(&conn, 2), ("pick".into(), "auto".into()));
        assert_eq!(flag(&conn, 5).0, "unflagged", "Sieve's own stale pick follows the selection");
        assert_eq!(flag(&conn, 4), ("reject".into(), "user".into()));
        assert_eq!(flag(&conn, 6), ("pick".into(), "user".into()));
        assert_eq!((r.picks, r.rejects, r.unflags, r.unchanged, r.user_flagged), (1, 0, 1, 0, 2));
        assert_eq!((plan.picks, plan.unflags, plan.user_flagged, plan.total), (1, 1, 2, 4));
        assert_eq!(r.changed, vec![2, 5]);
        assert_eq!(
            r.previous.iter().map(|s| (s.image_id, s.pick)).collect::<Vec<_>>(),
            [(2, PickFlag::Unflagged), (5, PickFlag::Pick)]
        );
        assert!(get_run(&conn, 1, false).unwrap().is_none(), "apply without a run row stamps nothing");
    }

    /// [`store`] plus frame similarities of moment 1 (photo 3 is nearest to 5 and 2).
    fn store_with_pairs(conn: &mut Connection) {
        store_similarities(
            conn,
            1,
            &[
                (5, 4, 0.8),
                (3, 5, 0.95),
                (5, 1, 0.5),
                (2, 1, 0.9),
                (3, 1, 0.85),
                (2, 3, 0.92),
                (2, 4, 0.3),
                (3, 4, 0.3),
                (1, 4, 0.2),
                (5, 2, 0.6),
                (1, 7, 0.9), // other project: skipped
            ],
        )
        .unwrap();
        store(conn);
    }

    #[test]
    fn covered_by_follows_user_edits_with_tier_and_origin() {
        let mut conn = setup();
        store_with_pairs(&mut conn);
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM target_similarity", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 10);
        let cov = covered_by(&conn, 5).unwrap().unwrap();
        assert_eq!(
            (cov.covered_by_id, cov.tier, cov.covered_by_origin),
            (4, SimilarityTier::VerySimilar, ChoiceOrigin::Engine)
        );
        assert_eq!(cov.text, "Similar to DSC0004 (kept)");
        assert_eq!(selection(&conn, 5).unwrap().unwrap().covered_tier, Some(SimilarityTier::VerySimilar));

        // The user adds 3: 5 and 2 are now nearest to it (the user's photo).
        let r = add(&mut conn, 3).unwrap();
        let cov = covered_by(&conn, 5).unwrap().unwrap();
        assert_eq!((cov.covered_by_id, cov.similarity, cov.tier), (3, 0.95, SimilarityTier::NearIdentical));
        assert_eq!((cov.covered_by_origin, cov.covered_by_name.as_str()), (ChoiceOrigin::User, "DSC0003"));
        assert_eq!(cov.text, "Almost identical to DSC0003 (you added it)");
        assert_eq!(selection(&conn, 2).unwrap().unwrap().covered_by, Some(3));
        assert_eq!(selection(&conn, 3).unwrap().unwrap().origin, ChoiceOrigin::User);
        assert!(r.changed.iter().any(|s| s.image_id == 5 && s.covered_by == Some(3)), "siblings reported");
        let m = &list_moments(&conn, 1).unwrap()[0];
        assert_eq!((m.delivered_ids.clone(), m.user_delivered_ids.clone()), (vec![1, 3, 4], vec![3]));

        // Undo restores covers, origin and reasons exactly.
        restore(&mut conn, &r.previous).unwrap();
        let five = selection(&conn, 5).unwrap().unwrap();
        assert_eq!((five.covered_by, five.covered_similarity), (Some(4), Some(0.8)));
        let three = selection(&conn, 3).unwrap().unwrap();
        assert_eq!(
            (three.choice, three.origin, three.reasons.len()),
            (TargetChoice::Alternative, ChoiceOrigin::Engine, 0)
        );

        // Swap 4 out for 5: 4 becomes 5's alternative, covered by 5 at the stored similarity.
        swap(&mut conn, 4, 5).unwrap();
        let four = selection(&conn, 4).unwrap().unwrap();
        assert_eq!((four.covered_by, four.covered_similarity), (Some(5), Some(0.8)));
        // The user rejects 1 in the grid: its rows are re-covered within the moment.
        note_user_flags(&mut conn, &[1], PickFlag::Reject).unwrap();
        assert_eq!(selection(&conn, 1).unwrap().unwrap().covered_by, Some(5));
        assert_eq!(selection(&conn, 2).unwrap().unwrap().covered_by, Some(5));

        // A cover in another moment survives a refresh without pairs.
        conn.execute("UPDATE target_selection SET covered_by = 5, covered_similarity = 0.5 WHERE image_id = 6", [])
            .unwrap();
        refresh_covers(&conn, &moments_of(&conn, [5]).unwrap()).unwrap();
        let cov = covered_by(&conn, 6).unwrap().unwrap();
        assert_eq!(
            (cov.tier, cov.text.as_str()),
            (SimilarityTier::AnotherMoment, "Looks like DSC0005 (you added it, another moment)")
        );
    }

    #[test]
    fn piles_reason_filter_moment_sort_and_lock() {
        let mut conn = setup();
        store_with_pairs(&mut conn);
        let c = counts(&conn, 1).unwrap();
        assert_eq!(c.piles, crate::ipc::types::TargetPileCounts { not_sure: 1, similar: 0, weaker: 0, defects: 1 });
        assert_eq!(selection(&conn, 6).unwrap().unwrap().pile, Some(TargetPile::Defects));
        assert_eq!(selection(&conn, 1).unwrap().unwrap().pile, None);
        // 5 set aside: covered at 0.8 -> similar.
        set_choice(&mut conn, &[5], TargetChoice::SetAside).unwrap();
        assert_eq!(counts(&conn, 1).unwrap().piles.similar, 1);
        let q = |piles: Vec<TargetPile>, kinds: Vec<TargetReasonKind>| ImageQuery {
            project_id: Some(1),
            target_piles: piles,
            target_reason_kinds: kinds,
            ..ImageQuery::default()
        };
        assert_eq!(
            repo::list_image_ids(&conn, &q(vec![TargetPile::Similar, TargetPile::Defects], vec![])).unwrap(),
            vec![5, 6]
        );
        assert_eq!(repo::list_image_ids(&conn, &q(vec![], vec![TargetReasonKind::UserChoice])).unwrap(), vec![5]);

        // By moment, then score: moment 1 rows (scores) first, then 6 (no moment).
        conn.execute("UPDATE target_selection SET score = image_id / 10.0 WHERE image_id IN (2, 3)", []).unwrap();
        conn.execute("UPDATE target_selection SET score = 0.1 WHERE image_id IN (1, 4, 5)", []).unwrap();
        let sorted = ImageQuery {
            project_id: Some(1),
            sort: crate::ipc::types::ImageSort::TargetMoment,
            ..ImageQuery::default()
        };
        assert_eq!(repo::list_image_ids(&conn, &sorted).unwrap(), vec![3, 2, 1, 4, 5, 6]);

        // Lock without changing anything.
        assert_eq!(lock(&mut conn, &[1, 4, 7]).unwrap(), vec![1, 4]);
        assert!(lock(&mut conn, &[1]).unwrap().is_empty());
        assert!(lock(&mut conn, &[999]).is_err());
        let one = selection(&conn, 1).unwrap().unwrap();
        assert_eq!((one.locked, one.origin, one.choice), (true, ChoiceOrigin::Engine, TargetChoice::Deliver));
        assert_eq!(flag(&conn, 1).0, "unflagged");

        // Keep (same choice) flags the photo at once.
        let r = set_choice(&mut conn, &[1], TargetChoice::Deliver).unwrap();
        assert_eq!((flag(&conn, 1), r.flags_changed), (("pick".into(), "user".into()), vec![1]));
    }

    #[test]
    fn apply_plan_counts_and_rejects_option() {
        let mut conn = setup();
        store(&mut conn);
        // 1, 4 delivered -> picks; 6 confident defect -> reject; 2 has a stale auto pick.
        conn.execute("UPDATE images SET pick = 'pick', pick_origin = 'auto' WHERE id = 2", []).unwrap();
        conn.execute("UPDATE images SET pick = 'pick', pick_origin = 'user' WHERE id = 3", []).unwrap();
        let no = TargetApplyOptions { rejects: false };
        let plan = plan_apply(&conn, 1, no).unwrap();
        assert_eq!(
            (plan.total, plan.picks, plan.rejects, plan.rejectable, plan.unflags, plan.unchanged, plan.user_flagged),
            (6, 2, 0, 1, 1, 2, 1)
        );
        let yes = plan_apply(&conn, 1, TargetApplyOptions::default()).unwrap();
        assert_eq!((yes.rejects, yes.unchanged), (1, 1));
        let r = apply(&mut conn, 1, no).unwrap();
        assert_eq!((r.picks, r.rejects, r.unflags), (2, 0, 1));
        assert_eq!(flag(&conn, 6).0, "unflagged");
        // Undo with the generic cull restore.
        repo::restore_cull_snapshot(&mut conn, &r.previous).unwrap();
        assert_eq!(flag(&conn, 2), ("pick".into(), "auto".into()));
        assert_eq!(flag(&conn, 1).0, "unflagged");
        let r = apply(&mut conn, 1, TargetApplyOptions::default()).unwrap();
        assert_eq!((r.picks, r.rejects, r.unflags, r.user_flagged), (2, 1, 1, 1));
        assert_eq!(flag(&conn, 6), ("reject".into(), "auto".into()));
        assert_eq!(plan_apply(&conn, 1, TargetApplyOptions::default()).unwrap().unchanged, 5);
        assert!(plan_apply(&conn, 99, no).is_err());
    }

    /// N3-1 (v20.2): undoing an apply restores the run's `applied_at` with the flags.
    #[test]
    fn restore_apply_resets_applied_at() {
        let mut conn = setup();
        store(&mut conn);
        conn.execute(
            "INSERT INTO target_runs (project_id, target_count, shoot_type, state, started_at) VALUES (1, 2, 'wedding', 'finished', 0)",
            [],
        )
        .unwrap();
        let applied = |conn: &Connection| get_run(conn, 1, false).unwrap().unwrap().applied_at_ms;
        assert_eq!(applied(&conn), None);
        let first = apply(&mut conn, 1, TargetApplyOptions::default()).unwrap();
        assert_eq!(first.previous_applied_at_ms, None);
        assert_eq!(applied(&conn), Some(first.applied_at_ms));
        let before_plan = |conn: &Connection| plan_apply(conn, 1, TargetApplyOptions::default()).unwrap();
        // Undo: flags back, run not applied.
        let changed = restore_apply(&mut conn, 1, &first.previous, first.previous_applied_at_ms).unwrap();
        assert_eq!(changed.len(), first.changed.len());
        assert_eq!(applied(&conn), None);
        assert_eq!(flag(&conn, 1).0, "unflagged");
        let plan = before_plan(&conn);
        assert_eq!((plan.picks, plan.rejects), (first.picks, first.rejects));
        // Redo (snapshot of the applied flags + the apply's stamp), then a second apply keeps
        // the first stamp as its previous one.
        let after = repo::cull_snapshot(&conn, &first.changed).unwrap();
        restore_apply(&mut conn, 1, &after, Some(first.applied_at_ms)).unwrap();
        assert_eq!(applied(&conn), Some(first.applied_at_ms));
        conn.execute("UPDATE images SET pick = 'unflagged' WHERE id = 1", []).unwrap();
        let second = apply(&mut conn, 1, TargetApplyOptions::default()).unwrap();
        assert_eq!(second.previous_applied_at_ms, Some(first.applied_at_ms));
        restore_apply(&mut conn, 1, &second.previous, second.previous_applied_at_ms).unwrap();
        assert_eq!(applied(&conn), Some(first.applied_at_ms));
        // Unknown project / image: nothing written.
        assert!(restore_apply(&mut conn, 99, &[], None).is_err());
        let bad = vec![CullSnapshot { image_id: 999, ..first.previous[0].clone() }];
        assert!(restore_apply(&mut conn, 1, &bad, None).is_err());
        assert_eq!(applied(&conn), Some(first.applied_at_ms));
    }

    #[test]
    fn people_keep_ids_and_answers_across_reruns() {
        let mut conn = setup();
        conn.execute(
            "INSERT INTO image_analysis (image_id, status, faces_json) VALUES (1, 'done', ?1)",
            [r#"[{"bbox":{"x":0.9,"y":0.4,"width":0.1,"height":0.15},"leftEye":{"x":0.92,"y":0.45},
                "rightEye":{"x":0.97,"y":0.45},"detectionScore":0.9,"ear":0.3,"eyesOpen":1.0,"sharpness":0.8,
                "blink":false,"inFocus":true,"primary":true,"considered":true}]"#],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO thumbnails (image_id, status, path, preview_path, width, height) VALUES (1, 'ready', '/t/1.jpg', '/t/1p.jpg', 512, 341)",
            [],
        )
        .unwrap();
        let bbox = NormRect { x: 0.9, y: 0.4, width: 0.1, height: 0.15 };
        upsert_embedding(&conn, 1, 0, "m1", &bbox, &[1.0, 0.0], 1.0).unwrap();
        let p = |role, ask, faces: Vec<(ImageId, u32)>, prior| PersonDraft {
            prior_id: prior,
            suggested_role: role,
            ask,
            samples: faces.clone(),
            faces,
            centroid: vec![1.0, 0.0],
        };
        let ids = replace_people(
            &mut conn,
            1,
            &[p(PersonRole::Main, false, vec![(1, 0)], None), p(PersonRole::Unknown, true, vec![], None)],
        )
        .unwrap();
        let overview = list_people(&conn, 1).unwrap();
        assert_eq!(overview.model_version.as_deref(), Some("m1"));
        assert_eq!(overview.questions, vec![ids[1]]);
        let main = &overview.people[0];
        assert_eq!((main.id, main.role, main.photo_count), (ids[0], PersonRole::Main, 1));
        let s = &main.samples[0];
        assert_eq!(s.preview_path.as_deref(), Some("/t/1p.jpg"));
        assert!(s.crop.x >= 0.0 && s.crop.x + s.crop.width <= 1.0 + 1e-6, "{:?}", s.crop);
        assert!((s.crop.width * s.image_aspect - s.crop.height).abs() < 1e-4, "square in pixels");
        assert_eq!(selection(&conn, 1).unwrap(), None);

        // The user answers; a re-run continuing person 2 keeps the answer, a new cluster does not.
        let answered = set_person_role(&conn, ids[1], Some(PersonRole::Important)).unwrap();
        assert!(answered.role_confirmed);
        assert!(list_people(&conn, 1).unwrap().questions.is_empty());
        let again = replace_people(&mut conn, 1, &[p(PersonRole::Unknown, true, vec![], Some(ids[1]))]).unwrap();
        assert_eq!(again, vec![ids[1]]);
        let kept = person(&conn, ids[1]).unwrap();
        assert_eq!((kept.role, kept.suggested_role), (PersonRole::Important, PersonRole::Unknown));
        assert!(person(&conn, ids[0]).is_err(), "unmatched old people are removed");
        assert_eq!(people_roles(&conn, 1).unwrap(), vec![(ids[1], PersonRole::Important)]);
        assert!(set_person_role(&conn, 999, None).is_err());
    }

    #[test]
    fn face_crop_stays_inside() {
        for (b, a) in [
            (NormRect { x: 0.0, y: 0.0, width: 0.05, height: 0.08 }, 1.5),
            (NormRect { x: 0.95, y: 0.9, width: 0.05, height: 0.1 }, 0.66),
            (NormRect { x: 0.2, y: 0.1, width: 0.8, height: 0.9 }, 1.5),
        ] {
            let c = face_crop(&b, a);
            assert!(
                c.x >= -1e-6 && c.y >= -1e-6 && c.x + c.width <= 1.0 + 1e-5 && c.y + c.height <= 1.0 + 1e-5,
                "{c:?}"
            );
            assert!((c.width * a - c.height).abs() < 1e-4);
        }
    }
}
