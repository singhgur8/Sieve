//! Baseline edit engine (Phase 10, IPC v21): one preset + one edited photo -> the whole shoot.
//!
//! Owned by rust-engine-dev (light normalization, clamps, smoothing, WB per lighting, low-key /
//! silhouette detection). The architect fixed the surface used by `ipc::commands` and `lib.rs`:
//! [`BaselineEdit`] (`new`, `is_running`, `start`, `cancel`), [`BaselineJob`], [`preview`] and
//! [`run_pipeline`]; the stages below are the seams the tests use.
//!
//! Model (roadmap Phase 10, decisions.md 2026-10-10; partition: [`BASELINE_PARTITION`]):
//! - **Look** = the anchor's settings for every `look` group, copied as-is ([`compose`]; the
//!   anchor carries the preset, see `db::baseline::look_source`).
//! - **Light** (exposure, contrast, highlights, shadows, whites, blacks, white balance), per
//!   photo, in five stages:
//!   1. **Measure** ([`LightMeter::measure`], in parallel, [`measure_threads`] workers, progress
//!      and cancel per photo): the photo's own Auto with the look ([`measured_settings`]: its
//!      settings with the anchor's look groups) through `develop::auto::auto_light`, the one
//!      light-only Auto Develop's Auto must also use on the anchor (auto white balance first,
//!      then auto tone of the six light sliders, faces weighted, under that white balance;
//!      vibrance / saturation untouched), and the [`FrameStats`] of the frame as exposed (every
//!      Auto slider at 0). Auto WB failing (too few neutral tones) -> the camera's as-shot,
//!      replaced by the group's white balance in smoothing (flagged only when no group has one).
//!   2. **Auto + offset** ([`photo_light`]): `light = Auto(photo) + anchor.offset`, the offset
//!      being the anchor's light minus its own Auto ([`measure_anchor`], [`LightOffset`]):
//!      exposure in EV, the tone sliders in slider units, temperature in **mireds** (so "a bit
//!      warmer than Auto" means the same shift under tungsten and daylight), tint additive.
//!      An anchor whose white balance is `as_shot` uses the camera's as-shot values (Auto WB if
//!      the camera recorded none). Auto failing -> the anchor's light, flagged `auto_failed`
//!      (smoothing then gives it its scene's light).
//!   3. **Low-key / silhouette** ([`low_key`]): a frame Auto wants to brighten that is a
//!      back-lit silhouette (bright surround, dark subject / dark face, little mid-tone) or a
//!      deliberate low-key frame (mostly dark with exposed highlights, lit face if any) keeps
//!      the anchor's offset **without Auto's lift** ([`without_lift`]: exposure and shadows
//!      not raised) and is flagged `silhouette` / `low_key`.
//!   4. **Smoothing** ([`smooth`]): per scene (`scene_id`; photos without one are chained by
//!      capture time, gaps <= [`PSEUDO_SCENE_GAP_MS`]), then per burst (`burst_group_id`):
//!      every light value is pulled towards the group's robust centre — the median, or the
//!      anchor's value when the anchor belongs to the group — keeping a limited share of each
//!      photo's own deviation ([`SCENE_PULL`], [`BURST_PULL`]). Exposure is smoothed in
//!      scene-referred terms: Auto EV + the camera's exposure (EXIF shutter x ISO / f-number²,
//!      relative to the group), so frames the camera exposed differently still render alike.
//!      White balance is clustered per lighting (mireds, gap [`MIRED_GAP`]): a scene mixing
//!      e.g. tungsten and window light keeps one balance per light and flags the photos
//!      outside the main light `mixed_light`. Exposure clusters likewise (gap
//!      [`EXPOSURE_GAP`]: flash vs ambient). Low-key frames are smoothed only among
//!      themselves; Auto failures receive their group's centre.
//!   5. **Clamps** ([`clamp_light`]): sane bounds ([`SANE`]), rounded like Lightroom (exposure
//!      0.05 EV, others integers, whole Kelvin); out-of-bounds values add `clamped` (not a
//!      flag on its own).
//!
//!   Deterministic: same catalog + settings -> same values (scope order is capture time, then
//!   id; parallel measurement is per photo and order-independent; ties broken by index).
//! - **Never**: crop, transform, masks keep the photo's own values (unmodelled `crs:` keys such
//!   as spot removal stay untouched in the sidecar).
//! - Scope / skip: `db::baseline::scope_photos`; edited photos are `skipped_edited` unless
//!   `replaceEdited`; photos on an earlier baseline and unedited photos are written; the anchor
//!   is never written.
//! - Preview: the same stages on the sampled photos (smoothing then sees only the sampled frames
//!   of a group plus the anchor, so a preview value can differ from the run's by up to the
//!   pull's deviation).
//!
//! Acceptance numbers (roadmap "Light normalization engine"): `examples/baseline_eval.rs` on
//! the synthetic scenes of [`synth`] (+ sample RAWs), recorded in the Status Log.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rayon::prelude::*;
use rusqlite::Connection;
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

pub use super::auto::FrameStats;
use super::{DevelopCache, SourceImage};
use crate::db::baseline::{self as store, BaselineDraft, PhotoEditState, ScopePhoto};
use crate::ipc::activity::activities;
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::events::{ActivityKind, ActivityState, BaselineRunFinished};
use crate::ipc::types::*;

#[doc(hidden)]
pub mod synth;

/// Version of the engine (stored in `BaselineRun.engineVersion`). Bump when results change.
pub const ENGINE_VERSION: &str = "baseline-2";

/// The Auto sliders the light stage uses (Lightroom's Basic Auto minus vibrance / saturation,
/// which are look).
pub const LIGHT_TONE_FIELDS: &[AdjustmentField] = super::auto::AUTO_LIGHT_FIELDS;

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

/// What the engine knows about one photo.
#[derive(Debug, Clone, PartialEq)]
pub struct PhotoInput {
    pub image_id: ImageId,
    pub src: SourceImage,
    pub format: ImageFormat,
    /// Current settings (the never groups are kept from these).
    pub current: ParametricAdjustments,
    pub scene_id: Option<SceneId>,
    pub burst_group_id: Option<BurstGroupId>,
    pub captured_at_ms: Option<i64>,
    /// Analysis face boxes (oriented preview frame), `None` when not analysed.
    pub faces: Option<Vec<NormRect>>,
    pub state: PhotoEditState,
    /// The original is missing (`RawImageEntry.missingSinceMs`).
    pub missing: bool,
    /// The camera's exposure from EXIF ([`camera_ev`]): log2(shutter x ISO/100 / f-number²),
    /// higher = brighter capture. `None` when any of the three is unknown.
    pub exposure_ev: Option<f32>,
}

/// Camera exposure in EV (higher = more light captured): log2(t x ISO / 100 / N²).
pub fn camera_ev(c: &CaptureMeta) -> Option<f32> {
    let (t, iso, n) = (c.shutter_seconds?, c.iso?, c.aperture?);
    (t > 0.0 && iso > 0 && n > 0.0).then(|| (t * f64::from(iso) / 100.0 / f64::from(n * n)).log2() as f32)
}

impl PhotoInput {
    /// From a scope row + its current settings and faces.
    pub fn new(p: &ScopePhoto, current: ParametricAdjustments, faces: Option<Vec<NormRect>>) -> Self {
        let e = &p.entry;
        PhotoInput {
            image_id: e.id,
            src: SourceImage { id: e.id, path: PathBuf::from(&e.path), orientation: e.orientation },
            format: e.format,
            current,
            scene_id: e.scene_id,
            burst_group_id: e.burst_group_id,
            captured_at_ms: e.capture.captured_at_ms,
            faces,
            state: p.state,
            missing: e.missing_since_ms.is_some(),
            exposure_ev: camera_ev(&e.capture),
        }
    }
}

/// Reads the inputs of `photos` (current settings + analysis faces).
pub fn photo_inputs(conn: &Connection, photos: &[ScopePhoto]) -> AppResult<Vec<PhotoInput>> {
    photos
        .iter()
        .map(|p| {
            let current = crate::db::repo::get_adjustments(conn, p.entry.id)?;
            let faces = super::auto::analysis_faces(conn, p.entry.id)?.map(|f| super::auto::face_boxes(&f));
            Ok(PhotoInput::new(p, current, faces))
        })
        .collect()
}

/// One photo's measurement: its Auto light and, when the meter renders, the frame as exposed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightMeasurement {
    pub auto: LightValues,
    pub frame: Option<FrameStats>,
    /// Auto white balance could not be estimated (too few neutral pixels): `auto`'s white
    /// balance is the camera's as-shot; smoothing gives the photo its group's white balance.
    pub wb_fallback: bool,
}

/// Measures Auto. [`DevelopMeter`] in the app; synthetic meters in tests.
pub trait LightMeter: Sync {
    /// Lightroom-style Auto light of `photo` rendered with `look` (its light groups ignored):
    /// the six [`LIGHT_TONE_FIELDS`] + auto white balance (always custom).
    fn auto_light(&self, photo: &PhotoInput, look: &ParametricAdjustments) -> AppResult<LightValues>;
    /// The camera's as-shot temperature / tint (to resolve an `as_shot` anchor).
    fn as_shot(&self, photo: &PhotoInput) -> AppResult<WhiteBalanceValues>;
    /// [`Self::auto_light`] plus the frame statistics low-key detection uses (`None` = unknown:
    /// never low-key).
    fn measure(&self, photo: &PhotoInput, look: &ParametricAdjustments) -> AppResult<LightMeasurement> {
        Ok(LightMeasurement { auto: self.auto_light(photo, look)?, frame: None, wb_fallback: false })
    }
}

/// [`LightMeter`] over the develop cache (`develop::auto`). Sources it decodes are dropped from
/// the cache again after measuring (memory stays flat over a whole shoot and the photos being
/// edited stay cached).
#[derive(Clone)]
pub struct DevelopMeter {
    pub cache: DevelopCache,
}

impl LightMeter for DevelopMeter {
    fn auto_light(&self, photo: &PhotoInput, look: &ParametricAdjustments) -> AppResult<LightValues> {
        self.measure(photo, look).map(|m| m.auto)
    }

