# Phase 8c UX review: culling clarity, Lightroom one-hand keys, Help

Author: ux-designer · 2026-10-03 · branch `phase-8c-feedback` @ 1bb6ae9 (read-only review)
For: frontend-dev (all items), architect (items marked **[ARCH]**)

Evidence: mock backend (`vite --port 1461`, `/?mock=201&keepers=not_rejected&meta=1`, plus `&autosync=1`; `window.__mockActivity`), 1280×800
and 1440×900. Throwaway Playwright drivers and screenshots are in the session scratchpad (`s1..s6.cjs`, `shots/{1280,1440}-NN-*.png`), not in the repo.
Screenshot names below refer to that set.

**Summary: P0 0 · P1 9 · P2 17.** The user's complaints are mostly fixed. Z/X/U and Space now work one-handed, the
keeper formula and the "Showing N of M" readout make the counts clear, the Rejected view shows who rejected each photo
and why, and Help is good. What is left is mostly consistency: one dead-end prompt (Apply suggestions), three
progress displays for one export, a dimmed reason strip, the word "keeper" meaning two things, and Help copy that has
fallen behind the UI.

---

## Chrome height (Library, Cull step, grid mode)

| Window | Top bar | Filter bar | Toolbar | Cull summary | Metadata row | Grid starts at | Chrome share |
|---|---|---|---|---|---|---|---|
| 1280×800 | 44 | 32 | 32 | 29 | – | 137 px | 17 % |
| 1280×800, Metadata on | 44 | 32 | 32 | 29 | 144 | 281 px | 35 % |
| 1440×900 | 44 | 32 | 32 | 29 | – | 137 px | 15 % |
| 1440×900, Metadata on | 44 | 32 | 32 | 29 | 144 | 281 px | 31 % |

Default chrome is acceptable: the summary bar is one row and the readout sits in the toolbar. The Metadata row is a toggle, like
Lightroom's Library Filter, so its height is fine once the columns fit (P1-7, P2-11). After Apply suggestions the summary wraps
to two rows at 1280 (161 px, `1280-08-after-apply.png`; see P2-10).

## Workflow verdicts

| Workflow | Steps today | Verdict |
|---|---|---|
| Flag one-handed | Z / X / U, Shift = flag and advance, 0–5, Caps Lock | Good. Verified in Grid, Loupe and Develop (Z picks in Develop, crop keeps X). |
| Zoom | Space: Grid → Loupe; Loupe / Compare / Develop: Fit ↔ 1:1, never leaves the view | Good. Verified: mode stays `loupe` / `develop` after 1–2 presses; Esc / G / E / Enter go back. |
| Arrows with 2 filters | Left/Right in Loupe | Positions advance 2..13 of 14 one per press with no skips; Shift+Z on a frame that leaves the filter lands on the next frame. A flash cannot be judged from stills; relies on `nav-glitch.spec.ts`. Confirm on the Mac with real previews. |
| Understand counts | Readout `Showing 7 of 101 photos · 0 selected`; summary `Keepers 94 = 8 picked + 86 unflagged · 7 rejected are left out` | Good. Every number filters the grid, except "by you / auto" (P1-6). |
| Review rejects | 1 click `Rejected 7` → cells read `Auto-rejected · Motion blur` / `Rejected by you` | Works. The strip is dimmed with the photo (P1-3) and hidden below 150 px thumbnails (P2-8). |
| Apply suggestions | Summary `Sieve suggests … Apply…` or More ▸ Apply suggestions… → dialog with counts → toast [Undo] / Cmd+Z | The explanation in place is good. **Dead end after the first run** (P1-1). |
| Keepers → Edit / Export | `Continue to Edit · 94 keepers`; Plan and Export repeat the formula + `Change keeper rule` | Good. The default rule shows first in the menu and has a "What are keepers?" link. |
| Metadata filters | Filter bar `Metadata` → 10 columns with counts; chips when the row is closed | Works, and the counts follow the other filters. Edited / Sidecar columns are off-screen (P1-7). |
| Background activity | Bottom-right corner rows with ring, count and bar; errors stay | Clear. It duplicates and overlaps the export card (P1-2). |
| Help | Help button, F1, `?` links on Auto-advance, Apply, keeper menu | Good structure, and search works. Some copy is stale (P1-8, P1-9). On a Mac keyboard F1 needs Fn (P2-1). |

