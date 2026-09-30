-- Sieve catalog schema v6 (Phase 6: export).
--
-- User export presets and export job history. Built-in presets live in code
-- (`ExportPreset::builtins()`, negative ids), not in this table.

CREATE TABLE export_presets (
    id            INTEGER PRIMARY KEY,
    -- Unique case-insensitively; must also not equal a built-in preset name (enforced in code).
    name          TEXT NOT NULL UNIQUE COLLATE NOCASE,
    -- ExportSettings JSON.
    settings_json TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);

-- One row per export_images call. Jobs left 'queued'/'running' by a previous session are
-- marked 'interrupted' at startup (never resumed).
CREATE TABLE export_jobs (
    id            INTEGER PRIMARY KEY,
    state         TEXT NOT NULL
                  CHECK (state IN ('queued', 'running', 'completed', 'cancelled', 'interrupted')),
    preset_name   TEXT,
    -- ExportSettings JSON as run (destination already resolved to a folder or source_folder).
    settings_json TEXT NOT NULL,
    -- Resolved destination incl. subfolder; NULL for source_folder.
    output_dir    TEXT,
    total         INTEGER NOT NULL,
    succeeded     INTEGER NOT NULL DEFAULT 0,
    failed        INTEGER NOT NULL DEFAULT 0,
    skipped       INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL,
    started_at    INTEGER,
    finished_at   INTEGER
);
CREATE INDEX idx_export_jobs_created ON export_jobs(created_at);

-- Per-image outcome; rows are inserted 'pending' at enqueue, in `ids` order (seq 0..).
-- Failure reasons for ExportJob.failures come from here.
CREATE TABLE export_items (
    job_id      INTEGER NOT NULL REFERENCES export_jobs(id) ON DELETE CASCADE,
    seq         INTEGER NOT NULL,
    image_id    INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
    status      TEXT NOT NULL DEFAULT 'pending'
                CHECK (status IN ('pending', 'done', 'failed', 'skipped')),
    -- Written file (done) or the existing file left alone (skipped).
    output_path TEXT,
    error       TEXT,
    PRIMARY KEY (job_id, seq)
);
CREATE INDEX idx_export_items_image ON export_items(image_id);
