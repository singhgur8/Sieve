# Sieve Architecture

## File layout

```
src-tauri/
  Cargo.toml
  tauri.conf.json              productName Sieve, id com.sieve.app
  capabilities/default.json    core:default + dialog:allow-open
  migrations/0001_init.sql     catalog schema v1 (append-only)
  migrations/0002_ingest.sql   v2: thumbnails.preview_path, idx_thumbnails_status
  migrations/0003_analysis.sql v3: image_analysis, quality_scores.suggested_*, auto_analyze
  migrations/0004_xmp.sql      v4: images.xmp_* sync columns + dirty triggers, xmp_auto_sync
  migrations/0005_editor.sql   v5: adjustment_history, presets, adjustments.neutral/history_entry_id, develop dirty triggers
  migrations/0006_export.sql   v6: export_presets, export_jobs, export_items
  migrations/0007_scenes.sql   v7: scenes, images.scene_id/scene_anchor, scene_features
  src/
    main.rs                    -> sieve_lib::run()
    lib.rs                     plugins, managed Catalog + Ingest + Analysis + XmpSync + DevelopCache + LutLibrary + Exporter,
                               cache/models/luts-dir resolution, asset scope, `sieve` render URI scheme,
                               specta_builder() (single registration point for commands + events),
                               debug-build export of src/ipc/bindings.ts
    ipc/
      types.rs                 all contract types (source of truth for TS)
      commands.rs              #[tauri::command] handlers + Catalog state (runs DB work on blocking pool)
      events.rs                ImportProgress, ThumbnailReady, ThumbnailFailed,
                               AnalysisProgress, AnalysisReady, AnalysisFailed, AnalysisFinished,
                               XmpSynced, XmpWriteFailed
      error.rs                 AppError { kind, message }
    db/
      mod.rs                   open (WAL, foreign_keys), user_version migrations
      schema.rs                ordered migration list
      repo.rs                  catalog queries (pure fns over Connection; unit tested)
    raw/mod.rs                 format identification + `extract` (embedded JPEG pick, LibRaw fallback)
      source.rs tiff.rs        byte source; TIFF/ARW IFD + EXIF parsing
      raf.rs cr3.rs jpeg.rs    Fuji RAF header, Canon CR3 ISO-BMFF boxes, JPEG marker scan
      meta.rs                  EXIF -> CaptureMeta (sub-second time, Fuji sensor layout)
      preview.rs turbo.rs      TurboJPEG n/8 scaled decode, resize, orientation, encode
      libraw.rs                minimal FFI to Homebrew libraw_r (build.rs locates it)
  examples/ingest_bench.rs     release benchmark: files/s, ready/failed, peak RSS/footprint
    ingest/mod.rs              background pipeline (Ingest state, start/regenerate, import_status)
    ml/mod.rs                  culling engine: Analysis state/worker, Analyzer (ONNX), score, group_bursts
    ml/thresholds.rs           default CullThresholds per ShootType (calibration data)
    xmp/mod.rs                 XMP sidecar sync: XmpSync state (auto-sync worker), read/write/merge, sidecar_path
    xmp/crs.rs                 develop settings <-> crs:/sieve: properties (mapping table)
    develop/mod.rs             DevelopCache (decoded-source LRU, latest-wins tickets, encoded renders), sieve:// protocol
      source.rs pipeline.rs    half-size linear LibRaw decode; parametric pipeline (shared with Phase 6 export)
      wb.rs                    temperature/tint <-> camera multipliers
      history.rs presets.rs    edit history + all command-path adjustment writes; presets (catalog SQL)
    lut/mod.rs                 .cube LUT library (directory) + parse/apply
    export/mod.rs              Exporter (job queue + worker, memory-bounded concurrency), plan, memory policy
      develop.rs               full-res LibRaw decode + render_full (shared pipeline, resize, sharpen, quantize)
      encode.rs metadata.rs    encoders + ICC; EXIF/XMP selection for exported files
      naming.rs presets.rs     file-name template expansion; export presets (built-ins + catalog SQL)
    scene/mod.rs               scenes + few-shot matching: constants (STATS_MAX_EDGE, tolerances), SceneFeatures,
                               DetectFrame, MatchImage, progress_emitter
      store.rs                 all scene SQL (membership/anchor invariants, detection frames, features, replace)
      features.rs detect.rs    preview appearance features; time-gap + similarity grouping (pure)
      stats.rs matching.rs     render-space ImageStats; relative grading (base/solve/lerp, Phase 9 seam)
src/
  ipc/bindings.ts              GENERATED from Rust. Do not edit.
  ipc/index.ts                 re-exports bindings + unwrap() + DEFAULT_QUERY + ALL_ADJUSTMENT_FIELDS
  App.tsx                      Phase 1 smoke-test UI (catalog state, import, list)
docs/                          this file, ipc-changelog.md, phase plans
.claude/agents/                specialist subagent definitions
```

