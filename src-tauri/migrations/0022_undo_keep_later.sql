-- Sieve catalog schema v22 (Phase 10, IPC v21.1): "Undo the rest" (`undo_edit_batch` with
-- `keepLaterEdits`). Photos of the batch that were edited after it are left with the later
-- edit; their item records when, so their baseline provenance reads `user_edited` (kept), not
-- `undone`.
ALTER TABLE edit_batch_items ADD COLUMN kept_at INTEGER;
