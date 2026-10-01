//! Per-frame inputs of the style model ([`FrameContext`]) and their flattening into a
//! numeric feature vector.
//!
//! Sources, all available in the catalog without user interaction:
//! - develop-source statistics: one *neutral* render (`ParametricAdjustments::defaults_for`
//!   the format, no crop) at `scene::STATS_MAX_EDGE`, measured by [`RenderFeatures::from_render`]
//!   (Phase 7 `ImageStats` plus a luma histogram, chroma and centre-vs-frame brightness);
//! - camera + EXIF: make, format, ISO, shutter, aperture, focal length, the camera's as-shot
//!   white balance, and the scene light level (EV100) derived from them;
//! - scene context: the frame's preview `SceneFeatures` (camera rendering) and the mean over
//!   its scene's members ([`SceneContext`]);
//! - Sieve's own Auto tone of the frame ([`AutoAnchor`]: `develop::auto::auto_tone` on the
//!   format defaults, as-shot WB, analysis faces), the per-frame anchor the tone sliders are
//!   predicted relative to (`targets`: `<slider>Auto` encodings).

use serde::{Deserialize, Serialize};

use crate::develop::pipeline::RenderedImage;
use crate::ipc::types::{AutoToneValues, ImageFormat, ParametricAdjustments, WhiteBalanceValues};
use crate::scene::{color, stats, SceneFeatures};

/// Bump when [`RenderFeatures`] / [`feature_vector`] change (a model trained on other
/// features is retrained, stored features recomputed).
pub const FEATURES_VERSION: u32 = 2;

/// Luma histogram bins of [`RenderFeatures::luma_hist`] (sRGB-encoded luma 0..=1).
pub const HIST_BINS: usize = 16;

/// Statistics of the neutral render (8-bit sRGB, orientation applied).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderFeatures {
    /// log2 geometric-mean luminance (Phase 7 `ImageStats::log_mean_luma`).
    pub log_mean_luma: f32,
    /// log2 of the luminance percentiles p1, p10, p50, p90, p99.
    pub log_percentiles: [f32; 5],
    pub clipped_highlights: f32,
    pub clipped_shadows: f32,
    /// Mean Oklab L, a, b.
    pub mean_oklab: [f32; 3],
    /// Oklab a/b of the neutral-surface estimate and its coverage (Phase 7).
    pub neutral_ab: [f32; 2],
    pub neutral_coverage: f32,
    /// Normalized histogram of encoded luma ([`HIST_BINS`] bins, sums to 1).
    pub luma_hist: Vec<f32>,
    /// Mean Oklab chroma.
    pub mean_chroma: f32,
    /// log2 mean luma of the central 50 % area minus that of the whole frame (backlit
    /// subject < 0, spot-lit subject > 0).
    pub center_delta: f32,
}

impl RenderFeatures {
    /// Measures a neutral render (the `RenderedImage` of `DevelopCache::render_image` or
    /// `pipeline::render`).
    pub fn from_render(img: &RenderedImage) -> RenderFeatures {
        let st = stats::measure(0, img, None);
        let (w, h) = (img.width as usize, img.height as usize);
        let n = (w * h).min(img.rgb.len() / 3);
        let lin = color::srgb_to_linear_table();
        let stride = ((n as f64 / 60_000.0).sqrt().ceil() as usize).max(1);
        let mut hist = vec![0.0f32; HIST_BINS];
        let (mut chroma, mut count) = (0.0f64, 0usize);
        let (mut c_sum, mut c_n, mut all_sum) = (0.0f64, 0usize, 0.0f64);
        for y in (0..h).step_by(stride) {
            for x in (0..w).step_by(stride) {
                let i = (y * w + x) * 3;
                if i + 3 > n * 3 {
                    continue;
                }
                let rgb = [lin[img.rgb[i] as usize], lin[img.rgb[i + 1] as usize], lin[img.rgb[i + 2] as usize]];
                let yl = color::luma(rgb);
                let e = color::srgb_encode(yl);
                hist[((e * HIST_BINS as f32) as usize).min(HIST_BINS - 1)] += 1.0;
                let lab = color::oklab(rgb);
                chroma += f64::from(lab[1].hypot(lab[2]));
                count += 1;
                all_sum += f64::from(yl);
                if x >= w / 4 && x < w - w / 4 && y >= h / 4 && y < h - h / 4 {
                    c_sum += f64::from(yl);
                    c_n += 1;
                }
            }
        }
        let cnt = count.max(1) as f32;
        hist.iter_mut().for_each(|v| *v /= cnt);
        let mean_all = (all_sum / count.max(1) as f64).max(1e-5);
        let mean_c = (c_sum / c_n.max(1) as f64).max(1e-5);
        let lp = |v: f32| v.max(color::MIN_LUMA).log2();
        let p = st.percentiles;
        RenderFeatures {
            log_mean_luma: st.log_mean_luma,
            log_percentiles: [lp(p.p1), lp(p.p10), lp(p.p50), lp(p.p90), lp(p.p99)],
            clipped_highlights: st.clipped_highlights,
            clipped_shadows: st.clipped_shadows,
            mean_oklab: [st.mean_oklab.l, st.mean_oklab.a, st.mean_oklab.b],
            neutral_ab: [st.neutral.a, st.neutral.b],
            neutral_coverage: st.neutral.coverage,
            luma_hist: hist,
            mean_chroma: color::finite((chroma / count.max(1) as f64) as f32, 0.0),
            center_delta: color::finite((mean_c / mean_all).log2() as f32, 0.0),
        }
    }
}

