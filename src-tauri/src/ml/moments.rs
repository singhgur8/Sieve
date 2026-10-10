//! Moments and shot types (Phase 9, IPC v20): frames of the same scene + people grouped across
//! the shoot, each classified couple / group / detail / candid / other.
//!
//! Owned by vision-ml-dev. The architect fixed the surface used by `ml::selection`'s pipeline:
//! [`detect_moments`] returning [`MomentPlan`]. The pure seam ([`frame_signals`],
//! [`classify_shot`], [`group_moments`], [`similarity`], [`detail_focus`]) is what the tests and
//! `ml::selection` use.
//!
//! Contract (docs/architecture.md, "Target-count culling"):
//! - A moment = time gap + visual similarity (phash / face layout) + same people; moments
//!   may span bursts and scenes but never projects.
//! - Shot type per frame: group = >= 3 faces posed; couple = the main pair dominant (portrait:
//!   the subject); detail = no face, a sharp salient object; candid = faces not posed; else
//!   other. The moment's type is the dominant one.
//! - Per-frame signals for the selection ("visible face" score: frontal, size, eyes;
//!   back-of-head / no-face; detail focus on the salient object) in [`FrameSignals`].
//!
//! How (all deterministic):
//! - Faces come from the analysis (`image_analysis.metrics_json` + `faces_json`, same order);
//!   people from `ml::identity` through `db::target::project_embeddings` (per-face person id)
//!   and `db::target::people_roles`. Without people (identity unavailable) every rule has a
//!   fallback: couple = one or two dominant, decent-size faces (wedding / portrait shoots).
//! - Similarity = 64-bit pHash distance + face layout (positions / sizes) + people (Jaccard).
//! - Grouping walks the shoot in capture order keeping "open" moments (last frame within
//!   [`MAX_GAP_MS`]); a frame joins the most similar open moment if the similarity clears a
//!   bar that rises with the time gap (bursts link easily, minutes apart only when nearly the
//!   same), so a second camera interleaved in time and returning to a scene both work. Group
//!   setups split when the head count or the people change; faces vs no faces split.
//! - Detail focus: the preview's tile sharpness grid (`ml::metrics::preview_tile_grid`,
//!   the same tiles as the stored `TileStats`) is computed for no-face frames only; in focus
//!   = the sharpest tiles lie in the central area where the object is.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;
use rusqlite::Connection;

use crate::db::target::{self, MomentDraft};
use crate::ipc::error::AppResult;
use crate::ipc::types::{FaceInfo, ImageId, NormRect, PersonId, PersonRole, ProjectId, ShootType, ShotType};
use crate::ml::metrics::{self, TileGrid};
use crate::ml::{scoring, FaceMetrics, ImageMetrics, MODEL_VERSION};

// ---------------------------------------------------------------------------
// Tunables
// ---------------------------------------------------------------------------

/// Face height (share of the frame) where "visible" starts / is complete. 0.10 of the frame
/// height on a 2048 px preview is a ~140 px face: crop-worthy.
pub const VISIBLE_SIZE_LO: f32 = 0.025;
pub const VISIBLE_SIZE_HI: f32 = 0.10;
/// Nose offset along the eye line (`FaceMetrics::yaw`) below which a face looks at the camera.
pub const LOOK_YAW: f32 = 0.25;
/// FaceMesh head yaw (degrees) below which a face looks at the camera.
pub const LOOK_HEAD_YAW_DEG: f32 = 25.0;
/// Eye openness below this (but not a blink) = borderline ("half closed").
pub const BORDERLINE_EYES: f32 = 0.55;
/// Below this openness the eyes count as closed.
pub const CLOSED_EYES: f32 = 0.25;
/// A frame with this many subject faces, posed, is a group.
pub const GROUP_MIN_FACES: u32 = 3;
/// Posed score at or above which a group frame counts as posed.
pub const POSED_MIN: f32 = 0.55;
/// Without people: a dominant face at least this tall (share of the frame) makes a couple
/// frame (wedding / portrait shoots).
pub const COUPLE_FALLBACK_FACE: f32 = 0.08;
/// Detail: the sharpest tiles must reach this (`TileStats.p90` scale).
pub const DETAIL_MIN_SHARP: f32 = 0.45;
/// Detail: shallow depth of field = median tile sharpness at most this share of the p90.
pub const DETAIL_SHALLOW: f32 = 0.72;
/// Detail focus: the central area (share of width / height on each side).
pub const DETAIL_CENTRE_MARGIN: f32 = 0.2;
/// Detail focus: the central area's sharpest tile must reach this share of the frame's.
pub const DETAIL_CENTRE_SHARE: f32 = 0.85;
/// Face expression (laugh / scream / cheer) at or above which a face "reacts".
pub const EXPRESSION_STRONG: f32 = 0.6;

/// Frames this close in time always link (one burst).
pub const BURST_GAP_MS: i64 = 2_000;
/// Up to this gap the normal similarity bar applies.
pub const LINK_GAP_MS: i64 = 30_000;
/// Open moments close after this gap.
pub const MAX_GAP_MS: i64 = 300_000;
/// Similarity bars: within a burst, within [`LINK_GAP_MS`], at [`MAX_GAP_MS`].
pub const LINK_SIM_BURST: f32 = 0.3;
pub const LINK_SIM: f32 = 0.5;
pub const LINK_SIM_FAR: f32 = 0.78;
/// A frame is compared with the last this many frames of an open moment.
const RECENT: usize = 8;
/// Faces moving more than this many face heights between frames <= 3 s apart = action.
const MOTION_FULL: f32 = 0.8;

// ---------------------------------------------------------------------------
// Signals
// ---------------------------------------------------------------------------

/// Judged state of a face's eyes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eyes {
    Open,
    /// Half closed (squint / mid-blink): worth a second look.
    Borderline,
    Closed,
    /// Not judged (too small, profile, downcast gaze).
    Unknown,
}

/// One face as the selection sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceSignal {
    pub bbox: NormRect,
    /// From `ml::identity` (None = unknown / not embedded).
    pub person_id: Option<PersonId>,
    pub role: Option<PersonRole>,
    /// 0..=1: frontal, large enough, eyes open, lit.
    pub visible: f32,
    /// Looking at the camera (frontal, eyes not closed).
    pub looking: bool,
    pub eyes: Eyes,
    /// 0..=1: open mouth (laugh / scream / cheer) or wide smile.
    pub expression: f32,
    /// Big enough relative to the largest face to be a subject (not a background guest).
    pub subject: bool,
}

