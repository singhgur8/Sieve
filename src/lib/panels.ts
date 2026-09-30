// Hidden-panel state per module (Lightroom's Tab / Shift+Tab), remembered across sessions (localStorage).
//   left / right: the Develop side panels; chrome: toolbar + filmstrip + filter summary (Develop) or filmstrip + filter summary (Loupe).
import { useSyncExternalStore } from "react";

export type PanelMode = "develop" | "loupe";
export interface Panels {
  left: boolean;
  right: boolean;
  chrome: boolean;
}

const NONE: Panels = { left: false, right: false, chrome: false };
const KEY = (m: PanelMode) => `sieve.panels.${m}`;

function load(m: PanelMode): Panels {
  try {
    const v = JSON.parse(localStorage.getItem(KEY(m)) ?? "null");
    if (v && typeof v === "object") return { left: !!v.left, right: !!v.right, chrome: !!v.chrome };
  } catch {
    /* fall through */
  }
  return NONE;
}

const state: Record<PanelMode, Panels> = { develop: load("develop"), loupe: load("loupe") };
const listeners = new Set<() => void>();

function save(m: PanelMode, next: Panels) {
  state[m] = next;
  try {
    localStorage.setItem(KEY(m), JSON.stringify(next));
  } catch {
    /* private mode: keep in memory */
  }
  listeners.forEach((l) => l());
}

export function getPanels(m: PanelMode): Panels {
  return state[m];
}

export function usePanels(m: PanelMode): Panels {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => state[m],
  );
}

/** Tab (Develop): hide / show both side panels; from full-bleed it restores everything. */
export function toggleSidePanels() {
  const p = state.develop;
  if (p.chrome) return save("develop", NONE);
  const hide = !(p.left && p.right);
  save("develop", { ...p, left: hide, right: hide });
}

/** Shift+Tab: hide / show everything (Develop: panels + toolbar + filmstrip; Loupe: filmstrip + filter summary). */
export function toggleChrome(m: PanelMode) {
  const p = state[m];
  if (p.chrome) return save(m, NONE);
  save(m, m === "develop" ? { left: true, right: true, chrome: true } : { ...p, chrome: true });
}

export function setPanelHidden(which: "left" | "right", hidden: boolean) {
  save("develop", { ...state.develop, [which]: hidden });
}

/** Test hook. */
export function resetPanels() {
  save("develop", NONE);
  save("loupe", NONE);
}