    fn as_shot(&self, photo: &PhotoInput) -> AppResult<WhiteBalanceValues> {
        self.cache
            .info(&photo.src)?
            .as_shot
            .ok_or_else(|| AppError::invalid("the camera's as-shot white balance is unknown"))
    }

    fn measure(&self, photo: &PhotoInput, look: &ParametricAdjustments) -> AppResult<LightMeasurement> {
        let was_decoded = self.cache.is_decoded(photo.image_id);
        let result = (|| {
            let adj = measured_settings(photo, look);
            let faces = super::auto::resolve_faces(&self.cache, &photo.src, photo.faces.clone());
            let a = super::auto::auto_light(&self.cache, &photo.src, &adj, faces.as_deref())?;
            let (wb, wb_fallback) = match a.white_balance {
                Some(wb) => (wb, false),
                None => match self.as_shot(photo) {
                    Ok(wb) => (wb, true),
                    Err(_) => return Err(AppError::invalid("could not estimate a white balance for this photo")),
                },
            };
            Ok(LightMeasurement {
                auto: LightValues {
                    exposure: a.tone.exposure.unwrap_or(0.0),
                    contrast: a.tone.contrast.unwrap_or(0.0),
                    highlights: a.tone.highlights.unwrap_or(0.0),
                    shadows: a.tone.shadows.unwrap_or(0.0),
                    whites: a.tone.whites.unwrap_or(0.0),
                    blacks: a.tone.blacks.unwrap_or(0.0),
                    temperature_k: wb.temperature_k,
                    tint: wb.tint,
                },
                frame: Some(a.frame),
                wb_fallback,
            })
        })();
        if !was_decoded {
            self.cache.drop_decoded(photo.image_id);
        }
        result
    }
}

/// The settings a photo is measured with: its current settings with every look group copied
/// from `look` (never groups such as the anchor's masks / crop are not carried over; light
/// groups are replaced by Auto anyway). For the anchor itself this is its own settings, which
/// is what Develop's Auto measures ([`super::auto::auto_light`]).
pub fn measured_settings(photo: &PhotoInput, look: &ParametricAdjustments) -> ParametricAdjustments {
    let mut a = photo.current.clone();
    a.copy_fields(look, &fields_of_class(SettingClass::Look));
    a
}

// ---------------------------------------------------------------------------
// Stages
// ---------------------------------------------------------------------------

/// `current` with every look group copied from `look` and the light groups set to `light`;
/// never groups (crop, transform, masks) keep `current`'s values.
pub fn compose(
    current: &ParametricAdjustments,
    look: &ParametricAdjustments,
    light: &LightValues,
) -> ParametricAdjustments {
    let mut out = current.clone();
    out.copy_fields(look, &fields_of_class(SettingClass::Look));
    light.apply_to(&out)
}

/// The anchor's light values (an `as_shot` white balance resolved to the camera's values, or
/// to the anchor's Auto WB when the camera recorded none), its Auto with its look, and the
/// offset between them.
pub fn measure_anchor(
    meter: &dyn LightMeter,
    anchor: &PhotoInput,
    look: &ParametricAdjustments,
) -> AppResult<BaselineAnchor> {
    let auto = meter.measure(anchor, look)?.auto;
    let light = match LightValues::of(look) {
        Some(v) => v,
        None => {
            let wb = meter
                .as_shot(anchor)
                .unwrap_or(WhiteBalanceValues { temperature_k: auto.temperature_k, tint: auto.tint });
            LightValues::of(&ParametricAdjustments {
                white_balance: WhiteBalance::Custom { temperature_k: wb.temperature_k, tint: wb.tint },
                ..look.clone()
            })
            .unwrap_or(auto)
        }
    };
    Ok(BaselineAnchor { image_id: anchor.image_id, light, auto, offset: LightOffset::between(&light, &auto) })
}

/// Light of one photo before smoothing.
#[derive(Debug, Clone, PartialEq)]
pub struct PhotoLight {
    /// The photo's Auto (`None`: Auto failed).
    pub auto: Option<LightValues>,
    pub light: LightValues,
    pub reasons: Vec<BaselineReason>,
    /// Low-key / silhouette: kept at its own brightness (smoothed only with its kind).
    pub kept_dark: bool,
    /// Auto WB failed: the white balance comes from the photo's group ([`smooth`]).
    pub wb_fallback: bool,
    /// [`smooth`] gave this photo its group's white balance (or whole light, Auto failed).
    pub filled: bool,
}

/// `auto` without its brightening: exposure and shadows not raised above 0 (what the camera
/// captured), the rest (highlight recovery, whites / blacks, contrast, white balance) kept.
pub fn without_lift(auto: &LightValues) -> LightValues {
    LightValues { exposure: auto.exposure.min(0.0), shadows: auto.shadows.min(0.0), ..*auto }
}

/// Per-photo light: `auto + anchor.offset` ([`LightOffset::add_to`]); low-key / silhouette
/// frames ([`low_key`]) use [`without_lift`] of their Auto; Auto failure -> the anchor's light +
/// `auto_failed`.
pub fn photo_light(
    meter: &dyn LightMeter,
    photo: &PhotoInput,
    look: &ParametricAdjustments,
    anchor: &BaselineAnchor,
) -> PhotoLight {
    match meter.measure(photo, look) {
        Ok(m) => {
            let dark = low_key(photo, &m.auto, m.frame.as_ref());
            let base = if dark.is_some() { without_lift(&m.auto) } else { m.auto };
            PhotoLight {
                auto: Some(m.auto),
                light: anchor.offset.add_to(&base),
                kept_dark: dark.is_some(),
                reasons: dark.into_iter().collect(),
                wb_fallback: m.wb_fallback,
                filled: false,
            }
        }
        Err(e) => auto_failed(anchor, &e.message),
    }
}

fn auto_failed(anchor: &BaselineAnchor, why: &str) -> PhotoLight {
    PhotoLight {
        auto: None,
        light: anchor.light,
        reasons: vec![BaselineReason {
            kind: BaselineReasonKind::AutoFailed,
            text: format!("Auto could not be computed ({why}); used the light of its scene / the anchor"),
        }],
        kept_dark: false,
        wb_fallback: true,
        filled: false,
    }
}

/// Auto must want to brighten by at least this much (EV) before a frame can be kept dark.
pub const LOW_KEY_MIN_LIFT: f32 = 0.25;

/// Deliberate low-key frame / back-lit silhouette, from the frame as exposed (`frame`, every
/// Auto slider at 0) and the faces: `Some` keeps the frame from being brightened blindly and
/// flags it. Only frames Auto would lift by >= [`LOW_KEY_MIN_LIFT`] qualify.
///
/// - **Silhouette**: >= 15 % bright (L* > 70) and >= 25 % dark (L* < 20) pixels, at most 45 %
///   mid-tones, highlights near white (p99.5 >= 0.85), and the subject dark: the face (when
///   known) below L* 25, or without a face the surround >= 10 L* brighter than the centre.
///   A bright face is never a silhouette.
/// - **Low-key**: >= 60 % dark, mean L* < 25, < 15 % bright, <= 35 % mid-tones, exposed
///   highlights (p99.5 >= 0.8: the exposure was set for them), and a lit face (L* >= 30)
///   when faces are known. An underexposed frame (no highlights, or a dark face) is not.
pub fn low_key(_photo: &PhotoInput, auto: &LightValues, frame: Option<&FrameStats>) -> Option<BaselineReason> {
    let f = frame?;
    if auto.exposure < LOW_KEY_MIN_LIFT {
        return None;
    }
    let lift = auto.exposure;
    let face_dark = f.face_l.map(|l| l < 25.0);
    let backlit = f.bright >= 0.15 && f.dark >= 0.25 && f.mid <= 0.45 && f.white_p995 >= 0.85;
    let subject_dark = match face_dark {
        Some(dark) => dark,
        None => f.border_l >= f.centre_l + 10.0,
    };
    if backlit && subject_dark {
        return Some(BaselineReason {
            kind: BaselineReasonKind::Silhouette,
            text: format!(
                "Back-lit silhouette (dark subject against a bright background): kept dark instead of Auto's +{lift:.1} EV"
            ),
        });
    }
    let lit_face = f.face_l.is_none_or(|l| l >= 30.0);
    if f.dark >= 0.6 && f.mean_l < 25.0 && f.bright < 0.15 && f.mid <= 0.35 && f.white_p995 >= 0.8 && lit_face {
        return Some(BaselineReason {
            kind: BaselineReasonKind::LowKey,
            text: format!(
                "Looks like a deliberate low-key frame (mostly dark, highlights exposed): kept its own brightness instead of Auto's +{lift:.1} EV"
            ),
        });
    }
    None
}

/// How far [`smooth`] pulls a group's values towards its centre: `v = c + clamp(k (v - c), ±d)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pull {
    /// Share of each photo's own deviation kept.
    pub keep: f32,
    /// Largest remaining deviation: exposure (EV) when the camera exposure is known for the
    /// whole group ...
    pub exposure: f32,
    /// ... and without it (Auto's own exposure differences may then be real exposure
    /// differences, so less is removed).
    pub exposure_keep_no_exif: f32,
    pub exposure_no_exif: f32,
    /// Contrast, highlights, shadows, whites, blacks (slider units).
    pub slider: f32,
    /// Temperature (mireds) and tint.
    pub mired: f32,
    pub tint: f32,
}

/// Scene level: consistent light through a scene, each frame keeping some of its own.
pub const SCENE_PULL: Pull = Pull {
    keep: 0.5,
    exposure: 0.3,
    exposure_keep_no_exif: 0.8,
    exposure_no_exif: 0.5,
    slider: 10.0,
    mired: 12.0,
    tint: 5.0,
};

/// Burst level (same moment, same light): nearly identical light.
pub const BURST_PULL: Pull = Pull {
    keep: 0.25,
    exposure: 0.1,
    exposure_keep_no_exif: 0.5,
    exposure_no_exif: 0.25,
    slider: 4.0,
    mired: 5.0,
    tint: 2.0,
};

/// Scene-referred exposures further apart than this (EV) are different light (flash / ambient,
/// lights switched): separate clusters, not averaged.
pub const EXPOSURE_GAP: f32 = 1.0;
/// White balances further apart than this (mireds; tungsten 3000 K = 333, daylight 5500 K =
/// 182) are different lighting: separate clusters, `mixed_light` at scene level.
pub const MIRED_GAP: f32 = 40.0;
/// Photos without a scene, taken at most this far apart, are smoothed as one scene.
pub const PSEUDO_SCENE_GAP_MS: i64 = 90_000;

