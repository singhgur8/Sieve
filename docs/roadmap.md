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

## Phase 7 — Anchor-photo scene matching ✅
- [x] **Contract** (architect): scenes table + `match_scene(anchor_ids, target_ids)`.
- [x] **Matching** (vision-ml-dev): group frames by scene (time + histogram/embedding similarity); normalize histogram + white point delta from 1–2 graded anchors; apply relative adjustments.
- [x] **UI** (frontend-dev): mark anchors, preview and apply to scene.
- [x] **QA gate**.
  - Acceptance: matched frames' mean luma and WB within tolerance of the anchor on sample scenes.

## Phase 7b — Real-world parity (user request 2026-09-29) ✅
Sample set: `/Users/gurjotsingh/Pictures/Jasmit Natalie Proposal` (45 GB; 537 ARW, 231 CR3, 79 RAF, 39 JPG, 394 user-edited
Lightroom XMPs). **Read-only** — never write sidecars there (auto-sync stays off; verify no file changes before/after);
copy subsets into `test-data/` for anything that writes.
- [x] **Slider responsiveness** (frontend-dev; rust-engine-dev if needed): no starvation while dragging — at most one
  render in flight per slot, send latest values when it lands; draft-size renders (~1024 px) while dragging, full
  quality on release/idle. Acceptance: preview visibly tracks a continuous drag (Playwright with delayed mock renders
  proves intermediate frames are shown; real bench of drag sequences reports frames shown/sec).
- [x] **Non-RAW sources** (architect → rust-engine-dev, frontend-dev): import and edit JPEG/HEIC/TIFF/PNG (and
  camera JPEG siblings of RAWs, grouped or listed per user setting): ingest/thumbnail/EXIF, develop from decoded sRGB→
  linear, XMP sidecars for JPEG edits (Lightroom writes `<name>.xmp` for RAW, embedded/sidecar for JPEG — decide),
  export.
- [x] **Lightroom develop parity** (architect → rust-engine-dev, frontend-dev): **full parity for every develop
  setting the user's edits use — Sieve must replace Lightroom, not supplement it (user requirement).** Parametric +
  point tone curve (master + RGB), color grading / split toning, camera calibration, sharpening, luminance/color noise
  reduction, vignette, grain, lens-profile-free basics; camera profiles via the Adobe Standard/Camera Matching DCPs
  installed locally by Adobe DNG Converter/Camera Raw (`/Library/Application Support/Adobe/CameraRaw/CameraProfiles`,
  read at runtime, never redistributed), and Adobe Looks via the RGB tables embedded in the XMP (`crs:Table_<md5>`)
  or the locally installed look profiles; fallback approximations only when the data is unavailable. Acceptance: for
  the user's edited frames, Sieve's render of the imported XMP matches Lightroom's render (ΔE2000 mean ≤ 3 vs a
  Lightroom-exported reference if the user provides one; otherwise orchestrator visual review) and every crs value
  round-trips.
- [x] **Mixed-camera validation at scale** (qa-engineer + owners): import the whole sample set read-only: ARW/RAF
  (X-Trans + Bayer)/CR3/JPG thumbnails + EXIF + develop + export correct; user XMP ratings + develop settings imported;
  throughput and peak memory at 925 files / 45 GB; culling suggestions vs the user's own ratings (agreement report);
  side-by-side of Sieve renders of the user's edits for visual parity review.
- [x] **Culling calibration to user picks** (vision-ml-dev): QA found 31% of the user's keepers suggested reject (burst duplicates, overexposed on 40% of frames). Acceptance on held-out part of the shoot: keeper false-reject ≤ 5%, overexposed < 10% unless truly clipped, no blink/missed_focus precision regression.
- [x] **QA gate**.

## Phase 7c — Local adjustments & masking ✅ (moved from Phase 11: user requires an all-in-one Lightroom replacement)
- [x] **Contract** (architect): mask groups with per-mask adjustment sets (Lightroom `crs:MaskGroupBasedCorrections`
  model): brush, linear gradient, radial gradient, luminance/color range, AI masks (subject, sky, background, people
  incl. face/skin/eyes/lips), add/subtract/intersect, invert, feather/density; XMP read/write parity with Lightroom.
- [x] **Segmentation models** (vision-ml-dev): permissively licensed ONNX models for subject, sky and people/face
  parts (CoreML where possible), cached per image, resolution-independent mask storage.
