// Crop tool overlay: dimmed outside, rule-of-thirds grid, 8 resize handles and a movable body over the fitted frame.
import { useRef } from "react";
import { HANDLES, ASPECTS, fitRatio, fractionRatio, lastLockedAspect, moveRect, resizeRect, saveCropAspect, type AspectId, type Handle, type Rect } from "../../lib/crop";

export interface CropTool {
  rect: Rect;
  /** Straighten angle in degrees (`crs:CropAngle`), applied on commit. */
  angle: number;
  aspect: AspectId;
  /** Swap width and height of the aspect ratio (portrait <-> landscape). */
  flip: boolean;
  /** Pixel ratio (w / h) of aspect "custom". */
  customRatio?: number;
}

/** Fraction-unit ratio (w / h) that the tool's aspect preset locks the rect to, or null when free. */
export function lockRatio(tool: Pick<CropTool, "aspect" | "flip" | "customRatio">, imageAspect: number): number | null {
  if (tool.aspect === "free") return null;
  const base = tool.aspect === "original" ? imageAspect : tool.aspect === "custom" ? (tool.customRatio ?? imageAspect) : ASPECTS.find((a) => a.id === tool.aspect)!.ratio!;
  return fractionRatio(tool.flip ? 1 / base : base, imageAspect);
}

/** Re-fits the rectangle to the tool's locked ratio (no-op when free). */
export function refit(tool: CropTool, imageAspect: number): CropTool {
  const fr = lockRatio(tool, imageAspect);
  return fr == null ? tool : { ...tool, rect: fitRatio(tool.rect, fr) };
}

/** X / the swap button: landscape <-> portrait. A free rectangle is first locked to its current ratio. */
export function swapTool(tool: CropTool, imageAspect: number): CropTool {
  if (tool.aspect === "free") {
    const r = tool.rect;
    const ratio = ((r.r - r.l) / (r.b - r.t)) * imageAspect;
    return refit({ ...tool, aspect: "custom", customRatio: ratio, flip: true }, imageAspect);
  }
  return refit({ ...tool, flip: !tool.flip }, imageAspect);
}

/** A: Free <-> the last locked aspect (Original by default). */
export function toggleLockTool(tool: CropTool, imageAspect: number): CropTool {
  if (tool.aspect === "free") {
    return refit({ ...tool, aspect: lastLockedAspect(), customRatio: undefined }, imageAspect);
  }
  saveCropAspect(tool.aspect);
  saveCropAspect("free");
  return { ...tool, aspect: "free" };
}

interface Props {
  tool: CropTool;
  /** Viewport size and the rendered frame's aspect ratio (w / h): the frame is letterboxed inside (`object-contain`). */
  size: { w: number; h: number };
  imageAspect: number;
  onChange: (t: CropTool) => void;
}

const HANDLE_POS: Record<Handle, { x: number; y: number; cursor: string }> = {
  nw: { x: 0, y: 0, cursor: "nwse-resize" },
  n: { x: 0.5, y: 0, cursor: "ns-resize" },
  ne: { x: 1, y: 0, cursor: "nesw-resize" },
  e: { x: 1, y: 0.5, cursor: "ew-resize" },
  se: { x: 1, y: 1, cursor: "nwse-resize" },
  s: { x: 0.5, y: 1, cursor: "ns-resize" },
  sw: { x: 0, y: 1, cursor: "nesw-resize" },
  w: { x: 0, y: 0.5, cursor: "ew-resize" },
};

export function CropOverlay({ tool, size, imageAspect, onChange }: Props) {
  const box = useRef<HTMLDivElement>(null);
  const drag = useRef<{ kind: Handle | "move"; x: number; y: number; rect: Rect } | null>(null);
  if (size.w <= 0 || size.h <= 0 || !(imageAspect > 0)) return null;
  const iw = Math.min(size.w, size.h * imageAspect);
  const ih = iw / imageAspect;
  const left = (size.w - iw) / 2;
  const top = (size.h - ih) / 2;
  const { rect } = tool;
  const fr = lockRatio(tool, imageAspect);

  const frac = (e: { clientX: number; clientY: number }): [number, number] => {
    const r = box.current!.getBoundingClientRect();
    return [(e.clientX - r.left) / r.width, (e.clientY - r.top) / r.height];
  };
  const down = (kind: Handle | "move") => (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    e.currentTarget.setPointerCapture(e.pointerId);
    const [x, y] = frac(e);
    drag.current = { kind, x, y, rect };
  };
  const move = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    const [x, y] = frac(e);
    onChange({ ...tool, rect: d.kind === "move" ? moveRect(d.rect, x - d.x, y - d.y) : resizeRect(d.rect, d.kind, x, y, fr) });
  };
  const up = () => {
    drag.current = null;
  };

  const pct = (v: number) => `${v * 100}%`;
  const dim = "pointer-events-none absolute bg-black/60";
  return (
    <div className="pointer-events-none absolute inset-0 z-10" data-testid="crop-overlay">
      <div ref={box} className="absolute" style={{ left, top, width: iw, height: ih }} data-testid="crop-frame">
        <div className={dim} style={{ left: 0, top: 0, width: "100%", height: pct(rect.t) }} />
        <div className={dim} style={{ left: 0, top: pct(rect.b), width: "100%", height: pct(1 - rect.b) }} />
        <div className={dim} style={{ left: 0, top: pct(rect.t), width: pct(rect.l), height: pct(rect.b - rect.t) }} />
        <div className={dim} style={{ left: pct(rect.r), top: pct(rect.t), width: pct(1 - rect.r), height: pct(rect.b - rect.t) }} />
        <div
          className="pointer-events-auto absolute cursor-move border border-white/90"
          style={{ left: pct(rect.l), top: pct(rect.t), width: pct(rect.r - rect.l), height: pct(rect.b - rect.t) }}
          data-testid="crop-rect"
          data-rect={JSON.stringify({ l: +rect.l.toFixed(4), t: +rect.t.toFixed(4), r: +rect.r.toFixed(4), b: +rect.b.toFixed(4) })}
          onPointerDown={down("move")}
          onPointerMove={move}
          onPointerUp={up}
          onPointerCancel={up}
          onDoubleClick={(e) => e.stopPropagation()}
        >
          {[1 / 3, 2 / 3].map((f) => (
            <div key={f}>
              <div className="pointer-events-none absolute inset-y-0 w-px bg-white/30" style={{ left: pct(f) }} />
              <div className="pointer-events-none absolute inset-x-0 h-px bg-white/30" style={{ top: pct(f) }} />
            </div>
          ))}
          {HANDLES.map((h) => (
            <div
              key={h}
              className="absolute size-3 -translate-x-1/2 -translate-y-1/2 touch-none rounded-sm border border-neutral-900 bg-white"
              style={{ left: pct(HANDLE_POS[h].x), top: pct(HANDLE_POS[h].y), cursor: HANDLE_POS[h].cursor }}
              data-testid={`crop-handle-${h}`}
              onPointerDown={down(h)}
              onPointerMove={move}
              onPointerUp={up}
              onPointerCancel={up}
            />
          ))}
        </div>
      </div>
    </div>
  );
}