## Ownership (parallel work without conflicts)

| Path | Owner |
|---|---|
| `src-tauri/src/ipc/`, `src-tauri/src/lib.rs`, `main.rs`, `src-tauri/migrations/`, `src-tauri/src/db/schema.rs`, `src/ipc/`, `docs/` | architect |
| rest of `src-tauri/` (incl. `db/repo.rs`, `raw/`, `ingest/`, `Cargo.toml`) | rust-engine-dev |
| `src-tauri/src/ml/`, `src-tauri/models/` (may append to `Cargo.toml`) | vision-ml-dev |
| `src-tauri/src/scene/` (surface in `scene/mod.rs` + `store.rs` fixed by the architect) | vision-ml-dev |
| `src-tauri/src/xmp/`, `src-tauri/src/develop/`, `src-tauri/src/lut/`, `src-tauri/src/export/` | rust-engine-dev |
| `src/` except `src/ipc/`, `package.json`, Vite/Tailwind/TS config | frontend-dev |
| everything, read-only | qa-engineer |

Specialists implement command *bodies*; signatures and types change only through the architect.

## IPC contract

All commands are `async`, return `Result<T, AppError>`, and in TS resolve to
`{ status: "ok", data } | { status: "error", error: AppError }` (use `unwrap()` to throw instead).

