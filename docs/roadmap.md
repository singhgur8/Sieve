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
- [x] **UX review** (ux-designer): re-check the new workflow end to end; no open P0/P1.
- [x] **QA gate**.

---

## Phase 8c — User feedback round 2: culling clarity + Lightroom interop (user request 2026-10-03) ✅
Runs in a Linux cloud container (no user RAWs, no CoreML, no macOS bundle): acceptance uses tests,
synthetic / downloaded sample files and Playwright on the mock backend; anything that needs the user's Mac is listed
under "Verify on the Mac" in the Status Log entry. Branches are pushed to `origin` (user permission 2026-10-03).
- [x] **Contract v18** (architect): (a) `KeeperRule` gains a mode "everything not rejected" and it becomes the default
  (user: "I would want to keep everything that's not rejected"); the old pick/stars/suggestion rule stays as an option.
  (b) `get_cull_summary(projectId)`: picked / unflagged / rejected (by you vs. auto-applied) / keepers, with the keeper
  formula broken down. (c) Human-readable suggestion reasons per image (`QualityScore.reasons`: kind + text, e.g.
  "Eyes closed", "Missed focus on the face", "Duplicate in burst (keeper DSC0123)") and the user-vs-auto origin of a
  reject flag. (d) Lightroom-style metadata filters on `ImageQuery` (file type / extension, camera, lens, ISO,
  focal length, aperture, shutter, capture date range, edited / unedited, has sidecar) + a command returning the
  distinct values with counts for the current scope. (e) A generic background-activity event (kind, label,
  done / total, finished/error) for import, analysis, XMP save, paste/sync, apply to scene, export, model download.
  Migration if needed; bindings regenerated; ipc-changelog entry.
- [x] **Culling shortcuts, Lightroom one-hand** (frontend-dev): Z = Pick, X = Reject (Shift = and advance), P stays
  as an alias, U unflags. Space: Grid → Loupe; in Loupe / Compare / Develop Space toggles Fit ↔ 1:1 zoom and never
  leaves the view (Esc / G / E / Enter go back to Grid). Z no longer zooms anywhere (crop tool keeps X = swap). Cheat
  sheet, tooltips and hints follow the keymap. Acceptance: Playwright covers every changed chord in each mode.
- [x] **Arrow-key navigation glitch** (frontend-dev): with two filters on, holding / tapping Left/Right shows a
  transition-like flash. Find the cause (CSS transitions, image swap without decode, re-mount, stale preview,
  filter re-query on each move) and fix it. Acceptance: Playwright at 25 key presses/s with two filters: each
  displayed frame belongs to the current photo (no previous-photo flash, no opacity animation), main-thread work per
  keypress measured before/after and recorded.
- [x] **XMP that Lightroom reads, both ways** (rust-engine-dev): Lightroom Classic (13.2+) stores flags in
  `xmpDM:pick` (1 / 0 / -1) and `xmpDM:good` (True / False / absent); `xmp:Label "Pick"` shows up as a bogus colour
  label and `xmp:Rating -1` is Bridge-only. Write flags as `xmpDM:pick` / `xmpDM:good`, stars as `xmp:Rating 0–5`
  (kept when rejected), real colour labels in `xmp:Label`, tags as keywords; remove a `Label "Pick"` written by older
  Sieve versions. Read all conventions (xmpDM first, then Rating -1 / Label "Pick"). Re-read sidecars changed by
  another app when a project opens and when the window regains focus (newer wins). Acceptance: fixture of a
  Lightroom Classic sidecar with pick / reject / stars / label round-trips; exiftool shows `XMP-xmpDM:Pick/Good`;
  unrelated fields byte-identical; externally edited sidecar picked up.
- [x] **"database is locked" on Save** (rust-engine-dev): Cmd+S over 2,389 photos while auto-sync ran gave "Saved
  metadata for 685 photos; 1704 sidecars could not be written. database is locked". Cause: catalog connections have
  no `busy_timeout`, so the auto-sync connection and the explicit save collide. Fix: busy timeout on every catalog
  connection, short write transactions, explicit save waits for / merges with a running auto-sync pass.
  Acceptance: test with ≥ 2,400 images running an explicit save while auto-sync and UI writes run → 0 failures;
  catalog errors reported separately from per-file errors.
- [x] **Suggestion reasons** (vision-ml-dev): fill `QualityScore.reasons` for every non-pick suggestion and every
  tag from the existing metrics (blink, missed focus, motion blur, under/over exposure, burst duplicate naming the
  keeper). Acceptance: unit tests per reason; every reject suggestion in the test catalogs has ≥ 1 reason.
