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
_To be completed from the Phase 8 QA gate._

## Known gaps
- Stars/picks: culling suggestions barely predict this user's keepers (AUC ~0.57); a content/preference model is
  Phase 10 (style learning).
- Face detection (SCRFD/insightface) weights are non-commercial; replace before any commercial distribution.
- Distribution beyond this Mac needs a Developer ID + notarization.
- Windows is out of scope.

## Remaining Lightroom-parity gaps
_To be completed from parity round 2 and mask/XMP polish results._
