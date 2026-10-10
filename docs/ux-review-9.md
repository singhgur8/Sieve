# Phase 9 UX review: Pick the best N (target-count culling)

Author: ux-designer · 2026-10-10 · branch `phase-9-target-cull` at dc5b9c7
For: frontend-dev (all items), architect (items marked **[ARCH]**)

Evidence: mock backend (`vite --port 1464`), Playwright drivers at 1280×800 and 1440×900 using `?mock=5000`, so project 1 "ceremony" has
**2,500 photos**, and a target of **800**. I ran three setups: no switch with the flags cleared (a fresh shoot), `&target=1`, and `&target=answered`.
The drivers were throwaway. The screenshots are in `test-data/ux-review-9/`:
`01-cull-offer`, `02-setup`, `04-summary`, `10-cull-with-run`, `11–15-people*`, `20–24-review*`, `25-cheatsheet`, `30–34-second*`,
`40–43-apply*`, `50-show-in-grid` (each as `-1280.png`, most also as `-1440.png`).
Engine copy was checked against `src-tauri/src/ml/selection.rs` (`choose`, `delivered_reason`) and `src-tauri/src/db/target.rs` (`apply`, `add`, `swap`, `set_choice`, `covered_by`).

**Summary: P0 3 · P1 11 · P2 11.**

The parts work, and the keys are mostly well chosen. The flow still does not answer the user's core complaint ("I ended up still going through each photo individually"):
1. On a 2,500-frame shoot, the Second look queues **1,488 photos one at a time** (59 % of the shoot).
2. Apply writes **rejects** after the dialog says "nothing is rejected".
3. "Already kept a similar one" ignores the photos the user has just added, which is the "a very similar photo was already added" case.

## Mock run numbers (2,500 photos, target 800)

| | Count | How the UI reviews it |
|---|---|---|
| Picked (deliver) | 534 (target 800, no explanation of the shortfall) | Pass 1, one photo at a time |
| Alternatives | 478 | Strip under each pick |
| Not sure | 493 | Pass 2, one at a time |
| Set aside | 995 (incl. confident defects, near-duplicates, weak frames) | Pass 2, one at a time, mixed in with Not sure in id order |

The mock undershoots the target (534 of 800). The real engine fills with next-best frames, so it should get closer. The pass 2 problem stays either way:
everything that is not delivered or an alternative goes into the queue.

## Workflow verdicts

| Workflow | Steps today | Verdict |
|---|---|---|
| Discover from the Cull step | Banner "Start here: let Sieve pick the best ~830…" → `Pick the best N…` | Good on a fresh shoot. The banner hides as soon as **any** flag exists. At 1280 the only other entry is an unlabelled sparkle icon. There is no shortcut. After a run, the Cull step shows nothing about it (P1-9). |
| Setup → run → summary | Type 800 → Enter → progress "step 1 of 3" → summary by shot type | Clear. The undershoot is unexplained and the cost of the Second look is hidden (P1-10). |
| People | Y confirms the couple → per card Y / N, arrows → `Re-run with these people` | Fast and clear. At 1280×800 the Yes / No buttons sit under the footer (P1-8). Cmd+Z says "Nothing to undo" (P2-3). |
| Re-run | Footer button → back to Pick with a new summary | Works. Photos approved by moving past them (Right) are not protected (P1-4). There is no "what changed" (P2-4). |
| Pass 1 (Review picks) | Per pick: Right / Z, Tab to cycle, S swap, A add, X set aside | Good keys and a clear Picked vs Alternative split. Reopening restarts at photo 1 (P1-2). Undo does not return to the photo (P1-3). There is no zoom (P1-7). "Reject" does not reject (P1-5). Enter swaps (P1-6). The review is per photo, not per moment (P1-1). |
| Pass 2 (Second look) | Per photo: Space skip, S swap, A add both | **Fails review-by-exception**: 1,488 photos, defects included, in id order (P0-1). Covered-by is stale after adds (P0-3). |
| Apply + keeper offer | `Apply flags…` → Apply → "Flagged 815 photos as Picked" → `Use picks as keepers` | The copy is wrong (534 picks announced, 815 "Picked", 362 actually rejected). Cmd+Z cannot undo it (P0-2). The keeper offer itself is good. |
| Help | `?` icon in the overlay header, F1, the cheat sheet groups "Pick the best N" first | Good. The Help text inherits the wrong "nothing is rejected" (P0-2) and "X rejects" (P1-5). |