- [x] **Culling clarity: counts, keepers, rejected pile, icons** (frontend-dev): always-visible grid readout
  "Showing N of M photos" that follows every filter (and the selection count); a cull summary (picked / unflagged /
  rejected / keepers = formula, each clickable as a filter) in the Cull step and where the app says how many photos
  go on to Edit; Edit / Export say what the keepers are made of and link to the keeper rule; a "Rejected" view whose
  cells show why (you rejected / auto, with the reasons); Apply suggestions ("Auto") explained in place (what it
  changes, that you review after, undoable). Every icon / badge on cells, loupe, filmstrip and toolbars has a
  hover description. Acceptance: Playwright on mock data: counts match the grid for 3 filter combos, summary
  formula adds up, reasons shown on rejected cells, every icon has a non-empty accessible title.
- [x] **Lightroom-style metadata filters** (frontend-dev, after the contract): Library Filter-style row (file type,
  camera, lens, ISO, focal length, aperture, shutter, date, edited, sidecar) with counts per value, combined with the
  existing tag / flag / star filters. Acceptance: Rust tests per filter; Playwright: filter by extension changes the
  grid and the count readout.
- [x] **Background activity indicator** (frontend-dev): a small corner indicator (spinner / progress bar + label +
  count) while long work runs (import, analysis, XMP save, paste / sync to many, apply to scene, export, model
  download); conflicting buttons disabled meanwhile; finished / error states. Acceptance: Playwright with a slow mock.
- [x] **Help & FAQ** (frontend-dev, content reviewed by ux-designer): in-app guide (Help button + F1) covering the
  culling workflow, what Auto / Apply suggestions does, keepers, auto-advance, what each icon means, where auto-save
  writes (XMP sidecars next to the RAWs, shown with the path) and how to see Sieve's culling in Lightroom (on import;
  for photos already in a catalog: Metadata → Read Metadata from Files; Lightroom writes back with Cmd+S or "Automatically
  write changes into XMP"); controls such as Auto-advance link to their FAQ entry.
- [x] **UX review** (ux-designer): re-check the cull → edit flow with the above; no open P0/P1.
- [x] **QA gate**.

---

## Phase 8d — User feedback round 3: review, editing feel, crop/upright, capture time (user request 2026-10-05)
Runs on the user's Mac (CoreML available). **Test photos rule (user, 2026-10-05):** tests use only the provided
sample sets — `Pictures/test RAWS` and `Pictures/Jasmit Natalie Proposal`, read-only, copied into `test-data/` before
anything writes XMP or exports. Never point `SIEVE_CATALOG` at the live catalog, never open the user's live projects,
never touch any other photo folder. Diagnosis of the live catalog was done on a read-only DB copy only.
- [x] **Contract v19** (architect): (a) per-photo capture time: catalog keeps original EXIF time + corrected time;
  `edit_capture_time(ids, mode)` with modes shift-by-offset / set-exact / "sync cameras" (offset from a reference
  pair) and undo; corrected time read from Lightroom sidecars (`exif:DateTimeOriginal` / `photoshop:DateCreated` in
  XMP) and written back the same way; grid sort / bursts / scenes use the corrected time. (b) `get_image_metadata(id)`
  for a per-photo info panel (file, capture time original + corrected, camera, lens, exposure, dims, size, GPS if
  any, sidecar path). (c) Transform / Upright: `ParametricAdjustments` gains Lightroom Transform (Upright mode
  off/auto/level/vertical/full/guided + guides, Vertical, Horizontal, Rotate, Aspect, Scale, Offset X/Y, Constrain
  crop) with `crs:` mapping (`PerspectiveUpright`, `UprightVersion`, `UprightTransform_*`, `Perspective*`,
  `UprightFocal*`, `UprightGuidedDependentDigest`…) and `auto_upright(id, mode)` returning the solved values.
  (d) applied-preset tracking per photo (last applied preset id, cleared when a preset-owned key changes) so the
  browser can highlight it; preview render with a temporary preset / with one panel group reset (no history entry).
  (e) batch edit commands for an arbitrary selection: paste copied settings / sync to N ids as one undoable batch.
  (f) reject strictness setting (conservative / balanced / aggressive) on the cull thresholds / project.
  Migration; bindings; ipc-changelog.
