// Confirmation for "Apply suggestions": shows what will happen before any rating / flag is overwritten.
import { useEffect, useMemo, useState } from "react";
import { commands, unwrap, type RawImageEntry, type SuggestionKinds } from "../ipc";
import { Dialog } from "./Dialog";
import { HelpLink } from "./HelpLink";

interface Props {
  /** The current multi-selection (0 or 1 photos count as "nothing selected", like Export). */
  selected: number[];
  /** Every photo in the current view. */
  all: number[];
  onCancel: () => void;
  onConfirm: (ids: number[], onlyUnset: boolean, kinds: SuggestionKinds) => void;
  /** Remembers the last choice of kinds per project. */
  projectId?: number | null;
  /** The "suggested rejects" filter is on: open with only Rejects checked. */
  onlyRejects?: boolean;
}

const kindsKey = (p: number | null | undefined) => `sieve.applyKinds.v1.${p ?? "all"}`;
function loadKinds(p: number | null | undefined, onlyRejects: boolean): SuggestionKinds {
  if (onlyRejects) return { picks: false, rejects: true, stars: false };
  try {
    const v = JSON.parse(localStorage.getItem(kindsKey(p)) ?? "null");
    if (v && typeof v.picks === "boolean" && typeof v.rejects === "boolean" && typeof v.stars === "boolean") return v;
  } catch {
    /* default */
  }
  return { picks: true, rejects: true, stars: true };
}

export function ApplySuggestionsDialog({ selected, all, onCancel, onConfirm, projectId, onlyRejects = false }: Props) {
  const [kinds, setKinds] = useState<SuggestionKinds>(() => loadKinds(projectId, onlyRejects));
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
    const out = { picks: 0, rejects: 0, starsOnly: 0, ratedAll: 0, skipped: 0, unanalyzed: 0, apply: 0, allPicks: 0, allRejects: 0, allStars: 0 };
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
      // Already what Sieve suggests: `apply_suggestions` leaves it alone (IPC v18.1), so the
      // default count equals the Cull summary's "Suggestions: …" numbers.
      if (r.pick === q.suggestedPick && r.rating === q.suggestedRating) {
        out.skipped++;
        continue;
      }
      // What each kind would change on this photo (v19.2 `kinds`); a "no flag" suggestion needs picks and rejects both.
      const wantsPick = q.suggestedPick === "pick" && r.pick !== "pick";
      const wantsReject = q.suggestedPick === "reject" && r.pick !== "reject";
      const wantsStars = q.suggestedRating > 0 && r.rating !== q.suggestedRating;
      if (wantsPick) out.allPicks++;
      if (wantsReject) out.allRejects++;
      if (wantsStars) out.allStars++;
      const flagChange = (wantsPick && kinds.picks) || (wantsReject && kinds.rejects);
      const starChange = wantsStars && kinds.stars;
      if (!flagChange && !starChange) {
        out.skipped++;
        continue;
      }
      out.apply++;
      if (wantsPick && kinds.picks) out.picks++;
      if (wantsReject && kinds.rejects) out.rejects++;
      if (starChange) out.ratedAll++;
      if (starChange && !flagChange) out.starsOnly++;
    }
    return out;
  }, [rows, skipManual, kinds]);
  const setKind = (k: keyof SuggestionKinds, v: boolean) =>
    setKinds((cur) => {
      const next = { ...cur, [k]: v };
      try {
        localStorage.setItem(kindsKey(projectId), JSON.stringify(next));
      } catch {
        /* not remembered */
      }
      return next;
    });

  const n = ids.length;
  return (
    <Dialog
      label="Apply suggestions"
      testid="apply-dialog"
      className="w-[26rem] rounded-lg border border-neutral-700 bg-neutral-900 p-4 shadow-xl"
      onCancel={onCancel}
      onConfirm={() => onConfirm(ids, skipManual, kinds)}
      canConfirm={() => rows != null && c.apply > 0}
    >
      <h2 className="mb-1 flex items-center gap-2 text-sm font-semibold" data-testid="apply-title">
        Apply suggestions <HelpLink id="apply-suggestions" title="What does this do?" />
      </h2>
      <p className="mb-3 text-xs text-neutral-400" data-testid="apply-explain">
        Fills in flags and stars from Sieve&apos;s analysis, by default only on photos you have not flagged or rated. Review the result in the Rejected view (each rejected photo shows why); Undo reverts it all.
      </p>
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
      <fieldset className="mb-3 flex flex-wrap gap-x-4 gap-y-1 text-sm" data-testid="apply-kinds">
        <legend className="sr-only">What to apply</legend>
        {([["rejects", "Rejects", c.allRejects], ["picks", "Picks", c.allPicks], ["stars", "Stars", c.allStars]] as const).map(([k, label, n]) => (
          <label key={k} className="flex items-center gap-1.5">
            <input type="checkbox" checked={kinds[k]} onChange={(e) => setKind(k, e.target.checked)} data-testid={`apply-kind-${k}`} />
            <span>
              {label} (<span data-testid={`apply-kind-${k}-count`}>{rows == null ? "…" : n}</span>)
            </span>
          </label>
        ))}
      </fieldset>
      <div className="mb-4 rounded bg-neutral-950 p-3 text-xs text-neutral-300" data-testid="apply-counts">
        {rows == null ? (
          "Counting..."
        ) : (
          <ul className="space-y-0.5">
            <li>
              <span data-testid="apply-count-apply">{c.apply}</span> will be updated: {c.picks} picked · {c.rejects} rejected · {c.starsOnly} stars only
            </li>
            {c.ratedAll > 0 && (
              <li className="text-neutral-400" data-testid="apply-count-stars-line">
                Stars are set on {c.ratedAll} of them
              </li>
            )}
            <li>
              <span data-testid="apply-count-skipped">{c.skipped}</span> left as they are (already flagged, rated or matching the suggestion)
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
          onClick={() => onConfirm(ids, skipManual, kinds)}
          data-testid="apply-confirm"
        >
          Apply to {rows == null ? n : c.apply}
        </button>
      </div>
    </Dialog>
  );
}
