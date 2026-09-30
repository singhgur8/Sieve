//! The sliders the model regresses per image ([`TARGETS`]) and the "style template" that
//! supplies everything else (profile, HSL, curves, colour grading, calibration, detail
//! settings, effects, B&W): the user's most frequent value set per camera.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::FrameContext;
use crate::ipc::types::{ParametricAdjustments, WhiteBalance, WhiteBalanceValues};

/// Reference white balance when a frame has no as-shot values.
const REFERENCE_WB: WhiteBalanceValues = WhiteBalanceValues { temperature_k: 6500.0, tint: 0.0 };

/// What a target encoding may depend on besides the adjustments.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TargetFrame {
    /// The camera's as-shot white balance (reference 6500 K / 0 if unknown).
    pub as_shot: WhiteBalanceValues,
    /// log2 mean luminance of the neutral render.
    pub log_mean_luma: f32,
}

impl TargetFrame {
    pub fn of(ctx: &FrameContext) -> TargetFrame {
        TargetFrame { as_shot: ctx.as_shot.unwrap_or(REFERENCE_WB), log_mean_luma: ctx.render.log_mean_luma }
    }

    /// Frame-independent encoding (template reset, tests).
    pub fn reference() -> TargetFrame {
        TargetFrame { as_shot: REFERENCE_WB, log_mean_luma: 0.0 }
    }
}

/// A regressed slider encoding: read from / written to `ParametricAdjustments` in model
/// units. Encodings of one slider share a `slot`; training keeps the one with the lower
/// cross-validated error (their errors are in the same units).
pub struct Target {
    /// Stable id (stored in the model file).
    pub name: &'static str,
    /// The slider this encodes (one model per slot).
    pub slot: &'static str,
    /// Report group.
    pub group: &'static str,
    /// Allowed range in model units.
    pub min: f32,
    pub max: f32,
    get: fn(&ParametricAdjustments, &TargetFrame) -> f32,
    set: fn(&mut ParametricAdjustments, f32, &TargetFrame),
}

impl Target {
    pub fn get(&self, adj: &ParametricAdjustments, f: &TargetFrame) -> f32 {
        (self.get)(adj, f)
    }

    pub fn set(&self, adj: &mut ParametricAdjustments, value: f32, f: &TargetFrame) {
        (self.set)(adj, value.clamp(self.min, self.max), f)
    }
}

fn wb(adj: &ParametricAdjustments, as_shot: WhiteBalanceValues) -> WhiteBalanceValues {
    match adj.white_balance {
        WhiteBalance::Custom { temperature_k, tint } => WhiteBalanceValues { temperature_k, tint },
        WhiteBalance::AsShot => as_shot,
    }
}

fn mired(k: f32) -> f32 {
    1e6 / k.max(1000.0)
}

fn set_wb(adj: &mut ParametricAdjustments, as_shot: WhiteBalanceValues, k: Option<f32>, tint: Option<f32>) {
    let cur = wb(adj, as_shot);
    let k = k.unwrap_or(cur.temperature_k).clamp(2000.0, 50000.0);
    let tint = tint.unwrap_or(cur.tint).clamp(-150.0, 150.0);
    adj.white_balance = WhiteBalance::Custom { temperature_k: (k / 10.0).round() * 10.0, tint: tint.round() };
}

/// `v` rounded to a multiple of `step` (exact decimal steps below 1).
fn round_to(v: f32, step: f32) -> f32 {
    if step >= 1.0 {
        (v / step).round() * step
    } else {
        let inv = (1.0 / step).round();
        (v * inv).round() / inv
    }
}

macro_rules! slider {
    ($name:literal, $group:literal, $min:expr, $max:expr, $step:expr, $($f:ident).+) => {
        Target {
            name: $name,
            slot: $name,
            group: $group,
            min: $min,
            max: $max,
            get: |a, _| a.$($f).+,
            set: |a, v, _| a.$($f).+ = round_to(v, $step),
        }
    };
}

