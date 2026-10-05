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

## v4 — 2026-09-29 (Phase 4: culling UI + XMP sidecars)
Types
- `RawImageEntry.xmp: XmpSyncState { dirty, syncedAtMs, error }`.
- New `XmpSyncReport { succeeded, skipped, failed: XmpFailure[], changed: number[] }`,
  `XmpFailure { imageId, reason }`, `XmpStatus { dirty, failed, running, autoSync }`.
- New `FilterCounts { total, tags: TagCount[], picked, rejected, unflagged, ratings: number[6], burstGroups,
  burstNonKeepers }`.
- `CatalogState.xmpAutoSync: boolean`; `ImportSummary.sidecarsRead: number`.
- `ImageQuery` (BREAKING): `pick: PickFlag | null` replaced by `picks: PickFlag[]` (any of; `[]` = no filter).
  New `maxRating: number | null`, `colorLabels: ColorLabel[]`, `collapseBursts: boolean` (hide non-keeper burst
  members), `sortDescending: boolean` (reverses the natural order). `ImageSort` gains `rating` (most stars first).
  Rating bounds > 5 fail with `invalid_argument`. `DEFAULT_QUERY` in `src/ipc/index.ts` updated.

Commands (new)
- `get_images(ids) -> RawImageEntry[]` (given order; atomic `not_found`).
- `list_image_ids(query) -> number[]` (all matches in sort order; offset/limit ignored) — select-all / batch ops.
- `get_filter_counts(folderId | null) -> FilterCounts`.
- `write_xmp(ids) -> XmpSyncReport` (catalog wins), `read_xmp(ids) -> XmpSyncReport` (sidecar wins).
- `set_xmp_auto_sync(enabled) -> null` (enabling flushes dirty images), `get_xmp_status() -> XmpStatus`.

Commands (changed; TS signatures unchanged except via `ImageQuery`)
- `set_rating`, `set_pick`, `set_color_label`, `set_user_tag`, `apply_suggestions` notify the XMP auto-sync
  writer after a successful write.
- `import_folder` reads existing sidecars of new / externally changed images before returning
  (`sidecarsRead`).

Events
- New `xmpSynced { written: number[], read: number[] }` (per auto-sync pass) and
  `xmpWriteFailed { imageId, reason }`.

Schema (migration `0004_xmp.sql`, user_version 4)
- `images`: `xmp_dirty`, `meta_updated_at`, `xmp_synced_at`, `xmp_mtime_ms`, `xmp_error`; partial index
  `idx_images_xmp_dirty`.
- Triggers `images_xmp_dirty`, `image_tags_xmp_{insert,delete,update}` set `xmp_dirty = 1` and `meta_updated_at`
  whenever rating/pick/color_label or the set of visible (non-suppressed) tags actually changes, from any writer.
- `catalog_meta.xmp_auto_sync` (default `'0'`).

Who updates what
- architect (done): types, events, commands, registration, migration, `XmpSync` managed in `lib.rs` (notified on
  launch and on `analysisFinished`), repo plumbing (`get_images`, `list_image_ids`, `filter_counts`,
  `xmp_auto_sync`, `set_xmp_auto_sync`, `xmp_status`, extended `list_images`, `ENTRY_SELECT` cols 43–45) + tests.
- rust-engine-dev: implement `src-tauri/src/xmp/` — `XmpSync::{notify (debounced auto-sync worker, emits
  xmpSynced/xmpWriteFailed), write_images, read_images, refresh_folder}` and the sidecar parser/merger, per the
  mapping and conflict policy in the module docs / `docs/architecture.md`. Keep the xmp module's SQL in `xmp/`.
  Tests only on tempdirs / `test-data/` copies (never `~/Pictures`); verify with `exiftool`.
- frontend-dev: migrate `pick` -> `picks`; use `listImageIds` for select-all/batch keyboard ops and loupe
  navigation, `getImages` to refresh rows after edits/events, `getFilterCounts` for the filter bar; show
  `entry.xmp` (dirty/error) and an auto-sync toggle (`catalogState.xmpAutoSync`, `setXmpAutoSync`), a
  "Save metadata" action (`writeXmp(selected)`, Cmd+S) and "Read metadata from file" (`readXmp`); listen to
  `xmpSynced` / `xmpWriteFailed`.

## Rename — 2026-09-29
- Product renamed LumenRAW → **Sieve** (user request). Crate `lumenraw`/`lumenraw_lib` → `sieve`/`sieve_lib`;
  bundle id `com.lumenraw.app` → `com.sieve.app` (app data/cache dirs move; no user catalogs existed yet);
  env vars `LUMENRAW_*` → `SIEVE_*`; XMP auto-tag keywords `LumenRAW|<tag>` → `Sieve|<tag>`.
  No IPC type/command changes. Historical entries above keep the old name.

## v5 — 2026-09-29 (Phase 5: editor)
Types
- `LutRef` (BREAKING): `{ path, amount }` -> `{ id: LutId, amount }` (library id, `[a-z0-9-]{1,64}`,
  `is_valid_lut_id`; validated by `ParametricAdjustments::validate`). `ParametricAdjustments` otherwise unchanged
  (it already carried every Process 2012 slider + `lut`).
- `ParametricAdjustments::copy_fields(src, fields)` (defines fields-mask semantics) and `is_neutral()`.
- New `AdjustmentField` (fields mask groups): `white_balance, exposure, contrast, highlights, shadows, whites, blacks,
  texture, clarity, dehaze, vibrance, saturation, hsl_hue, hsl_saturation, hsl_luminance, lut`.
- New `RenderSlot` (`main | before | detail`), `RenderOptions { maxEdge 64..=8192, slot, region: NormRect | null }`,
  `Histogram { red, green, blue, luma }` (256 bins each, of the 8-bit sRGB output),
  `RenderedPreview { imageId, slot, seq, url, width, height, histogram, renderMs, lutMissing }`.
- New `WhiteBalanceValues { temperatureK, tint }`, `DevelopInfo { imageId, asShot, sourceWidth, sourceHeight,
  fullWidth, fullHeight }`.
- New `HistoryEntry { id, label, createdAtMs }`, `AdjustmentHistory { imageId, entries, currentEntryId, canUndo,
  canRedo }`, `EditState { adjustments, history }`.
- New `Preset { id, name, adjustments, fields, createdAtMs, updatedAtMs }`.
- New `LutKind` (`lut_1d | lut_3d`), `LutInfo { id, name, kind, size, path }`.
- `RawImageEntry.hasEdits` now means "adjustments differ from neutral" (a reset image has a row but no edits).
- `XmpSyncState.dirty` / `XmpSyncReport.changed` now also cover develop settings (crs:).

Commands (changed)
- `save_adjustments(id, adjustments, label: string) -> AdjustmentHistory` (BREAKING: new `label`, returns history).
  Pushes a history entry; same-label saves within 1.5 s coalesce; unchanged values are a no-op. Notifies XMP auto-sync.
- `write_xmp` / auto-sync also write `crs:` develop settings (images with an adjustments row only); `read_xmp` and
  `import_folder` also import `crs:` settings (PV2012+) from Lightroom-edited sidecars. Mapping: `xmp/crs.rs`.

Commands (new)
- Render: `render_preview(id, adjustments, options) -> RenderedPreview | null` (null = superseded, latest-wins per
  (id, slot)); `get_develop_info(id) -> DevelopInfo`; `prepare_develop(ids) -> null` (background decode).
- History: `get_history(id) -> AdjustmentHistory`; `undo_adjustments(id)`, `redo_adjustments(id)`,
  `goto_history(id, entryId)` -> `EditState`.
- Batch (atomic, one history entry per changed image, XMP notify): `paste_settings(ids, adjustments, fields)`,
  `sync_settings(sourceId, targetIds, fields)`, `reset_adjustments(ids)`, `apply_preset(ids, presetId)`.
  Copy is frontend-only (keep the copied `ParametricAdjustments` + fields in UI state).
- Presets: `list_presets() -> Preset[]`, `save_preset(id | null, name, adjustments, fields) -> Preset`,
  `delete_preset(id)`.
- LUTs: `list_luts() -> LutInfo[]`, `import_lut(path) -> LutInfo`, `delete_lut(id, force)` (`invalid_argument` if
  referenced and not `force`).

Events: none new (renders resolve their command; `xmpSynced.read` covers develop settings read by auto-sync).

Transport
- New custom URI scheme `sieve` (`register_asynchronous_uri_scheme_protocol`, served on the blocking pool from
  memory): `sieve://localhost/render/<id>/<slot>?v=<seq>` (Windows: `http://sieve.localhost/...`). CSP `img-src` and
  `connect-src` allow `sieve: http://sieve.localhost`. Use `RenderedPreview.url` as `<img src>` directly.

Schema (migration `0005_editor.sql`, user_version 5)
- `adjustments.neutral` (drives `hasEdits`), `adjustments.history_entry_id` (history cursor).
- New tables `adjustment_history` (+ `idx_adjustment_history_image`) and `presets` (name unique, NOCASE).
- Triggers `adjustments_xmp_insert` / `adjustments_xmp_update` set `images.xmp_dirty` + `meta_updated_at` on real
  develop changes.
- Data fix: path-style `lut` objects removed from stored adjustments.

Config
- `SIEVE_LUTS=/dir` (default `<app_data_dir>/luts`, created at startup); `SIEVE_DEVELOP_CACHE_MB` (default 1024).

Who updates what
- architect (done): types + tests, commands, registration, `sieve` protocol registration, managed `DevelopCache` +
  `LutLibrary`, CSP, migration + test, `repo::save_adjustments` keeps `neutral`, `ENTRY_SELECT` `hasEdits` via
  `neutral = 0`, `DevelopCache::ticket/is_current` (latest-wins bookkeeping), `render_url`, `ALL_ADJUSTMENT_FIELDS`
  in `src/ipc/index.ts`.
- rust-engine-dev: fill every `todo!()` in `src-tauri/src/develop/` (`DevelopCache::{render, info, prefetch,
  encoded}`, `handle_protocol`, `source::decode_half_size` + LibRaw FFI additions, `pipeline::render`,
  `wb::{multipliers_for, values_for}`, `history::*`, `presets::*`), `src-tauri/src/lut/` (`Lut::{parse, apply}`,
  `LutLibrary::{list, import, delete, load}`, `references`), and `src-tauri/src/xmp/crs.rs` (`encode`, `decode`) +
  wiring into `XmpSync` write/read/refresh (develop settings read through `history::commit(.., LABEL_READ_XMP)`,
  listed in `changed`). Acceptance: cached 2048 px render < 100 ms; XMP crs round trip lossless; LUT reference values.
- frontend-dev: editor panel (sliders -> `renderPreview` per input event with `slot: "main"`, ignore `null` results
  and results older than the last shown `seq`; `saveAdjustments(id, adj, "<Slider name>")` on release),
  histogram from `RenderedPreview.histogram`, before/after (`slot: "before"`, neutral or "Original" adjustments),
  WB mode + Temp/Tint seeded from `getDevelopInfo().asShot`, history panel + Cmd+Z / Shift+Cmd+Z
  (`undoAdjustments` / `redoAdjustments`, `gotoHistory`), copy/paste/sync with a fields dialog
  (`ALL_ADJUSTMENT_FIELDS`), presets (`listPresets`/`savePreset`/`applyPreset`/`deletePreset`), LUT picker
  (`listLuts`/`importLut` via the dialog plugin, amount slider). Add mock cases for the new commands in
  `src/testing/mockBackend.ts` (render `url` can point at a Playwright-routed JPEG). `saveAdjustments` gained `label`.

## v6 — 2026-09-29 (Phase 6: export)
Additive only; no existing type or command changed.

Types
- Ids: `ExportPresetId` (user presets > 0, built-ins < 0), `ExportJobId`.
- Enums (snake_case strings): `ExportFormatKind` (`jpeg | tiff | png | webp | heic`), `BitDepth` (`"8" | "16"`),
  `ChromaSubsampling` (`"444" | "422" | "420"`), `TiffCompression` (`none | lzw | zip`), `ExportColorSpace`
  (`srgb | display_p3 | adobe_rgb`), `SharpenMedia` (`screen | matte | glossy`), `SharpenAmount`
  (`low | standard | high`), `CollisionPolicy` (`unique_suffix | overwrite | skip`), `MetadataInclude`
  (`all | copyright_only | copyright_and_contact | none`), `ExportJobState`
  (`queued | running | completed | cancelled | interrupted`).
- Tagged unions (`kind`): `ExportFormat` (`jpeg {quality, chromaSubsampling}`, `tiff {bitDepth, compression}`,
  `png {bitDepth}`, `webp {quality, lossless}`, `heic {quality}`), `ResizeMode` (`none`, `long_edge {px}`,
  `short_edge {px}`, `megapixels {mp}`, `width_height {width, height}`), `ExportDestination` (`choose`,
  `folder {path}`, `source_folder`).
- Structs: `ResizeOptions {mode, dontEnlarge, resolutionPpi}`, `OutputSharpening {media, amount}`,
  `FileNaming {template, startNumber, collision}`, `MetadataOptions {include, removeLocation, includeKeywords,
  copyright, creator}`, `ExportSettings {format, colorSpace, resize, sharpening, naming, destination, subfolder,
  metadata}` (+ `validate()`), `ExportPreset {id, name, builtIn, settings, createdAtMs, updatedAtMs}`
  (+ `builtins()`), `ExportFormatInfo {kind, available, reason, bitDepths, supportsMetadata}`,
  `ExportCapabilities {formats, maxParallel, memoryBudgetMb}`, `PlannedFile {imageId, path, exists}`,
  `ExportPlan {outputDir, files, existing}`, `ExportFailure {imageId, fileName, reason}`,
  `ExportJob {id, state, presetName, format, total, done, succeeded, failed, skipped, outputDir, failures,
  createdAtMs, finishedAtMs}`.
- File-name templates: literal text + `{filename}`, `{seq}` / `{seq:N}`, `{date}` / `{date:FMT}` (YYYY YY MM DD hh mm
  ss, `- _ .` space), `{rating}`, `{camera}`, `{folder}`, `{id}`; parsed/validated by `parse_filename_template`
  (Rust, not on the wire). TS mirrors: `EXPORT_FILENAME_TOKENS`, `EXPORT_EXTENSIONS` in `src/ipc/index.ts`.
- Built-in presets (read-only, destination `choose`): -1 "Client JPEG full-res sRGB q90" (JPEG q90 4:4:4, full
  res, 300 ppi, no sharpening, metadata all), -2 "Web 2048 sRGB" (JPEG q80 4:2:0, long edge 2048, 72 ppi, screen
  standard, copyright only, location removed), -3 "Print TIFF 16-bit Adobe RGB" (TIFF 16 LZW, full res, 300 ppi,
  glossy standard, metadata all).