- [x] **Mask rendering** (rust-engine-dev): masks evaluated at any resolution; local adjustments in the shared pipeline
  (preview + export), within the slider latency budget.
- [x] **Masking UI** (frontend-dev): Lightroom-style masks panel, brush with size/feather/flow/auto-mask, gradient
  handles, AI select buttons, overlay visualization, keyboard shortcuts (O overlay, K brush, M linear, Shift+M radial).
- [x] **Mask pipeline integration** (rust-engine-dev, after 7b pipeline lands): call the develop/masks seam from develop/pipeline.rs + mod.rs (preview + export; export computes missing AI mattes first); adapt apply_group_blends to the pipeline's working space (linear ProPhoto vs Rec.2020 stub); lib.rs `XmpSync::with_mask_cache(mask_cache)` (architect-approved) so Lightroom mattes are cached on read; DevelopCache::info reports ai_mask_needs_update; launch catch-up for masks_pending_import; render the user's 50 masked frames and compare with/without masks.
- [x] **Masking polish** (frontend-dev + rust-engine-dev): per-mask tone curve editor, reorder groups/components, overlay rendered for the visible region at 100% zoom, verify crop-angle sign + gradient geometry against real renders, luminance eyedropper via backend sample (not canvas).
- [x] **QA gate**: the user's 50 AI subject masks (Adaptive: Subject presets; mattes embedded as JPEG XL in crs:Table_*) import and render like Lightroom (decoded mattes); new brush/gradient/range/AI masks work end to end; round trip preserves Lightroom mask data byte-for-byte when unchanged.

## Phase 8 — Hardening + packaging ✅
- [x] **Mask/XMP polish** (rust-engine-dev, vision-ml-dev): don't write default-valued global crs keys Lightroom omitted (only write keys that differ from defaults or already exist); Sky matte patchy on hazy low-contrast horizons (AZA06714); luminance-range mask blockiness (guide resample in develop/masks/overlay.rs); refit local Clarity/Texture/Temperature on the parity-round-1 pipeline (in-mask ΔE 3.78 → ≤ 3).
- [x] **Parity round 2** (rust-engine-dev): halo-free local Shadows at strong edges without losing fit; highlight reconstruction for raw-clipped skies (removes the green/magenta arc, IMG_5698/5674); night/low-key frames (DSCF5919 4.18, DSCF5923 4.56); calibrate Clarity/Texture/Dehaze and Whites+ against Camera Raw. Acceptance: held-out mean ≤ 2.5, no frame > 4, halo ≤ 4/255 on the step probe.
- [x] **Crop straighten preview** (frontend-dev + rust-engine-dev): live rotation preview while dragging the crop angle (Lightroom parity); currently applied only on commit.
- [x] **UX review** (ux-designer → frontend-dev): full-workflow review (import → cull → edit → scenes → export) for polish, friction and keyboard coverage; frontend-dev implements P0/P1 findings; ux-designer re-checks. Acceptance: no open P0/P1, keyboard cheat sheet in-app, Playwright green.
- [x] Perf pass (import, analysis, grid, export) with numbers in Status Log. Known items: `render_preview` does a catalog query per slider frame (cache SourceImage in DevelopCache); `handle_protocol` copies the JPEG per hit.
- [x] Error states, empty states, crash-safe catalog writes.
- [x] `pnpm tauri build` → `.app` / `.dmg`; smoke-test the bundle.
- [x] Final report in `docs/final-report.md`: what works, known gaps, how to use; include remaining Lightroom-parity gaps, if any.

## Phase 8b — User feedback round 1: guided workflow + Lightroom-style Develop (user request 2026-09-30)
Source: the user's first hands-on test of the release build. Phase 10 (style learning) is pulled into this phase because
the user asked for "auto edit based on what the model thinks I like".
- [x] **Contract** (architect): IPC v14 — catalog-wide preset/profile library imported from folders (Lightroom
  `.xmp` develop presets + legacy `.lrtemplate`, `.xmp` creative profiles with RGB/Look tables, `.dcp`, `.cube`),
  grouped by source folder, available in every project; `auto_tone(imageId)` / `auto_white_balance(imageId)`
  returning adjustments; per-folder workflow step state (cull / edit / export) and per-scene edit anchors (chosen
  representative, edited flag, applied flag); style-model commands (train from catalog/XMPs, status, predict);
  XMP auto-sync default on for new catalogs/folders.
