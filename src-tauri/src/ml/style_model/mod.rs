//! Personal style learning / auto edit (Phase 8b): learns the user's develop settings from
//! their edited frames and predicts a full `ParametricAdjustments` for a new frame.
//!
//! Owned by vision-ml-dev. Pure computation: no catalog, no IPC. The command layer (IPC v14
//! `ml/style.rs`) gathers [`StyleSample`]s from the catalog, calls [`train`], stores
//! [`StyleModel::to_json`], and for prediction builds a [`FrameContext`] and calls
//! [`StyleModel::predict`].
//!
//! Model (decision 2026-09-30 in `docs/decisions.md`):
//! - **Template**: everything that is a "look" rather than a per-photo correction (profile,
//!   HSL, parametric + point curves, colour grading, calibration, detail settings, effects,
//!   B&W) is the user's most frequent value set for that camera (fallback: over all cameras).
//!   A photographer applies a preset and then corrects each frame; this reproduces the preset
//!   exactly instead of averaging it.
//! - **Per-image sliders** ([`targets::TARGETS`]: WB (relative to as shot or absolute),
//!   Exposure (slider or target brightness), Contrast,
//!   Highlights, Shadows, Whites, Blacks, Texture, Clarity, Dehaze, Vibrance, Saturation,
//!   Sharpening, Luminance NR): ridge regression on standardized features + gradient-boosted
//!   trees on its residuals, both in pure Rust ([`learn`]). Ridge strength and the number of
//!   boosting rounds are chosen per slider by grouped cross-validation (folds = scenes, so
//!   a validation frame never has a same-scene sibling in training); the same CV picks each
//!   slider's encoding. Older edits can be down-weighted (recency half-life chosen by
//!   forward-chaining validation: styles drift).
//! - Features ([`features`]): neutral-render statistics, camera/EXIF, scene context.
//! - Crop and masks are never predicted (framing and subject are per photo).

pub mod eval;
pub mod features;
pub mod learn;
pub mod targets;

use std::collections::BTreeMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};

pub use features::{FrameContext, RenderFeatures, SceneContext, FEATURES_VERSION};
use learn::{Gbdt, GbdtParams, Ridge, Standardizer};
use targets::{CameraTemplate, Target, TargetFrame, TARGETS};

use crate::ipc::error::AppResult;
use crate::ipc::types::{
    ImageFormat, ImageStats, LumaPercentiles, MatchOptions, NeutralEstimate, OklabColor, ParametricAdjustments,
};

/// Bump when the stored model format changes (older files are rejected -> retrain).
pub const STYLE_MODEL_VERSION: u32 = 1;

/// Fewest edited frames [`train`] accepts (UI: "Edit at least 20 photos").
pub const MIN_SAMPLES: usize = 20;

/// Ridge strengths tried per slider (multiplied by the summed sample weight).
const RIDGE_LAMBDAS: [f64; 9] = [0.003, 0.01, 0.03, 0.1, 0.3, 1.0, 3.0, 10.0, 30.0];

/// One edited frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StyleSample {
    pub context: FrameContext,
    /// The user's settings (catalog edit or Lightroom XMP).
    pub adjustments: ParametricAdjustments,
    /// Cross-validation group, normally the scene id (frames of one scene are held out
    /// together); `None` = contiguous capture-time blocks.
    pub group: Option<i64>,
    pub captured_at_ms: Option<i64>,
    /// The user's settings rendered (no crop, no masks) at `scene::STATS_MAX_EDGE` and
    /// measured with [`RenderFeatures::from_render`]; enables stage B ([`OUTPUTS`]).
    #[serde(default)]
    pub edited: Option<RenderFeatures>,
}

