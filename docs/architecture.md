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
  migrations/0008_ux.sql       v8: burst_keeper_pins (user-chosen burst keepers)
  migrations/0009_parity_sources.sql v9: images.format CHECK += jpeg/heic/tiff/png (in place), companion_path, develop_warnings
  migrations/0010_masks.sql    v10: mask_cache (AI mattes), images.masks_pending_import
  migrations/0011_hardening.sql v11: filter-bar indexes, images.missing_since_ms (+ partial index)
  migrations/0012_workflow_styles.sql v12: projects (+ folders.project_id, export_jobs.project_id), style library
                               (style_groups, style_profiles, presets per group), edit_batches, scene edit plan
                               columns, style_models/style_features, keeper_rule, XMP auto-sync on
  migrations/0013_workflow_state.sql v13: adjustment_history.source/batch_id (per-photo edit source), batch item
                               provenance + review flags, scenes.skipped/applied_covered_json
  migrations/0014_linear_undo.sql v14: clears the applied state of scenes whose apply batch was undone (IPC v16)
  migrations/0015_apply_bases.sql v15: edit_batch_bases (the batch a scene apply's representative settings came
                               from; linear undo across Auto edit -> Apply, IPC v17)
  src/
    main.rs                    -> sieve_lib::run()
    lib.rs                     plugins, managed Catalog + Ingest + Analysis + XmpSync + DevelopCache + LutLibrary + Exporter,
                               cache/models/luts-dir resolution, asset scope, `sieve` render URI scheme,
                               specta_builder() (single registration point for commands + events),
                               debug-build export of src/ipc/bindings.ts
    ipc/
      types.rs                 all contract types (source of truth for TS)
      masks.rs                 mask / local-adjustment types (v10), re-exported from types.rs
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
      raster.rs                non-RAW sources (JPEG/HEIC/TIFF/PNG): ingest extract, linear decode, embedded XMP (v9)
  examples/ingest_bench.rs     release benchmark: files/s, ready/failed, peak RSS/footprint
    ingest/mod.rs              background pipeline (Ingest state, start/regenerate, import_status)
    ml/mod.rs                  culling engine: Analysis state/worker, Analyzer (ONNX), score, group_bursts
    ml/thresholds.rs           default CullThresholds per ShootType (calibration data)
    ml/auto_faces.rs           on-demand SCRFD face boxes for Auto tone on unanalysed photos (held by DevelopCache)
    ml/masking.rs              AI mask seam: Segmenter (managed state), SegmentModel trait (v10)
    xmp/mod.rs                 XMP sidecar sync: XmpSync state (auto-sync worker), read/write/merge, sidecar_path
    xmp/crs.rs                 develop settings <-> crs:/sieve: properties (mapping table)
    xmp/masks.rs               crs:MaskGroupBasedCorrections <-> masks (mapping tables, Lightroom mattes) (v10)
    develop/mod.rs             DevelopCache (decoded-source LRU, latest-wins tickets, encoded renders), sieve:// protocol
      source.rs pipeline.rs    half-size linear LibRaw decode; parametric pipeline (shared with Phase 6 export)
      wb.rs                    temperature/tint <-> camera multipliers
      history.rs presets.rs    edit history + all command-path adjustment writes; presets (catalog SQL)
      parity.rs                Lightroom-parity stages: curves, color grading, calibration, detail, effects, crop (v9)
      masks.rs                 mask evaluation, local parameter planes, overlays, MaskCache (AI mattes) (v10)
    profiles/mod.rs            installed Adobe DCPs + looks (read in place), ProfileLibrary (v9)
      dcp.rs look.rs table.rs  DCP parser; look profiles; Adobe crs:Table_ big-table decoder
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
| rest of `src-tauri/` (incl. `db/repo.rs`, `db/projects.rs`, `styles/`, `raw/`, `ingest/`, `Cargo.toml`) | rust-engine-dev |
| `src-tauri/src/ml/`, `src-tauri/models/` (may append to `Cargo.toml`) | vision-ml-dev |
| `src-tauri/src/scene/` (surface in `scene/mod.rs` + `store.rs` fixed by the architect) | vision-ml-dev |
| `src-tauri/src/xmp/`, `src-tauri/src/develop/`, `src-tauri/src/lut/`, `src-tauri/src/export/`, `src-tauri/src/profiles/` | rust-engine-dev |
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
| `apply_suggestions` / `applySuggestions` | `ids: number[], onlyUnset: boolean` | `ApplySuggestionsResult` (`{applied, skipped}`) |
| `get_images` / `getImages` | `ids: number[]` | `RawImageEntry[]` (given order) |
| `list_image_ids` / `listImageIds` | `query: ImageQuery` | `number[]` (all matches, sorted; offset/limit ignored) |
| `get_filter_counts` / `getFilterCounts` | `folderId: number \| null` | `FilterCounts` |
| `write_xmp` / `writeXmp` | `ids: number[]` | `XmpSyncReport` (catalog wins) |
| `read_xmp` / `readXmp` | `ids: number[]` | `XmpSyncReport` (sidecar wins) |
| `set_xmp_auto_sync` / `setXmpAutoSync` | `enabled: boolean` | `null` (enabling flushes dirty images) |
| `get_xmp_status` / `getXmpStatus` | – | `XmpStatus` |
| `render_preview` / `renderPreview` | `id: number, adjustments: ParametricAdjustments, options: RenderOptions` | `RenderedPreview \| null` (`null` = superseded) |
| `get_develop_info` / `getDevelopInfo` | `id: number` | `DevelopInfo` |
| `sample_white_balance` / `sampleWhiteBalance` (v11) | `id: number, point: NormPoint` (sensor frame), `adjustments: ParametricAdjustments` | `WhiteBalanceValues` (`invalid_argument` if clipped/too dark) |
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
| `set_burst_keeper` / `setBurstKeeper` | `groupId: number, imageId: number` | `BurstGroup` (pinned; kicks rescore) |
| `get_cull_snapshot` / `getCullSnapshot` | `ids: number[]` | `CullSnapshot[]` (given order) |
| `restore_cull_snapshot` / `restoreCullSnapshot` | `snapshots: CullSnapshot[]` | `number[]` (changed ids; atomic) |
| `get_ui_prefs` / `getUiPrefs` | – | `UiPrefs` |
| `set_ui_prefs` / `setUiPrefs` | `prefs: UiPrefs` | `null` (replaces all) |
| `reveal_in_finder` / `revealInFinder` | `path: string` (absolute, existing) | `null` |
| `write_xmp_all_dirty` / `writeXmpAllDirty` | `folderId: number \| null` | `XmpSyncReport` (catalog wins) |
| `list_profiles` / `listProfiles` | `id: number` | `ProfileCatalog` |
| `list_masks` / `listMasks` | `id: number` | `MaskList` (stored groups + AI status) |
| `save_masks` / `saveMasks` | `id: number, masks: MaskGroup[], label: string` | `AdjustmentHistory` (replaces only `masks`) |
| `compute_ai_mask` / `computeAiMask` | `id: number, request: AiMaskRequest` | `AiMaskInfo` (cached per image + kind + model) |
| `detect_people` / `detectPeople` | `id: number` | `DetectedPerson[]` |
| `render_mask_overlay` / `renderMaskOverlay` | `id: number, adjustments: ParametricAdjustments, target: MaskOverlayTarget, options: MaskOverlayOptions` | `RenderedMaskOverlay \| null` (`mask` slot, latest-wins) |
| `get_mask_capabilities` / `getMaskCapabilities` | – | `MaskCapabilities` |
| `model_downloads_status` / `modelDownloadsStatus` (v12) | – | `ModelDownloadStatus` (groups + installed files + `downloading`) |
| `download_models` / `downloadModels` (v12) | `group: string` (`MODEL_GROUP_SEGMENTATION`) | `null` (background; `invalid_argument` if unknown / already downloading) |
| `cancel_model_download` / `cancelModelDownload` (v12) | – | `null` (no-op when idle) |
| `relocate_folder` / `relocateFolder` (v13) | `folderId: number, newPath: string` | `RelocateResult` (`{matched, stillMissing}`; `invalid_argument` if none found) |
| `restore_catalog_backup` / `restoreCatalogBackup` (v13) | `index: number` (1 = newest) | `CatalogHealth` (`restorePending: true`; applied at next launch) |
| `list_projects` / `listProjects` (v14) | – | `Project[]` (last opened first, then newest) |
| `get_project` / `getProject` (v14) | `projectId: number` | `Project` |
| `create_project` / `createProject` (v14) | `path: string, name: string \| null, shootType: ShootType \| null, options: ImportOptions` | `CreateProjectResult` (`existing: true` if the folder is already in the catalog) |
| `open_project` / `openProject` (v14) | `projectId: number` | `Project` (stamps `lastOpenedAtMs`) |
| `rename_project` / `renameProject` (v14) | `projectId: number, name: string` | `Project` |
| `set_project_cover` / `setProjectCover` (v14) | `projectId: number, imageId: number \| null` | `Project` (`null` = automatic cover) |
| `set_project_shoot_type` / `setProjectShootType` (v14) | `projectId: number, shootType: ShootType` | `null` (kicks a rescore) |
| `remove_project` / `removeProject` (v14) | `projectId: number` | `RemoveProjectResult` (catalog rows + cached thumbnails only; files untouched) |
| `get_workflow_step` / `set_workflow_step` (v14) | `projectId: number` (+ `step: WorkflowStep`) | `WorkflowStep` / `null` |
| `set_keeper_rule` / `setKeeperRule` (v14) | `rule: KeeperRule` | `null` |
| `get_edit_plan` / `getEditPlan` (v14) | `projectId: number` | `EditPlan` |
| `set_scene_representative` / `setSceneRepresentative` (v14) | `sceneId: number, imageId: number \| null` | `SceneEditEntry` |
| `apply_scene_edit` / `applySceneEdit` (v14) | `sceneId: number, options: SceneApplyOptions \| null` | `ApplyScenesResult` (one undoable batch) |
| `apply_all_edited_scenes` / `applyAllEditedScenes` (v14) | `projectId: number, options: SceneApplyOptions \| null` | `ApplyScenesResult` (v17: scenes it cannot apply are left out and listed in `skippedScenes`) |
| `undo_edit_batch` / `undoEditBatch` (v14; linear since v16: `conflict` when photos were edited after the batch, v17: or a scene apply was made from its settings) | `batchId: number` | `UndoBatchResult` |
| `get_edit_batches` / `getEditBatches` (v16) | `batchIds: number[]` | `EditBatchInfo[]` |
| `paste_previous` / `pastePrevious` (v14) | `targetIds: number[], previousId: number, fields: AdjustmentField[] \| null` | `null` |
| `import_style_folder` / `importStyleFolder` (v14) | `path: string` | `ImportStyleReport` |
| `list_styles` / `listStyles` (v14) | – | `StyleLibrary` |
| `remove_style_group` / `removeStyleGroup` (v14) | `groupId: number` | `null` (built-ins -> `invalid_argument`) |
| `resolve_preset` / `resolvePreset` (v14) | `id: number, presetId: number, adjustments: ParametricAdjustments \| null` | `ParametricAdjustments` (nothing saved) |
| `auto_tone` / `autoTone` (v14) | `id: number, adjustments: ParametricAdjustments \| null, keys: AdjustmentField[] \| null` | `AutoToneValues` (nothing saved; unanalysed photos: face boxes detected on demand, `ml::auto_faces`) |
| `auto_white_balance` / `autoWhiteBalance` (v14) | `id: number, adjustments: ParametricAdjustments \| null` | `WhiteBalanceValues` (nothing saved) |
| `style_model_status` / `styleModelStatus` (v14) | – | `StyleModelStatus` |
| `train_style_model` / `trainStyleModel`, `cancel_style_training` (v14) | – | `null` (background; `styleModel*` events) |
| `predict_style` / `predictStyle` (v14) | `imageIds: number[]` | `StylePrediction[]` (nothing saved) |
| `apply_style_prediction` / `applyStylePrediction` (v14) | `imageIds: number[]` | `EditBatchResult` |
| `set_scene_skipped` / `setSceneSkipped` (v15) | `sceneId: number, skipped: boolean` | `SceneEditEntry` |
| `get_edit_states` / `getEditStates` (v15) | `imageIds: number[]` | `ImageEditState[]` |
| `mark_reviewed` / `markReviewed` (v15) | `imageIds: number[]` | `number[]` (ids whose needs-a-look was cleared) |
| `cancel_scene_apply` / `cancelSceneApply` (v15) | – | `null` (running apply resolves with `cancelled: true`) |
| `list_xmp_failures` / `listXmpFailures` (v15) | `projectId: number \| null` | `XmpFailure[]` (capture order) |