Commands (new)
- `get_export_capabilities() -> ExportCapabilities`.
- `list_export_presets() -> ExportPreset[]` (built-ins first); `save_export_preset(id | null, name, settings) ->
  ExportPreset` (built-in id -> `invalid_argument`); `delete_export_preset(id)`.
- `plan_export(ids, settings) -> ExportPlan` (dry run: final paths + existing files).
- `export_images(ids, settings, presetName | null) -> ExportJob` (queued; returns immediately; adjustments
  snapshotted; `choose` destination / empty ids -> `invalid_argument`; unknown id -> `not_found`; duplicate ids
  dropped).
- `cancel_export(jobId)`; `get_export_jobs() -> ExportJob[]` (active first, then 50 most recent).

Events (new)
- `exportProgress {jobId, done, total, failed, skipped, currentFile}` (throttled).
- `exportFinished {jobId, succeeded, skipped, failed: ExportFailure[], cancelled, outputDir, elapsedMs}` (once per job).

Schema (migration `0006_export.sql`, user_version 6)
- `export_presets` (user presets; name unique NOCASE; `settings_json`).
- `export_jobs` (state CHECK, counters, resolved `output_dir`, `settings_json`, timestamps) + `idx_export_jobs_created`.
- `export_items` (`(job_id, seq)` PK, image, status `pending|done|failed|skipped`, `output_path`, `error`;
  cascades from jobs and images) + `idx_export_items_image`.

Config
- `SIEVE_EXPORT_MEMORY_MB` (default 25% of physical RAM clamped to 2..=8 GiB).

Who updates what
- architect (done): types + validation + template parser + built-ins + tests, commands (thin bodies), events,
  registration, managed `Exporter` (+ `recover_interrupted` at startup), migration + test, memory policy fns
  (`export::memory_budget_bytes`, `estimate_image_bytes`, `MAX_PARALLEL`) + test, TS helpers.
- rust-engine-dev: fill every `todo!()` in `src-tauri/src/export/`: `Exporter::{capabilities, enqueue, cancel,
  jobs}` (+ real `recover_interrupted`), `plan`, `presets::{list, save, delete}`, `develop::{decode_full,
  output_size, render_full}`, `naming::expand`, `encode::{icc_profile, probe_formats, write_file}`,
  `metadata::collect`; refactor `develop::pipeline` so preview and export share the stages up to the linear
  working image (see `export/develop.rs` docs). Acceptance (roadmap Phase 6): 50-file batch per format from
  `test-data/`, dimensions / JPEG quality / ICC / metadata verified with `exiftool`, memory flat during batch.
- frontend-dev: export dialog (preset list via `listExportPresets`, preset editor with every `ExportSettings`
  field, formats greyed out when `getExportCapabilities().formats[i].available` is false, token help from
  `EXPORT_FILENAME_TOKENS`, destination picker via the dialog plugin replacing `choose` with `folder`),
  `planExport` warning for existing files, `exportImages(selectedIds, settings, preset.name)`, a jobs panel
  (`getExportJobs`, `exportProgress` / `exportFinished`, `cancelExport`). Add mock cases for the new commands in
  `src/testing/mockBackend.ts` (the build does not need them; UI tests will).

## v7 — 2026-09-29 (Phase 7: scenes + scene matching)
Types
- `SceneId`; `SceneMethod` (`auto | manual`); `SceneTask` (`detect | match`).
- `Scene { id, folderId | null, startedAtMs | null, endedAtMs | null, imageIds (capture order), anchorIds (0..=2,
  subset), method, createdAtMs, updatedAtMs }` (`Scene::MAX_ANCHORS = 2`).
- `SceneDetectOptions { maxGapMs (1000..=86400000, default 120000), similarity (0..=1, default 0.7), replaceManual
  (default false) }` (+ `validate()`, `Default`).
- `MatchOptions { matchExposure (true), matchWhiteBalance (true), matchTone (false), strength 0..=1 (1), copyFields
  (all) }` (+ `validate()`, `matched_fields()`, `MAX_TARGETS = 2000`). TS mirror: `DEFAULT_MATCH_OPTIONS`.
- `ImageStats { imageId, region, width, height, meanLuma, logMeanLuma, percentiles: LumaPercentiles {p1,p10,p50,p90,p99},
  clippedHighlights, clippedShadows, meanOklab: OklabColor {l,a,b}, neutral: NeutralEstimate {x, y, a, b, coverage},
  whiteBalance | null (effective), asShot | null, lutMissing }` — measured on the rendered 8-bit output at 640 px.
- `MatchDelta { exposure, temperatureK, tint, contrast, whites, blacks }`.
- `MatchPreview { targetId, anchorIds, anchorWeight, base, full, adjustments, delta, reference, before, predicted,
  converged, notes }`; `MatchApplication { imageId, adjustments }`.
- `ParametricAdjustments::lerp(a, b, t)` (numeric linear, temperature in mireds when both custom, otherwise nearer side;
  LUT amount when same id). TS mirror `lerpAdjustments` in `src/ipc/index.ts` (keep in sync).
- `RawImageEntry` gains `sceneId: number | null`, `isSceneAnchor: boolean`.
- `ImageQuery` gains `sceneId: number | null` (members of that scene; `#[serde(default)]`, so omitting it still
  deserializes). `DEFAULT_QUERY` updated.
- TS constants: `SCENE_MATCH_TOLERANCE = { ev: 0.15, ab: 0.012 }` (Rust `scene::TOLERANCE_EV/AB`), `MAX_SCENE_ANCHORS`.

Commands (new)
- `detect_scenes(folderId | null, options | null) -> Scene[]` (blocking; replaces auto scenes in scope, keeps manual
  ones unless `replaceManual`, anchors survive; progress `sceneProgress {task: "detect"}`).
- `list_scenes(folderId | null) -> Scene[]`, `get_scene(id) -> Scene`.
- `create_scene(imageIds) -> Scene`, `set_scene_members(id, imageIds) -> Scene`, `set_scene_anchors(id, anchorIds) ->
  Scene`, `merge_scenes(ids) -> Scene`, `split_scene(id, firstImageId) -> Scene[2]`, `delete_scene(id)`. Membership
  edits make a scene `manual`; anchors are members (<= 2); an image leaving a scene loses its anchor flag; emptied
  scenes are deleted.
- `match_scene(anchorIds (1..=2), targetIds, options) -> MatchPreview[]` (nothing saved; anchors dropped from targets;
  progress `sceneProgress {task: "match"}`).
- `apply_scene_match(applications: MatchApplication[], label | null) -> number[]` (changed ids; atomic; one history
  entry per changed image, default label "Match Scene"; XMP notify).
- `get_render_stats(id, adjustments | null, region | null) -> ImageStats` (acceptance helper; `null` = stored
  adjustments).

Events (new)
- `sceneProgress { task, done, total }` (throttled).

Schema (migration `0007_scenes.sql`, user_version 7)
- `scenes` (folder_id nullable, started/ended ms, method CHECK, timestamps) + `idx_scenes_folder`.
- `images.scene_id` (FK ON DELETE SET NULL) + `idx_images_scene`, `images.scene_anchor`.
- `scene_features` (image_id PK cascade, version, features_json, computed_at).
- Scene columns are not XMP-mapped (no dirty triggers).

Who updates what
- architect (done): types + tests, commands, event, registration, migration + test, `scene/` module surface with
  `scene::store` (all scene SQL, implemented + tested), pure semantic helpers `matching::{base_adjustments,
  choose_anchors, delta, touched_fields}` + tests, `stats::render_stats` glue, additive seams
  `DevelopCache::render_image` (+ `RenderedPixels`) and `develop::history::commit_batch`, `repo` ENTRY_SELECT cols
  46-47 + `sceneId` filter, TS helpers, mock backend fields.
- vision-ml-dev: fill every `todo!()` in `src-tauri/src/scene/`: `features::{compute, compute_missing}`,
  `detect::group` (rules in its doc), `stats::measure`, `matching::{blend_stats, solve, match_images}`; tune
  `SceneDetectOptions::default()` values if data demands (contract change -> architect). Acceptance (roadmap Phase 7):
  on sample scenes from `test-data/`, every matched target at strength 1 has |logMeanLuma - reference| <=
  `TOLERANCE_EV` and |neutral.ab - reference| <= `TOLERANCE_AB` (`get_render_stats` with `MatchPreview.full`);
  plus unit tests for `detect::group` rules and `solve` on synthetic renders.
- rust-engine-dev: nothing required; review `DevelopCache::render_image` / `history::commit_batch` (may move them).
- frontend-dev: see "Frontend API summary" below; add mock cases for the new commands in `src/testing/mockBackend.ts`.

Frontend API summary
- Scene strip (library/develop): `listScenes(folderId)` on folder change; "Detect scenes" button ->
  `detectScenes(folderId, null)` with a progress bar from `events.sceneProgress` (`task === "detect"`); clicking a
  scene filters the grid with `ImageQuery.sceneId`; badges from `RawImageEntry.sceneId` / `isSceneAnchor`.
- Scene editing: select frames -> "New scene" (`createScene`), "Merge" (`mergeScenes`), "Split here" (`splitScene`),
  "Remove from scene" (`setSceneMembers` with the rest), delete (`deleteScene`).
- Anchors: "Mark as anchor" in grid/develop toggles `setSceneAnchors(sceneId, [...])` (max `MAX_SCENE_ANCHORS`;
  replace the oldest or refuse with a toast).
- Match: "Match scene" -> `matchScene(scene.anchorIds, scene.imageIds, options)` (options panel: exposure / white
  balance / tone toggles, copy-fields picker reusing the Sync dialog, strength slider; progress from
  `sceneProgress` `task === "match"`). Show per-target before/after (render `before` = stored adjustments,
  `after` = `lerpAdjustments(p.base, p.full, strength)` via `renderPreview`), `delta` values, `converged` / `notes`
  warnings. Strength slider recomputes locally with `lerpAdjustments` (no new `matchScene` call).
- Apply: `applySceneMatch(selected.map(p => ({ imageId: p.targetId, adjustments: lerpAdjustments(p.base, p.full,
  strength) })), null)`; then refresh rows (`getImages`) and history; undo per image via the existing history.

## v8 — 2026-09-29 (UX additions, from `docs/ux-review-1.md` "Needs architect")
Types (new)
- `ApplySuggestionsResult { applied, skipped }`.
- `CullSnapshot { imageId, rating, pick, colorLabel }` (culling undo; tags not included).
- `UiPrefs { lastExportFolder?: string | null }` (extensible; all fields optional).

Commands
- BREAKING `apply_suggestions(ids, onlyUnset: boolean) -> ApplySuggestionsResult` (was `(ids) -> number`).
  `onlyUnset` skips images already flagged (`pick != unflagged`) or rated (`rating != 0`); `skipped` also counts
  unanalyzed images. XMP notified only when something was applied.
- New `set_burst_keeper(groupId, imageId) -> BurstGroup`: user-chosen keeper; moves `duplicate_burst`
  (keeper loses it, other members gain it; suppressed/user rows untouched), pins the choice so regrouping keeps
  it, notifies XMP sync and kicks a `rescore` (suggestions follow; `analysisFinished` when done).
  Unknown group/image -> `not_found`; non-member -> `invalid_argument`.
- New `get_cull_snapshot(ids) -> CullSnapshot[]` and `restore_cull_snapshot(snapshots) -> number[]` (changed ids).
  Restore is atomic (unknown id / rating > 5 -> nothing written), marks changed images XMP-dirty, notifies sync.
- New `get_ui_prefs() -> UiPrefs` / `set_ui_prefs(prefs) -> null` (replace whole struct; stored as JSON in
  `catalog_meta['ui_prefs']`; unreadable JSON reads as defaults).
- New `reveal_in_finder(path) -> null`: `/usr/bin/open -R <path>` (no shell). Path must be absolute
  (`invalid_argument`) and exist (`not_found`). No plugin/capability needed.
- New `write_xmp_all_dirty(folderId | null) -> XmpSyncReport`: writes every XMP-dirty image (catalog wins),
  regardless of auto-sync. Chosen over an `ImageQuery.xmpDirtyOnly` flag (one call, no id round trip).

Schema (migration `0008_ux.sql`, user_version 8)
- `burst_keeper_pins (image_id PK -> images ON DELETE CASCADE, pinned_at)`. `ml::worker::rescore_all` applies
  pins via `ml::bursts::apply_pins` (pinned member with best `overall` becomes keeper).

Who updates what
- architect (done): types, commands (bodies implemented: thin repo/store code), registration, migration, repo
  `apply_suggestions`/`cull_snapshot`/`restore_cull_snapshot`/`ui_prefs`/`set_ui_prefs`, `ml::store::{set_burst_keeper,
  set_duplicate_tag, pinned_keepers}` (+ `write_bursts` now uses `set_duplicate_tag`), `ml::bursts::apply_pins` +
  its call in `rescore_all`, `xmp::XmpSync::write_dirty` + `xmp::store::dirty_ids_in`, tests for each, bindings,
  mock backend cases, `App.tsx` call site of `applySuggestions` (passes `false`, reads `.applied`).
- vision-ml-dev: nothing required; review `apply_pins` in `rescore_all`.
- rust-engine-dev: nothing required; review `XmpSync::write_dirty`.
- frontend-dev: build the UX items on these commands (see architecture.md "UX additions (v8)").

## v9 — 2026-09-29 (Phase 7b: Lightroom develop parity, profiles/looks, non-RAW sources)
Types
- BREAKING (TS name only) `RawFormat` -> `ImageFormat` (+ `jpeg | heic | tiff | png`); Rust keeps `type RawFormat =
  ImageFormat`, TS `index.ts` exports `type RawFormat = ImageFormat`. Helpers: `is_raw`, `from_extension`,
  `pairs_with_raw`, `ImageFormat::RAW`; TS `RAW_FORMATS`, `isRawFormat`.
- `ParametricAdjustments` (additive; `#[serde(default)]`, so TS fields are optional and old JSON deserializes):
  `toneCurve: ToneCurve {parametric: ParametricCurve, point: PointCurves}`, `colorGrading: ColorGrading`
  (`ColorWheel` x4, `blending`, `balance`), `calibration: CameraCalibration` (`PrimaryCalibration` x3,
  `shadowTint`), `detail: DetailAdjustments {sharpening: Sharpening, noiseReduction: NoiseReduction}`,
  `effects: EffectsAdjustments {vignette: PostCropVignette (+ VignetteStyle), grain: Grain}`,
  `blackAndWhite: BlackAndWhite`, `crop: CropSettings`, `profile: ProfileSettings {cameraProfile, look:
  LookSettings {name, uuid, amount}}`. Ranges in the Rust docs / `validate()`. Defaults = Lightroom's
  (`defaults_for(format)`, `is_neutral_for(format)`); generated TS constants `DEFAULT_ADJUSTMENTS`,
  `DEFAULT_ADJUSTMENTS_NON_RAW`. `copy_fields`, `lerp` (+ TS `lerpAdjustments`, `copyAdjustmentFields`) cover the
  new groups.
