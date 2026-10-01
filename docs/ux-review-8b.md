# Phase 8b UX review: guided Cull → Edit → Export, Lightroom-style Develop

Author: ux-designer · 2026-09-30 · branch `phase-8b-feedback` @ 3436100 (read-only review)
For: frontend-dev (all items), architect (items marked **[ARCH]**)

Evidence: mock backend (`vite --port 1933`, `/?mock=201…`, `&autosync=1`, `&style=ready|learnable`, `&nokeepers=1`, `&projects=0`),
1280×800 and 1728×1117. Screenshots: `test-data/ux-review-8b/shots/` (`{1280,1728}-NN-*.png`). Throwaway Playwright drivers:
`test-data/ux-review-8b/{review,probe,probe2}.spec.ts` (config `pw.config.ts`); raw measurements in `measure.log`, `probe.log`, `probe2.log`.

**Summary: P0 2 · P1 9 · P2 15.** The guided workflow is in good shape: it is clear, it guides the user, and the
Develop layout now matches Lightroom. Two P0 bugs break the core "edit one photo per scene" loop: scene-to-scene
navigation lands on the wrong photo, and a stale profile hover preview hides the real render. The P1s are mostly
lost or untracked per-photo state (needs-a-look, new keepers after apply, skip) and one broken toast action.

---

## Workflow verdicts (steps measured on the mock)

| Workflow | Steps today | Verdict / friction |
|---|---|---|
| Open app → home → open project | 1 click (card), or Tab+Enter | Good. The empty state and New project dialog are clear. Import options (JPEG/HEIC) are missing from New project (P1-9). |
| Cull (pass/fail + stars) | P/X/U, 0–5, arrows, Shift = advance; clickable stars | Unchanged and good. At 1280 the Sort control is clipped (P1-7). |
| Cull → Edit | 1 click `Continue to Edit · 42 keepers` or Cmd+Alt+2 | Good. The plan builds automatically (detect over unassigned keepers). |
| Plan → edit representative | ↑/↓ + Enter/D (or `Edit ▸`) | Good from the plan. **Broken** between scenes: ›, N, Shift+N and the checklist jump open the first frame, not the representative (P0-1). |
| Develop the rep (presets, profile, Auto) | Cmd+U, Profile ⊞, presets panel | Layout matches LR. **The profile browser leaves a stuck hover preview** (P0-2). |
| Auto edit (my style) | 1 click / Cmd+Alt+U; replace-confirm when hand-edited; learn dialog when untrained | Good. Disabled-state explanation relies on a tooltip on a disabled button (P2-8). |
| Apply to scene → review → undo | Cmd+Shift+Enter or 1 click → toast `[Review] [Undo]` | Apply works and the row/strip markers are good. **Toast Review does nothing** (P1-1). Undo is single-level (P1-6). "!" marks are session-only and never clear (P1-2). |
| Apply all → Export | `Auto edit N remaining` → `Apply N edited scenes` → `Continue to Export → 42` → choose folder → Export | 4 clicks + folder for a fully automatic pass. Excellent. Export opens scoped to `Keepers (43)` by default. |
| Back home → switch projects | Project name ▾ → project, or Home | Good. Switching drops needs-a-look and auto-edited state (P1-2). |

---

## P0: blocks the workflow

### P0-1 Scene navigation opens the first frame of the scene instead of its representative
- **Where**: `src/App.tsx` `openScene` (l. 669–680) together with the "active left the result set" effect (l. 147–158). This affects
  `EditContextBar` ‹ / › (`edit-prev-scene` / `edit-next-scene`), keys N / Shift+N (`stepScene`), the checklist popover jump
  (`edit-checklist-<id>`) and `reviewFrames` when it changes scene.
- **Evidence**: `probe2.log`: `› next scene -> DSC00041.ARW chip=Not the representative` (rep is DSC00055), `N -> DSC00081` (rep 96),
  `checklist jump scene1 -> DSC00001` (rep 23). Screens `1280-23b-review-next.png`, `1728-23b-review-next.png` (Scene 2, active 41, rep 55 only ringed).
  `Edit ▸` from the Plan works because the rep is in both the old and the new id list.
- **Why it matters**: the step's core loop is "edit the rep, N, edit the next rep". Today the photographer edits a random member instead. `Apply to scene` then
  stays disabled (the rep is still "To do"), or a later apply overwrites the member's edit. The scene count never moves.
- **Cause**: `setQuery({sceneId})` and `sel.set([rep])` run in the same tick. The effect then sees the rep missing from the *old* scene's ids and moves the selection to
  `ids[0]`. When the new ids arrive, that frame is gone too, so the selection moves to the new scene's first frame.
- **Fix spec**: add `pendingActive = useRef<number|null>(null)`. `openScene(sceneId, photo)` sets the query, sets `pendingActive.current = id` and does **not** call `sel.set`.
  In the result-set effect: if `pendingActive.current != null`, then once `ids.includes(pending)` call `sel.set([pending], pending)` and clear it; while it is pending, skip the
  "move to neighbour" branch. Apply the same pattern to `reviewFrames` / `nextReview`. Acceptance (add to `workflow.spec.ts`): from the rep of scene 1, `edit-next-scene`,
  N, Shift+N and `edit-checklist-<id>` each land on `plan.scenes[i].representativeId`, with `edit-chip[data-kind=rep]`.

### P0-2 Profile Browser hover preview sticks after the browser closes; the viewer shows a stale render through later edits
- **Where**: `src/components/develop/ProfilePanel.tsx` (browser rows, hover preview) → `DevelopView` preview state ("Preview: <name>" label in the viewer).
- **Evidence**: `1728-16-masks-open.png`: the Navigator shows `23 main EV 0.35` (after Auto Tone), but the viewer shows `23 navigator EV 0` with the label `Preview: None`.
  The browser was opened from `profile-browse` and closed with Esc while the pointer rested on the `None` row. The same happens in `1280-55-copy.png`. It persists
  through Auto Tone, opening Masks, and the Copy dialog.
