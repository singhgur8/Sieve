import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from "react";
import { Flag, X } from "lucide-react";
import { commands, unwrap, type FaceInfo, type RawImageEntry } from "../ipc";
import type { Library } from "../hooks/useLibrary";
import { formatShutter, formatTime, LABEL_COLOR, tagName, TAG_STYLE, trimNum } from "../lib/format";
import { BurstBadge, CompanionBadge, HealthBadge, Stars, XmpBadge } from "./Cell";
import { flagTitle, rejectInfo, tagTitle } from "../lib/cull";
import { Filmstrip } from "./Filmstrip";
import { CompareBar, CompareTag } from "./CompareBar";
import { usePanels } from "../lib/panels";
import { usePrefetchNeighbours } from "../hooks/usePrefetch";
import { FIT, ZoomPane, fillScale, scaleForPct, zoomAt, type Metrics, type View } from "./ZoomPane";
import { ZOOM_PRESETS, type ZoomPreset } from "../lib/zoom";

/** Compare pair: `a` is the Select (the keeper so far), `b` the Candidate. `focus` is the active pane. */
export interface CompareState {
  a: number;
  b: number;
  focus: "a" | "b";
}

export interface LoupeHandle {
  /** Space: Fit <-> 100% at the point under the cursor. */
  toggleZoom: () => void;
  setPreset: (p: ZoomPreset) => void;
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
  /** Compare: filmstrip click picks the Candidate. */
  onCandidate?: (id: number) => void;
  onSwap?: () => void;
  onMakeSelect?: () => void;
  onEditCompare?: () => void;
  onExitCompare?: () => void;
  /** Click on a star: rate that photo (0 clears). */
  onRate?: (id: number, rating: number) => void;
  /** "Locate folder…" for the folder of image `imageId`. */
  onLocate?: (imageId: number) => void;
  /** Burst sizes by group id (burst badge text). */
  burstSizes?: Map<number, number>;
}

