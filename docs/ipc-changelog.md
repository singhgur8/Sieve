# IPC Contract Changelog

Every change to `src-tauri/src/ipc/` or the catalog schema gets an entry: what changed, and which agent must update what.

## v1 — 2026-09-29 (Phase 1)
- Initial contract: types `RawImageEntry`, `CullTag`, `CullTagEntry`, `QualityScore`, `ShootType`,
  `ParametricAdjustments`, `ImageQuery`/`ImagePage`, `CatalogState`, `AppError`; 12 commands; 3 events.
- Catalog schema v1 (`migrations/0001_init.sql`).
- Deviation from the Phase 1 plan: `ImportSummary.unsupported` became `invalid`
  (RAW extension but bad header); non-RAW files are ignored rather than counted. Added `folderId`.
- `CullTag::Overexposed` added to cover exposure clipping.

## v2 — 2026-09-29 (Phase 2: ingest)
Types
- `ThumbnailState::Ready` gains `previewPath: string | null` (2048 px loupe preview); `path` is the
  512 px grid thumbnail; `width`/`height` are the thumbnail's pixel size after orientation.
- `CatalogState.cacheDir: string` — cache root; thumbnails live in `<cacheDir>/thumbs/`.
- New `ImportStatus { total, pending, ready, failed, running }`.
- `CaptureMeta.capturedAtMs` semantics pinned: EXIF wall-clock time incl. `SubSecTimeOriginal`,
  interpreted as UTC ("naive" ms). Display with `timeZone: "UTC"`.
- No other EXIF fields were missing (ISO, shutter, aperture, focal, lens, model, dims, orientation,
  `sensorLayout` already existed).

Commands
- `import_folder` now takes `AppHandle` + `State<Ingest>` (invisible to TS; TS signature unchanged),
  registers files, calls `Ingest::start` and returns without decoding.
- New `regenerate_thumbnails(ids: number[]) -> null` / `regenerateThumbnails`.
- New `get_import_status() -> ImportStatus` / `getImportStatus`.
- `get_catalog_state` fills `cacheDir`.

Events
- `importProgress` gains `failed` (subset of `done`); counts are per pipeline run; throttled.
- `thumbnailReady` gains `previewPath: string | null`.
- New `thumbnailFailed { imageId, reason }`.

Schema (migration `0002_ingest.sql`, user_version 2)
- `thumbnails.preview_path TEXT`; index `idx_thumbnails_status`.

Config
- `LUMENRAW_CACHE=/path` overrides the cache root (default `app_cache_dir()`).
- Asset protocol enabled (`tauri` feature `protocol-asset`), scope `$APPCACHE/thumbs/**`, plus the
  resolved `<cache>/thumbs` dir added at runtime. CSP set (see `docs/decisions.md`).

Who updates what
- rust-engine-dev: implement `src-tauri/src/ingest/` (`Ingest::start`, `Ingest::regenerate`, worker,
  events) and the `raw/` decode/EXIF functions it calls; repo functions for writing EXIF +
  thumbnail rows. `repo::catalog_state` gained a `cache_dir` param and `ENTRY_SELECT` reads
  `t.preview_path` as column 39 (already done by architect to keep the build green).
- frontend-dev: show thumbnails via `convertFileSrc(thumbnail.path)`; listen to `importProgress`,
  `thumbnailReady`, `thumbnailFailed`; call `getImportStatus()` on mount to restore progress;
  offer retry via `regenerateThumbnails(ids)` for failed images.

## v3 — 2026-09-29 (Phase 3: culling engine)
Types
- `QualityScore` gains `suggestedRating: number` (0–5) and `suggestedPick: PickFlag`. Suggestions never
  touch the user's `rating`/`pick` except via `applySuggestions`.
