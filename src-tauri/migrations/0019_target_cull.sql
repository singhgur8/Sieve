-- Sieve catalog schema v19 (Phase 9, IPC v20): target-count culling — people (face identity
-- clusters), moments, target runs and per-image selection. See docs/architecture.md
-- "Target-count culling". SQL lives in `db::target`; the engines in `ml::{identity, moments,
-- selection}`.

-- 1. People: face identity clusters per project. `user_role` NULL = not confirmed (the
--    effective role is then `suggested_role`). `ask` = the engine wants the user asked
--    "Is this person important?". `centroid` (f32 little-endian, `dim` values) lets re-runs
--    keep person ids (and so the user's answers) when clusters are rebuilt.
--    `sample_faces_json` = JSON `[[imageId, faceIndex], ...]`, best first (<= 6).
CREATE TABLE people (
    id                INTEGER PRIMARY KEY,
    project_id        INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    suggested_role    TEXT NOT NULL DEFAULT 'unknown'
                      CHECK (suggested_role IN ('main', 'important', 'other', 'unknown')),
    user_role         TEXT CHECK (user_role IN ('main', 'important', 'other', 'unknown')),
    ask               INTEGER NOT NULL DEFAULT 0 CHECK (ask IN (0, 1)),
    sample_faces_json TEXT NOT NULL DEFAULT '[]',
    centroid          BLOB,
    dim               INTEGER CHECK (dim IS NULL OR dim > 0),
    face_count        INTEGER NOT NULL DEFAULT 0,
    photo_count       INTEGER NOT NULL DEFAULT 0,
    created_at        INTEGER NOT NULL,
    updated_at        INTEGER NOT NULL,
    CHECK (centroid IS NULL OR length(centroid) = 4 * dim)
);
CREATE INDEX idx_people_project ON people(project_id);

-- 2. Face embeddings, one per detected face: keyed by image + index into
--    `image_analysis.faces_json` (the `get_faces` order). `embedding` = `dim` f32 values,
--    little-endian (`db::target::{encode_embedding, decode_embedding}`), L2-normalised by the
--    model wrapper. `bbox_json` = the face box (`NormRect`) the embedding was computed from: a
--    re-analysis that moves / reorders faces makes it stale (the engine compares and
--    re-embeds). `quality` 0..=1 = how usable the face is for identity (size, frontal, sharp).
--    `person_id` = cluster assignment (NULL = unassigned / too poor to cluster).
CREATE TABLE face_embeddings (
    image_id      INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
    face_index    INTEGER NOT NULL CHECK (face_index >= 0),
    model_version TEXT NOT NULL,
    dim           INTEGER NOT NULL CHECK (dim > 0),
    embedding     BLOB NOT NULL,
    bbox_json     TEXT NOT NULL,
    quality       REAL NOT NULL DEFAULT 0,
    person_id     INTEGER REFERENCES people(id) ON DELETE SET NULL,
    computed_at   INTEGER NOT NULL,
    PRIMARY KEY (image_id, face_index),
    CHECK (length(embedding) = 4 * dim)
) WITHOUT ROWID;
CREATE INDEX idx_face_embeddings_person ON face_embeddings(person_id) WHERE person_id IS NOT NULL;

-- 3. Moments: frames of the same scene + people across the shoot (members via
--    `target_selection.moment_id`). Rebuilt by every run. `person_ids_json` = JSON `[personId]`.
CREATE TABLE moments (
    id                INTEGER PRIMARY KEY,
    project_id        INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    shot_type         TEXT NOT NULL CHECK (shot_type IN ('couple', 'group', 'detail', 'candid', 'other')),
    started_at_ms     INTEGER,
    ended_at_ms       INTEGER,
    representative_id INTEGER REFERENCES images(id) ON DELETE SET NULL,
    person_ids_json   TEXT NOT NULL DEFAULT '[]'
);
CREATE INDEX idx_moments_project ON moments(project_id, started_at_ms);

-- 4. The latest target run per project.
CREATE TABLE target_runs (
    project_id    INTEGER PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    target_count  INTEGER NOT NULL CHECK (target_count > 0),
    shoot_type    TEXT NOT NULL,
    state         TEXT NOT NULL CHECK (state IN ('running', 'finished', 'failed', 'cancelled')),
    message       TEXT,
    model_version TEXT NOT NULL DEFAULT '',
    started_at    INTEGER NOT NULL,
    finished_at   INTEGER,
    applied_at    INTEGER
);

-- 5. Per-image selection of the project's run. `locked` = the user decided (target edit or
--    own flag): re-runs keep `choice` / `alternative_of` / `rank`. `alternative_of` / `rank`
--    only for `alternative`; `covered_by` = nearest delivered similar photo.
--    `reasons_json` = JSON `TargetReason[]`.
CREATE TABLE target_selection (
    image_id           INTEGER PRIMARY KEY REFERENCES images(id) ON DELETE CASCADE,
    project_id         INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    choice             TEXT NOT NULL CHECK (choice IN ('deliver', 'alternative', 'not_sure', 'set_aside')),
    moment_id          INTEGER REFERENCES moments(id) ON DELETE SET NULL,
    shot_type          TEXT CHECK (shot_type IN ('couple', 'group', 'detail', 'candid', 'other')),
    alternative_of     INTEGER REFERENCES images(id) ON DELETE SET NULL,
    rank               INTEGER CHECK (rank IS NULL OR rank >= 1),
    covered_by         INTEGER REFERENCES images(id) ON DELETE SET NULL,
    covered_similarity REAL,
    score              REAL NOT NULL DEFAULT 0,
    reasons_json       TEXT NOT NULL DEFAULT '[]',
    locked             INTEGER NOT NULL DEFAULT 0 CHECK (locked IN (0, 1)),
    updated_at         INTEGER NOT NULL
);
CREATE INDEX idx_target_selection_project ON target_selection(project_id, choice);
CREATE INDEX idx_target_selection_alt ON target_selection(alternative_of) WHERE alternative_of IS NOT NULL;
CREATE INDEX idx_target_selection_covered ON target_selection(covered_by) WHERE covered_by IS NOT NULL;
CREATE INDEX idx_target_selection_moment ON target_selection(moment_id);

-- 6. The scorer's own flag suggestion. `suggested_pick` (what every reader uses: cull summary,
--    `suggested` filter, Apply suggestions, keeper rule) becomes the *effective* suggestion:
--    with a selection row, `deliver` -> 'pick', other choices -> `scored_pick` with 'pick'
--    dropped to 'unflagged' (confident-defect rejects stay); without one, `scored_pick`.
--    Kept in step by `ml::store::write_scored` and `db::target::overlay_suggestions`.
ALTER TABLE quality_scores ADD COLUMN scored_pick TEXT NOT NULL DEFAULT 'unflagged'
    CHECK (scored_pick IN ('pick', 'reject', 'unflagged'));
UPDATE quality_scores SET scored_pick = suggested_pick;
