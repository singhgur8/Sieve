// Small parts shared by the steps of "Pick the best N".
import { useCallback, useRef, useState, type ReactNode } from "react";
import { ImageOff, Loader2 } from "lucide-react";
import type { ImageSelection, RawImageEntry, ShotType } from "../../ipc";
import { previewSrc, thumbSrc } from "../../lib/entryImage";
import { SHOT_HINT, SHOT_LABEL, SHOT_STYLE } from "../../lib/target";
import type { ActionId } from "../../lib/keymap";
import type { StageKey } from "./types";
import { FIT, ZoomPane, scaleForPct, zoomAt, type Metrics, type View } from "../ZoomPane";

/** A keyboard key as a small cap, e.g. next to a button label. */
export function Key({ children }: { children: ReactNode }) {
  return <kbd className="rounded border border-neutral-600 bg-neutral-800 px-1 text-[10px] font-semibold leading-4 text-neutral-200">{children}</kbd>;
}

export function ShotBadge({ type, testid }: { type: ShotType | null; testid?: string }) {
  if (!type) return null;
  return (
    <span className={`rounded px-1.5 py-0.5 text-[11px] font-semibold ${SHOT_STYLE[type]}`} data-testid={testid} data-shot={type} title={`${SHOT_LABEL[type]}: ${SHOT_HINT[type]}`}>
      {SHOT_LABEL[type]}
    </span>
  );
}

/** The photo's image at thumbnail or preview size; a spinner while the entry loads. */
export function Pic({ entry, big = false, className = "", testid }: { entry: RawImageEntry | undefined; big?: boolean; className?: string; testid?: string }) {
  const src = entry ? (big ? previewSrc(entry) : thumbSrc(entry)) : null;
  const [broken, setBroken] = useState<string | null>(null);
  return (
    <div className={`relative flex items-center justify-center overflow-hidden bg-neutral-900 ${className}`} data-testid={testid} data-id={entry?.id}>
      {src && broken !== src ? (
        <img src={src} alt={entry?.fileName} draggable={false} decoding="async" className={`size-full object-contain ${entry?.pick === "reject" ? "opacity-40" : ""}`} onError={() => setBroken(src)} />
      ) : entry ? (
        <ImageOff className="size-6 text-neutral-500" aria-label="No preview" />
      ) : (
        <Loader2 className="size-5 animate-spin text-neutral-700" />
      )}
    </div>
  );
}

/** Reasons of a selection row as one line each (the first is the main one). */
export function Reasons({ sel, max = 2 }: { sel: ImageSelection | undefined; max?: number }) {
  if (!sel || sel.reasons.length === 0) return null;
  return (
    <ul className="space-y-0.5" data-testid="target-reasons">
      {sel.reasons.slice(0, max).map((r, i) => (
        <li key={i} className={i === 0 ? "text-neutral-100" : "text-neutral-400"} data-kind={r.kind}>
          {r.text}
        </li>
      ))}
    </ul>
  );
}

/** A button that shows its key, e.g. "Swap in (S)". */
export function ActionButton({
  testid,
  label,
  keys,
  title,
  onClick,
  tone = "neutral",
  disabled,
}: {
  testid: string;
  label: string;
  keys: string[];
  title: string;
  onClick: () => void;
  tone?: "neutral" | "good" | "bad" | "primary";
  disabled?: boolean;
}) {
  const color = { neutral: "bg-neutral-800 hover:bg-neutral-700", good: "bg-emerald-800 hover:bg-emerald-700", bad: "bg-red-900 hover:bg-red-800", primary: "bg-sky-700 hover:bg-sky-600" }[tone];
  return (
    <button className={`flex h-9 items-center gap-2 whitespace-nowrap rounded-md px-3 text-sm text-neutral-50 disabled:opacity-40 ${color}`} data-testid={testid} title={title} aria-label={`${label}. ${title}`} disabled={disabled} onClick={onClick}>
      {label}
      {keys.map((k) => (
        <Key key={k}>{k}</Key>
      ))}
    </button>
  );
}

/** What a step shows for a key; the shell calls it for every overlay key. Returns true when the key was used. */
export type StageHandler = (id: ActionId, e: StageKey) => boolean;

/**
 * Zoom shared by two panes (P1-7): the same view (scale + normalised centre) drives both, so 100% lands on the same point of both
 * photos and survives Left / Right. `toggle` is Fit <-> 100% at a pane point (or the centre).
 */
export function useSyncedZoom() {
  const [view, setView] = useState<View>(FIT);
  const metrics = useRef<Metrics | null>(null);
  const [, bump] = useState(0);
  const toggle = useCallback((at?: { x: number; y: number }) => {
    const m = metrics.current;
    setView((v) => {
      if (v.scale > 1.001) return FIT;
      if (!m) return v;
      const a = at ?? m.hover ?? { x: m.cw / 2, y: m.ch / 2 };
      return zoomAt(m, Math.max(1.5, scaleForPct(m, 100)), a.x, a.y, 100);
    });
  }, []);
  return { view, setView, metrics, toggle, measured: () => bump((n) => n + 1) };
}
export type SyncedZoom = ReturnType<typeof useSyncedZoom>;

/** A pane of two that zoom together. `primary` owns the shared metrics (the one whose pixels define "100%"). */
export function ZoomPic({ entry, zoom, primary = false, className = "", testid }: { entry: RawImageEntry | undefined; zoom: SyncedZoom; primary?: boolean; className?: string; testid?: string }) {
  const own = useRef<Metrics | null>(null);
  return (
    <div className={`relative overflow-hidden bg-neutral-900 ${className}`} data-testid={testid} data-id={entry?.id} data-zoom={zoom.view.scale > 1.001 ? "100" : "fit"} data-cx={zoom.view.cx.toFixed(3)} data-cy={zoom.view.cy.toFixed(3)}>
      <ZoomPane
        entry={entry}
        version={0}
        view={zoom.view}
        onView={zoom.setView}
        metricsRef={primary ? zoom.metrics : own}
        onMeasured={primary ? zoom.measured : undefined}
        rescaleActual={primary}
        onClickZoom={(x, y) => zoom.toggle({ x, y })}
      />
    </div>
  );
}