- [x] **Projects + home page** (architect → rust-engine-dev, frontend-dev; user request 2026-09-30): a project = one
  shoot with its source folder(s) on disk, name, cover photo, shoot type, workflow step, created/last-opened dates. The
  app opens on a Projects home page (cards: cover, name, path, photo/keeper/edited counts, step, last opened; sort and
  search); "New project" = pick folder → name (defaults to folder name) → import; open / rename / remove from catalog
  (never deletes files) / reveal in Finder / locate moved folder. Inside a project, Library, Develop, filters, scenes,
  counts and export only see that project's photos; a project switcher in the TopBar returns home or jumps to another.
  Existing catalogs migrate one project per imported root folder. Acceptance: two projects imported, switching shows
  only each project's photos, counts right, relaunch reopens the home page, removing a project leaves files intact.
- [x] **Preset & profile library** (rust-engine-dev + frontend-dev): "Import presets & profiles…" takes a whole folder
  (recursive); items appear in every project under their folder groups (Develop left panel "Presets", profile browser
  next to Profile); applying a preset sets only the keys it contains (Lightroom semantics); creative profiles apply
  their look table with an Amount slider; `.cube` files appear as profiles, not a separate LUT concept; hover preview.
  Acceptance: import the user's Lightroom preset/profile folders (read-only copy in `test-data/`), each preset
  applies exactly its keys; profile amount 0/100/200 behaves like Lightroom.
- [x] **Auto tone + Auto WB** (rust-engine-dev): Lightroom-style "Auto" in Basic (Exposure, Contrast, Highlights,
  Shadows, Whites, Blacks, Vibrance, Saturation) and "Auto" white balance; Shift-double-click a slider = auto that
  slider. Acceptance: on the user's frames auto results land near the user's own edits (report deltas) and never clip
  skin; one undo step.
- [x] **Compare view** (frontend-dev): Compare is a view in Library and Develop (two photos side by side, synced zoom);
  a thin filmstrip of the current filtered gallery stays at the bottom with the selection highlighted; click / arrow
  keys pick the candidate, the other pane stays the select; editing sliders in Compare edit the active pane
  (Lightroom "Reference view" behaviour); swap / make-select shortcuts.
