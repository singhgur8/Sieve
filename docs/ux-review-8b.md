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
