//! Target-count culling contract (Phase 9, IPC v20). Re-exported from [`crate::ipc::types`];
//! same conventions (camelCase fields, snake_case enum values identical to the catalog's).
//!
//! The user asks for "the best N" of a shoot (a guideline, not a cap). One run per project:
//! 1. **People** (`ml::identity`): faces are embedded and clustered into [`Person`]s per
//!    project; the main subject (usually the couple) is found automatically and other
//!    recurring people are asked about ("Is this person important?", [`PeopleOverview::questions`]).
//! 2. **Moments** (`ml::moments`): frames are grouped across the shoot into [`Moment`]s (same
//!    scene + people) with a [`ShotType`].
//! 3. **Selection** (`ml::selection`): every analysed photo of the project gets an
//!    [`ImageSelection`]: `deliver` (the delivery set), `alternative` (ranked under one
//!    delivered photo of its moment), `not_sure` or `set_aside`, with reasons and a
//!    "covered by" link to the nearest delivered similar photo.
//!
//! Flags: the selection is the project's **suggestion** while the run exists:
//! `quality_scores.suggested_pick` = `pick` for `deliver`, else the scorer's own suggestion
//! with `pick` dropped (confident-defect rejects of the project's `RejectStrictness` stay).
//! `apply_target_selection` writes it to the flags (origin `auto`); the user's own flags always
//! win and lock the photo's choice (see `docs/architecture.md`, "Target-count culling").
//!
//! v20.1 (UX review 9): target edits (keep, swap, add, set aside, not sure) write the flags
//! of the photos they touch at once, as the user's; `apply_target_selection` flags the rest
//! (dry run: `plan_target_apply`) and returns `CullSnapshot`s for one-step undo. `covered_by`
//! is recomputed within the moment after every edit (frame similarities stored per run),
//! with a similarity tier and who delivered the covering photo.
//!
//! v20.2: undo / redo of an apply goes through `restore_target_apply`, which also restores
//! the run's `applied_at` (`TargetApplyResult.previous_applied_at_ms`).

use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

use super::types::{string_enum, CullSnapshot, ImageId, NormRect, ProjectId, ShootType};

/// Catalog row id of a person (face identity cluster of one project, IPC v20).
pub type PersonId = i64;
/// Catalog row id of a moment (cross-shoot group of frames, IPC v20).
pub type MomentId = i64;

/// Largest accepted `TargetRunSettings.targetCount`.
pub const MAX_TARGET_COUNT: u32 = 100_000;

// ---------------------------------------------------------------------------
// People
// ---------------------------------------------------------------------------

string_enum! {
    /// How much a person matters for the selection (IPC v20). Photos with important people
    /// are favoured, the main subject most.
    pub enum PersonRole {
        /// The main subject (the couple: up to 2 people; a portrait: 1).
        Main => "main",
        /// Recurring person the user said is important (parents, siblings, ...).
        Important => "important",
        /// Recurring person the user said is not important, or a guest.
        Other => "other",
        /// Not decided (the engine had no opinion and the user was not asked / did not answer).
        Unknown => "unknown",
    }
}

/// One face to show for a person: a crop of the photo's existing loupe preview. No face files
/// are written: show `previewPath` (via `convertFileSrc`, like `ThumbnailState.previewPath`)
/// cropped to `crop` (TS helper `faceCropStyle` in `src/ipc/index.ts`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FaceSample {
    pub image_id: ImageId,
    /// Index into the image's faces (`get_faces(imageId)` order = `image_analysis.faces_json`).
    pub face_index: u32,
    /// The detected face box (`FaceInfo.bbox` frame: normalized preview, orientation applied).
    pub bbox: NormRect,
    /// Square (in pixels) padded crop around the face, inside 0..=1, same frame as `bbox`.
    pub crop: NormRect,
    /// Width / height of the preview (orientation applied), to size the crop box.
    #[specta(type = Number)]
    pub image_aspect: f32,
    /// Absolute path of the 2048 px preview (`null` while it is missing).
    pub preview_path: Option<String>,
}