/// Per-frame signals for the selection.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameSignals {
    pub image_id: ImageId,
    pub moment_key: Option<u32>,
    pub shot_type: ShotType,
    /// People recognised in the frame (sorted, unique).
    pub person_ids: Vec<PersonId>,
    /// 0..=1: a subject's face clearly visible (frontal, large enough, eyes open).
    pub visible_face: f32,
    /// Detail shots: focus is on the salient object (`None` = not a detail / not judged).
    pub detail_in_focus: Option<bool>,
    pub captured_at_ms: Option<i64>,
    pub phash: Option<u64>,
    /// `QualityScore.overall`.
    pub overall: f32,
    /// Considered faces, largest first.
    pub faces: Vec<FaceSignal>,
    /// Subject faces / of those looking at the camera.
    pub subjects: u32,
    pub looking: u32,
    /// A main-role person's face is a subject / every such face looks at the camera.
    pub main_present: bool,
    pub main_looking: bool,
    /// An important-role person (not main) is visible.
    pub important_present: bool,
    /// 0..=1: everyone looking at the camera in a tidy row (group portraits).
    pub posed: f32,
    /// 0..=1: laughing / screaming / cheering faces.
    pub expression: f32,
    /// 0..=1: faces moving between neighbouring frames of the moment (action).
    pub motion: f32,
    /// A main subject's (or, without people, the primary face's) eyes are half closed.
    pub eyes_borderline: bool,
    /// Faces present but none visible (backs of heads, turned away, tiny).
    pub turned_away: bool,
    /// No subject face but a sharp salient object (detail candidate).
    pub sharp_object: bool,
}

impl FrameSignals {
    /// Neutral signals for `image_id` (fixtures and fallbacks).
    pub fn new(image_id: ImageId) -> Self {
        Self {
            image_id,
            moment_key: None,
            shot_type: ShotType::Other,
            person_ids: Vec::new(),
            visible_face: 0.0,
            detail_in_focus: None,
            captured_at_ms: None,
            phash: None,
            overall: 0.0,
            faces: Vec::new(),
            subjects: 0,
            looking: 0,
            main_present: false,
            main_looking: false,
            important_present: false,
            posed: 0.0,
            expression: 0.0,
            motion: 0.0,
            eyes_borderline: false,
            turned_away: false,
            sharp_object: false,
        }
    }

    /// Activity / reaction frame: high expression or motion with at least two faces visible.
    pub fn is_activity(&self) -> bool {
        let visible = self.faces.iter().filter(|f| f.subject && f.visible >= 0.3).count();
        visible >= 2 && (self.expression >= EXPRESSION_STRONG || self.motion >= 0.5)
    }
}

/// [`detect_moments`] result.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MomentPlan {
    pub moments: Vec<MomentDraft>,
    pub frames: Vec<FrameSignals>,
}

/// People known for the project.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PeopleContext {
    pub roles: BTreeMap<PersonId, PersonRole>,
}

impl PeopleContext {
    pub fn new(roles: &[(PersonId, PersonRole)]) -> Self {
        Self { roles: roles.iter().copied().collect() }
    }

    /// Face identity produced people for this project.
    pub fn known(&self) -> bool {
        !self.roles.is_empty()
    }

    pub fn role(&self, p: Option<PersonId>) -> Option<PersonRole> {
        p.and_then(|p| self.roles.get(&p).copied())
    }
}

/// One analysed frame for [`frame_signals`].
#[derive(Debug, Clone, Copy)]
pub struct FrameInput<'a> {
    pub image_id: ImageId,
    pub captured_at_ms: Option<i64>,
    pub overall: f32,
    pub metrics: &'a ImageMetrics,
    /// `faces_json` (same order as `metrics.faces`).
    pub faces: &'a [FaceInfo],
    /// Person per face index (shorter = unknown).
    pub persons: &'a [Option<PersonId>],
    /// Tile sharpness grid of the preview (no-face frames only).
    pub grid: Option<&'a TileGrid>,
}

fn smoothstep(lo: f32, hi: f32, x: f32) -> f32 {
    ((x - lo) / (hi - lo)).clamp(0.0, 1.0)
}

/// Eye state of a face from the scorer's flags (`FaceInfo`, thresholds already applied).
pub fn eyes_of(info: &FaceInfo) -> Eyes {
    if info.blink {
        return Eyes::Closed;
    }
    match info.eyes_open {
        None => Eyes::Unknown,
        Some(o) if o < CLOSED_EYES => Eyes::Closed,
        Some(o) if o < BORDERLINE_EYES => Eyes::Borderline,
        Some(_) => Eyes::Open,
    }
}

/// Faces looking at the camera: frontal and turned less than [`LOOK_YAW`] /
/// [`LOOK_HEAD_YAW_DEG`].
pub fn looks_at_camera(m: &FaceMetrics) -> bool {
    let yaw_ok = match m.head_yaw {
        Some(deg) => deg.abs() <= LOOK_HEAD_YAW_DEG,
        None => m.yaw.abs() <= LOOK_YAW,
    };
    m.frontal && yaw_ok
}

/// 0..=1 expression of a face: open mouth (laughing, screaming, cheering) scores highest, a
/// wide smile less.
pub fn expression_of(m: &FaceMetrics) -> f32 {
    let open = m.mouth_open.map_or(0.0, |v| smoothstep(0.08, scoring::LAUGH_MOUTH_OPEN + 0.05, v));
    let smile = m.mouth_width.map_or(0.0, |v| 0.7 * smoothstep(0.5, scoring::LAUGH_MOUTH_WIDTH + 0.04, v));
    open.max(smile)
}

/// Visibility 0..=1 of one face: size x pose x eyes x light.
pub fn face_visibility(m: &FaceMetrics, eyes: Eyes) -> f32 {
    let size = smoothstep(VISIBLE_SIZE_LO, VISIBLE_SIZE_HI, m.bbox.height);
    let pose = if m.frontal {
        match m.head_yaw {
            Some(deg) => 1.0 - 0.5 * smoothstep(30.0, 70.0, deg.abs()),
            None => 1.0 - 0.4 * smoothstep(0.25, 0.6, m.yaw.abs()),
        }
    } else {
        // Profile / turned away: the detector found a face but no usable landmarks.
        match m.head_yaw {
            Some(deg) => (0.55 - 0.4 * smoothstep(45.0, 85.0, deg.abs())).max(0.15),
            None => 0.4,
        }
    };
    let eyes = match eyes {
        Eyes::Open => 1.0,
        Eyes::Unknown => 0.8,
        Eyes::Borderline => 0.65,
        Eyes::Closed => 0.35,
    };
    let light = if m.face_luma < scoring::FOCUS_MIN_FACE_LUMA { 0.5 } else { 1.0 };
    let sharp = 0.7 + 0.3 * smoothstep(0.2, 0.5, m.face_sharpness);
    (size * pose * eyes * light * sharp).clamp(0.0, 1.0)
}

/// Detail focus check on a tile grid: `Some(true)` when the central area's sharpest tile is
/// (nearly) the frame's sharpest and sharp enough, `Some(false)` when the sharpness sits
/// elsewhere (background / edge in focus, object soft), `None` when nothing can be judged.
pub fn detail_focus(grid: &TileGrid) -> Option<bool> {
    if grid.nx == 0 || grid.ny == 0 {
        return None;
    }
    let mut frame_max = 0.0f32;
    let mut centre_max: Option<f32> = None;
    for ty in 0..grid.ny {
        for tx in 0..grid.nx {
            let Some(v) = grid.at(tx, ty) else { continue };
            frame_max = frame_max.max(v);
            let cx = (tx as f32 + 0.5) / grid.nx as f32;
            let cy = (ty as f32 + 0.5) / grid.ny as f32;
            let m = DETAIL_CENTRE_MARGIN;
            if cx >= m && cx <= 1.0 - m && cy >= m && cy <= 1.0 - m {
                centre_max = Some(centre_max.map_or(v, |c: f32| c.max(v)));
            }
        }
    }
    if frame_max <= 0.0 {
        return None;
    }
    let centre = centre_max.unwrap_or(0.0);
    Some(centre >= DETAIL_CENTRE_SHARE * frame_max && centre >= DETAIL_MIN_SHARP * 0.9)
}

