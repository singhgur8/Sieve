# UX Review 2 — 2026-09-30

Reviewer: ux-designer. Build: worktree `agent-a3cd530decf4218c2` (phase-7b parity UI + Lightroom-parity Develop panels + Phase 7c masking), mock backend `/?mock=2000`, Chromium via Playwright at 1280×800 and 1728×1117.
Screenshots: `/Users/gurjotsingh/Documents/GitHub/Sieve/test-data/ux-review-2/shots/` (`1280-*`, `1728-*`; `m` = masks, `c` = culling/dialogs). Throwaway scripts: `s1.cjs`–`s4.cjs` in the same folder. Component paths are relative to `src/`.

Measurements (1280×800): Library chrome 108 px (first cell at y=116), 140 px with the scene strip. Develop viewer 768×603 px (75% of the height; the 560 px target is met), but only 60% of the width: side panels take 224 + 288 px. The Adjust panel is 3640 px tall with every section open. With 6 masks, the Masks panel is 1797 px tall, and the selected mask's first local slider is below the fold.

## Review 1 follow-up (verified in this build)
| Item | Status | Evidence |
|---|---|---|
| P0-1 Keys leak through dialogs / Esc leaves Develop from a dialog | Fixed. X then Esc in Copy Settings: the photo stays unflagged and Develop stays open | `1280-c46-copy-dialog.png`, script log |
| P0-2 Apply suggestions confirm + undo | Fixed (dialog with counts, skip-manual default on, undo toast). **New friction: the scope defaults to a 1-photo selection** (see P1-8) | `1280-c45-apply-suggestions.png` |
| P0-3 Export disabled reason | Fixed ("Choose a destination folder" in the footer; Destination is the first section) | `1280-c48-export.png` |
| P1-1 Chrome budget | Fixed (108/140 px, 603 px viewer). **Regression: the filter-bar count is clipped at 1280 ("2000 of 20")** (P1-7) | `1280-01-grid.png`, `1280-c44-filter-blink.png` |
| P1-2 Toasts / export ring | Fixed (bottom-center toasts, ring on Export) | `1280-c40-compare.png` |
| P1-3 Culling undo | Fixed outside Develop. **Gap: culling done inside Develop cannot be undone there** (P1-4) | script log "Undid: Rate 1 photo 3★" |
| P1-4 Burst keeper | Fixed (K in Compare, Keeper badge, toast) | `1280-c40-compare.png` |
| P1-5 Flags/stars in Develop | Fixed (toolbar and filmstrip) | `1280-10-develop.png` |
| P1-6 Copy/paste/sync reach | Fixed (Cmd+Shift+V in Grid, sync tooltip, Cmd+Shift+B) | code + `1280-10-develop.png` |
| P1-7 Match panel | Fixed (anchor thumbnail, "Include rejected (3)", Esc closes, "Also copy from anchor") | `1280-c51-match.png` |
| P1-8 XMP pill | Fixed | `1280-01-grid.png` |
| P1-9 Empty states | Fixed. The copy is now stale: it doesn't mention JPEG/HEIC import (P2-12) | `1280-60-empty.png` |
| P1-10 Cheat sheet / hints | Fixed; density and ordering need work (P1-11) | `1280-c42-cheatsheet.png` |
| Review 1 P2s | Home/End, I, Y, Cmd+Shift+R, E→Loupe, preset delete confirm, AA contrast (no `text-neutral-500/600` left) and the export scope default are done. **Not done:** Caps Lock auto-advance, history labels with values, hue-gradient Color Mixer tracks | code |

