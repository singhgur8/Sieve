# Phase 8b UX spec: guided Cull → Edit → Export + Lightroom-style Develop

Author: ux-designer · 2026-09-30 · branch `phase-8b-feedback` (read-only review)
For: frontend-dev (implementation), architect (items marked **[ARCH]** need contract work, most of them in IPC v14)

Evidence: current build against the mock backend (`pnpm dev --port 1921`, `/?mock=500`) at 1280×800 and 1728×1117.
Screenshots are in `test-data/ux-review/shots-8b/` (capture script `test-data/ux-review/capture-8b.cjs`):
`{1280,1728}-01-grid`, `-02-grid-scenes`, `-03-develop`, `-04/05-develop-panel-mid/bottom`, `-06-copy-dialog`,
`-07-profile-browser`, `-08-masks-tab`, `-09-scene-anchor`, `-10-match-panel`, `-11-export`.

---

## 0. What exists today and why the user got lost

Measured (1280×800, Develop): the chrome is TopBar 44 + filter summary 28 + scene strip 32 + Develop toolbar 37 + filmstrip 88
= **229 px vertical**. The photo gets 768×571. At 1728×1117 it gets 1184×888.

| User complaint | What the screenshots show |
|---|---|
| "I do not see a way to copy paste edits" | Copy/Paste do exist, but only as two small grey chips in the middle of the toolbar above the photo (`1280-03-develop.png`, x≈490–625), between "100%" and "Sync". Lightroom users look at the bottom of the left panel ("Copy…" / "Paste"). Paste is greyed out until something is copied, so it looks broken. |
| "Select exactly which parts of the edit I want" | The Copy dialog (`1280-06-copy-dialog.png`) is a flat 2-column list of 27 checkboxes in data order ("HSL hue", "LUT", "Masking" …). There are no Lightroom groups (Basic Tone, Color, Effects, Treatment & Profile), no tri-state parents, and the choice is not remembered between uses. |
| "Style the editing like Lightroom: button positions and features" | Left panel: Presets + History only (no Navigator or Snapshots) and mostly empty. Right panel: Adjust/Masks tabs, **Crop as the first section**, then Profile as its own section, Basic without Auto, Presence as a separate section, a "LUT" section, and Treatment inside Color Mixer. Every slider takes two rows (label row + track row), so Basic needs about 340 px. |
| "Steps like AfterShoot; edit one photo per scene; app copies it to the rest" | Scenes are a 32 px chip strip in every mode (`1280-02-grid-scenes.png`: "Scene 1 · 40 … Scene 10 · 40", overflowing at 1280). Anchors are hidden in the "Scene ▾" menu or on Shift+A. "Match scene" opens a full-screen modal (`1280-10-match-panel.png`) that starts empty and needs 3 more clicks. Nothing tells the user which scenes are done. |
| "Auto edit based on what the model thinks I like" | Not present (the style model is new in 8b). |

Verdict: the building blocks work (paste, sync, presets, scene match, history, masks). The UI does not guide the user through them, and the Develop layout does not match Lightroom. Both are **P0** for this phase because the user named them.

---

## 1. Information architecture: Steps × Views

- A **Project** is one catalog folder (`FolderEntry`). Steps are **per project** (`WorkflowStep`, v14).
- **Steps** (new, AfterShoot-style): `1 Cull` → `2 Edit` → `3 Export`. A step decides:
  - the photo scope: Cull = every photo in the project; Edit and Export = the **keepers** of the project;
  - the context bar under the TopBar;
  - which overview view "G" opens: Cull → Grid, Edit → **Plan** (the scene checklist).
- **Views** stay as they are: Grid / Loupe / Compare / Develop (+ **Plan** in the Edit step). Every view is reachable in
  every step. For example, D in the Cull step still opens Develop for a quick fix, and the step does not change.
- Steps are non-linear. Clicking any step pill is always allowed, and none is ever locked. A step is marked "done" by the
  backend state (below), never by forcing the user.

---

## 2. Global chrome

### 2.1 TopBar (44 px, replaces current `TopBar.tsx` layout) **P0**

```
1280×800
x: 0    12  36 44                    264 272 300        ~404                               ~876     958            1078        1228 1236 1268 1280
   |    [◎] |  [📁 Smith Wedding ▾ 500] [+]  |   (1 Cull · 212 keepers) › (2 Edit · 5/14) › (3 Export)   | [▦][⛶][◫][⚙] | [✓ Saved to XMP] | [⋯] |
        logo   project picker 220px    import 28     step bar, centred, ~470 px                          views 4×28      status 150        more 32
```
```
1728×1117 (labels appear on views and the Import button at ≥1440 px)
[◎ Sieve] [📁 Smith Wedding ▾ 500] [+ Import]      (1 Cull · 212 keepers) › (2 Edit · 5 of 14 scenes) › (3 Export · 212)      [▦ Grid][⛶ Loupe][◫ Compare][⚙ Develop] [✓ Saved to XMP] [⋯]
```
- **Project picker** (`data-testid="project-picker"`): 220 px (1728: 280 px), h-7, folder icon + folder basename (truncate,
  full path in `title`) + muted image count + chevron. Menu rows (32 px): basename, muted parent path, image count and a step badge
  ("Cull", "Edit 5/14", "Exported"). Then a divider, "Import shoot folder… ⇧⌘I", "Locate folder…" (moved from ⋯),
  and "All photos in catalog". Choosing "All photos" hides the step bar and shows the text "Pick a shoot to use
  steps" at 12 px neutral-400 in its place. This picker replaces the "All folders" select in `GridToolbar`, so remove it there.
  With one folder in the catalog, it is selected automatically.
- **Import** (`import-button`): 28×28 icon button (`Plus`), with the label "Import" at ≥1440 px. The Import-options ▾ popover moves
  into the project picker menu as a sub-row "Import options…". Tooltip: `Import a shoot folder (Cmd+Shift+I)`.
- **Shoot type** and **Analyze** move out of the TopBar into the Cull context bar (§3).
- **Export button** is removed from the TopBar. Step 3 replaces it and Cmd+Shift+E still works everywhere. The export
  progress ring moves onto the step 3 pill.
- **Save button** is replaced by the XMP status pill (owned by the "XMP auto-save" task). This spec only reserves the slot: 150 px,
  h-7, rounded-full. States: `✓ Saved to XMP` (neutral-300 text, no fill), `Saving…` (spinner), `12 pending`, and
  `2 failed` (red-950 fill). Clicking it opens the XMP popover from that task. Cmd+S keeps writing the selection.
- **View switcher**: icon-only 28×28 segments at <1440 px (icon + label at ≥1440). In the Edit step a 5th leading segment
  **Plan** (`ListChecks` icon) appears. The active segment uses bg-sky-800. Tooltips carry the shortcut via `hint()`.

### 2.2 Step bar (`components/StepBar.tsx`, `data-testid="step-bar"`) **P0**

Three pills joined by 16 px `ChevronRight` connectors (neutral-600). Each pill is h-7, px-3, rounded-full, 12 px text:
`[number circle 16 px] Label · sub-count`.

| Pill | Sub-count copy (in order of precedence) | Done when (from backend) |
|---|---|---|
| 1 Cull | `Analyzing 45%` while analysis runs → `212 keepers` → `No keepers yet` | `WorkflowStep ≥ edit` (user pressed Continue, or entered Edit with keepers) |
| 2 Edit | `Grouping…` while the plan builds → `5 of 14 scenes` → `All scenes applied` | every plan scene is `applied` or `skipped` |
| 3 Export | `Exporting 45%` (with the progress ring in the circle) → `212 photos` → `Exported 212` | an export job of this project finished OK |

