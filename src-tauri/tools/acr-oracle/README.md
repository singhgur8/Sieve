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