---

## P0

### P0-1 The Second look is a 1,488-photo, one-at-a-time pass
- **Where**: `target/SecondStep.tsx` (`listImageIds({targetChoices: ["not_sure","set_aside"]})`, `skip`/`advance`), summary `SetupStep.tsx` (`target-next-second`). Screenshots `30-second-1280.png`, `31-second-later-1280.png`.
- **What**: Not sure (493) and Set aside (995) form one queue in id order. The first five photos were: blown highlights on skin, a near-duplicate, low overall quality, motion blur, a duplicate in burst. Confident-defect frames (`Forced::Defect`, which Apply will reject anyway) and near-duplicates at 0.9 or more sit next to the real close calls.
  Each one needs a key press: 1,488 presses, the same "each photo individually" the user complained about.
- **Why**: the rule says pass 2 is "fast". Fast means only the photos where the user's eye changes the outcome. The defects and the near-duplicates of a kept photo were already decided by the rules the user gave.
- **Fix** (frontend; the reason kinds are already on `ImageSelection.reasons[0].kind`):
  1. **Split the queue into four piles**, shown as a segmented control in the Second look sub-header (replacing "0 of 1,488 reviewed"):
     `Not sure 493 · Similar to a kept photo 712 · Weaker frames 141 · Defects 142`.
     The counts are illustrative. Classify each row by its first reason:
     - **Not sure**: choice `not_sure`.
     - **Similar to a kept photo**: `set_aside` with kind `near_duplicate` / `not_best_of_setup`, or with a covered-by of 0.7 or more.
     - **Defects**: `set_aside` with kind `defect` / `user_choice` "You rejected".
     - **Weaker frames**: everything else (`below_target`, `no_visible_face`, `detail_out_of_focus`, `other`).
  2. **Default pile = Not sure.** The overlay opens there, the summary button reads `Second look · 493 not sure`, and Pass 1's `Second look →` goes there.
  3. **Similar to a kept photo** opens as a **moment grid**, not one at a time. There is one row per moment: on the left the kept photo(s) of that moment with a green `Picked` chip, on the right the set-aside frames at 160 px, each with a small similarity %.
     Keys: arrows move, A adds the focused frame, S swaps it with the moment's kept photo, Shift+Right / `]` jumps to the next moment. The row is marked reviewed when you leave it.
     For a quick visual check this is 15–30 rows per screen instead of 712 key presses.
  4. **Defects** and **Weaker frames** are not part of the pass. They are reachable from the segmented control and open as a plain grid with the reason on each cell (the existing `Show in grid` filter plus a reason chip).
     Copy under the control: "Defects are rejected when you apply. Weaker frames stay unflagged."
  5. **Order** inside the Not sure pile: by moment (capture time of the moment), then by `score` descending, so each moment's close calls come together.
  6. Shift+Space (or Shift+Right) = "Skip the rest of this moment": marks every remaining photo of the current moment in this pile reviewed and jumps to the next moment.
- **[ARCH], optional**: `ImageQuery.targetReasonKinds?: TargetReasonKind[]`, so the grid and the counts do not need every row fetched. Until then, fetch `get_image_selections` in pages of 500. 1,488 rows is fine.
- **Acceptance**:
  - With `?mock=5000&target=answered` the Second look opens on Not sure, and `data-total` equals `counts.notSure`.
  - The pile counts add up to `notSure + setAside`.
  - In the Similar pile, every row shows its moment's delivered photo.
  - Shift+Space moves to the first photo of the next moment.

### P0-2 Apply: the copy says nothing is rejected, then it rejects; the counts are conflated; it cannot be undone **[ARCH]**
- **Where**: `TargetView.tsx` `ApplyDialog` (`target-apply-text`, `target-apply-result`), the `target-apply` button title, `helpContent.tsx` `pick-best-n`. Backend: `db::target::apply` writes `pick = suggested_pick`, and the overlay keeps `reject` for confident defects.
  Screenshots `40-apply-1280.png`, `41-applied-offer-1280.png`, `43-grid-after-apply-1280.png`.
