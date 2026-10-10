//! Target selection tests: one per user rule (roadmap Phase 9) on synthetic shoots built from
//! `FrameSignals` fixtures, plus a wedding-like day, invariants, locking, determinism and the
//! identity-absent fallback, and the catalog pipeline end to end.

use std::collections::{BTreeMap, HashMap};

use super::*;
use crate::db::target::MomentDraft;
use crate::ml::moments::fixtures::{face, person, row, set_faces};
use crate::ml::moments::{moment_shot_type, Eyes, FaceSignal, MomentPlan};

const MAIN_A: PersonId = 1;
const MAIN_B: PersonId = 2;
const MUM: PersonId = 3;
const GUEST: PersonId = 9;

/// Deterministic, well spread 64-bit hashes (splitmix64).
fn mix(i: u64) -> u64 {
    let mut z = i.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Hash of `pose` (0 = base; poses differ by 12+ bits) and burst frame `b` (1 bit each).
fn pose_hash(base: u64, pose: u32, b: u32) -> u64 {
    let pose_bits = if pose == 0 { 0 } else { 0xFFFu64 << (12 * (pose - 1)) };
    let burst_bits = if b == 0 { 0 } else { 1u64 << (59 + b.min(4)) };
    base ^ pose_bits ^ burst_bits
}

struct Shoot {
    frames: Vec<SelectionFrame>,
    signals: Vec<FrameSignals>,
    types: BTreeMap<u32, ShotType>,
    roles: Vec<(PersonId, PersonRole)>,
    shoot_type: ShootType,
    locked: Vec<LockedChoice>,
    next_id: ImageId,
}

impl Shoot {
    fn new() -> Self {
        Self {
            frames: Vec::new(),
            signals: Vec::new(),
            types: BTreeMap::new(),
            roles: vec![
                (MAIN_A, PersonRole::Main),
                (MAIN_B, PersonRole::Main),
                (MUM, PersonRole::Important),
                (GUEST, PersonRole::Other),
            ],
            shoot_type: ShootType::Wedding,
            locked: Vec::new(),
            next_id: 1,
        }
    }

    fn push(
        &mut self,
        key: u32,
        t_s: i64,
        phash: u64,
        shot: ShotType,
        faces: Vec<FaceSignal>,
        overall: f32,
    ) -> ImageId {
        let id = self.next_id;
        self.next_id += 1;
        let mut s = FrameSignals::new(id);
        s.moment_key = Some(key);
        s.captured_at_ms = Some(t_s * 1000);
        s.phash = Some(phash);
        s.overall = overall;
        s.shot_type = shot;
        set_faces(&mut s, faces);
        let mut f = SelectionFrame::new(id, overall);
        f.captured_at_ms = s.captured_at_ms;
        f.phash = s.phash;
        self.frames.push(f);
        self.signals.push(s);
        id
    }

    fn sig(&mut self, id: ImageId) -> &mut FrameSignals {
        self.signals.iter_mut().find(|s| s.image_id == id).unwrap()
    }

    fn frame(&mut self, id: ImageId) -> &mut SelectionFrame {
        self.frames.iter_mut().find(|f| f.image_id == id).unwrap()
    }

    fn moments(&self) -> Vec<MomentDraft> {
        let mut by_key: BTreeMap<u32, Vec<&FrameSignals>> = BTreeMap::new();
        for s in &self.signals {
            by_key.entry(s.moment_key.unwrap()).or_default().push(s);
        }
        by_key
            .into_iter()
            .map(|(key, members)| MomentDraft {
                key,
                shot_type: self.types.get(&key).copied().unwrap_or_else(|| moment_shot_type(&members)),
                started_at_ms: members.iter().filter_map(|m| m.captured_at_ms).min(),
                ended_at_ms: members.iter().filter_map(|m| m.captured_at_ms).max(),
                representative_id: members.first().map(|m| m.image_id),
                person_ids: Vec::new(),
            })
            .collect()
    }

    fn input(&self, target: u32) -> SelectionInput {
        SelectionInput {
            target_count: target,
            shoot_type: self.shoot_type,
            people: self.roles.clone(),
            locked: self.locked.clone(),
            plan: MomentPlan { moments: self.moments(), frames: self.signals.clone() },
        }
    }

    fn run(&self, target: u32) -> Vec<SelectionDraft> {
        let input = self.input(target);
        let drafts = choose(&self.frames, &input);
        check_invariants(&self.frames, &input, &drafts);
        drafts
    }

    // --- builders -------------------------------------------------------------------------

    /// The couple in pose `pose` (face positions differ per pose; `angle` shifts slightly).
    fn couple_faces(pose: u32, angle: f32, people: bool) -> Vec<FaceSignal> {
        let x = 0.15 + 0.13 * pose as f32 + angle;
        let y = 0.2 + 0.06 * (pose % 2) as f32;
        let (mut a, mut b) = (face(x, y, 0.22), face(x + 0.22, y + 0.01, 0.21));
        if people {
            a = person(a, MAIN_A, PersonRole::Main);
            b = person(b, MAIN_B, PersonRole::Main);
        }
        vec![a, b]
    }

    /// `n` people in a row; the couple at the centre when `with_main`; the first `not_looking`
    /// faces (and the couple when `main_away`) not looking.
    fn group_faces(n: usize, with_main: bool, not_looking: usize, main_away: bool) -> Vec<FaceSignal> {
        let mut faces = row(n, 0.09);
        let mid = n / 2;
        for (i, f) in faces.iter_mut().enumerate() {
            if with_main && (i == mid || i == mid + 1) {
                *f = person(f.clone(), if i == mid { MAIN_A } else { MAIN_B }, PersonRole::Main);
                if main_away {
                    f.looking = false;
                }
            } else if i < not_looking {
                f.looking = false;
            }
        }
        faces
    }
}

fn by_id(drafts: &[SelectionDraft]) -> HashMap<ImageId, &SelectionDraft> {
    drafts.iter().map(|d| (d.image_id, d)).collect()
}

fn delivered(drafts: &[SelectionDraft]) -> Vec<ImageId> {
    drafts.iter().filter(|d| d.choice == TargetChoice::Deliver).map(|d| d.image_id).collect()
}

fn choice_of(drafts: &[SelectionDraft], id: ImageId) -> TargetChoice {
    by_id(drafts)[&id].choice
}

/// Holds for every run: one draft per frame with reasons; alternatives point to a delivered
/// frame of the same moment with a rank; "covered by" points to a delivered frame, of the same
/// moment whenever that moment has one; defects / user rejects never delivered; locked kept.
fn check_invariants(frames: &[SelectionFrame], input: &SelectionInput, drafts: &[SelectionDraft]) {
    assert_eq!(drafts.len(), frames.len(), "one draft per frame");
    let map = by_id(drafts);
    assert_eq!(map.len(), drafts.len(), "unique ids");
    let moment: HashMap<ImageId, Option<u32>> = drafts.iter().map(|d| (d.image_id, d.moment_key)).collect();
    let delivered_in: BTreeMap<Option<u32>, usize> =
        drafts.iter().filter(|d| d.choice == TargetChoice::Deliver).fold(BTreeMap::new(), |mut m, d| {
            *m.entry(d.moment_key).or_default() += 1;
            m
        });
    let locked: HashMap<ImageId, &LockedChoice> = input.locked.iter().map(|l| (l.image_id, l)).collect();
    for f in frames {
        let d = map[&f.image_id];
        assert!(!d.reasons.is_empty(), "image {} has no reason", d.image_id);
        assert!((0.0..=1.0).contains(&d.score));
        if let Some(l) = locked.get(&f.image_id) {
            assert_eq!(d.choice, l.choice, "locked choice of {} kept", f.image_id);
            assert_eq!(d.reasons[0].kind, TargetReasonKind::UserChoice);
            continue;
        }
        if f.scored_pick == PickFlag::Reject || f.user_pick == Some(PickFlag::Reject) {
            assert_ne!(d.choice, TargetChoice::Deliver, "defect / user reject {} delivered", f.image_id);
        }
        match d.choice {
            TargetChoice::Deliver => {
                assert!(d.alternative_of.is_none() && d.rank.is_none() && d.covered_by.is_none());
            }
            TargetChoice::Alternative => {
                let of = d.alternative_of.expect("alternative_of");
                assert_eq!(map[&of].choice, TargetChoice::Deliver);
                assert_eq!(moment[&of], d.moment_key, "alternative in the same moment");
                assert!(d.rank.is_some_and(|r| (1..=MAX_ALTERNATIVES).contains(&r)));
                assert_eq!(d.covered_by, Some(of));
            }
            _ => {}
        }
        if d.choice != TargetChoice::Deliver {
            if let Some(c) = d.covered_by {
                assert_eq!(map[&c].choice, TargetChoice::Deliver, "covered_by {} of {} delivered", c, d.image_id);
                let s = d.covered_similarity.expect("similarity");
                assert!((0.0..=1.0).contains(&s));
                if delivered_in.contains_key(&d.moment_key) {
                    assert_eq!(moment[&c], d.moment_key, "covered by the same moment for {}", d.image_id);
                } else {
                    assert!(s >= COVER_MIN);
                }
            } else {
                assert!(!delivered_in.contains_key(&d.moment_key), "{} lacks covered_by", d.image_id);
            }
        }
    }
    // Ranks of one delivered photo are 1..n without gaps.
    let mut ranks: BTreeMap<ImageId, Vec<u32>> = BTreeMap::new();
    for d in drafts.iter().filter(|d| d.choice == TargetChoice::Alternative) {
        ranks.entry(d.alternative_of.unwrap()).or_default().push(d.rank.unwrap());
    }
    for (_, mut r) in ranks {
        r.sort_unstable();
        assert_eq!(r, (1..=r.len() as u32).collect::<Vec<_>>());
    }
}

// ---------------------------------------------------------------------------
// Rule 1: couple variations
// ---------------------------------------------------------------------------

#[test]
fn rule1_couple_variations_kept_only_near_identical_collapse() {
    let mut s = Shoot::new();
    let base = mix(1);
    let mut poses: Vec<Vec<ImageId>> = Vec::new();
    for pose in 0..4u32 {
        let mut ids = Vec::new();
        for b in 0..4u32 {
            let overall = 0.7 + 0.02 * b as f32;
            ids.push(s.push(
                1,
                (pose * 20 + b) as i64,
                pose_hash(base, pose, b),
                ShotType::Couple,
                Shoot::couple_faces(pose, 0.0, true),
                overall,
            ));
        }
        poses.push(ids);
    }
    // The same pose as pose 0 from another angle: faces shift a little, the hash a lot.
    let angle = s.push(1, 90, pose_hash(base, 4, 0), ShotType::Couple, Shoot::couple_faces(0, 0.06, true), 0.72);
    let drafts = s.run(100);
    let del = delivered(&drafts);
    // One per pose (the best burst frame, b = 3) and the other angle.
    assert_eq!(del.len(), 5, "{del:?}");
    for ids in &poses {
        assert!(del.contains(&ids[3]));
        for &id in &ids[..3] {
            let d = by_id(&drafts)[&id];
            assert_eq!(d.choice, TargetChoice::Alternative);
            assert_eq!(d.alternative_of, Some(ids[3]));
            assert_eq!(d.reasons[0].kind, TargetReasonKind::NearDuplicate);
            assert!(d.reasons[0].text.starts_with("Almost the same as DSC"));
            assert!(d.covered_similarity.unwrap() >= NEAR_DUP_COUPLE);
        }
    }
    assert!(del.contains(&angle), "a different angle of the same pose is kept");
    let reasons: Vec<TargetReasonKind> =
        drafts.iter().filter(|d| d.choice == TargetChoice::Deliver).map(|d| d.reasons[0].kind).collect();
    assert!(reasons.iter().all(|&k| k == TargetReasonKind::CoupleVariation));
    // Alternatives ranked best first.
    let alt = by_id(&drafts)[&poses[0][2]];
    assert_eq!(alt.rank, Some(1));
}

// ---------------------------------------------------------------------------
// Rule 2: one per group setup, main subject looking, activity extras
// ---------------------------------------------------------------------------

#[test]
fn rule2_one_per_group_setup_most_looking_main_must_look() {
    let mut s = Shoot::new();
    let base = mix(2);
    // Setup of 8: looking 6, 8 (couple looking away), 7, 8 (all), 5.
    let a = s.push(1, 0, pose_hash(base, 0, 0), ShotType::Group, Shoot::group_faces(8, true, 2, false), 0.8);
    let b = s.push(1, 2, pose_hash(base, 0, 1), ShotType::Group, Shoot::group_faces(8, true, 0, true), 0.9);
    let c = s.push(1, 4, pose_hash(base, 0, 2), ShotType::Group, Shoot::group_faces(8, true, 1, false), 0.7);
    let best = s.push(1, 6, pose_hash(base, 0, 3), ShotType::Group, Shoot::group_faces(8, true, 0, false), 0.6);
    let e = s.push(1, 8, pose_hash(base, 0, 4), ShotType::Group, Shoot::group_faces(8, true, 3, false), 0.75);
    let drafts = s.run(100);
    assert_eq!(delivered(&drafts), vec![best], "exactly one: most faces looking with the couple looking");
    let d = by_id(&drafts)[&best];
    assert_eq!(d.reasons[0].kind, TargetReasonKind::GroupBest);
    assert_eq!(d.reasons[0].text, "Best of this group: 8 of 8 faces looking at the camera");
    for id in [a, b, c, e] {
        let d = by_id(&drafts)[&id];
        assert_eq!(d.choice, TargetChoice::Alternative, "{id}");
        assert_eq!(d.alternative_of, Some(best));
    }
    // b (higher score, couple looking away) is an alternative, not the delivery.
    assert!(by_id(&drafts)[&b].rank.is_some());
}

#[test]
fn rule2_group_without_main_looking_is_not_delivered() {
    let mut s = Shoot::new();
    let base = mix(3);
    let ids: Vec<ImageId> = (0..4)
        .map(|i| {
            s.push(
                1,
                i * 2,
                pose_hash(base, 0, i as u32),
                ShotType::Group,
                Shoot::group_faces(6, true, i as usize % 2, true),
                0.8,
            )
        })
        .collect();
    let drafts = s.run(100);
    assert!(delivered(&drafts).is_empty());
    let unsure: Vec<&SelectionDraft> = drafts.iter().filter(|d| d.choice == TargetChoice::NotSure).collect();
    assert_eq!(unsure.len(), 1);
    assert!(ids.contains(&unsure[0].image_id));
    assert_eq!(unsure[0].reasons[0].text, "The couple isn't looking at the camera in any frame of this group");
    assert!(drafts.iter().all(|d| d.choice != TargetChoice::Deliver));
}

#[test]
fn rule2_activity_frames_are_extra_variations() {
    let mut s = Shoot::new();
    let base = mix(4);
    let posed = s.push(1, 0, pose_hash(base, 0, 0), ShotType::Group, Shoot::group_faces(6, true, 0, false), 0.8);
    let _posed2 = s.push(1, 2, pose_hash(base, 0, 1), ShotType::Group, Shoot::group_faces(6, true, 1, false), 0.8);
    // Activity burst: everyone laughing / jumping, faces move.
    let mut act = Vec::new();
    for i in 0..4u32 {
        let mut faces = Shoot::group_faces(6, true, 6, true);
        for f in &mut faces {
            f.expression = 0.9;
            f.bbox.y -= 0.03 * i as f32;
        }
        let id = s.push(1, 4 + i as i64, pose_hash(base, 1 + i, 0), ShotType::Candid, faces, 0.7);
        s.sig(id).expression = 0.9;
        s.sig(id).motion = 0.6;
        act.push(id);
    }
    // A near-identical repeat of the first activity frame: collapses.
    let mut faces = Shoot::group_faces(6, true, 6, true);
    for f in &mut faces {
        f.expression = 0.9;
    }
    let repeat = s.push(1, 9, pose_hash(base, 1, 1), ShotType::Candid, faces, 0.69);
    s.sig(repeat).expression = 0.9;
    s.types.insert(1, ShotType::Group);
    let drafts = s.run(100);
    let del = delivered(&drafts);
    assert!(del.contains(&posed));
    let extras: Vec<&SelectionDraft> = drafts
        .iter()
        .filter(|d| d.choice == TargetChoice::Deliver && d.reasons[0].kind == TargetReasonKind::GroupActivity)
        .collect();
    assert_eq!(extras.len(), MAX_ACTIVITY as usize, "activity extras capped");
    assert!(extras.iter().all(|d| act.contains(&d.image_id)));
    assert_eq!(del.len(), 1 + MAX_ACTIVITY as usize);
    assert_ne!(choice_of(&drafts, repeat), TargetChoice::Deliver);
}

// ---------------------------------------------------------------------------
// Rule 3: one per detail, focus on the object
// ---------------------------------------------------------------------------

#[test]
fn rule3_one_per_detail_with_focus_on_the_object() {
    let mut s = Shoot::new();
    // Rings: the sharpest-scoring frame has its focus off the object.
    let off = s.push(1, 0, mix(10), ShotType::Detail, vec![], 0.9);
    let ok1 = s.push(1, 3, mix(10) ^ 0b111, ShotType::Detail, vec![], 0.75);
    let ok2 = s.push(1, 6, mix(10) ^ 0b1111_0000, ShotType::Detail, vec![], 0.7);
    for (id, focus) in [(off, false), (ok1, true), (ok2, true)] {
        s.sig(id).detail_in_focus = Some(focus);
        s.sig(id).sharp_object = true;
    }
    // Shoes: every frame out of focus on the object.
    let shoes: Vec<ImageId> = (0..2)
        .map(|i| {
            let id = s.push(2, 600 + i * 3, mix(11) ^ i as u64, ShotType::Detail, vec![], 0.8);
            s.sig(id).detail_in_focus = Some(false);
            id
        })
        .collect();
    let drafts = s.run(100);
    assert_eq!(delivered(&drafts), vec![ok1], "one per detail, the in-focus one");
    assert_eq!(by_id(&drafts)[&ok1].reasons[0].kind, TargetReasonKind::DetailBest);
    assert_eq!(by_id(&drafts)[&ok2].choice, TargetChoice::Alternative);
    let d_off = by_id(&drafts)[&off];
    assert_eq!(d_off.choice, TargetChoice::SetAside);
    assert_eq!(d_off.reasons[0].kind, TargetReasonKind::DetailOutOfFocus);
    assert_eq!(d_off.covered_by, Some(ok1));
    let unsure: Vec<ImageId> =
        shoes.iter().copied().filter(|&id| choice_of(&drafts, id) == TargetChoice::NotSure).collect();
    assert_eq!(unsure.len(), 1, "best out-of-focus detail asks the user");
    assert_eq!(by_id(&drafts)[&unsure[0]].reasons[0].kind, TargetReasonKind::DetailOutOfFocus);
}

// ---------------------------------------------------------------------------
// Rule 4: candids only with visible faces / action
// ---------------------------------------------------------------------------

#[test]
fn rule4_candids_need_visible_faces_or_action() {
    let mut s = Shoot::new();
    // A guest across the room: small but frontal, sharp face = crop-worthy.
    let mut small = face(0.7, 0.4, 0.07);
    small.visible = 0.6;
    let crop = s.push(1, 0, mix(20), ShotType::Candid, vec![small], 0.7);
    // Backs of heads: no face found.
    let back = s.push(1, 5, mix(20) ^ 0xFF_FF00, ShotType::Candid, vec![], 0.8);
    // Faces turned away.
    let mut away = face(0.3, 0.3, 0.15);
    away.visible = 0.15;
    away.looking = false;
    let turned = s.push(1, 10, mix(20) ^ 0xFFFF_0000_0000, ShotType::Candid, vec![away.clone(), away], 0.8);
    // Dancing: faces only partly visible but lots of action.
    let mut dancers = vec![face(0.2, 0.3, 0.12), face(0.6, 0.3, 0.12)];
    for f in &mut dancers {
        f.visible = 0.35;
        f.expression = 0.9;
    }
    let dance = s.push(2, 900, mix(21), ShotType::Candid, dancers, 0.7);
    s.sig(dance).expression = 0.9;
    let drafts = s.run(100);
    let del = delivered(&drafts);
    assert!(del.contains(&crop));
    assert!(del.contains(&dance), "action with faces visible counts");
    assert_eq!(by_id(&drafts)[&crop].reasons[0].text, "Clear face, would crop well");
    assert_eq!(by_id(&drafts)[&dance].reasons[0].text, "Action with faces visible");
    for id in [back, turned] {
        let d = by_id(&drafts)[&id];
        assert_eq!(d.choice, TargetChoice::SetAside, "{id}");
        assert_eq!(d.reasons[0].kind, TargetReasonKind::NoVisibleFace);
        assert_eq!(d.covered_by, Some(crop));
    }
    assert_eq!(by_id(&drafts)[&back].reasons[0].text, "No face visible (back of head or turned away)");
}

// ---------------------------------------------------------------------------
// Rule 5: important people boost
// ---------------------------------------------------------------------------

#[test]
fn rule5_important_people_boosted_couple_most() {
    let mut s = Shoot::new();
    let guest =
        s.push(1, 0, mix(30), ShotType::Candid, vec![person(face(0.3, 0.3, 0.2), GUEST, PersonRole::Other)], 0.8);
    let mum =
        s.push(2, 900, mix(31), ShotType::Candid, vec![person(face(0.3, 0.3, 0.2), MUM, PersonRole::Important)], 0.7);
    let bride = s.push(
        3,
        1800,
        mix(32),
        ShotType::Candid,
        vec![
            person(face(0.3, 0.3, 0.2), MAIN_A, PersonRole::Main),
            person(face(0.6, 0.3, 0.2), GUEST, PersonRole::Other),
        ],
        0.65,
    );
    // Target 1: the main subject wins despite the lowest score; target 2 adds mum.
    assert_eq!(delivered(&s.run(1)), vec![bride]);
    let two = s.run(2);
    assert_eq!(delivered(&two), vec![mum, bride]);
    let d = by_id(&two)[&mum];
    assert!(d
        .reasons
        .iter()
        .any(|r| r.kind == TargetReasonKind::ImportantPerson && r.text == "Shows an important person"));
    assert!(by_id(&two)[&bride].reasons.iter().any(|r| r.text == "The couple is in it"));
    // Mum's frame is a close call against the guest's? No: the guest is just below the cut.
    assert_eq!(choice_of(&two, guest), TargetChoice::NotSure);
    // Without people (identity unavailable) the better-scored guest frame comes first.
    s.roles.clear();
    for sig in &mut s.signals {
        for f in &mut sig.faces {
            f.person_id = None;
            f.role = None;
        }
        let faces = sig.faces.clone();
        set_faces(sig, faces);
    }
    assert_eq!(delivered(&s.run(1)), vec![guest]);
}

// ---------------------------------------------------------------------------
// Rule 6: next best until the target, never junk; target +/-10%
// ---------------------------------------------------------------------------

#[test]
fn rule6_next_best_fills_to_target_never_junk() {
    let mut s = Shoot::new();
    // 30 venue / scenery moments, each one distinct good frame + one weak frame.
    let mut good = Vec::new();
    let mut weak = Vec::new();
    for m in 0..30u32 {
        let t = m as i64 * 600;
        good.push(s.push(m + 1, t, mix(100 + m as u64), ShotType::Other, vec![], 0.6 + 0.01 * m as f32));
        weak.push(s.push(m + 1, t + 5, mix(100 + m as u64) ^ 0xFFFF_FFFF_0000, ShotType::Other, vec![], 0.2));
    }
    let d10 = s.run(10);
    assert_eq!(delivered(&d10).len(), 10);
    // The best ten (highest overall) are delivered.
    assert!(good[20..].iter().all(|id| delivered(&d10).contains(id)));
    assert!(d10
        .iter()
        .filter(|d| d.choice == TargetChoice::Deliver)
        .all(|d| d.reasons[0].kind == TargetReasonKind::NextBest));
    // A huge target never pads with weak frames.
    let all = s.run(500);
    assert_eq!(delivered(&all).len(), 30);
    assert!(weak.iter().all(|id| choice_of(&all, *id) != TargetChoice::Deliver));
    assert_eq!(by_id(&all)[&weak[0]].reasons[0].text, "Weak frame overall (low score)");
}

#[test]
fn target_is_a_guideline_within_ten_percent() {
    let s = wedding_day(true);
    let qualifying = delivered(&s.run(10_000)).len();
    for target in [20u32, 40, 60, 80] {
        let drafts = s.run(target);
        let n = delivered(&drafts).len() as f32;
        let t = target as f32;
        if (qualifying as f32) >= t * 0.9 {
            assert!(n >= (t * 0.9).floor() && n <= (t * 1.1).floor(), "target {target}: delivered {n}");
        }
        assert!(n as usize <= qualifying);
    }
    // First-of-moment frames may take it slightly over (never beyond +10%).
    let moments_with_core = s
        .run(10_000)
        .iter()
        .filter(|d| d.choice == TargetChoice::Deliver)
        .filter(|d| d.reasons.iter().any(|r| r.text.starts_with("Best")))
        .count();
    let tight = s.run(moments_with_core as u32 - 2);
    assert!(delivered(&tight).len() <= ((moments_with_core as f32 - 2.0) * 1.1).floor() as usize);
}

// ---------------------------------------------------------------------------
// Defects, user decisions, locking
// ---------------------------------------------------------------------------

#[test]
fn defects_and_user_rejects_never_delivered_user_picks_kept() {
    let mut s = Shoot::new();
    let base = mix(40);
    let defect = s.push(1, 0, pose_hash(base, 0, 0), ShotType::Couple, Shoot::couple_faces(0, 0.0, true), 0.95);
    s.frame(defect).scored_pick = PickFlag::Reject;
    s.frame(defect).reasons = vec![SuggestionReason {
        kind: SuggestionReasonKind::Blink,
        text: "Eyes closed".into(),
        related_image_id: None,
    }];
    let rejected = s.push(1, 20, pose_hash(base, 1, 0), ShotType::Couple, Shoot::couple_faces(1, 0.0, true), 0.9);
    s.frame(rejected).user_pick = Some(PickFlag::Reject);
    let picked = s.push(1, 40, pose_hash(base, 2, 0), ShotType::Couple, Shoot::couple_faces(2, 0.0, true), 0.3);
    s.frame(picked).user_pick = Some(PickFlag::Pick);
    let other = s.push(1, 60, pose_hash(base, 3, 0), ShotType::Couple, Shoot::couple_faces(3, 0.0, true), 0.8);
    let drafts = s.run(100);
    assert_eq!(delivered(&drafts), vec![picked, other]);
    let d = by_id(&drafts)[&defect];
    assert_eq!(
        (d.choice, d.reasons[0].kind, d.reasons[0].text.as_str()),
        (TargetChoice::SetAside, TargetReasonKind::Defect, "Eyes closed")
    );
    let d = by_id(&drafts)[&rejected];
    assert_eq!((d.choice, d.reasons[0].text.as_str()), (TargetChoice::SetAside, "You rejected this photo"));
    assert_eq!(by_id(&drafts)[&picked].reasons[0].text, "You picked this photo");
    // User picks count towards the target.
    assert_eq!(delivered(&s.run(1)), vec![picked]);
}

#[test]
fn locked_choices_are_kept_and_count_towards_the_target() {
    let mut s = Shoot::new();
    let base = mix(50);
    let ids: Vec<ImageId> = (0..4u32)
        .map(|p| {
            s.push(
                1,
                p as i64 * 20,
                pose_hash(base, p, 0),
                ShotType::Couple,
                Shoot::couple_faces(p, 0.0, true),
                0.9 - 0.1 * p as f32,
            )
        })
        .collect();
    let free = s.run(100);
    assert_eq!(delivered(&free), ids);
    // The user set the best aside and delivered the weakest; locked as alternative of #2.
    s.locked = vec![
        LockedChoice { image_id: ids[0], choice: TargetChoice::SetAside, alternative_of: None, rank: None },
        LockedChoice { image_id: ids[3], choice: TargetChoice::Deliver, alternative_of: None, rank: None },
        LockedChoice {
            image_id: ids[2],
            choice: TargetChoice::Alternative,
            alternative_of: Some(ids[1]),
            rank: Some(1),
        },
    ];
    let drafts = s.run(2);
    assert_eq!(choice_of(&drafts, ids[0]), TargetChoice::SetAside);
    assert_eq!(choice_of(&drafts, ids[3]), TargetChoice::Deliver);
    assert_eq!(by_id(&drafts)[&ids[2]].alternative_of, Some(ids[1]));
    assert_eq!(delivered(&drafts), vec![ids[1], ids[3]], "locked deliver counts: one more to reach 2");
}

// ---------------------------------------------------------------------------
// Not sure
// ---------------------------------------------------------------------------

#[test]
fn not_sure_for_close_calls_creative_blur_and_borderline_eyes() {
    let mut s = Shoot::new();
    let base = mix(60);
    let sharp = s.push(1, 0, pose_hash(base, 0, 0), ShotType::Couple, Shoot::couple_faces(0, 0.0, true), 0.85);
    // Creative blur, a distinct pose, weaker: competes, loses, asks.
    let blur = s.push(1, 20, pose_hash(base, 1, 0), ShotType::Couple, Shoot::couple_faces(1, 0.0, true), 0.5);
    s.frame(blur).creative_blur = true;
    // Creative blur near-identical to the delivered frame: just an alternative.
    let blur_dup = s.push(1, 1, pose_hash(base, 0, 1), ShotType::Couple, Shoot::couple_faces(0, 0.0, true), 0.6);
    s.frame(blur_dup).creative_blur = true;
    // Borderline eyes on the bride, distinct pose.
    let eyes = s.push(1, 40, pose_hash(base, 2, 0), ShotType::Couple, Shoot::couple_faces(2, 0.0, true), 0.9);
    s.sig(eyes).eyes_borderline = true;
    s.sig(eyes).faces[0].eyes = Eyes::Borderline;
    // Another pose just below the cut.
    let close = s.push(1, 60, pose_hash(base, 3, 0), ShotType::Couple, Shoot::couple_faces(3, 0.0, true), 0.8);
    let drafts = s.run(1);
    assert_eq!(delivered(&drafts), vec![sharp]);
    let r = |id| by_id(&drafts)[&id].reasons[0].text.clone();
    assert_eq!(choice_of(&drafts, blur), TargetChoice::NotSure);
    assert_eq!(choice_of(&drafts, eyes), TargetChoice::NotSure);
    assert_eq!(r(eyes), "Eyes may be half closed: check");
    assert_eq!(choice_of(&drafts, close), TargetChoice::NotSure);
    assert_eq!(r(close), "Close call: just missed the target");
    assert_eq!(choice_of(&drafts, blur_dup), TargetChoice::Alternative);
    // With room for it the creative blur pose is delivered like any other variation.
    let roomy = s.run(10);
    assert_eq!(choice_of(&roomy, blur), TargetChoice::Deliver);
    assert!(by_id(&roomy)[&blur].reasons.iter().any(|r| r.text == "Intentional blur, the best of its moment"));
}

#[test]
fn creative_blur_competes_like_any_frame() {
    let mut s = Shoot::new();
    let base = mix(70);
    // Same pose, near-identical: the creative blur frame scores higher and wins the pose.
    let blur = s.push(1, 0, pose_hash(base, 0, 0), ShotType::Couple, Shoot::couple_faces(0, 0.0, true), 0.8);
    s.frame(blur).creative_blur = true;
    let sharp = s.push(1, 1, pose_hash(base, 0, 1), ShotType::Couple, Shoot::couple_faces(0, 0.0, true), 0.7);
    let drafts = s.run(10);
    assert_eq!(delivered(&drafts), vec![blur]);
    assert_eq!(choice_of(&drafts, sharp), TargetChoice::Alternative);
    // Scored lower, it is the alternative (not kept just for being intentional).
    s.frame(blur).overall = 0.6;
    s.sig(blur).overall = 0.6;
    let drafts = s.run(10);
    assert_eq!(delivered(&drafts), vec![sharp]);
    assert_eq!(choice_of(&drafts, blur), TargetChoice::Alternative);
}

// ---------------------------------------------------------------------------
// The wedding-like day
// ---------------------------------------------------------------------------

/// Getting ready details (4 details x 3, one detail all out of focus), 4 couple portrait
/// moments (4 poses x 4 burst frames), 5 group setups x 6 (setup 2 with a 4-frame activity
/// burst; setup 4 with the couple never looking), reception candids (3 moments x 8, half backs
/// of heads), 3 creative blur frames in the couple moments, 5 venue frames. `people` = identity
/// available.
fn wedding_day(people: bool) -> Shoot {
    let mut s = Shoot::new();
    if !people {
        s.roles.clear();
    }
    let mut key = 0u32;
    let mut t = 0i64;
    let mut next = |gap: i64| {
        key += 1;
        t += gap;
        (key, t)
    };
    // Details.
    for d in 0..4u64 {
        let (k, t0) = next(300);
        for i in 0..3u32 {
            let id = s.push(
                k,
                t0 + i as i64 * 4,
                mix(1000 + d) ^ (0b111 << (i * 3)),
                ShotType::Detail,
                vec![],
                0.7 + 0.05 * i as f32,
            );
            s.sig(id).sharp_object = true;
            s.sig(id).detail_in_focus = Some(d != 3 && i != 2);
        }
    }
    // Couple portraits.
    for m in 0..4u64 {
        let (k, t0) = next(600);
        let base = mix(2000 + m);
        for pose in 0..4u32 {
            for b in 0..4u32 {
                let overall = 0.65 + 0.03 * b as f32 + 0.02 * pose as f32;
                s.push(
                    k,
                    t0 + (pose * 25 + b) as i64,
                    pose_hash(base, pose, b),
                    ShotType::Couple,
                    Shoot::couple_faces(pose, 0.0, people),
                    overall,
                );
            }
        }
        if m < 3 {
            let id = s.push(
                k,
                t0 + 110,
                pose_hash(base, 4, 0) ^ 0xF0_0000_0000_0000,
                ShotType::Couple,
                Shoot::couple_faces(1, 0.03, people),
                0.6,
            );
            s.frame(id).creative_blur = true;
        }
    }
    // Group setups.
    let heads = [8usize, 5, 12, 4, 6];
    for (g, &n) in heads.iter().enumerate() {
        let (k, t0) = next(if g == 0 { 900 } else { 40 });
        s.types.insert(k, ShotType::Group);
        let base = mix(3000 + g as u64);
        for i in 0..6u32 {
            let main_away = g == 3;
            s.push(
                k,
                t0 + i as i64 * 2,
                pose_hash(base, 0, i),
                ShotType::Group,
                Shoot::group_faces(n, people, (i % 3) as usize, main_away),
                0.7 + 0.01 * i as f32,
            );
        }
        if g == 1 {
            for i in 0..4u32 {
                let mut faces = Shoot::group_faces(n, people, n, true);
                for f in &mut faces {
                    f.expression = 0.9;
                    f.bbox.y -= 0.03 * i as f32;
                }
                let id = s.push(k, t0 + 14 + i as i64, pose_hash(base, 1 + i, 0), ShotType::Candid, faces, 0.68);
                s.sig(id).expression = 0.9;
                s.sig(id).motion = 0.7;
            }
        }
    }
    // Reception candids.
    for m in 0..3u64 {
        let (k, t0) = next(1800);
        let base = mix(4000 + m);
        for i in 0..8u32 {
            let faces = if i % 2 == 1 {
                vec![]
            } else {
                let mut f = face(0.2 + 0.07 * i as f32, 0.3, 0.12);
                if people && i == 0 {
                    f = person(f, MUM, PersonRole::Important);
                } else if people {
                    f = person(f, GUEST + i as i64, PersonRole::Other);
                }
                vec![f]
            };
            s.push(
                k,
                t0 + i as i64 * 15,
                pose_hash(base, i % 4, i / 4),
                ShotType::Candid,
                faces,
                0.6 + 0.02 * i as f32,
            );
        }
    }
    // Venue.
    let (k, t0) = next(600);
    for i in 0..5u32 {
        s.push(k, t0 + i as i64 * 10, mix(5000) ^ (0xFFF << (12 * i)), ShotType::Other, vec![], 0.62);
    }
    if people {
        s.roles.extend((10..20).map(|p| (p, PersonRole::Other)));
    }
    s
}

fn summary(drafts: &[SelectionDraft]) -> BTreeMap<String, [u32; 4]> {
    let mut out: BTreeMap<String, [u32; 4]> = BTreeMap::new();
    for d in drafts {
        let col = match d.choice {
            TargetChoice::Deliver => 0,
            TargetChoice::Alternative => 1,
            TargetChoice::NotSure => 2,
            TargetChoice::SetAside => 3,
        };
        out.entry(d.shot_type.map_or("none", |s| s.as_str()).to_owned()).or_default()[col] += 1;
        out.entry("total".into()).or_default()[col] += 1;
    }
    out
}

#[test]
fn wedding_day_follows_every_rule() {
    let s = wedding_day(true);
    let total = s.frames.len();
    let all = s.run(10_000);
    let full = summary(&all);
    println!("wedding day ({total} frames), target 10000 [deliver, alternative, not_sure, set_aside]: {full:?}");
    // Couple: 4 moments x 4 poses + 3 creative blur variations; one per near-identical burst.
    assert_eq!(full["couple"][0], 19);
    // Groups: one per setup except the one where the couple never looks (asks instead), plus
    // the activity extras.
    let group_best = all
        .iter()
        .filter(|d| d.choice == TargetChoice::Deliver && d.reasons[0].kind == TargetReasonKind::GroupBest)
        .count();
    assert_eq!(group_best, 4);
    let activity = all
        .iter()
        .filter(|d| d.choice == TargetChoice::Deliver && d.reasons[0].kind == TargetReasonKind::GroupActivity)
        .count();
    assert_eq!(activity, MAX_ACTIVITY as usize);
    // Details: three in focus, one asks.
    let detail_best = all
        .iter()
        .filter(|d| d.choice == TargetChoice::Deliver && d.reasons[0].kind == TargetReasonKind::DetailBest)
        .count();
    assert_eq!(detail_best, 3);
    // Candids: backs of heads never delivered.
    for d in all.iter().filter(|d| {
        d.reasons.iter().any(|r| r.kind == TargetReasonKind::NoVisibleFace) && d.shot_type == Some(ShotType::Candid)
    }) {
        assert_ne!(d.choice, TargetChoice::Deliver);
    }
    let candid_del = full["candid"][0];
    assert!(candid_del >= 3, "{candid_del}");

    let qualifying = full["total"][0];
    for target in [30u32, 40, 60] {
        let drafts = s.run(target);
        let sum = summary(&drafts);
        println!("wedding day, target {target}: {sum:?}");
        let n = sum["total"][0] as f32;
        if qualifying as f32 >= target as f32 * 0.9 {
            assert!(n >= (target as f32 * 0.9).floor() && n <= (target as f32 * 1.1).floor(), "target {target}: {n}");
        } else {
            assert_eq!(n as u32, qualifying, "never padded beyond the qualifying frames");
        }
        // Every first-of-moment group / detail survives the tighter target.
        let gb = drafts
            .iter()
            .filter(|d| d.choice == TargetChoice::Deliver && d.reasons[0].kind == TargetReasonKind::GroupBest)
            .count();
        assert_eq!(gb, 4);
    }
}

#[test]
fn wedding_day_is_deterministic_and_input_order_independent() {
    let s = wedding_day(true);
    let a = s.run(50);
    let mut shuffled = s.frames.clone();
    shuffled.reverse();
    shuffled.rotate_left(17);
    let mut input = s.input(50);
    input.plan.frames.reverse();
    input.plan.moments.reverse();
    let b = choose(&shuffled, &input);
    assert_eq!(a, b);
    assert_eq!(a, s.run(50));
}

#[test]
fn identity_absent_fallback_still_sensible() {
    // Same day without people, moments rebuilt by `group_moments` from time + similarity.
    let mut s = wedding_day(false);
    for sig in &mut s.signals {
        sig.moment_key = None;
    }
    s.types.clear();
    // Frame shot types the way `frame_signals` derives them without people, then moments.
    let people = crate::ml::moments::PeopleContext::default();
    for sig in &mut s.signals {
        sig.shot_type = crate::ml::moments::classify_shot(sig, &people, ShootType::Wedding);
    }
    let moments = crate::ml::moments::group_moments(&mut s.signals);
    let types: BTreeMap<&str, usize> = moments.iter().fold(BTreeMap::new(), |mut m, d| {
        *m.entry(d.shot_type.as_str()).or_default() += 1;
        m
    });
    println!("identity absent: {} moments {types:?}", moments.len());
    assert!(moments.len() >= 20 && moments.len() <= 50, "{}", moments.len());
    let drafts = s.run(60);
    let sum = summary(&drafts);
    println!("identity absent, target 60: {sum:?}");
    assert!(sum["couple"][0] >= 12, "couple variations still found");
    assert!(sum.get("group").is_some_and(|g| g[0] >= 4), "a frame per group setup");
    let n = sum["total"][0] as f32;
    let qualifying = delivered(&s.run(10_000)).len() as f32;
    assert!(n <= 66.0 && (n >= 54.0 || n == qualifying), "{n} of {qualifying}");
    // Backs of heads are never delivered, people known or not.
    for d in &drafts {
        let sig = s.signals.iter().find(|x| x.image_id == d.image_id).unwrap();
        let venue = sig.image_id > s.signals.len() as ImageId - 5;
        if sig.shot_type != ShotType::Detail && sig.subjects == 0 && !venue {
            assert_ne!(d.choice, TargetChoice::Deliver, "no-face frame {} delivered", d.image_id);
        }
    }
    // Without the couple known, no group is held back for "main not looking".
    assert!(drafts.iter().all(|d| !d.reasons.iter().any(|r| r.text.contains("isn't looking"))));
}

// ---------------------------------------------------------------------------
// Catalog pipeline
// ---------------------------------------------------------------------------

mod catalog {
    use super::*;
    use crate::ipc::types::{ExposureStats, FaceInfo, NormPoint, NormRect};
    use crate::ml::{FaceMetrics, HighlightStats, ImageMetrics, TileStats};
    use rusqlite::params;

    fn fm(x: f32, h: f32) -> FaceMetrics {
        FaceMetrics {
            bbox: NormRect { x, y: 0.2, width: h * 0.75, height: h },
            left_eye: NormPoint { x, y: 0.3 },
            right_eye: NormPoint { x: x + 0.1, y: 0.3 },
            detection_score: 0.9,
            ear: Some(0.3),
            sharpness: 0.7,
            ear_left: Some(0.3),
            ear_right: Some(0.3),
            mouth_open: Some(0.02),
            yaw: 0.0,
            iod_px: 60.0,
            face_sharpness: 0.7,
            eye_texture: 10.0,
            anisotropy: 0.05,
            frontal: true,
            truncated: false,
            eye_open_prob: Some(0.9),
            head_pitch: Some(0.0),
            head_yaw: Some(0.0),
            mesh_ear: Some(0.3),
            face_luma: 0.5,
            mouth_width: Some(0.5),
            blown: 0.0,
        }
    }

    /// Project 1: photos 1..=6 analysed (a couple burst 1-3, a different pose 4, a reject 5, a
    /// no-face soft frame 6), photo 7 analysis failed, photo 8 without analysis.
    fn setup() -> Connection {
        let conn = crate::db::open_in_memory();
        conn.execute_batch(
            "INSERT INTO projects (id, name, shoot_type, created_at) VALUES (1, 'W', 'wedding', 0);
             INSERT INTO folders (id, path, added_at, project_id) VALUES (1, '/w', 0, 1);",
        )
        .unwrap();
        for id in 1..=8i64 {
            conn.execute(
                "INSERT INTO images (id, folder_id, path, file_name, format, camera_make, file_size, file_mtime_ms,
                                     imported_at, captured_at_ms)
                 VALUES (?1, 1, ?2, ?3, 'arw', 'sony', 1, 0, 0, ?4)",
                params![id, format!("/w/DSC{id:04}.ARW"), format!("DSC{id:04}.ARW"), id * 1000],
            )
            .unwrap();
        }
        for id in 1..=6i64 {
            let faces = match id {
                1..=3 => vec![fm(0.3, 0.22), fm(0.55, 0.21)],
                4 => vec![fm(0.1, 0.15), fm(0.7, 0.16)],
                5 => vec![fm(0.3, 0.22), fm(0.55, 0.21)],
                _ => vec![],
            };
            let phash: u64 = match id {
                1..=3 => 0x0F0F_0F0F_0F0F_0F0F ^ (id as u64 - 1),
                4 => 0x0F0F_0F0F_0F0F_0F0F ^ 0xFFF_F000,
                5 => 0x0F0F_0F0F_0F0F_0F0F ^ 0xF0,
                _ => 0xF0F0_F0F0_F0F0_F0F0,
            };
            let infos: Vec<FaceInfo> = faces
                .iter()
                .map(|f| FaceInfo {
                    bbox: f.bbox,
                    left_eye: f.left_eye,
                    right_eye: f.right_eye,
                    detection_score: 0.9,
                    ear: f.ear,
                    eyes_open: Some(0.9),
                    sharpness: 0.7,
                    blink: false,
                    in_focus: true,
                    primary: false,
                    considered: true,
                })
                .collect();
            let m = ImageMetrics {
                width: 2048,
                height: 1365,
                faces,
                global_sharpness: if id == 6 { 0.3 } else { 0.8 },
                exposure: ExposureStats { clipped_highlights_pct: 0.0, clipped_shadows_pct: 0.0, mean_luma: 0.4 },
                phash,
                tiles: TileStats { p90: if id == 6 { 0.3 } else { 0.8 }, p50: 0.5, textured: 0.8, anisotropy: 0.05 },
                highlights: HighlightStats::default(),
            };
            conn.execute(
                "INSERT INTO image_analysis (image_id, status, model_version, analyzed_at, phash, faces_json, metrics_json)
                 VALUES (?1, 'done', ?2, 0, ?3, ?4, ?5)",
                params![id, MODEL_VERSION, phash as i64, serde_json::to_string(&infos).unwrap(), serde_json::to_string(&m).unwrap()],
            )
            .unwrap();
            let scored = if id == 5 { "reject" } else { "unflagged" };
            let reasons = if id == 5 { r#"[{"kind":"blink","text":"Eyes closed"}]"# } else { "[]" };
            conn.execute(
                "INSERT INTO quality_scores (image_id, overall, global_sharpness, clipped_highlights_pct,
                                             clipped_shadows_pct, mean_luma, model_version, analyzed_at,
                                             suggested_pick, scored_pick, reasons_json)
                 VALUES (?1, ?2, 0.8, 0, 0, 0.4, ?3, 0, ?4, ?4, ?5)",
                params![id, if id == 6 { 0.3 } else { 0.6 + 0.05 * id as f64 }, MODEL_VERSION, scored, reasons],
            )
            .unwrap();
        }
        for id in 1..=8i64 {
            let (status, preview) =
                if id == 8 { ("pending", None) } else { ("ready", Some(format!("/nonexistent/{id}.jpg"))) };
            conn.execute(
                "INSERT INTO thumbnails (image_id, status, preview_path) VALUES (?1, ?2, ?3)",
                params![id, status, preview],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO image_analysis (image_id, status, model_version, analyzed_at, error) VALUES (7, 'failed', ?1, 0, 'decode failed')",
            [MODEL_VERSION],
        )
        .unwrap();
        conn
    }

    #[test]
    fn pipeline_selects_and_stores_with_reasons() {
        let mut conn = setup();
        let job = TargetJob { project_id: 1, target_count: 10, shoot_type: ShootType::Wedding };
        let cancel = AtomicBool::new(false);
        let mut phases = Vec::new();
        let out =
            run_pipeline(&mut conn, Path::new("/nonexistent"), &job, &cancel, &mut |l, _, _| phases.push(l.to_owned()))
                .unwrap();
        assert!(!out.cancelled);
        assert!(out.model_version.ends_with(SELECTION_VERSION));
        let msg = out.message.unwrap();
        assert!(msg.starts_with("Picked 2 of 8 photos (2 not analysed"), "{msg}");
        assert!(phases.iter().any(|p| p.starts_with("Choosing the best 10")));
        let sel = target::selections(&conn, &(1..=8).collect::<Vec<_>>()).unwrap();
        let by: HashMap<ImageId, _> = sel.iter().map(|s| (s.image_id, s)).collect();
        assert_eq!(by.len(), 8);
        // The burst: best (3) delivered, 1 / 2 its alternatives; another pose (4) delivered.
        assert_eq!(by[&3].choice, TargetChoice::Deliver);
        assert_eq!(by[&4].choice, TargetChoice::Deliver);
        for id in [1, 2] {
            assert_eq!(by[&id].choice, TargetChoice::Alternative);
            assert_eq!(by[&id].alternative_of, Some(3));
            assert_eq!(by[&id].covered_by, Some(3));
        }
        assert_eq!(by[&5].choice, TargetChoice::SetAside);
        assert_eq!(by[&5].reasons[0].text, "Eyes closed");
        assert_eq!(by[&7].choice, TargetChoice::NotSure);
        assert_eq!(by[&7].reasons[0].text, "Sieve could not analyse this photo: decode failed");
        assert_eq!(by[&8].reasons[0].kind, TargetReasonKind::NotAnalyzed);
        // The overlay: delivered photos are suggested picks, the defect keeps its reject.
        let pick: String =
            conn.query_row("SELECT suggested_pick FROM quality_scores WHERE image_id = 3", [], |r| r.get(0)).unwrap();
        assert_eq!(pick, "pick");
        let rej: String =
            conn.query_row("SELECT suggested_pick FROM quality_scores WHERE image_id = 5", [], |r| r.get(0)).unwrap();
        assert_eq!(rej, "reject");
        assert_eq!(pending_analysis(&conn, 1).unwrap(), 1, "photo 8 has no thumbnail yet");
        // Re-running keeps the user's lock.
        target::note_user_flags(&mut conn, &[1], PickFlag::Pick).unwrap();
        run_pipeline(&mut conn, Path::new("/x"), &job, &cancel, &mut |_, _, _| {}).unwrap();
        assert_eq!(target::selection(&conn, 1).unwrap().unwrap().choice, TargetChoice::Deliver);
        cancel.store(true, Ordering::SeqCst);
        assert!(run_pipeline(&mut conn, Path::new("/x"), &job, &cancel, &mut |_, _, _| {}).unwrap().cancelled);
    }

    #[test]
    fn empty_project_reports_nothing_to_choose() {
        let mut conn = crate::db::open_in_memory();
        conn.execute("INSERT INTO projects (id, name, shoot_type, created_at) VALUES (1, 'P', 'wedding', 0)", [])
            .unwrap();
        let job = TargetJob { project_id: 1, target_count: 800, shoot_type: ShootType::Wedding };
        let out = run_pipeline(&mut conn, Path::new("/x"), &job, &AtomicBool::new(false), &mut |_, _, _| {}).unwrap();
        assert!(out.message.unwrap().starts_with("No photos to choose from yet"));
        assert_eq!(target::counts(&conn, 1).unwrap().total, 0);
    }
}

/// Timing on a 2,556-frame shoot (18 wedding days): `cargo test --release --lib perf_ -- --ignored --nocapture`.
#[test]
#[ignore]
fn perf_wedding_size_shoot() {
    let mut s = Shoot::new();
    for rep in 0..18i64 {
        let day = wedding_day(true);
        for (f, sig) in day.frames.iter().zip(&day.signals) {
            let id = s.next_id;
            s.next_id += 1;
            let mut sig = sig.clone();
            sig.image_id = id;
            sig.moment_key = sig.moment_key.map(|k| k + rep as u32 * 100);
            sig.captured_at_ms = sig.captured_at_ms.map(|t| t + rep * 100_000_000);
            sig.phash = sig.phash.map(|h| h ^ mix(rep as u64 + 77));
            let mut f = f.clone();
            f.image_id = id;
            f.phash = sig.phash;
            f.captured_at_ms = sig.captured_at_ms;
            s.frames.push(f);
            s.signals.push(sig);
        }
        for (k, t) in &day.types {
            s.types.insert(k + rep as u32 * 100, *t);
        }
    }
    let mut ungrouped = s.signals.clone();
    for x in &mut ungrouped {
        x.moment_key = None;
    }
    let t = std::time::Instant::now();
    let moments = crate::ml::moments::group_moments(&mut ungrouped);
    let t_group = t.elapsed();
    let input = s.input(800);
    let t = std::time::Instant::now();
    let drafts = choose(&s.frames, &input);
    let t_choose = t.elapsed();
    println!(
        "{} frames: group_moments {t_group:?} ({} moments), choose {t_choose:?}, delivered {}",
        s.frames.len(),
        moments.len(),
        delivered(&drafts).len()
    );
}
