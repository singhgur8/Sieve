// Culling undo/redo (pick / rating / color label). Entries hold the state to restore; applying one
// captures the current state first so the opposite stack can restore it (restore_cull_snapshot is atomic).
import { useCallback, useRef } from "react";
import { commands, unwrap, type CullSnapshot, type RawImageEntry } from "../ipc";

const MAX = 100;

export interface CullEntry {
  label: string;
  ids: number[];
  /** State to restore when this entry is applied. */
  snaps: CullSnapshot[];
  /** When the change was made (ms), so Develop can undo the newest of culling vs adjustment. */
  at: number;
}

export const snapOf = (e: RawImageEntry): CullSnapshot => ({ imageId: e.id, rating: e.rating, pick: e.pick, colorLabel: e.colorLabel, pickOrigin: e.pickOrigin });

interface Deps {
  getEntry: (id: number) => RawImageEntry | undefined;
  /** Called with the ids that changed after a restore. */
  onRestored: (ids: number[]) => void;
  toast: (msg: string) => void;
  onError: (e: unknown) => void;
}

export function useCullUndo({ getEntry, onRestored, toast, onError }: Deps) {
  const undoStack = useRef<CullEntry[]>([]);
  const redoStack = useRef<CullEntry[]>([]);
  const busy = useRef(false);

  /** Snapshot of the current state; cached rows are read synchronously, the rest via the backend. */
  const capture = useCallback(
    async (ids: number[]): Promise<CullSnapshot[]> => {
      const out: CullSnapshot[] = [];
      const missing: number[] = [];
      for (const id of ids) {
        const e = getEntry(id);
        if (e) out.push(snapOf(e));
        else missing.push(id);
      }
      if (missing.length) out.push(...(await unwrap(commands.getCullSnapshot(missing))));
      return out;
    },
    [getEntry],
  );

  /** Records a change made with `before` as the previous state. */
  const record = useCallback((label: string, before: CullSnapshot[]): CullEntry => {
    const entry: CullEntry = { label, ids: before.map((s) => s.imageId), snaps: before, at: Date.now() };
    undoStack.current = [...undoStack.current, entry].slice(-MAX);
    redoStack.current = [];
    return entry;
  }, []);

  const apply = useCallback(
    async (entry: CullEntry, from: "undo" | "redo") => {
      if (busy.current) return;
      busy.current = true;
      try {
        const current = await unwrap(commands.getCullSnapshot(entry.ids));
        const changed = await unwrap(commands.restoreCullSnapshot(entry.snaps));
        const opposite: CullEntry = { label: entry.label, ids: entry.ids, snaps: current, at: Date.now() };
        if (from === "undo") {
          undoStack.current = undoStack.current.filter((x) => x !== entry);
          redoStack.current = [...redoStack.current, opposite].slice(-MAX);
        } else {
          redoStack.current = redoStack.current.filter((x) => x !== entry);
          undoStack.current = [...undoStack.current, opposite].slice(-MAX);
        }
        onRestored(changed);
        toast(`${from === "undo" ? "Undid" : "Redid"}: ${entry.label}`);
      } catch (e) {
        onError(e);
      } finally {
        busy.current = false;
      }
    },
    [onRestored, toast, onError],
  );

  const undo = useCallback(async () => {
    const e = undoStack.current[undoStack.current.length - 1];
    if (!e) return toast("Nothing to undo");
    await apply(e, "undo");
  }, [apply, toast]);

  const redo = useCallback(async () => {
    const e = redoStack.current[redoStack.current.length - 1];
    if (!e) return toast("Nothing to redo");
    await apply(e, "redo");
  }, [apply, toast]);

  /** Undo one specific recorded entry (toast Undo buttons), wherever it sits in the stack. */
  const undoEntry = useCallback(
    async (entry: CullEntry) => {
      if (!undoStack.current.includes(entry)) return toast("Already undone");
      await apply(entry, "undo");
    },
    [apply, toast],
  );

  /** Timestamp of the entry Cmd+Z would undo / Cmd+Shift+Z would redo (0 = none). */
  const undoAt = useCallback(() => undoStack.current[undoStack.current.length - 1]?.at ?? 0, []);
  const redoAt = useCallback(() => redoStack.current[redoStack.current.length - 1]?.at ?? 0, []);

  return { capture, record, undo, redo, undoEntry, undoAt, redoAt };
}
