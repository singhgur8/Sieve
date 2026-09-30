# LumenRAW Roadmap

Source of truth for autonomous execution (see "Autonomous Orchestration" in `CLAUDE.md`).
The orchestrator works top to bottom: the first unchecked task is the next task.
A phase is done only when every task is checked **and** its acceptance checks pass.

Conventions
- Branch per phase: `phase-N-<slug>`; fast-forward merge to `main` after the QA gate.
- `Owner` = agent in `.claude/agents/`. Contract/schema changes always go through `architect` first.
- Sample RAWs: `/Users/gurjotsingh/Pictures/test RAWS` (788 Sony `.ARW`, ~20 GB). **Read-only.**
  Copy subsets into `test-data/` (gitignored) for anything that writes (XMP, exports).
  Set `LUMENRAW_SAMPLES` to point elsewhere; paths change per project.
- Baseline gate for every phase (run by `qa-engineer`):
  `cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check` and `pnpm build`.

---

## Phase 1 — Scaffold + IPC contracts ✅
- [x] Tauri v2 + React/TS/Tailwind scaffold, typed IPC via tauri-specta, SQLite catalog v1.

## Phase 2 — Ingest: thumbnails + metadata ✅
- [x] **Contract** (architect): add fields/commands needed for extraction progress & cache dir; asset-protocol scope for thumbnail cache in `tauri.conf.json`; log in `docs/ipc-changelog.md`.
- [x] **LibRaw binding** (rust-engine-dev): `brew install libraw`; pick FFI crate (evaluate `libraw-rs`, `rsraw`, or own `bindgen` wrapper); record choice in `docs/decisions.md`.
- [x] **Embedded preview extraction** (rust-engine-dev): fastest path first (embedded JPEG from ARW/RAF/CR3, via LibRaw `unpack_thumb` or direct container parsing); write resized JPEG (long edge 512) + preview (long edge 2048) to `<app_cache>/thumbs/`; orientation applied.
- [x] **EXIF → catalog** (rust-engine-dev): capture time incl. sub-seconds, ISO, shutter, aperture, focal length, lens, model, dims, orientation; Fuji `sensor_layout` from model.
- [x] **Pipeline** (rust-engine-dev): background rayon pool after `import_folder`; emits `importProgress` + `thumbnailReady`; bounded memory (streaming, no whole-set buffering); status `failed` with reason on error.
- [x] **Frontend hook-up** (frontend-dev): grid-less list shows thumbnails via `convertFileSrc`; progress bar.
- [x] **QA gate**.
  - Acceptance: importing the full sample folder completes; 788/788 thumbnails `ready`; EXIF populated (capture time non-null for all); throughput ≥ 30 files/s on this Mac; peak RSS does not grow with file count (compare 100 vs 788 files); Fuji/Canon paths covered by unit tests with synthetic containers.

## Phase 3 — Culling engine ✅
- [x] **Contract** (architect): analysis commands (`analyze_images`, `cancel_analysis`), per-shoot-type thresholds, suggested rating/pick fields.
- [x] **Models** (vision-ml-dev): download SCRFD (face det) + 5/106-pt landmark ONNX into `src-tauri/models/` (gitignored) with a fetch script + checksums; `ort` with CoreML EP, CPU fallback.
- [x] **Metrics** (vision-ml-dev): face boxes; EAR blink per face; Laplacian variance on eye/face crop vs full frame; motion vs creative blur heuristic (directional blur + subject sharp elsewhere); exposure clipping + mean luma.
- [x] **Bursts** (vision-ml-dev): group by capture-time gap ≤ `burst_window_ms` + perceptual hash distance; pick burst keeper; tag others `duplicate_burst`.
- [x] **Scoring** (vision-ml-dev): `QualityScore` + tags + suggested stars/pick, weighted by `ShootType` (Wedding/Portrait: eyes open + eye sharpness dominate; group shots require every face open-eyed).
- [x] **Ground truth** (orchestrator): view ~50 sample previews, label blink / sharp / soft / keeper in `test-data/labels.json`.
- [x] **QA gate**.
  - Acceptance: full sample set analyzed; avg < 50 ms/image at preview size; precision ≥ 0.8 for `blink` and `missed_focus` on the labeled set (report recall too); burst groups sensible on spot-check.