/// A face identity cluster of one project (IPC v20).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub id: PersonId,
    pub project_id: ProjectId,
    /// Effective role: the user's answer when `roleConfirmed`, else `suggestedRole`.
    pub role: PersonRole,
    /// The user set the role (`set_person_role`); re-runs never change it.
    pub role_confirmed: bool,
    /// The engine's guess (main pair = most frequent faces appearing together, large and central).
    pub suggested_role: PersonRole,
    /// The engine wants to ask the user about this person (recurring, not main). Questions still
    /// open = `ask && !roleConfirmed`.
    pub ask: bool,
    /// Photos of the project showing this person.
    pub photo_count: u32,
    /// Faces assigned to this person (>= photoCount when a person appears twice, e.g. mirrors).
    pub face_count: u32,
    /// Up to 6 faces, best first (`samples[0]` = the representative face for the people grid).
    pub samples: Vec<FaceSample>,
}

/// `list_people` result (IPC v20).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PeopleOverview {
    pub project_id: ProjectId,
    /// Main people first, then important, then by `photoCount` (descending), then id.
    pub people: Vec<Person>,
    /// "Is this person important?" questions still open (`ask && !roleConfirmed`), in asking
    /// order (most photos first).
    pub questions: Vec<PersonId>,
    /// Face identity model of the stored embeddings; `null` = face identity not available (no
    /// model / never run): `people` is empty and the selection runs without people.
    pub model_version: Option<String>,
    /// User-facing note (e.g. "Face recognition is not available yet"), else `null`.
    pub message: Option<String>,
}

// ---------------------------------------------------------------------------
// Moments
// ---------------------------------------------------------------------------

string_enum! {
    /// What kind of photo a frame / moment is (IPC v20); drives the selection rules.
    pub enum ShotType {
        /// The main pair (or the portrait subject) dominates: keep many variations.
        Couple => "couple",
        /// >= 3 faces, posed: one per group setup (plus activity / reaction frames).
        Group => "group",
        /// No face, a sharp salient object (rings, dress, decor): one per detail.
        Detail => "detail",
        /// Faces not posed (guests, dancing, reception): keep when a face / action is visible.
        Candid => "candid",
        Other => "other",
    }
}

string_enum! {
    /// Who made a photo's current selection choice (IPC v20.1).
    pub enum ChoiceOrigin {
        /// The engine (a target run).
        Engine => "engine",
        /// The user: a target edit (swap, add, set aside, not sure) or their own flag changed
        /// the choice. Locking a choice without changing it (keep, review lock) keeps the origin.
        User => "user",
    }
}

/// Similarity from which two frames count as "almost identical" ([`SimilarityTier`]).
pub const SIMILARITY_NEAR_IDENTICAL: f32 = 0.9;
/// Similarity from which two frames count as "very similar" ([`SimilarityTier`]).
pub const SIMILARITY_VERY_SIMILAR: f32 = 0.7;

string_enum! {
    /// How close a photo is to the delivered photo covering it (IPC v20.1), for the wording
    /// "Almost identical to DSC0412 (kept)" / "Similar to …" / "Same moment as …" / "Looks
    /// like … (kept, another moment)".
    pub enum SimilarityTier {
        /// Same moment, similarity >= [`SIMILARITY_NEAR_IDENTICAL`] (0.9).
        NearIdentical => "near_identical",
        /// Same moment, similarity >= [`SIMILARITY_VERY_SIMILAR`] (0.7).
        VerySimilar => "very_similar",
        /// Same moment, less similar.
        SameMoment => "same_moment",
        /// The covering photo is in another moment.
        AnotherMoment => "another_moment",
    }
}

impl SimilarityTier {
    pub fn of(similarity: f32, same_moment: bool) -> Self {
        if !same_moment {
            SimilarityTier::AnotherMoment
        } else if similarity >= SIMILARITY_NEAR_IDENTICAL {
            SimilarityTier::NearIdentical
        } else if similarity >= SIMILARITY_VERY_SIMILAR {
            SimilarityTier::VerySimilar
        } else {
            SimilarityTier::SameMoment
        }
    }
}

string_enum! {
    /// Second-look pile of a `not_sure` / `set_aside` photo (IPC v20.1). Computed by the
    /// backend, first match wins:
    /// 1. `not_sure`: choice `not_sure`;
    /// 2. `defects`: `set_aside` and Apply would reject it (the effective suggestion is
    ///    `reject`: a confident defect) or the user rejected it;
    /// 3. `similar`: `set_aside` with first reason `near_duplicate` / `not_best_of_setup`, or
    ///    covered by a delivered photo at similarity >= 0.7;
    /// 4. `weaker`: every other `set_aside` photo (stays unflagged at Apply).
    pub enum TargetPile {
        NotSure => "not_sure",
        Similar => "similar",
        Weaker => "weaker",
        Defects => "defects",
    }
}

