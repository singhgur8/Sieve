# Phase 1 — Tauri v2 scaffold + IPC contracts + catalog schema

## Context
Repo is empty (only CLAUDE.md, .gitignore, .gitattributes, `.claude/agents/`). Phase 1 must produce a fixed, typed Rust↔TS contract and a catalog schema so the specialist agents (rust-engine-dev, vision-ml-dev, frontend-dev) can later work in parallel against it. No RAW decoding or ML in this phase.

Decisions (confirmed): install Rust via rustup · pnpm via corepack · **tauri-specta** generates TS types + typed command wrappers from Rust · product name **LumenRAW**, bundle id `com.lumenraw.app`.

## Step 0 — Toolchain
- `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y` (stable, aarch64-apple-darwin). Xcode CLT already present.
- `corepack enable pnpm`.

## Step 1 — Scaffold
- `pnpm create tauri-app` (template `react-ts`, manager pnpm, non-interactive) into scratch dir, then move contents into repo root (repo is non-empty).
- Tailwind v4 via `@tailwindcss/vite`; add `lucide-react`; add `tauri-plugin-dialog` (folder picker for import).
- Set `productName: LumenRAW`, identifier `com.lumenraw.app` in `tauri.conf.json`.

## Step 2 — Rust layout (`src-tauri/src/`)
```
main.rs            -> lumenraw_lib::run()
lib.rs             builder, plugins, managed state, specta command/event registration;
                   in debug builds exports ../src/ipc/bindings.ts (BigInt -> number)
ipc/mod.rs
ipc/types.rs       all contract types (below), #[derive(Serialize, Deserialize, specta::Type)]
ipc/commands.rs    #[tauri::command] #[specta::specta] handlers (thin; call db::repo)
ipc/events.rs      ImportProgress, ThumbnailReady, AnalysisProgress (defined now, emitted later)
ipc/error.rs       AppError { kind: ErrorKind, message } — serializable, thiserror
db/mod.rs          open (WAL, foreign_keys=ON), migrate via PRAGMA user_version
db/schema.rs       include_str!("../../migrations/0001_init.sql")
db/repo.rs         catalog queries (insert/list/filter/update)
raw/mod.rs         format detection only: extension + magic bytes
                   (ARW = TIFF "II*\0"; RAF = "FUJIFILMCCD-RAW"; CR3 = ISO-BMFF "ftypcrx ")
ml/mod.rs          empty placeholder module for vision-ml-dev
```
Crates: tauri 2, tauri-plugin-dialog, serde/serde_json, specta + tauri-specta + specta-typescript (pinned matching rc versions), rusqlite (`bundled`), thiserror, walkdir. DB held as `Mutex<Connection>` in managed state for now (pool later if needed).

## Step 3 — Contract types (`ipc/types.rs`, all serde `camelCase`)
- `RawFormat { Arw, Raf, Cr3 }` · `CameraMake { Sony, Fujifilm, Canon, Other }` · `SensorLayout { Bayer, XTrans, Unknown }` (Fuji can be either → needed for demosaic choice later).
- `CaptureMeta { capturedAtMs: Option<i64>, iso, shutterSeconds, aperture, focalLengthMm, lens: Option<String> }` (all Option; filled in Phase 2).
- `ThumbnailState` tagged enum: `Pending | Ready { path, width, height } | Failed { reason }` — thumbnails are files in app cache dir, not DB blobs.
- `CullTag { Blink, MissedFocus, MotionBlur, CreativeBlur, Underexposed, Overexposed, DuplicateBurst }` (Overexposed added to cover "exposure clipping").
- `CullTagEntry { tag, source: TagSource { Auto, User }, confidence: f32, suppressed: bool }` — user can dismiss an auto tag without losing it (non-destructive).
- `QualityScore { overall, faceSharpness?, globalSharpness, eyesOpen?, composition?, faceCount, exposure: ExposureStats { clippedHighlightsPct, clippedShadowsPct, meanLuma }, modelVersion }` (0..1 normalized scores).
- `ShootType { Wedding, Portrait, Sports, Event, Landscape, General }`.
- `PickFlag { Pick, Reject, Unflagged }`, `rating: u8` 0–5, `ColorLabel` enum (LR-compatible).
- `RawImageEntry { id, folderId, path, fileName, format, camera: { make, model, sensorLayout }, capture, width?, height?, orientation?, fileSize, fileMtimeMs, thumbnail, rating, pick, colorLabel?, burstGroupId?, tags: Vec<CullTagEntry>, quality: Option<QualityScore>, hasEdits }`.
- `ParametricAdjustments` — names/ranges mirror Adobe `crs:` Process 2012 for 1:1 XMP mapping in Phase 5:
  `processVersion`, `whiteBalance: AsShot | Custom { temperatureK, tint }` (tint −150..150), `exposure` (−5..5 EV), `contrast, highlights, shadows, whites, blacks, texture, clarity, dehaze, vibrance, saturation` (−100..100), `hsl: { hue, saturation, luminance }` each `HslChannels { red, orange, yellow, green, aqua, blue, purple, magenta }`, `lut: Option<LutRef { path, amount }>`. `Default` = all neutral.
