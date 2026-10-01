-- Sieve catalog schema v15 (IPC v17: strictly linear undo across scene applies).
--
-- A scene apply copies the representative's settings to the members. When those settings
-- were written by an earlier edit batch (e.g. "Auto Edit (My Style)" of the representative),
-- the apply is built on that batch: `undo_edit_batch` of the earlier batch refuses with
-- `conflict` while the apply is not undone. One row per representative an apply was made
-- from whose current settings came from a batch. Applies before v17 have no rows (they do
-- not block).
CREATE TABLE edit_batch_bases (
    batch_id      INTEGER NOT NULL REFERENCES edit_batches(id) ON DELETE CASCADE,
    image_id      INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
    base_batch_id INTEGER NOT NULL REFERENCES edit_batches(id) ON DELETE CASCADE,
    PRIMARY KEY (batch_id, image_id)
);
CREATE INDEX idx_edit_batch_bases_base ON edit_batch_bases(base_batch_id);