/// Whether `adj` counts as a style sample: something beyond crop / masks / rating differs
/// from the format's neutral settings.
pub fn is_style_sample(adj: &ParametricAdjustments, format: ImageFormat) -> bool {
    targets::strip_per_image(adj) != ParametricAdjustments::defaults_for(format)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrainOptions {
    /// Cross-validation folds for hyper-parameters.
    pub folds: usize,
    pub gbdt: GbdtParams,
    /// Only fit the template + ridge (no trees); faster, a little less accurate.
    pub linear_only: bool,
    pub recency: RecencyWeighting,
}

impl Default for TrainOptions {
    fn default() -> Self {
        Self { folds: 5, gbdt: GbdtParams::default(), linear_only: false, recency: RecencyWeighting::Off }
    }
}

/// Weighting of older edits. Default `Off`: on the proposal shoot `Auto` picked a 28-sample
/// half-life that discarded one camera's history (bodies interleave in time) and lost
/// 1.6 dE on the per-camera split (docs/decisions.md 2026-09-30).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecencyWeighting {
    /// Chosen by forward-chaining validation (none, n/2, n/4, n/8 samples).
    Auto,
    Off,
    /// Weight halves every this many samples back in capture time.
    HalfLife(f32),
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum StyleError {
    #[error("need at least {min} edited photos to learn a style (have {have})")]
    Insufficient { have: usize, min: usize },
    #[error("style model file is invalid: {0}")]
    InvalidModel(String),
    #[error("style model was trained by an older version (retrain)")]
    Outdated,
}

/// Per-slider model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetModel {
    pub name: String,
    pub ridge: Ridge,
    pub gbdt: Gbdt,
    pub lambda: f64,
    /// Training range (predictions are clamped to it).
    pub min: f32,
    pub max: f32,
    /// Cross-validated mean absolute error (model units) of this model / of predicting the
    /// training mean.
    pub cv_mae: f32,
    pub cv_mae_mean: f32,
    /// Weight of the joint nearest-neighbour estimate ([`Knn`]); 0 = ridge + trees only.
    #[serde(default)]
    pub knn_weight: f32,
    /// Cross-validated ridge + trees predictions of the training samples (training only).
    #[serde(skip)]
    pub cv_pred: Vec<f32>,
}

/// Joint nearest neighbours: the `k` training frames closest in standardized feature space
/// vote with their *whole* slider set (recency-weighted mean), which keeps combinations the
/// user actually uses (Contrast / Highlights / Whites / Blacks compensate each other).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Knn {
    pub k: usize,
    /// Standardized training features.
    pub x: Vec<Vec<f32>>,
    pub w: Vec<f32>,
    /// Per kept target (same order as `StyleModel::targets`), per training sample.
    pub y: Vec<Vec<f32>>,
}

impl Knn {
    /// Weighted mean of the `k` nearest of `candidates` for each of `ys`.
    fn vote(x: &[Vec<f32>], w: &[f32], ys: &[Vec<f32>], candidates: &[usize], q: &[f32], k: usize) -> Vec<f32> {
        let mut d: Vec<(f32, usize)> =
            candidates.iter().map(|&i| (x[i].iter().zip(q).map(|(a, b)| (a - b) * (a - b)).sum::<f32>(), i)).collect();
        let k = k.min(d.len());
        if k == 0 {
            return vec![0.0; ys.len()];
        }
        d.select_nth_unstable_by(k - 1, |a, b| a.0.total_cmp(&b.0));
        let near = &d[..k];
        let sw: f32 = near.iter().map(|&(_, i)| w[i]).sum::<f32>().max(1e-9);
        ys.iter().map(|y| near.iter().map(|&(_, i)| y[i] * w[i]).sum::<f32>() / sw).collect()
    }

    pub fn predict(&self, q: &[f32]) -> Vec<f32> {
        let all: Vec<usize> = (0..self.x.len()).collect();
        Self::vote(&self.x, &self.w, &self.y, &all, q, self.k)
    }
}

/// A trained, serialisable style model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StyleModel {
    pub version: u32,
    pub features_version: u32,
    pub trained_at_ms: i64,
    pub sample_count: u32,
    /// Recency half-life used (samples; `None` = equal weights).
    pub half_life: Option<f32>,
    pub feature_names: Vec<String>,
    pub standardizer: Standardizer,
    pub targets: Vec<TargetModel>,
    #[serde(default)]
    pub knn: Option<Knn>,
    /// Stage B ([`OUTPUTS`]); empty if trained without measured edited renders.
    #[serde(default)]
    pub outputs: Vec<TargetModel>,
    pub templates: Vec<CameraTemplate>,
    pub global_template: CameraTemplate,
    /// Median distance (standardized features) of a training frame to its nearest training
    /// frame of another scene fold; the scale of [`StyleModel::confidence`]. 0 = unknown.
    #[serde(default)]
    pub typical_nn_distance: f32,
}

/// Summary of a training run (for `getStyleStatus` / logs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrainReport {
    pub samples: u32,
    pub cameras: u32,
    pub duration_ms: f64,
    pub half_life: Option<f32>,
    /// Target encoding -> (cv MAE, cv MAE of the mean predictor), model units, recency-weighted.
    pub cv: BTreeMap<String, (f32, f32)>,
    /// Encodings kept (one per slider).
    pub chosen: Vec<String>,
}

/// Slider groups [`StyleModel::refine`] solves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefineGroups {
    pub exposure: bool,
    pub white_balance: bool,
    /// Contrast / whites / blacks.
    pub tone: bool,
}

