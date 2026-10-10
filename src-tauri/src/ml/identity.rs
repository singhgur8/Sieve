//! Face identity (Phase 9, IPC v20): face embeddings and per-project clustering into people.
//!
//! Owned by vision-ml-dev. The architect fixed the surface used by `ml::selection`'s pipeline:
//! [`update_people`] and its [`IdentityOutcome`]. [`cluster_people`] / [`suggest_roles`] are the
//! pure seam (deterministic tests on synthetic embeddings).
//!
//! Contract (docs/architecture.md, "Target-count culling"):
//! - Faces = `image_analysis.faces_json` entries (`get_faces` order = `metrics_json.faces`);
//!   embeddings are stored per (image, face index) with `db::target::upsert_embedding` (f32 LE
//!   BLOB + `dim`, L2-normalised, `model_version`, the face box it came from); stale ones (box
//!   moved / other model) are recomputed. Embed from the 2048 px preview like the analysis.
//! - Clusters are stored with `db::target::replace_people`; a cluster continuing an existing
//!   person sets `prior_id` (match by centroid) so person ids, and with them the user's
//!   answers (`people.user_role`), survive re-runs.
//! - Main subject: the most frequent pair of faces appearing together (frequency, size,
//!   centrality); Portrait: the single most frequent face. Other recurring people get
//!   `ask = true` ("Is this person important?"). Guests seen once or twice: `other`, no ask.
//! - Without a model (not installed) return `people: None`: the stored people stay as they
//!   are and the selection runs without people.
//!
//! Pipeline ([`update_people`]):
//! 1. Plan: every analysed photo of the project; per face the identity quality
//!    ([`embed::face_quality`]: size, pose, blur, detector score). Faces below
//!    [`EMBED_MIN_QUALITY`] are junk (tiny background faces, profiles) and never embedded.
//! 2. Backfill: faces without a current embedding are embedded on a small thread pool (one
//!    SCRFD + ArcFace session pair per thread): decode the preview, re-detect for the 5
//!    keypoints (the analysis keeps only the eyes), match by box, align, embed.
//! 3. [`cluster_people`]: quality-ordered leader clustering of good faces, agglomerative
//!    merge of clusters by quality-weighted centroid, then one reassignment pass of every
//!    embedded face (weaker faces need a higher similarity). A person never has two faces in
//!    one photo. Clusters seen in fewer than [`MIN_PERSON_PHOTOS`] photos are not people.
//! 4. [`suggest_roles`]: main pair / portrait subject and the people to ask about.
//!
//! The selection engine reads the result with [`image_people`] (person + effective role per
//! face) or `db::target::people_roles`.

mod embed;

pub use embed::{
    align, face_quality, similarity, FaceEmbedder, FacePipeline, FaceRequest, ARCFACE_DST, EMBED_DIM, EMBED_MODEL,
    EMBED_MODEL_VERSION, EMBED_SIZE,
};

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;

use rusqlite::{params, Connection};

use crate::db::target::{self, PersonDraft, StoredEmbedding, MAX_SAMPLES};
use crate::ipc::error::AppResult;
use crate::ipc::types::{ImageId, NormRect, PersonId, PersonRole, ProjectId, ShootType};
use crate::ml::ImageMetrics;

