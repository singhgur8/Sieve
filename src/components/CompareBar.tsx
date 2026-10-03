// Compare view controls shared by Library Compare and Develop Compare: pane labels, Swap, Make Select, leave.
import { ArrowLeftRight, ArrowUpToLine, X } from "lucide-react";
import { hint } from "../lib/keymap";

export type CompareFocus = "a" | "b";

interface Props {
  focus: CompareFocus;
  aName: string;
  bName: string;
  onSwap: () => void;
  onMakeSelect: () => void;
  onFocus: (k: CompareFocus) => void;
  /** Library Compare: hand the pair to Develop (editing). */
  onEdit?: () => void;
  onExit?: () => void;
}

const btn = "flex items-center gap-1 rounded bg-neutral-800 px-2 py-1 text-xs text-neutral-200 hover:bg-neutral-700";

export function CompareBar({ focus, aName, bName, onSwap, onMakeSelect, onFocus, onEdit, onExit }: Props) {
  const tab = (k: CompareFocus, label: string, name: string) => (
    <button
      className={`rounded px-2 py-1 text-xs ${focus === k ? "bg-sky-800 text-sky-100" : "bg-neutral-800 text-neutral-300 hover:bg-neutral-700"}`}
      onClick={() => onFocus(k)}
      aria-pressed={focus === k}
      title={`${label}: ${name}. Ratings, flags and edits go to the active pane${hint("compareFocus", 1)}`}
      data-testid={`compare-focus-${k}`}
    >
      {label} <span className="text-neutral-300">{name}</span>
    </button>
  );
  return (
    <div className="flex shrink-0 items-center gap-2 border-b border-neutral-800 bg-neutral-950 px-3 py-1 text-neutral-300" data-testid="compare-bar">
      {tab("a", "Select", aName)}
      {tab("b", "Candidate", bName)}
      <button className={btn} onClick={onSwap} title={`Swap Select and Candidate${hint("compareSwap")}`} data-testid="compare-swap">
        <ArrowLeftRight className="size-3.5" /> Swap
      </button>
      <button className={btn} onClick={onMakeSelect} title={`Make the Candidate the Select and move on${hint("compareMakeSelect")}`} data-testid="compare-make-select">
        <ArrowUpToLine className="size-3.5" /> Make Select
      </button>
      {onEdit && (
        <button className={btn} onClick={onEdit} title={`Edit the pair in Develop${hint("develop")}`} data-testid="compare-edit">
          Edit in Develop
        </button>
      )}
      <span className="ml-auto text-[11px] text-neutral-400">Click a thumbnail or use Left / Right to choose the Candidate</span>
      {onExit && (
        <button className={btn} onClick={onExit} title={`Leave Compare${hint("compare")}`} data-testid="compare-exit">
          <X className="size-3.5" /> Done
        </button>
      )}
    </div>
  );
}

/** Tiny "Select" / "Candidate" chip for a filmstrip cell. */
export function CompareTag({ id, a, b }: { id: number; a: number; b: number }) {
  if (id !== a && id !== b) return null;
  return (
    <span
      title={id === a ? "Select: the frame you are keeping so far (left pane)" : "Candidate: the frame being compared against the Select (right pane)"}
      aria-label={id === a ? "Select" : "Candidate"}
      className={`absolute inset-x-0 top-0 text-center text-[9px] font-semibold leading-3 ${id === a ? "bg-sky-700 text-white" : "bg-amber-400 text-black"}`}
      data-testid={`film-tag-${id}`}
    >
      {id === a ? "Select" : "Candidate"}
    </span>
  );
}