Visual states (all pass WCAG AA on #0a0a0a):
- **Active**: bg-sky-800, label sky-50, sub-count sky-200, number circle white with sky-900 digit.
- **Done**: bg-neutral-800, label neutral-200, number circle replaced by a 16 px `Check` in emerald-400.
- **Upcoming**: bg-neutral-800, label neutral-300, sub-count neutral-400, number circle neutral-600 with neutral-200 digit.
- Hover: bg-neutral-700 (the active pill stays sky). Focus-visible: 2 px sky-400 ring.
- Tooltips: `Cull: flag, star and reject (Cmd+Alt+1)`, `Edit: one photo per scene, then apply (Cmd+Alt+2)`,
  `Export keepers (Cmd+Alt+3)`.

Click behaviour:
- Cull → Grid view of all photos (keeps the Cull filter state).
- Edit → Plan view. The first time, this builds the plan (§4.1).
- Export → opens the Export dialog scoped to keepers (§6). The pill is "active" while the dialog is open. Closing the dialog returns to the previous step.

Keys: **Cmd+Alt+1 / Cmd+Alt+2 / Cmd+Alt+3** (these mirror Lightroom's Cmd+Alt+1 Library / Cmd+Alt+2 Develop).
Pressing Cmd+Alt+2 while already in Edit goes to Plan.

### 2.3 Filmstrip header (22 px, above the existing filmstrip) **P1**

Lightroom puts source and filter info here, so the Develop filter-summary row (28 px, "No filters · 500 of 500 Edit
filters") is deleted and its content moves into this header:
`Smith Wedding › Keepers › Scene 3   ·   36 photos · 1 selected · DSC00123.ARW` on the left (11 px, neutral-400; the filename in neutral-200)
and `Filter: Picked ▾` plus a `This scene only` toggle chip (`Shift+S`) on the right. `Filter ▾` opens the existing
FilterBar as a popover, anchored bottom-up. In the Cull step the "Scene" segment is left out.

Filmstrip cells: 72×72 at 1280 (strip area 80 px), 88×88 at ≥1600 (strip area 96 px). Edit-step markers are specified in §4.4.

---

## 3. Cull step **P1** (behaviour is unchanged; placement and the exit button change)

Context bar = today's FilterBar row (32) + GridToolbar row (32). Changes:
- GridToolbar row, left: `Shoot [Wedding ▾]` (select, 120 px) and the `Analyze ▾` split button (110 px, moved from the TopBar;
  analysis progress stays in `ProgressBars`).
- GridToolbar row: remove "All folders" (now the project picker). At <1440 px, fold `Size` + `Sort` into a `View ▾` popover (60 px).
- GridToolbar row, far right: primary **`Continue to Edit → 212 keepers`** (h-7, bg-emerald-700, hover emerald-600, text
  white, `data-testid="continue-edit"`). Clicking it sets `WorkflowStep=edit` and opens the Plan. With 0 keepers the label is
  `Continue to Edit` and the Plan empty state explains what to do (§4.6).
- The scene strip is **hidden by default in Cull** (scenes are an Edit concept now). Shift+S still shows it (small-fixes task).
- Keys: none new. P/X/U/0–5/6–9/arrows/Space/E/G/D/C/Z stay exactly as the current keymap has them.

Chrome in Cull Grid at 1280: TopBar 44 + filter 32 + toolbar 32 = 108 (was 140 with the scene strip).

---

## 4. Edit step: scene plan, representative, Auto edit, Apply **P0**

### 4.1 Building the plan (first entry, or after keepers change)
1. Enter Edit → `getEditPlan(folderId)` **[ARCH v14]**. If it is `null` or `outdated`, call `buildEditPlan(folderId, keeperRule)`:
   the backend detects scenes over the keepers (if none exist), picks one representative per scene (the best keeper with the most
   typical lighting), and marks tiny scenes `minor`.
2. Plan header while building: `Grouping 212 keepers into lighting scenes… 45%` (progress from `sceneProgress{task:"detect"}` or a
   v14 plan-progress event), with a 200 px emerald progress bar. The list shows 6 skeleton rows (neutral-900 blocks with a
   pulse). The user can switch steps while this runs.
3. When keepers change later (a new pick or reject in Cull), the plan returns `outdated=true` and a 32 px amber-950 banner
   appears on top of the list: `3 new keepers and 1 removed since this plan was made. [Update plan]`. Updating never touches edits
   that were already applied.

### 4.2 Plan view (`components/edit/PlanView.tsx`, `data-testid="plan-view"`)

```
1280×800                                                                                         (TopBar 0–44)
y44  ┌ Plan header 48 px ─────────────────────────────────────────────────────────────────────────────────────────┐
     │ Edit · 14 scenes · 212 keepers  [Keepers: Picked ▾]   ████▓▓▓░░░░ 5 edited · 3 applied · 1 skipped · 5 to do  │
     │                                   [✦ Auto edit 5 remaining (my style)] [Apply 4 edited scenes (140) ▸]  [⋯]   │
y92  ├ Tabs 32 px: All 14 | To do 5 | Edited 4 | Applied 3 | Skipped 1                            Sort: Time ▾ ──────┤
y124 ├ Scene row 112 px + 8 gap (≈5.5 rows visible) ──────────────────────────────────────────────────────────────────┤
     │ ○ │┌─────────┐│ Scene 3                     │ ▫▫▫▫▫▫▫ +28               │        [Edit ▸] [✦ Auto edit] [⋯] │
     │   ││ rep     ││ 14:02–14:31 · 36 keepers    │ member thumbs 72×48        │                                  │
     │   ││ 144×96  ││ To do: edit this photo      │                            │                                  │
     │   │└─────────┘│ DSC00123.ARW                │                            │                                  │
     └───┴──────────┴─────────────────────────────┴────────────────────────────┴──────────────────────────────────┘
cols: 12 pad | status 28 | rep 144 | 12 | text 220 | 12 | strip flex (≈7 thumbs) | 12 | actions 280 | 12 pad
```
At 1728×1117: header 48, tabs 32, rows 140 (rep 180×120, member thumbs 90×60, text 260, actions 320), about 6.8 rows visible.

**Row content** (`plan-scene-{sceneId}`, `data-status`):
- Status icon (20 px), see the table below.
- Representative thumbnail: the rendered thumbnail (after edits) with a 2 px amber-400 ring and an "R" corner badge (amber-400 on black/70,
  10 px bold). With a second anchor, a smaller 48×32 thumbnail sits under it, labelled "2nd".
- Text block: `Scene 3` (13 px semibold neutral-100), `14:02–14:31 · 36 keepers` (12 px neutral-400), a status line (12 px, colour per
  state), and the rep filename (11 px neutral-500, truncated).
- Member strip: the other keepers in capture order, 72×48, 4 px gap, the last slot a `+28` chip (click opens Grid filtered to the scene).
  Applied members show a 10 px emerald `Check` in the corner; members that need a look show an amber `!`.
- Actions: right-aligned and depend on the state (table). ⋯ menu (every state): `Change representative…` (opens Develop on the scene
  with the hint chip "Press Shift+A on the photo you want"), `Add second reference photo`, `Apply with options…` (opens the existing
  MatchPanel), `Copy exactly (no matching)`, `Include photos I edited myself (2)` (toggle), `Show all 36 photos`, `Skip this scene`
  / `Include this scene`, and `Undo apply` (only when applied).

| State (`data-status`) | Icon | Status line copy | Primary | Secondary |
|---|---|---|---|---|
| `todo` | `Circle` neutral-500 | `To do: edit this photo` (neutral-400) | `Edit ▸` (sky-700) | `✦ Auto edit` |
| `auto` | `Wand2` amber-400 | `Auto edited (my style), review it` (amber-300) | `Review ▸` | `Apply to 35` |
| `edited` | `CircleDot` sky-400 | `Edited, ready to apply to 35` (sky-300) | `Apply to 35` (emerald-700) | `Edit ▸` |
| `applied` | `CheckCircle2` emerald-400 | `Applied to 35 · 3 need a look` / `Applied to 35` (emerald-300; the "need a look" part in amber-300) | `Review 3 ▸` (only when > 0) | `Edit ▸` |
| `stale` | `RefreshCw` amber-400 | `Changed since it was applied` (amber-300) | `Re-apply to 35` | `Edit ▸` |
| `skipped` | `MinusCircle` neutral-600 | `Skipped, no edit copied` (neutral-500; the row is at opacity 60) | `Include` (ghost) | — |
| `minor` (flag, any state) | as above | adds ` · small scene` | same | same |

"35" is the number of targets = keepers − representative − (photos the user edited themselves, unless included). The tooltip on
Apply spells it out: `Copies this edit to 35 photos, matching exposure and white balance to each. 2 photos you edited yourself are
kept.`

**Header**
- `Keepers: Picked ▾` (**[ARCH]** keeper rule in EditPlan) opens a menu: `Picked (212)`, `Picked or rated 1★ and up (260)`,
  `Everything not rejected (464)`. Default: Picked if the project has any picks, otherwise Everything not rejected.
- The segmented progress bar (200 px, 6 px tall, rounded) has segments applied (emerald-500), edited+auto (sky-500), skipped (neutral-600) and
  to do (neutral-800), followed by the count text.
- `✦ Auto edit N remaining (my style)` is secondary and runs on every `todo` representative (§4.5). It is hidden when N = 0.
- `Apply N edited scenes (M) ▸` is primary emerald and runs Apply for every `edited`, `auto` and `stale` scene in order. It is disabled with
  the tooltip `Edit or auto edit a scene first` when N = 0.
- When every scene is applied or skipped, the primary button becomes **`Continue to Export → 212`**.
- ⋯: `Auto edit every keeper individually (my style)` (P1, §4.5), `Retrain my style`, `Style: learned from 394 photos · 2 Oct`
  (disabled info row), `Rebuild plan…` (confirm dialog: "Scenes and representatives are chosen again. Edits are kept.").
- Tabs filter the rows. Counts come from the plan. Sort: `Time` (default) or `Most photos first`. Minor scenes always sort to the end,
  under a 24 px divider labelled `Small scenes (6 scenes, 9 photos)`, collapsed by default.

**Plan keys**: Up/Down move the row focus (2 px sky-500 ring, scrolled into view). **Enter** or **D** open the representative in
Develop. **Cmd+Alt+U** auto edits the focused scene. **Cmd+Shift+Enter** applies the focused scene. Cmd+Z undoes the last batch.
G in the Edit step always returns here.

### 4.3 Edit context bar in Develop (Edit step only; 32 px under the TopBar; `data-testid="edit-context"`)

```
1280: [‹][Scene 3 of 14 ▾][›]  (● Edited · representative)  Edit this photo, then apply it to the other 35.   [✦ Auto edit (my style)] [Apply to scene (35) ▾] [☰ Plan]
       24   150px          24    status chip ~180             hint 12 px neutral-400, hidden < 1440 px          150                       180 (emerald)            60
```
- `‹`/`›` go to the previous/next scene's representative (keys **Shift+N / N**, "next scene to edit": N skips `applied` and
  `skipped` scenes; Shift+N does not skip).
- `Scene 3 of 14 ▾` opens the **compact checklist popover** (360 px wide, max 480 px tall, scrolls): one 36 px row per scene with the status
  icon, `Scene N`, `36`, and the rep filename. Click jumps to that rep. The popover footer says `Open plan (G)`.
- Status chip, depending on the active photo:
  - the rep: shows the scene state (`To do`, `Auto edited, review`, `Edited`, `Applied`, `Changed since applied`);
  - another scene member: `From Scene 3's edit ✓` (emerald), or `Needs a look: exposure did not match` (amber, with the note from
    MatchPreview), then a `Next to review ›` link (key **N** reviews the next needs-look frame while one is open), or `Your own edit` (sky) when the user edited it;
  - not a representative and not applied: `Not the representative · Make it the representative (Shift+A)` (link).
- `Apply to scene (35)` is disabled for `todo` (tooltip `Edit this photo or auto edit it first`). The ▾ half holds the same items as the row ⋯
  (Apply with options…, Copy exactly, Include my own edits, Undo apply).
- In the Cull step this bar is absent, so Develop in Cull gets 32 px more photo.

**Representative changes**: in the Edit step, Shift+A on a non-representative member **replaces** the representative (it does not toggle a 2nd anchor).
The old rep keeps its edits. Toast: `Scene 3 representative: DSC00140 [Undo]`. On the current rep, Shift+A does nothing and shows the notice
`Already the representative`. A 2nd reference is added only through the menu. Maps to `setSceneAnchors` (anchorIds[0] = representative).

### 4.4 Filmstrip in the Edit step
- Source = the project's keepers, capture order. `This scene only` (Shift+S) narrows it to the current scene. It is **on by default** when
  Develop opens from the plan.
- At a scene boundary, draw a 2 px neutral-600 vertical rule. The first cell of each scene gets a `S3` badge (this reuses today's scene badge).
- Rep cell: 2 px amber-400 inset ring + "R". Applied: small emerald check, bottom-right. Needs a look: amber `!`, bottom-right.
  User-edited: the existing edit badge.

### 4.5 Auto edit (my style)

**Where**: Plan row secondary button; Edit context bar; plan header "Auto edit N remaining"; ⋯ "every keeper individually".

**Style model states** (`getStyleStatus()` **[ARCH v14]**: `{state, sampleCount, minSamples, trainedAtMs, newSinceTraining, progress}`):
| State | Button looks like | Click |
|---|---|---|
| `ready` | `✦ Auto edit (my style)` | runs it |
| `untrained` and `sampleCount ≥ minSamples` | same label | popover (320 px): **"Learn your style first?"** `Sieve learns from the 394 photos you have edited in this catalog, including Lightroom edits read from XMP. It takes about a minute and stays on this Mac.` `[Learn and auto edit]` `[Cancel]` |
| `training` | spinner + `Learning your style… 40%` (disabled) | — |
| `insufficient` | disabled, 50% opacity | tooltip: `Edit at least 20 photos (you have 6) so Sieve can learn your style. Photos edited in Lightroom count after "Read metadata from file".` |
| `error` | enabled | toast with the message and `[Retry]` |

**Behaviour**:
- On one scene: `predictStyle([repId])` → commit as one history entry **"Auto Edit (My Style)"** on the rep (Cmd+Z in Develop
  undoes it). The scene becomes `auto`. If the user is in Develop on the rep, the sliders update in place.
  If the rep already has user edits, ask first: `Replace your edit on DSC00123 with an auto edit? [Replace] [Cancel]`.
- "Auto edit N remaining": one batch over the `todo` reps. Plan header shows `Auto editing 3 of 5…`. Result toast `Auto edited 5 scenes. Review
  each one, then apply. [Undo]`.
- "Every keeper individually" (P1): confirm dialog `Auto edit 212 keepers one by one? Photos you edited yourself are skipped (14).
  This does not use scene matching.` → batch with progress in the plan header → scenes whose members all came from style
  become `applied`, with the status line `Auto edited individually`.
- Scene `auto` → `edited` happens automatically on the first manual change to the rep (backend tracks this through the history source).
- Apply is allowed straight from `auto`, so users who trust the model can do: Auto edit remaining → Apply all.

### 4.6 Apply to scene / Apply all / Review / Undo

**Apply to scene** (row, context bar, Cmd+Shift+Enter):
1. No modal. Call `applySceneEdit(sceneId, {mode:"match", includeUserEdited:false})` **[ARCH v14]**. The backend runs `match_scene` from the
   rep (and the 2nd reference) to the targets with `DEFAULT_MATCH_OPTIONS` (exposure + WB matched, all fields except crop & masks
   copied, strength 1) and commits atomically as one **batch**. It returns `{batchId, changedIds, needsReviewIds}`.
2. Progress: the button turns into a 180 px progress pill `Applying… 12/35` (emerald bar inside), driven by `sceneProgress{task:"match"}`.
   The row shows the same. `Esc` or the pill's × cancels **[ARCH: cancel_scene_apply]**; with no cancel support the × is hidden.
3. Done: row → `applied`. Toast (8 s): `Applied Scene 3 to 35 photos · 3 need a look [Review] [Undo]`. Without misses:
   `Applied Scene 3 to 35 photos [Undo]`.
4. Review: Develop on the first needs-look frame, filmstrip set to "This scene only", context chip in review mode, N = next.
   Fixing a needs-look frame by hand clears its `!` (backend: user edit after apply).

**Apply all edited scenes**: sequential batches under one umbrella batch (one Undo). Plan header shows `Applying 4 scenes · Scene 5 (2 of 4) ·
40%` with Cancel (stops after the current scene; finished scenes stay applied). Toast: `Applied 4 scenes to 140 photos · 7 need a look
[Review] [Undo]`.

**Copy exactly (no matching)** = paste the rep's settings with the remembered Copy-Settings fields to the targets, with no normalisation (Lightroom Sync
semantics). Same batch, toast and undo.

**Apply with options…** = the existing `MatchPanel` (strength, fields, per-photo include), unchanged except that its
"Copy from anchor" field picker uses the new grouped Copy Settings component (§5.6) and its Apply commits through the same batch path so undo works.

**Undo rules**
- Toast `[Undo]` and **Cmd+Z** (when the batch is the newest undoable action, in any view): `revertBatch(batchId)` **[ARCH v14]**.
  It restores each photo's pre-batch adjustments. Photos edited again since then are left alone and reported:
  `Undid Apply Scene 3 on 33 photos · 2 changed since, kept`.
- The row ⋯ `Undo apply` works later too, as long as it is the scene's last apply.
- The global undo order stays "newest first" across culling, per-photo adjustments and batches (extend the existing
  `lastCommitAt` comparison in `App.tsx` with the batch timestamp).
- Replace today's `undoBatch` loop (one `undoAdjustments` per photo) with `revertBatch` for paste, sync, preset-to-many and reset-many
  too. The current loop undoes the wrong entry if the user edited a target in between.

**Empty and error states** (Plan view body, centred, max-width 420, 14 px neutral-300 + 12 px neutral-400 + buttons):
| Case | Copy | Buttons |
|---|---|---|
| no keepers | **No keepers yet.** Pick photos in Cull (P) or choose which photos count as keepers. | `[Back to Cull]` `[Use all photos not rejected (464)]` |
| project has no photos | **This shoot has no photos.** | `[Import shoot folder…]` |
| plan build failed | **Could not group the keepers into scenes.** + error message | `[Try again]` |
| one scene only | (normal list, 1 row; no special case) | — |
| every scene done | row list + header success line `All 14 scenes done` | `[Continue to Export → 212]` |
| No project selected ("All photos") | **Pick a shoot to edit.** Steps work per shoot folder. | project picker opens |

---

## 5. Develop: Lightroom Classic layout **P0** (styling details P1 where marked)

### 5.1 Layout at 1280×800 (Edit step; the Cull step has no context bar → +32 px viewer)

```
y 0 ┌──────────────────────────────── TopBar 44 ───────────────────────────────────────────────────────────────┐
y44 ├──────────────────────────────── Edit context bar 32 ─────────────────────────────────────────────────────┤
y76 ├── Left 224 ───────┬────────────────── Viewer 768 × 590 ──────────────────┬── Right 288 ─────────────────┤
    │ NAVIGATOR FIT 100%│                                                     │ Histogram 100 + info line 20 │
    │ [224×149 preview] │                                                     ├ Tool strip 32: [⌗ Crop][◌ Mask]│y196
    │ PRESETS        [+]│                                                     ├ (crop drawer when active)    │y228
    │  ▸ User Presets 4 │                                                     │ ▾ BASIC                   ⟲ │
    │  ▸ VSCO Film 01 48│                 photo                                │   Treatment [Color][B&W]     │
    │ SNAPSHOTS      [+]│                                                     │   Profile [Adobe Color ▾][⊞] │
    │ HISTORY        [×]│                                                     │   WB [🖉] [As Shot ▾]         │
    │  Exposure +0.35   │                                                     │   Temp ────o──── 5200       │
    │  Import (…)       │                                                     │   Tint ─────o─── +8          │
    │                   │                                                     │   Tone ─────────── [Auto]    │
    │                   ├─ Viewer toolbar 32 ──────────────────────────────────┤   Exposure ──o── 0.00 …      │
y666│                   │[◧ \][▥ Y][Fit|100%] ⚑ ✕ ★★★☆☆ ●  DSC00123.ARW  ⚠ │   Presence …                 │
    ├ [Copy…] [Paste] 36├──────────────────────────────────────────────────────┤ [Previous|Sync…] [Reset] 36 │y662
y698├──────── Filmstrip header 22 ───────────────────────────────────────────────────────────────────────────┤
y720├──────── Filmstrip 80 (cells 72×72) ────────────────────────────────────────────────────────────────────┤
y800└────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```
Panel row = y 76–698 (622 px). Viewer 590 tall (was 571). Right sections scroll area = 622 − 120 − 32 − 36 = **434 px**.

### 5.2 Layout at 1728×1117
TopBar 44 · context 32 · panel row y 76–999 (**923 px**) · filmstrip header 22 · filmstrip 96 (cells 88×88).
Left **240** (at ≥1600 px; 224 below) · viewer **1168 × 891** (was 1184×888 with more chrome; this layout adds the Navigator and keeps the photo size) ·
right **320**. Right sections area = 923 − 120 − 32 − 36 = **735 px**, which fits Basic fully open (≈470 px) plus two closed headers.

### 5.3 Left panel (`LeftPanel.tsx`, top → bottom; whole panel scrolls, bottom bar sticky)
| # | Section | Default | Content |
|---|---|---|---|
| 1 | **Navigator** (`section-navigator`) **P1** | open | Header `NAVIGATOR` + right-aligned text toggles `FIT` `100%` (11 px, the active one neutral-100, the other neutral-500). Body: the photo preview fitted in width × 2/3 (224×149 / 240×160) on neutral-950. When zoomed, a 1 px white rectangle with a black/40 outside veil shows the visible region; click/drag pans (`setZoom`). **Preset / profile hover preview is shown here** (Lightroom behaviour): after 150 ms of hovering a preset, render it at maxEdge 480 and show it with a 10 px label `Preview: <name>`; mouse-leave restores. |
| 2 | **Presets** **P0** | open | See §7. Header `PRESETS` + `+` icon (menu: `Create Preset…  ⇧⌘N`, `Import Presets & Profiles…`, `Reset hidden groups` (P2)). |
| 3 | **Snapshots** **P1 [ARCH]** | closed | Header `SNAPSHOTS` + `+` (⌘N). `+` adds an inline 24 px text input prefilled with `2026-09-30 14:32`, Enter saves. Rows: name; click applies it (history "Snapshot: <name>"); context menu `Update with current settings`, `Rename`, `Delete`. Empty: `No snapshots. Save the current look with + (Cmd+N).` Needs `list/create/update/delete_snapshot(imageId…)` (not in the v14 list; add it or ship the section later). |
| 4 | **History** | open | Unchanged list. Header: undo/redo icons stay; add `×` "Clear history" only if the backend supports it (skip otherwise, no work invented). |
| — | **Bottom bar** (36 px, border-top neutral-800, px-3, two equal buttons h-7 gap-2) **P0** | — | `Copy…` (`copy-settings`) and `Paste` (`paste-settings`). Alt held: `Copy…` reads `Copy` and copies with the remembered fields without the dialog. Paste tooltip when enabled: `Paste 24 settings from DSC00012 to 3 photos (Cmd+Shift+V)`; when disabled: `Copy settings first (Cmd+Shift+C)`. |

Remove from the old toolbar: `Library` back button (G / step bar), Copy, Paste, Sync, Reset, Before, Split, 100% (these move, below).

### 5.4 Right panel (`AdjustPanel.tsx`), exact order
1. **Histogram** (always shown, not collapsible): 100 px graph + a 20 px info line `ISO 800 · 85 mm · f/1.8 · 1/250 s` (11 px
   neutral-400, from EXIF on the entry; hidden when missing). P2: clipping triangles in the top corners, toggled by J.
2. **Tool strip** (32 px, px-3, gap-1, border-bottom) **P0**: `[Crop icon]` (R) and `[CircleDashed icon]` Masking (Shift+W; badge with the mask
   count). Icon buttons 28×28, the active one bg-sky-800. **The Adjust / Masks tabs are removed.**
   - Crop active: the Crop drawer (today's `CropPanel` content plus `Close` and `Reset` buttons at its bottom) appears right under the strip, and the
     sections stay below it (Lightroom). The `crop` section is removed from the section list.
   - Masking active: `MasksPanel` replaces the section list until closed (strip icon, Esc cascade, or Shift+W).
3. **Basic** (`section-basic`, open by default), rows top → bottom:
   - `Treatment` + segmented `[Color] [Black & White]` (moved from Color Mixer; V still toggles).
   - `Profile` + `[Adobe Color ▾]` (dropdown: Favorites (P2), 5 most recent, a divider, `Browse…`) + `[⊞]` 24×24 opening the
     Profile Browser (§7.2). Amount slider (0–200%) under it when the current profile supports it.
   - `WB` + `[Pipette]` 24×24 (W) + `[As Shot ▾]` select: `As Shot`, `Auto`, `Custom` (P2: `Daylight 5500/+10`, `Cloudy 6500/+10`,
     `Shade 7500/+10`, `Tungsten 2850/0`, `Fluorescent 3800/+21`, `Flash 5500/0`; RAW only). Choosing `Auto` calls
     `autoWhiteBalance` (§8). Editing Temp/Tint switches the label to `Custom`.
   - Temp, Tint.
   - Sub-header row `Tone` (11 px uppercase neutral-400) with **`Auto`** button right-aligned (h-5, px-2, 11 px, bg-neutral-800) (Cmd+U).
   - Exposure, Contrast, Highlights, Shadows, Whites, Blacks.
   - Sub-header `Presence`: Texture, Clarity, Dehaze, Vibrance, Saturation (the separate Presence section is removed).
4. **Tone Curve** (closed)
5. **HSL / Color** (closed; header reads `B&W` when the treatment is B&W, with the B&W mixer). Tabs `Hue | Saturation | Luminance`.
6. **Color Grading** (closed)
7. **Detail** (closed)
8. **Effects** (closed)
9. **Calibration** (closed)
10. **Bottom bar** (36 px, sticky, two equal buttons): left `Previous` when ≤ 1 photo is selected (Cmd+Alt+V; disabled with the tooltip `No previous
    photo yet` when none), which becomes **`Sync…`** when > 1 photo is selected (Cmd+Shift+S; Alt held: `Sync` without the dialog = Cmd+Alt+S).
    Right `Reset` (Cmd+Shift+R; label `Reset (3)` when several are selected). P2: an Auto Sync switch (Cmd+Alt+Shift+A) left of Sync….

Removed: the `LUT` section (`.cube` files become profiles, §7.2), the `Profile` and `Presence` sections, the `crop` section.
Section state: bump the storage key to `sieve.develop.sections.v2` so everyone gets these defaults once. Section ids in order:
`basic, tone-curve, hsl, color-grading, detail, effects, calibration`. Alt-click solo stays.
P2: Cmd+1 Basic, Cmd+2 Tone Curve, Cmd+3 HSL/Color, Cmd+4 Color Grading, Cmd+5 Detail, Cmd+8 Effects, Cmd+9 Calibration
toggle sections (Lightroom numbers; 6 and 7 are left free for LR's Lens/Transform).

### 5.5 Slider row (Lightroom single-line) **P1**
Change `Slider.tsx` to one 24 px row: label (72 px, 12 px neutral-300, left, truncate) · track (flex, 2 px neutral-700, thumb 10 px white
with a 1 px neutral-900 border, accent fill from default to value as today) · value (44 px, right-aligned tabular, 12 px, click-to-type as today).
Changed values render the label in neutral-100. Double-click resets. **Shift+double-click = auto for that slider** (§8). Mask sliders use the same row.
Result: Basic ≈ 470 px (was ≈ 600 with two-row sliders); at 1280 about 90% of it is visible without scrolling.

### 5.6 Viewer toolbar (32 px, bottom of the viewer column, Lightroom position) **P1**
Left→right: `[Columns2] Before` (\) toggle · `[SplitSquareHorizontal] Split` (Y) · segmented `Fit | 100%` (Z / Space) · 12 px gap ·
clickable pick flag and reject X (P / X) · clickable stars (0–5; clicking the current rating clears it) · color label dot (menu) ·
filename (12 px neutral-300, truncate, `title` = full path) · right: `WarningsChip`. Remove the `768x512 · 9 ms` render readout from the UI
(keep it in `data-*` for tests). Shift+Tab hides this toolbar with the rest of the chrome, as today.

### 5.7 Copy Settings dialog (`SettingsFieldsDialog`, replaces `FieldsDialog`) **P0**
Shared by **Copy Settings**, **Synchronize Settings**, **New Develop Preset** and Match "Copy from anchor".

- Size: 680 px wide (3 × 200 px columns + 2 × 20 px gaps + 2 × 20 px padding); height auto (~520 px); centred; bg-neutral-900, 1 px neutral-700 border, radius 8.
- Title (14 px semibold): `Copy Settings` / `Synchronize Settings` / `New Develop Preset`.
- Preset variant only, above the columns: `Preset Name [________]` (autofocus) and `Group [User Presets ▾]` (user groups only).
- Rows 22 px, 12 px. Parent rows semibold with a **tri-state** checkbox (checked / indeterminate / unchecked; clicking an indeterminate
  parent checks all its children). Children indented 20 px.

| Column 1 | Column 2 | Column 3 |
|---|---|---|
| ☐ White Balance `white_balance` | ☐ **Treatment & Profile** | ☐ **Masking** `masks` (disabled with `(none)` when the photo has none; P2 **[ARCH]**: one child per mask group, like Lightroom) |
| ☐ **Basic Tone** | ⠀☐ Profile `profile` | ☐ Crop `crop` |
| ⠀☐ Exposure `exposure` | ⠀☐ Treatment (Black & White) `black_and_white` | |
| ⠀☐ Contrast `contrast` | ☐ Sharpening `sharpening` | |
| ⠀☐ Highlights `highlights` | ☐ Noise Reduction `noise_reduction` | |
| ⠀☐ Shadows `shadows` | ☐ **Effects** | |
| ⠀☐ White Clipping `whites` | ⠀☐ Post-Crop Vignetting `vignette` | |
| ⠀☐ Black Clipping `blacks` | ⠀☐ Grain `grain` | |
| ☐ Tone Curve `tone_curve` | ☐ Calibration `calibration` | |
| ☐ Texture `texture` | (☐ LUT `lut`, only while legacy LUT refs exist **[ARCH v14 decides]**) | |
| ☐ Clarity `clarity` | | |
| ☐ Dehaze `dehaze` | | |
| ☐ **Color** | | |
| ⠀☐ Vibrance `vibrance` | | |
| ⠀☐ Saturation `saturation` | | |
| ⠀☐ Color Mixer: Hue `hsl_hue` | | |
| ⠀☐ Color Mixer: Saturation `hsl_saturation` | | |
| ⠀☐ Color Mixer: Luminance `hsl_luminance` | | |
| ☐ Color Grading `color_grading` | | |

- Footer (52 px): left text buttons `Check All` `Check None` `Check Modified` (the last ticks only groups that differ from the photo's defaults);
  right `Cancel` (neutral-800) and the primary (`Copy` / `Synchronize` / `Create`, sky-700).
- **Remembered choice**: Copy and Sync share `localStorage["sieve.copyFields.v1"]`; the preset dialog uses `sieve.presetFields.v1`. The first time,
  Copy/Sync default to all except Crop and Masking (`DEFAULT_SYNC_FIELDS`) and the preset dialog to `Check Modified`.
- Focus: Copy/Sync autofocus the **primary button**, so `Cmd+Shift+C`, `Enter` copies in two key presses. Tab walks the checkboxes
  column by column, Space toggles, Enter confirms, Esc cancels (the existing `Dialog` modal system). Culling keys stay blocked while it is open.
- After Copy: toast `Copied 24 settings from DSC00123`. After Paste: `Pasted 24 settings to 3 photos [Undo]`.
- Keep the existing `data-testid`s (`fields-dialog`, `field-<name>`, `fields-all`, `fields-none`, `fields-confirm`, `fields-cancel`) and
  add `fields-modified` and `field-group-<basic_tone|color|treatment|effects>`.

---

## 6. Export step **P1**
- Step 3 / Cmd+Shift+E opens the existing Export dialog. Add a first scope radio **`Keepers (212)`** (default when opened from step 3 or
  when the Edit step is active), before `Selection` and `All filtered`. `Skip rejected` stays.
- Title from step 3: `Export 212 keepers`.
- While exporting, the step 3 pill shows the ring and `Exporting 45%`. Clicking it opens the jobs panel. When finished, the pill reads `Exported 212`, and
  the jobs panel keeps `Show in Finder` (exists).
- No other changes to the dialog internals (reviewed in earlier rounds; no open P0/P1 there).

---

## 7. Presets & profiles library (UI side of the v14 library) **P0** (hover previews P1)

### 7.1 Presets panel (left)
- Groups in this order: `User Presets` (Sieve-saved), then one group per imported source folder, named by relative path (`VSCO / Film 01`;
  nesting capped at 2 levels, deeper folders flattened into the path). Group row: chevron, folder icon (imported) or user icon, name, muted count.
  `User Presets` is open by default and imported groups are closed; the open/closed state is remembered (`sieve.presetGroups.v1`).
- Item row (24 px): name (truncate). Hover shows `→ 3` when several photos are selected (exists), and a 150 ms dwell starts the **Navigator hover
  preview** (§5.3) using `resolvePreset(imageId, presetId)` **[ARCH v14]**. Items with ignored keys show a muted `partial` tag whose tooltip names
  what is ignored (e.g. `Lens Corrections not supported, ignored`) **[ARCH: PresetInfo.unsupported]**.
- Click applies to the targets (active photo, or the multi-selection) → history `Preset: <name>`. Many photos → batch with toast + Undo.
- Context menu: user presets have `Update with Current Settings`, `Rename`, `Delete`. Imported groups have `Remove group from Sieve` (confirm:
  "Presets are removed from Sieve only; the files are not touched.").
- Import: `+` → `Import Presets & Profiles…` → directory picker → `importPresetFolder(path)`. While it runs, the header shows `Importing…` with a
  spinner. Result toast: `Imported 146 presets in 9 groups and 32 profiles · 3 files skipped [Details]`. Details opens a dialog listing
  skipped files with their reasons. The imported items appear **in every project** (catalog-wide).
- Empty state (no presets at all): `No presets yet. Save one with + or import a Lightroom presets folder.` + link button `Import presets & profiles…`.
- P2: a filter input (24 px) at the top of the section when there are more than 50 presets.

### 7.2 Profile Browser (right panel)
- `⊞` in Basic replaces the section list with the browser (Lightroom): header `Profile Browser` + `Close` (Esc). Filter `All ▾`
  (`All`, `Color`, `B&W`). Amount slider (0–200%) pinned under the header when the current profile supports it.
- Groups: `Adobe Raw`, `Camera Matching`, `Legacy` (as today), then the imported folders (same names as the preset groups). `.cube`
  files are items in their folder's group with a muted `LUT` tag, and `.dcp` and look profiles get `DCP` / no tag. Rows are 28 px (P2: 3-column
  thumbnail grid rendered with each profile, as Lightroom does).
- Hover an item for 150 ms → the **main viewer** previews it (render with that profile; label `Preview: <name>` top-left); mouse-leave restores it.
  Click applies it (`Profile: <name>`, Amount reset to 100%).
- Footer: `Import Presets & Profiles…` (same command as §7.1).
- Migration: photos with a legacy `lut` show it as the current profile `LUT: <name>` (**[ARCH v14]** decides the conversion).

---

## 8. Auto Tone / Auto White Balance **P0**
- `Auto` (Tone row) and **Cmd+U**: `autoTone(imageId, adjustments)` **[ARCH v14]** returns Exposure, Contrast, Highlights, Shadows,
  Whites, Blacks, Vibrance and Saturation. Commit as **one** history entry `Auto Tone`. While waiting, the button shows a 12 px spinner and is disabled;
  sliders jump to the new values when it resolves. On error, show a toast with the message.
- WB `Auto` in the dropdown and **Cmd+Shift+U**: `autoWhiteBalance(imageId, adjustments)` → `{temperatureK, tint}` → commit
  `whiteBalance: {mode:"custom", …}` with the label `White Balance: Auto` and the dropdown showing `Auto` until Temp/Tint are touched
  (needs `WhiteBalance` mode `auto`, or a frontend-only label flag; **[ARCH]** choose one).
- **Shift+double-click** on a slider label or thumb applies auto to that one slider: call `autoTone` and take only that key (Temp/Tint → `autoWhiteBalance`,
  that key only). History label `Auto: Exposure`.
- Tooltips: `Auto (Cmd+U)`, `Auto white balance (Cmd+Shift+U)`. Cheat sheet entries are added through `keymap.ts`.
- Several photos selected: Auto applies to the active photo only (Lightroom applies it to all selected only with Auto Sync, which is P2).

---

## 9. Keyboard map (existing vs proposed; conflicts resolved)

Existing entries come from `src/lib/keymap.ts` plus the in-flight small-fixes/Compare branch (`fe-8b-fixes`).

| Action | Existing | Proposed | Scope | Notes |
|---|---|---|---|---|
| Pick / Reject / Unflag | P / X / U (Shift = advance) | same | all | — |
| Stars / labels | 0–5 / 6–9 | same | all | — |
| Prev / next photo | ← / → | same | all | Compare uses ↑/↓ for swap/make-select (fe-8b-fixes) |
| Grid / Loupe / Compare / Develop | G / E, Space / C / D | same; **G in the Edit step → Plan** | all | Plan is the Edit step's overview |
| Step: Cull / Edit / Export | — | **Cmd+Alt+1 / Cmd+Alt+2 / Cmd+Alt+3** | all | mirrors LR module keys; Cmd+1–9 is kept for panels |
| Next / previous scene to edit | — | **N / Shift+N** | Edit step (Plan, Develop) | N is unbound today; while reviewing, N = next needs-look frame |
| Apply to scene | — | **Cmd+Shift+Enter** | Edit step | Cmd+Enter stays the Export dialog's confirm |
| Auto edit (my style) | — | **Cmd+Alt+U** | Edit step, Develop | next to Cmd+U / Cmd+Shift+U |
| Make representative | Shift+A = toggle anchor | **Shift+A = make representative** in the Edit step; toggle anchor elsewhere | all | the 2nd reference is added through the menu only |
| Scene strip / "This scene only" | Shift+S (fe-8b-fixes) | same; in the Edit step it toggles the filmstrip's `This scene only` | all | same meaning: scene filter |
| Auto Tone | — | **Cmd+U** | Develop | LR |
| Auto White Balance | — | **Cmd+Shift+U** | Develop | LR |
| Auto one slider | — | **Shift+double-click** | Develop | LR |
| Copy settings… | Cmd+Shift+C | same (Enter confirms right away) | Develop | LR |
| Copy without dialog | — | **Alt-click `Copy…`** | Develop | Sieve addition |
| Paste settings | Cmd+Shift+V | same | Develop, Grid | LR |
| Paste from previous | Cmd+Alt+V | same; button `Previous` | Develop | Excludes crop & masks (deliberate: per-frame geometry); the tooltip says so |
| Sync… / Sync without dialog | Cmd+Shift+S / — | same / **Cmd+Alt+S** | Develop | LR |
| Auto Sync toggle | — | Cmd+Alt+Shift+A (**P2**) | Develop | LR |
| Reset | Cmd+Shift+R | same | Develop | LR |
| New preset | Cmd+Shift+N | same | Develop | LR |
| New snapshot | — | **Cmd+N** (P1, needs backend) | Develop | LR |
| Before/after · Split | \ · Y | same | Develop | LR |
| Crop · Masking · WB picker · B&W | R · Shift+W · W · V | same | Develop | LR |
| Toggle panels | Cmd+1…9 unbound | **P2**: Cmd+1 Basic, 2 Tone Curve, 3 HSL/Color, 4 Color Grading, 5 Detail, 8 Effects, 9 Calibration | Develop | LR numbering |
| Clipping indicators | — | **J** (P2) | Develop | LR; J is unbound (Shift+J = color range) |
| Undo / Redo | Cmd+Z / Cmd+Shift+Z | same; now also covers **batches** (newest first) | all | §4.6 |
| Save XMP · Export · Import · Cheat sheet | Cmd+S · Cmd+Shift+E · Cmd+Shift+I · ? | same | all | — |

Conflict check: N, Cmd+U, Cmd+Shift+U, Cmd+Alt+U, Cmd+Alt+S, Cmd+Alt+1/2/3, Cmd+Shift+Enter and J are unbound in the current keymap and in the
fe-8b-fixes diff. Shift+A changes meaning only inside the Edit step, and the tooltip and cheat sheet say so (`where: "Edit step: make representative; elsewhere: toggle anchor"`).
All new entries go through `KEYMAP` so `hint()` and the `?` sheet pick them up. Add a cheat-sheet group `Workflow` for the step, plan and apply keys.

---

## 10. Data each element needs

| UI element | Data / command | Status |
|---|---|---|
| Project picker | `CatalogState.folders` (`id, path, imageCount`) + per-folder step and plan summary | **[ARCH v14]** add `FolderEntry.workflowStep` (or a `listWorkflows()`) and `editSummary {scenes, done}` |
| Step bar counts | keepers count; analysis progress (`useBackendStatus`); export progress (`useExportJobs`); `WorkflowStep` | **[ARCH v14]** `getWorkflow(folderId)` / `setWorkflowStep(folderId, step)`; keepers count from `EditPlan.keeperCount` |
| Cull "Continue to Edit" | `setWorkflowStep(folderId,"edit")` | v14 |
| Plan view | `getEditPlan(folderId) → EditPlan {folderId, keeperRule, keeperCount, outdated, scenes: EditPlanScene[]}`; `EditPlanScene {sceneId, number, startedAtMs, endedAtMs, keeperIds, representativeId, secondAnchorId, status: todo\|auto\|edited\|applied\|stale\|skipped, minor, appliedCount, needsReviewIds, userEditedIds, lastBatchId}`; `buildEditPlan(folderId, keeperRule)` + progress event; `edit-plan-changed` event | **[ARCH v14]**. The keeper rule (picked / picked+1★ / not rejected) is part of the contract. |
| Change representative | `setSceneAnchors(sceneId, [newRep, …])` (exists) → plan re-reads | exists; the plan must derive `representativeId = anchorIds[0]` |
| Skip scene | `setSceneSkipped(sceneId, bool)` | **[ARCH v14]** |
| Apply to scene / Apply all | `applySceneEdit(sceneId, {mode:"match"\|"exact", includeUserEdited}) → {batchId, changedIds, needsReviewIds}`; `applyEditedScenes(folderId) → {batchId, …}`; progress via `sceneProgress{task:"match"}`; optional `cancelSceneApply()` | **[ARCH v14]**. Built on `matchScene` + `applySceneMatch` (exist). Exposure/WB are normalised per frame, as the roadmap requires. |
| Undo batches | `revertBatch(batchId) → {reverted, keptChanged}` | **[ARCH v14]** (new; needed for correct undo of every multi-photo op) |
| Needs-look / applied / user-edited markers | per-image edit source (`none\|user\|applied\|auto\|pasted`) or the plan's id lists | **[ARCH v14]** |
| Auto edit (my style) | `getStyleStatus()`, `trainStyle()` + progress event, `predictStyle(imageIds) → ParametricAdjustments[]` or `applyStyle(imageIds) → {batchId}` | **[ARCH v14]** style-model commands |
| Auto Tone / Auto WB | `autoTone(id, adjustments) → partial adjustments (8 keys)`; `autoWhiteBalance(id, adjustments) → WhiteBalanceValues` | **[ARCH v14]**; decide whether WB mode `auto` exists |
| Copy / Paste / Sync / Previous | `pasteSettings` / `syncSettings` (exist); `pastePrevious(targetIds)` (v14; persists "previous" across sessions, replacing the module variable `previousId`) | exists + v14 |
| Copy dialog groups | `ALL_ADJUSTMENT_FIELDS`, `ADJUSTMENT_FIELD_LABELS` (exist; the grouping is frontend-only). Per-mask children need a finer field or mask-id list | per-mask: **[ARCH]** P2 |
| Presets panel | `listPresetLibrary() → {groups: [{id, name, kind: user\|imported, sourcePath, presets: PresetInfo[]}]}`, `importPresetFolder(path) → ImportReport {presets, profiles, groups, skipped:[{file, reason}]}`, `applyPreset(ids, presetId)` (only the keys the preset contains), `resolvePreset(imageId, presetId) → ParametricAdjustments` (hover preview), `removePresetGroup(id)`; `PresetInfo.unsupported: string[]` | **[ARCH v14]** |
| Profile browser | `listProfiles(imageId)` extended with imported groups, `kind: dcp\|look\|cube`, `supportsAmount` (0–200) | **[ARCH v14]** |
| Snapshots | `list/create/update/delete_snapshot` | **[ARCH]** not in the v14 list; add it or defer the section |
| Navigator | thumbnail/preview URL (exists), zoom state (exists), `renderPreview` at 480 for hover (exists) | exists |
| Histogram info line | EXIF fields on `RawImageEntry` (iso, focal, aperture, shutter), if present | check; hide when absent |
| Export "Keepers" scope | `EditPlan.keeperRule` → `ImageQuery` (or the plan's keeper ids) | v14 |

---

## 11. Ranked findings and work items

### P0: blocks the requested workflow
1. **P0-1 Step bar + project picker** (§2.1–2.2). Where: `TopBar.tsx`, new `StepBar.tsx`; screenshot `1280-01-grid.png` (no steps; the TopBar is full).
   Why: the user asked for AfterShoot-style steps, and there is no sense of "where am I in the job". **[ARCH v14 WorkflowStep]**
2. **P0-2 Edit Plan (scene checklist)** with representatives, states, tabs, header progress and empty states (§4.1–4.2, 4.6). New
   `components/edit/PlanView.tsx`. Screenshot `1280-02-grid-scenes.png` shows today's replacement: an overflowing chip row with no status. **[ARCH v14 EditPlan]**
3. **P0-3 Apply to scene / Apply all / Review / batch Undo** without a modal (§4.3, 4.6). `SceneStrip.tsx`, `MatchPanel.tsx`
   (kept as "Apply with options…"), `App.tsx` undo. Screenshot `1280-10-match-panel.png` (an empty modal and 3 more clicks today). **[ARCH v14 applySceneEdit, revertBatch]**
4. **P0-4 Auto edit (my style)** entry points and model states (§4.5). **[ARCH v14 style commands]**
5. **P0-5 Copy… / Paste in the Lightroom position** (left-panel bottom) and **Previous|Sync… / Reset** (right-panel bottom) (§5.3, 5.4).
   `DevelopView.tsx` toolbar → `LeftPanel.tsx` / `AdjustPanel.tsx`. Screenshot `1280-03-develop.png`. Why: the user could not find copy/paste.
6. **P0-6 Grouped Copy Settings dialog** with tri-state groups, Check All/None/Modified, a remembered choice and the primary button autofocused (§5.7).
   `FieldsDialog.tsx`. Screenshot `1280-06-copy-dialog.png`.
7. **P0-7 Lightroom right-panel order**: histogram → tool strip (Crop, Masking; tabs removed) → Basic (Treatment, Profile+browser, WB with
   Auto, Tone with Auto, Presence) → Tone Curve → HSL/Color → Color Grading → Detail → Effects → Calibration; Basic open, the rest closed; LUT, Profile,
   Presence and Crop sections removed (§5.4). `AdjustPanel.tsx`, `sections.ts`. Screenshots `1280-03/04/05`, `1280-08-masks-tab.png`.
8. **P0-8 Presets library UI**: folder groups, import folder, catalog-wide availability, apply-only-contained-keys (§7.1), and
   `.cube` items as profiles (§7.2 list form). **[ARCH v14]**
9. **P0-9 Auto Tone / Auto WB** buttons + Cmd+U / Cmd+Shift+U, one undo step (§8). **[ARCH v14]**

### P1: noticeable friction or polish gap
1. **P1-1 Single-row Lightroom sliders** (§5.5). `Slider.tsx`. Basic from ≈600 px to ≈470 px.
2. **P1-2 Viewer toolbar at the bottom** with clickable flags, stars and label; remove the render-ms readout and the Library back button (§5.6).
3. **P1-3 Filmstrip header** replacing the Develop filter-summary row; Edit-step scene markers (§2.3, 4.4). Recovers 28 px.
4. **P1-4 Navigator** with FIT/100%, pan rectangle and preset hover preview (§5.3).
5. **P1-5 Profile Browser as a full-panel view** with main-viewer hover preview; Amount 0–200 (§7.2). Screenshot `1280-07-profile-browser.png`
   (today it expands inline and pushes Basic off screen).
6. **P1-6 Snapshots section** (§5.3). **[ARCH]** not in the v14 list.
7. **P1-7 Cull context bar**: Shoot + Analyze moved in, `Continue to Edit`, scene strip hidden in Cull, folder select removed (§3).
8. **P1-8 Export "Keepers" scope** and step-3 progress/finished states (§6).
9. **P1-9 Shift+double-click auto slider**, and `Sync` without the dialog (Cmd+Alt+S / Alt-click) (§8, §9).
10. **P1-10 Auto edit every keeper individually** (§4.5).
11. **P1-11 Small scenes grouped** at the end of the plan (§4.2).

### P2: nice to have
1. WB presets (Daylight … Flash) in the WB dropdown (§5.4).
2. J clipping indicators on the histogram and viewer.
3. Auto Sync switch next to Sync… (Cmd+Alt+Shift+A).
4. Per-mask checkboxes under Masking in the Copy dialog **[ARCH]**.
5. Profile thumbnails grid; Favorites; preset filter field.
6. Cmd+1…9 section toggles.
7. Preset Amount slider (Lightroom 11+).

### Architect summary (contract items raised by this spec)
WorkflowStep per folder (+ folder summary); EditPlan (keeper rule, per-scene rep/status/minor/needs-review/user-edited ids,
outdated flag, build progress); `setSceneSkipped`; `applySceneEdit` / `applyEditedScenes` returning `batchId`; `revertBatch`
(used by every multi-photo op); optional `cancelSceneApply`; per-image edit source; style status/train/predict(or apply) with progress;
`autoTone` / `autoWhiteBalance` taking the current adjustments, plus the WB `auto` mode decision; `pastePrevious`; preset library groups,
`importPresetFolder` report, `resolvePreset`, `PresetInfo.unsupported`, `removePresetGroup`; profile kinds incl. `cube` with Amount 0–200 and
the legacy `lut` migration; snapshots (not yet listed); per-mask copy (P2).

---

## 12. Areas that need no change
- The Masks panel content and tools (only its entry point moves from a tab to the tool strip). The overlay/menus work belongs to the separate Masks UX task.
- The crop tool itself (only its drawer position changes), the WB picker, slider typing and double-click reset, the History list behaviour, and section solo (Alt-click).
- Export dialog internals (apart from the Keepers scope), the jobs panel, and Show in Finder.
- Culling keys and Grid/Loupe behaviour. The Compare-in-Develop work is specified by its own task and is not respecified here.
- Keymap architecture (`KEYMAP` → `hint()` → cheat sheet): only new entries are needed.

## 13. Test hooks for QA (new `data-testid`s)
`step-bar`, `step-cull`, `step-edit`, `step-export` (with `data-state="active|done|upcoming"`), `project-picker`, `continue-edit`,
`plan-view`, `plan-header`, `plan-progress`, `plan-tab-<todo|edited|applied|skipped|all>`, `plan-scene-<id>` (`data-status`),
`plan-apply-<id>`, `plan-auto-<id>`, `plan-apply-all`, `plan-auto-remaining`, `plan-keepers-rule`, `edit-context`, `edit-scene-menu`,
`edit-apply`, `edit-auto`, `tool-crop`, `tool-masking`, `section-navigator`, `section-snapshots`, `copy-settings`, `paste-settings`,
`previous-settings`, `sync-settings`, `reset-all`, `auto-tone`, `wb-select`, `profile-browser`, `preset-group-<id>`, `preset-import`,
`fields-modified`, `field-group-<id>`, `filmstrip-header`, `viewer-toolbar`.
