import type { ReactNode } from "react";
import { ArrowDownAZ, ArrowUpAZ, ChevronDown, SkipForward, SlidersHorizontal } from "lucide-react";
import { Menu } from "./Menu";
import { useWide } from "./edit/bits";
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
  /** Photos in the catalog / folder before filtering (null while loading). */
  catalogTotal?: number | null;
  /** What the photos are called in the count (`keepers` in the Edit / Export steps). */
  unit?: string;
  capsLock?: boolean;
  autoAdvance: boolean;
  onAutoAdvance: (v: boolean) => void;
  /** Rating / label / burst / folder filters, shown to the left. */
  filters?: ReactNode;
  /** Project open: shoot type and Analyze (moved out of the TopBar). */
  leading?: ReactNode;
  /** Pinned at the right edge, never scrolled away ("Continue to Edit"). */
  trailing?: ReactNode;
}

const seg = (on: boolean) => `flex items-center gap-1 rounded px-2 py-1 text-xs ${on ? "bg-sky-800 text-sky-100" : "bg-neutral-800 hover:bg-neutral-700"}`;

/** Library-only row: extra filters plus thumbnail size, sort and auto-advance. */
export function GridToolbar(p: Props) {
  // Below 1440 px the view controls fold into a View popover so the count and filters stay visible.
  const wide = useWide("(min-width: 1440px)");
  const controls = (
    <>
      <label className="flex items-center gap-1.5">
        Size
        <input type="range" className="solid-range w-24" min={90} max={420} step={10} value={p.size} onChange={(e) => p.onSize(Number(e.target.value))} data-testid="thumb-size" />
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
        <input type="checkbox" checked={p.autoAdvance || !!p.capsLock} onChange={(e) => p.onAutoAdvance(e.target.checked)} data-testid="auto-advance" />
        <SkipForward className="size-3.5" /> Auto-advance{p.capsLock ? " (Caps Lock)" : ""}
      </label>
    </>
  );
  return (
    <div className="flex h-8 shrink-0 items-center border-b border-neutral-800" data-testid="grid-toolbar-row">
    <div
      className="flex h-full min-w-0 flex-1 items-center gap-x-3 overflow-x-auto overflow-y-hidden whitespace-nowrap px-3 text-xs text-neutral-300"
      data-testid="grid-toolbar"
    >
      {p.leading}
      {p.leading && <span className="mx-1 h-4 w-px shrink-0 bg-neutral-700" />}
      {p.filters}
      <span className="mx-1 h-4 w-px shrink-0 bg-neutral-700" />
      {wide ? (
        controls
      ) : (
        <Menu
          triggerTestId="view-menu"
          triggerClass="flex h-7 shrink-0 items-center gap-1 rounded bg-neutral-800 px-2 text-xs text-neutral-200 hover:bg-neutral-700"
          trigger={
            <>
              <SlidersHorizontal className="size-3.5" /> View <ChevronDown className="size-3" />
            </>
          }
          title="Thumbnail size, sort order and auto-advance"
        >
          {() => (
            <div className="flex w-64 flex-col gap-3 px-3 py-2 text-xs text-neutral-300" data-testid="view-menu-panel">
              {controls}
            </div>
          )}
        </Menu>
      )}
      <span className="ml-auto min-w-[120px] shrink-0 truncate pl-2 text-right" data-testid="selection-count">
        {p.catalogTotal != null && p.catalogTotal !== p.total ? `${p.total} of ${p.catalogTotal}${p.unit && p.unit !== "photos" ? ` ${p.unit}` : ""}` : `${p.total} ${p.unit ?? "photos"}`} · {p.selectedCount} selected
      </span>
    </div>
    {p.trailing && <div className="flex shrink-0 items-center px-3">{p.trailing}</div>}
    </div>
  );
}