| Command (Rust / TS) | Args | Returns |
|---|---|---|
| `get_catalog_state` / `getCatalogState` | – | `CatalogState` |
| `set_shoot_type` / `setShootType` | `shootType: ShootType` | `null` |
| `set_burst_window` / `setBurstWindow` | `ms: number` (100–60000) | `null` |
| `import_folder` / `importFolder` | `path: string, options: ImportOptions` | `ImportSummary` (returns after registering; extraction runs in background) |
| `regenerate_thumbnails` / `regenerateThumbnails` | `ids: number[]` | `null` (resets to pending, re-queues) |
| `get_import_status` / `getImportStatus` | – | `ImportStatus` |
| `list_images` / `listImages` | `query: ImageQuery` | `ImagePage` |
| `get_image` / `getImage` | `id: number` | `RawImageEntry` |
| `set_rating` / `setRating` | `ids: number[], rating: number` (0–5) | `null` |
| `set_pick` / `setPick` | `ids: number[], pick: PickFlag` | `null` |
| `set_color_label` / `setColorLabel` | `ids: number[], label: ColorLabel \| null` | `null` |
| `set_user_tag` / `setUserTag` | `ids: number[], tag: CullTag, present: boolean` | `null` |
| `get_adjustments` / `getAdjustments` | `id: number` | `ParametricAdjustments` (neutral if unedited) |
| `save_adjustments` / `saveAdjustments` | `id: number, adjustments: ParametricAdjustments, label: string` | `AdjustmentHistory` (pushes/coalesces a history entry) |
| `analyze_images` / `analyzeImages` | `scope: AnalysisScope` | `null` (background) |
| `cancel_analysis` / `cancelAnalysis` | – | `null` |
| `get_analysis_status` / `getAnalysisStatus` | – | `AnalysisStatus` |
| `set_auto_analyze` / `setAutoAnalyze` | `enabled: boolean` | `null` |
| `get_cull_thresholds` / `getCullThresholds` | `shootType: ShootType` | `CullThresholds` (effective) |
| `set_cull_thresholds` / `setCullThresholds` | `shootType: ShootType, thresholds: CullThresholds \| null` | `null` (`null` = reset) |
| `get_faces` / `getFaces` | `id: number` | `FaceInfo[]` |
| `list_burst_groups` / `listBurstGroups` | `folderId: number \| null` | `BurstGroup[]` |
| `apply_suggestions` / `applySuggestions` | `ids: number[]` | `number` (images updated) |
| `get_images` / `getImages` | `ids: number[]` | `RawImageEntry[]` (given order) |
| `list_image_ids` / `listImageIds` | `query: ImageQuery` | `number[]` (all matches, sorted; offset/limit ignored) |
| `get_filter_counts` / `getFilterCounts` | `folderId: number \| null` | `FilterCounts` |
| `write_xmp` / `writeXmp` | `ids: number[]` | `XmpSyncReport` (catalog wins) |
| `read_xmp` / `readXmp` | `ids: number[]` | `XmpSyncReport` (sidecar wins) |
| `set_xmp_auto_sync` / `setXmpAutoSync` | `enabled: boolean` | `null` (enabling flushes dirty images) |
| `get_xmp_status` / `getXmpStatus` | – | `XmpStatus` |
| `render_preview` / `renderPreview` | `id: number, adjustments: ParametricAdjustments, options: RenderOptions` | `RenderedPreview \| null` (`null` = superseded) |
| `get_develop_info` / `getDevelopInfo` | `id: number` | `DevelopInfo` |
| `prepare_develop` / `prepareDevelop` | `ids: number[]` | `null` (background decode) |
| `get_history` / `getHistory` | `id: number` | `AdjustmentHistory` |
| `undo_adjustments` / `undoAdjustments` | `id: number` | `EditState` |
| `redo_adjustments` / `redoAdjustments` | `id: number` | `EditState` |
| `goto_history` / `gotoHistory` | `id: number, entryId: number` | `EditState` |
| `paste_settings` / `pasteSettings` | `ids: number[], adjustments: ParametricAdjustments, fields: AdjustmentField[]` | `null` |
| `sync_settings` / `syncSettings` | `sourceId: number, targetIds: number[], fields: AdjustmentField[]` | `null` |
| `reset_adjustments` / `resetAdjustments` | `ids: number[]` | `null` |
| `apply_preset` / `applyPreset` | `ids: number[], presetId: number` | `null` |
| `list_presets` / `listPresets` | – | `Preset[]` |
| `save_preset` / `savePreset` | `id: number \| null, name: string, adjustments: ParametricAdjustments, fields: AdjustmentField[]` | `Preset` |
| `delete_preset` / `deletePreset` | `id: number` | `null` |
| `list_luts` / `listLuts` | – | `LutInfo[]` |
| `import_lut` / `importLut` | `path: string` | `LutInfo` |
| `delete_lut` / `deleteLut` | `id: string, force: boolean` | `null` |
| `get_export_capabilities` / `getExportCapabilities` | – | `ExportCapabilities` |
| `list_export_presets` / `listExportPresets` | – | `ExportPreset[]` (built-ins first) |
| `save_export_preset` / `saveExportPreset` | `id: number \| null, name: string, settings: ExportSettings` | `ExportPreset` |
| `delete_export_preset` / `deleteExportPreset` | `id: number` | `null` |
| `plan_export` / `planExport` | `ids: number[], settings: ExportSettings` | `ExportPlan` (dry run) |
| `export_images` / `exportImages` | `ids: number[], settings: ExportSettings, presetName: string \| null` | `ExportJob` (queued; background) |
| `cancel_export` / `cancelExport` | `jobId: number` | `null` |
| `get_export_jobs` / `getExportJobs` | – | `ExportJob[]` |
| `detect_scenes` / `detectScenes` | `folderId: number \| null, options: SceneDetectOptions \| null` | `Scene[]` (blocking; replaces auto scenes) |
| `list_scenes` / `listScenes` | `folderId: number \| null` | `Scene[]` (capture order) |
| `get_scene` / `getScene` | `id: number` | `Scene` |
| `create_scene` / `createScene` | `imageIds: number[]` | `Scene` (manual) |
| `set_scene_members` / `setSceneMembers` | `id: number, imageIds: number[]` | `Scene` (manual) |
| `set_scene_anchors` / `setSceneAnchors` | `id: number, anchorIds: number[]` (0..=2 members) | `Scene` |
| `merge_scenes` / `mergeScenes` | `ids: number[]` (>= 2) | `Scene` (into `ids[0]`) |
| `split_scene` / `splitScene` | `id: number, firstImageId: number` | `Scene[]` (`[id, new]`) |
| `delete_scene` / `deleteScene` | `id: number` | `null` |
| `match_scene` / `matchScene` | `anchorIds: number[] (1..=2), targetIds: number[], options: MatchOptions` | `MatchPreview[]` (nothing saved) |
| `apply_scene_match` / `applySceneMatch` | `applications: MatchApplication[], label: string \| null` | `number[]` (changed ids; history + XMP) |
| `get_render_stats` / `getRenderStats` | `id: number, adjustments: ParametricAdjustments \| null, region: NormRect \| null` | `ImageStats` |

`set_shoot_type`, `set_burst_window` and `set_cull_thresholds` (for the current shoot type) kick a `rescore`;
`import_folder` / `regenerate_thumbnails` kick `pending` analysis when `autoAnalyze` is on.
`import_folder` reads existing sidecars (`sidecarsRead`); culling writes notify the XMP auto-sync writer.

