// Single source of truth for keyboard shortcuts: the global handler (App), tooltips (`hint`) and the
// cheat sheet all read this table. Order matters: the first matching definition for the current mode wins.

export type Mode = "grid" | "loupe" | "compare" | "develop";
const ALL: Mode[] = ["grid", "loupe", "compare", "develop"];
const LIB: Mode[] = ["grid", "loupe", "compare"];

export type ActionId =
  | "pick"
  | "reject"
  | "unflag"
  | "rate"
  | "label"
  | "navH"
  | "navV"
  | "gridJump"
  | "toggleLoupe"
  | "gridLoupe"
  | "devToLoupe"
  | "toGrid"
  | "escape"
  | "developEscape"
  | "cropSwap"
  | "cropLock"
  | "cropOverlay"
  | "cropOverlayRotate"
  | "cropReset"
  | "cropStraightenDrag"
  | "cropAutoStraighten"
  | "panelsToggle"
  | "panelsHide"
  | "bwToggle"
  | "faceDevelop"
  | "wbPicker"
  | "pastePrev"
  | "savePreset"
  | "develop"
  | "compare"
  | "compareSwap"
  | "compareMakeSelect"
  | "compareFocus"
  | "scenesToggle"
  | "zoomLoupe"
  | "zoomDevelop"
  | "face"
  | "info"
  | "photoInfo"
  | "captureTime"
  | "keeper"
  | "keeperSet"
  | "anchor"
  | "stepCull"
  | "stepEdit"
  | "stepExport"
  | "nextScene"
  | "prevScene"
  | "applyScene"
  | "planSkip"
  | "planStop"
  | "autoEdit"
  | "before"
  | "split"
  | "crop"
  | "cropCommit"
  | "guided"
  | "undoAdj"
  | "redoAdj"
  | "undoCull"
  | "redoCull"
  | "copy"
  | "copyAll"
  | "pasteAll"
  | "paste"
  | "sync"
  | "syncQuiet"
  | "autoSync"
  | "autoTone"
  | "autoWb"
  | "reset"
  | "maskPanel"
  | "maskBrush"
  | "maskLinear"
  | "maskRadial"
  | "maskColor"
  | "maskLuminance"
  | "maskOverlay"
  | "maskOverlayStyle"
  | "maskPins"
  | "maskSize"
  | "maskFeather"
  | "maskAuto"
  | "maskDelete"
  | "maskMoveUp"
  | "maskMoveDown"
  | "selectBurst"
  | "selectAll"
  | "selectNone"
  | "saveXmp"
  | "export"
  | "import"
  | "filterBar"
  | "cheatSheet"
  | "help"
  | "capsAdvance"
  | "dialogConfirm"
  | "dialogCancel";

export interface Chord {
  /** `KeyboardEvent.key` (case-insensitive for letters). */
  key: string;
  /** Cmd (mac) / Ctrl. */
  mod?: boolean;
  /** true = required, false/undefined = must not be held, "any" = ignored. */
  shift?: boolean | "any";
  /** Alt / Option: true = required, otherwise it must not be held. */
  alt?: boolean;
}

export interface KeyDef {
  id: ActionId;
  group: string;
  label: string;
  chords: Chord[];
  /** Modes where the chord is active. */
  modes: Mode[];
  /** Human readable keys for the cheat sheet / tooltips (first entry is used by `hint`). */
  display: string[];
  /** Where it applies, as shown in the cheat sheet. */
  where: string;
  /** Handled elsewhere (modal system); documented only. */
  external?: boolean;
  /** Only active while the crop tool is open (matched before the culling keys, so X / A do not cull or auto-mask). */
  needs?: "crop" | "compare";
  /** Not listed in the cheat sheet (a per-mode variant of a listed row). */
  hidden?: boolean;
}

/** Tool state the matcher needs to pick between chords that share a key. */
export interface KeyContext {
  cropping?: boolean;
  /** A Compare pair is open (Library Compare, or Compare inside Develop). */
  comparing?: boolean;
}

