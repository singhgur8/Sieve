import { memo, useState } from "react";
import { AlertTriangle, Anchor, CloudUpload, Flag, ImageOff, Layers, Loader2, Star, Unplug, X } from "lucide-react";
import { convertFileSrc, type RawImageEntry } from "../ipc";
import { useEntryHealth } from "../lib/errors";
import { LABEL_COLOR, TAG_SHORT, TAG_STYLE, tagName } from "../lib/format";

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

export function XmpBadge({ entry }: { entry: RawImageEntry }) {
  if (entry.xmp.error)
    return (
      <span title={`Sidecar not written: ${entry.xmp.error}`} data-xmp="error" data-testid={`xmp-error-${entry.id}`} className="text-red-400">
        <AlertTriangle className="size-3.5" />
      </span>
    );
  if (entry.xmp.dirty)
    return (
      <span title="Metadata not saved to sidecar" data-xmp="dirty" className="text-amber-400">
        <CloudUpload className="size-3.5" />
      </span>
    );
  return null;
}

export function SceneBadge({ entry, testPrefix = "scene-badge" }: { entry: RawImageEntry; testPrefix?: string }) {
  if (entry.sceneId == null) return null;
  return (
    <span
      title={entry.isSceneAnchor ? `Scene ${entry.sceneId} anchor` : `Scene ${entry.sceneId}`}
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
    <span title={`Paired with ${name}`} data-testid={`${testPrefix}-${entry.id}`} className="rounded bg-neutral-800/90 px-1 text-[10px] font-semibold text-neutral-200">
      +JPG
    </span>
  );
}

/** Cached thumbnail; when the file cannot be loaded (cache folder cleaned, drive offline) a placeholder replaces the broken image. */
function Thumb({ src, name }: { src: string; name: string | undefined }) {
  const [broken, setBroken] = useState<string | null>(null);
  if (broken === src)
    return (
      <div className="flex size-full flex-col items-center justify-center gap-1 text-center" data-testid="thumb-broken" title="The cached preview could not be loaded. Regenerate previews from the More menu, or re-import the folder.">
        <ImageOff className="size-5 text-neutral-500" />
        <span className="text-[10px] text-neutral-400">Preview unavailable</span>
      </div>
    );
  return <img src={src} decoding="async" draggable={false} alt={name} className="size-full object-contain" onError={() => setBroken(src)} />;
}

export const Cell = memo(function Cell({ id, entry, version, size, selected, active, onClick, onDoubleClick, onRate }: Props) {
  const t = entry?.thumbnail;
  const compact = size < 150;
  const tags = entry?.tags.filter((x) => !x.suppressed) ?? [];
  return (
    <div
      data-testid={`cell-${id}`}
      data-selected={selected}
      data-active={active}
      data-pick={entry?.pick}
      data-rating={entry?.rating}
      onClick={(e) => onClick(id, e)}
      onDoubleClick={() => onDoubleClick(id)}
      className={`relative select-none overflow-hidden rounded bg-neutral-900 ${
        selected ? "ring-2 ring-sky-500" : ""
      } ${active ? "outline outline-2 -outline-offset-2 outline-white" : ""} ${entry?.pick === "reject" ? "opacity-50" : ""}`}
      style={{ contain: "strict" }}
    >
      {t?.status === "ready" ? (
        <Thumb src={`${convertFileSrc(t.path)}?v=${version}`} name={entry?.fileName} />
      ) : t?.status === "failed" ? (
        <div className="flex size-full flex-col items-center justify-center gap-1 px-1 text-center" title={t.reason} data-testid={`thumb-failed-${id}`}>
          <ImageOff className="size-5 text-red-500" />
          {!compact && <span className="text-[10px] text-red-300">No preview</span>}
        </div>
      ) : (
        <div className="flex size-full items-center justify-center">
          <Loader2 className={`size-5 text-neutral-700 ${entry ? "animate-spin" : ""}`} />
        </div>
      )}
      {entry && (
        <>
          <div className="pointer-events-none absolute left-1 top-1 flex items-center gap-1">
            {entry.pick === "pick" && <Flag className="size-3.5 fill-green-500 text-green-500" />}
            {entry.pick === "reject" && <X className="size-4 text-red-500" strokeWidth={3} />}
            {entry.colorLabel && <span className={`size-2.5 rounded-full ${LABEL_COLOR[entry.colorLabel]}`} title={entry.colorLabel} />}
          </div>
          <div className="pointer-events-none absolute right-1 top-1 flex items-center gap-1">
            <HealthBadge entry={entry} />
            <XmpBadge entry={entry} />
            <CompanionBadge entry={entry} />
            <SceneBadge entry={entry} />
            {entry.burstGroupId != null && (
              <span
                title={`Burst #${entry.burstGroupId}${entry.isBurstKeeper ? " (keeper)" : ""}`}
                className={`flex items-center gap-0.5 rounded px-1 text-[10px] ${
                  entry.isBurstKeeper ? "bg-green-900 text-green-200" : "bg-neutral-800/90 text-neutral-300"
                }`}
              >
                <Layers className="size-3" />
                {entry.isBurstKeeper ? "★" : ""}
              </span>
            )}
          </div>
          <div className="pointer-events-none absolute inset-x-0 bottom-0 flex items-end justify-between gap-1 bg-gradient-to-t from-black/80 to-transparent px-1 pb-0.5 pt-3">
            <div className="flex flex-wrap gap-0.5">
              {tags.map((x) => (
                <span key={x.tag} title={tagName(x.tag)} className={`rounded px-1 text-[9px] font-semibold leading-4 ${TAG_STYLE[x.tag]}`}>
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
