# Phase 8d UX review: review, editing feel, crop, capture time

Author: ux-designer · 2026-10-05 · branch `phase-8d-feedback`, started at 544cbca. The Transform merge 4bb2c2f landed during the review, and the Transform / Upright panel was skipped as asked.
For: frontend-dev (all items), architect (items marked **[ARCH]**)

Evidence: mock backend (`vite --port 1443`, `/?mock=201` + `&keepers=not_rejected&twocams=1&meta=1` or `&style=ready`), Playwright drivers at 1280×800 and
1728×1117. The drivers were throwaway and have been deleted. The screenshots this review refers to are kept in `test-data/ux-review-8d/`:
`cull-1280.png`, `capture-sync-default-1280.png`, `develop-1280.png`, `crop-toast-1280.png`, `plan-overflow-1280.png`,
`selection-before-1280.png` / `selection-after-1280.png`, `edit-all-1280.png`.

**Summary: P0 0 · P1 8 · P2 14.** Most of the 17 feedback points are answered well:
- Zoom persists across the arrow keys, and Space / click zooms at the cursor.
- Slider typing and nudging work.
- Hold-the-dot works.
- Presets highlight and preview on the main image.
- Cmd/Ctrl+C / V work in Develop and the Grid.
- Generic Auto is available.
- Disabled Apply buttons show their reason in place.

Four things are still open:
1. "Edit N selected" still edits only one photo. This was the user's "editing many photos in scenes" request.
2. The reject side of culling still cannot be reviewed or applied on its own. This was the user's "0 auto-rejects" complaint.
3. Sync two cameras has unsafe defaults and cannot tell two bodies of the same model apart. A wedding shooter's two cameras are usually the same model.
4. A few layout breaks at 1280.

---

## Chrome height (Cull step, grid, 1280×800)

| State | Rows above the grid | Grid starts at | Chrome share |
|---|---|---|---|
| Default (8c) | top 44 · filter 32 · toolbar 32 · summary 29 | 137 px | 17 % |
| Default (8d) | + strictness row 28 | 165 px | 21 % |
| 2+ selected (8d) | + selection bar 32 | 197 px | 25 % |

The strictness row is acceptable on its own (P2-12). The selection bar is the problem: it appears on the second click of a multi-select and moves the grid
under the pointer (P1-2).

## Workflow verdicts

| Workflow | Steps today | Verdict |
|---|---|---|
| Strictness → suggestions | Change select → count updates → `Suggestions … Apply…` → dialog → Enter | Works and the count updates live. You cannot see *which* photos would be rejected, and you cannot apply only rejects (P1-5). The explanation text is wrong (P1-6). |
| Review rejected by you vs auto | `Rejected 17` → `(7 by you, 10 auto)` | Works. The cells say `Auto-rejected · Eyes closed` / `Duplicate in burst (best DSC00027)`. The split still looks like grey prose, and `0 auto` does not explain itself (P1-7). |
| See a photo's date/time | Cmd+I (or the Info button) → floating card; the Loupe overlay shows `… (corrected)` | Good content (original vs corrected, source, Revert). The card covers the summary bar and the Continue button (P2-2). Make names are lowercase (P2-3). |
| Shift one camera −1 h | Select → Cmd+Shift+T → Shift tab (defaults −1 h) → Apply → toast Undo | 3 steps, clear before → after preview. Good. |
| Sync two cameras | Cmd-click one frame per camera → Cmd+Shift+T → Sync tab → Apply | Works when you pre-select the pair, but nothing tells you to. Opened from one photo, it pre-picks arbitrary frames and Apply is live (P1-3). Only photos "in view" move (P1-3). Identical bodies cannot be told apart (P1-4). |
| Zoom while culling | Space / click at cursor, Left/Right keep 100% at the same relative point, Fit/Fill/50–400 % buttons | Good. Verified: 100% stays across 2 arrow presses with the full-res tile. Space always targets 100%, not the last preset (P2-1). |
| Slider entry | Click the value → type → Enter / Esc / Tab; hover + Up/Down (Shift ×10, Alt fine) | Good. No change needed. |
| Generic Auto | `Auto` at the top of Basic: tone + WB, one history entry | Works. Two buttons labelled "Auto" in one panel (P2-7). |
| Hold the blue dot | Press-hold the dot → `Preview: Without Basic changes` | Works. Release restores with no flash. No change needed. |
| Presets | Hover 120 ms → main image + Navigator preview; click applies and highlights; a slider change clears it | Good. No change needed. |
| Cmd/Ctrl+C / V | Develop: Cmd+C toast → Right → Cmd+V → `Pasted settings to 1 photo` [Undo]. Grid: Cmd+C → select 5 → Cmd+V (one Undo) | Good. Cheat-sheet grouping is odd (P2-13). |
| Edit many in a scene | Selection bar `Edit 5 selected` → Develop → move a slider | **Only the active photo changes.** The notice says to press Cmd+Alt+S after every change (P1-1, `edit-all-1280.png`). |
| Apply to scene | Context bar Apply → toast "matched … copied …" [Review] [Undo]; skipped photos [Show] | It now carries grain / clarity (flush before apply), and the reasons are in place. At 1280 the Plan header's Apply all is off-screen (P1-8). The toast copy needs tidying (P2-10). |
| Crop | R → drag / rotate outside / A / X / O / Shift+O / Cmd+Alt+R / Enter / Esc | Lightroom-like. The max inscribed rectangle looks right at 8°. Controls are duplicated in the panel and in a bar (P2-9). A toast covers the top-right handle (P2-8). |
| Switching edited photos | Filmstrip / grid show the edited renders | Correct in the mock. A flash cannot be judged from stills, so this relies on `edited-preview.spec.ts`. Confirm on the Mac. |
| Slider lag | – | Not measurable on the mock. Relies on the drag benchmark in the Develop-feel task. |

---

## P0

None.

---

## P1