## Workflow verdicts
- **Import:** Import button with an options split (Include JPEG/HEIC/TIFF/PNG, Pair JPEG with RAW). 2 clicks + native picker. The +JPG grid badge is clear. No change needed beyond the P2 copy fixes (the real progress UI needs the real backend).
- **Cull 500 frames:** P/X/U, 0–5, Shift+P advance, Cmd+Z, K keeper in Compare, F face zoom and 3-state tag chips all work in 1 key each. Friction: Apply suggestions acts on the one clicked photo (P1-8). The count is clipped at 1280 (P1-7). The Loupe filmstrip has no flags/stars (P2-4).
- **Edit (Develop):** D, then sliders. Panels match Lightroom's content (Profile browser, Tone Curve point + parametric, Color Grading, Detail, Effects, Calibration, B&W, LUT). Friction:
  - You can't see at a glance which sliders you moved (P1-6).
  - The side panels can't be hidden (P1-5).
  - There is no WB eyedropper (P1-12).
  - Reset and presets silently hit the whole filmstrip selection (P1-9).
  - Culling undo is dead inside Develop (P1-4).
- **Crop (R):** R opens the tool, Enter applies, Esc cancels. Friction:
  - **X rejects the photo instead of swapping the orientation (P0-2).**
  - The crop controls stay scrolled out of view when started with R (P1-3).
  - The default aspect is Free instead of Original.
- **Masking:** Shift+W, then click Subject: 2 steps to an AI mask. Brush K, Linear M, Radial Shift+M, Color Shift+J, Luminance Shift+Q, overlay O / Shift+O and pins H all work, and the handles look right (`1280-m26/27`). Friction:
  - **Esc after a one-shot tool (luminance click, radial drag) throws you out to the Grid (P0-1).**
  - K/M/etc. refuse to work until the panel is open (P1-1).
  - With 3 or more masks, the local sliders sit below the fold (P1-2).
- **Scene matching:** Detect scenes (Analyze ▾), Shift+A anchor, Match scene, Preview, Apply. 5 steps, clear. No change needed.
- **Export:** Cmd+Shift+E, preset, Choose folder, Cmd+Enter. Clear. One risk: "All filtered" silently includes rejects (P1-10).

---

## P0 — blocks or confuses the workflow

### P0-1 Esc in Develop throws the user out to the Grid mid-edit
- **Where:** `App.tsx` `case "escape"`, `develop/DevelopView.tsx` `cancelMaskTool`, `lib/keymap.ts` `escape`. Script log in the review folder ("Esc with no tool → mode grid"); `1280-m29-luminance.png` shows the state just before.
- **What happens:**
  - Luminance-range click, radial drag, linear drag and People all end the tool on their own. Arrow navigation also ends the brush.
  - The photographer's next reflexive Esc ("done with this mask") leaves Develop. That closes the Masks panel and drops the mask selection.
  - The same happens after closing any popover. Lightroom never leaves Develop on Esc.
- **Why it matters:** Masking is iterative (tool, then Esc, then tweak the sliders). Getting kicked to a 2000-photo grid loses your place several times per image.
- **Spec:**
  - In Develop, Esc works as a cascade and **never changes the module**. Handle each step in order and stop at the first one that applies:
    1. Cancel the crop tool.
    2. End the active mask tool.
    3. If the Masks panel is open and a component or group is selected, deselect it (`masks.select(null)`).
    4. If the Masks panel is open, switch to the Adjust tab.
    5. Otherwise do nothing.
  - Leaving Develop stays on G (Grid), E (Loupe) and the "Library" button.
  - Change the keymap `escape`: `modes: ["grid","loupe","compare"]`. Add a new `developEscape` entry in Develop with the label "Cancel tool / deselect mask / close Masks panel".
  - Cheat sheet text for Esc: "Back to Grid (Grid: clear selection)" for the Library modes.

### P0-2 X while cropping rejects the photo; in Develop the reject can't be undone with Cmd+Z
- **Where:** `lib/keymap.ts` (`reject` is active in all modes), `develop/CropPanel.tsx`; `1280-16-crop.png`. Script log: "after X in crop: flags reject".
- **What happens:**
  - Lightroom users press X in the crop tool to swap landscape/portrait. Here it rejects the photo.
  - With Auto-advance on, it also jumps to the next photo and throws away the crop in progress.
  - Cmd+Z in Develop only undoes adjustments (`undoCull` is Library-only), so the user undoes their last slider move instead of the reject.