Events (`events.x.listen(cb)`): `importProgress {done,total,failed}`,
`thumbnailReady {imageId,path,previewPath,width,height}`, `thumbnailFailed {imageId,reason}` (Phase 2),
`analysisProgress {done,total,failed}`, `analysisReady {imageId}`, `analysisFailed {imageId,reason}`,
`analysisFinished {analyzed,failed,cancelled,burstGroups}` (Phase 3),
`xmpSynced {written,read}`, `xmpWriteFailed {imageId,reason}` (Phase 4). Rendered previews use the `sieve` URI scheme (Phase 5).
`exportProgress {jobId,done,total,failed,skipped,currentFile}`,
`exportFinished {jobId,succeeded,skipped,failed,cancelled,outputDir,elapsedMs}` (Phase 6).
`sceneProgress {task: "detect" | "match", done, total}` (Phase 7).

Batch writes (`ids: number[]`) are atomic: an unknown id fails the whole batch with `not_found`.

### Wire conventions
- Struct fields `camelCase`; enum values `snake_case` strings, identical to the DB column values.
- IDs / unix-ms timestamps are `i64` → TS `number` (all < 2^53).
- Floats are exported as `number`; the backend never sends NaN.
- `ParametricAdjustments` mirrors Adobe `crs:` Process 2012 names/ranges for 1:1 XMP mapping.
  Stored JSON is overlaid on neutral defaults when read, so adding a slider needs no migration.
- Removing an **auto** tag suppresses it (row kept, `suppressed = true`); removing a **user** tag deletes it.
  Suppressed tags never match filters or count in `tagCounts`.

## Ingest pipeline (Phase 2)

- `import_folder` registers files (thumbnail rows `pending`) and calls `Ingest::start`, which returns
  immediately. `start` is an idempotent kick: one background worker streams `pending` rows in small
  batches (index `idx_thumbnails_status`) and processes them on a rayon pool, so memory is bounded and
  new imports / `regenerate_thumbnails` join a running pipeline. On app start `lib.rs` calls `start`
  to resume work left by a previous session.
- The worker uses its own SQLite connection to the catalog path (WAL), not the command connection.
- Per image, one pass: embedded JPEG + EXIF via `raw::` → `<cacheDir>/thumbs/<id>_512.jpg` (grid) and
  `<id>_2048.jpg` (loupe), orientation applied → update `images` EXIF columns + `thumbnails` row →
  emit `thumbnailReady` or `thumbnailFailed` (reason also stored in `thumbnails.error`) and throttled
  `importProgress` (per run; `done == total` = idle).
- Cache root: `app_cache_dir()` or `SIEVE_CACHE=/path`; exposed as `CatalogState.cacheDir`.
- Frontend loads images with `convertFileSrc(path)`. Asset protocol scope: `$APPCACHE/thumbs/**` plus
  the resolved `<cacheDir>/thumbs` added at runtime. CSP allows `asset:` / `http://asset.localhost`
  in `img-src` (production; `devCsp` is null for Vite HMR). Since v5 also `sieve: http://sieve.localhost` (renders).

## Analysis / culling (Phase 3)

- `Analysis` (managed state, `ml/`) mirrors `Ingest`: `start(app, scope)` is an idempotent kick that
  returns immediately; one worker thread with its own SQLite connection pulls work from the DB and runs
  a rayon pool (one `Analyzer` = ONNX sessions per pool thread). `cancel()` stops after in-flight images;
  unprocessed work stays pending.
- Auto-run: with `catalog_meta.auto_analyze = '1'` (default) analysis is kicked on launch, after
  `import_folder` and after `regenerate_thumbnails`. It runs *concurrently* with ingest: while ingest is
  running the worker waits for more previews instead of exiting.
- Input: the 2048 px preview (`thumbnails.preview_path`), orientation applied. Face coordinates are
  normalized to that frame.
- Needs-analysis predicate: thumbnail `ready` with a `preview_path`, and (no `image_analysis` row, or
  `status = 'queued'`, or `model_version <> ml::MODEL_VERSION`, or `analyzed_at < thumbnails.extracted_at`).
  `failed` rows with the current version are not retried until forced (`images`/`folder`/`all` scope set
  `status = 'queued'`) or the preview is re-extracted.
- Two stages: `Analyzer::measure` (expensive, threshold-independent → `image_analysis.metrics_json`, `phash`)
  and `score(metrics, thresholds, shootType)` (pure → `quality_scores`, `faces_json`, auto tags). A `rescore`
  (shoot type / threshold / burst window change) reruns only `score` + burst grouping from stored metrics.