v15: `get_filter_counts(folderId, projectId, keepersOnly)` and `ImageQuery.keepersOnly` (keepers only);
`SceneApplyOptions.excludeIds`.

v14 project scoping: `import_folder(path, options, projectId | null)`, `get_filter_counts(folderId, projectId)`,
`list_burst_groups(folderId, projectId)`, `list_scenes(folderId, projectId)`, `detect_scenes(folderId, projectId,
options)`, `ImageQuery.projectId`, `AnalysisScope::project` (folder AND project; `null` = no constraint).

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
`modelDownloadProgress {group,name,fileIndex,fileCount,bytesDone,bytesTotal}`,
`modelDownloadFinished {group,ok,cancelled,error}` (v12).
`styleModelProgress {phase,done,total}`, `styleModelFinished {ok,cancelled,error,status}` (v14);
`sceneProgress` task `"apply"` (v14).

Batch writes (`ids: number[]`) are atomic: an unknown id fails the whole batch with `not_found`.

Error kinds (`AppError.kind`; the `message` is always user-facing): `not_found` (catalog row), `invalid_argument`,
`io`, `database`, `internal`, and since v13 `file_missing` (original not at its path), `disk_full`, `read_only`
(volume read-only / no permission), `decode_failed` (original exists but cannot be decoded), `catalog_read_only`
(damaged catalog, see Catalog health). Per-file failures in reports (`XmpFailure`, `ExportFailure`,
`thumbnailFailed`) stay plain strings; a missing original's reason starts with `Original file is missing`.

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
- Mapping, catalog -> sidecar (Lightroom Classic 13.2+ flags, Phase 8c; the catalog is authoritative on write):
  | Catalog | XMP |
  |---|---|
  | `pick = pick` | `xmpDM:pick = "1"`, `xmpDM:good = "True"` |
  | `pick = reject` | `xmpDM:pick = "-1"`, `xmpDM:good = "False"` |
  | `pick = unflagged` | `xmpDM:good` removed; `xmpDM:pick = "0"` only if the sidecar already has `xmpDM:pick` |
  | `rating` 0..=5 | `xmp:Rating` (always, also when rejected; replaces a legacy `-1`) |
  | `colorLabel` | `xmp:Label = "Red"/"Yellow"/"Green"/"Blue"/"Purple"` |
  | no `colorLabel` | `xmp:Label` removed only if it held one of those names or the legacy `"Pick"` |
  | visible tags | `lr:hierarchicalSubject` `Sieve\|<tag>` + `dc:subject` `<tag>` |
  `xmpDM` = `http://ns.adobe.com/xmp/1.0/DynamicMedia/`. Lightroom Classic ignores `xmp:Rating -1` (Bridge's
  reject) and shows `xmp:Label "Pick"` as an unknown colour label, which is what Sieve wrote before Phase 8c;
  both are migrated by the next write. Writes replace only `Sieve|*` items (and their `dc:subject` leaves), bump
  `xmp:MetadataDate`, and preserve every other field/namespace byte for byte (attribute or element form kept as
  found). Atomic (temp file + rename).
