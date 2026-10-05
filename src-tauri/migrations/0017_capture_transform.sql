-- Sieve catalog schema v17 (Phase 8d, IPC v19): capture time correction, applied presets,
-- reject strictness, paste batches.

-- 1. Capture time. `images.captured_at_ms` stays the time every query uses (sort, bursts,
--    scenes, filters, export naming, project ranges) and becomes the *corrected* time;
--    `exif_captured_at_ms` keeps the file's own EXIF time. `capture_time_source` says where
--    `captured_at_ms` comes from: `exif` (= exif_captured_at_ms), `sidecar` (a corrected time
--    read from the XMP, e.g. Lightroom's Edit Capture Time) or `user` (`edit_capture_time`).
--    Thumbnail extraction writes `exif_captured_at_ms` and refreshes `captured_at_ms` only
--    while the source is `exif` (`repo::record_extraction`). Corrections mark the sidecar
--    dirty in code (`db::capture_time`), not by trigger.
ALTER TABLE images ADD COLUMN exif_captured_at_ms INTEGER;
ALTER TABLE images ADD COLUMN capture_time_source TEXT NOT NULL DEFAULT 'exif'
    CHECK (capture_time_source IN ('exif', 'sidecar', 'user'));
UPDATE images SET exif_captured_at_ms = captured_at_ms;

-- 2. Applied preset (`AdjustmentHistory.appliedPresetId`): the preset last applied to the
--    image and its settings right after the apply. Reported while the preset's fields still
--    equal that snapshot (`develop::history::applied_preset`), so any change of a preset-owned
--    setting clears it without hooking every writer.
ALTER TABLE adjustments ADD COLUMN applied_preset_id INTEGER REFERENCES presets(id) ON DELETE SET NULL;
ALTER TABLE adjustments ADD COLUMN applied_preset_json TEXT;

-- 3. Reject strictness per project (`Project.rejectStrictness`).
ALTER TABLE projects ADD COLUMN reject_strictness TEXT NOT NULL DEFAULT 'balanced'
    CHECK (reject_strictness IN ('conservative', 'balanced', 'aggressive'));

-- 4. Paste / Sync / Paste from Previous record undoable edit batches (`EditBatchKind::Paste`).
--    Relax the kind CHECK with the documented writable_schema edit (see 0009 for why a table
--    rebuild is not used). Runs after the ADD COLUMNs above. Idempotent.
PRAGMA writable_schema = ON;
UPDATE sqlite_schema
   SET sql = replace(sql,
                     'CHECK (kind IN (''scene_apply'', ''style_prediction''))',
                     'CHECK (kind IN (''scene_apply'', ''style_prediction'', ''paste''))')
 WHERE type = 'table' AND name = 'edit_batches';
PRAGMA writable_schema = RESET;
