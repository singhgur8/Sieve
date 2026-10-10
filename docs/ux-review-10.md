# Phase 10 UX review: Baseline edit (one preset + one photo -> the whole shoot, finish in Lightroom)

Author: ux-designer · 2026-10-10 · branch `phase-10-baseline-edit` at d0b77fe
For: frontend-dev (all items), architect (items marked **[ARCH]**)

Evidence: mock backend (`vite --port 1468`), throwaway Playwright drivers at 1280×800 and 1440×900 on
`?mock=201&style=ready&baseline=presets|anchor|1` (project 1 "ceremony", 43 keepers in 3 scenes). I walked the flow as the
wedding photographer: Plan → Start baseline edit / Cmd+Alt+B → preset → anchor → Develop → Edit the rest → apply → the 7
flagged photos → fix one → undo (button, Cmd+Z in the view, Cmd+Z in the Plan) → Finish in Lightroom → cheat sheet.
Screenshots were kept in a scratch folder and deleted afterwards, together with `test-data/ui-screens/`.
Copy was checked against `src-tauri/src/develop/baseline.rs` (`measure_anchor`, `DevelopMeter::auto_light`, the model header),
`BASELINE_PARTITION` in `src-tauri/src/ipc/baseline.rs`, `db/baseline.rs` (`photo_states`, `plan_counts`, `look_source`) and
`ipc/commands.rs` `undo_edit_batch`.

**The engine behind the real backend is still being finished in parallel** (roadmap task "Light normalization engine").
`develop/baseline.rs` is a stub today: light = the photo's plain Auto, with no anchor offset, no burst / scene smoothing and
no low-key flags. The mock (`src/testing/mockBaseline.ts`) follows the target model, so this review judges the UI against
the target model. Until the engine lands, the copy about "nudged the same way" and "matching values in a burst" is not
true on a real catalog. The mock's renders are flat colour placeholders (the "before" slot is always the same blue), so
whether the look really carries over can only be judged on the Mac.

**Summary: P0 2 · P1 7 · P2 12.**

The flow is short and the entry point is obvious: 7 clicks plus the anchor adjustment, and every control has a tooltip.
Three things undermine it:
1. **The anchor.** An untouched anchor already carries a hidden "offset from Auto" to every photo, and the obvious
   **Auto** button in Develop wipes the preset's Vibrance / Saturation, which are then copied to the whole shoot (P0-1).
2. **The old Apply to Scene flow is still the loudest thing on screen,** both before and after the run. After the run the
   Plan reports every scene as "Edited, ready to apply to N" with green Apply buttons. Pressing them overwrites the
   per-photo light (P0-2).
3. **Undo.** One Undo exists, but it goes stale after Cmd+Z, Cmd+Z does nothing inside the view, and fixing a single
   flagged photo (the natural next step) blocks it (P1-2).

## Workflow verdicts