## Phase 4 — Culling UI + XMP write
- [ ] **Contract** (architect): XMP sync commands (`write_xmp`, `read_xmp`, auto-sync setting), keyboard-driven batch ops.
- [ ] **XMP sidecars** (rust-engine-dev): mapping — reject → `xmp:Rating -1`; stars → `xmp:Rating 0–5`; pick → `xmp:Label "Pick"` (color labels otherwise via `xmp:Label`); auto tags → `lr:hierarchicalSubject` `LumenRAW|<tag>` + `dc:subject`. Preserve unrelated existing XMP fields; read existing sidecars on import.
- [ ] **Grid** (frontend-dev): virtualized grid, 5,000+ items at 60 fps, thumbnail sizes, sort.
- [ ] **Filter bar** (frontend-dev): include/exclude tags (any/all), pick, min rating, burst, folder.
- [ ] **Loupe + compare** (frontend-dev): single loupe with zoom to 100%, 2-up burst compare, face-crop zoom.
- [ ] **Keyboard** (frontend-dev): Lightroom keys — P / X / U, 0–5, arrows, Space loupe, C compare, auto-advance.
- [ ] **UI tests** (frontend-dev): Playwright against Vite dev server with `@tauri-apps/api/mocks`; screenshots saved for orchestrator review.
- [ ] **QA gate**.
  - Acceptance: Playwright suite passes; `exiftool` reads ratings/labels/keywords from written sidecars on a `test-data/` copy; Lightroom-compatible field names; real app launches and culls sample set end to end.

## Phase 5 — Editor
- [ ] **Contract** (architect): preview render command (returns image path or bytes), presets, copy/paste settings, undo history.
- [ ] **Render engine** (rust-engine-dev): parametric pipeline on preview-res linear image (WB, exposure, contrast, highlights/shadows/whites/blacks, texture/clarity/dehaze, vibrance/saturation, HSL); tile/threads via rayon.
- [ ] **LUT** (rust-engine-dev): `.cube` parser (1D/3D, sizes up to 65), trilinear/tetrahedral interpolation, amount blend.
- [ ] **XMP develop settings** (rust-engine-dev): write/read `crs:` fields 1:1 with `ParametricAdjustments`.
- [ ] **Editor UI** (frontend-dev): slider panel, before/after, histogram, presets, sync settings across selection.
- [ ] **QA gate**.
  - Acceptance: slider change → updated preview < 100 ms; XMP round trip lossless; LUT output matches reference values in unit tests; Playwright slider tests.

## Phase 6 — Full RAW develop + Export
- [ ] **Contract** (architect): `ExportPreset` type + `export_images` / `cancel_export` + `exportProgress` event.
- [ ] **Full-res develop** (rust-engine-dev): LibRaw demosaic (Bayer + X-Trans), same pipeline as preview at full res.
- [ ] **Export presets** (rust-engine-dev): JPEG (quality 0–100), TIFF 8/16-bit, PNG, optional WebP/HEIC; resize (long edge / short edge / megapixels / none); sRGB / Display P3 / Adobe RGB with embedded ICC; output sharpening (screen/matte/glossy × low/std/high); filename template; metadata (all / copyright only / none); named saved presets.
- [ ] **Export UI** (frontend-dev): preset editor, batch export dialog with progress + cancel.
- [ ] **QA gate**.
  - Acceptance: 50-file batch per format from `test-data/`; output dimensions, JPEG quality, ICC profile and metadata verified with `exiftool`; memory flat during batch.

