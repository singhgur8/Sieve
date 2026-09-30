# UX Review 1 — 2026-09-29

Reviewer: ux-designer (run as general-purpose with `.claude/agents/ux-designer.md`). Scope: full UI on branch
`worktree-agent-a14626820cb8eecdf` (Library, loupe, compare, Develop, Export, Scenes/Match) against the mock backend.
Screenshots: `test-data/ux-review/shots/` (gitignored; `1280-*`, `1728-*`). Component paths are under `src/`.

Measurements: Library chrome 216 px above the grid at 1280×800 (27%), top bar wraps to 2 rows; 304 px (38%) after
Detect scenes. Develop viewer 459 px tall (57%). Escape in Copy Settings closed Develop; X in that dialog rejected the
photo behind it; Escape does not close the Match panel; Home does nothing in the grid; Export's disabled reason is
below the fold at 800 px.

## P0 — blocks or confuses the workflow
- **P0-1 Keys leak through dialogs; Esc in a Develop dialog exits Develop.** (`App.tsx` handler, `develop/FieldsDialog.tsx`,
  `scenes/MatchPanel.tsx`) Spec: `modalOpen` counter in App via `useModal()` (inc on mount, dec on unmount); global
  handler returns early when > 0. Every modal: `role="dialog" aria-modal`, focus first control on mount, trap Tab;
  topmost modal only handles Esc (cancel) and Enter (confirm if enabled) via capture-phase window listener +
  stopPropagation.
- **P0-2 Apply suggestions can overwrite a shoot's manual culling in one click, no confirm/undo; suggestions never
  shown.** Spec: confirm dialog "Apply suggestions to N photos" with outcome counts; checkbox "Skip photos I already
  flagged or rated" (default on); undo bar restoring prior pick/rating per photo; loupe info shows
  "Suggested: Reject · 2★" when it differs. Architect: `onlyUnset` option.
- **P0-3 Export disabled with no visible reason.** (`export/ExportDialog.tsx`) Spec: Destination section first; amber
  footer reason when disabled ("Choose a destination folder" / "Fix the file name template" / "Invalid subfolder" /
  "Check the size fields"), click scrolls to and focuses the field; pre-fill last folder (session; persisted needs
  architect `lastExportFolder`).

## P1 — friction / polish
- **P1-1 Chrome dominates.** One 44 px top bar ≥ 1280 px: left Import, Shoot, Analyze split-button (menu: Re-analyze
  all, Auto-analyze); right XMP status pill, Save metadata, Export, `⋯` (Read from file, Auto-sync XMP, Apply
  suggestions). Filter bar full only in Grid; elsewhere a 28 px summary ("Filtered: … · 152 of 2000 · Edit filters"),
  Cmd+F toggles. Size/Sort/Auto-advance in Grid only; in Develop the module switcher merges into the Develop toolbar.
  Scene strip hidden until scenes exist ("Detect scenes" in Analyze menu), then one 32 px non-wrapping scrollable row
  with a "Scene ▾" menu for the 7 actions and Match scene as primary. Targets: Develop viewer ≥ 70% height at 1280×800
  (≥ 560 px); Library chrome ≤ 150 px with scenes.
- **P1-2 Messages shift layout and persist.** Bottom-center toasts above filmstrip (≤ 480 px, 4 s; 8 s with action;
  errors sticky). Export job cards bottom-right; finished job collapses after 8 s to a 28 px pill
  ("Export done · 40 · Reveal"); progress ring on the Export button. Architect: `reveal_in_finder(path)`.
- **P1-3 No undo for culling.** Frontend culling undo stack (≤ 100 entries: ids + previous pick/rating/label);
  Cmd+Z / Cmd+Shift+Z outside Develop with a toast; Develop keeps adjustment undo.
- **P1-4 Burst keeper can't be changed.** Architect: `set_burst_keeper(groupId, imageId)`. K in Compare (focused frame
  → keeper + Pick), Shift+K in Grid/Loupe; Keeper badge in Compare overlay.
- **P1-5 Develop hides flags/stars.** Flag, stars, label dot next to filename in Develop toolbar; filmstrip items show
  flag/X top-left and n★ bottom-right; rejects dimmed 50%.
- **P1-6 Copy/Paste/Sync only in Develop.** Lift copied-settings clipboard to App so Cmd+Shift+V pastes to the Grid
  selection; disabled Sync tooltip "Cmd/Shift-click other photos in the filmstrip to sync to them"; Cmd+Shift+S = Sync…;
  Cmd+Shift+B = select active photo's burst.