/// A prediction.
#[derive(Debug, Clone, PartialEq)]
pub struct StylePrediction {
    pub adjustments: ParametricAdjustments,
    /// The frame's camera had training edits (its own template was used).
    pub known_camera: bool,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// Fold of each sample: groups (scenes) assigned round-robin in capture order, or
/// contiguous time blocks when there are too few groups.
fn folds_of(samples: &[StyleSample], k: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..samples.len()).collect();
    order.sort_by_key(|&i| (samples[i].captured_at_ms.unwrap_or(i64::MAX), i));
    let mut groups: Vec<i64> = Vec::new();
    for &i in &order {
        if let Some(g) = samples[i].group {
            if !groups.contains(&g) {
                groups.push(g);
            }
        }
    }
    let mut fold = vec![0usize; samples.len()];
    if groups.len() >= k && samples.iter().all(|s| s.group.is_some()) {
        for (i, s) in samples.iter().enumerate() {
            let g = groups.iter().position(|g| Some(*g) == s.group).unwrap_or(0);
            fold[i] = g % k;
        }
    } else {
        for (rank, &i) in order.iter().enumerate() {
            fold[i] = rank * k / samples.len().max(1);
        }
    }
    fold
}

fn wmae(a: &[f32], b: &[f32], w: &[f32]) -> f32 {
    let sw: f32 = w.iter().sum();
    a.iter().zip(b).zip(w).map(|((x, y), w)| (x - y).abs() * w).sum::<f32>() / sw.max(1e-9)
}

/// Recency weights: `0.5^(rank / half_life)`, rank 0 = the newest capture (`None` = equal).
fn recency_weights(samples: &[StyleSample], half_life: Option<f32>) -> Vec<f32> {
    let Some(h) = half_life.filter(|h| *h > 0.0) else { return vec![1.0; samples.len()] };
    let mut order: Vec<usize> = (0..samples.len()).collect();
    order.sort_by_key(|&i| (std::cmp::Reverse(samples[i].captured_at_ms.unwrap_or(i64::MIN)), i));
    let mut w = vec![1.0; samples.len()];
    for (rank, &i) in order.iter().enumerate() {
        w[i] = 0.5f32.powf(rank as f32 / h);
    }
    w
}

fn frame(s: &StyleSample) -> TargetFrame {
    TargetFrame::of(&s.context)
}

/// Forward-chaining choice of the recency half-life: fit ridge on the oldest 75 % of the
/// samples with each candidate weighting, score the newest 25 % (mean over sliders of MAE
/// relative to the weighted-mean predictor). Styles drift (a look is refined over a shoot
/// and between shoots), which scene-grouped cross-validation cannot see.
fn choose_half_life(samples: &[StyleSample], x: &[Vec<f32>], candidates: &[Option<f32>]) -> Option<f32> {
    let mut order: Vec<usize> = (0..samples.len()).collect();
    order.sort_by_key(|&i| (samples[i].captured_at_ms.unwrap_or(i64::MAX), i));
    let cut = samples.len() * 3 / 4;
    let (old, new) = order.split_at(cut);
    if new.len() < 5 || old.len() < MIN_SAMPLES / 2 {
        return None;
    }
    let old_s: Vec<StyleSample> = old.iter().map(|&i| samples[i].clone()).collect();
    let xo: Vec<Vec<f32>> = old.iter().map(|&i| x[i].clone()).collect();
    let mut best = (f32::MAX, None);
    for &h in candidates {
        let w = recency_weights(&old_s, h.map(|h| h * old.len() as f32 / samples.len() as f32));
        let (mut score, mut slots) = (0.0, 0);
        for t in TARGETS.iter().filter(|t| t.slot == t.name) {
            let yo: Vec<f32> = old.iter().map(|&i| t.get(&samples[i].adjustments, &frame(&samples[i]))).collect();
            let yn: Vec<f32> = new.iter().map(|&i| t.get(&samples[i].adjustments, &frame(&samples[i]))).collect();
            let r = Ridge::fit_weighted(&xo, &yo, &w, 0.3);
            let m = yo.iter().zip(&w).map(|(y, w)| y * w).sum::<f32>() / w.iter().sum::<f32>();
            let base = yn.iter().map(|y| (y - m).abs()).sum::<f32>() / yn.len() as f32;
            let err = new.iter().zip(&yn).map(|(&i, y)| (r.predict(&x[i]) - y).abs()).sum::<f32>() / yn.len() as f32;
            if base > 1e-6 {
                score += err / base;
                slots += 1;
            }
        }
        let score = score / slots.max(1) as f32;
        if score < best.0 - 1e-4 {
            best = (score, h);
        }
    }
    best.1
}

