# Project: Sieve (Autonomous Local Culling, Editing & RAW Engine)

## Vision
A 100% offline, local desktop application built with Tauri v2 (Rust + React/TypeScript) designed to replace Adobe Lightroom Classic for culling, editing, and exporting high-volume shoots.

## Core Pillars & Specs
1. Camera Pipeline Priority:
   - Sony (.ARW) -> Fuji (.RAF with X-Trans & Bayer support) -> Canon (.CR3).
2. Intelligent Culling Engine:
   - Evaluates: Blinks, Face Sharpness (Laplacian on face crop), Global Sharpness, Exposure Clipping, Composition Score.
   - Tagging System: Every photo receives explicit status tags (`blink`, `missed_focus`, `motion_blur`, `creative_blur`, `underexposed`, `duplicate_burst`).
   - Non-destructive Filtering: Users can filter by any tag combination (e.g., viewing motion-blurred frames intentionally).
   - Burst Grouping: Clusters shots taken within configurable time windows (e.g., <= 1.5s) and semantic similarity.
   - Shoot Context: Prioritize subjects based on shoot mode (e.g., Wedding/Couple, Portrait, Sports/Action).
3. Editing & Color Pipeline:
   - Parametric Sliders: Exposure, Contrast, Highlights, Shadows, Whites, Blacks, Temp, Tint, Vibrance, Saturation, HSL/Color Mixer.
   - 3D LUT Support: Ingest and apply standard `.cube` lookup tables.
   - Dual Compatibility: Write edits to standard Adobe `.xmp` sidecar files AND develop internally via LibRaw for instant full-res JPEG/TIFF export.
4. Few-Shot Scene Matching (One-Shot Learning):
   - User grades 1-2 anchor photos in a lighting scenario.
   - System normalizes histograms and white point deltas, automatically applying relative adjustments across all matching frames in that burst/scene.

## User Workflow (drives priorities)
1. Import a project folder (a new folder per shoot; one catalog holds many).
2. Auto-cull, then review/override: pass/fail flags + 0–5 stars.
3. Culling results are written to XMP sidecars next to the RAWs (Lightroom/Bridge-readable):
   reject = `xmp:Rating -1`; stars = `xmp:Rating 0–5`; pick = `xmp:Label "Pick"`;
   auto tags = `lr:hierarchicalSubject` `Sieve|<tag>`. Never clobber unrelated XMP fields.
4. Edit keepers in-app (no export needed until culling + editing are done).
5. Export client deliverables with Lightroom-style presets: JPEG (quality), TIFF 8/16, PNG, optional
   WebP/HEIC; resize; sRGB / Display P3 / Adobe RGB with ICC; output sharpening; filename template;
   metadata options; saved presets.
- Primary shoot types: Wedding/Couples and Portrait (eyes open + eye sharpness dominate scoring).
- Platform: macOS (Apple Silicon, CoreML). Windows is out of scope until a test machine exists.

## Tech Stack
- Host: Tauri v2
- Backend: Rust (`libraw-rs` for decoding/export, `onnxruntime` via CoreML/DirectML, `rayon` for thread pooling)
- Frontend: React + TypeScript + Vite + Tailwind CSS + Lucide Icons + Virtualized Grid
- State/Storage: SQLite (embedded via `rusqlite` for instant catalog search) + In-memory Rust cache

## Active Personas & Execution Boundaries
- [Architect]: Focuses on specs, schemas, and IPC definitions. Operates in Plan Mode.
- [Rust Engine Dev]: Focuses on `src-tauri/`, LibRaw pipeline, multi-threaded thumbnail extraction, and XMP read/write.
- [Vision/ML Dev]: Focuses on ONNX runtime integration (SCRFD face detection, Eye Aspect Ratio, Laplacian blur metrics).
- [Frontend Dev]: Focuses on `src/`, 60fps virtualized filmstrip, dual-pane loupe comparison, and slider controls.

## Milestone Roadmap
Detailed tasks, owners and acceptance criteria: `docs/roadmap.md` (source of truth). Summary:
- [x] Phase 1: Tauri v2 Scaffold + Core IPC Data Contracts (Image, CullTags, EditParams, CatalogState)
- [ ] Phase 2: Ultra-fast embedded thumbnail extraction pipeline (Sony, Fuji, Canon)
- [ ] Phase 3: Culling & Burst detection worker with tag emission
- [ ] Phase 4: Virtualized Photo Grid + Tag Filter + Loupe Zoom View
- [ ] Phase 5: Parametric Slider Engine, .CUBE LUT Parser & XMP Exporter
- [ ] Phase 6: Full RAW internal demosaic & JPEG export engine
- [ ] Phase 7: Anchor-photo scene matching (One-Shot relative grading)

## Development
- Toolchain: Rust stable (rustup; `~/.cargo/bin` may need adding to PATH), Node 24, pnpm (via corepack),
  `brew install libraw exiftool` (LibRaw + libjpeg-turbo are linked dynamically).
- Run app: `pnpm tauri dev` (set `SIEVE_CATALOG=/tmp/x.sqlite` for a scratch catalog).
- Rust tests/lint: `cd src-tauri && cargo test && cargo clippy --all-targets && cargo fmt --check`.
- Frontend type check + build: `pnpm build`.
- IPC contract: Rust types in `src-tauri/src/ipc/` are the source of truth; `src/ipc/bindings.ts` is generated
  (never edit it). Regenerate with `UPDATE_BINDINGS=1 cargo test bindings`; log changes in `docs/ipc-changelog.md`.
- Architecture, ownership map and schema: `docs/architecture.md`.

## Autonomous Orchestration
The main session is the orchestrator. It runs `docs/roadmap.md` to completion without checking in with the user.
1. Read `docs/roadmap.md`; the first unchecked task is next. Work on branch `phase-N-<slug>`.
2. Contract/schema changes go to the `architect` agent first. Then spawn specialists (`rust-engine-dev`,
   `vision-ml-dev`, `frontend-dev`) — in parallel with `isolation: "worktree"` when they touch disjoint paths —
   and merge their work. If a custom agent type is unavailable, spawn `general-purpose` with the contents of
   its `.claude/agents/<name>.md` as the role prompt.
3. After each task, `qa-engineer` runs the baseline gate + the phase's acceptance checks and reports pass/fail per
   task with the owning agent for each failure. Route failures back to the owner; max 3 fix rounds per task.
4. On pass: tick the task, append to the Status Log in `docs/roadmap.md`, commit. At phase end, fast-forward
   merge the phase branch into `main` and continue with the next phase immediately.
5. Make judgment calls yourself and record them in `docs/decisions.md` (date, decision, why, alternatives).
   Only stop for true external blockers (missing hardware, credentials, paid services, or 3 failed fix rounds):
   write `docs/BLOCKED.md` with what is needed, then stop.
6. Never modify anything under `/Users/gurjotsingh/Pictures/`. Copy sample files into `test-data/` (gitignored)
   before any test that writes XMP or exports.
7. Verify with evidence: run the commands, view screenshots/previews, inspect outputs. Never tick a task on
   assumption. Do not push (no remote configured) unless the user adds one.
8. Items under "Future phases" in the roadmap are notes only; do not start them unless the user asks.