const c = (key: string, o: Omit<Chord, "key"> = {}): Chord => ({ key, ...o });
const digits = (from: number, to: number) => Array.from({ length: to - from + 1 }, (_, i) => c(String(from + i)));

export const KEYMAP: KeyDef[] = [
  // ---- crop tool (must come before the culling keys: X swaps the orientation, A locks the aspect, only while cropping) ----
  { id: "cropSwap", group: "Develop", label: "Swap crop orientation (landscape / portrait)", chords: [c("x")], modes: ["develop"], display: ["X"], where: "While cropping", needs: "crop" },
  { id: "cropOverlay", group: "Develop", label: "Cycle the crop guide overlay (thirds, grid, golden ratio / spiral, diagonal, triangle, aspect ratios)", chords: [c("o")], modes: ["develop"], display: ["O"], where: "While cropping", needs: "crop" },
  { id: "cropOverlayRotate", group: "Develop", label: "Rotate the crop guide overlay", chords: [c("o", { shift: true })], modes: ["develop"], display: ["Shift+O"], where: "While cropping", needs: "crop" },
  { id: "cropReset", group: "Develop", label: "Reset the crop", chords: [c("r", { mod: true, alt: true })], modes: ["develop"], display: ["Cmd+Alt+R"], where: "Develop (resets only the crop, one history entry)" },
  { id: "cropStraightenDrag", group: "Develop", label: "Draw a straighten line on the photo", chords: [], modes: ["develop"], display: ["Cmd-drag"], where: "While cropping", needs: "crop" },
  { id: "cropAutoStraighten", group: "Develop", label: "Auto straighten (level the horizon)", chords: [], modes: ["develop"], display: ["Shift+double-click Angle"], where: "While cropping", needs: "crop" },
  { id: "cropLock", group: "Develop", label: "Lock / unlock the crop aspect ratio", chords: [c("a")], modes: ["develop"], display: ["A"], where: "While cropping", needs: "crop" },

  // ---- culling ----
  { id: "pick", group: "Culling", label: "Pick (Shift = and advance; P is an alias)", chords: [c("z", { shift: "any" }), c("p", { shift: "any" })], modes: ALL, display: ["Z", "P"], where: "Everywhere" },
  { id: "reject", group: "Culling", label: "Reject (Shift = and advance)", chords: [c("x", { shift: "any" })], modes: ALL, display: ["X"], where: "Everywhere" },
  { id: "unflag", group: "Culling", label: "Unflag", chords: [c("u")], modes: ALL, display: ["U"], where: "Everywhere" },
  { id: "rate", group: "Culling", label: "Star rating", chords: digits(0, 5), modes: ALL, display: ["0-5"], where: "Everywhere" },
  { id: "label", group: "Culling", label: "Color label (red, yellow, green, blue)", chords: digits(6, 9), modes: ALL, display: ["6-9"], where: "Everywhere" },
  { id: "keeper", group: "Culling", label: "Make this frame the best of its burst and Pick it", chords: [c("k")], modes: ["compare"], display: ["K"], where: "Compare" },
  { id: "keeperSet", group: "Culling", label: "Make the active photo the best of its burst", chords: [c("k", { shift: true })], modes: ALL, display: ["Shift+K"], where: "Everywhere" },
  { id: "undoCull", group: "Culling", label: "Undo culling change", chords: [c("z", { mod: true })], modes: LIB, display: ["Cmd+Z"], where: "Outside Develop" },
  { id: "redoCull", group: "Culling", label: "Redo culling change", chords: [c("z", { mod: true, shift: true })], modes: LIB, display: ["Cmd+Shift+Z"], where: "Outside Develop" },
  { id: "anchor", group: "Scenes", label: "Toggle scene anchor / make representative", chords: [c("a", { shift: true })], modes: ALL, display: ["Shift+A"], where: "Edit step: make representative; elsewhere: toggle anchor" },
  { id: "selectBurst", group: "Scenes", label: "Select the active photo's burst", chords: [c("b", { mod: true, shift: true })], modes: ALL, display: ["Cmd+Shift+B"], where: "Everywhere" },

  // ---- workflow (steps of a project) ----
  { id: "stepCull", group: "Workflow", label: "Step 1: Cull", chords: [c("1", { mod: true, alt: true })], modes: ALL, display: ["Cmd+Alt+1"], where: "Project open" },
  { id: "stepEdit", group: "Workflow", label: "Step 2: Edit (again: Plan)", chords: [c("2", { mod: true, alt: true })], modes: ALL, display: ["Cmd+Alt+2"], where: "Project open" },
  { id: "stepExport", group: "Workflow", label: "Step 3: Export keepers", chords: [c("3", { mod: true, alt: true })], modes: ALL, display: ["Cmd+Alt+3"], where: "Project open" },
  { id: "nextScene", group: "Workflow", label: "Next scene to edit (or next frame to review)", chords: [c("n")], modes: ["grid", "develop"], display: ["N"], where: "Edit step: Plan, Develop" },
  { id: "prevScene", group: "Workflow", label: "Previous scene", chords: [c("n", { shift: true })], modes: ["grid", "develop"], display: ["Shift+N"], where: "Edit step: Plan, Develop" },
  { id: "applyScene", group: "Workflow", label: "Apply to scene", chords: [c("Enter", { mod: true, shift: true })], modes: ["grid", "develop"], display: ["Cmd+Shift+Enter"], where: "Edit step: Plan, Develop" },
  { id: "planSkip", group: "Workflow", label: "Skip / include the focused scene", chords: [], modes: ALL, display: ["S"], where: "Edit step: Plan", external: true },
  { id: "capsAdvance", group: "Culling", label: "Auto-advance while Caps Lock is on", chords: [], modes: ALL, display: ["Caps Lock"], where: "Everywhere", external: true },
  { id: "planStop", group: "Workflow", label: "Stop applying", chords: [], modes: ALL, display: ["Esc"], where: "Edit step: Plan (while applying)", external: true },
  { id: "autoEdit", group: "Workflow", label: "Auto edit (my style)", chords: [c("u", { mod: true, alt: true })], modes: ["grid", "develop"], display: ["Cmd+Alt+U"], where: "Edit step: Plan, Develop" },

  // ---- navigation ----
  { id: "navH", group: "Navigate", label: "Previous / next photo (Compare: the candidate; Grid: Shift extends)", chords: [c("ArrowLeft", { shift: "any" }), c("ArrowRight", { shift: "any" })], modes: ALL, display: ["Left / Right"], where: "Everywhere" },
  { id: "navV", group: "Navigate", label: "Row up / down", chords: [c("ArrowUp", { shift: "any" }), c("ArrowDown", { shift: "any" })], modes: ["grid"], display: ["Up / Down"], where: "Grid" },
  { id: "gridJump", group: "Navigate", label: "First / last / page up / page down (Shift extends)", chords: [c("Home", { shift: "any" }), c("End", { shift: "any" }), c("PageUp", { shift: "any" }), c("PageDown", { shift: "any" })], modes: ["grid"], display: ["Home / End / PgUp / PgDn"], where: "Grid" },
  { id: "toggleLoupe", group: "Navigate", label: "Grid to Loupe and back", chords: [c("Enter"), c("e")], modes: LIB, display: ["Enter", "E"], where: "Grid, Loupe, Compare" },
  { id: "gridLoupe", group: "Navigate", label: "Open the photo in Loupe", chords: [c(" ")], modes: ["grid"], display: ["Space"], where: "Grid" },
  { id: "devToLoupe", group: "Navigate", label: "Open in Loupe", chords: [c("e")], modes: ["develop"], display: ["E"], where: "Develop" },
  { id: "toGrid", group: "Navigate", label: "Back to Grid", chords: [c("g")], modes: ["loupe", "compare", "develop"], display: ["G"], where: "Outside Grid" },
  { id: "escape", group: "Navigate", label: "Back to Grid (Grid: clear selection)", chords: [c("Escape")], modes: LIB, display: ["Esc"], where: "Grid, Loupe, Compare (never with a dialog open)" },
  { id: "developEscape", group: "Navigate", label: "Cancel tool / deselect mask / close Masks panel (never leaves Develop)", chords: [c("Escape")], modes: ["develop"], display: ["Esc"], where: "Develop" },
  { id: "panelsToggle", group: "Navigate", label: "Hide / show the side panels", chords: [c("Tab")], modes: ["develop"], display: ["Tab"], where: "Develop" },
  { id: "panelsHide", group: "Navigate", label: "Hide / show all panels, filmstrip and toolbar", chords: [c("Tab", { shift: true })], modes: ["develop", "loupe"], display: ["Shift+Tab"], where: "Develop, Loupe" },
  { id: "develop", group: "Navigate", label: "Develop", chords: [c("d")], modes: LIB, display: ["D"], where: "Grid, Loupe, Compare" },
  { id: "compare", group: "Navigate", label: "Compare view: two photos side by side (again: leave it; in Library it returns to Loupe)", chords: [c("c")], modes: ALL, display: ["C"], where: "Library and Develop" },
  { id: "compareSwap", group: "Navigate", label: "Swap Select and Candidate", chords: [c("ArrowDown")], modes: ["compare", "develop"], display: ["Down"], where: "Compare (Library and Develop)", needs: "compare" },
  { id: "compareMakeSelect", group: "Navigate", label: "Make the Candidate the Select (next photo becomes the Candidate)", chords: [c("ArrowUp")], modes: ["compare", "develop"], display: ["Up"], where: "Compare (Library and Develop)", needs: "compare" },
  { id: "compareFocus", group: "Navigate", label: "Switch the active pane (ratings, flags and edits go to it)", chords: [c("Tab"), c("c", { shift: true })], modes: ["compare"], display: ["Tab", "Shift+C"], where: "Library Compare" },
  { id: "compareFocus", group: "Navigate", label: "Switch the active pane", chords: [c("c", { shift: true })], modes: ["develop"], display: ["Shift+C"], where: "Develop Compare", needs: "compare", hidden: true },
  { id: "scenesToggle", group: "Scenes", label: "Show / hide the scene strip (hiding clears the scene filter)", chords: [c("s", { shift: true })], modes: ALL, display: ["Shift+S"], where: "Everywhere" },
  { id: "selectAll", group: "Navigate", label: "Select all", chords: [c("a", { mod: true })], modes: ["grid"], display: ["Cmd+A"], where: "Grid" },
  { id: "selectNone", group: "Navigate", label: "Select none", chords: [c("d", { mod: true })], modes: ["grid"], display: ["Cmd+D"], where: "Grid" },
  { id: "filterBar", group: "Navigate", label: "Show / hide the filter bar", chords: [c("f", { mod: true })], modes: ALL, display: ["Cmd+F"], where: "Library" },

  // ---- view ----
  { id: "zoomLoupe", group: "View", label: "Zoom Fit / 1:1 at the cursor (never leaves the view; zoom stays while you step with the arrows)", chords: [c(" ")], modes: ["loupe", "compare"], display: ["Space"], where: "Loupe, Compare" },
  { id: "zoomDevelop", group: "View", label: "Zoom Fit / 100% at the cursor (stays while you step with the arrows)", chords: [c(" ")], modes: ["develop"], display: ["Space"], where: "Develop" },
  { id: "faceDevelop", group: "View", label: "Zoom to each face at 100% (Shift = backwards)", chords: [c("f", { shift: "any" })], modes: ["develop"], display: ["F", "Shift+F"], where: "Develop" },
  { id: "face", group: "View", label: "Cycle face zoom (Shift = backwards)", chords: [c("f", { shift: "any" })], modes: ["loupe", "compare"], display: ["F", "Shift+F"], where: "Loupe, Compare" },
  { id: "photoInfo", group: "View", label: "Show / hide the photo info panel (time, camera, exposure, file)", chords: [c("i", { mod: true })], modes: ALL, display: ["Cmd+I"], where: "Library, Develop" },
  { id: "captureTime", group: "Library", label: "Edit capture time: shift, set or sync two cameras", chords: [c("t", { mod: true, shift: true })], modes: ALL, display: ["Cmd+Shift+T"], where: "Library, Develop" },
  { id: "info", group: "View", label: "Cycle info overlay (full / filename / hidden)", chords: [c("i")], modes: ["loupe", "compare"], display: ["I"], where: "Loupe, Compare" },

  // ---- develop ----
  { id: "before", group: "Develop", label: "Before / after", chords: [c("\\")], modes: ["develop"], display: ["\\"], where: "Develop" },
  { id: "split", group: "Develop", label: "Split view", chords: [c("y")], modes: ["develop"], display: ["Y"], where: "Develop" },
  { id: "crop", group: "Develop", label: "Crop tool (again: apply)", chords: [c("r")], modes: ["develop"], display: ["R"], where: "Develop" },
  { id: "cropCommit", group: "Develop", label: "Apply crop (Esc cancels it)", chords: [c("Enter")], modes: ["develop"], display: ["Enter"], where: "While cropping" },
  { id: "guided", group: "Develop", label: "Guided Upright tool: draw up to 4 guide lines (Esc exits)", chords: [c("t", { shift: true })], modes: ["develop"], display: ["Shift+T"], where: "Develop" },
  { id: "undoAdj", group: "Develop", label: "Undo (culling or adjustment, newest first)", chords: [c("z", { mod: true })], modes: ["develop"], display: ["Cmd+Z"], where: "Develop" },
  { id: "redoAdj", group: "Develop", label: "Redo (culling or adjustment)", chords: [c("z", { mod: true, shift: true })], modes: ["develop"], display: ["Cmd+Shift+Z"], where: "Develop" },
  { id: "bwToggle", group: "Develop", label: "Toggle Black & White", chords: [c("v")], modes: ["develop"], display: ["V"], where: "Develop" },
  { id: "wbPicker", group: "Develop", label: "White balance picker (click a neutral grey)", chords: [c("w")], modes: ["develop"], display: ["W"], where: "Develop" },
  { id: "pastePrev", group: "Copy & paste settings", label: "Paste settings from the previous photo (not crop / masks)", chords: [c("v", { mod: true, alt: true })], modes: ["develop"], display: ["Cmd+Alt+V"], where: "Develop" },
  { id: "savePreset", group: "Develop", label: "Save preset...", chords: [c("n", { mod: true, shift: true })], modes: ["develop"], display: ["Cmd+Shift+N"], where: "Develop" },
  { id: "copy", group: "Copy & paste settings", label: "Copy settings...", chords: [c("c", { mod: true, shift: true })], modes: ["develop"], display: ["Cmd+Shift+C"], where: "Develop" },
  { id: "paste", group: "Copy & paste settings", label: "Paste settings (Grid: to the selection)", chords: [c("v", { mod: true, shift: true })], modes: ["develop", "grid"], display: ["Cmd+Shift+V"], where: "Develop, Grid" },
  // Plain Cmd+C / Cmd+V: copy EVERY setting of the active photo (not crop / masks) and paste to every selected photo, one undoable
  // batch. Work in the Library and in Develop.
  { id: "copyAll", group: "Copy & paste settings", label: "Copy all settings of the active photo", chords: [c("c", { mod: true })], modes: ALL, display: ["Cmd+C"], where: "Everywhere (not while typing)" },
  { id: "pasteAll", group: "Copy & paste settings", label: "Paste the copied settings to every selected photo (one Undo)", chords: [c("v", { mod: true })], modes: ALL, display: ["Cmd+V"], where: "Everywhere (not while typing)" },
  { id: "sync", group: "Copy & paste settings", label: "Synchronize settings...", chords: [c("s", { mod: true, shift: true })], modes: ["develop"], display: ["Cmd+Shift+S"], where: "Develop" },
  { id: "syncQuiet", group: "Copy & paste settings", label: "Sync settings without the dialog (remembered fields)", chords: [c("s", { mod: true, alt: true })], modes: ["develop"], display: ["Cmd+Alt+S"], where: "Develop" },
  { id: "autoSync", group: "Copy & paste settings", label: "Auto Sync on / off (2+ photos selected: every change goes to all of them)", chords: [c("a", { mod: true, alt: true, shift: true })], modes: ["develop"], display: ["Cmd+Alt+Shift+A"], where: "Develop" },
  { id: "autoTone", group: "Develop", label: "Auto tone (Basic: Tone > Auto)", chords: [c("u", { mod: true })], modes: ["develop"], display: ["Cmd+U"], where: "Develop" },
  { id: "autoWb", group: "Develop", label: "Auto white balance", chords: [c("u", { mod: true, shift: true })], modes: ["develop"], display: ["Cmd+Shift+U"], where: "Develop" },
  { id: "reset", group: "Develop", label: "Reset all adjustments", chords: [c("r", { mod: true, shift: true })], modes: ["develop"], display: ["Cmd+Shift+R"], where: "Develop" },

  // ---- masks (Develop; the tool keys open the Masks panel themselves, so K never clashes with Compare's keeper) ----
  { id: "maskPanel", group: "Masks", label: "Show / hide the Masks panel", chords: [c("w", { shift: true })], modes: ["develop"], display: ["Shift+W"], where: "Develop" },
  { id: "maskBrush", group: "Masks", label: "Brush (Alt = erase, A = auto mask); opens Masks", chords: [c("k")], modes: ["develop"], display: ["K"], where: "Develop" },
  { id: "maskLinear", group: "Masks", label: "Linear gradient", chords: [c("m")], modes: ["develop"], display: ["M"], where: "Develop" },
  { id: "maskRadial", group: "Masks", label: "Radial gradient", chords: [c("m", { shift: true })], modes: ["develop"], display: ["Shift+M"], where: "Develop" },
  { id: "maskColor", group: "Masks", label: "Color range", chords: [c("j", { shift: true })], modes: ["develop"], display: ["Shift+J"], where: "Develop" },
  { id: "maskLuminance", group: "Masks", label: "Luminance range", chords: [c("q", { shift: true })], modes: ["develop"], display: ["Shift+Q"], where: "Develop" },
  { id: "maskOverlay", group: "Masks", label: "Show / hide the mask overlay", chords: [c("o")], modes: ["develop"], display: ["O"], where: "Develop (photo has masks)" },
  { id: "maskOverlayStyle", group: "Masks", label: "Cycle the overlay color / mode", chords: [c("o", { shift: true })], modes: ["develop"], display: ["Shift+O"], where: "Develop (photo has masks)" },
  { id: "maskPins", group: "Masks", label: "Show / hide mask pins", chords: [c("h")], modes: ["develop"], display: ["H"], where: "Develop (photo has masks)" },
  { id: "maskSize", group: "Masks", label: "Brush size smaller / larger", chords: [c("["), c("]")], modes: ["develop"], display: ["[ / ]"], where: "Develop, brush active" },
  { id: "maskFeather", group: "Masks", label: "Brush feather less / more", chords: [c("[", { shift: true }), c("]", { shift: true }), c("{", { shift: "any" }), c("}", { shift: "any" })], modes: ["develop"], display: ["Shift+[ / Shift+]"], where: "Develop, brush active" },
  { id: "maskAuto", group: "Masks", label: "Toggle brush auto mask", chords: [c("a")], modes: ["develop"], display: ["A"], where: "Develop, brush active" },
  { id: "maskDelete", group: "Masks", label: "Delete the selected mask component (Enter / Esc: finish the tool)", chords: [c("Delete"), c("Backspace")], modes: ["develop"], display: ["Delete"], where: "Develop, Masks panel open" },
  { id: "maskMoveUp", group: "Masks", label: "Move the selected mask (or its component) up in the stack", chords: [c("ArrowUp", { alt: true })], modes: ["develop"], display: ["Alt+Up"], where: "Develop, Masks panel open" },
  { id: "maskMoveDown", group: "Masks", label: "Move the selected mask (or its component) down in the stack", chords: [c("ArrowDown", { alt: true })], modes: ["develop"], display: ["Alt+Down"], where: "Develop, Masks panel open" },

  // ---- app ----
  { id: "saveXmp", group: "App", label: "Save metadata (XMP)", chords: [c("s", { mod: true })], modes: ALL, display: ["Cmd+S"], where: "Everywhere" },
  { id: "export", group: "App", label: "Export...", chords: [c("e", { mod: true, shift: true })], modes: ALL, display: ["Cmd+Shift+E"], where: "Everywhere" },
  { id: "import", group: "App", label: "Import folder...", chords: [c("i", { mod: true, shift: true })], modes: ALL, display: ["Cmd+Shift+I"], where: "Everywhere" },
  { id: "cheatSheet", group: "App", label: "Keyboard shortcuts", chords: [c("?", { shift: "any" }), c("/", { mod: true })], modes: ALL, display: ["?", "Cmd+/"], where: "Everywhere" },
  { id: "help", group: "App", label: "Help & FAQ", chords: [c("F1"), c("?", { mod: true, shift: "any" })], modes: ALL, display: ["F1", "Cmd+?"], where: "Everywhere" },
  { id: "dialogConfirm", group: "Dialogs", label: "Confirm (Export: Cmd+Enter)", chords: [], modes: ALL, display: ["Enter"], where: "Topmost dialog", external: true },
  { id: "dialogCancel", group: "Dialogs", label: "Cancel / close", chords: [], modes: ALL, display: ["Esc"], where: "Topmost dialog", external: true },
];

