# ACR oracle (calibration tooling, local only)

Measures Adobe Camera Raw's behaviour so Sieve's develop pipeline can match Lightroom. Needs
the free Adobe DNG Converter (`/Applications/Adobe DNG Converter.app`), `exiftool`, and
Python 3 with numpy + Pillow (`/usr/bin/python3` on macOS has both in the user site).

How it works: `oracle.py` writes synthetic linear DNGs (camera space = linear ProPhoto, D50)
with embedded `crs:` settings, has DNG Converter render them (the full-size sRGB preview is
Camera Raw's render) and reads the pixels back. Only measured *behaviour* is kept (fitted
tables); no Adobe code or data is copied. Real frames are converted in place (read-only) by
`dng_ref.sh` / `dng_list.sh` / `variants.sh` into `test-data/` (gitignored).

| Script | Produces |
|---|---|
| `exp_tone.py`, `exp_ramp.py`, `exp_expo.py`, `exp_combo.py`, `exp_more.py` | neutral-ramp responses (`*.json`, `tone_default.npy`) |
| `tone_model.py <out.rs>` | `src/develop/tone_data.rs` (base curve, slider and exposure tables) |
| `exp_curves.py`, `param_model.py <out.rs>` | `src/develop/param_data.rs` (parametric curve shapes); `spline_test.py`, `order_test.py` check the point curve |
| `exp_adapt*.py`, `exp_curve_adapt.py` | image-adaptivity of Shadows/Highlights (key / white relative) and curves (none) |
| `exp_color.py` | colour sliders vs `examples/oracle_render.rs` (calibration, HSL, vibrance, grading gains) |
| `variants.sh`, `variants_cmp.sh`, `slider_resp.py`, `diag.py` | per-slider comparisons on a real frame; parity diagnostics |
| `bt.py`, `lt.py` | Adobe big-table (`crs:Table_*`) decoding experiments |

End-to-end parity: `cargo run --release --example parity_eval` (see its docs).

## Real-frame oracle (fix round 1, 2026-09-30)

Settings *variants* of the user's edited frames, rendered by Camera Raw from DNG copies and
by Sieve from the identical XMP, so each stage can be measured on real images. Work dir
`$P2` (default `<repo>/test-data/p2`): `dng/` (DNG copies, `-p0`, delete when done),
`var/<variant>/<stem>.{ref.jpg,xmp}`, `out/<variant>/` (Sieve renders, `SIEVE_DUMP` PPMs).
Sources are only read (never write under `~/Pictures`).

| Script | Does |
|---|---|
| `variant.sh`, `variants.py <list> <v,...>` | Camera Raw renders of settings variants (`neutral`, `tone`, `notone`, `nocolor`, single-slider sweeps `nS+100` ..., `z*` isolations, `name=set:neutral\|Tag=v;...`) |
| `eval.sh <list> <v\|-\|hold>`, `table.py`, `dump_variants.py`, `diag_variants.py` | Sieve vs Camera Raw dE2000 per image/variant (`-` = fit-set refs `$P2/ref`, `hold` = held-out `$P2/hold-ref`) |
| `diff.py <out-dir> [stems]` | dL / da / db and dE binned by L* |
| `tonefit.py`, `scale_probe.py`, `adapt_probe.py`, `local_probe.py` | slider delta in scene EV per pixel (inverting Camera Raw's neutral tone mapping); spatial / image-adaptivity probes |
| `fit_local.py`, `gen_local.py`, `fit_ref.py` | first local-operator fits (Python features) |
| `fit2.py [--write] [--loo]` | final fit (bilateral-grid bases since round 2) on features computed by Sieve (`parity_eval` with `SIEVE_DUMP_EV=1` on `neutral`): references + tables -> `src/develop/local_tone_data.rs` |
| `check_local.py`, `halo_probe.py`, `align_probe.py`, `cc_probe.py`, `montage.py`, `settings.py` | per-image bias, Camera Raw step-edge halos, frame alignment, CameraCalibration derivation, visual montages, sidecar settings dump |

Refit after changing the adaptation filter: build `parity_eval`, run
`SIEVE_DUMP_EV=1 SIEVE_DUMP=1 ./eval.sh $P2/fit-list.txt neutral`, re-dump the sweep variants
(`dump_variants.py ... nS+50,...,nH-100`), then `python3 fit2.py --write`.

## Local (mask) sliders (Phase 8)

Isolated Camera Raw responses of single local sliders on the 10 `mask-render` reference
frames, with a geometric mask both renderers evaluate identically:

1. `cargo run --release --example local_variants -- --reference test-data/mask-render/ref --out test-data/mask-polish/varxmp`
   writes each frame's sidecar (read in place) with its masks replaced by one radial mask
   carrying one slider (`temp-20`, `temp+40`, `temp-60`, `tint+-30`, `clar-20/-60`,
   `tex-15/-60`, `contr-16/+30`, `expo+50`).
2. `XMP_FILE=test-data/mask-polish/varxmp/<v>/STEM.xmp python3 mask_variants.py r-<v> <all local names>`
   renders them with DNG Converter (AI masks cannot be varied: DNG Converter drops an edited
   AI mask).
3. `mask_render --xmp-dir test-data/mask-polish/varxmp/<v> --reference test-data/mask-polish/var/r-<v> --reference-nomask test-data/mask-render/ref-nomask --only-ref`
   prints per frame the in-mask dE, the *floor* (Sieve vs Camera Raw with no masks on either
   side: the global pipeline's share) and the *local effect error* (Lab distance between the
   two renderers' mask-minus-no-mask differences), plus mean effect dLab / micro-contrast.

The fitted constants live in `src/develop/local.rs` (`LOCAL_TEMP_WARM/COOL`, `LOCAL_TINT*`,
`LOCAL_*_CURVE`).

## Parity round 2 (2026-09-30)

| Script | Does |
|---|---|
| `agg.py <variant>...` | dL / dE / dC by reference L* over all frames of a variant's `out/` dir |
| `resp.py <variant>...` | slider response: Camera Raw vs Sieve dL / dC per neutral-L* band (Dehaze / Clarity calibration) |
| `clip_shoulder.py` | equivalent scene-EV shift per image near the raw clip (the clip-anchored highlight shoulder) |
| `clip_regions.py STEM...` | colour of raw-clipped regions by number of clipped channels (`parity_eval` with `SIEVE_DUMP_CAM=1`, out dir `$P2/out/clip`) |

`parity_eval` also dumps white-balanced camera RGB with `SIEVE_DUMP_CAM=1` (`<stem>.cam.f32` +
`<stem>.cam.txt`). The adaptation bases are hyper-Gaussian bilateral grids now (`fit2.py`
`BASE`, written into `local_tone_data.rs`); `fit2.py` drops EV deltas beyond 4 EV (low-key
frames make the neutral tone-map inversion ill-conditioned). The Clarity / Texture / Dehaze
sweeps (`nCl`, `nTx`, `nDh` +-50) were rendered for the fit set with `variants.py`.
