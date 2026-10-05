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