- `AdjustmentField` += `tone_curve, color_grading, calibration, sharpening, noise_reduction, vignette, grain,
  black_and_white, crop, profile`; `AdjustmentField::DEFAULT_SYNC` (all but crop) / TS `DEFAULT_SYNC_FIELDS`,
  `ADJUSTMENT_FIELD_LABELS`. `MatchOptions::default().copyFields` = `DEFAULT_SYNC` (was ALL).
- `DevelopWarningCode` (`profile_unavailable, look_unavailable, masks_unsupported, retouch_unsupported,
  lens_corrections_unsupported, transform_unsupported, legacy_process_version, source_color_assumed`),
  `DevelopWarning {code, detail}`.
- `RawImageEntry` += `companionPath: string | null`, `developWarnings: DevelopWarning[]`.
- `DevelopInfo` += `warnings: DevelopWarning[]` (entry warnings + profile/look availability + source colour);
  `asShot` is `{6500, 0}` for non-RAW sources.
- `ImportOptions` += `includeNonRaw` (default false), `pairJpegWithRaw` (default true) (both optional on the wire).
  `ImportSummary` += `companions`.
- `CameraProfileInfo`, `LookProfileInfo`, `ProfileCatalog`. TS: `CurvePoint`, `IDENTITY_CURVE`,
  `MAX_CURVE_POINTS`, `defaultAdjustments(format)`, `completeAdjustments(adj, format)`, `CompleteAdjustments`.

