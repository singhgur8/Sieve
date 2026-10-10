-- Sieve catalog schema v18 (Phase 8d feedback, IPC v19.2): camera body serials, Auto Sync
-- batches.

-- 1. Body serial number (`CameraInfo.serial`, `CameraBody`): written by thumbnail extraction
--    (`repo::record_extraction`) from EXIF `BodySerialNumber` (else DNG `CameraSerialNumber`).
--    `camera_serial_read` = the file was read for it (0 for photos imported before v18; the
--    background pass `db::camera_serial::backfill` reads them after startup without blocking
--    it). A read file without a serial keeps `camera_serial` NULL with `camera_serial_read = 1`.
ALTER TABLE images ADD COLUMN camera_serial TEXT;
ALTER TABLE images ADD COLUMN camera_serial_read INTEGER NOT NULL DEFAULT 0;
CREATE INDEX idx_images_folder_body ON images(folder_id, camera_make, camera_model, camera_serial);
CREATE INDEX idx_images_serial_unread ON images(id) WHERE camera_serial_read = 0;

-- 2. `sync_delta` records undoable batches of kind `sync` (`EditBatchKind::Sync`). Relax the
--    kind CHECK with the documented writable_schema edit (see 0009 / 0017). Idempotent.
PRAGMA writable_schema = ON;
UPDATE sqlite_schema
   SET sql = replace(sql,
                     'CHECK (kind IN (''scene_apply'', ''style_prediction'', ''paste''))',
                     'CHECK (kind IN (''scene_apply'', ''style_prediction'', ''paste'', ''sync''))')
 WHERE type = 'table' AND name = 'edit_batches';
PRAGMA writable_schema = RESET;
