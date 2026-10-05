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
  type OverlayId,
  type Rect,
  FULL,
  rectArea,
} from "../../lib/crop";
import { ORIENT_TRANSFORM, overlayShapes } from "../../lib/overlays";

export interface CropTool {
  rect: Rect;
  /**
   * The rectangle the user intends (set by start / resize / move / aspect / reset). With "constrain to image" the shown
   * `rect` is always derived from it for the current angle, so turning back toward 0 grows the crop back (Lightroom).
   */
  base: Rect;
  /** Lightroom "Constrain to image" (default on): the rectangle never leaves the rotated image. */
  constrain: boolean;
  overlay: OverlayId;
  /** Shift+O: orientation 0..3 of the asymmetric overlays. */
  overlayOrient: number;
  /** The angle tool is armed: the next drag draws a line along the horizon. */
  angleTool?: boolean;
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
  const rot = tool.constrain ? previewRotation(tool.angle, orientation) : 0;
  const rect = rot ? fitInsideRotated(tool.base, imageAspect, rot) : tool.base;
  return rectEq(rect, tool.rect) ? tool : { ...tool, rect };
}

const rectEq = (a: Rect, b: Rect) => a.l === b.l && a.t === b.t && a.r === b.r && a.b === b.b;

/** A fresh tool: `rect` is both the shown and the intended rectangle. */
export function newTool(rect: Rect, angle: number, aspect: AspectId, overlay: OverlayId): CropTool {
  return { rect, base: rect, angle, aspect, flip: false, constrain: true, overlay, overlayOrient: 0 };
}

/** Sets the intended rectangle (the shown one follows via `constrainTool`). */
export const withBase = (tool: CropTool, rect: Rect): CropTool => ({ ...tool, rect, base: rect });

/** Reset: full frame, no angle, locked to Original (the overlay choice and Constrain stay). */
export const resetTool = (tool: CropTool): CropTool => ({ ...tool, rect: FULL, base: FULL, angle: 0, aspect: "original", flip: false, customRatio: undefined, rotating: false, angleTool: false });

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
  const base = tool.aspect === "original" || tool.aspect === "asShot" ? imageAspect : tool.aspect === "custom" ? (tool.customRatio ?? imageAspect) : ASPECTS.find((a) => a.id === tool.aspect)!.ratio!;
  return fractionRatio(tool.flip ? 1 / base : base, imageAspect);
}

/** Re-fits the rectangle to the tool's locked ratio (no-op when free). */
export function refit(tool: CropTool, imageAspect: number, prev?: Pick<CropTool, "aspect" | "flip" | "customRatio">): CropTool {
  const fr = lockRatio(tool, imageAspect);
  if (fr == null) return tool;
  // A rectangle that was the largest of the previous ratio re-maximises (so X twice returns to the same crop).
  const pfr = prev ? lockRatio(prev, imageAspect) : null;
  const wasMax = pfr != null && rectArea(tool.base) >= 0.98 * rectArea(fitRatio(FULL, pfr)) && !tool.angle;
  return withBase(tool, fitRatio(wasMax ? FULL : tool.base, fr));
}

/** X / the swap button: landscape <-> portrait. A free rectangle is first locked to its current ratio. */
export function swapTool(tool: CropTool, imageAspect: number): CropTool {
  if (tool.aspect === "free") {
    const r = tool.rect;
    const ratio = ((r.r - r.l) / (r.b - r.t)) * imageAspect;
    return refit({ ...tool, aspect: "custom", customRatio: ratio, flip: true }, imageAspect);
  }
  return refit({ ...tool, flip: !tool.flip }, imageAspect, tool);
}

/** Aspect preset chosen from the list. Custom starts from the current rectangle's pixel ratio (editable as W : H). */
export function setAspectTool(tool: CropTool, id: AspectId, imageAspect: number): CropTool {
  saveCropAspect(id);
  if (id === "custom") {
    const r = tool.rect;
    const ratio = tool.aspect === "custom" ? (tool.customRatio ?? imageAspect) : ((r.r - r.l) / (r.b - r.t)) * imageAspect;
    return refit({ ...tool, aspect: "custom", flip: false, customRatio: ratio }, imageAspect, tool);
  }
  return refit({ ...tool, aspect: id, customRatio: undefined }, imageAspect, tool);
}

