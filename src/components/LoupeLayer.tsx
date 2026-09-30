import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from "react";
import { Flag, X } from "lucide-react";
import { commands, unwrap, type FaceInfo, type RawImageEntry } from "../ipc";
import type { Library } from "../hooks/useLibrary";
import { formatShutter, LABEL_COLOR, tagName, TAG_STYLE, trimNum } from "../lib/format";
import { CompanionBadge, HealthBadge, Stars, XmpBadge } from "./Cell";
import { Filmstrip } from "./Filmstrip";
import { usePanels } from "../lib/panels";
import { FIT, ZoomPane, type Metrics, type View } from "./ZoomPane";

export interface CompareState {
  pool: number[];
  a: number;
  b: number;
  focus: "a" | "b";
}

export interface LoupeHandle {
  toggleZoom: () => void;
  cycleFace: (dir: 1 | -1) => void;
  resetView: () => void;
  cycleInfo: () => void;
}

interface Props {
  mode: "loupe" | "compare";
  lib: Library;
  activeId: number | null;
  compare: CompareState | null;
  onFocusPane: (which: "a" | "b") => void;
  onOpen: (id: number) => void;
  /** "Locate folder…" for the folder of image `imageId`. */
  onLocate?: (imageId: number) => void;
}

/** Full-area loupe / 2-up compare. Owns zoom/pan state so pans do not re-render the grid. */
export const LoupeLayer = forwardRef<LoupeHandle, Props>(function LoupeLayer({ mode, lib, activeId, compare, onFocusPane, onOpen, onLocate }, ref) {
  const [view, setView] = useState<View>(FIT);
  const [faces, setFaces] = useState<FaceInfo[]>([]);
  const [info, setInfo] = useState<InfoLevel>("full");
  const faceIdx = useRef(-1);
  const metrics = useRef<Metrics | null>(null);
  const panels = usePanels("loupe");
  const viewRef = useRef(view);
  viewRef.current = view;

  // Single loupe: navigating resets to fit (compare keeps the shared zoom while stepping).
  useEffect(() => {
    if (mode === "loupe") setView(FIT);
    faceIdx.current = -1;
  }, [activeId, mode]);

  useEffect(() => {
    setFaces([]);
    if (activeId == null) return;
    let stale = false;
    unwrap(commands.getFaces(activeId))
      .then((f) => !stale && setFaces(f))
      .catch(() => {});
    return () => {
      stale = true;
    };
  }, [activeId]);

  useImperativeHandle(
    ref,
    () => ({
      toggleZoom: () => {
        const m = metrics.current;
        if (!m) return;
        const v = viewRef.current;
        if (v.scale > 1.01) setView(FIT);
        else setView({ scale: Math.max(1.5, m.natW / m.fitW), cx: v.cx, cy: v.cy });
      },
      cycleFace: (dir) => {
        const m = metrics.current;
        if (!m || faces.length === 0) return;
        const order = [...faces.keys()].sort((p, q) => Number(faces[q].primary) - Number(faces[p].primary) || p - q);
        const cur = faceIdx.current;
        let next = cur + dir;
        if (next >= order.length || next < -1) next = dir > 0 ? -1 : order.length - 1;
        faceIdx.current = next;
        if (next < 0) return setView(FIT);
        const b = faces[order[next]].bbox;
        const scale = Math.min(16, Math.max(1.5, (0.4 * m.cw) / (m.fitW * b.width), (0.4 * m.ch) / (m.fitH * b.height)));
        setView({ scale: Math.min(scale, 16), cx: b.x + b.width / 2, cy: b.y + b.height / 2 });
      },
      resetView: () => setView(FIT),
      cycleInfo: () => setInfo((i) => (i === "full" ? "name" : i === "name" ? "off" : "full")),
    }),
    [faces],
  );

  const zoomLabel = view.scale <= 1.001 ? "Fit" : `${Math.round((metrics.current ? (metrics.current.fitW / metrics.current.natW) * view.scale : view.scale) * 100)}%`;

  return (
    <div className="absolute inset-0 z-10 flex flex-col bg-neutral-950" data-testid={mode === "compare" ? "compare" : "loupe"}>
      <div className="relative flex min-h-0 flex-1 gap-1">
        {mode === "compare" && compare ? (
          <>
            {(["a", "b"] as const).map((k) => {
              const id = compare[k];
              return (
                <div
                  key={k}
                  className={`relative min-w-0 flex-1 border-2 ${compare.focus === k ? "border-sky-500" : "border-transparent"}`}
                  data-testid={`compare-pane-${k}`}
                  data-image-id={id}
                >
                  <ZoomPane entry={lib.getEntry(id)} version={lib.version(id)} view={view} onView={setView} metricsRef={metrics} onFocus={() => onFocusPane(k)} testId={`zoom-${k}`} />
                  <InfoOverlay entry={lib.getEntry(id)} level={info} showKeeper={mode === "compare"} onLocate={onLocate} />
                </div>
              );
            })}
          </>
        ) : activeId != null ? (
          <div className="relative min-w-0 flex-1">
            <ZoomPane entry={lib.getEntry(activeId)} version={lib.version(activeId)} view={view} onView={setView} metricsRef={metrics} testId="zoom-a" />
            <InfoOverlay entry={lib.getEntry(activeId)} level={info} showKeeper={false} onLocate={onLocate} />
          </div>
        ) : null}
        <div
          className="pointer-events-none absolute bottom-2 right-3 rounded bg-black/70 px-2 py-0.5 text-xs text-neutral-200"
          data-testid="zoom-label"
        >
          {zoomLabel}
          {faces.length > 0 ? ` · ${faces.length} face${faces.length > 1 ? "s" : ""}` : ""}
        </div>
      </div>
      {mode === "loupe" && !panels.chrome && (
        <Filmstrip lib={lib} activeId={activeId} onPick={(id) => onOpen(id)} cellW={80} cellH={64} height={72} scenePrefix="loupe-film-scene" align="center" />
      )}
    </div>
  );
});