- **Why it matters**: the main viewer silently stops reflecting the user's edits. Sliders appear to do nothing, and the photographer may judge colour on the wrong image.
- **Fix spec**: clear the hover preview (a) when the browser unmounts or closes (Esc, Close, `⊞` toggle), (b) on `pointerleave` of the browser list *and* of the whole right
  panel, (c) on any adjustment commit, (d) when the active photo changes, and (e) on window `blur`. Click-to-apply also clears it, so the applied render replaces the preview.
  The same rule applies to the preset hover preview in the Navigator. Acceptance: open the browser with the pointer over a row, press Esc → `hover-preview-label`
  count 0 and the viewer URL equals the committed render.

---

## P1: noticeable friction or polish gap

### P1-1 Toast `[Review]` after Apply does nothing
- **Where**: `useWorkflow.applyScene` / `applyAll` pass `onReview = reviewFrames`. `reviewFrames` (`App.tsx` l. 731) reads `rowOfScene` from the render that created the
  toast, *before* `noteOutcomes` filled the review map.
- **Evidence**: `1280-23-review.png`: the toast `Applied Scene 1 to 14 photos · 1 need a look [Review]` → `Nothing to review in this scene`, while the filmstrip shows `!` on 21.
  `probe2.log`: `toast Review -> view=0`. The row button `Review N` (`plan-review-<id>`) works. No test covers `apply-review`.
- **Fix spec**: change the callback to `onReview(sceneId, ids: number[])` and pass `s.notConvergedIds` from the apply result (for Apply all, the first scene that has some).
  `reviewFrames(sceneId, ids?)` uses `ids ?? row.review`. Add a Playwright step that clicks `apply-review` and expects Develop on the first needs-look frame.

### P1-2 Needs-a-look, auto-edited and applied-per-photo state is session-only, and fixing a frame never clears its "!" **[ARCH]**
- **Where**: `useWorkflow` `review` / `autoAt` / `appliedCount` maps (reset on project change; `App` is keyed by project).
- **Evidence**: `probe2.log`: after a project switch the row reads `Applied to 15` (was `· 1 need a look`) and `plan-review-1` is gone (`1280-64-plan-after-switch.png`).
  After editing the needs-look frame (exposure 0.5), the chip still reads `Needs a look: exposure did not match` and the row still says `1 need a look`.
  An `auto` scene reverts to `Edited` after a reload.
- **Why it matters**: the frames the matcher could not handle are exactly the ones a photographer must check before delivery. Today they are lost on reload or
  project switch, and the list never empties.
