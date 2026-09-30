//! Relative grading from 1-2 anchors. Owned by vision-ml-dev except the pure semantic
//! helpers [`base_adjustments`], [`choose_anchors`] and [`delta`] (architect; they define the
//! contract and are tested here).
//!
//! Semantics (per target `T`, anchor `A` with stored adjustments `adjA`):
//! 1. `base` = [`base_adjustments`]`(adjT, adjA, wbA, options)`: `T`'s settings with
//!    `copyFields` + the matched groups taken from `A`; white balance resolved to `custom`
//!    (the anchor's effective temperature/tint) when matched.
//! 2. `reference` = stats of `A` rendered with `adjA` (two anchors: blended, see
//!    [`choose_anchors`]).
//! 3. `full` = [`solve`]: `base` + corrections so `T` rendered with `full` matches `reference`:
//!    exposure from the `logMeanLuma` difference (EV), temperature/tint from the rendered
//!    neutral difference (Oklab a/b of `neutral`, mapped through `develop::wb` so the target's
//!    render-space neutral lands on the reference's), optional small contrast/whites/blacks
//!    from percentile differences. Iterate render -> measure -> correct (the pipeline is
//!    non-linear; 2-3 rounds) until within `TOLERANCE_EV` / `TOLERANCE_AB` or no progress.
//!    Clamp to slider ranges (note it).
//! 4. `adjustments` = `ParametricAdjustments::lerp(base, full, options.strength)`.

use std::sync::atomic::{AtomicU32, Ordering};

use rayon::prelude::*;

use super::stats::render_stats;
use super::{color, MatchImage, Progress};
use crate::develop::{wb, DevelopCache};
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::{
    AdjustmentField, ImageStats, LumaPercentiles, MatchDelta, MatchOptions, MatchPreview, NeutralEstimate, OklabColor,
    ParametricAdjustments, WhiteBalance, WhiteBalanceValues,
};
use crate::lut::LutLibrary;

/// Output of [`solve`].
#[derive(Debug, Clone, PartialEq)]
pub struct Solved {
    /// `base` + the full correction (valid slider ranges).
    pub full: ParametricAdjustments,
    /// The target measured with `full`.
    pub predicted: ImageStats,
    /// `predicted` is within `TOLERANCE_EV` / `TOLERANCE_AB` of the reference (for the matched
    /// groups).
    pub converged: bool,
    pub notes: Vec<String>,
}

/// Which anchor(s) a target is matched to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnchorChoice {
    /// Index into the anchors slice.
    pub primary: usize,
    /// Second anchor index and its weight 0..=1 when the target lies strictly between two
    /// anchors in capture time (blend = `lerp(primary, second, weight)`).
    pub second: Option<(usize, f32)>,
}

/// Semantics (architect). `target` with the anchor's groups copied: `options.copyFields` plus
/// [`MatchOptions::matched_fields`]. When white balance is matched it becomes
/// `custom { anchor's effective values }` (`anchor_wb` = the anchor's as-shot values, used when
/// the anchor is `as_shot`; if unknown the anchor's `as_shot` mode is copied as-is).
pub fn base_adjustments(
    target: &ParametricAdjustments,
    anchor: &ParametricAdjustments,
    anchor_as_shot: Option<WhiteBalanceValues>,
    options: &MatchOptions,
) -> ParametricAdjustments {
    let mut out = target.clone();
    out.copy_fields(anchor, &options.copy_fields);
    out.copy_fields(anchor, &options.matched_fields());
    if options.match_white_balance {
        if let (WhiteBalance::AsShot, Some(v)) = (anchor.white_balance, anchor_as_shot) {
            out.white_balance = WhiteBalance::Custom { temperature_k: v.temperature_k, tint: v.tint };
        }
    }
    out
}

/// Semantics (architect). One anchor -> it. Two anchors with capture times and the target
/// strictly between them -> both, weight = time fraction towards the later one. Otherwise the
/// nearest anchor in capture time (ties and missing times -> the first anchor).
/// `anchor_times` in the order anchors were given.
pub fn choose_anchors(anchor_times: &[Option<i64>], target_time: Option<i64>) -> AnchorChoice {
    let single = AnchorChoice { primary: 0, second: None };
    let (Some(t), [Some(a), Some(b)]) = (target_time, anchor_times) else {
        return single;
    };
    let (early, late, ta, tb) = if a <= b { (0, 1, *a, *b) } else { (1, 0, *b, *a) };
    if ta < t && t < tb {
        let w = (t - ta) as f64 / (tb - ta) as f64;
        return AnchorChoice { primary: early, second: Some((late, w as f32)) };
    }
    let (da, db) = ((t - a).abs(), (t - b).abs());
    AnchorChoice { primary: if db < da { 1 } else { 0 }, second: None }
}

/// Semantics (architect). `full - base` of the corrected sliders (temperature in Kelvin; 0
/// unless both are `custom`).
pub fn delta(base: &ParametricAdjustments, full: &ParametricAdjustments) -> MatchDelta {
    let (temperature_k, tint) = match (base.white_balance, full.white_balance) {
        (
            WhiteBalance::Custom { temperature_k: tb, tint: ib },
            WhiteBalance::Custom { temperature_k: tf, tint: i_f },
        ) => (tf - tb, i_f - ib),
        _ => (0.0, 0.0),
    };
    MatchDelta {
        exposure: full.exposure - base.exposure,
        temperature_k,
        tint,
        contrast: full.contrast - base.contrast,
        whites: full.whites - base.whites,
        blacks: full.blacks - base.blacks,
    }
}