- **What**: the dialog says "This marks the **534** picked photos as Picked … **nothing is rejected** or deleted". The result says "Flagged **815** photos as Picked". The grid afterwards shows **Rejected 541 (Auto 362)**: 362 photos were rejected in the catalog and in their XMP (`xmpDM:pick="-1"`).
  `applied` counts every changed row (picks + rejects + unflags) and the copy calls all of them "Picked". The button title says "you can undo it", but Cmd+Z after closing gives "Nothing to undo".
  The Run tooltip says "Nothing is flagged until you apply", yet swap / add / set aside in the passes write pick flags (and XMP) at once (`write_user_flag`).
- **Why**: this is about trust in what lands in the sidecars. Lightroom will show 362 rejects the user was told would not exist, and there is no way back from inside Sieve.
- **Fix**:
  1. **[ARCH]** `plan_target_apply(projectId) -> { picks: u32, rejects: u32, unflags: u32, unchanged: u32, userFlagged: u32 }`, a dry run of the same SQL. Add `apply_target_selection(projectId, { rejects: bool })`; with `rejects: false`, defect rows stay unflagged.
     Return the `CullSnapshot`s of the changed rows (as `apply_suggestions` / the cull undo do), so one Cmd+Z restores them. Register the apply in the Cull undo stack, label "Apply Pick the best N".
  2. Dialog copy (numbers from the plan):
     > **Apply the picks?**
     > • **534** photos get the Picked flag.
     > ☑ **Reject 362** photos Sieve is confident are defects (closed eyes, missed focus, blur). *(checkbox, on by default when strictness is Balanced or Aggressive)*
     > • 12 photos lose an old Sieve pick.
     > Your own flags (358) and all stars stay as they are. Flags go to the catalog and the XMP sidecars. Cmd+Z undoes it.
     Button: `Apply to 908 photos` (follows the checkbox).
  3. Result copy: "Picked 534 · Rejected 362 · Unflagged 12. 1,592 left as they were." Add an `Undo` button (`target-apply-undo`) next to Close.
  4. Run tooltip: "Your keeps, swaps and adds are flagged as you make them. Apply flags the rest."
  5. Help, replace "Nothing is deleted or rejected by the selection itself." with: "Nothing is deleted. Apply flags the picks, and (if you leave the box ticked) rejects the photos Sieve is confident are defects. Cmd+Z undoes it."
- **Acceptance**: the dialog numbers equal the plan. After Apply, `Picked`/`Rejected` deltas in the Cull summary match the result line. Unticking rejects → 0 new rejects. Cmd+Z restores all flags (snapshot equality).

### P0-3 "Already kept a similar one" ignores the photos the user adds **[ARCH]**
- **Where**: `SecondStep.tsx` (`getCoveredBy`). Backend: `db::target::add` / `set_choice` do not touch other rows' `covered_by`; only `swap` repoints it.
- **What**: in the Second look I pressed A ("Add both") on DSC00018. The next frame, DSC00020 (same moment), still said "Already kept a similar one: **DSC00023** · 67 %", not DSC00018, the photo added one second earlier.
  The same happens after Pass 1 "Add too". Covered-by only knows the engine's picks. To get from 534 to 800 the user adds about 266 photos, mostly inside existing moments, and each second add in a moment shows no warning.
- **Why**: this is exactly the user's pass 2 complaint ("a very similar photo was already added from the good list").
- **Fix**:
  1. Frontend, works today: under the right-hand "kept" pane add a **"Kept from this moment (N)" strip**: every `deliveredIds` of the photo's moment (from `listMoments`) as 64 px thumbs.
     Photos the user added (reason `user_choice` "You added this" / "You swapped this in") get a sky ring and the label "you added".
     When the moment already has a kept photo that the user added after the run, the caption line becomes: "**You already added DSC00018 from this moment**. Already kept: DSC00023 (67 %)". The `Add both` button turns amber: `Add a 3rd from this moment`.
  2. **[ARCH]** `add` / `set_choice(deliver)` / `restore` recompute `covered_by` + `covered_similarity` for the non-delivered rows of the same moment against the new delivered set (`moments::similarity` over the stored signals). Moments are small, so this is cheap. Then the big pane shows the truly nearest kept photo.
  3. Similarity wording (`covered_by` text, backend): ≥ 0.9 "Almost identical to DSC0412 (kept)", 0.7–0.9 "Similar to DSC0412 (kept)", below 0.7 "Same moment as DSC0412 (kept)", different moment "Looks like DSC0412 (kept, another moment)".