- `RawImageEntry.isBurstKeeper: boolean`.
- `CatalogState.autoAnalyze: boolean`.
- New `FaceInfo { bbox: NormRect, leftEye, rightEye: NormPoint, detectionScore, ear, eyesOpen, sharpness,
  blink, inFocus, primary, considered }`; `NormRect {x,y,width,height}` / `NormPoint {x,y}` are 0..1 of the
  (orientation-corrected) preview, origin top-left.
- New `CullThresholds` (+ `ScoreWeights`): per-shoot-type blink EAR, min face size, face/global sharpness
  minimums, under/overexposure limits, burst hash distance, pick/reject cut-offs, score weights.
  `CullThresholds::validate()`.
- New `AnalysisScope` (tagged by `kind`): `pending | images{ids} | folder{folderId} | all | rescore`.
- New `AnalysisStatus { total, analyzed, failed, pending, waiting, running }`.
- New `BurstGroup { id, startedAtMs, endedAtMs, keeperImageId, imageIds }`.

Commands (new)
- `analyze_images(scope) -> null`, `cancel_analysis() -> null`, `get_analysis_status() -> AnalysisStatus`.
- `set_auto_analyze(enabled) -> null`.
- `get_cull_thresholds(shootType) -> CullThresholds`, `set_cull_thresholds(shootType, thresholds | null) -> null`
  (`null` resets to defaults; rescore kicked if `shootType` is current).
- `get_faces(id) -> FaceInfo[]`, `list_burst_groups(folderId | null) -> BurstGroup[]`,
  `apply_suggestions(ids) -> number` (count updated; unanalyzed skipped; atomic, `not_found` on unknown id).

Commands (changed; TS signatures unchanged)
- `import_folder` and `regenerate_thumbnails` also kick analysis (`pending`) when `autoAnalyze` is on.
- `set_shoot_type` and `set_burst_window` also kick a `rescore`.

Events
- `analysisProgress` gains `failed`.
- New `analysisReady {imageId}`, `analysisFailed {imageId, reason}`,
  `analysisFinished {analyzed, failed, cancelled, burstGroups}`.

Schema (migration `0003_analysis.sql`, user_version 3)
- New table `image_analysis` (status queued/done/failed, model_version, analyzed_at, error, phash, faces_json,
  metrics_json) + `idx_image_analysis_status`.
- `quality_scores.suggested_rating`, `quality_scores.suggested_pick`.
- `catalog_meta`: `auto_analyze` (default `'1'`); overrides under `cull_thresholds.<shoot_type>` (JSON).

Config
- `LUMENRAW_MODELS=/path` overrides the model dir (default `src-tauri/models` in debug, `<resource_dir>/models`
  in release — bundling is Phase 8).

Who updates what
- architect (done): types, events, commands, registration, migration, `Analysis` managed in `lib.rs`
  (auto-start on launch), repo plumbing for the new read/user-write commands (`auto_analyze`,
  `set_auto_analyze`, `shoot_type`, `cull_thresholds`, `set_cull_thresholds`, `get_faces`,
  `list_burst_groups`, `apply_suggestions`) + `ENTRY_SELECT` columns 40–42, with tests.
- vision-ml-dev: fill the stubs in `src-tauri/src/ml/mod.rs` — `Analysis::start` (queue scope + worker),
  `analysis_status`, `Analyzer::{load, measure}`, `score`, `group_bursts`, `MODEL_VERSION`; tune
  `ml/thresholds.rs`. Put the worker's SQL (write `image_analysis`, `quality_scores`, auto tags, burst groups,
  needs-analysis query) in `ml/` (e.g. `ml/store.rs`), not in `db/repo.rs`, to avoid conflicts.
  Add `ort` etc. to `Cargo.toml`.
- frontend-dev: listen to `analysis*` events, show progress/cancel, `isBurstKeeper`, suggestions with an
  "apply suggestions" action; thresholds editor and auto-analyze toggle are optional in Phase 3 (Phase 4 UI).
  `DEFAULT_QUERY` unchanged.