/// Contract (vision-ml-dev). Blend of two anchors' stats at `weight` (towards `b`): e.g.
/// `logMeanLuma` / percentiles in log space, neutral / Oklab linearly. `image_id` = `a`'s.
pub fn blend_stats(a: &ImageStats, b: &ImageStats, weight: f32) -> ImageStats {
    let t = if weight.is_finite() { weight.clamp(0.0, 1.0) } else { 0.0 };
    let lin = |x: f32, y: f32| x + (y - x) * t;
    let log = |x: f32, y: f32| {
        let (lx, ly) = (x.max(color::MIN_LUMA).log2(), y.max(color::MIN_LUMA).log2());
        lin(lx, ly).exp2()
    };
    let wb = |x: Option<WhiteBalanceValues>, y: Option<WhiteBalanceValues>| match (x, y) {
        (Some(x), Some(y)) => Some(WhiteBalanceValues {
            temperature_k: 1.0e6 / lin(1.0e6 / x.temperature_k, 1.0e6 / y.temperature_k),
            tint: lin(x.tint, y.tint),
        }),
        (x, y) => {
            if t >= 0.5 {
                y
            } else {
                x
            }
        }
    };
    let (pa, pb) = (&a.percentiles, &b.percentiles);
    let (na, nb) = (&a.neutral, &b.neutral);
    ImageStats {
        image_id: a.image_id,
        region: a.region,
        width: a.width,
        height: a.height,
        mean_luma: log(a.mean_luma, b.mean_luma),
        log_mean_luma: lin(a.log_mean_luma, b.log_mean_luma),
        percentiles: LumaPercentiles {
            p1: log(pa.p1, pb.p1),
            p10: log(pa.p10, pb.p10),
            p50: log(pa.p50, pb.p50),
            p90: log(pa.p90, pb.p90),
            p99: log(pa.p99, pb.p99),
        },
        clipped_highlights: lin(a.clipped_highlights, b.clipped_highlights),
        clipped_shadows: lin(a.clipped_shadows, b.clipped_shadows),
        mean_oklab: OklabColor {
            l: lin(a.mean_oklab.l, b.mean_oklab.l),
            a: lin(a.mean_oklab.a, b.mean_oklab.a),
            b: lin(a.mean_oklab.b, b.mean_oklab.b),
        },
        neutral: NeutralEstimate {
            x: lin(na.x, nb.x),
            y: lin(na.y, nb.y),
            a: lin(na.a, nb.a),
            b: lin(na.b, nb.b),
            coverage: lin(na.coverage, nb.coverage),
        },
        white_balance: wb(a.white_balance, b.white_balance),
        as_shot: wb(a.as_shot, b.as_shot),
        lut_missing: a.lut_missing || b.lut_missing,
    }
}

/// Solver unknowns (slider space; temperature as mireds, ~linear in the rendered neutral).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Var {
    Exposure,
    Mired,
    Tint,
    Contrast,
    Whites,
    Blacks,
}

/// Solver residuals (target - reference).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Res {
    /// `logMeanLuma` (EV).
    Luma,
    /// `neutral.a` / `neutral.b`.
    NeutralA,
    NeutralB,
    /// Tonal spread log2(p90 / p10) (EV) -> contrast.
    Spread,
    /// log2 p99 (EV) -> whites.
    High,
    /// log2 p1 (EV) -> blacks.
    Low,
}

impl Var {
    /// Normalization (one unit of the solver's internal space).
    fn scale(self) -> f64 {
        match self {
            Var::Exposure => 1.0,
            Var::Mired => 10.0,
            Var::Tint => 10.0,
            Var::Contrast | Var::Whites | Var::Blacks => 20.0,
        }
    }

    /// Largest change per iteration (slider units).
    fn max_step(self) -> f64 {
        match self {
            Var::Exposure => 2.0,
            Var::Mired => 40.0,
            Var::Tint => 25.0,
            Var::Contrast | Var::Whites | Var::Blacks => 20.0,
        }
    }

    /// Valid range (slider units; mireds for temperature).
    fn range(self) -> (f64, f64) {
        match self {
            Var::Exposure => (-5.0, 5.0),
            Var::Mired => (1.0e6 / f64::from(wb::MAX_TEMP), 1.0e6 / f64::from(wb::MIN_TEMP)),
            Var::Tint => (f64::from(wb::MIN_TINT), f64::from(wb::MAX_TINT)),
            Var::Contrast | Var::Whites | Var::Blacks => (-100.0, 100.0),
        }
    }

    /// Largest total correction from `base` (slider units). Frames of one scene share the
    /// light, so a larger correction means the statistics are misled by content (a white
    /// close-up, coloured stage lights) and following them would ruin the frame; tone
    /// corrections stay small by design.
    fn max_correction(self) -> f64 {
        match self {
            Var::Exposure => 3.0,
            Var::Mired => 80.0,
            Var::Tint => 50.0,
            Var::Contrast | Var::Whites | Var::Blacks => 40.0,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Var::Exposure => "exposure",
            Var::Mired => "temperature",
            Var::Tint => "tint",
            Var::Contrast => "contrast",
            Var::Whites => "whites",
            Var::Blacks => "blacks",
        }
    }
}

/// Renders per solve (the base included).
const MAX_RENDERS: usize = 10;
/// The solver stops early inside this share of the acceptance tolerances (margin).
const INNER_TOLERANCE: f64 = 0.4;
/// Tone residual unit (EV); tone is matched roughly and is not part of `converged`.
const TONE_TOLERANCE_EV: f64 = 0.15;

impl Res {
    fn scale(self) -> f64 {
        match self {
            Res::Luma => f64::from(super::TOLERANCE_EV),
            Res::NeutralA | Res::NeutralB => f64::from(super::TOLERANCE_AB),
            Res::Spread | Res::High | Res::Low => TONE_TOLERANCE_EV,
        }
    }

    fn primary(self) -> bool {
        matches!(self, Res::Luma | Res::NeutralA | Res::NeutralB)
    }

    fn value(self, s: &ImageStats) -> f64 {
        let l2 = |v: f32| f64::from(v.max(color::MIN_LUMA)).log2();
        match self {
            Res::Luma => f64::from(s.log_mean_luma),
            Res::NeutralA => f64::from(s.neutral.a),
            Res::NeutralB => f64::from(s.neutral.b),
            Res::Spread => l2(s.percentiles.p90) - l2(s.percentiles.p10),
            Res::High => l2(s.percentiles.p99),
            Res::Low => l2(s.percentiles.p1),
        }
    }
}