---

## P0

None.

---

## P1

### P1-1 "Sieve suggests N picks …" stays after Apply and opens a dialog that can only say "Apply to 0" **[ARCH, small]**
- **Where**: `CullSummaryBar.tsx` (`cull-sum-suggest`), `ApplySuggestionsDialog.tsx`; backend `repo.rs` cull summary (`suggested_pick_pending` / `suggested_reject_pending`).
- **Evidence**: before Apply the summary says `22 picks and 8 rejects`, but the dialog then reports `71 will be updated: 20 picked, 8 rejected`.
  After Apply the summary still shows `Sieve suggests 2 picks and 0 rejects you have not acted on. Apply…`. Clicking it opens
  `0 will be updated … Apply to 0` with the button disabled (`1280-08-after-apply.png`, `1280-50-apply-again.png`).
- **Why**: a first-time user follows the prompt and hits a disabled button. The two numbers disagree because the summary counts unflagged
  photos, while Apply's default (`onlyUnset`) also skips photos that are rated.
- **Fix**:
  1. Contract (architect): the pending counts use the same rule as `apply_suggestions(onlyUnset=true)`: analysed, `pick = unflagged` **and** `rating = 0`,
     with a suggestion that would change something. Rename the doc comment to "what Apply suggestions would change with its default settings". Until that lands, the frontend
     hides the button whenever the dialog's default count would be 0 (cheap check: the summary already refreshes after apply).
  2. Copy drops zero parts and stays on one line: `Suggestions: 20 picks · 8 rejects — Apply…` (`20 picks — Apply…` when there are no rejects). Tooltip unchanged.
  3. Acceptance: after one Apply with defaults, `cull-sum-suggest` is gone. With the button visible, the dialog's `apply-count-apply` equals picks + rejects (+ rated-only) shown on it.

### P1-2 Export progress shows three times and the activity rows overlap the export card
- **Where**: `ActivityWidget.tsx` (`fixed bottom-3 right-3`), `export/ExportJobsPanel.tsx` (`fixed bottom-24 right-3`), TopBar step pill ring.
- **Evidence**: `1280-40-export-running.png`: the step pill reads `Exporting 10%`, the card reads `9/94 · 1 failed`, and the activity row reads `Exporting 94 photos 8 / 94`. With one more
  activity (XMP save) the widget's top edge (y≈696) already sits under the card (bottom y≈704). With three rows the card covers the widget. Analysis and import have the
  same duplication in principle: the top `AnalysisBar` / `ImportBar` plus a corner row from the backend's activity events.
- **Why**: duplicated progress looks like two separate jobs, and an overlap hides the Cancel button or the error text.
- **Fix**: one bottom-right stack. Render `ExportJobsPanel`'s cards inside the ActivityWidget container, as the first children of the same `flex-col gap-2`, and
  drop its own `fixed` positioning. In `ActivityWidget`, skip activities of kind `export` while an export job card is listed, and kinds `analysis` / `import` while
  their top bars are visible (they carry Cancel and failure counts). Keep corner rows for `xmp_save`, `paste_sync`, `apply_scene` and `model_download`, and for analysis / import
  when their bar is not shown. Acceptance: with an export plus two other activities running, no two bottom-right elements' bounding boxes intersect, and only one
  element contains `/94`.

### P1-3 The reject reason strip is dimmed with the photo (≈4.3:1 at 10 px)
- **Where**: `Cell.tsx`: `opacity-50` on the whole cell root when `pick === "reject"`.
- **Evidence**: `1280-04-rejected-cell.png`, `1280-05-rejected-view.png`, `1280-09-auto-rejects.png`. `Rejected by you` / `Auto-rejected · Motion blur` render at half
  opacity: estimated #847676 on #240a0a ≈ 4.3:1, below AA (4.5:1) for 10 px text. The flag X, badges and stars are dimmed too.
