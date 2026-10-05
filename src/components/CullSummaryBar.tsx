// Cull step readout: picked / unflagged / rejected (by you vs. auto) and how the keepers add up, every number a filter.
import { useState } from "react";
import { Sparkles } from "lucide-react";
import type { CullSummary, KeeperRule, PickOrigin, RejectStrictness } from "../ipc";
import type { Query } from "../hooks/useLibrary";
import { keeperEquation } from "../lib/cull";
import { KeeperRuleMenu } from "./KeeperRule";

interface Props {
  summary: CullSummary;
  query: Query;
  setQuery: (fn: (q: Query) => Query) => void;
  onKeeperRule: (r: KeeperRule) => void;
  onApplySuggestions: () => void;
  /** Project's reject strictness (v19) and its setter. */
  strictness?: RejectStrictness;
  onStrictness?: (v: RejectStrictness) => void;
}

export const STRICT_TEXT: Record<RejectStrictness, string> = {
  conservative: "Only unusable frames: nothing in focus, far too dark or blown out, or several defects at once. Closed eyes and burst duplicates are never rejected.",
  balanced: "Also missed focus or motion blur on the main subject, closed eyes on the main subject, and burst frames clearly worse than the best one.",
  aggressive: "Also any closed eyes or soft focus, and weaker burst frames even when the difference is small. Expect some keepers among the suggestions.",
};

const base = "whitespace-nowrap rounded px-1.5 py-0.5 text-xs transition-colors";
const idle = "bg-neutral-800 text-neutral-200 hover:bg-neutral-700";
const on = "bg-sky-800 text-sky-100 ring-1 ring-white/30";

/** Exactly this pick filter (and nothing else about picks / keepers / flag origin) is on. */
const onlyPicks = (q: Query, ...p: string[]) => !q.keepersOnly && q.pickOrigin == null && q.picks.length === p.length && p.every((x) => (q.picks as string[]).includes(x));
/** Exactly "rejected by `o`" is on (v18.1 `ImageQuery.pickOrigin`). */
const onlyRejectedBy = (q: Query, o: PickOrigin) => !q.keepersOnly && q.pickOrigin === o && q.picks.length === 1 && q.picks[0] === "reject";

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** "20 picks · 8 rejects · 3 stars only", zero parts dropped (UX 8c P1-1). */
export function suggestionParts(s: CullSummary): string {
  return [
    s.suggestedPickPending > 0 && plural(s.suggestedPickPending, "pick"),
    s.suggestedRejectPending > 0 && plural(s.suggestedRejectPending, "reject"),
    s.suggestedRatingPending > 0 && `${s.suggestedRatingPending} stars only`,
  ]
    .filter(Boolean)
    .join(" · ");
}

