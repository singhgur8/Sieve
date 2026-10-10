import type { ActionId } from "../../lib/keymap";
import type { RawImageEntry, TargetEditResult, TargetRun } from "../../ipc";

/** The key event as far as steps care (Shift for backwards). */
export interface StageKey {
  key: string;
  shiftKey: boolean;
}

export type Stage = "setup" | "people" | "review" | "second";

/** What the shell gives every step. */
export interface TargetCtx {
  projectId: number;
  run: TargetRun | null;
  refreshRun: () => Promise<void>;
  entries: { get: (id: number) => RawImageEntry | undefined; need: (ids: number[]) => void; drop: (ids: number[]) => void };
  /** Runs a target edit, records its `previous` snapshots for Undo (Cmd+Z) and refreshes counts; null on error. */
  edit: (label: string, p: Promise<TargetEditResult>, at?: number) => Promise<TargetEditResult | null>;
  /** Set by Undo: the photo (and stage) the undone action was made on; the step moves there, then calls `focusDone`. */
  focus: { id: number; stage: Stage; n: number } | null;
  focusDone: () => void;
  undo: () => void;
  canUndo: boolean;
  /** Bumps after every edit or undo: steps refetch what they show. */
  rev: number;
  notify: (msg: string) => void;
  onError: (e: unknown) => void;
  /** Close the overlay and show these choices in the grid (filter chips). */
  showInGrid: (choices: ("deliver" | "alternative" | "not_sure" | "set_aside")[]) => void;
  go: (stage: Stage) => void;
}

export type { ActionId };