Commands
- New `list_profiles(id) -> ProfileCatalog` (installed DCPs for the image's camera + installed looks).
- `reset_adjustments` resets to the image's format default (RAW vs non-RAW), incl. profile.
- `get_develop_info` merges the image's stored `developWarnings` into `warnings`.
- `import_folder` honours the new options (pairing implemented in `repo::import_folder`).
- `get_adjustments` for an unedited non-RAW image returns `DEFAULT_ADJUSTMENTS_NON_RAW`.

XMP
- `crs:` mapping extended (table: `docs/architecture.md` "crs mapping", source of truth `xmp/crs.rs`
  `PARITY_SCALARS`, `PARITY_BOOLS`, `CRS_CURVES`, `CAMERA_PROFILE`). Scalars are written and read now; point curves
  are read now (written once `packet` supports Seq replace); profile/look read+write pending (`decode_profile`,
  `encode_profile` stubs). Unsupported features stored per image at every read (`crs::unsupported_warnings`).
- Non-RAW sidecar path: `<file name>.xmp` (e.g. `DSCF1234.JPG.xmp`), `xmp::sidecar_path`.

Schema (migration `0009_parity_sources.sql`, user_version 9)
- `images.format` CHECK accepts `jpeg, heic, tiff, png` (in-place `writable_schema` edit; no table rebuild).
- `images.companion_path TEXT`, `images.develop_warnings TEXT` (JSON; NULL = none). Neither is XMP-mapped.

Config
- `SIEVE_CAMERA_PROFILES`, `SIEVE_LOOK_PROFILES` (`:`-separated dirs; default system + user Adobe CameraRaw dirs).

Who updates what
- architect (done): types + validation + tests, generated constants, `list_profiles` + `ProfileLibrary` managed
  state, migration + test, `repo::{import_folder (pairing), image_format, get_adjustments / save_adjustments (format
  defaults), ENTRY_SELECT cols 48-49}`, `raw::{format_from_extension, header_matches, default_*}` for new formats,
  `xmp::{sidecar_path (non-RAW), store::set_develop_warnings}` + read-path wiring, `crs` scalar parity
  encode/decode + curve helpers + `unsupported_warnings` + tests, `packet::DocSource` (`CrsSource` impl, `look()`
  returns None), `DevelopInfo.warnings` plumbing, TS helpers, mock backend (`list_profiles`, complete adjustments,
  new entry fields), minimal `src/lib/adjust.ts` delegation (`FIELD_LABEL`, `copyFields` -> `src/ipc`).
- rust-engine-dev: every `todo!()` in `raw/raster.rs` (non-RAW ingest/decode/embedded XMP), `profiles/`
  (`ProfileLibrary::{catalog, dcp, look}`, `Dcp::{parse, peek_names, illuminant_weight}`, `LookProfile::{parse_file,
  from_sidecar}`, `table::decode`), `develop/parity.rs` (all stages + `crop_geometry`), `xmp/crs.rs`
  (`decode_profile`, `encode_profile`); `packet`: nested-struct reader (`DocSource::look`, Look parameters),
  `rdf:Seq` create/replace + `Desired` seq edits (wire `encode_curves`), `<crs:Look>` struct write; develop/export
  integration: dispatch non-RAW paths to `raw::raster` (by extension), `display_referred` flag, profile + look
  stages, crop-aware sizes/regions, `DevelopCache::info` warnings (profile/look availability, source colour), reset
  of history "Original" for non-RAW (uses `repo::get_adjustments`, already format-aware), embedded-XMP fallback in
  the read path. Acceptance: roadmap Phase 7b (ΔE2000 / visual parity on the user's frames, crs round trip).
- frontend-dev: panels Tone Curve (parametric sliders + split handles, point-curve editor with channel selector
  master/R/G/B, `MAX_CURVE_POINTS`), Color Grading (3 wheels + global, luminance sliders, blending, balance),
  Calibration, Detail (sharpening + noise reduction), Effects (vignette + grain), B&W toggle + mixer, Crop
  (straighten angle + enable/reset; drag tool can follow), Profile browser (`listProfiles`, groups, look amount when
  `supportsAmount`); import dialog toggles (`includeNonRaw`, `pairJpegWithRaw`); warning badges from
  `RawImageEntry.developWarnings` and `DevelopInfo.warnings`; companion indicator ("RAW+JPG"). Use
  `defaultAdjustments(entry.format)` for "neutral" / before, `completeAdjustments` when reading, and
  `DEFAULT_SYNC_FIELDS` as the Sync default. Replace `neutralAdjustments()` in `src/lib/adjust.ts` with
  `defaultAdjustments(format)`.

## v10 — 2026-09-29 (Phase 7c: local adjustments / masks)
Model = Lightroom's `crs:MaskGroupBasedCorrections`, 1:1. Types in `src-tauri/src/ipc/masks.rs` (re-exported from
`ipc::types`); design, frames and XMP mapping in `docs/architecture.md` "Masks (Phase 7c, IPC v10)".

Types
- `ParametricAdjustments.masks: MaskGroup[]` (`#[serde(default)]`, TS-optional, backend always sends it; default
  `[]`). History, undo, presets, copy/paste/sync cover masks. `AdjustmentField` += `masks` (label "Masking"); not in
  `DEFAULT_SYNC` / `DEFAULT_SYNC_FIELDS` (per-frame, like crop). `copy_fields(masks)` drops AI digests
  (`MaskGroup::transferable`, TS `transferableMaskGroup`): the target recomputes its mattes. `lerp`: nearer side's.
- `MaskGroup {id, name, active, amount (0..=2), adjustments: LocalAdjustments, components: MaskComponent[]}`;
  `MaskComponent {id, name, active, mode: MaskBlendMode (add|subtract|intersect), inverted, opacity (0..=1),
  shape: MaskShape}`; ids = Lightroom SyncIDs (32 upper hex; TS `newMaskId()`).
- `MaskShape` (tag `kind`): `brush {strokes: BrushStroke[] {radius, flow, feather, density, erase, autoMask,
  dabs: NormPoint[]}}`, `linear {zero, full}`, `radial {top, left, bottom, right, angle, midpoint, roundness,
  feather, flipped}`, `luminance {featherLow, low, high, featherHigh, smoothness}`, `color {samples: ColorSample[]
  {point, area, lightroomModel}, amount}`, `ai {target: AiTarget, referencePoint, digest}`, `unsupported {what}`
  (Lightroom kinds Sieve does not model; preserved, not creatable).
- `AiTarget` (tag `kind`): `subject | sky | background | people {parts: PersonPart[]} | object {region} |
  landscape {category} | other {subType, subCategory}`. `PersonPart` (face_skin, body_skin, eyebrows, eye_sclera,
  iris_pupil, lips, teeth, hair, clothes), `LandscapeCategory`, `AiTargetKind`.
- `LocalAdjustments` (UI units: temperature, tint, exposure -4..=4 EV, contrast, highlights, shadows, whites, blacks,
  texture, clarity, dehaze, hue -180..=180, saturation, sharpness, noise, moire, defringe, color {hue, saturation},
  curveRefineSaturation (default 100), toneCurve: PointCurves); generated constant `DEFAULT_LOCAL_ADJUSTMENTS`.
- **Frame**: every mask coordinate is in the *sensor frame* (un-oriented, uncropped, normalized), like Lightroom;
  brush radius = fraction of the sensor width. Helpers `orient_point` / `unorient_point` (TS `orientPoint`,
  `unorientPoint`).
- Command types: `AiMaskRequest`, `AiMaskInfo`, `AiMaskOrigin` (lightroom|sieve), `AiMaskState`
  (ready|needs_update|computing|unavailable), `AiMaskStatus`, `MaskList`, `DetectedPerson`, `MaskOverlayTarget`,
  `MaskOverlayOptions`, `RenderedMaskOverlay`, `AiCapability`, `MaskCapabilities`. Limits `MaskLimits` (TS
  `MASK_LIMITS`): 100 groups, 64 components/group, 200k dabs, 5 colour samples, 64-char names.
- `RenderSlot` += `mask` (overlays only; `render_preview` rejects it). `DevelopWarningCode` += `ai_mask_needs_update`;
  `masks_unsupported` now means "components Sieve cannot render" once the v10 reader lands.
- `validate()` covers masks (`validate_masks`). TS helpers: `newMaskGroup`, `newMaskComponent`,
  `defaultLocalAdjustments`, `completeAdjustments` fills `masks`.

Commands
- `list_masks(id) -> MaskList` (stored groups + AI component status).
- `save_masks(id, masks, label) -> AdjustmentHistory` (replaces only `masks`; one history entry; XMP dirty).
  Implemented (thin glue over `repo::get_adjustments` + `history::commit`).
- `compute_ai_mask(id, request: AiMaskRequest) -> AiMaskInfo` (blocking pool; cached per image + kind + model).
- `detect_people(id) -> DetectedPerson[]`.
- `render_mask_overlay(id, adjustments, target, options) -> RenderedMaskOverlay | null` (grayscale JPEG on
  `sieve://localhost/render/<id>/mask?v=<seq>`, latest-wins; same frame as `render_preview`).
- `get_mask_capabilities() -> MaskCapabilities`.
- Live preview: masks travel inside `render_preview`'s `adjustments` (no extra call per frame).

Schema (migration `0010_masks.sql`, user_version 10)
- `mask_cache (image_id, digest, kind, origin, model_version, input_digest, path, width, height, bounds_x/y/w/h,
  coverage, created_at)`, PK (image_id, digest), index (image_id, kind, model_version), cascade on image delete.
  Mattes = PNG files under `<cacheDir>/masks/`.
- `images.masks_pending_import` (1 = sidecar had masks at a pre-v10 read; set by the migration from
  `develop_warnings`). The XMP writer must not touch masks while it is 1.

Managed state (lib.rs): `develop::masks::MaskCache` (config: catalog path, cache dir) and `ml::masking::Segmenter`
(models dir, catalog path; holds a `MaskCache`). Both constructors do no I/O.

Who updates what
- architect (done): types, validation, tests, generated constant, commands + registration, migration + test,
  stubs below, TS helpers, mock backend (`list_masks`, `save_masks`, `compute_ai_mask`, `detect_people`,
  `render_mask_overlay`, `get_mask_capabilities`), minimal frontend compile fixes (`useEditor` slot records +
  `mask`, `WarningsChip` text for `ai_mask_needs_update`).
- rust-engine-dev: every `todo!()` in `develop/masks.rs` (`AlphaMask::{sample, coverage}`, `MaskCache::{put,
  resolve, load, status, sweep}`, `MatteSource for MaskCache`, `MaskGeometry`, `evaluate_component`, `combine`,
  `evaluate`, `LocalPlanes::build`, `apply_group_blends`, `render_overlay`) and `xmp/masks.rs` (`read`,
  `decode_table`, `decode_matte` (JXL via ImageIO), `apply`); pipeline integration at the seam documented in
  `develop/masks.rs` (steps 1-6) for preview **and** export (export computes missing AI mattes via
  `Segmenter::compute` first); `DevelopCache` access to `MaskCache` (add `mask_cache` to `DevelopConfig`; the
  architect approves the one-line change in `lib.rs`); XMP read path: import masks + store Lightroom mattes +
  clear `masks_pending_import`; one-off catch-up for flagged images (XmpSync at launch + `read_xmp`); write path:
  run `xmp::masks::apply` after `packet::merge` unless `masks_pending_import`; `crs::unsupported_warnings`:
  replace the blanket `masks_unsupported` with `xmp::masks::MasksRead.warnings`; `DevelopCache::info` adds
  `ai_mask_needs_update`. Acceptance: roadmap Phase 7c (user's masks import and render; round trip preserves them).
- vision-ml-dev: `ml/masking.rs` (`Segmenter::{capabilities, model_version, compute, is_computing,
  detect_people}`) and concrete `SegmentModel`s (e.g. `ml/segment.rs`); models + licences in `docs/decisions.md`.
- frontend-dev: Masks panel + tools (API summary in `docs/architecture.md` "Masks"): groups list (add, rename,
  toggle, delete, reorder, amount), component list with add/subtract/intersect, invert, opacity; local sliders
  (`LocalAdjustments`); tools: brush (size/feather/flow/density/auto-mask, eraser with Alt), linear + radial
  handles, luminance/colour range, AI buttons from `getMaskCapabilities` (People picker via `detectPeople`,
  "Update" for `needs_update`); overlay via `renderMaskOverlay`; shortcuts. Live edits go through
  `renderPreview(adjustments with masks)`; commit with `saveMasks` (or `saveAdjustments`). Convert pointer
  positions with `unorientPoint` (+ crop mapping). Replace the `mask: null/false` placeholders in `useEditor.ts`
  if the editor state is restructured; `RenderSlot` records must include `mask`.

## v11 — 2026-09-30 (UX review 2: white balance picker, P1-12)
Commands
- New `sample_white_balance(id, point: NormPoint, adjustments: ParametricAdjustments) -> WhiteBalanceValues`
  (TS `commands.sampleWhiteBalance(id, point, adjustments)`). `point` is in the **sensor frame** (normalized,
  un-oriented, uncropped; same convention as masks: convert a viewer click with `unorientPoint` + crop mapping).
  Averages the 5x5 develop-source pixels (linear camera RGB, before WB) around the point and returns the Temp/Tint
  that neutralizes them, through the same colour path as `DevelopInfo.asShot` (DCP of `adjustments.profile`, else
  the camera matrix), clamped to 2000..=50000 K / -150..=150. Errors: `invalid_argument` with a user-facing message
  when the point is outside 0..=1, any sample pixel is clipped ("...clipped (overexposed)...") or the sample is too
  dark ("...too dark to measure..."). Decodes the source if not cached.
- No type or schema changes. `develop::camera::values_of_multipliers` factored out of `as_shot_values`.

Who updates what
- architect (done): command + registration, body (`DevelopCache::sample_white_balance`, `develop::sample_multipliers`;
  thin, rust-engine-dev owns it from here), tests, bindings, mock backend (`sample_white_balance`: temperature
  `3000 + 5000·x`, tint `(y − 0.5)·40`, clipped error for `y < 0.05`).
- frontend-dev: P1-12 UI (W toggles the picker; click -> `sampleWhiteBalance` -> commit
  `whiteBalance: {custom: values}` with history label "White Balance: Picker"; show the error message on failure).

## v11.1 — 2026-09-30 (doc-only)
- `CullThresholds.overexposedClipPct` now means the share of a subject face's skin that is blown (every channel ≥ 250); a frame > 60% blown also counts. Saved overrides from earlier versions are read with the new meaning (stricter).
- `QualityScore.suggestedPick`: burst non-keepers are capped below the keeper, never rejected for being duplicates. No type changes.

## v12 — 2026-09-30 (Phase 8: in-app model downloads)
Commands
- `model_downloads_status() -> ModelDownloadStatus` (TS `commands.modelDownloadsStatus()`): downloadable groups
  (currently one, `"segmentation"` = the 6 AI-mask models, 559,081,960 bytes) with per-file `installed` (present
  with the expected size; files are SHA-256 verified before being renamed in) and `downloading` (group id in
  flight or `null`).
- `download_models(group: string) -> null` (TS `commands.downloadModels(group)`): starts a background download
  (thin wrapper over `model_fetch::fetch` into the segmenter's models dir) and resolves immediately.
  `invalid_argument` for an unknown group or while a download is already running. Installed files are skipped,
  `.part` files resumed.
- `cancel_model_download() -> null` (TS `commands.cancelModelDownload()`): no-op when idle.

Types / constants: `ModelDownloadStatus { groups, downloading }`, `ModelGroupStatus { id, label, installed,
bytesTotal, files }`, `ModelFileStatus { name, installed, bytes }`; generated constant `MODEL_GROUP_SEGMENTATION`.

Events
- `ModelDownloadProgress { group, name, fileIndex, fileCount, bytesDone, bytesTotal }` (`model-download-progress`),
  throttled (~5/s, plus one per completed file); bytes cover the whole group.
- `ModelDownloadFinished { group, ok, cancelled, error }` (`model-download-finished`): exactly once per accepted
  `download_models`. `cancelled` (not in the original request) lets the UI skip the error toast on user cancel;
  `error` is the user-facing message (curl error, "<file>: SHA-256 mismatch ...", "model download cancelled").
- After `ok`, `get_mask_capabilities()` reflects the models immediately (the segmenter checks files per call;
  no restart; covered by `model_fetch::tests::mask_capabilities_follow_installed_files_without_restart`).

No schema changes. Managed state `model_fetch::ModelDownloads` registered in lib.rs.

Who updates what
- architect (done): types, events, commands + registration, `ModelDownloads` (thin, in `model_fetch.rs`), Rust
  tests (status with/without files, cancel against a stalled HTTP endpoint, checksum failure event, busy /
  unknown group, capabilities flip), bindings, mock backend (`?models=missing` starts uninstalled with AI
  families unavailable; simulated progress 10 steps/file at `window.__mockModelDelay` ms (default 40);
  `window.__mockModelFail = "<msg>"` fails at the third file; cancel honoured), `tests/ui/models.spec.ts`.
- frontend-dev: download UI where AI masks are unavailable (Masks panel "Download AI models (559 MB)" with a
  progress bar from `modelDownloadProgress`, Cancel, error display on `modelDownloadFinished` unless
  `cancelled`), then refetch `getMaskCapabilities()` on `ok`. On mount, `modelDownloadsStatus().downloading`
  restores an in-flight download's progress state.
- vision-ml-dev (small): the unavailable reason in `Segmenter::available` still says "run
  scripts/fetch-models.sh"; in the app the remedy is the download button (suggest "model file X not installed").

## v13 — 2026-09-30 (Phase 8 hardening: missing originals, catalog health, error kinds)
All changes are additive (no field removed or renamed). Schema v11 (`migrations/0011_hardening.sql`).

Types
- `RawImageEntry.missingSinceMs: number | null` — unix ms when the original was first found missing at `path`
  (render / develop info / WB picker / export / sidecar write / thumbnail extraction / re-import). Cleared by the
  next successful access, a re-import that finds the file, or `relocate_folder`.
- `ImageQuery.missingOnly?: boolean` (default `false`) — only images with `missingSinceMs` set.
- `FilterCounts.missing: number` — images with `missingSinceMs` set in the scope.
- `CatalogState.health: CatalogHealth`:
  `CatalogHealth { status: CatalogHealthStatus ("ok" | "read_only" | "replaced"), message: string | null,
  backups: CatalogBackup[], restorePending: boolean }`,
  `CatalogBackup { index: number (1 = newest), path: string, createdAtMs: number, sizeBytes: number }`.
  `message` is user-facing (null when `ok`). `restorePending` (not in the original request) = a backup is staged
  and will replace the catalog at the next launch.
- `RelocateResult { matched: number, stillMissing: number }`.
- `ErrorKind` += `"file_missing" | "disk_full" | "read_only" | "decode_failed" | "catalog_read_only"`. The sites
  that produced these situations switch kind, messages unchanged: missing original was `not_found` -> `file_missing`;
  disk full was `io` (or `database` for catalog writes) -> `disk_full`; read-only volume / permission denied was
  `io` -> `read_only`; decoder failure on an existing original was `io` -> `decode_failed`; write to a damaged
  catalog was `database` -> `catalog_read_only`. `not_found` now only means "no such catalog row".
- `AiCapability.reason` for missing mask models is now "AI masking models are not installed. Download them from
  the Masks panel (~560 MB)." (was "model file X not installed (run scripts/fetch-models.sh)").

Commands
- `relocate_folder(folderId, newPath) -> RelocateResult` (TS `commands.relocateFolder(folderId, newPath)`):
  repoints a folder and its images (old relative path first, then unique file name anywhere under `newPath`),
  clears their missing flags, forgets their develop sources and re-queues thumbnails that failed as missing.
  Unfound images keep their path and are flagged (`stillMissing`). Errors, nothing changed: `not_found` (folder),
  `invalid_argument` (not a folder / already another catalog folder / none of the photos found).
- `restore_catalog_backup(index) -> CatalogHealth` (TS `commands.restoreCatalogBackup(index)`): stages
  `backups[index]` (`db::stage_restore`, backup must pass `quick_check`); the next launch swaps it in. Errors:
  `not_found` (no such backup), `invalid_argument` (backup damaged). The UI says "Relaunch to finish".

Behaviour
- Clean-shutdown marker: `<catalog>.clean` written on app exit, removed by the next launch, which then skips
  `quick_check` (a crash leaves no marker, so it checks).
- Migration 0011: `idx_images_folder_pick_rating`, partial `idx_image_tags_live`, `images.missing_since_ms`,
  partial `idx_images_missing`. `grid_bench --plans`: all three used; `get_filter_counts(all)` 7.0 ms at 50k.

Who updates what
- architect (done): types, schema, commands + registration, bodies (repo `relocate_folder` / `set_original_missing`
  / `note_access_failure`, `db::health_state` / `mark_clean_shutdown`, error-kind switch in `raw::access` /
  `db::explain_error` / export, missing hooks in ingest/xmp/export/develop commands, masking reason), Rust tests,
  bindings, mock backend, `tests/ui/hardening.spec.ts`, docs. rust-engine-dev owns the bodies from here.
- frontend-dev:
  1. Missing originals: badge on grid cells / loupe when `missingSinceMs != null`; a "Missing" facet in the filter
     bar driven by `FilterCounts.missing` (hidden when 0) that sets `ImageQuery.missingOnly`; on a `file_missing`
     error from render / develop info, show the message with a "Locate folder…" action.
  2. "Locate folder…" (folder context menu and the missing banner): pick a directory, call
     `relocateFolder(folderId, path)`, toast "Found N photos" (+ "M still missing"), then refetch catalog state,
     filter counts and visible rows; show `invalid_argument` messages inline.
  3. Catalog health: when `health.status !== "ok"` show a persistent banner with `health.message` and a
     "Restore backup…" dialog listing `health.backups` (date from `createdAtMs`, size); on confirm
     `restoreCatalogBackup(index)` then show "Relaunch to finish" (also whenever `health.restorePending`).
  4. Map new error kinds to remedies: `disk_full` (free space / choose another destination), `read_only` (choose a
     writable location), `decode_failed` (file damaged / unsupported), `catalog_read_only` (open the restore dialog).
  Mock: `?missing=5` flags images 1–5 missing (render / develop info fail with `file_missing`); `relocateFolder`
  finds everything unless the path contains `empty` (error) or `partial` (first missing image stays missing);
  `?health=read_only|replaced` (read_only refuses rating/pick/label/adjustment writes with `catalog_read_only`);
  three mock backups; `restoreCatalogBackup` sets `restorePending`.

## v14 — 2026-09-30 (Phase 8b: projects, style library, auto tone/WB, guided workflow, style model)
Schema v12 (`migrations/0012_workflow_styles.sql`). **Breaking** for TS callers (new required arguments / fields,
listed below); Rust is the source of truth, `src/ipc/bindings.ts` regenerated, `src/ipc/index.ts` mirrors updated.

Projects (home page)
- A project is one shoot: `Project { id, name, folders: ProjectFolder[] ({id, path, imageCount, exists}),
  coverImageId, coverChosen, coverThumbnailPath, shootType, workflowStep, createdAtMs, lastOpenedAtMs, photoCount,
  keeperCount, editedCount, pickedCount, rejectedCount, missingCount, capturedFromMs, capturedToMs }`.
  Every folder belongs to exactly one project (`FolderEntry.projectId`). Cover = user's choice, else automatic
  (best non-rejected: picked, most stars, earliest). Keeper count uses `CatalogState.keeperRule`.
- Commands: `list_projects()` (last opened first, then newest), `get_project(projectId)`,
  `create_project(path, name | null, shootType | null, options) -> CreateProjectResult {project, import, existing}`
  (name defaults to the folder name; a folder already in the catalog, or inside a catalog folder, returns its
  project with `existing: true`), `open_project(projectId)` (stamps `lastOpenedAtMs`; the app always starts on
  the home page), `rename_project(projectId, name)` (trimmed, 1..=200), `set_project_cover(projectId, imageId | null)`,
  `set_project_shoot_type(projectId, shootType)` (kicks a rescore),
  `remove_project(projectId) -> RemoveProjectResult {removedImages, removedFolders}` (catalog rows + cached
  thumbnails only; originals, sidecars, exports untouched). Reveal in Finder = existing
  `reveal_in_finder(folder.path)`; Locate moved folder = existing `relocate_folder(folderId, newPath)` driven by
  `ProjectFolder.exists == false`.
- `import_folder(path, options, projectId | null)` — **new required argument**: `projectId` adds the folder to
  that project (`invalid_argument` if it is in another one); `null` = the folder's project, or a new project named
  after the folder. A path inside an already imported folder now reuses that folder row (was: a second folder row).
  `ImportSummary.projectId` added.
- Project scoping (**new required arguments**, pass `null` for "no constraint"): `ImageQuery.projectId?`
  (optional on the wire, default `null`), `get_filter_counts(folderId, projectId)`,
  `list_burst_groups(folderId, projectId)`, `list_scenes(folderId, projectId)`,
  `detect_scenes(folderId, projectId, options)`; `AnalysisScope::project {projectId}`. Folder and project AND
  together (a folder of another project matches nothing); unknown project -> `not_found`.
- `ExportJob.projectId: number | null` — the project all of the job's images belong to (derived; drives the
  Export step's "Exported N").
- `CatalogState.shootType` is now the default for new projects; culling scores each image with its project's
  shoot type (see vision-ml-dev below).

Guided workflow (step per **project**, moved from the folder)
- `WorkflowStep = "cull" | "edit" | "export"`; `get_workflow_step(projectId)`, `set_workflow_step(projectId, step)`,
  `Project.workflowStep`. (The v14 draft had `FolderEntry.workflowStep` / folder ids; removed before release.)
- Keepers: `KeeperRule {minRating, useSuggestions}` (`CatalogState.keeperRule`, `set_keeper_rule`,
  `DEFAULT_KEEPER_RULE`; TS mirrors `isKeeper` / `isKeeperValues`).
- Edit step: `get_edit_plan(projectId) -> EditPlan {projectId, keeperRule, keeperIds, unassignedKeeperIds,
  scenes: SceneEditEntry[]}`; `SceneEditEntry {sceneId, imageIds, memberCount, representativeId,
  representativeSource ("auto" | "user"), representativeReason, edited, editedAtMs, appliedAtMs, status
  ("to_edit" | "edited" | "applied" | "outdated")}`; `set_scene_representative(sceneId, imageId | null)`;
  `apply_scene_edit(sceneId, options | null)` / `apply_all_edited_scenes(projectId, options | null)` ->
  `ApplyScenesResult {batch: EditBatchResult, scenes: SceneApplyOutcome[]}` with `SceneApplyOptions
  {matchOptions, includeNonKeepers, skipUserEdited}` (`DEFAULT_SCENE_APPLY_OPTIONS`); `sceneProgress` task
  `"apply"`. `undo_edit_batch(batchId) -> UndoBatchResult {restoredIds, skippedIds}`.
- `paste_previous(targetIds, previousId, fields | null)` (Lightroom "Previous", `PASTE_PREVIOUS_FIELDS`).

Style library (presets + profiles, catalog-wide, grouped by source folder)
- `import_style_folder(path) -> ImportStyleReport {root, groupIds, presets, profiles, skipped}` (recursive:
  `.xmp` develop presets, `.lrtemplate`, `.xmp` creative profiles, `.dcp`, `.cube`; one group per folder,
  re-import replaces), `list_styles() -> StyleLibrary {groups: StyleGroup[]}` ("User Presets", imported groups,
  "LUTs"; `USER_PRESETS_GROUP_ID` / `LUT_LIBRARY_GROUP_ID`), `remove_style_group(groupId)`,
  `resolve_preset(id, presetId, adjustments | null)` (hover preview; applying = existing `apply_preset`, Lightroom
  semantics: only the preset's keys). `StylePreset`, `StyleProfile` (+ `applyStyleProfile` TS mirror),
  `StyleGroupKind`, `StyleProfileKind`, `StyleSourceFormat`.
- `Preset` += `groupId`, `sourceFormat`, `settingKeys`; preset names unique per group. `CameraProfileInfo` /
  `LookProfileInfo` += `styleId`; `ProfileCatalog.luts: LutProfileInfo[]` (LUTs are shown as profiles).
  `LutRef.amount` range 0..=200 (was 0..=100).
- `RenderSlot` += `"navigator"` (Navigator + hover previews; never supersedes `main`).

Auto tone / white balance
- `auto_tone(id, adjustments | null, keys | null) -> AutoToneValues` (keys ⊆ `AUTO_TONE_FIELDS`; Shift-double-click
  = one key; TS `applyAutoTone`), `auto_white_balance(id, adjustments | null) -> WhiteBalanceValues`. Nothing is
  saved; the UI commits one history entry ("Auto Tone" / "Auto White Balance").

Style model ("Auto edit (my style)")
- `style_model_status()`, `train_style_model()` (background; events `styleModelProgress {phase, done, total}` and
  exactly one `styleModelFinished {ok, cancelled, error, status}`), `cancel_style_training()`,
  `predict_style(imageIds) -> StylePrediction[]` (nothing saved), `apply_style_prediction(imageIds) ->
  EditBatchResult` (undoable batch). `StyleModelStatus`, `StyleModelState`, `StyleTrainPhase`, `StyleValidation`.

Copy Settings / misc
- `AdjustmentField` += `noise_reduction_luminance`, `noise_reduction_color` (subsets of `noise_reduction`),
  `process_version`; `COPY_SETTINGS_GROUPS` (Lightroom's Copy dialog layout). `UiPrefs` += `copyFields`,
  `xmpExplainerSeen`, `sceneStripVisible`.
- XMP auto-sync is **on by default**: new catalogs start on; migration 0012 flips existing catalogs whose
  setting was never chosen by the user (`set_xmp_auto_sync` now records `xmp_auto_sync_user_set`).

Schema v12
- New: `projects`, `folders.project_id` (one project per existing folder, same id, named after the folder,
  catalog shoot type, created = folder `added_at`), `export_jobs.project_id`, `style_groups`, `style_profiles`,
  `presets` rebuilt with `group_id` / `source_format` / `settings_json` / `setting_keys_json` /
  `supports_amount` / `warnings_json` (existing rows -> User Presets, same ids), `edit_batches` +
  `edit_batch_items`, `scenes.representative_*` / `applied_*`, `style_models`, `style_features`,
  `catalog_meta.keeper_rule`.

Who updates what
- architect (done): types, schema, commands + registration, catalog-side bodies (`db::projects` incl.
  `FolderScope` scoping of queries/counts/bursts/scenes/edit plan/analysis, import targets, project CRUD and
  removal, export job project, workflow step, keeper rule, edit plan + apply bookkeeping, edit batches, style
  library listing/removal, XMP auto-sync default), Rust tests (migration v12, project scoping, removal), bindings,
  `src/ipc/index.ts` mirrors, mock backend (2 mock projects, v14 commands), minimal compile fixes in `src/` (pass
  `null` for the new scope arguments; `navigator` slot entries), docs.
- rust-engine-dev:
  1. `styles::import_folder` (preset/profile/DCP/cube import, per-folder groups, skips with reasons),
     `styles::resolve_preset` / `apply_preset` for imported presets (key by key, Lightroom semantics), refresh
     `ProfileLibrary` with imported looks/DCPs; `LutRef.amount` > 100 extrapolation in the render.
  2. `develop::auto::auto_tone` / `auto_white_balance` (acceptance: near the user's edits, never clip skin).
  3. Projects: `remove_project` should also evict per-image in-memory caches (mask cache, render cache) and skip
     removed ids still queued in ingest / analysis / XMP sync (TODO in `ipc::commands::remove_project`); optional:
     a new project for a folder that *contains* existing catalog folders could absorb them (today they stay in
     their own projects).
  4. XMP auto-save status + debounce already exist; verify defaults on a fresh catalog.
- vision-ml-dev:
  1. Culling scores each image with its project's shoot type: use `db::projects::shoot_type_of_image` (or a
     per-project grouping) in `ml::worker` (lines using `repo::shoot_type`) for analysis and `Rescore`.
  2. `scene::workflow::propose_representative` (best keeper with the most typical light) and carrying
     `representative_*` / `applied_*` over in `store::replace_scenes`.
  3. `ml::style` (train / status / predict; acceptance: ΔE vs user renders better than Auto tone and no edit).
- frontend-dev:
  1. Projects home page (cards, sort, search, New project via `createProject`, open via `openProject`, rename,
     remove with confirmation, reveal, "Locate folder…" when `folders[i].exists` is false, set cover) and a TopBar
     project switcher; the app starts on the home page.
  2. Inside a project pass `projectId` everywhere: `ImageQuery.projectId`, `getFilterCounts(null, projectId)`,
     `listBurstGroups(null, projectId)`, `listScenes(null, projectId)`, `detectScenes(null, projectId, options)`,
     `importFolder(path, options, projectId)` for "Add folder", `getEditPlan(projectId)`,
     `applyAllEditedScenes(projectId, …)`, step bar via `get/setWorkflowStep(projectId)`; Export step shows jobs
     whose `projectId` matches. The `null` scope arguments added to `App.tsx`, `FilterBar.tsx`, `useScenes.ts` by
     the architect are placeholders.
  3. Style library UI, Auto buttons, Copy dialog (`COPY_SETTINGS_GROUPS`), Previous / paste previous, plan view,
     style-model UI per `docs/ux-spec-8b.md`.
  Mock: two projects (`ceremony` = folder 1, `reception` = folder 2; `?projects=0` for none); `createProject`
  adds an empty project (existing path -> `existing: true`); `removeProject` drops its photos; `?autosync=1` gives
  the v14 default auto-sync on (off otherwise for the existing suites); style library, auto tone/WB, edit plan,
  style model (untrained) emulated; `set_scene_representative` / `apply_scene_edit` / `apply_all_edited_scenes`
  not emulated yet.

## v19.2 — 2026-10-05 (Phase 8d feedback: camera bodies, project-wide camera sync, suggestion filter, Auto Sync)

Driven by `docs/ux-review-8d.md` P1-1, P1-3, P1-4, P1-5 ([ARCH] items). Additive on the wire except one TS call
signature (`applySuggestions` has a third argument). Schema v18 (`migrations/0018_camera_serial_sync.sql`):
`images.camera_serial`, `images.camera_serial_read` (+ indexes `idx_images_folder_body`, partial
`idx_images_serial_unread`); `edit_batches.kind` CHECK accepts `sync` (writable_schema edit, as 0009 / 0017).
`src/ipc/bindings.ts` regenerated.

Camera bodies (P1-4)
- `CameraInfo.serial: string | null` (`#[serde(default)]`, optional in TS, always sent): EXIF `BodySerialNumber`
  (else DNG `CameraSerialNumber`), trimmed; all-zero / blank values read as `null`. Written by thumbnail extraction
  (`raw::meta::ImageMeta.serial`, `repo::record_extraction`: new imports and `regenerate_thumbnails`). Photos
  imported before v19.2 are read by a background thread at startup (`db::camera_serial::backfill`, ~1-3 ms per
  file, never blocks; missing originals are retried next launch). Until then their serial is `null`: refetch
  entries (or the facet) when opening the capture-time dialog rather than caching serials for the session.
- `ImageMetadata.cameraSerial` = the catalog's serial, else read from the file.
- New `CameraBody {make, model | null, serial | null}`; `MetadataFilter.bodies?: CameraBody[]` (exact match; a
  `null` model / serial matches only unknown values; blank = unknown); `MetadataFilterOptions.bodies?:
  CameraBodyCount[]` (`{body, count}`; ignores `metadata.bodies`, like every facet ignores its own constraint;
  order: known models by make / model, then serial, unknown serial last, unknown model last). `cameras` /
  `CameraFilter` are unchanged (model level).

Project-wide sync cameras (P1-3, P1-4)
- `CaptureTimeEdit.sync_cameras` gains `scope?: CameraSyncScope` (`"selected"` default = v19 behaviour, moves
  `ids`; `"body"` = every photo of the target's **project** with the target's make + model + serial; `"model"` =
  make + model, any serial). With `body` / `model` the command ignores `ids` (pass `[]`) and the grid's filters,
  and fails with `invalid_argument` ("the reference photo is from the camera being moved; ...") when the reference
  is in that set. Result / undo unchanged (`changedIds`, `previous` -> `restore_capture_times`).
- Preview counts for the dialog: `list_image_ids({projectId, metadata: {bodies: [targetBody]}})` (or `cameras` for
  model scope) = exactly the photos that move; "including N hidden by the current filters" = that count minus the
  same query with the grid's filters.

Suggestion filter + apply per kind (P1-5)
- `PendingSuggestion = "reject" | "pick" | "rating"`; `ImageQuery.suggested?: PendingSuggestion | null`: analysed,
  unflagged, 0 stars, with that suggestion (`rating` = no flag suggested but stars) = the
  `CullSummary.suggested*Pending` rule. Honoured by `list_images`, `list_image_ids`, `get_metadata_filter_options`.
- `FilterCounts.suggestedReject / suggestedPick / suggestedRating` (`#[serde(default)]`): pending suggestions among
  the counted images (follow `keepersOnly` / `metadata` / `pickOrigin`).
- `apply_suggestions(ids, onlyUnset, kinds: SuggestionKinds | null)` — **new third argument** (`null` = all, the
  v18.1 behaviour). `SuggestionKinds {picks, rejects, stars}`: suggested pick flags with `picks`, rejects with
  `rejects`, a suggested "no flag" (clears a flag; only reachable with `onlyUnset = false`) only with both, the
  suggested stars with `stars`. `{picks: false, rejects: true, stars: false}` over the project flags exactly the
  `suggested: "reject"` photos and changes no rating. Counts for the dialog's checkboxes: `CullSummary`
  `suggestedRejectPending` / `suggestedPickPending` / `suggestedRatingPending` (unchanged).

Auto Sync / relative sync (P1-1)
- `sync_delta(sourceId, before, after, targetIds, options: SyncDeltaOptions | null) -> SyncDeltaResult`. Writes
  `after` to the source and the `before` -> `after` change to every target, as **one** undoable batch of new kind
  `EditBatchKind::Sync` (`"sync"`) that includes the source: `undo_edit_batch(result.batch.batchId)` reverts all
  photos (P1-1 item 5). Only groups that differ between `before` and `after` are touched (never `crop`, `masks`,
  `transform`; `SyncDeltaResult.fields` lists them). `SyncDeltaOptions {relative?: AdjustmentField[] (default
  ["exposure", "white_balance"]; anything else -> invalid_argument; [] = absolute copy like Lightroom), fields?:
  AdjustmentField[] | null (limit), label?: string | null (history label of every entry, default "Auto Sync")}`.
  Relative exposure: target + (after - before), clamped -5..5, 0.01 EV. Relative WB: temperature shifted in mireds,
  tint added, clamped, rounded to whole K / tint; `as_shot` sides resolved to the camera as-shot values (the
  command decodes those photos through the develop cache); a change *to* as-shot is copied; unresolvable as-shot ->
  absolute copy, listed in `absoluteWbIds`. `SyncDeltaResult {batch: EditBatchResult, fields, relativeFields,
  absoluteWbIds, history: AdjustmentHistory (the source's, as save_adjustments returns)}`. Duplicates / the source in
  `targetIds` are ignored; atomic; unknown id -> `not_found`. Reports a `paste_sync` activity for 2+ photos.
- With relative exposure / WB the matched-scene restriction of P1-1 item 4 is no longer needed: frames keep their
  per-frame differences.

Mock backend (`src/testing/mockBackend.ts`)
- Entries carry `camera.serial` (Sony 06258214, Fuji 61000657, Canon 032021001234); new `?twobodies=1`: every 3rd
  frame from a second ILCE-7M4 (serial 05119876) whose clock is 1 h ahead. `bodies` filter + facet, `suggested`
  filter, `FilterCounts.suggested*`, `apply_suggestions` kinds, sync-cameras scopes (project-wide), `sync_delta`
  (as-shot = 5200 K / +8, batch kind `sync`), `get_image_metadata.cameraSerial` from the entry.
  Contract check: `tests/ui/ipc-v19-2-mock.spec.ts`.

Who updates what
- architect (done): types, schema v18, `raw::meta` / `raw::tiff` serial tags, `repo` (entry serial, bodies
  filter / facet, suggested filter, filter counts, `apply_suggestions_kinds`, extraction), `db::camera_serial`
  (backfill, started in `lib.rs`), `db::capture_time::{camera_scope_ids, edit}` scopes, `develop::sync_delta` +
  command + registration, Rust tests, bindings, mock, `App.tsx` compile fix (`applySuggestions(t, onlyUnset, null)`).
- frontend-dev:
  1. P1-1: Auto Sync switch (`autoSync` keymap Cmd+Alt+Shift+A). While on with 2+ selected, commit each edit with
     `commands.syncDelta(activeId, beforeCommit, afterCommit, otherSelectedIds, {label})` **instead of**
     `saveAdjustments` (the source is written by it; use `result.history` like the save result). Cmd+Z right after:
     `undoEditBatch(result.batch.batchId)`. Drop the matched-scene exclusion of exposure / WB (relative by default).
  2. P1-3 / P1-4: group cameras by make + model + serial (`entry.camera.serial`, label `ILCE-7M4 (…8214)` when two
     bodies share a model; `MetadataFilterOptions.bodies` has the project's bodies with counts); Sync tab sends
     `{kind: "sync_cameras", referenceId, targetId, scope: "body"}` with `ids: []` (or `"model"` when serials are
     unknown / the user picks the model; `"selected"` for "The selected photos"). Summary counts via
     `listImageIds({projectId, metadata: {bodies: [body]}})`. Metadata filter Camera column: use `bodies`.
  3. P1-5: `Review N suggested rejects` = grid query `suggested: "reject"`; `isFiltered` / `describeFilters` /
     chips / `membershipSensitive` must know `suggested`; Apply dialog checkboxes -> `applySuggestions(ids, true,
     {picks, rejects, stars})`, counts from `CullSummary` (or `FilterCounts.suggested*` for a filtered scope).
- rust-engine-dev: nothing required; `raw::exif_info` keeps its own serial read for the Metadata panel fallback.
  New extraction paths must keep filling `ImageMeta.serial`.
- vision-ml-dev: nothing required.

## v19 — 2026-10-05 (Phase 8d: capture time, metadata panel, Transform / Upright, preset tracking, paste batches, reject strictness)

Schema v17 (`migrations/0017_capture_transform.sql`): `images.exif_captured_at_ms` (backfilled from
`captured_at_ms`), `images.capture_time_source` (`exif` | `sidecar` | `user`, default `exif`);
`adjustments.applied_preset_id` (FK presets, `ON DELETE SET NULL`) + `adjustments.applied_preset_json`;
`projects.reject_strictness` (`conservative` | `balanced` | `aggressive`, default `balanced`); `edit_batches.kind`
CHECK accepts `paste` (writable_schema edit, as 0009).

Capture time (a)
- `CaptureMeta.capturedAtMs` is now the **corrected** time (what sort, bursts, scenes, metadata filters, export
  naming and project ranges use; no query changed). New `CaptureMeta.originalCapturedAtMs` (the file's EXIF time,
  never edited) and `CaptureMeta.captureTimeSource: CaptureTimeSource` (`exif` | `sidecar` | `user`); both
  `#[serde(default)]` (optional in TS, always sent).
- `edit_capture_time(ids, mode: CaptureTimeEdit) -> CaptureTimeEditResult {changedIds, skippedIds, offsetMs,
  previous: CaptureTimeSnapshot[]}`. Modes (tagged `kind`): `shift {offsetMs}`, `set_exact {referenceId,
  capturedAtMs}` (reference must be in `ids`; others shift by the same offset; a reference without a time just
  gets it), `sync_cameras {referenceId, targetId}` (offset = reference - target, applied to `ids`; both need a
  time), `revert` (back to the EXIF time). Photos without a time are skipped. Atomic; duplicate id / out of
  1900..2200 (`MIN_CAPTURE_TIME_MS` / `MAX_CAPTURE_TIME_MS`) -> `invalid_argument`. Marks sidecars dirty,
  notifies auto-sync, refreshes scene bounds, kicks a rescore (bursts regroup).
- `restore_capture_times(snapshots: CaptureTimeSnapshot[]) -> number[]` (undo / redo; changed ids).
- Extraction (`repo::record_extraction`) writes `exif_captured_at_ms` and touches `captured_at_ms` only while the
  source is `exif`, so re-extraction keeps corrections.
- Helper for the XMP read path: `db::capture_time::apply_sidecar_time(conn, id, Option<naive ms>)` (equal to EXIF
  or `None` -> `exif`; different -> `sidecar`; does not mark dirty).

Per-photo metadata (b)
- `get_image_metadata(id) -> ImageMetadata {imageId, path, fileName, folderPath, format, extension, fileSize,
  fileMtimeMs, capturedAtMs, originalCapturedAtMs, captureTimeSource, camera, lens, iso, shutterSeconds, aperture,
  focalLengthMm, focalLength35mm, exposureCompensationEv, flashFired, cameraSerial, width, height, orientation,
  gps: GpsLocation {latitude, longitude, altitudeM} | null, sidecarPath, sidecarExists, companionPath, missing}`.
  Catalog values are filled now; `focalLength35mm`, `exposureCompensationEv`, `flashFired`, `cameraSerial`, `gps`
  are `null` until rust-engine-dev reads them from the file.

Transform / Upright (c)
- `ParametricAdjustments.transform: TransformSettings` (`#[serde(default)]`) = `{upright: UprightMode, guides:
  UprightGuide[] (<= 4, sensor frame), vertical, horizontal (-100..100), rotate (-10..10 deg), aspect (-100..100),
  scale (50..150, default 100), offsetX, offsetY (-100..100), constrainCrop, solution: UprightSolution | null}`.
  `UprightMode` = `off | auto | level | vertical | full | guided` (`crs:PerspectiveUpright` 0 / 1 / 3 / 4 / 2 / 5:
  `crs_value` / `from_crs`). `UprightSolution {mode, matrix: number[9] (row-major homography, corrected ->
  source, sensor frame normalized), rotationDeg, crs: CrsProperty[] (Lightroom's Upright* values verbatim)}`.
  `crs:` mapping table on `TransformSettings` (rust doc) and in architecture.md. Default is neutral, so `hasEdits`
  / `neutral` are unchanged for existing rows.
- `AdjustmentField::Transform` (`"transform"`): in `ALL`, `PASTE_PREVIOUS` and the Copy Settings "Transform" item
  (now supported); not in `DEFAULT_SYNC` (per-frame geometry, like crop). `lerp`: the nearer side's.
- `auto_upright(id, mode, adjustments | null) -> UprightResult {mode, solution | null, message | null}` (nothing
  saved). **Stub**: no solution, message "Upright is not available yet".
- TS mirrors updated (`completeAdjustments`, `ADJUSTMENT_FIELD_SET` / labels, `copyAdjustmentFields`,
  `DEFAULT_SYNC_FIELDS`, `lerpAdjustments`).

Applied preset + preview variants (d)
- `AdjustmentHistory.appliedPresetId: number | null` (`#[serde(default)]`): set by `apply_preset`, reported while
  every group in the preset's `fields` still equals what the apply produced (any owned change, undo past it, or
  another preset clears it; redo restores it; deleting the preset clears it). Returned by `save_adjustments`,
  `get_history`, undo / redo / goto.
- `render_preview_variant(id, adjustments, variant: PreviewVariant, options) -> RenderedPreview | null`, variant
  `preset {presetId}` (= `resolve_preset` on top of the live settings) or `without_fields {fields}` (those groups
  back at the format defaults). No save, no history. Same latest-wins / `sieve://` path as `render_preview`.
- `RenderSlot::Preview` (`"preview"`): use it for these renders so the `main` render stays (hover-out /
  release = show the last `main` URL). TS `Record<RenderSlot, ...>` literals need a `preview` key (fixed in
  `useEditor.ts`).

Batch edits (e)
- `paste_settings`, `sync_settings`, `paste_previous` now return `EditBatchResult` (was `null`) and record one
  undoable batch of new kind `EditBatchKind::Paste` (`"paste"`); duplicates in the id list are ignored (were
  applied twice); `batchId = null` when nothing changed; `undo_edit_batch(batchId)` takes the whole paste back
  (linear-undo rules as for scene applies). History labels / `EditSource::Pasted` unchanged.

Reject strictness (f)
- `RejectStrictness` (`conservative | balanced | aggressive`, default `balanced`); `Project.rejectStrictness`;
  `set_project_reject_strictness(projectId, strictness) -> null` (kicks a rescore; unknown -> `not_found`).
  `db::projects::reject_strictness_of_image(conn, id)` for the scorer.

Mock backend (`src/testing/mockBackend.ts`)
- All new commands; capture times carry `originalCapturedAtMs` / `captureTimeSource`; `?twocams=1` makes every 3rd
  frame a Canon EOS R5 (`IMG_xxxxx.CR3`) whose clock is 1 h ahead (sync-cameras fixture); `get_image_metadata`
  gives GPS on every 5th frame; `auto_upright` returns a small rotation homography (`?upright=none` = no lines;
  Guided needs 2 guides); paste / sync / paste previous record `paste` batches; `appliedPresetId` tracked;
  projects carry `rejectStrictness`.

Who updates what
- architect (done): types, schema v17, commands + registration, `db::capture_time` (edit / restore / sidecar helper
  / catalog metadata), extraction keeps corrections, `history::applied_preset` + `record_applied_preset`
  (`styles::apply_preset` records), `styles::resolve_preview_variant`, `batches::apply_fields_recorded`, projects
  SQL, Rust tests, bindings, TS mirrors, mock, `useEditor.ts` compile fix.
- rust-engine-dev: XMP read: corrected time from `exif:DateTimeOriginal` (else `photoshop:DateCreated`; naive ms
  incl. fractional seconds, ignore the zone offset like EXIF) via `db::capture_time::apply_sidecar_time`; XMP write:
  when `capture_time_source != 'exif'` write `exif:DateTimeOriginal` + `photoshop:DateCreated`
  (`YYYY-MM-DDTHH:MM:SS.ss`), when `exif` remove only values Sieve wrote (never clobber Lightroom's). `crs:` read /
  write of `transform` per the mapping table (drop Perspective / Upright from `crs::unsupported_warnings` and from
  the preserved-only list; keep `solution.crs` verbatim while the mode is unchanged; imported presets' Perspective
  keys -> `AdjustmentField::Transform` in `styles/preset_file.rs`). Render + export: apply `solution` (when
  `solution.mode == upright`) and the manual sliders before the crop, `constrainCrop`. Fill the file-read fields of
  `get_image_metadata`. Upright solver (homography from detected lines) for `auto_upright`.
- vision-ml-dev: line detection for `auto_upright` (Level / Vertical / Full / Auto / Guided); reject strictness in
  the scorer (`projects::reject_strictness_of_image`, memoize like `ShootTypes` in `ml/worker.rs`).
- frontend-dev: Metadata panel (`getImageMetadata`), Edit Capture Time dialog (`editCaptureTime` + undo via
  `restoreCaptureTimes(result.previous)`; refetch `changedIds`); Transform panel (`transform`, `autoUpright`); preset
  highlight from `AdjustmentHistory.appliedPresetId`; hover preview / press-and-hold via `renderPreviewVariant` in
  slot `preview`; paste / sync results (`EditBatchResult`) into the batch undo UI; reject strictness control
  (`Project.rejectStrictness`, `setProjectRejectStrictness`).

## v18.1 — 2026-10-03 (UX review 8c P1-1, P1-6)

Suggestions pending = what Apply changes (P1-1)
- `CullSummary.suggestedRejectPending` / `suggestedPickPending` now count only **untouched** photos: analysed,
  `pick = unflagged` **and** `rating = 0` (was: unflagged with any stars). New **required**
  `CullSummary.suggestedRatingPending`: untouched photos whose suggestion is no flag but `suggestedRating > 0`.
  `apply_suggestions(every image in scope, onlyUnset = true)` changes exactly the sum of the three
  (`repo::tests::suggestions_pending_equals_default_apply`).
- `apply_suggestions` skips images that already match their suggestion (same flag and stars): they count as
  `skipped`, not `applied`, and their rows are not rewritten (no spurious XMP dirty). TS signature unchanged.
- Frontend (done here): `CullSummaryBar` copy `Suggestions: 20 picks · 8 rejects · 3 star-rated — Apply…`
  (zero parts dropped, hidden when nothing is pending); `ApplySuggestionsDialog` counts a photo already matching
  its suggestion as "left as they are", so its default count equals the bar. Mock mirrors both.

Flag origin filter (P1-6)
- `ImageQuery.pickOrigin?: PickOrigin | null` (`null` / missing = anyone). `user` = flagged (pick or reject) with
  origin `user` (or a pre-v18 flag), `auto` = flagged by `apply_suggestions` and unchanged since. Unflagged images
  never match; combined with `picks` (AND), so `{picks: ["reject"], pickOrigin: "auto"}` returns
  `CullSummary.rejectedAuto` images. Honoured by `list_images`, `list_image_ids`, `get_metadata_filter_options`.
- `get_filter_counts(folderId, projectId, keepersOnly, metadata, pickOrigin)` — **new fifth argument** (`null` =
  anyone): facet counts over images flagged by that origin only.
- Frontend (done here): the summary's "N by you" / "M auto" toggle `{picks: ["reject"], pickOrigin}` with an
  active state; filter-bar chip `Auto-rejected ×` / `Rejected by you ×` (`filter-origin`, clears the origin);
  `isFiltered` / `describeFilters` / `useFilterCounts` / `membershipSensitive` know the field. Summary bar is one
  row (`flex-nowrap`, formula truncates with the full text in its title; UX P2 #10).
- Other agents: nothing required. Code that builds `ImageQuery` objects by hand keeps working (optional field);
  direct callers of `commands.getFilterCounts` must pass the fifth argument (only `FilterBar.useFilterCounts`).

## v18 — 2026-10-03 (Phase 8c: culling clarity, metadata filters, background activity)

Keeper rule (a)
- `KeeperRule.mode: KeeperMode` (`"not_rejected"` | `"picks_and_ratings"`), **required** on the wire.
  `DEFAULT_KEEPER_RULE` = `{mode: "not_rejected", minRating: 1, useSuggestions: true}` (user decision 2026-10-03:
  keep everything not rejected). `picks_and_ratings` = the pre-v18 rule; `minRating` / `useSuggestions` only apply
  there but are kept (and validated 1..=5) in both modes.
- Mirrors updated: `KeeperRule::is_keeper_values`, `repo::keeper_predicate`, `PROJECT_SQL` (project counts),
  `isKeeperValues` / `isKeeper` in `src/ipc/index.ts`.
- Schema v16 migrates the stored rule: the old default `{minRating:1,useSuggestions:true}` (written into every
  catalog by migration 0012) -> `not_rejected`; any other stored rule -> `picks_and_ratings` with its thresholds.

Cull summary (b)
- `get_cull_summary(projectId | null) -> CullSummary {total, picked, pickedAuto, unflagged, rejected,
  rejectedByUser, rejectedAuto, starred, keepers, keeperBreakdown: {picked, unflagged, starred, suggested},
  keeperRule, suggestedRejectPending, suggestedPickPending, unanalyzed}`. `keepers` equals a `keepersOnly` query
  over the same scope; the breakdown adds up to it (`not_rejected`: picked + unflagged; `picks_and_ratings`:
  picked + starred + suggested). Unknown project -> `not_found`.

Pick origin + suggestion reasons (c)
- `PickOrigin` (`"user"` | `"auto"`); `RawImageEntry.pickOrigin: PickOrigin | null` (null while unflagged);
  `CullSnapshot.pickOrigin?: PickOrigin | null` (optional; missing restores as `user`). Schema v16
  `images.pick_origin`: `apply_suggestions` -> `auto` (only when it changes the flag), `set_pick` / sidecar reads
  / restore without origin -> `user`. Flags from before v18 read as `user`.
- `QualityScore.reasons: SuggestionReason[]` (`{kind: SuggestionReasonKind, text, relatedImageId?}`; kinds
  `blink | missed_focus | motion_blur | creative_blur | underexposed | overexposed | duplicate_burst | low_score |
  other`). Schema v16 `quality_scores.reasons_json` (default `[]`), written by `ml::store::write_scored`. Empty until
  vision-ml-dev fills them.
- `XmpSyncState.hasSidecar: boolean` (a sidecar existed at the last write / read).

Metadata filters (d)
- `ImageQuery.metadata?: MetadataFilter` (every field optional): `formats: ImageFormat[]`, `extensions: string[]`
  (no dot, case-insensitive, `[A-Za-z0-9]{1,10}`), `cameras: CameraFilter[]` (`{make, model | null}`),
  `lenses: (string | null)[]` (null = unknown), `iso / focalLengthMm / aperture / shutterSeconds: NumberRange`
  (`{min?, max?}` inclusive, 1e-6 relative slack; focal length / aperture compared at 0.1), `captured: DateRange`
  (`{fromMs?, toMs?}`, naive ms, to exclusive), `edited: boolean`, `hasSidecar: boolean`. Invalid values ->
  `invalid_argument`.
- `get_filter_counts(folderId, projectId, keepersOnly, metadata)` — **new fourth argument** (`null` = none).
- `get_metadata_filter_options(query: ImageQuery) -> MetadataFilterOptions {total, formats, extensions, cameras,
  lenses, isos, focalLengths, apertures, shutterSpeeds, captureDays, edited, hasSidecar}`: distinct values with
  counts; each facet ignores its own constraint (Lightroom cascading columns); unknown values counted as `null`
  entries (last).
- Schema v16 indexes `idx_images_folder_camera`, `idx_images_folder_lens`.

Background activity (e)
- Event `activityEvent {id, kind: ActivityKind, label, done, total | null, state: ActivityState, message | null}`;
  kinds `import | analysis | xmp_save | paste_sync | apply_scene | export | model_download | other`; states
  `running | finished | error | cancelled`. Per activity: running events (start always, then <= 10/s), then one
  terminal event. Helper `ipc::activity::Activities` (managed state; `activities(&app)`): channel API
  `progress(channel, kind, label, done, total)` / `finish(channel, state, message)`, handle API
  `start(kind, label, total) -> ActivityHandle` (`progress`, `finish`, `fail`, `cancel`; dropped = `error`).
- Wired: ingest (`"import"`), analysis (`"analysis"`, until `analysisFinished`), export (per job), auto-sync passes
  and explicit `write_xmp` / `write_xmp_all_dirty` (`xmp_save`, message "Saved metadata for N photos; M sidecars
  could not be written"), `paste_settings` / `sync_settings` on 2+ photos (`paste_sync`, indeterminate),
  `apply_scene_edit` / `apply_all_edited_scenes` (`apply_scene`, indeterminate), `download_models`
  (`model_download`, KiB).

Sidecar refresh (added for rust-engine-dev's "XMP both ways")
- `refresh_sidecars(projectId | null) -> number[]`: re-reads sidecars whose mtime changed since Sieve last wrote /
  read them (another app edited them) for the project's folders (`null` = all folders), via
  `XmpSync::refresh_folders`; images with unsaved catalog changes are skipped (auto-sync's newer-wins handles
  them). Returns the changed image ids (refetch with `get_images`); forgets their develop sources. Unknown project
  -> `not_found`. Frontend: call on project open and on window focus (debounced). Mock returns `[]`.

Who updates what
- architect (done): types, schema v16, commands + registration, `Activities` managed state + wiring above, SQL
  (filters, facets, summary, keeper predicate), pick-origin writers (`repo`, `xmp::store::apply_read`), Rust tests,
  bindings, TS `isKeeperValues`, compile fixes (`FilterBar.tsx` 4th arg, `PlanView.tsx` rules carry `mode` and a
  fifth entry "Everything not rejected" appended so menu indices 0..3 are unchanged, `useWorkflow.setKeeperRule`
  takes `KeeperRule`), mock backend (summary, facets, metadata filtering + counts, pick origin, reasons per tag,
  `hasSidecar`, activity events for write_xmp / save all / `__mockXmpFlush` / export, `window.__mockActivity(e)`;
  `?meta=1` varied file types / cameras / lenses; `?keepers=not_rejected` = v18 default rule — the mock keeps the
  pre-v18 rule by default so existing suites are unchanged).
- frontend-dev: `refreshSidecars(projectId)` on project open + window focus (debounced; refetch the returned ids); cull summary readout (`getCullSummary`, refetch after culling writes / `analysisFinished`); keeper
  rule menu (new default, `mode`); Rejected view (`pickOrigin`, `quality.reasons`); metadata filter row
  (`getMetadataFilterOptions(query)` + `ImageQuery.metadata` + `getFilterCounts(.., metadata)`); corner indicator
  from `events.activityEvent`; `useCullUndo.snapOf` should include `pickOrigin: e.pickOrigin` so undo restores
  auto flags exactly.
- vision-ml-dev: fill `QualityScore.reasons` in `ml::scoring` (tags / low score) and `ml::bursts` capping
  (`duplicate_burst` with `relatedImageId` = keeper); persisted automatically by `write_scored`.
- rust-engine-dev: XMP flag rewrite (xmpDM) must keep `pick_origin = 'user'` when a sidecar read changes the flag
  (`xmp::store::apply_read`); report new long-running work through `ipc::activity`.

## v17 (no contract change) — 2026-09-30 (Auto tone: on-demand faces on unanalysed photos)
No type, command signature, event or schema change; `src/ipc/bindings.ts` unchanged (`cargo test bindings` passes).
- Behaviour: `auto_tone` on a photo without analysis results now detects faces itself (`ml::auto_faces`, SCRFD on the neutral render, cached per image) instead of relying only on the skin-colour estimate, so skin is protected as on analysed photos; results for such photos change (usually Whites / Highlights a little lower on portraits). First call after launch loads the detector (~0.2 s); then about +30 ms per photo, 0 ms once cached. Without the model file the previous bounded estimate is used.
- Style model: the Auto anchor of unanalysed frames uses the same path; their cached `style_features` are recomputed once (`facesOnDemand`, serde default `false`).
- `DevelopCache::with_auto_faces` / `auto_faces()`, `develop::auto::resolve_faces`, `ml::models::FaceDetector` are new Rust APIs; `lib.rs` wires the detector (culling models dir).
- frontend-dev / vision-ml-dev: nothing required. rust-engine-dev: any new caller of `auto_tone_with_faces` with `faces = None` should go through `develop::auto::resolve_faces` first.

## v17 — 2026-09-30 (UX re-check 2 P1-12: reset scenes, lenient Apply all, linear undo across Auto edit -> Apply)
Schema v15 (`migrations/0015_apply_bases.sql`). Driven by `docs/ux-review-8b.md` "Re-check 2" P1-12. **Breaking** for
exhaustive `switch`es / `Record`s over `SceneEditStatus` (new `reset`) and for TS object literals typed as
`EditPlanCounts` / `ApplyScenesResult` (new fields; only the mock builds them). `src/ipc/bindings.ts` regenerated.
No frontend compile fix was needed (`useWorkflow`'s status ternary falls through to `edited` / `auto` for `reset`,
which is wrong UI but compiles; see frontend-dev 1).

Reset scenes (P1-12 fix 1)
- New `SceneEditStatus` `"reset"`: the scene was applied (`appliedAtMs` set) but its representative has no edits any
  more (reset in Develop, per-image undo of its only edit, or its auto edit undone). Before v17 such a scene was
  `outdated` with `edited: false`, so Re-apply / Apply all failed with "The representative has no edits yet".
  - The members keep the last apply's look (`appliedIds` unchanged); `appliedBatch` stays as is (undoable unless a
    later edit blocks it), so "Undo apply" restores the members. Undoing it makes the scene `to_edit`.
  - Editing the representative again: `outdated` (re-apply), or `applied` if it lands on the applied settings.
  - `unappliedKeeperIds` is always empty for `reset` scenes (nothing can be applied to them yet).
  - Not applied by `apply_all_edited_scenes` (it also requires `edited` now).
- `EditPlanCounts.reset` (new): `reset` scenes not skipped. **`EditPlanCounts.toEdit` now counts `to_edit` and
  `reset` scenes** (both are "to do"), so the To do tab / header / progress need no extra sum; status counts no longer
  partition the scenes (`toEdit + edited + applied + outdated` = not-skipped scenes; `reset` is a subset of `toEdit`).
- `isEditPlanDone` is unchanged (a `reset` scene is not `applied`, so the plan is not done).

Apply all skips what it cannot apply; errors name the scene (P1-12 fix 2)
- `ApplyScenesResult.skippedScenes: SkippedScene[]` (new; plan order): scenes `apply_all_edited_scenes` was going to
  apply but could not. Nothing was written to them; every other scene was applied (one batch, as before).
  `SkippedScene {sceneId, reason: SceneSkipReason, message}`; `SceneSkipReason = "not_edited" | "no_keepers" |
  "failed"`. `failed` = reading or matching the scene failed with a per-scene error (`not_found`, `invalid_argument`,
  `io`, `file_missing`, `decode_failed`, e.g. its representative's original is missing); catalog-level errors
  (`database`, `internal`, `disk_full`, `read_only`, `catalog_read_only`) still fail the whole call. With the
  candidate filter fixed, `not_edited` / `no_keepers` are defensive; `failed` is the realistic case.
- `apply_scene_edit` keeps failing (never `skippedScenes`), and every per-scene error message now starts with the
  scene's plan number, the same number the Edit step shows (`EditPlan.scenes` index + 1; "This scene" when it is
  not in a plan):
  - representative never edited: `invalid_argument` "Scene 2: edit its representative first, then apply."
  - `reset` scene: `invalid_argument` "Scene 1: its representative was reset after the last apply. Edit it first,
    then apply."
  - matching errors keep their kind: e.g. `file_missing` "Scene 3: DSC01234.ARW is missing ...".

Linear undo across Auto edit -> Apply (P1-12 fix 3, the UX recommendation; decision in `docs/decisions.md`)
- A scene apply made from a representative whose current settings were written by an edit batch (in practice
  "Auto edit (my style)") records that batch as its base (`edit_batch_bases`). While the apply is not undone, the
  base batch counts that representative as a conflict: `EditBatchInfo.conflictCount` includes it, `undoable` is
  false, and `undo_edit_batch(base)` fails with `conflict`:
  - only applies block it: "A scene was applied from this edit since; undo that apply first" (n > 1: "n scenes were
    applied from this edit since; undo those applies first");
  - photos were also edited after it: the v16 message, counting both ("Later edits on n photos; undo those first").
- Undo the apply first (it is the newer batch, `EditPlan.latestBatch`), then the auto edit, as in Lightroom. The exact
  P1-11 repro (Auto edit 3 scenes -> Apply Scene 1 -> Undo on the older "Auto edited 3 scenes" toast) now gets the
  `conflict` instead of silently putting Scene 1 into the inconsistent state. Applies made before v17 record no base
  and do not block (no backfill). An apply that changed nothing in the scene records no base; an apply from a
  hand-graded representative has none.

Who updates what
- architect (done): types, schema v15, `develop::batches::{BatchBase, commit_recorded_with_bases, base_of,
  dependent_ids, edited_after_ids, applied_from_message}` + `conflict_ids` / `undo` / `batch_info` counting
  dependents, `scene::workflow::{scene_label, named, skippable, apply_all_inputs, match_jobs, MatchedJobs}` +
  `reset` status / counts / `edited_scenes`, `commands::apply_scenes` (lenient for apply all, matching moved to the
  testable `workflow::match_jobs`), Rust tests (both P1-12 repros, the apply-all skip / failure / cancel paths, the
  message variants), bindings, mock backend (everything above; test hook `window.__mockApplyFailScenes = [sceneId]`
  makes that scene's matching fail with `file_missing`), `tests/ui/ipc-v17-mock.spec.ts` (invoke-level contract
  check of the mock), and `tests/ui/recheck-fixes.spec.ts` "toast and row Undo follow the backend's undoable flag"
  updated: after Auto edit -> Apply Scene 1 the auto edit's toast Undo now retires (it asserted the v16 behaviour).
- frontend-dev (`useWorkflow` `SceneUi`, `PlanView`, `EditContextBar`, `bits.tsx`; spec in the review's P1-12):
  1. `ui = "reset"` when `entry.status === "reset"` (replaces the review's `outdated && !edited` test). Row: amber
     `RefreshCw`, status line `Representative reset · N photos keep the earlier look` (N = `appliedIds.length`),
     primary `Edit ▸`, secondary `Undo apply` (`plan-undo-inline-<id>`) while `appliedBatch?.undoable`, no Re-apply.
     It sits under the To do tab; tab / header / progress counts come from `counts.toEdit` (already includes reset).
  2. `Apply N edited scenes` must not count `reset` scenes: use `counts.edited + counts.outdated` (+ applied scenes
     with `unappliedKeeperIds`), or the client filter `!skipped && edited && (edited|outdated|applied with new
     keepers)` = `workflow::edited_scenes`.
  3. Develop context bar on a `reset` scene's representative: chip `Reset since applied · representative`,
     `Apply to scene` disabled with the visible hint `Edit this photo first`.
  4. After `applyAllEditedScenes`, when `result.skippedScenes` is non-empty: keep the success toast for the applied
     scenes and add an info toast listing each `message` (e.g. "Scene 2: DSC00056.ARW is missing"), with a
     `Show` / jump to the first skipped scene's row. Errors from `applySceneEdit` already name the scene: show the
     message as is.
  5. Conflict handling: the auto edit's toast Undo / Plan Cmd+Z now also retire after an apply built on it
     (`getEditBatches` / `latestBatch` say `undoable: false`); a `conflict` from `undoEditBatch` with the new message
     goes to the existing info toast.
- rust-engine-dev / vision-ml-dev: nothing required. Other multi-image write paths built from one image's settings
  may pass `BatchBase`s to `commit_recorded_with_bases` to join the linear-undo chain.

## v16 — 2026-09-30 (UX re-check P1-11: linear batch undo, persisted batch undo state)
Schema v14 (`migrations/0014_linear_undo.sql`). Driven by `docs/ux-review-8b.md` "Re-check" P1-11 and P2 #13 (batch
undo lost on Home -> back). **Breaking** for TS object literals typed as `SceneEditEntry` / `EditPlan` (new fields;
only the mock builds them) and for exhaustive `switch`es over `ErrorKind` (new `conflict`). `src/ipc/bindings.ts`
regenerated.

Linear batch undo (P1-11)
- New `ErrorKind` `"conflict"`: the operation would undo something a later edit was built on; nothing changed.
- `undo_edit_batch(batchId)` now fails with `conflict` ("Later edits on n photos; undo those first" / "... on 1
  photo; ...") when any photo of the batch has a history entry newer than the batch's own entry for it: a later
  batch (apply, auto edit) or a manual edit (sliders, paste, preset, reset, Read from XMP). Exceptions that do not
  block: undoing the later batch first (the restored entry is stamped with this batch, so it becomes undoable
  again), a per-image undo (Cmd+Z in Develop) of the later edit, and a per-image undo of the batch's own edit
  (that photo is then left alone and reported in `skippedIds`, the field's only meaning since v16).
- Undoing a batch also clears the applied state of every scene whose last apply was that batch: `status`
  `applied`/`outdated` -> `edited` (or `to_edit`), `appliedAtMs` / `appliedBatch` -> null, `appliedIds` empty
  (members are back on their previous settings). Before v16 the real backend left such scenes `applied` (only the
  mock cleared them); migration 0014 repairs catalogs that undid an apply before v16.
- Verified on the real backend (Rust tests): a scene reports `applied` only while the representative's current
  settings equal those it was applied from; a later rep edit -> `outdated` (re-apply needed), a per-image undo
  back to those settings -> `applied`, a per-image undo past them -> `outdated`. The mock now compares settings
  the same way (it used timestamps).

Persisted batch undo state (P2 #13)
- `EditBatchKind = "scene_apply" | "style_prediction"`.
- `EditBatchInfo {batchId, label, kind, createdAtMs, undoneAtMs, imageCount, conflictCount, undoable}`;
  `undoable = undoneAtMs == null && conflictCount == 0` = `undo_edit_batch` would succeed now.
- `SceneEditEntry.appliedBatch: EditBatchInfo | null`: the batch of the scene's last apply that changed something
  (null when never applied or that batch was undone). Persisted: survives Home -> back and reloads.
- `EditPlan.latestBatch: EditBatchInfo | null`: the newest batch (any kind) not undone that changed a photo of the
  project.
- `get_edit_batches(batchIds) -> EditBatchInfo[]` (given order; unknown id -> `not_found`): refresh the
  undoability of the batches the UI holds (toasts, session stack).

Who updates what
- architect (done): types, error kind, schema v14, `develop::batches::{conflict_ids, batch_info, batch_infos,
  latest_batch_where}` + conflict check and scene clearing in `undo`, `scene::workflow` plan fields, command +
  registration, Rust tests, bindings, mock backend (conflict on out-of-order undo with the same message, `undoable`
  / `conflictCount`, `appliedBatch`, `latestBatch`, `get_edit_batches`, settings-equality scene status, undo
  clears only scenes whose last apply was the batch).
- frontend-dev:
  1. P1-11: show a toast's Undo (and the row ⋯ "Undo apply") only while its batch is `undoable`: after any edit
     commit / batch / undo, refresh with `getEditBatches(ids of the visible toasts)` (or use the refetched plan's
     `appliedBatch` / `latestBatch`); otherwise drop the button and keep the text. Handle a `conflict` error from
     `undoEditBatch` (e.g. a race) with an info toast showing its message, not an error.
  2. P2 #13: row ⋯ "Undo apply" = `undoEditBatch(scene.appliedBatch.batchId)`, enabled when
     `scene.appliedBatch?.undoable`; works after Home -> back. Optionally seed the session batch stack from
     `plan.latestBatch` (Cmd+Z may stay session-scoped).
  3. After `undoEditBatch` refetch the plan: undone applies now come back as `edited` from the real backend too.
- rust-engine-dev / vision-ml-dev: nothing required. New multi-image write paths must keep going through
  `develop::batches::commit_recorded` so their entries carry `batch_id` (linear undo relies on it).

## v15 — 2026-09-30 (Phase 8b feedback: persisted workflow state, skipped / minor scenes, apply options)
Schema v13 (`migrations/0013_workflow_state.sql`). Driven by `docs/ux-review-8b.md` P1-2..P1-5 and the triage's P2
backend items. **Breaking** for TS callers in two places: `get_filter_counts` has a third argument
(`keepersOnly: boolean | null`), and object literals typed as `SceneEditEntry` / `EditPlan` /
`SceneApplyOutcome` / `ApplyScenesResult` need the new fields (only the mock builds them). Everything else is
additive. `src/ipc/bindings.ts` regenerated.

Per-photo workflow state (P1-2)
- `EditSource = "none" | "user" | "pasted" | "auto_style" | "scene_apply" | "sidecar"`.
- `ImageEditState {imageId, editSource, batchId, appliedSceneId, needsReview, reviewReason}`.
  Derived from the history entry the image's cursor points at (`adjustment_history.source` / `batch_id`, new in
  schema v13), so it is persisted, survives reloads / project switches, and follows per-image undo / redo.
  `needsReview` is set by an apply for targets whose match did not converge (`SceneApplyOutcome.notConvergedIds`;
  `reviewReason` = the match notes, else "Exposure or white balance did not fully match the representative") and
  clears as soon as the image is edited in any way (its settings no longer come from that apply), or with
  `mark_reviewed`. A per-image undo back to the applied settings shows it again (unless marked reviewed).
  `undo_edit_batch` restores the provenance the frames had before the batch.
- `get_edit_states(imageIds) -> ImageEditState[]` (any image, e.g. the Develop chip / filmstrip outside the plan);
  `mark_reviewed(imageIds) -> ImageId[]` ("Looks good": clears needs-a-look without touching settings; returns
  the ids that were cleared).
- `EditPlan` += `editStates` (one per keeper, `keeperIds` order), `needsReviewIds` (keepers, capture order).
- `SceneEditEntry` += `appliedIds` (members whose current settings came from an apply; non-keepers included when
  applied to), `needsReviewIds` (keepers), `autoEdited` (the representative's current settings came from the
  style model).

Coverage of applies (P1-3) and the plan `outdated` flag
- `SceneEditEntry.unappliedKeeperIds`: for an applied, not-skipped scene, keepers the last apply did not cover
  (became keepers / moved into the scene after it), not the representative, and with no edit of their own
  (`editSource` `none` or `auto_style`). "Apply to N new" = the existing `apply_scene_edit` (frames applied
  before come out unchanged and get no new history entry). Applies before v15 have no recorded coverage: their
  unapplied keepers are the non-representative keepers that still have no edit.
- `EditPlan.outdated`: `unassignedKeeperIds` non-empty or any not-skipped scene has `unappliedKeeperIds`.
  "All done" = `isEditPlanDone(plan)` (new TS helper in `src/ipc/index.ts`): scenes non-empty, `!outdated`, every
  scene applied or skipped.
- `apply_all_edited_scenes` now also applies `applied` scenes with `unappliedKeeperIds`, and never skipped scenes.

Skipped and minor scenes (P1-4)
- `set_scene_skipped(sceneId, skipped) -> SceneEditEntry` (no settings change; idempotent; unknown scene ->
  `not_found`, no keepers -> `invalid_argument`). `SceneEditEntry.skipped`; the status is kept (read `skipped`
  first). Applying a skipped scene includes it again. The flag (and the coverage) follow the representative
  through re-detection like the rest of the plan state.
- `SceneEditEntry.minor` = at most `MINOR_SCENE_MAX_KEEPERS` (= 2, exported constant) keepers.
- `EditPlan.counts: EditPlanCounts {scenes, toEdit, edited, applied, outdated, skipped, minor, needsReview,
  unappliedKeepers, unassignedKeepers}`; status counts exclude skipped scenes.

Apply options (P1-5) and cancel (P2-4)
- `SceneApplyOptions.excludeIds?: ImageId[]` (optional on the wire; `DEFAULT_SCENE_APPLY_OPTIONS.excludeIds = []`):
  frames left alone, reported in the new `SceneApplyOutcome.excludedIds`, and counted as covered (they do not
  become `unappliedKeeperIds`). "Apply with options…" should now call `apply_scene_edit(sceneId, {matchOptions:
  panel options, excludeIds: unchecked frames, ...})` instead of `match_scene` + `apply_scene_match`, so it is a
  batch (plan status, needs-a-look, one-step undo via `undo_edit_batch`). `match_scene` stays the preview path.
- `cancel_scene_apply()`: stops the running `apply_scene_edit` / `apply_all_edited_scenes` between steps of 32
  targets; scenes whose matching finished are committed as the call's batch, the rest are untouched; the call
  resolves with `ApplyScenesResult.cancelled = true` (new field; `scenes` = the committed scenes only).

Server-side keepers filter (P2-5) and XMP failures (P2-6)
- `ImageQuery.keepersOnly?: boolean` (optional, default false): keepers under the catalog's `KeeperRule` (SQL
  mirror of `KeeperRule::is_keeper_values`).
- `get_filter_counts(folderId, projectId, keepersOnly)` — **new third argument** (`null` = false): every facet
  counted over keepers only, for the Edit / Export steps ("15 of 42 keepers").
- `list_xmp_failures(projectId | null) -> XmpFailure[]` (images whose last sidecar write/read failed, capture
  order; replaces paging the catalog in `openXmpErrors`). "Show" = grid query by ids (`get_images`).

Schema v13
- `adjustment_history.source` (CHECK user/pasted/auto_style/scene_apply/sidecar; backfilled from labels: Original
  and "Read from XMP" -> sidecar, "Apply to Scene"/"Match Scene" -> scene_apply, "Auto Edit (My Style)" ->
  auto_style, Paste/Sync/Paste from Previous -> pasted, else user), `adjustment_history.batch_id` (backfilled from
  matching `edit_batch_items`), `edit_batch_items.before_source / before_batch_id / review_reason / reviewed_at`,
  `scenes.skipped`, `scenes.applied_covered_json`.

Who updates what
- architect (done): types, schema v13, commands + registration (`SceneApplyControl` managed state), bodies
  (`develop::history::source_for_label`, `develop::batches::{edit_states, edit_states_where, mark_reviewed}` +
  provenance in `commit_recorded` / `undo`, `scene::workflow` plan fields / skip / coverage / exclusions / cancel,
  `scene::store::replace_scenes` carries `skipped` + coverage, `repo::{keeper_predicate, filter_counts_keepers,
  xmp_failures}`), Rust tests, bindings, `isEditPlanDone` in `src/ipc/index.ts`, mock backend (everything above
  emulated; mock applies take one `__mockSceneDelay` step per scene and honour `cancel_scene_apply` between
  scenes), `FilterBar.tsx` compile fix (`getFilterCounts(folderId, projectId, null)`).
- frontend-dev:
  1. P1-2: delete `useWorkflow`'s session maps (`review`, `autoAt`, `appliedCount`); derive the row / strip /
     chip markers from `EditPlan` (`scenes[i].needsReviewIds`, `appliedIds`, `autoEdited`, `editStates`) and, for
     photos outside the plan, `getEditStates(ids)`. Refetch the plan (or `getEditStates([id])`) after
     `save_adjustments` / undo so the "!" clears when the user edits the frame. Add a "Looks good" action on the
     needs-look chip -> `markReviewed([id])`. Review navigation (N) walks `plan.needsReviewIds`.
  2. P1-3: status line `Applied to N · M new keepers not edited` + primary `Apply to M new` when
     `unappliedKeeperIds.length > 0`; the "keepers not in a scene" banner from `unassignedKeeperIds` (`[Group
     them]` -> detect + reload); `allDone` / "All scenes applied" via `isEditPlanDone(plan)`.
  3. P1-4: `Skipped N` tab (`counts.skipped`), row ⋯ Skip / Include (`setSceneSkipped`), key S in the Plan,
     skipped rows dimmed, minor scenes (`minor`) folded last under "Small scenes (N scenes, M photos)"; tab counts
     from `plan.counts`.
  4. P1-5: MatchPanel "Apply" -> `applySceneEdit(sceneId, {...DEFAULT_SCENE_APPLY_OPTIONS, matchOptions,
     excludeIds})` with the same toast / `LastBatch` as Apply to scene (drop the per-image undo loop).
  5. P2-4: × on the `Applying… n/N` pill and Esc in the Plan -> `cancelSceneApply()`; handle `result.cancelled`
     (toast "Stopped after N scenes" with Undo when `batch.batchId`).
  6. P2-5: Edit / Export step grids query with `keepersOnly: true` and `getFilterCounts(null, projectId, true)`;
     filmstrip header "15 of 42 keepers".
  7. P2-6: `openXmpErrors` -> `listXmpFailures(projectId)`; each row gets a `Show` link (grid filtered to the
     failed ids).
- rust-engine-dev / vision-ml-dev: nothing required. New multi-image write paths that should count as an apply or
  auto edit must go through `develop::batches::commit_recorded` (it stamps the history entries); labels decide the
  source of everything else (`history::source_for_label`).

## v19.1 — 2026-10-05 (Phase 8d: edited previews; additive, no schema change)

Types
- `RawImageEntry.editedPreview: EditedPreview | null` — cached renders of the photo's current develop settings
  (`null` when `hasEdits = false` or not rendered yet; may briefly lag the newest edit).
- New `EditedPreview { thumbUrl, previewUrl }`: 512 px / 2048 px JPEGs, orientation + crop applied, served by the
  `sieve` scheme at `sieve://localhost/edited/<id>/<hash>/{thumb,preview}.jpg` (`http://sieve.localhost/...` on
  Windows). Content-addressed (image id + settings hash): a new edit = new URLs; responses are
  `Cache-Control: public, max-age=31536000, immutable`; a missing file answers 404 and queues a render.

Events
- New `editedPreviewChanged { imageId, preview: EditedPreview | null }` (`edited-preview-changed`): a background
  render finished (after edits, paste / sync / presets / scene apply / undo batches, XMP reads, `prepareDevelop`
  neighbours, or a listed photo whose preview was missing / stale); `null` = the photo is unedited again.

Behaviour (rust-engine-dev, `develop::edited`)
- Files live in `<cacheDir>/edited/` only (never next to the photos), one settings hash per image, LRU-bounded by
  bytes (default 1024 MB, `SIEVE_EDITED_CACHE_MB`); `remove_project` deletes the removed images' files.
- Every committed adjustments write (`repo::save_adjustments`) queues a regeneration (350 ms debounce, waits for the
  transaction to commit; rolled-back writes render nothing); a low-priority worker (QoS utility, own pool, yields
  while interactive renders run) renders it, reusing Develop's settled full-quality render when it matches.

Frontend
- Use `src/lib/entryImage.ts` (`thumbSrc`, `previewSrc`, `developPlaceholder`) for any image of a photo; listen to
  `editedPreviewChanged` (done in `useLibrary`). Develop never uses the embedded preview of an edited photo as the
  placeholder (spinner until its render lands when no edited preview exists yet).
- Mock: entries carry `editedPreview`; edits emit the event after `window.__mockEditedDelay` ms (default 120);
  URLs `/mock/edited/<id>/<hash>/{thumb,preview}.jpg` (routed in `tests/ui/helpers.ts`).
