// Cull step readout: picked / unflagged / rejected (by you vs. auto) and how the keepers add up, every number a filter.
import { Sparkles } from "lucide-react";
import type { CullSummary, KeeperRule } from "../ipc";
import type { Query } from "../hooks/useLibrary";
import { keeperEquation } from "../lib/cull";
import { KeeperRuleMenu } from "./KeeperRule";

interface Props {
  summary: CullSummary;
  query: Query;
  setQuery: (fn: (q: Query) => Query) => void;
  onKeeperRule: (r: KeeperRule) => void;
  onApplySuggestions: () => void;
}

const base = "whitespace-nowrap rounded px-1.5 py-0.5 text-xs transition-colors";
const idle = "bg-neutral-800 text-neutral-200 hover:bg-neutral-700";
const on = "bg-sky-800 text-sky-100 ring-1 ring-white/30";

/** Exactly this pick filter (and nothing else about picks / keepers) is on. */
const onlyPicks = (q: Query, ...p: string[]) => !q.keepersOnly && q.picks.length === p.length && p.every((x) => (q.picks as string[]).includes(x));

export function CullSummaryBar({ summary: s, query, setQuery, onKeeperRule, onApplySuggestions }: Props) {
  const toggle = (...p: string[]) => setQuery((q) => ({ ...q, keepersOnly: false, picks: onlyPicks(q, ...p) ? [] : (p as Query["picks"]) }));
  const keepersOn = !!query.keepersOnly;
  return (
    <div className="flex shrink-0 flex-wrap items-center gap-x-2 gap-y-1 border-b border-neutral-800 px-3 py-1 text-xs text-neutral-300" data-testid="cull-summary" data-keepers={s.keepers} data-total={s.total}>
      <button
        className={`${base} ${onlyPicks(query, "pick") ? on : idle}`}
        data-testid="cull-sum-picked"
        title={`Show the ${s.picked} picked photos${s.pickedAuto > 0 ? ` (${s.pickedAuto} picked automatically, the rest by you)` : ""}`}
        onClick={() => toggle("pick")}
      >
        Picked <b>{s.picked}</b>
      </button>
      <button className={`${base} ${onlyPicks(query, "unflagged") ? on : idle}`} data-testid="cull-sum-unflagged" title={`Show the ${s.unflagged} photos you have not flagged yet (neither picked nor rejected)`} onClick={() => toggle("unflagged")}>
        Unflagged <b>{s.unflagged}</b>
      </button>
      <button
        className={`${base} ${onlyPicks(query, "reject") ? on : idle}`}
        data-testid="cull-sum-rejected"
        title={`Review the ${s.rejected} rejected photos, each with the reason it was rejected: ${s.rejectedByUser} by you, ${s.rejectedAuto} automatically`}
        onClick={() => toggle("reject")}
      >
        Rejected <b>{s.rejected}</b>
      </button>
      <span className="text-neutral-400" data-testid="cull-sum-reject-split">
        (
        <button className="underline-offset-2 hover:underline" data-testid="cull-sum-by-you" title="Rejected by you (flag keys, sidecars, undo). Shown in the Rejected view with the reason" onClick={() => toggle("reject")}>
          {s.rejectedByUser} by you
        </button>
        ,{" "}
        <button className="underline-offset-2 hover:underline" data-testid="cull-sum-auto" title="Rejected by Apply suggestions and not changed by you since. Shown in the Rejected view with the reason" onClick={() => toggle("reject")}>
          {s.rejectedAuto} auto
        </button>
        )
      </span>
      <span className="mx-1 h-4 w-px bg-neutral-700" />
      <button
        className={`${base} ${keepersOn ? on : idle}`}
        data-testid="cull-sum-keepers"
        title={`Show the ${s.keepers} keepers: the photos that go on to Edit and Export`}
        onClick={() => setQuery((q) => ({ ...q, picks: [], keepersOnly: !q.keepersOnly }))}
      >
        Keepers <b>{s.keepers}</b>
      </button>
      <span data-testid="cull-sum-formula" title="Edit and Export work on the keepers only. Nothing is deleted: left-out photos stay in the catalog and can be flagged again at any time">
        {keeperEquation(s)}
      </span>
      <KeeperRuleMenu current={s.keeperRule} onPick={onKeeperRule} testid="cull-sum-rule" />
      {(s.suggestedRejectPending > 0 || s.suggestedPickPending > 0) && (
        <button
          className="ml-auto flex items-center gap-1 whitespace-nowrap rounded bg-neutral-800 px-1.5 py-0.5 text-sky-200 hover:bg-neutral-700"
          data-testid="cull-sum-suggest"
          title="Sieve has an opinion on photos you have not flagged. Apply suggestions copies it to their flags and stars (you review the result in the Rejected view and can undo it)"
          onClick={onApplySuggestions}
        >
          <Sparkles className="size-3" />
          Sieve suggests {s.suggestedPickPending} pick{s.suggestedPickPending === 1 ? "" : "s"} and {s.suggestedRejectPending} reject{s.suggestedRejectPending === 1 ? "" : "s"} you have not acted on. Apply…
        </button>
      )}
    </div>
  );
}
