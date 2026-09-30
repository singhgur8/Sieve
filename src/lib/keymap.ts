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
  | "devToLoupe"
  | "toGrid"
  | "escape"
  | "develop"
  | "compare"
  | "tab"
  | "zoomLoupe"
  | "zoomDevelop"
  | "face"
  | "info"
  | "keeper"
  | "keeperSet"
  | "anchor"
  | "before"
  | "split"
  | "crop"
  | "cropCommit"
  | "undoAdj"
  | "redoAdj"
  | "undoCull"
  | "redoCull"
  | "copy"
  | "paste"
  | "sync"
  | "reset"
  | "selectBurst"
  | "selectAll"
  | "selectNone"
  | "saveXmp"
  | "export"
  | "import"
  | "filterBar"
  | "cheatSheet"
  | "dialogConfirm"
  | "dialogCancel";

export interface Chord {
  /** `KeyboardEvent.key` (case-insensitive for letters). */
  key: string;
  /** Cmd (mac) / Ctrl. */
  mod?: boolean;
  /** true = required, false/undefined = must not be held, "any" = ignored. */
  shift?: boolean | "any";
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
}

const c = (key: string, o: Omit<Chord, "key"> = {}): Chord => ({ key, ...o });
const digits = (from: number, to: number) => Array.from({ length: to - from + 1 }, (_, i) => c(String(from + i)));