type InfoLevel = "full" | "name" | "off";

const PICK_LABEL = { pick: "Pick", reject: "Reject", unflagged: "Unflagged" } as const;

/** "Suggested: Reject · 2★" when the automatic suggestion differs from what is set. */
function suggestion(entry: RawImageEntry): string | null {
  const q = entry.quality;
  if (!q) return null;
  if (q.suggestedPick === entry.pick && q.suggestedRating === entry.rating) return null;
  return `Suggested: ${PICK_LABEL[q.suggestedPick]} · ${q.suggestedRating}★`;
}

function InfoOverlay({ entry, level, showKeeper, onLocate }: { entry: RawImageEntry | undefined; level: InfoLevel; showKeeper: boolean; onLocate?: (id: number) => void }) {
  if (!entry || level === "off") return null;
  if (level === "name")
    return (
      <div className="pointer-events-none absolute left-2 top-2 rounded bg-black/60 px-2 py-1 text-xs font-medium text-neutral-100" data-testid="info-overlay" data-level="name">
        {entry.fileName}
      </div>
    );
  const suggested = suggestion(entry);
  const c = entry.capture;
  const exif = [
    c.iso != null ? `ISO ${c.iso}` : null,
    c.shutterSeconds != null ? formatShutter(c.shutterSeconds) : null,
    c.aperture != null ? `f/${trimNum(c.aperture)}` : null,
    c.focalLengthMm != null ? `${trimNum(c.focalLengthMm)}mm` : null,
  ].filter(Boolean);
  const tags = entry.tags.filter((t) => !t.suppressed);
  return (
    <div className="pointer-events-none absolute left-2 top-2 flex max-w-[90%] flex-col gap-1 rounded bg-black/60 px-2 py-1 text-xs" data-testid="info-overlay" data-level="full">
      <div className="flex items-center gap-2">
        <span className="font-medium text-neutral-100">{entry.fileName}</span>
        {entry.pick === "pick" && <Flag className="size-3.5 fill-green-500 text-green-500" />}
        {entry.pick === "reject" && <X className="size-4 text-red-500" strokeWidth={3} />}
        {entry.colorLabel && <span className={`size-2.5 rounded-full ${LABEL_COLOR[entry.colorLabel]}`} />}
        <Stars n={entry.rating} />
        <HealthBadge entry={entry} testPrefix="loupe-health" />
        {onLocate && entry.missingSinceMs != null && (
          <button onClick={() => onLocate(entry.id)} data-testid="loupe-locate" className="pointer-events-auto rounded bg-neutral-800 px-1.5 text-[10px] text-neutral-200 hover:bg-neutral-700">
            Locate folder…
          </button>
        )}
        <XmpBadge entry={entry} />
        <CompanionBadge entry={entry} testPrefix="loupe-companion" />
        {showKeeper && entry.isBurstKeeper && (
          <span className="rounded bg-green-900 px-1.5 text-green-200" data-testid="keeper-badge">
            Keeper
          </span>
        )}
      </div>
      {suggested && (
        <div className="text-sky-300" data-testid="suggested-line">
          {suggested}
        </div>
      )}
      <div className="text-neutral-400">
        {exif.join(" · ")}
        {entry.quality ? ` · Q ${Math.round(entry.quality.overall * 100)}` : ""}
        {entry.burstGroupId != null ? ` · burst #${entry.burstGroupId}${entry.isBurstKeeper ? " keeper" : ""}` : ""}
      </div>
      {tags.length > 0 && (
        <div className="flex gap-1">
          {tags.map((t) => (
            <span key={t.tag} className={`rounded px-1 ${TAG_STYLE[t.tag]}`}>
              {tagName(t.tag)}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}
