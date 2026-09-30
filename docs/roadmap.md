# Sieve Roadmap

Source of truth for autonomous execution (see "Autonomous Orchestration" in `CLAUDE.md`).
The orchestrator works top to bottom: the first unchecked task is the next task.
A phase is done only when every task is checked **and** its acceptance checks pass.

Conventions
- Branch per phase: `phase-N-<slug>`; fast-forward merge to `main` after the QA gate.
- `Owner` = agent in `.claude/agents/`. Contract/schema changes always go through `architect` first.
- Sample RAWs: `/Users/gurjotsingh/Pictures/test RAWS` (788 Sony `.ARW`, ~20 GB). **Read-only.**
  Copy subsets into `test-data/` (gitignored) for anything that writes (XMP, exports).
  Set `SIEVE_SAMPLES` to point elsewhere; paths change per project.
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

## Phase 4 — Culling UI + XMP write ✅
- [x] **Contract** (architect): XMP sync commands (`write_xmp`, `read_xmp`, auto-sync setting), keyboard-driven batch ops.
- [x] **XMP sidecars** (rust-engine-dev): mapping — reject → `xmp:Rating -1`; stars → `xmp:Rating 0–5`; pick → `xmp:Label "Pick"` (color labels otherwise via `xmp:Label`); auto tags → `lr:hierarchicalSubject` `Sieve|<tag>` + `dc:subject`. Preserve unrelated existing XMP fields; read existing sidecars on import.
- [x] **Grid** (frontend-dev): virtualized grid, 5,000+ items at 60 fps, thumbnail sizes, sort.
- [x] **Filter bar** (frontend-dev): include/exclude tags (any/all), pick, min rating, burst, folder.
- [x] **Loupe + compare** (frontend-dev): single loupe with zoom to 100%, 2-up burst compare, face-crop zoom.
- [x] **Keyboard** (frontend-dev): Lightroom keys — P / X / U, 0–5, arrows, Space loupe, C compare, auto-advance.
- [x] **UI tests** (frontend-dev): Playwright against Vite dev server with `@tauri-apps/api/mocks`; screenshots saved for orchestrator review.
- [x] **QA gate**.
  - Acceptance: Playwright suite passes; `exiftool` reads ratings/labels/keywords from written sidecars on a `test-data/` copy; Lightroom-compatible field names; real app launches and culls sample set end to end.

## Phase 5 — Editor ✅
- [x] **Contract** (architect): preview render command (returns image path or bytes), presets, copy/paste settings, undo history.
- [x] **Render engine** (rust-engine-dev): parametric pipeline on preview-res linear image (WB, exposure, contrast, highlights/shadows/whites/blacks, texture/clarity/dehaze, vibrance/saturation, HSL); tile/threads via rayon.
- [x] **LUT** (rust-engine-dev): `.cube` parser (1D/3D, sizes up to 65), trilinear/tetrahedral interpolation, amount blend.
- [x] **XMP develop settings** (rust-engine-dev): write/read `crs:` fields 1:1 with `ParametricAdjustments`.
- [x] **Editor UI** (frontend-dev): slider panel, before/after, histogram, presets, sync settings across selection.
- [x] **QA gate**.
  - Acceptance: slider change → updated preview < 100 ms; XMP round trip lossless; LUT output matches reference values in unit tests; Playwright slider tests.

## Phase 6 — Full RAW develop + Export ✅
- [x] **Contract** (architect): `ExportPreset` type + `export_images` / `cancel_export` + `exportProgress` event.
- [x] **Full-res develop** (rust-engine-dev): LibRaw demosaic (Bayer + X-Trans), same pipeline as preview at full res.
- [x] **Export presets** (rust-engine-dev): JPEG (quality 0–100), TIFF 8/16-bit, PNG, optional WebP/HEIC; resize (long edge / short edge / megapixels / none); sRGB / Display P3 / Adobe RGB with embedded ICC; output sharpening (screen/matte/glossy × low/std/high); filename template; metadata (all / copyright only / none); named saved presets.
- [x] **Export UI** (frontend-dev): preset editor, batch export dialog with progress + cancel.
- [x] **QA gate**.
  - Acceptance: 50-file batch per format from `test-data/`; output dimensions, JPEG quality, ICC profile and metadata verified with `exiftool`; memory flat during batch.