/// Faces below this identity quality are not embedded (junk: tiny, profile, very soft).
pub const EMBED_MIN_QUALITY: f32 = 0.15;
/// Faces at or above this quality may found / shape clusters; weaker ones only join.
pub const SEED_MIN_QUALITY: f32 = 0.35;
/// Cosine similarity for a good face to join a cluster (face vs. centroid).
pub const ASSIGN_SIM: f32 = 0.40;
/// Extra similarity a face below [`SEED_MIN_QUALITY`] needs to join.
pub const WEAK_FACE_MARGIN: f32 = 0.08;
/// Cosine similarity of two cluster centroids to merge them.
pub const MERGE_SIM: f32 = 0.45;
/// A new cluster continues a prior person when their centroids are this similar.
pub const PRIOR_SIM: f32 = 0.50;
/// Clusters seen in fewer photos are not people (one-off guests).
pub const MIN_PERSON_PHOTOS: usize = 2;
/// A main subject appears in at least this many photos.
pub const MIN_MAIN_PHOTOS: usize = 3;
/// The main pair appears together in at least this many photos ...
pub const MIN_PAIR_PHOTOS: usize = 3;
/// ... and the less prominent of the two has at least this share of the most prominent
/// person's weight (otherwise the shoot has one main subject).
pub const PAIR_MIN_REL: f32 = 0.3;
/// Ask about at most this many people (most photos first) ...
pub const ASK_MAX: usize = 8;
/// ... seen in at least this many photos ...
pub const ASK_MIN_PHOTOS: usize = 4;
/// ... and in at least this share of the photos with people.
pub const ASK_MIN_SHARE: f32 = 0.01;
/// Worker threads for the embedding backfill (each holds its own ~200 MB of sessions).
const MAX_EMBED_THREADS: usize = 4;
/// Box coordinates match when they differ by less than this (normalised units).
const BOX_EPS: f32 = 1e-4;

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
    /// User-facing note for the run message (e.g. "Face recognition is not installed").
    pub message: Option<String>,
}

/// An existing person, for matching new clusters to old ids.
#[derive(Debug, Clone, PartialEq)]
pub struct PriorPerson {
    pub id: PersonId,
    pub centroid: Vec<f32>,
    pub user_role: Option<PersonRole>,
}

/// One analysed photo's embedding work.
#[derive(Debug, Clone, PartialEq)]
struct ImageWork {
    image_id: ImageId,
    preview: PathBuf,
    faces: Vec<FaceRequest>,
}

/// Faces of the project that are usable for identity, with their current box and quality.
#[derive(Debug, Default)]
struct Plan {
    /// (image, face index) -> (box, quality) of every face with quality >= the minimum.
    valid: HashMap<(ImageId, u32), (NormRect, f32)>,
    /// Photos with faces lacking a current embedding.
    work: Vec<ImageWork>,
    /// Stored rows to drop (face gone or no longer usable).
    stale: Vec<(ImageId, u32)>,
}

fn same_box(a: &NormRect, b: &NormRect) -> bool {
    (a.x - b.x).abs() < BOX_EPS
        && (a.y - b.y).abs() < BOX_EPS
        && (a.width - b.width).abs() < BOX_EPS
        && (a.height - b.height).abs() < BOX_EPS
}

fn plan(conn: &Connection, project_id: ProjectId, stored: &[StoredEmbedding]) -> AppResult<Plan> {
    let stored: HashMap<(ImageId, u32), &StoredEmbedding> =
        stored.iter().map(|e| ((e.image_id, e.face_index), e)).collect();
    let mut stmt = conn.prepare(
        "SELECT a.image_id, a.metrics_json, t.preview_path
         FROM image_analysis a JOIN images i ON i.id = a.image_id JOIN folders f ON f.id = i.folder_id
         LEFT JOIN thumbnails t ON t.image_id = a.image_id
         WHERE f.project_id = ?1 AND a.status = 'done' AND a.metrics_json IS NOT NULL
         ORDER BY a.image_id",
    )?;
    let rows = stmt.query_map([project_id], |r| {
        Ok((r.get::<_, ImageId>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?))
    })?;
    let mut out = Plan::default();
    let mut seen: BTreeSet<(ImageId, u32)> = BTreeSet::new();
    for row in rows {
        let (image_id, metrics, preview) = row?;
        let Ok(metrics) = serde_json::from_str::<ImageMetrics>(&metrics) else { continue };
        let mut faces = Vec::new();
        for (i, f) in metrics.faces.iter().enumerate() {
            let key = (image_id, i as u32);
            let q = face_quality(f, metrics.height);
            if q < EMBED_MIN_QUALITY {
                continue;
            }
            seen.insert(key);
            out.valid.insert(key, (f.bbox, q));
            let current =
                stored.get(&key).is_some_and(|e| e.model_version == EMBED_MODEL_VERSION && same_box(&e.bbox, &f.bbox));
            if !current {
                faces.push(FaceRequest { face_index: i as u32, bbox: f.bbox });
            }
        }
        if let (false, Some(p)) = (faces.is_empty(), preview) {
            out.work.push(ImageWork { image_id, preview: PathBuf::from(p), faces });
        }
    }
    // Rows of faces that vanished / became junk, and rows of other images of the project
    // that are no longer analysed.
    out.stale = stored.keys().filter(|k| !seen.contains(k)).copied().collect();
    out.stale.sort_unstable();
    Ok(out)
}