- **Acceptance**: in the Second look, Add on photo A, then go to B in the same moment: the strip shows A with "you added" and the caption names A. With the ARCH change, `get_covered_by(B).coveredById == A` when A is nearer.

---

## P1

### P1-1 Pass 1 is per photo; add a moment view and "accept the rest of this moment"
- **Where**: `ReviewStep.tsx`. Screenshot `24-review-rejected-1280.png`.
- **What**: 534 picks = at least 534 key presses (Right or Z), one photo at a time. The strip shows only the alternatives, not the **other picks of the same moment**. With "many couple variations" a moment can hold 4–6 picks, and you cannot see whether two of them are near-identical, which is the duplicate you want to catch here.
- **Fix**:
  1. The strip shows the whole moment: first the moment's other picks (green ring, label `Picked`), then the current pick's alternatives (sky ring, `#rank`). Header: "This moment: 4 picked · 6 alternatives". Clicking a pick in the strip jumps to it.
  2. Sub-header: add "Moment 12 of 180" next to "Photo 41 of 534".
  3. **Shift+Right** (and **Shift+Z**) = "Looks good, next moment": marks every pick of the current moment reviewed and jumps to the first pick of the next moment. Button `Next moment ⇧→` in the action bar.
- **Acceptance**: in a moment with 3 picks, Shift+Right adds 3 to `data-reviewed` and `data-current` is the first pick of the next moment.

### P1-2 Pass 1 restarts at photo 1 every time it opens
- **Where**: `ReviewStep.tsx` `loadList` (`idx.current` starts at 0). The Second look already resumes.
- **What**: I reviewed 8, pressed Esc, then reopened: "Photo 1 of 534". The same happens after `Show in grid` → sparkle icon. On 534 picks over two sittings, the user hunts for where they stopped.
- **Fix**: open at the first pick not in `loadReviewed(projectId,"review")` (as `SecondStep` does). Add **`]` / `[`** = next / previous *unreviewed*, and Home / End = first / last pick. When everything is reviewed, open at photo 1 with the green "All 534 picks reviewed" bar.
- **Acceptance**: review 8, close, reopen → `data-current` is the 9th pick.

### P1-3 Undo does not take you back to what it undid
- **Where**: `TargetView.tsx` `undo`. Screenshot `33-second-undo-1280.png`.
- **What**: after X on DSC00004 and a swap on DSC00010, Cmd+Z restored both, but the view stayed on the current photo. Only a toast ("Undid: Swap DSC00023 for DSC00020") says what happened. In pass 2 the undone photo is several frames back.
- **Fix**: store the photo id the action was made on in the undo entry, `{ label, previous, at: imageId, stage }`. On undo, switch to that stage when needed and select `at`. In Review that is the pick it was made on (for a swap, the original pick). In Second look it is the photo, and its "reviewed" mark is removed. The toast keeps the label.
- **Acceptance**: X on pick 4 → Right ×3 → Cmd+Z → `data-current` = 4 and `data-total` back to 534.

### P1-4 A re-run can drop picks the user already approved by moving past them
- **Where**: `PeopleStep.tsx` `rerun`, footer copy `target-rerun-note`, Help. Engine: only `locked` rows survive (`Forced::Locked`). Right in Review marks a pick "reviewed" in localStorage only. Z locks it.
- **What**: the hint bar says "Left / Right: previous / next photo", so most users approve by moving on. The footer promises "A re-run keeps every keep, reject and swap you made", but the photos they moved past are not locked and can drop out after "Re-run with these people".
- **Fix**: before `runTargetSelection`, call `setTargetChoice(reviewedDeliveredIds, "deliver")`, the delivered ids in the review set, which locks them. Do not push this to the undo stack.
  Footer copy: "Answers apply at the next run. A re-run keeps the picks you have reviewed and every keep, set-aside and swap you made."
  `clearReviewed` is never called. After a re-run, drop the reviewed ids that are no longer delivered.
- **Acceptance**: Right past 5 picks → change a person → re-run → those 5 are still `deliver` with `locked: true`.

