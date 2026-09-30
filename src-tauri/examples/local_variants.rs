//! Local-slider calibration variants (Phase 8): for each Camera Raw reference frame, writes
//! the frame's sidecar (read in place, never written) with its masks replaced by one radial
//! mask carrying a single local slider, so Camera Raw (DNG Converter, `tools/acr-oracle`) and
//! Sieve (`mask_render --xmp-dir`) render the identical geometric mask.
//!
//! ```text
//! cargo run --release --example local_variants -- --reference DIR --out DIR
//!     [--folder DIR]
//! ```
//! Writes `<out>/<variant>/<stem>.xmp` for every `<stem>.ref.jpg` in `--reference`.
//! Variants: `temp-20`, `temp+40`, `tint-30`, `temp-60`, `tint+30`, `clar-20`, `clar-60`, `tex-15`, `tex-60`,
//! `contr-16`, `contr+30`, `expo+50` (exposure +0.5 EV).

use std::path::PathBuf;

use sieve_lib::ipc::types::{LocalAdjustments, MaskBlendMode, MaskComponent, MaskGroup, MaskShape, RadialMask};
use sieve_lib::xmp;

type Set = fn(&mut LocalAdjustments);

const VARIANTS: [(&str, Set); 12] = [
    ("temp-20", |a| a.temperature = -20.0),
    ("temp+40", |a| a.temperature = 40.0),
    ("tint-30", |a| a.tint = -30.0),
    ("temp-60", |a| a.temperature = -60.0),
    ("tint+30", |a| a.tint = 30.0),
    ("clar-20", |a| a.clarity = -20.0),
    ("clar-60", |a| a.clarity = -60.0),
    ("tex-15", |a| a.texture = -15.0),
    ("tex-60", |a| a.texture = -60.0),
    ("contr-16", |a| a.contrast = -16.0),
    ("contr+30", |a| a.contrast = 30.0),
    ("expo+50", |a| a.exposure = 0.5),
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let reference = PathBuf::from(arg("--reference").expect("--reference"));
    let out = PathBuf::from(arg("--out").expect("--out"));
    let folder =
        PathBuf::from(arg("--folder").unwrap_or_else(|| "/Users/gurjotsingh/Pictures/Jasmit Natalie Proposal".into()));
    let stems: Vec<String> = std::fs::read_dir(&reference)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(".ref.jpg").map(str::to_owned))
        .collect();
    let sidecars: Vec<PathBuf> = walkdir::WalkDir::new(&folder)
        .into_iter()
        .flatten()
        .map(|e| e.path().to_path_buf())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("xmp")))
        .collect();
    for stem in &stems {
        let Some(sc) = sidecars.iter().find(|p| p.file_stem().is_some_and(|s| s == stem.as_str())) else {
            eprintln!("{stem}: no sidecar");
            continue;
        };
        let text = std::fs::read_to_string(sc).unwrap();
        for (name, set) in VARIANTS {
            let mut adjustments = LocalAdjustments::default();
            set(&mut adjustments);
            let group = MaskGroup {
                id: format!("{:032X}", 0xC0FFEEu32),
                name: name.into(),
                active: true,
                amount: 1.0,
                adjustments,
                components: vec![MaskComponent {
                    id: format!("{:032X}", 0xBEEFu32),
                    name: String::new(),
                    active: true,
                    mode: MaskBlendMode::Add,
                    inverted: false,
                    opacity: 1.0,
                    shape: MaskShape::Radial(RadialMask {
                        top: 0.15,
                        left: 0.2,
                        bottom: 0.85,
                        right: 0.8,
                        angle: 0.0,
                        midpoint: 50.0,
                        roundness: 0.0,
                        feather: 40.0,
                        flipped: false,
                    }),
                }],
            };
            let written = xmp::masks::apply(&text, &[group]).unwrap();
            let dir = out.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{stem}.xmp")), written).unwrap();
        }
        println!("{stem}: {} variants", VARIANTS.len());
    }
}