/// Mean appearance of the frame's scene (all members, edited or not), from the stored
/// preview [`SceneFeatures`] and EXIF.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneContext {
    pub members: u32,
    pub log_mean_luma: f32,
    pub mean_oklab: [f32; 3],
    /// Mean EV100 of the members that have exposure EXIF.
    pub ev100: Option<f32>,
}

impl SceneContext {
    /// Context of a scene from its members' preview features and EV100 values (`None`
    /// entries are skipped). `None` if no member has features.
    pub fn from_members<'a>(
        features: impl IntoIterator<Item = &'a SceneFeatures>,
        ev100: impl IntoIterator<Item = Option<f32>>,
    ) -> Option<SceneContext> {
        let (mut n, mut luma, mut lab) = (0u32, 0.0f64, [0.0f64; 3]);
        for f in features {
            n += 1;
            luma += f64::from(f.log_mean_luma);
            for (a, v) in lab.iter_mut().zip(f.mean_oklab) {
                *a += f64::from(v);
            }
        }
        if n == 0 {
            return None;
        }
        let evs: Vec<f32> = ev100.into_iter().flatten().filter(|v| v.is_finite()).collect();
        let nf = f64::from(n);
        Some(SceneContext {
            members: n,
            log_mean_luma: (luma / nf) as f32,
            mean_oklab: lab.map(|v| (v / nf) as f32),
            ev100: (!evs.is_empty()).then(|| evs.iter().sum::<f32>() / evs.len() as f32),
        })
    }
}

/// Sieve's Auto tone of a frame (`develop::auto::auto_tone` on the format defaults with the
/// as-shot white balance and the analysis faces): absolute slider values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoAnchor {
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub vibrance: f32,
    pub saturation: f32,
}

impl AutoAnchor {
    /// From a full `auto_tone` result (missing values = 0).
    pub fn from_values(v: &AutoToneValues) -> AutoAnchor {
        let g = |x: Option<f32>| x.filter(|v| v.is_finite()).unwrap_or(0.0);
        AutoAnchor {
            exposure: g(v.exposure),
            contrast: g(v.contrast),
            highlights: g(v.highlights),
            shadows: g(v.shadows),
            whites: g(v.whites),
            blacks: g(v.blacks),
            vibrance: g(v.vibrance),
            saturation: g(v.saturation),
        }
    }

    /// From the tone + presence sliders of `adj` (e.g. a reference Auto's result).
    pub fn from_adjustments(adj: &ParametricAdjustments) -> AutoAnchor {
        AutoAnchor {
            exposure: adj.exposure,
            contrast: adj.contrast,
            highlights: adj.highlights,
            shadows: adj.shadows,
            whites: adj.whites,
            blacks: adj.blacks,
            vibrance: adj.vibrance,
            saturation: adj.saturation,
        }
    }

    pub fn values(&self) -> [f32; 8] {
        [
            self.exposure,
            self.contrast,
            self.highlights,
            self.shadows,
            self.whites,
            self.blacks,
            self.vibrance,
            self.saturation,
        ]
    }
}

/// Everything the model sees about one frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameContext {
    pub format: ImageFormat,
    /// EXIF make / model (camera templates are keyed by [`camera_key`]).
    pub make: Option<String>,
    pub model: Option<String>,
    pub iso: Option<u32>,
    pub shutter_seconds: Option<f64>,
    pub aperture: Option<f32>,
    pub focal_length_mm: Option<f32>,
    /// The camera's as-shot white balance (`DevelopInfo.asShot` / `ImageStats.asShot`).
    pub as_shot: Option<WhiteBalanceValues>,
    /// Neutral render statistics.
    pub render: RenderFeatures,
    /// The frame's own preview features (scene detection's `scene_features` row).
    pub preview: Option<SceneFeatures>,
    pub scene: Option<SceneContext>,
    /// Sieve's Auto tone of the frame (`None` = not computed; anchored encodings then fall
    /// back to absolute values).
    #[serde(default)]
    pub auto: Option<AutoAnchor>,
    /// Capture time (the model's "most recent edit" lookup; `None` = unknown).
    #[serde(default)]
    pub captured_at_ms: Option<i64>,
}

impl FrameContext {
    /// Scene light level: EV100 = log2(N^2 / t) - log2(ISO / 100).
    pub fn ev100(&self) -> Option<f32> {
        ev100(self.iso, self.shutter_seconds, self.aperture)
    }