/// Photos per second-look pile ([`TargetPile`]); `notSure + similar + weaker + defects =
/// TargetCounts.notSure + TargetCounts.setAside`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TargetPileCounts {
    pub not_sure: u32,
    pub similar: u32,
    pub weaker: u32,
    pub defects: u32,
}

/// A group of frames of the same scene and people across the shoot (IPC v20).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Moment {
    pub id: MomentId,
    pub project_id: ProjectId,
    /// Dominant shot type of the frames.
    pub shot_type: ShotType,
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
    /// Members in capture order (at least 1).
    pub image_ids: Vec<ImageId>,
    /// People seen in the moment (most frames first).
    pub person_ids: Vec<PersonId>,
    /// Members chosen for delivery, in capture order.
    pub delivered_ids: Vec<ImageId>,
    /// v20.1: the members of `deliveredIds` the user put there (origin `user`: added, swapped
    /// in, picked), in capture order.
    pub user_delivered_ids: Vec<ImageId>,
    /// Best frame (the cover of the moment), if any.
    pub representative_id: Option<ImageId>,
}

// ---------------------------------------------------------------------------
// Target run
// ---------------------------------------------------------------------------

/// What the user asked for (IPC v20).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TargetRunSettings {
    /// How many photos to deliver (a guideline: the result may be ~10% off when the moments
    /// call for it). 1..=`MAX_TARGET_COUNT`.
    pub target_count: u32,
    /// `null` = the project's shoot type. `TargetRun.settings` always carries the resolved one.
    pub shoot_type: Option<ShootType>,
}

impl TargetRunSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.target_count == 0 || self.target_count > MAX_TARGET_COUNT {
            return Err(format!("targetCount {} is outside 1..={MAX_TARGET_COUNT}", self.target_count));
        }
        Ok(())
    }
}

string_enum! {
    /// Lifecycle of a project's target run (IPC v20).
    pub enum TargetRunState {
        Running => "running",
        Finished => "finished",
        /// Ended by an error (`TargetRun.message`); the previous selection is kept.
        Failed => "failed",
        /// Stopped by `cancel_target_selection` (or the app quit); the previous selection is kept.
        Cancelled => "cancelled",
    }
}

string_enum! {
    /// Choice of the selection for one photo (IPC v20).
    pub enum TargetChoice {
        /// In the delivery set (suggested pick).
        Deliver => "deliver",
        /// A ranked alternative to one delivered photo of the same moment (`alternativeOf`).
        Alternative => "alternative",
        /// Borderline: reviewed in pass 2.
        NotSure => "not_sure",
        /// Not chosen (near-duplicate, weaker variation, defect, no visible face ...). Never a
        /// reject by itself: only confident defects keep their reject suggestion.
        SetAside => "set_aside",
    }
}

string_enum! {
    /// Category of a [`TargetReason`] (IPC v20).
    pub enum TargetReasonKind {
        /// A variation of the main pair (pose / angle / expression).
        CoupleVariation => "couple_variation",
        /// Best frame of a group setup (most faces looking, main subject looking).
        GroupBest => "group_best",
        /// Group activity / reaction frame kept as an extra variation.
        GroupActivity => "group_activity",
        /// Best frame of a detail (focus on the object).
        DetailBest => "detail_best",
        /// Candid with the subject's face / action clearly visible (also as a crop).
        CandidVisible => "candid_visible",
        /// Shows people the user marked important (or the main subject).
        ImportantPerson => "important_person",
        /// Next best frame, added to reach the target.
        NextBest => "next_best",
        /// Almost the same as the delivered `relatedImageId`.
        NearDuplicate => "near_duplicate",
        /// Another frame of the same group setup / detail is better (`relatedImageId`).
        NotBestOfSetup => "not_best_of_setup",
        /// No visible face (back of the head, face hidden).
        NoVisibleFace => "no_visible_face",
        /// Detail shot whose focus is not on the object.
        DetailOutOfFocus => "detail_out_of_focus",
        /// A defect (the scorer's reason, e.g. eyes closed, missed focus).
        Defect => "defect",
        /// Good, but the target was reached by better frames.
        BelowTarget => "below_target",
        /// The user decided (flag, swap, add, set aside).
        UserChoice => "user_choice",
        /// Not analysed yet.
        NotAnalyzed => "not_analyzed",
        Other => "other",
    }
}