/// Builds the signals of one frame (shot type included; moment key and motion are set by
/// [`group_moments`]).
pub fn frame_signals(input: &FrameInput, people: &PeopleContext, shoot: ShootType) -> FrameSignals {
    let m = input.metrics;
    let mut faces: Vec<FaceSignal> = Vec::new();
    for (i, fm) in m.faces.iter().enumerate() {
        let info = input.faces.get(i);
        let considered = info.map_or(fm.detection_score >= scoring::MIN_CONSIDER_SCORE, |f| f.considered);
        if !considered {
            continue;
        }
        let eyes = info.map_or(Eyes::Unknown, eyes_of);
        let person_id = input.persons.get(i).copied().flatten();
        faces.push(FaceSignal {
            bbox: fm.bbox,
            person_id,
            role: people.role(person_id),
            visible: face_visibility(fm, eyes),
            looking: looks_at_camera(fm) && !matches!(eyes, Eyes::Closed),
            eyes,
            expression: expression_of(fm),
            subject: false,
        });
    }
    let largest = faces.iter().map(|f| f.bbox.height).fold(0.0, f32::max);
    for f in &mut faces {
        f.subject = f.bbox.height >= scoring::SUBJECT_REL * largest;
    }
    let subj: Vec<&FaceSignal> = faces.iter().filter(|f| f.subject).collect();
    let subjects = subj.len() as u32;
    let looking = subj.iter().filter(|f| f.looking).count() as u32;
    let visible_face = subj.iter().map(|f| f.visible).fold(0.0, f32::max);
    let mains: Vec<&&FaceSignal> = subj.iter().filter(|f| f.role == Some(PersonRole::Main)).collect();
    let main_present = !mains.is_empty();
    let main_looking = main_present && mains.iter().all(|f| f.looking);
    let important_present = subj.iter().any(|f| f.role == Some(PersonRole::Important) && f.visible >= 0.3);
    let posed = posed_score(&subj);
    let expression = {
        let vis: Vec<f32> = subj.iter().filter(|f| f.visible >= 0.3).map(|f| f.expression).collect();
        if vis.is_empty() {
            0.0
        } else {
            let max = vis.iter().copied().fold(0.0, f32::max);
            let mean = vis.iter().sum::<f32>() / vis.len() as f32;
            0.5 * max + 0.5 * mean
        }
    };
    // Borderline eyes on the main subject (or, without people, the largest face).
    let eyes_borderline = if main_present {
        mains.iter().any(|f| f.eyes == Eyes::Borderline)
    } else {
        subj.first().is_some_and(|f| f.eyes == Eyes::Borderline)
    };
    let turned_away = subjects > 0 && visible_face < 0.3;
    let tiles = &m.tiles;
    let sharp_object = subjects == 0
        && tiles.p90 >= DETAIL_MIN_SHARP
        && (tiles.p50 <= DETAIL_SHALLOW * tiles.p90 || input.grid.and_then(detail_focus) == Some(true));
    let mut person_ids: Vec<PersonId> = faces.iter().filter_map(|f| f.person_id).collect();
    person_ids.sort_unstable();
    person_ids.dedup();
    let mut s = FrameSignals {
        image_id: input.image_id,
        moment_key: None,
        shot_type: ShotType::Other,
        person_ids,
        visible_face,
        detail_in_focus: None,
        captured_at_ms: input.captured_at_ms,
        phash: Some(m.phash),
        overall: input.overall,
        faces,
        subjects,
        looking,
        main_present,
        main_looking,
        important_present,
        posed,
        expression,
        motion: 0.0,
        eyes_borderline,
        turned_away,
        sharp_object,
    };
    s.shot_type = classify_shot(&s, people, shoot);
    if s.shot_type == ShotType::Detail {
        s.detail_in_focus = input.grid.and_then(detail_focus);
    }
    s
}

/// Posed 0..=1: share of subject faces looking at the camera, plus (3+ faces) similar face
/// sizes (a row of people at one distance).
fn posed_score(subj: &[&FaceSignal]) -> f32 {
    if subj.is_empty() {
        return 0.0;
    }
    let n = subj.len() as f32;
    let share = subj.iter().filter(|f| f.looking).count() as f32 / n;
    if subj.len() < 3 {
        return share;
    }
    let mean = subj.iter().map(|f| f.bbox.height).sum::<f32>() / n;
    let var = subj.iter().map(|f| (f.bbox.height - mean).powi(2)).sum::<f32>() / n;
    let cv = if mean > 0.0 { var.sqrt() / mean } else { 1.0 };
    0.75 * share + 0.25 * (1.0 - (cv / 0.5).min(1.0))
}

/// Shot type of one frame from its signals.
/// - no subject face: detail when a sharp salient object, else other;
/// - >= [`GROUP_MIN_FACES`] subject faces, posed: group;
/// - people known: couple when main-role faces are the only known subject faces (and at most
///   two subjects or every subject is main), else candid;
/// - people unknown (or no face recognised): wedding / portrait shoots call one or two
///   dominant decent-size faces a couple; everything else with faces is candid.
pub fn classify_shot(f: &FrameSignals, people: &PeopleContext, shoot: ShootType) -> ShotType {
    let subj: Vec<&FaceSignal> = f.faces.iter().filter(|x| x.subject).collect();
    if subj.is_empty() {
        return if f.sharp_object { ShotType::Detail } else { ShotType::Other };
    }
    let n = subj.len() as u32;
    if n >= GROUP_MIN_FACES && f.posed >= POSED_MIN {
        return ShotType::Group;
    }
    let mains = subj.iter().filter(|x| x.role == Some(PersonRole::Main)).count() as u32;
    let known_others = subj.iter().filter(|x| x.role.is_some() && x.role != Some(PersonRole::Main)).count();
    if people.known() && mains > 0 {
        return if known_others == 0 && (n <= 2 || mains == n) { ShotType::Couple } else { ShotType::Candid };
    }
    if people.known() && subj.iter().any(|x| x.role.is_some()) {
        return ShotType::Candid;
    }
    // No people recognised in this frame.
    let couple_shoot = matches!(shoot, ShootType::Wedding | ShootType::Portrait);
    let largest = subj.iter().map(|x| x.bbox.height).fold(0.0, f32::max);
    if couple_shoot && n <= 2 && largest >= COUPLE_FALLBACK_FACE && f.visible_face >= 0.3 {
        ShotType::Couple
    } else {
        ShotType::Candid
    }
}

// ---------------------------------------------------------------------------
// Similarity + grouping
// ---------------------------------------------------------------------------