/** Custom W : H entered by the user (pixel ratio). */
export function setCustomRatio(tool: CropTool, w: number, h: number, imageAspect: number): CropTool {
  if (!(w > 0) || !(h > 0)) return tool;
  return refit({ ...tool, aspect: "custom", flip: false, customRatio: w / h }, imageAspect);
}

/** A: Free <-> the last locked aspect (Original by default). */
export function toggleLockTool(tool: CropTool, imageAspect: number): CropTool {
  if (tool.aspect === "free") {
    return refit({ ...tool, aspect: lastLockedAspect(), customRatio: undefined }, imageAspect, tool);
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
  /**
   * Seam for the Upright / Transform warp outline (v19.3, R1-3): the valid image area as 4 points, fractions of the corrected
   * frame (clockwise), or null / undefined = the rotated frame only. "Constrain to image" and the paper fill will use it.
   */
  validQuad?: [number, number][] | null;
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

/** Alt-drag on a handle: the opposite side mirrors the dragged one about the centre. */
function resizeCentred(rect: Rect, handle: Handle, px: number, py: number, fr: number | null): Rect {
  const cx = (rect.l + rect.r) / 2;
  const cy = (rect.t + rect.b) / 2;
  const hx = handle.includes("w") ? -1 : handle.includes("e") ? 1 : 0;
  const hy = handle.includes("n") ? -1 : handle.includes("s") ? 1 : 0;
  const ax = hx < 0 ? rect.r : hx > 0 ? rect.l : cx;
  const ay = hy < 0 ? rect.b : hy > 0 ? rect.t : cy;
  const n = resizeRect(rect, handle, hx ? ax + 2 * (px - cx) : px, hy ? ay + 2 * (py - cy) : py, fr);
  const w = n.r - n.l;
  const h = n.b - n.t;
  const s = Math.min(1, (2 * Math.min(cx, 1 - cx)) / w, (2 * Math.min(cy, 1 - cy)) / h);
  return { l: cx - (w * s) / 2, r: cx + (w * s) / 2, t: cy - (h * s) / 2, b: cy + (h * s) / 2 };
}

const deg = (rad: number) => (rad * 180) / Math.PI;
/** Folds an angle difference into (-180, 180]. */
const wrap = (d: number) => ((((d + 180) % 360) + 360) % 360) - 180;

type P = { x: number; y: number };

export function CropOverlay({ tool, size, imageAspect, orientation = 1, onChange, validQuad = null }: Props) {
  const box = useRef<HTMLDivElement>(null);
  const layer = useRef<HTMLDivElement>(null);
  const drag = useRef<{ kind: Handle | "move"; x: number; y: number; rect: Rect } | null>(null);
  const spin = useRef<{ mode: "rotate" | "line"; a0: number; rot0: number; start: P; c: P } | null>(null);
  const [line, setLine] = useState<{ a: P; b: P } | null>(null);
  const [meta, setMeta] = useState(false);
  const armed = !!tool.angleTool;
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
    // Lightroom modifiers: Alt resizes about the centre, Shift keeps the current ratio of a free crop for this drag.
    const keep = e.shiftKey && fr == null ? ((d.rect.r - d.rect.l) / Math.max(1e-6, d.rect.b - d.rect.t)) : fr;
    const next = d.kind === "move" ? moveRect(d.rect, x - d.x, y - d.y) : e.altKey ? resizeCentred(d.rect, d.kind, x, y, keep) : resizeRect(d.rect, d.kind, x, y, keep);
    onChange(withBase(tool, approach(d.rect, next, imageAspect, tool.constrain ? rot : 0)));
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
    onChange({ ...tool, angle, rotating }); // the shown rect is re-derived from `base` by the caller (constrainTool)
  };
  const spinDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    const p = lp(e);
    if (e.metaKey || e.ctrlKey || armed) {
      spin.current = { mode: "line", a0: 0, rot0: rot, start: p, c: p };
      setLine({ a: p, b: p });
    } else {
      const c = rectCentre();
      spin.current = { mode: "rotate", a0: deg(Math.atan2(p.y - c.y, p.x - c.x)), rot0: rot, start: p, c };
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
      const c = s.c; // fixed at drag start: the rect centre slides while the crop re-fits
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
      if (Math.hypot(p.x - s.start.x, p.y - s.start.y) >= 6) {
        const r = levelRotation(s.start, p, s.rot0);
        const angle = Math.round(Math.max(-45, Math.min(45, angleFromRotation(r, orientation))) * 100) / 100;
        onChange({ ...tool, angle, rotating: false, angleTool: false });
      } else if (armed) onChange({ ...tool, angleTool: false });
    } else onChange({ ...tool, rotating: false });
  };

  const pct = (v: number) => `${v * 100}%`;
  const dim = "pointer-events-none absolute bg-black/60";
  const grid = [1, 2, 3, 4, 5, 6, 7, 8, 9].map((i) => i / 10);
  return (
    <div className="pointer-events-none absolute inset-0 z-10" data-testid="crop-overlay" data-rotation={rot.toFixed(2)} data-valid-quad={validQuad ? JSON.stringify(validQuad) : undefined}>
      <div
        ref={layer}
        className="pointer-events-auto absolute -inset-6 touch-none"
        style={{ cursor: meta || armed ? "crosshair" : ROTATE_CURSOR, zIndex: meta || armed ? 30 : 0 }}
        data-cmd={meta || armed}
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
        {!tool.constrain && rot !== 0 && <PaperFill iw={iw} ih={ih} rot={rot} />}
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
          {tool.rotating ? (
            grid.map((f) => (
              <div key={f} data-testid="crop-grid-line">
                <div className="pointer-events-none absolute inset-y-0 w-px bg-white/45" style={{ left: pct(f) }} />
                <div className="pointer-events-none absolute inset-x-0 h-px bg-white/45" style={{ top: pct(f) }} />
              </div>
            ))
          ) : (
            <Guide id={tool.overlay} orient={tool.overlayOrient} pa={(iw * (rect.r - rect.l)) / Math.max(1, ih * (rect.b - rect.t))} />
          )}
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

/** The composition guide inside the crop rectangle (vector, non-scaling 1px strokes). */
function Guide({ id, orient, pa }: { id: OverlayId; orient: number; pa: number }) {
  const { lines, paths } = overlayShapes(id, pa);
  return (
    <svg className="pointer-events-none absolute inset-0 size-full" viewBox="0 0 1 1" preserveAspectRatio="none" data-testid="crop-guide" data-overlay={id} data-orient={orient}>
      <g transform={ORIENT_TRANSFORM[orient % 4]} stroke="rgba(255,255,255,0.55)" strokeWidth={1} fill="none" vectorEffect="non-scaling-stroke">
        {lines.map((l, i) => (
          <line key={i} x1={l[0]} y1={l[1]} x2={l[2]} y2={l[3]} vectorEffect="non-scaling-stroke" />
        ))}
        {paths.map((d, i) => (
          <path key={i} d={d} vectorEffect="non-scaling-stroke" />
        ))}
      </g>
    </svg>
  );
}

/**
 * "Constrain to image" off: the part of the frame the rotated photo does not cover is white, as in the render
 * (the backend fills the gap white). Frame rectangle minus the rotated photo quad (even-odd).
 */
function PaperFill({ iw, ih, rot }: { iw: number; ih: number; rot: number }) {
  const a = (rot * Math.PI) / 180;
  const cos = Math.cos(a);
  const sin = Math.sin(a);
  const quad = [
    [-iw / 2, -ih / 2],
    [iw / 2, -ih / 2],
    [iw / 2, ih / 2],
    [-iw / 2, ih / 2],
  ]
    .map(([x, y]) => `${(iw / 2 + x * cos - y * sin).toFixed(2)},${(ih / 2 + x * sin + y * cos).toFixed(2)}`)
    .join(" L");
  return (
    <svg className="pointer-events-none absolute inset-0 size-full" viewBox={`0 0 ${iw} ${ih}`} data-testid="crop-paper">
      <path fillRule="evenodd" fill="white" d={`M0,0 H${iw} V${ih} H0 Z M${quad} Z`} />
    </svg>
  );
}