fn delete_embedding(conn: &Connection, image_id: ImageId, face_index: u32) -> AppResult<()> {
    conn.prepare_cached("DELETE FROM face_embeddings WHERE image_id = ?1 AND face_index = ?2")?
        .execute(params![image_id, face_index])?;
    Ok(())
}

/// The project's people as stored (for continuing ids).
fn prior_people(conn: &Connection, project_id: ProjectId) -> AppResult<Vec<PriorPerson>> {
    let mut stmt = conn.prepare("SELECT id, centroid, dim, user_role FROM people WHERE project_id = ?1 ORDER BY id")?;
    let rows = stmt.query_map([project_id], |r| {
        Ok((
            r.get::<_, PersonId>(0)?,
            r.get::<_, Option<Vec<u8>>>(1)?,
            r.get::<_, Option<i64>>(2)?,
            r.get::<_, Option<String>>(3)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, blob, dim, role) = row?;
        let centroid = match (blob, dim) {
            (Some(b), Some(d)) if d > 0 => target::decode_embedding(&b, d as usize).unwrap_or_default(),
            _ => Vec::new(),
        };
        out.push(PriorPerson { id, centroid, user_role: role.as_deref().and_then(PersonRole::parse) });
    }
    Ok(out)
}

/// Result of one image of the backfill.
type EmbedResult = (ImageId, Result<Vec<(u32, Option<Vec<f32>>)>, String>);

/// Embeds `work` on a small pool; `sink` gets each image's result on the calling thread.
/// Returns an error when no worker could load the models.
fn embed_parallel(
    models_dir: &Path,
    work: &[ImageWork],
    cancel: &AtomicBool,
    sink: &mut dyn FnMut(EmbedResult) -> AppResult<()>,
) -> AppResult<Result<(), String>> {
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, MAX_EMBED_THREADS);
    let threads = threads.min(work.len().max(1));
    let next = AtomicUsize::new(0);
    let (tx, rx) = mpsc::channel::<Result<EmbedResult, String>>();
    let mut sink_err = None;
    let mut load_errors = Vec::new();
    std::thread::scope(|s| {
        for _ in 0..threads {
            let tx = tx.clone();
            let next = &next;
            s.spawn(move || {
                let mut pipe = match FacePipeline::load(models_dir) {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        return;
                    }
                };
                loop {
                    if cancel.load(Ordering::SeqCst) {
                        return;
                    }
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(item) = work.get(i) else { return };
                    let res = pipe
                        .embed_preview(&item.preview, &item.faces)
                        .map(|vs| item.faces.iter().map(|f| f.face_index).zip(vs).collect::<Vec<_>>());
                    if tx.send(Ok((item.image_id, res))).is_err() {
                        return;
                    }
                }
            });
        }
        drop(tx);
        for msg in rx {
            match msg {
                Ok(r) if sink_err.is_none() => {
                    if let Err(e) = sink(r) {
                        sink_err = Some(e);
                        cancel_local(&next, work.len());
                    }
                }
                Ok(_) => {}
                Err(e) => load_errors.push(e),
            }
        }
    });
    if let Some(e) = sink_err {
        return Err(e);
    }
    if load_errors.len() == threads {
        return Ok(Err(load_errors.swap_remove(0)));
    }
    Ok(Ok(()))
}

/// Makes the workers run out of work (used when the sink fails).
fn cancel_local(next: &AtomicUsize, len: usize) {
    next.store(len, Ordering::SeqCst);
}

