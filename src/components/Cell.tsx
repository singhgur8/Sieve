import { memo } from "react";
import { AlertTriangle, CloudUpload, Flag, ImageOff, Layers, Loader2, Star, X } from "lucide-react";
import { convertFileSrc, type RawImageEntry } from "../ipc";
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
}

export function Stars({ n, className = "size-3" }: { n: number; className?: string }) {
  return (
    <span className="flex" aria-label={`${n} stars`}>
      {[1, 2, 3, 4, 5].map((i) => (
        <Star key={i} className={`${className} ${i <= n ? "fill-amber-400 text-amber-400" : "text-neutral-700"}`} />
      ))}
    </span>
  );
}

export function XmpBadge({ entry }: { entry: RawImageEntry }) {
  if (entry.xmp.error)
    return (
      <span title={`XMP error: ${entry.xmp.error}`} data-xmp="error" className="text-red-400">
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

export const Cell = memo(function Cell({ id, entry, version, size, selected, active, onClick, onDoubleClick }: Props) {
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
        <img
          src={`${convertFileSrc(t.path)}?v=${version}`}
          decoding="async"
          draggable={false}
          alt={entry?.fileName}
          className="size-full object-contain"
        />
      ) : t?.status === "failed" ? (
        <div className="flex size-full items-center justify-center" title={t.reason}>
          <ImageOff className="size-5 text-red-500" />
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
            <XmpBadge entry={entry} />
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
            {entry.rating > 0 && (compact ? <span className="text-[10px] text-amber-400">{entry.rating}★</span> : <Stars n={entry.rating} />)}
          </div>
        </>
      )}
    </div>
  );
});