/// pHash similarity 0..=1 (Hamming 0 = 1.0, 32 = unrelated = 0).
pub fn hash_similarity(a: u64, b: u64) -> f32 {
    (1.0 - (a ^ b).count_ones() as f32 / 32.0).clamp(0.0, 1.0)
}

/// Face layout similarity 0..=1: subject faces matched greedily by centre distance (in face
/// heights) and size. `None` when either frame has no face.
pub fn layout_similarity(a: &FrameSignals, b: &FrameSignals) -> Option<f32> {
    let fa: Vec<&NormRect> = a.faces.iter().filter(|f| f.subject).map(|f| &f.bbox).collect();
    let fb: Vec<&NormRect> = b.faces.iter().filter(|f| f.subject).map(|f| &f.bbox).collect();
    // A frame without a face (back of the head, face hidden, detection flicker) says nothing
    // about the layout: the hash decides.
    if fa.is_empty() || fb.is_empty() {
        return None;
    }
    let centre = |r: &NormRect| (r.x + r.width / 2.0, r.y + r.height / 2.0);
    let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
    for (i, ra) in fa.iter().enumerate() {
        for (j, rb) in fb.iter().enumerate() {
            let (ca, cb) = (centre(ra), centre(rb));
            let h = ra.height.max(rb.height).max(1e-3);
            let d = ((ca.0 - cb.0).powi(2) + (ca.1 - cb.1).powi(2)).sqrt() / h;
            let size = ra.height.min(rb.height) / h;
            pairs.push(((-(d / 0.6).powi(2)).exp() * size, i, j));
        }
    }
    pairs.sort_by(|x, y| y.0.total_cmp(&x.0).then(x.1.cmp(&y.1)).then(x.2.cmp(&y.2)));
    let (mut used_a, mut used_b) = (vec![false; fa.len()], vec![false; fb.len()]);
    let mut sum = 0.0;
    for (s, i, j) in pairs {
        if !used_a[i] && !used_b[j] {
            used_a[i] = true;
            used_b[j] = true;
            sum += s;
        }
    }
    Some(sum / fa.len().max(fb.len()) as f32)
}

/// Jaccard of the recognised people, `None` when either frame has nobody recognised.
pub fn people_similarity(a: &FrameSignals, b: &FrameSignals) -> Option<f32> {
    if a.person_ids.is_empty() || b.person_ids.is_empty() {
        return None;
    }
    let sa: BTreeSet<_> = a.person_ids.iter().collect();
    let sb: BTreeSet<_> = b.person_ids.iter().collect();
    Some(sa.intersection(&sb).count() as f32 / sa.union(&sb).count() as f32)
}

/// Visual similarity 0..=1 of two frames (1 = the same picture): pHash (0.55), face layout
/// (0.3, when faces) and people (0.15, when recognised), renormalised over what is known.
pub fn similarity(a: &FrameSignals, b: &FrameSignals) -> f32 {
    let mut num = 0.0;
    let mut den = 0.0;
    if let (Some(x), Some(y)) = (a.phash, b.phash) {
        num += 0.55 * hash_similarity(x, y);
        den += 0.55;
    }
    if let Some(l) = layout_similarity(a, b) {
        num += 0.3 * l;
        den += 0.3;
    }
    if let Some(p) = people_similarity(a, b) {
        num += 0.15 * p;
        den += 0.15;
    }
    if den == 0.0 {
        0.0
    } else {
        (num / den).clamp(0.0, 1.0)
    }
}

/// Similarity bar for linking a frame to a moment whose last frame is `gap_ms` earlier
/// (`None` = unknown time).
pub fn link_threshold(gap_ms: Option<i64>) -> f32 {
    match gap_ms {
        None => LINK_SIM_FAR,
        Some(g) if g <= BURST_GAP_MS => LINK_SIM_BURST,
        Some(g) if g <= LINK_GAP_MS => LINK_SIM,
        Some(g) => LINK_SIM + (LINK_SIM_FAR - LINK_SIM) * smoothstep(LINK_GAP_MS as f32, MAX_GAP_MS as f32, g as f32),
    }
}

struct Open {
    key: u32,
    members: Vec<usize>,
    last_ms: Option<i64>,
}

/// Whether frame `f` may join a moment whose recent frames are `recent` (`gap` from the last).
fn compatible(f: &FrameSignals, recent: &[&FrameSignals], gap: Option<i64>) -> bool {
    let burst = gap.is_some_and(|g| g <= BURST_GAP_MS);
    // Details are their own moments (one per detail); a no-face frame that is not a detail
    // (backs of heads, a guest turning away) stays with the people around it.
    let details = recent.iter().filter(|r| r.shot_type == ShotType::Detail).count() * 2 > recent.len();
    if (f.shot_type == ShotType::Detail) != details && !burst {
        return false;
    }
    let mut counts: Vec<u32> = recent.iter().map(|r| r.subjects).collect();
    counts.sort_unstable();
    let median = counts.get(counts.len() / 2).copied().unwrap_or(0);
    if f.subjects >= GROUP_MIN_FACES && median >= GROUP_MIN_FACES {
        let diff = f.subjects.abs_diff(median) as f32;
        if diff >= 2f32.max(0.25 * median as f32) {
            return false;
        }
    }
    if !burst && f.person_ids.len() >= 2 {
        let last_people = recent.iter().rev().find(|r| r.person_ids.len() >= 2);
        if let Some(r) = last_people {
            if people_similarity(f, r).is_some_and(|j| j < 0.5) {
                return false;
            }
        }
    }
    true
}

/// Mean displacement (in face heights) of matched subject faces between two frames, 0..=1.
fn face_motion(a: &FrameSignals, b: &FrameSignals) -> f32 {
    let fa: Vec<&FaceSignal> = a.faces.iter().filter(|f| f.subject && f.visible >= 0.2).collect();
    let fb: Vec<&FaceSignal> = b.faces.iter().filter(|f| f.subject && f.visible >= 0.2).collect();
    if fa.is_empty() || fb.is_empty() {
        return 0.0;
    }
    let centre = |r: &NormRect| (r.x + r.width / 2.0, r.y + r.height / 2.0);
    let mut total = 0.0;
    for x in &fa {
        let cx = centre(&x.bbox);
        let d = fb
            .iter()
            .map(|y| {
                let cy = centre(&y.bbox);
                ((cx.0 - cy.0).powi(2) + (cx.1 - cy.1).powi(2)).sqrt() / x.bbox.height.max(y.bbox.height).max(1e-3)
            })
            .fold(f32::INFINITY, f32::min);
        total += d.min(2.0);
    }
    (total / fa.len() as f32 / MOTION_FULL).clamp(0.0, 1.0)
}