/// Regressed slider encodings.
/// - Temperature: mired offset from the camera's as-shot value (cameras on auto WB) or
///   absolute mired (fixed camera WB / a user who dials one look temperature).
/// - Tint: offset from as-shot, or absolute.
/// - Exposure: the slider, or the target brightness `exposure + log2 mean luma of the neutral
///   render` (a user who brings frames to one brightness).
pub const TARGETS: &[Target] = &[
    Target {
        name: "temperatureMired",
        slot: "temperature",
        group: "whiteBalance",
        min: -150.0,
        max: 150.0,
        get: |a, f| mired(wb(a, f.as_shot).temperature_k) - mired(f.as_shot.temperature_k),
        set: |a, v, f| set_wb(a, f.as_shot, Some(1e6 / (mired(f.as_shot.temperature_k) + v).max(20.0)), None),
    },
    Target {
        name: "temperatureAbsMired",
        slot: "temperature",
        group: "whiteBalance",
        min: 20.0,
        max: 500.0,
        get: |a, f| mired(wb(a, f.as_shot).temperature_k),
        set: |a, v, f| set_wb(a, f.as_shot, Some(1e6 / v), None),
    },
    Target {
        name: "tint",
        slot: "tint",
        group: "whiteBalance",
        min: -60.0,
        max: 60.0,
        get: |a, f| wb(a, f.as_shot).tint - f.as_shot.tint,
        set: |a, v, f| set_wb(a, f.as_shot, None, Some(f.as_shot.tint + v)),
    },
    Target {
        name: "tintAbs",
        slot: "tint",
        group: "whiteBalance",
        min: -150.0,
        max: 150.0,
        get: |a, f| wb(a, f.as_shot).tint,
        set: |a, v, f| set_wb(a, f.as_shot, None, Some(v)),
    },
    slider!("exposure", "tone", -5.0, 5.0, 0.01, exposure),
    Target {
        name: "exposureBrightness",
        slot: "exposure",
        group: "tone",
        min: -25.0,
        max: 10.0,
        get: |a, f| a.exposure + f.log_mean_luma,
        set: |a, v, f| a.exposure = round_to((v - f.log_mean_luma).clamp(-5.0, 5.0), 0.01),
    },
    slider!("contrast", "tone", -100.0, 100.0, 1.0, contrast),
    slider!("highlights", "tone", -100.0, 100.0, 1.0, highlights),
    slider!("shadows", "tone", -100.0, 100.0, 1.0, shadows),
    slider!("whites", "tone", -100.0, 100.0, 1.0, whites),
    slider!("blacks", "tone", -100.0, 100.0, 1.0, blacks),
    slider!("texture", "presence", -100.0, 100.0, 1.0, texture),
    slider!("clarity", "presence", -100.0, 100.0, 1.0, clarity),
    slider!("dehaze", "presence", -100.0, 100.0, 1.0, dehaze),
    slider!("vibrance", "presence", -100.0, 100.0, 1.0, vibrance),
    slider!("saturation", "presence", -100.0, 100.0, 1.0, saturation),
    slider!("sharpening", "detail", 0.0, 150.0, 1.0, detail.sharpening.amount),
    slider!("luminanceNr", "detail", 0.0, 100.0, 1.0, detail.noise_reduction.luminance),
];

pub fn target(name: &str) -> Option<&'static Target> {
    TARGETS.iter().find(|t| t.name == name)
}

/// Settings a style never carries: framing (crop) and local masks (subject-specific).
pub fn strip_per_image(adj: &ParametricAdjustments) -> ParametricAdjustments {
    let mut a = adj.clone();
    a.crop = Default::default();
    a.masks.clear();
    a
}

/// `adj` with the regressed sliders reset to their defaults (white balance as shot) and the
/// per-image settings stripped: the part of an edit a template holds.
pub fn template_part(adj: &ParametricAdjustments) -> ParametricAdjustments {
    let mut a = strip_per_image(adj);
    let d = ParametricAdjustments::default();
    let f = TargetFrame::reference();
    for t in TARGETS {
        t.set(&mut a, t.get(&d, &f), &f);
    }
    a.white_balance = WhiteBalance::AsShot;
    a
}

/// A camera's most frequent template.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraTemplate {
    pub camera_key: String,
    /// Training edits from this camera / with exactly this template.
    pub samples: u32,
    pub support: u32,
    pub adjustments: ParametricAdjustments,
}

/// Most frequent [`template_part`] per camera key, and overall (`None` if empty).
pub fn mode_templates<'a>(
    items: impl IntoIterator<Item = (String, &'a ParametricAdjustments)>,
) -> (Vec<CameraTemplate>, Option<CameraTemplate>) {
    let mut per: HashMap<String, HashMap<String, (u32, ParametricAdjustments)>> = HashMap::new();
    let mut all: HashMap<String, (u32, ParametricAdjustments)> = HashMap::new();
    for (key, adj) in items {
        let t = template_part(adj);
        let s = serde_json::to_string(&t).unwrap_or_default();
        per.entry(key).or_default().entry(s.clone()).or_insert((0, t.clone())).0 += 1;
        all.entry(s).or_insert((0, t)).0 += 1;
    }
    let best = |key: &str, m: &HashMap<String, (u32, ParametricAdjustments)>| {
        let total = m.values().map(|v| v.0).sum();
        // Deterministic: highest count, then the smallest JSON.
        m.iter().max_by(|a, b| a.1 .0.cmp(&b.1 .0).then_with(|| b.0.cmp(a.0))).map(|(_, (c, adj))| CameraTemplate {
            camera_key: key.to_owned(),
            samples: total,
            support: *c,
            adjustments: adj.clone(),
        })
    };
    let mut cams: Vec<CameraTemplate> = per.iter().filter_map(|(k, m)| best(k, m)).collect();
    cams.sort_by(|a, b| a.camera_key.cmp(&b.camera_key));
    (cams, best("*", &all))
}