## Phase 7 — Anchor-photo scene matching
- [x] **Contract** (architect): scenes table + `match_scene(anchor_ids, target_ids)`.
- [x] **Matching** (vision-ml-dev): group frames by scene (time + histogram/embedding similarity); normalize histogram + white point delta from 1–2 graded anchors; apply relative adjustments.
- [x] **UI** (frontend-dev): mark anchors, preview and apply to scene.
- [ ] **QA gate**.
  - Acceptance: matched frames' mean luma and WB within tolerance of the anchor on sample scenes.

## Phase 7b — Real-world parity (user request 2026-09-29)
Sample set: `/Users/gurjotsingh/Pictures/Jasmit Natalie Proposal` (45 GB; 537 ARW, 231 CR3, 79 RAF, 39 JPG, 394 user-edited
Lightroom XMPs). **Read-only** — never write sidecars there (auto-sync stays off; verify no file changes before/after);
copy subsets into `test-data/` for anything that writes.
- [ ] **Slider responsiveness** (frontend-dev; rust-engine-dev if needed): no starvation while dragging — at most one
  render in flight per slot, send latest values when it lands; draft-size renders (~1024 px) while dragging, full
  quality on release/idle. Acceptance: preview visibly tracks a continuous drag (Playwright with delayed mock renders
  proves intermediate frames are shown; real bench of drag sequences reports frames shown/sec).
- [ ] **Non-RAW sources** (architect → rust-engine-dev, frontend-dev): import and edit JPEG/HEIC/TIFF/PNG (and
  camera JPEG siblings of RAWs, grouped or listed per user setting): ingest/thumbnail/EXIF, develop from decoded sRGB→
  linear, XMP sidecars for JPEG edits (Lightroom writes `<name>.xmp` for RAW, embedded/sidecar for JPEG — decide),
  export.
- [ ] **Lightroom develop parity** (architect → rust-engine-dev, frontend-dev): **full parity for every develop
  setting the user's edits use — Sieve must replace Lightroom, not supplement it (user requirement).** Parametric +
  point tone curve (master + RGB), color grading / split toning, camera calibration, sharpening, luminance/color noise
  reduction, vignette, grain, lens-profile-free basics; camera profiles via the Adobe Standard/Camera Matching DCPs
  installed locally by Adobe DNG Converter/Camera Raw (`/Library/Application Support/Adobe/CameraRaw/CameraProfiles`,
  read at runtime, never redistributed), and Adobe Looks via the RGB tables embedded in the XMP (`crs:Table_<md5>`)
  or the locally installed look profiles; fallback approximations only when the data is unavailable. Acceptance: for
  the user's edited frames, Sieve's render of the imported XMP matches Lightroom's render (ΔE2000 mean ≤ 3 vs a
  Lightroom-exported reference if the user provides one; otherwise orchestrator visual review) and every crs value
  round-trips.
- [ ] **Mixed-camera validation at scale** (qa-engineer + owners): import the whole sample set read-only: ARW/RAF
  (X-Trans + Bayer)/CR3/JPG thumbnails + EXIF + develop + export correct; user XMP ratings + develop settings imported;
  throughput and peak memory at 925 files / 45 GB; culling suggestions vs the user's own ratings (agreement report);
  side-by-side of Sieve renders of the user's edits for visual parity review.
- [ ] **QA gate**.

## Phase 7c — Local adjustments & masking (moved from Phase 11: user requires an all-in-one Lightroom replacement)
- [ ] **Contract** (architect): mask groups with per-mask adjustment sets (Lightroom `crs:MaskGroupBasedCorrections`
  model): brush, linear gradient, radial gradient, luminance/color range, AI masks (subject, sky, background, people
  incl. face/skin/eyes/lips), add/subtract/intersect, invert, feather/density; XMP read/write parity with Lightroom.
- [ ] **Segmentation models** (vision-ml-dev): permissively licensed ONNX models for subject, sky and people/face
  parts (CoreML where possible), cached per image, resolution-independent mask storage.
- [ ] **Mask rendering** (rust-engine-dev): masks evaluated at any resolution; local adjustments in the shared pipeline
  (preview + export), within the slider latency budget.
- [ ] **Masking UI** (frontend-dev): Lightroom-style masks panel, brush with size/feather/flow/auto-mask, gradient
  handles, AI select buttons, overlay visualization, keyboard shortcuts (O overlay, K brush, M linear, Shift+M radial).
- [ ] **QA gate**: the user's 95 mask groups import and render plausibly; round trip preserves Lightroom mask data.