fn mired(k: f32) -> f32 {
    1e6 / k.max(1.0)
}

fn median(values: &[f32]) -> f32 {
    let mut v = values.to_vec();
    v.sort_by(f32::total_cmp);
    let n = v.len();
    if n == 0 {
        0.0
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// Single-linkage clusters of `values` (positions), split where sorted neighbours are more than
/// `gap` apart; in value order, ties by position.
fn clusters(values: &[f32], gap: f32) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|&a, &b| values[a].total_cmp(&values[b]).then(a.cmp(&b)));
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut last = f32::NEG_INFINITY;
    for i in order {
        match out.last_mut() {
            Some(c) if values[i] - last <= gap => c.push(i),
            _ => out.push(vec![i]),
        }
        last = values[i];
    }
    out
}

/// Pulls `values[members]` towards `centre` (the median of the members when `None`).
fn pull(values: &mut [f32], members: &[usize], centre: Option<f32>, keep: f32, max_dev: f32) -> f32 {
    let c = centre.unwrap_or_else(|| median(&members.iter().map(|&i| values[i]).collect::<Vec<_>>()));
    if members.len() + usize::from(centre.is_some()) >= 2 {
        for &i in members {
            values[i] = c + (keep * (values[i] - c)).clamp(-max_dev, max_dev);
        }
    }
    c
}

/// One member of a smoothing group: a measured photo (`Some(index)` into `lights`) or the
/// anchor (`None`, fixed).
#[derive(Clone, Copy)]
struct Member {
    light: Option<usize>,
}

/// Groups of [`smooth`]: scenes (with pseudo-scenes by capture time) and bursts, as indices into
/// `all` (photos + the anchor last, when given). Deterministic order.
fn groups(all: &[&PhotoInput]) -> (Vec<Vec<usize>>, Vec<Vec<usize>>) {
    let mut scenes: BTreeMap<SceneId, Vec<usize>> = BTreeMap::new();
    let mut bursts: BTreeMap<BurstGroupId, Vec<usize>> = BTreeMap::new();
    let mut loose: Vec<usize> = Vec::new();
    for (i, p) in all.iter().enumerate() {
        match p.scene_id {
            Some(s) => scenes.entry(s).or_default().push(i),
            None if p.captured_at_ms.is_some() => loose.push(i),
            None => {}
        }
        if let Some(b) = p.burst_group_id {
            bursts.entry(b).or_default().push(i);
        }
    }
    let mut scene_groups: Vec<Vec<usize>> = scenes.into_values().collect();
    loose.sort_by_key(|&i| (all[i].captured_at_ms, all[i].image_id));
    let mut chain: Vec<usize> = Vec::new();
    for i in loose {
        if let Some(&prev) = chain.last() {
            let gap = all[i].captured_at_ms.unwrap_or(0) - all[prev].captured_at_ms.unwrap_or(0);
            if gap > PSEUDO_SCENE_GAP_MS {
                scene_groups.push(std::mem::take(&mut chain));
            }
        }
        chain.push(i);
    }
    if !chain.is_empty() {
        scene_groups.push(chain);
    }
    (scene_groups, bursts.into_values().collect())
}

/// Smooths one group (`members` of one population) in place; at scene level (`flag_mixed`)
/// photos outside the main lighting cluster get `mixed_light`. Returns the centre light of the
/// main clusters (for Auto failures), `None` without members.
fn smooth_group(
    all: &[&PhotoInput],
    members: &[(usize, Member)],
    anchor_light: Option<&LightValues>,
    lights: &mut [PhotoLight],
    p: &Pull,
    flag_mixed: bool,
) -> Option<GroupCentre> {
    if members.is_empty() {
        return None;
    }
    let light_of = |m: &Member, lights: &[PhotoLight]| match m.light {
        Some(i) => lights[i].light,
        None => *anchor_light.expect("anchor member has a light"),
    };
    let vals: Vec<LightValues> = members.iter().map(|(_, m)| light_of(m, lights)).collect();
    let fixed: Option<usize> = members.iter().position(|(_, m)| m.light.is_none());
    // Exposure, scene-referred when the camera exposure is known for every member.
    let evs: Option<Vec<f32>> = members.iter().map(|(i, _)| all[*i].exposure_ev).collect();
    let ev_median = evs.as_deref().map(median);
    let rel: Vec<f32> = match (&evs, ev_median) {
        (Some(e), Some(m)) => e.iter().map(|v| v - m).collect(),
        _ => vec![0.0; members.len()],
    };
    let (exp_keep, exp_dev) =
        if evs.is_some() { (p.keep, p.exposure) } else { (p.exposure_keep_no_exif, p.exposure_no_exif) };
    let mut out = vals.clone();
    let main_of = |cl: &[Vec<usize>]| -> usize {
        match fixed {
            Some(f) => cl.iter().position(|c| c.contains(&f)).unwrap_or(0),
            None => {
                let mut best = 0;
                for (k, c) in cl.iter().enumerate() {
                    if c.len() > cl[best].len() {
                        best = k;
                    }
                }
                best
            }
        }
    };
    let centre_of = |c: &[usize], v: &[f32]| fixed.filter(|f| c.contains(f)).map(|f| v[f]);
    let movable = |c: &[usize]| c.iter().copied().filter(|&k| Some(k) != fixed).collect::<Vec<_>>();
    let mut centre = vals[fixed.unwrap_or(0)];

    let mut c: Vec<f32> = vals.iter().zip(&rel).map(|(v, r)| v.exposure + r).collect();
    let cl = clusters(&c, EXPOSURE_GAP);
    let main = main_of(&cl);
    for (k, group) in cl.iter().enumerate() {
        let fc = centre_of(group, &c);
        let cc = pull(&mut c, &movable(group), fc, exp_keep, exp_dev);
        if k == main {
            centre.exposure = cc;
        }
    }
    for (o, (cv, r)) in out.iter_mut().zip(c.iter().zip(&rel)) {
        o.exposure = cv - r;
    }

    // White balance per lighting cluster (mireds), tint within the same clusters. Photos whose
    // Auto WB failed take no part and receive the main cluster's centre.
    let mut mireds: Vec<f32> = vals.iter().map(|v| mired(v.temperature_k)).collect();
    let mut tints: Vec<f32> = vals.iter().map(|v| v.tint).collect();
    let wb_ok = |g: usize| members[g].1.light.is_none_or(|li| !lights[li].wb_fallback);
    let wb_members: Vec<usize> = (0..members.len()).filter(|&g| wb_ok(g)).collect();
    let sub: Vec<f32> = wb_members.iter().map(|&g| mireds[g]).collect();
    let cl: Vec<Vec<usize>> =
        clusters(&sub, MIRED_GAP).into_iter().map(|c| c.into_iter().map(|j| wb_members[j]).collect()).collect();
    let main = main_of(&cl);
    let mut main_k = centre.temperature_k;
    for (k, group) in cl.iter().enumerate() {
        let mv = movable(group);
        let (fm, ft) = (centre_of(group, &mireds), centre_of(group, &tints));
        let cm = pull(&mut mireds, &mv, fm, p.keep, p.mired);
        let ct = pull(&mut tints, &mv, ft, p.keep, p.tint);
        if k == main {
            centre.temperature_k = 1e6 / cm.max(1.0);
            centre.tint = ct;
            main_k = centre.temperature_k;
        }
    }
    let has_wb = !cl.is_empty();
    let wb_flags: Vec<bool> = (0..members.len()).map(wb_ok).collect();
    for (g, (o, (m, t))) in out.iter_mut().zip(mireds.iter().zip(&tints)).enumerate() {
        if wb_flags[g] || !has_wb {
            o.temperature_k = 1e6 / m.max(1.0);
            o.tint = *t;
        } else {
            o.temperature_k = centre.temperature_k;
            o.tint = centre.tint;
        }
    }
    if has_wb {
        for (g, ok) in wb_flags.iter().enumerate() {
            if let (false, Some(li)) = (*ok, members[g].1.light) {
                lights[li].filled = true;
            }
        }
    }
    if flag_mixed && cl.len() > 1 {
        for (k, group) in cl.iter().enumerate() {
            if k == main {
                continue;
            }
            let own_k = 1e6 / median(&group.iter().map(|&g| mireds[g]).collect::<Vec<_>>()).max(1.0);
            for &g in group {
                if let Some(li) = members[g].1.light {
                    lights[li].reasons.push(BaselineReason {
                        kind: BaselineReasonKind::MixedLight,
                        text: format!(
                            "This scene mixes lighting (about {:.0} K and {:.0} K): balanced for its own light, check it matches its neighbours",
                            (main_k / 100.0).round() * 100.0,
                            (own_k / 100.0).round() * 100.0
                        ),
                    });
                }
            }
        }
    }

    // Tone sliders: one cluster each.
    let everyone: Vec<usize> = (0..members.len()).collect();
    let mv = movable(&everyone);
    type Slot = fn(&mut LightValues) -> &mut f32;
    let slots: [Slot; 5] =
        [|v| &mut v.contrast, |v| &mut v.highlights, |v| &mut v.shadows, |v| &mut v.whites, |v| &mut v.blacks];
    for slot in slots {
        let mut s: Vec<f32> = vals.iter().map(|v| *slot(&mut v.clone())).collect();
        let fs = fixed.map(|f| s[f]);
        let cs = pull(&mut s, &mv, fs, p.keep, p.slider);
        *slot(&mut centre) = cs;
        for (o, v) in out.iter_mut().zip(s) {
            *slot(o) = v;
        }
    }

    for ((_, m), v) in members.iter().zip(out) {
        if let Some(i) = m.light {
            lights[i].light = v;
        }
    }
    Some(GroupCentre { light: centre, ev_median })
}

/// The main-cluster centre of a smoothed group; `light.exposure` is scene-referred (add the
/// photo's camera exposure relative to `ev_median` back) when `ev_median` is known.
struct GroupCentre {
    light: LightValues,
    ev_median: Option<f32>,
}

