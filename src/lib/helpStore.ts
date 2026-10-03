// Open / closed state of the Help & FAQ panel (F1, the Help button, the "?" links next to controls).
import { useSyncExternalStore } from "react";

interface HelpState {
  open: boolean;
  /** Entry to show (a `HelpEntry.id`); null = the first one. */
  entry: string | null;
}

let state: HelpState = { open: false, entry: null };
const listeners = new Set<() => void>();
const set = (s: HelpState) => {
  state = s;
  listeners.forEach((l) => l());
};

export const openHelp = (entry: string | null = null) => set({ open: true, entry });
export const closeHelp = () => set({ open: false, entry: null });

export function useHelpState(): HelpState {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => {
        listeners.delete(l);
      };
    },
    () => state,
    () => state,
  );
}