/// Embeds the project's faces that lack a current embedding and clusters them into people.
/// `progress(done, total)` may be called often (the caller throttles). Checks `cancel`
/// between images (a cancelled run keeps what was embedded and returns `people: None`).
pub fn update_people(
    conn: &mut Connection,
    project_id: ProjectId,
    shoot_type: ShootType,
    models_dir: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u32, Option<u32>),
) -> AppResult<IdentityOutcome> {
    if !models_dir.join(EMBED_MODEL).is_file() {
        return Ok(IdentityOutcome {
            model_version: None,
            embedded: 0,
            people: None,
            message: Some("Face recognition is not installed (run scripts/fetch-models.sh)".to_owned()),
        });
    }
    let stored = target::project_embeddings(conn, project_id)?;
    let plan = plan(conn, project_id, &stored)?;
    {
        let tx = conn.transaction()?;
        for &(image, face) in &plan.stale {
            delete_embedding(&tx, image, face)?;
        }
        tx.commit()?;
    }
    let total = plan.work.len() as u32;
    progress(0, Some(total));
    let mut embedded = 0u32;
    if !plan.work.is_empty() {
        let mut done = 0u32;
        let mut batch: Vec<EmbedResult> = Vec::new();
        let valid = &plan.valid;
        let flush = |conn: &mut Connection, batch: &mut Vec<EmbedResult>| -> AppResult<u32> {
            let tx = conn.transaction()?;
            let mut n = 0;
            for (image, res) in batch.drain(..) {
                let Ok(faces) = res else {
                    // Unreadable preview: retried next run (stale rows are not clustered).
                    continue;
                };
                for (face, vec) in faces {
                    match (vec, valid.get(&(image, face))) {
                        (Some(v), Some((bbox, q))) => {
                            target::upsert_embedding(&tx, image, face, EMBED_MODEL_VERSION, bbox, &v, *q)?;
                            n += 1;
                        }
                        _ => delete_embedding(&tx, image, face)?,
                    }
                }
            }
            tx.commit()?;
            Ok(n)
        };
        let loaded = embed_parallel(models_dir, &plan.work, cancel, &mut |r| {
            batch.push(r);
            done += 1;
            progress(done, Some(total));
            if batch.len() >= 32 {
                embedded += flush(conn, &mut batch)?;
            }
            Ok(())
        })?;
        embedded += flush(conn, &mut batch)?;
        if let Err(e) = loaded {
            eprintln!("[identity] face recognition unavailable: {e}");
            return Ok(IdentityOutcome {
                model_version: None,
                embedded,
                people: None,
                message: Some("Face recognition could not start; people were not updated".to_owned()),
            });
        }
    }
    if cancel.load(Ordering::SeqCst) {
        return Ok(IdentityOutcome {
            model_version: Some(EMBED_MODEL_VERSION.to_owned()),
            embedded,
            people: None,
            message: None,
        });
    }
    let faces: Vec<StoredEmbedding> = target::project_embeddings(conn, project_id)?
        .into_iter()
        .filter(|e| {
            e.model_version == EMBED_MODEL_VERSION
                && e.vector.len() == EMBED_DIM
                && plan.valid.get(&(e.image_id, e.face_index)).is_some_and(|(b, _)| same_box(b, &e.bbox))
        })
        .collect();
    let prior = prior_people(conn, project_id)?;
    let mut people = cluster_people(&faces, &prior);
    suggest_roles(&mut people, &faces, shoot_type);
    Ok(IdentityOutcome {
        model_version: Some(EMBED_MODEL_VERSION.to_owned()),
        embedded,
        people: Some(people),
        message: None,
    })
}

// ---------------------------------------------------------------------------
// Clustering
// ---------------------------------------------------------------------------

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// A cluster under construction.
#[derive(Debug, Clone)]
struct Cluster {
    /// Quality-weighted sum of member vectors.
    sum: Vec<f32>,
    /// Normalised `sum`.
    centroid: Vec<f32>,
    /// Indices into the face list.
    members: Vec<usize>,
    images: BTreeSet<ImageId>,
}

impl Cluster {
    fn new(dim: usize) -> Self {
        Self { sum: vec![0.0; dim], centroid: vec![0.0; dim], members: Vec::new(), images: BTreeSet::new() }
    }

    fn add(&mut self, i: usize, f: &StoredEmbedding) {
        let w = f.quality.max(0.05);
        self.sum.iter_mut().zip(&f.vector).for_each(|(s, v)| *s += w * v);
        self.members.push(i);
        self.images.insert(f.image_id);
        self.refresh();
    }