/** Full-area loupe / 2-up compare. Owns zoom/pan state so pans do not re-render the grid. */
export const LoupeLayer = forwardRef<LoupeHandle, Props>(function LoupeLayer({ mode, lib, activeId, compare, onFocusPane, onOpen, onCandidate, onSwap, onMakeSelect, onEditCompare, onExitCompare, onRate, onLocate, burstSizes }, ref) {
  const [view, setView] = useState<View>(FIT);
  const [faces, setFaces] = useState<FaceInfo[]>([]);
  const [info, setInfo] = useState<InfoLevel>("full");
  const faceIdx = useRef(-1);
  const metrics = useRef<Metrics | null>(null);
  const panels = usePanels("loupe");
  const viewRef = useRef(view);
  viewRef.current = view;

  // Zoom level and relative position persist while stepping through photos (Lightroom): only face stepping resets.
  const navKey = `${mode}:${activeId}`;
  useEffect(() => {
    faceIdx.current = -1;
  }, [navKey]);
  usePrefetchNeighbours(lib, activeId);

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

  /** Fit <-> 100%, anchored at the cursor (pane centre when the pointer is outside the pane). */
  const toggle = (at?: { x: number; y: number }) => {
    const m = metrics.current;
    if (!m) return;
    const v = viewRef.current;
    if (v.scale > 1.001) return setView(FIT);
    const a = at ?? m.hover ?? { x: m.cw / 2, y: m.ch / 2 };
    setView(zoomAt(m, Math.max(1.5, scaleForPct(m, 100)), a.x, a.y, 100));
  };
  const setPreset = (p: ZoomPreset) => {
    const m = metrics.current;
    if (!m) return;
    if (p === "fit") return setView(FIT);
    const pct = typeof p === "number" ? p : undefined;
    const scale = typeof p === "number" ? scaleForPct(m, p) : fillScale(m);
    // Buttons keep the centre of what is on screen in place.
    setView(zoomAt(m, scale, m.cw / 2, m.ch / 2, pct));
  };
  const presetActive = (p: ZoomPreset) => {
    const m = metrics.current;
    if (p === "fit") return view.scale <= 1.001;
    if (!m || view.scale <= 1.001) return false;
    if (p === "fill") return Math.abs(view.scale - fillScale(m)) < 0.01;
    return Math.abs((m.fitW / m.natW) * view.scale * 100 - p) < 1;
  };

  useImperativeHandle(
    ref,
    () => ({
      toggleZoom: () => toggle(),
      setPreset,
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

  const posId = mode === "compare" && compare ? compare[compare.focus] : activeId;
  const posIdx = posId != null ? lib.ids.indexOf(posId) : -1;
  const zoomLabel = view.scale <= 1.001 ? "Fit" : `${Math.round((metrics.current ? (metrics.current.fitW / metrics.current.natW) * view.scale : view.scale) * 100)}%`;

  return (
    <div className="absolute inset-0 z-10 flex flex-col bg-neutral-950" data-testid={mode === "compare" ? "compare" : "loupe"}>
      {mode === "compare" && compare && (
        <CompareBar
          focus={compare.focus}
          aName={lib.getEntry(compare.a)?.fileName ?? ""}
          bName={lib.getEntry(compare.b)?.fileName ?? ""}
          onSwap={() => onSwap?.()}
          onMakeSelect={() => onMakeSelect?.()}
          onFocus={onFocusPane}
          onEdit={onEditCompare}
          onExit={onExitCompare}
        />
      )}
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
                  <ZoomPane entry={lib.getEntry(id)} version={lib.version(id)} view={view} onView={setView} metricsRef={metrics} onFocus={() => onFocusPane(k)} onClickZoom={(x, y) => toggle({ x, y })} testId={`zoom-${k}`} rescaleActual={k === "b"} />
                  <InfoOverlay entry={lib.getEntry(id)} level={info} showKeeper={mode === "compare"} onLocate={onLocate} onRate={onRate} burstSizes={burstSizes} />
                </div>
              );
            })}
          </>
        ) : activeId != null ? (
          <div className="relative min-w-0 flex-1">
            <ZoomPane entry={lib.getEntry(activeId)} version={lib.version(activeId)} view={view} onView={setView} metricsRef={metrics} onClickZoom={(x, y) => toggle({ x, y })} testId="zoom-a" />
            <InfoOverlay entry={lib.getEntry(activeId)} level={info} showKeeper={false} onLocate={onLocate} onRate={onRate} burstSizes={burstSizes} />
          </div>
        ) : null}
        {posIdx >= 0 && (
          <div
            className="absolute bottom-2 left-3 rounded bg-black/70 px-2 py-0.5 text-xs text-neutral-200"
            data-testid="loupe-position"
            title={`Photo ${posIdx + 1} of ${lib.ids.length} in the current view (filters apply)`}
            aria-label={`Photo ${posIdx + 1} of ${lib.ids.length}`}
          >
            {posIdx + 1} of {lib.ids.length}
          </div>
        )}
        <div className="absolute bottom-2 right-3 flex items-center gap-2">
          <div className="flex gap-px" role="group" aria-label="Zoom presets" data-testid="zoom-presets">
            {ZOOM_PRESETS.map((z) => (
              <button
                key={String(z.id)}
                type="button"
                tabIndex={-1}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => setPreset(z.id)}
                aria-pressed={presetActive(z.id)}
                data-testid={`zoom-preset-${z.id}`}
                title={`${z.title}${z.id === "fit" ? ". Space toggles Fit and 100% at the cursor" : ""}`}
                className={`rounded bg-black/70 px-1.5 py-0.5 text-xs ${presetActive(z.id) ? "text-white ring-1 ring-sky-500" : "text-neutral-400 hover:text-neutral-100"}`}
              >
                {z.label}
              </button>
            ))}
          </div>
          <div
            className="rounded bg-black/70 px-2 py-0.5 text-xs text-neutral-200"
            title={`Zoom level${faces.length > 0 ? `, ${faces.length} face${faces.length > 1 ? "s" : ""} found (F steps through them)` : ""}. Space or a click zooms to 100% at the cursor and back to Fit; zoom and position stay while you step through photos`}
            data-testid="zoom-label"
          >
            {zoomLabel}
            {faces.length > 0 ? ` · ${faces.length} face${faces.length > 1 ? "s" : ""}` : ""}
          </div>
        </div>
      </div>
      {mode === "compare" && compare && (
        <Filmstrip
          lib={lib}
          activeId={compare.b}
          selected={new Set([compare.a])}
          marked={new Set([compare.b])}
          onPick={(id) => onCandidate?.(id)}
          onRate={onRate}
          badge={(id) => <CompareTag id={id} a={compare.a} b={compare.b} />}
          cellW={64}
          cellH={48}
          height={60}
          scenePrefix="compare-film-scene"
          align="center"
        />
      )}
      {mode === "loupe" && !panels.chrome && (
        <Filmstrip lib={lib} activeId={activeId} onPick={(id) => onOpen(id)} onRate={onRate} cellW={80} cellH={64} height={72} scenePrefix="loupe-film-scene" align="center" />
      )}
    </div>
  );
});

type InfoLevel = "full" | "name" | "off";

const PICK_LABEL = { pick: "Pick", reject: "Reject", unflagged: "Unflagged" } as const;

/** "Sieve suggests reject" / "Suggested: Pick · 2★" when the automatic suggestion differs from what is set. */
function suggestion(entry: RawImageEntry): string | null {
  const q = entry.quality;
  if (!q) return null;
  if (q.suggestedPick === entry.pick && q.suggestedRating === entry.rating) return null;
  if (q.suggestedPick === "reject") return `Sieve suggests reject${q.suggestedRating > 0 ? ` · ${q.suggestedRating}★` : ""}`;
  return `Suggested: ${PICK_LABEL[q.suggestedPick]} · ${q.suggestedRating}★`;
}

