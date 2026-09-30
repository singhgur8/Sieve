// Canvas layer over the Develop viewer for masking: mask overlay image, pins, gradient handles and the brush /
// gradient / range tools. Pointer positions become displayed-frame fractions (`screenToDisp`) and are converted to the
// sensor frame (un-oriented, uncropped) with `dispToSensor` before they reach the mask data.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { commands, unwrap, type LinearMask, type MaskGroup, type MaskShape, type NormPoint, type NormRect, type RadialMask, type RenderedMaskOverlay } from "../../ipc";
import type { Editor } from "../../hooks/useEditor";
import type { MasksApi } from "../../hooks/useMasks";
import {
  brushRadiusPx,
  clamp,
  dispRectToSensor,
  dispToScreen,
  dispToSensor,
  ellipseFromDrag,
  radialFromScreen,
  radialToScreen,
  screenToDisp,
  sensorToDisp,
  type Box,
  type Frame,
} from "../../lib/maskGeom";
import { MAX_STROKE_DABS, OVERLAY_STYLES, sizeToRadius, type OverlayStyle } from "../../lib/masks";

interface Props {
  masks: MasksApi;
  editor: Editor;
  id: number | null;
  frame: Frame | null;
  box: Box | null;
  /** Visible region of the frame while zoomed to 100% (null when fitted): the overlay is rendered for it, so it stays sharp. */
  region?: NormRect | null;
  onError: (e: unknown) => void;
}

type Pt = { x: number; y: number };
const DEG = 180 / Math.PI;
/** Overlay strength (55-60 % per the UX spec) and fade-out time. */
const OVERLAY_ALPHA = 0.58;
const OVERLAY_FADE_MS = 250;

/** L* / 100 of an sRGB pixel (the space `luminance_weight` works in). */
function lightness(px: ArrayLike<number>): number {
  const [r, g, b] = [px[0], px[1], px[2]];
  const lin = (v: number) => (v / 255) ** 2.2;
  const Y = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
  return (Y > 0.008856 ? 116 * Y ** (1 / 3) - 16 : 903.3 * Y) / 100;
}

/** Reads one pixel of `src` at (sx, sy) through a canvas; throws when the canvas is tainted. */
function readPixel(src: CanvasImageSource, sx: number, sy: number): ArrayLike<number> {
  const c = document.createElement("canvas");
  c.width = 1;
  c.height = 1;
  const g = c.getContext("2d", { willReadFrequently: true });
  if (!g) throw new Error("no 2d context");
  g.drawImage(src, sx, sy, 1, 1, 0, 0, 1, 1);
  return g.getImageData(0, 0, 1, 1).data;
}

/**
 * Luminance (CIE L* / 100) of the rendered image the viewer shows under `p` (displayed-frame fraction), or null when
 * unreadable. The pixels come from the preview render itself. A `sieve://` image taints the canvas, so on failure the
 * render URL is fetched as a blob and decoded (`createImageBitmap`, or an `<img>` on an object URL for formats it rejects).
 */
export async function sampleLuma(p: NormPoint): Promise<number | null> {
  const img = document.querySelector<HTMLImageElement>('[data-testid="view-main"]');
  if (!img || !img.naturalWidth) return null;
  const at = (w: number, h: number) => [clamp(Math.round(p.x * w), 0, w - 1), clamp(Math.round(p.y * h), 0, h - 1)] as const;
  try {
    return lightness(readPixel(img, ...at(img.naturalWidth, img.naturalHeight)));
  } catch {
    /* tainted canvas: decode the bytes ourselves */
  }
  try {
    const res = await fetch(img.currentSrc || img.src);
    if (!res.ok) return null;
    const blob = await res.blob();
    try {
      const bmp = await createImageBitmap(blob);
      const px = readPixel(bmp, ...at(bmp.width, bmp.height));
      bmp.close();
      return lightness(px);
    } catch {
      const url = URL.createObjectURL(blob);
      try {
        const im = new Image();
        im.src = url;
        await im.decode();
        return lightness(readPixel(im, ...at(im.naturalWidth, im.naturalHeight)));
      } finally {
        URL.revokeObjectURL(url);
      }
    }
  } catch {
    return null;
  }
}