- [ ] **Auto-reject logic** (vision-ml-dev): on the user's 2,824-photo wedding shoot the engine suggested reject for
  10 photos while the user rejected 1,025 (418 of those suggested *pick*); missed_focus / blink / burst duplicate tags
  exist but don't become reject suggestions — the Phase 7b keeper calibration (keeper false-reject 0%) over-corrected.
  Make tags with sufficient confidence suggest reject (burst non-keepers, closed eyes on the main subject, missed
  focus, motion blur, badly under/over exposed), shoot-type aware; expose one "reject strictness" setting
  (conservative / balanced / aggressive, default balanced). Calibrate only on the test sets (labels.json + the Jasmit
  set's edited/starred keepers). Acceptance on held-out test frames: balanced suggests reject for ≥ 25% of non-keepers
  with keeper false-reject ≤ 5%; numbers per strictness level in the Status Log; every reject has a reason.
- [ ] **Capture time + per-photo info** (rust-engine-dev + frontend-dev, after contract): Library Metadata panel shows
  the selected photo's date/time and EXIF (not only gallery facts); Edit Capture Time dialog (shift selection by
  ±h/m/s, set exact, sync two cameras by picking one frame from each that happened at the same moment) — Lightroom's
  "Edit Capture Time" equivalent; Lightroom-corrected times picked up from sidecars. Acceptance: fixture sidecar with
  Lightroom-shifted DateTimeOriginal imports with the corrected time; shifting one camera by −1 h reorders a mixed
  two-camera test set; exiftool shows the written time; undo restores; originals byte-identical.
- [x] **Loupe zoom like Lightroom** (frontend-dev): zoom level and relative position persist while moving with the
  arrow keys (Loupe, Develop, Compare); Space / click zooms to the point under the cursor (not the centre); zoom
  presets (Fit, Fill, 50/100/200/400%) in the toolbar + Navigator; drag to pan, smooth; low-res instantly then sharp
  tile with no jump. Acceptance: Playwright — zoom at a corner, press Right 5×, still 100% at the same relative point;
  Space at a cursor position zooms there.
- [ ] **Develop editing feel** (frontend-dev; rust-engine-dev for render side): (1) slider value editable — click the
  number to type, Up/Down ±1 step (Shift ×10, Alt fine) while a slider or value is focused/hovered, Enter commits,
  Esc cancels, double-click label resets; (2) Cmd+C / Cmd+V (and Ctrl+C / Ctrl+V; Cmd+Shift+C keeps the dialog)
  copy/paste all settings in Develop and in the Library/scenes selection (paste to every selected photo as one batch);
  (3) generic **Auto** (photo analysis, `auto_tone` + Auto WB) always available in Basic, separate from "Auto (my
  style)"; (4) press-and-hold the blue "changed" dot on a panel (Basic, Effects, Detail, …) shows the photo without
  that panel's changes until released; (5) slider drag never stutters: input handled independently of rendering
  (thumb follows pointer every frame), latest-wins progressive renders (small draft continuously during drag, full on
  release), research how Lightroom does it and record in decisions.md. Acceptance: Playwright for each; drag test on a
  real 33 MP ARW from the test set: pointer-to-thumb latency p95 < 20 ms, ≥ 15 renders shown per second of dragging,
  measured before/after.
- [ ] **Smooth switching between edited photos** (frontend-dev + rust-engine-dev): no flash of the unedited photo when
  moving between photos in Develop / scenes — show the last rendered edited preview (cached per photo + edit hash)
  until the new render lands, prerender neighbours; edited renders also used for Library grid / loupe / scenes
  thumbnails so edits are visible outside Develop. Acceptance: Playwright on a slow mock: no frame shows the unedited
  preview of an edited photo; Rust test: edited thumbnail regenerated after an edit.
- [ ] **Presets: highlight + hover preview on the main image** (frontend-dev, after contract): the applied preset is
  highlighted in the browser; hovering a preset previews it on the main image (not only the Navigator), reverting on
  hover-out; nothing applied until click. Acceptance: Playwright — hover shows preview render, mouse-out restores,
  click applies + highlights, changing a preset-owned slider clears the highlight.
- [ ] **Apply to Scene / batch editing fixes** (vision-ml-dev + frontend-dev): Apply to Scene must carry every
  setting of the representative (Effects — grain, clarity, vignette, dehaze — Detail, HSL, curves, colour grading,
  calibration, profile, LUT, masks where meaningful) with only exposure / WB normalised per frame; today grain /
  clarity etc. are dropped. Disabled Apply buttons say why in place (e.g. "Edit the representative first",
  "No keepers in this scene") with a one-click fix; scenes view supports multi-select + Paste / Sync / "Edit all in
  scene". Acceptance: Rust test — every non-normalised field of the representative equals the target after apply;
  Playwright — every disabled Apply has a visible reason; paste to a selection of 5 in scenes edits all 5, one undo.
