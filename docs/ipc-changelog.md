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
