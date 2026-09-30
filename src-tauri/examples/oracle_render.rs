//! Calibration helper for `tools/acr-oracle`: renders a synthetic linear image whose camera
//! space is linear ProPhoto (white = 65535, like the oracle's synthetic DNGs) with the given
//! adjustments (JSON, `ParametricAdjustments`, missing fields = defaults) and writes a binary
//! PPM (8-bit sRGB).
//!
//! ```text
//! cargo run --release --example oracle_render -- <in.rgb16> <width> <height> <adj.json> <out.ppm>
//! ```
//! `in.rgb16`: interleaved little-endian u16 RGB.

use sieve_lib::develop::camera::Profile;
use sieve_lib::develop::pipeline;
use sieve_lib::develop::source::ColorInfo;
use sieve_lib::ipc::types::ParametricAdjustments;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let bytes = std::fs::read(&a[0]).expect("input");
    let (w, h): (u32, u32) = (a[1].parse().unwrap(), a[2].parse().unwrap());
    let px: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    // Neutral base (no detail processing, no profile) + the JSON overrides, merged.
    let mut base = ParametricAdjustments::default();
    base.detail.sharpening.amount = 0.0;
    base.detail.noise_reduction.color = 0.0;
    base.profile = sieve_lib::ipc::types::ProfileSettings::none();
    let mut v = serde_json::to_value(&base).unwrap();
    let over: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&a[3]).unwrap()).unwrap();
    fn merge(a: &mut serde_json::Value, b: &serde_json::Value) {
        match (a, b) {
            (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
                for (k, v) in b {
                    merge(a.entry(k.clone()).or_insert(serde_json::Value::Null), v);
                }
            }
            (a, b) => *a = b.clone(),
        }
    }
    merge(&mut v, &over);
    let adj: ParametricAdjustments = serde_json::from_value(v).unwrap();
    // XYZ (D50) -> linear ProPhoto as the "camera" matrix; neutral (1,1,1) = D50.
    let m = sieve_lib::develop::camera::XYZ_TO_PROPHOTO.map(|r| r.map(|v| v as f32));
    let color = ColorInfo {
        as_shot_mul: Some([1.0; 3]),
        daylight_mul: [1.0; 3],
        rgb_cam: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        xyz_to_cam: m,
        calibration: [1.0; 3],
    };
    let profile = Profile::matrix(0.0);
    let input = pipeline::RenderInput::simple(w, h, &px, &color, &profile);
    let out = pipeline::render(&input, &adj, None);
    let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
    ppm.extend_from_slice(&out.rgb);
    std::fs::write(&a[4], ppm).unwrap();
}
