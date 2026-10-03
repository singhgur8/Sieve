// Confirmation for "Apply suggestions": shows what will happen before any rating / flag is overwritten.
import { useEffect, useMemo, useState } from "react";
import { commands, unwrap, type RawImageEntry } from "../ipc";
import { Dialog } from "./Dialog";

interface Props {
  /** The current multi-selection (0 or 1 photos count as "nothing selected", like Export). */
  selected: number[];
  /** Every photo in the current view. */
  all: number[];
  onCancel: () => void;
  onConfirm: (ids: number[], onlyUnset: boolean) => void;
}

export function ApplySuggestionsDialog({ selected, all, onCancel, onConfirm }: Props) {
  const [scope, setScope] = useState<"selected" | "all">(selected.length > 1 ? "selected" : "all");
  const ids = scope === "selected" ? selected : all;
  const [rows, setRows] = useState<RawImageEntry[] | null>(null);
  const [skipManual, setSkipManual] = useState(true);

  useEffect(() => {
    let live = true;
    (async () => {
      const all: RawImageEntry[] = [];
      for (let i = 0; i < ids.length; i += 200) {
        try {
          all.push(...(await unwrap(commands.getImages(ids.slice(i, i + 200)))));
        } catch {
          break;
        }
        if (!live) return;
      }
      if (live) setRows(all);
    })();
    return () => {
      live = false;
    };
  }, [ids]);

  const c = useMemo(() => {
    const out = { picks: 0, rejects: 0, rated: 0, skipped: 0, unanalyzed: 0, apply: 0 };
    for (const r of rows ?? []) {
      const q = r.quality;
      if (!q) {
        out.unanalyzed++;
        continue;
      }
      if (skipManual && (r.pick !== "unflagged" || r.rating !== 0)) {
        out.skipped++;
        continue;
      }
      out.apply++;
      if (q.suggestedPick === "pick") out.picks++;
      else if (q.suggestedPick === "reject") out.rejects++;
      if (q.suggestedRating > 0) out.rated++;
    }
    return out;
  }, [rows, skipManual]);

  const n = ids.length;
  return (
    <Dialog
      label="Apply suggestions"
      testid="apply-dialog"
      className="w-[26rem] rounded-lg border border-neutral-700 bg-neutral-900 p-4 shadow-xl"
      onCancel={onCancel}
      onConfirm={() => onConfirm(ids, skipManual)}
      canConfirm={() => rows != null && c.apply > 0}
    >
      <h2 className="mb-1 text-sm font-semibold" data-testid="apply-title">
        Apply suggestions
      </h2>
      <div className="mb-3 space-y-1.5 text-xs text-neutral-400" data-testid="apply-explain">
        <p>
          Sieve analysed your photos (blinks, focus, blur, exposure, duplicates in a burst) and suggests a flag and stars for each. <b className="text-neutral-200">Applying copies those suggestions in place</b>: it only changes flags and stars, never your files or edits, and
          by default only photos you have not flagged or rated yourself.
        </p>
        <p>Nothing is final. Afterwards, open the Rejected view: every auto-rejected photo shows why it was rejected, and you can flag it back (Z / U). Undo (Cmd+Z, or the Undo button that follows) reverts the whole apply.</p>
      </div>
      <div className="mb-3 flex gap-4 text-sm" role="radiogroup" aria-label="Scope" data-testid="apply-scope">
        <label className={`flex items-center gap-1.5 ${selected.length === 0 ? "opacity-50" : ""}`}>
          <input type="radio" name="apply-scope" checked={scope === "selected"} disabled={selected.length === 0} onChange={() => setScope("selected")} data-testid="apply-scope-selected" />
          Selected ({selected.length})
        </label>
        <label className="flex items-center gap-1.5">
          <input type="radio" name="apply-scope" checked={scope === "all"} onChange={() => setScope("all")} data-testid="apply-scope-all" />
          All in view ({all.length})
        </label>
      </div>
      <label className="mb-3 flex items-center gap-2 text-sm">
        <input type="checkbox" data-autofocus checked={skipManual} onChange={(e) => setSkipManual(e.target.checked)} data-testid="apply-only-unset" />
        Skip photos I already flagged or rated
      </label>
      <div className="mb-4 rounded bg-neutral-950 p-3 text-xs text-neutral-300" data-testid="apply-counts">
        {rows == null ? (
          "Counting..."
        ) : (
          <ul className="space-y-0.5">
            <li>
              <span data-testid="apply-count-apply">{c.apply}</span> will be updated: {c.picks} picked, {c.rejects} rejected, {c.rated} star-rated
            </li>
            <li>
              <span data-testid="apply-count-skipped">{c.skipped}</span> left as they are (already flagged or rated)
            </li>
            {c.unanalyzed > 0 && (
              <li>
                <span data-testid="apply-count-unanalyzed">{c.unanalyzed}</span> not analyzed yet
              </li>
            )}
          </ul>
        )}
      </div>
      <div className="flex justify-end gap-2 text-xs">
        <button className="rounded bg-neutral-800 px-3 py-1.5 hover:bg-neutral-700" onClick={onCancel} data-testid="apply-cancel">
          Cancel
        </button>
        <button
          className="rounded bg-sky-700 px-3 py-1.5 text-white hover:bg-sky-600 disabled:opacity-40"
          disabled={rows == null || c.apply === 0}
          onClick={() => onConfirm(ids, skipManual)}
          data-testid="apply-confirm"
        >
          Apply to {rows == null ? n : c.apply}
        </button>
      </div>
    </Dialog>
  );
}
