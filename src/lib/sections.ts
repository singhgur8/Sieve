// Collapsed/expanded state of the Develop panel sections, remembered across sessions (localStorage).
import { useSyncExternalStore } from "react";

const KEY = "sieve.develop.sections.v2";
/** Sections that start closed (the long tail of Lightroom panels); everything else starts open. */
const CLOSED_BY_DEFAULT = new Set(["tone-curve", "hsl", "color-grading", "detail", "effects", "calibration"]);

/** Every section id in panel order (solo mode closes the others). */
export const SECTION_IDS = ["basic", "tone-curve", "hsl", "color-grading", "detail", "effects", "calibration"];

function load(): Record<string, boolean> {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? "{}");
    return v && typeof v === "object" ? v : {};
  } catch {
    return {};
  }
}

let state: Record<string, boolean> = load();
const listeners = new Set<() => void>();

function save(next: Record<string, boolean>) {
  state = next;
  try {
    localStorage.setItem(KEY, JSON.stringify(next));
  } catch {
    /* private mode: keep in memory */
  }
  listeners.forEach((l) => l());
}

const isOpen = (id: string) => state[id] ?? !CLOSED_BY_DEFAULT.has(id);

export function useSectionOpen(id: string): boolean {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => isOpen(id),
  );
}

export function toggleSection(id: string) {
  save({ ...state, [id]: !isOpen(id) });
}

export function setSectionOpen(id: string, open: boolean) {
  if (isOpen(id) !== open) save({ ...state, [id]: open });
}

/** Alt-click: open `id` and close every other section. */
export function soloSection(id: string) {
  const next: Record<string, boolean> = {};
  for (const s of SECTION_IDS) next[s] = s === id;
  save(next);
}

/** Test hook. */
export function resetSections() {
  save({});
}
