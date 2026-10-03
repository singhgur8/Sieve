import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { ImageOff } from "lucide-react";
import { convertFileSrc, type RawImageEntry } from "../ipc";

/** Zoom relative to "fit" (1 = fit), centre of the viewport in normalised image coordinates. */
export interface View {
  scale: number;
  cx: number;
  cy: number;
  /** True while the view is 1:1 (100%) of the decoded preview: re-derived when the preview finishes decoding. */
  actual?: boolean;
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
  /** Re-derive a 1:1 zoom when the preview decodes (Compare: only the pane that owns the shared metrics). */
  rescaleActual?: boolean;
}

/** Fit / zoom / pan surface for one preview (uses the 2048px preview, thumbnail underneath while loading). */
export function ZoomPane({ entry, version, view, onView, metricsRef, testId, onFocus, maxScale = 16, rescaleActual = true }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });
  // The preview is decoded off-screen first; it is only mounted once it can paint in full. Until then the
  // (already decoded, tiny) thumbnail of the SAME photo shows, so a swap never paints the previous photo or a blank.
  const [decoded, setDecoded] = useState<{ url: string; w: number; h: number } | null>(null);
  const [broken, setBroken] = useState<string | null>(null);
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

  const previewUrl = previewPath ? `${convertFileSrc(previewPath)}?v=${version}` : null;
  const thumbUrl = thumbPath ? `${convertFileSrc(thumbPath)}?v=${version}` : null;
  useEffect(() => {
    if (!previewUrl) return;
    let stale = false;
    const im = new Image();
    im.src = previewUrl;
    im.decode().then(
      () => !stale && setDecoded({ url: previewUrl, w: im.naturalWidth, h: im.naturalHeight }),
      () => !stale && setBroken(previewPath),
    );
    return () => {
      stale = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [previewUrl]);
  const nat = decoded && decoded.url === previewUrl ? decoded : null;

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

  // 1:1 was chosen against the thumbnail's dimensions if Space came before the preview decoded: re-derive it
  // against the real pixels (and after a resize) so "100%" stays 100%.
  const wantScale = fitW > 0 ? Math.max(1, (nat?.w ?? nw) / fitW) : 1;
  useEffect(() => {
    if (!rescaleActual || !view.actual || !nat || Math.abs(view.scale - wantScale) < 0.001) return;
    onView({ ...view, scale: wantScale });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [nat?.w, nat?.h, wantScale, view.actual, rescaleActual]);

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
    onView({ scale: view.scale, actual: view.actual, cx: (size.w / 2 - nx) / dw, cy: (size.h / 2 - ny) / dh });
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
      onDoubleClick={() => onView(view.scale > 1 ? FIT : { scale: Math.max(1, (metricsRef.current?.natW ?? fitW) / fitW), cx: 0.5, cy: 0.5, actual: true })}
    >
      {thumbUrl && <img key={thumbUrl} src={thumbUrl} alt="" draggable={false} className={imgClass} style={style} data-testid="zoom-thumb" />}
      {t?.status === "failed" && (
        <Unavailable title="No preview for this photo" detail={t.reason} />
      )}
      {previewPath && broken === previewPath && (
        <Unavailable title="Preview unavailable" detail="The cached preview could not be loaded (cache folder cleaned or drive offline). Use More > Regenerate previews, or re-import the folder." />
      )}
      {previewUrl && nat && <img key={previewUrl} src={previewUrl} alt={entry?.fileName} draggable={false} className={imgClass} style={style} onError={() => setBroken(previewPath)} />}
    </div>
  );
}

function Unavailable({ title, detail }: { title: string; detail: string }) {
  return (
    <div className="pointer-events-none absolute inset-0 flex flex-col items-center justify-center gap-2 p-6 text-center" data-testid="preview-unavailable">
      <ImageOff className="size-8 text-neutral-400" />
      <p className="text-sm font-medium text-neutral-200">{title}</p>
      <p className="max-w-sm break-words text-xs text-neutral-400">{detail}</p>
    </div>
  );
}