/// One user-facing reason behind a choice (IPC v20), e.g.
/// `{kind: "near_duplicate", text: "Almost the same as DSC0412", relatedImageId: 412}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TargetReason {
    pub kind: TargetReasonKind,
    /// Short, sentence case, no trailing period (same style as `SuggestionReason.text`).
    pub text: String,
    #[serde(default)]
    pub related_image_id: Option<ImageId>,
}

/// The selection state of one photo (IPC v20).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImageSelection {
    pub image_id: ImageId,
    pub choice: TargetChoice,
    pub moment_id: Option<MomentId>,
    /// The frame's own shot type (the moment's is its dominant one).
    pub shot_type: Option<ShotType>,
    /// `alternative` only: the delivered photo this is an alternative to.
    pub alternative_of: Option<ImageId>,
    /// `alternative` only: 1 = best alternative of `alternativeOf`.
    pub rank: Option<u32>,
    /// Non-delivered photos: the nearest delivered similar photo ("already kept a similar
    /// one"); `null` when nothing similar was delivered.
    pub covered_by: Option<ImageId>,
    /// Similarity to `coveredBy`, 0..=1 (1 = identical).
    #[specta(type = Option<Number>)]
    pub covered_similarity: Option<f32>,
    /// v20.1: tier of `coveredSimilarity` (`null` without `coveredBy`).
    pub covered_tier: Option<SimilarityTier>,
    /// Selection priority 0..=1 (higher = chosen earlier).
    #[specta(type = Number)]
    pub score: f32,
    /// Most important first.
    pub reasons: Vec<TargetReason>,
    /// The user decided (target edit or own flag, or `lock_target_choices`): re-runs keep
    /// the choice.
    pub locked: bool,
    /// v20.1: who made the current choice.
    pub origin: ChoiceOrigin,
    /// v20.1: second-look pile (`not_sure` / `set_aside` photos only, else `null`).
    pub pile: Option<TargetPile>,
    /// People recognised in the photo.
    pub person_ids: Vec<PersonId>,
}

/// Per shot type counts of a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ShotTypeCount {
    pub shot_type: ShotType,
    pub total: u32,
    pub deliver: u32,
}

/// Counts over a project's selection rows (IPC v20). `total = deliver + alternative + notSure
/// + setAside`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TargetCounts {
    pub total: u32,
    pub deliver: u32,
    pub alternative: u32,
    pub not_sure: u32,
    pub set_aside: u32,
    /// Rows the user decided (`ImageSelection.locked`).
    pub locked: u32,
    /// Shot types present, in `ShotType` order.
    pub per_shot_type: Vec<ShotTypeCount>,
    /// v20.1: second-look piles.
    pub piles: TargetPileCounts,
}

/// A project's latest target run (IPC v20).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TargetRun {
    pub project_id: ProjectId,
    /// `shootType` resolved (never `null`).
    pub settings: TargetRunSettings,
    pub state: TargetRunState,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
    /// Last `apply_target_selection` (flags written), `null` = never applied.
    pub applied_at_ms: Option<i64>,
    /// User-facing summary or error, e.g. "Picked 812 of 2,512 photos"; `null` while running.
    pub message: Option<String>,
    /// Engine versions (identity / moments / selection) that produced the selection.
    pub model_version: String,
    /// Over the current selection rows (a failed / cancelled run keeps the previous ones).
    pub counts: TargetCounts,
    /// Open "Is this person important?" questions (`PeopleOverview.questions.length`).
    pub people_questions: u32,
}

/// `get_alternatives` result (IPC v20).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Alternatives {
    /// The image asked about.
    pub image_id: ImageId,
    /// The delivered photo the strip belongs to: the image itself when delivered, its
    /// `alternativeOf` when an alternative, else `null` (no strip).
    pub delivered: Option<ImageSelection>,
    /// Alternatives of `delivered`, by rank (empty when `delivered` is `null`).
    pub alternatives: Vec<ImageSelection>,
}

