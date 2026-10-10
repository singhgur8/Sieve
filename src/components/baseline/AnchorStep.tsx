// Baseline edit, step 2: choose the anchor, the one photo you adjust by hand. A good keeper with the subject in
// reasonable light works best (a couple photo); the strip lists one keeper per scene.
import { useEffect, useState } from "react";
import { Check } from "lucide-react";
import type { Library } from "../../hooks/useLibrary";
import { Thumb } from "../edit/bits";

interface Props {
  lib: Library;
  candidates: number[];
  /** Every keeper, for "Show all keepers". */
  allIds: number[];
  anchorId: number;
  activeId: number | null;
  onPick: (id: number) => void;
}

export function AnchorStep({ lib, candidates, allIds, anchorId, activeId, onPick }: Props) {
  const { ensure } = lib;
  const [all, setAll] = useState(false);
  const [shown, setShown] = useState(120);
  const base = all ? allIds : candidates;
  const full = base.includes(anchorId) ? base : [anchorId, ...base];
  // Big shoots: the grid grows 120 photos at a time, only those are loaded.
  const ids = all ? full.slice(0, shown) : full;
  useEffect(() => ensure(ids), [ensure, ids.join(",")]); // eslint-disable-line react-hooks/exhaustive-deps
  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="baseline-step-anchor">
      <div className="flex items-center gap-3 border-b border-neutral-800 px-4 py-2 text-xs text-neutral-300">
        <span>Pick the photo you will adjust by hand. Its light and white balance are the example for the rest, so choose a good keeper in typical light.</span>
        <button
          type="button"
          className="ml-auto shrink-0 whitespace-nowrap rounded-md bg-neutral-800 px-2.5 py-1 hover:bg-neutral-700"
          data-testid="baseline-anchor-all"
          aria-pressed={all}
          title={all ? "Only the best keeper of each scene" : "List every keeper, not just one per scene"}
          onClick={() => setAll((v) => !v)}
        >
          {all ? "One per scene" : `Show all keepers (${allIds.length})`}
        </button>
        <button
          type="button"
          className="shrink-0 whitespace-nowrap rounded-md bg-neutral-800 px-2.5 py-1 hover:bg-neutral-700 disabled:opacity-40"
          data-testid="baseline-anchor-current"
          disabled={activeId == null}
          title={activeId == null ? "Open or select a photo first" : "Use the photo you have selected as the anchor"}
          onClick={() => activeId != null && onPick(activeId)}
        >
          Use the selected photo
        </button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto p-4" data-testid="baseline-anchor-grid">
        <div className="grid grid-cols-[repeat(auto-fill,minmax(160px,1fr))] gap-3">
          {ids.map((id) => {
            const e = lib.getEntry(id);
            const on = id === anchorId;
            return (
              <button
                key={id}
                type="button"
                data-testid={`baseline-anchor-${id}`}
                data-selected={on}
                aria-pressed={on}
                title={`${e?.fileName ?? `Photo ${id}`}: ${on ? "this is the anchor" : "use this photo as the anchor"}`}
                onClick={() => onPick(id)}
                className={`relative overflow-hidden rounded-lg bg-neutral-900 text-left ring-1 ${on ? "ring-2 ring-emerald-500" : "ring-neutral-800 hover:ring-neutral-600"}`}
              >
                <Thumb entry={e} version={lib.version(id)} className="aspect-[3/2] w-full" />
                {on && <Check className="absolute right-1 top-1 size-4 rounded-full bg-emerald-600 p-0.5 text-white" aria-label="Anchor" />}
                <span className="block truncate px-2 py-1 text-xs text-neutral-200">{e?.fileName ?? `#${id}`}</span>
              </button>
            );
          })}
        </div>
        {all && shown < full.length && (
          <button type="button" className="mt-3 h-7 rounded-md bg-neutral-800 px-3 text-xs hover:bg-neutral-700" data-testid="baseline-anchor-more" title="Show more keepers" onClick={() => setShown((n) => n + 120)}>
            Show more ({full.length - shown} left)
          </button>
        )}
      </div>
    </div>
  );
}
