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
  src/
    main.rs                    -> sieve_lib::run()
    lib.rs                     plugins, managed Catalog + Ingest + Analysis + XmpSync, cache/models-dir resolution, asset scope,
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
src/
  ipc/bindings.ts              GENERATED from Rust. Do not edit.
  ipc/index.ts                 re-exports bindings + unwrap() + DEFAULT_QUERY
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
| `src-tauri/src/xmp/` | rust-engine-dev |
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
| `save_adjustments` / `saveAdjustments` | `id: number, adjustments: ParametricAdjustments` | `null` |
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

`set_shoot_type`, `set_burst_window` and `set_cull_thresholds` (for the current shoot type) kick a `rescore`;
`import_folder` / `regenerate_thumbnails` kick `pending` analysis when `autoAnalyze` is on.
`import_folder` reads existing sidecars (`sidecarsRead`); culling writes notify the XMP auto-sync writer.

Events (`events.x.listen(cb)`): `importProgress {done,total,failed}`,
`thumbnailReady {imageId,path,previewPath,width,height}`, `thumbnailFailed {imageId,reason}` (Phase 2),
`analysisProgress {done,total,failed}`, `analysisReady {imageId}`, `analysisFailed {imageId,reason}`,
`analysisFinished {analyzed,failed,cancelled,burstGroups}` (Phase 3),
`xmpSynced {written,read}`, `xmpWriteFailed {imageId,reason}` (Phase 4).

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
  in `img-src` (production; `devCsp` is null for Vite HMR).

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

## Catalog (SQLite)

Location: `<app_data_dir>/catalog.sqlite` (override with `SIEVE_CATALOG=/path`). WAL, `foreign_keys=ON`,
migrations tracked by `PRAGMA user_version`.

| Table | Purpose |
|---|---|
| `catalog_meta` | `shoot_type`, `burst_window_ms`, `auto_analyze`, `xmp_auto_sync`, `cull_thresholds.<shoot_type>` (JSON) |
| `folders` | imported roots |
| `images` | one row per RAW: identity, camera, EXIF, rating/pick/label, burst group, XMP sync state (`xmp_dirty`, `meta_updated_at`, `xmp_synced_at`, `xmp_mtime_ms`, `xmp_error`) |
| `thumbnails` | status pending/ready/failed, `path` (512 px), `preview_path` (2048 px), dims, `error` (pixels are files, not blobs) |
| `image_tags` | `(image_id, tag)` PK, source auto/user, confidence, suppressed |
| `quality_scores` | culling-engine scores per image + `suggested_rating` / `suggested_pick` (derived; rewritten on rescore) |
| `image_analysis` | per-image analysis status (queued/done/failed), model version, error, `phash` (u64 as i64), `faces_json` (`FaceInfo[]`), `metrics_json` (`ml::ImageMetrics`) |
| `burst_groups` | time/similarity clusters, optional keeper |
| `adjustments` | `ParametricAdjustments` JSON + process version, XMP sync time |

Deferred: `scenes` (Phase 7).

## Keeping the contract in sync
- `cargo run`/`pnpm tauri dev` (debug) regenerates `src/ipc/bindings.ts`.
- `cargo test` fails (`bindings_are_up_to_date`) if the committed bindings are stale;
  fix with `UPDATE_BINDINGS=1 cargo test bindings`.
- Log every contract change in `docs/ipc-changelog.md`.
