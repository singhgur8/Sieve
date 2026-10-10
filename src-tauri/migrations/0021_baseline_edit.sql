-- Sieve catalog schema v21 (Phase 10, IPC v21): baseline edit (preset look + per-photo light
-- over a whole shoot), its runs, per-photo results and provenance.

-- 1. Runs. Kept per project (small); `get_baseline_run` reads the newest. `settings_json` =
--    the resolved `BaselineSettings`; `anchor_json` = `BaselineAnchor` once measured;
--    `batch_id` = the one edit batch the run wrote (kind `baseline`, NULL = nothing written).
CREATE TABLE baseline_runs (
    id             INTEGER PRIMARY KEY,
    project_id     INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    anchor_id      INTEGER REFERENCES images(id) ON DELETE SET NULL,
    preset_id      INTEGER REFERENCES presets(id) ON DELETE SET NULL,
    settings_json  TEXT NOT NULL,
    state          TEXT NOT NULL CHECK (state IN ('running', 'finished', 'failed', 'cancelled')),
    message        TEXT,
    engine_version TEXT NOT NULL DEFAULT '',
    anchor_json    TEXT,
    counts_json    TEXT,
    batch_id       INTEGER REFERENCES edit_batches(id) ON DELETE SET NULL,
    started_at     INTEGER NOT NULL,
    finished_at    INTEGER
);
CREATE INDEX idx_baseline_runs_project ON baseline_runs(project_id, id);

-- 2. Per-photo results of the project's latest *finished* run only (a finished run replaces
--    the rows of the project's earlier runs; failed / cancelled runs keep them).
--    `reasons_json` = `BaselineReason[]`; `auto_json` / `light_json` = `LightValues` or NULL.
CREATE TABLE baseline_results (
    run_id         INTEGER NOT NULL REFERENCES baseline_runs(id) ON DELETE CASCADE,
    image_id       INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
    outcome        TEXT NOT NULL
                   CHECK (outcome IN ('applied', 'flagged', 'skipped_edited', 'anchor', 'failed')),
    reasons_json   TEXT NOT NULL DEFAULT '[]',
    scene_id       INTEGER,
    burst_group_id INTEGER,
    auto_json      TEXT,
    light_json     TEXT,
    PRIMARY KEY (run_id, image_id)
) WITHOUT ROWID;
CREATE INDEX idx_baseline_results_image ON baseline_results(image_id, outcome);

-- 3. Provenance: the last baseline run that wrote each photo's settings. The photo is "on the
--    baseline" while its history cursor is that entry (`adjustments.history_entry_id =
--    history_entry_id` and the entry's `batch_id = batch_id`); any later edit, per-photo undo
--    or other batch moves the cursor (user_edited); undoing the batch -> undone. Derived on
--    read, no triggers. A re-run that changes the photo replaces the row.
CREATE TABLE baseline_provenance (
    image_id         INTEGER PRIMARY KEY REFERENCES images(id) ON DELETE CASCADE,
    run_id           INTEGER NOT NULL REFERENCES baseline_runs(id) ON DELETE CASCADE,
    batch_id         INTEGER NOT NULL REFERENCES edit_batches(id) ON DELETE CASCADE,
    history_entry_id INTEGER NOT NULL,
    flagged          INTEGER NOT NULL DEFAULT 0,
    applied_at       INTEGER NOT NULL
);
CREATE INDEX idx_baseline_provenance_run ON baseline_provenance(run_id);

-- 4. New edit batch kind `baseline` and history source `baseline` (`EditBatchKind::Baseline`,
--    `EditSource::Baseline`). Relax both CHECKs with the documented writable_schema edit (see
--    0009 / 0017 / 0018). Idempotent.
PRAGMA writable_schema = ON;
UPDATE sqlite_schema
   SET sql = replace(sql,
                     'CHECK (kind IN (''scene_apply'', ''style_prediction'', ''paste'', ''sync''))',
                     'CHECK (kind IN (''scene_apply'', ''style_prediction'', ''paste'', ''sync'', ''baseline''))')
 WHERE type = 'table' AND name = 'edit_batches';
UPDATE sqlite_schema
   SET sql = replace(sql,
                     'CHECK (source IN (''user'', ''pasted'', ''auto_style'', ''scene_apply'', ''sidecar''))',
                     'CHECK (source IN (''user'', ''pasted'', ''auto_style'', ''scene_apply'', ''sidecar'', ''baseline''))')
 WHERE type = 'table' AND name = 'adjustment_history';
PRAGMA writable_schema = RESET;