    fn absorb(&mut self, other: Cluster) {
        self.sum.iter_mut().zip(&other.sum).for_each(|(s, v)| *s += v);
        self.members.extend(other.members);
        self.images.extend(other.images);
        self.refresh();
    }

    fn refresh(&mut self) {
        self.centroid.clone_from(&self.sum);
        embed::normalize(&mut self.centroid);
    }

    /// Two clusters of one person almost never share a photo (only through a wrong face).
    fn may_merge(&self, other: &Cluster) -> bool {
        let shared = self.images.intersection(&other.images).count();
        let smaller = self.images.len().min(other.images.len());
        shared * 20 <= smaller
    }
}

/// Clustering thresholds (defaults = the module constants; the evaluation sweeps them).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClusterParams {
    pub seed_min_quality: f32,
    pub assign_sim: f32,
    pub weak_face_margin: f32,
    pub merge_sim: f32,
    pub prior_sim: f32,
    pub min_person_photos: usize,
}

impl Default for ClusterParams {
    fn default() -> Self {
        Self {
            seed_min_quality: SEED_MIN_QUALITY,
            assign_sim: ASSIGN_SIM,
            weak_face_margin: WEAK_FACE_MARGIN,
            merge_sim: MERGE_SIM,
            prior_sim: PRIOR_SIM,
            min_person_photos: MIN_PERSON_PHOTOS,
        }
    }
}

/// Clusters `faces` into people, continuing `prior` people where centroids match. Pure and
/// deterministic. Faces of another dimension than the first are ignored. Drafts come out
/// most photos first with `suggested_role = unknown`, `ask = false` (see [`suggest_roles`]).
pub fn cluster_people(faces: &[StoredEmbedding], prior: &[PriorPerson]) -> Vec<PersonDraft> {
    cluster_people_with(faces, prior, &ClusterParams::default())
}

