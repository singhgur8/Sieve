# Blocked: Phase 9 — Calibrate on the user's shoot (2026-10-10)

Everything else in Phase 9 is done and merged to `main`. This task needs data only the user has.

**Needed from the user** (no RAWs required):
1. JPEG previews of the 2,500-photo wedding, named like the RAWs. On the Mac, from the shoot folder:
   `mkdir -p ~/Desktop/sieve-calib && exiftool -b -PreviewImage -w ~/Desktop/sieve-calib/%f.jpg -ext arw -ext raf -ext cr3 -r .`
2. The final `.xmp` sidecars with the user's ~800 picks: `cp *.xmp ~/Desktop/sieve-calib/`
   (if culled in Lightroom: Metadata → Save Metadata to Files first, otherwise the picks are only in the Lightroom catalog).
3. A way to get the folder here, e.g. a Google Drive folder name (the Drive connector is attached), or run it locally:
   `cd src-tauri && cargo run --release --example target_eval -- ~/Desktop/sieve-calib --target 800 --shoot wedding`

**Then:** run `target_eval`, compare with the user's picks (overall / per shot type / per moment), tune the thresholds
(`NEAR_DUP_COUPLE`, `CANDID_MIN_VISIBLE`, `COVER_MIN`, clustering cuts), record numbers in `docs/decisions.md`, tick the task,
delete the previews.
