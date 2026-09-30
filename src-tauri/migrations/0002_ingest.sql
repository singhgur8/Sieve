-- LumenRAW catalog schema v2 (Phase 2: ingest).
-- EXIF columns already exist on `images` (captured_at_ms is ms-precision incl. sub-seconds).

-- 2048 px loupe preview next to the 512 px grid thumbnail (`thumbnails.path`).
ALTER TABLE thumbnails ADD COLUMN preview_path TEXT;

-- The pipeline streams `WHERE status = 'pending'` in batches and the UI counts by status.
CREATE INDEX idx_thumbnails_status ON thumbnails(status);