| Step | Steps today | Verdict |
|---|---|---|
| Entry (Plan) | Green banner "Baseline edit · Pick a preset, adjust one photo…" + `Start baseline edit →`, or Cmd+Alt+B | Obvious. The banner copy is right. The amber "Auto edit 3 remaining (my style)" and the per-scene `Edit ▸` buttons still compete (P0-2). |
| 1 Preset | Click a tile → `Next: choose the anchor photo` | Clear, with search and groups. Tiles are small (150 px, two thirds of the screen empty at 1440). Changing the preset after step 3 stacks presets (P1-6). |
| 2 Anchor | Pre-selected (open keeper → edited keeper → first scene's photo) → `Adjust it in Develop` | Fine. Only one candidate per scene (3 of 43 keepers) (P2-7). |
| 3 Adjust | Develop with the green Baseline bar "Preset: Soft Film · Your photo: …" → `Edit the rest` | The bar is a good idea. Its content misleads on an untouched anchor, and Auto destroys the look (P0-1). Apply to scene (15) is just as green right under it (P0-2). Cmd+Alt+B goes back to step 1, not to Edit the rest (P1-4). |
| 4 Edit the rest | Scope (Keepers default) + Skip / Replace → 12 before/after pairs → `Edit 42 photos` → progress + Stop → result | Short and safe. Progress, Stop ("nothing is changed") and the counts are right. The preview is too small to decide on and does not show the anchor (P1-5). The copy is not accurate (P1-1). |
| Review flagged | `Show the 7 that need a look` → grid with an amber reason bar → D → "Needs a look: … · Looks good · Next to review ›" | The reasons are grouped and readable. Develop loses the baseline context, Looks good has no key, and Apply to scene is live (P1-7). |
| Undo | `Undo baseline` button; Cmd+Z in the Plan | The button works on a clean run. Stale and blocked states (P1-2). |
| 5 Finish in Lightroom | Sidecar status + Save now, two Lightroom paths | The menu path is right. The import-preset trap, the overwrite warning and the "LUT is Sieve-only" caveat are missing (P1-3). |

Step count (click path): Start (or Cmd+Alt+B) → preset tile → Next → Adjust it in Develop → *adjust* → Edit the rest →
Edit 42 photos → Finish in Lightroom (→ Save now when auto-save is off). That is 7 clicks and 3 screen changes
(overlay → Develop → overlay). It is short enough; I would not add or merge steps beyond P2-7.

---

## P0

### P0-1 The anchor: an untouched anchor carries a hidden offset, and Auto wipes the preset's colour
- **Where**: `baseline/BaselineView.tsx` `goDevelop`, `baseline/BaselineBar.tsx`, `hooks/useBaseline.ts` `describeOffset`,
  `develop/DevelopView.tsx` Auto (`what === "all"`: `autoTone(id, cur, null)` = `AUTO_TONE`, which **includes Vibrance and
  Saturation**, plus `autoWhiteBalance`). Engine: `develop::baseline::measure_anchor` (anchor light = its sliders, as-shot
  WB when not custom; offset = light − Auto).
- **What**:
  1. Pick Soft Film, press `Adjust it in Develop` and touch nothing. The bar already reads
     "Your photo: **0.0 EV, cooler than Auto, more magenta, highlights +24, shadows -10**". This is "the unedited photo minus
     its Auto", and it is added to every photo's Auto. The photographer who likes the preset as it is gets a church
     as-shot WB delta and an un-Auto'd tone curve on every frame. That photographer is the common case ("I essentially want
     the preset colors to be there").
  2. The step 3 hint says "light and white balance first", and the obvious tool is the blue **Auto** button at the top
     of Basic. It sets Vibrance / Saturation to Auto values, overwriting Soft Film's +12 / −6. Vibrance / Saturation are
     **look** in `BASELINE_PARTITION`, so the preset's colour is then copied *without* the preset to all 42 photos.
  3. In the mock, Auto then shows "warmer than Auto, highlights -34, shadows +26": Develop's Auto and the baseline's Auto
     are different measurements (`auto_tone` vs `auto_tone_with_faces` + `LIGHT_TONE_FIELDS`). So "press Auto, then nudge"
     does not start from zero either.
- **Why**: the model is "Auto + the anchor's offset from its own Auto". The user can only reason about it if the offset
  starts at **zero** ("same as Auto") and grows only with what they change by hand. Today the default offset is noise, and
  the one-click fix destroys the look the user picked in step 1.
- **Fix**:
  1. **Start the anchor from Auto.** In `goDevelop`, after the preset is applied, call
     `previewBaseline(projectId, settings, {sampleCount: 1})` and write `anchor.auto` into the anchor's **light fields only**
     (exposure, contrast, highlights, shadows, whites, blacks, WB custom temperature / tint). Add the preset's own light
     values as deltas: for each `AdjustmentField` in `BASELINE_LIGHT_FIELDS` that the preset's `fields` contain, add the
     preset value (Exposure +0.2 → Auto + 0.2 EV). Keep a preset WB as an absolute value. Save it as one history entry,
     "Baseline: start from Auto". Only do this when the anchor's light fields have no user history entry after the preset
     entry. Never overwrite light the user already set by hand.
  2. **Baseline bar** (left to right): `Baseline` · `Preset: Soft Film` · `Your light vs Auto: same as Auto` (or
     `+0.3 EV, warmer`) · `Start from Auto` (secondary button, tooltip "Set exposure, contrast, highlights, shadows, whites,
     blacks and white balance to Auto for this photo. The preset's colours stay") · `Edit the rest →`. Replace the
     "(adjust exposure and white balance first)" hint with "carried to every photo" in neutral-400.
  3. **Auto in Develop while the Baseline bar is shown and the active photo is the anchor**: call
     `autoTone(id, cur, ["exposure","contrast","highlights","shadows","whites","blacks"])` (no Vibrance / Saturation) plus
     WB. Tooltip: "Auto light and white balance (the preset's colours are kept during the baseline edit)". The same applies to
     Shift+double-click on Vibrance / Saturation: leave it alone, the user asked for it explicitly.
  4. **[ARCH]**: make Develop's Auto on the anchor (tone fields above + WB, faces resolved the same way) return exactly
     `LightMeter::auto_light(anchor, look)`, so "Auto" in Develop gives "same as Auto" in the bar. Alternatively, add a
     `baseline_anchor_auto(projectId, anchorId, presetId) -> LightValues` command that the bar and the Auto button both use.
     Until then the frontend uses `preview_baseline(...).anchor.auto` (no contract change needed for fixes 1–2).
- **Acceptance**: `?baseline=presets`, pick Soft Film → Develop: the bar reads "Your light vs Auto: same as Auto"; the
  anchor's Vibrance is still +12; history shows "Preset: Soft Film" then "Baseline: start from Auto". Exposure +0.3 →
  "+0.3 EV". Press Auto → back to "same as Auto", Vibrance still +12.

### P0-2 The old scene flow contradicts the baseline, before and after the run **[ARCH]**
- **Where**: `edit/PlanView.tsx` (header progress, `plan-apply-all`, `SceneRowView` "Apply to N", tabs), `edit/EditContextBar.tsx`
  (scene bar in Develop), step pill "Edit · 0 of 3 scenes" in the TopBar. Backend: `EditPlan` scene state (`SceneEditEntry`).
- **What**:
  - *During step 3*: under the green Baseline bar, the scene bar reads "Edited · representative · **Edit this photo, then
    apply it to the other 15.**", with an equally green `Apply to scene (15)` and an amber `Auto edit (my style)`.
  - *After the run*: the Plan reads "3 edited · 0 applied · 0 to do". Every scene says "Edited, ready to apply to 15 / 16 / 9"
    with a green `Apply to 15`, and the header has `Apply 3 edited scenes (40)`. The step pill still counts 0 of 3 scenes.
    The baseline wrote each representative like any other photo, and the Plan reads that as a user edit waiting to be applied.
  - Pressing any of them runs a scene match from the representative over 40 baseline photos. This replaces their per-photo
    "Auto + offset" light (undoable, but the user does not know they did something wrong).
  - The flagged-photo review in Develop shows the same green `Apply to scene (15)`.
- **Why**: the user's complaint was the session-by-session scene flow. The new flow is now only one of two equal paths,
  and after it finishes the app tells the user the job is not done and invites the old path over the result.
- **Fix**:
  1. **[ARCH]**: add a scene status `on_baseline` (or a `baselineCount` per `SceneEditEntry`). A scene whose representative's
     history cursor is the baseline entry counts as done ("On baseline"), not as "edited, ready to apply".
     `EditPlan` progress counts it as applied.
  2. Plan after a run (finished, not undone): the header progress reads `Baseline: 42 of 43 keepers · 7 need a look`.
     Scene rows show a green `On baseline` chip, `Review N` (flagged in that scene) and `Edit ▸`, and **no** `Apply to N`.
     `Apply N edited scenes` is hidden unless a representative was edited after the baseline. Then the row reads "You changed
     the representative after the baseline · Apply to 15" in amber, so refinement scene by scene stays possible.
     Add the tab `On baseline 3` next to `Applied`.
  3. Step pill: `Edit · baseline ✓` when a run is finished and not undone.
  4. Develop while the Baseline bar is shown (step 3): hide the scene bar's `Apply to scene` split button and the
     "Edit this photo, then apply it…" hint. Show only the scene navigator and `Plan`.
     In the flagged review (filter `baselineOutcomes: ["flagged"]`): render `Apply to scene` as a neutral (grey) button with
     the tooltip "Copies this photo's edit over its scene and replaces the baseline light of those photos".
  5. Plan header, fresh project: move `Auto edit N remaining (my style)` into the header's `⋯` menu while the banner is
     shown, so the banner's button is the only coloured call to action above the scenes.
- **Acceptance**: `?baseline=1` → Plan: no `Apply to N` button, no `Apply 3 edited scenes`, header "Baseline: 42 of 43
  keepers · 7 need a look". `?baseline=anchor` → step 3: no `Apply to scene` in the scene bar.

## P1

### P1-1 The copy does not describe the look / light model accurately
- **Where**: `RestStep.tsx` `baseline-summary-line`, `BaselineBar.tsx` tooltip, `helpContent.tsx` `baseline-edit`,
  `FinishStep.tsx` section 3, `RestStep.tsx` sample "kept" tooltip.
- **What** (checked against `BASELINE_PARTITION` and `develop/baseline.rs`):
  - "Each photo **keeps its own** exposure and white balance" is wrong. Each photo gets **its own Auto** for **seven**
    settings (exposure, contrast, highlights, shadows, whites, blacks, WB), plus the anchor's offset.
  - "The preset's look is copied as it is" is only partly true. The look is copied **from the anchor** (preset + anything
    the user changed there in colour, curve, detail or effects). The preset's own light values only arrive through the offset.
  - Lens corrections and spot removal are not mentioned. They are never touched (unmodelled keys stay in the sidecar),
    and photographers will ask.
  - The help lists three flag reasons. The contract has six (`mixed_light`, `clamped` and `unreadable` are missing).
- **Fix** (exact copy):
  - Step 4 summary, second line: "Colours, profile, curve, grain and detail are copied from the anchor (the preset plus
    your changes). Exposure, contrast, highlights, shadows, whites, blacks and white balance are set for each photo: its own
    Auto, plus how your anchor differs from its Auto (**+0.3 EV, warmer**). Frames of one burst get matching values."
  - Bar tooltip: "Colours come from the preset and this photo. Light is Auto for every photo, shifted the way you shift
    this one."
  - Help, model bullet: the step 4 copy above, then "Crop, straighten, masks, spot removal and lens corrections are never
    changed." Reasons bullet: "Needs a look: dark on purpose, silhouettes, mixed light, Auto failed, or a value hit the end
    of its slider. They are edited too; check them with Show the photos that need a look."
  - Finish step, section 3: "Every edited photo now has the anchor's look and its own Auto-based light and white balance,
    as standard Lightroom settings in its .xmp sidecar. Crop, straighten, masks, spot removal and lens corrections are as
    they were."
  - While `run.engineVersion` starts with `baseline-stub`, add one line under the result: "Preview engine: light is plain
    Auto for now (your anchor's offset is not applied yet)." Remove it when the engine lands.
- **Acceptance**: no string in `src/components/baseline/` or `helpContent.tsx` contains "keeps its own exposure".

### P1-2 Undo goes stale, Cmd+Z is silent in the view, and fixing one flagged photo blocks it **[ARCH]**
- **Where**: `RestStep.tsx` (`baseline-undo` uses `p.run.batch` from when the run finished), `App.tsx` key routing for
  `baselineOpen` (only help / steps / save pass through), `useBaselineRun`, the result toast, `PlanView` `BaselineBanner`.
- **What** (all reproduced):
  1. Cmd+Z in the Plan → toast "Undid Baseline Edit on 42 photos". The banner still says "Edited 42 photos; 7 need a
     look. Open it to review…". Reopening shows the full result with `Undo baseline` **enabled** and the flagged list.
  2. Cmd+Z inside the Baseline view does nothing and gives no feedback.
  3. The finish toast ("Edited 42 photos; 7 need a look") has no Undo action. Apply to scene's toast has one.
  4. Open a flagged photo, change Exposure, come back: `Undo baseline` is still enabled with "Put all 42 photos back…".
     Clicking it fails with "Later edits on 1 photo; undo those first". Reviewing the flagged photos is the expected next
     step, so the "one Undo" disappears exactly when it is needed.
- **Fix**:
  1. `useBaselineRun.refresh()` after any `undoEditBatch`, on `wf.reportBatch`, when `RestStep` mounts, and on window focus.
     The banner and result then read "Undone. The photos are back to how they were." with `Run again`.
  2. In the Baseline view, Cmd+Z = `Undo baseline` when the run is finished, not undone, and `batch.undoable`. Otherwise
     show a toast with the reason (`blockedUndo` text).
  3. Finish toast: `Edited 42 photos · 7 need a look` with action `Undo` (testid `baseline-undo-toast`).
  4. When `batch.conflictCount > 0`: disable the button and show inline (amber, under the buttons): "You changed 1 photo
     after the baseline (DSC00011). **Undo the rest** puts the other 41 back and keeps your change."
  5. **[ARCH]**: `undo_edit_batch(batchId, options: {keepLaterEdits: bool} | null)`. With `keepLaterEdits`, restore the
     photos still on the batch's entry and leave the others (`skippedIds`). This mirrors "a re-run updates only photos
     still on the baseline". The button label is then `Undo the rest (41)`.
- **Acceptance**: `?baseline=1`, Cmd+Z in the Plan → banner text "Undone…", reopen → `Undo baseline` disabled "Already
  undone". Fix one flagged photo → the inline message and `Undo the rest (41)`, which restores 41 and keeps 1.

### P1-3 Finish in Lightroom: the steps are right but incomplete
- **Where**: `FinishStep.tsx` sections 1–2.
- **What**: Metadata > Read Metadata from Files (Library module) and "a new import reads the sidecars" are both correct.
  Missing:
  - **The import-preset trap.** If the Import dialog has a Develop preset under "Apply During Import", Lightroom applies it
    over the sidecar settings. Wedding photographers routinely set an import preset.
  - **Overwrite warning.** Read Metadata from Files replaces whatever was done to those photos in Lightroom since, and
    Lightroom asks for confirmation.
  - **The LUT caveat.** `sieve:LutId` is "not read by Lightroom" (`BASELINE_PARTITION`). If the anchor uses a LUT, the
    look in Lightroom will differ. A creative profile or `.dcp` imported from outside Lightroom's own folders shows as
    "missing profile" there.
  - **Which folder.** The path is not shown and there is no Reveal in Finder.
  - **Cmd+S** is only in a tooltip.
- **Fix** (section 2, exact copy):
  - Line 0 (above the two paths): the project folder path in mono + `Reveal in Finder` + `Copy path`.
  - "**Photos already in a Lightroom catalog:** in the Library module select them (Cmd+A selects the folder), then choose
    **Metadata > Read Metadata from Files** and confirm. This replaces any changes made to them in Lightroom since."
  - "**A new import:** import the folder as usual, with **Apply During Import > Develop Settings: None**. Otherwise
    Lightroom puts its preset over the baseline. Lightroom reads the sidecars by itself."
  - Section 1: append "(Save, Cmd+S)" visibly after the `Save now` button.
  - Conditional warning (amber, section 3) when the anchor's `lut` is set: "Your look uses the LUT **{name}**. Lightroom
    cannot read Sieve LUTs, so it will look different there. Use a Lightroom profile instead, or export from Sieve."
    When the profile comes from a `.dcp` / imported profile group: "Lightroom needs the profile **{name}** installed."
- **Acceptance**: the Finish step shows the folder path and the "Develop Settings: None" line. With a LUT on the anchor,
  the amber LUT warning is visible.

### P1-4 Cmd+Alt+B in Develop goes back to step 1 instead of Edit the rest
- **Where**: `App.tsx` `openBaseline` (session `stage === "setup"` → step 1), `BaselineBar` `baseline-bar-rest` tooltip "(Cmd+Alt+B)".
- **What**: in step 3 the tooltip promises Cmd+Alt+B = Edit the rest. Pressing it opens step 1 (Preset). Reproduced at
  both sizes.
- **Fix**: in `openBaseline()`, when `bBar` is true and `bSession.anchorId != null`, behave like `onRest`: set
  `stage: "rest"` and open step 4. Keep step 1 only when no session exists.
- **Acceptance**: `?baseline=presets` → steps 1–3 → Cmd+Alt+B → `data-step="4"`.

### P1-5 The before/after preview is too small to decide on and does not show the anchor
- **Where**: `RestStep.tsx` `baseline-samples` (`minmax(260px,1fr)`, two halves of about 166 px at 1280).
- **What**: 12 pairs of thumbnail-sized halves, the "needs a look" reason only in a tooltip. The anchor, which the user is
  comparing against, is not on the page. There is no enlarge and no way to see the afters side by side for consistency.
  Before/after answers "did it improve this photo". The photographer's question here is "do all of them now look like my
  anchor".
- **Fix**:
  1. First tile, pinned: the anchor's current render, labelled `Anchor · your edit`, with an emerald ring.
  2. A segmented toggle above the grid: `Before / after` | `After only`. *After only* shows one 3:2 tile per sample at
     `minmax(220px,1fr)`, grouped by scene with a scene header. Default = **After only**: it is the consistency check.
  3. Hold `\` = all tiles show Before (Lightroom's before/after key). Releasing it shows After. Hint text next to the toggle.
  4. Click a tile = a lightbox at fit size with Before | After side by side, ←/→ through the samples, Esc closes.
  5. Flagged samples: show the reason text under the tile (amber, one line, truncated), not only in a tooltip.
  6. `Show 24 more` under the grid (`sampleCount` up to `MAX_BASELINE_SAMPLES` 48).
- **Acceptance**: step 4 shows the anchor tile first. *After only* is the default. `\` held flips every tile to Before.

### P1-6 Changing the preset after step 3 stacks presets; "No preset" does not remove one
- **Where**: `PresetStep.tsx` (tiles render `renderPreviewVariant(anchor, currentAdjustments, preset)`),
  `BaselineView.tsx` `goDevelop` (`applyPreset` when `appliedPresetId !== presetId`).
- **What**: after Soft Film has been applied (step 3), going back to step 1 and choosing Warm Matte applies Warm Matte
  on top: Soft Film's grain and HSL stay, because Warm Matte does not set them. The tiles preview that mixed state too.
  "No preset · your photo as it is" then means "with Soft Film", and choosing it removes nothing.
- **Fix**:
  - Tiles render on the anchor with `BASELINE_LOOK_FIELDS` reset to neutral (light and never fields kept).
  - Choosing a different preset (or No preset) after one was applied: write one history entry
    "Baseline preset: Warm Matte" = look fields reset to neutral, then the preset's fields.
  - If the anchor has user edits in look fields after the preset entry, confirm first: "Replace Soft Film and your colour
    changes on DSC00023 with Warm Matte? Your light and white balance stay." with `Replace` / `Cancel`.
- **Acceptance**: Soft Film → Develop → step 1 → Warm Matte → Develop: grain amount 0, HSL orange saturation 0, clarity −10.

### P1-7 Reviewing the flagged photos: no baseline context and no key for "Looks good"
- **Where**: `App.tsx` `showBaselineFlagged` (grid), `EditContextBar.tsx` (`edit-looks-good`, `edit-next-review`).
- **What**: the grid strip is good ("7 photos need a look after the baseline: 5 × dark on purpose…"). In Develop the
  Baseline bar is gone, the reason chip and `Looks good` are there, but `Looks good` has no shortcut and `Next to review ›`
  has no tooltip. Getting back to the result means Cmd+Alt+B (works) or the grid's `Baseline edit` link. 7 photos means 14
  clicks.
- **Fix**:
  - Cmd+Enter = **Looks good, next** (mark reviewed, go to the next flagged photo; at the end a toast "All 7 checked" with
    `Back to the baseline`). Add it to the keymap (`baselineLooksGood`, modes grid / develop, where "Needs a look"). Button
    title "Keep these settings and go to the next photo that needs a look (Cmd+Enter)". `Next to review ›` title:
    "Next photo that needs a look (N)".
  - Keep a slim Baseline bar while the `baselineOutcomes` filter is on: `Baseline · Needs a look: 3 of 7 left ·
    Back to the result (Cmd+Alt+B)`.
  - After `Looks good`, remove the photo from the "needs a look" count shown in the flag bar and in step 4.
- **Acceptance**: `?baseline=1` → Show the 7 → D → Cmd+Enter ×7 → the toast "All 7 checked".

## P2
1. **"0.0 EV" in the offset text** (`describeOffset`): −0.05 passes the 0.05 threshold and rounds to "0.0 EV". Round
   first (`Math.round(x*10)/10`) and skip the part when it is 0.
2. **Result labels**: "Edited 42" heads a row whose first chip says "Applied 35". Rename the chips to `Look fine 35` ·
   `Need a look 7` · `Skipped (already edited) 0` · `Could not read N`.
3. **Flagged list shows "#76", "#77"** before entries load (`RestStep` ensures only the first 6 after the fetch). Await
   `lib.ensure` (or render the file name from `getBaselineResults` + `getImages`) before showing the list.
4. **"Change settings and run again" hides the result with no way back.** Add `Cancel` next to `Edit N photos` while
   `rerun` is true, to return to the result.
5. **"Preset none" after a reload** (the session is lost while the anchor carries Soft Film). Fill `presetName` from
   `run.settings.presetId` or `getHistory(anchor).appliedPresetId`.
6. **Preset tiles**: `minmax(220px,1fr)`. At 1440 and wider, a right-hand pane shows the selected preset large on the anchor.
   Arrow keys move between tiles, Enter = Next.
7. **Anchor choices**: only one keeper per scene (3 of 43). Add `Show all keepers` (a virtualised grid of keepers).
   Optional: put the anchor strip at the top of step 1 to save a screen.
8. **Cheat sheet**: list `Cmd+Alt+B Baseline edit` first in Workflow when the Edit step is shown (today 9th, below Apply
   to scene). Add the in-view keys (`Enter` next step, `Cmd+Enter` primary action: Edit N photos / Finish in Lightroom) and
   implement them in `BaselineView`.
9. **Plan banner when done**: add `Finish in Lightroom →` as a second button and show the counts ("42 edited · 7 need a
   look · 3 checked"), so the user does not open the whole overlay to reach step 5.
10. **"Photos you already edited"** also catches Sieve's own Auto edit / Apply to scene / sidecar edits (`photo_states`:
    any non-neutral photo). Copy: legend "Photos that already have an edit (yours, Auto edit, Apply to scene or a
    sidecar)". Show the count on each option: `Skip them (5)` / `Replace their edit (5)`.
11. **Window title** stays "Sieve" on every screen. Set `document.title` to `ceremony · Baseline edit · Sieve` (step name
    optional) so Mission Control and the Window menu tell projects apart. All controls in the Baseline view, the bar, the
    banner and the flag bar have tooltips (checked; none untitled).
12. **Mock fidelity** (tests only): `list_styles` in `mockBackend.ts` lists the imported "Wedding Looks" presets again
    under User Presets, so every preset appears twice and both copies show as selected. The mock "before" render is a
    constant colour. Fix both so the screenshots can be used for a visual review.

## Keyboard map (Baseline edit)

| Action | Existing | Proposed | Notes |
|---|---|---|---|
| Open Baseline edit | Cmd+Alt+B (Plan, Develop) | keep | No conflict (Cmd+Shift+B = select burst). |
| Cmd+Alt+B during step 3 | opens step 1 (bug) | → step 4, Edit the rest (P1-4) | Matches the bar tooltip. |
| Leave the view | Esc (not while running) | keep | Works; Esc is ignored while running. |
| Next step / primary action in the view | – | Enter / Cmd+Enter (P2-8) | Enter is not used inside the overlay today. |
| Undo the baseline | button; Cmd+Z in the Plan only | Cmd+Z in the view too, plus a toast Undo (P1-2) | |
| Before / after in step 4 | – | hold `\` (P1-5) | Lightroom's key. |
| Looks good, next flagged | – (button only) | Cmd+Enter (P1-7) | Cmd+Enter is unused. Enter stays toggleLoupe / crop commit. |
| Next photo that needs a look | N | keep, add the title | Already `nextScene` "or next frame to review". |
| Save sidecars | Cmd+S | keep, show it visibly on Finish (P1-3) | |

## Per-quote verdict (roadmap Phase 10)

| User quote | Verdict | Why |
|---|---|---|
| "I don't think we did a good job on the editing portion" (too session-by-session) | **Partly met** | The new path is one pass for the whole shoot. The scene-by-scene path is still the loudest during and after it (P0-2). |
| "I would like to have a decent baseline to start from" | **Met in the UI, at risk in quality** | Scope, preview, one apply. The baseline is only decent if the anchor offset starts at zero (P0-1) and the engine lands. |
| "select a preset from my selection" | **Met** | The library with groups, search and previews on the anchor. Switching presets later stacks them (P1-6). |
| "once that preset and one photo has an edit, just edit the rest of the library in that same way" | **Met** | `Edit the rest` → `Edit 42 photos`. 7 clicks in total. |
| "I essentially want the preset colors to be there…" | **At risk** | The look is copied as designed, but Auto on the anchor wipes the preset's Vibrance / Saturation for the whole shoot (P0-1). LUT looks do not reach Lightroom (P1-3). |
| "…but make sure you auto edit the rest of the photos so their lighting and what not looks good" | **Met by the model, not yet by the engine** | The mock does Auto + offset with low-key / Auto-failed flags. The real engine is a stub (plain Auto). The copy overstates it (P1-1). |
| "I will just switch over to Lightroom to finish off that edit" | **Mostly met** | Sidecar status and the correct menu path. The import-preset trap and the overwrite / LUT caveats are missing (P1-3). |

## Needs no change
- The entry banner copy and placement, the step pills and their titles, `Back to the plan` / Esc.
- Scope defaults (Keepers), Skip / Replace with live counts ("42 already on a baseline will be updated").
- Progress, Stop ("Nothing is changed until the run is complete" is true: `store_results` is atomic), the result summary
  text, the grid's needs-a-look strip with grouped reasons and the per-photo reason, and the `Baseline: needs a look`
  filter chip.
- Every control has a tooltip.

## Re-check 1 (2026-10-10)

Branch `phase-10-baseline-edit` at 93cd1de (frontend fixes 120baa1, IPC v21.1 + the real baseline engine merged).
Evidence: mock backend (`vite --port 1469`), throwaway Playwright walks at 1280×800 and 1440×900 on `?baseline=presets`,
`anchor` and `1`: Plan → Cmd+Alt+B → preset (keyboard) → Enter → Enter → Develop → +0.3 EV → Cmd+Alt+B → step 4 (hold `\`,
lightbox, Before / after) → Cmd+Enter → result → Plan → Show the 7 → D → fix one flagged photo → Cmd+Enter ×N → Cmd+Alt+B →
Cmd+Z → Undo the rest → Plan; plus Finish, Cmd+Z in the Plan, reopen after undo, cheat sheet, anchor "Show all", preset
replace confirm. `tests/ui/baseline-edit.spec.ts`: 27/27 pass. Screenshots were deleted afterwards.
The engine stub note no longer shows (`engineVersion` is not `baseline-stub` any more), so the "nudged the same way" /
"matching values in a burst" copy is now backed by the engine.

**Summary: every P0 and P1 is fixed except one new P1 inside P1-7 (Cmd+Enter does nothing after you fix a flagged photo).
The three skips are acceptable. There are 8 new P2s.**

### Verdict per item

| Item | Verdict | Evidence / note |
|---|---|---|
| P0-1 anchor offset / Auto wipes colour | **Fixed** | Soft Film → Develop: the bar reads "Your light vs Auto: same as Auto", and history shows "Preset: Soft Film" then "Baseline: start from Auto". Warm Matte opens at "+0.2 EV" (the preset's own exposure, as specced). +0.3 EV gives "+0.5 EV". Auto in Develop has the light-only tooltip and keeps Vibrance / Saturation. [ARCH] `auto_light` is shared by Develop's Auto and the meter, so Auto now reads "same as Auto". New P2-N6 below. |
| P0-2 old scene flow contradicts the baseline | **Fixed** (fix 5 skipped, see below) | Step 3: the scene bar shows only the navigator, "Edited · representative" and Plan, with no Apply / Auto edit. After the run the header reads "Baseline: 42 of 43 keepers · 7 need a look". Rows show "On baseline · N need a look", `Review N` and `Edit ▸`, with no `Apply to N` and no `Apply 3 edited scenes`. The `On baseline 3` tab is there, the step pill reads "Edit · baseline ✓" and the header CTA is `Continue to Export`. The refined-representative amber row is covered by the spec test. |
| P0-2 fix 5 skipped (Auto edit N remaining into ⋯) | **Skip accepted, now P2** | At 1280 on a fresh project the green `Start baseline edit` is the largest, highest-contrast control, directly under the header. The amber header button is small, and after a run it is gone. It still competes a little, so it stays a P2 (unchanged spec). |
| P1-1 copy | **Fixed** | Step 4 summary, the bar tooltip, help (six reasons; "never changed" list), and Finish section 3 match the spec. No "keeps its own exposure" anywhere. |
| P1-2 undo | **Fixed** | Cmd+Z in the Plan: the banner shows "Undone. The photos are back to how they were." with `Run again`. Reopening shows `Undone` disabled. The finish toast has `Undo`. Cmd+Z in the view undoes the baseline, or says why not. After fixing one photo, `Undo baseline` is disabled with the amber line "You changed 1 photo after the baseline (DSC00011.ARW). Undo the rest puts the other 41 back…", and `Undo the rest (41)` restores 41 and keeps 1 (`keepLaterEdits`). Copy leftovers: P2-N5. |
| P1-3 Finish in Lightroom | **Fixed** | Folder in mono + Reveal in Finder + Copy path. The "Develop Settings: None" line, "…and confirm. This replaces any changes…", and "(Save, Cmd+S)" are visible. LUT and imported-profile warnings are conditional (spec test). Multi-folder gap: P2-N8. |
| P1-4 Cmd+Alt+B in step 3 | **Fixed** | Goes to step 4 at both sizes. |
| P1-5 preview | **Fixed** | Pinned "Anchor · your edit" tile with an emerald ring. `After only` is the default and groups by scene. Holding `\` flips every tile to Before. Click opens a fit-size Before \| After lightbox with ←/→ and "2 of 12"; Esc closes only the lightbox. The flagged reason is shown under the tile, and `Show 24 more` is there. |
| P1-6 preset stacking | **Fixed** | Tiles render with the look reset. Changing Soft Film → Warm Matte after a colour edit shows the amber confirm "Replace Soft Film and your colour changes on DSC00001.ARW with Warm Matte? Your light and white balance stay." with Replace / Cancel. |
| P1-7 flagged review | **Partly fixed: new P1-7a open** | Slim amber bar "Baseline · Needs a look: 7 of 7 left · Back to the result (Cmd+Alt+B)". `Looks good` and `Next to review ›` have titles. Cmd+Enter walks untouched photos to "All 7 checked" + `Back to the baseline`. The Apply split button is hidden on representatives that are on the baseline and grey elsewhere. **But see P1-7a.** |
| P2-1 "0.0 EV" | Fixed | Rounds first. |
| P2-2 result labels | Fixed | `Look fine 35 · Need a look 7 · Skipped (already edited) 0`. |
| P2-3 "#76" names | Fixed | File names show in the flagged list at both sizes. |
| P2-4 rerun Cancel | Fixed | `Cancel` next to `Edit N photos` while rerunning. |
| P2-5 "Preset none" after reload | Fixed | Filled from the run / the anchor's `appliedPresetId`. |
| P2-6 preset tiles | **Partly fixed; the skip of the large pane is accepted** | Tiles are 220 px (4 per row at 1440). Without a large pane the 3-preset library still leaves most of the screen empty, which is acceptable for a one-time choice. Arrow keys work **only once**: see P2-N7. |
| P2-7 anchor choices | Fixed | `Show all keepers (43)` / `One per scene`, paged by 120. |
| P2-8 cheat sheet / in-view keys | Fixed (rows-merged skip accepted) | Cmd+Alt+B is first in Workflow. Enter / Cmd+Enter / Cmd+Z / `\` are implemented and listed in a "Baseline edit" group at the end of the sheet (reachable by the filter). |
| P2-9 Plan banner when done | Fixed | "42 edited · 7 need a look · N checked" + `Finish in Lightroom →` + `Open baseline edit`. |
| P2-10 "already edited" | Fixed | Legend "(yours, Auto edit, Apply to scene or a sidecar)", `Skip them (0)` / `Replace their edit (0)`. |
| P2-11 window title | Fixed | `ceremony · Baseline edit · Sieve`. |
| P2-12 mock fidelity | Fixed | Presets are listed once. |

### New P1

#### P1-7a Cmd+Enter is dead on a flagged photo you just fixed (the normal review path)
- **Where**: `App.tsx` `case "baselineLooksGood"` (`if (!wf.needsReviewSet.has(active)) return setNotice("This photo is not marked as needing a look")`),
  `EditContextBar.tsx` (needs-a-look chip).
- **What** (reproduced at 1280 and 1440): `?baseline=1` → Show the 7 → D on DSC00011 → Exposure +1.20. Any edit clears the
  photo's needs-a-look mark: the review bar drops to "6 of 7 left". The chip and `Looks good` disappear, and the context bar
  now reads "Not the representative · Make it the representative (Shift+A)". Pressing Cmd+Enter (12 times in the walk) only
  shows the toast "This photo is not marked as needing a look". The photo does not change and the user is stuck until they
  find `N` or the filmstrip. Fixing a photo and moving on is the point of the review: 7 flagged photos, most of which
  get a touch-up.
- **Fix**:
  1. In `baselineLooksGood`: if the active photo is **not** in `needsReviewSet`, do not refuse. Go to the next photo in
     `needsReviewIds` after it in keeper order (wrap to the first). If none are left, show the existing toast
     `All N checked` with `Back to the baseline`. Show "This photo is not marked as needing a look" only when the
     baseline-flagged filter is off **and** nothing is left to review.
  2. When an edit clears the mark on a photo the baseline flagged (the photo is in `getBaselineResults(..., ["flagged"])`
     and no longer in `needsReviewSet`), keep a chip in the context bar instead of falling back to the representative
     hint: emerald `Fixed · was: dark on purpose` (title "You changed this photo, so it no longer needs a look"), followed by
     `Next to review ›` with the title "Next photo that needs a look (Cmd+Enter or N)".
- **Acceptance**: `?baseline=1` → Show the 7 → D → change Exposure → the chip reads "Fixed · was: …". Cmd+Enter → the
  next flagged photo opens and the review bar reads "6 of 7 left". Cmd+Enter ×6 → "All 7 checked".

### New P2
- **N1 Two "finished" notices at once.** After a run, the centre toast "Edited 42 photos · 7 need a look [Undo]" and the
  activity corner "Edited 42 photos; 7 need a look" show the same news at the same time (1280 result screen). Fix: do
  not pop the corner activity notice for `baseline_edit` when the run finishes in the foreground. Keep it only in the
  activity list.
- **N2 The finish toast covers the review controls.** The persistent toast with Undo sits top-centre in Develop, over the
  context bar, right on `Looks good` / `Next to review ›` when the user goes straight to Show the 7 → D. Fix: dismiss
  the baseline finish toast when the user opens the flagged filter (its Undo stays in the result and on Cmd+Z). Or, in
  Develop, place toasts below the context bars (top offset = bars' height + 8 px).
- **N3 A flagged representative shows no reason.** On DSC00055 (flagged, representative of scene 2) the context bar
  shows "On baseline · representative" and no reason chip or `Looks good`. Only the slim bar's count says it needs a
  look. Fix: when the active photo is in `needsReviewSet`, render the amber needs-a-look chip + `Looks good` even on a
  representative. Move the "representative" fact to the chip's title.
- **N4 `Auto edit (my style)` is live in the flagged review.** It runs over the whole scene and replaces the baseline light
  there, as Apply did. Fix: while `flaggedReview`, hide it, the same way it is hidden under the Baseline bar.
- **N5 Undo copy leftovers.** (a) Cmd+Z in the view with a later edit toasts "Later edits on 1 photo. Undo those first",
  while the screen offers `Undo the rest (41)`. Change it to "You changed 1 photo after the baseline. Use Undo the rest (41)
  to put the others back". (b) After `Undo the rest`, the result and the banner say "Undone. The photos are back to how
  they were." but one photo kept its change. When `keptIds` is non-empty, use "Undone for 41 photos. DSC00011.ARW keeps
  your change." (c) An undone result still shows the `Look fine 35 · Need a look 7` chips. Hide them when undone.
  (d) `Undo the rest` and `Show the N that need a look` are both amber side by side. Make `Undo the rest` neutral grey:
  the amber explanation line already carries the warning.
- **N6 Start from Auto drops the preset's own light.** Warm Matte opens at "+0.2 EV" (Auto + the preset's exposure), but
  `Start from Auto` (`autoLight(START_LABEL)`) sets plain Auto, so it reads "same as Auto" and loses the +0.2 the user
  picked the preset for. Fix: have `Start from Auto` re-run `startLight(auto, session.presetLight, …)` (the same values as
  on entry). Tooltip: "Set exposure … white balance to Auto for this photo, plus the preset's own light. The preset's colours
  stay". Develop's `Auto` stays plain Auto.
- **N7 Preset arrow keys work once.** After clicking a tile, → selects the next preset, but focus drops to `<body>` (the
  tile re-renders), so the next ←/→ does nothing. Fix: keep keys stable and, after `onPick`, focus the tile with
  `data-selected="true"` in an effect. Or handle the arrows on the view's window listener in step 1 instead of on the grid
  `onKeyDown`.
- **N8 Finish shows only the first folder.** `FinishStep` uses `folders[0]`. For a project with several folders, list each
  path (one line each, with its own Reveal / Copy), headed "The sidecars are in these folders:".

### Keyboard map changes since the review
| Action | Before | Now | Note |
|---|---|---|---|
| Cmd+Alt+B in step 3 | step 1 | step 4 | P1-4 fixed. |
| Cmd+Z in the Baseline view | nothing | undo the baseline / reason toast | Fix the reason copy (N5a). |
| Enter / Cmd+Enter in the view | – | next step / Edit N photos, Finish | Works. |
| Hold `\` in step 4 | – | Before | Works. |
| Cmd+Enter in the flagged review | – | Looks good, next | Dead after an edit (P1-7a). |
| ←/→ on preset tiles | – | move + select | Works once (N7). |

### Open P0 / P1 after Re-check 1
- **P1-7a** Cmd+Enter is dead on a flagged photo you just fixed (frontend-dev, `App.tsx` `baselineLooksGood` + `EditContextBar`).
- No open P0. Nothing needs the architect.