/// Cross-validated ridge + boosted-trees fit of one value per sample (see [`train`]).
fn fit_values(
    name: &str,
    y: &[f32],
    x: &[Vec<f32>],
    w: &[f32],
    fold: &[usize],
    k: usize,
    options: &TrainOptions,
) -> TargetModel {
    let y = y.to_vec();
    let (lo, hi) = y.iter().fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
    let rounds = if options.linear_only { 0 } else { options.gbdt.max_rounds };
    let split = |f: usize| -> (Vec<usize>, Vec<usize>) {
        ((0..y.len()).filter(|&i| fold[i] != f).collect(), (0..y.len()).filter(|&i| fold[i] == f).collect())
    };
    let pick = |idx: &[usize], v: &[f32]| -> Vec<f32> { idx.iter().map(|&i| v[i]).collect() };
    let rows = |idx: &[usize]| -> Vec<Vec<f32>> { idx.iter().map(|&i| x[i].clone()).collect() };
    // Ridge strength: cross-validated weighted MAE.
    let mut ridge_pred = vec![vec![0.0f32; y.len()]; RIDGE_LAMBDAS.len()];
    let mut mean_pred = vec![0.0f32; y.len()];
    for f in 0..k {
        let (tr, va) = split(f);
        if tr.is_empty() || va.is_empty() {
            continue;
        }
        let (xt, yt, wt) = (rows(&tr), pick(&tr, &y), pick(&tr, w));
        let m = yt.iter().zip(&wt).map(|(y, w)| y * w).sum::<f32>() / wt.iter().sum::<f32>().max(1e-9);
        for (li, &lambda) in RIDGE_LAMBDAS.iter().enumerate() {
            let r = Ridge::fit_weighted(&xt, &yt, &wt, lambda);
            for &i in &va {
                ridge_pred[li][i] = r.predict(&x[i]).clamp(lo, hi);
            }
        }
        for &i in &va {
            mean_pred[i] = m;
        }
    }
    let (best_l, mut cv_mae) = ridge_pred
        .iter()
        .enumerate()
        .map(|(li, p)| (li, wmae(p, &y, w)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap_or((0, 0.0));
    let lambda = RIDGE_LAMBDAS[best_l];
    let cv_mae_mean = wmae(&mean_pred, &y, w);
    // Boosting on the ridge residuals: validation error per round, summed over folds.
    let mut best_rounds = 0usize;
    if rounds > 0 {
        let mut curve = vec![0.0f64; rounds + 1];
        for f in 0..k {
            let (tr, va) = split(f);
            if tr.is_empty() || va.is_empty() {
                continue;
            }
            let (xt, yt, wt) = (rows(&tr), pick(&tr, &y), pick(&tr, w));
            let r = Ridge::fit_weighted(&xt, &yt, &wt, lambda);
            let res: Vec<f32> = xt.iter().zip(&yt).map(|(row, v)| v - r.predict(row)).collect();
            let xv = rows(&va);
            let base: Vec<f32> = xv.iter().map(|row| r.predict(row)).collect();
            let err = |add: &[f32]| -> f64 {
                va.iter()
                    .zip(base.iter().zip(add))
                    .map(|(&i, (b, a))| f64::from(((b + a).clamp(lo, hi) - y[i]).abs() * w[i]))
                    .sum()
            };
            let mut last = err(&vec![0.0f32; va.len()]);
            curve[0] += last;
            let mut last_round = 0;
            Gbdt::fit(&xt, &res, &wt, rounds, &options.gbdt, &xv, |round, pred| {
                last = err(pred);
                last_round = round;
                curve[round] += last;
            });
            // Early-stopped folds keep their last value for the remaining rounds.
            for c in curve.iter_mut().skip(last_round + 1) {
                *c += last;
            }
        }
        let sw: f64 = w.iter().map(|v| f64::from(*v)).sum();
        let (br, be) = curve
            .iter()
            .enumerate()
            .map(|(r, e)| (r, e / sw))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap_or((0, f64::from(cv_mae)));
        // Require a real improvement over ridge alone (1 %) before adding trees.
        if be < f64::from(cv_mae) * 0.99 {
            best_rounds = br;
            cv_mae = be as f32;
        }
    }
    // Cross-validated predictions of the chosen configuration (for the kNN blend).
    let mut cv_pred = ridge_pred[best_l].clone();
    if best_rounds > 0 {
        for f in 0..k {
            let (tr, va) = split(f);
            if tr.is_empty() || va.is_empty() {
                continue;
            }
            let (xt, yt, wt) = (rows(&tr), pick(&tr, &y), pick(&tr, w));
            let r = Ridge::fit_weighted(&xt, &yt, &wt, lambda);
            let res: Vec<f32> = xt.iter().zip(&yt).map(|(row, v)| v - r.predict(row)).collect();
            let g = Gbdt::fit(&xt, &res, &wt, best_rounds, &options.gbdt, &[], |_, _| {});
            for &i in &va {
                cv_pred[i] = (r.predict(&x[i]) + g.predict(&x[i])).clamp(lo, hi);
            }
        }
    }
    let ridge = Ridge::fit_weighted(x, &y, w, lambda);
    let gbdt = if best_rounds > 0 {
        let res: Vec<f32> = x.iter().zip(&y).map(|(row, v)| v - ridge.predict(row)).collect();
        Gbdt::fit(x, &res, w, best_rounds, &options.gbdt, &[], |_, _| {})
    } else {
        Gbdt::default()
    };
    TargetModel {
        name: name.to_owned(),
        ridge,
        gbdt,
        lambda,
        min: lo,
        max: hi,
        cv_mae,
        cv_mae_mean,
        knn_weight: 0.0,
        cv_pred,
    }
}

fn fit_target(
    t: &Target,
    samples: &[StyleSample],
    x: &[Vec<f32>],
    w: &[f32],
    fold: &[usize],
    k: usize,
    options: &TrainOptions,
) -> TargetModel {
    let y: Vec<f32> = samples.iter().map(|s| t.get(&s.adjustments, &frame(s))).collect();
    fit_values(t.name, &y, x, w, fold, k, options)
}

/// Rendered-output statistics the model also learns (stage B): the look of the user's
/// render, which [`StyleModel::refine`] then reaches by solving exposure / white balance
/// (and optionally contrast / whites / blacks) on the frame itself.
/// Reads one output statistic.
pub type OutputGetter = fn(&RenderFeatures) -> f32;

pub const OUTPUTS: &[(&str, OutputGetter)] = &[
    ("outLogMeanLuma", |r| r.log_mean_luma),
    ("outNeutralA", |r| r.neutral_ab[0]),
    ("outNeutralB", |r| r.neutral_ab[1]),
    ("outLogP1", |r| r.log_percentiles[0]),
    ("outLogP10", |r| r.log_percentiles[1]),
    ("outLogP90", |r| r.log_percentiles[3]),
    ("outLogP99", |r| r.log_percentiles[4]),
];

/// Learns a style from `samples` (the caller filters with [`is_style_sample`]).
/// `progress(0..=1)` is called between target encodings.
pub fn train(
    samples: &[StyleSample],
    options: &TrainOptions,
    progress: &dyn Fn(f32),
) -> Result<(StyleModel, TrainReport), StyleError> {
    if samples.len() < MIN_SAMPLES {
        return Err(StyleError::Insufficient { have: samples.len(), min: MIN_SAMPLES });
    }
    let t0 = Instant::now();
    let raw: Vec<Vec<f32>> = samples.iter().map(|s| features::feature_vector(&s.context)).collect();
    let standardizer = Standardizer::fit(&raw);
    let x: Vec<Vec<f32>> = raw.iter().map(|r| standardizer.apply(r)).collect();
    let k = options.folds.clamp(2, 10);
    let fold = folds_of(samples, k);

    let (templates, global) = targets::mode_templates(samples.iter().map(|s| (s.context.camera_key(), &s.adjustments)));
    let global_template = global.ok_or(StyleError::Insufficient { have: 0, min: MIN_SAMPLES })?;

    let n = samples.len() as f32;
    let half_life = match options.recency {
        RecencyWeighting::Auto => choose_half_life(samples, &x, &[None, Some(n / 2.0), Some(n / 4.0), Some(n / 8.0)]),
        RecencyWeighting::Off => None,
        RecencyWeighting::HalfLife(h) => Some(h),
    };
    let w = recency_weights(samples, half_life);

    let mut chosen: Vec<TargetModel> = Vec::new();
    let mut cv = BTreeMap::new();
    for (ti, t) in TARGETS.iter().enumerate() {
        let tm = fit_target(t, samples, &x, &w, &fold, k, options);
        cv.insert(t.name.to_owned(), (tm.cv_mae, tm.cv_mae_mean));
        let slot_of = |m: &TargetModel| targets::target(&m.name).map_or("", |t| t.slot);
        match chosen.iter_mut().find(|m| slot_of(m) == t.slot) {
            Some(m) if tm.cv_mae < m.cv_mae => *m = tm,
            Some(_) => {}
            None => chosen.push(tm),
        }
        progress((ti + 1) as f32 / TARGETS.len() as f32);
    }
    // Joint kNN blend: k and per-target weights by the same cross-validation.
    let ys: Vec<Vec<f32>> = chosen
        .iter()
        .map(|m| {
            let t = targets::target(&m.name).expect("known target");
            samples.iter().map(|s| t.get(&s.adjustments, &frame(s))).collect()
        })
        .collect();
    let mut knn = None;
    if !options.linear_only {
        let mut best: Option<(f32, usize, Vec<f32>, Vec<f32>)> = None;
        for kk in [3usize, 7, 15] {
            let mut cvk = vec![vec![0.0f32; samples.len()]; chosen.len()];
            for f in 0..k {
                let cand: Vec<usize> = (0..samples.len()).filter(|&i| fold[i] != f).collect();
                for i in (0..samples.len()).filter(|&i| fold[i] == f) {
                    for (t, v) in Knn::vote(&x, &w, &ys, &cand, &x[i], kk).into_iter().enumerate() {
                        cvk[t][i] = v;
                    }
                }
            }
            let (mut score, mut alphas, mut maes) = (0.0, Vec::new(), Vec::new());
            for (t, m) in chosen.iter().enumerate() {
                let (a, e) = [0.0f32, 0.25, 0.5, 0.75, 1.0]
                    .iter()
                    .map(|&a| {
                        let p: Vec<f32> =
                            cvk[t].iter().zip(&m.cv_pred).map(|(kn, rg)| a * kn + (1.0 - a) * rg).collect();
                        (a, wmae(&p, &ys[t], &w))
                    })
                    .min_by(|p, q| p.1.total_cmp(&q.1))
                    .unwrap_or((0.0, m.cv_mae));
                score += e / m.cv_mae_mean.max(1e-6);
                alphas.push(a);
                maes.push(e);
            }
            if best.as_ref().is_none_or(|b| score < b.0) {
                best = Some((score, kk, alphas, maes));
            }
        }
        if let Some((_, kk, alphas, maes)) = best {
            if alphas.iter().any(|a| *a > 0.0) {
                for ((m, a), e) in chosen.iter_mut().zip(&alphas).zip(&maes) {
                    m.knn_weight = *a;
                    m.cv_mae = *e;
                    cv.insert(format!("{}+knn", m.name), (*e, m.cv_mae_mean));
                }
            }
            // Kept even when every blend weight is 0: [`StyleModel::confidence`] measures the
            // distance to the training frames with it.
            knn = Some(Knn { k: kk, x: x.clone(), w: w.clone(), y: ys });
        }
    }

    // Stage B on the samples whose edited render was measured.
    let mut outputs = Vec::new();
    let with_out: Vec<usize> = (0..samples.len()).filter(|&i| samples[i].edited.is_some()).collect();
    if with_out.len() >= MIN_SAMPLES {
        let xs: Vec<Vec<f32>> = with_out.iter().map(|&i| x[i].clone()).collect();
        let ws: Vec<f32> = with_out.iter().map(|&i| w[i]).collect();
        let sub: Vec<StyleSample> = with_out.iter().map(|&i| samples[i].clone()).collect();
        let fs = folds_of(&sub, k);
        for (name, get) in OUTPUTS {
            let y: Vec<f32> = sub.iter().filter_map(|s| s.edited.as_ref().map(get)).collect();
            let tm = fit_values(name, &y, &xs, &ws, &fs, k, options);
            cv.insert((*name).to_owned(), (tm.cv_mae, tm.cv_mae_mean));
            outputs.push(tm);
        }
    }
    let model = StyleModel {
        version: STYLE_MODEL_VERSION,
        features_version: FEATURES_VERSION,
        trained_at_ms: now_ms(),
        sample_count: samples.len() as u32,
        half_life,
        feature_names: features::feature_names(),
        standardizer,
        targets: chosen,
        knn,
        outputs,
        templates,
        global_template,
        typical_nn_distance: typical_nn_distance(&x, &fold),
    };
    let report = TrainReport {
        samples: samples.len() as u32,
        cameras: model.templates.len() as u32,
        duration_ms: t0.elapsed().as_secs_f64() * 1000.0,
        half_life,
        cv,
        chosen: model.targets.iter().map(|t| t.name.clone()).collect(),
    };
    Ok((model, report))
}

/// Median over `x` of the Euclidean distance to the nearest row of *another fold* (folds =
/// scenes, so this is how far a frame of a new scene typically is from the training frames;
/// at most 2000 rows sampled evenly).
fn typical_nn_distance(x: &[Vec<f32>], fold: &[usize]) -> f32 {
    let step = x.len().div_ceil(2000).max(1);
    let mut d: Vec<f32> = (0..x.len())
        .step_by(step)
        .filter_map(|i| {
            x.iter()
                .enumerate()
                .filter(|(j, _)| fold[*j] != fold[i])
                .map(|(_, r)| dist2(&x[i], r))
                .min_by(f32::total_cmp)
                .map(f32::sqrt)
        })
        .filter(|v| v.is_finite())
        .collect();
    if d.is_empty() {
        return 0.0;
    }
    d.sort_by(f32::total_cmp);
    d[d.len() / 2]
}

fn dist2(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(p, q)| (p - q) * (p - q)).filter(|v| v.is_finite()).sum()
}

impl StyleModel {
    /// How well the training edits cover this frame, 0..=1: 1 while the nearest training frame
    /// is no farther than a new scene's typically is ([`Self::typical_nn_distance`]), decaying
    /// beyond (0.61 at twice that distance); x0.6 for a camera without training edits.
    pub fn confidence(&self, ctx: &FrameContext) -> f32 {
        let camera = if self.templates.iter().any(|t| t.camera_key == ctx.camera_key()) { 1.0 } else { 0.6 };
        let (Some(knn), true) = (self.knn.as_ref(), self.typical_nn_distance > 0.0) else {
            return 0.8 * camera;
        };
        let q = self.standardizer.apply(&features::feature_vector(ctx));
        let d = knn.x.iter().map(|r| dist2(r, &q)).min_by(f32::total_cmp).map_or(f32::INFINITY, f32::sqrt);
        let excess = (d / self.typical_nn_distance - 1.0).max(0.0);
        let c = (-0.5 * excess).exp() * camera;
        if c.is_finite() {
            c.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// Predicted settings for a frame: the camera's template + regressed sliders. Crop and
    /// masks are left empty (the caller keeps the frame's own crop if it wants).
    pub fn predict(&self, ctx: &FrameContext) -> StylePrediction {
        let key = ctx.camera_key();
        let own = self.templates.iter().find(|t| t.camera_key == key);
        let mut adj = own.unwrap_or(&self.global_template).adjustments.clone();
        let x = self.standardizer.apply(&features::feature_vector(ctx));
        let f = TargetFrame::of(ctx);
        let near = self.knn.as_ref().map(|k| k.predict(&x));
        for (ti, tm) in self.targets.iter().enumerate() {
            let Some(t) = targets::target(&tm.name) else { continue };
            let mut v = (tm.ridge.predict(&x) + tm.gbdt.predict(&x)).clamp(tm.min, tm.max);
            if let Some(n) = near.as_ref().and_then(|n| n.get(ti)) {
                v = tm.knn_weight * n + (1.0 - tm.knn_weight) * v;
            }
            if v.is_finite() {
                t.set(&mut adj, v, &f);
            }
        }
        StylePrediction { adjustments: adj, known_camera: own.is_some() }
    }

    /// Predicted statistics of the user's render of this frame (stage B), as the
    /// `ImageStats` reference the Phase 7 solver matches. `None` without stage B.
    pub fn predict_output(&self, ctx: &FrameContext) -> Option<ImageStats> {
        if self.outputs.is_empty() {
            return None;
        }
        let x = self.standardizer.apply(&features::feature_vector(ctx));
        let get = |name: &str| {
            self.outputs
                .iter()
                .find(|m| m.name == name)
                .map(|m| (m.ridge.predict(&x) + m.gbdt.predict(&x)).clamp(m.min, m.max))
        };
        let l = get("outLogMeanLuma")?;
        let (a, b) = (get("outNeutralA")?, get("outNeutralB")?);
        let (p1, p10, p90, p99) = (get("outLogP1")?, get("outLogP10")?, get("outLogP90")?, get("outLogP99")?);
        let lin = |v: f32| v.min(0.0).exp2();
        Some(ImageStats {
            image_id: 0,
            region: None,
            width: 0,
            height: 0,
            mean_luma: lin(l),
            log_mean_luma: l,
            percentiles: LumaPercentiles { p1: lin(p1), p10: lin(p10), p50: lin(l), p90: lin(p90), p99: lin(p99) },
            clipped_highlights: 0.0,
            clipped_shadows: 0.0,
            mean_oklab: OklabColor { l: 0.0, a, b },
            neutral: NeutralEstimate { x: 0.3127, y: 0.329, a, b, coverage: 1.0 },
            white_balance: None,
            as_shot: ctx.as_shot,
            lut_missing: false,
        })
    }

    /// Stage B: starting from `start` (normally [`Self::predict`]), solve the groups in
    /// `groups` so the frame's render reaches the predicted output statistics. `measure` renders the frame with the given settings at
    /// `scene::STATS_MAX_EDGE` (no crop) and measures it (`scene::stats::render_stats`).
    /// Typically 2-5 renders. Returns `start` unchanged without stage B.
    pub fn refine(
        &self,
        ctx: &FrameContext,
        start: &ParametricAdjustments,
        groups: RefineGroups,
        measure: &mut dyn FnMut(&ParametricAdjustments) -> AppResult<ImageStats>,
    ) -> AppResult<ParametricAdjustments> {
        let Some(reference) = self.predict_output(ctx) else { return Ok(start.clone()) };
        if !(groups.exposure || groups.white_balance || groups.tone) {
            return Ok(start.clone());
        }
        let options = MatchOptions {
            match_exposure: groups.exposure,
            match_white_balance: groups.white_balance,
            match_tone: groups.tone,
            ..Default::default()
        };
        Ok(crate::scene::matching::solve(&reference, start, &options, measure)?.full)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn from_json(s: &str) -> Result<StyleModel, StyleError> {
        let v: serde_json::Value = serde_json::from_str(s).map_err(|e| StyleError::InvalidModel(e.to_string()))?;
        let version = v.get("version").and_then(serde_json::Value::as_u64).unwrap_or(0);
        let fv = v.get("featuresVersion").and_then(serde_json::Value::as_u64).unwrap_or(0);
        if version != u64::from(STYLE_MODEL_VERSION) || fv != u64::from(FEATURES_VERSION) {
            return Err(StyleError::Outdated);
        }
        serde_json::from_value(v).map_err(|e| StyleError::InvalidModel(e.to_string()))
    }
}

/// Every numeric setting of `adj` by report group (`whiteBalance` in K / tint, `tone`,
/// `presence`, `hsl`, `toneCurve` (parametric + point curves sampled at 9 inputs),
/// `colorGrading`, `calibration`, `detail`, `effects`, `blackAndWhite`), for slider-level
/// error reports. Crop, masks, LUT and profile are excluded.
pub fn slider_values(
    adj: &ParametricAdjustments,
    as_shot: Option<crate::ipc::types::WhiteBalanceValues>,
) -> Vec<(String, String, f32)> {
    use crate::ipc::types::WhiteBalance;
    let mut out = Vec::new();
    let wbv = match adj.white_balance {
        WhiteBalance::Custom { temperature_k, tint } => Some((temperature_k, tint)),
        WhiteBalance::AsShot => as_shot.map(|w| (w.temperature_k, w.tint)),
    };
    if let Some((k, t)) = wbv {
        out.push(("whiteBalance".into(), "temperatureK".into(), k));
        out.push(("whiteBalance".into(), "tint".into(), t));
    }
    let group_of = |name: &str| match name {
        "exposure" | "contrast" | "highlights" | "shadows" | "whites" | "blacks" => "tone",
        _ => "presence",
    };
    let json = serde_json::to_value(adj).unwrap_or_default();
    for (key, v) in json.as_object().into_iter().flatten() {
        match key.as_str() {
            "exposure" | "contrast" | "highlights" | "shadows" | "whites" | "blacks" | "texture" | "clarity"
            | "dehaze" | "vibrance" | "saturation" => {
                out.push((group_of(key).into(), key.clone(), v.as_f64().unwrap_or(0.0) as f32));
            }
            "hsl" | "colorGrading" | "calibration" | "detail" | "effects" | "blackAndWhite" => {
                flatten(key, key, v, &mut out);
            }
            _ => {}
        }
    }
    let p = &adj.tone_curve.parametric;
    for (n, v) in [
        ("parametric.shadows", p.shadows),
        ("parametric.darks", p.darks),
        ("parametric.lights", p.lights),
        ("parametric.highlights", p.highlights),
        ("parametric.shadowSplit", p.shadow_split),
        ("parametric.midtoneSplit", p.midtone_split),
        ("parametric.highlightSplit", p.highlight_split),
    ] {
        out.push(("toneCurve".into(), n.into(), v));
    }
    let pc = &adj.tone_curve.point;
    for (n, c) in [("master", &pc.master), ("red", &pc.red), ("green", &pc.green), ("blue", &pc.blue)] {
        for i in 0..=8 {
            let x = i as f32 * 255.0 / 8.0;
            out.push(("toneCurve".into(), format!("point.{n}@{x:.0}"), linear_curve(c, x)));
        }
    }
    out
}

fn flatten(group: &str, path: &str, v: &serde_json::Value, out: &mut Vec<(String, String, f32)>) {
    match v {
        serde_json::Value::Number(n) => out.push((group.into(), path.into(), n.as_f64().unwrap_or(0.0) as f32)),
        serde_json::Value::Bool(b) => out.push((group.into(), path.into(), if *b { 1.0 } else { 0.0 })),
        serde_json::Value::Object(m) => {
            for (k, v) in m {
                flatten(group, &format!("{path}.{k}"), v, out);
            }
        }
        _ => {}
    }
}

/// Piecewise-linear value of a point curve at `x` (report use only).
fn linear_curve(c: &[[f32; 2]], x: f32) -> f32 {
    match c {
        [] => x,
        [p] => p[1],
        _ => {
            if x <= c[0][0] {
                return c[0][1];
            }
            for w in c.windows(2) {
                if x <= w[1][0] {
                    let t = (x - w[0][0]) / (w[1][0] - w[0][0]).max(1e-6);
                    return w[0][1] + t * (w[1][1] - w[0][1]);
                }
            }
            c[c.len() - 1][1]
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