- **P1-7 Match panel gaps.** Anchor thumbnails (64 px) + filenames in header; exclude rejected by default with toggle
  "Include rejected (n)"; Esc closes; rename "Copy from anchor: 16 groups" → "Also copy from anchor: All settings ▾".
- **P1-8 XMP save state unclear.** Amber pill "50 photos not saved to XMP" only when dirty; click saves all dirty.
  Architect: `dirtyOnly` query flag or `write_xmp_all_dirty()`.
- **P1-9 First-run / empty states.** Empty catalog: "Import a shoot folder to start" + Import button (Cmd+Shift+I) +
  supported formats; empty filter result: add "Clear filters" button.
- **P1-10 Shortcut discoverability.** `?` / Cmd+/ opens a cheat sheet generated from the keyboard map; shortcut hints in
  tooltips (Before `\`, Split Y, Reset Cmd+Shift+R).

## P2 — nice to have
1. Home/End/PageUp/PageDown in Grid (Shift extends). 2. Caps Lock = auto-advance. 3. I cycles loupe info overlay
(full / filename / hidden). 4. Develop: Y split, Cmd+Shift+R reset, E → Loupe; confirm preset delete inline.
5. History labels with values ("Exposure +0.05"). 6. Gradient tracks on Color Mixer hue sliders. 7. Filter bar:
"Match any/all" labels, 5-star "at least" row, capitalized Shoot options. 8. Small thumbs: hide scene badge when
filtering by that scene, ≤ 2 tags + "+n", min 10 px font. 9. Contrast: replace `text-neutral-600/500` helper text
with `text-neutral-400` (WCAG AA). 10. Scene chip tooltip with time range + first-frame thumb. 11. Export scope
defaults to "All filtered" when selection ≤ 1; Cmd+Enter exports. 12. Loupe hover toolbar (Pick/Reject/stars).

## Keyboard map (proposed; conflicts resolved)
| Key | Context | Action |
|---|---|---|
| P / X / U (Shift = advance) | all | pick / reject / unflag |
| Caps Lock | all | auto-advance while on |
| 0–5 / 6–9 | all | rating / color label |
| ← → ↑ ↓ | Grid/Loupe/Develop; Compare steps focused pane | navigate |
| Home / End / PgUp / PgDn | Grid | first / last / page |
| Space / Enter / E | Grid → Loupe; Loupe/Compare → Grid | toggle loupe |
| E | Develop | → Loupe |
| G / Esc | outside Grid (never with a dialog open) | → Grid |
| Esc | Grid | clear selection |
| D / C / Tab | — | Develop / Compare / switch compare pane |
| Z | Grid, Loupe / Develop | loupe 1:1 / 100% |
| F / Shift+F | Loupe, Compare | cycle face zoom (deliberate break from LR full-screen) |
| I | Loupe, Compare | cycle info overlay |
| K / Shift+K | Compare / Grid, Loupe | set burst keeper |
| Shift+A | all | toggle scene anchor |
| `\` / Y | Develop | before-after / split |
| Cmd+Z / Cmd+Shift+Z | Develop: adjustments; else culling | undo / redo |
| Cmd+Shift+C / V | Develop / Develop + Grid | copy / paste settings |
| Cmd+Shift+S / R / B | Develop / Develop / all | sync… / reset / select burst |
| Cmd+A / Cmd+D | Grid | select all / none |
| Cmd+S / Cmd+Shift+E / Cmd+Shift+I | all | save XMP / export / import |
| Cmd+F | Library | show/hide filter bar |
| ? or Cmd+/ | all | cheat sheet |
| Enter / Esc | topmost dialog | confirm / cancel |

## No change needed
Grid cells (letterboxed thumbs, badges, active vs selected, dimmed rejects); tag filter chips and "n of total · Clear";
F face zoom; Compare with shared zoom + focus border; Develop panel (histogram, sections, per-section reset,
double-click reset, history, split); Fields dialog layout; Export dialog contents (only order/reason/scope default
change); Match panel core (Before vs Predicted cards, converged badges, local strength slider, Deselect not converged,
Undo bar).

## Needs real backend to judge
Import/analysis speed and progress smoothness; thumbnail load during fast scroll; real render latency and slider feel;
face detection order/zoom; scene detection quality; apply_suggestions overwrite semantics; XMP errors on read-only
drives; native folder picker and export destinations; empty-catalog first run.

## Needs architect
`apply_suggestions` `onlyUnset`; persisted `lastExportFolder`; `reveal_in_finder(path)`; `set_burst_keeper(groupId,
imageId)`; save-all-dirty XMP (`dirtyOnly` query flag or `write_xmp_all_dirty`).