- **Why it matters:** This silently changes culling results that are then written to XMP and used to pick deliverables.
- **Spec:**
  - While `cropTool != null`, route these keys before the culling keys (new ids `cropSwap`, `cropLock`, active only while cropping):
    - **X:** swap the orientation. Same as the `crop-flip` button. If the aspect is Free, first lock to the current rectangle's ratio, then swap.
    - **A:** toggle the aspect lock (Free ↔ the last locked aspect, default Original).
    - **Shift+X, P, U and 0–5** still cull as normal.
  - Update the cheat sheet row: "X: swap crop orientation (while cropping)".
  - Also fix the undo gap. That's P1-4, but ship it together with this.

---

## P1 — noticeable friction or polish gap

### P1-1 Mask tool shortcuts require opening the panel first
- **Where:** `DevelopView.tsx` `maskKey` (the "Open the Masks panel first (Shift+W)" toast); `1280-m20-masks-empty.png`.
- **Lightroom behaviour:** K, M, Shift+M, Shift+J and Shift+Q open Masking and start the tool in one key.
- **Spec:**
  - Tool keys (`maskBrush`, `maskLinear`, `maskRadial`, `maskColor`, `maskLuminance`) call `masks.setOpen(true)` and then the tool (end the crop tool first if one is active).
  - O / Shift+O / H with the panel closed: open the panel only if the photo has masks; otherwise no-op, no toast.
  - Delete/Backspace with the panel closed: silent no-op.
  - Remove the toast. Update `where` in the keymap to "Develop".

### P1-2 Masks panel: the selected mask's sliders are pushed below the fold
- **Where:** `develop/MasksPanel.tsx`; `1280-m21-subject.png` (Exposure at y≈700 with 1 mask), `1280-m31-masks-scrolled.png` (6 masks).
- **What happens:** The 10-button Create grid (~180 px) and fully expanded group cards push the local sliders off-screen. The user scrolls for every adjustment. The "Luminance Ra…" label is truncated.
- **Spec:**
  - With 0 masks: keep today's 2-column Create grid. Rename the labels "Color Range" → "Color", "Luminance Range" → "Luminance"; the full name goes in the tooltip.
  - With 1 or more masks, replace the grid with one 28 px **icon row**:
    - 10 icon buttons of 24×24, 4 px gap, in `CREATE_ORDER`.
    - Tooltip format: "Brush (K)".
    - Unavailable AI tools are dimmed and the tooltip gives the reason.
    - Header text: "Create" (11 px, uppercase) to the left.
  - Mask list:
    - Container `max-h-[35%] min-h-[96px] overflow-y-auto`. Scroll the selected group into view on selection.
    - A group with 1 component renders as a **single row**: eye, kind icon, group name, `⋯`, and the "update"/"not rendered" pills when relevant.
    - Show component rows only when the group has 2 or more components, or when the group is selected (the Add/Subtract/Intersect row appears only on selection, as today).
  - Local sliders go below the list in their own scroll area (the panel becomes a flex column: fixed header, list, then settings with `flex-1 overflow-y-auto`).
  - **Acceptance:** at 1280×800 with 6 masks, the selected mask's Amount and Exposure sliders are visible without scrolling.