- Per image, one transaction: `image_analysis` + `quality_scores` + auto tags; then `analysisReady` /
  `analysisFailed` and throttled `analysisProgress`. When the queue drains (and ingest is idle): burst
  regrouping over images with `captured_at_ms` (gap ≤ `burst_window_ms`, phash Hamming ≤
  `burstHashDistance`), keeper = best `overall`, `duplicate_burst` on the others with lowered suggestions;
  then `analysisFinished`.
- Auto tag rules: upsert `source = 'auto'` with confidence; delete auto tags no longer emitted unless
  suppressed; never modify suppressed rows (no resurrection) or user rows. The engine never writes
  `images.rating` / `images.pick`; `apply_suggestions` is the only path from suggestions to user values.
- Thresholds: `ml::thresholds::default_thresholds(shootType)`; overrides stored as JSON in
  `catalog_meta['cull_thresholds.<shoot_type>']`, overlaid on defaults when read (`repo::cull_thresholds`).
- Ownership of SQL: repo.rs holds the command-side reads/user writes; the worker's SQL lives in `ml/`.
- Models: `<models_dir>/det_10g.onnx`, `2d106det.onnx` (`scripts/fetch-models.sh`); `SIEVE_MODELS`
  overrides the dir (default `src-tauri/models` in debug, `<resource_dir>/models` in release).

## XMP sidecars (Phase 4)

- Sidecar: `<basename>.xmp` next to the RAW (Lightroom convention; `DSC0001.ARW` -> `DSC0001.xmp`). Two RAWs
  with the same basename in one folder would share a sidecar (not supported; last writer wins).
- Mapping, catalog -> sidecar:
  | Catalog | XMP |
  |---|---|
  | `pick = reject` | `xmp:Rating = -1` (stars not representable) |
  | `rating` 0..=5 (not rejected) | `xmp:Rating` |
  | `pick = pick` | `xmp:Label = "Pick"` (wins over a colour label) |
  | `colorLabel` (not picked) | `xmp:Label = "Red"/"Yellow"/"Green"/"Blue"/"Purple"` |
  | neither | `xmp:Label` removed only if it held "Pick" or one of those names |
  | visible tags | `lr:hierarchicalSubject` `Sieve\|<tag>` + `dc:subject` `<tag>` |
  Writes replace only `Sieve|*` items (and their `dc:subject` leaves), bump `xmp:MetadataDate`, and preserve
  every other field/namespace. Atomic (temp file + rename).
- Sidecar -> catalog (read/import): `-1` -> reject; `0..=5` -> rating, `pick` iff Label "Pick" else unflagged;
  label names -> `colorLabel`. `Sieve|*` keywords are not read back (analysis owns tags).
- Dirty tracking is in the schema: triggers set `images.xmp_dirty = 1` + `meta_updated_at` when rating / pick /
  color_label or visible tags change, whoever writes them (commands, `apply_suggestions`, analysis auto tags).
  A successful write/read sets `xmp_dirty = 0`, `xmp_synced_at`, `xmp_mtime_ms` (sidecar mtime), clears `xmp_error`.
- Conflict policy: `write_xmp` = catalog wins; `read_xmp` and import = sidecar wins; auto-sync = newer wins: a dirty
  image whose sidecar mtime still equals `xmp_mtime_ms` is written; if the sidecar changed on disk too, the newer of
  sidecar mtime vs `meta_updated_at` wins. Import refreshes non-dirty images whose sidecar mtime changed.
- Auto-sync (`catalog_meta.xmp_auto_sync`, default off): `XmpSync::notify` re-arms a 1 s debounce; the worker (own
  SQLite connection, never the command mutex) syncs all dirty images, emits `xmpSynced` per pass and
  `xmpWriteFailed` per failure. Notified after culling writes, on enabling, on launch and on `analysisFinished`.

## Editor (Phase 5)

### Render path (slider feedback < 100 ms)
1. Slider input -> `renderPreview(id, liveAdjustments, { maxEdge, slot: "main", region: null })` on every input
   event (no client throttling needed; the backend coalesces). Nothing is saved.
2. The command validates, takes a **ticket** `(id, slot, seq)` on the async side (arrival order), resolves the RAW
   path, then on the blocking pool: skip if a newer ticket exists -> render -> store the JPEG in memory as the newest
   for `(id, slot)` -> return metadata. At most one render per key runs; requests queued behind it that are no
   longer newest resolve `null`, so a fast drag renders "current, then latest" instead of every frame.
3. The frontend sets `<img src={preview.url}>` (`sieve://localhost/render/<id>/<slot>?v=<seq>`), served from memory
   by the async `sieve` URI scheme handler (`Cache-Control: no-store`); `?v=` busts WebKit's cache. It ignores
   results with a `seq` lower than the one displayed.