### P1-5 "Reject" in Pass 1 does not reject
- **Where**: `ReviewStep.tsx` `target-reject` label; keymap `targetReject` label; Help "X rejects (moves it to Set aside)".
- **What**: the red `Reject X` button moves the photo to Set aside. `set_choice` never rejects, and the toast says "Set aside DSC00004". This contradicts the Apply copy and Lightroom's meaning of X.
- **Fix**: label `Set aside X` (keep the red tone), tooltip "Take it out of the delivery set. Nothing is rejected or deleted (X)". Keymap label "Set aside: take this photo out of the delivery set". Help: "X sets it aside (it is not rejected)".

### P1-6 Enter swaps (both passes)
- **Where**: keymap `targetSwap` chords `s`, `Enter`. Strip tooltip "Enter swaps it in".
- **What**: Enter is "accept / confirm" everywhere else (People: Enter = Yes; Grid: Enter = Loupe). Here it **replaces the pick** with the shown alternative (Review), or replaces the kept photo (Second look). An accidental Enter changes the delivery set and moves the view onto the other photo.
- **Fix**: remove `Enter` from `targetSwap`. Review: Enter = Keep and next (same as Z). Second look: Enter = Skip (leave as is). Strip tooltip: "Click to compare; S swaps it in (double-click also swaps)".

### P1-7 No zoom while choosing between near-identical frames
- **Where**: `ReviewStep.tsx` / `SecondStep.tsx` (`Pic` is a static `<img>`).
- **What**: choosing between "Almost the same as DSC00010" frames is choosing eye sharpness. Neither pass can zoom, so the user leaves the flow for Loupe.
- **Fix**: use `ZoomPane` for both panes, with **synced** zoom and pan (as in Compare).
  - **Review picks**: Space = Fit ↔ 100% at the main face (the Phase 8c Loupe convention). Click = zoom at the point.
  - **Second look**: Space stays Skip (the hot key). Click on either pane zooms both at the point; click again fits. Shift+Space is taken by "skip moment" (P0-1), so zoom is mouse-only here.
  - Zoom persists across Left / Right, as in Loupe since 8d.
- **Acceptance**: Space in Review → both panes at 100% on the same normalised point. Right → still 100%.

### P1-8 People: the Yes / No buttons sit under the footer at 1280×800
- **Where**: `PeopleStep.tsx` cards (`grid-cols-2 md:grid-cols-4`, the face fills the card width, about 188 px). Screenshot `13-people-answered2-1280.png`.
- **What**: the focused card's `Yes Y` / `No N` row is clipped by the sticky footer. Y / N still work, but you cannot see the answer you just gave.
- **Fix**: cap the big face at `size-36` (144 px) and centre it. Use `grid-cols-4 xl:grid-cols-5`. On focus change call `scrollIntoView({ block: "nearest" })` on the focused card, with `scroll-padding-bottom: 72px` on the scroller (the footer height).
- **Acceptance**: at 1280×800 the focused card's Yes / No buttons are fully visible after each answer, through the 4th card.

### P1-9 The Cull step does not show a run's state; entry and shortcut are weak; two "Apply" paths compete
- **Where**: `App.tsx` (TargetOffer condition `picked + rejected === 0`), `TargetEntry.tsx` `TargetButton` (icon-only below 1440), `CullSummaryBar` Suggestions. Screenshots `10-cull-with-run-1280.png`, `10-cull-with-run-1440.png`.
- **What**:
  - After a run, the grid looks unchanged. At 1280 the only trace is a sparkle icon with no label.
  - The summary bar says "Suggestions: 377 picks · 299 rejects · 1110 stars only — Apply…", a different number from "Best 534", with a different Apply (that one also writes stars).
  - The offer disappears once any flag exists (someone who flagged three frames never sees it).
  - There is no key to open the flow.
- **Fix**:
  1. When a run exists, show a **Best N row** where the offer sits (same height, 32 px, sky tint):
     `✦ Best 800: 534 picked · reviewed 41 of 534 · second look 12 of 493 · not applied yet   [Continue review] [Apply flags…]`.
     After apply: `… · applied 14:32`. `Continue review` opens the stage with unfinished work.
  2. The offer condition becomes "no run and fewer than 5 % of photos flagged".
  3. Shortcut **B** (Cull step, Grid / Loupe) opens Pick the best N, at the stage with unfinished work. Plain B is unused in the keymap, and Lightroom's B (Quick Collection) does not exist in Sieve.
     Tooltip and aria: "Pick the best N (B)". At 1280 keep the icon, but add the label `Best 534` once a run exists. It fits once the offer row carries the state.
  4. While a run exists, the Suggestions chip reads "Suggestions (from Best 800): 534 picks · 299 rejects · 1,110 stars — Apply…", and its dialog notes that stars are included. Or hide the chip and point to `Apply flags…`.
