import { memo, useState } from "react";
import { AlertTriangle, Anchor, CloudUpload, FileCheck, Flag, ImageOff, Layers, Loader2, Star, Unplug, X } from "lucide-react";
import { convertFileSrc, type RawImageEntry } from "../ipc";
import { useEntryHealth } from "../lib/errors";
import { formatTime, LABEL_COLOR, TAG_SHORT, TAG_STYLE } from "../lib/format";
import { flagTitle, rejectInfo, sidecarName, suggestedReject, tagTitle } from "../lib/cull";

interface Props {
  id: number;
  entry: RawImageEntry | undefined;
  version: number;
  size: number;
  selected: boolean;
  active: boolean;
  onClick: (id: number, e: React.MouseEvent) => void;
  onDoubleClick: (id: number) => void;
  /** Click on a star: rate this photo only (0 clears). */
  onRate?: (id: number, rating: number) => void;
  /** Size of the photo's burst (for the badge text). */
  burstSize?: number;
}

const stopAll = (e: React.SyntheticEvent) => e.stopPropagation();

/**
 * Star row. With `onRate` each star is a button: click sets that rating, clicking the current rating clears it,
 * hovering previews the result. Events never bubble, so rating does not change the selection or open the loupe.
 */
export function Stars({ n, className = "size-3", onRate, testId }: { n: number; className?: string; onRate?: (rating: number) => void; testId?: string }) {
  const [hover, setHover] = useState(0);
  if (!onRate)
    return (
      <span className="flex" aria-label={`${n} stars`}>
        {[1, 2, 3, 4, 5].map((i) => (
          <Star key={i} className={`${className} ${i <= n ? "fill-amber-400 text-amber-400" : "text-neutral-700"}`} />
        ))}
      </span>
    );
  const shown = hover || n;
  return (
    <span
      className="pointer-events-auto flex"
      role="group"
      aria-label={`${n} stars`}
      data-testid={testId}
      data-rating={n}
      onMouseLeave={() => setHover(0)}
      onClick={stopAll}
      onDoubleClick={stopAll}
      onMouseDown={stopAll}
      onPointerDown={stopAll}
    >
      {[1, 2, 3, 4, 5].map((i) => (
        <button
          key={i}
          type="button"
          tabIndex={-1}
          aria-label={i === n ? `Clear rating (${i} stars)` : `Rate ${i} star${i === 1 ? "" : "s"}`}
          title={i === n ? "Click to clear the rating" : `${i} star${i === 1 ? "" : "s"}`}
          data-testid={testId ? `${testId}-${i}` : undefined}
          data-filled={i <= shown}
          className="cursor-pointer p-0.5"
          onMouseEnter={() => setHover(i === n ? 0 : i)}
          onClick={(e) => {
            e.stopPropagation();
            setHover(0);
            onRate(i === n ? 0 : i);
          }}
        >
          <Star className={`${className} ${i <= shown ? (hover ? "fill-amber-200 text-amber-200" : "fill-amber-400 text-amber-400") : "text-neutral-400"}`} />
        </button>
      ))}
    </span>
  );
}

/** "Missing" (original moved / drive disconnected) or "Unreadable" (decode failed), from what the backend reported. */
export function HealthBadge({ entry, testPrefix = "health" }: { entry: RawImageEntry; testPrefix?: string }) {
  const h = useEntryHealth(entry);
  if (!h) return null;
  const missing = h.kind === "missing";
  return (
    <span
      title={h.message}
      data-testid={`${testPrefix}-${entry.id}`}
      data-health={h.kind}
      className={`flex items-center gap-0.5 rounded px-1 text-[10px] font-semibold ${missing ? "bg-amber-800 text-amber-100" : "bg-red-900 text-red-100"}`}
    >
      {missing ? <Unplug className="size-3" /> : <ImageOff className="size-3" />}
      {missing ? "Missing" : "Unreadable"}
    </span>
  );
}

/**
 * Sidecar state of one photo. Errors and unsaved changes always show; with `showSaved` a photo whose sidecar is
 * up to date gets a quiet check mark too ("Saved to DSC0001.xmp next to the original").
 */
export function XmpBadge({ entry, showSaved = false }: { entry: RawImageEntry; showSaved?: boolean }) {
  const sidecar = sidecarName(entry.fileName);
  if (entry.xmp.error)
    return (
      <span title={`${sidecar} could not be written: ${entry.xmp.error}`} aria-label={`${sidecar} could not be written`} data-xmp="error" data-testid={`xmp-error-${entry.id}`} className="text-red-400">
        <AlertTriangle className="size-3.5" />
      </span>
    );
  if (entry.xmp.dirty)
    return (
      <span title={`Changes not saved to ${sidecar} yet (Save, or wait for auto-sync)`} aria-label={`Changes not saved to ${sidecar} yet`} data-xmp="dirty" className="text-amber-400">
        <CloudUpload className="size-3.5" />
      </span>
    );
  if (showSaved && entry.xmp.syncedAtMs != null)
    return (
      <span title={`Saved to ${sidecar} next to the original`} aria-label={`Saved to ${sidecar}`} data-xmp="saved" data-testid={`xmp-saved-${entry.id}`} className="text-emerald-500">
        <FileCheck className="size-3.5" />
      </span>
    );
  return null;
}