4. On slider release (or debounced): `saveAdjustments(id, adj, "Exposure")` -> history entry + XMP dirty.
- Histogram (256 bins R/G/B/luma of the 8-bit output) and `renderMs` come back with every render.
- Slots are independent streams: `before` (before/after view), `detail` (region renders for 1:1 zoom).

### Develop source + cache
- LibRaw `half_size` decode (camera RGB, no WB, linear, 16-bit; ~3000 px long edge for 24 MP) + as-shot multipliers,
  colour matrix, levels. Decoded once per image (~0.3-0.8 s), kept in `DevelopCache` (LRU by bytes, default 1 GiB,
  `SIEVE_DEVELOP_CACHE_MB`) with a working-size f32 copy. `prepareDevelop(neighbourIds)` warms it in the background.
- White balance happens in the pipeline on raw data: `as_shot` uses the camera multipliers; `custom` converts
  temperature/tint -> multipliers through the camera matrix (`develop::wb`). `getDevelopInfo().asShot` gives the
  as-shot temperature/tint for the sliders.
- Pipeline (shared with Phase 6 full-res export): WB -> camera->linear Rec.2020 -> exposure -> tone (contrast,
  highlights/shadows/whites/blacks) -> texture/clarity/dehaze -> vibrance/saturation -> HSL -> sRGB encode -> LUT
  (amount blend) -> 8-bit -> histogram -> JPEG (TurboJPEG q90 4:4:4). Orientation applied; `region` crops first.

### History, presets, copy/paste
- Per-image linear history of full snapshots (`adjustment_history`), cursor in `adjustments.history_entry_id`;
  first entry "Original". Undo/redo/goto move the cursor and rewrite `adjustments`. New edits after an undo drop the
  redo tail. Same-label saves within 1.5 s coalesce; max 200 entries per image. All command-path adjustment writes go
  through `develop::history` (`commit`, `apply_fields`).
- Fields masks (`AdjustmentField[]`) select groups; semantics = `ParametricAdjustments::copy_fields`. Copy lives in
  frontend state; `pasteSettings(ids, copied, fields)`; `syncSettings(sourceId, targetIds, fields)` reads the source's
  stored adjustments; presets store adjustments + fields; `applyPreset` copies only its fields. Batches are atomic and
  push one entry per changed image.
- `RawImageEntry.hasEdits` = adjustments differ from neutral (`adjustments.neutral = 0`).

### LUTs
- Library = directory `<app_data_dir>/luts/` (`SIEVE_LUTS`), files `<id>.cube`, shared by all catalogs; Phase 9
  writes generated LUTs there. `importLut` validates + copies (idempotent by content hash in the id).
  `ParametricAdjustments.lut = { id, amount }`; a missing id renders without the LUT (`lutMissing`).
  `deleteLut` refuses while referenced unless `force`.

### XMP develop settings
- `crs:` properties map 1:1 to `ParametricAdjustments` (table in `xmp/crs.rs`); LUT in `sieve:LutId/LutAmount`.
  Written only for images with an adjustments row, so Lightroom edits of images never touched in Sieve survive;
  all unowned `crs:` properties (curves, crop, sharpening, masks...) are preserved.
