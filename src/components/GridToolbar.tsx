import { ArrowDownAZ, ArrowUpAZ, Columns2, Grid3x3, Maximize, SkipForward } from "lucide-react";
import type { ImageSort } from "../ipc";
import type { Query } from "../hooks/useLibrary";

export type Mode = "grid" | "loupe" | "compare";

const SORTS: { key: ImageSort; label: string }[] = [
  { key: "capture_time", label: "Capture time" },
  { key: "file_name", label: "File name" },
  { key: "rating", label: "Rating" },
  { key: "quality", label: "Quality" },
];

interface Props {
  mode: Mode;
  query: Query;
  setQuery: (fn: (q: Query) => Query) => void;
  size: number;
  onSize: (n: number) => void;
  selectedCount: number;
  total: number;
  autoAdvance: boolean;
  onAutoAdvance: (v: boolean) => void;
  onMode: (m: Mode) => void;
}

const seg = (on: boolean) => `flex items-center gap-1 rounded px-2 py-1 text-xs ${on ? "bg-sky-800 text-sky-100" : "bg-neutral-800 hover:bg-neutral-700"}`;

export function GridToolbar(p: Props) {
  return (
    <div className="flex flex-wrap items-center gap-3 border-b border-neutral-800 px-4 py-1.5 text-xs text-neutral-400" data-testid="grid-toolbar">
      <div className="flex gap-1">
        <button className={seg(p.mode === "grid")} onClick={() => p.onMode("grid")} title="Grid (G)" data-testid="mode-grid">
          <Grid3x3 className="size-3.5" /> Grid
        </button>
        <button className={seg(p.mode === "loupe")} onClick={() => p.onMode("loupe")} title="Loupe (Space / E / Enter)" data-testid="mode-loupe">
          <Maximize className="size-3.5" /> Loupe
        </button>
        <button className={seg(p.mode === "compare")} onClick={() => p.onMode("compare")} title="Compare (C)" data-testid="mode-compare">
          <Columns2 className="size-3.5" /> Compare
        </button>
      </div>
      <label className="flex items-center gap-2">
        Size
        <input type="range" min={90} max={420} step={10} value={p.size} onChange={(e) => p.onSize(Number(e.target.value))} data-testid="thumb-size" />
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
        onClick={() => p.setQuery((q) => ({ ...q, sortDescending: !q.sortDescending }))}
        title="Reverse sort order"
      >
        {p.query.sortDescending ? <ArrowUpAZ className="size-3.5" /> : <ArrowDownAZ className="size-3.5" />}
        {p.query.sortDescending ? "Descending" : "Ascending"}
      </button>
      <label className="flex items-center gap-1.5" title="Advance to the next photo after flagging/rating (Shift+P / Shift+X always advance)">
        <input type="checkbox" checked={p.autoAdvance} onChange={(e) => p.onAutoAdvance(e.target.checked)} data-testid="auto-advance" />
        <SkipForward className="size-3.5" /> Auto-advance
      </label>
      <span className="ml-auto" data-testid="selection-count">
        {p.selectedCount} selected · {p.total} photos
      </span>
    </div>
  );
}