- [ ] **Crop like Lightroom + Upright** (rust-engine-dev + frontend-dev + vision-ml-dev for line detection): fix
  straighten over-cropping (crop after rotation must be the largest rectangle of the chosen aspect inside the rotated
  frame, as Lightroom); Lightroom crop behaviour — drag corners/edges, drag inside to move image, drag outside to
  rotate, aspect presets + lock (A), X swaps orientation; Lightroom keymap (R enter, Enter/R commit, Esc cancel, O cycles overlays, Shift+O rotates overlay), angle tool (draw along a horizon), Auto straighten,
  Constrain to image, reset; Transform panel with Upright Auto / Level / Vertical / Full / Guided (vision line
  detection) and manual sliders, rendered in preview + export, written to `crs:` so Lightroom matches. Acceptance:
  Rust tests — rotation by θ keeps the max inscribed rect (area within 0.5% of the analytic value), Upright Level on a
  synthetic tilted horizon ≤ 0.3°, Vertical corrects converging verticals on a synthetic building; vs Camera Raw on
  ≥ 3 test frames with Lightroom Upright (if available in Jasmit XMPs) the angle within 0.5°; Playwright for crop
  interactions + keys.
- [ ] **Loupe true 1:1 + toolbar fit** (architect → frontend-dev): Loupe / Compare 100%+ shows full-resolution detail
  (region render as in Develop) instead of 1:1 of the 2048 px preview; fix the pre-existing grid toolbar overflow at
  1280 px (`ux8b-fixes.spec.ts:169` P1-7, fails on main 4d66c57 too). Acceptance: Playwright — 100% in Loupe requests a
  full-res region; P1-7 green.