/// `get_covered_by` result (IPC v20): "already kept a similar one". v20.1: recomputed within
/// the moment after every edit (the nearest delivered frame of the same moment, including
/// photos the user added), with a tier and the covering photo's origin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CoveredBy {
    pub image_id: ImageId,
    /// The delivered similar photo.
    pub covered_by_id: ImageId,
    /// v20.1: its file name without extension, e.g. "DSC0412".
    pub covered_by_name: String,
    #[specta(type = Number)]
    pub similarity: f32,
    /// v20.1: wording tier of `similarity` / `sameMoment`.
    pub tier: SimilarityTier,
    /// Both photos are in the same moment.
    pub same_moment: bool,
    /// v20.1: who delivered `coveredById` (`user` = the user added / swapped / picked it).
    pub covered_by_origin: ChoiceOrigin,
    /// User-facing, by tier: "Almost identical to DSC0412 (kept)", "Similar to DSC0412
    /// (kept)", "Same moment as DSC0412 (kept)", "Looks like DSC0412 (kept, another moment)";
    /// "kept" reads "you added it" when `coveredByOrigin` is `user`.
    pub text: String,
}

/// Undo record of a target edit: the selection row and the flags of one photo before it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TargetSnapshot {
    pub image_id: ImageId,
    pub choice: TargetChoice,
    pub alternative_of: Option<ImageId>,
    pub rank: Option<u32>,
    pub covered_by: Option<ImageId>,
    /// v20.1.
    #[specta(type = Option<Number>)]
    pub covered_similarity: Option<f32>,
    pub locked: bool,
    /// v20.1.
    pub origin: ChoiceOrigin,
    /// v20.1: restored too (an undone add loses its "You added this").
    pub reasons: Vec<TargetReason>,
    pub cull: CullSnapshot,
}

/// Result of a target edit (`swap_alternative`, `add_alternative`, `set_target_choice`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TargetEditResult {
    /// Selection rows that changed (now), incl. re-ranked / re-pointed siblings.
    pub changed: Vec<ImageSelection>,
    /// Images whose flag changed (refetch with `get_images`).
    pub flags_changed: Vec<ImageId>,
    /// Their state before the edit (`restore_target_snapshot` undoes it).
    pub previous: Vec<TargetSnapshot>,
    pub counts: TargetCounts,
}

/// `plan_target_apply` / `apply_target_selection` options (IPC v20.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TargetApplyOptions {
    /// Also reject the photos Sieve is confident are defects (the effective suggestion is
    /// `reject`). The dialog shows it as a checkbox, on by default. `false`: those photos are
    /// left as they are, except that a stale Sieve pick on them is cleared.
    pub rejects: bool,
}

impl Default for TargetApplyOptions {
    fn default() -> Self {
        Self { rejects: true }
    }
}

/// `plan_target_apply` result: exactly what `apply_target_selection` with the same options
/// would do now (IPC v20.1). Over the project's photos with a selection row:
/// `total = picks + rejects + unflags + unchanged + userFlagged`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TargetApplyPlan {
    pub total: u32,
    /// Photos that would get the Picked flag.
    pub picks: u32,
    /// Photos that would be rejected (0 with `rejects: false`).
    pub rejects: u32,
    /// Photos `rejects: true` would reject, whatever the options (confident defects not
    /// rejected yet), for the checkbox label "Also reject N photos with clear defects".
    pub rejectable: u32,
    /// Photos that would lose a flag Sieve set earlier (a stale pick / reject).
    pub unflags: u32,
    /// Unflagged or Sieve-flagged photos already matching the selection.
    pub unchanged: u32,
    /// Photos the user flagged (pick or reject, incl. by target edits): never touched.
    pub user_flagged: u32,
}

/// `apply_target_selection` result (IPC v20.1; was `ApplySuggestionsResult`). Picks and
/// rejects are counted separately; same fields as the plan.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TargetApplyResult {
    pub picks: u32,
    pub rejects: u32,
    pub unflags: u32,
    pub unchanged: u32,
    pub user_flagged: u32,
    /// Images whose flag changed (refetch with `get_images`).
    pub changed: Vec<ImageId>,
    /// Their flags before the apply: push on the Cull undo stack ("Apply Pick the best N") and
    /// undo with `restore_target_apply(projectId, previous, previousAppliedAtMs)` (v20.2; apply
    /// does not change selection rows).
    pub previous: Vec<CullSnapshot>,
    /// `TargetRun.appliedAtMs` now.
    pub applied_at_ms: i64,
    /// `TargetRun.appliedAtMs` before this apply (`None`: never applied). Undo restores it, so
    /// the run reads "not applied yet" again (IPC v20.2).
    pub previous_applied_at_ms: Option<i64>,
}