/// Consistent light across scenes and bursts (see the module docs, stage 4): adjusts
/// `lights[i]` (same order as `photos`) in place and adds `mixed_light` reasons. `anchor`:
/// the anchor's input and light, the fixed centre of its own scene / burst.
pub fn smooth(photos: &[&PhotoInput], lights: &mut [PhotoLight], anchor: Option<(&PhotoInput, &LightValues)>) {
    let mut all: Vec<&PhotoInput> = photos.to_vec();
    let anchor_light = anchor.map(|(a, l)| {
        all.push(a);
        l
    });
    let n = photos.len();
    let (scenes, bursts) = groups(&all);
    for (level, list, pull_params) in [(0, &scenes, &SCENE_PULL), (1, &bursts, &BURST_PULL)] {
        for g in list {
            let member = |i: usize| Member { light: (i < n).then_some(i) };
            let normal: Vec<(usize, Member)> = g
                .iter()
                .filter(|&&i| i >= n || (!lights[i].kept_dark && lights[i].auto.is_some()))
                .map(|&i| (i, member(i)))
                .collect();
            let dark: Vec<(usize, Member)> =
                g.iter().filter(|&&i| i < n && lights[i].kept_dark).map(|&i| (i, member(i))).collect();
            let centre = smooth_group(&all, &normal, anchor_light, lights, pull_params, level == 0);
            smooth_group(&all, &dark, None, lights, pull_params, false);
            if let Some(c) = centre {
                let failed: Vec<usize> = g.iter().copied().filter(|&i| i < n && lights[i].auto.is_none()).collect();
                for i in failed {
                    let mut l = c.light;
                    if let (Some(m), Some(ev)) = (c.ev_median, all[i].exposure_ev) {
                        l.exposure -= ev - m;
                    }
                    lights[i].light = l;
                    lights[i].filled = true;
                }
            }
        }
    }
}

/// Sane bounds of a baseline's light values (narrower than the sliders: a baseline never needs
/// more, and a value beyond them means the measurement went wrong).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sane {
    pub exposure: (f32, f32),
    pub slider: (f32, f32),
    pub temperature_k: (f32, f32),
    pub tint: (f32, f32),
}

pub const SANE: Sane =
    Sane { exposure: (-4.0, 4.0), slider: (-100.0, 100.0), temperature_k: (2000.0, 15000.0), tint: (-60.0, 60.0) };

/// Clamps to [`SANE`] and rounds like Lightroom; `true` when something was clamped.
pub fn clamp_light(v: &LightValues) -> (LightValues, bool) {
    let mut clamped = false;
    let mut c = |x: f32, (lo, hi): (f32, f32)| {
        let x = if x.is_finite() { x } else { 0.0 };
        if x < lo || x > hi {
            clamped = true;
        }
        x.clamp(lo, hi)
    };
    let s = SANE;
    let zero = |x: f32| if x == 0.0 { 0.0 } else { x };
    let out = LightValues {
        exposure: zero((c(v.exposure, s.exposure) * 20.0).round() / 20.0),
        contrast: zero(c(v.contrast, s.slider).round()),
        highlights: zero(c(v.highlights, s.slider).round()),
        shadows: zero(c(v.shadows, s.slider).round()),
        whites: zero(c(v.whites, s.slider).round()),
        blacks: zero(c(v.blacks, s.slider).round()),
        temperature_k: c(v.temperature_k, s.temperature_k).round(),
        tint: zero(c(v.tint, s.tint).round()),
    };
    (out, clamped)
}

/// Indices of up to `n` photos spread across scenes in capture order: scenes (photos without a
/// scene form one group) share `n` round-robin; within a scene the picks are evenly spaced over
/// one frame per burst first, then the other frames. Sorted, deterministic.
pub fn pick_samples(photos: &[PhotoInput], n: usize) -> Vec<usize> {
    let mut groups: Vec<(Option<SceneId>, Vec<usize>)> = Vec::new();
    for (i, p) in photos.iter().enumerate() {
        match groups.iter_mut().find(|(k, _)| *k == p.scene_id) {
            Some((_, v)) => v.push(i),
            None => groups.push((p.scene_id, vec![i])),
        }
    }
    let mut quota = vec![0usize; groups.len()];
    let mut left = n.min(photos.len());
    while left > 0 {
        for (q, (_, g)) in quota.iter_mut().zip(&groups) {
            if left > 0 && *q < g.len() {
                *q += 1;
                left -= 1;
            }
        }
    }
    let spread = |list: &[usize], q: usize| -> Vec<usize> {
        (0..q).map(|j| list[((2 * j + 1) * list.len()) / (2 * q)]).collect()
    };
    let mut out = Vec::new();
    for ((_, g), q) in groups.iter().zip(quota) {
        let (mut firsts, mut rest) = (Vec::new(), Vec::new());
        for &i in g {
            let b = photos[i].burst_group_id;
            if b.is_none() || !firsts.iter().any(|&j: &usize| photos[j].burst_group_id == b) {
                firsts.push(i);
            } else {
                rest.push(i);
            }
        }
        if q <= firsts.len() {
            out.extend(spread(&firsts, q));
        } else {
            out.extend(firsts.iter().copied());
            out.extend(spread(&rest, q - firsts.len()));
        }
    }
    out.sort_unstable();
    out
}

/// Measurement workers: the available cores, at most 6 (each holds one decoded source;
/// `SIEVE_BASELINE_THREADS` overrides).
pub fn measure_threads() -> usize {
    std::env::var("SIEVE_BASELINE_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()).min(6))
}

/// [`photo_light`] for every photo of `photos` in parallel (results in input order). `None` when
/// cancelled. `progress(done, total)` on the calling thread after each photo.
pub fn measure_all(
    meter: &dyn LightMeter,
    photos: &[&PhotoInput],
    look: &ParametricAdjustments,
    anchor: &BaselineAnchor,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u32, u32),
) -> Option<Vec<PhotoLight>> {
    let total = photos.len() as u32;
    if total == 0 {
        return Some(Vec::new());
    }
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(measure_threads())
        .thread_name(|i| format!("baseline-measure-{i}"))
        // Concurrent LibRaw decodes with a full OpenMP team each stall (see `ml::style`).
        .start_handler(|_| crate::ml::style::single_threaded_openmp())
        .build()
        .ok();
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let results = std::thread::scope(|s| {
        let pool = &pool;
        let worker = s.spawn(move || {
            let work = move || {
                photos
                    .par_iter()
                    .map_with(tx, |tx, p| {
                        if cancel.load(Ordering::SeqCst) {
                            return None;
                        }
                        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            photo_light(meter, p, look, anchor)
                        }))
                        .unwrap_or_else(|_| auto_failed(anchor, "internal error"));
                        let _ = tx.send(());
                        Some(r)
                    })
                    .collect::<Vec<Option<PhotoLight>>>()
            };
            match pool {
                Some(p) => p.install(work),
                None => work(),
            }
        });
        let mut done = 0;
        for () in rx.iter() {
            done += 1;
            progress(done, total);
        }
        worker.join().unwrap_or_else(|e| std::panic::resume_unwind(e))
    });
    if cancel.load(Ordering::SeqCst) {
        return None;
    }
    results.into_iter().collect()
}

/// Plans every photo of `photos` (scope order). `None` when cancelled. `progress(done,
/// total)` after each measured photo. `anchor_input`: the anchor photo (its scene / burst use
/// the anchor's light as their centre).
#[allow(clippy::too_many_arguments)]
pub fn compute(
    meter: &dyn LightMeter,
    look: &ParametricAdjustments,
    anchor: &BaselineAnchor,
    anchor_input: Option<&PhotoInput>,
    photos: &[PhotoInput],
    settings: &BaselineSettings,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u32, u32),
) -> Option<Vec<BaselineDraft>> {
    let result = |p: &PhotoInput, outcome: BaselineOutcome, reasons: Vec<BaselineReason>| BaselinePhotoResult {
        image_id: p.image_id,
        outcome,
        reasons,
        scene_id: p.scene_id,
        burst_group_id: p.burst_group_id,
        auto: None,
        light: None,
    };
    let mut drafts: Vec<Option<BaselineDraft>> = vec![None; photos.len()];
    let mut to_measure: Vec<usize> = Vec::new();
    for (i, p) in photos.iter().enumerate() {
        let skip = if p.image_id == settings.anchor_id {
            Some(result(p, BaselineOutcome::Anchor, Vec::new()))
        } else if p.state == PhotoEditState::Edited && !settings.replace_edited {
            Some(result(p, BaselineOutcome::SkippedEdited, Vec::new()))
        } else if p.missing {
            Some(result(
                p,
                BaselineOutcome::Failed,
                vec![BaselineReason { kind: BaselineReasonKind::Unreadable, text: "The original is missing".into() }],
            ))
        } else {
            None
        };
        match skip {
            Some(r) => drafts[i] = Some(BaselineDraft { result: r, adjustments: None }),
            None => to_measure.push(i),
        }
    }
    let measured: Vec<&PhotoInput> = to_measure.iter().map(|&i| &photos[i]).collect();
    let mut lights = measure_all(meter, &measured, look, anchor, cancel, progress)?;
    let anchor_ref = anchor_input.filter(|a| a.image_id == anchor.image_id).map(|a| (a, &anchor.light));
    smooth(&measured, &mut lights, anchor_ref);
    for (&i, pl) in to_measure.iter().zip(lights) {
        let p = &photos[i];
        let (light, clamped) = clamp_light(&pl.light);
        let mut reasons = pl.reasons;
        if pl.wb_fallback && pl.auto.is_some() && !pl.filled {
            reasons.push(BaselineReason {
                kind: BaselineReasonKind::AutoFailed,
                text: "Auto white balance found too few neutral tones and no neighbour to borrow from; used the camera's white balance plus the anchor's offset".into(),
            });
        }
        if clamped {
            reasons.push(BaselineReason {
                kind: BaselineReasonKind::Clamped,
                text: "A light value hit the baseline's limits".into(),
            });
        }
        let flagged = reasons.iter().any(|r| r.kind != BaselineReasonKind::Clamped);
        let adjustments = compose(&p.current, look, &light);
        let mut r = result(p, if flagged { BaselineOutcome::Flagged } else { BaselineOutcome::Applied }, reasons);
        r.auto = pl.auto;
        r.light = Some(light);
        drafts[i] = Some(BaselineDraft { result: r, adjustments: Some(adjustments) });
    }
    Some(drafts.into_iter().flatten().collect())
}

