// Horizontal virtualized filmstrip shared by Develop and Loupe: thumbnail, flag / label, stars, scene and edited markers.
import { useEffect, useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Flag, X } from "lucide-react";
import { convertFileSrc } from "../ipc";
import type { Library } from "../hooks/useLibrary";
import { LABEL_COLOR } from "../lib/format";
import { SceneBadge, Stars } from "./Cell";

interface Props {
  lib: Library;
  activeId: number | null;
  /** Multi-selection ring (Develop); Loupe passes none. */
  selected?: Set<number>;
  onPick: (id: number, e: React.MouseEvent) => void;
  /** Cell size in px and the strip height. */
  cellW: number;
  cellH: number;
  height: number;
  scenePrefix: string;
  align?: "auto" | "center";
  /** Click on a star: rate that photo (0 clears). */
  onRate?: (id: number, rating: number) => void;
  /** Extra per-cell badge (Compare: Select / Candidate). */
  badge?: (id: number) => React.ReactNode;
  /** Cells that get the secondary (candidate) ring. */
  marked?: Set<number>;
}

export function Filmstrip({ lib, activeId, selected, onPick, cellW, cellH, height, scenePrefix, align = "auto", onRate, badge, marked }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const { ids, ensure } = lib;
  const v = useVirtualizer({ horizontal: true, count: ids.length, getScrollElement: () => ref.current, estimateSize: () => cellW + 4, overscan: 8 });
  const idx = activeId == null ? -1 : ids.indexOf(activeId);
  useEffect(() => {
    if (idx >= 0) v.scrollToIndex(idx, { align });
  }, [idx, v, align]);
  const items = v.getVirtualItems();
  const first = items[0]?.index ?? 0;
  const last = (items[items.length - 1]?.index ?? -1) + 1;
  useEffect(() => {
    ensure(ids.slice(first, last));
  }, [ids, first, last, ensure]);

  return (
    <div ref={ref} className="shrink-0 overflow-x-auto overflow-y-hidden border-t border-neutral-800 bg-neutral-900" style={{ height }} data-testid="filmstrip">
      <div style={{ width: v.getTotalSize(), height: "100%", position: "relative" }}>
        {items.map((it) => {
          const fid = ids[it.index];
          const e = lib.getEntry(fid);
          const t = e?.thumbnail;
          const active = fid === activeId;
          const isSel = selected?.has(fid) ?? false;
          const isMarked = marked?.has(fid) ?? false;
          return (
            <div
              key={it.key}
              role="button"
              tabIndex={-1}
              data-testid={`film-${fid}`}
              data-active={active}
              data-selected={isSel}
              data-marked={isMarked}
              onClick={(ev) => onPick(fid, ev)}
              className={`absolute top-2 cursor-pointer overflow-hidden rounded bg-neutral-800 ${isSel ? "ring-2 ring-sky-500" : ""} ${isMarked ? "ring-2 ring-amber-400" : ""} ${active ? "outline outline-2 outline-white" : ""} ${e?.pick === "reject" && !active ? "opacity-50" : ""}`}
              style={{ left: it.start, width: cellW, height: cellH }}
            >
              {t?.status === "ready" && <img src={`${convertFileSrc(t.path)}?v=${lib.version(fid)}`} alt="" draggable={false} className="size-full object-cover" />}
              {e && (e.pick !== "unflagged" || e.colorLabel) && (
                <span className="pointer-events-none absolute left-0.5 top-0.5 flex items-center gap-0.5" data-testid={`film-flag-${fid}`} data-pick={e.pick}>
                  {e.pick === "pick" && <Flag className="size-3 fill-green-500 text-green-500" />}
                  {e.pick === "reject" && <X className="size-3.5 text-red-500" strokeWidth={3} />}
                  {e.colorLabel && <span className={`size-2 rounded-full ${LABEL_COLOR[e.colorLabel]}`} />}
                </span>
              )}
              {e && e.rating > 0 && (
                <span className="absolute bottom-0.5 right-0.5 rounded bg-black/70 px-0.5" data-testid={`film-rating-${fid}`} data-rating={e.rating}>
                  <Stars n={e.rating} className="size-2.5" onRate={onRate && ((r) => onRate(fid, r))} testId={`film-stars-${fid}`} />
                </span>
              )}
              {badge?.(fid)}
              {e && e.sceneId != null && (
                <span className="absolute bottom-0.5 left-0.5">
                  <SceneBadge entry={e} testPrefix={scenePrefix} />
                </span>
              )}
              {e?.hasEdits && <span className="absolute right-0.5 top-0.5 size-2 rounded-full bg-sky-400" title="Edited" data-testid={`film-edited-${fid}`} />}
            </div>
          );
        })}
      </div>
    </div>
  );
}
