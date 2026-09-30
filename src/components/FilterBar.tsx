import { useEffect, useState } from "react";
import { Filter, Layers, RotateCcw } from "lucide-react";
import { commands, unwrap, type CatalogState, type ColorLabel, type CullTag, type FilterCounts, type PickFlag } from "../ipc";
import type { Query } from "../hooks/useLibrary";
import { ALL_TAGS, LABEL_COLOR, tagName, TAG_STYLE } from "../lib/format";
import { BASE_QUERY } from "../hooks/useLibrary";

const LABELS: ColorLabel[] = ["red", "yellow", "green", "blue", "purple"];
const PICKS: { key: PickFlag; label: string }[] = [
  { key: "pick", label: "Picked" },
  { key: "reject", label: "Rejected" },
  { key: "unflagged", label: "Unflagged" },
];

interface Props {
  query: Query;
  setQuery: (fn: (q: Query) => Query) => void;
  catalog: CatalogState | null;
  epoch: number;
  shown: number;
}

const chip = "rounded px-2 py-0.5 text-xs transition-colors";
const off = "bg-neutral-800 text-neutral-300 hover:bg-neutral-700";

export function isFiltered(q: Query): boolean {
  return (
    q.includeTags.length > 0 ||
    q.excludeTags.length > 0 ||
    q.picks.length > 0 ||
    q.minRating != null ||
    q.maxRating != null ||
    q.colorLabels.length > 0 ||
    q.collapseBursts ||
    q.folderId != null
  );
}

