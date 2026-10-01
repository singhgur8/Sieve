# Sieve — Final Report (2026-09-30)

Sieve is an offline macOS (Apple Silicon) desktop app — Tauri v2, Rust engine, React UI — for culling, editing and
exporting high-volume shoots, intended to replace Lightroom Classic for this workflow.

Update 2026-09-30 (Phase 8b, from your first hands-on test): Projects home page, guided Cull → Edit → Export,
Lightroom-style Develop layout, preset/profile library, Auto tone/WB, Auto edit in your style, XMP auto-save.

## How to use
0. **Projects**: the app opens on a Projects home page (one project per shoot). "New project" → pick folder → name →
   shoot type. Inside a project everything (grid, filters, scenes, counts, export) sees only that shoot; the TopBar
   switcher returns home. Removing a project never touches files. A step bar guides **Cull → Edit → Export**.
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
6. **Edit step** (Cmd+Alt+2): keepers grouped into scenes, one proposed representative per scene. Edit it (or
   "Auto edit (my style)" — trains on your edited photos, ≥ 20), then "Apply to scene" / "Apply all" copies the edit to
   the rest with exposure/WB matched per frame; every apply is reviewable ("needs a look") and undoable, scenes can be
   skipped. Develop follows Lightroom Classic's layout (Navigator/Presets/Snapshots/History left; Histogram, Basic …
   Calibration right; Copy…/Paste, Previous/Reset; Cmd+Shift+C/V, Cmd+Alt+V, Sync…). "Import presets & profiles…"
   takes whole Lightroom preset/profile folders; Auto (Cmd+U) and Auto WB (Cmd+Shift+U); Shift+double-click a slider
   = auto that slider. Compare view (C) in Library and Develop.
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
| Packaging | Self-contained `Sieve.app` (64 MB) / `.dmg` (37 MB), bundled dylibs + culling models; bundle smoke (import → analysis → render → export) passes with no Homebrew on PATH |
| Projects + workflow | Two projects scoped correctly on the real backend; remove leaves files byte-identical; old catalogs migrate one project per folder (your 396 + 39); plan/apply/undo persisted (linear batch undo) |
| Presets & profiles | 535 of your/Adobe presets apply exactly their keys (29,606 checked); profile Amount 0/100/200 vs Camera Raw ΔE 2.1 / 2.7 / 3.6 |
| Auto tone / WB | vs Camera Raw's own Auto: exposure 0.17 EV, render ΔL* 2.8; Auto WB 565 K (as-shot 1142 K); skin clip 0/202 face frames (1/202 on unanalysed photos, same as Camera Raw) |
| Auto edit (my style) | Held-out 149 of your edits: ΔE2000 5.82 vs Auto tone 7.19 vs no edit 9.42 (better than Auto on 108/149); predict ~20 ms warm, train ~30 s |
| XMP auto-save | On by default, debounced, Lightroom crs: fields preserved; Saved/Saving/Error status, failure list + Retry |
| Tests | 450 Rust + 279 Playwright tests; clippy/fmt clean |

## Known gaps
- Stars/picks: culling suggestions barely predict this user's keepers (AUC ~0.57); style learning covers edits, not
  keeper choice.
- Auto edit (my style): roughly level with Auto tone on Canon frames; white balance is not really learned (stays near
  as-shot, as you usually do); trained on one shoot so far — it improves as you edit more.
- Develop Snapshots section is hidden (no backend yet); a few UX P2s remain (docs/ux-review-8b.md).
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
1. Use the guided workflow on a real new shoot and send feedback (round 2), especially Auto edit (my style).
2. A keeper-preference model (learn which frames you keep), the remaining culling gap.
3. Lens profile corrections, then healing/remove — the most-used Lightroom tools still missing.
4. A tone-interaction fit round on extreme Contrast/Whites/Blacks (closes the masked-frame floor).
5. Replace SCRFD with a commercially licensed detector and get a Developer ID before sharing the app.