/// Initial slope d(residual)/d(variable) in physical units (typical values of the develop
/// pipeline on the sample renders; Broyden updates refine them per image).
fn initial_slope(r: Res, v: Var, reference: &ImageStats) -> f64 {
    match (r, v) {
        (Res::Luma, Var::Exposure) => 1.0,
        (Res::NeutralA, Var::Mired) => WB_SLOPE[0][0],
        (Res::NeutralA, Var::Tint) => WB_SLOPE[0][1],
        (Res::NeutralB, Var::Mired) => WB_SLOPE[1][0],
        (Res::NeutralB, Var::Tint) => WB_SLOPE[1][1],
        (Res::Spread, Var::Contrast) => Res::Spread.value(reference).max(1.0) * 0.0035,
        (Res::High, Var::Whites) => 0.004,
        (Res::Low, Var::Blacks) => 0.012,
        _ => 0.0,
    }
}

/// d(neutral a, b) / d(mired, tint) of the develop pipeline around daylight (rows a, b).
const WB_SLOPE: [[f64; 2]; 2] = [[-0.0001, 0.0006], [-0.0007, -0.0004]];

fn get(adj: &ParametricAdjustments, v: Var) -> f64 {
    let f = match v {
        Var::Exposure => adj.exposure,
        Var::Mired => match adj.white_balance {
            WhiteBalance::Custom { temperature_k, .. } => 1.0e6 / temperature_k,
            WhiteBalance::AsShot => 1.0e6 / 5500.0,
        },
        Var::Tint => match adj.white_balance {
            WhiteBalance::Custom { tint, .. } => tint,
            WhiteBalance::AsShot => 0.0,
        },
        Var::Contrast => adj.contrast,
        Var::Whites => adj.whites,
        Var::Blacks => adj.blacks,
    };
    f64::from(f)
}

fn set(adj: &mut ParametricAdjustments, v: Var, value: f64) {
    let x = value as f32;
    match v {
        Var::Exposure => adj.exposure = x,
        Var::Mired => {
            let tint = match adj.white_balance {
                WhiteBalance::Custom { tint, .. } => tint,
                WhiteBalance::AsShot => 0.0,
            };
            let k = (1.0e6 / x).clamp(wb::MIN_TEMP, wb::MAX_TEMP);
            adj.white_balance = WhiteBalance::Custom { temperature_k: k, tint };
        }
        Var::Tint => {
            let k = match adj.white_balance {
                WhiteBalance::Custom { temperature_k, .. } => temperature_k,
                WhiteBalance::AsShot => 5500.0,
            };
            adj.white_balance = WhiteBalance::Custom { temperature_k: k, tint: x.clamp(wb::MIN_TINT, wb::MAX_TINT) };
        }
        Var::Contrast => adj.contrast = x,
        Var::Whites => adj.whites = x,
        Var::Blacks => adj.blacks = x,
    }
}