export function CullSummaryBar({ summary: s, query, setQuery, onKeeperRule, onApplySuggestions, strictness, onStrictness }: Props) {
  const toggle = (...p: string[]) => setQuery((q) => ({ ...q, keepersOnly: false, pickOrigin: null, picks: onlyPicks(q, ...p) ? [] : (p as Query["picks"]) }));
  const toggleRejectedBy = (o: PickOrigin) =>
    setQuery((q) => (onlyRejectedBy(q, o) ? { ...q, picks: [], pickOrigin: null } : { ...q, keepersOnly: false, picks: ["reject"], pickOrigin: o }));
  const keepersOn = !!query.keepersOnly;
  const formula = keeperEquation(s);
  const suggestions = suggestionParts(s);
  const [autoNote, setAutoNote] = useState<{ x: number; y: number } | null>(null);
  const seg = (active: boolean) => `whitespace-nowrap px-1.5 py-0.5 text-xs transition-colors ${active ? on : idle}`;
  const splitPart = (n: number, o: PickOrigin, label: string) => {
    const testid = `cull-sum-${o === "user" ? "by-you" : "auto"}`;
    const who = o === "user" ? "you rejected yourself (flag keys, sidecars, undo)" : "rejected by Apply suggestions (and not changed by you since)";
    const pressed = onlyRejectedBy(query, o);
    // A zero part would only show an empty grid: it explains itself instead (and points to the pending suggestions).
    if (n === 0 && !pressed) {
      return (
        <button
          className={seg(false)}
          aria-pressed={false}
          data-testid={testid}
          title={`No photos are ${who}`}
          onClick={(e) => {
            const r = e.currentTarget.getBoundingClientRect();
            setAutoNote((cur) => (cur ? null : { x: Math.max(8, Math.min(r.left, window.innerWidth - 328)), y: r.bottom + 4 }));
          }}
        >
          {label} <b>{n}</b>
        </button>
      );
    }
    return (
      <button className={seg(pressed)} aria-pressed={pressed} data-testid={testid} title={`Show only the ${n} photos ${who}, each with the reason`} onClick={() => toggleRejectedBy(o)}>
        {label} <b>{n}</b>
      </button>
    );
  };
  return (
    <>
    <div className="flex shrink-0 flex-nowrap items-center gap-x-2 overflow-hidden whitespace-nowrap border-b border-neutral-800 px-3 py-1 text-xs text-neutral-300" data-testid="cull-summary" data-keepers={s.keepers} data-total={s.total}>
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
      <span className="inline-flex shrink-0 divide-x divide-neutral-900 overflow-hidden rounded" data-testid="cull-sum-reject-split">
        <button
          className={seg(onlyPicks(query, "reject"))}
          data-testid="cull-sum-rejected"
          title={`Review the ${s.rejected} rejected photos, each with the reason it was rejected: ${s.rejectedByUser} by you, ${s.rejectedAuto} automatically`}
          onClick={() => toggle("reject")}
        >
          Rejected <b>{s.rejected}</b>
        </button>
        {splitPart(s.rejectedByUser, "user", "By you")}
        {splitPart(s.rejectedAuto, "auto", "Auto")}
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
      <span
        className="min-w-0 truncate"
        data-testid="cull-sum-formula"
        title={`${formula}. Edit and Export work on the keepers only. Nothing is deleted: left-out photos stay in the catalog and can be flagged again at any time`}
      >
        {formula}
      </span>
      <KeeperRuleMenu current={s.keeperRule} onPick={onKeeperRule} testid="cull-sum-rule" />
      {suggestions && (
        <button
          className="ml-auto flex items-center gap-1 whitespace-nowrap rounded bg-neutral-800 px-1.5 py-0.5 text-sky-200 hover:bg-neutral-700"
          data-testid="cull-sum-suggest"
          title="Sieve has an opinion on photos you have not flagged. Apply suggestions copies it to their flags and stars (you review the result in the Rejected view and can undo it)"
          onClick={onApplySuggestions}
        >
          <Sparkles className="size-3" />
          Suggestions: {suggestions} — Apply…
        </button>
      )}
    </div>
    {autoNote && (
      <>
        <div className="fixed inset-0 z-40" onClick={() => setAutoNote(null)} />
        <div className="fixed z-50 w-80 rounded-lg border border-neutral-700 bg-neutral-900 p-2.5 text-xs text-neutral-200 shadow-xl" style={{ left: autoNote.x, top: autoNote.y }} data-testid="auto-zero-note" role="dialog">
          {s.suggestedRejectPending > 0 ? (
            <>
              No photos were auto-rejected yet. Sieve suggests rejecting {s.suggestedRejectPending}.{" "}
              <button
                className="text-sky-300 underline hover:text-sky-200"
                data-testid="auto-zero-apply"
                onClick={() => {
                  setAutoNote(null);
                  onApplySuggestions();
                }}
              >
                Review and apply suggestions…
              </button>
            </>
          ) : (
            <>No photos were auto-rejected yet, and Sieve has no reject suggestions at this strictness.</>
          )}
        </div>
      </>
    )}
    {strictness && onStrictness && (
      <div className="flex shrink-0 items-center gap-2 overflow-hidden whitespace-nowrap border-b border-neutral-800 px-3 py-0.5 text-[11px] text-neutral-400" data-testid="strictness-row">
        <label className="flex items-center gap-1.5">
          <span className="text-neutral-300">Reject strictness</span>
          <select
            value={strictness}
            onChange={(e) => onStrictness(e.target.value as RejectStrictness)}
            className="rounded border border-neutral-700 bg-neutral-900 px-1 py-0.5 text-xs text-neutral-100"
            data-testid="reject-strictness"
            title="How readily Sieve suggests rejecting a photo. Changing it re-evaluates the suggestions; nothing is rejected until you apply them"
            aria-label="Reject strictness"
          >
            <option value="conservative">Conservative</option>
            <option value="balanced">Balanced</option>
            <option value="aggressive">Aggressive</option>
          </select>
        </label>
        <span className="min-w-0 truncate" data-testid="strictness-explain">{STRICT_TEXT[strictness]}</span>
        <span className="ml-auto text-neutral-300" data-testid="strictness-count" data-rejects={s.suggestedRejectPending} title="Photos you have not flagged or rated that Sieve would reject if you press Apply suggestions">
          {s.suggestedRejectPending} reject {s.suggestedRejectPending === 1 ? "suggestion" : "suggestions"}
        </span>
      </div>
    )}
    </>
  );
}