### P1-3 Crop tool: controls hidden when started with R; Lightroom defaults missing
- **Where:** `develop/CropPanel.tsx`, `develop/CropOverlay.tsx`, `DevelopView.tsx` `startCrop`; `1280-16-crop.png`, `1728-16-crop.png` (the panel is scrolled to Tone Curve, so aspect, angle and Done are off-screen). The edge handles also sit on the panel borders.
- **Spec:**
  - While cropping, show a **floating crop bar** at the viewer's bottom center: 36 px tall, `bg-black/70`, rounded, 8 px gap. Contents, left to right:
    - Aspect select.
    - Lock toggle (A).
    - Orientation swap (X).
    - Angle slider (160 px, −45…45, double-click resets).
    - Reset.
    - Cancel (Esc).
    - Done (Enter, primary).
  - Also call `scrollIntoView({block:"nearest"})` on `section-crop`.
  - Default aspect is **Original** (locked), as in Lightroom. Remember the last used aspect in localStorage `sieve.crop.aspect`.
  - In crop mode, inset the image by 24 px on every side so the handles don't sit on the panel borders.
  - Replace the top-left badge with the bar (the badge duplicates it).

### P1-4 One undo in Develop
- **Where:** `lib/keymap.ts` (`undoCull` has `modes: LIB`), `hooks/useCullUndo.ts`, `App.tsx`.
- **Spec:**
  - `useCullUndo` stores `at: number` (ms) per entry. `useEditor` exposes `lastCommitAt` for the active image.
  - In Develop, Cmd+Z undoes whichever is newer: the last culling entry or the last adjustment commit. Cmd+Shift+Z mirrors this for redo.
  - Toast for culling undo: "Undid: Reject DSC00010.ARW".
  - Cheat sheet: a single "Cmd+Z Undo (culling or adjustment, newest first)" row in Develop.

### P1-5 Hide panels (Tab / Shift+Tab) so photos dominate
- **Where:** `DevelopView.tsx` layout, `App.tsx`. Pressing Tab in Develop today just moves focus to the Import button (`1280-c47-develop-tab.png`).
- **Spec (Lightroom convention):**
  - Develop: Tab toggles the left panel (224 px) and the right panel together. Shift+Tab also hides the filmstrip, the filter summary and the Develop toolbar (full-bleed; any key such as Tab restores them).
  - Loupe: Shift+Tab hides the filmstrip and the filter summary.
  - Compare keeps Tab for pane switching.
  - Persist the state per mode in localStorage.
  - Add a 16 px chevron on each panel's inner edge (tooltip "Hide panel (Tab)").
  - Masks and crop must keep working with the right panel hidden, using shortcuts and the floating crop bar.
  - **Acceptance:** at 1280×800 with panels hidden, the viewer is ≥ 1280×603.

### P1-6 Sliders don't show what you changed; values can't be typed
- **Where:** `develop/Slider.tsx` (native range with `accentColor` fills from the minimum, so every bipolar slider at 0 looks half-applied; `1280-10-develop.png`).
- **Spec:**
  - Custom track: 2 px `neutral-700` track. The accent fill runs from the **default value** to the current value (center-origin for bipolar sliders, left-origin for 0-based sliders like Sharpening Amount). Band-colored sliders keep their colour.
  - Thumb: 10 px, white.
  - Value text: `neutral-100 font-medium` when ≠ default, `neutral-400` at default.
  - Section header: a 6 px `sky-400` dot after the title when any field in the section differs from its default (the `Section` `badge` prop already exists).
  - Click the value to edit it inline: text input, Enter commits (one history entry), Esc cancels, Up/Down step, Shift ×10. Clamp to min/max. Temp accepts Kelvin.
  - Keep double-click reset and the arrow-key behaviour.

### P1-7 Filter bar count clipped at 1280 px (regression)
- **Where:** `components/FilterBar.tsx`; `1280-01-grid.png` ("2000 of 20"), `1280-c44-filter-blink.png`.
- **Spec:**
  - Remove the count from the filter bar. The Grid toolbar's right side becomes "152 of 2000 · 1 selected". Drop the duplicate "· 2000 photos".
  - Make the chip row `min-w-0 overflow-x-auto` with no wrap.
  - **Acceptance:** nothing is clipped at 1280 px with every chip visible.

