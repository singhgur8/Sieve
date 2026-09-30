-- Sieve catalog schema v9 (Phase 7b: non-RAW sources + Lightroom develop parity, IPC v9).
--
-- 1. `images.format` accepts non-RAW sources. SQLite cannot ALTER a CHECK constraint, and a
--    table rebuild would need `PRAGMA foreign_keys = OFF` outside the migration transaction
--    (dropping `images` with FKs on cascades into every child table). Relaxing a CHECK does
--    not change the on-disk format, so the documented `writable_schema` edit is used instead
--    (https://sqlite.org/lang_altertable.html#otheralter): rewrite the stored CREATE TABLE text;
--    `writable_schema = RESET` reloads the schema on this connection (the UPDATE also bumps
--    the schema cookie for others). Runs *after* the ADD COLUMNs below: ALTER TABLE splices
--    text at offsets of the parsed schema, which must not be stale. Idempotent: replace() is a
--    no-op if the old text is absent. Verified by `db::tests::v9_*`.

-- 2. Camera JPEG/HEIC paired with a RAW at import (`ImportOptions.pairJpegWithRaw`):
--    absolute path of the sibling, which is not an image row of its own. Not XMP-mapped.
ALTER TABLE images ADD COLUMN companion_path TEXT;

-- 3. Sidecar develop settings found at the last XMP read that Sieve preserves but does not
--    render (`RawImageEntry.developWarnings`): JSON array of `DevelopWarning`, NULL = none.
--    Written by the XMP read path only (`xmp::store::set_develop_warnings`). Not XMP-mapped.
ALTER TABLE images ADD COLUMN develop_warnings TEXT;

-- Develop parity fields live in `adjustments.params_json` (new keys `toneCurve`,
-- `colorGrading`, `calibration`, `detail`, `effects`, `blackAndWhite`, `crop`, `profile`).
-- Rows written before v9 lack them; readers overlay stored JSON on the format's defaults
-- (`ParametricAdjustments::defaults_for`), so no data migration is needed. `adjustments.neutral`
-- of existing rows stays correct: an old row is neutral iff its old fields are neutral, and
-- the new groups read as defaults.

-- (1, continued) Relax the format CHECK.
PRAGMA writable_schema = ON;
UPDATE sqlite_schema
   SET sql = replace(sql,
                     'CHECK (format IN (''arw'', ''raf'', ''cr3''))',
                     'CHECK (format IN (''arw'', ''raf'', ''cr3'', ''jpeg'', ''heic'', ''tiff'', ''png''))')
 WHERE type = 'table' AND name = 'images';
PRAGMA writable_schema = RESET;