- Sidecar -> catalog (read/import), flag: `xmpDM:pick` (1 / -1 / 0) first, then `xmpDM:good` (True / False), then
  legacy `xmp:Rating -1` -> reject, `xmp:Label "Pick"` -> pick, else unflagged. `xmp:Rating 0..=5` -> rating
  (`-1` keeps the catalog's stars); label names -> `colorLabel`. `Sieve|*` keywords are not read back (analysis
  owns tags).
- Changes made by another app (Lightroom, Bridge) are picked up by `XmpSync::refresh_folders(folders)` (one
  `stat` per clean image; reads sidecars whose mtime moved; returns the changed image ids). Import calls
  `refresh_folder`; project open / window focus need a command wrapping `refresh_folders` (architect).
- Catalog concurrency: every catalog connection has a 5 s busy timeout (`db::BUSY_TIMEOUT`); XMP sync never holds
  a transaction across file I/O (file first, then one short `BEGIN IMMEDIATE` / autocommit update per image);
  an explicit save and an auto-sync pass are serialised (`io_lock`; the pass yields between images); a catalog
  error ends an explicit run with one error instead of N per-file failures.
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
- White balance picker (IPC v11, `sampleWhiteBalance`): mean of the 5x5 develop-source pixels (half-size decode,
  camera RGB, before WB) around a sensor-frame point (convert clicks with `unorientPoint` + crop mapping, as for
  masks); multipliers = G/R, 1, G/B of that mean; Temp/Tint through `camera::values_of_multipliers` (the DCP of
  `adjustments.profile` when resolved, else `wb::values_for` with `cam_xyz`: the same path as `asShot`), clamped to
  the slider ranges. Any sample pixel >= 64200/65535 in any channel => `invalid_argument` "...clipped...";
  any channel mean < 16 => "...too dark...". The UI commits `whiteBalance: custom` ("White Balance: Picker").
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

## UX additions (v8)

Thin command code implemented by the architect (bodies may be taken over by the listed owners):
repo functions in `db/repo.rs` (rust-engine-dev), `ml::store::set_burst_keeper` / `set_duplicate_tag` /
`pinned_keepers` and `ml::bursts::apply_pins` (vision-ml-dev), `XmpSync::write_dirty` (rust-engine-dev).

- Culling undo (frontend stack): before a culling write, `getCullSnapshot(ids)` and push it; undo =
  `restoreCullSnapshot(before)` (take a fresh snapshot first to allow redo). Covers rating/pick/label, including
  `applySuggestions` batches. Tag changes are undone with the inverse `setUserTag`.
- `applySuggestions(ids, onlyUnset)`: "Apply to unflagged only" = `onlyUnset: true` (keeps manual culls).
- Burst keeper: `setBurstKeeper(groupId, imageId)` updates `burst_groups.keeper_image_id` and `duplicate_burst`
  immediately (one tag rule: `ml::store::set_duplicate_tag`, shared with `write_bursts`) and pins the image in
  `burst_keeper_pins`; `rescore_all` makes a pinned member the keeper of whatever group it lands in (best
  `overall` if several), so suggestions (non-keeper demotion) follow after the kicked rescore. Choosing another
  keeper unpins the other members of that group.
- `UiPrefs` in `catalog_meta['ui_prefs']` (JSON, per catalog). Frontend does read-modify-write.
- `revealInFinder(path)`: `/usr/bin/open -R` with the path as a single argument (no shell, no plugin).
- `writeXmpAllDirty(folderId | null)`: explicit "Save all metadata" regardless of auto-sync.

## Lightroom parity + non-RAW sources (Phase 7b, IPC v9)

Goal (user requirement): Sieve replaces Lightroom for the user's edits. Every develop setting in the user's
394 sample sidecars round-trips through `crs:` and renders like Lightroom; what cannot be rendered yet is
preserved byte-for-byte and reported (`DevelopWarning`).

