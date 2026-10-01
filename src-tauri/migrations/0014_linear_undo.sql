-- Sieve catalog schema v14 (IPC v16: linear batch undo).
--
-- `undo_edit_batch` now clears the applied state of the scenes whose last apply was the
-- undone batch (status back to `edited`). Catalogs that undid an apply before v16 still
-- report those scenes as applied: clear them the same way.
UPDATE scenes
   SET applied_at_ms = NULL, applied_params_json = NULL, applied_batch_id = NULL, applied_covered_json = NULL
 WHERE applied_batch_id IN (SELECT id FROM edit_batches WHERE undone_at IS NOT NULL);