/// [`cluster_people`] with explicit thresholds.
pub fn cluster_people_with(faces: &[StoredEmbedding], prior: &[PriorPerson], p: &ClusterParams) -> Vec<PersonDraft> {
    let Some(dim) = faces.first().map(|f| f.vector.len()).filter(|&d| d > 0) else { return Vec::new() };
    let usable: Vec<usize> = (0..faces.len()).filter(|&i| faces[i].vector.len() == dim).collect();
    // Best faces first; ties by (image, face) for determinism.
    let mut order = usable.clone();
    order.sort_by(|&a, &b| {
        faces[b]
            .quality
            .total_cmp(&faces[a].quality)
            .then(faces[a].image_id.cmp(&faces[b].image_id))
            .then(faces[a].face_index.cmp(&faces[b].face_index))
    });

    // 1. Leader clustering of good faces.
    let mut clusters: Vec<Cluster> = Vec::new();
    for &i in order.iter().filter(|&&i| faces[i].quality >= p.seed_min_quality) {
        let f = &faces[i];
        let best = clusters
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.images.contains(&f.image_id))
            .map(|(k, c)| (k, dot(&c.centroid, &f.vector)))
            .filter(|&(_, s)| s >= p.assign_sim)
            .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)));
        match best {
            Some((k, _)) => clusters[k].add(i, f),
            None => {
                let mut c = Cluster::new(dim);
                c.add(i, f);
                clusters.push(c);
            }
        }
    }

    // 2. Agglomerative merge by centroid similarity (best pair first).
    merge_clusters(&mut clusters, p.merge_sim);

    // 3. Reassign every face to its best centroid (one face per photo per person).
    let centroids: Vec<Vec<f32>> = clusters.iter().map(|c| c.centroid.clone()).collect();
    let mut cands: Vec<(f32, usize, usize)> = Vec::new();
    for &i in &usable {
        let f = &faces[i];
        let need = if f.quality >= p.seed_min_quality { p.assign_sim } else { p.assign_sim + p.weak_face_margin };
        for (k, c) in centroids.iter().enumerate() {
            let s = dot(c, &f.vector);
            if s >= need {
                cands.push((s, i, k));
            }
        }
    }
    cands.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut assigned = vec![false; faces.len()];
    let mut taken: BTreeSet<(usize, ImageId)> = BTreeSet::new();
    let mut fresh: Vec<Cluster> = (0..centroids.len()).map(|_| Cluster::new(dim)).collect();
    for (_, i, k) in cands {
        if assigned[i] || !taken.insert((k, faces[i].image_id)) {
            continue;
        }
        assigned[i] = true;
        fresh[k].add(i, &faces[i]);
    }

    // 4. People = clusters seen in enough photos.
    let mut people: Vec<Cluster> = fresh.into_iter().filter(|c| c.images.len() >= p.min_person_photos).collect();
    people.sort_by(|a, b| b.images.len().cmp(&a.images.len()).then(a.images.first().cmp(&b.images.first())));

    // 5. Continue prior people (greedy, most similar pair first).
    let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
    for (k, c) in people.iter().enumerate() {
        for (j, pp) in prior.iter().enumerate() {
            if pp.centroid.len() == dim {
                let s = dot(&c.centroid, &pp.centroid);
                if s >= p.prior_sim {
                    pairs.push((s, k, j));
                }
            }
        }
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut prior_of: Vec<Option<PersonId>> = vec![None; people.len()];
    let mut used = vec![false; prior.len()];
    for (_, k, j) in pairs {
        if prior_of[k].is_none() && !used[j] {
            prior_of[k] = Some(prior[j].id);
            used[j] = true;
        }
    }

    people
        .into_iter()
        .zip(prior_of)
        .map(|(c, prior_id)| {
            let mut members = c.members.clone();
            members.sort_by_key(|&i| (faces[i].image_id, faces[i].face_index));
            let mut by_quality = members.clone();
            by_quality.sort_by(|&a, &b| {
                faces[b].quality.total_cmp(&faces[a].quality).then(faces[a].image_id.cmp(&faces[b].image_id))
            });
            PersonDraft {
                prior_id,
                suggested_role: PersonRole::Unknown,
                ask: false,
                faces: members.iter().map(|&i| (faces[i].image_id, faces[i].face_index)).collect(),
                samples: by_quality
                    .iter()
                    .take(MAX_SAMPLES)
                    .map(|&i| (faces[i].image_id, faces[i].face_index))
                    .collect(),
                centroid: c.centroid,
            }
        })
        .collect()
}

/// Merges the most similar pair of clusters until no pair reaches `merge_sim` (best pair
/// first; a max-heap of candidate pairs with lazy invalidation, so each merge costs one
/// row of similarities instead of a full rescan).
fn merge_clusters(clusters: &mut Vec<Cluster>, merge_sim: f32) {
    use std::cmp::Ordering as Cmp;
    use std::collections::BinaryHeap;

    /// (similarity, a, b, version of a, version of b); ordered by similarity, then the
    /// smaller indices first (deterministic).
    #[derive(PartialEq)]
    struct Cand(f32, usize, usize, u32, u32);
    impl Eq for Cand {}
    impl PartialOrd for Cand {
        fn partial_cmp(&self, o: &Self) -> Option<Cmp> {
            Some(self.cmp(o))
        }
    }
    impl Ord for Cand {
        fn cmp(&self, o: &Self) -> Cmp {
            self.0.total_cmp(&o.0).then(o.1.cmp(&self.1)).then(o.2.cmp(&self.2))
        }
    }

    let n = clusters.len();
    if n < 2 {
        return;
    }
    let mut alive = vec![true; n];
    let mut version = vec![0u32; n];
    let mut heap = BinaryHeap::new();
    for a in 0..n {
        for b in a + 1..n {
            let s = dot(&clusters[a].centroid, &clusters[b].centroid);
            if s >= merge_sim {
                heap.push(Cand(s, a, b, 0, 0));
            }
        }
    }
    while let Some(Cand(_, a, b, va, vb)) = heap.pop() {
        if !alive[a] || !alive[b] || version[a] != va || version[b] != vb {
            continue;
        }
        if !clusters[a].may_merge(&clusters[b]) {
            continue; // re-offered if either side changes
        }
        let other = std::mem::replace(&mut clusters[b], Cluster::new(0));
        clusters[a].absorb(other);
        alive[b] = false;
        version[a] += 1;
        for c in (0..n).filter(|&c| alive[c] && c != a) {
            let s = dot(&clusters[a].centroid, &clusters[c].centroid);
            if s >= merge_sim {
                let (x, y) = if a < c { (a, c) } else { (c, a) };
                heap.push(Cand(s, x, y, version[x], version[y]));
            }
        }
    }
    let mut k = 0;
    clusters.retain(|_| {
        k += 1;
        alive[k - 1]
    });
}