/// Solves `a x = b` (small, dense; partial pivoting). `None` if singular.
fn solve_linear(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-9 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        let pivot_row = a[col].clone();
        for row in col + 1..n {
            let f = a[row][col] / pivot_row[col];
            for (x, p) in a[row].iter_mut().zip(&pivot_row).skip(col) {
                *x -= f * p;
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let s: f64 = (row + 1..n).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - s) / a[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// A measured point of the solve.
struct Point {
    adj: ParametricAdjustments,
    stats: ImageStats,
    /// Normalized residuals, one per active residual.
    r: Vec<f64>,
    merit: f64,
}

/// Contract (vision-ml-dev). The general solver (reused by Phase 9 reference matching): find
/// `full` = `base` + corrections of the groups enabled in `options` (`strength` is ignored
/// here) such that `measure(full)` matches `reference` (see module docs). `measure` renders the
/// *target* with the given adjustments and returns its stats (an error aborts the solve).
///
/// Method: quasi-Newton (Broyden) on normalized residuals (`logMeanLuma` in units of
/// `TOLERANCE_EV`, neutral a/b in units of `TOLERANCE_AB`, tone percentiles in 0.15 EV) over
/// the unknowns exposure (EV), temperature (mireds), tint and, with `matchTone`, contrast /
/// whites / blacks. The initial Jacobian holds the pipeline's typical slopes; every render
/// refines it (so couplings such as exposure moving the neutral pixels are learned). Steps
/// are limited per iteration and always taken from the best point so far; stops inside 40 %
/// of the tolerances or after `MAX_RENDERS` renders. Typically 2-4 renders.
pub fn solve(
    reference: &ImageStats,
    base: &ParametricAdjustments,
    options: &MatchOptions,
    measure: &mut dyn FnMut(&ParametricAdjustments) -> AppResult<ImageStats>,
) -> AppResult<Solved> {
    let mut notes: Vec<String> = Vec::new();
    let mut start = base.clone();
    let mut renders = 1usize;
    let mut first = measure(&start)?;

    let mut vars: Vec<Var> = Vec::new();
    let mut res: Vec<Res> = Vec::new();
    if options.match_exposure {
        vars.push(Var::Exposure);
        res.push(Res::Luma);
    }
    if options.match_white_balance {
        if let WhiteBalance::AsShot = start.white_balance {
            match first.white_balance {
                Some(v) => {
                    // Continue from the explicit values the as-shot setting resolves to.
                    start.white_balance = WhiteBalance::Custom { temperature_k: v.temperature_k, tint: v.tint };
                    first = measure(&start)?;
                    renders += 1;
                }
                None => notes.push("white balance not matched: the file has no as-shot white balance".into()),
            }
        }
        if let WhiteBalance::Custom { .. } = start.white_balance {
            vars.extend([Var::Mired, Var::Tint]);
            res.extend([Res::NeutralA, Res::NeutralB]);
        }
    }
    if options.match_tone {
        vars.extend([Var::Contrast, Var::Whites, Var::Blacks]);
        res.extend([Res::Spread, Res::High, Res::Low]);
    }
    if vars.is_empty() {
        return Ok(Solved { full: start, predicted: first, converged: true, notes });
    }
    if res.contains(&Res::NeutralA) {
        if first.neutral.coverage <= 0.0 {
            notes.push("few neutral surfaces in this frame: white point matched on its average colour".into());
        }
        if reference.neutral.coverage <= 0.0 {
            notes.push("few neutral surfaces in the anchor: white point matched on its average colour".into());
        }
    }

    let (nv, nr) = (vars.len(), res.len());
    let residuals =
        |s: &ImageStats| -> Vec<f64> { res.iter().map(|r| (r.value(s) - r.value(reference)) / r.scale()).collect() };
    let merit =
        |r: &[f64]| -> f64 { r.iter().zip(&res).map(|(v, k)| if k.primary() { v * v } else { 0.25 * v * v }).sum() };
    let point = |adj: ParametricAdjustments, stats: ImageStats| -> Point {
        let r = residuals(&stats);
        let m = merit(&r);
        Point { adj, stats, r, merit: m }
    };
    let within = |p: &Point, inner: f64| -> bool {
        p.r.iter().zip(&res).all(|(v, k)| match k {
            Res::Luma => v.abs() <= inner,
            Res::NeutralA | Res::NeutralB => ab_pair_ok(p, &res, inner),
            _ => true,
        })
    };
    // Normalized Jacobian: J[i][j] = d r_i / d u_j with u_j = var_j / scale_j.
    let init_j = |i: usize, j: usize| initial_slope(res[i], vars[j], reference) * vars[j].scale() / res[i].scale();
    let mut jac: Vec<Vec<f64>> = (0..nr).map(|i| (0..nv).map(|j| init_j(i, j)).collect()).collect();

    let mut best = point(start.clone(), first);
    let mut clamped: Vec<Var> = Vec::new();
    while renders < MAX_RENDERS {
        let tone_ok = best.r.iter().zip(&res).all(|(v, k)| k.primary() || v.abs() <= 1.0);
        if within(&best, INNER_TOLERANCE) && tone_ok {
            break;
        }
        let rhs: Vec<f64> = best.r.iter().map(|v| -v).collect();
        let step = solve_linear(jac.clone(), rhs).unwrap_or_else(|| {
            // Singular: diagonal (Jacobi) step.
            (0..nv).map(|j| if jac[j][j].abs() > 1e-6 { -best.r[j] / jac[j][j] } else { 0.0 }).collect()
        });
        let mut next = best.adj.clone();
        let mut du = vec![0.0f64; nv];
        for (j, &v) in vars.iter().enumerate() {
            let cur = get(&best.adj, v);
            let limit = v.max_step();
            let mut target = cur + (step[j] * v.scale()).clamp(-limit, limit);
            let (mut lo, mut hi) = v.range();
            let b = get(&start, v);
            lo = lo.max(b - v.max_correction());
            hi = hi.min(b + v.max_correction());
            if target < lo || target > hi {
                target = target.clamp(lo, hi);
                if !clamped.contains(&v) {
                    clamped.push(v);
                }
            }
            set(&mut next, v, target);
            du[j] = (get(&next, v) - cur) / v.scale();
        }
        let norm2: f64 = du.iter().map(|d| d * d).sum();
        if norm2 < 1e-10 {
            break; // no move possible (clamped)
        }
        let stats = measure(&next)?;
        renders += 1;
        let p = point(next, stats);
        // Broyden rank-1 update from the step actually taken.
        for (i, row) in jac.iter_mut().enumerate() {
            let pred: f64 = row.iter().zip(&du).map(|(j, d)| j * d).sum();
            let err = (p.r[i] - best.r[i]) - pred;
            for (j, d) in row.iter_mut().zip(&du) {
                *j += err * d / norm2;
            }
        }
        // Keep the own-variable slopes sane (right sign, bounded) so a noisy step cannot
        // flip the model.
        for (i, row) in jac.iter_mut().enumerate() {
            let j0 = init_j(i, i);
            if j0 != 0.0 {
                let ratio = row[i] / j0;
                if !(0.2..=5.0).contains(&ratio) {
                    row[i] = j0 * ratio.clamp(0.2, 5.0);
                }
            }
        }
        if p.merit < best.merit {
            best = p;
        }
    }

    let at_trust =
        |adj: &ParametricAdjustments, v: Var| ((get(adj, v) - get(&start, v)).abs() - v.max_correction()).abs() < 1e-3;
    // A white point that could only be "matched" by a capped correction and still is not
    // is a misleading neutral estimate (blown or colour-lit surfaces): following it half way
    // tints the frame. Keep the anchor's white balance instead (safe: same light).
    if res.contains(&Res::NeutralA)
        && !ab_pair_ok(&best, &res, 1.0)
        && (at_trust(&best.adj, Var::Mired) || at_trust(&best.adj, Var::Tint))
    {
        let mut adj = best.adj.clone();
        adj.white_balance = start.white_balance;
        notes.push(
            "white point not matched (no reliable neutral in this frame): the anchor's white balance is kept".into(),
        );
        // Re-solve the other groups with the white balance fixed (exposure was solved
        // together with the rejected white point).
        let rest = MatchOptions { match_white_balance: false, ..options.clone() };
        let sub = solve(reference, &adj, &rest, measure)?;
        notes.extend(sub.notes);
        best = point(sub.full, sub.predicted);
        clamped.clear();
    }
    let converged = within(&best, 1.0);
    for v in clamped {
        let (lo, hi) = v.range();
        let x = get(&best.adj, v);
        if (x - lo).abs() < 1e-3 || (x - hi).abs() < 1e-3 {
            notes.push(format!("{} correction limited by the slider range", v.name()));
        } else if at_trust(&best.adj, v) {
            notes.push(format!("{} correction capped (unusually large for one scene: check this frame)", v.name()));
        }
    }
    if !converged {
        let r = |k: Res| res.iter().position(|x| *x == k).map(|i| best.r[i] * k.scale());
        let mut why = Vec::new();
        if let Some(d) = r(Res::Luma).filter(|d| d.abs() > f64::from(super::TOLERANCE_EV)) {
            why.push(format!("brightness {d:+.2} EV from the anchor"));
        }
        if let (Some(a), Some(b)) = (r(Res::NeutralA), r(Res::NeutralB)) {
            let d = a.hypot(b);
            if d > f64::from(super::TOLERANCE_AB) {
                why.push(format!("white point {d:.3} (Oklab a/b) from the anchor"));
            }
        }
        if !why.is_empty() {
            notes.push(format!("not fully matched: {}", why.join(", ")));
        }
    }
    Ok(Solved { full: best.adj, predicted: best.stats, converged, notes })
}

/// The white-point tolerance is a Euclidean distance over a and b together.
fn ab_pair_ok(p: &Point, res: &[Res], inner: f64) -> bool {
    let ia = res.iter().position(|x| *x == Res::NeutralA);
    let ib = res.iter().position(|x| *x == Res::NeutralB);
    match (ia, ib) {
        (Some(a), Some(b)) => p.r[a].hypot(p.r[b]) <= inner,
        _ => true,
    }
}

/// Contract (vision-ml-dev). Blocking. Builds one [`MatchPreview`] per target, in `targets`
/// order (anchors are never targets; the caller removed them). Per target: [`choose_anchors`]
/// -> `base` ([`base_adjustments`], blended with `ParametricAdjustments::lerp` for two anchors)
/// -> `reference` (anchor stats via `super::stats::render_stats`, computed once per anchor;
/// [`blend_stats`] for two) -> `before` (target, stored adjustments) -> [`solve`] ->
/// `adjustments = lerp(base, full, strength)` -> `predicted` (= `solved.predicted` when
/// strength is 1, else measured) -> [`delta`]. Targets in parallel (rayon; the develop cache
/// coalesces decodes). A per-target render failure fails the call with that error (the RAW
/// is unreadable). `progress(done, total)` per finished target.
pub fn match_images(
    cache: &DevelopCache,
    luts: &LutLibrary,
    anchors: &[MatchImage],
    targets: &[MatchImage],
    options: &MatchOptions,
    progress: Progress,
) -> AppResult<Vec<MatchPreview>> {
    if anchors.is_empty() {
        return Err(AppError::invalid("match needs at least one anchor"));
    }
    let anchor_stats: Vec<ImageStats> = anchors
        .par_iter()
        .map(|a| render_stats(cache, luts, &a.src, &a.adjustments, None))
        .collect::<AppResult<_>>()?;
    let anchor_times: Vec<Option<i64>> = anchors.iter().map(|a| a.captured_at_ms).collect();
    let total = u32::try_from(targets.len()).unwrap_or(u32::MAX);
    let finished = AtomicU32::new(0);
    targets
        .par_iter()
        .map(|t| {
            let preview = match_one(cache, luts, anchors, &anchor_stats, &anchor_times, t, options);
            let d = finished.fetch_add(1, Ordering::Relaxed) + 1;
            progress(d, total);
            preview
        })
        .collect()
}

fn match_one(
    cache: &DevelopCache,
    luts: &LutLibrary,
    anchors: &[MatchImage],
    anchor_stats: &[ImageStats],
    anchor_times: &[Option<i64>],
    t: &MatchImage,
    options: &MatchOptions,
) -> AppResult<MatchPreview> {
    let choice = choose_anchors(anchor_times, t.captured_at_ms);
    let base_for =
        |i: usize| base_adjustments(&t.adjustments, &anchors[i].adjustments, anchor_stats[i].as_shot, options);
    let p = choice.primary;
    let (base, reference, anchor_ids, anchor_weight) = match choice.second {
        Some((j, w)) => (
            ParametricAdjustments::lerp(&base_for(p), &base_for(j), w),
            blend_stats(&anchor_stats[p], &anchor_stats[j], w),
            vec![anchors[p].src.id, anchors[j].src.id],
            w,
        ),
        None => (base_for(p), anchor_stats[p].clone(), vec![anchors[p].src.id], 0.0),
    };
    let before = render_stats(cache, luts, &t.src, &t.adjustments, None)?;
    let mut measure = |adj: &ParametricAdjustments| render_stats(cache, luts, &t.src, adj, None);
    let solved = solve(&reference, &base, options, &mut measure)?;
    let adjustments = ParametricAdjustments::lerp(&base, &solved.full, options.strength);
    let predicted = if adjustments == solved.full { solved.predicted.clone() } else { measure(&adjustments)? };
    let mut notes = solved.notes;
    if reference.lut_missing || predicted.lut_missing {
        notes.push("the LUT is missing from the library; measured without it".into());
    }
    Ok(MatchPreview {
        target_id: t.src.id,
        anchor_ids,
        anchor_weight,
        delta: delta(&base, &solved.full),
        base,
        full: solved.full,
        adjustments,
        reference,
        before,
        predicted,
        converged: solved.converged,
        notes,
    })
}

/// Groups a match changes (for UI hints / tests): `copyFields` + matched groups.
pub fn touched_fields(options: &MatchOptions) -> Vec<AdjustmentField> {
    let mut out = options.copy_fields.clone();
    for f in options.matched_fields() {
        if !out.contains(&f) {
            out.push(f);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adj(exposure: f32, wb: WhiteBalance) -> ParametricAdjustments {
        ParametricAdjustments { exposure, white_balance: wb, ..Default::default() }
    }

    #[test]
    fn base_copies_fields_and_resolves_as_shot_wb() {
        let target = ParametricAdjustments { exposure: 0.3, clarity: 40.0, ..Default::default() };
        let anchor = ParametricAdjustments { exposure: 1.0, contrast: 25.0, clarity: -10.0, ..Default::default() };
        let as_shot = WhiteBalanceValues { temperature_k: 5100.0, tint: 4.0 };

        // Default options: everything copied, WB resolved to the anchor's as-shot values.
        let b = base_adjustments(&target, &anchor, Some(as_shot), &MatchOptions::default());
        assert_eq!(b.exposure, 1.0);
        assert_eq!(b.clarity, -10.0);
        assert_eq!(b.white_balance, WhiteBalance::Custom { temperature_k: 5100.0, tint: 4.0 });

        // Only exposure matched, nothing else copied: target keeps its clarity + as-shot WB.
        let opts = MatchOptions {
            match_white_balance: false,
            copy_fields: vec![AdjustmentField::Contrast],
            ..MatchOptions::default()
        };
        let b = base_adjustments(&target, &anchor, Some(as_shot), &opts);
        assert_eq!((b.exposure, b.contrast, b.clarity), (1.0, 25.0, 40.0));
        assert_eq!(b.white_balance, WhiteBalance::AsShot);

        // Unknown as-shot values: the anchor's mode is copied.
        let b = base_adjustments(&target, &anchor, None, &MatchOptions::default());
        assert_eq!(b.white_balance, WhiteBalance::AsShot);
        // A custom anchor WB is copied as is.
        let custom = WhiteBalance::Custom { temperature_k: 3200.0, tint: -5.0 };
        let b = base_adjustments(&target, &adj(0.0, custom), Some(as_shot), &MatchOptions::default());
        assert_eq!(b.white_balance, custom);
    }

    #[test]
    fn anchor_choice_nearest_or_between() {
        assert_eq!(choose_anchors(&[Some(10)], Some(99)), AnchorChoice { primary: 0, second: None });
        assert_eq!(choose_anchors(&[Some(0), Some(100)], None), AnchorChoice { primary: 0, second: None });
        assert_eq!(choose_anchors(&[Some(0), None], Some(50)), AnchorChoice { primary: 0, second: None });
        assert_eq!(
            choose_anchors(&[Some(0), Some(100)], Some(25)),
            AnchorChoice { primary: 0, second: Some((1, 0.25)) }
        );
        // Given in reverse time order: primary is the earlier one.
        assert_eq!(
            choose_anchors(&[Some(100), Some(0)], Some(75)),
            AnchorChoice { primary: 1, second: Some((0, 0.75)) }
        );
        // Outside the span: nearest; tie -> first.
        assert_eq!(choose_anchors(&[Some(0), Some(100)], Some(130)), AnchorChoice { primary: 1, second: None });
        assert_eq!(choose_anchors(&[Some(0), Some(100)], Some(-5)), AnchorChoice { primary: 0, second: None });
        assert_eq!(choose_anchors(&[Some(0), Some(100)], Some(0)), AnchorChoice { primary: 0, second: None });
        assert_eq!(choose_anchors(&[Some(50), Some(50)], Some(50)), AnchorChoice { primary: 0, second: None });
    }

    #[test]
    fn delta_and_touched_fields() {
        let base = adj(0.5, WhiteBalance::Custom { temperature_k: 5000.0, tint: 0.0 });
        let full = ParametricAdjustments {
            whites: 5.0,
            ..adj(1.25, WhiteBalance::Custom { temperature_k: 5400.0, tint: -3.0 })
        };
        let d = delta(&base, &full);
        assert_eq!((d.exposure, d.temperature_k, d.tint, d.whites), (0.75, 400.0, -3.0, 5.0));
        assert_eq!(delta(&adj(0.0, WhiteBalance::AsShot), &full).temperature_k, 0.0);

        let opts = MatchOptions { copy_fields: vec![AdjustmentField::Clarity], match_tone: true, ..Default::default() };
        let f = touched_fields(&opts);
        assert_eq!(f.len(), 6);
        assert!(f.contains(&AdjustmentField::Clarity) && f.contains(&AdjustmentField::Blacks));
    }

    // ---- solve() on synthetic renders -------------------------------------------------

    use crate::develop::pipeline::RenderedImage;
    use crate::ipc::types::Histogram;
    use crate::scene::stats::measure;
    use crate::scene::{TOLERANCE_AB, TOLERANCE_EV};

    /// A synthetic scene: linear reflectances (grey card, white shirt, skin, red, foliage,
    /// dark suit...) tiled over a frame, lit by an illuminant `(mired, tint)` at `level` EV.
    struct Scene {
        patches: Vec<[f32; 3]>,
        mired: f32,
        tint: f32,
        level: f32,
    }

    impl Scene {
        fn new(variant: usize, mired: f32, tint: f32, level: f32) -> Self {
            let mut patches = vec![
                [0.18, 0.18, 0.18],
                [0.60, 0.60, 0.60],
                [0.55, 0.40, 0.32],
                [0.45, 0.08, 0.06],
                [0.10, 0.22, 0.07],
                [0.03, 0.03, 0.035],
                [0.40, 0.40, 0.40],
                [0.12, 0.16, 0.35],
            ];
            // Different framing: another mix of the same surfaces.
            patches.rotate_left(variant % 8);
            patches.truncate(6 + variant % 3);
            Scene { patches, mired, tint, level }
        }

        /// Render with a toy pipeline: WB setting vs illuminant -> R/B and G gains
        /// (warmer when the setting's mireds are below the light's), exposure gain, a
        /// filmic-ish curve, contrast around grey, sRGB encode.
        fn render(&self, adj: &ParametricAdjustments) -> RenderedImage {
            let (set_mired, set_tint) = match adj.white_balance {
                WhiteBalance::Custom { temperature_k, tint } => (1.0e6 / temperature_k, tint),
                WhiteBalance::AsShot => (self.mired, self.tint),
            };
            let warm = (self.mired - set_mired) * 0.004;
            let magenta = (set_tint - self.tint) * 0.012;
            let gains = [warm.exp2(), (-magenta).exp2(), (-warm).exp2()];
            let ev = self.level + adj.exposure;
            let contrast = 1.0 + adj.contrast / 100.0 * 0.45;
            let (w, h) = (96usize, 64usize);
            let mut rgb = Vec::with_capacity(w * h * 3);
            for y in 0..h {
                for x in 0..w {
                    let p = self.patches[(x / 16 + (y / 16) * 6) % self.patches.len()];
                    // Gentle shading so percentiles are not degenerate.
                    let shade = 0.7 + 0.6 * (x as f32 / w as f32);
                    let v: Vec<f32> = (0..3)
                        .map(|c| {
                            let lin = p[c] * gains[c] * shade * ev.exp2();
                            let e = (lin.max(1e-6) / 0.18).log2() * contrast;
                            let s = 0.18 * e.exp2();
                            let d = s / (s + 0.35) * 1.35;
                            (color::srgb_encode(d) * 255.0 + 0.5).clamp(0.0, 255.0)
                        })
                        .collect();
                    rgb.extend(v.iter().map(|c| *c as u8));
                }
            }
            RenderedImage {
                width: w as u32,
                height: h as u32,
                rgb,
                histogram: Histogram { red: vec![], green: vec![], blue: vec![], luma: vec![] },
            }
        }

        fn stats(&self, adj: &ParametricAdjustments) -> ImageStats {
            let mut s = measure(1, &self.render(adj), None);
            s.white_balance = match adj.white_balance {
                WhiteBalance::Custom { temperature_k, tint } => Some(WhiteBalanceValues { temperature_k, tint }),
                WhiteBalance::AsShot => Some(WhiteBalanceValues { temperature_k: 1.0e6 / self.mired, tint: self.tint }),
            };
            s
        }
    }

    fn custom(k: f32, tint: f32, exposure: f32) -> ParametricAdjustments {
        ParametricAdjustments {
            exposure,
            white_balance: WhiteBalance::Custom { temperature_k: k, tint },
            ..Default::default()
        }
    }

    fn check(reference: &ImageStats, got: &ImageStats) {
        assert!((got.log_mean_luma - reference.log_mean_luma).abs() <= TOLERANCE_EV, "EV {got:?}");
        let d = (got.neutral.a - reference.neutral.a).hypot(got.neutral.b - reference.neutral.b);
        assert!(d <= TOLERANCE_AB, "ab {d}");
    }

    #[test]
    fn solve_matches_brightness_and_white_point_on_synthetic_renders() {
        // Anchor: daylight scene graded warm (+8 tint, 6200 K setting under 5500 K light), +0.6 EV.
        let anchor = Scene::new(0, 1.0e6 / 5500.0, 0.0, -0.5);
        let anchor_adj = custom(6200.0, 8.0, 0.6);
        let reference = anchor.stats(&anchor_adj);
        // Same look pasted on frames with other framing, a warmer light (4800 K) with a
        // green cast and 1 EV less light: the plain copy is off, the solve fixes it.
        for (variant, mired, tint, level) in
            [(1, 1.0e6 / 4800.0, -6.0, -1.5), (2, 1.0e6 / 6500.0, 5.0, 0.2), (3, 1.0e6 / 5500.0, 0.0, -0.5)]
        {
            let target = Scene::new(variant, mired, tint, level);
            let mut renders = 0;
            let mut m = |a: &ParametricAdjustments| {
                renders += 1;
                Ok(target.stats(a))
            };
            let solved = solve(&reference, &anchor_adj, &MatchOptions::default(), &mut m).unwrap();
            assert!(solved.converged, "variant {variant}: {:?}", solved.notes);
            assert!(renders <= MAX_RENDERS, "{renders} renders");
            check(&reference, &target.stats(&solved.full));
            assert_eq!(solved.predicted, target.stats(&solved.full), "predicted = measured full");
            // Only the matched groups move; the look's other sliders stay.
            assert_eq!(solved.full.contrast, anchor_adj.contrast);
            // Warmer light (4800 K) needs a lower temperature setting to look the same.
            if variant == 1 {
                let d = delta(&anchor_adj, &solved.full);
                assert!(d.temperature_k < -200.0 && d.exposure > 0.5, "{d:?}");
            }
        }
    }

    #[test]
    fn solve_respects_disabled_groups_and_as_shot_base() {
        let anchor = Scene::new(0, 200.0, 0.0, -0.5);
        let reference = anchor.stats(&custom(5000.0, 0.0, 0.0));
        let target = Scene::new(1, 200.0, 0.0, -1.7);
        let opts = MatchOptions { match_white_balance: false, ..MatchOptions::default() };
        let base = ParametricAdjustments::default(); // as-shot WB
        let solved = solve(&reference, &base, &opts, &mut |a| Ok(target.stats(a))).unwrap();
        assert!(solved.converged);
        assert_eq!(solved.full.white_balance, WhiteBalance::AsShot, "WB untouched when not matched");
        assert!((solved.predicted.log_mean_luma - reference.log_mean_luma).abs() <= TOLERANCE_EV);

        // As-shot base with WB matching: resolved to custom values, then corrected.
        let target = Scene::new(2, 240.0, 4.0, -0.5);
        let solved = solve(&reference, &base, &MatchOptions::default(), &mut |a| Ok(target.stats(a))).unwrap();
        assert!(solved.converged, "{:?}", solved.notes);
        assert!(matches!(solved.full.white_balance, WhiteBalance::Custom { .. }));
        check(&reference, &solved.predicted);

        // Nothing matched: base returned as is, one render.
        let none = MatchOptions { match_exposure: false, match_white_balance: false, ..MatchOptions::default() };
        let mut n = 0;
        let solved = solve(&reference, &base, &none, &mut |a| {
            n += 1;
            Ok(target.stats(a))
        })
        .unwrap();
        assert_eq!((solved.full, n), (base.clone(), 1));
    }

    #[test]
    fn solve_with_tone_and_clamped_exposure() {
        let anchor = Scene::new(0, 200.0, 0.0, -0.5);
        let anchor_adj = ParametricAdjustments { contrast: 30.0, ..custom(5000.0, 0.0, 0.3) };
        let reference = anchor.stats(&anchor_adj);
        let target = Scene::new(0, 200.0, 0.0, -1.0);
        let opts = MatchOptions { match_tone: true, ..MatchOptions::default() };
        let solved = solve(&reference, &custom(5000.0, 0.0, 0.3), &opts, &mut |a| Ok(target.stats(a))).unwrap();
        assert!(solved.converged, "{:?}", solved.notes);
        assert!((solved.full.contrast - 30.0).abs() < 15.0, "contrast found {}", solved.full.contrast);
        assert!(solved.full.validate().is_ok());

        // 9 EV too dark: the correction is capped at +3 EV from base, not converged, noted.
        let dark = Scene::new(1, 200.0, 0.0, -9.5);
        let solved = solve(&reference, &anchor_adj, &MatchOptions::default(), &mut |a| Ok(dark.stats(a))).unwrap();
        assert!(!solved.converged);
        assert!((solved.full.exposure - 3.3).abs() < 1e-5, "{}", solved.full.exposure);
        assert!(solved.notes.iter().any(|n| n.contains("exposure correction capped")), "{:?}", solved.notes);
        // ... and by the slider range when base is already bright.
        let bright_base = ParametricAdjustments { exposure: 3.0, ..anchor_adj.clone() };
        let solved = solve(&reference, &bright_base, &MatchOptions::default(), &mut |a| Ok(dark.stats(a))).unwrap();
        assert_eq!(solved.full.exposure, 5.0);
        assert!(solved.notes.iter().any(|n| n.contains("slider range")), "{:?}", solved.notes);
        assert!(solved.full.validate().is_ok());
    }

    #[test]
    fn solve_keeps_anchor_white_balance_when_the_frame_has_no_neutral() {
        let reference = Scene::new(0, 200.0, 0.0, -0.5).stats(&custom(5000.0, 0.0, 0.3));
        // Only saturated warm surfaces (stage lights on red drapes): the "neutral" is red.
        let red = Scene {
            patches: vec![[0.50, 0.10, 0.05], [0.35, 0.12, 0.04], [0.60, 0.25, 0.08]],
            mired: 200.0,
            tint: 0.0,
            level: -1.2,
        };
        let base = custom(5000.0, 0.0, 0.3);
        let solved = solve(&reference, &base, &MatchOptions::default(), &mut |a| Ok(red.stats(a))).unwrap();
        assert!(!solved.converged);
        assert_eq!(solved.full.white_balance, base.white_balance, "{:?}", solved.notes);
        assert!(solved.notes.iter().any(|n| n.contains("white balance is kept")), "{:?}", solved.notes);
        // Brightness is still matched (re-solved with the white balance fixed).
        assert!((solved.predicted.log_mean_luma - reference.log_mean_luma).abs() <= TOLERANCE_EV);
        assert_eq!(solved.predicted, red.stats(&solved.full));
    }

    #[test]
    fn solve_propagates_measure_errors() {
        let reference = Scene::new(0, 200.0, 0.0, 0.0).stats(&custom(5000.0, 0.0, 0.0));
        let err = solve(&reference, &ParametricAdjustments::default(), &MatchOptions::default(), &mut |_| {
            Err(AppError::internal("unreadable"))
        });
        assert!(err.is_err());
    }

    #[test]
    fn blend_stats_interpolates() {
        let s = Scene::new(0, 200.0, 0.0, 0.0);
        let a = s.stats(&custom(5000.0, 0.0, 0.0));
        let b = s.stats(&custom(4000.0, 10.0, 1.0));
        let mid = blend_stats(&a, &b, 0.5);
        let zero = blend_stats(&a, &b, 0.0);
        assert_eq!(
            (zero.log_mean_luma, zero.neutral, zero.white_balance),
            (a.log_mean_luma, a.neutral, a.white_balance)
        );
        assert!((mid.log_mean_luma - (a.log_mean_luma + b.log_mean_luma) / 2.0).abs() < 1e-5);
        assert!((mid.neutral.b - (a.neutral.b + b.neutral.b) / 2.0).abs() < 1e-6);
        let p50 = (a.percentiles.p50 * b.percentiles.p50).sqrt();
        assert!((mid.percentiles.p50 - p50).abs() < 1e-4);
        let wb = mid.white_balance.unwrap();
        assert!((wb.temperature_k - 1.0e6 / 225.0).abs() < 1.0 && (wb.tint - 5.0).abs() < 1e-4);
        let b2 = ImageStats { image_id: 9, ..b.clone() };
        assert_eq!(blend_stats(&a, &b2, 1.0).image_id, a.image_id);
    }

    #[test]
    fn neutral_estimate_follows_the_cast_not_the_colourful_objects() {
        let s = Scene::new(0, 200.0, 0.0, -0.3);
        let balanced = s.stats(&custom(5000.0, 0.0, 0.0));
        assert!(balanced.neutral.a.hypot(balanced.neutral.b) < 0.01, "{:?}", balanced.neutral);
        assert!(balanced.neutral.coverage > 0.05);
        let warm = s.stats(&custom(7000.0, 0.0, 0.0));
        assert!(warm.neutral.b > balanced.neutral.b + 0.01, "{:?}", warm.neutral);
        let magenta = s.stats(&custom(5000.0, 20.0, 0.0));
        assert!(magenta.neutral.a > balanced.neutral.a + 0.01, "{:?}", magenta.neutral);
        // Brightness alone barely moves the estimate.
        let bright = s.stats(&custom(5000.0, 0.0, 0.7));
        assert!((bright.neutral.b - balanced.neutral.b).abs() < 0.004);
    }
}
