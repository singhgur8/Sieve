-- Sieve catalog schema v12 (Phase 8b, IPC v14): style library (presets + profiles), guided
-- workflow (per-folder step, per-scene edit plan), undoable edit batches, style model,
-- keeper rule, XMP auto-sync on by default.

-- 1. Style library. Groups = source folders of `import_style_folder` (+ two built-ins with
--    fixed ids, `ipc::types::USER_PRESETS_GROUP_ID` / `LUT_LIBRARY_GROUP_ID`).
CREATE TABLE style_groups (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    kind        TEXT NOT NULL CHECK (kind IN ('user', 'imported', 'luts')),
    -- Absolute source folder (imported groups; re-import of the same folder replaces the
    -- group's items). NULL for the built-ins.
    source_path TEXT UNIQUE,
    imported_at INTEGER
);
INSERT INTO style_groups (id, name, kind) VALUES (1, 'User Presets', 'user'), (2, 'LUTs', 'luts');

-- Presets: rebuilt so names are unique per group (imported folders reuse names) instead of
-- catalog-wide. Nothing references `presets`, so drop + rename inside the migration
-- transaction is safe with foreign keys on. Existing rows become User Presets, same ids.
CREATE TABLE presets_v12 (
    id              INTEGER PRIMARY KEY,
    group_id        INTEGER NOT NULL DEFAULT 1 REFERENCES style_groups(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    -- ParametricAdjustments JSON (overlaid on the RAW defaults when read).
    params_json     TEXT NOT NULL,
    -- AdjustmentField[] JSON, non-empty.
    fields_json     TEXT NOT NULL,
    source_format   TEXT NOT NULL DEFAULT 'sieve'
                    CHECK (source_format IN ('sieve', 'xmp_preset', 'lrtemplate')),
    source_path     TEXT,
    -- Imported presets: the `crs:` settings exactly as found in the file (format owned by
    -- `xmp::crs`; applied key by key). NULL for Sieve presets (applied via fields_json).
    settings_json   TEXT,
    -- JSON string[] of the crs property names in settings_json (StylePreset.settingKeys).
    setting_keys_json TEXT NOT NULL DEFAULT '[]',
    supports_amount INTEGER NOT NULL DEFAULT 0,
    -- JSON string[] (StylePreset.warnings).
    warnings_json   TEXT NOT NULL DEFAULT '[]',
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    UNIQUE (group_id, name COLLATE NOCASE)
);
INSERT INTO presets_v12 (id, group_id, name, params_json, fields_json, created_at, updated_at)
    SELECT id, 1, name, params_json, fields_json, created_at, updated_at FROM presets;
DROP TABLE presets;
ALTER TABLE presets_v12 RENAME TO presets;
CREATE INDEX idx_presets_group ON presets(group_id);

-- Profiles of the library. Looks (.xmp) and DCPs are read in place at source_path (never
-- copied; see `profiles` module docs); .cube files are copied into the LUT library (lut_id)
-- and source_path is that copy.
CREATE TABLE style_profiles (
    id              INTEGER PRIMARY KEY,
    group_id        INTEGER NOT NULL REFERENCES style_groups(id) ON DELETE CASCADE,
    kind            TEXT NOT NULL CHECK (kind IN ('look', 'camera_profile', 'lut')),
    name            TEXT NOT NULL,
    source_format   TEXT NOT NULL CHECK (source_format IN ('sieve', 'xmp_profile', 'dcp', 'cube')),
    source_path     TEXT NOT NULL,
    look_uuid       TEXT,
    lut_id          TEXT,
    camera_profile  TEXT,
    camera_model    TEXT,
    supports_amount INTEGER NOT NULL DEFAULT 0,
    monochrome      INTEGER NOT NULL DEFAULT 0,
    created_at      INTEGER NOT NULL
);
CREATE INDEX idx_style_profiles_group ON style_profiles(group_id);
CREATE INDEX idx_style_profiles_look ON style_profiles(look_uuid) WHERE look_uuid IS NOT NULL;

-- 2. Guided workflow: step bar per folder ("project").
ALTER TABLE folders ADD COLUMN workflow_step TEXT NOT NULL DEFAULT 'cull'
    CHECK (workflow_step IN ('cull', 'edit', 'export'));

-- 3. Undoable multi-image edits (apply scene edit, apply style prediction). One row per
--    changed image with its settings before and after; undo restores `before_json` for images
--    whose current settings still equal `after_json`.
CREATE TABLE edit_batches (
    id         INTEGER PRIMARY KEY,
    label      TEXT NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('scene_apply', 'style_prediction')),
    created_at INTEGER NOT NULL,
    undone_at  INTEGER
);
CREATE TABLE edit_batch_items (
    batch_id    INTEGER NOT NULL REFERENCES edit_batches(id) ON DELETE CASCADE,
    image_id    INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
    -- Scene the image was edited for (scene applies), NULL otherwise.
    scene_id    INTEGER REFERENCES scenes(id) ON DELETE SET NULL,
    before_json TEXT NOT NULL,
    after_json  TEXT NOT NULL,
    PRIMARY KEY (batch_id, image_id)
);
CREATE INDEX idx_edit_batch_items_image ON edit_batch_items(image_id, batch_id);

-- 4. Per-scene edit plan. `representative_*` NULL = not chosen yet (the plan proposes one and
--    stores it as 'auto'). `applied_params_json` = the representative's adjustments at the
--    last apply (status `outdated` when they differ from its current ones).
ALTER TABLE scenes ADD COLUMN representative_id INTEGER REFERENCES images(id) ON DELETE SET NULL;
ALTER TABLE scenes ADD COLUMN representative_source TEXT CHECK (representative_source IN ('auto', 'user'));
ALTER TABLE scenes ADD COLUMN representative_reason TEXT;
ALTER TABLE scenes ADD COLUMN applied_at_ms INTEGER;
ALTER TABLE scenes ADD COLUMN applied_params_json TEXT;
ALTER TABLE scenes ADD COLUMN applied_batch_id INTEGER REFERENCES edit_batches(id) ON DELETE SET NULL;

-- 5. Style model ("auto edit in my style"). The newest row is the active model.
CREATE TABLE style_models (
    id              INTEGER PRIMARY KEY,
    version         TEXT NOT NULL,
    trained_at      INTEGER NOT NULL,
    examples        INTEGER NOT NULL,
    model_blob      BLOB NOT NULL,
    -- StyleValidation JSON, NULL if not validated.
    validation_json TEXT
);
-- Per-image features for training / prediction (JSON owned by `ml::style`; stale when
-- `version` differs or the image's preview/source changed after `computed_at`).
CREATE TABLE style_features (
    image_id      INTEGER PRIMARY KEY REFERENCES images(id) ON DELETE CASCADE,
    version       TEXT NOT NULL,
    features_json TEXT NOT NULL,
    computed_at   INTEGER NOT NULL
);

-- 6. Keeper rule (KeeperRule JSON).
INSERT OR IGNORE INTO catalog_meta (key, value) VALUES ('keeper_rule', '{"minRating":1,"useSuggestions":true}');

-- 7. XMP auto-sync on by default (user request, Phase 8b). Catalogs created before v12 cannot
--    tell "never touched" from "turned off" (both '0'), and no release ever shipped the toggle
--    to other users, so every '0' flips once. From now on `set_xmp_auto_sync` records
--    `xmp_auto_sync_user_set` so a future default change can respect an explicit choice.
UPDATE catalog_meta SET value = '1'
 WHERE key = 'xmp_auto_sync' AND value = '0'
   AND NOT EXISTS (SELECT 1 FROM catalog_meta WHERE key = 'xmp_auto_sync_user_set');
INSERT OR IGNORE INTO catalog_meta (key, value) VALUES ('xmp_auto_sync', '1');
