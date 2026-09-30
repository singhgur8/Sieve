-- Sieve catalog schema v7 (Phase 7: scenes + scene matching).
--
-- A scene is a lighting scenario (consecutive frames under the same light) graded from 1-2
-- anchors. Membership is a column on images (an image is in at most one scene); anchors are a
-- flag on member rows. All writes go through `scene::store` (which keeps the derived
-- started/ended/folder columns and the anchor invariants: anchors are members, <= 2 per scene).

CREATE TABLE scenes (
    id            INTEGER PRIMARY KEY,
    -- Folder of all members; NULL when members span folders (manual scenes only).
    folder_id     INTEGER REFERENCES folders(id) ON DELETE CASCADE,
    -- Min / max captured_at_ms of the members (NULL if none has a capture time).
    started_at_ms INTEGER,
    ended_at_ms   INTEGER,
    method        TEXT NOT NULL CHECK (method IN ('auto', 'manual')),
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);
CREATE INDEX idx_scenes_folder ON scenes(folder_id, started_at_ms);

ALTER TABLE images ADD COLUMN scene_id INTEGER REFERENCES scenes(id) ON DELETE SET NULL;
-- 1 = graded anchor of its scene (meaningless while scene_id IS NULL; store clears it).
ALTER TABLE images ADD COLUMN scene_anchor INTEGER NOT NULL DEFAULT 0;
CREATE INDEX idx_images_scene ON images(scene_id);

-- Appearance features for scene detection, computed from the 2048 px preview by
-- `scene::features` (JSON of `scene::SceneFeatures`). Stale when `version` differs from
-- `scene::FEATURES_VERSION` or the preview was re-extracted after `computed_at`.
CREATE TABLE scene_features (
    image_id      INTEGER PRIMARY KEY REFERENCES images(id) ON DELETE CASCADE,
    version       TEXT NOT NULL,
    features_json TEXT NOT NULL,
    computed_at   INTEGER NOT NULL
);