// ---------------------------------------------------------------------------
// Roles
// ---------------------------------------------------------------------------

/// How prominent a face is in its photo, 0.25..=1: size relative to the photo's largest face
/// and closeness to the frame centre.
fn prominence(bbox: &NormRect, largest_h: f32) -> f32 {
    let rel = if largest_h > 0.0 { (bbox.height / largest_h).clamp(0.0, 1.0) } else { 1.0 };
    let (cx, cy) = (bbox.x + bbox.width / 2.0, bbox.y + bbox.height / 2.0);
    let off = (((cx - 0.5).powi(2) + (cy - 0.5).powi(2)).sqrt() / std::f32::consts::FRAC_1_SQRT_2).clamp(0.0, 1.0);
    (0.5 + 0.5 * rel) * (1.0 - 0.5 * off)
}

/// Per-person statistics for role suggestion.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoleStats {
    /// Photos per person.
    pub photos: Vec<usize>,
    /// Sum of face prominence per person.
    pub weight: Vec<f32>,
    /// (a, b) with a < b -> (photos together, sum of min prominence).
    pub together: BTreeMap<(usize, usize), (usize, f32)>,
    /// Photos with at least one person.
    pub photos_with_people: usize,
}

/// Co-occurrence statistics of `people` (face boxes from `faces`).
pub fn role_stats(people: &[PersonDraft], faces: &[StoredEmbedding]) -> RoleStats {
    let boxes: HashMap<(ImageId, u32), NormRect> = faces.iter().map(|f| ((f.image_id, f.face_index), f.bbox)).collect();
    let mut largest: HashMap<ImageId, f32> = HashMap::new();
    for f in faces {
        let e = largest.entry(f.image_id).or_insert(0.0);
        *e = e.max(f.bbox.height);
    }
    let mut per_image: BTreeMap<ImageId, Vec<(usize, f32)>> = BTreeMap::new();
    let mut stats = RoleStats { photos: vec![0; people.len()], weight: vec![0.0; people.len()], ..Default::default() };
    for (p, person) in people.iter().enumerate() {
        let mut best: BTreeMap<ImageId, f32> = BTreeMap::new();
        for key in &person.faces {
            let prom = boxes.get(key).map_or(0.25, |b| prominence(b, largest.get(&key.0).copied().unwrap_or(b.height)));
            let e = best.entry(key.0).or_insert(0.0);
            *e = e.max(prom);
        }
        stats.photos[p] = best.len();
        stats.weight[p] = best.values().sum();
        for (img, prom) in best {
            per_image.entry(img).or_default().push((p, prom));
        }
    }
    stats.photos_with_people = per_image.len();
    for list in per_image.values() {
        for (i, &(a, pa)) in list.iter().enumerate() {
            for &(b, pb) in &list[i + 1..] {
                let key = if a < b { (a, b) } else { (b, a) };
                let e = stats.together.entry(key).or_insert((0, 0.0));
                e.0 += 1;
                e.1 += pa.min(pb);
            }
        }
    }
    stats
}

