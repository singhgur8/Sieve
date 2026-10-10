-- IPC v20.1 (Phase 9 UX review): who made a selection choice, and frame similarities within
-- moments so "covered by" stays correct after the user's edits.

-- Who made the photo's current choice: the engine (a run) or the user (a target edit / own
-- flag that changed the choice). Locking a choice without changing it keeps the origin.
ALTER TABLE target_selection ADD COLUMN origin TEXT NOT NULL DEFAULT 'engine'
    CHECK (origin IN ('engine', 'user'));
UPDATE target_selection SET origin = 'user'
 WHERE locked = 1 AND json_extract(reasons_json, '$[0].kind') = 'user_choice';

-- Visual similarity (0..=1, `ml::moments::similarity`) of two frames of the same moment,
-- written by every run (`image_a < image_b`). Used to recompute `covered_by` after edits.
CREATE TABLE target_similarity (
    image_a    INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
    image_b    INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
    similarity REAL NOT NULL,
    PRIMARY KEY (image_a, image_b),
    CHECK (image_a < image_b)
) WITHOUT ROWID;
CREATE INDEX idx_target_similarity_b ON target_similarity(image_b);