### Adjustment model
`ParametricAdjustments` gained groups (all `#[serde(default)]`; TS sees them optional, the backend always sends
them; `completeAdjustments` / `defaultAdjustments(format)` in `src/ipc/index.ts` fill them):
`toneCurve {parametric, point {master, red, green, blue}}`, `colorGrading {shadows, midtones, highlights, global:
{hue, saturation, luminance}, blending, balance}`, `calibration {red/green/blue {hue, saturation}, shadowTint}`,
`detail {sharpening {amount, radius, detail, masking}, noiseReduction {...6}}`, `effects {vignette {amount, midpoint,
roundness, feather, highlights, style}, grain {amount, size, roughness}}`, `blackAndWhite {enabled, mixer}`,
`crop {enabled, top, left, bottom, right, angle}`, `profile {cameraProfile, look {name, uuid, amount}}`.

Defaults are Lightroom's defaults for the source (`ParametricAdjustments::defaults_for(format)`; generated TS
constants `DEFAULT_ADJUSTMENTS`, `DEFAULT_ADJUSTMENTS_NON_RAW`): RAW = profile "Adobe Color" (DCP Adobe Standard +
look Adobe Color), sharpening 40/1.0/25/0, color NR 25/50/50, splits 25/50/75, blending 50; non-RAW = no profile,
sharpening 0, color NR 0. `neutral`/`hasEdits` compare against the image's format default. Stored JSON is overlaid on
the format default when read (`repo::get_adjustments`, history, presets), so pre-v9 rows load unchanged.

Fields masks: new `AdjustmentField`s `tone_curve, color_grading, calibration, sharpening, noise_reduction, vignette,
grain, black_and_white, crop, profile`. `AdjustmentField::DEFAULT_SYNC` / TS `DEFAULT_SYNC_FIELDS` = all but `crop`
(default of `MatchOptions.copyFields`; the Sync dialog's default selection). `reset_adjustments` resets every
group (incl. profile) to the image's format default.

Phase 7c (masks, IPC v10) plugs in as `ParametricAdjustments.masks` (+ field `masks`); see "Masks" below.

### crs mapping (`xmp/crs.rs` is the source of truth)
| Group | crs properties | Format |
|---|---|---|
| parametric curve | `ParametricShadows/Darks/Lights/Highlights` (-100..100); `ParametricShadowSplit/MidtoneSplit/HighlightSplit` | signed; splits plain |
| point curves | `ToneCurvePV2012`, `...Red/Green/Blue` (`rdf:Seq` of `"x, y"`), `ToneCurveName2012` (`Linear` if master identity else `Custom`) | Seq items |
| color grading | shadows = `SplitToningShadowHue/Saturation` + `ColorGradeShadowLum`; highlights = `SplitToningHighlightHue/Saturation` + `ColorGradeHighlightLum`; `ColorGradeMidtoneHue/Sat/Lum`; `ColorGradeGlobalHue/Sat/Lum`; `ColorGradeBlending`; balance = `SplitToningBalance` | hue/sat/blending plain, lum/balance signed |
| legacy split toning | `SplitToning*` without `ColorGradeBlending` -> blending 100 | read only |
| calibration | `RedHue RedSaturation GreenHue GreenSaturation BlueHue BlueSaturation ShadowTint` | signed |
| sharpening | `Sharpness` (0..150), `SharpenRadius` (0.5..3, `+1.0`), `SharpenDetail`, `SharpenEdgeMasking` | plain; radius signed decimal |
| noise reduction | `LuminanceSmoothing`, `LuminanceNoiseReductionDetail`, `LuminanceNoiseReductionContrast`, `ColorNoiseReduction`, `ColorNoiseReductionDetail`, `ColorNoiseReductionSmoothness` | plain |
| vignette | `PostCropVignetteAmount` (signed), `...Midpoint`, `...Roundness` (signed), `...Feather`, `...HighlightContrast`, `...Style` (1/2/3) | |
| grain | `GrainAmount`, `GrainSize`, `GrainFrequency` (= roughness) | plain |
| B&W | `ConvertToGrayscale` (True/False), `GrayMixerRed..Magenta` | signed |
| crop | `HasCrop` (True/False), `CropTop/Left/Bottom/Right` (0..1, un-oriented frame), `CropAngle` | plain |
| profile | `CameraProfile` + `<crs:Look>` struct (`Name, Amount, UUID, Parameters`) | see `crs::CAMERA_PROFILE` |

Write rules: every owned scalar is written on every develop write (Lightroom accepts the full set); curves need
`rdf:Seq` create/replace in `packet` (rust-engine-dev: `crs::encode_curves` is ready); the profile is written only
when it differs from the sidecar's (`crs::encode_profile`), `crs:CameraProfileDigest` removed then; `crs:Look`
struct copied from the installed look profile. Never written here: `crs:Table_*`, `CameraProfileDigest`, masks
(v10: `xmp/masks.rs`), retouch, lens, transform, `PointColors`, `CurveRefineSaturation`, `crd:*` -- preserved byte-for-byte.
Read rules: top-level properties only (the look's `crs:Parameters` never leak into the user's settings); missing
properties read as RAW defaults; out-of-range values clamp; unordered splits / inverted crop fall back to defaults;
malformed numbers/curves fail the develop import (ratings still sync). `crs::unsupported_warnings` fills
`RawImageEntry.developWarnings` at every read (masks incl. legacy gradient/brush corrections, retouch, lens
corrections, Upright/perspective, pre-2012 process version).

### Profiles and looks (`profiles/`)
Adobe DCPs (`/Library/Application Support/Adobe/CameraRaw/CameraProfiles`, `Adobe Standard/*.dcp`,
`Camera/<model>/*.dcp`) and look profiles (`.../CameraRaw/Settings/**/*.xmp`, `crs:PresetType="Look"`) are read
in place at runtime (never bundled, copied or redistributed; `SIEVE_CAMERA_PROFILES` / `SIEVE_LOOK_PROFILES`
override). On the dev Mac: Adobe Standard DCPs for all three sample cameras (Sony ILCE-7M4, Canon EOS M6 Mark II,
Fujifilm X-M5) + Camera Matching for the Sony/Canon; Adobe Raw looks incl. Adobe Color
(UUID B952C231111CD8E0ECCF14B86BAA7077, LookTable E1095149...) and Adobe Monochrome. The sample sidecars reference
looks by UUID/table MD5 only (their `crs:Table_*` attributes belong to masks), so looks resolve from the installed
Settings; a look embedded in a sidecar (`crs:Table_<md5>` at top level) is the fallback. Missing DCP/look ->
LibRaw-matrix base / no look + `profile_unavailable` / `look_unavailable` in `DevelopInfo.warnings`.
Pipeline order and DCP/table formats: `profiles/mod.rs`, `profiles/dcp.rs`, `profiles/table.rs` docs.
`list_profiles(id)` feeds the profile browser.

### Pipeline (preview + export; `develop/parity.rs`)
crop geometry -> camera RGB -> WB -> (calibration folded into) ForwardMatrix/DCP HueSatMap -> exposure (+ baseline)
-> noise reduction -> PV2012 tone (existing) -> DCP LookTable -> DCP tone curve -> look (table + parameters, at
amount) -> parametric curve -> point curves -> HSL / vibrance / saturation -> color grading -> B&W mix -> post-crop
vignette -> grain -> capture sharpening -> LUT -> output. Radii in full-res pixels scaled by the working scale.
Crop enabled: renders, `RenderOptions.region`, histograms, scene stats and exports cover the cropped frame;
`DevelopInfo` sizes stay uncropped.

### Non-RAW sources
- Formats: `ImageFormat` (was `RawFormat`) += `jpeg, heic, tiff, png` (extensions jpg/jpeg/jpe, heic/heif/hif,
  tif/tiff, png; magic bytes checked). Imported only with `ImportOptions.includeNonRaw` (default false).
- Companions: with `pairJpegWithRaw` (default true) a same-stem JPEG/HEIC next to a RAW (case-insensitive, same
  directory; JPEG preferred over HEIC) becomes the RAW's `companionPath` instead of an image (Lightroom's default).
  An already-imported sibling stays an image. TIFF/PNG never pair.
