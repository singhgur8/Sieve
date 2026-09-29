-- LumenRAW catalog schema v1.
-- Enum-like TEXT columns store the same snake_case strings used on the IPC wire.

CREATE TABLE catalog_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

INSERT INTO catalog_meta (key, value) VALUES
    ('shoot_type', 'general'),
    ('burst_window_ms', '1500');

CREATE TABLE folders (
    id       INTEGER PRIMARY KEY,
    path     TEXT NOT NULL UNIQUE,
    added_at INTEGER NOT NULL            -- unix ms
);

CREATE TABLE burst_groups (
    id              INTEGER PRIMARY KEY,
    started_at_ms   INTEGER NOT NULL,
    ended_at_ms     INTEGER NOT NULL,
    keeper_image_id INTEGER              -- no FK: avoids a cycle with images
);

CREATE TABLE images (
    id              INTEGER PRIMARY KEY,
    folder_id       INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
    path            TEXT NOT NULL UNIQUE,
    file_name       TEXT NOT NULL,
    format          TEXT NOT NULL CHECK (format IN ('arw', 'raf', 'cr3')),
    camera_make     TEXT NOT NULL,
    camera_model    TEXT,
    sensor_layout   TEXT NOT NULL DEFAULT 'unknown'
                    CHECK (sensor_layout IN ('bayer', 'x_trans', 'unknown')),
    lens            TEXT,
    captured_at_ms  INTEGER,
    iso             INTEGER,
    shutter_s       REAL,
    aperture        REAL,
    focal_length_mm REAL,
    width           INTEGER,
    height          INTEGER,
    orientation     INTEGER,
    file_size       INTEGER NOT NULL,
    file_mtime_ms   INTEGER NOT NULL,
    rating          INTEGER NOT NULL DEFAULT 0 CHECK (rating BETWEEN 0 AND 5),
    pick            TEXT NOT NULL DEFAULT 'unflagged'
                    CHECK (pick IN ('pick', 'reject', 'unflagged')),
    color_label     TEXT,
    burst_group_id  INTEGER REFERENCES burst_groups(id) ON DELETE SET NULL,
    imported_at     INTEGER NOT NULL
);

CREATE INDEX idx_images_captured_at ON images(captured_at_ms);
CREATE INDEX idx_images_folder      ON images(folder_id);
CREATE INDEX idx_images_burst       ON images(burst_group_id);

-- Thumbnail pixels live as files in the app cache dir; this table only tracks them.
CREATE TABLE thumbnails (
    image_id     INTEGER PRIMARY KEY REFERENCES images(id) ON DELETE CASCADE,
    status       TEXT NOT NULL DEFAULT 'pending'
                 CHECK (status IN ('pending', 'ready', 'failed')),
    path         TEXT,
    width        INTEGER,
    height       INTEGER,
    error        TEXT,
    extracted_at INTEGER
);

CREATE TABLE image_tags (
    image_id   INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
    tag        TEXT NOT NULL,
    source     TEXT NOT NULL CHECK (source IN ('auto', 'user')),
    confidence REAL NOT NULL DEFAULT 1.0,
    suppressed INTEGER NOT NULL DEFAULT 0,   -- user dismissed an auto tag; kept for audit
    PRIMARY KEY (image_id, tag)
);

CREATE INDEX idx_image_tags_tag ON image_tags(tag, image_id);

CREATE TABLE quality_scores (
    image_id               INTEGER PRIMARY KEY REFERENCES images(id) ON DELETE CASCADE,
    overall                REAL NOT NULL,
    face_sharpness         REAL,
    global_sharpness       REAL NOT NULL,
    eyes_open              REAL,
    composition            REAL,
    face_count             INTEGER NOT NULL DEFAULT 0,
    clipped_highlights_pct REAL NOT NULL,
    clipped_shadows_pct    REAL NOT NULL,
    mean_luma              REAL NOT NULL,
    model_version          TEXT NOT NULL,
    analyzed_at            INTEGER NOT NULL
);

-- Slider values as JSON (ParametricAdjustments) so new sliders need no migration.
CREATE TABLE adjustments (
    image_id        INTEGER PRIMARY KEY REFERENCES images(id) ON DELETE CASCADE,
    params_json     TEXT NOT NULL,
    process_version INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    xmp_synced_at   INTEGER
);