export function MaskLayer({ masks, editor, id, frame, box, region = null, onError }: Props) {
  const root = useRef<HTMLDivElement>(null);
  const [cursor, setCursor] = useState<Pt | null>(null);
  const [rubber, setRubber] = useState<{ a: Pt; b: Pt } | null>(null);
  const linDrag = useRef<{ f: Pt; z: Pt; m: Pt } | null>(null);
  const gest = useRef<{ start: Pt; last: Pt; started: boolean; erase: boolean; dabs: number } | null>(null);
  const { tool, groups, selGroup, selComp } = masks;

  const local = useCallback((e: { clientX: number; clientY: number }): Pt => {
    const r = root.current!.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  }, []);

  // ---- overlay (latest-wins, one request in flight) ----
  const [ovState, setOv] = useState<{ res: RenderedMaskOverlay; region: NormRect | null } | null>(null);
  const ov = ovState?.res ?? null;
  type OvReq = { id: number; adj: typeof editor.adj; groupId: string; componentId: string | null; maxEdge: number; region: NormRect | null };
  const ovReq = useRef<{ busy: boolean; latest: OvReq | null }>({ busy: false, latest: null });
  const target = masks.hover ?? (selGroup ? { groupId: selGroup, componentId: null } : null);
  const targetOk = target && groups.some((g) => g.id === target.groupId);
  // Fitted: the whole frame at screen size. Zoomed to 100%: only the visible region, at its screen size (long edge, as
  // `RenderOptions.maxEdge` with a region), so the mask is as sharp as the photo instead of a stretched fitted render.
  const dpr = window.devicePixelRatio || 1;
  const shown = region && box ? { w: region.width * box.w, h: region.height * box.h } : box;
  const maxEdge = shown ? clamp(Math.ceil(Math.max(shown.w, shown.h) * dpr), 64, region ? 2048 : 1200) : 0;
  const regionKey = region ? JSON.stringify(region) : "";
  const pump = useCallback(() => {
    const r = ovReq.current;
    if (r.busy || !r.latest) return;
    const a = r.latest;
    r.busy = true;
    unwrap(commands.renderMaskOverlay(a.id, a.adj, { groupId: a.groupId, componentId: a.componentId }, { maxEdge: a.maxEdge, region: a.region }))
      .then((res) => {
        if (res && r.latest?.id === res.imageId) setOv({ res, region: a.region });
      })
      .catch(onError)
      .finally(() => {
        r.busy = false;
        if (r.latest && r.latest !== a) pump();
      });
  }, [onError]);
  const adj = editor.adj;
  useEffect(() => {
    if (id == null || !masks.overlayVisible || !targetOk || !target || maxEdge === 0) {
      ovReq.current.latest = null;
      // Keep the last overlay while it fades out, then drop it so a later show never flashes a stale matte.
      const t = setTimeout(() => setOv(null), OVERLAY_FADE_MS + 50);
      return () => clearTimeout(t);
    }
    ovReq.current.latest = { id, adj, groupId: target.groupId, componentId: target.componentId, maxEdge, region };
    pump();
    return undefined;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id, masks.overlayVisible, targetOk, target?.groupId, target?.componentId, adj, maxEdge, regionKey, pump]);
  useEffect(() => setOv(null), [id]);

  // ---- geometry helpers ----
  const toSensor = useCallback((p: Pt): NormPoint => dispToSensor(screenToDisp(p.x, p.y, box!), frame!), [box, frame]);
  const toScreen = useCallback((s: NormPoint): Pt => dispToScreen(sensorToDisp(s, frame!), box!), [box, frame]);

  const selected = useMemo(() => {
    const g = groups.find((x) => x.id === selGroup);
    const c = g?.components.find((x) => x.id === selComp) ?? null;
    return g && c ? { g, c } : null;
  }, [groups, selGroup, selComp]);

  if (!frame || !box || box.w < 4) return <div ref={root} className="pointer-events-none absolute inset-0" data-testid="mask-layer" />;

  const style: OverlayStyle = OVERLAY_STYLES[masks.overlayStyle] ?? OVERLAY_STYLES[0];

  // ---- tool gestures (capture surface) ----
  const inBox = (p: Pt) => p.x >= box.x && p.y >= box.y && p.x <= box.x + box.w && p.y <= box.y + box.h;

  const onDown = (e: React.PointerEvent) => {
    if (!tool || e.button !== 0) return;
    const p = local(e);
    if (!inBox(p) && tool.kind !== "brush") return;
    e.currentTarget.setPointerCapture(e.pointerId);
    gest.current = { start: p, last: p, started: false, erase: e.altKey, dabs: 1 };
    if (tool.kind === "brush") {
      masks.strokeStart(toSensor(p), { erase: e.altKey });
      gest.current.started = true;
    } else if (tool.kind === "color" || tool.kind === "object") setRubber({ a: p, b: p });
  };

  const onMove = (e: React.PointerEvent) => {
    const p = local(e);
    setCursor(p);
    const g = gest.current;
    if (!g || !tool) return;
    if (tool.kind === "brush") {
      const rPx = brushRadiusPx(sizeToRadius(masks.brush.size), frame, box);
      const spacing = Math.max(2, rPx * 0.2);
      if (Math.hypot(p.x - g.last.x, p.y - g.last.y) >= spacing && g.dabs < MAX_STROKE_DABS) {
        g.last = p;
        g.dabs++;
        masks.strokeMove(toSensor(p));
      }
    } else if (tool.kind === "linear") {
      if (!g.started && Math.hypot(p.x - g.start.x, p.y - g.start.y) < 4) return;
      const shape: MaskShape = { kind: "linear", full: toSensor(g.start), zero: toSensor(p) };
      if (!g.started) {
        g.started = true;
        masks.shapeStart("linear", shape);
      } else masks.shapeUpdate(shape);
    } else if (tool.kind === "radial") {
      if (!g.started && Math.hypot(p.x - g.start.x, p.y - g.start.y) < 4) return;
      const base: RadialMask = { top: 0, left: 0, bottom: 0, right: 0, angle: 0, midpoint: 50, roundness: 0, feather: 50, flipped: false };
      const shape: MaskShape = { kind: "radial", ...radialFromScreen(ellipseFromDrag(g.start, p), frame, box, base) };
      if (!g.started) {
        g.started = true;
        masks.shapeStart("radial", shape);
      } else masks.shapeUpdate(shape);
    } else if (tool.kind === "color" || tool.kind === "object") setRubber({ a: g.start, b: p });
  };

  const onUp = (e: React.PointerEvent) => {
    const g = gest.current;
    gest.current = null;
    setRubber(null);
    if (!g || !tool) return;
    const p = local(e);
    const dragged = Math.hypot(p.x - g.start.x, p.y - g.start.y) >= 4;
    switch (tool.kind) {
      case "brush":
        masks.commit();
        break;
      case "linear":
      case "radial":
        if (g.started) {
          masks.commit();
          masks.endTool();
        }
        break;
      case "color": {
        const a = screenToDisp(g.start.x, g.start.y, box);
        const b = screenToDisp(p.x, p.y, box);
        masks.addColorSample({ point: dispToSensor(dragged ? { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 } : a, frame), area: dragged ? dispRectToSensor(a, b, frame) : null });
        break;
      }
      case "luminance": {
        const at = screenToDisp(g.start.x, g.start.y, box);
        void sampleLuma(at).then((v) => {
          const l = v ?? 0.5;
          masks.setLuminance((s) => ({
            ...s,
            low: clamp(l - 0.15, 0, 1),
            high: clamp(l + 0.15, 0, 1),
            featherLow: clamp(l - 0.3, 0, 1),
            featherHigh: clamp(l + 0.3, 0, 1),
          }));
          masks.endTool();
        });
        break;
      }
      case "object":
        if (dragged) void masks.createObject(dispRectToSensor(screenToDisp(g.start.x, g.start.y, box), screenToDisp(p.x, p.y, box), frame));
        break;
    }
  };

  // ---- pins ----
  const pinPoint = (g: MaskGroup, index: number): Pt => {
    const c = g.components[0];
    let s: NormPoint | null = null;
    if (c) {
      const sh = c.shape;
      if (sh.kind === "brush") s = sh.strokes[0]?.dabs[0] ?? null;
      else if (sh.kind === "linear") s = { x: (sh.zero.x + sh.full.x) / 2, y: (sh.zero.y + sh.full.y) / 2 };
      else if (sh.kind === "radial") s = { x: (sh.left + sh.right) / 2, y: (sh.top + sh.bottom) / 2 };
      else if (sh.kind === "ai") s = sh.referencePoint;
      else if (sh.kind === "color") s = sh.samples[0]?.point ?? null;
    }
    if (s) return toScreen(s);
    return { x: box.x + box.w * (0.5 + 0.04 * index), y: box.y + box.h * (0.5 + 0.04 * index) };
  };

  // ---- gradient handles for the selected component ----
  const drag = (start?: () => void) => ({
    onPointerDown: (e: React.PointerEvent) => {
      e.stopPropagation();
      e.currentTarget.setPointerCapture(e.pointerId);
      start?.();
    },
  });

  const linearHandles = (g: MaskGroup, cid: string, sh: LinearMask) => {
    const full = toScreen(sh.full);
    const zero = toScreen(sh.zero);
    const mid = { x: (full.x + zero.x) / 2, y: (full.y + zero.y) / 2 };
    const len = Math.hypot(zero.x - full.x, zero.y - full.y) || 1;
    const d = { x: (zero.x - full.x) / len, y: (zero.y - full.y) / len };
    const n = { x: -d.y, y: d.x };
    const ext = Math.hypot(box.w, box.h);
    const line = (c: Pt, cls: string, tid: string) => (
      <line key={tid} data-testid={tid} x1={c.x - n.x * ext} y1={c.y - n.y * ext} x2={c.x + n.x * ext} y2={c.y + n.y * ext} className={cls} strokeWidth={1.25} />
    );
    const upd = (f: Pt, z: Pt) => masks.editShape(g.id, cid, () => ({ kind: "linear", full: toSensor(f), zero: toSensor(z) }), "Mask: Linear Gradient");
    const hp = { r: 6, className: "cursor-pointer fill-white stroke-black", style: { pointerEvents: "all" as const } };
    const rot = { x: mid.x + n.x * 46, y: mid.y + n.y * 46 };
    const begin = () => {
      linDrag.current = { f: full, z: zero, m: mid };
    };
    return (
      <g data-testid="linear-handles">
        {line(full, "stroke-white/90", "linear-line-full")}
        {line(mid, "stroke-white/50", "linear-line-mid")}
        {line(zero, "stroke-white/90", "linear-line-zero")}
        <line x1={mid.x} y1={mid.y} x2={rot.x} y2={rot.y} className="stroke-white/50" />
        <circle
          {...hp}
          cx={mid.x}
          cy={mid.y}
          data-testid="linear-handle-pin"
          {...drag(begin)}
          onPointerMove={(e) => {
            const dragging = linDrag.current;
            if (!dragging || !e.currentTarget.hasPointerCapture(e.pointerId)) return;
            const p = local(e);
            const dx = p.x - dragging.m.x;
            const dy = p.y - dragging.m.y;
            upd({ x: dragging.f.x + dx, y: dragging.f.y + dy }, { x: dragging.z.x + dx, y: dragging.z.y + dy });
          }}
          onPointerUp={() => masks.commit()}
        />
        {(
          [
            ["full", full, zero],
            ["zero", zero, full],
          ] as const
        ).map(([which, pt, other]) => (
          <rect
            key={which}
            x={pt.x - 5}
            y={pt.y - 5}
            width={10}
            height={10}
            data-testid={`linear-handle-${which}`}
            className="cursor-move fill-sky-300 stroke-black"
            style={{ pointerEvents: "all" }}
            {...drag(begin)}
            onPointerMove={(e) => {
              const dragging = linDrag.current;
            if (!dragging || !e.currentTarget.hasPointerCapture(e.pointerId)) return;
              const p = local(e);
              const dir = { x: (pt.x - other.x) / len, y: (pt.y - other.y) / len };
              const t = Math.max(4, (p.x - other.x) * dir.x + (p.y - other.y) * dir.y);
              const np = { x: other.x + dir.x * t, y: other.y + dir.y * t };
              if (which === "full") upd(np, zero);
              else upd(full, np);
            }}
            onPointerUp={() => masks.commit()}
          />
        ))}
        <circle
          {...hp}
          cx={rot.x}
          cy={rot.y}
          data-testid="linear-handle-rotate"
          className="cursor-grab fill-amber-300 stroke-black"
          {...drag(begin)}
          onPointerMove={(e) => {
            const dragging = linDrag.current;
            if (!dragging || !e.currentTarget.hasPointerCapture(e.pointerId)) return;
            const p = local(e);
            const a = Math.atan2(p.y - mid.y, p.x - mid.x) - Math.atan2(rot.y - mid.y, rot.x - mid.x);
            const c = Math.cos(a);
            const s = Math.sin(a);
            const rotate = (q: Pt): Pt => ({ x: mid.x + (q.x - mid.x) * c - (q.y - mid.y) * s, y: mid.y + (q.x - mid.x) * s + (q.y - mid.y) * c });
            upd(rotate(full), rotate(zero));
          }}
          onPointerUp={() => masks.commit()}
        />
      </g>
    );
  };

  const radialHandles = (g: MaskGroup, cid: string, sh: RadialMask) => {
    const e = radialToScreen(sh, frame, box);
    const cos = Math.cos(e.rot / DEG);
    const sin = Math.sin(e.rot / DEG);
    const at = (lx: number, ly: number): Pt => ({ x: e.cx + lx * cos - ly * sin, y: e.cy + lx * sin + ly * cos });
    const toLocal = (p: Pt): Pt => ({ x: (p.x - e.cx) * cos + (p.y - e.cy) * sin, y: -(p.x - e.cx) * sin + (p.y - e.cy) * cos });
    const upd = (n: Partial<{ cx: number; cy: number; rx: number; ry: number; rot: number }>, extra?: Partial<RadialMask>) =>
      masks.editShape(g.id, cid, (s) => (s.kind === "radial" ? { kind: "radial", ...radialFromScreen({ ...e, ...n }, frame, box, { ...s, ...extra }) } : s), "Mask: Radial Gradient");
    const fk = clamp(1 - sh.feather / 100, 0.02, 1);
    const dot = "cursor-pointer fill-white stroke-black";
    const st = { pointerEvents: "all" as const };
    const edge = (tid: string, lx: number, ly: number, axis: "rx" | "ry") => {
      const p = at(lx, ly);
      return (
        <rect
          key={tid}
          x={p.x - 5}
          y={p.y - 5}
          width={10}
          height={10}
          data-testid={tid}
          className="cursor-move fill-sky-300 stroke-black"
          style={st}
          {...drag()}
          onPointerMove={(ev) => {
            if (!ev.currentTarget.hasPointerCapture(ev.pointerId)) return;
            const l = toLocal(local(ev));
            upd({ [axis]: Math.max(6, Math.abs(axis === "rx" ? l.x : l.y)) });
          }}
          onPointerUp={() => masks.commit()}
        />
      );
    };
    const rot = at(e.rx + 28, 0);
    const fp = at(0, e.ry * fk);
    return (
      <g data-testid="radial-handles">
        <ellipse cx={e.cx} cy={e.cy} rx={e.rx} ry={e.ry} transform={`rotate(${e.rot} ${e.cx} ${e.cy})`} className="fill-none stroke-white" strokeWidth={1.25} data-testid="radial-ellipse" />
        <ellipse cx={e.cx} cy={e.cy} rx={e.rx * fk} ry={e.ry * fk} transform={`rotate(${e.rot} ${e.cx} ${e.cy})`} className="fill-none stroke-white/60" strokeDasharray="4 3" />
        <circle
          cx={e.cx}
          cy={e.cy}
          r={6}
          data-testid="radial-handle-center"
          className={dot}
          style={st}
          {...drag()}
          onPointerMove={(ev) => {
            if (!ev.currentTarget.hasPointerCapture(ev.pointerId)) return;
            const p = local(ev);
            upd({ cx: p.x, cy: p.y });
          }}
          onPointerUp={() => masks.commit()}
        />
        {edge("radial-handle-e", e.rx, 0, "rx")}
        {edge("radial-handle-w", -e.rx, 0, "rx")}
        {edge("radial-handle-n", 0, -e.ry, "ry")}
        {edge("radial-handle-s", 0, e.ry, "ry")}
        <circle
          cx={fp.x}
          cy={fp.y}
          r={4}
          data-testid="radial-handle-feather"
          className="cursor-ns-resize fill-emerald-300 stroke-black"
          style={st}
          {...drag()}
          onPointerMove={(ev) => {
            if (!ev.currentTarget.hasPointerCapture(ev.pointerId)) return;
            const l = toLocal(local(ev));
            upd({}, { feather: Math.round(clamp(100 * (1 - Math.abs(l.y) / e.ry), 0, 100)) });
          }}
          onPointerUp={() => masks.commit()}
        />
        <circle
          cx={rot.x}
          cy={rot.y}
          r={6}
          data-testid="radial-handle-rotate"
          className="cursor-grab fill-amber-300 stroke-black"
          style={st}
          {...drag()}
          onPointerMove={(ev) => {
            if (!ev.currentTarget.hasPointerCapture(ev.pointerId)) return;
            const p = local(ev);
            upd({ rot: Math.atan2(p.y - e.cy, p.x - e.cx) * DEG });
          }}
          onPointerUp={() => masks.commit()}
        />
      </g>
    );
  };

  const brushR = tool?.kind === "brush" ? brushRadiusPx(sizeToRadius(masks.brush.size), frame, box) : 0;
  const showOv = ov && targetOk;
  const ovShown = masks.overlayVisible;
  const ovRegion = ovState?.region ?? null;
  const ovBox = ovRegion
    ? { left: box.x + ovRegion.x * box.w, top: box.y + ovRegion.y * box.h, width: ovRegion.width * box.w, height: ovRegion.height * box.h }
    : { left: box.x, top: box.y, width: box.w, height: box.h };

  return (
    <div ref={root} className="pointer-events-none absolute inset-0 overflow-hidden" data-testid="mask-layer" data-tool={tool?.kind ?? ""} data-box={JSON.stringify(box)}>
      {showOv &&
        (style.mode === "color" ? (
          <div
            className="absolute"
            style={{ ...ovBox, opacity: ovShown ? 1 : 0, transition: `opacity ${OVERLAY_FADE_MS}ms ease-out` }}
            data-testid="mask-overlay"
            data-overlay-seq={ov!.seq}
            data-overlay-style={style.id}
            data-overlay-visible={ovShown}
            data-overlay-pinned={masks.overlayOn}
            data-overlay-region={ovRegion ? JSON.stringify(ovRegion) : ""}
          >
            {/* Dark red reads as a mask even on bright areas: the photo is darkened inside the mask (inverted matte, multiply)... */}
            <img src={ov!.url} alt="" draggable={false} className="absolute inset-0 size-full" style={{ mixBlendMode: "multiply", filter: "invert(1)", opacity: OVERLAY_ALPHA }} />
            {/* ...and tinted with the colour at the same strength (screen). */}
            <div className="absolute inset-0 isolate" style={{ mixBlendMode: "screen" }}>
              <div className="absolute inset-0" style={{ backgroundColor: style.color, opacity: OVERLAY_ALPHA }} data-testid="mask-overlay-tint" />
              <img src={ov!.url} alt="" draggable={false} className="absolute inset-0 size-full" style={{ mixBlendMode: "multiply" }} />
            </div>
          </div>
        ) : (
          <img
            src={ov!.url}
            alt=""
            draggable={false}
            className="absolute"
            style={{ ...ovBox, opacity: ovShown ? (style.mode === "gray" ? 0.65 : 1) : 0, transition: `opacity ${OVERLAY_FADE_MS}ms ease-out` }}
            data-testid="mask-overlay"
            data-overlay-seq={ov!.seq}
            data-overlay-style={style.id}
            data-overlay-visible={ovShown}
            data-overlay-pinned={masks.overlayOn}
            data-overlay-region={ovRegion ? JSON.stringify(ovRegion) : ""}
          />
        ))}

      <svg className="absolute inset-0 size-full overflow-visible" data-testid="mask-svg">
        {selected && selected.c.active && selected.g.active && selected.c.shape.kind === "linear" && linearHandles(selected.g, selected.c.id, selected.c.shape)}
        {selected && selected.c.active && selected.g.active && selected.c.shape.kind === "radial" && radialHandles(selected.g, selected.c.id, selected.c.shape)}
        {masks.pins &&
          groups.map((g, i) => {
            const on = g.id === selGroup;
            // The selected gradient's own centre handle is its pin.
            if (on && selected && selected.g.id === g.id && (selected.c.shape.kind === "linear" || selected.c.shape.kind === "radial") && selected.c.active) return null;
            const p = pinPoint(g, i);
            return (
              <g key={g.id} transform={`translate(${p.x} ${p.y})`} data-testid={`mask-pin-${g.id}`} data-selected={on} data-active={g.active} style={{ pointerEvents: "all", cursor: "pointer" }} onPointerDown={(e) => e.stopPropagation()} onClick={(e) => { e.stopPropagation(); masks.select(g.id, g.components[0]?.id ?? null); }}>
                <circle r={9} className={on ? "fill-sky-400 stroke-white" : "fill-black/60 stroke-white"} strokeWidth={1.5} opacity={g.active ? 1 : 0.4} />
                <circle r={3} className={on ? "fill-white" : "fill-white/80"} />
              </g>
            );
          })}
        {rubber && <rect x={Math.min(rubber.a.x, rubber.b.x)} y={Math.min(rubber.a.y, rubber.b.y)} width={Math.abs(rubber.a.x - rubber.b.x)} height={Math.abs(rubber.a.y - rubber.b.y)} className="fill-sky-400/10 stroke-sky-300" strokeDasharray="4 3" />}
        {tool?.kind === "brush" && cursor && (
          <g data-testid="brush-cursor" data-radius={brushR.toFixed(1)} pointerEvents="none">
            <circle cx={cursor.x} cy={cursor.y} r={Math.max(1, brushR)} className="fill-none stroke-white" strokeWidth={1.25} />
            <circle cx={cursor.x} cy={cursor.y} r={Math.max(1, brushR * (1 - masks.brush.feather / 100))} className="fill-none stroke-white/60" strokeDasharray="3 3" />
            {(masks.brush.erase || gest.current?.erase) && <line x1={cursor.x - 4} x2={cursor.x + 4} y1={cursor.y} y2={cursor.y} className="stroke-white" />}
          </g>
        )}
      </svg>

      {tool && (
        <div
          className="pointer-events-auto absolute inset-0 cursor-crosshair"
          style={{ cursor: tool.kind === "brush" ? "none" : "crosshair", touchAction: "none" }}
          data-testid="mask-capture"
          onPointerDown={onDown}
          onPointerMove={onMove}
          onPointerUp={onUp}
          onPointerLeave={() => setCursor(null)}
        />
      )}
    </div>
  );
}