- **Why**: this strip answers the user's "why is it rejected" complaint, and it is hardest to read exactly in the view built for reading it.
- **Fix**: move the dimming to the image only. `Thumb`'s `<img>` (and the failed / loading placeholders) get `opacity-40` when rejected; overlays (flag, badges, reason
  strip, tags, stars) stay at full opacity. Keep the selection ring and outline at full opacity too. Acceptance: computed opacity of
  `reject-reason-<id>` and its ancestors is 1. Text contrast ≥ 7:1 (red-100 on red-950/85).

### P1-4 Cell "Suggested: Motion blur" does not say what is suggested
- **Where**: `lib/cull.ts` `suggestedReject()` → `Cell.tsx` `suggested-reason-<id>` (blue strip).
- **Evidence**: `1280-01-cull.png`: `Suggested: Low overall quality`, `Suggested: Motion blur`, on a calm blue strip. The Loupe says `Suggested: Reject · 0★: Low overall quality`
  (`1280-20-loupe.png`).
- **Why**: a first-time user reads "Suggested: Motion blur" as a tag, or even as praise. They cannot tell that Sieve proposes a reject and has not applied it.
- **Fix**: copy `Sieve suggests reject · Motion blur` (bold `Sieve suggests reject`, like `Auto-rejected`). Use a dashed top border (`border-t border-dashed border-red-400/60`) on the
  existing sky strip so it reads as "proposed, not done". The tooltip stays as it is. Same words in the Loupe `suggested-line` for the reject case:
  `Sieve suggests reject: Low overall quality` (keep the `· N★` part only when a rating is suggested).

### P1-5 "Keeper" means two different things
- **Where**: user-facing strings: `Cell.tsx` `BurstBadge` (`Burst of 5 — this is the keeper` / `not the keeper`), `LoupeLayer.tsx` badge `Keeper` + info line `burst #5 keeper`,
  `helpContent.tsx` legend `Burst keeper` / `"Collapse bursts" … shows only keepers` / culling list "K makes the active frame the burst keeper", `keymap.ts` labels for `keeper` / `keeperSet`, cheat sheet.
- **Evidence**: in the Cull step the summary says `Keepers 94 = 8 picked + 86 unflagged`, while a frame in a burst says "not the keeper" and still counts in those 94.
  Help's Keepers entry defines keepers as "photos that move on to Edit".
- **Why**: the user explicitly asked what keepers are. Two meanings of the same word on one screen bring the confusion back.
- **Fix**: keep "keepers" for the Edit/Export set only. Rename the burst concept to **"best of burst"** in UI copy (no IPC rename needed):
  badge titles `Burst of 5 — best frame (Sieve's choice)` / `Burst of 5 — not the best frame. <reason>`; Loupe badge `Best of burst`, info `burst #5 · best`;
  Help legend `Best of burst` and `"Collapse bursts" shows only the best frame of each burst`; keymap `K`: `Make this frame the best of its burst and pick it`, `Shift+K`: `Make the active photo the best of its burst`;
  Help culling list to match. Acceptance: `grep -i "burst keeper\|the keeper\|>Keeper<"` over `src/components` and `src/lib` finds no user-facing strings.

### P1-6 "(7 by you, 8 auto)" look like filters but both show all 15 rejects **[ARCH]**
- **Where**: `CullSummaryBar.tsx` `cull-sum-by-you` / `cull-sum-auto` (both call `toggle("reject")`); `ImageQuery` has no origin field.
- **Evidence**: `1280-09-auto-rejects.png`: clicking `8 auto` → `Showing 15 of 101`.
- **Why**: the rest of the bar keeps the rule "the number you click is the number you get". Reviewing only Auto's rejects is the obvious next step after Apply suggestions.
- **Fix**: contract: `ImageQuery.pickOrigin?: "user" | "auto" | null` (applies with `picks`), also counted by `get_filter_counts`. Frontend: `8 auto` sets `picks:["reject"], pickOrigin:"auto"`,
  shows the button as active, and the readout then says `Showing 8 of 101`. Also shown as a chip `Auto-rejected ×` in the filter bar. Until the contract lands, render
  the split as plain text (not buttons, no hover underline).

