import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { convertFileSrc, type RawImageEntry } from "../ipc";

/** Zoom relative to "fit" (1 = fit), centre of the viewport in normalised image coordinates. */
export interface View {
  scale: number;
  cx: number;
  cy: number;
}
export const FIT: View = { scale: 1, cx: 0.5, cy: 0.5 };

export interface Metrics {
  fitW: number;
  fitH: number;
  natW: number;
  natH: number;
  cw: number;
  ch: number;
}

interface Props {
  entry: RawImageEntry | undefined;
  version: number;
  view: View;
  onView: (v: View) => void;
  metricsRef: React.MutableRefObject<Metrics | null>;
  testId?: string;
  onFocus?: () => void;
  maxScale?: number;
}

/** Fit / zoom / pan surface for one preview (uses the 2048px preview, thumbnail underneath while loading). */
export function ZoomPane({ entry, version, view, onView, metricsRef, testId, onFocus, maxScale = 16 }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });
  const [nat, setNat] = useState<{ w: number; h: number } | null>(null);
  const t = entry?.thumbnail;
  const previewPath = t?.status === "ready" ? (t.previewPath ?? t.path) : null;
  const thumbPath = t?.status === "ready" ? t.path : null;

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setSize({ w: el.clientWidth, h: el.clientHeight });
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    setNat(null);
  }, [previewPath]);

  const nw = nat?.w ?? (t?.status === "ready" ? t.width : 3) ?? 3;
  const nh = nat?.h ?? (t?.status === "ready" ? t.height : 2) ?? 2;
  const fit = size.w > 0 && size.h > 0 ? Math.min(size.w / nw, size.h / nh) : 1;
  const fitW = nw * fit;
  const fitH = nh * fit;
  const dw = fitW * view.scale;
  const dh = fitH * view.scale;
  const clampPos = (p: number, viewport: number, d: number) => (d <= viewport ? (viewport - d) / 2 : Math.min(0, Math.max(viewport - d, p)));
  const x = clampPos(size.w / 2 - view.cx * dw, size.w, dw);
  const y = clampPos(size.h / 2 - view.cy * dh, size.h, dh);
  metricsRef.current = { fitW, fitH, natW: nat?.w ?? nw, natH: nat?.h ?? nh, cw: size.w, ch: size.h };

  const latest = useRef({ view, dw, dh, x, y, size, onView, maxScale });
  latest.current = { view, dw, dh, x, y, size, onView, maxScale };

  // Wheel zoom around the cursor (needs a non-passive listener).
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const l = latest.current;
      const rect = el.getBoundingClientRect();
      const mx = e.clientX - rect.left;
      const my = e.clientY - rect.top;
      const px = (mx - l.x) / l.dw;
      const py = (my - l.y) / l.dh;
      const scale = Math.min(l.maxScale, Math.max(1, l.view.scale * Math.exp(-e.deltaY * 0.0025)));
      if (scale === 1) return l.onView({ scale: 1, cx: 0.5, cy: 0.5 });
      const ndw = (l.dw / l.view.scale) * scale;
      const ndh = (l.dh / l.view.scale) * scale;
      l.onView({ scale, cx: (l.size.w / 2 - (mx - px * ndw)) / ndw, cy: (l.size.h / 2 - (my - py * ndh)) / ndh });
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  const drag = useRef<{ sx: number; sy: number; x: number; y: number } | null>(null);
  const onPointerDown = (e: React.PointerEvent) => {
    onFocus?.();
    if (view.scale <= 1) return;
    drag.current = { sx: e.clientX, sy: e.clientY, x, y };
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const onPointerMove = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    const nx = d.x + e.clientX - d.sx;
    const ny = d.y + e.clientY - d.sy;
    onView({ scale: view.scale, cx: (size.w / 2 - nx) / dw, cy: (size.h / 2 - ny) / dh });
  };
  const onPointerUp = () => {
    drag.current = null;
  };

  const style = { width: dw, height: dh, transform: `translate(${x}px, ${y}px)` } as const;
  const imgClass = "pointer-events-none absolute left-0 top-0 max-w-none select-none";
  return (
    <div
      ref={ref}
      data-testid={testId}
      data-scale={view.scale}
      className={`relative size-full overflow-hidden bg-black ${view.scale > 1 ? "cursor-grab active:cursor-grabbing" : ""}`}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onDoubleClick={() => onView(view.scale > 1 ? FIT : { scale: Math.max(1, (metricsRef.current?.natW ?? fitW) / fitW), cx: 0.5, cy: 0.5 })}
    >
      {thumbPath && <img src={`${convertFileSrc(thumbPath)}?v=${version}`} alt="" draggable={false} className={imgClass} style={style} />}
      {previewPath && (
        <img
          src={`${convertFileSrc(previewPath)}?v=${version}`}
          alt={entry?.fileName}
          draggable={false}
          className={imgClass}
          style={style}
          onLoad={(e) => setNat({ w: e.currentTarget.naturalWidth, h: e.currentTarget.naturalHeight })}
        />
      )}
    </div>
  );
}
