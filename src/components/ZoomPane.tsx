import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { ImageOff } from "lucide-react";
import { commands, completeAdjustments, convertFileSrc, unwrap, type RawImageEntry } from "../ipc";

/** Zoom relative to "fit" (1 = fit), centre of the viewport in normalised image coordinates. */
export interface View {
  scale: number;
  cx: number;
  cy: number;
  /** Set while the zoom is pinned to a percentage of the decoded preview's pixels (100 = 1:1): re-derived when the preview finishes decoding or the photo changes. */
  pct?: number;
}
export const FIT: View = { scale: 1, cx: 0.5, cy: 0.5 };

export interface Metrics {
  fitW: number;
  fitH: number;
  natW: number;
  natH: number;
  cw: number;
  ch: number;
  /** Where the image's top-left sits in the pane, its displayed size, and the last pointer position over the pane (pane px, null when outside). */
  x: number;
  y: number;
  dw: number;
  dh: number;
  hover: { x: number; y: number } | null;
}

/**
 * Zoom to `scale` (relative to fit) keeping the image point under pane position (mx, my) fixed: the Lightroom
 * behaviour for Space / click. The resulting centre is clamped so the image never leaves the pane.
 */
export function zoomAt(m: Metrics, scale: number, mx: number, my: number, pct?: number): View {
  if (scale <= 1.001) return FIT;
  const px = (mx - m.x) / m.dw;
  const py = (my - m.y) / m.dh;
  const ndw = m.fitW * scale;
  const ndh = m.fitH * scale;
  const clampC = (c: number, vp: number, d: number) => {
    const lo = vp / 2 / d;
    return Math.min(1 - lo, Math.max(lo, c));
  };
  return {
    scale,
    pct,
    cx: ndw <= m.cw ? 0.5 : clampC((m.cw / 2 - (mx - px * ndw)) / ndw, m.cw, ndw),
    cy: ndh <= m.ch ? 0.5 : clampC((m.ch / 2 - (my - py * ndh)) / ndh, m.ch, ndh),
  };
}

/** Scale (relative to fit) at which the preview shows `pct` percent of its own pixels. */
export const scaleForPct = (m: Metrics, pct: number) => (m.fitW > 0 ? ((pct / 100) * m.natW) / m.fitW : 1);
/** Scale (relative to fit) at which the image covers the whole pane. */
export const fillScale = (m: Metrics) => (m.fitW > 0 && m.fitH > 0 ? Math.max(m.cw / m.fitW, m.ch / m.fitH) : 1);

interface Props {
  entry: RawImageEntry | undefined;
  version: number;
  view: View;
  onView: (v: View) => void;
  metricsRef: React.MutableRefObject<Metrics | null>;
  testId?: string;
  onFocus?: () => void;
  maxScale?: number;
  /** Re-derive a pinned-percentage zoom when the preview decodes (Compare: only the pane that owns the shared metrics). */
  rescaleActual?: boolean;
  /** Space-less click on the image (no drag): Lightroom toggles Fit / 100% at that point. */
  onClickZoom?: (mx: number, my: number) => void;
}