### P1-7 Metadata row: Edited and Sidecar columns are off-screen
- **Where**: `MetadataFilterRow.tsx`: 10 columns × `w-40` (160 px) = 1,600 px inside `overflow-x-auto`.
- **Evidence**: `1280-02-meta-row.png` ends at Capture date; `1440-02-meta-row.png` cuts Edited and hides Sidecar. There is no scrollbar or other sign of more columns,
  and a mouse wheel does not scroll sideways.
- **Why**: "edited / unedited" and "has sidecar" are the two filters the user's Lightroom round trip needs most, and they cannot be seen.
- **Fix**: columns `flex-1 basis-0` with `min-w`: File type 80, Camera 136, Lens 160, ISO 72, Focal length 80, Aperture 72, Shutter 80, Capture date 120. Merge Edited and Sidecar
  into one **Status** column of 112 px with four values `Edited n`, `Unedited n`, `Has sidecar n`, `No sidecar n` (two independent groups in one list, divider between). Total minimum width is 1,024 px, so
  everything fits at 1280 with no horizontal scroll. Below that, keep `overflow-x-auto` but add a right-edge fade (`mask-image: linear-gradient(to right, #000 92%, transparent)`).
  Acceptance: at 1280×800, `meta-col-*` right edges are all ≤ the viewport width.

### P1-8 Help copy has fallen behind the UI (Keepers, Apply suggestions)
- **Where**: `lib/helpContent.tsx`.
- **What and exact replacement**:
  - Keepers: "To change the rule, open the Keepers menu in the Edit step (Plan view)." is stale: the rule menu is also in the Cull summary bar and the Export dialog. Replace it with
    "To change the rule, use **Change keeper rule** in the Cull summary bar (under the toolbar), the Keepers menu in the Edit plan, or the Export dialog." List the rules in menu
    order (default first): build the list from the same `DISPLAY_ORDER` as `RuleItems`. Export `DISPLAY_ORDER` from `KeeperRule.tsx`.
  - Apply suggestions, first paragraph: "(top bar, More menu)" → "(the **Sieve suggests … Apply…** button in the Cull summary, or More ▸ Apply suggestions…)".
  - Apply suggestions: "It does not touch labels, edits or files." is wrong when auto-save is on: the new flags and stars are written to the sidecars. Replace with
    "It does not change color labels or edits. Like any flag or rating, the result is written to the XMP sidecars (right away with auto-save on, or when you press Cmd+S)."
  - More ▸ menu note (`TopBar.tsx` `apply-info`): "only on photos you have not touched" → "by default only on photos you have not flagged or rated".
  - `TopBar.tsx` `apply-suggestions` title: "Copy suggested rating and pick to the selection (or all photos when nothing is selected)" → "Fill in flags and stars from Sieve's suggestions for the selection or everything in view. Opens a preview first".

### P1-9 Icon legend and cell hover text use different words, and the legend misses the new strips
- **Where**: `helpContent.tsx` `TAG_TEXT` vs `lib/cull.ts` `TAG_MEANING`. These are two dictionaries for the same tags.
- **Drift found**: missed focus: "Focus missed the face." vs "The subject or face is not sharp". Creative blur: "Blur that looks intentional, such as panning." vs "Intentional blur (panning, bokeh), not
  treated as a defect". Duplicate: "Another frame in the same burst was chosen as the keeper." vs "A better frame of the same burst exists". Overexposed: "Highlights are clipped." vs "Highlights are blown out".
- **Missing from the legend**: the red `Rejected by you` / `Auto-rejected · reason` strip, the suggestion strip from P1-4, `Unreadable` badge, the Loupe saved check (`FileCheck`, "Saved to … next to the original").
- **Fix**: delete `TAG_TEXT` and use `TAG_MEANING` (one source) in the legend. Final wording, sentence case, no period, used in both places:
  blink "Eyes closed"; missed_focus "The face or subject is not sharp"; motion_blur "Blurred by camera or subject movement"; creative_blur "Intentional blur (panning, bokeh). Never rejected on its own";
  underexposed "Too dark"; overexposed "Highlights are blown out"; duplicate_burst "A better frame of the same burst exists" (with P1-5 wording). Add a legend group **"Strips on thumbnails"** with
  two rows: red strip "Rejected, and by whom (you or Auto), with Sieve's reason"; suggestion strip "Sieve suggests rejecting this photo. Nothing has changed until you press X or Apply suggestions".
  Add `Unreadable` and the saved check to "Badges on thumbnails". Drop the CSS `capitalize` on legend labels (P2-16). Acceptance: a unit test asserts that every legend tag text equals `TAG_MEANING[tag]`.