- Sidecars: `<file name>.xmp` (`IMG_1.JPG.xmp`) via `xmp::sidecar_path`; originals are never modified.
  Lightroom embeds XMP in JPEG/TIFF and ignores sidecars for them, so `<stem>.xmp` would buy no interop and would
  collide with the RAW sibling's sidecar. Embedded XMP (Lightroom-edited JPEGs) is read-only input when no sidecar
  exists (`raw::raster::embedded_xmp`).
- Develop: `raw::raster::decode_linear` (ICC-aware; sRGB / Display P3 / Adobe RGB / ProPhoto; unknown -> sRGB +
  `source_color_assumed`) -> `to_linear_image` (display-referred flag: no baseline exposure / base curve, so
  neutral = the original). As-shot WB reported as 6500 K / 0. Export uses the same decode at full size.


## Masks (Phase 7c, IPC v10)

Goal (user requirement): Sieve replaces Lightroom, so local adjustments use Lightroom's model
(`crs:MaskGroupBasedCorrections`) 1:1: a user's Lightroom masks import, render and round-trip; masks made in Sieve
open in Lightroom. Types: `src-tauri/src/ipc/masks.rs`; XMP: `xmp/masks.rs`; rendering + matte cache:
`develop/masks.rs`; AI: `ml/masking.rs`.

### What the user's sidecars contain (Jasmit Natalie Proposal, 394 XMPs, Lightroom Classic 8.1 / ACR 17.1)
- 50 sidecars with a top-level `crs:MaskGroupBasedCorrections`: 50 groups, 1 component each, all `Mask/Image`
  `MaskSubType="1"` ("Subject 1"), `MaskBlendMode="0"`, not inverted, `MaskValue="1"`, from the Adaptive: Subject
  presets (`CorrectionName` "Cool Soft" 48, "Pop" 1, "Warm Pop" 1; `CorrectionAmount` 1.0..1.51).
  Non-zero locals: `LocalClarity2012` 50 (-0.195..0.101), `LocalTexture` 50 (-0.149..0.201), `LocalTemperature` 49
  (-0.198..0.151), `LocalContrast2012` 5, `LocalExposure2012` 1 (0.0825); `LocalCurveRefineSaturation` = 100 on all;
  the PV2010 `LocalExposure/Brightness/Contrast/Clarity/Saturation/Sharpness/ToningHue/ToningSaturation` = 0.
- 45 more copies sit inside `<crs:Preset><crs:Parameters>` (the applied preset's own definition: no digest,
  `ReferencePoint="0.5 0.5"`, `ErrorReason="0"`). With the 50 top-level groups these are the "95 mask groups";
  only the top level is image state. No brush / gradient / range masks were used as masks.