/// The main subject(s) by index into the people: the pair that appears together most
/// (weighted by size and centrality, plus the weaker partner's own weight), or the single
/// most prominent person (Portrait, or no convincing pair). Pure.
pub fn main_subjects(stats: &RoleStats, shoot_type: ShootType) -> Vec<usize> {
    let n = stats.photos.len();
    let top = (0..n)
        .filter(|&p| stats.photos[p] >= MIN_MAIN_PHOTOS)
        .max_by(|&a, &b| stats.weight[a].total_cmp(&stats.weight[b]).then(b.cmp(&a)));
    let Some(top) = top else { return Vec::new() };
    if shoot_type == ShootType::Portrait {
        return vec![top];
    }
    let w_top = stats.weight[top];
    let best = stats
        .together
        .iter()
        .filter(|(&(a, b), &(count, _))| {
            count >= MIN_PAIR_PHOTOS
                && stats.photos[a] >= MIN_MAIN_PHOTOS
                && stats.photos[b] >= MIN_MAIN_PHOTOS
                && stats.weight[a].min(stats.weight[b]) >= PAIR_MIN_REL * w_top
        })
        .map(|(&(a, b), &(_, co))| (co + 0.5 * stats.weight[a].min(stats.weight[b]), a, b))
        .max_by(|x, y| x.0.total_cmp(&y.0).then(y.1.cmp(&x.1)).then(y.2.cmp(&x.2)));
    match best {
        Some((_, a, b)) => vec![a, b],
        None => vec![top],
    }
}

/// Sets `suggested_role` / `ask` of `people`: the main pair (or portrait subject) is `main`;
/// recurring people (>= [`ASK_MIN_PHOTOS`] photos and [`ASK_MIN_SHARE`] of the photos with
/// people), at most [`ASK_MAX`] by photo count, are asked about (`unknown` until answered);
/// everyone else is `other`. `faces` supplies the face boxes (size, centrality).
pub fn suggest_roles(people: &mut [PersonDraft], faces: &[StoredEmbedding], shoot_type: ShootType) {
    let stats = role_stats(people, faces);
    let mains = main_subjects(&stats, shoot_type);
    let ask_min = ASK_MIN_PHOTOS.max((ASK_MIN_SHARE * stats.photos_with_people as f32).ceil() as usize);
    let mut askable: Vec<usize> =
        (0..people.len()).filter(|p| !mains.contains(p) && stats.photos[*p] >= ask_min).collect();
    askable.sort_by(|&a, &b| {
        stats.photos[b].cmp(&stats.photos[a]).then(stats.weight[b].total_cmp(&stats.weight[a])).then(a.cmp(&b))
    });
    askable.truncate(ASK_MAX);
    for (p, person) in people.iter_mut().enumerate() {
        let (role, ask) = if mains.contains(&p) {
            (PersonRole::Main, false)
        } else if askable.contains(&p) {
            (PersonRole::Unknown, true)
        } else {
            (PersonRole::Other, false)
        };
        person.suggested_role = role;
        person.ask = ask;
    }
}

// ---------------------------------------------------------------------------
// Read side for the selection engine
// ---------------------------------------------------------------------------

/// A face of a photo that belongs to a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FacePerson {
    /// Index into `faces_json` (`get_faces` order).
    pub face_index: u32,
    pub person_id: PersonId,
    /// Effective role (the user's answer, else the suggestion).
    pub role: PersonRole,
}

/// People per photo of the project (photos without recognised people are absent), faces in
/// `faces_json` order. What `ml::moments` / `ml::selection` read after [`update_people`] +
/// `db::target::replace_people`.
pub fn image_people(conn: &Connection, project_id: ProjectId) -> AppResult<HashMap<ImageId, Vec<FacePerson>>> {
    let mut stmt = conn.prepare(
        "SELECT e.image_id, e.face_index, p.id, COALESCE(p.user_role, p.suggested_role)
         FROM face_embeddings e JOIN people p ON p.id = e.person_id
         WHERE p.project_id = ?1 ORDER BY e.image_id, e.face_index",
    )?;
    let rows = stmt.query_map([project_id], |r| {
        Ok((r.get::<_, ImageId>(0)?, r.get::<_, u32>(1)?, r.get::<_, PersonId>(2)?, r.get::<_, String>(3)?))
    })?;
    let mut out: HashMap<ImageId, Vec<FacePerson>> = HashMap::new();
    for row in rows {
        let (image, face_index, person_id, role) = row?;
        let role = PersonRole::parse(&role).unwrap_or(PersonRole::Unknown);
        out.entry(image).or_default().push(FacePerson { face_index, person_id, role });
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
