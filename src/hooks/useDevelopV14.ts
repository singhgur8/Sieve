// Develop hooks on the IPC v14 commands: the style library (Presets panel groups, folder import), hover previews
// (150 ms dwell -> `resolvePreset` / `renderPreview` on the `navigator` slot) and snapshots (no backend command yet).
import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { commands, unwrap, type ImportStyleReport, type ParametricAdjustments, type StyleGroup } from "../ipc";

export interface StyleLibraryApi {
  /** Every style group (User Presets, imported folders, LUTs), library order. */
  groups: StyleGroup[];
  /** Bumped after every change (import / removal / reload): profile lists re-read on it. */
  version: number;
  importing: boolean;
  reload: () => Promise<void>;
  /** Directory picker, then `importStyleFolder`; null when cancelled or failed (the error goes to `onError`). */
  importFolder: () => Promise<ImportStyleReport | null>;
  removeGroup: (id: number) => Promise<void>;
}

/** `list_styles()` with import / remove; reloads on demand (after saving a user preset, for instance). */
export function useStyleLibrary(onError: (e: unknown) => void): StyleLibraryApi {
  const [groups, setGroups] = useState<StyleGroup[]>([]);
  const [version, setVersion] = useState(0);
  const [importing, setImporting] = useState(false);
  const reload = useCallback(async () => {
    try {
      const lib = await unwrap(commands.listStyles());
      setGroups(lib.groups);
      setVersion((v) => v + 1);
    } catch (e) {
      onError(e);
    }
  }, [onError]);
  useEffect(() => {
    void reload();
  }, [reload]);
  const importFolder = useCallback(async () => {
    const path = await open({ title: "Import presets & profiles (folder)", directory: true });
    if (typeof path !== "string") return null;
    setImporting(true);
    try {
      const report = await unwrap(commands.importStyleFolder(path));
      await reload();
      return report;
    } catch (e) {
      onError(e);
      return null;
    } finally {
      setImporting(false);
    }
  }, [reload, onError]);
  const removeGroup = useCallback(
    async (id: number) => {
      try {
        await unwrap(commands.removeStyleGroup(id));
        await reload();
      } catch (e) {
        onError(e);
      }
    },
    [reload, onError],
  );
  return { groups, version, importing, reload, importFolder, removeGroup };
}

export interface HoverPreview {
  url: string;
  label: string;
  /** Where it is shown: the Navigator (presets) or the main viewer (profiles). */
  to: "navigator" | "viewer";
}

/**
 * Lightroom hover preview: after 150 ms on an item, render the photo with the adjustments `getAdjustments` resolves
 * (slot `navigator`, so the main render is never superseded); `stop()` (mouse leave) restores the normal view.
 */
export function useHoverPreview(imageId: number | null, maxEdgeFor: (to: "navigator" | "viewer") => number) {
  const [preview, setPreview] = useState<HoverPreview | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const token = useRef(0);
  const stop = useCallback(() => {
    clearTimeout(timer.current);
    token.current++;
    setPreview(null);
  }, []);
  const start = useCallback(
    (label: string, to: "navigator" | "viewer", getAdjustments: () => Promise<ParametricAdjustments>) => {
      if (imageId == null) return;
      clearTimeout(timer.current);
      const t = ++token.current;
      timer.current = setTimeout(() => {
        void (async () => {
          try {
            const adj = await getAdjustments();
            const r = await unwrap(commands.renderPreview(imageId, adj, { maxEdge: Math.max(64, Math.min(2048, maxEdgeFor(to))), slot: "navigator", region: null }));
            if (r && t === token.current) setPreview({ url: r.url, label, to });
          } catch {
            /* a failed hover preview is silent: the normal view stays */
          }
        })();
      }, 150);
    },
    [imageId, maxEdgeFor],
  );
  useEffect(() => stop, [imageId, stop]);
  return { preview, start, stop };
}

export interface SnapshotView {
  id: string;
  name: string;
}

export interface Snapshots {
  supported: boolean;
  items: SnapshotView[];
}

/** Snapshots need list/create/update/delete commands that are not in the v14 contract yet. */
export function useSnapshots(_imageId: number | null): Snapshots {
  return { supported: false, items: [] };
}