- **Contract (architect)**: per image `editSource: none|user|auto_style|scene_apply|pasted` and `needsReview: bool` (set by apply for non-converged targets, cleared by any
  later user commit on that image). In `SceneEditEntry` add `needsReviewIds`, `autoEdited` (the rep's last commit came from the style model) and `appliedIds`.
  **Frontend**: derive the `auto` / review / applied markers from the plan only and delete the session maps. Until the contract lands, at least drop an image from the
  review map when `save_adjustments` commits for it.

### P1-3 Plan "outdated": keepers added after an apply stay unedited while the scene reads "Applied" **[ARCH]**
- **Where**: backend `scene/workflow.rs::entry_for`. The status is `Applied` while the rep's settings equal `applied_params_json`, whatever happened to the membership.
  `EditPlan.unassignedKeeperIds` is never shown in `PlanView`.
- **Why it matters**: a photographer picks 5 more frames in Cull after applying, and the plan and step bar say "All scenes applied". Those frames are exported unedited.
- **Contract**: `SceneEditEntry.unappliedKeeperIds` (keepers that are not the rep, not in the scene's last apply batch, and not user-edited).
- **Frontend**: when that list is non-empty, the status line reads `Applied to 14 · 3 new keepers not edited` (amber) and the primary button reads `Apply to 3 new`
  (same `applySceneEdit`; the backend already skips user-edited targets). If `unassignedKeeperIds.length > 0`, show a 32 px amber-950 banner above the tabs:
  `5 keepers are not in a scene yet. [Group them]` → `loadPlan(true)`. `allDone` and the Edit pill's "All scenes applied" require both lists to be empty.

### P1-4 No way to skip a scene, and small scenes are not grouped **[ARCH]**
- **Where**: `PlanView` (tabs All / To do / Edited / Applied; the row ⋯ has no Skip).
- **Why it matters**: real shoots produce many 1–3-keeper scenes (detail shots, one-off frames). The plan never reaches "All done", `Continue to Export` never
  appears, and the step bar never completes unless the photographer edits every one.
- **Contract**: `set_scene_skipped(sceneId, bool)`; `SceneEditEntry.skipped`, `minor` (≤ 2 keepers, or the backend's own rule).
- **Frontend** (as specified in ux-spec-8b §4.2): a `Skipped N` tab; row ⋯ `Skip this scene` / `Include this scene`; skipped rows at opacity 60 with the line
  `Skipped, no edit copied`; minor scenes sort last under a 24 px divider `Small scenes (6 scenes, 9 photos)`, collapsed by default. `allDone` = every scene applied or skipped.
  Key: **S** in the Plan toggles skip for the focused row (S is unbound; Shift+S stays the scene toggle).

### P1-5 "Apply with options…" bypasses the batch path: no plan state, per-photo undo loop **[ARCH small]**
- **Where**: `App.tsx` `MatchPanel onApplied` (l. 1497–1516) → `apply_scene_match` → `develop::history::commit_batch` (no `edit_batches` row and no
  `applied_params_json`, `commands.rs` l. 1264).
- **Why it matters**: after "Apply with options…" the row still reads `Edited, ready to apply`, Cmd+Z does not undo it as one step, and its toast Undo loops
  `undoAdjustments` (it undoes the wrong entry if a target was touched in between).
- **Fix**: route MatchPanel's Apply through `applySceneEdit(sceneId, {matchOptions: panel options, …})`. **[ARCH]**: add `excludeIds: ImageId[]` to `SceneApplyOptions`
  for the panel's per-photo checkboxes. Use the same toast and `LastBatch` as Apply to scene.

### P1-6 Batch undo is single-level, and undone toasts keep their Undo button
- **Where**: `useWorkflow.lastBatch` (one slot); `App.tsx` `undoCull` / `undoAdj` compare only `wf.lastBatch.at`.
- **Evidence**: `probe.log`: Cmd+Alt+U (auto edit) → Cmd+Shift+Enter (apply) → Cmd+Z undoes the apply → a second Cmd+Z gives `Nothing to undo` (the auto edit
  is still applied). The apply toast `[Review] [Undo]` stays on screen after the undo (`1280-51-masks.png`).
- **Fix spec**: `batchStack: LastBatch[]` (cap 20). Cmd+Z pops the newest whose `at` is greater than the newest culling/adjustment change. `undoBatch`
  removes that batch from the stack and dismisses its toast (`toasts.dismiss(id)`; return the id from `push`). The row ⋯ `Undo apply` stays enabled while that
  scene's batch is still in the stack.

### P1-7 Grid toolbar overflows at 1280: Sort, sort direction, Auto-advance and the count are clipped
- **Where**: `GridToolbar.tsx` (`grid-toolbar`, `overflow-x:auto`); screens `1280-04-cull-grid.png`, `1280-05-cull-flagged.png` (only "Cap" of the Sort select is visible).
  Measured: scrollWidth 1453 vs clientWidth 1044 (`measure.log`). The same happens in the Edit-step grid (`1280-30-grid-edit-step.png`, Auto-advance clipped).
- **Fix spec** (ux-spec-8b §3, not implemented): below 1440 px, fold `Size` + `Sort` + direction + `Auto-advance` into a `View ▾` popover (h-7, 64 px, `data-testid="view-menu"`).
  Keep the count `101 photos · 1 selected` (truncate, min 120 px) in the row. Remove the `All folders` select when the project has one folder (keep it, as
  `Folder ▾`, for multi-folder projects). Acceptance: `grid-toolbar` scrollWidth ≤ clientWidth at 1280×800 in the Cull and Edit steps.

### P1-8 Cheat sheet cannot be scrolled from the keyboard, and Space closes it
- **Where**: `CheatSheet.tsx`. Focus lands on the Close button, so PageDown/↓ do nothing (`probe.log`: scrollTop 0) and Space activates Close. At 1280 the content is
  1540 px in a 641 px box (59% hidden, no affordance, `1280-08-cheatsheet.png`). In the Plan it says "Showing Library first".
- **Fix spec**: give `cheat-columns` `tabIndex={-1}` and focus it on open (not the X). ↑/↓/PgUp/PgDn/Home/End/Space scroll it (Space must not close it). Add a 24 px bottom
  fade while more content is below. Add a filter input in the header (`Type to filter`, autofocused, matches label/keys; Esc clears it first, then closes).
  Ordering: in a project's Edit step (Plan or Develop) put `Workflow` first, then `Develop`. Pass `mode="plan"` → subtitle `Showing Edit step first`.
  Widen the key column to `w-32` so `Space Enter E` does not wrap.

### P1-9 New project dialog has no import options
- **Where**: `home/ProjectDialogs.tsx NewProjectDialog` (`1280-02-new-project.png`). The home empty state says "JPEG… when enabled in Import options", but that
  control exists only in the in-project Import ▾.
- **Fix spec**: under Shoot type, add two checkboxes bound to `useImportOptions`: `Include JPEG, HEIC, TIFF, PNG` (`new-project-nonraw`) and
  `Pair a camera JPEG with its RAW` (`new-project-pair`, disabled unless the first is on). Copy for the empty state: `…PNG (turn on in New project)`.

---

## P2: nice to have

1. **Icon-only view buttons inside a project (<1440)** (`TopBar.tsx`, `1280-11-plan.png`). Plan (`ListChecks`) and Develop (`SlidersHorizontal`) look alike at 14 px.
   Spec: below 1440 show the label on the **active** segment only (`[☰ Plan] [▦] [⛶] [◫] [≡]`). Free about 75 px by hiding the `Save` button while auto-sync is on
   (Cmd+S still works; the XMP popover gets `Save selection now`). Use `LayoutList` for Plan so it differs from the Develop icon.
2. **Snapshots ships as a dead section** with developer copy `Snapshots need backend support (not wired yet)` (`snapshot-add` title). Hide `section-snapshots`
   until **[ARCH]** `list/create/update/delete_snapshot` exists. A wedding workflow rarely needs it, so there is no reason to show a disabled control.
3. **Histogram hidden while Masks is open** (`1280-51-masks.png`, `1728-16-masks-open.png`): at 1728 there is about 600 px of free space below the masks. Keep the histogram
   when the right panel is ≥ 860 px tall. Below that, show a compact 48 px histogram without the info line.
4. **Apply cancel** **[ARCH `cancel_scene_apply`]**: no cancel during `Applying… n/N`. Add an × on the progress pill and Esc in the Plan; finished scenes stay applied.
   P2 because the measured mock applies are short; promote it if real scenes of more than 100 frames take over 10 s.
5. **Keepers filter is client-side** **[ARCH `ImageQuery.keepersOnly` + keeper-scoped `get_filter_counts`]**: in the Edit step the filter chips count the whole project
   (`Picked 9 · Rejected 8 · Unflagged 84` above a 42-keeper grid) and the filmstrip header reads `Filtered: Scene 1 · 15 of 101`. Show `15 of 42 keepers`.
6. **XMP failure listing** **[ARCH `list_xmp_failures` or `ImageQuery.xmpErrorOnly`]**: `openXmpErrors` pages the whole catalog 1000 at a time. Each failure row should
   also jump to the photo (`Show` link → Grid filtered to failed photos).
7. **Shift+S in the Edit step** toggles the hidden scene strip instead of `This scene only`; the first press does nothing visible (`App.tsx` `scenesToggle`).
   In the Edit step map it to `sceneOnly.toggle` (ux-spec-8b §9).
8. **Disabled Auto edit explains itself only through `title` on a `disabled` button** (WKWebView does not reliably show it; `1280-40-style-none-hover.png`). Use
   `aria-disabled` + a muted style. A click shows the existing tip toast (`requestAutoEdit` already does this for `insufficient`). Hide the per-row `Auto edit` buttons
   in that state and keep only the header button.
9. **Toasts cover the Develop viewer toolbar** (filename, stars) (`1728-23b-review-next.png`). In Develop, raise the toast stack above the toolbar and filmstrip
   (`bottom: filmstrip + header + toolbar + 8`).
10. **Small text contrast**: `text-neutral-500` filenames in plan rows (#737373 on #171717 = 3.8:1) and tab counts (4.2:1 on #0a0a0a) fail AA. Use `neutral-400`.
11. **XMP pending pill is amber with a ring**, which reads as a warning for a normal 1–2 s state. Use neutral + spinner for `pending`; keep amber/red for `error`.
12. **Keeper rule menu changes every project** (catalog-wide `set_keeper_rule`), but the menu does not say so. Add a footer line: `Applies to all projects`.
13. **Home keyboard**: Cmd+Shift+I = New project, Cmd+F or `/` focuses search, Enter opens the focused card (works), ←/→/↑/↓ move between cards.
    Inside a project, **Cmd+Shift+O** opens the project switcher (LR "Open Catalog"). The card focus ring is a 1 px border; use a 2 px sky-500 ring.
14. **Plan member strip at ≥1600** leaves about 200 px empty before the actions (`1728-11-plan.png`). Size `memberShown` from the measured strip width
    (`floor((w - chip) / (thumb + 4))`).
15. **Filmstrip `S1` badge on every cell** when `This scene only` is on, and it overlaps the first star at 88 px (`1728-12-develop-rep.png`). Show the scene badge only on the
    first cell of a scene (ux-spec-8b §4.4) and move it to the top-right corner.

---

## Triage of the known gaps (docs/decisions.md 2026-09-30)

| Gap | Priority | Finding |
|---|---|---|
| Skipped / minor scenes | **P1 [ARCH]** | P1-4 |
| Persisted per-photo applied / needs-a-look / auto-edited source | **P1 [ARCH]** | P1-2 |
| Plan `outdated` flag | **P1 [ARCH]** | P1-3 |
| Apply cancel | P2 [ARCH] | P2-4 |
| Server-side keepers filter | P2 [ARCH] | P2-5 |
| XMP failure listing | P2 [ARCH] | P2-6 |
| Snapshots `+` disabled | P2 (hide) | P2-2 |
| Histogram hidden while Masks open | P2 | P2-3 |
| Cheat sheet scrolling | **P1** | P1-8 |
| Sort scrolling out of view at 1280 | **P1** | P1-7 |
| Icon-only view buttons in a project | P2 | P2-1 |

---

## Keyboard map (existing vs proposed; conflicts resolved)

| Action | Existing | Proposed | Scope | Note |
|---|---|---|---|---|
| Pick / Reject / Unflag, stars, labels | P / X / U, 0–5, 6–9 | same | all | inert in the Plan (correct) |
| Steps Cull / Edit / Export | Cmd+Alt+1/2/3 | same | project | verified |
| Plan row focus / open rep | ↑/↓, Enter / D | + **Home / End** first/last scene | Plan | — |
| Next / prev scene | N / Shift+N | same; **must land on the rep** (P0-1) | Edit step | — |
| Next needs-look frame | N (review) | same, fed by the persisted list (P1-2) | Develop | — |
| Apply to scene | Cmd+Shift+Enter | same | Plan, Develop | verified |
| Auto edit (my style) | Cmd+Alt+U | same | Plan, Develop | verified |
| Skip / include scene | — | **S** | Plan | S unbound; Shift+S unchanged |
| Make representative | Shift+A | same | Edit step | — |
| This scene only | Shift+S (toggles the hidden strip) | **Shift+S = This scene only** in the Edit step | Edit step | P2-7 |
| Undo / redo | Cmd+Z / Cmd+Shift+Z (one batch slot) | same, over a **batch stack** | all | P1-6 |
| Auto Tone / Auto WB | Cmd+U / Cmd+Shift+U | same | Develop | — |
| Copy / Paste / Previous / Sync / Reset | Cmd+Shift+C / V, Cmd+Alt+V, Cmd+Shift+S, Cmd+Shift+R | same | Develop | — |
| Cheat sheet | ? / Cmd+/ | same; ↑/↓/PgDn scroll, type to filter | all | P1-8 |
| New project (home) | — | **Cmd+Shift+I** | Home | same chord as Import inside a project |
| Search projects | — | **Cmd+F**, **/** | Home | Cmd+F is the filter bar inside a project |
| Project switcher | — | **Cmd+Shift+O** | project | LR "Open Catalog"; unbound |

Conflict check: S, Home/End in the Plan, Cmd+Shift+O and the home-only chords are unbound in `keymap.ts`. Home/End are Grid-only today and the Plan intercepts its own keys first.
Register every new chord in `KEYMAP` so `hint()` and the cheat sheet pick it up.

---

## No change needed
- Home page layout, cards, empty state, sort and search; the project switcher menu; relaunch to home.
- Step bar states and counts, `Continue to Edit`, and the step persistence on the card.
- Plan view layout, row states, header progress, `Continue to Export`, the Rebuild confirm, and the no-keepers empty state (`1280-42-plan-nokeepers.png`).
- Edit context bar layout; the replace-edit confirm; the Learn-your-style dialog (`1280-41-style-learnable-click.png`).
- Develop layout: panel order, tool strip, single-row sliders, Copy… / Paste and Previous / Reset placement, the grouped Copy Settings dialog (`1280-55-copy.png`), the viewer toolbar.
- Compare view (`1280-10-compare.png`), the XMP explainer copy and the XMP popover.
- Export dialog with the `Keepers (43)` scope, default from step 3 (`1280-63-export.png`).

## Re-check (2026-09-30, `phase-8b-feedback` @ 1d0bdb9)

Author: ux-designer (read-only). Evidence: mock backend (`vite --port 1933`, `/?mock=201&style=ready`), Chromium at 1280×800 and 1728×1117.
Drivers: `test-data/ux-review-8b/recheck/{recheck,recheck2,probe3,probe4}.spec.ts` (config `pw.config.ts`). Logs: `recheck.log`, `probe3.log`, `probe4.log`.
Screenshots: `test-data/ux-review-8b/recheck/{1280,1728}-NN-*.png`.

**Result: all 2 P0 and 9 P1 from the review are resolved. The fixes add 2 new P1s, so the gate "no open P0/P1" is not met yet.**
Both new P1s are small, contained frontend fixes. One of them also needs a backend guard **[ARCH]**.

### Original P0/P1 status

| Item | Status | Evidence |
|---|---|---|
| P0-1 Scene nav lands on the first frame | **Resolved** | `recheck.log`: reps 23/55/96. `›`=55, N=96, Shift+N=55, `‹`=23, and the checklist jump to Scene 3 = 96. Every one has `edit-chip=rep`, at both sizes. `1280-11-develop-next-scene.png` shows `Scene 2 of 3 · To do · representative` on DSC00055. |
| P0-2 Stuck profile hover preview | **Resolved** | Hover `Preview: Adobe Standard`, then Esc: label count 0, browser closed, still in Develop. No label after Cmd+U. Leaving a preset hover clears it as well (`1280-13/14`). |
| P1-1 Toast Review does nothing | **Resolved** for Apply to scene / Apply all: toast Review opens Develop on 21 with `chip=review` (`1280-18-review-frame.png`). **New P1-10** covers a regression on the "Apply with options…" path. |
| P1-2 Session-only needs-a-look | **Resolved** | After Home and back, scene 1 still reads `Applied to 15 · 1 need a look` and scene 2 still reads `auto` (`1280-19-plan-after-home.png`). Editing 21 clears its `!` (chip `member`, no `film-review-21`). Cmd+Z brings it back (chip `review`), as specified. Not verified across a real page reload, because the mock resets (frontend's stated limit). |
| P1-3 New keepers after apply | **Resolved** | After Apply all, picking frame 4 in Cull gives `Applied to 15 · 1 need a look · 1 new keeper not edited`, `Apply to 1 new`, the step pill reads `Needs a look`, and `Continue to Export` is hidden (`1280-61-plan-new-keeper.png`). The Develop context bar offers `Apply to 1 new` (`1280-62`). The unassigned banner `15 keepers are not in a scene yet. [Group them]` renders correctly at both sizes (`*-47-plan-unassigned-minor.png`). |
| P1-4 Skip / minor scenes | **Resolved** | S on the focused row toggles skip at both sizes. Skipped rows are at opacity 60 with `Skipped, no edit copied` and appear under the `Skipped 1` tab (`1280-15-plan-skipped.png`). The state persists through Home and back. `Small scenes (1 scene, 2 photos)` is folded last, and its rows can be reached with ↓ once expanded. |
| P1-5 Options apply bypasses the batch | **Resolved** as specified: it now goes through `apply_scene_edit`, the row reads Applied, and there is one toast with `Review` and `Undo`. **New P1-10** covers the target set it now uses. |
| P1-6 Single-level batch undo | **Resolved** | Apply, then Cmd+Z, undoes the apply, and its toast Undo disappears (`undo buttons=0`). The suite covers Auto edit then two Cmd+Z presses. **New P1-11** covers out-of-order undo from an older toast. |
| P1-7 Toolbar overflow at 1280 | **Resolved** | `grid-toolbar` scrollWidth 1044 equals clientWidth 1044. `View ▾` holds Size, Sort, direction and Auto-advance, and Esc closes it (`1280-03-view-menu.png`). Nothing overflows the viewport in Cull, Plan, Develop, the Edit-step grid, the Match panel, the Applying state or Export at 1280 (`recheck.log` overflow lines; the filmstrip's own horizontal scroll is expected). |
| P1-8 Cheat sheet keyboard | **Resolved** | PgDn + Space scroll it (scrollTop 894 at 1280) and the sheet stays open. Type-to-filter works. The first Esc clears the filter and the second closes the sheet. In the Edit step the subtitle reads `Showing Edit step first` with Workflow listed first (`1280-52-cheat-plan.png`). |
| P1-9 Import options in New project | **Resolved** | Both checkboxes are present, and Pair is enabled only when Include is on (`1280-01b-new-project-checked.png`). |

### Regression checks (keyboard, layout)

- **S in the Plan**: no conflict. Plain S is unbound everywhere else. Shift+S is inert in the Plan (it is in `PLAN_INERT`; `scene-strip` count stays 0). Cmd+S (save), Cmd+Shift+S and Cmd+Alt+S (sync) are only reached with modifiers. With Caps Lock on, S still works (the handler lowercases the key). S does nothing in the Edit-step grid or in Develop (no toast, no state change). When a row button has DOM focus (Tab), S acts on the focused row, which is consistent.
- **Esc in the Plan**: when idle it does nothing (it keeps the row focus and does not leave the Plan). With a row ⋯ menu open during an apply, the first Esc closes only the menu and the apply keeps running (`menu=0 applying=1`). The second Esc cancels it, with the toast `Stopped after 1 scene (15 photos)`. Esc in Develop closes the profile browser and stays in Develop.
- **Layout at 1280**: no new overflow (see P1-7). The top bar shows a label only on the active segment (`Plan`), and Snapshots is hidden.

### New P1

#### P1-10 "Apply with options…" in the Edit step edits non-keepers, so its counts disagree with the row and its toast Review opens an empty Develop
- **Where**: `src/components/scenes/MatchPanel.tsx` `apply()` (l. 110–120) hard-codes `includeNonKeepers: true, skipUserEdited: false`. The panel's card list is every scene member except the rep. `App.tsx` `reviewFrames` (l. 756) then opens whatever id it receives.
- **Evidence** (`probe3.log`, `1280-42-match-panel-run.png`, `1280-60-options-toast-review.png`): Scene 3 has 10 keepers and the plan button reads `Apply to 9`. The panel reads `19 targets` and `Apply to 19 photos`, listing non-keepers 82, 83, 84 and others. After the apply, the row reads `Applied to 19` with no needs-a-look, but the toast reads `Applied Scene 3 to 18 photos · 3 need a look`. Those 3 frames (84, 91, 98) are non-keepers. Toast `[Review]` opens Develop with no image: the viewer is black, the Navigator says `No preview`, the context bar says `Not in a scene`, the sliders sit at defaults, and N does nothing.
- **Why it matters**: the photographer opens "with options" to adjust strength or untick a frame. Instead the edit spreads to culled-out frames, the counts in the toast, the row and the button no longer agree, and the Review button leads to a blank, editable-looking screen. Separately, `skipUserEdited: false` silently overwrites frames the user had retouched by hand, while the plain Apply leaves them alone.
- **Fix spec (frontend)**:
  1. In the Edit step (`onApplied` undefined), the panel's targets are the scene's **keepers** minus the rep, which is the same set as `Apply to N`. Next to `Include rejected (n)`, add a checkbox `Include non-keepers (n)` (`match-include-nonkeepers`), off by default. When on, add those cards and send `includeNonKeepers: true`. Otherwise send `false`, and do not list non-keeper ids in `excludeIds`.
  2. Cards for frames whose `editSource` is `user` or `pasted` (`get_edit_states`) start unchecked, with a neutral badge `Edited by you`. Keep `skipUserEdited: false` so an explicit tick overrides. The footer reads `n of m selected · k edited by you (unticked)`.
  3. Guard in `reviewFrames`: keep only ids in the current result set (`ids`). If none remain, show the toast `The frames that need a look are not keepers` and do not change view. Develop must never open with an id outside the result set.
  4. Acceptance: in Scene 3, Options gives `Apply to 9 photos`. The toast count matches the row's `need a look`, and toast Review lands on a keeper with `chip=review`.

#### P1-11 Undo on an older toast silently undoes a batch that a newer batch was built on **[ARCH]**
- **Where**: `useWorkflow` batch stack and toast actions. Every Undo toast stays clickable, whatever was applied after it.
- **Evidence** (`probe4.log`, `1280-64-older-undo.png`): run Auto edit (3 scenes), then Apply Scene 1, then click Undo on the *older* toast `Auto edited 3 scenes`. Scene 1 still reads `Applied to 15 · 1 need a look`, and its rep 23 still shows `Applied · representative`, but its exposure is back to 0 (Original). Member 24 keeps `From Scene 1's edit ✓` with exposure 0.9, which was derived from the undone look.
- **Why it matters**: the plan says the scene is done, while the representative and its 15 frames no longer match. An export would deliver an inconsistent scene, and nothing on screen says so. Lightroom's undo is strictly linear, and photographers expect the same here.
- **Fix spec (frontend)**: a toast's Undo is shown only while its batch is the **newest** entry of the batch stack *and* no later culling or adjustment commit touched its images. Otherwise remove the button and keep the text, as is already done for undone batches. The row ⋯ `Undo apply` follows the same rule. Cmd+Z behaviour is unchanged.
- **[ARCH]**: `undo_edit_batch` must return `conflict` ("Later edits on n photos; undo those first") when any image in the batch has a history entry newer than the batch's entry. The plan status should also stop reporting `applied` when the rep's current settings no longer equal `applied_params_json`; check that the real backend, not only the mock, recomputes this after an undo of the rep's batch.

### Remaining P2 (none block the gate)

Done since the review: P2-1 (label on the active segment), P2-2 (Snapshots hidden), P2-4 (apply cancel ×/Esc), P2-5 (keepers filter: `16 of 44 keepers` in the grid and filmstrip), P2-6 (`list_xmp_failures` + Show), P2-10 (plan filenames now `neutral-400`).

Still open:
1. P2-3: the histogram is hidden while Masks is open.
2. P2-7: Shift+S in the Edit step still toggles the hidden scene strip. It does not toggle `This scene only` (`1280-63-develop-shift-s.png`).
3. P2-8: the disabled Auto edit still explains itself only through `title` (no `aria-disabled` pattern).
4. P2-9: toasts cover the Develop viewer toolbar (Before / Split / Compare / flags) at 1280. Undo toasts also persist and stack, so two toasts cover it for minutes (`1280-62-develop-new-keeper.png`). Raise the stack above the toolbar, and auto-dismiss Undo toasts after 10 s (the Undo stays reachable with Cmd+Z).
5. P2-11: the XMP `pending` pill is still amber with a ring.
6. P2-12: the keeper rule menu has no `Applies to all projects` footer.
7. P2-13: home keyboard (Cmd+Shift+I, Cmd+F or `/`, arrow keys between cards) and Cmd+Shift+O for the project switcher are not bound.
8. P2-14: the plan member strip is fixed at 6 or 8 thumbs, which leaves about 220 px empty at 1728 (`1728-47`).
9. P2-15: the `S1`/`S2` scene badge appears on every filmstrip cell (`1280-11`).
10. New: the cheat sheet does not list the Plan keys. Filtering for "skip" returns no rows (`1280-04b`). Add Workflow rows `S | Skip / include the focused scene | Plan` and `Esc | Stop applying | Plan` (register them in `KEYMAP`, matched by the Plan handler), and add `(S)` to the `Include this scene` menu item.
11. New: the progress pill reads `Applying… 0/0` before the first progress event (`1280-44-applying.png`). Show `Applying…` until `total > 0`.
12. New: the header primary reads `Apply 0 edited scenes (0)` (disabled) when the plan is outdated only through unassigned keepers (`*-47`). Hide the button when there is nothing to apply; the banner's `Group them` is the next action.
13. New: the batch undo stack is session-only. After Home and back, Cmd+Z reports `Nothing to undo` and the row ⋯ `Undo apply` is disabled, although `undo_edit_batch` could still undo the scene's last batch. Enable row `Undo apply` from persisted plan state (the scene's last batch id, **[ARCH]** if that is not exposed). Cmd+Z may stay session-scoped, which matches Lightroom.
14. New: the View ▾ popover's sort-direction control is an icon-only, full-width button (`1280-03-view-menu.png`). Label it `Ascending` / `Descending` with the icon.
15. Noted, not a defect: the Match panel outside the Edit step still uses `apply_scene_match` with a per-photo Undo loop (frontend decision). That flow is only for Library scenes. Move it to the batch path when convenient.

### No change needed
The Plan layout, skip and minor presentation, the unassigned banner, the Applying pill with × and Esc, the New project dialog, the 1280 View menu, the cheat sheet's keyboard model, the Develop context bar's `Apply to 1 new`, the XMP failures popover, and Export.

## Re-check 2 (2026-09-30, `phase-8b-feedback` @ 50e7457)

Author: ux-designer (read-only). Evidence: mock backend (`vite --port 1933`, `/?mock=201&style=ready`), Chromium at 1280×800 and 1728×1117.
Drivers: `test-data/ux-review-8b/recheck2/{p10_11,p11b,p11c,p11d,walk}.spec.ts` (config `pw.config.ts`). Logs: `p10_11.log`, `p11b.log`, `p11c.log`, `p11d.log`, `walk.log`.
Screenshots: `test-data/ux-review-8b/recheck2/{1280,1728}-NN-*.png`.

**Result: P1-10 and P1-11 are resolved, and none of the earlier P0/P1 items has regressed. One new P1 (P1-12 [ARCH]) is open, so the gate "no open P0/P1" is still not met.**
P1-12 is a state that the P1-11 fix exposes and that a plain Reset can also reach: a scene whose representative goes back to unedited after an apply. That scene then blocks **Apply all** for the whole project.

### P1-10 / P1-11

| Item | Status | Evidence |
|---|---|---|
| P1-10 Options apply edits non-keepers | **Resolved** (both sizes) | Scene 3: the plan says `Apply to 9`. The panel lists 9 cards (81, 85, 86, 87, 89, 93, 99, 100, 101) with no non-keepers, and its button reads `Apply to 9 photos`. `Include non-keepers (11)` starts unticked (`*-01/02`). After the apply, the row reads `Applied to 9` and the toast reads `Applied Scene 3 to 9 photos`, so the counts agree. Scene 1 via Options: the row reads `Applied to 15 · 1 need a look`, and toast Review opens Develop on keeper 21 with `chip=review` (`*-36-options-review-develop.png`). A member I edited by hand (56) starts unticked with the `Edited by you` badge, and the footer reads `15 of 16 selected · 1 edited by you (unticked)` (`1728-05`). The suite covers the non-keeper Review guard. |
| P1-11 Older toast Undo silently breaks a scene | **Resolved** as designed in v16 (a per-photo conflict rule, not toast order) | Exact repro (Auto edit 3 scenes → Apply Scene 1 → Undo on the older `Auto edited 3 scenes` toast): the undo is still allowed, because the apply did not write the reps. Scene 1 now **says so**: the status is `stale`, the line reads `Changed since it was applied` (amber), the button reads `Re-apply to 15`, the step pill reads `0 of 3 scenes`, and `Continue to Export` is hidden (`*-11-after-older-undo.png`). The silent inconsistency is gone. Conflict path: Apply Scene 2, then edit member 56 in Develop. The apply toast loses Undo at once (Review stays). The row ⋯ and Develop `Undo apply` are disabled with `Later edits on 1 photo. Undo those first` (`1280-18-row-undo-disabled.png`). Cmd+Z in Develop is linear: it undoes the apply first, then the rep (`p11c.log`). After Home and back, the row `Undo apply` is enabled from `appliedBatch`, and Cmd+Z in the Plan undoes it through `latestBatch` (`*-19`). **However**, `Re-apply to 15` in that state fails. See P1-12. |

### Earlier P0/P1: regression spot-check (both sizes, `walk.log`)

| Item | Status |
|---|---|
| P0-1 Scene nav | No regression. `›`=55, N=96, Shift+N=55, checklist jump to Scene 3 = 96, all `chip=rep`. |
| P0-2 Profile hover | No regression. `Preview: Adobe Standard` → Esc → label count 0, and the view stays in Develop. |
| P1-1 Toast Review | No regression. It works for Apply to scene and for Options (21, `chip=review`). |
| P1-2 Persisted needs-a-look | No regression. `Applied to 15 · 1 need a look` survives Home and back. |
| P1-3 New keepers | No regression. Picking 4 → `· 1 new keeper not edited`, `Apply to 1 new`, `Continue to Export` hidden (`*-39`). |
| P1-4 Skip (S) | No regression. S toggles skip on and off. The cheat sheet filter `skip` now lists `S · Skip / include the focused scene · Edit step: Plan` (`*-33`). |
| P1-5 / P1-6 | No regression. Options goes through the batch path, and the batch stack is backend-driven. |
| P1-7 Toolbar at 1280 | No regression. scrollWidth 1044 = clientWidth 1044, and the sort control reads `Ascending`. Nothing overflows in Cull, Plan, Develop, Match or Export. |
| P1-8 Cheat sheet | No regression. PgDn + Space scroll it (894 px at 1280) and the sheet stays open. Esc twice closes it. |
| P1-9 New project options | Not touched since re-check 1 (no related commits). |

**End-to-end walk** (both sizes): Home → open → cull (P, 3, →, X, Cmd+Z ×3) → `Continue to Edit` → Auto edit remaining → `Apply 2 edited scenes (25)` → pill `All scenes applied` → `Continue to Export` opens `Export 43 keepers` (`*-37`, `*-38`). This is unchanged and still the shortest path: 4 clicks plus a folder.

### New P1

#### P1-12 A scene whose representative is back to unedited after an apply is a dead end, and it blocks Apply all for every scene **[ARCH]**
- **Repro** (either path):
  - (a) The P1-11 repro: Auto edit 3 scenes → Apply Scene 1 → Undo on the older `Auto edited` toast.
  - (b) Auto edit Scene 1 → Apply → `Edit ▸` → Cmd+Shift+R (Reset) on the representative.
- **Evidence** (`p11b.log`, `p11d.log`, `1280-15-reapply-stale.png`, `1280-23-plan-stuck-reset.png`): the row reads `Changed since it was applied` and its primary green button reads `Re-apply to 15`. The tab and the header count it as `Edited 1` and `Apply 1 edited scene (15)`. Clicking Re-apply gives the red error `The representative has no edits yet. Edit it first.`, and nothing else changes. Worse, after Auto edit on scenes 2 and 3, `Apply 3 edited scenes (40)` fails with the same error and applies **nothing**, so `Continue to Export` never appears. The error does not say which scene is the problem. In path (a), the 15 members also keep the earlier look while the rep is back to the original.
- **Why it matters**: resetting the representative to start the look over is a normal move. After it, the main button of the step ("apply everything") stops working for the whole shoot, with an error that does not point to the cause. The row's call to action (`Re-apply`) can never succeed.
- **Cause (real backend too)**: `scene/workflow.rs::entry_for` reports `Outdated` whenever `applied_params_json` differs from the rep's settings, even when the rep has no edits. `edited_scenes` includes every `Outdated` scene. `apply_inputs` returns `invalid_argument` on the first unedited rep, and that aborts the whole Apply all.
- **Fix, [ARCH]**:
  1. `edited_scenes` excludes scenes whose `edited` is false.
  2. In `apply_all_edited_scenes`, a scene that cannot be applied is skipped, not fatal. Where a single-scene error remains, it names the scene: `Scene 1: edit its representative first, then apply.`
  3. Recommended, for truly linear undo on the P1-11 path: count a scene apply made from a representative's batch entry as a later edit of that batch, so that `undo_edit_batch(autoEdit)` returns `conflict` while that apply stands. Undo the apply first, then the auto edit, as in Lightroom.
- **Fix, frontend** (`useWorkflow` `SceneUi`, `PlanView`, `EditContextBar`, `bits.tsx`):
  - Add `ui = "reset"` when `entry.status === "outdated" && !entry.edited`.
  - Row: amber `RefreshCw` icon. Status line `Representative reset · 15 photos keep the earlier look` (amber). Actions: primary `Edit ▸` (blue, as for To do). A secondary button `Undo apply` (`plan-undo-inline-<id>`) appears when `appliedBatch?.undoable` and restores the members. No `Re-apply` button.
  - The scene counts under the **To do** tab and as "to do" in the header and progress. It is excluded from `Apply N edited scenes`.
  - Develop context bar: the chip reads `Reset since applied · representative`, and `Apply to scene` is disabled with the visible hint `Edit this photo first`.
- **Acceptance**: after either repro, the row shows `Edit ▸` and no Re-apply, and the header reads `Apply 2 edited scenes (25)` once scenes 2 and 3 are auto-edited. Apply all then applies those 2 scenes with no error.

### Remaining P2 (none block the gate)

Done since re-check 1:
- #10: S and Esc are in the cheat sheet, and the menu shows `(S)`.
- #11: the pill shows `Applying…` until progress arrives.
- #12: `Apply 0` is hidden in the unassigned-only case.
- #13: row `Undo apply` comes from persisted plan state and works after Home and back.
- #14: the sort direction is labelled `Ascending` / `Descending`.
- P2-9: toasts sit at the top in Develop and Undo toasts auto-dismiss after 10 s.

Still open from earlier (not touched by the commits since; not re-verified unless noted):
1. P2-3: the histogram is hidden while Masks is open.
2. P2-7: Shift+S in the Edit step toggles the scene strip, not `This scene only`.
3. P2-8: disabled controls explain themselves only through `title`. This now also applies to the disabled row/Develop `Undo apply` with `Later edits on 1 photo…` (`1280-18`). Show the reason as a second muted line inside the menu item.
4. P2-11: the XMP `pending` pill is amber with a ring.
5. P2-12: the keeper rule menu has no `Applies to all projects` footer.
6. P2-13: home keyboard (Cmd+Shift+I, Cmd+F or `/`, arrow keys between cards) and Cmd+Shift+O are not bound.
7. P2-14: the plan member strip is fixed at 8 thumbs, leaving about 110 px empty at 1728 (`1728-11`).
8. P2-15: the `S1` badge appears on every filmstrip cell (still visible in `1280-36`).
9. #15 (noted): the Library-scope Match panel still uses the per-photo undo loop.

New (P2):

10. **Stale "touched" hint keeps Undo disabled after a per-image undo.** `useWorkflow.syncBatches` keeps a client hint while the backend says the batch is undoable:

    ```ts
    touchedRef.current = new Set([...touchedRef.current].filter((id) => m.get(id)?.undoable !== false))
    ```

    Repro: Apply Scene 2 → edit member 56 → Cmd+Z on 56 in Develop (back to the applied value) → the row `Undo apply` stays disabled with `Later edits touched these photos…`. After Home and back it is enabled and works (`p11b.log`). Fix: once the backend has answered for a batch, drop the hint: `filter((id) => !m.has(id))`.
11. **Cmd+Z in the Plan says `Nothing to undo`** when the newest batch is blocked by a later per-photo edit, even though something exists to undo (`p11b.log`). Show the conflict reason instead: `Later edits on 1 photo (DSC00056.ARW). Undo those in Develop first`.
12. **The undo toast label changes after Home and back**: it reads `Undid Apply to Scene on 16 photos` (backend label) instead of `Undid Apply Scene 2…`. Build the label from the plan's scene number.
13. **Top-placed toasts in Develop cover open dialogs and the top of the photo.** With the Match panel opened from Develop, the `Auto edited` toast covers the panel header (`1 anchor … · 16 targets`, `1728-05`). Without a dialog, it sits over the top-centre of the photo (`1280-36`). Spec: while a modal is open, place the stack `bottom-4 left-4` (clear of the dialog header and footer). In Develop without a modal, align it to the viewer's top-right (`right: rightPanel + 12px`, `top: 88px`, `w-[400px]`) so it does not sit on faces in the centre of the frame.

### Keyboard
No change needed. Cmd+Z is linear in Develop (batch first, then per-image) and conflict-aware in the Plan. S, Esc and Shift+S in the Plan behave as in re-check 1, and no new chords were added.

### No change needed
The Match panel layout in Edit-step mode (keeper targets, the `Include non-keepers (n)` opt-in, the `Edited by you` badge and footer count), backend-driven toast Undo retraction, the info toast for `conflict`, row `Undo apply` after Home and back, the `Applying…` pill, the cheat sheet's Plan rows, the disabled `Apply 0 edited scenes (0)` preview on a fresh plan, and Export.