export function FilterBar({ query, setQuery, catalog, epoch, shown }: Props) {
  const [counts, setCounts] = useState<FilterCounts | null>(null);
  const folderId = query.folderId;
  useEffect(() => {
    let stale = false;
    unwrap(commands.getFilterCounts(folderId))
      .then((c) => !stale && setCounts(c))
      .catch(() => {});
    return () => {
      stale = true;
    };
  }, [folderId, epoch]);

  const tagCount = (t: CullTag) => counts?.tags.find((x) => x.tag === t)?.count ?? 0;
  const pickCount = (p: PickFlag) => (counts ? { pick: counts.picked, reject: counts.rejected, unflagged: counts.unflagged }[p] : 0);

  /** off -> include -> exclude -> off */
  const cycleTag = (t: CullTag) =>
    setQuery((q) => {
      if (q.includeTags.includes(t)) return { ...q, includeTags: q.includeTags.filter((x) => x !== t), excludeTags: [...q.excludeTags, t] };
      if (q.excludeTags.includes(t)) return { ...q, excludeTags: q.excludeTags.filter((x) => x !== t) };
      return { ...q, includeTags: [...q.includeTags, t] };
    });
  const toggle = <T,>(list: T[], v: T) => (list.includes(v) ? list.filter((x) => x !== v) : [...list, v]);

  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-1.5 border-b border-neutral-800 px-4 py-2 text-xs" data-testid="filter-bar">
      <Filter className="size-3.5 text-neutral-500" />
      <div className="flex flex-wrap items-center gap-1" data-testid="filter-tags">
        {ALL_TAGS.map((t) => {
          const inc = query.includeTags.includes(t);
          const exc = query.excludeTags.includes(t);
          return (
            <button
              key={t}
              data-testid={`tag-${t}`}
              data-state={inc ? "include" : exc ? "exclude" : "off"}
              onClick={() => cycleTag(t)}
              title={inc ? "Showing only. Click to exclude." : exc ? "Excluded. Click to clear." : "Click to include, again to exclude"}
              className={`${chip} ${inc ? TAG_STYLE[t] + " ring-1 ring-white/40" : exc ? "bg-neutral-900 text-neutral-500 line-through ring-1 ring-red-800" : off}`}
            >
              {tagName(t)} <span className="opacity-60">{tagCount(t)}</span>
            </button>
          );
        })}
        <select
          aria-label="Tag match mode"
          value={query.tagMatch}
          onChange={(e) => setQuery((q) => ({ ...q, tagMatch: e.target.value as Query["tagMatch"] }))}
          className="rounded bg-neutral-800 px-1 py-0.5"
        >
          <option value="any">any</option>
          <option value="all">all</option>
        </select>
      </div>

      <div className="flex items-center gap-1" data-testid="filter-picks">
        {PICKS.map((p) => (
          <button
            key={p.key}
            data-testid={`pick-${p.key}`}
            onClick={() => setQuery((q) => ({ ...q, picks: toggle(q.picks, p.key) }))}
            className={`${chip} ${query.picks.includes(p.key) ? "bg-sky-800 text-sky-100" : off}`}
          >
            {p.label} <span className="opacity-60">{pickCount(p.key)}</span>
          </button>
        ))}
      </div>

      <div className="flex items-center gap-1">
        <span className="text-neutral-500">Rating</span>
        <select
          aria-label="Minimum rating"
          value={query.minRating ?? ""}
          onChange={(e) => setQuery((q) => ({ ...q, minRating: e.target.value === "" ? null : Number(e.target.value) }))}
          className="rounded bg-neutral-800 px-1 py-0.5"
        >
          <option value="">min</option>
          {[0, 1, 2, 3, 4, 5].map((n) => (
            <option key={n} value={n}>
              {n}★+ {counts ? `(${counts.ratings.slice(n).reduce((a, b) => a + b, 0)})` : ""}
            </option>
          ))}
        </select>
        <select
          aria-label="Maximum rating"
          value={query.maxRating ?? ""}
          onChange={(e) => setQuery((q) => ({ ...q, maxRating: e.target.value === "" ? null : Number(e.target.value) }))}
          className="rounded bg-neutral-800 px-1 py-0.5"
        >
          <option value="">max</option>
          {[0, 1, 2, 3, 4, 5].map((n) => (
            <option key={n} value={n}>
              ≤{n}★
            </option>
          ))}
        </select>
      </div>

      <div className="flex items-center gap-1" data-testid="filter-labels">
        {LABELS.map((l) => (
          <button
            key={l}
            title={l}
            aria-label={`label ${l}`}
            data-testid={`label-${l}`}
            onClick={() => setQuery((q) => ({ ...q, colorLabels: toggle(q.colorLabels, l) }))}
            className={`size-4 rounded-full ${LABEL_COLOR[l]} ${query.colorLabels.includes(l) ? "ring-2 ring-white" : "opacity-40 hover:opacity-80"}`}
          />
        ))}
      </div>

      <label className="flex items-center gap-1.5 text-neutral-300">
        <input
          type="checkbox"
          data-testid="collapse-bursts"
          checked={query.collapseBursts}
          onChange={(e) => setQuery((q) => ({ ...q, collapseBursts: e.target.checked }))}
        />
        <Layers className="size-3.5" />
        Collapse bursts{counts && counts.burstNonKeepers > 0 ? ` (${counts.burstNonKeepers} hidden)` : ""}
      </label>

      {catalog && catalog.folders.length > 0 && (
        <select
          aria-label="Folder"
          data-testid="folder-select"
          value={query.folderId ?? ""}
          onChange={(e) => setQuery((q) => ({ ...q, folderId: e.target.value === "" ? null : Number(e.target.value) }))}
          className="max-w-48 rounded bg-neutral-800 px-1 py-0.5"
        >
          <option value="">All folders</option>
          {catalog.folders.map((f) => (
            <option key={f.id} value={f.id}>
              {f.path.split("/").filter(Boolean).pop()} ({f.imageCount})
            </option>
          ))}
        </select>
      )}

      <span className="ml-auto flex items-center gap-2 text-neutral-400" data-testid="shown-count">
        {shown}
        {counts ? ` of ${counts.total}` : ""}
        {isFiltered(query) && (
          <button
            onClick={() => setQuery((q) => ({ ...BASE_QUERY, sort: q.sort, sortDescending: q.sortDescending }))}
            className="flex items-center gap-1 rounded bg-neutral-800 px-2 py-0.5 hover:bg-neutral-700"
            data-testid="clear-filters"
          >
            <RotateCcw className="size-3" />
            Clear
          </button>
        )}
      </span>
    </div>
  );
}