- AI mattes: `crs:Table_<MaskDigest>` (50, one per component) = Adobe base-85 -> 16-byte header (`2,1,0,0`) + TIFF,
  8-bit gray tile, compression 52546 (JPEG XL), e.g. 1605x1332 at `Origin="0,296"` in a
  `WholeImageArea="0/1,0/1,1920/1,2880/1"` space (2880 px long edge). Decoded (ImageIO): soft subject mattes,
  stored un-oriented (an orientation-8 frame's matte is sideways). `ModelVersion` 251659306, `InputDigest` present.
- The other 115 `crs:Table_*` belong to `crs:RetouchAreas` (20 sidecars, generative/heal spots: `pm_patch`,
  `pm_patch_mask`, `pm_patch_variation`, `IngestInfo`), whose masks are `Mask/Ellipse` (1) and `Mask/Paint` (23
  strokes, 1322 `d x y` dabs + 1 `r` item): out of scope (retouch), preserved.
- Brush radius scale verified from those strokes: `crs:Radius` x sensor width + ~9 px = Lightroom's `pm_target_*`
  box on 12/12 strokes (Sony 7008 px, Canon 6960 px, Fuji 6240 px widths).

### Model
`ParametricAdjustments.masks: MaskGroup[]` (in the stored adjustments JSON; history/undo/presets/copy/sync for free).
A group = local slider set (`LocalAdjustments`, UI units, offsets added to the global sliders where the mask is 1)
+ ordered components combined with add (max) / subtract (`acc*(1-v)`) / intersect (`acc*v`); component value =
`opacity * (inverted ? 1 - shape : shape)`; group weight = mask x `amount` (0..=2). Shapes: brush strokes, linear,
radial, luminance range, colour range, AI (`subject | sky | background | people{parts} | object{region} |
landscape{category} | other`), `unsupported` (Lightroom kinds Sieve does not model, preserved verbatim). Ids are
Lightroom SyncIDs (32 hex), which the XMP writer uses to preserve unmodelled attributes.

**Frame.** All geometry is in the sensor frame: normalized, un-oriented, uncropped (Lightroom's convention; crop
`crs:Crop*` is in the same frame). The UI converts pointer positions: displayed (cropped, oriented) -> uncropped
oriented (crop rect + angle) -> `unorientPoint(p, orientation)`. Brush radius = fraction of the sensor width.

### Rendering
Masks are evaluated per output pixel in the sensor frame, so preview, 1:1 regions and full-res export use one
code path at any resolution (`develop/masks.rs` module docs: component formulas, pipeline seam steps 1-6). Local
parameters become per-pixel offset planes (`LocalPlanes`) that the existing stages add to their global value;
per-group curves / colour tint blend after the global point curves. Mask weights are cached per (image, geometry,
components) so local slider drags only rebuild the planes. Range masks read a guide image after global WB +
exposure. Unmasked images pay nothing (`LocalPlanes::build` returns `None`).

Implementation (Phase 7c integration): `develop/masks/render.rs` resolves + loads the mattes of a render
(`ResolvedMattes`, Sieve mattes refined once against a neutral sensor-frame render), evaluates / caches the group
weights (`local_planes`) and is called by `DevelopCache::{render, render_image}` (preview, scene stats) and by the
export (`export_one`, after `compute_missing` has run the Segmenter). `develop/local.rs` (`LocalOps`) is the
per-pixel side inside `pipeline::develop`: stage A local WB (matrix), stage C Shadows/Highlights/Clarity/Texture/
Dehaze amounts, stage D local tone as an input gain before the global LUT (exact global slider response), Hue /
Saturation in Oklab, group curves/colour after the LUT (display-linear ProPhoto), stage E Sharpness/Noise.
Moire/Defringe are stored and round-tripped but not rendered. Scales fitted to Camera Raw: `docs/decisions.md`.
Tools: `examples/mask_render.rs` (renders the user's masked frames with/without masks, ΔE vs Camera Raw),
`examples/mask_bench.rs` (latency through `DevelopCache`).

### AI mattes
- Stored in `mask_cache` + PNG files (`<cacheDir>/masks/<id>/<digest>.png`), with sensor-frame bounds; sampled
  bilinearly at render resolution.
- Lightroom mattes (origin `lightroom`) are decoded at XMP read from the sidecar's table and render exactly as
  Lightroom's selection, whatever their kind (no model needed).
- Sieve mattes (origin `sieve`) come from `compute_ai_mask` (`ml::masking::Segmenter`, models chosen by
  vision-ml-dev, cached per image + `AiMask::cache_kind` + model version, invalidated when the original changes).
  A component without a digest (pasted, synced, preset, or Lightroom mask without a table) uses the cached Sieve
  matte of its kind if any, else renders empty with `ai_mask_needs_update` ("Update" in the UI). Export computes
  missing mattes before rendering.

### XMP
Read/write rules are in `xmp/masks.rs` (module docs; mapping tables `LOCAL_SCALARS`, `LOCAL_CURVES`, `BLEND_MODES`,
`AI_SUBTYPES`, `PERSON_PARTS`, `LANDSCAPE_CATEGORIES` with an evidence level each). Summary:
- Unchanged masks leave the sidecar's element byte-for-byte; changed ones are regenerated with each item's
  unmodelled attributes carried over by SyncID; unchanged items re-emitted verbatim.
- Lightroom's AI tables are kept while their component exists (and removed when it is deleted); Sieve never writes
  its own mattes into sidecars. Sieve-made AI components are written in Lightroom's preset form (kind + reference
  point, no digest), which Lightroom recomputes with its own model on open.
- `images.masks_pending_import` (migration 0010) protects sidecar masks read before v10 from being overwritten by
  an empty list until they are imported.
- Lightroom-verified: group/component attribute set, subject subtype, add blend code, local scalar scale
  (1/100 for percentage sliders, observed), matte table format and placement, brush radius/dab format.
  Provisional (no sample): other AI subtypes / person-part / landscape codes, subtract/intersect codes, local
  exposure (EV/4) and hue (deg/180) scales, range-mask structure, brush erase/feather encoding, per-mask curves.
  Unknown codes read as `other` / `unsupported` and are preserved, so provisional mappings can only affect masks
  created in Sieve.

### Frontend API summary (frontend-dev)
- Open the Masks panel (Shift+W): `listMasks(id)` -> groups + AI status; `getMaskCapabilities()` once per session
  (disable AI buttons with the reason as tooltip).
- Create: "Add" menu -> brush / linear / radial / luminance / colour / subject / sky / background / people /
  object (/ landscape): `newMaskGroup("Mask N", [newMaskComponent(shape, "Brush 1")])`. AI: `computeAiMask(id,
  {target, referencePoint, force: false})` -> set `shape.digest` to the result. People: `detectPeople(id)` ->
  pick person(s) + parts (checkboxes from `capabilities.personParts`) -> one component with
  `{kind: "people", parts}` + `referencePoint`. Background = `{kind: "background"}`.
- Components: add / subtract / intersect (Lightroom "Add", "Subtract" buttons; Alt-click = subtract), invert,
  opacity, show/hide, rename, delete, reorder; group: amount, show/hide, rename, duplicate, invert, delete.
- Editing: while dragging (brush stroke, gradient handle, local slider) send `renderPreview(id,
  {...adjustments, masks: live}, opts)` as for global sliders (draft size while dragging); commit with
  `saveMasks(id, masks, "Mask: <what>")` on release (same-label saves coalesce).
- Overlay (O toggles, Shift+O cycles colour; "Show overlay" checkbox): `renderMaskOverlay(id, liveAdjustments,
  {groupId, componentId}, {maxEdge, region})` -> grayscale JPEG with the same frame as the main render; composite
  as a CSS luminance mask over a colour layer (Lightroom styles: colour overlay, on black/white, B&W). Hovering a
  component shows its own overlay (`componentId`); the selected group shows the combined mask.
- Tools and shortcuts (Lightroom Classic): K brush (`[` / `]` size, Shift+`[` / `]` feather, Alt = eraser,
  A auto-mask), M linear gradient, Shift+M radial gradient, Shift+J colour range, Shift+Q luminance range,
  O overlay, Shift+O overlay colour, H show/hide pins, Delete remove the selected component, Enter / Esc done,
  Cmd+Z undo (history covers masks). Brush cursor radius on screen = `radius x sensorWidth` in displayed px
  (from `DevelopInfo.fullWidth/Height` + orientation).
- Warnings: `ai_mask_needs_update` (offer "Update all" = `computeAiMask` for each `needs_update` component, then
  save the digests), `masks_unsupported` (Lightroom components kept but not rendered).

## Catalog (SQLite)

Location: `<app_data_dir>/catalog.sqlite` (override with `SIEVE_CATALOG=/path`). WAL, `foreign_keys=ON`,
migrations tracked by `PRAGMA user_version`.

| Table | Purpose |
|---|---|
| `catalog_meta` | `shoot_type` (default for new projects), `burst_window_ms`, `auto_analyze`, `xmp_auto_sync` (+ `xmp_auto_sync_user_set`, v12), `keeper_rule` (JSON `KeeperRule`, v12), `cull_thresholds.<shoot_type>` (JSON), `ui_prefs` (JSON `UiPrefs`) |
| `projects` | one shoot (v12): name, `cover_image_id` (NULL = automatic), `shoot_type`, `workflow_step`, `created_at`, `last_opened_at` |
| `folders` | imported roots; `project_id` (v12, every folder in exactly one project; cascade on project delete) |
| `images` | one row per image (RAW or, since v9, JPEG/HEIC/TIFF/PNG): identity, `format`, camera, EXIF, rating/pick/label, burst group, XMP sync state (`xmp_dirty`, `meta_updated_at`, `xmp_synced_at`, `xmp_mtime_ms`, `xmp_error`), `scene_id`, `scene_anchor`, `companion_path` (paired camera JPEG/HEIC), `develop_warnings` (JSON `DevelopWarning[]` from the last XMP read), `masks_pending_import` (v10: sidecar masks not imported yet), `missing_since_ms` (v11: original found missing, see below) |
| `thumbnails` | status pending/ready/failed, `path` (512 px), `preview_path` (2048 px), dims, `error` (pixels are files, not blobs) |
| `image_tags` | `(image_id, tag)` PK, source auto/user, confidence, suppressed |
| `quality_scores` | culling-engine scores per image + `suggested_rating` / `suggested_pick` (derived; rewritten on rescore) |
| `image_analysis` | per-image analysis status (queued/done/failed), model version, error, `phash` (u64 as i64), `faces_json` (`FaceInfo[]`), `metrics_json` (`ml::ImageMetrics`) |
| `burst_groups` | time/similarity clusters, optional keeper |
| `burst_keeper_pins` | images the user chose as burst keepers (survive regrouping) |
| `adjustments` | `ParametricAdjustments` JSON + process version, `neutral`, `history_entry_id` (cursor); `xmp_synced_at` unused |
| `adjustment_history` | per-image snapshots (label, params JSON, created/updated); v13: `source` (who produced it) and `batch_id` (edit batch that wrote it) |
| `presets` | `group_id` (style group, v12), name (unique per group, NOCASE), params JSON, fields JSON, `source_format`, `settings_json` + `setting_keys_json` (imported crs: settings), `supports_amount`, `warnings_json` |
| `style_groups` / `style_profiles` | style library (v12): groups per imported source folder + built-ins 1 "User Presets" / 2 "LUTs"; looks / DCPs (read in place) / LUTs (library copies) |
| `edit_batches` / `edit_batch_items` | undoable multi-image edits (v12): per image `before_json` / `after_json` (+ scene); v13: `before_source` / `before_batch_id` (restored by undo), `review_reason` / `reviewed_at` (needs a look); v15: `edit_batch_bases` (batch, representative, base batch): an apply made from settings another batch wrote blocks that batch's undo (IPC v17) |
| `style_models` / `style_features` | personal style model blobs + validation; per-image features (v12, owned by `ml::style`) |
| `export_presets` | user export presets: name (unique, NOCASE), `ExportSettings` JSON (built-ins are in code) |
| `export_jobs` | one per `export_images`: state, resolved output dir, settings JSON, counters, timestamps, `project_id` (v12: all images in one project, else NULL) |
| `export_items` | per (job, seq): image, status pending/done/failed/skipped, output path, error |
| `scenes` | lighting scenarios: derived folder / started / ended, method auto/manual (members via `images.scene_id`); v12 edit plan: `representative_id/_source/_reason`, `applied_at_ms`, `applied_params_json`, `applied_batch_id`; v13: `skipped`, `applied_covered_json` (ids the last apply considered) |
| `scene_features` | per-image appearance features for detection (JSON, `version`, `computed_at`) |
| `mask_cache` | AI mattes per (image, digest): kind, origin lightroom/sieve, model version, input digest, PNG path under `<cacheDir>/masks/`, size, sensor-frame bounds, coverage (v10) |

Filter-bar indexes (schema v11, `0011_hardening.sql`): `idx_images_folder_pick_rating (folder_id, pick, rating)`
(whole-catalog `get_filter_counts` groups by it: ~7 ms at 50k images, `examples/grid_bench.rs --plans`),
partial `idx_image_tags_live (tag, image_id) WHERE suppressed = 0` (tag counts / filters), partial
`idx_images_missing (folder_id) WHERE missing_since_ms IS NOT NULL` (`missing` facet / `missingOnly`).

### Missing originals (v13)
`images.missing_since_ms` (`RawImageEntry.missingSinceMs`) = when an access first found the original gone.
Set (keeping the first time) by: `render_preview` / `get_develop_info` / `sample_white_balance` failing with
`file_missing` (command layer, `note_if_missing`), export items failing with a missing reason (`export`), sidecar
writes (`xmp::store::mark_failed`), thumbnail extraction (`repo::record_extraction`) and a re-import of the folder
(`repo::import_folder` stats every catalogued file of the folder). Cleared by: a successful extraction, sidecar
write or export, `get_develop_info` while the file is back, a re-import that finds it, and `relocate_folder`.
Helpers: `repo::set_original_missing`, `repo::note_access_failure` (reason-prefix based). `relocate_folder`
(`repo::relocate_folder`, atomic) finds each image at its old relative path under the new root, else by unique
file name anywhere below it; it refuses a root holding none of the photos or one that is another catalog folder,
forgets the moved images' develop sources and re-queues thumbnails that had failed as missing.

### Catalog health (v13)
`CatalogState.health` = `db::health_state(path)`: the launch check's result (`ok` / `read_only` / `replaced`, see
the `db` module docs), a user-facing message, `<catalog>.bak-1..3` and whether a restore is staged
(`<catalog>.restore`, written by `restore_catalog_backup` = `db::stage_restore`; swapped in by the next launch's
first open, the replaced file kept as `.corrupt-<ms>`). Clean shutdown: `RunEvent::Exit` (lib.rs) writes
`<catalog>.clean` (`db::mark_clean_shutdown`, never for a read-only catalog); the next first open removes it and,
if it was present, the file has the SQLite header and no restore was applied, skips `quick_check` (124 ms at
50k images). Backups follow the usual rules either way.

