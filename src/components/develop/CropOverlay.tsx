// Crop tool overlay: dimmed outside, rule-of-thirds grid, 8 resize handles and a movable body over the fitted frame.
// Straightening (Lightroom parity): dragging outside the rectangle rotates the photo about the crop centre, Cmd/Ctrl-drag
// draws a line that is levelled, and the rectangle stays axis-aligned and inside the rotated image.
import { useEffect, useRef, useState } from "react";
import {
  HANDLES,
  ASPECTS,
  angleFromRotation,
  fitInsideRotated,
  fitRatio,
  fractionRatio,
  insideRotated,
  lastLockedAspect,
  moveRect,
  previewRotation,
  resizeRect,
  saveCropAspect,
  type AspectId,
  type Handle,
  type Rect,
} from "../../lib/crop";

export interface CropTool {
  rect: Rect;
  /** Straighten angle in degrees (`crs:CropAngle`), applied on commit. */
  angle: number;
  aspect: AspectId;
  /** Swap width and height of the aspect ratio (portrait <-> landscape). */
  flip: boolean;
  /** Pixel ratio (w / h) of aspect "custom". */
  customRatio?: number;
  /** True while the angle is being dragged (slider, rotate-by-drag): the fine grid shows. */
  rotating?: boolean;
}

/** Keeps the tool's rectangle inside the straightened (rotated) image: Lightroom "constrain to image". */
export function constrainTool(tool: CropTool, imageAspect: number, orientation: number): CropTool {
  const rot = previewRotation(tool.angle, orientation);
  if (!rot) return tool;
  const rect = fitInsideRotated(tool.rect, imageAspect, rot);
  return rect === tool.rect ? tool : { ...tool, rect };
}

