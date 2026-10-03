import { memo, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { Library } from "../hooks/useLibrary";
import { FolderOpen } from "lucide-react";
import { Cell } from "./Cell";
import { hint } from "../lib/keymap";

const GAP = 6;
const PAD = 8;

interface Props {
  lib: Library;
  targetSize: number;
  selected: Set<number>;
  active: number | null;
  onColsChange: (cols: number, page: number) => void;
  onCellClick: (id: number, e: React.MouseEvent) => void;
  onCellDoubleClick: (id: number) => void;
  onRate?: (id: number, rating: number) => void;
  /** Burst sizes by group id (burst badge text). */
  burstSizes?: Map<number, number>;
  /** True when the catalog holds no photos at all (first run). */
  catalogEmpty: boolean;
  filtered: boolean;
  onImport: () => void;
  onClearFilters: () => void;
}

export const PhotoGrid = memo(PhotoGridImpl);

function PhotoGridImpl({ lib, targetSize, selected, active, onColsChange, onCellClick, onCellDoubleClick, onRate, burstSizes, catalogEmpty, filtered, onImport, onClearFilters }: Props) {
  const parentRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const { ids } = lib;

  useLayoutEffect(() => {
    const el = parentRef.current;
    if (!el) return;
    setWidth(el.clientWidth);
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const inner = Math.max(0, width - PAD * 2);
  const cols = Math.max(1, Math.floor((inner + GAP) / (targetSize + GAP)));
  const cellSize = Math.max(40, Math.floor((inner - GAP * (cols - 1)) / cols));
  const rowHeight = cellSize + GAP;
  const rowCount = Math.ceil(ids.length / cols);

  useEffect(() => {
    const h = parentRef.current?.clientHeight ?? 0;
    onColsChange(cols, cols * Math.max(1, Math.floor(h / rowHeight)));
  }, [cols, rowHeight, onColsChange]);

  const virtualizer = useVirtualizer({
    count: rowCount,
    getScrollElement: () => parentRef.current,
    estimateSize: () => rowHeight,
    overscan: 3,
    paddingStart: PAD,
    paddingEnd: PAD,
  });
  // Row height changes with the size slider / window width.
  useLayoutEffect(() => {
    virtualizer.measure();
  }, [rowHeight, virtualizer]);

  const items = virtualizer.getVirtualItems();
  const first = items.length ? items[0].index * cols : 0;
  const last = items.length ? Math.min(ids.length, (items[items.length - 1].index + 1) * cols) : 0;

  // Fetch entries for what is (nearly) on screen.
  const { ensure } = lib;
  useEffect(() => {
    ensure(ids.slice(first, last));
  }, [ids, first, last, ensure]);

  const activeIndex = active == null ? -1 : ids.indexOf(active);
  useEffect(() => {
    if (activeIndex >= 0) virtualizer.scrollToIndex(Math.floor(activeIndex / cols), { align: "auto" });
  }, [activeIndex, cols, virtualizer]);

  if (lib.loaded && ids.length === 0) {
    if (catalogEmpty && !filtered) {
      return (
        <div className="flex flex-1 flex-col items-center justify-center gap-3 text-center" data-testid="grid-empty-catalog">
          <FolderOpen className="size-10 text-neutral-400" />
          <h2 className="text-lg font-semibold text-neutral-100">Import a shoot folder to start</h2>
          <p className="max-w-md text-sm text-neutral-400">Sieve reads your RAW files in place, culls them automatically and writes ratings to XMP sidecars next to the originals.</p>
          <button onClick={onImport} data-testid="empty-import" className="flex items-center gap-2 rounded-md bg-sky-700 px-4 py-2 text-sm font-medium text-white hover:bg-sky-600">
            <FolderOpen className="size-4" /> Import folder{hint("import")}
          </button>
          <p className="text-xs text-neutral-400">Sony ARW, Fujifilm RAF, Canon CR3 · JPEG, HEIC, TIFF, PNG when enabled in Import ▾</p>
        </div>
      );
    }
    return (
      <div className="flex flex-1 flex-col items-center justify-center gap-3 text-sm text-neutral-400" data-testid="grid-empty">
        <span data-testid="grid-empty-text">
          {filtered ? "No photos match the current filters." : "This folder has no photos to show."}
        </span>
        {filtered && (
          <button onClick={onClearFilters} data-testid="empty-clear-filters" className="rounded bg-neutral-800 px-3 py-1.5 text-neutral-100 hover:bg-neutral-700">
            Clear filters
          </button>
        )}
      </div>
    );
  }

  return (
    <div ref={parentRef} className="flex-1 overflow-y-auto overflow-x-hidden" data-testid="grid-scroll">
      <div style={{ height: virtualizer.getTotalSize(), position: "relative", width: "100%" }} data-testid="grid-inner">
        {items.map((row) => {
          const start = row.index * cols;
          const rowIds = ids.slice(start, start + cols);
          return (
            <div
              key={row.key}
              data-testid="grid-row"
              className="absolute left-0 grid"
              style={{
                top: 0,
                transform: `translateY(${row.start}px)`,
                height: cellSize,
                left: PAD,
                right: PAD,
                gap: GAP,
                gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))`,
              }}
            >
              {rowIds.map((id) => (
                <Cell
                  key={id}
                  id={id}
                  entry={lib.getEntry(id)}
                  version={lib.version(id)}
                  size={cellSize}
                  selected={selected.has(id)}
                  active={active === id}
                  onClick={onCellClick}
                  onDoubleClick={onCellDoubleClick}
                  onRate={onRate}
                  burstSize={(() => {
                    const g = lib.getEntry(id)?.burstGroupId;
                    return g != null ? burstSizes?.get(g) : undefined;
                  })()}
                />
              ))}
            </div>
          );
        })}
      </div>
    </div>
  );
}
