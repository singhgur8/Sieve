-- Sieve catalog schema v13 (IPC v15: persisted per-photo workflow state, skipped scenes,
-- apply coverage).
--
-- Per-photo state is derived from the history entry the image's cursor points at
-- (`adjustments.history_entry_id`), so per-image undo / redo / goto and any later edit update
-- it without bookkeeping:
--   edit source  = `adjustment_history.source` of the cursor entry (neutral settings = none);
--   applied from = `adjustment_history.batch_id` of the cursor entry (+ its `edit_batch_items`
--                  row: scene, review state);
--   needs a look = that item has `review_reason` and no `reviewed_at`.

-- 1. Who produced each snapshot: 'user' (slider / tool commits, presets, Auto Tone),
--    'pasted' (Paste / Sync / Paste from Previous), 'auto_style' (style model),
--    'scene_apply' (Apply to Scene, Match Scene), 'sidecar' (Original entries and Read from
--    XMP: settings that came from outside Sieve's history). NULL = derive from the label.
ALTER TABLE adjustment_history ADD COLUMN source TEXT
    CHECK (source IN ('user', 'pasted', 'auto_style', 'scene_apply', 'sidecar'));
-- Edit batch that wrote this entry (no FK: batches are never deleted while entries exist,
-- and entries are pruned independently).
ALTER TABLE adjustment_history ADD COLUMN batch_id INTEGER;

UPDATE adjustment_history SET source = CASE
    WHEN label = 'Original' THEN 'sidecar'
    WHEN label = 'Read from XMP' THEN 'sidecar'
    WHEN label IN ('Apply to Scene', 'Match Scene') THEN 'scene_apply'
    WHEN label = 'Auto Edit (My Style)' THEN 'auto_style'
    WHEN label IN ('Paste Settings', 'Sync Settings', 'Paste from Previous') THEN 'pasted'
    ELSE 'user' END;

-- Backfill: the newest entry of each recorded batch item whose snapshot equals what the
-- batch wrote.
UPDATE adjustment_history SET batch_id = (
    SELECT bi.batch_id FROM edit_batch_items bi JOIN edit_batches b ON b.id = bi.batch_id
     WHERE bi.image_id = adjustment_history.image_id
       AND b.label = adjustment_history.label
       AND bi.after_json = adjustment_history.params_json
     ORDER BY bi.batch_id DESC LIMIT 1)
 WHERE label IN ('Apply to Scene', 'Auto Edit (My Style)');

-- 2. Batch items: the image's state before the batch (restored by undo_edit_batch) and the
--    "needs a look" flag of scene applies (match did not converge).
ALTER TABLE edit_batch_items ADD COLUMN before_source TEXT;
ALTER TABLE edit_batch_items ADD COLUMN before_batch_id INTEGER;
-- User-facing reason; NULL = converged (nothing to review).
ALTER TABLE edit_batch_items ADD COLUMN review_reason TEXT;
-- Unix ms of `mark_reviewed`; NULL = still needs a look.
ALTER TABLE edit_batch_items ADD COLUMN reviewed_at INTEGER;
CREATE INDEX idx_adjustment_history_batch ON adjustment_history(batch_id) WHERE batch_id IS NOT NULL;

-- 3. Scenes: skipped in the Edit step, and the frames the last apply considered (targets,
--    frames left alone, excluded frames and the representative; JSON array of ids). Keepers
--    outside it are "added after the apply". NULL = applied before v13 (or never).
ALTER TABLE scenes ADD COLUMN skipped INTEGER NOT NULL DEFAULT 0;
ALTER TABLE scenes ADD COLUMN applied_covered_json TEXT;