### Projects (v14)
A project (`db::projects`) is one shoot: `projects` row + its `folders` (`folders.project_id`). Import decides the
project with `ImportTarget`: `Auto` (`import_folder(path, null)`: the folder's project, or a new one named after
the folder), `New` (`create_project`: new project with name / shoot type, unless the folder is already known),
`Existing(id)` (`import_folder(path, id)`, "Add folder"). A path equal to or inside a catalog folder reuses that
folder row (and so its project). Migration 0012 creates one project per existing folder (same id).

Scoping: `FolderScope::resolve(folderId, projectId)` turns the pair into a folder-id set (`None` = whole catalog,
empty = nothing) used by `repo::filter_counts`, `repo::list_burst_groups`, `scene::store::{list_scenes,
detection_frames, replace_scenes}` and `scene::workflow::edit_plan` (predicates with inlined integer ids);
`ImageQuery.projectId` becomes `i.folder_id IN (SELECT id FROM folders WHERE project_id = ?)`. Scenes never span
folders (detection groups per folder), so they never span projects. Project counts (`list_projects`) are one grouped
query over projects ⟕ folders ⟕ images ⟕ quality_scores with the keeper rule inlined (mirror of
`KeeperRule::is_keeper_values`). Removal deletes the project row; folders -> images -> every per-image table
cascade; empty burst groups and scenes are deleted; the command removes the images' cached thumbnails/previews
(image ids can be reused by the next import) and forgets their develop sources. Files on disk are never touched.