## Phase 7 — Anchor-photo scene matching
- [ ] **Contract** (architect): scenes table + `match_scene(anchor_ids, target_ids)`.
- [ ] **Matching** (vision-ml-dev): group frames by scene (time + histogram/embedding similarity); normalize histogram + white point delta from 1–2 graded anchors; apply relative adjustments.
- [ ] **UI** (frontend-dev): mark anchors, preview and apply to scene.
- [ ] **QA gate**.
  - Acceptance: matched frames' mean luma and WB within tolerance of the anchor on sample scenes.

## Phase 8 — Hardening + packaging
- [ ] Perf pass (import, analysis, grid, export) with numbers in Status Log.
- [ ] Error states, empty states, crash-safe catalog writes.
- [ ] `pnpm tauri build` → `.app` / `.dmg`; smoke-test the bundle.
- [ ] Final report in `docs/final-report.md`: what works, known gaps, how to use.

---

## Future phases (notes to revisit — not part of the autonomous run)

### Phase 9 — Reference-match grading ("make photo A look like photo B")
Requested 2026-09-29. User provides a reference photo B (any source/JPEG) and a target RAW A.
- Estimate a color/tone transform from A → B: global tone curve + WB/tint + HSL shifts, fit in a perceptual space (e.g. Oklab) from histogram/moment matching, optionally region-aware (skin, sky, shadows/highlights).
- Output options:
  1. **Starting point**: solved values written into `ParametricAdjustments` (editable sliders).
  2. **Custom LUT**: bake the fitted transform into a `.cube` (33³ or 65³) saved to a user LUT library, reusable via the Phase 5 LUT slot.
- Evaluation: ΔE between graded A and B on matched regions; side-by-side UI with strength slider.
- Depends on: Phase 5 (render engine, LUT support), Phase 7 (relative grading math).

### Other ideas
- Windows build (DirectML EP) — needs a Windows machine to test.
- Code signing / notarization for distribution.

---

## Status Log
Orchestrator appends one entry per completed task/phase: date, what was done, verify results, decisions.

- 2026-09-29 — Phase 1 complete (`bae9d2c`): 12 Rust tests, frontend build, dev app launches.
- 2026-09-29 — Phase 2 Contract: IPC v2 (previewPath, cacheDir, ImportStatus, regenerate_thumbnails, get_import_status, thumbnailFailed), migration 0002, asset protocol scoped to cache. Gate: 13 tests, clippy, fmt, pnpm build green.
- 2026-09-29 — Phase 2 complete: own FFI to libraw_r + TurboJPEG, direct ARW/RAF/CR3 container parsing, rayon ingest pipeline (8 threads). QA: 34 tests + 2 real-sample ignored tests pass; 396/396 ARW ready, 0 null capture times; 160.9 files/s; footprint 252 MB (100 files) vs 264 MB (396); EXIF/orientation match exiftool; frontend list + progress + retry, stale-race and cache-bust fixes. Sample folder trimmed to 396 by user. GUI rendering not screenshot-verified (terminal lacks Screen Recording permission); app launches on QA catalog with no runtime errors.
- 2026-09-29 — Phase 3 Contract: IPC v3 (analyze_images/cancel/status, CullThresholds per shoot type, FaceInfo, BurstGroup, suggestions + apply_suggestions, auto-analyze), migration 0003. Gate: 37 tests, clippy, fmt, build green.
- 2026-09-29 — Phase 3 complete: SCRFD + 106-pt landmarks (CoreML), OMZ eye-state CNN + FaceMesh V2 head pitch (CPU), per-eye sharpness, bursts (time + pHash), per-shoot-type scoring, analysis worker. Ground truth: 50 sampled + 53 review frames (test-data/labels.json v2). Fix round 1 after held-out precision failed (blink 0.43, missed_focus 0.55). Final (orchestrator re-run): blink P 1.00 R 0.22 (5/0/18), missed_focus P 0.85 R 0.61 (11/2/7), 14.0 ms/image throughput (44 ms single-thread), 396/396 analyzed, 23 bursts; no tagged frames outside labeled sets. Caveat: blink is deliberately conservative (undetermined when signals disagree); recall low.
