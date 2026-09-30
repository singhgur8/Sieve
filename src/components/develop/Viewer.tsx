import { useEffect, useRef } from "react";
import { Loader2 } from "lucide-react";
import type { NormRect } from "../../ipc";
import type { RenderView } from "../../hooks/useEditor";

export interface Zoom {
  on: boolean;
  /** Centre of the visible area, normalised 0..1 of the full frame. */
  cx: number;
  cy: number;
}

export interface Size {
  w: number;
  h: number;
}

/** Top-left of the full-resolution frame inside the viewport (centres when smaller, clamps when larger). */
export function frameOffset(z: Zoom, size: Size, fw: number, fh: number) {
  const clamp = (pos: number, vp: number, d: number) => (d <= vp ? (vp - d) / 2 : Math.min(0, Math.max(vp - d, pos)));
  return { x: clamp(size.w / 2 - z.cx * fw, size.w, fw), y: clamp(size.h / 2 - z.cy * fh, size.h, fh) };
}

/** Normalised rectangle of the frame visible at 100%. */
export function visibleRegion(z: Zoom, size: Size, fw: number, fh: number): NormRect | null {
  if (!z.on || !fw || !fh || !size.w) return null;
  const o = frameOffset(z, size, fw, fh);
  const x = Math.max(0, -o.x / fw);
  const y = Math.max(0, -o.y / fh);
  const round = (n: number) => Math.round(n * 10000) / 10000;
  return { x: round(x), y: round(y), width: round(Math.min(1 - x, size.w / fw)), height: round(Math.min(1 - y, size.h / fh)) };
}

interface Props {
  main: RenderView | null;
  before: RenderView | null;
  detail: RenderView | null;
  detailRegion: NormRect | null;
  showBefore: boolean;
  split: boolean;
  splitPos: number;
  onSplitPos: (p: number) => void;
  zoom: Zoom;
  size: Size;
  fw: number;
  fh: number;
  loading: boolean;
  onSize: (s: Size) => void;
  onPan: (z: Zoom) => void;
  onPanEnd: () => void;
  onToggleZoom: (at?: { x: number; y: number }) => void;
}

const img = "pointer-events-none absolute select-none";

export function Viewer(p: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const drag = useRef<{ sx: number; sy: number; cx: number; cy: number } | null>(null);
  const splitDrag = useRef(false);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const report = () => p.onSize({ w: el.clientWidth, h: el.clientHeight });
    report();
    const ro = new ResizeObserver(report);
    ro.observe(el);
    return () => ro.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const zoomed = p.zoom.on && p.fw > 0;
  const off = frameOffset(p.zoom, p.size, p.fw, p.fh);

  const onDown = (e: React.PointerEvent) => {
    if (!zoomed) return;
    drag.current = { sx: e.clientX, sy: e.clientY, cx: p.zoom.cx, cy: p.zoom.cy };
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const onMove = (e: React.PointerEvent) => {
    if (splitDrag.current) {
      const r = ref.current!.getBoundingClientRect();
      p.onSplitPos(Math.min(0.98, Math.max(0.02, (e.clientX - r.left) / r.width)));
      return;
    }
    const d = drag.current;
    if (!d) return;
    p.onPan({ on: true, cx: d.cx - (e.clientX - d.sx) / p.fw, cy: d.cy - (e.clientY - d.sy) / p.fh });
  };
  const onUp = () => {
    splitDrag.current = false;
    if (drag.current) {
      drag.current = null;
      p.onPanEnd();
    }
  };

  const shown = p.showBefore ? p.before : p.main;
  const alt = (v: RenderView | null) => (v ? `Render ${v.imageId} #${v.seq}` : "");
  return (
    <div
      ref={ref}
      data-testid="viewer"
      data-zoomed={zoomed}
      className={`relative size-full overflow-hidden bg-black ${zoomed ? "cursor-grab active:cursor-grabbing" : ""}`}
      onPointerDown={onDown}
      onPointerMove={onMove}
      onPointerUp={onUp}
      onDoubleClick={(e) => {
        const r = ref.current!.getBoundingClientRect();
        p.onToggleZoom({ x: (e.clientX - r.left) / r.width, y: (e.clientY - r.top) / r.height });
      }}
    >
      {zoomed ? (
        <div className="absolute" style={{ left: off.x, top: off.y, width: p.fw, height: p.fh }}>
          {shown && <img src={shown.url} alt={alt(shown)} draggable={false} className={`${img} inset-0 size-full`} data-testid="view-main" />}
          {!p.showBefore && p.detail && p.detailRegion && (
            <img
              src={p.detail.url}
              alt={alt(p.detail)}
              draggable={false}
              data-testid="view-detail"
              className={img}
              style={{ left: p.detailRegion.x * p.fw, top: p.detailRegion.y * p.fh, width: p.detailRegion.width * p.fw, height: p.detailRegion.height * p.fh }}
            />
          )}
        </div>
      ) : p.split && p.before && p.main ? (
        <>
          <img src={p.main.url} alt={alt(p.main)} draggable={false} className={`${img} inset-0 size-full object-contain`} data-testid="view-main" />
          <img
            src={p.before.url}
            alt={alt(p.before)}
            draggable={false}
            className={`${img} inset-0 size-full object-contain`}
            style={{ clipPath: `inset(0 ${(1 - p.splitPos) * 100}% 0 0)` }}
            data-testid="view-before"
          />
          <div
            className="absolute inset-y-0 w-3 -translate-x-1/2 cursor-col-resize"
            style={{ left: `${p.splitPos * 100}%` }}
            onPointerDown={(e) => {
              e.stopPropagation();
              splitDrag.current = true;
              ref.current!.setPointerCapture(e.pointerId);
            }}
            data-testid="split-handle"
          >
            <div className="mx-auto h-full w-px bg-white/80" />
          </div>
          <span className="pointer-events-none absolute left-2 top-2 rounded bg-black/60 px-1.5 text-[10px] text-white">Before</span>
          <span className="pointer-events-none absolute right-2 top-2 rounded bg-black/60 px-1.5 text-[10px] text-white">After</span>
        </>
      ) : (
        shown && <img src={shown.url} alt={alt(shown)} draggable={false} className={`${img} inset-0 size-full object-contain`} data-testid={p.showBefore ? "view-before" : "view-main"} />
      )}
      {p.showBefore && <span className="pointer-events-none absolute left-2 top-2 rounded bg-black/60 px-1.5 text-xs text-white" data-testid="before-badge">Before</span>}
      {p.loading && !shown && (
        <div className="absolute inset-0 flex items-center justify-center">
          <Loader2 className="size-6 animate-spin text-neutral-400" />
        </div>
      )}
    </div>
  );
}

/**
 * Screen box (viewer px) of the displayed, cropped frame: the fitted image (`object-contain` of the render's aspect)
 * or the 100% frame. Null until something is rendered. Used by the masking layer to place overlays and tools.
 */
export function frameBox(z: Zoom, size: Size, fw: number, fh: number, view: { width: number; height: number } | null): { x: number; y: number; w: number; h: number } | null {
  if (!size.w || !size.h) return null;
  if (z.on && fw > 0 && fh > 0) {
    const o = frameOffset(z, size, fw, fh);
    return { x: o.x, y: o.y, w: fw, h: fh };
  }
  if (!view || !view.width || !view.height) return null;
  const s = Math.min(size.w / view.width, size.h / view.height);
  const w = view.width * s;
  const h = view.height * s;
  return { x: (size.w - w) / 2, y: (size.h - h) / 2, w, h };
}