---

## P2 (17)

1. **F1 on a Mac keyboard is brightness** unless Fn is held. Add `Cmd+?` (Cmd+Shift+/) as a second `help` chord, the macOS Help convention, and show `F1 / Cmd+?` in the Help button tooltip and the cheat sheet.
2. `GridToolbar.tsx` auto-advance title says "Shift+P / Shift+X always advance". Change it to "Shift+Z / Shift+X always advance" (build it from `hint`).
3. `App.tsx` `continue-edit` title says "Change the keeper rule in the summary bar **above**". The bar is below the button; use "in the Cull summary bar".
4. `lib/cull.ts` `sidecarName` returns `DSC0001.xmp` for JPEG/TIFF/PNG/HEIC. The real sidecar is `DSC0001.JPG.xmp` (`xmp::sidecar_path`). Mirror the RAW check: RAW extensions → replace, anything else → append `.xmp`.
5. Reason headline on cells uses `reasons[0]` blindly. Skip note kinds (`creative_blur`) when choosing the headline (the mock shows `Auto-rejected · Intentional blur (kept)`). Also make `mockReasons` follow the engine order: defects first, notes last.
6. `Rejected by you: Low overall quality` (Loupe) / `Rejected by you · Duplicate in burst` (cell) reads as if the user gave that reason. Use `Rejected by you · Sieve noted: …` for user rejects and `Auto-rejected · …` for Auto.
7. Long reasons are truncated at 6 columns (`Duplicate in burst (kee…`). At cell size ≥ 200 px allow `line-clamp-2` on the reason strip.
8. Below 150 px thumbnails the reject strip is hidden. Show a 9 px `A` chip next to the red X for Auto rejects, so the origin stays visible. Its title is the same as the strip.
9. Picked / Rejected / Unflagged appear in both the filter bar (`Picked, Rejected, Unflagged`) and the summary bar (`Picked, Unflagged, Rejected`) in a different order. Use one order (Picked, Unflagged, Rejected) in both, or hide the filter-bar flag chips while the Cull summary is visible.
10. Summary bar wraps to two rows at 1280 once the suggestion button is long (`1280-08-after-apply.png`), which shifts the grid by 24 px. Use `flex-nowrap`, and let the formula span truncate (`min-w-0 truncate`, full text in its title). P1-1 also shortens the copy.
11. Metadata row height 144 → 112 px (`h-28`). With P1-7 it still shows five values per column, and the chrome drops from 281 to 249 px at 1280×800.
12. The one-time XMP explainer (`XmpStatus.tsx` `XmpExplainer`) does not say how to see the result in Lightroom. Add a last bullet with `HelpLink id="lightroom"`: "Using Lightroom Classic too? See how it reads these sidecars".
13. Help ▸ Lightroom: add "If Lightroom shows the *metadata changed externally* badge on a thumbnail, click it and choose **Import Settings from Disk**". Change "Catalog Settings" to "Catalog Settings ▸ Metadata".
14. Help ▸ Where your work is saved: the roadmap asks for the path. With a project open, add a line `This project: <root folder path>` with a `Reveal in Finder` button (existing `revealInFinder`).
15. Decorative icons without a title: the funnel in `filter-bar` / `filter-summary`, and the spinner in `analysis-bar`. Add `aria-hidden` (they sit next to text).
16. Help legend labels use CSS `capitalize` ("Pick Flag", "Missed Focus"); the rest of the app uses sentence case. Remove `capitalize`.
17. Cheat sheet: Caps Lock auto-advance is only in Help. Add a row `Caps Lock — Auto-advance while on` under Culling (documented only, `external: true`).

---

## Keyboard map (changes only; everything else stays as in `keymap.ts`)