### P1-8 Apply suggestions defaults to the one photo you last clicked
- **Where:** `App.tsx` `askApplySuggestions`, `ApplySuggestionsDialog.tsx`; `1280-c45-apply-suggestions.png` ("Apply suggestions to 1 photo … 0 will be updated").
- **What happens:** Clicking a cell always selects it, so the whole-shoot action almost never targets the shoot.
- **Spec:**
  - Same rule as Export: when the selection has 1 photo or fewer, the scope is all photos in the view.
  - Add scope radios at the top of the dialog: "Selected (n)" / "All in view (n)"; counts recompute on change.
  - Title: "Apply suggestions". Body copy: "Replaces flags and star ratings with Sieve's suggestions. You can undo it afterwards."

### P1-9 Reset and presets silently apply to every selected filmstrip photo
- **Where:** `DevelopView.tsx` `targets()`, `doReset`, `doApplyPreset`.
- **What happens:** After Cmd+Shift+B (select burst), Reset or a preset hits the whole burst. Cmd+Z only restores the active photo.
- **Spec:**
  - When targets > 1:
    - The Reset button label becomes "Reset (5)".
    - Preset rows show a "→ 5" hint on hover.
    - After the action, show a toast "Reset 5 photos" / "Applied 'Warm Film' to 5 photos" with **Undo**. Undo calls `undoAdjustments` for every changed id; reuse the Match Scene undo code in `App.tsx`.
  - With one target: unchanged.

### P1-10 Export "All filtered" includes rejects
- **Where:** `export/ExportDialog.tsx` scope footer; `1280-c48-export.png` (1848 photos including 143 rejects).
- **Spec:**
  - Add a checkbox next to the scope radios: "Skip rejected (143)". Show it only when the scope contains rejects; default on.
  - The count and the button read "Export 1705".
  - Frontend-only: filter ids by `pick !== "reject"`. For ids not loaded, use `listImages` with `picks: ["pick","unflagged"]` on the current query.

### P1-11 Cheat sheet: current context first, tighter layout
- **Where:** `components/CheatSheet.tsx`; `1280-c42-cheatsheet.png`, `1728-c42-cheatsheet.png`.
- **What happens:** ~190 px of dead space between keys and labels. The Masks group is only reachable by scrolling.
- **Spec:**
  - Key column `w-28` (112 px).
  - Layout: CSS columns `columns-2` (below 1400 px wide) / `columns-3` (1400 px and wider), `break-inside-avoid` per group.
  - Groups for the current mode come first: Develop shows Develop, Masks, Culling; Library modes show Culling, Navigate, View.
  - Header subtitle: "Showing Develop first".
  - **Acceptance:** at 1728×1117 in Develop, every Develop and Masks row is visible without scrolling.

### P1-12 No white-balance eyedropper (W) — Architect
- **Where:** Basic section in `develop/AdjustPanel.tsx`.
- **Why:** Mixed venue lighting makes the WB picker the most-used Develop tool for wedding photographers. Lightroom key: W.
- **Architect:** `sample_white_balance(id, point: NormPoint /* sensor frame */, adjustments) -> { temperatureK, tint }`, averaging 5×5 source pixels.
- **UI:**
  - W toggles the picker: crosshair cursor and hint badge "Click a neutral grey or white. Esc cancels".
  - Click sets `whiteBalance: custom` with history label "White Balance: Picker" and ends the picker.
  - Add a picker icon button left of As Shot / Custom (tooltip "White balance picker (W)").

---

## P2 — nice to have
1. **V toggles Black & White** (Lightroom). Same action as the `bw-on`/`bw-off` buttons; history label "Black & White" / "Color".
2. **Develop zoom keys:**
   - Space toggles Fit/100% (Lightroom Develop).
   - F / Shift+F cycles 100% zoom to each detected face (reuse the Loupe face list) so you can check eye sharpness while editing portraits.