- [ ] **UX review** (ux-designer): review the above in cull → edit (presets, scenes, crop, sliders, zoom); no open P0/P1.
- [ ] **QA gate**: baseline + every acceptance above, on test-set copies only; live catalog untouched (mtime/size of
  `~/Library/Application Support/com.sieve.app/catalog.sqlite` unchanged by the QA run).

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
- 2026-09-30 — Style learning re-opened: with the real `auto_tone` as the style_eval baseline (665d55e), held-out 149 frames: predicted 7.46 vs real Auto tone 7.19 vs no edit 9.42 (prediction wins 67/149; Fuji 1.55 vs 8.34, Sony 7.26 vs 6.55, Canon 9.56 vs 8.98). Acceptance "clearly better than Auto tone" not met → vision-ml fix round 1. The earlier PASS compared against the reference auto. The style_e2e Auto figure (12.21) disagrees with style_eval (7.19) and must be explained.
- 2026-09-30 — Phase 8b Style learning complete after fix round 1 (style model v2, merged a74cd04): Auto-tone anchor + forward-chained recency/as-shot blends + bounded exposure refinement. QA re-run (independent): per-camera held-out 149 frames ΔE2000 predicted 5.82 / median 4.26 vs real Auto tone (with faces) 7.19 / 6.06 vs no edit 9.42 / 9.03; wins 108/149 vs Auto, 139/149 vs none; time-cut split 7.53 vs 8.57 vs 9.86 (101/149). Per camera vs Auto: Sony 5.08 vs 6.55, Fuji 3.76 vs 8.34 (n=8), Canon 8.75 vs 8.98 (≈ tie). Slider MAE predicted / Auto / none: tone 16.7 / 42.7 / 42.3, presence 2.40 / 4.81 / 7.67, detail 0.10 / 7.02, HSL 0.16 / 7.55, WB 128 K / 130 / 130 (WB not meaningfully learned; shadows and saturation worse than Auto / none). Through the catalog: in-app validation 3.14 vs 6.81 vs 8.13; 12 frames 6.69 vs 8.11 vs 9.37; predict 223 ms cold / 19 ms warm; train 32.5 s cold / 20 s retrain; apply + undo exact (12/12). style_e2e Auto discrepancy explained (unanalysed catalog → no faces; Auto without faces fix in progress). Caveat: one shoot only.
- 2026-09-30 — Phase 8b UX review complete (335e161): review found P0 2 / P1 9 / P2 15 (docs/ux-review-8b.md); three fix rounds — (1) frontend P0-1 scene nav → representative, P0-2 hover preview, P1-1/6/7/8/9 + IPC v15 (persisted per-photo workflow state, unapplied/unassigned keepers, skip/minor scenes, options apply as batch, plan outdated, apply cancel, keepersOnly, listXmpFailures) and UI; (2) re-check 1 new P1-10 (options apply hit non-keepers) / P1-11 (older Undo broke a scene) → IPC v16 linear batch undo with `conflict`, persisted appliedBatch/latestBatch, real-backend bug fixed (undo of an apply left the scene applied; migration 0014); (3) re-check 2 new P1-12 (reset representative blocked Apply all) → IPC v17 `reset` status, lenient Apply all with skippedScenes, linear undo across Auto edit → Apply (migration 0015). Re-check 3: all P0/P1 resolved, no new P0/P1, P2s listed. Gate 446 Rust + 276 Playwright. Also fixed: rescore test race (waits for AnalysisFinished).
- 2026-09-30 — Phase 8b complete (QA gate PASS at 0bc45d4): 450 Rust + 279 Playwright, clippy 0, fmt; ignored real-sample tests touched in 8b pass (two need specific folders/fixtures, not bugs); release bundle app 64 MB / dmg 37 MB, hdiutil VALID, bundle smoke PASS (CoreML analysis, render, export inside Sieve.app), startup repaired the stale bundled-model links; real backend: project → analysis 10/10 → edit plan (9 keepers, 2 scenes) → auto_tone analysed + unanalysed (on-demand face found) → XMP auto-sync 7 sidecars with Lightroom crs: preserved; live v11 catalog copy migrates to v15 (2 projects, 435 images, quick_check ok). Also: Auto on unanalysed photos uses on-demand SCRFD (skin-clip frames 16 → 1 of 202), test/example paths made relative to the crate. Repo cleanup at the user's request (80 GB → ~19 GB incl. release build).
- 2026-10-03 — Phase 8c complete (QA gate PASS, `docs/qa-8c.md`), run in a Linux cloud container (CoreML EP made macOS-only, TurboJPEG 3 / ONNX Runtime from GitHub, `scripts/linux-cloud-env.sh`). Keys: Z pick / X reject (P alias), Space = Loupe from Grid and Fit↔1:1 zoom elsewhere (+ a real bug fixed: 1:1 chosen before the preview decoded gave e.g. 354%). Arrow-key glitch: previous photo painted while the next decoded (43/126 frames → 0), late zoom reset, blank Develop on switch, heavy re-renders (Loupe ≈131 → ≈80–95 ms per key). XMP: flags as `xmpDM:pick` / `xmpDM:good` (Lightroom Classic 13.2+), stars kept on reject, legacy `Label "Pick"` / `Rating -1` migrated, `refresh_sidecars` on project open / focus; "database is locked" root cause = deferred savepoint spanning the sidecar write + no busy timeout (2,400-image repro 287 ok / 2,113 locked → 2,400 / 0). IPC v18 / v18.1: keeper rule `not_rejected` default (migration 0016), cull summary, pick origin + filter, suggestion reasons, metadata filters + facets, activity events. UI: cull summary + keeper formula, "Showing N of M", rejected pile with who/why, icon titles, metadata filter row, corner activity stack, Help & FAQ (F1 / Cmd+?). UX review P1 9 → re-check 1 P1 5 → re-check 2 P1 0. Gate: 479 Rust, clippy 0, fmt, build, Playwright 318/319 (load-sensitive Phase 8b mask test hardened in 487c58c: 10/10 under 6 busy loops). Verify on the Mac: see `docs/qa-8c.md`.
- 2026-10-05 — Phase 8d Loupe zoom like Lightroom (e201f1d, merged bf6cbc4): QA PASS — zoom + relative position persist across arrows in Loupe / Compare / Develop, Space / click zoom at the cursor, Fit / Fill / 50–400% presets in toolbar + Navigator, rAF-batched translate3d pan; new zoom-lr.spec 5/5, nav-glitch no-flash green; Playwright 323/324 (the 1 failure, P1-7 toolbar at 1280, also fails on main 4d66c57 → new task). Loupe 100% is 1:1 of the 2048 preview → follow-up task.
- 2026-10-05 — Phase 8d Contract v19 (79b2ece, merged): schema v17 (0017: original + corrected capture time with source, applied preset, project reject strictness, paste batches); edit_capture_time / restore_capture_times, get_image_metadata, auto_upright (stub), render_preview_variant (slot `preview`, no history), set_project_reject_strictness; paste / sync / paste-previous return one undoable batch; ParametricAdjustments.transform with crs mapping. Gate on merged phase branch: 490 Rust (0 failed), pnpm build; contract agent: clippy/fmt clean, Playwright 318/319 (P1-7 pre-existing).
