import type { ReactNode } from "react";
import { ArrowDownAZ, ArrowUpAZ, SkipForward } from "lucide-react";
import type { ImageSort } from "../ipc";
import type { Query } from "../hooks/useLibrary";
import type { Mode } from "../lib/keymap";

export type { Mode };

const SORTS: { key: ImageSort; label: string }[] = [
  { key: "capture_time", label: "Capture time" },
  { key: "file_name", label: "File name" },
  { key: "rating", label: "Rating" },
  { key: "quality", label: "Quality" },
];

interface Props {
  query: Query;
  setQuery: (fn: (q: Query) => Query) => void;
  size: number;
  onSize: (n: number) => void;
  selectedCount: number;
  total: number;
  autoAdvance: boolean;
  onAutoAdvance: (v: boolean) => void;
  /** Rating / label / burst / folder filters, shown to the left. */
  filters?: ReactNode;
}

const seg = (on: boolean) => `flex items-center gap-1 rounded px-2 py-1 text-xs ${on ? "bg-sky-800 text-sky-100" : "bg-neutral-800 hover:bg-neutral-700"}`;

/** Library-only row: extra filters plus thumbnail size, sort and auto-advance. */
export function GridToolbar(p: Props) {
  return (
    <div
      className="flex h-8 shrink-0 items-center gap-x-3 overflow-x-auto overflow-y-hidden whitespace-nowrap border-b border-neutral-800 px-3 text-xs text-neutral-300"
      data-testid="grid-toolbar"
    >
      {p.filters}
      <span className="mx-1 h-4 w-px shrink-0 bg-neutral-700" />
      <label className="flex items-center gap-1.5">
        Size
        <input type="range" className="w-20" min={90} max={420} step={10} value={p.size} onChange={(e) => p.onSize(Number(e.target.value))} data-testid="thumb-size" />
      </label>
      <label className="flex items-center gap-1.5">
        Sort
        <select
          value={p.query.sort}
          data-testid="sort-select"
          onChange={(e) => p.setQuery((q) => ({ ...q, sort: e.target.value as ImageSort }))}
          className="rounded bg-neutral-800 px-1.5 py-0.5 text-neutral-200"
        >
          {SORTS.map((s) => (
            <option key={s.key} value={s.key}>
              {s.label}
            </option>
          ))}
        </select>
      </label>
      <button
        className={seg(p.query.sortDescending)}
        data-testid="sort-dir"
        aria-label={p.query.sortDescending ? "Descending" : "Ascending"}
        onClick={() => p.setQuery((q) => ({ ...q, sortDescending: !q.sortDescending }))}
        title={p.query.sortDescending ? "Descending (click for ascending)" : "Ascending (click for descending)"}
      >
        {p.query.sortDescending ? <ArrowUpAZ className="size-3.5" /> : <ArrowDownAZ className="size-3.5" />}
      </button>
      <label className="flex items-center gap-1.5" title="Advance to the next photo after flagging/rating (Shift+P / Shift+X always advance)">
        <input type="checkbox" checked={p.autoAdvance} onChange={(e) => p.onAutoAdvance(e.target.checked)} data-testid="auto-advance" />
        <SkipForward className="size-3.5" /> Auto-advance
      </label>
      <span className="ml-auto pl-2" data-testid="selection-count">
        {p.selectedCount} selected · {p.total} photos
      </span>
    </div>
  );
}