The guided-workflow step (`projects.workflow_step`) is per project; the edit plan, keepers and the Export step's
jobs (`export_jobs.project_id`) are per project too.

### Edit step state (v15)
Per-photo state is never stored as a flag that must be cleared: it is derived from the history entry the image's
cursor points at (`develop::batches::edit_states`). `adjustment_history.source` is set from the label on insert
(`history::source_for_label`: Original / Read from XMP -> sidecar, Apply to Scene / Match Scene -> scene_apply,
Auto Edit (My Style) -> auto_style, Paste / Sync / Paste from Previous -> pasted, everything else -> user);
`commit_recorded` stamps the batch's entries with `batch_id`; `batches::undo` re-stamps the restored entries with
the item's `before_source` / `before_batch_id`. Neutral settings are always `none`; settings without history are
`sidecar`. Needs a look = cursor entry from a scene apply whose batch item has `review_reason` and no
`reviewed_at`. So any edit clears it, per-image undo brings it back, and nothing is lost on reload.

Plan (`scene::workflow::edit_plan`): per scene `skipped`, `minor` (<= `MINOR_SCENE_MAX_KEEPERS` keepers),
`appliedIds` / `needsReviewIds` / `autoEdited` from the states, `unappliedKeeperIds` = keepers outside
`applied_covered_json` (representative, targets, frames left alone and `excludeIds` of the last apply) with
source none / auto_style. `outdated` = unassigned keepers or unapplied keepers in a scene that is not skipped.
`replace_scenes` carries `skipped` and the coverage with the rest of the plan state. Applies run per scene in steps
of 32 targets and check `SceneApplyControl` (managed state, `cancel_scene_apply`) between steps; finished scenes
are committed as one batch.

## Keeping the contract in sync
- `cargo run`/`pnpm tauri dev` (debug) regenerates `src/ipc/bindings.ts`.
- `cargo test` fails (`bindings_are_up_to_date`) if the committed bindings are stale;
  fix with `UPDATE_BINDINGS=1 cargo test bindings`.
- Log every contract change in `docs/ipc-changelog.md`.

## Packaging (Phase 8)

`pnpm tauri build` (Apple Silicon, macOS 15+) produces
`src-tauri/target/release/bundle/macos/Sieve.app` (~58 MB) and `bundle/dmg/Sieve_<version>_aarch64.dmg` (~35 MB).
`beforeBuildCommand` = `pnpm build && bash scripts/fetch-models.sh --culling`.

- **Native libraries** (`build.rs`, macOS): LibRaw (`libraw_r`) and TurboJPEG are found in Homebrew as before,
  then copied with their non-system transitive deps (`libomp`, `libjpeg.8`, `liblcms2.2`) into
  `src-tauri/target/sieve-stage/Frameworks/`, install ids / references rewritten to `@rpath/<name>`, ad-hoc
  re-signed, and linked from there. `tauri.conf.json` `bundle.macOS.frameworks` lists the staged files
  (build.rs fails with a clear message if Homebrew's dependency graph changes); tauri-build copies them to
  `target/Frameworks` and adds `-rpath @executable_path/../Frameworks`, the bundler ships them in
  `Contents/Frameworks`. build.rs also copies them to `target/<profile>/Frameworks` so test/example binaries
  (`target/<profile>/{deps,examples}`) resolve the same rpath. No `/opt/homebrew` load command remains in the
  bundle. Homebrew bottles set the minimum OS (currently 15.0 = `minimumSystemVersion`).
- **ONNX Runtime** is statically linked by `ort` (no dylib); the CoreML EP works from the bundle.
- **Models**: the culling models (~27 MB: SCRFD, 2d106, open/closed eye, FaceMesh) are staged by build.rs into
  `target/sieve-stage/models/` and bundled as `Contents/Resources/models` (resource map in tauri.conf.json;
  release bundle builds fail if they are missing). The AI-mask models (~560 MB) are not bundled: release builds
  read them from `<app_data_dir>/models` (`~/Library/Application Support/com.sieve.app/models`), where
  `model_fetch::link_bundled` symlinks the bundled face models at startup (people / part masks need them) and
  `model_fetch::fetch` downloads the segmentation set (system `curl`, resumable `.part`, SHA-256 from
  `models/checksums.sha256`, rename only after verification). Until installed, AI masks report "model file ...
  not installed"; `scripts/fetch-models.sh --dest <that dir>` pre-seeds them. Debug builds and `SIEVE_MODELS`
  keep using a single directory.
- **In-app download (IPC v12)**: managed state `model_fetch::ModelDownloads` (lib.rs, dir = the segmenter's
  models dir: `<app_data_dir>/models` release, `src-tauri/models` dev, or `SIEVE_MODELS`). `download_models`
  runs `model_fetch::fetch` on a `model-download` thread (one at a time), emits `ModelDownloadProgress`
  (throttled ~5/s + one per completed file) and exactly one `ModelDownloadFinished`; `cancel_model_download`
  kills curl (the `.part` is kept and resumed next time). `model_downloads_status` is a size check per file.
  No restart needed: `Segmenter::capabilities` / `available` check the model files on every call and sessions
  load lazily, so `get_mask_capabilities` flips to available as soon as the verified files are renamed in.
- **Signing**: ad-hoc (`signingIdentity: "-"`), hardened runtime, `Entitlements.plist` =
  `com.apple.security.cs.disable-library-validation` only (required: without it dyld rejects the ad-hoc
  Frameworks under the hardened runtime — verified). Not sandboxed (user-chosen folders, sidecars, Adobe
  profiles read in place). Adobe files are never bundled. Developer ID signing + notarization need an Apple
  Developer account (future; Tauri notarizes when `APPLE_*` env vars are set).
- **Icon**: `src-tauri/icons/sieve.svg` → `pnpm tauri icon src-tauri/icons/sieve.svg` (then drop android/ios).
- **Smoke test**: `cargo build --release --example bundle_smoke && scripts/bundle-smoke.sh <copied samples>
  [work_dir]` — checks load commands and signature, launches `Contents/MacOS/sieve` with a clean env and scratch
  catalog (migrations, dylibs mapped from `Contents/Frameworks` via `lsof`; the hardened runtime ignores
  `DYLD_PRINT_LIBRARIES`), then runs import → thumbnails → CoreML culling → render → full-res JPEG export from
  inside a copy of the bundle.
