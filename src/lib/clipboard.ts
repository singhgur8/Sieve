// Copied develop settings, shared by Develop (Copy / Paste / Sync) and the Library (paste to the grid selection).
import { useSyncExternalStore } from "react";
import type { AdjustmentField, ParametricAdjustments } from "../ipc";

export interface Copied {
  adjustments: ParametricAdjustments;
  fields: AdjustmentField[];
  /** File name of the photo the settings came from (tooltips and toasts). */
  fromName?: string;
}

let current: Copied | null = null;
const listeners = new Set<() => void>();

export const getClipboard = () => current;

export function setClipboard(c: Copied | null) {
  current = c;
  listeners.forEach((l) => l());
}

export function useClipboard(): Copied | null {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    getClipboard,
    getClipboard,
  );
}