    pub fn camera_key(&self) -> String {
        camera_key(self.make.as_deref(), self.model.as_deref())
    }
}

/// EV100 from EXIF (`None` if any value is missing or not positive).
pub fn ev100(iso: Option<u32>, shutter_seconds: Option<f64>, aperture: Option<f32>) -> Option<f32> {
    let (iso, t, n) = (f64::from(iso?), shutter_seconds?, f64::from(aperture?));
    (iso > 0.0 && t > 0.0 && n > 0.0).then(|| ((n * n / t).log2() - (iso / 100.0).log2()) as f32)
}

/// Normalized "make model" key of a camera (`"unknown"` if neither is known).
pub fn camera_key(make: Option<&str>, model: Option<&str>) -> String {
    let s = format!("{} {}", make.unwrap_or("").trim(), model.unwrap_or("").trim()).trim().to_ascii_lowercase();
    if s.is_empty() {
        "unknown".into()
    } else {
        s
    }
}

/// Names of [`feature_vector`] entries, in order.
pub fn feature_names() -> Vec<String> {
    let mut v: Vec<String> = [
        "isRaw",
        "makeSony",
        "makeCanon",
        "makeFuji",
        "log2Iso",
        "log2Shutter",
        "log2Aperture",
        "log2Focal",
        "ev100",
        "asShotMired",
        "asShotTint",
        "logMeanLuma",
        "logP1",
        "logP10",
        "logP50",
        "logP90",
        "logP99",
        "clipHigh",
        "clipLow",
        "oklabL",
        "oklabA",
        "oklabB",
        "neutralA",
        "neutralB",
        "neutralCoverage",
        "chroma",
        "centerDelta",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    v.extend((0..HIST_BINS).map(|i| format!("hist{i}")));
    v.extend(
        [
            "previewLogLuma",
            "previewL",
            "previewA",
            "previewB",
            "sceneLog2Members",
            "sceneLogLuma",
            "sceneL",
            "sceneA",
            "sceneB",
            "sceneEv100",
            "ev100MinusScene",
            "previewLumaMinusScene",
            "autoExposure",
            "autoContrast",
            "autoHighlights",
            "autoShadows",
            "autoWhites",
            "autoBlacks",
            "autoVibrance",
            "autoSaturation",
            "autoBrightness",
        ]
        .iter()
        .map(|s| (*s).to_owned()),
    );
    v
}

/// Numeric features (`NaN` = missing; the model imputes training medians).
pub fn feature_vector(ctx: &FrameContext) -> Vec<f32> {
    let nan = f32::NAN;
    let make = ctx.make.as_deref().unwrap_or("").to_ascii_lowercase();
    let is = |p: &str| if make.starts_with(p) { 1.0 } else { 0.0 };
    let log2 = |v: Option<f64>| v.filter(|v| *v > 0.0).map_or(nan, |v| v.log2() as f32);
    let ev = ctx.ev100();
    let r = &ctx.render;
    let mut v = vec![
        if ctx.format.is_raw() { 1.0 } else { 0.0 },
        is("sony"),
        is("canon"),
        is("fuji"),
        log2(ctx.iso.map(f64::from)),
        log2(ctx.shutter_seconds),
        log2(ctx.aperture.map(f64::from)),
        log2(ctx.focal_length_mm.map(f64::from)),
        ev.unwrap_or(nan),
        ctx.as_shot.map_or(nan, |w| 1e6 / w.temperature_k.max(1000.0)),
        ctx.as_shot.map_or(nan, |w| w.tint),
        r.log_mean_luma,
    ];
    v.extend(r.log_percentiles);
    v.extend([r.clipped_highlights, r.clipped_shadows]);
    v.extend(r.mean_oklab);
    v.extend(r.neutral_ab);
    v.extend([r.neutral_coverage, r.mean_chroma, r.center_delta]);
    let mut hist = r.luma_hist.clone();
    hist.resize(HIST_BINS, 0.0);
    v.extend(hist);
    match &ctx.preview {
        Some(p) => v.extend([p.log_mean_luma, p.mean_oklab[0], p.mean_oklab[1], p.mean_oklab[2]]),
        None => v.extend([nan; 4]),
    }
    match &ctx.scene {
        Some(s) => {
            v.extend([(s.members.max(1) as f32).log2(), s.log_mean_luma]);
            v.extend(s.mean_oklab);
            v.push(s.ev100.unwrap_or(nan));
            v.push(match (ev, s.ev100) {
                (Some(a), Some(b)) => a - b,
                _ => nan,
            });
            v.push(ctx.preview.as_ref().map_or(nan, |p| p.log_mean_luma - s.log_mean_luma));
        }
        None => v.extend([nan; 8]),
    }
    match &ctx.auto {
        Some(a) => {
            v.extend(a.values());
            v.push(a.exposure + r.log_mean_luma);
        }
        None => v.extend([nan; 9]),
    }
    v.iter_mut().for_each(|x| {
        if x.is_infinite() {
            *x = nan;
        }
    });
    v
}
