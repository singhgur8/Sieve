//! Peak-memory probe of one full-resolution export develop (decode + pipeline + finish) with
//! the user's sidecar settings (or `--nr` user-like settings with noise reduction), in
//! memory only (nothing is written). Run under `/usr/bin/time -l` for the peak RSS.
//!
//! ```text
//! cargo run --release --example export_mem -- <raw> [--nr]
//! ```

use std::path::PathBuf;
use std::time::Instant;

use sieve_lib::develop::{camera, source};
use sieve_lib::export::develop;
use sieve_lib::ipc::types::{ExportPreset, ParametricAdjustments};
use sieve_lib::profiles::ProfileLibrary;
use sieve_lib::xmp;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = PathBuf::from(&args[0]);
    let format = sieve_lib::raw::format_from_extension(&path).expect("raw");
    let sidecar = xmp::sidecar_path(&path);
    let mut adj = std::fs::read_to_string(&sidecar)
        .ok()
        .and_then(|t| xmp::packet::parse_for(&t, format).ok())
        .and_then(|v| v.develop)
        .unwrap_or_else(|| ParametricAdjustments::defaults_for(format));
    if args.iter().any(|a| a == "--nr") {
        adj.detail.noise_reduction.luminance = 40.0;
        adj.detail.noise_reduction.luminance_contrast = 30.0;
        adj.detail.noise_reduction.color = 25.0;
        adj.shadows = 50.0;
        adj.highlights = -60.0;
        adj.clarity = -10.0;
    }
    let settings = ExportPreset::builtins().into_iter().next().expect("preset").settings;
    let t = Instant::now();
    let (src, meta) = source::decode_full_meta(&path).expect("decode");
    let profile = camera::resolve(&meta, &adj.profile, &ProfileLibrary::shared(), Some(&sidecar));
    let mut buf = Vec::new();
    let o = sieve_lib::raw::extract(&path, format, &mut buf).ok().and_then(|e| e.meta.orientation).map(|o| o as u8);
    drop(buf);
    let img = develop::render_full(&src, o, &adj, None, &settings, &profile, 1).expect("render");
    println!(
        "{}: {}x{} -> {}x{} in {:.0} ms (NR luminance {} color {})",
        path.display(),
        src.width,
        src.height,
        img.width,
        img.height,
        t.elapsed().as_secs_f64() * 1000.0,
        adj.detail.noise_reduction.luminance,
        adj.detail.noise_reduction.color
    );
}