- `ImageQuery { includeTags, excludeTags, tagMatch: Any|All, pick?, minRating?, burstGroupId?, folderId?, sort: CaptureTime|FileName|Quality, offset, limit }` → `ImagePage { items, total }`.
- `CatalogState { catalogPath, imageCount, shootType, burstWindowMs (default 1500), folders: Vec<FolderEntry>, tagCounts: Vec<TagCount> }`.

## Step 4 — IPC commands (all `async`, `Result<T, AppError>`)
| Command | Args | Returns | Phase 1 body |
|---|---|---|---|
| `get_catalog_state` | – | `CatalogState` | real |
| `set_shoot_type` | `ShootType` | `()` | real |
| `set_burst_window` | `ms: u32` | `()` | real |
| `import_folder` | `path, ImportOptions { recursive }` | `ImportSummary { added, skipped, unsupported }` | real: walk dir, detect format, insert rows w/ thumbnail `Pending`. No decode. |
| `list_images` | `ImageQuery` | `ImagePage` | real (tag filter via SQL) |
| `get_image` | `id` | `RawImageEntry` | real |
| `set_rating` / `set_pick` / `set_color_label` | `ids: Vec<ImageId>, value` | `()` | real |
| `set_user_tag` | `ids, tag, present: bool` | `()` | real |
| `get_adjustments` / `save_adjustments` | `id` / `id, ParametricAdjustments` | adj / `()` | real (JSON column) |
Events (typed via tauri-specta): `ImportProgress { done, total }`, `ThumbnailReady { imageId, path }`, `AnalysisProgress { done, total }`.

## Step 5 — SQLite schema (`src-tauri/migrations/0001_init.sql`)
Catalog at `app_data_dir/catalog.sqlite`, WAL.
- `catalog_meta(key PK, value)` — shoot_type, burst_window_ms.
- `folders(id, path UNIQUE, added_at)`
- `images(id, folder_id FK, path UNIQUE, file_name, format CHECK IN ('arw','raf','cr3'), camera_make, camera_model, sensor_layout, lens, captured_at_ms, iso, shutter_s, aperture, focal_length_mm, width, height, orientation, file_size, file_mtime_ms, rating DEFAULT 0, pick DEFAULT 'unflagged', color_label, burst_group_id FK, imported_at)` + indexes on captured_at_ms, folder_id, burst_group_id.
- `thumbnails(image_id PK FK, status, path, width, height, error, extracted_at)`
- `image_tags(image_id FK, tag, source, confidence, suppressed, PK(image_id, tag))` + index on tag.
- `quality_scores(image_id PK FK, overall, face_sharpness, global_sharpness, eyes_open, composition, face_count, clipped_highlights_pct, clipped_shadows_pct, mean_luma, model_version, analyzed_at)`
- `burst_groups(id, started_at_ms, ended_at_ms, keeper_image_id)`
- `adjustments(image_id PK FK, params_json, process_version, updated_at, xmp_synced_at)` — JSON so adding sliders needs no migration.
All FKs `ON DELETE CASCADE`. Scenes table deferred to Phase 7.

## Step 6 — Frontend (minimal, contract smoke test only)
- `src/ipc/bindings.ts` (generated, committed) + `src/ipc/index.ts` re-export.
- `App.tsx`: shows `CatalogState`, "Import folder" button (dialog plugin) → `import_folder` → plain list from `list_images`. Real grid is Phase 4.

## Step 7 — Docs
- `docs/architecture.md`: file layout, ownership map (per `.claude/agents/`), IPC table, schema.
- `docs/ipc-changelog.md`: v1 entry.
- Tick Phase 1 in CLAUDE.md; add build commands.

## Verification
- `cargo test` in `src-tauri/`: migration applies on in-memory DB; format detection (magic bytes fixtures); import→list round trip; tag include/exclude filter; adjustments JSON round trip; **bindings freshness test** (export to temp, diff against committed `src/ipc/bindings.ts`).
- `pnpm tsc --noEmit` and `pnpm build`.
- `pnpm tauri dev`: app launches, import a folder of dummy `.ARW/.RAF/.CR3` files (magic-byte stubs) → counts shown, rows visible; `sqlite3 catalog.sqlite` spot-check.
- Final message: file layout + IPC signature summary for your review before any image processing work.