function chordMatches(ch: Chord, e: KeyboardEvent): boolean {
  const mod = e.metaKey || e.ctrlKey;
  if (!!ch.mod !== mod) return false;
  if (!!ch.alt !== e.altKey) return false;
  if (ch.shift !== "any" && !!ch.shift !== e.shiftKey) return false;
  if (ch.key.length === 1) {
    // With Option held macOS types a different character; the physical key still identifies letters.
    if (ch.alt && /^[a-z]$/.test(ch.key)) return e.code === `Key${ch.key.toUpperCase()}`;
    if (ch.alt && /^[0-9]$/.test(ch.key)) return e.code === `Digit${ch.key}`;
    return e.key.toLowerCase() === ch.key;
  }
  return e.key === ch.key;
}

/** The definition triggered by `e` in `mode`, if any. */
export function matchKey(e: KeyboardEvent, mode: Mode, ctx: KeyContext = {}): KeyDef | null {
  for (const d of KEYMAP) {
    if (d.external || !d.modes.includes(mode)) continue;
    if (d.needs === "crop" && !ctx.cropping) continue;
    if (d.needs === "compare" && !ctx.comparing) continue;
    if (d.chords.some((ch) => chordMatches(ch, e))) return d;
  }
  return null;
}

/** " (P)" style suffix for tooltips: `title={"Pick" + hint("pick")}`. */
export function hint(id: ActionId, index = 0): string {
  const d = KEYMAP.find((x) => x.id === id && x.display.length > 0);
  return d ? ` (${d.display[Math.min(index, d.display.length - 1)]})` : "";
}

/** Keymap grouped for the cheat sheet, in declaration order. */
export function keymapGroups(): { group: string; items: KeyDef[] }[] {
  const out: { group: string; items: KeyDef[] }[] = [];
  for (const d of KEYMAP) {
    if (d.hidden) continue;
    let g = out.find((x) => x.group === d.group);
    if (!g) out.push((g = { group: d.group, items: [] }));
    g.items.push(d);
  }
  return out;
}