/** Fit / zoom / pan surface for one preview (uses the 2048px preview, thumbnail underneath while loading). */
export function ZoomPane({ entry, version, view, onView, metricsRef, testId, onFocus, maxScale = 64, rescaleActual = true, onClickZoom }: Props) {
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

  // Full-resolution pixels per preview pixel: "100%" means 1:1 of the sensor image, not of the 2048 px preview.
  const longFull = Math.max(entry?.width ?? 0, entry?.height ?? 0);
  const k = nat && longFull > 0 ? Math.max(1, longFull / Math.max(nat.w, nat.h)) : 1;
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
  // The pointer position lives on the shared metrics object (not per pane) so Compare's two panes agree on it.
  metricsRef.current = { fitW, fitH, natW: (nat?.w ?? nw) * k, natH: (nat?.h ?? nh) * k, cw: size.w, ch: size.h, x, y, dw, dh, hover: metricsRef.current?.hover ?? null };
  const setHover = (h: { x: number; y: number } | null) => {
    if (metricsRef.current) metricsRef.current.hover = h;
  };

  // A percentage zoom chosen against the thumbnail's dimensions (before the preview decoded) or against the
  // previous photo is re-derived against the real pixels (and after a resize) so "100%" stays 100%.
  const wantScale = fitW > 0 && view.pct ? Math.max(1, ((view.pct / 100) * (nat?.w ?? nw) * k) / fitW) : 1;
  useEffect(() => {
    if (!rescaleActual || !view.pct || !nat || Math.abs(view.scale - wantScale) < 0.001) return;
    onView({ ...view, scale: wantScale });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [nat?.w, nat?.h, wantScale, view.pct, rescaleActual]);

  // Full-resolution detail tile for the visible area (Lightroom Loupe at 100%+): a region render with the photo's
  // current edits, requested after the view settles (latest wins), painted over the upscaled preview at its
  // normalised position, so it never moves the image. Crop is ignored so the tile registers with the uncropped preview.
  const [tile, setTile] = useState<{ id: number; v: number; url: string; rx: number; ry: number; rw: number; rh: number } | null>(null);
  const ticket = useRef(0);
  const entryId = entry?.id ?? null;
  const wantTile = !!nat && entryId != null && k > 1.01 && dw > nat.w * 1.02 && size.w > 0;
  useEffect(() => {
    const mine = ++ticket.current;
    if (!wantTile || entryId == null || !nat) {
      setTile(null);
      return;
    }
    const padX = 0.15 * size.w;
    const padY = 0.15 * size.h;
    const rx = Math.max(0, (-x - padX) / dw);
    const ry = Math.max(0, (-y - padY) / dh);
    const rw = Math.min(1, (-x + size.w + padX) / dw) - rx;
    const rh = Math.min(1, (-y + size.h + padY) / dh) - ry;
    if (rw <= 0 || rh <= 0) return;
    const dpr = window.devicePixelRatio || 1;
    const maxEdge = Math.round(Math.min(8192, Math.max(64, Math.min(Math.max(rw * nat.w * k, rh * nat.h * k), Math.max(rw * dw, rh * dh) * dpr))));
    const timer = setTimeout(() => {
      void (async () => {
        try {
          const a = completeAdjustments(await unwrap(commands.getAdjustments(entryId)), entry?.format);
          if (mine !== ticket.current) return;
          const r = await unwrap(commands.renderPreview(entryId, { ...a, crop: { ...a.crop, enabled: false } }, { maxEdge, slot: "detail", region: { x: rx, y: ry, width: rw, height: rh } }));
          if (!r || mine !== ticket.current) return;
          const im = new Image();
          im.src = r.url;
          await im.decode().catch(() => undefined);
          if (mine !== ticket.current) return;
          setTile({ id: entryId, v: version, url: r.url, rx, ry, rw, rh });
        } catch {
          /* the upscaled preview stays */
        }
      })();
    }, 150);
    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [wantTile, entryId, version, view.scale, view.cx, view.cy, size.w, size.h, nat?.w, nat?.h]);
  const showTile = wantTile && tile && tile.id === entryId && tile.v === version ? tile : null;

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

  // Drag to pan. The view is committed at most once per animation frame; only the CSS translate changes.
  const drag = useRef<{ sx: number; sy: number; x: number; y: number; moved: boolean } | null>(null);
  const raf = useRef(0);
  const pending = useRef<View | null>(null);
  useEffect(() => () => cancelAnimationFrame(raf.current), []);
  const flush = () => {
    raf.current = 0;
    const v = pending.current;
    pending.current = null;
    if (v) latest.current.onView(v);
  };
  const onPointerDown = (e: React.PointerEvent) => {
    onFocus?.();
    if (e.button !== 0) return;
    drag.current = { sx: e.clientX, sy: e.clientY, x, y, moved: false };
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const local = (e: React.PointerEvent) => {
    const r = e.currentTarget.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  };
  const onPointerMove = (e: React.PointerEvent) => {
    setHover(local(e));
    const d = drag.current;
    if (!d) return;
    if (!d.moved && Math.hypot(e.clientX - d.sx, e.clientY - d.sy) < 4) return;
    d.moved = true;
    if (view.scale <= 1) return;
    const nx = d.x + e.clientX - d.sx;
    const ny = d.y + e.clientY - d.sy;
    pending.current = { scale: view.scale, pct: view.pct, cx: (size.w / 2 - nx) / dw, cy: (size.h / 2 - ny) / dh };
    if (!raf.current) raf.current = requestAnimationFrame(flush);
  };
  const onPointerUp = (e: React.PointerEvent) => {
    const d = drag.current;
    drag.current = null;
    if (d && !d.moved && onClickZoom) {
      const p = local(e);
      onClickZoom(p.x, p.y);
    }
  };
  const onPointerLeave = () => {
    setHover(null);
  };

  const style = { width: dw, height: dh, transform: `translate3d(${x}px, ${y}px, 0)` } as const;
  const imgClass = "pointer-events-none absolute left-0 top-0 max-w-none select-none";
  return (
    <div
      ref={ref}
      data-testid={testId}
      data-scale={view.scale}
      data-cx={view.cx.toFixed(4)}
      data-cy={view.cy.toFixed(4)}
      className={`relative size-full overflow-hidden bg-black ${view.scale > 1 ? "cursor-grab active:cursor-grabbing" : onClickZoom ? "cursor-zoom-in" : ""}`}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={() => (drag.current = null)}
      onPointerLeave={onPointerLeave}
    >
      {thumbUrl && <img key={thumbUrl} src={thumbUrl} alt="" draggable={false} className={imgClass} style={style} data-testid="zoom-thumb" />}
      {t?.status === "failed" && (
        <Unavailable title="No preview for this photo" detail={t.reason} />
      )}
      {previewPath && broken === previewPath && (
        <Unavailable title="Preview unavailable" detail="The cached preview could not be loaded (cache folder cleaned or drive offline). Use More > Regenerate previews, or re-import the folder." />
      )}
      {previewUrl && nat && <img key={previewUrl} src={previewUrl} alt={entry?.fileName} draggable={false} className={imgClass} style={style} onError={() => setBroken(previewPath)} />}
      {showTile && (
        <img
          key={showTile.url}
          src={showTile.url}
          alt=""
          draggable={false}
          className={imgClass}
          data-testid="zoom-detail"
          style={{ width: showTile.rw * dw, height: showTile.rh * dh, transform: `translate3d(${x + showTile.rx * dw}px, ${y + showTile.ry * dh}px, 0)` }}
        />
      )}
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