function InfoOverlay({
  entry,
  level,
  showKeeper,
  onLocate,
  onRate,
  burstSizes,
}: {
  entry: RawImageEntry | undefined;
  level: InfoLevel;
  showKeeper: boolean;
  onLocate?: (id: number) => void;
  onRate?: (id: number, rating: number) => void;
  burstSizes?: Map<number, number>;
}) {
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
  const reject = rejectInfo(entry);
  const reasons = entry.quality?.reasons ?? [];
  const burstSize = entry.burstGroupId != null ? burstSizes?.get(entry.burstGroupId) : undefined;
  return (
    <div className="pointer-events-none absolute left-2 top-2 flex max-w-[90%] flex-col gap-1 rounded bg-black/60 px-2 py-1 text-xs" data-testid="info-overlay" data-level="full">
      <div className="flex items-center gap-2">
        <span className="font-medium text-neutral-100">{entry.fileName}</span>
        {entry.pick === "pick" && (
          <span className="pointer-events-auto" title={flagTitle(entry)} aria-label={flagTitle(entry)} data-testid="loupe-flag">
            <Flag className="size-3.5 fill-green-500 text-green-500" />
          </span>
        )}
        {entry.pick === "reject" && (
          <span className="pointer-events-auto" title={flagTitle(entry)} aria-label={flagTitle(entry)} data-testid="loupe-flag">
            <X className="size-4 text-red-500" strokeWidth={3} />
          </span>
        )}
        {entry.colorLabel && <span className={`pointer-events-auto size-2.5 rounded-full ${LABEL_COLOR[entry.colorLabel]}`} title={`Color label: ${entry.colorLabel}`} aria-label={`Color label ${entry.colorLabel}`} />}
        <Stars n={entry.rating} className="size-3.5" onRate={onRate && ((r) => onRate(entry.id, r))} testId={`loupe-stars-${entry.id}`} />
        <span className="pointer-events-auto flex items-center gap-2">
          <HealthBadge entry={entry} testPrefix="loupe-health" />
          <XmpBadge entry={entry} showSaved />
          <CompanionBadge entry={entry} testPrefix="loupe-companion" />
          <BurstBadge entry={entry} size={burstSize} testPrefix="loupe-burst" />
        </span>
        {onLocate && entry.missingSinceMs != null && (
          <button onClick={() => onLocate(entry.id)} data-testid="loupe-locate" title="Point the folder of this photo at where it was moved" className="pointer-events-auto rounded bg-neutral-800 px-1.5 text-[10px] text-neutral-200 hover:bg-neutral-700">
            Locate folder…
          </button>
        )}
        {showKeeper && entry.isBurstKeeper && (
          <span className="pointer-events-auto rounded bg-green-900 px-1.5 text-green-200" title="Best of its burst: Sieve's choice, the others are duplicates" data-testid="keeper-badge">
            Best of burst
          </span>
        )}
      </div>
      {reject && (
        <div className="text-red-300" data-testid="loupe-reject-reason" data-origin={reject.who} title="Why this photo is rejected">
          <b>{reject.origin}</b>
          {reject.reasons.length > 0 ? `${reject.who === "auto" ? ": " : ". Sieve noted: "}${reject.reasons.join("; ")}` : ""}
        </div>
      )}
      {suggested && (
        <div className="text-sky-300" data-testid="suggested-line" title="What Apply suggestions would set. Nothing changes until you apply it">
          {suggested}
          {!reject && entry.quality?.suggestedPick === "reject" && reasons.length > 0 ? `: ${reasons.map((r) => r.text).join("; ")}` : ""}
        </div>
      )}
      {c.capturedAtMs != null && (
        <div className="text-neutral-300" data-testid="info-capture-time" data-source={c.captureTimeSource ?? "exif"} title={c.captureTimeSource && c.captureTimeSource !== "exif" ? `Corrected capture time (the file says ${c.originalCapturedAtMs != null ? formatTime(c.originalCapturedAtMs) : "nothing"})` : "Capture time from the file"}>
          {formatTime(c.capturedAtMs)}
          {c.captureTimeSource && c.captureTimeSource !== "exif" ? " (corrected)" : ""}
        </div>
      )}
      <div className="text-neutral-400" title="Capture settings, Sieve's quality score (0-100) and burst membership">
        {exif.join(" · ")}
        {entry.quality ? ` · Q ${Math.round(entry.quality.overall * 100)}` : ""}
        {entry.burstGroupId != null ? ` · burst #${entry.burstGroupId}${entry.isBurstKeeper ? " · best" : ""}` : ""}
      </div>
      {tags.length > 0 && (
        <div className="flex gap-1">
          {tags.map((t) => (
            <span key={t.tag} className={`pointer-events-auto rounded px-1 ${TAG_STYLE[t.tag]}`} title={tagTitle(entry, t.tag)} aria-label={tagTitle(entry, t.tag)}>
              {tagName(t.tag)}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}