- Read on `read_xmp`, import and newer-wins auto-sync when PV2012+ settings exist; applied as a "Read from XMP"
  history entry. Lossy only for WB "Auto" (-> as shot) and named WB presets (-> custom with Lightroom's values).
  Rendering of imported settings approximates Adobe's, not pixel-identical.
- Dirty tracking: triggers on `adjustments` (0005) join the 0004 triggers, so one `xmp_dirty` flag covers both.

## Export (Phase 6)

### Flow
1. Export dialog: pick a preset (`listExportPresets`; built-ins are read-only, destination `choose`), edit settings,
   choose the folder (dialog plugin) -> `destination = {kind: "folder", path}`. Optional `planExport` shows final
   names and "N files already exist". Formats with `available = false` in `getExportCapabilities()` are disabled.
2. `exportImages(ids, settings, presetName)` validates, resolves/creates `<destination>/<subfolder>`, snapshots
   each image's stored adjustments, writes `export_jobs` + `export_items` (`pending`, `seq` = position in `ids`) and
   returns the `queued` job. Later edits do not affect a queued job.
3. One `export` worker thread (own SQLite connection) runs jobs one at a time in id order. Within a job, images are
   developed concurrently (below); `exportProgress` is throttled; each job ends with exactly one `exportFinished`.
4. `cancelExport(jobId)`: in-flight images stop at a checkpoint (temp file removed) or finish; unstarted images
   stay `pending`; state `cancelled`. Written files are kept. On launch, jobs left queued/running are `interrupted`.

### Per image (same pipeline as the preview)
`export::develop::decode_full` (LibRaw full demosaic: preview decode settings with `half_size = 0`, `user_qual = 3`
= AHD for Bayer, 3-pass Markesteijn for X-Trans; `highlight = 0`, no WB, linear 16-bit camera RGB)
-> resample in linear light to `output_size` (orientation applied)
-> `develop::pipeline` stages up to the display-referred linear Rec.2020 working image (resolution-independent)
-> output colour space: no LUT = linear Rec.2020 -> target primaries + transfer curve (keeps P3 / Adobe RGB gamut);
   with a LUT = sRGB-encode -> LUT (as in the preview) -> target space
-> output sharpening (media x amount; on encoded luminance, radius scaled to output size)
-> quantize (8/16-bit) -> encode with the target ICC profile, resolution (ppi) and filtered metadata
-> `<name>.<ext>.sieve-tmp` in the target dir, fsync, rename. A missing LUT exports without it.
Resizing *before* the pipeline makes web exports cheap and matches the preview (which renders a downsampled
source); full-size exports run the pipeline at full resolution. WYSIWYG check: an sRGB 8-bit export at the preview's
size without sharpening matches `render_preview` within 2 levels.

### Memory bound
- Budget `B` = `SIEVE_EXPORT_MEMORY_MB`, else 25% of physical RAM clamped to 2..=8 GiB (`export::memory_budget_bytes`).
- Per-image estimate (`export::estimate_image_bytes`) = 16 B x source px (LibRaw decode) + 24 B x output px (f32
  working set + quantized output) + 64 MiB: 24 MP full-res ~1.0 GB, 61 MP full-res ~2.4 GB, any -> 2048 px ~0.5 GB.
- A weighted semaphore over `B` (an estimate larger than `B` is capped at `B`, so it runs alone) plus a hard cap of
  `MAX_PARALLEL = 4` images; each image parallelizes internally with rayon. 16 GB Mac: 4 GiB budget = 4 x 24 MP or
  1 x 61 MP full-res at a time. The develop preview cache (`DevelopCache`) is neither used nor evicted by exports.

### Naming, collisions, metadata
- Template grammar: literal text + `{filename}`, `{seq}`/`{seq:N}` (`ids` position + `startNumber`), `{date}`/
  `{date:FMT}` (capture wall-clock, else file mtime), `{rating}`, `{camera}`, `{folder}`, `{id}`
  (`ipc::types::parse_filename_template`). Token values are sanitized; the format's lower-case extension is appended.
- Collisions with files on disk follow `collision` (`unique_suffix` -> `-2`, `-3`...; `overwrite`; `skip` counts as
  `skipped`); names repeated within one job always get unique suffixes.
- Metadata: `all` = RAW EXIF + sidecar XMP/IPTC (keywords optional, location optionally stripped);
  `copyright_only`; `copyright_and_contact`; `none`. Never exported: `crs:` develop settings, `Sieve|*` tags.
  Orientation is written as 1; ICC and resolution are always embedded. `copyright` / `creator` override the source.

## Scenes & matching (Phase 7)

### Scenes
- A scene = consecutive frames under one lighting scenario, graded from 1-2 anchors. Membership is
  `images.scene_id` (at most one scene per image); anchors are `images.scene_anchor` on member rows (<= 2).
  `scenes` stores derived `folder_id` (NULL if members span folders), `started_at_ms` / `ended_at_ms`, and `method`.
  All writes go through `scene::store` (invariants in its module docs). Grid: `ImageQuery.sceneId`,
  `RawImageEntry.sceneId` / `isSceneAnchor`.
- `detect_scenes(folderId, options)`: `store::detection_frames` (folder order, capture time, file name; excludes
  members of manual scenes unless `replaceManual`) -> `features::compute_missing` off the catalog lock (2048 px
  previews, rayon, cached in `scene_features` under `FEATURES_VERSION`, stale when the preview is re-extracted) ->
  `detect::group` (pure: time gap > `maxGapMs` splits; bursts never split; appearance similarity >= `similarity`
  to stay; never crosses folders) -> `store::replace_scenes` (drops auto scenes in scope, creates new auto scenes,
  carries anchor flags over).
- User edits (`create_scene`, `set_scene_members`, `merge_scenes`, `split_scene`) make scenes `manual`; re-detection
  leaves them alone unless `replaceManual`. `set_scene_anchors` keeps the method.

### Matching (relative grading)
- Statistics (`ImageStats`) are measured on the **rendered** 8-bit output of the editor pipeline at 640 px
  (`DevelopCache::render_image`: same cached source, pipeline and LUT as `render_preview`, no tickets / encoding):
  linear mean / log-mean luminance, percentiles, clipping, mean Oklab, neutral estimate (xy + Oklab a/b), effective
  and as-shot white balance.
- Per target T with anchor A: `base` = T's settings with `copyFields` + matched groups copied from A (WB resolved to
  A's effective `custom` temp/tint); `reference` = A rendered with its settings; `full` = `solve(reference, base,
  options, measure)` iterating render -> measure -> correct (exposure from log-mean luma EV difference, temp/tint from
  the rendered neutral difference, optional contrast/whites/blacks from percentiles); `adjustments =
  ParametricAdjustments::lerp(base, full, strength)`. Two anchors: targets between them in capture time blend both
  (settings via `lerp`, stats via `blend_stats`), others use the nearest.
- `solve` only needs a reference `ImageStats`, a base and a measure callback, so Phase 9 (reference-photo matching,
  e.g. stats of a JPEG) reuses it; a baked LUT output would come from the same fitted correction.
- Acceptance: at strength 1, |`logMeanLuma` - reference| <= `TOLERANCE_EV` (0.15 EV) and |`neutral.ab` - reference|
  <= `TOLERANCE_AB` (0.012 Oklab), checked with `get_render_stats(target, preview.full)`.
- `apply_scene_match(applications, label)` commits per-image adjustments via `develop::history::commit_batch` (atomic,
  one "Match Scene" entry per changed image, XMP notify); the UI's strength slider recomputes with the TS mirror
  `lerpAdjustments` and applies what it previews.
- Cost: first match of an image decodes its RAW into the develop cache (~0.3-0.8 s, shared with the editor); each
  measurement is a 640 px pipeline run (~10-20 ms). Targets are processed in parallel; `sceneProgress` reports.

## Catalog (SQLite)

Location: `<app_data_dir>/catalog.sqlite` (override with `SIEVE_CATALOG=/path`). WAL, `foreign_keys=ON`,
migrations tracked by `PRAGMA user_version`.

| Table | Purpose |
|---|---|
| `catalog_meta` | `shoot_type`, `burst_window_ms`, `auto_analyze`, `xmp_auto_sync`, `cull_thresholds.<shoot_type>` (JSON) |
| `folders` | imported roots |
| `images` | one row per RAW: identity, camera, EXIF, rating/pick/label, burst group, XMP sync state (`xmp_dirty`, `meta_updated_at`, `xmp_synced_at`, `xmp_mtime_ms`, `xmp_error`), `scene_id`, `scene_anchor` |
| `thumbnails` | status pending/ready/failed, `path` (512 px), `preview_path` (2048 px), dims, `error` (pixels are files, not blobs) |
| `image_tags` | `(image_id, tag)` PK, source auto/user, confidence, suppressed |
| `quality_scores` | culling-engine scores per image + `suggested_rating` / `suggested_pick` (derived; rewritten on rescore) |
| `image_analysis` | per-image analysis status (queued/done/failed), model version, error, `phash` (u64 as i64), `faces_json` (`FaceInfo[]`), `metrics_json` (`ml::ImageMetrics`) |
| `burst_groups` | time/similarity clusters, optional keeper |
| `adjustments` | `ParametricAdjustments` JSON + process version, `neutral`, `history_entry_id` (cursor); `xmp_synced_at` unused |
| `adjustment_history` | per-image snapshots (label, params JSON, created/updated) |
| `presets` | name (unique, NOCASE), params JSON, fields JSON |
| `export_presets` | user export presets: name (unique, NOCASE), `ExportSettings` JSON (built-ins are in code) |
| `export_jobs` | one per `export_images`: state, resolved output dir, settings JSON, counters, timestamps |
| `export_items` | per (job, seq): image, status pending/done/failed/skipped, output path, error |
| `scenes` | lighting scenarios: derived folder / started / ended, method auto/manual (members via `images.scene_id`) |
| `scene_features` | per-image appearance features for detection (JSON, `version`, `computed_at`) |

## Keeping the contract in sync
- `cargo run`/`pnpm tauri dev` (debug) regenerates `src/ipc/bindings.ts`.
- `cargo test` fails (`bindings_are_up_to_date`) if the committed bindings are stale;
  fix with `UPDATE_BINDINGS=1 cargo test bindings`.
- Log every contract change in `docs/ipc-changelog.md`.
