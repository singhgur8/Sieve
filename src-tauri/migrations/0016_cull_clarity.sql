-- Sieve catalog schema v16 (Phase 8c, IPC v18): culling clarity.

-- 1. Who set the pick / reject flag (`PickOrigin`): `auto` = `apply_suggestions`, `user` =
--    everything else (flag commands, undo of user flags, sidecar reads). Written explicitly by
--    every writer of `pick` (no trigger: an UPDATE cannot tell whether it set the column).
--    Flags set before v16 cannot be attributed and count as `user`. Meaningless while
--    `pick = 'unflagged'` (reported as null).
ALTER TABLE images ADD COLUMN pick_origin TEXT NOT NULL DEFAULT 'user'
    CHECK (pick_origin IN ('user', 'auto'));

-- 2. Human-readable reasons of a suggestion (`QualityScore.reasons`, JSON
--    `SuggestionReason[]`), written with the score by the culling engine.
ALTER TABLE quality_scores ADD COLUMN reasons_json TEXT NOT NULL DEFAULT '[]';

-- 3. Keeper rule gains `mode` (IPC v18). The new default is "everything not rejected" (user
--    decision 2026-10-03). Migration 0012 stored the old default
--    {"minRating":1,"useSuggestions":true} in every catalog; catalogs still holding it get the
--    new default, a rule the user customised keeps its behaviour as `picks_and_ratings`.
UPDATE catalog_meta
   SET value = json_set(value, '$.mode',
           CASE WHEN json_extract(value, '$.minRating') = 1 AND json_extract(value, '$.useSuggestions') = 1
                THEN 'not_rejected' ELSE 'picks_and_ratings' END)
 WHERE key = 'keeper_rule' AND json_valid(value) AND json_extract(value, '$.mode') IS NULL;

-- 4. Facets of the Library Filter metadata row (`get_metadata_filter_options`) group by
--    camera and lens within a project's folders.
CREATE INDEX idx_images_folder_camera ON images(folder_id, camera_make, camera_model);
CREATE INDEX idx_images_folder_lens ON images(folder_id, lens);