// ---------------------------------------------------------------------------
// Preview + run
// ---------------------------------------------------------------------------

/// Everything a preview / run needs, read from the catalog.
pub struct Prepared {
    pub settings: BaselineSettings,
    pub look: ParametricAdjustments,
    pub anchor: PhotoInput,
    pub photos: Vec<PhotoInput>,
    pub counts: BaselinePlanCounts,
}

/// Resolves `settings` and reads the scope, the anchor and the look. Errors as
/// `db::baseline::resolve_settings`.
pub fn prepare(conn: &Connection, project_id: ProjectId, settings: &BaselineSettings) -> AppResult<Prepared> {
    let settings = store::resolve_settings(conn, project_id, settings)?;
    let look = store::look_source(conn, &settings)?;
    let scope = store::scope_photos(conn, project_id, &settings)?;
    let counts = store::plan_counts(&scope, &settings);
    let anchor_entry = crate::db::repo::get_images(conn, &[settings.anchor_id])?.remove(0);
    let anchor_state = store::photo_states(conn, &[settings.anchor_id])?;
    let anchor_row = ScopePhoto {
        entry: anchor_entry,
        state: anchor_state.get(&settings.anchor_id).copied().unwrap_or(PhotoEditState::Edited),
    };
    let anchor = photo_inputs(conn, std::slice::from_ref(&anchor_row))?.remove(0);
    let photos = photo_inputs(conn, &scope)?;
    Ok(Prepared { settings, look, anchor, photos, counts })
}

/// `preview_baseline` on prepared inputs: the anchor is measured, then the samples (spread
/// across scenes, or `options.imageIds`) are planned. Nothing is written. `imageIds` outside
/// the scope -> `invalid_argument`.
pub fn preview(
    meter: &dyn LightMeter,
    project_id: ProjectId,
    prepared: &Prepared,
    options: &BaselinePreviewOptions,
) -> AppResult<BaselinePreview> {
    let n = options.sample_count;
    if n == 0 || n > MAX_BASELINE_SAMPLES {
        return Err(AppError::invalid(format!("sampleCount must be 1..={MAX_BASELINE_SAMPLES}")));
    }
    let idx: Vec<usize> = match &options.image_ids {
        Some(ids) => {
            if ids.is_empty() || ids.len() > MAX_BASELINE_SAMPLES as usize {
                return Err(AppError::invalid(format!("imageIds must have 1..={MAX_BASELINE_SAMPLES} photos")));
            }
            ids.iter()
                .map(|id| {
                    prepared
                        .photos
                        .iter()
                        .position(|p| p.image_id == *id)
                        .ok_or_else(|| AppError::invalid(format!("image {id} is not in the baseline's scope")))
                })
                .collect::<AppResult<_>>()?
        }
        None => {
            // Prefer photos the run would write; fill with the rest.
            let writable: Vec<PhotoInput> = prepared
                .photos
                .iter()
                .filter(|p| p.image_id != prepared.settings.anchor_id)
                .filter(|p| p.state != PhotoEditState::Edited || prepared.settings.replace_edited)
                .cloned()
                .collect();
            pick_samples(&writable, n as usize)
                .into_iter()
                .filter_map(|i| prepared.photos.iter().position(|p| p.image_id == writable[i].image_id))
                .collect()
        }
    };
    let anchor = measure_anchor(meter, &prepared.anchor, &prepared.look)?;
    let sample: Vec<PhotoInput> = idx.iter().map(|&i| prepared.photos[i].clone()).collect();
    let never = AtomicBool::new(false);
    let drafts = compute(
        meter,
        &prepared.look,
        &anchor,
        Some(&prepared.anchor),
        &sample,
        &prepared.settings,
        &never,
        &mut |_, _| {},
    )
    .unwrap_or_default();
    let samples = sample
        .iter()
        .zip(drafts)
        .map(|(p, d)| BaselineSample {
            after: d.adjustments.clone().unwrap_or_else(|| p.current.clone()),
            before: p.current.clone(),
            photo: d.result,
        })
        .collect();
    Ok(BaselinePreview {
        project_id,
        settings: prepared.settings.clone(),
        anchor,
        samples,
        counts: prepared.counts,
        engine_version: ENGINE_VERSION.to_owned(),
    })
}

/// One accepted `run_baseline` (the run row is already `running`).
#[derive(Debug, Clone, PartialEq)]
pub struct BaselineJob {
    pub run_id: BaselineRunId,
    pub project_id: ProjectId,
    /// Resolved.
    pub settings: BaselineSettings,
}

/// What [`run_pipeline`] produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PipelineOutcome {
    /// Stopped by `cancel` before anything was written.
    pub cancelled: bool,
    pub message: Option<String>,
    /// Photos written (for the XMP notify).
    pub changed: usize,
}

/// User-facing summary of a finished run.
pub fn summary(counts: &BaselineCounts) -> String {
    let written = counts.applied + counts.flagged;
    let mut m = format!("Edited {written} photo{}", if written == 1 { "" } else { "s" });
    if counts.flagged > 0 {
        m.push_str(&format!("; {} need a look", counts.flagged));
    }
    if counts.skipped_edited > 0 {
        m.push_str(&format!("; {} already edited were skipped", counts.skipped_edited));
    }
    if counts.failed > 0 {
        m.push_str(&format!("; {} could not be read", counts.failed));
    }
    m
}

/// The whole run on the worker's connection: prepare, measure the anchor (stored on the run),
/// plan every photo ([`compute`]), then write ([`store::store_results`]: one batch + results +
/// provenance). Nothing is written when cancelled before the last step.
pub fn run_pipeline(
    conn: &mut Connection,
    meter: &dyn LightMeter,
    job: &BaselineJob,
    cancel: &AtomicBool,
    report: &mut dyn FnMut(&str, u32, Option<u32>),
) -> AppResult<PipelineOutcome> {
    report("Preparing the baseline", 0, None);
    let prepared = prepare(conn, job.project_id, &job.settings)?;
    let anchor = measure_anchor(meter, &prepared.anchor, &prepared.look)?;
    store::set_anchor(conn, job.run_id, &anchor)?;
    let drafts = compute(
        meter,
        &prepared.look,
        &anchor,
        Some(&prepared.anchor),
        &prepared.photos,
        &prepared.settings,
        cancel,
        &mut |d, t| report("Editing photos", d, Some(t)),
    );
    let Some(drafts) = drafts else {
        return Ok(PipelineOutcome { cancelled: true, ..PipelineOutcome::default() });
    };
    if cancel.load(Ordering::SeqCst) {
        return Ok(PipelineOutcome { cancelled: true, ..PipelineOutcome::default() });
    }
    report("Saving", 0, None);
    let (batch, counts) = store::store_results(conn, job.run_id, &drafts)?;
    Ok(PipelineOutcome { cancelled: false, message: Some(summary(&counts)), changed: batch.changed_ids.len() })
}

// ---------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------

/// Resolved at startup by `lib.rs`.
#[derive(Debug, Clone)]
pub struct BaselineConfig {
    /// Catalog file; the worker opens its own connection to it.
    pub catalog_path: PathBuf,
}

/// Managed Tauri state: the baseline worker (one run at a time, any project).
pub struct BaselineEdit {
    config: BaselineConfig,
    running: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

impl BaselineEdit {
    pub fn new(config: BaselineConfig) -> Self {
        Self { config, running: Arc::new(AtomicBool::new(false)), cancel: Arc::new(AtomicBool::new(false)) }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Starts `job` on a background thread and returns. Reports `activityEvent` kind
    /// `baseline_edit` and ends with exactly one `BaselineRunFinished`. A run in progress ->
    /// `invalid_argument`.
    pub fn start(&self, app: &AppHandle, job: BaselineJob) -> AppResult<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Err(AppError::invalid("A baseline edit is already running"));
        }
        self.cancel.store(false, Ordering::SeqCst);
        let (config, running, cancel, app) =
            (self.config.clone(), self.running.clone(), self.cancel.clone(), app.clone());
        let spawned = std::thread::Builder::new().name("baseline-edit".into()).spawn(move || {
            worker(&app, &config, &job, &cancel);
            running.store(false, Ordering::SeqCst);
        });
        if let Err(e) = spawned {
            self.running.store(false, Ordering::SeqCst);
            return Err(AppError::internal(e.to_string()));
        }
        Ok(())
    }

    /// Asks the running job to stop (nothing is written). No-op when idle.
    pub fn cancel(&self) {
        if self.is_running() {
            self.cancel.store(true, Ordering::SeqCst);
        }
    }
}