## Phase 8 — Hardening + packaging
- [ ] **UX review** (ux-designer → frontend-dev): full-workflow review (import → cull → edit → scenes → export) for polish, friction and keyboard coverage; frontend-dev implements P0/P1 findings; ux-designer re-checks. Acceptance: no open P0/P1, keyboard cheat sheet in-app, Playwright green.
- [ ] Perf pass (import, analysis, grid, export) with numbers in Status Log. Known items: `render_preview` does a catalog query per slider frame (cache SourceImage in DevelopCache); `handle_protocol` copies the JPEG per hit.
- [ ] Error states, empty states, crash-safe catalog writes.
- [ ] `pnpm tauri build` → `.app` / `.dmg`; smoke-test the bundle.
- [ ] Final report in `docs/final-report.md`: what works, known gaps, how to use; include remaining Lightroom-parity gaps, if any.

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

### Phase 10 — Personal style learning / auto-edit (user request 2026-09-29)
Learn the user's editing style from their Lightroom XMPs (e.g. 394 edited frames in the Jasmit Natalie Proposal set:
consistent parametric curve, split toning, calibration, NR) and propose full edits for new shoots, per scene/lighting.
Approach sketch: features from the develop-source stats (Phase 7) + scene context → predict ParametricAdjustments
(gradient-boosted trees or small MLP on-device), refined with Phase 7 relative matching; evaluate by ΔE vs the
user's own renders on held-out shoots. Depends on Phase 7b parity (so predicted settings render like Lightroom).

### Phase 11 — (moved into the run as Phase 7c)

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
- 2026-09-29 — Phase 4 complete: IPC v4 + migration 0004; XMP sidecars via span-preserving quick-xml merge (Rating -1 reject, Label Pick/colors, lr:hierarchicalSubject Sieve|<tag> + dc:subject; crs:/foreign keywords preserved byte-for-byte), newer-wins debounced auto-sync (off by default); virtualized grid (@tanstack/react-virtual), filter bar, loupe/face zoom/2-up compare, Lightroom keys. QA: 94 Rust tests + 5 ignored (exiftool round trip on real ARW copies) pass; Playwright 14/14 (5,000 items, max 66 DOM nodes, 16.7 ms/frame scroll); real app launches on demo catalog with CoreML, no errors. Product renamed LumenRAW -> Sieve (user request). Caveat: real GUI not visually verified by orchestrator (no Screen Recording permission) — covered by Playwright on mock backend.
- 2026-09-29 — Phase 5 Contract: IPC v5 (render_preview via sieve:// scheme latest-wins, develop info, history undo/redo, presets, paste/sync/reset, LUT library, crs: XMP seam), migration 0005. Gate: 100 tests, clippy, fmt, build, UI 14/14.
- 2026-09-29 — Phase 5 complete: LibRaw linear decode (C shim for half_size/user_flip/cam_xyz), DevelopCache LRU, parametric pipeline (DNG-model WB, tone, texture/clarity/dehaze, vibrance/sat/HSL, filmic base, Rec.2020->sRGB), .cube LUTs (tetrahedral/trilinear), history/presets/paste/sync, crs: XMP read/write, sieve:// render protocol; Develop UI. QA: 129 Rust tests + 7 ignored real-sample tests pass; Playwright 27/27 (+ WB reset fixes); warm 2048 render p50 18.6 ms / p95 42.4 ms (target < 100), region 13 ms, cold decode 337 ms; crs round trip lossless (exiftool-verified); LUT reference values; app migrates to v5 clean. Note: sample ARWs are Sony lossless M-size, so 'half-size' source equals full size for these files; no RAF/CR3 real samples.
- 2026-09-29 — Phase 6 Contract: IPC v6 (ExportSettings/ExportPreset + built-ins, capabilities, plan_export, export_images/cancel/jobs, exportProgress/exportFinished, memory-budgeted job runner), migration 0006. Gate: 133 tests, clippy, fmt, build, UI 38/38.
- 2026-09-29 — Phase 6 complete: full-res LibRaw decode (AHD / X-Trans 3-pass), shared develop pipeline (preview == export, Lanczos-3), output sharpening, sRGB/P3/AdobeRGB with generated ICC, JPEG/TIFF16/PNG/WebP/HEIC encoders, whitelisted metadata, naming templates, memory-budgeted job runner. QA: 157 tests + 10 ignored real-sample tests; 6 formats x 50 files, 0 failures; exiftool: dims, JPEG q90/q80, ICC, 16-bit TIFF, orientation 1, metadata per option, no crs:/Sieve| leakage; peak footprint 899 MiB; app migrates to v6.
