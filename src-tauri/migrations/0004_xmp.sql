-- LumenRAW catalog schema v4 (Phase 4: XMP sidecar sync).
--
-- XMP-mapped values are images.rating / pick / color_label and non-suppressed image_tags.
-- Triggers below flag an image `xmp_dirty` whenever one of those actually changes, from
-- any writer (commands, apply_suggestions, the analysis worker's auto tags), so no code
-- path has to remember to do it. The XMP module clears the flag after a successful write
-- or read (its own UPDATE of xmp_* columns does not fire these triggers).

-- Catalog has XMP-mapped changes not yet written to the sidecar.
ALTER TABLE images ADD COLUMN xmp_dirty INTEGER NOT NULL DEFAULT 0;
-- Unix ms of the last change to an XMP-mapped value (set by the triggers).
ALTER TABLE images ADD COLUMN meta_updated_at INTEGER;
-- Unix ms when catalog and sidecar last agreed (successful write or read). NULL = never.
ALTER TABLE images ADD COLUMN xmp_synced_at INTEGER;
-- Sidecar file mtime (unix ms) observed right after that write/read; NULL = no sidecar then.
-- A different mtime on disk means the sidecar was edited externally since.
ALTER TABLE images ADD COLUMN xmp_mtime_ms INTEGER;
-- Reason of the last failed write/read; NULL after a success.
ALTER TABLE images ADD COLUMN xmp_error TEXT;

CREATE INDEX idx_images_xmp_dirty ON images(xmp_dirty) WHERE xmp_dirty = 1;

CREATE TRIGGER images_xmp_dirty
AFTER UPDATE OF rating, pick, color_label ON images
WHEN OLD.rating IS NOT NEW.rating OR OLD.pick IS NOT NEW.pick OR OLD.color_label IS NOT NEW.color_label
BEGIN
    UPDATE images
    SET xmp_dirty = 1, meta_updated_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
    WHERE id = NEW.id;
END;

CREATE TRIGGER image_tags_xmp_insert
AFTER INSERT ON image_tags
WHEN NEW.suppressed = 0
BEGIN
    UPDATE images
    SET xmp_dirty = 1, meta_updated_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
    WHERE id = NEW.image_id;
END;

CREATE TRIGGER image_tags_xmp_delete
AFTER DELETE ON image_tags
WHEN OLD.suppressed = 0
BEGIN
    UPDATE images
    SET xmp_dirty = 1, meta_updated_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
    WHERE id = OLD.image_id;
END;

-- Confidence/source changes do not affect the sidecar; only visibility (suppressed) does.
CREATE TRIGGER image_tags_xmp_update
AFTER UPDATE OF suppressed, tag ON image_tags
WHEN OLD.suppressed IS NOT NEW.suppressed OR OLD.tag IS NOT NEW.tag
BEGIN
    UPDATE images
    SET xmp_dirty = 1, meta_updated_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
    WHERE id = NEW.image_id;
END;

-- Write sidecars automatically (debounced) after XMP-mapped changes. Off by default:
-- writing into photo folders must be an explicit choice.
INSERT INTO catalog_meta (key, value) VALUES ('xmp_auto_sync', '0');