export function SceneBadge({ entry, testPrefix = "scene-badge" }: { entry: RawImageEntry; testPrefix?: string }) {
  if (entry.sceneId == null) return null;
  return (
    <span
      title={entry.isSceneAnchor ? `Scene ${entry.sceneId}: the anchor photo (graded by you; the other frames of the scene are matched to it)` : `Scene ${entry.sceneId}: photos shot in the same lighting`}
      aria-label={entry.isSceneAnchor ? `Scene ${entry.sceneId} anchor` : `Scene ${entry.sceneId}`}
      data-testid={`${testPrefix}-${entry.id}`}
      data-anchor={entry.isSceneAnchor}
      className={`flex items-center gap-0.5 rounded px-1 text-[10px] ${entry.isSceneAnchor ? "bg-amber-800 text-amber-100" : "bg-emerald-950/90 text-emerald-200"}`}
    >
      {entry.isSceneAnchor && <Anchor className="size-3" />}S{entry.sceneId}
    </span>
  );
}

/** RAW with a camera JPEG sibling paired at import (`ImportOptions.pairJpegWithRaw`). */
export function CompanionBadge({ entry, testPrefix = "companion" }: { entry: RawImageEntry; testPrefix?: string }) {
  if (!entry.companionPath) return null;
  const name = entry.companionPath.split("/").pop();
  return (
    <span title={`Camera JPEG ${name} is paired with this RAW (shown as one photo)`} aria-label={`Paired with ${name}`} data-testid={`${testPrefix}-${entry.id}`} className="rounded bg-neutral-800/90 px-1 text-[10px] font-semibold text-neutral-200">
      +JPG
    </span>
  );
}

/** "Burst of 5 — best frame" / "Burst of 5 — not the best frame". ("Keeper" is reserved for the Edit / Export set.) */
export function BurstBadge({ entry, size, testPrefix = "burst-badge" }: { entry: RawImageEntry; size?: number; testPrefix?: string }) {
  if (entry.burstGroupId == null) return null;
  const of = size ? `Burst of ${size}` : `Burst #${entry.burstGroupId}`;
  const dup = entry.quality?.reasons?.find((r) => r.kind === "duplicate_burst")?.text;
  const title = entry.isBurstKeeper ? `${of} — best frame (Sieve's choice)` : `${of} — not the best frame${dup ? `. ${dup}` : ""}`;
  return (
    <span
      title={title}
      aria-label={title}
      data-testid={`${testPrefix}-${entry.id}`}
      className={`flex items-center gap-0.5 rounded px-1 text-[10px] ${entry.isBurstKeeper ? "bg-green-900 text-green-200" : "bg-neutral-800/90 text-neutral-300"}`}
    >
      <Layers className="size-3" />
      {entry.isBurstKeeper ? "★" : ""}
    </span>
  );
}

/** Cached thumbnail; when the file cannot be loaded (cache folder cleaned, drive offline) a placeholder replaces the broken image. */
function Thumb({ src, name, dim }: { src: string; name: string | undefined; dim: boolean }) {
  const [broken, setBroken] = useState<string | null>(null);
  if (broken === src)
    return (
      <div className={`flex size-full flex-col items-center justify-center gap-1 text-center ${dim ? "opacity-40" : ""}`} data-testid="thumb-broken" title="The cached preview could not be loaded. Regenerate previews from the More menu, or re-import the folder.">
        <ImageOff className="size-5 text-neutral-400" />
        <span className="text-[10px] text-neutral-400">Preview unavailable</span>
      </div>
    );
  return <img src={src} decoding="async" draggable={false} alt={name} className={`size-full object-contain ${dim ? "opacity-40" : ""}`} onError={() => setBroken(src)} />;
}

