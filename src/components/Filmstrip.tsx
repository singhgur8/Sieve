// Horizontal virtualized filmstrip shared by Develop and Loupe: thumbnail, flag / label, stars, scene and edited markers.
import { memo, useCallback, useEffect, useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Flag, X } from "lucide-react";
import { type RawImageEntry } from "../ipc";
import type { Library } from "../hooks/useLibrary";
import { LABEL_COLOR } from "../lib/format";
import { SceneBadge, Stars } from "./Cell";
import { flagTitle } from "../lib/cull";
import { thumbSrc } from "../lib/entryImage";

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
  /** Space above the cells in px (default 8). */
  top?: number;
}

export function Filmstrip({ lib, activeId, selected, onPick, cellW, cellH, height, scenePrefix, align = "auto", onRate, badge, marked, top = 8 }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  // Handlers arrive as fresh closures every render; cells get stable wrappers so they can stay memoised.
  const pickRef = useRef(onPick);
  pickRef.current = onPick;
  const pick = useCallback((id: number, ev: React.MouseEvent) => pickRef.current(id, ev), []);
  const rateRef = useRef(onRate);
  rateRef.current = onRate;
  const rate = useCallback((id: number, r: number) => rateRef.current?.(id, r), []);
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
          return (
            <FilmCell
              key={it.key}
              fid={fid}
              e={lib.getEntry(fid)}
              sig={entrySig(lib.getEntry(fid))}
              v={lib.version(fid)}
              active={fid === activeId}
              isSel={selected?.has(fid) ?? false}
              isMarked={marked?.has(fid) ?? false}
              left={it.start}
              top={top}
              cellW={cellW}
              cellH={cellH}
              scenePrefix={scenePrefix}
              badge={badge}
              onPick={pick}
              onRate={onRate ? rate : undefined}
            />
          );
        })}
      </div>
    </div>
  );
}

interface CellProps {
  fid: number;
  e: RawImageEntry | undefined;
  /** `entrySig(e)` taken at render time (an entry may be mutated in place, so the object itself cannot be diffed). */
  sig: string;
  v: number;
  active: boolean;
  isSel: boolean;
  isMarked: boolean;
  left: number;
  top: number;
  cellW: number;
  cellH: number;
  scenePrefix: string;
  badge?: (id: number) => React.ReactNode;
  onPick: (id: number, e: React.MouseEvent) => void;
  onRate?: (id: number, rating: number) => void;
}

/** One filmstrip cell. Memoised: stepping the active photo re-renders only the two cells whose state changed. */
const FilmCell = memo(FilmCellImpl, cellPropsEqual);

/** Entries are compared by what the cell draws (the backend / mock may update them in place). */
function entrySig(e: RawImageEntry | undefined): string {
  if (!e) return "";
  const t = e.thumbnail;
  return `${e.id}|${e.pick}|${e.pickOrigin}|${e.colorLabel}|${e.rating}|${e.sceneId}|${e.isSceneAnchor}|${e.hasEdits}|${t.status}|${t.status === "ready" ? t.path : ""}|${e.editedPreview?.thumbUrl ?? ""}`;
}
function cellPropsEqual(a: CellProps, b: CellProps): boolean {
  for (const k of Object.keys(a) as (keyof CellProps)[]) if (k !== "e" && a[k] !== b[k]) return false;
  return true;
}

function FilmCellImpl({ fid, e, v, active, isSel, isMarked, left, top, cellW, cellH, scenePrefix, badge, onPick, onRate }: CellProps) {
  const t = e?.thumbnail;
  return (
  <div
                  role="button"
    tabIndex={-1}
    data-testid={`film-${fid}`}
    data-active={active}
    data-selected={isSel}
    data-marked={isMarked}
    onClick={(ev) => onPick(fid, ev)}
    className={`absolute cursor-pointer overflow-hidden rounded bg-neutral-800 ${isSel ? "ring-2 ring-sky-500" : ""} ${isMarked ? "ring-2 ring-amber-400" : ""} ${active ? "outline outline-2 outline-white" : ""} ${e?.pick === "reject" && !active ? "opacity-50" : ""}`}
    style={{ left, top, width: cellW, height: cellH }}
  >
    {t?.status === "ready" && <img src={thumbSrc(e, v)!} alt="" draggable={false} className="size-full object-cover" />}
    {e && (e.pick !== "unflagged" || e.colorLabel) && (
      <span className="absolute left-0.5 top-0.5 flex items-center gap-0.5" data-testid={`film-flag-${fid}`} data-pick={e.pick}>
        {e.pick === "pick" && (
          <span title={flagTitle(e)} aria-label={flagTitle(e)}>
            <Flag className="size-3 fill-green-500 text-green-500" />
          </span>
        )}
        {e.pick === "reject" && (
          <span title={flagTitle(e)} aria-label={flagTitle(e)}>
            <X className="size-3.5 text-red-500" strokeWidth={3} />
          </span>
        )}
        {e.colorLabel && <span className={`size-2 rounded-full ${LABEL_COLOR[e.colorLabel]}`} title={`Color label: ${e.colorLabel}`} aria-label={`Color label ${e.colorLabel}`} />}
      </span>
    )}
    {e && e.rating > 0 && (
      <span className="absolute bottom-0.5 right-0.5 rounded bg-black/70 px-0.5" title={`Rated ${e.rating} star${e.rating === 1 ? "" : "s"}`} data-testid={`film-rating-${fid}`} data-rating={e.rating}>
        <Stars n={e.rating} className="size-2.5" onRate={onRate && ((r) => onRate(fid, r))} testId={`film-stars-${fid}`} />
      </span>
    )}
    {badge?.(fid)}
    {e && e.sceneId != null && (
      <span className="absolute bottom-0.5 left-0.5">
        <SceneBadge entry={e} testPrefix={scenePrefix} />
      </span>
    )}
    {e?.hasEdits && <span className="absolute right-0.5 top-0.5 size-2 rounded-full bg-sky-400" title="Edited: this photo has develop adjustments" aria-label="Edited" data-testid={`film-edited-${fid}`} />}
  </div>
  );
}
