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

use super::{MatchImage, Progress};
use crate::develop::DevelopCache;
use crate::ipc::error::AppResult;
use crate::ipc::types::{
    AdjustmentField, ImageStats, MatchDelta, MatchOptions, MatchPreview, ParametricAdjustments, WhiteBalance,
    WhiteBalanceValues,
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
    let _ = (a, b, weight);
    todo!("vision-ml-dev: interpolate reference stats between two anchors")
}

/// Contract (vision-ml-dev). The general solver (reused by Phase 9 reference matching): find
/// `full` = `base` + corrections of the groups enabled in `options` (`strength` is ignored
/// here) such that `measure(full)` matches `reference` (see module docs). `measure` renders the
/// *target* with the given adjustments and returns its stats (an error aborts the solve).
pub fn solve(
    reference: &ImageStats,
    base: &ParametricAdjustments,
    options: &MatchOptions,
    measure: &mut dyn FnMut(&ParametricAdjustments) -> AppResult<ImageStats>,
) -> AppResult<Solved> {
    let _ = (reference, base, options, measure);
    todo!("vision-ml-dev: iterative exposure / white point / tone correction")
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
    let _ = (cache, luts, anchors, targets, options, progress);
    todo!("vision-ml-dev: orchestrate renders + solve per target")
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
}