export const KEYMAP: KeyDef[] = [
  // ---- culling ----
  { id: "pick", group: "Culling", label: "Pick (Shift = and advance)", chords: [c("p", { shift: "any" })], modes: ALL, display: ["P"], where: "Everywhere" },
  { id: "reject", group: "Culling", label: "Reject (Shift = and advance)", chords: [c("x", { shift: "any" })], modes: ALL, display: ["X"], where: "Everywhere" },
  { id: "unflag", group: "Culling", label: "Unflag", chords: [c("u")], modes: ALL, display: ["U"], where: "Everywhere" },
  { id: "rate", group: "Culling", label: "Star rating", chords: digits(0, 5), modes: ALL, display: ["0-5"], where: "Everywhere" },
  { id: "label", group: "Culling", label: "Color label (red, yellow, green, blue)", chords: digits(6, 9), modes: ALL, display: ["6-9"], where: "Everywhere" },
  { id: "keeper", group: "Culling", label: "Make this frame the burst keeper and Pick it", chords: [c("k")], modes: ["compare"], display: ["K"], where: "Compare" },
  { id: "keeperSet", group: "Culling", label: "Make the active photo its burst keeper", chords: [c("k", { shift: true })], modes: LIB, display: ["Shift+K"], where: "Grid, Loupe" },
  { id: "undoCull", group: "Culling", label: "Undo culling change", chords: [c("z", { mod: true })], modes: LIB, display: ["Cmd+Z"], where: "Outside Develop" },
  { id: "redoCull", group: "Culling", label: "Redo culling change", chords: [c("z", { mod: true, shift: true })], modes: LIB, display: ["Cmd+Shift+Z"], where: "Outside Develop" },
  { id: "anchor", group: "Scenes", label: "Toggle scene anchor", chords: [c("a", { shift: true })], modes: ALL, display: ["Shift+A"], where: "Everywhere" },
  { id: "selectBurst", group: "Scenes", label: "Select the active photo's burst", chords: [c("b", { mod: true, shift: true })], modes: ALL, display: ["Cmd+Shift+B"], where: "Everywhere" },

  // ---- navigation ----
  { id: "navH", group: "Navigate", label: "Previous / next photo (Compare: focused pane; Grid: Shift extends)", chords: [c("ArrowLeft", { shift: "any" }), c("ArrowRight", { shift: "any" })], modes: ALL, display: ["Left / Right"], where: "Everywhere" },
  { id: "navV", group: "Navigate", label: "Row up / down", chords: [c("ArrowUp", { shift: "any" }), c("ArrowDown", { shift: "any" })], modes: ["grid"], display: ["Up / Down"], where: "Grid" },
  { id: "gridJump", group: "Navigate", label: "First / last / page up / page down (Shift extends)", chords: [c("Home", { shift: "any" }), c("End", { shift: "any" }), c("PageUp", { shift: "any" }), c("PageDown", { shift: "any" })], modes: ["grid"], display: ["Home / End / PgUp / PgDn"], where: "Grid" },
  { id: "toggleLoupe", group: "Navigate", label: "Grid to Loupe and back", chords: [c(" "), c("Enter"), c("e")], modes: LIB, display: ["Space", "Enter", "E"], where: "Grid, Loupe, Compare" },
  { id: "devToLoupe", group: "Navigate", label: "Open in Loupe", chords: [c("e")], modes: ["develop"], display: ["E"], where: "Develop" },
  { id: "toGrid", group: "Navigate", label: "Back to Grid", chords: [c("g")], modes: ["loupe", "compare", "develop"], display: ["G"], where: "Outside Grid" },
  { id: "escape", group: "Navigate", label: "Back to Grid (in Grid: clear selection; while cropping: cancel the crop)", chords: [c("Escape")], modes: ALL, display: ["Esc"], where: "Everywhere (never with a dialog open)" },
  { id: "develop", group: "Navigate", label: "Develop", chords: [c("d")], modes: LIB, display: ["D"], where: "Grid, Loupe, Compare" },
  { id: "compare", group: "Navigate", label: "Compare (from Compare: back to Loupe)", chords: [c("c")], modes: LIB, display: ["C"], where: "Grid, Loupe, Compare" },
  { id: "tab", group: "Navigate", label: "Switch the focused Compare pane", chords: [c("Tab")], modes: ["compare"], display: ["Tab"], where: "Compare" },
  { id: "selectAll", group: "Navigate", label: "Select all", chords: [c("a", { mod: true })], modes: ["grid"], display: ["Cmd+A"], where: "Grid" },
  { id: "selectNone", group: "Navigate", label: "Select none", chords: [c("d", { mod: true })], modes: ["grid"], display: ["Cmd+D"], where: "Grid" },
  { id: "filterBar", group: "Navigate", label: "Show / hide the filter bar", chords: [c("f", { mod: true })], modes: ALL, display: ["Cmd+F"], where: "Library" },

  // ---- view ----
  { id: "zoomLoupe", group: "View", label: "Zoom to 1:1", chords: [c("z")], modes: LIB, display: ["Z"], where: "Grid, Loupe, Compare" },
  { id: "zoomDevelop", group: "View", label: "Zoom to 100%", chords: [c("z")], modes: ["develop"], display: ["Z"], where: "Develop" },
  { id: "face", group: "View", label: "Cycle face zoom (Shift = backwards)", chords: [c("f", { shift: "any" })], modes: ["loupe", "compare"], display: ["F", "Shift+F"], where: "Loupe, Compare" },
  { id: "info", group: "View", label: "Cycle info overlay (full / filename / hidden)", chords: [c("i")], modes: ["loupe", "compare"], display: ["I"], where: "Loupe, Compare" },

  // ---- develop ----
  { id: "before", group: "Develop", label: "Before / after", chords: [c("\\")], modes: ["develop"], display: ["\\"], where: "Develop" },
  { id: "split", group: "Develop", label: "Split view", chords: [c("y")], modes: ["develop"], display: ["Y"], where: "Develop" },
  { id: "crop", group: "Develop", label: "Crop tool (again: apply)", chords: [c("r")], modes: ["develop"], display: ["R"], where: "Develop" },
  { id: "cropCommit", group: "Develop", label: "Apply crop (Esc cancels it)", chords: [c("Enter")], modes: ["develop"], display: ["Enter"], where: "While cropping" },
  { id: "undoAdj", group: "Develop", label: "Undo adjustment", chords: [c("z", { mod: true })], modes: ["develop"], display: ["Cmd+Z"], where: "Develop" },
  { id: "redoAdj", group: "Develop", label: "Redo adjustment", chords: [c("z", { mod: true, shift: true })], modes: ["develop"], display: ["Cmd+Shift+Z"], where: "Develop" },
  { id: "copy", group: "Develop", label: "Copy settings...", chords: [c("c", { mod: true, shift: true })], modes: ["develop"], display: ["Cmd+Shift+C"], where: "Develop" },
  { id: "paste", group: "Develop", label: "Paste settings (Grid: to the selection)", chords: [c("v", { mod: true, shift: true })], modes: ["develop", "grid"], display: ["Cmd+Shift+V"], where: "Develop, Grid" },
  { id: "sync", group: "Develop", label: "Sync settings...", chords: [c("s", { mod: true, shift: true })], modes: ["develop"], display: ["Cmd+Shift+S"], where: "Develop" },
  { id: "reset", group: "Develop", label: "Reset all adjustments", chords: [c("r", { mod: true, shift: true })], modes: ["develop"], display: ["Cmd+Shift+R"], where: "Develop" },

  // ---- app ----
  { id: "saveXmp", group: "App", label: "Save metadata (XMP)", chords: [c("s", { mod: true })], modes: ALL, display: ["Cmd+S"], where: "Everywhere" },
  { id: "export", group: "App", label: "Export...", chords: [c("e", { mod: true, shift: true })], modes: ALL, display: ["Cmd+Shift+E"], where: "Everywhere" },
  { id: "import", group: "App", label: "Import folder...", chords: [c("i", { mod: true, shift: true })], modes: ALL, display: ["Cmd+Shift+I"], where: "Everywhere" },
  { id: "cheatSheet", group: "App", label: "Keyboard shortcuts", chords: [c("?", { shift: "any" }), c("/", { mod: true })], modes: ALL, display: ["?", "Cmd+/"], where: "Everywhere" },
  { id: "dialogConfirm", group: "Dialogs", label: "Confirm (Export: Cmd+Enter)", chords: [], modes: ALL, display: ["Enter"], where: "Topmost dialog", external: true },
  { id: "dialogCancel", group: "Dialogs", label: "Cancel / close", chords: [], modes: ALL, display: ["Esc"], where: "Topmost dialog", external: true },
];

function chordMatches(ch: Chord, e: KeyboardEvent): boolean {
  const mod = e.metaKey || e.ctrlKey;
  if (!!ch.mod !== mod) return false;
  if (ch.shift !== "any" && !!ch.shift !== e.shiftKey) return false;
  return ch.key.length === 1 ? e.key.toLowerCase() === ch.key : e.key === ch.key;
}

/** The definition triggered by `e` in `mode`, if any. */
export function matchKey(e: KeyboardEvent, mode: Mode): KeyDef | null {
  if (e.altKey) return null;
  for (const d of KEYMAP) {
    if (d.external || !d.modes.includes(mode)) continue;
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
    let g = out.find((x) => x.group === d.group);
    if (!g) out.push((g = { group: d.group, items: [] }));
    g.items.push(d);
  }
  return out;
}