/** Largest step from `from` (valid) towards `to` that keeps the rect inside the rotated image (bisection; keeps a locked ratio). */
function approach(from: Rect, to: Rect, aspect: number, rot: number): Rect {
  if (!rot || insideRotated(to, aspect, rot)) return to;
  const at = (t: number): Rect => ({ l: from.l + (to.l - from.l) * t, t: from.t + (to.t - from.t) * t, r: from.r + (to.r - from.r) * t, b: from.b + (to.b - from.b) * t });
  let lo = 0;
  let hi = 1;
  for (let i = 0; i < 20; i++) {
    const m = (lo + hi) / 2;
    if (insideRotated(at(m), aspect, rot)) lo = m;
    else hi = m;
  }
  return at(lo);
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
  /** EXIF orientation of the photo (a mirrored one flips the sense of the straighten rotation). */
  orientation?: number;
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

const ROTATE_CURSOR = `url("data:image/svg+xml;utf8,${encodeURIComponent(
  '<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke-linecap="round" stroke-linejoin="round"><path stroke="black" stroke-width="4" d="M20 12a8 8 0 1 1-2.5-5.8M20 3v4.5h-4.5"/><path stroke="white" stroke-width="2" d="M20 12a8 8 0 1 1-2.5-5.8M20 3v4.5h-4.5"/></svg>',
)}") 12 12, crosshair`;

const deg = (rad: number) => (rad * 180) / Math.PI;
/** Folds an angle difference into (-180, 180]. */
const wrap = (d: number) => ((((d + 180) % 360) + 360) % 360) - 180;

type P = { x: number; y: number };

export function CropOverlay({ tool, size, imageAspect, orientation = 1, onChange }: Props) {
  const box = useRef<HTMLDivElement>(null);
  const layer = useRef<HTMLDivElement>(null);
  const drag = useRef<{ kind: Handle | "move"; x: number; y: number; rect: Rect } | null>(null);
  const spin = useRef<{ mode: "rotate" | "line"; a0: number; rot0: number; start: P } | null>(null);
  const [line, setLine] = useState<{ a: P; b: P } | null>(null);
  const [meta, setMeta] = useState(false);
  // While Cmd / Ctrl is held the hit layer moves above the crop rectangle, so a straighten line can start anywhere.
  useEffect(() => {
    const sync = (e: KeyboardEvent) => setMeta(e.metaKey || e.ctrlKey);
    const off = () => setMeta(false);
    window.addEventListener("keydown", sync);
    window.addEventListener("keyup", sync);
    window.addEventListener("blur", off);
    return () => {
      window.removeEventListener("keydown", sync);
      window.removeEventListener("keyup", sync);
      window.removeEventListener("blur", off);
    };
  }, []);
  if (size.w <= 0 || size.h <= 0 || !(imageAspect > 0)) return null;
  const iw = Math.min(size.w, size.h * imageAspect);
  const ih = iw / imageAspect;
  const left = (size.w - iw) / 2;
  const top = (size.h - ih) / 2;
  const { rect } = tool;
  const fr = lockRatio(tool, imageAspect);
  const rot = previewRotation(tool.angle, orientation);

  const frac = (e: { clientX: number; clientY: number }): [number, number] => {
    const r = box.current!.getBoundingClientRect();
    return [(e.clientX - r.left) / r.width, (e.clientY - r.top) / r.height];
  };
  const down = (kind: Handle | "move") => (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    // Cmd / Ctrl-drag anywhere draws a straighten line (handled by the hit layer behind).
    if (e.metaKey || e.ctrlKey) return;
    e.stopPropagation();
    e.currentTarget.setPointerCapture(e.pointerId);
    const [x, y] = frac(e);
    drag.current = { kind, x, y, rect };
  };
  const move = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    const [x, y] = frac(e);
    const next = d.kind === "move" ? moveRect(d.rect, x - d.x, y - d.y) : resizeRect(d.rect, d.kind, x, y, fr);
    onChange({ ...tool, rect: approach(d.rect, next, imageAspect, rot) });
  };
  const up = () => {
    drag.current = null;
  };

  // ---- rotate by dragging outside the rectangle / Cmd-drag straighten line ----
  const lp = (e: { clientX: number; clientY: number }): P => {
    const r = layer.current!.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  };
  const rectCentre = (): P => {
    const r = layer.current!.getBoundingClientRect();
    const b = box.current!.getBoundingClientRect();
    return { x: b.left - r.left + ((rect.l + rect.r) / 2) * b.width, y: b.top - r.top + ((rect.t + rect.b) / 2) * b.height };
  };
  const setRotation = (r: number, rotating: boolean) => {
    const angle = Math.round(Math.max(-45, Math.min(45, angleFromRotation(r, orientation))) * 100) / 100;
    const t = { ...tool, angle, rotating };
    onChange({ ...t, rect: fitInsideRotated(t.rect, imageAspect, previewRotation(angle, orientation)) });
  };
  const spinDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    const p = lp(e);
    if (e.metaKey || e.ctrlKey) {
      spin.current = { mode: "line", a0: 0, rot0: rot, start: p };
      setLine({ a: p, b: p });
    } else {
      const c = rectCentre();
      spin.current = { mode: "rotate", a0: deg(Math.atan2(p.y - c.y, p.x - c.x)), rot0: rot, start: p };
      onChange({ ...tool, rotating: true });
    }
  };
  /** Rotation that levels a line drawn from a to b on the currently displayed (rotated) image. */
  const levelRotation = (a: P, b: P, rot0: number) => {
    let al = wrap(deg(Math.atan2(b.y - a.y, b.x - a.x)) * 2) / 2; // fold to (-90, 90]
    if (Math.abs(al) > 45) al -= Math.sign(al) * 90; // the nearer of horizontal / vertical
    return rot0 - al;
  };
  const spinMove = (e: React.PointerEvent) => {
    const s = spin.current;
    if (!s) return;
    const p = lp(e);
    if (s.mode === "line") setLine({ a: s.start, b: p });
    else {
      const c = rectCentre();
      setRotation(s.rot0 + wrap(deg(Math.atan2(p.y - c.y, p.x - c.x)) - s.a0), true);
    }
  };
  const spinUp = (e: React.PointerEvent) => {
    const s = spin.current;
    spin.current = null;
    setLine(null);
    if (!s) return;
    const p = lp(e);
    if (s.mode === "line") {
      if (Math.hypot(p.x - s.start.x, p.y - s.start.y) >= 6) setRotation(levelRotation(s.start, p, s.rot0), false);
    } else onChange({ ...tool, rotating: false });
  };

  const pct = (v: number) => `${v * 100}%`;
  const dim = "pointer-events-none absolute bg-black/60";
  const grid = tool.rotating ? [1, 2, 3, 4, 5, 6, 7, 8, 9].map((i) => i / 10) : [1 / 3, 2 / 3];
  return (
    <div className="pointer-events-none absolute inset-0 z-10" data-testid="crop-overlay" data-rotation={rot.toFixed(2)}>
      <div
        ref={layer}
        className="pointer-events-auto absolute -inset-6 touch-none"
        style={{ cursor: meta ? "crosshair" : ROTATE_CURSOR, zIndex: meta ? 30 : 0 }}
        data-cmd={meta}
        data-testid="crop-rotate-layer"
        onPointerDown={spinDown}
        onPointerMove={spinMove}
        onPointerUp={spinUp}
        onPointerCancel={spinUp}
      />
      {line && (
        <svg className="pointer-events-none absolute -inset-6 size-[calc(100%+3rem)]" data-testid="crop-straighten-line">
          <line x1={line.a.x} y1={line.a.y} x2={line.b.x} y2={line.b.y} stroke="black" strokeWidth={3} />
          <line x1={line.a.x} y1={line.a.y} x2={line.b.x} y2={line.b.y} stroke="white" strokeWidth={1.5} strokeDasharray="5 3" />
        </svg>
      )}
      <div ref={box} className="pointer-events-none absolute" style={{ left, top, width: iw, height: ih }} data-testid="crop-frame">
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
          {grid.map((f) => (
            <div key={f} data-testid={tool.rotating ? "crop-grid-line" : undefined}>
              <div className={`pointer-events-none absolute inset-y-0 w-px ${tool.rotating ? "bg-white/45" : "bg-white/30"}`} style={{ left: pct(f) }} />
              <div className={`pointer-events-none absolute inset-x-0 h-px ${tool.rotating ? "bg-white/45" : "bg-white/30"}`} style={{ top: pct(f) }} />
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