- **Acceptance**: after a run the row is visible at 1280 with live counts. B opens the overlay.

### P1-10 Summary: undershoot unexplained, the cost of the Second look hidden
- **Where**: `SetupStep.tsx` `target-summary`. Screenshot `04-summary-1280.png`.
- **What**: "Picked 534 of 2,500 photos · target 800" with no word on the 266 missing. `Second look` gives no count (it is 1,488 today).
- **Fix**:
  - When `deliver < 0.9 × target`, add: "266 short of 800: only 534 photos qualify under the rules. The 493 Not sure are the closest; add the ones you like in the Second look."
  - When `deliver > target`: "Over by 34: the first frame of every moment is always kept (up to +10 %)." This matches `TARGET_SLACK` core tiers.
  - Buttons: `Review the 534 picks`, `Second look · 493 not sure` (after P0-1).

### P1-11 The toast covers the alternatives strip / the photo
- **Where**: global `Toasts` while the overlay is open. Screenshots `24-review-rejected-1280.png`, `24-review-rejected-1440.png`, `34-second-nocover-1280.png`.
- **What**: every action toasts at the bottom centre (y ≈ 665–705 at 800 px). That covers alternatives #2–#3 and the caption, exactly where the eye goes next. With one action per photo, the strip is covered most of the time.
- **Fix**: while `target-view` is open, anchor toasts at the top right under the overlay header (`top: 100px; right: 12px`), 3 s, at most 1 visible (newest replaces). The header Undo button already carries the label ("Undo: Set aside DSC00004").

---

## P2

1. **Number format**: the run-finished toast reads "Picked 534 of 2500 photos". Use `toLocaleString` ("2,500"), as elsewhere.
2. **Second look state chip**: `Set aside` / `Not sure` is an 11 px black chip. Use the same chip style as `Picked` (amber `Not sure`, neutral `Set aside`) at the caption, next to the file name, and keep the reason on the same line.
3. **People undo**: Cmd+Z in People says "Nothing to undo". Either push role changes to the stack (label "Mark person important"), or make the toast "Change an answer with Y / N".
4. **Re-run diff**: after a re-run, toast plus summary line: "Re-run: +18 picked, −11 (moved to Not sure / Set aside); your 37 decisions kept".
5. **X in Second look** does nothing. Make X = "No: set aside and next" (Not sure → Set aside, locked). Muscle memory from Cull and Pass 1.
6. **Stars / labels in the passes**: 0–5 and 6–9 do nothing in Review / Second look. Allow them (write rating / label; target choice untouched), so favourites can be starred during pass 1. The user's 2nd / 3rd passes are star passes.
7. **Grid filter**: the Best N chip appears only after `Show in grid`. When a run exists, add a `Best N` segment to the filter row (Picks / Alternatives / Not sure / Set aside, with counts). Optionally add a small corner badge on cells (`B` green for picked, `A` for alternative).
8. **Keeper offer**: show the result: "Keepers become 849: 632 picks + 217 other photos you rated 1★+". The rule counts ratings outside the delivery set, so the number surprises otherwise.
9. **Moment label** repeats the shot type: badge `Couple` + "Couple · 02:00 PM · frame 2 of 8". Drop the type from the label when the badge shows.
10. **No-cover pane** (`34-second-nocover-1280.png`): half the screen holds one sentence. When nothing is kept nearby, show the moment strip (P0-3) and give the photo the full width.
11. **Empty alternatives**: with no alternatives, Tab / S / A are silent. Flash the strip's empty text ("No alternatives for this moment") for 600 ms on those keys.

Mock fidelity notes for QA, not UI bugs: the mock delivers a photo the user rejected (cell 36 "Rejected by you" under Best N: Picks, `50-show-in-grid-1280.png`). The engine never does (`Forced::UserReject`).
The mock also marks "Back of the head" as Not sure, where the engine sets it aside, and it undershoots the target more than the engine would.

---

## Per-user-rule verdict

