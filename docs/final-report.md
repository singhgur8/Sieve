# Sieve — Final Report (2026-09-30)

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
| Develop | Full Lightroom Basic/Tone curve/HSL/Color grading/Detail/Effects/Calibration, Adobe camera profiles + Looks (read locally), .cube LUTs, crop/straighten with live preview, presets, history, copy/paste/sync; slider draft frame ~19 ms while dragging, 2048 px render ~39 ms |
| Masks | Brush, linear, radial, luminance/color range, AI Subject/Sky/Background/People (+ face parts); your 50 Lightroom AI masks import and render (mattes decoded, byte-identical round trip) |
| Scene matching | Grade 1–2 anchors → 97% of targets within 0.15 EV / WB tolerance |
| Export | JPEG/TIFF 8/16/PNG/WebP/HEIC, resize, sRGB/P3/Adobe RGB ICC, output sharpening, templates, metadata options, presets; full-res JPEG ~3 files/s, 2048 px ~7 files/s, ≤ 0.95 GB peak |
| Robustness | Crash-safe catalog (WAL, integrity check, 3 rotating backups, read-only fallback, restore); missing originals flagged with "Locate folder…"; disk-full/read-only/undecodable errors explained; view-level error boundaries |
| Packaging | Self-contained `Sieve.app` (62 MB) / `.dmg` (38 MB), bundled dylibs + culling models; bundle smoke (import → analysis → render → export) passes with no Homebrew on PATH |
| Tests | 376 Rust + 168 Playwright tests; clippy/fmt clean |

## Known gaps
- Stars/picks: culling suggestions barely predict this user's keepers (AUC ~0.57); a content/preference model is
  Phase 10 (style learning).
- Face detection (SCRFD/insightface) weights are non-commercial; replace before any commercial distribution.
- Distribution beyond this Mac needs a Developer ID + notarization.
- Windows is out of scope.

## Remaining Lightroom-parity gaps
Measured against Adobe Camera Raw renders of your own edits (ΔE2000; < 1 invisible, 2–3 only side by side, > 5 obvious):
- **Global edits: 2.43 mean on held-out frames, worst 3.39** (16 held-out frames, ARW 2.80 / CR3 2.48 / RAF 2.08; the 20 tuning frames average 2.14). Close but not pixel-identical;
  differences are side-by-side visible, mostly tone in the mid tones.
- **Extreme tone settings**: frames combining Contrast −60…−91 with Whites −72 / Blacks +62 render mid/upper tones
  3–5 L* darker and darks ~2 L* brighter than Camera Raw (on your 10 masked Sony frames the unmasked render is already 3.38 from Camera Raw).
- **Masked frames: 3.42 whole frame / 3.54 inside masks** after parity round 2 (10 frames with your AI Subject
  masks; 3.38 of that is the unmasked floor above). On isolated single-slider Camera Raw tests the local sliders
  match within ~0.05 (e.g. local Clarity −60: ×0.921 vs ×0.922).
- **Point curves** push mid tones ~1–1.5 L* darker than Lightroom.
- **Shadows** lift a little too much next to large blown skies (AZA06487).
- **Clarity above +30** lacks Camera Raw's mid-tone lift (you only use negative Clarity).
- **Local Contrast** differs in kind (Camera Raw shifts L more, slope less).
- **Editing an imported AI mask**: Sieve keeps and renders Lightroom's AI mattes as-is. Re-computing one uses Sieve's
  own models (BiRefNet/U²-Net/YOLOX+EfficientSAM), so edges differ from Adobe's.
- **Not implemented**: lens profile corrections (distortion/vignetting/CA from Adobe lens profiles), Upright/transform,
  healing/clone/generative remove, Denoise AI / Super Resolution / HDR / panorama merge, tethering, print/book/web
  modules, face recognition by person.

## Recommended next steps
1. Phase 10 (style learning from your 394 edited XMPs) — the biggest win for your time, and it also covers the
   keeper-prediction gap.
2. Lens profile corrections, then healing/remove — the most-used Lightroom tools still missing.
3. A tone-interaction fit round on extreme Contrast/Whites/Blacks (closes the masked-frame floor).
4. Replace SCRFD with a commercially licensed detector and get a Developer ID before sharing the app.
