// Guided Upright tool: draw up to 4 lines along things that should be straight (the photo is shown without its transform).
// Guides are stored in the sensor frame (un-oriented, 0..1); the overlay works in displayed (oriented) fractions.
import { useRef, useState } from "react";
import { X } from "lucide-react";
import type { UprightGuide } from "../../ipc";
import { guideFromDisplay, guideToDisplay, MAX_GUIDES } from "../../lib/transform";

interface Props {
  guides: UprightGuide[];
  /** Viewport size and the shown frame's aspect (w / h): the frame is letterboxed inside (`object-contain`). */
  size: { w: number; h: number };
  imageAspect: number;
  orientation: number;
  onChange: (g: UprightGuide[]) => void;
}

type Drag = { i: number; end: "start" | "end"; draw: boolean };
const clamp01 = (v: number) => Math.min(1, Math.max(0, v));

export function GuideOverlay({ guides, size, imageAspect, orientation, onChange }: Props) {
  const box = useRef<HTMLDivElement>(null);
  const [draft, setDraft] = useState<UprightGuide[] | null>(null);
  const draftRef = useRef<UprightGuide[] | null>(null);
  const drag = useRef<Drag | null>(null);
  if (size.w <= 0 || size.h <= 0 || !(imageAspect > 0)) return null;
  const iw = Math.min(size.w, size.h * imageAspect);
  const ih = iw / imageAspect;
  const shown = draft ?? guides;
  const px = (g: UprightGuide) => {
    const a = guideToDisplay(orientation, g.start);
    const b = guideToDisplay(orientation, g.end);
    return { x1: a.x * iw, y1: a.y * ih, x2: b.x * iw, y2: b.y * ih };
  };
  const frac = (e: { clientX: number; clientY: number }) => {
    const r = box.current!.getBoundingClientRect();
    return guideFromDisplay(orientation, { x: clamp01((e.clientX - r.left) / r.width), y: clamp01((e.clientY - r.top) / r.height) });
  };
  const put = (d: UprightGuide[] | null) => {
    draftRef.current = d;
    setDraft(d);
  };
  const downNew = (e: React.PointerEvent) => {
    if (e.button !== 0 || guides.length >= MAX_GUIDES) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    const p = frac(e);
    drag.current = { i: guides.length, end: "end", draw: true };
    put([...guides, { start: p, end: p }]);
  };
  const downEnd = (i: number, end: "start" | "end") => (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = { i, end, draw: false };
    put(guides.map((g) => ({ ...g })));
  };
  const move = (e: React.PointerEvent) => {
    const d = drag.current;
    const cur = draftRef.current;
    if (!d || !cur) return;
    const p = frac(e);
    put(cur.map((g, j) => (j === d.i ? { ...g, [d.end]: p } : g)));
  };
  const up = () => {
    const d = drag.current;
    const cur = draftRef.current;
    drag.current = null;
    put(null);
    if (!d || !cur) return;
    const g = cur[d.i];
    const a = px(g);
    const long = Math.hypot(a.x2 - a.x1, a.y2 - a.y1) >= 8;
    if (d.draw && !long) return; // a click, not a line
    if (JSON.stringify(cur) !== JSON.stringify(guides) && long) onChange(cur);
  };

  return (
    <div className="pointer-events-none absolute inset-0 z-10" data-testid="guide-overlay" data-count={guides.length}>
      <div
        ref={box}
        className="pointer-events-auto absolute touch-none cursor-crosshair"
        style={{ left: (size.w - iw) / 2, top: (size.h - ih) / 2, width: iw, height: ih }}
        data-testid="guide-frame"
        onPointerDown={downNew}
        onPointerMove={move}
        onPointerUp={up}
        onPointerCancel={up}
      >
        <svg className="pointer-events-none absolute inset-0 size-full" viewBox={`0 0 ${iw} ${ih}`}>
          {shown.map((g, i) => {
            const l = px(g);
            return (
              <g key={i} data-testid={`guide-line-${i}`}>
                <line {...l} stroke="black" strokeWidth={3} />
                <line {...l} stroke="#38bdf8" strokeWidth={1.5} />
              </g>
            );
          })}
        </svg>
        {shown.map((g, i) => {
          const l = px(g);
          const end = (which: "start" | "end", x: number, y: number) => (
            <div
              key={which}
              className="absolute size-3 -translate-x-1/2 -translate-y-1/2 cursor-move touch-none rounded-full border border-neutral-900 bg-sky-300"
              style={{ left: x, top: y }}
              data-testid={`guide-handle-${i}-${which}`}
              onPointerDown={downEnd(i, which)}
              onPointerMove={move}
              onPointerUp={up}
              onPointerCancel={up}
            />
          );
          return (
            <div key={i}>
              {end("start", l.x1, l.y1)}
              {end("end", l.x2, l.y2)}
              {!draft && (
                <button
                  className="absolute flex size-4 -translate-x-1/2 -translate-y-1/2 items-center justify-center rounded-full bg-black/70 text-white hover:bg-red-700"
                  style={{ left: (l.x1 + l.x2) / 2, top: (l.y1 + l.y2) / 2 }}
                  title="Delete this guide"
                  aria-label={`Delete guide ${i + 1}`}
                  data-testid={`guide-delete-${i}`}
                  onPointerDown={(e) => e.stopPropagation()}
                  onClick={() => onChange(guides.filter((_, j) => j !== i))}
                >
                  <X className="size-3" />
                </button>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