/// Groups `frames` into moments (any order in, capture order used: time, then id; frames
/// without a time last). Sets `moment_key`, `motion` and returns the moments (keys 1..,
/// in order of their first frame). Deterministic.
pub fn group_moments(frames: &mut [FrameSignals]) -> Vec<MomentDraft> {
    let mut order: Vec<usize> = (0..frames.len()).collect();
    order.sort_by_key(|&i| (frames[i].captured_at_ms.is_none(), frames[i].captured_at_ms, frames[i].image_id));
    let mut open: Vec<Open> = Vec::new();
    let mut closed: Vec<Open> = Vec::new();
    let mut next_key = 1u32;
    for &i in &order {
        let t = frames[i].captured_at_ms;
        // Close moments too old to continue.
        let (keep, gone): (Vec<Open>, Vec<Open>) = open.into_iter().partition(|o| match (t, o.last_ms) {
            (Some(t), Some(l)) => t - l <= MAX_GAP_MS,
            _ => true,
        });
        open = keep;
        closed.extend(gone);
        let mut best: Option<(f32, usize)> = None;
        for (oi, o) in open.iter().enumerate() {
            let gap = match (t, o.last_ms) {
                (Some(t), Some(l)) => Some(t - l),
                _ => None,
            };
            let recent: Vec<&FrameSignals> = o.members.iter().rev().take(RECENT).map(|&m| &frames[m]).collect();
            if !compatible(&frames[i], &recent, gap) {
                continue;
            }
            let sim = recent.iter().map(|r| similarity(&frames[i], r)).fold(0.0, f32::max);
            let bar = link_threshold(gap);
            if sim >= bar && best.is_none_or(|(s, _)| sim > s) {
                best = Some((sim, oi));
            }
        }
        match best {
            Some((_, oi)) => {
                open[oi].members.push(i);
                if t.is_some() {
                    open[oi].last_ms = t;
                }
            }
            None => {
                open.push(Open { key: next_key, members: vec![i], last_ms: t });
                next_key += 1;
            }
        }
    }
    closed.extend(open);
    closed.sort_by_key(|o| o.key);

    let mut moments = Vec::with_capacity(closed.len());
    for o in &closed {
        // Motion: face displacement against the previous member <= 3 s earlier.
        for w in o.members.windows(2) {
            let (a, b) = (w[0], w[1]);
            let close =
                matches!((frames[a].captured_at_ms, frames[b].captured_at_ms), (Some(x), Some(y)) if y - x <= 3_000);
            if close {
                let m = face_motion(&frames[a], &frames[b]);
                frames[b].motion = frames[b].motion.max(m);
            }
        }
        for &m in &o.members {
            frames[m].moment_key = Some(o.key);
        }
        let members: Vec<&FrameSignals> = o.members.iter().map(|&m| &frames[m]).collect();
        moments.push(moment_draft(o.key, &members));
    }
    moments
}

/// The moment's shot type: group when at least 30% of its frames are posed groups and most
/// frames show >= 3 faces (activity frames of a group setup are candid on their own); else
/// the most frequent frame type (ties: couple, group, detail, candid, other).
pub fn moment_shot_type(members: &[&FrameSignals]) -> ShotType {
    let n = members.len().max(1);
    let count = |t: ShotType| members.iter().filter(|f| f.shot_type == t).count();
    let many_faces = members.iter().filter(|f| f.subjects >= GROUP_MIN_FACES).count();
    if count(ShotType::Group) * 10 >= n * 3 && many_faces * 2 > n {
        return ShotType::Group;
    }
    let order = [ShotType::Couple, ShotType::Group, ShotType::Detail, ShotType::Candid, ShotType::Other];
    let mut best = ShotType::Other;
    let mut best_n = 0;
    for t in order {
        let c = count(t);
        if c > best_n {
            best = t;
            best_n = c;
        }
    }
    best
}

fn moment_draft(key: u32, members: &[&FrameSignals]) -> MomentDraft {
    let shot_type = moment_shot_type(members);
    let times: Vec<i64> = members.iter().filter_map(|f| f.captured_at_ms).collect();
    let rep = members
        .iter()
        .max_by(|a, b| {
            let sa = a.overall * 0.6 + a.visible_face * 0.4;
            let sb = b.overall * 0.6 + b.visible_face * 0.4;
            sa.total_cmp(&sb).then(b.image_id.cmp(&a.image_id))
        })
        .map(|f| f.image_id);
    let mut people: Vec<PersonId> = members.iter().flat_map(|f| f.person_ids.iter().copied()).collect();
    people.sort_unstable();
    people.dedup();
    MomentDraft {
        key,
        shot_type,
        started_at_ms: times.iter().min().copied(),
        ended_at_ms: times.iter().max().copied(),
        representative_id: rep,
        person_ids: people,
    }
}

// ---------------------------------------------------------------------------
// Catalog
// ---------------------------------------------------------------------------

/// An analysed photo of the project as read by [`detect_moments`].
struct Loaded {
    id: ImageId,
    captured_at_ms: Option<i64>,
    overall: f32,
    preview: Option<PathBuf>,
    metrics: ImageMetrics,
    faces: Vec<FaceInfo>,
}

/// SQL predicate (over `i` = images, `a` = image_analysis, `q` = quality_scores) for "analysed
/// with the current model and scored".
pub const ANALYSED: &str = "a.status = 'done' AND a.model_version = ?2 AND a.metrics_json IS NOT NULL
                            AND q.image_id IS NOT NULL";

