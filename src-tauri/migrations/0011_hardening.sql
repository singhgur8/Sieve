-- Sieve catalog schema v11 (Phase 8 hardening, IPC v13).

-- 1. Filter-bar indexes (measured with `examples/grid_bench.rs --plans`, 50k images):
--    - `get_filter_counts` of the whole catalog groups by (folder_id, pick, rating) and
--      follows this covering index instead of scanning the table + a temp B-tree;
--    - live (non-suppressed) tag counts / tag filters read a partial covering index.
CREATE INDEX idx_images_folder_pick_rating ON images(folder_id, pick, rating);
CREATE INDEX idx_image_tags_live ON image_tags(tag, image_id) WHERE suppressed = 0;

-- 2. Missing originals: unix ms when an access (render, develop info, export, sidecar
--    write, thumbnail extraction, re-import) first found the original gone; NULL = present
--    or never checked. Cleared by a successful access, a re-import that finds the file, or
--    `relocate_folder`. Not XMP-mapped (the 0004 dirty triggers watch other columns).
ALTER TABLE images ADD COLUMN missing_since_ms INTEGER;
-- `missing` facet count / `ImageQuery.missingOnly` without a table scan.
CREATE INDEX idx_images_missing ON images(folder_id) WHERE missing_since_ms IS NOT NULL;