| Action | Before 8c | Now (verified) | Proposed |
|---|---|---|---|
| Pick | P | Z (P alias), Shift = and advance | keep |
| Reject | X | X, Shift = and advance | keep |
| Unflag | U | U | keep |
| Grid → Loupe | Enter / E / Space | Enter / E / Space | keep |
| Zoom Fit ↔ 1:1 | Z / Space (Space left the view) | Space in Loupe / Compare / Develop; never leaves the view | keep |
| Back to Grid | G / Esc / Enter / E | same | keep |
| Crop: swap orientation | X while cropping | X while cropping (matched before Reject) | keep |
| Help & FAQ | none | F1 | F1 **+ Cmd+?** (P2-1) |
| Cheat sheet | ? / Cmd+/ | ? / Cmd+/ | keep (Cmd+? is free: the cheat-sheet chord requires no modifier for `?`) |
| Burst best frame | K (Compare), Shift+K | same | keep keys, relabel (P1-5) |

No conflicts found. Z is not bound in Develop other than Pick, and Cmd+Z stays undo. K is the mask brush in Develop and "best of burst + pick" only in
Library Compare. Typing in Help search or other inputs never triggers culling keys (`isTypingTarget` covers `type=search`). Dialogs block global keys (`modalCount`).
Z = Pick differs from Lightroom, where Z is zoom. The user asked for it explicitly (decisions.md, 2026-10-03), so keep it.

---

## Per-complaint verdict (first-time user's view)

| User complaint (2026-10-03) | Verdict | Notes |
|---|---|---|
| "I want Z and X … so I can do it faster using one hand" | **Resolved** | Z pick, X reject, Shift = advance, in every mode; cheat sheet and Help say so. Fix the stale tooltip (P2-2). |
| "space should be the zoom in button" / "idk why space bar exits" | **Resolved** | Space opens Loupe from Grid, then toggles Fit ↔ 1:1 and never leaves Loupe / Compare / Develop. |
| Arrow keys flash with two filters on | **Resolved (tests); confirm on Mac** | Positions step one per press with two filters; flash freedom rests on `nav-glitch.spec.ts`; check with real previews. |
| "I would want to keep everything that's not rejected" | **Resolved** | Default rule, shown first in the menu, with the formula in Cull, Plan and Export. P1-5 removes the clash with "burst keeper". |
| Lightroom showed "no data" for Sieve's sidecars | **Resolved in code; confirm on Mac** | xmpDM flags, plus Help steps (Read Metadata from Files, Cmd+S / auto-write) that match Lightroom's menus. P2-12/13 make the steps easier to find. |
| "database is locked" on Save | **Resolved (backend)** | The activity widget now shows one clear error with the count if a catalog error ever happens (`1280-29-activity-error.png`). |
| How many photos am I looking at / where did they go | **Resolved** | `Showing N of M photos · k selected` follows every filter. "by you / auto" is the exception (P1-6). |
| Why is this photo rejected / who rejected it | **Mostly resolved** | Origin + reason on every rejected cell and in the Loupe. Readability (P1-3), the unclear "Suggested" strip (P1-4), small thumbnails (P2-8). |
| What do the icons mean | **Mostly resolved** | Every icon has a title (audit: only decorative ones missing, P2-15); legend in Help. The legend wording drifts from the hover text and misses the new strips (P1-9). |
| What does Auto / Apply suggestions do | **Mostly resolved** | Explained in the dialog, the menu and Help, and undoable. The prompt reaches a dead end after one run (P1-1), and Help says "does not touch files" (P1-8). |
| Lightroom-style metadata filters | **Mostly resolved** | Works with counts and chips. Edited / Sidecar columns cannot be seen at 1280–1440 (P1-7). |
| Nothing shows that work is running | **Resolved, needs tidy-up** | Corner widget for save / paste / apply / export. It duplicates and overlaps the export card (P1-2). |
| Help & FAQ | **Resolved** | Searchable, linked from controls. Copy fixes (P1-8); F1 on a Mac keyboard (P2-1). |

**Needs no changes**: Space / Z / X key handling and the keymap structure; the readout; the keeper rule menu and formula in Cull, Plan and Export;
the Apply suggestions dialog layout and its counts; the Rejected view filter itself; Help panel structure, search and Esc behaviour; the activity widget's look and error state.
