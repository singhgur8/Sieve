# Sieve — Final Report (draft; finalized after the Phase 8 QA gate)

Sieve is an offline macOS (Apple Silicon) desktop app — Tauri v2, Rust engine, React UI — for culling, editing and
exporting high-volume shoots, intended to replace Lightroom Classic for this workflow.

## How to use
1. **Install**: open `Sieve.dmg` (built with `pnpm tauri build`), drag Sieve to Applications. First launch: right-click →
   Open (ad-hoc signed, not notarized). macOS 15.0+. Adobe DNG Converter / Camera Raw installed locally is optional but
   gives Lightroom-matching camera profiles and Looks (read in place, never copied).
2. **Import** (Cmd+Shift+I): pick a shoot folder. ARW, RAF (X-Trans + Bayer), CR3, JPEG/HEIC/TIFF/PNG. Existing
   Lightroom XMP sidecars (ratings, labels, develop settings, masks) are read.
3. **Cull**: Analyze runs automatically (blink, focus, motion/creative blur, exposure, bursts, per-shoot-type scoring).
   Review with P/X/U, 0–5, 6–9, arrows, Space (loupe), C (compare), F (face zoom), K (burst keeper), Cmd+Z. Filter by
   any tag combination. "Apply suggestions" only fills unset photos by default and is undoable. `?` shows all shortcuts.
4. **Save metadata** (Cmd+S or auto-sync): XMP sidecars next to the originals — reject `xmp:Rating -1`, stars 0–5,
   pick `xmp:Label "Pick"`, tags `Sieve|<tag>`; unrelated fields preserved byte for byte.
5. **Develop** (D): Basic, tone curve, HSL/Color Mixer, color grading, detail, effects, calibration, camera profiles,
   LUTs (.cube), crop/straighten, masks (brush, linear, radial, range, AI Subject/Sky/Background/People) — AI mask
   models download once from the Masks panel (~560 MB). Copy/paste/sync (Cmd+Shift+C/V/S), presets, history,
   before/after (`\`, Y).
6. **Scenes**: detect scenes, grade 1–2 anchors (Shift+A), Match scene applies relative adjustments to the rest.
7. **Export** (Cmd+Shift+E): JPEG/TIFF 8/16/PNG/WebP/HEIC, resize, sRGB/P3/Adobe RGB with ICC, output sharpening,
   filename template, metadata options, saved presets.

## What works (verified; see Status Log in `docs/roadmap.md`)
| Area | Evidence (this Mac, M-series) |
|---|---|
| Ingest (ARW/RAF X-Trans+Bayer/CR3/JPG) | 847 mixed RAWs (45 GB proposal shoot) all ready, ~100–108 files/s, ~330 MB peak; EXIF matches exiftool; originals never modified |
| Culling | SCRFD faces + 106-pt landmarks + eye-state CNN (CoreML), per-eye sharpness, motion vs creative blur, exposure, bursts, per-shoot-type scoring; 9 ms/image; calibrated on your picks: held-out keeper false-reject 0/115 |
| Culling UI | Virtualized grid (50k-image catalog: worst query 15 ms), tag filters, loupe/face zoom, 2-up compare, burst keeper, culling undo, Lightroom keys, `?` cheat sheet |
| XMP | Lightroom/Bridge-readable ratings, rejects, Pick label, `Sieve|<tag>` keywords; Lightroom develop settings and masks read and written; unchanged writes touch only `MetadataDate`; JPEG edits in `<name>.JPG.xmp` |
| Develop | Full Lightroom Basic/Tone curve/HSL/Color grading/Detail/Effects/Calibration, Adobe camera profiles + Looks (read locally), .cube LUTs, crop/straighten with live preview, presets, history, copy/paste/sync; slider draft frame ~29 ms while dragging |
| Masks | Brush, linear, radial, luminance/color range, AI Subject/Sky/Background/People (+ face parts); your 50 Lightroom AI masks import and render (mattes decoded, byte-identical round trip) |
| Scene matching | Grade 1–2 anchors → 97% of targets within 0.15 EV / WB tolerance |
| Export | JPEG/TIFF 8/16/PNG/WebP/HEIC, resize, sRGB/P3/Adobe RGB ICC, output sharpening, templates, metadata options, presets; full-res JPEG ~3 files/s, 2048 px ~7 files/s, ≤ 0.95 GB peak |
| Robustness | Crash-safe catalog (WAL, integrity check, 3 rotating backups, read-only fallback, restore); missing originals flagged with "Locate folder…"; disk-full/read-only/undecodable errors explained; view-level error boundaries |
| Packaging | Self-contained `Sieve.app` (58 MB) / `.dmg` (38 MB), bundled dylibs + culling models; bundle smoke (import → analysis → render → export) passes with no Homebrew on PATH |
| Tests | 364 Rust + 168 Playwright tests; clippy/fmt clean |

## Known gaps
- Stars/picks: culling suggestions barely predict this user's keepers (AUC ~0.57); a content/preference model is
  Phase 10 (style learning).
- Face detection (SCRFD/insightface) weights are non-commercial; replace before any commercial distribution.
- Distribution beyond this Mac needs a Developer ID + notarization.
- Windows is out of scope.

## Remaining Lightroom-parity gaps
_To be completed from parity round 2 and mask/XMP polish results._
