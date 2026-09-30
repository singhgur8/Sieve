-- Sieve catalog schema v10 (Phase 7c: local adjustments / masks, IPC v10).
--
-- Mask groups themselves live in `adjustments.params_json` (new key `masks`, see
-- `ParametricAdjustments.masks`): history, presets, undo and the XMP dirty triggers of 0005
-- cover them with no schema change. Rows written before v10 lack the key and read as no masks.

-- 1. AI mattes, one row per (image, bitmap). Pixels are files, not blobs:
--    `<cacheDir>/<path>` = 8-bit grayscale PNG of the matte, placed at `bounds_*` in the sensor
--    frame (un-oriented, uncropped, normalized 0..1; Lightroom crops mattes to their bounding
--    box). Sources:
--    - origin 'lightroom': decoded from a sidecar's `crs:Table_<MaskDigest>` at XMP read;
--      `digest` = Lightroom's MaskDigest, `model_version` = 'lr:<crs:ModelVersion>',
--      `input_digest` = crs:InputDigest. Re-creatable from the sidecar while the mask exists.
--    - origin 'sieve': computed by `ml::masking::Segmenter`; `digest` = MD5 (upper hex) of the
--      PNG bytes, `model_version` = `SegmentModel::id()`, `input_digest` = the source file's
--      fingerprint at compute time (size + mtime, `ml::masking` docs) so a changed original
--      invalidates it.
--    `kind` = `AiMask::cache_kind()` (e.g. 'subject', 'people:face_skin@0.4123,0.2211'): the
--    renderer resolves an AI component without a digest to the newest 'sieve' row with the
--    same kind and the current model version. Cascade-deleted with the image; blob files are
--    removed by the owner of the cache (`develop::masks::MaskCache`), orphans by a sweep.
CREATE TABLE mask_cache (
    image_id      INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
    digest        TEXT    NOT NULL,
    kind          TEXT    NOT NULL,
    origin        TEXT    NOT NULL CHECK (origin IN ('lightroom', 'sieve')),
    model_version TEXT    NOT NULL,
    input_digest  TEXT,
    path          TEXT    NOT NULL,
    width         INTEGER NOT NULL CHECK (width > 0),
    height        INTEGER NOT NULL CHECK (height > 0),
    bounds_x      REAL    NOT NULL,
    bounds_y      REAL    NOT NULL,
    bounds_w      REAL    NOT NULL CHECK (bounds_w > 0),
    bounds_h      REAL    NOT NULL CHECK (bounds_h > 0),
    coverage      REAL    NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL,
    PRIMARY KEY (image_id, digest)
);
CREATE INDEX idx_mask_cache_kind ON mask_cache (image_id, kind, model_version);

-- 2. Sidecar masks read before v10 were only reported (`masks_unsupported` in
--    `images.develop_warnings`), never imported, so those images' `adjustments.masks` is empty
--    while their sidecar has mask groups. Until the masks are imported (next XMP read / the
--    one-off catch-up, `xmp` docs), the writer must leave `crs:MaskGroupBasedCorrections` alone:
--    writing the empty list would delete the user's Lightroom masks. 1 = import pending.
--    Not XMP-mapped; does not mark the image dirty (0004 triggers watch other columns).
ALTER TABLE images ADD COLUMN masks_pending_import INTEGER NOT NULL DEFAULT 0;
UPDATE images SET masks_pending_import = 1
 WHERE develop_warnings IS NOT NULL
   AND develop_warnings LIKE '%"masks_unsupported"%';