| User rule (roadmap Phase 9) | Engine | UI | Verdict |
|---|---|---|---|
| Target is a guideline, not a cap; 2nd / 3rd pass | +10 % slack for core frames; fills with next-best | Copy says "a guideline, not a limit"; header "Picked 534 · target 800" | **Partly**: shortfall / overshoot unexplained (P1-10) |
| Main subject found automatically; ask "is this person important?" Y / N | Main pair, boosts 0.3 / 0.2 | Couple confirm (Y), cards Y / N, arrows, badge count, re-run | **Pass**, with layout fix P1-8 |
| Couple: many variations, only near-identical collapse | 0.9 near-dup bar, "Another pose or angle of the couple" | Pass 1 shows one pick at a time; the moment's other picks are invisible | **Pass (engine)**; UI needs the moment strip (P1-1) |
| Groups: one per setup, most looking, main looking; activity extras | Yes (+ Not sure when the couple never looks) | Reason "Best of this group: 5 of 6 faces looking" | **Pass** |
| Details: own share, one per detail, focus on the object | Yes (Not sure when out of focus) | Per-shot summary card "Detail 62 of 496" | **Pass** |
| Candids: face / action visible, crop-worthy; drop backs of heads | Yes ("Clear face, would crop well") | Reasons shown | **Pass** |
| Blur competes like any frame | Delivered blur gets "Intentional blur, the best of its moment"; others "Possible creative blur: your call" → Not sure | Reasons shown | **Pass** (slight lean to Not sure, acceptable) |
| Pass 1: picked set with an alternatives strip; add / swap with one key | Yes | Z / X / S / A / Tab, clear Picked vs Alternative | **Pass with fixes** P1-2, P1-3, P1-5, P1-6, P1-7 |
| Pass 2: Not sure + Set aside, fast, "already kept a similar one: DSC0412" (skip / swap / add both) | Covered-by for every non-delivered frame, not refreshed after adds | 1,488 one at a time; stale covered-by after adds | **Fail**: P0-1, P0-3 |
| Never clobber the user's flags | User flags lock the choice; apply skips user flags | – | **Pass**; but Apply writes rejects silently (P0-2) |

## Proposed keymap (Pick the best N overlay)

| Action | Today | Proposed | Note |
|---|---|---|---|
| Open Pick the best N (Cull step) | – | **B** | P1-9; B is free |
| Previous / next photo | Left / Right | same | |
| Next / previous moment | – | **Shift+Right / Shift+Left** (Review); Shift+Space skip rest of moment (Second look) | P1-1, P0-1 |
| Next / previous unreviewed | – | **] / [** | P1-2 |
| First / last | – | Home / End | |
| Keep (and next) | Z, P | Z, P, **Enter** (Review) | P1-6 |
| Set aside | X ("Reject") | X, label "Set aside"; Second look: X = No, set aside and next | P1-5, P2-5 |
| Cycle alternatives | Tab / Shift+Tab, Up / Down | same | |
| Swap | S, Enter | **S only** | P1-6 |
| Add too / Add both | A | same | |
| Skip (Second look) | Space, N | Space, N, **Enter** | |
| Zoom Fit ↔ 100% | – | **Space** (Review), click (both passes) | P1-7 |
| Stars / labels | – | 0–5, 6–9 | P2-6 |
| People: yes / no | Y, Enter / N | same | |
| Undo | Cmd+Z (not in People) | Cmd+Z, navigates to the photo; People included | P1-3, P2-3 |
| Back to grid | Esc | same | |

One-hand check: the hot keys are all on the left hand (Z keep, X set aside, S swap, A add, Tab cycle, Space skip). Right-hand arrows navigate. With Enter = Keep and Space = zoom (Review), the right hand alone can also run pass 1 (arrows, Enter, Up / Down).
There are no conflicts inside a stage: N = Skip only in Second look and No only in People, and Enter means "accept what is shown" in every stage.

## Needs no change
- Setup form: count + shoot type, the remembered target per shoot type, Enter runs, progress with cancel and "previous selection is kept".
- The couple confirmation card, and the Y / N answer flow with auto-advance to the next open card.
- Picked (green) vs Alternative (sky) colour coding and the per-reason captions. They match the engine texts in `selection.rs`.
- The keeper offer after apply (except P2-8). The cheat sheet groups "Pick the best N" first while the overlay is open, and the Help entry is reachable from the overlay header.
