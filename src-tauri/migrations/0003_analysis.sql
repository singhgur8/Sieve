-- LumenRAW catalog schema v3 (Phase 3: culling engine).

-- Per-image analysis state + threshold-independent measurements. `quality_scores`
-- and auto tags are derived from these and can be recomputed without ML (rescore).
-- An image needs (re)analysis when its thumbnail is ready with a preview and:
-- no row, status 'queued', model_version <> current, or analyzed_at < thumbnails.extracted_at.
CREATE TABLE image_analysis (
    image_id      INTEGER PRIMARY KEY REFERENCES images(id) ON DELETE CASCADE,
    status        TEXT NOT NULL CHECK (status IN ('queued', 'done', 'failed')),
    model_version TEXT,
    analyzed_at   INTEGER,             -- unix ms of the last attempt
    error         TEXT,                -- failure reason when status = 'failed'
    phash         INTEGER,             -- 64-bit perceptual hash, u64 bit-cast to i64
    faces_json    TEXT,                -- Vec<FaceInfo> (IPC type), rewritten on rescore
    metrics_json  TEXT                 -- ml::ImageMetrics (internal; tied to model_version)
);

CREATE INDEX idx_image_analysis_status ON image_analysis(status);

-- Engine suggestions, kept apart from the user's images.rating / images.pick.
ALTER TABLE quality_scores ADD COLUMN suggested_rating INTEGER NOT NULL DEFAULT 0
    CHECK (suggested_rating BETWEEN 0 AND 5);
ALTER TABLE quality_scores ADD COLUMN suggested_pick TEXT NOT NULL DEFAULT 'unflagged'
    CHECK (suggested_pick IN ('pick', 'reject', 'unflagged'));

-- Run analysis automatically after import / on launch. Per-shoot-type threshold
-- overrides live under keys 'cull_thresholds.<shoot_type>' (JSON, overlaid on defaults).
INSERT INTO catalog_meta (key, value) VALUES ('auto_analyze', '1');
