import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { Library } from "../hooks/useLibrary";
import { Cell } from "./Cell";

const GAP = 6;
const PAD = 8;

interface Props {
  lib: Library;
  targetSize: number;
  selected: Set<number>;
  active: number | null;
  onColsChange: (cols: number) => void;
  onCellClick: (id: number, e: React.MouseEvent) => void;
  onCellDoubleClick: (id: number) => void;
}

export function PhotoGrid({ lib, targetSize, selected, active, onColsChange, onCellClick, onCellDoubleClick }: Props) {
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
    onColsChange(cols);
  }, [cols, onColsChange]);

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
    return (
      <div className="flex flex-1 items-center justify-center text-sm text-neutral-500" data-testid="grid-empty">
        No images match the current filters.
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
                />
              ))}
            </div>
          );
        })}
      </div>
    </div>
  );
}