- [x] **Masks UX** (frontend-dev): overlay colour darker red (≈ #c00000 at 55–60% opacity) so the mask is obvious;
  overlay shows automatically while painting/dragging/adjusting a mask and fades out when done unless "Show overlay"
  (O) is on; Add / Subtract / Intersect menus render in a portal so they are never clipped by the panel.
- [x] **Small fixes** (frontend-dev): stars clickable wherever shown (grid cell, loupe, Develop toolbar, filmstrip;
  clicking the current rating clears it); scenes toggle (show/hide the scene strip and clear the scene filter, with a
  shortcut); grid size slider track/thumb visible on the dark theme (WCAG contrast).
- [x] **Lightroom-style Develop layout** (ux-designer spec → frontend-dev): match Lightroom Classic positions — left
  panel Navigator / Presets / Snapshots / History; right panel Histogram, tool strip (Crop, Masking), Basic (Profile +
  browser, WB with Auto, Tone with Auto, Presence), Tone Curve, HSL/Color, Color Grading, Detail, Effects,
  Calibration; bottom-left "Copy…" / "Paste", bottom-right "Previous" / "Reset"; Copy… dialog with every setting group
  as checkboxes (Check All / Check None, remembers last choice); Cmd+Shift+C / Cmd+Shift+V / Cmd+Alt+V (paste from
  previous); Sync… for multi-selection; filmstrip at the bottom.
- [x] **Guided workflow: Cull → Edit → Export** (architect → frontend-dev, vision-ml-dev): a step bar per project
  (AfterShoot-style). Cull step = today's culling. Edit step: keepers are grouped into scenes; for each scene the app
  proposes one representative photo to edit (best keeper, most typical lighting); a checklist shows scenes to edit /
  edited / applied; "Auto edit (my style)" pre-fills the representative; once edited, "Apply to scene" copies the
  edit to the rest of the scene with relative scene matching (exposure/WB normalised per frame), reviewable and
  undoable; "Apply all edited scenes". Export step = export dialog for keepers.
- [x] **Style learning / auto edit** (vision-ml-dev; was Future Phase 10): learn the user's style from their edited
  frames (the 394 proposal XMPs + future edits in the catalog) and predict full ParametricAdjustments for a new frame
  (features: develop-source stats, scene context, camera; model on-device, retrainable from the catalog). Acceptance:
  on held-out user edits, predicted-vs-user render ΔE2000 clearly better than Auto tone and than "no edit" (report
  both), and slider-level errors per group.
- [x] **XMP auto-save** (rust-engine-dev + frontend-dev): auto-sync on by default (debounced write of ratings, flags,
  tags and edits to sidecars, existing sidecar fields preserved); clear status (saved / pending / error) and a
  one-time explanation of how Sieve reads existing XMP as the starting point and merges changes into it.
- [ ] **UX review** (ux-designer): re-check the new workflow end to end; no open P0/P1.
- [ ] **QA gate**.

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

### Phase 10 — Personal style learning / auto-edit (user request 2026-09-29) — moved into the run as Phase 8b "Style learning"
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
- 2026-09-29 — Phase 7b slider responsiveness: one render in flight per slot + draft (≤1024 px) while dragging, full on release/idle; Playwright: 24–25 intermediate renders shown during a 1 s drag with 40 ms renders (0 before fix). UX review 1 P0/P1 implemented (69/69 Playwright; Library chrome 108/140 px, Develop viewer 571 px at 1280×800). Real-backend slider feel still to confirm in the app.
- 2026-09-29 — Phase 7 complete: scene features/detection (time gaps + histogram similarity, bursts never split), render-space ImageStats, Broyden solver for exposure/temp/tint (+tone) with safety caps, time-weighted 2-anchor blending, scene UI (strip, anchors, Match panel with local strength lerp, apply + undo). QA: 189 tests + 10 ignored; scene_eval 142/146 targets (97.3%) within 0.15 EV / 0.012 ab, all 4 misses flagged (dark LED scene, blown close-up); 48 scenes over 396 frames; 7 ms/target warm; app migrates to v8. Also merged here: IPC v8 UX additions + UX review 1 implementation (Playwright 69/69).
- 2026-09-30 — Phase 7b culling calibration: keeper_eval on the user's proposal shoot (keeper = edited or ≥2★), time split 60/40. Held-out keepers suggested reject 37.4% → 0%; overexposed 27.7% → 0% (now blown skin / >60% blown frame); remaining 7 rejects all non-keepers; wedding-label tag precision unchanged. Gap: stars/picks barely predict this user's keepers (AUC ~0.57) — content/preference model needed (Phase 10).
- 2026-09-30 — Phase 7b QA (final): non-RAW end to end PASS (neutral ≤ 2 levels, <name>.JPG.xmp sidecars, exports, originals byte-identical); parity PASS at 3.0 bar (held-out 2.88; ARW 2.97 / CR3 2.81 / RAF 2.87; 4 frames > 3 → Phase 8 round 2); culling calibration PASS (held-out keeper false-reject 0/115, wedding-label precision unchanged); scale PASS (orchestrator-run: 847/847 ready, 108 files/s, 330 MB footprint, 394 sidecars 0 errors, source listing of 1281 files identical). Open: M-size ARW half-size preview crop regression (fix in progress) blocks the QA gate.
- 2026-09-30 — Phase 7c implemented (pending QA): v10 masks contract; segmentation models (BiRefNet subject, U²-Net sky, YOLOX + EfficientSAM people, MediaPipe parts; all MIT/Apache, SCRFD caveat); Lightroom mask XMP I/O (50/50 user mattes decoded, byte-identical round trip); mask evaluation + local planes merged with parity round 1 (full-frame mask == global slider; outside-mask change ≤ 0.011 ΔE); user's 10 masked frames vs Camera Raw 3.42 (in-mask 3.78); masked 2048 p95 78 ms; masking UI + UX review 2 fixes + polish (140 Playwright). Phase 8 UX review done (reviews 1 and 2, all P0/P1 implemented); crop straighten live preview done.
- 2026-09-30 — Phase 7b complete: M-size ARW half-size crop regression fixed (158d475; crop scale from decoded vs full size); direct_copy_matches_mem_image + real_raw_export_matches_preview pass; 259 tests green. Parity round 2 continues in Phase 8.
- 2026-09-30 — Phase 7c complete: QA PASS — 329 tests + 140 Playwright; user's 50 Lightroom masks import/render (outside-mask ΔE ≤ 0.011; vs Camera Raw 3.42 with masks / 4.02 without on 10 frames); round trip changes only the edited local value, mattes byte-identical; new Subject/Sky/People/brush/linear/radial/range masks confined to their regions, exported, Lightroom-structured XMP; AI subject 3–5 s cold (cached after), sky 0.25 s; masked 2048 p95 71 ms. Follow-ups → Phase 8 Mask/XMP polish.
- 2026-09-30 — Phase 8 perf + crash safety merged (add7aec, pending final QA). Slider: source lookup 0.100 → 0.001 ms (SourceImage cached in DevelopCache), draft frame p50 ~28 ms, IPC+protocol ~0.03 ms (protocol copy kept: 3 µs); warm 2048 render p50 72 ms (regressed from 18.6 ms → parity round 2). Ingest 847 RAWs 92.7–99.6 → 99.8–107 files/s, 330 MB peak; analysis 9.1 ms/image. Grid 50k: worst median 231.7 → 25.3 ms (16.5 ms with v13 indexes). Export 50 ARW: JPEG full 2.94/s, JPEG 2048 P3 6.99/s, TIFF16 1.72/s, WebP 6.19/s, HEIC 7.15/s, peak ≤ 942 MiB. Catalog: WAL+NORMAL+fullfsync, quick_check on open, 3 rotating VACUUM INTO backups, read-only fallback, kill test all-or-nothing; panic=unwind. IPC v12 model downloads merged.
- 2026-09-30 — Phase 8 QA (7cd235c): Perf, Error states + crash-safe catalog, Packaging PASS. Gate: 357 Rust + 168 Playwright, clippy/fmt clean; 19/22 ignored real-sample tests pass (3 need SIEVE_SAMPLE_XMP_DIR / downloaded mask models — pass with setup); M-size crop tests pass. Grid 50k worst median 15.4 ms, filter counts 6.9 ms; draft slider frame ~29 ms. IPC v13 (missing originals + Locate folder, catalog health banner + backup restore, specific error kinds) and error/empty states + error boundaries + in-app model download UI. Bundle smoke PASS (import → CoreML analysis → render → export inside Sieve.app); dmg 38 MB `hdiutil verify` VALID (built with CI=true). Open: warm 2048 render p50 81 ms → parity round 2.
- 2026-09-30 — Phase 8 Mask/XMP polish merged: writer skips default-valued keys Lightroom omitted (real masked sidecars: unchanged write only touches MetadataDate; single edits touch only that key); range masks edge-aware + anti-aliased; Sky fed upright frames + 640 px prior (AZA06714 coverage 0.18 → 0.49, clean horizon; no-sky frames stay 0); local Temp/Tint/Clarity/Texture refit on isolated Camera Raw variants (local effect error 2.01 → 1.23, in-mask ΔE 3.78 → 3.53, whole-frame 3.42 → 3.32). In-mask target ≤ 3 not met: the unmasked floor on the same pixels is 3.37 (global pipeline) → carried into Parity round 2. Gate 364 Rust + 168 Playwright.
- 2026-09-30 — Phase 8 Parity round 2 merged: highlight reconstruction for raw-clipped channels + clip-anchored roll-off (green/magenta arc gone; AZA06509 3.8 → 2.0), halo-free bilateral-grid Shadows/Highlights bases (step-probe halo 0/255, was 15), X-Trans colour-NR floor, Dehaze recalibration (night frames DSCF5919 4.18 → 3.16, DSCF5923 4.56 → 3.02). ΔE2000 vs Camera Raw: fit 2.54 → 2.14, held-out 2.88 → 2.43 (max 3.39) — acceptance met. Render speed: warm 2048 p50 70.9 → 38–41 ms, 1024 draft slider 28 → 18.9 ms, parity unchanged. Masked-frame floor unchanged (3.38; extreme Contrast/Whites/Blacks interaction) → known gap. Gate 376 Rust + 168 Playwright.
- 2026-09-30 — Phase 8 final QA (cfd6ccd): gate 376 Rust + 168 Playwright PASS; parity re-run independently: held-out 2.43 (16 frames; ARW 2.80 / CR3 2.48 / RAF 2.08), max AZA06413 3.39, halo 0/255; slider_latency 2048 p50 38.0 ms, draft 18.9 ms; ignored real-sample tests pass except the 2 needing downloaded mask models (xmp minimal-diff test fixed for the missing-original guard); bundle rebuilt (app 62 MB, dmg 38 MB, hdiutil VALID, smoke PASS). Masked frames after round 2 (mask_render, 10 frames): whole 3.42 / in-mask 3.54 / unmasked floor 3.38. Isolated local-slider variants vs Camera Raw within ~0.05 (Clarity −60 ×0.921 vs 0.922). Export peak ≤ 942 MiB, JPEG full 2.94 files/s (perf entry). Remaining outlier AZA06487 3.22 (Shadows lift next to blown sky). Final report written: `docs/final-report.md`.
- 2026-09-30 — Phase 8b Compare view, Masks UX, Small fixes complete (f1b291f, merged 463976d): QA PASS — 376 Rust + 177 Playwright (fixes8b 9/9); Compare in Library + Develop (filmstrip Select/Candidate, arrows/click, swap Down / make-select Up, sliders edit active pane, synced zoom); mask overlay #c00000 @ 0.58, auto-show/fade, O pins, portal menus verified in a 640 px window; stars clickable in cell/loupe/Develop/filmstrip (click current clears); Shift+S scenes toggle; size slider track 7.8:1 / thumb 19:1. Stale `src-tauri/target` from the old checkout path cleaned.
- 2026-09-30 — Phase 8b Contract complete (cacd536, merged 85dd2b5): IPC v14 — Projects (list/get/create/open/rename/cover/shoot type/remove; migration 0012 one project per folder; projectId scoping on queries, counts, bursts, scenes, analysis, export), workflow step per project, edit plan + apply-to-scene batches, catalog-wide style/profile library, auto_tone / auto_white_balance (stubs), style model train/status/predict, Copy groups + paste-from-previous, XMP auto-sync default on. Gate on phase branch: 384 Rust, clippy 0, fmt, build, 177 Playwright. Repo disk cleanup at the user's request (80 GB → 16 GB; test outputs + RAW copies + merged worktree + stale target removed; rule added to CLAUDE.md).
- 2026-09-30 — Phase 8b Projects + home page and Lightroom-style Develop layout complete (merged fe45b42): QA PASS — 384 Rust, clippy 0, fmt, build, 204 Playwright. Projects on the real backend: two imported projects scoped correctly (queries 10/10, filter counts, scenes; cross-scope 0), remove_project leaves 30 files (20 ARW + 10 XMP) byte-identical, FK check clean; migration 0012 on a copy of the live v11 catalog → 2 projects (396 / 39) = one per root folder. Develop: panel order, Copy Settings dialog (25 fields, tri-state, Check All/None/Modified, remembered), Cmd+Shift+C/V, Cmd+Alt+V, Sync…, Previous/Reset, presets/profile browser/Auto wired to v14. Deviations accepted: Snapshots + disabled (no backend command yet), histogram yields to the Masks panel.
- 2026-09-30 — Phase 8b Preset & profile library, Auto tone + Auto WB, Guided workflow, Style learning, XMP auto-save complete (merged up to 0eb51b5): QA PASS — 419 Rust, clippy 0, fmt, build, 226 Playwright. Presets: 535 real presets × 2 bases, 29,606 keys, 0 outside a preset's keys; profile Amount vs Camera Raw (Vintage 09 ΔE 2.10/2.67/3.62 at 0/100/200). Auto vs Camera Raw Auto (oracle via DNG Converter, 374 frames): exposure 0.17 EV, render ΔL* 2.82 (none 5.69), Auto WB 565 K (as-shot 1142 K); skin clip 0/202 face frames > 0.3%; vs user's edits exposure 0.53 EV (contrast/blacks are the user's preset look → style model's job); one history entry. Style model: held-out 149 frames ΔE2000 7.46 vs no edit 9.42 vs reference auto 9.61 (121/149 better than no edit); through the catalog with the real auto_tone (12 frames) 6.33 vs auto 12.21 vs none 8.78; in-app validation 4.43 / 5.81 / 7.97; WB temperature the weak group (329 K vs 129 K). Workflow: UI spec + scene/workflow + batch round-trip tests; batch undo exact on a real catalog (12/12); per-frame exposure normalisation covered by Phase 7 matching tests only. XMP: auto-sync default on, sidecar fields preserved, status + explainer, exiftool round trips pass. Also merged: analysis skips images removed mid-pass (7bd3424), flaky style-import/mask-timing tests fixed (12 green runs).
