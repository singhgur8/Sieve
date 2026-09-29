# Project: LumenRAW (Autonomous Local Culling, Editing & RAW Engine)

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
- [ ] Phase 1: Tauri v2 Scaffold + Core IPC Data Contracts (Image, CullTags, EditParams, CatalogState)
- [ ] Phase 2: Ultra-fast embedded thumbnail extraction pipeline (Sony, Fuji, Canon)
- [ ] Phase 3: Culling & Burst detection worker with tag emission
- [ ] Phase 4: Virtualized Photo Grid + Tag Filter + Loupe Zoom View
- [ ] Phase 5: Parametric Slider Engine, .CUBE LUT Parser & XMP Exporter
- [ ] Phase 6: Full RAW internal demosaic & JPEG export engine
- [ ] Phase 7: Anchor-photo scene matching (One-Shot relative grading)
