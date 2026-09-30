// Import dialog options (Lightroom's "Include JPEG / pair with RAW"), remembered across sessions.
import { useCallback, useState } from "react";
import type { ImportOptions } from "../ipc";

const KEY = "sieve.import.options";
export const DEFAULT_IMPORT_OPTIONS: ImportOptions = { recursive: true, includeNonRaw: false, pairJpegWithRaw: true };

function load(): ImportOptions {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? "null");
    if (v && typeof v === "object") return { ...DEFAULT_IMPORT_OPTIONS, ...v, recursive: true };
  } catch {
    /* fall through */
  }
  return DEFAULT_IMPORT_OPTIONS;
}

export function useImportOptions(): [ImportOptions, (patch: Partial<ImportOptions>) => void] {
  const [opts, setOpts] = useState<ImportOptions>(load);
  const update = useCallback((patch: Partial<ImportOptions>) => {
    setOpts((o) => {
      const n = { ...o, ...patch };
      try {
        localStorage.setItem(KEY, JSON.stringify(n));
      } catch {
        /* keep in memory */
      }
      return n;
    });
  }, []);
  return [opts, update];
}
