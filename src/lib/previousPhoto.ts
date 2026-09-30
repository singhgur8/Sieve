// Last photo edited in Develop (source of "Previous" / Paste from previous). Survives the module being re-entered.
// v14 `pastePrevious` persists this in the backend; until then it lives here.
import { useSyncExternalStore } from "react";

let previous: number | null = null;
const listeners = new Set<() => void>();

export const getPreviousPhoto = () => previous;

export function setPreviousPhoto(id: number | null) {
  if (previous === id) return;
  previous = id;
  listeners.forEach((l) => l());
}

export function usePreviousPhoto(): number | null {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    getPreviousPhoto,
    getPreviousPhoto,
  );
}