### P1-1 "Edit 5 selected" / "Edit all in scene" edits one photo: add Lightroom's Auto Sync
- **Where**: `scenes/SelectionBar.tsx` (`sel-edit-all`), `App.tsx` `editAllInScene` (notice text), `develop/AdjustPanel.tsx` right bar (`sync-settings`), `useEditor.ts` commit path.
- **Evidence**: `edit-all-1280.png`. After `Edit 5 selected`, the notice reads "Editing 1 of 5: Cmd+Alt+S syncs this photo's settings to the other 4". Exposure +0.50 changed only photo 6. Photos 2–5 in the filmstrip are unchanged.
- **Why**: the user asked to edit many photos at once. The button promises 5 and delivers 1, and the extra key press per change is easy to forget. Lightroom's answer is Auto Sync.
- **Fix**:
  1. In the right bar, when 2+ photos are selected, add an **Auto Sync** switch to the left of `Sync…`: a small toggle plus the label "Auto Sync". While it is on, the Sync button reads `Auto Sync · 5`.
     Lightroom shortcut: **Cmd+Alt+Shift+A** toggles it (new keymap id `autoSync`, Develop). Tooltip: "Every change goes to all N selected photos (Cmd+Alt+Shift+A)".
  2. When it is on, every *committed* edit on the active photo calls `syncSettings(activeId, otherSelectedIds, changedFields)` with only the fields that edit changed. Commits are: slider release, typed value, the end of a nudge burst, Auto, the WB picker, a preset, profile, B&W, HSL / curve / grading / detail / effects / calibration. The call is one batch per commit. Drafts during a drag render the active photo only. Crop, masks and transform never auto-sync, as in Lightroom.
  3. It turns on automatically when Develop is entered through `Edit N selected` / `Edit all in scene`. It is off otherwise, and the state is remembered for the session. New notice: "Editing 5 photos. Auto Sync is on: changes go to all 5. Turn it off to edit one."
  4. Matched scenes: when the selection came from `Edit all in scene` on a scene that is applied, changes to **exposure / white balance** are not synced. Copying them would flatten the per-frame matching. Instead, show the inline note "Exposure and white balance stay per photo in a matched scene. Use Apply to scene." Proper relative sync needs a delta mode **[ARCH]**: `sync_settings` with `relative: [exposure, white_balance]`, which adds the source's change instead of copying the value. Until then, exclude those fields.
  5. Undo: Cmd+Z right after an auto-synced edit reverts it on all photos (the active photo's entry and its sync batch together). If the linear batch undo cannot pair them, **[ARCH]**: record the active photo's entry inside the same `paste` batch.
- **Acceptance**: Playwright: select 5 → `Edit 5 selected` → set exposure 0.5 → exactly one `sync_settings` with 4 targets and `fields` = `["exposure"]`. One Cmd+Z → all 5 back to 0. With the switch off, no `sync_settings` call. Cmd+Alt+Shift+A flips the switch.

### P1-2 The selection bar pushes the grid down 32 px on the second click
- **Where**: `scenes/SelectionBar.tsx` (in-flow `h-8` row), rendered by `App.tsx` above the grid whenever 2+ photos are selected or a scene filter is on.
- **Evidence**: `selection-before-1280.png` → `selection-after-1280.png`. Row 1 moves from y=173 to y=205 on the Shift-click. This also happens in the **Cull** step, where the bar's actions (Copy settings / Paste / Sync / Edit) are not what you are doing.
- **Why**: the photo moves under the pointer between the first and second click, so the next Shift / Cmd-click lands on the wrong frame. It is the most common culling gesture (select a burst, reject).
- **Fix**: make the bar a floating overlay that never reflows the grid. Position it `absolute` at the bottom centre of the grid viewport, 12 px above the bottom edge, height 36, `rounded-lg border border-neutral-700 bg-neutral-900/95 shadow-xl`, same content. In the Cull step, show it only when the clipboard holds settings or a scene filter is on. Otherwise show nothing: the selection count is already in the toolbar readout ("· 5 selected").
- **Acceptance**: the bounding box of `cell-2` is identical before and after Shift-clicking a range, in Cull and Edit. `selection-bar` does not intersect the toolbar. In the Cull step with an empty clipboard and no scene filter, `selection-bar` has count 0.

### P1-3 Sync two cameras: arbitrary default pair, "in view" scope, no visual check
- **Where**: `CaptureTimeDialog.tsx` (`pickFrame`, `syncTargets` from `viewIds`, `frameSelect` capped at 500 options).
- **Evidence**: `capture-sync-default-1280.png`. Opened from one selected photo, the Sync tab pre-picks `DSC00001.RAF` and `DSC00004.JPG` (unrelated frames) and Apply is enabled: "50 photos will change (−1 s)". The scope is "all … frames **in view**", so with the Keepers or Rejected filter on, the hidden frames of that camera keep the wrong clock. The frame lists stop at 500 entries. One camera on a 2,824-frame wedding has more than that.
- **Why**: one click on Apply shifts a whole camera by a meaningless offset. A filtered view silently leaves the rest of the camera unsynced, so bursts and scenes split later. Picking "the same moment" from a list of file names is guesswork.
- **Fix**:
  1. Defaults: pre-fill the two frames only when the selection contains exactly one frame from each of the two cameras. Then also open the dialog **directly on the Sync tab**. Otherwise leave both frame pickers empty ("Choose a frame…"), keep Apply disabled, and show the hint "Tip: Cmd-click one photo from each camera in the grid, then press Cmd+Shift+T."
  2. Scope: the target camera's frames are **every photo of that camera in the project**, ignoring the filters. Query the project with an empty filter. Summary copy: "All 1,412 Canon EOS R5 photos in this project move +1 h 0 min 3 s". If filters hide some of them, add "(including 37 hidden by the current filters)".
  3. Visual check: under each frame picker, show the chosen frame's thumbnail (120 px wide, `Thumb`) with its time. Replace the 500-option `<select>` with a searchable list (type to filter by name or time) that has no cap.
- **Acceptance**: opened with 1 photo selected → both pickers empty, Apply disabled. Opened with one frame of each camera selected → Sync tab active, pair pre-filled, thumbnails visible. With the Keepers filter on, `edit_capture_time` receives every project id of the target camera.

### P1-4 Two bodies of the same model cannot be synced **[ARCH]**
- **Where**: `CaptureTimeDialog.tsx` groups by `camera.make + model`. `CameraInfo` has no serial, and the `get_image_metadata` serial is a stub (`null`).
- **Why**: wedding second shooters and two-body shooters usually run two identical bodies (2× ILCE-7M4). Today the dialog says "Only one camera … nothing to sync". This is exactly the user's case: two cameras an hour apart.
- **Fix**:
  - Contract (architect): add `CameraInfo.serial: string | null`, from EXIF `BodySerialNumber` / Sony `InternalSerialNumber` / Fuji `SerialNumber` / Canon `SerialNumber`, extracted at ingest. Fill `ImageMetadata.cameraSerial` from it. Add `serial` as a Metadata-filter facet.
  - Frontend: group by make + model + serial. Label `Sony ILCE-7M4 (…4567)` (last 4 digits) when two groups share a model.
  - Fallback when serials are missing: a third choice in the "Clock to fix" picker, **"The selected photos"**, which moves exactly the current selection. Combined with the grid this covers any case.
- **Acceptance**: a two-body fixture of the same model, different serials, shows 2 cameras in the Sync tab. Syncing moves only the second body's frames. Without serials, "The selected photos" moves the selection only.

### P1-5 Reject suggestions cannot be reviewed or applied on their own **[ARCH, small]**
- **Where**: `CullSummaryBar.tsx` (`strictness-count`, `cull-sum-suggest`), `ApplySuggestionsDialog.tsx`; IPC `apply_suggestions(ids, onlyUnset)`, `ImageQuery`.
- **Evidence**: `cull-1280.png` shows "8 reject suggestions" with no way to see those 8. Apply writes picks, rejects **and** stars together: "71 will be updated: 20 picked · 10 rejected · 41 stars only".
- **Why**: the user wants Sieve to reject the junk ("0 auto-rejects"). To do that they must accept 20 picks and 41 star ratings into their XMP, or apply blind and undo. A photographer who rates by hand will not do that.
- **Fix**:
  1. Contract: `ImageQuery.suggested?: "reject" | "pick" | null`, matching pending suggestions under the same rule as the summary counts (unflagged, unrated, analysed). Also `apply_suggestions(ids, onlyUnset, kinds: { picks: bool, rejects: bool, stars: bool })`, defaulting to all true for compatibility.
  2. Frontend: `N reject suggestions` becomes a chip button `Review 8 suggested rejects`. It filters the grid to `suggested: "reject"`; cells already show "Sieve suggests reject · reason". While that filter is on, the Apply dialog opens with only **Rejects** checked.
  3. Dialog: three checkboxes, "Rejects (8) · Picks (20) · Stars (41)", with counts that follow the checkboxes. Remember the last choice per project. Button: `Apply to 8`.
- **Acceptance**: clicking `Review 8 suggested rejects` shows exactly the 8 photos. Applying with only Rejects checked changes `pick` on 8 photos and `rating` on none. Undo restores.

### P1-6 Strictness explanations are wrong and push users to Aggressive
- **Where**: `CullSummaryBar.tsx` `STRICT_TEXT`, `helpContent.tsx` `reject-strictness`.
- **Evidence**: the UI says Conservative rejects "closed eyes". Per `decisions.md` (Conservative = Phase 7b rules), blinks never reject there. Balanced reads "Clear failures plus most missed focus", but it also rejects closed eyes on the main subject, motion blur, and burst frames clearly worse than the best. Aggressive reads "Also … burst duplicates, any closed eyes", which implies Balanced does neither.
- **Why**: a user who wants blinks and burst duplicates rejected will choose Aggressive. Aggressive has a 20 % keeper false-reject rate versus 3.5 % for Balanced on the calibration set.
- **Fix** (UI and Help, same words):
  - Conservative: "Only unusable frames: nothing in focus, far too dark or blown out, or several defects at once. Closed eyes and burst duplicates are never rejected."
  - Balanced (default): "Also missed focus or motion blur on the main subject, closed eyes on the main subject, and burst frames clearly worse than the best one."
  - Aggressive: "Also any closed eyes or soft focus, and weaker burst frames even when the difference is small. Expect some keepers among the suggestions."
- **Acceptance**: the `strictness-explain` text equals the strings above for each level. The Help entry uses the same three sentences.

### P1-7 "By you / auto" is still easy to miss, and "0 auto" does not explain itself
- **Where**: `CullSummaryBar.tsx` `cull-sum-reject-split` (plain grey text in parentheses, `text-neutral-400`).
- **Evidence**: `cull-1280.png`: `Rejected 7 ( 7 by you,  0 auto )` renders as prose next to button-styled chips. This is the user's first feedback point ("couldn't find it at first"). Before Apply, "0 auto" sits next to "8 reject suggestions" without connecting the two.
- **Fix**: render the split as a segmented control attached to `Rejected`. Make it one rounded group: `Rejected 7 | By you 7 | Auto 0`, where each segment uses the same chip style as `Picked`. Hover shows the existing tooltips. A zero `Auto` segment stays visible and enabled-looking, and clicking it shows the inline popover: "No photos were auto-rejected yet. Sieve suggests rejecting 8: Review them" (the link is P1-5's filter). Apply the same pressed state as the other chips.
- **Acceptance**: `cull-sum-by-you` and `cull-sum-auto` have the chip background (`bg-neutral-800`), not text-only. Clicking `Auto 0` with pending reject suggestions shows the popover with the review link.

### P1-8 1280-px layout breaks in the Edit step and the Navigator header
- **Where**: `edit/PlanView.tsx` header row, `TopBar.tsx` in the Edit step (Plan button added), `develop/LeftPanel.tsx` Navigator `action`.
- **Evidence**:
  - `plan-overflow-1280.png`: `plan-apply-all` sits at x 1237–1430 and `plan-more` at 1438–1470, past the 1280 edge. The in-place reason "Edit a scene's representative first · Open representative" pushed them out.
  - In Develop in the Edit step, `more-menu` spans 1254–1290 (clipped).
  - `develop-1280.png`: the Navigator header reads `NAVIGATORFIT Fill 50% 100% 20`, with no gap after the title and 200 % / 400 % cut off. At 1728 it still collides and 400 % is cut.
- **Fix**:
  1. Plan header: at < 1440 px, move the reason text *under* the Apply all button. Use a two-line block: the button, then the 11 px reason with the fix link, right-aligned. Give the progress bar `min-w-0 flex-1` (max 200 px) so it shrinks first. `Apply all` and `…` never shrink.
  2. Top bar in the Edit step: at < 1440 the step pills drop their counts (`Cull`, `Edit 1/3`, `Export`), as the 8c P1-7 fix did for the grid toolbar.
  3. Navigator header: show `Fit Fill 100% 200%` (drop 50 % and 400 %, which stay in the viewer toolbar), use sentence case like the toolbar (`Fit`, not `FIT`), and put `gap-2` after the title. Lightroom's Navigator also shows 4 presets.
- **Acceptance**: at 1280×800, no element in the top 160 px has `right > innerWidth` in Plan, Develop or Grid, in any step. Every `nav-*` button is fully inside the left panel at 1280 and 1728.

---

## P2

- **P2-1 Space zooms to the last zoom-in preset (Lightroom).** `LoupeLayer.tsx` `toggle` always targets 100 %. Remember the last non-Fit preset clicked (toolbar or Navigator) per session, and toggle Fit ↔ that preset. Default to 100 %. The tooltip on the preset buttons says "Space toggles Fit and this zoom". Acceptance: click 200 %, then Fit, then Space → 200 %.
- **P2-2 Photo info card covers controls.** `PhotoInfoPanel.tsx` is `fixed top-14 right-3`. In the Grid it hides `Continue to Edit` and the Suggestions button. In Develop it hides the histogram and Basic. Anchor it to the content area instead: Grid / Loupe at `top = top of grid + 8 px`, right 12. In Develop, put it over the **left** panel area (left 12, below the Navigator) so the sliders stay usable. Acceptance: no intersection with `continue-edit`, `cull-sum-suggest` or `right-aside`.
- **P2-3 Camera names are lowercase** ("sony ILCE-7M4", "fujifilm X-T5") because `CameraMake` is an enum. Add a display map: Sony, Fujifilm, Canon, and omit "other". Use it in `PhotoInfoPanel`, `CaptureTimeDialog` and the Metadata filter.
- **P2-4 Sub-second sync offset reads "(+)".** `formatOffset(400)` returns "+" alone, and the preview shows identical before/after times while saying "18 photos will change". Format offsets under 1 s as `+0.4 s`. When the offset is below 1 min, show preview times with tenths.
- **P2-5 Edit Capture Time could open on the right tab.** Covered by P1-3.1 for Sync. The Shift tab default "Earlier 1 h" is good.
- **P2-6 Optional capture time on grid cells.** Add `View ▸ Show capture time` (off by default) to print `14:02:31` in the cell footer, left of the stars, so interleaving can be checked after a sync. Lightroom's expanded cells offer the same.
- **P2-7 Two "Auto" buttons in Basic.** Rename the Tone subheading's button to `Auto tone` (Cmd+U). Shorten the top helper text to the tooltip only. It is truncated at 1280 ("tone and white balance from this …").
- **P2-8 Develop toasts sit on the crop's top-right handle.** `Toasts.tsx` `placement="top"` → `top-[88px] right-[300px]` (`crop-toast-1280.png`): the 10 s paste toast covers the corner handle at (967, 113). While cropping or a mask tool is active, place toasts at the bottom-left of the viewer, above the viewer toolbar.
- **P2-9 Crop controls are duplicated** (right panel and the viewer `crop-bar`, `crop-toast-1280.png`). Keep the panel as the source, as in Lightroom. Show the viewer bar only when the right panel is hidden (Tab / Shift+Tab).
- **P2-10 Apply-to-scene toast copy.** Today: "Applied Scene 1 to 15 photos · 1 need a look. exposure + white balance matched per photo; 26 setting groups copied as they are (grain, clarity, HSL, curves...)". Spec: "Applied Scene 1 to 15 photos. Exposure and white balance matched per photo; everything else copied (grain, clarity, HSL, curves…). 1 needs a look." Use singular / plural correctly and drop the group count.
- **P2-11 Plan rows repeat and truncate the reason.** A to-do row says "To do: edit this photo" and also "Edit the represent… Open representative" (truncated at 1280). For `todo`, drop the ApplyWhy line, because the status line already says it, and make "edit this photo" the link. A soft "Already applied" in amber under a green "Applied to 15" reads like a warning. Soft reasons use `text-neutral-400`.
- **P2-12 Strictness row.** At ≥ 1440 px, put the select inline in the summary bar, before Suggestions, and drop the second row. Drop the duplicate right-hand count once P1-5's `Review N suggested rejects` chip exists.
- **P2-13 Cheat sheet grouping.** Cmd+C / Cmd+V are listed under "Scenes". Move them into a new group "Copy & paste settings" with Cmd+Shift+C, Cmd+Shift+V, Cmd+Alt+V, Cmd+Shift+S and Cmd+Alt+S, and the proposed Cmd+Alt+Shift+A (Auto Sync). Fix the `copyAll` comment in `keymap.ts` that says "Library only for now".
- **P2-14 Filmstrip scene badge overlaps stars.** In Edit-step Develop, `S1` sits on the star overlay (`16 S1 ★★☆` reads as "S1 r"). Move the scene badge to the top-left corner, or hide stars under 80 px cell width.

## No changes needed
Loupe / Compare / Develop zoom persistence and cursor-anchored zoom. Slider typing / nudging / Tab. Hold-the-dot. Preset highlight and hover preview. Cmd/Ctrl+C / V
in Develop and the Grid (Undo toast). Generic Auto behaviour. Reasons on disabled Apply in the Develop context bar and scene strip. Rejected view reasons
(`Auto-rejected · Duplicate in burst (best DSC00027)`). The Lightroom crop keymap (R / Enter / Esc / A / X / O / Shift+O / Cmd+Alt+R). Edit Capture Time Shift / Set / Revert tabs.

---

## Keyboard map (changes only; everything else in `src/lib/keymap.ts` stays)

| Action | Today | Proposed | Lightroom Classic | Notes |
|---|---|---|---|---|
| Zoom toggle (Loupe / Compare / Develop) | Space / click: Fit ↔ 100 % at cursor | Space / click: Fit ↔ **last zoom-in preset** (default 100 %) | Space / Z: toggles the two Navigator presets | P2-1 |
| Auto Sync on/off | – | **Cmd+Alt+Shift+A** (Develop, 2+ selected) | Cmd+Alt+Shift+A | P1-1. No conflict: Shift+A = anchor, Cmd+A = select all (Grid only). |
| Copy all / paste to selection | Cmd/Ctrl+C, Cmd/Ctrl+V (group "Scenes") | same keys, group "Copy & paste settings" | Cmd+Shift+C / V | P2-13. Ctrl accepted deliberately (decision 2026-10-05). |
| Copy dialog / paste | Cmd+Shift+C / Cmd+Shift+V | unchanged | same | |
| Edit capture time | Cmd+Shift+T | unchanged | none (menu only) | No conflict with Shift+T (Guided Upright). |
| Photo info | Cmd+I | unchanged | Cmd+I = info overlay toggle | Acceptable: I still cycles the overlay as in Lightroom. |
| Hold "without this panel" | hold the dot (Space / Enter while focused) | unchanged | panel on/off switch (click) | Do not bind a global key; Space must stay zoom. |
| Slider nudge | hover + Up / Down (Shift ×10, Alt fine); focused: all arrows | unchanged | same | Hovering a slider in Develop Compare takes Up / Down from Compare (acceptable: the pointer is on a slider). |
| Review suggested rejects | – | none (chip click) | – | P1-5. No key needed. |
| Clipping overlay | – | J (not in this phase) | J | Gap noted, not requested. Needs a render-side clipping mask. |

Conflicts checked: X / A / O / Shift+O are scoped to crop (`needs: "crop"`), ahead of reject / auto-mask / mask overlay. K is the brush in Develop and keeper in Compare.
Cmd+Alt+R crop reset vs Cmd+Shift+R reset all. Shift+T Guided vs Cmd+Shift+T capture time. Up / Down hover-nudge vs Compare swap. None of these is ambiguous today.

---

## Re-check 1 (2026-10-05, HEAD 96142fc, contract v19.2)

Evidence: mock backend on `vite --port 1448` (`/?mock=201&keepers=not_rejected`, `/?mock=60&scope=all&twobodies=1&meta=1`, `/?mock=200&scope=all[&upright=none]`), throwaway Playwright drivers at 1280×800 and 1728×1117 (deleted). The existing suites `ux-8d-p1`, `ux-8d-p2`, `ux-8d-v192`, `upright`, `crop-lr` and `crop-geometry` pass against the same server (52/52).
Kept screenshots (`test-data/ux-review-8d/`): `rc1-cull-1280.png`, `rc1-capture-sync-1280.png`, `rc1-autosync-1280.png`, `rc1-after-apply-1280.png`, `rc1-guided-1280.png`, `rc1-crop-auto-1728.png`.

**Summary: open P0 0 · P1 4 (all new: R1-1 crop discarded on navigation, R1-2 typed crop angle ignored, R1-3 crop vs Upright mismatch [ARCH], R1-4 capture-time Apply below the fold at 1280) · P2 14.** All eight P1s from the first pass are resolved.

### Status of the first-pass P1s

| # | Status | Evidence |
|---|---|---|
| P1-1 Auto Sync | **Resolved** | `rc1-autosync-1280.png`: `Edit 5 selected` opens Develop with the `Auto Sync` switch on and the note in the right panel. A committed edit is one `sync_delta` (source + 4) with relative exposure / WB, and one Cmd+Z reverts all 5. Cmd+Alt+Shift+A flips the switch (suite `ux-8d-p1`). The notice copy contradicts the panel (R1-P2-1). |
| P1-2 Selection bar reflow | **Resolved** | Cull step, 1280 and 1728: the bounding box of `cell-2` is identical before and after a Shift-click range (y = 173). In Cull with an empty clipboard, `selection-bar` has count 0. Elsewhere it floats bottom-centre. It covers the last row (R1-P2-3). |
| P1-3 Sync two cameras defaults / scope | **Resolved** | One photo selected: both pickers read "Choose a frame…", Apply is disabled, and the Cmd-click tip shows. A one-per-camera pair opens on Sync with both thumbnails. Scope "This body … in this project (15)". A searchable list with no cap. With a filter hiding frames: "including N hidden by the current filters". At 1280×800 Apply sits below the fold (R1-4). |
| P1-4 Two identical bodies | **Resolved** | `rc1-capture-sync-1280.png`: `Sony ILCE-7M4 (…8214)` vs `(…9876)`. "The selected photos only (2)" fallback sends `scope: "selected"`. The facet lists both bodies. The model-scope label is muddled (R1-P2-5). |
| P1-5 Review / apply rejects only | **Resolved** | `rc1-cull-1280.png`: the `Review 8 suggested rejects` chip shows exactly 8 photos. The Apply dialog opens with only Rejects checked, `Apply to 8`, and sends `kinds {picks:false, rejects:true, stars:false}`. The choice is remembered per project. What happens after Apply needs work (R1-P2-6). |
| P1-6 Strictness text | **Resolved** | The `strictness-explain` text and Help match the specified sentences for all three levels. |
| P1-7 By you / Auto | **Resolved** | Segmented chips `Rejected 7 · By you 7 · Auto 0`. `Auto 0` opens "No photos were auto-rejected yet. Sieve suggests rejecting 8. Review them". Esc does not close it (R1-P2-4). |
| P1-8 1280 layout | **Resolved** | 1280 and 1728: no element in the top 160 px has `right > innerWidth` in the Plan or in Develop. The Plan's Apply all reason sits under the button. The Navigator reads `Fit Fill 100% 200%` inside the panel. |

### Transform / Upright and crop vs Lightroom Classic (first review)

What matches Lightroom and needs no change:
- **Upright buttons.** Same order and labels (Off / Auto / Guided, Level / Vertical / Full). One history entry each (`Upright: Vertical`). The section's blue changed dot shows.
- **No usable lines.** An inline amber note appears, nothing is saved and the mode stays Off, as Lightroom does. The backend copy ("No vertical lines found", "The correction would be too extreme") is clear.
- **Manual sliders.** Vertical / Horizontal / Rotate / Aspect / Scale / Offset X / Y have Lightroom's ranges and support typing, nudging and double-click reset.
- **Constrain Crop.** Off by default, as in Lightroom.
- **Guided tool.** Shift+T arms it. It shows the photo without its transform, solves live from 2 guides, allows at most 4, x deletes a guide and Esc exits.
- **Auto straighten in the crop tool.** The `Auto` button and Shift+double-click on Angle both work. In `rc1-crop-auto-1728.png` it levels by +1.2° and the crop re-fits to the largest rectangle.
- **The "over-cropped after Done" bug is fixed.** At 5° with Original 3:2 locked, the rectangle is 0.8874 of the frame on both axes. That equals the analytic maximum inscribed rectangle, min(1.5/(1.5 cos θ + sin θ), 1/(1.5 sin θ + cos θ)) = 0.8874. Rotating back to 0 grows it back (crop-lr suite). The stored crop round-trips: reopening R shows the same rectangle.
- **Constrain to image off.** The rotated corners render as white paper, matching the export.
- **Keymap.** R, Enter, Esc, A, X, O, Shift+O and Cmd+Alt+R all work while cropping. The crop-scoped X / A / O / Shift+O win over reject / auto-mask / mask overlay; these are the only duplicate chords in `keymap.ts` (scripted check over every mode).
- **Crop bar.** It now appears only when the right panel is hidden.
- **Accepted deviation.** Dragging inside the crop moves the rectangle, not the image (decisions.md).

The user said: "when I manually straighten and click done, the image is cropped in way more than it should". The plain straighten case is fixed. Two paths still give a result the user did not see or intend (R1-1, R1-3), and typing an exact angle does not work (R1-2).

### New P1

#### R1-1 Leaving the crop tool any way except Done / Enter / R silently throws the crop away
- **Where**: `develop/DevelopView.tsx`. The `useEffect(() => { setCropTool(null); … }, [id])` runs on photo change. `toggleGuided` calls `setCropTool(null)`, and so does leaving Develop (G / E / Grid / Loupe buttons, step switches, filmstrip click).
- **Evidence**: driver at 1280. R, Angle +4.0°, then Right arrow: 0 `save_adjustments` with label "Crop", and the next photo opens with no crop tool. The same happens with G: 0 saves.
- **Why**: in Lightroom, moving to another photo or module while the Crop Overlay is open **applies** the crop. The crop tool stays active on the next photo when you change photos within Develop. A photographer straightens, presses → to do the next frame, and loses the work without a word. This looks exactly like "the crop tool doesn't work like Lightroom".
- **Fix**:
  1. Every exit other than Esc / Cancel commits first. That covers photo change (arrows, filmstrip, N / Shift+N, Compare), G / E / D / C, step switches (Cmd+Alt+1/2/3), Shift+T (Guided), opening Masks (K / M / Shift+M …) and the toolbar view buttons. The commit is one "Crop" history entry on the photo being left. No entry if nothing changed (`commitCrop` already checks this).
  2. Implementation: expose `commitPendingTool(): Promise<void>` on the Develop handle (it calls `commitCrop()` and awaits the editor flush). App's navigation paths `await` it before changing `active` / `mode` / `step`. Inside DevelopView, Shift+T and the mask keys call `commitCrop()` instead of `setCropTool(null)`.
  3. Photo change within Develop (arrows, filmstrip): after committing, **re-open the crop tool on the new photo**. Call `startCrop()` once the new photo's `info` is loaded, with the same aspect lock and overlay. This matches Lightroom, where R stays on while you step through a series.
  4. Esc and Cancel stay the only discard paths.
- **Acceptance**: R, angle 4 (slider), Right arrow → exactly one `save_adjustments` with label "Crop" and `crop.angle = 4` for photo 1, and `crop-overlay` is visible on photo 2. R, angle 4, G → one Crop save. R, angle 4, Esc → none.

#### R1-2 A typed crop angle is thrown away
- **Where**: `develop/CropPanel.tsx`, the `Slider id="crop-angle"`: `onInput={(v) => crop.change({ ...tool, angle: v, rotating: true })}` followed by `onCommit={() => crop.change({ ...tool, rotating: false })}`. When a value is typed, both run in the same tick. `onCommit` spreads the stale `tool`, whose angle is still 0, so it overwrites the typed angle.
- **Evidence**: click the Angle value, type `5`, then Tab or Enter → still `0.0°` with the rectangle unchanged. Dragging and hover + Up / Down work (+0.5° after 5 presses). Filling the range gives +5.0°.
- **Why**: "Angle: type −1.3" is how Lightroom users straighten precisely. It is the sibling of the user's straighten complaint.
- **Fix**: make `CropApi.change` accept an updater (`change(t => ({ ...t, rotating: false }))`), applied through `setCropTool(prev => constrainTool(f(prev), …))`. Use the updater for every `onCommit` / `onReset` / `onKeyUp` / `onBlur` / `onPointerUp` in `CropPanel` and `CropBar`.
- **Acceptance**: type 5 + Enter in the Angle value → `+5.0°`. With Original 3:2 locked, `crop-rect` is `{l:0.0563, t:0.0563, r:0.9437, b:0.9437}`. The crop tool stays open, so Enter in the field does not also commit the crop. The same holds for −1.3 typed into `cropbar-angle`.

#### R1-3 Crop after Upright / Transform: what you draw is not what you get **[ARCH, small]**
- **Where**: `useEditor.ts`. In crop mode, `render` sends `crop.enabled = false` but keeps `transform`. Rust `develop::transform::Geometry::of` then calls `constrain_crop`, which turns a *disabled* crop into the largest frame inside the warp whenever `transform.constrainCrop` is on. `CropOverlay` (`constrainTool`, `insideRotated`, `PaperFill`) only knows the crop angle, not the Upright / manual warp.
- **Evidence**: code reading. The mock does not warp, so this cannot be seen in screenshots. Confirm on the Mac with a Vertical-corrected hall frame.
  - With Constrain Crop **on**, the "uncropped" frame under the crop tool is already auto-cropped, but the overlay treats it as the full frame. A rectangle drawn at 10 % insets is stored as 10 % of the *full* warped frame. After Done the photo shows more than the box did, or it is re-shrunk by `constrain_crop`.
  - With Constrain Crop **off**, "Constrain to image" in the crop tool lets the rectangle take in the empty, white warp corners.
- **Why**: this is the remaining "cropped differently than I set it" path, and Upright is exactly what the user asked for. In Lightroom, the Crop Overlay shows the whole warped image, and "Constrain to Image" keeps the rectangle inside the warped image area.
- **Fix**:
  1. Frontend, no contract change: while cropping, render with `transform: { ...a.transform, constrainCrop: false }` as well as `crop.enabled = false`. The tool then shows the full warped frame with its white corners.
  2. Contract (architect): give the overlay the warped image outline in the corrected frame. Either `RenderResult.validQuad: [number, number][] | null` (4 points, fractions of the corrected frame, null without a warp) or a small `transform_bounds(id, adjustments) -> [number, number][]`. The backend already computes this outline in `region_planes`.
  3. `CropOverlay`: "Constrain to image" keeps the rectangle inside the intersection of the rotated frame and `validQuad` (a convex polygon test replacing `insideRotated`; `fitInsideRotated` and `approach` use it). `PaperFill` paints the area outside `validQuad`.
  4. Starting the tool on a photo with Constrain Crop on and no stored crop: begin from what `constrain_crop` would produce, so the first frame you see is the current result.
- **Acceptance (real app)**: Vertical on a frame with converging verticals, Constrain Crop on, then R. The whole warped image is visible, and the rectangle starts at the auto-constrained frame. Dragging a corner outward stops at the warp edge. Done → the rendered result matches the rectangle (corner positions within 1 %).

#### R1-4 Edit Capture Time: Apply is below the fold at 1280×800, and Enter does nothing
- **Where**: `CaptureTimeDialog.tsx`. `<Dialog … className="flex max-h-[90vh] … overflow-y-auto">` is passed no `onConfirm`.
- **Evidence**: `rc1-capture-sync-1280.png`. Opened on a pre-filled pair, the dialog body scrolls and `capture-apply` sits at y 772–804 in an 800 px window. The four-row preview is cut off. At 1728 it fits.
- **Why**: on the most common laptop size, the last step of a 3-step task (select pair → Cmd+Shift+T → Apply) is hidden.
- **Fix**:
  - Make the footer (Cancel / Apply) `sticky bottom-0` with `bg-neutral-900` and a top border. Only the middle scrolls.
  - Below 900 px height, use `h-24` frame lists (instead of `h-28`), 96 px thumbnails, and two preview rows plus "…and N more".
  - Pass `onConfirm={apply}` with `canConfirm={!applyDisabled}`, so Enter applies when focus is not in the search field.
  - Initial focus goes to the first tab (or the reference search box on Sync), not the `?` help icon. Today the help icon gets a visible focus ring on open.
- **Acceptance**: at 1280×800, opened on a pair, `capture-apply` is fully inside the viewport without scrolling. Enter applies once. Esc cancels.

### New P2

- **R1-P2-1 Auto Sync copy contradicts itself.** The notice from `App.tsx` `editAllInScene` says "(exposure and white balance stay per photo for now)". The right panel says they "are applied as a change". The `SelectionBar` `sel-edit-all` tooltip still says "Cmd+Alt+S syncs". Notice: "Editing 5 photos. Auto Sync is on: every change goes to all 5; exposure and white balance move by the same amount on each. Turn Auto Sync off (Cmd+Alt+Shift+A) to edit one." Tooltip: "Open Develop with the whole selection, Auto Sync on".
- **R1-P2-2 Right bar at 1280.** `Auto Sync · 5` wraps to two lines next to the switch, which is also labelled Auto Sync (`rc1-autosync-1280.png`). While Auto Sync is on, label the button `Sync…` (count in the tooltip) and add `whitespace-nowrap`. Put the count on the switch instead: `Auto Sync · 5`.
- **R1-P2-3 Floating selection bar.** `5 selected` wraps onto two lines: add `whitespace-nowrap` to `selbar-count`. The bar covers the last grid row: while it is shown, add `padding-bottom: 56px` to the grid scroller so the last row can scroll clear of it.
- **R1-P2-4 `Auto 0` popover.** Esc does not close it, and only the click-catcher does. Close it on Esc (and on a step / filter change).
- **R1-P2-5 Capture dialog copy.** The model scope reads "every Sony ILCE-7M4 (…9876) photo from any body", which names one body and then says "any body". Change it to "This camera model: every Sony ILCE-7M4 photo, any body (45)".
- **R1-P2-6 After `Apply to 8` from Review suggested rejects, the grid is empty** ("No photos match the current filters", chip `Review 0 suggested rejects` still pressed; `rc1-after-apply-1280.png`). When the apply ran under the `suggested: "reject"` filter, switch the view to Rejected · Auto (the `Auto` segment's filter) and toast "Rejected 8 photos, shown here. [Undo]". When the pending count is 0, render the chip as muted text "No reject suggestions" instead of a pressed button.
- **R1-P2-7 Apply dialog counts.** "Rejects ( 8 )" has stray spaces because the flex `gap-1.5` applies between "(", the count span and ")". Wrap `({n})` in one span.
- **R1-P2-8 Guided shows two pressed Upright buttons** (`rc1-guided-1280.png`: Guided and Vertical). While the Guided tool is armed, press only Guided and show the stored mode as a hint ("Current: Vertical, kept until 2 guides are drawn"). Also add an on-image badge top-left, like the WB picker's: "Guided Upright: drag 2–4 lines along straight edges · x deletes · Esc done". The instruction only lives in the panel today, which is often scrolled away.
- **R1-P2-9 Grid while dragging Transform sliders** (Lightroom shows a grid overlay while any Transform slider is dragged). Show the crop tool's 10×10 fine grid over the image while a `tf-*` slider is being dragged or nudged, and hide it 300 ms after release.
- **R1-P2-10 Crop handle modifiers** (Lightroom): Alt-drag on a handle resizes symmetrically about the centre, and Shift-drag on a free crop keeps the current ratio for that drag. `CropOverlay` `down` / `move` ignore modifiers today. Pass `e.altKey` / `e.shiftKey` into `resizeRect`.
- **R1-P2-11 Cheat sheet / Cmd+Alt+R.** Add rows "Cmd-drag: draw a straighten line" and "Shift+double-click Angle: Auto straighten" (While cropping). In Lightroom, Cmd+Alt+R resets the crop from anywhere in Develop: drop `needs: "crop"` from `cropReset`, so outside the tool it resets `crop` only, as one "Reset Crop" history entry.

### Remaining first-pass P2s

Resolved: P2-1 (Space to the last preset), P2-2 (info card placement), P2-3 (camera names), P2-4 (sub-second offset), P2-5 (covered by P1-3), P2-7 (`Auto` / `Auto tone`), P2-8 (toast placement while cropping), P2-9 (crop bar only with the panel hidden), P2-10 (toast copy), P2-13 (cheat-sheet group "Copy & paste settings"), P2-14 (filmstrip scene badge).
Still open:
- **P2-6**: optional capture time on grid cells.
- **P2-11**: Plan rows still show "To do: edit this photo" plus a truncated "Edit the represent… Open representative" (1280).
- **P2-12**: the strictness row is still a second row at 1728.

**Open P2 total: 14** (R1-P2-1 … R1-P2-11, P2-6, P2-11, P2-12).

### Keyboard map changes from this re-check

| Action | Today | Proposed | Lightroom Classic | Notes |
|---|---|---|---|---|
| Leave the crop tool by changing photo / view | discards | **commits** (and stays on for the next photo in Develop) | commits | R1-1 |
| Reset crop | Cmd+Alt+R while cropping | Cmd+Alt+R anywhere in Develop | same | R1-P2-11. No conflict: Cmd+Shift+R resets all. |
| Confirm Edit Capture Time | – | Enter (not in the search field) | – | R1-4 |
| Close the `Auto 0` popover | click outside | + Esc | – | R1-P2-4 |
| Crop handle Alt / Shift drag | – | Alt = from centre, Shift = keep ratio | same | R1-P2-10 |

No new chord conflicts. The only duplicate chords in `keymap.ts` are the intentional crop-scoped ones: X / O / Shift+O / A while cropping, ahead of reject / mask overlay / overlay style / auto mask. The cheat sheet scrolls inside the modal and is filterable at 1280×800 (content 1,648 px in a 637 px viewport) without overflowing the window, so it needs no change.

---

## Re-check 2 (2026-10-05, HEAD 2c5dff6, contract v19.3)

Evidence: mock backend on `vite --port 1452` (`/?mock=200&scope=all`, `/?mock=60&scope=all&twocams=1`), a throwaway Playwright driver at 1280×800 and 1728×1117 (deleted). It covered 16 exit keys from an open crop tool, plus undo, Reset all and a History click while cropping, plus stepping through an Upright photo. The suites `ux-8d-r1`, `ux-8d-r1b`, `ipc-v19-3-mock`, `crop-lr`, `crop-geometry`, `upright`, `ux-8d-p1`, `ux-8d-p2`, `ux-8d-v192` and `apply-scene-8d` pass on their own server (78/78).
Kept screenshots (`test-data/ux-review-8d/`): `rc2-crop-stale-after-undo-1280.png`, `rc2-capture-1280.png`, `rc2-crop-upright-1280.png`.

**Summary: open P0 0 · P1 2 (both new, both side effects of R1-1's commit-on-exit: R2-1 undo / reset / history while cropping is re-applied on exit, R2-2 stepping through Upright photos with the tool open writes a crop to each) · P2 3 new + P2-6 = 4.** R1-1 … R1-4 are resolved. R1-P2-1 … 11, P2-11 and P2-12 are resolved.

### Status of the re-check 1 P1s

| # | Status | Evidence |
|---|---|---|
| R1-1 Crop commits on exit | **Resolved** (two side effects: R2-1, R2-2) | Angle typed 2.5 / 4, then each exit key. **Commit, one `Crop` save on the photo being left:** → (and the tool re-opens on photo 2 at 0.0° with the same lock and overlay), G, E, C (Compare in Develop), Shift+T, Cmd+Alt+1 / 2, K, Cmd+Shift+E, filmstrip click. Three quick → presses give exactly one save. **No commit, tool stays open:** Esc, then Esc again on the re-opened tool. **No navigation, tool stays open (correct):** `\`, Tab, Shift+Tab, Y, D, Cmd+Shift+V, Cmd+Shift+C, Cmd+S. Clicking a filmstrip frame within 1.5 s of C does not re-open the tool inside Compare, so the carry does not leak. 1280 and 1728 behave identically. |
| R1-2 Typed angle | **Resolved** | Typing 5 + Enter gives `+5.0°`, and the rect is the analytic 0.0563 inset (suite). On an Upright photo, typing −1.3 gives `-1.3°` with the tool still open. The angle stored on Done is −1.3. |
| R1-3 Crop vs Upright | **Resolved** (mock; confirm the real warp on the Mac) | While cropping, `render_preview` sends `crop.enabled=false, transform.constrainCrop=false`. With Vertical + Constrain Crop on, R starts at the constrained crop `{l .06, t .12, r .94, b 1}` with the mock trapezoid `validQuad`. Dragging the SW handle to the frame corner stops at the warp edge (`l` stays .06). `PaperFill` does not paint outside `validQuad`, but the real render already shows the white warp wedges, so this needs no change. See P2 R2-P2-1 for the panel position. |
| R1-4 Capture dialog | **Resolved** | `rc2-capture-1280.png`: on a pre-filled pair at 1280×800, Apply sits at y 715–747 with a sticky footer (Shift tab 569, Set tab 570). Enter on a focused tab applies once, and Esc cancels (suite). Initial focus is the reference search box, so Enter straight after Cmd+Shift+T does nothing (R2-P2-2). |

### P2 status

Resolved, checked by suite and spot check: R1-P2-1 notice / tooltip copy, R1-P2-2 `Auto Sync · 3` on one line with `Sync…`, R1-P2-3 one-line count and 56 px grid padding, R1-P2-4 Esc closes `Auto 0`, R1-P2-5 "This camera model: every Canon EOS R5 photo, any body (20)" (`rc2-capture-1280.png`), R1-P2-6 Apply lands on Rejected · Auto with "Rejected N photos, shown here" and "No reject suggestions", R1-P2-7 `Rejects (8)`, R1-P2-8 only Guided pressed plus the hint and the on-image badge, R1-P2-9 transform grid, R1-P2-10 Alt / Shift handle drags, R1-P2-11 cheat-sheet rows and Cmd+Alt+R outside the tool ("Reset Crop", one entry), P2-11 Plan to-do row, P2-12 strictness inline at ≥1440.
Still open: P2-6 (optional capture time on grid cells). No regressions found in these areas.

### New P1

#### R2-1 Undo / Reset all / a History click while the crop tool is open is silently re-applied on exit
- **Where**: `develop/DevelopView.tsx`. `cropTool` is seeded once in `startCrop` from `editor.adj.crop`. `editor.undo` / `redo`, `doReset` (Cmd+Shift+R), the History panel, paste (Cmd+Shift+V), presets and Previous all change `editor.adj.crop` underneath without touching the tool. `commitCrop` then compares the stale tool to the new `adj.crop`, sees a difference and saves the old crop again.
- **Evidence**: `rc2-crop-stale-after-undo-1280.png`. Commit a 4° crop, press R, then click History "Original". The history shows Original selected and the filmstrip cell has lost its "edited" mark, but the tool still shows `+4.0°` with the rotated frame. Press → and you get `save_adjustments` `Crop` with angle 4 on photo 1: the undo is gone. Cmd+Z and Cmd+Shift+R give the same result, at 1280 and 1728.
- **Why**: this got worse with R1-1. Before, → discarded the stale tool, so the undo survived. Now every exit re-commits the stale tool. Undoing a bad straighten is precisely what someone does while the crop tool is open, and the result is the opposite of what History says.
- **Fix**:
  1. Seed the tool from the stored crop whenever `editor.adj.crop` or `editor.adj.transform` changes from **outside** the tool while it is open: undo / redo, History click, Reset all, Reset Crop, paste, preset, Previous, sync, Auto Sync from another photo. Re-run `startCrop(carry = current tool)` so the aspect lock, overlay and Constrain to image are kept. Implementation: keep `toolBaseline = JSON(editor.adj.crop)` when the tool is seeded. In an effect on `editor.adj.crop`, if the tool is open and `JSON(adj.crop) !== toolBaseline`, re-seed and update the baseline. `commitCrop` sets the baseline to what it saves before calling `editor.change`, so its own commit does not trigger a re-seed.
  2. Cmd+Z with uncommitted tool changes (the tool differs from its seed): the first Cmd+Z reverts the tool to its seed without touching history, and the toast reads "Crop changes undone". The next Cmd+Z is a normal undo, and the tool re-seeds from it (step 1). This matches Lightroom, where the overlay always shows the current history state.
- **Acceptance**:
  - Commit 4°, R, Cmd+Z: the tool shows `0.0°` and the full frame, and → saves no `Crop` on photo 1.
  - The same holds with a History click on "Original" and with Cmd+Shift+R.
  - R, angle 3 (uncommitted), Cmd+Z: the tool goes back to its opening state and there is no history change.

#### R2-2 Stepping through Upright-corrected photos with the crop tool open writes a crop to every photo passed
- **Where**: `DevelopView.tsx` re-open effect (`reopen` → `startCrop(carry)`) together with the `get_transform_bounds` effect, which re-fits the rectangle to `validQuad` 150 ms later (`changeCrop((t) => t)`), and `commitPendingTool`, which commits any difference from the stored crop.
- **Evidence**:
  - Photo 2 has Upright Vertical with Constrain Crop off. On photo 1, press R, then →. The re-opened tool on photo 2 shows the full frame `{0,0,1,1}` at 100 ms, then jumps to `{.0566 …}` at 1 s. Press → again and you get `save_adjustments` `Crop` on photo 2, although the crop tool was never touched there.
  - The same happens on any Upright photo with R then → (`1:Crop:0`).
  - On the Mac, an Upright Auto synced across a scene means that holding → through it with the tool open crops all of them, and the frame visibly jumps on each.
- **Why**: navigation must not edit photos the user only looked at. It also adds a "Crop" history entry and an XMP write per frame. In Lightroom, Constrain to Image is stored per photo, so browsing never changes a crop.
- **Fix**:
  1. Distinguish **explicit** from **implicit** commits. Done, Enter and R commit what is on screen (today's behaviour). Implicit exits (photo change, G / E / C, step switch, Shift+T, mask keys, Export, toolbar view buttons) commit only when the user changed the tool since it was seeded: handles, move, angle, aspect / lock / swap, Reset, Auto, straighten line. Add `dirty: boolean` to `CropTool`. `changeCrop` sets it when the change is user-initiated. `startCrop`, the quad re-fit and the R2-1 re-seed do not set it. `commitPendingTool` skips `commitCrop` when `!dirty` and just closes the tool, which is still carried to the next photo.
  2. No jump: when the photo has a non-neutral transform, `startCrop` awaits `get_transform_bounds` before showing the tool, as it already does with Constrain Crop on. The first frame you see is the fitted one.
- **Acceptance**:
  - The two-photo scenario above gives 0 `save_adjustments` on photo 2.
  - The `crop-rect` on photo 2 is never `{0,0,1,1}` once the overlay is visible.
  - R, then Enter on the same Upright photo still saves one `Crop`, because Enter is explicit.
  - R, angle 2, → still saves one `Crop` (R1-1).

### New P2

- **R2-P2-1 The crop panel can be scrolled out of view when R opens it** (`rc2-crop-upright-1280.png`: Transform open, so the Angle field, Constrain to image and Done are above the fold at 1280 and 1728). This is exactly the Upright → R path. When the tool opens, scroll the right panel so `crop-panel` is in view (`scrollIntoView({ block: "nearest" })`). Restore the previous scroll position when the tool closes.
- **R2-P2-2 Enter right after Cmd+Shift+T does nothing on a pre-filled pair**, because focus starts in the reference search box. When both frames are already chosen (a one-per-camera selection), put the initial focus on the `Sync two cameras` tab, so Enter applies. Keep the search box as the initial focus when a frame still has to be picked. Also: Enter in the search box with an empty query applies (when `canConfirm`).
- **R2-P2-3 Cmd+S and Copy (Cmd+Shift+C) while cropping act on the stored crop, not the one on screen.** Lightroom applies the overlay first. Add `"saveXmp"` and `"copy"` to `LEAVES_CROP` in `App.tsx`, so they call `commitPendingTool()` first. After R2-2 this is a no-op when the tool is untouched.

### Keyboard map changes from this re-check

| Action | Today | Proposed | Lightroom Classic | Notes |
|---|---|---|---|---|
| Cmd+Z / Cmd+Shift+Z while cropping | undoes underneath the tool; the stale tool re-applies on exit | 1st Cmd+Z reverts uncommitted tool changes; then normal undo and the tool re-seeds | overlay follows history | R2-1 |
| Navigate with the crop tool open, untouched | commits a re-fitted crop | closes and re-opens without saving | no change to the photo | R2-2 |
| Cmd+S / Cmd+Shift+C while cropping | ignore the on-screen crop | commit first (if dirty) | applies the overlay | R2-P2-3 |

No new chord conflicts. Cmd+Alt+R (Reset Crop, now Develop-wide) vs Cmd+Shift+R (Reset all) vs R (crop tool) are distinct. `cropStraightenDrag` / `cropAutoStraighten` are display-only rows with no chords.

**Open P0 0 · P1 2 (R2-1, R2-2) · P2 4 (R2-P2-1 … 3, P2-6).** Neither P1 needs a contract change.

## Re-check 3 (2026-10-05, HEAD 330b2c7, contract v19.3)

Evidence: mock backend on `vite --port 1455` (`/?mock=200&scope=all`, `/?mock=60&scope=all&twocams=1`), with a throwaway Playwright driver at 1280×800 and 1728×1117 (deleted). The suites `ux-8d-r2`, `ux-8d-r1`, `ux-8d-r1b`, `crop-lr`, `crop-geometry`, `upright` and `ipc-v19-3-mock` pass on their own server (57/57).
Kept screenshots (`test-data/ux-review-8d/`): `rc3-crop-scroll-1280.png` (Upright, then R: the crop panel is in view) and `rc3-transform-while-crop-1280.png` (Vertical +40 while cropping, untouched tool).

**Summary: open P0 0 · P1 0.** R2-1 and R2-2 are resolved, and so are R2-P2-1 … 3 (R2-P2-1 with a small scroll-restore offset, R3-P2-1). No regressions came from the dirty flag or the re-seed. There are 3 new P2s, one of them pre-existing, plus P2-6.

### Status of the re-check 2 P1s

| # | Status | Evidence (1280 and 1728 identical) |
|---|---|---|
| R2-1 Tool follows history | **Resolved** | Commit 4°, R (`+4.0°`), then Cmd+Z: the tool shows `0.0°` and `{0,0,1,1}`, and → gives 0 `save_adjustments`. The tool re-opens on photo 2 at `0.0°`. History "Original" and Cmd+Shift+R give the same result (suite). R, angle 3, Cmd+Z: back to `+4.0°`, 0 `undo_adjustments`, toast "Crop changes undone". The next Cmd+Z is a normal undo, and the tool re-seeds to `0.0°`. Paste (Cmd+Shift+V with Crop ticked) while cropping re-seeds the tool to the pasted `+4.0°`, and the next → gives no extra `Crop` save. |
| R2-2 Implicit exits commit only touched tools | **Resolved** | Photo 2 has Upright Vertical. R on photo 1, then →, then →: 0 saves. Polling `crop-rect` every 50 ms on photo 2 never shows `{0,0,1,1}`, so the tool no longer jumps. R, then Enter on the Upright photo saves one `Crop`, because Enter is explicit. R, angle 2, then → saves one `Crop` (R1-1). Changing the overlay (O) or toggling the Angle tool does not count as a change: → gives 0 saves. Three paced → presses with an untouched tool give 0 saves and keep the tool open. |

### Regression probes (dirty flag, re-seed, accepted transform deviation)

- **Explicit Done still saves.** Done on an untouched tool on a plain photo saves nothing (correct: no change). Done after angle 2.5 saves one `Crop`, and R re-opens at `+2.5°`. A handle drag followed by Enter saves one `Crop`.
- **Quick → after a real edit.** Angle 3, then three → presses with no pause give exactly one `Crop`, on photo 1. However, the tool is closed on photo 4 (R3-P2-2, pre-existing).
- **Transform slider while cropping** (Constrain Crop off, untouched tool). Vertical +40 records one `Transform: Vertical`, and the overlay re-fits to `{.0741 … .9259}` (`rc3-transform-while-crop-1280.png`). → then saves nothing, so the stored crop stays full frame. Coming back with ← re-opens the tool on the same fitted rectangle, and Enter saves it. This matches the accepted deviation (the baseline tracks `adj.crop` only) and Lightroom: with Constrain Crop off, the white warp edges show until you apply a crop. The fitted rectangle is a proposal that is consistent and repeatable, and browsing never writes it. **No change needed.**
- **Exposure changed while the tool is dirty.** Angle 3, then Exposure +1, then Cmd+Z: the crop tool reverts and Exposure stays at +1. The second Cmd+Z undoes Exposure. Undo order is inverted here (R3-P2-3). Cmd+Shift+Z with a dirty tool redoes the adjustment and keeps the tool's edit (`+5.0°`), which is fine.
- **Cmd+S / Cmd+Shift+C** (R2-P2-3). When the tool is untouched, neither saves a `Crop`. When it is dirty, each saves one `Crop` first and then copies / writes the XMP. Both now close the tool. For Copy this is fine, because a dialog opens. For Cmd+S it is a small change from re-check 2, where the tool stayed open (folded into R3-P2-3).

### P2 status

- **R2-P2-1** Resolved, with a residue: after Upright, R brings Angle, Constrain to image and Done into view at 1280 (`rc3-crop-scroll-1280.png`; at 1728 the panel already fits). On Esc the right panel returns to scrollTop 583 instead of 517 at 1280, a 66 px shift (R3-P2-1).
- **R2-P2-2** Resolved. On a pre-filled pair, initial focus is on `capture-tab-sync` and Enter applies at once (1 `edit_capture_time`). Enter in the empty reference search applies, and Enter with text in the search does not. With a single photo selected, focus starts on the Shift tab.
- **R2-P2-3** Resolved (see probes above).
- **P2-6** is still open (optional).

### New P2

- **R3-P2-1 Closing the crop tool does not restore the right-panel scroll exactly** (`DevelopView.tsx` R2-P2-1 effect). Probable cause: the scroll position is captured in a `setTimeout` after the crop panel has already been inserted, and browser scroll anchoring has adjusted it by then. **Fix:** capture `{el, scrollTop}` synchronously in `startCrop` before `setCropTool`, and only when the tool is opened by the user (R / toolbar, not the re-seed or re-open paths). Restore it in a `useLayoutEffect` when `cropOpen` turns false, and set `overflow-anchor: none` on the right-panel scroller. **Acceptance:** at 1280×800 with Transform open (scrollTop 517), R then Esc gives scrollTop 517 ±1.
- **R3-P2-2 Holding → with the crop tool open closes the tool after the first frame** (pre-existing since R1-1, not caused by R2). The `[id]` effect replaces `reopen` with `carryRef.current`, which is `null` when the next → arrives before the re-opened tool exists. **Fix:** keep a pending carry, so that the id effect becomes `setReopen((prev) => carryRef.current ?? prev)`. The tool then re-opens on whichever photo you land on. Untouched tools never commit (R2-2), so this cannot write crops. **Acceptance:** R, angle 3, then three → presses with no delay: one `Crop` on photo 1, and the tool is open on photo 4 at `0.0°` with the same lock and overlay.
- **R3-P2-3 Cmd+Z / Cmd+S ordering with a dirty tool.** (a) The first Cmd+Z always reverts the tool, even when a slider change was made after the tool edit. Lightroom undoes the most recent action. **Fix:** stamp `dirtyAt` on the tool when it first becomes dirty. In `undoAdj`, call `revertTool()` only when `dirtyAt >= max(lastCommitAt, cull.undoAt, lastBatch.at)`. Otherwise do a normal undo and keep the tool's edit (the crop is unchanged, so there is no re-seed). (b) Cmd+S: after `commitPendingTool`, re-open the tool on the same photo (`startCrop(carry)`), so saving metadata does not exit Crop. **Acceptance:** angle 3, Exposure +1, Cmd+Z: Exposure goes back to 0 and the tool stays at `+3.0°`. Then Cmd+Z again: "Crop changes undone". R, angle 2, Cmd+S: one `Crop`, the XMP is written, and the tool is still open at `+2.0°`.

### Keyboard map changes from this re-check

| Action | Today | Proposed | Lightroom Classic | Notes |
|---|---|---|---|---|
| Hold → with the crop tool open | the tool closes after the first frame | the tool stays open on the landing photo | the tool stays open | R3-P2-2 |
| Cmd+Z with a dirty tool after a later slider change | reverts the tool first | undoes the newest action | newest first | R3-P2-3 |
| Cmd+S while cropping | commits if dirty, closes the tool | commits if dirty, the tool stays open | the tool stays open | R3-P2-3 |

No new conflicts.

**Open P0 0 · P1 0 · P2 4 (R3-P2-1 … 3, P2-6).** None of them needs a contract change.