fn worker(app: &AppHandle, config: &BaselineConfig, job: &BaselineJob, cancel: &AtomicBool) {
    let channel = format!("baseline-{}", job.run_id);
    let hub = activities(app);
    let mut report = |label: &str, done: u32, total: Option<u32>| {
        if let Some(h) = &hub {
            h.progress(&channel, ActivityKind::BaselineEdit, label, done, total);
        }
    };
    report("Baseline edit", 0, None);
    let result = match app.try_state::<DevelopCache>() {
        Some(cache) => {
            let meter = DevelopMeter { cache: cache.inner().clone() };
            crate::db::open(&config.catalog_path)
                .and_then(|mut conn| run_pipeline(&mut conn, &meter, job, cancel, &mut report))
        }
        None => Err(AppError::internal("develop cache unavailable")),
    };
    let (state, message) = match &result {
        Ok(o) if o.cancelled => (BaselineRunState::Cancelled, Some("Stopped; nothing was changed".to_owned())),
        Ok(o) => (BaselineRunState::Finished, o.message.clone()),
        Err(e) => (BaselineRunState::Failed, Some(e.message.clone())),
    };
    let run = crate::db::open(&config.catalog_path).and_then(|conn| {
        store::finish_run(&conn, job.run_id, state, message.as_deref())?;
        store::run_by_id(&conn, job.run_id, true)
    });
    if matches!(&result, Ok(o) if o.changed > 0) {
        if let Some(xmp) = app.try_state::<crate::xmp::XmpSync>() {
            xmp.notify(app);
        }
    }
    if let Some(h) = &hub {
        let activity_state = match state {
            BaselineRunState::Finished => ActivityState::Finished,
            BaselineRunState::Cancelled => ActivityState::Cancelled,
            _ => ActivityState::Error,
        };
        h.finish(&channel, activity_state, message.clone());
    }
    match run {
        Ok(run) => {
            let _ = BaselineRunFinished { run }.emit(app);
        }
        Err(e) => eprintln!("baseline edit: could not report the run: {}", e.message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    /// Auto = exposure by image id, 5000 K; image 4 fails.
    struct FakeMeter;
    impl LightMeter for FakeMeter {
        fn auto_light(&self, photo: &PhotoInput, _look: &ParametricAdjustments) -> AppResult<LightValues> {
            if photo.image_id == 4 {
                return Err(AppError::invalid("no"));
            }
            Ok(LightValues {
                exposure: photo.image_id as f32 * 0.1,
                temperature_k: 5000.0,
                tint: 3.0,
                ..Default::default()
            })
        }
        fn as_shot(&self, _photo: &PhotoInput) -> AppResult<WhiteBalanceValues> {
            Ok(WhiteBalanceValues { temperature_k: 5200.0, tint: 1.0 })
        }
    }

    fn seed() -> Connection {
        let conn = open_in_memory();
        conn.execute_batch(
            "INSERT INTO projects (id, name, shoot_type, created_at) VALUES (1, 'p', 'wedding', 0);
             INSERT INTO folders (id, path, added_at, project_id) VALUES (1, '/f', 0, 1);
             INSERT INTO images (id, folder_id, path, file_name, format, camera_make, sensor_layout,
                                 file_size, file_mtime_ms, imported_at, captured_at_ms)
             VALUES (1, 1, '/f/a.arw', 'a.arw', 'arw', 'sony', 'bayer', 1, 0, 0, 0),
                    (2, 1, '/f/b.arw', 'b.arw', 'arw', 'sony', 'bayer', 1, 0, 0, 200000),
                    (3, 1, '/f/c.arw', 'c.arw', 'arw', 'sony', 'bayer', 1, 0, 0, 400000),
                    (4, 1, '/f/d.arw', 'd.arw', 'arw', 'sony', 'bayer', 1, 0, 0, 600000),
                    (5, 1, '/f/e.arw', 'e.arw', 'arw', 'sony', 'bayer', 1, 0, 0, 800000);",
        )
        .unwrap();
        conn
    }

    /// The anchor (1): a look (HSL, grain, profile amount), its own light, a crop.
    fn edit_anchor(conn: &mut Connection) -> ParametricAdjustments {
        let mut a = ParametricAdjustments::default();
        a.hsl.saturation.orange = -20.0;
        a.effects.grain.amount = 25.0;
        a.vibrance = 15.0;
        a.exposure = 0.8;
        a.white_balance = WhiteBalance::Custom { temperature_k: 5600.0, tint: 6.0 };
        a.crop.enabled = true;
        a.crop.top = 0.1;
        crate::develop::history::commit(conn, 1, &a, "Exposure").unwrap();
        a
    }

    #[test]
    fn copies_look_sets_auto_plus_offset_keeps_geometry_and_skips_edited() {
        let mut conn = seed();
        let anchor = edit_anchor(&mut conn);
        // Photo 5: the user's edit (skipped).
        let own = ParametricAdjustments { clarity: 40.0, ..Default::default() };
        crate::develop::history::commit(&mut conn, 5, &own, "Clarity").unwrap();

        let settings =
            BaselineSettings { anchor_id: 1, preset_id: None, scope: BaselineScope::All, replace_edited: false };
        let run_id = store::begin_run(&conn, 1, &settings, ENGINE_VERSION).unwrap();
        let job = BaselineJob { run_id, project_id: 1, settings: settings.clone() };
        let never = AtomicBool::new(false);
        let out = run_pipeline(&mut conn, &FakeMeter, &job, &never, &mut |_, _, _| {}).unwrap();
        assert!(!out.cancelled);
        store::finish_run(&conn, run_id, BaselineRunState::Finished, out.message.as_deref()).unwrap();

        let results = store::results(&conn, 1, None).unwrap();
        let outcome = |id: ImageId| results.iter().find(|r| r.image_id == id).unwrap().outcome;
        assert_eq!(outcome(1), BaselineOutcome::Anchor);
        assert_eq!(outcome(2), BaselineOutcome::Applied);
        assert_eq!(outcome(4), BaselineOutcome::Flagged, "auto failed -> anchor light, flagged");
        assert_eq!(outcome(5), BaselineOutcome::SkippedEdited);

        let p2 = crate::db::repo::get_adjustments(&conn, 2).unwrap();
        // Look identical to the anchor's.
        let mut look2 = ParametricAdjustments::default();
        look2.copy_fields(&p2, &fields_of_class(SettingClass::Look));
        let mut look1 = ParametricAdjustments::default();
        look1.copy_fields(&anchor, &fields_of_class(SettingClass::Look));
        assert_eq!(look1, look2);
        // Light = Auto + the anchor's offset (photos minutes apart: no smoothing group); the
        // offset is in mireds, so 5000 K Auto + (5600 K anchor vs its 5000 K Auto) = 5600 K.
        assert!((p2.exposure - (0.2 + 0.7)).abs() < 1e-6, "{}", p2.exposure);
        assert_eq!(p2.white_balance, WhiteBalance::Custom { temperature_k: 5600.0, tint: 6.0 });
        assert!(!p2.crop.enabled);
        let p4 = crate::db::repo::get_adjustments(&conn, 4).unwrap();
        assert!((p4.exposure - 0.8).abs() < 1e-6, "anchor light on auto failure");
        assert_eq!(crate::db::repo::get_adjustments(&conn, 5).unwrap(), own);
        assert_eq!(crate::db::repo::get_adjustments(&conn, 1).unwrap(), anchor, "anchor untouched");
        // The anchor's offset was recorded.
        let run = store::get_run(&conn, 1, false).unwrap().unwrap();
        let a = run.anchor.unwrap();
        assert!((a.offset.exposure - (0.8 - 0.1)).abs() < 1e-5);
        assert!(a.offset.temperature_mired < 0.0);
        assert_eq!(run.counts.applied + run.counts.flagged, 3);
    }

    #[test]
    fn cancelled_run_writes_nothing_and_preview_writes_nothing() {
        let mut conn = seed();
        edit_anchor(&mut conn);
        let settings =
            BaselineSettings { anchor_id: 1, preset_id: None, scope: BaselineScope::All, replace_edited: false };
        let run_id = store::begin_run(&conn, 1, &settings, ENGINE_VERSION).unwrap();
        let job = BaselineJob { run_id, project_id: 1, settings: settings.clone() };
        let cancel = AtomicBool::new(true);
        assert!(run_pipeline(&mut conn, &FakeMeter, &job, &cancel, &mut |_, _, _| {}).unwrap().cancelled);
        assert!(crate::db::repo::get_adjustments(&conn, 2).unwrap().is_neutral());

        let prepared = prepare(&conn, 1, &settings).unwrap();
        let pv =
            preview(&FakeMeter, 1, &prepared, &BaselinePreviewOptions { sample_count: 2, image_ids: None }).unwrap();
        assert_eq!(pv.samples.len(), 2);
        assert!(pv.samples.iter().all(|s| s.photo.image_id != 1));
        assert_ne!(pv.samples[0].before, pv.samples[0].after);
        assert!(crate::db::repo::get_adjustments(&conn, 2).unwrap().is_neutral());
        let pinned = BaselinePreviewOptions { sample_count: 12, image_ids: Some(vec![3]) };
        assert_eq!(preview(&FakeMeter, 1, &prepared, &pinned).unwrap().samples[0].photo.image_id, 3);
        let bad = BaselinePreviewOptions { sample_count: 0, image_ids: None };
        assert!(preview(&FakeMeter, 1, &prepared, &bad).is_err());
    }

    #[test]
    fn compose_keeps_the_photos_geometry_and_masks() {
        let mut current = ParametricAdjustments::default();
        current.crop.enabled = true;
        current.crop.left = 0.2;
        current.transform.rotate = 2.0;
        current.clarity = 50.0;
        let mut look = ParametricAdjustments::default();
        look.crop.enabled = true;
        look.crop.top = 0.3;
        look.clarity = 10.0;
        look.exposure = 3.0;
        let light = LightValues { exposure: 0.25, temperature_k: 4800.0, ..Default::default() };
        let out = compose(&current, &look, &light);
        assert_eq!(out.crop, current.crop);
        assert_eq!(out.transform, current.transform);
        assert_eq!(out.clarity, 10.0, "look copied");
        assert_eq!(out.exposure, 0.25, "light from the engine, not the look");
    }

    #[test]
    fn samples_spread_across_scenes() {
        let photo = |id: ImageId, scene: Option<SceneId>, burst: Option<BurstGroupId>| PhotoInput {
            image_id: id,
            src: SourceImage { id, path: PathBuf::new(), orientation: None },
            format: ImageFormat::Arw,
            current: ParametricAdjustments::default(),
            scene_id: scene,
            burst_group_id: burst,
            captured_at_ms: Some(id),
            faces: None,
            state: PhotoEditState::Unedited,
            missing: false,
            exposure_ev: None,
        };
        let photos: Vec<PhotoInput> = (0..30).map(|i| photo(i, Some(i / 10), Some(i / 2))).collect();
        let s = pick_samples(&photos, 6);
        assert_eq!(s.len(), 6);
        for scene in 0..3 {
            assert_eq!(s.iter().filter(|&&i| photos[i].scene_id == Some(scene)).count(), 2, "{s:?}");
        }
        assert_eq!(pick_samples(&photos, 100).len(), 30);
    }

    /// Every `crs:` key Sieve writes or imports is classified by the partition table the same
    /// way as the group the preset importer assigns it to.
    #[test]
    fn partition_agrees_with_the_crs_mapping() {
        use crate::styles::preset_file::field_of;
        use crate::xmp::crs;
        let mut keys: Vec<String> = Vec::new();
        keys.extend(crs::CRS_FIELDS.iter().map(|(n, _)| (*n).to_owned()));
        for (p, _) in crs::CRS_HSL_PREFIXES {
            for (b, _) in crs::CRS_HSL_BANDS {
                keys.push(format!("{p}{b}"));
            }
        }
        keys.extend(crs::PARITY_SCALARS.iter().map(|f| f.name.to_owned()));
        keys.extend(crs::PARITY_BOOLS.iter().map(|(n, _)| (*n).to_owned()));
        keys.extend(crs::CRS_CURVES.iter().map(|(n, _)| (*n).to_owned()));
        keys.extend(crs::TRANSFORM_SCALARS.iter().map(|(n, ..)| (*n).to_owned()));
        keys.extend([crs::VIGNETTE_STYLE, crs::PERSPECTIVE_UPRIGHT, crs::CAMERA_PROFILE, crs::LOOK].map(str::to_owned));
        for k in &keys {
            let class = crs_key_class(k).unwrap_or_else(|| panic!("{k} not in BASELINE_PARTITION"));
            if let Some(f) = field_of(k) {
                assert_eq!(class, setting_class(f), "{k} ({f:?})");
            }
        }
    }

    fn input(id: ImageId, scene: Option<SceneId>, burst: Option<BurstGroupId>, t: i64, ev: Option<f32>) -> PhotoInput {
        PhotoInput {
            image_id: id,
            src: SourceImage { id, path: PathBuf::new(), orientation: None },
            format: ImageFormat::Arw,
            current: ParametricAdjustments::default(),
            scene_id: scene,
            burst_group_id: burst,
            captured_at_ms: Some(t),
            faces: None,
            state: PhotoEditState::Unedited,
            missing: false,
            exposure_ev: ev,
        }
    }

    fn lit(exposure: f32, k: f32) -> PhotoLight {
        PhotoLight {
            auto: Some(LightValues::default()),
            light: LightValues { exposure, temperature_k: k, tint: 5.0, ..Default::default() },
            reasons: Vec::new(),
            kept_dark: false,
            wb_fallback: false,
            filled: false,
        }
    }

    fn spread(v: impl Iterator<Item = f32>) -> f32 {
        let v: Vec<f32> = v.collect();
        v.iter().cloned().fold(f32::MIN, f32::max) - v.iter().cloned().fold(f32::MAX, f32::min)
    }

    #[test]
    fn bursts_get_consistent_light_and_camera_exposure_is_kept() {
        // One burst, no EXIF: Auto's differences are damped.
        let photos: Vec<PhotoInput> = (1..=4).map(|i| input(i, Some(1), Some(7), i * 300, None)).collect();
        let refs: Vec<&PhotoInput> = photos.iter().collect();
        let mut lights = vec![lit(0.0, 5000.0), lit(0.4, 5300.0), lit(-0.3, 4800.0), lit(0.2, 5100.0)];
        let before = spread(lights.iter().map(|l| l.light.exposure));
        smooth(&refs, &mut lights, None);
        let after = spread(lights.iter().map(|l| l.light.exposure));
        assert!(after < 0.4 * before, "{before} -> {after}");
        assert!(spread(lights.iter().map(|l| mired(l.light.temperature_k))) < 12.0);
        assert!(lights.iter().all(|l| l.reasons.is_empty()));

        // The camera exposed frame 2 one stop darker and Auto compensated: scene-referred they
        // agree, so the compensation is kept (the rendered frames match).
        let photos: Vec<PhotoInput> = [(1, 0.0), (2, -1.0), (3, 0.0)]
            .iter()
            .map(|&(i, ev)| input(i, Some(1), Some(7), i * 300, Some(ev)))
            .collect();
        let refs: Vec<&PhotoInput> = photos.iter().collect();
        let mut lights = vec![lit(0.1, 5000.0), lit(1.2, 5000.0), lit(0.2, 5000.0)];
        smooth(&refs, &mut lights, None);
        let rendered: Vec<f32> =
            lights.iter().zip(&photos).map(|(l, p)| l.light.exposure + p.exposure_ev.unwrap()).collect();
        assert!(spread(rendered.into_iter()) < 0.06, "{lights:?}");
        assert!(lights[1].light.exposure > 1.05);
    }

    #[test]
    fn the_anchor_is_the_centre_of_its_burst_and_failures_borrow_the_group_light() {
        // Same camera exposure throughout (EXIF known): the burst pull applies in full.
        let anchor = input(100, Some(1), Some(7), 0, Some(0.0));
        let photos: Vec<PhotoInput> = (1..=3).map(|i| input(i, Some(1), Some(7), i * 300, Some(0.0))).collect();
        let refs: Vec<&PhotoInput> = photos.iter().collect();
        let mut lights = vec![lit(0.5, 5200.0), lit(0.6, 5300.0), lit(0.0, 6500.0)];
        lights[2].auto = None; // Auto failed: carries the anchor's light, takes the group's.
        let a = LightValues { exposure: 0.3, temperature_k: 5000.0, tint: 5.0, ..Default::default() };
        smooth(&refs, &mut lights, Some((&anchor, &a)));
        for l in &lights[..2] {
            assert!((l.light.exposure - 0.3).abs() <= 0.1 + 1e-6, "{l:?}");
            assert!((mired(l.light.temperature_k) - mired(5000.0)).abs() <= BURST_PULL.mired + 1e-3);
        }
        assert!(lights[2].filled);
        assert!((lights[2].light.exposure - 0.3).abs() < 1e-6);
    }

    #[test]
    fn mixed_lighting_keeps_one_white_balance_per_light_and_flags_the_minority() {
        let photos: Vec<PhotoInput> = (1..=6).map(|i| input(i, Some(3), None, i * 20_000, None)).collect();
        let refs: Vec<&PhotoInput> = photos.iter().collect();
        let mut lights = vec![
            lit(0.5, 3000.0),
            lit(0.5, 3100.0),
            lit(0.5, 5600.0),
            lit(0.5, 2950.0),
            lit(0.5, 3050.0),
            lit(0.5, 5400.0),
        ];
        smooth(&refs, &mut lights, None);
        for i in [0, 1, 3, 4] {
            assert!(lights[i].light.temperature_k < 3300.0, "{:?}", lights[i]);
            assert!(lights[i].reasons.is_empty());
        }
        for i in [2, 5] {
            assert!(lights[i].light.temperature_k > 5000.0, "{:?}", lights[i]);
            assert_eq!(lights[i].reasons[0].kind, BaselineReasonKind::MixedLight);
        }
    }

    #[test]
    fn low_key_frames_are_not_smoothed_with_the_rest() {
        let photos: Vec<PhotoInput> = (1..=4).map(|i| input(i, Some(1), Some(7), i * 300, None)).collect();
        let refs: Vec<&PhotoInput> = photos.iter().collect();
        let mut lights = vec![lit(1.0, 5000.0), lit(1.1, 5000.0), lit(0.9, 5000.0), lit(0.0, 5000.0)];
        lights[3].kept_dark = true;
        smooth(&refs, &mut lights, None);
        assert_eq!(lights[3].light.exposure, 0.0);
        assert!(lights[..3].iter().all(|l| l.light.exposure > 0.85));
    }

    #[test]
    fn photos_without_scenes_are_chained_by_capture_time() {
        let all =
            [input(1, None, None, 0, None), input(2, None, None, 60_000, None), input(3, None, None, 400_000, None)];
        let refs: Vec<&PhotoInput> = all.iter().collect();
        let (scenes, bursts) = groups(&refs);
        assert_eq!(scenes, vec![vec![0, 1], vec![2]]);
        assert!(bursts.is_empty());
    }

    fn frame(dark: f32, mid: f32, bright: f32, p995: f32, face: Option<f32>, centre: f32, border: f32) -> FrameStats {
        FrameStats {
            mean_l: dark * 8.0 + mid * 45.0 + bright * 85.0,
            key: 0.0,
            dark,
            mid,
            bright,
            white_p995: p995,
            face_l: face,
            centre_l: centre,
            border_l: border,
        }
    }

    #[test]
    fn low_key_and_silhouettes_are_detected_from_the_histogram_and_faces() {
        let p = input(1, None, None, 0, None);
        let lift = LightValues { exposure: 0.8, ..Default::default() };
        let kind = |f: FrameStats, a: &LightValues| low_key(&p, a, Some(&f)).map(|r| r.kind);
        // Back-lit couple: bright sky around, dark faces.
        let sil = frame(0.5, 0.15, 0.35, 0.95, Some(8.0), 20.0, 50.0);
        assert_eq!(kind(sil, &lift), Some(BaselineReasonKind::Silhouette));
        // Without faces: the surround is brighter than the centre.
        assert_eq!(kind(frame(0.5, 0.15, 0.35, 0.95, None, 20.0, 50.0), &lift), Some(BaselineReasonKind::Silhouette));
        // A bright face is never a silhouette (bride in white, groom in black).
        assert_eq!(kind(frame(0.5, 0.15, 0.35, 0.95, Some(55.0), 20.0, 50.0), &lift), None);
        // Spot-lit portrait on black: low-key.
        let lk = frame(0.8, 0.15, 0.05, 0.9, Some(50.0), 30.0, 5.0);
        assert_eq!(kind(lk, &lift), Some(BaselineReasonKind::LowKey));
        // Underexposed: dark face, or no highlights -> Auto may brighten it.
        assert_eq!(kind(frame(0.8, 0.15, 0.05, 0.9, Some(12.0), 30.0, 5.0), &lift), None);
        assert_eq!(kind(frame(0.8, 0.15, 0.05, 0.4, None, 30.0, 5.0), &lift), None);
        // A dim church with plenty of mid-tones.
        assert_eq!(kind(frame(0.35, 0.55, 0.1, 1.0, Some(40.0), 35.0, 30.0), &lift), None);
        // Auto would not brighten: nothing to protect.
        assert_eq!(kind(sil, &LightValues { exposure: 0.1, ..Default::default() }), None);
        // Unknown frame (synthetic meters): never.
        assert!(low_key(&p, &lift, None).is_none());
        // Kept dark = no Auto lift.
        let w = without_lift(&LightValues { exposure: 0.8, shadows: 40.0, highlights: -30.0, ..Default::default() });
        assert_eq!((w.exposure, w.shadows, w.highlights), (0.0, 0.0, -30.0));
    }

    #[test]
    fn clamps_to_sane_bounds_and_rounds_like_lightroom() {
        let (v, c) =
            clamp_light(&LightValues { exposure: 0.333, temperature_k: 5432.4, tint: 3.4, ..Default::default() });
        assert!(!c);
        assert_eq!((v.exposure, v.temperature_k, v.tint), (0.35, 5432.0, 3.0));
        let (v, c) = clamp_light(&LightValues {
            exposure: 6.0,
            contrast: f32::NAN,
            temperature_k: 30000.0,
            tint: -90.0,
            ..Default::default()
        });
        assert!(c);
        assert_eq!((v.exposure, v.contrast, v.temperature_k, v.tint), (4.0, 0.0, 15000.0, -60.0));
    }

    #[test]
    fn measurement_runs_in_parallel_with_progress_and_cancel() {
        let photos: Vec<PhotoInput> = (2..=40).map(|i| input(i, None, None, i * 1_000_000, None)).collect();
        let refs: Vec<&PhotoInput> = photos.iter().collect();
        let anchor =
            measure_anchor(&FakeMeter, &input(1, None, None, 0, None), &ParametricAdjustments::default()).unwrap();
        let mut seen = Vec::new();
        let never = AtomicBool::new(false);
        let out = measure_all(&FakeMeter, &refs, &ParametricAdjustments::default(), &anchor, &never, &mut |d, t| {
            seen.push((d, t))
        })
        .unwrap();
        assert_eq!(out.len(), 39);
        assert_eq!(seen.last(), Some(&(39, 39)));
        assert!(out[2].auto.is_none(), "image 4 fails");
        assert!((out[0].light.exposure - (0.2 + anchor.offset.exposure)).abs() < 1e-6);
        let cancel = AtomicBool::new(true);
        assert!(measure_all(&FakeMeter, &refs, &ParametricAdjustments::default(), &anchor, &cancel, &mut |_, _| {})
            .is_none());
    }

    /// An `as_shot` anchor uses the camera's as-shot values (Auto WB when unknown).
    #[test]
    fn as_shot_anchor_resolves_the_camera_white_balance() {
        let look = ParametricAdjustments { white_balance: WhiteBalance::AsShot, ..Default::default() };
        let a = measure_anchor(&FakeMeter, &input(1, None, None, 0, None), &look).unwrap();
        assert_eq!((a.light.temperature_k, a.light.tint), (5200.0, 1.0));
        assert!(a.offset.temperature_mired < 0.0, "5200 K as-shot vs 5000 K Auto: warmer");
        struct NoAsShot;
        impl LightMeter for NoAsShot {
            fn auto_light(&self, p: &PhotoInput, l: &ParametricAdjustments) -> AppResult<LightValues> {
                FakeMeter.auto_light(p, l)
            }
            fn as_shot(&self, _p: &PhotoInput) -> AppResult<WhiteBalanceValues> {
                Err(AppError::invalid("unknown"))
            }
        }
        let a = measure_anchor(&NoAsShot, &input(1, None, None, 0, None), &look).unwrap();
        assert_eq!(a.offset.temperature_mired, 0.0);
        assert_eq!(a.offset.tint, 0.0);
    }

    /// Baseline results never train the style model; the user's edits do.
    #[test]
    fn baseline_results_are_not_style_training_examples() {
        let mut conn = seed();
        edit_anchor(&mut conn);
        let settings =
            BaselineSettings { anchor_id: 1, preset_id: None, scope: BaselineScope::All, replace_edited: false };
        let run_id = store::begin_run(&conn, 1, &settings, ENGINE_VERSION).unwrap();
        let job = BaselineJob { run_id, project_id: 1, settings };
        run_pipeline(&mut conn, &FakeMeter, &job, &AtomicBool::new(false), &mut |_, _, _| {}).unwrap();
        let ids = [1, 2, 3, 4, 5];
        assert_eq!(crate::ml::style::trainable(&conn, &ids).unwrap(), vec![1]);
        assert_eq!(
            crate::ml::style::style_sample_frames(&conn).unwrap().iter().map(|f| f.src.id).collect::<Vec<_>>(),
            vec![1]
        );
    }

    /// End to end on rendered synthetic scenes with the real meter (develop cache, Auto): look
    /// keys identical to the anchor's, never keys untouched, deterministic, frame brightness
    /// more consistent than copy-paste, silhouettes / low-key flagged and not lifted.
    #[test]
    fn synthetic_scenes_end_to_end() {
        use super::synth::{self, SceneKind};
        let dir = tempfile::tempdir().unwrap();
        let (w, h) = (192u32, 128u32);
        let shoot = synth::shoot(8, 3);
        let mut photos: Vec<PhotoInput> = shoot
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let path = dir.path().join(format!("f{i}.png"));
                synth::write_png(&path, w, h, &synth::render(f, w, h)).unwrap();
                let id = i as ImageId + 1;
                PhotoInput {
                    image_id: id,
                    src: SourceImage { id, path, orientation: None },
                    format: ImageFormat::Png,
                    current: ParametricAdjustments::defaults_for(ImageFormat::Png),
                    scene_id: Some(f.scene_id),
                    burst_group_id: Some(f.burst_id),
                    captured_at_ms: Some(f.captured_at_ms),
                    faces: Some(f.faces.clone()),
                    state: PhotoEditState::Unedited,
                    missing: false,
                    exposure_ev: Some(f.exposure_ev),
                }
            })
            .collect();
        photos[2].current.crop.enabled = true;
        photos[2].current.crop.left = 0.2;
        let cache = DevelopCache::new(crate::develop::DevelopConfig::default());
        let meter = DevelopMeter { cache: cache.clone() };
        // Anchor: an outdoor frame with a look and the user's light (+0.3 EV, warmer).
        let ai = shoot.iter().position(|f| f.kind == SceneKind::BrightOutdoor).unwrap() + 1;
        let mut look = ParametricAdjustments::defaults_for(ImageFormat::Png);
        look.vibrance = 15.0;
        look.hsl.saturation.orange = -12.0;
        look.effects.grain.amount = 15.0;
        let auto = meter.measure(&photos[ai], &look).unwrap().auto;
        // Develop's Auto (the shared light-only Auto) on the anchor = the meter's Auto.
        let dev =
            crate::develop::auto::auto_light(&cache, &photos[ai].src, &look, photos[ai].faces.as_deref()).unwrap();
        assert_eq!(dev.tone.exposure, Some(auto.exposure));
        assert_eq!(dev.white_balance.map(|w| w.temperature_k), Some(auto.temperature_k));
        assert!(dev.tone.vibrance.is_none() && dev.tone.saturation.is_none());
        let offset = LightOffset { exposure: 0.3, temperature_mired: -15.0, tint: 4.0, ..Default::default() };
        let anchor_settings = clamp_light(&offset.add_to(&auto)).0.apply_to(&look);
        photos[ai].current = anchor_settings.clone();
        photos[ai].state = PhotoEditState::Edited;
        let anchor = measure_anchor(&meter, &photos[ai], &anchor_settings).unwrap();
        assert!((anchor.offset.exposure - 0.3).abs() < 0.03, "{:?}", anchor.offset);
        let settings = BaselineSettings {
            anchor_id: photos[ai].image_id,
            preset_id: None,
            scope: BaselineScope::All,
            replace_edited: false,
        };
        let never = AtomicBool::new(false);
        let run = || {
            compute(&meter, &anchor_settings, &anchor, Some(&photos[ai]), &photos, &settings, &never, &mut |_, _| {})
                .unwrap()
        };
        let drafts = run();
        assert_eq!(drafts, run(), "deterministic");
        let look_of = |a: &ParametricAdjustments| {
            let mut l = ParametricAdjustments::default();
            l.copy_fields(a, &fields_of_class(SettingClass::Look));
            serde_json::to_vec(&l).unwrap()
        };
        let luts = crate::lut::LutLibrary::new(dir.path().join("luts"));
        let frame_l = |p: &PhotoInput, a: &ParametricAdjustments| {
            let r = cache.render_image(&p.src, a, None, w, &luts).unwrap();
            synth::lab_mean(&r.image.rgb, r.image.width, r.image.height, None)[0]
        };
        let mut by_scene: std::collections::BTreeMap<SceneKind, (Vec<f32>, Vec<f32>)> = Default::default();
        for ((p, d), f) in photos.iter().zip(&drafts).zip(&shoot) {
            let Some(adj) = &d.adjustments else {
                assert_eq!(d.result.outcome, BaselineOutcome::Anchor);
                continue;
            };
            assert_eq!(look_of(adj), look_of(&anchor_settings), "look keys of #{}", p.image_id);
            assert_eq!(adj.crop, p.current.crop, "never keys of #{}", p.image_id);
            let kinds: Vec<BaselineReasonKind> = d.result.reasons.iter().map(|r| r.kind).collect();
            match f.kind {
                SceneKind::BacklitSilhouette => assert!(kinds.contains(&BaselineReasonKind::Silhouette), "{kinds:?}"),
                SceneKind::LowKeyPortrait => assert!(kinds.contains(&BaselineReasonKind::LowKey), "{kinds:?}"),
                _ => assert!(
                    !kinds.iter().any(|k| matches!(k, BaselineReasonKind::LowKey | BaselineReasonKind::Silhouette)),
                    "{:?} #{} {kinds:?}",
                    f.kind,
                    p.image_id
                ),
            }
            if matches!(f.kind, SceneKind::BacklitSilhouette | SceneKind::LowKeyPortrait) {
                // Not lifted beyond the anchor's offset (+ the camera's exposure differences).
                assert!(d.result.light.unwrap().exposure <= 0.3 + 0.35, "{:?}", d.result.light);
                continue;
            }
            if f.daylight {
                continue;
            }
            let pasted = compose(&p.current, &anchor_settings, &anchor.light);
            let e = by_scene.entry(f.kind).or_default();
            e.0.push(frame_l(p, &pasted));
            e.1.push(frame_l(p, adj));
        }
        for (kind, (copy, base)) in &by_scene {
            let (c, b) = (spread(copy.iter().copied()), spread(base.iter().copied()));
            assert!(b < 0.6 * c, "{kind:?}: frame L* range copy {c:.2} baseline {b:.2}");
        }
    }
}