3. **Caps Lock auto-advance** (still open from review 1): `e.getModifierState("CapsLock")` ORed with the Auto-advance checkbox; show the checkbox as on with a "Caps Lock" suffix.
4. **Loupe:** show the +JPG badge in the info overlay. Give the Loupe filmstrip the same flag, star and edited markers as the Develop filmstrip (reuse its markup).
5. **History labels with values:** "Exposure +0.35", "Temp 5600 K", "Mask: Subject 1 Exposure +0.50".
6. **Masks:**
   - After undo or delete, if `selGroup` no longer exists, select the last group so Delete keeps working.
   - Color range: show up to 5 sample swatches, each with an ×, instead of "1 of 5 samples" / "Remove last".
7. **Mask effect presets** (Soften Skin, Whiten Teeth, Iris Enhance, Brighten Eyes) as a dropdown above the local sliders. Saving user effects needs storage (Architect: presets table `kind = "local"`).
8. **Crop:** drag outside the rectangle to rotate (rotate cursor). Cmd-drag across the photo draws a straighten line.
9. **Paste from previous** (Lightroom Cmd+Alt+V: all settings except crop and masks from the previously active photo). `matchKey` currently rejects every Alt chord, so it needs an explicit `alt` flag on `Chord`.
10. **Cmd+Shift+N** = Save preset… (Lightroom's "New preset").
11. Right panel 320 px wide when the window is ≥ 1600 px (288 px today even at 1728).
12. **Empty catalog:**
    - Hide the filter bar and the Grid toolbar.
    - Supported-formats copy: "Sony ARW, Fujifilm RAF, Canon CR3 · JPEG, HEIC, TIFF, PNG when enabled in Import ▾".
13. **Warnings chip copy:** the `masks_unsupported` text still says "Masks not supported yet". Since v10 it means "some components can't be rendered". Change it:
    - Short: "Some masks can't be rendered".
    - Long: "{detail} use Lightroom mask types Sieve can't render yet (for example Depth Range). They stay in the sidecar; everything else renders."
    - Action: "Show in Masks panel", which opens the panel; the rows already carry a "not rendered" pill.
    (`develop/WarningsChip.tsx`; `1280-15-warning-popover.png`.)

---

## Keyboard map (existing → proposed; conflicts resolved)
| Key | Context | Existing | Proposed | Note |
|---|---|---|---|---|
| P / X / U (Shift = advance) | all | pick / reject / unflag | same, **except X while cropping** | Lightroom |
| X | Develop, crop tool active | rejects (conflict) | **swap crop orientation** | P0-2, Lightroom |
| A | Develop crop / brush | brush auto-mask | crop: **toggle aspect lock**; brush: auto-mask | mutually exclusive tools |
| 0–5 / 6–9 | all | rating / label | same | |
| Caps Lock | all | — | auto-advance while on | P2-3 |
| ← → / ↑ ↓ | Grid/Loupe/Develop; Compare pane | navigate (ends the mask tool) | same | |
| Home/End/PgUp/PgDn | Grid | jump | same | |
| Space / Enter / E | Grid ↔ Loupe | toggle | same | |
| Space | Develop | — | Fit ↔ 100% | P2-2, Lightroom |
| E / G | Develop | Loupe / Grid | same | |
| D / C | Library | Develop / Compare | same | |
| Esc | Grid / Loupe / Compare | clear selection / → Grid | same | |
| Esc | Develop | cancel crop → end tool → **Grid** | cancel crop → end tool → deselect mask → close Masks → no-op | **P0-1** |
| Tab | Compare | switch pane | same | |
| Tab / Shift+Tab | Develop, Loupe | moves DOM focus | hide side panels / hide all | P1-5, Lightroom |
| Z | Library / Develop | 1:1 / 100% | same | |
| F / Shift+F | Loupe, Compare | face zoom | + Develop (100% on faces) | P2-2 |
| I | Loupe, Compare | info overlay | same | |
| K | Compare | keeper + Pick | same | |
| K | Develop | brush (only with the Masks panel open) | brush, **opens Masks if closed** | P1-1; mode-scoped, no clash with Compare |
| Shift+K | Grid, Loupe | set keeper | same | |
| M / Shift+M / Shift+J / Shift+Q | Develop | linear / radial / color / luminance (panel open only) | same, **open Masks if closed** | P1-1, Lightroom |
| Shift+W | Develop | Masks panel | same | Lightroom |
| O / Shift+O / H | Develop | overlay / overlay style / pins (toast when closed) | same; silent when no masks | Lightroom |
| [ ] / Shift+[ ] | Develop brush | size / feather | same | Lightroom |
| Delete / Backspace | Develop, Masks panel | delete component (toast when closed) | same; silent when closed | |
| R / Enter | Develop | crop / apply | same | |
| W | Develop | — | **WB eyedropper** | P1-12, Architect |
| V | Develop | — | **B&W toggle** | P2-1, Lightroom |
| \ / Y | Develop | before-after / split | same | |
| Shift+A | all | scene anchor | same | |
| Cmd+Z / Cmd+Shift+Z | Library: culling; Develop: adjustments | split stacks | Develop: **newest of culling or adjustment** | P1-4 |
| Cmd+Shift+C / V / S / R | Develop (+V Grid) | copy / paste / sync / reset | same; multi-target reset/preset gets toast + Undo | P1-9 |
| Cmd+Alt+V | Develop | — | paste from previous | P2-9 (needs Alt chords) |
| Cmd+Shift+N | Develop | — | save preset… | P2-10 |
| Cmd+Shift+B | all | select burst | same | |
| Cmd+A / Cmd+D | Grid | select all / none | same | |
| Cmd+F | all | filter bar | same | |
| Cmd+S / Cmd+Shift+E / Cmd+Shift+I | all | save XMP / export / import | same | |
| ? / Cmd+/ | all | cheat sheet | same, context-first | P1-11 |
| Enter / Cmd+Enter / Esc | topmost dialog | confirm / export / cancel | same | |

Conflict check: after the changes, every chord maps to at most one action per mode/tool state.
- K: Compare = keeper; Develop = brush.
- X: cull everywhere except while cropping.
- A: crop tool vs brush tool; they are never active together.
- O: Masks overlay only (crop overlay cycling is intentionally not mapped).
- Tab: Compare = pane switch; Develop/Loupe = panels.

## No change needed
- Profile browser: grouped camera profiles and Looks, "not installed" labels, Amount slider (`1280-11`).
- Tone Curve: RGB/R/G/B point editor with keyboard nudge and delete, parametric regions with split handles.
- Color Grading: wheel with Shift fine-tune and double-click reset, zone dots showing the tint.
- Detail, Effects, Calibration: layout and ranges.
- Section collapse memory and Alt-click solo.
- B&W mode switch in the Color Mixer header.
- LUT section.
- Mask canvas UX: brush cursor ring, tool hint badge, linear/radial handles, pins, overlay colour cycling (`1280-m24/26/27`, `1280-m22`).
- People picker dialog (`1280-m32`).
- The "Masks (n)" tab counter and the per-component Paint/Invert/Delete controls.
- Import options popover and the +JPG grid badge.
- Compare view with the Keeper badge.
- Match panel.
- Export dialog structure.
- Modal layer / focus trap.
- Toasts and the XMP pill.
- Library chrome budget.

## Needs real backend to judge
- Draft-render slider feel on real RAWs.
- AI mask latency and quality, and overlay alignment on rotated or cropped frames.
- Real profile/Look rendering.
- Import/analysis progress.
- Export timings and Reveal in Finder.

## Needs architect
- P1-12: `sample_white_balance(id, point, adjustments)`.
- P2-7: local (mask-effect) presets storage.
- Optional P2: an auto WB / auto tone command for Lightroom's Auto buttons.
