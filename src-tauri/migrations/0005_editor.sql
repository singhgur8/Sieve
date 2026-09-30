-- Sieve catalog schema v5 (Phase 5: editor).
--
-- Adds per-image edit history (undo/redo), develop presets, and XMP dirty tracking for
-- develop settings (crs:). LUT files live in a directory library, not in the catalog.

-- LutRef changed from {path, amount} to {id, amount} (IPC v5). Drop old-shape references
-- (never written by a released build) so stored JSON keeps deserializing. Runs before the
-- dirty triggers below exist.
UPDATE adjustments
SET params_json = json_remove(params_json, '$.lut')
WHERE json_type(params_json, '$.lut') = 'object' AND json_extract(params_json, '$.lut.id') IS NULL;

-- 1 = the stored adjustments equal the neutral defaults (after a reset or undo to
-- "Original"). `RawImageEntry.hasEdits` = a row exists with neutral = 0. The row itself is
-- kept (history cursor, and a neutral crs: set must still reach the sidecar).
ALTER TABLE adjustments ADD COLUMN neutral INTEGER NOT NULL DEFAULT 0;
-- adjustment_history.id whose snapshot equals params_json; NULL = no history yet.
ALTER TABLE adjustments ADD COLUMN history_entry_id INTEGER;
-- (adjustments.xmp_synced_at from v1 is unused: sync state lives on images.xmp_*.)

-- Snapshots, oldest first by id. The first entry per image is 'Original'. Entries with
-- id > adjustments.history_entry_id are the redo tail (dropped by the next edit).
CREATE TABLE adjustment_history (
    id          INTEGER PRIMARY KEY,
    image_id    INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
    label       TEXT NOT NULL,
    params_json TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    -- Last time a coalesced save replaced this snapshot (= created_at otherwise).
    updated_at  INTEGER NOT NULL
);
CREATE INDEX idx_adjustment_history_image ON adjustment_history(image_id, id);

CREATE TABLE presets (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE COLLATE NOCASE,
    -- ParametricAdjustments JSON (overlaid on neutral defaults when read, like adjustments).
    params_json TEXT NOT NULL,
    -- AdjustmentField[] JSON, non-empty.
    fields_json TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

-- Develop settings are XMP-mapped (crs:) from v5: any real change flags the sidecar dirty,
-- exactly like rating/pick/label/tags in 0004.
CREATE TRIGGER adjustments_xmp_insert
AFTER INSERT ON adjustments
BEGIN
    UPDATE images
    SET xmp_dirty = 1, meta_updated_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
    WHERE id = NEW.image_id;
END;

CREATE TRIGGER adjustments_xmp_update
AFTER UPDATE OF params_json ON adjustments
WHEN OLD.params_json IS NOT NEW.params_json
BEGIN
    UPDATE images
    SET xmp_dirty = 1, meta_updated_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
    WHERE id = NEW.image_id;
END;