export const Cell = memo(function Cell({ id, entry, version, size, selected, active, onClick, onDoubleClick, onRate, burstSize }: Props) {
  const t = entry?.thumbnail;
  const compact = size < 150;
  const dim = entry?.pick === "reject";
  const tags = entry?.tags.filter((x) => !x.suppressed) ?? [];
  const reject = entry && !compact ? rejectInfo(entry) : null;
  const suggested = entry && !compact && !reject ? suggestedReject(entry) : null;
  return (
    <div
      data-testid={`cell-${id}`}
      data-selected={selected}
      data-active={active}
      data-pick={entry?.pick}
      data-rating={entry?.rating}
      title={entry ? (entry.capture.capturedAtMs != null ? `${entry.fileName} · ${formatTime(entry.capture.capturedAtMs)}${entry.capture.captureTimeSource && entry.capture.captureTimeSource !== "exif" ? " (corrected)" : ""}` : entry.fileName) : undefined}
      onClick={(e) => onClick(id, e)}
      onDoubleClick={() => onDoubleClick(id)}
      className={`relative select-none overflow-hidden rounded bg-neutral-900 ${
        selected ? "ring-2 ring-sky-500" : ""
      } ${active ? "outline outline-2 -outline-offset-2 outline-white" : ""}`}
      style={{ contain: "strict" }}
    >
      {t?.status === "ready" ? (
        <Thumb src={`${convertFileSrc(t.path)}?v=${version}`} name={entry?.fileName} dim={dim} />
      ) : t?.status === "failed" ? (
        <div className={`flex size-full flex-col items-center justify-center gap-1 px-1 text-center ${dim ? "opacity-40" : ""}`} title={t.reason} data-testid={`thumb-failed-${id}`}>
          <ImageOff className="size-5 text-red-500" />
          {!compact && <span className="text-[10px] text-red-300">No preview</span>}
        </div>
      ) : (
        <div className={`flex size-full items-center justify-center ${dim ? "opacity-40" : ""}`}>
          <Loader2 className={`size-5 text-neutral-700 ${entry ? "animate-spin" : ""}`} />
        </div>
      )}
      {entry && (
        <>
          <div className="absolute left-1 top-1 flex items-center gap-1">
            {entry.pick === "pick" && (
              <span title={flagTitle(entry)} aria-label={flagTitle(entry)} data-testid={`flag-${id}`}>
                <Flag className="size-3.5 fill-green-500 text-green-500" />
              </span>
            )}
            {entry.pick === "reject" && (
              <span title={flagTitle(entry)} aria-label={flagTitle(entry)} data-testid={`flag-${id}`}>
                <X className="size-4 text-red-500" strokeWidth={3} />
              </span>
            )}
            {entry.pick === "reject" && compact && entry.pickOrigin === "auto" && (
              <span title="Auto-rejected by Sieve" aria-label="Auto-rejected" data-testid={`auto-chip-${id}`} className="rounded bg-red-950/85 px-0.5 text-[9px] font-bold leading-3 text-red-100">
                A
              </span>
            )}
            {entry.colorLabel && <span className={`size-2.5 rounded-full ${LABEL_COLOR[entry.colorLabel]}`} title={`Color label: ${entry.colorLabel}`} aria-label={`Color label ${entry.colorLabel}`} />}
          </div>
          <div className="absolute right-1 top-1 flex items-center gap-1">
            <HealthBadge entry={entry} />
            <XmpBadge entry={entry} />
            <CompanionBadge entry={entry} />
            <SceneBadge entry={entry} />
            <BurstBadge entry={entry} size={burstSize} />
          </div>
          {reject ? (
            <div
              className={`absolute inset-x-0 bottom-5 bg-red-950/85 px-1 text-[10px] leading-4 text-red-100 ${size >= 200 ? "line-clamp-2" : "truncate"}`}
              data-testid={`reject-reason-${id}`}
              data-origin={reject.who}
              title={`${reject.origin}${reject.reasons.length ? `: ${reject.reasons.join("; ")}` : ""}`}
            >
              <b>{reject.origin}</b>
              {reject.headline ? `${reject.lead}${reject.headline}` : ""}
            </div>
          ) : suggested ? (
            <div className="absolute inset-x-0 bottom-5 truncate border-t border-dashed border-red-400/60 bg-sky-950/85 px-1 text-[10px] leading-4 text-sky-100" data-testid={`suggested-reason-${id}`} title={`${suggested.lead} · ${suggested.reason}. Nothing was changed. Press X to reject it, or use Apply suggestions`}>
              <b>{suggested.lead}</b> · {suggested.reason}
            </div>
          ) : null}
          <div className="absolute inset-x-0 bottom-0 flex items-end justify-between gap-1 bg-gradient-to-t from-black/80 to-transparent px-1 pb-0.5 pt-3">
            <div className="flex flex-wrap gap-0.5">
              {tags.map((x) => (
                <span key={x.tag} title={tagTitle(entry, x.tag)} aria-label={tagTitle(entry, x.tag)} className={`rounded px-1 text-[9px] font-semibold leading-4 ${TAG_STYLE[x.tag]}`}>
                  {TAG_SHORT[x.tag]}
                </span>
              ))}
            </div>
            {entry.rating > 0 && <Stars n={entry.rating} className={compact ? "size-3" : "size-3.5"} onRate={onRate && ((r) => onRate(id, r))} testId={`stars-cell-${id}`} />}
          </div>
        </>
      )}
    </div>
  );
});