fn load(conn: &Connection, project_id: ProjectId) -> AppResult<Vec<Loaded>> {
    let sql = format!(
        "SELECT i.id, i.captured_at_ms, q.overall, t.preview_path, a.metrics_json, a.faces_json
         FROM images i JOIN folders f ON f.id = i.folder_id
         JOIN image_analysis a ON a.image_id = i.id
         LEFT JOIN quality_scores q ON q.image_id = i.id
         LEFT JOIN thumbnails t ON t.image_id = i.id
         WHERE f.project_id = ?1 AND {ANALYSED}
         ORDER BY i.id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params![project_id, MODEL_VERSION], |r| {
        Ok((
            r.get::<_, ImageId>(0)?,
            r.get::<_, Option<i64>>(1)?,
            r.get::<_, f64>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, Option<String>>(5)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, captured_at_ms, overall, preview, metrics, faces) = row?;
        let Ok(metrics) = serde_json::from_str::<ImageMetrics>(&metrics) else { continue };
        let faces: Vec<FaceInfo> = faces.and_then(|f| serde_json::from_str(&f).ok()).unwrap_or_default();
        out.push(Loaded {
            id,
            captured_at_ms,
            overall: overall as f32,
            preview: preview.map(PathBuf::from),
            metrics,
            faces,
        });
    }
    Ok(out)
}

/// Ids of the project's photos [`detect_moments`] uses (analysed with the current model).
pub fn analysed_ids(conn: &Connection, project_id: ProjectId) -> AppResult<BTreeSet<ImageId>> {
    let sql = format!(
        "SELECT i.id FROM images i JOIN folders f ON f.id = i.folder_id
         JOIN image_analysis a ON a.image_id = i.id LEFT JOIN quality_scores q ON q.image_id = i.id
         WHERE f.project_id = ?1 AND {ANALYSED}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params![project_id, MODEL_VERSION], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn same_box(a: &NormRect, b: &NormRect) -> bool {
    (a.x - b.x).abs() <= 0.02 && (a.y - b.y).abs() <= 0.02 && (a.height - b.height).abs() <= 0.02
}

/// Groups the project's analysed photos into moments and classifies them, using the people
/// stored by `ml::identity` (`face_embeddings.person_id`, roles via `db::target::people_roles`).
/// Works without people (identity unavailable: fallbacks in [`classify_shot`]). Checks
/// `cancel` while computing detail focus (returns what it has; the caller checks `cancel`).
pub fn detect_moments(
    conn: &Connection,
    project_id: ProjectId,
    shoot_type: ShootType,
    cancel: &AtomicBool,
) -> AppResult<MomentPlan> {
    let loaded = load(conn, project_id)?;
    let people = PeopleContext::new(&target::people_roles(conn, project_id)?);
    // Per-face person ids, only where the embedding's box still matches the analysed face.
    let mut persons: HashMap<ImageId, Vec<Option<PersonId>>> = HashMap::new();
    if people.known() {
        let boxes: HashMap<ImageId, &Vec<FaceInfo>> = loaded.iter().map(|l| (l.id, &l.faces)).collect();
        for e in target::project_embeddings(conn, project_id)? {
            let (Some(pid), Some(faces)) = (e.person_id, boxes.get(&e.image_id)) else { continue };
            let idx = e.face_index as usize;
            if faces.get(idx).is_some_and(|f| same_box(&f.bbox, &e.bbox)) {
                let v = persons.entry(e.image_id).or_default();
                if v.len() <= idx {
                    v.resize(idx + 1, None);
                }
                v[idx] = Some(pid);
            }
        }
    }
    // Detail focus needs where the sharpness is: tile grids of no-face frames only.
    let needs_grid = |l: &Loaded| {
        l.faces.iter().all(|f| !f.considered) && l.metrics.tiles.p90 >= DETAIL_MIN_SHARP && l.preview.is_some()
    };
    let grids: HashMap<ImageId, TileGrid> = loaded
        .par_iter()
        .filter(|l| needs_grid(l))
        .filter_map(|l| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            metrics::preview_tile_grid(l.preview.as_ref()?).ok().map(|g| (l.id, g))
        })
        .collect();
    let empty: Vec<Option<PersonId>> = Vec::new();
    let mut frames: Vec<FrameSignals> = loaded
        .iter()
        .map(|l| {
            let input = FrameInput {
                image_id: l.id,
                captured_at_ms: l.captured_at_ms,
                overall: l.overall,
                metrics: &l.metrics,
                faces: &l.faces,
                persons: persons.get(&l.id).unwrap_or(&empty),
                grid: grids.get(&l.id),
            };
            frame_signals(&input, &people, shoot_type)
        })
        .collect();
    let moments = group_moments(&mut frames);
    Ok(MomentPlan { moments, frames })
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! Synthetic frames for the moments and selection tests.
    use super::*;

    pub fn face(x: f32, y: f32, h: f32) -> FaceSignal {
        FaceSignal {
            bbox: NormRect { x, y, width: h * 0.75, height: h },
            person_id: None,
            role: None,
            visible: 0.9,
            looking: true,
            eyes: Eyes::Open,
            expression: 0.1,
            subject: true,
        }
    }

    /// A frame with `faces` (subject flags and counts derived), time `t_s` seconds, hash.
    pub fn frame(id: ImageId, t_s: i64, phash: u64, faces: Vec<FaceSignal>) -> FrameSignals {
        let mut f = FrameSignals::new(id);
        f.captured_at_ms = Some(t_s * 1000);
        f.phash = Some(phash);
        f.overall = 0.7;
        set_faces(&mut f, faces);
        f
    }

    pub fn set_faces(f: &mut FrameSignals, faces: Vec<FaceSignal>) {
        f.subjects = faces.iter().filter(|x| x.subject).count() as u32;
        f.looking = faces.iter().filter(|x| x.subject && x.looking).count() as u32;
        f.visible_face = faces.iter().filter(|x| x.subject).map(|x| x.visible).fold(0.0, f32::max);
        f.main_present = faces.iter().any(|x| x.subject && x.role == Some(PersonRole::Main));
        f.main_looking =
            f.main_present && faces.iter().filter(|x| x.subject && x.role == Some(PersonRole::Main)).all(|x| x.looking);
        f.important_present = faces.iter().any(|x| x.subject && x.role == Some(PersonRole::Important));
        let mut ids: Vec<PersonId> = faces.iter().filter_map(|x| x.person_id).collect();
        ids.sort_unstable();
        ids.dedup();
        f.person_ids = ids;
        let subj: Vec<&FaceSignal> = faces.iter().filter(|x| x.subject).collect();
        f.posed = posed_score(&subj);
        f.turned_away = f.subjects > 0 && f.visible_face < 0.3;
        f.faces = faces;
    }

    /// `n` faces in a row at height `h` (a group).
    pub fn row(n: usize, h: f32) -> Vec<FaceSignal> {
        (0..n).map(|i| face(0.1 + i as f32 * 0.8 / n as f32, 0.3, h)).collect()
    }

    pub fn person(mut f: FaceSignal, id: PersonId, role: PersonRole) -> FaceSignal {
        f.person_id = Some(id);
        f.role = Some(role);
        f
    }

    /// `base` with `bits` low bits flipped (pHash distance = `bits`).
    pub fn near(base: u64, bits: u32) -> u64 {
        base ^ ((1u64 << bits) - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use crate::ipc::types::{ExposureStats, NormPoint};
    use crate::ml::{HighlightStats, TileStats};

    const H1: u64 = 0x0F0F_0F0F_0F0F_0F0F;
    const H2: u64 = 0xF0F0_F0F0_F0F0_F0F0;

    fn fm(x: f32, y: f32, h: f32) -> FaceMetrics {
        FaceMetrics {
            bbox: NormRect { x, y, width: h * 0.75, height: h },
            left_eye: NormPoint { x: x + 0.2 * h, y: y + 0.4 * h },
            right_eye: NormPoint { x: x + 0.5 * h, y: y + 0.4 * h },
            detection_score: 0.9,
            ear: Some(0.3),
            sharpness: 0.7,
            ear_left: Some(0.3),
            ear_right: Some(0.3),
            mouth_open: Some(0.02),
            yaw: 0.05,
            iod_px: 60.0,
            face_sharpness: 0.7,
            eye_texture: 10.0,
            anisotropy: 0.05,
            frontal: true,
            truncated: false,
            eye_open_prob: Some(0.9),
            head_pitch: Some(0.0),
            head_yaw: Some(5.0),
            mesh_ear: Some(0.3),
            face_luma: 0.5,
            mouth_width: Some(0.5),
            blown: 0.0,
        }
    }

    fn info(f: &FaceMetrics, eyes_open: Option<f32>, blink: bool) -> FaceInfo {
        FaceInfo {
            bbox: f.bbox,
            left_eye: f.left_eye,
            right_eye: f.right_eye,
            detection_score: f.detection_score,
            ear: f.ear,
            eyes_open,
            sharpness: f.sharpness,
            blink,
            in_focus: true,
            primary: false,
            considered: true,
        }
    }

    fn metrics(faces: Vec<FaceMetrics>, p90: f32, p50: f32) -> ImageMetrics {
        ImageMetrics {
            width: 2048,
            height: 1365,
            faces,
            global_sharpness: p90,
            exposure: ExposureStats { clipped_highlights_pct: 0.0, clipped_shadows_pct: 0.0, mean_luma: 0.4 },
            phash: H1,
            tiles: TileStats { p90, p50, textured: 0.8, anisotropy: 0.05 },
            highlights: HighlightStats::default(),
        }
    }

    fn signals(
        m: &ImageMetrics,
        people: &PeopleContext,
        persons: &[Option<PersonId>],
        grid: Option<&TileGrid>,
    ) -> FrameSignals {
        let faces: Vec<FaceInfo> = m.faces.iter().map(|f| info(f, Some(0.9), false)).collect();
        let input =
            FrameInput { image_id: 1, captured_at_ms: Some(0), overall: 0.7, metrics: m, faces: &faces, persons, grid };
        frame_signals(&input, people, ShootType::Wedding)
    }

    fn grid(sharp_at: (usize, usize), centre_soft: bool) -> TileGrid {
        let (nx, ny) = (8, 5);
        let mut tiles = vec![Some(0.2); nx * ny];
        tiles[sharp_at.1 * nx + sharp_at.0] = Some(0.8);
        if !centre_soft {
            tiles[2 * nx + 4] = Some(0.78);
        }
        TileGrid { nx, ny, tiles }
    }

    #[test]
    fn visible_face_score_frontal_size_eyes() {
        let big = fm(0.4, 0.2, 0.2);
        let open = face_visibility(&big, Eyes::Open);
        assert!(open > 0.9, "{open}");
        assert!(face_visibility(&big, Eyes::Closed) < 0.5 * open);
        let tiny = fm(0.4, 0.2, 0.02);
        assert_eq!(face_visibility(&tiny, Eyes::Open), 0.0);
        let mut profile = fm(0.4, 0.2, 0.2);
        profile.frontal = false;
        profile.head_yaw = Some(80.0);
        assert!(face_visibility(&profile, Eyes::Unknown) < 0.3);
        let mut dark = fm(0.4, 0.2, 0.2);
        dark.face_luma = 0.05;
        assert!(face_visibility(&dark, Eyes::Open) <= 0.5 * open + 1e-6);
    }

    #[test]
    fn eyes_from_scorer_flags() {
        let f = fm(0.4, 0.2, 0.2);
        assert_eq!(eyes_of(&info(&f, Some(0.9), false)), Eyes::Open);
        assert_eq!(eyes_of(&info(&f, Some(0.4), false)), Eyes::Borderline);
        assert_eq!(eyes_of(&info(&f, Some(0.1), false)), Eyes::Closed);
        assert_eq!(eyes_of(&info(&f, Some(0.9), true)), Eyes::Closed);
        assert_eq!(eyes_of(&info(&f, None, false)), Eyes::Unknown);
    }

    #[test]
    fn shot_type_labelled_fixtures() {
        let none = PeopleContext::default();
        // Couple (no identity): two large faces.
        let couple = signals(&metrics(vec![fm(0.3, 0.2, 0.25), fm(0.55, 0.22, 0.24)], 0.8, 0.5), &none, &[], None);
        assert_eq!(couple.shot_type, ShotType::Couple);
        // Group: five posed faces in a row.
        let five: Vec<FaceMetrics> = (0..5).map(|i| fm(0.1 + i as f32 * 0.16, 0.3, 0.1)).collect();
        let group = signals(&metrics(five.clone(), 0.8, 0.6), &none, &[], None);
        assert_eq!(group.shot_type, ShotType::Group);
        assert_eq!(group.looking, 5);
        // Candid: five faces, most turned away (not posed).
        let mut turned = five;
        for f in turned.iter_mut().skip(1) {
            f.frontal = false;
            f.head_yaw = Some(60.0);
        }
        assert_eq!(signals(&metrics(turned, 0.8, 0.6), &none, &[], None).shot_type, ShotType::Candid);
        // Candid: one small face (a guest across the room).
        assert_eq!(signals(&metrics(vec![fm(0.7, 0.4, 0.06)], 0.8, 0.6), &none, &[], None).shot_type, ShotType::Candid);
        // Detail: no face, sharp object, shallow depth of field.
        let detail = signals(&metrics(vec![], 0.8, 0.3), &none, &[], Some(&grid((4, 2), false)));
        assert_eq!(detail.shot_type, ShotType::Detail);
        assert_eq!(detail.detail_in_focus, Some(true));
        // Other: no face, everything equally sharp (venue / backs of heads).
        assert_eq!(signals(&metrics(vec![], 0.8, 0.75), &none, &[], None).shot_type, ShotType::Other);
        // Other: no face, soft.
        assert_eq!(signals(&metrics(vec![], 0.3, 0.1), &none, &[], None).shot_type, ShotType::Other);
    }

    #[test]
    fn shot_type_with_people() {
        let people = PeopleContext::new(&[(1, PersonRole::Main), (2, PersonRole::Main), (3, PersonRole::Important)]);
        let m = metrics(vec![fm(0.3, 0.2, 0.25), fm(0.55, 0.22, 0.24)], 0.8, 0.5);
        let couple = signals(&m, &people, &[Some(1), Some(2)], None);
        assert_eq!(couple.shot_type, ShotType::Couple);
        assert!(couple.main_present && couple.main_looking);
        // Bride with her mother: candid (with an important person).
        let with_mum = signals(&m, &people, &[Some(1), Some(3)], None);
        assert_eq!(with_mum.shot_type, ShotType::Candid);
        assert!(with_mum.important_present);
        // Two guests, faces large: candid when people are known (not the couple).
        let guests =
            signals(&m, &PeopleContext::new(&[(1, PersonRole::Main), (4, PersonRole::Other)]), &[Some(4), None], None);
        assert_eq!(guests.shot_type, ShotType::Candid);
        // The main subject looking away in a group: main present, not looking.
        let mut five: Vec<FaceMetrics> = (0..5).map(|i| fm(0.1 + i as f32 * 0.16, 0.3, 0.1)).collect();
        five[2].head_yaw = Some(50.0);
        let g = signals(&metrics(five, 0.8, 0.6), &people, &[None, None, Some(1), None, None], None);
        assert_eq!(g.shot_type, ShotType::Group);
        assert!(g.main_present && !g.main_looking);
        assert_eq!(g.looking, 4);
    }

    #[test]
    fn back_of_head_is_turned_away_not_detail() {
        let none = PeopleContext::default();
        let mut back = fm(0.4, 0.3, 0.15);
        back.frontal = false;
        back.head_yaw = Some(85.0);
        back.face_luma = 0.08;
        let s = signals(&metrics(vec![back], 0.8, 0.3), &none, &[], None);
        assert!(s.turned_away);
        assert!(s.visible_face < 0.3);
        assert_ne!(s.shot_type, ShotType::Detail);
    }

    #[test]
    fn detail_focus_on_central_object() {
        assert_eq!(detail_focus(&grid((4, 2), false)), Some(true));
        // Sharpest tile in a corner, centre soft: focus missed the object.
        assert_eq!(detail_focus(&grid((0, 0), true)), Some(false));
        assert_eq!(detail_focus(&TileGrid { nx: 4, ny: 3, tiles: vec![None; 12] }), None);
        let none = PeopleContext::default();
        let s = signals(&metrics(vec![], 0.8, 0.3), &none, &[], Some(&grid((0, 0), true)));
        assert_eq!(s.shot_type, ShotType::Detail);
        assert_eq!(s.detail_in_focus, Some(false));
    }

    #[test]
    fn expression_and_activity() {
        let mut laugh = fm(0.4, 0.2, 0.2);
        laugh.mouth_open = Some(0.3);
        assert!(expression_of(&laugh) >= 0.99);
        assert!(expression_of(&fm(0.4, 0.2, 0.2)) < 0.3);
        let mut f = frame(1, 0, H1, row(4, 0.1));
        assert!(!f.is_activity());
        f.expression = 0.8;
        assert!(f.is_activity());
    }

    #[test]
    fn similarity_hash_layout_people() {
        let a = frame(1, 0, H1, vec![face(0.3, 0.2, 0.25), face(0.55, 0.2, 0.25)]);
        let same = frame(2, 1, near(H1, 2), vec![face(0.31, 0.2, 0.25), face(0.56, 0.2, 0.25)]);
        let moved = frame(3, 30, near(H1, 14), vec![face(0.1, 0.3, 0.15), face(0.7, 0.25, 0.18)]);
        let other = frame(4, 60, H2, vec![]);
        let s_same = similarity(&a, &same);
        let s_moved = similarity(&a, &moved);
        assert!(s_same > 0.9, "{s_same}");
        assert!(s_moved < 0.6 && s_moved > 0.2, "{s_moved}");
        assert!(similarity(&a, &other) < 0.1);
        assert_eq!(hash_similarity(H1, H1), 1.0);
        assert_eq!(hash_similarity(H1, H2), 0.0);
    }

    fn keys(frames: &[FrameSignals]) -> Vec<u32> {
        frames.iter().map(|f| f.moment_key.unwrap()).collect()
    }

    #[test]
    fn moments_split_on_time_gap_and_scene() {
        let mut frames = vec![
            frame(1, 0, H1, vec![face(0.3, 0.2, 0.25)]),
            frame(2, 1, near(H1, 2), vec![face(0.31, 0.2, 0.25)]),
            frame(3, 10, near(H1, 6), vec![face(0.35, 0.2, 0.24)]),
            // 20 min later, same look: a new moment.
            frame(4, 1210, H1, vec![face(0.3, 0.2, 0.25)]),
            // 5 s later, a different scene.
            frame(5, 1215, H2, vec![face(0.3, 0.2, 0.25), face(0.6, 0.2, 0.2)]),
        ];
        let moments = group_moments(&mut frames);
        assert_eq!(keys(&frames), vec![1, 1, 1, 2, 3]);
        assert_eq!(moments.len(), 3);
        assert_eq!(moments[0].started_at_ms, Some(0));
        assert_eq!(moments[0].ended_at_ms, Some(10_000));
    }

    #[test]
    fn moments_split_group_setups_by_head_count_and_people() {
        let mut frames = vec![
            frame(1, 0, H1, row(6, 0.1)),
            frame(2, 2, near(H1, 2), row(6, 0.1)),
            // Same backdrop, three people leave: next setup.
            frame(3, 40, near(H1, 4), row(3, 0.12)),
            frame(4, 42, near(H1, 4), row(3, 0.12)),
        ];
        group_moments(&mut frames);
        assert_eq!(keys(&frames), vec![1, 1, 2, 2]);

        // Same head count, different people (identity known): split.
        let p = |ids: [PersonId; 3]| -> Vec<FaceSignal> {
            row(3, 0.12).into_iter().zip(ids).map(|(f, id)| person(f, id, PersonRole::Other)).collect()
        };
        let mut frames = vec![
            frame(1, 0, H1, p([1, 2, 3])),
            frame(2, 3, near(H1, 1), p([1, 2, 3])),
            frame(3, 40, near(H1, 2), p([4, 5, 6])),
        ];
        group_moments(&mut frames);
        assert_eq!(keys(&frames), vec![1, 1, 2]);
    }

    #[test]
    fn moments_follow_interleaved_cameras_and_detail_vs_faces() {
        // Two cameras on two scenes, interleaved in time.
        let mut frames = vec![
            frame(1, 0, H1, vec![face(0.3, 0.2, 0.25)]),
            frame(2, 3, H2, vec![face(0.6, 0.3, 0.1), face(0.2, 0.3, 0.1)]),
            frame(3, 6, near(H1, 3), vec![face(0.32, 0.2, 0.25)]),
            frame(4, 9, near(H2, 3), vec![face(0.6, 0.3, 0.1), face(0.2, 0.3, 0.1)]),
            // A detail (no face) 10 s later with a hash close to scene 1: its own moment.
            frame(5, 19, near(H1, 4), vec![]),
            // Backs of heads (no face) right after scene 1: stays with scene 1.
            frame(6, 20, near(H1, 3), vec![]),
        ];
        frames[4].shot_type = ShotType::Detail;
        group_moments(&mut frames);
        assert_eq!(keys(&frames), vec![1, 2, 1, 2, 3, 1]);
    }

    #[test]
    fn moment_type_and_motion() {
        let mut frames = vec![
            frame(1, 0, H1, row(5, 0.1)),
            frame(2, 1, near(H1, 2), row(5, 0.1)),
            frame(3, 2, near(H1, 3), row(5, 0.1)),
        ];
        for f in &mut frames {
            f.shot_type = ShotType::Group;
        }
        // Third frame: everyone jumps (faces move up 0.1 = one face height).
        let mut jumped = row(5, 0.1);
        for x in &mut jumped {
            x.bbox.y -= 0.1;
        }
        set_faces(&mut frames[2], jumped);
        frames[2].shot_type = ShotType::Candid;
        let moments = group_moments(&mut frames);
        assert_eq!(moments.len(), 1);
        assert_eq!(moments[0].shot_type, ShotType::Group);
        assert!(frames[2].motion > 0.9, "{}", frames[2].motion);
        assert!(frames[1].motion < 0.1);
    }

    #[test]
    fn grouping_is_deterministic_and_order_independent() {
        let mk = || {
            vec![
                frame(3, 6, near(H1, 3), vec![face(0.32, 0.2, 0.25)]),
                frame(1, 0, H1, vec![face(0.3, 0.2, 0.25)]),
                frame(2, 3, H2, vec![]),
            ]
        };
        let mut a = mk();
        let mut b = mk();
        b.reverse();
        let ma = group_moments(&mut a);
        let mb = group_moments(&mut b);
        assert_eq!(ma, mb);
        let ka: BTreeMap<_, _> = a.iter().map(|f| (f.image_id, f.moment_key)).collect();
        let kb: BTreeMap<_, _> = b.iter().map(|f| (f.image_id, f.moment_key)).collect();
        assert_eq!(ka, kb);
    }
}
