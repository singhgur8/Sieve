import { useEffect, useState } from "react";
import { Filter, FolderSearch, Layers, RotateCcw, Unplug } from "lucide-react";
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
  counts: FilterCounts | null;
  /** "Locate folder…" (IPC v13 relocate_folder). */
  onLocate?: () => void;
}

const chip = "whitespace-nowrap rounded px-2 py-0.5 text-xs transition-colors";
const off = "bg-neutral-800 text-neutral-300 hover:bg-neutral-700";
const rowClass = "flex h-8 shrink-0 items-center gap-x-3 overflow-x-auto overflow-y-hidden whitespace-nowrap border-b border-neutral-800 px-3 text-xs";

export function isFiltered(q: Query): boolean {
  return (
    q.includeTags.length > 0 ||
    q.excludeTags.length > 0 ||
    q.picks.length > 0 ||
    q.minRating != null ||
    q.maxRating != null ||
    q.colorLabels.length > 0 ||
    q.collapseBursts ||
    q.folderId != null ||
    q.missingOnly === true ||
    q.sceneId != null
  );
}

/** Filter counts for the current folder; refreshed whenever the library changes (`epoch`). */
export function useFilterCounts(folderId: number | null, epoch: number): FilterCounts | null {
  const [counts, setCounts] = useState<FilterCounts | null>(null);
  useEffect(() => {
    let stale = false;
    unwrap(commands.getFilterCounts(folderId))
      .then((c) => !stale && setCounts(c))
      .catch(() => {});
    return () => {
      stale = true;
    };
  }, [folderId, epoch]);
  return counts;
}

const toggle = <T,>(list: T[], v: T) => (list.includes(v) ? list.filter((x) => x !== v) : [...list, v]);

/** Row 1 of the Library chrome: culling tags, flags and the result count. */
export function FilterBar({ query, setQuery, counts, onLocate }: Props) {
  const tagCount = (t: CullTag) => counts?.tags.find((x) => x.tag === t)?.count ?? 0;
  const pickCount = (p: PickFlag) => (counts ? { pick: counts.picked, reject: counts.rejected, unflagged: counts.unflagged }[p] : 0);

  /** off -> include -> exclude -> off */
  const cycleTag = (t: CullTag) =>
    setQuery((q) => {
      if (q.includeTags.includes(t)) return { ...q, includeTags: q.includeTags.filter((x) => x !== t), excludeTags: [...q.excludeTags, t] };
      if (q.excludeTags.includes(t)) return { ...q, excludeTags: q.excludeTags.filter((x) => x !== t) };
      return { ...q, includeTags: [...q.includeTags, t] };
    });

  return (
    <div className={`${rowClass} min-w-0`} data-testid="filter-bar">
      <Filter className="size-3.5 shrink-0 text-neutral-400" />
      <div className="flex shrink-0 items-center gap-1" data-testid="filter-tags">
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
              className={`${chip} ${inc ? TAG_STYLE[t] + " ring-1 ring-white/40" : exc ? "bg-neutral-900 text-neutral-400 line-through ring-1 ring-red-800" : off}`}
            >
              {tagName(t)} <span className="opacity-70">{tagCount(t)}</span>
            </button>
          );
        })}
        <select
          aria-label="Tag match mode"
          value={query.tagMatch}
          onChange={(e) => setQuery((q) => ({ ...q, tagMatch: e.target.value as Query["tagMatch"] }))}
          className="rounded bg-neutral-800 px-1 py-0.5"
        >
          <option value="any">Match any</option>
          <option value="all">Match all</option>
        </select>
      </div>

      <div className="flex shrink-0 items-center gap-1" data-testid="filter-picks">
        {PICKS.map((p) => (
          <button
            key={p.key}
            data-testid={`pick-${p.key}`}
            onClick={() => setQuery((q) => ({ ...q, picks: toggle(q.picks, p.key) }))}
            className={`${chip} ${query.picks.includes(p.key) ? "bg-sky-800 text-sky-100" : off}`}
          >
            {p.label} <span className="opacity-70">{pickCount(p.key)}</span>
          </button>
        ))}
      </div>

      {((counts?.missing ?? 0) > 0 || query.missingOnly) && (
        <div className="flex shrink-0 items-center gap-1" data-testid="filter-missing-group">
          <button
            data-testid="filter-missing"
            data-state={query.missingOnly ? "include" : "off"}
            onClick={() => setQuery((q) => ({ ...q, missingOnly: !q.missingOnly }))}
            title="Photos whose original file cannot be found"
            className={`${chip} flex items-center gap-1 ${query.missingOnly ? "bg-amber-800 text-amber-100 ring-1 ring-white/40" : off}`}
          >
            <Unplug className="size-3" /> Missing <span className="opacity-70">{counts?.missing ?? 0}</span>
          </button>
          {onLocate && (counts?.missing ?? 0) > 0 && (
            <button onClick={onLocate} data-testid="locate-folder" className={`${chip} ${off} flex items-center gap-1`}>
              <FolderSearch className="size-3" /> Locate folder…
            </button>
          )}
        </div>
      )}

      {isFiltered(query) && (
        <button
          onClick={() => setQuery((q) => ({ ...BASE_QUERY, sort: q.sort, sortDescending: q.sortDescending }))}
          className="ml-auto flex shrink-0 items-center gap-1 rounded bg-neutral-800 px-2 py-0.5 text-neutral-300 hover:bg-neutral-700"
          data-testid="clear-filters"
        >
          <RotateCcw className="size-3" />
          Clear
        </button>
      )}
    </div>
  );
}

/** Rating / label / burst / folder filters (row 2 of the Library chrome, left of the view controls). */
export function FilterExtras({ query, setQuery, counts, catalog, onLocate }: { query: Query; setQuery: Props["setQuery"]; counts: FilterCounts | null; catalog: CatalogState | null; onLocate?: () => void }) {
  return (
    <>
      <div className="flex items-center gap-1">
        <span className="text-neutral-400">Rating</span>
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
            className={`size-3.5 shrink-0 rounded-full ${LABEL_COLOR[l]} ${query.colorLabels.includes(l) ? "ring-2 ring-white" : "opacity-40 hover:opacity-80"}`}
          />
        ))}
      </div>

      <label className="flex items-center gap-1.5 text-neutral-300" title={counts && counts.burstNonKeepers > 0 ? `${counts.burstNonKeepers} burst frames hidden when collapsed` : "Show only the keeper of each burst"}>
        <input
          type="checkbox"
          data-testid="collapse-bursts"
          checked={query.collapseBursts}
          onChange={(e) => setQuery((q) => ({ ...q, collapseBursts: e.target.checked }))}
        />
        <Layers className="size-3.5" />
        Collapse bursts{counts && counts.burstNonKeepers > 0 && query.collapseBursts ? ` (${counts.burstNonKeepers} hidden)` : ""}
      </label>

      {catalog && catalog.folders.length > 0 && (
        <select
          aria-label="Folder"
          data-testid="folder-select"
          value={query.folderId ?? ""}
          onChange={(e) => setQuery((q) => ({ ...q, folderId: e.target.value === "" ? null : Number(e.target.value) }))}
          className="max-w-40 rounded bg-neutral-800 px-1 py-0.5"
        >
          <option value="">All folders</option>
          {catalog.folders.map((f) => (
            <option key={f.id} value={f.id}>
              {f.path.split("/").filter(Boolean).pop()} ({f.imageCount})
            </option>
          ))}
        </select>
      )}
      {onLocate && query.folderId != null && (
        <button onClick={onLocate} data-testid="locate-selected-folder" title="Point this folder at where its photos were moved" className="flex items-center gap-1 rounded bg-neutral-800 px-2 py-0.5 text-neutral-300 hover:bg-neutral-700">
          <FolderSearch className="size-3" /> Locate…
        </button>
      )}
    </>
  );
}

/** One-line description of the active filters (used by the collapsed summary bar). */
export function describeFilters(q: Query, sceneNumber?: (id: number) => number): string {
  const parts: string[] = [];
  q.includeTags.forEach((t) => parts.push(tagName(t)));
  q.excludeTags.forEach((t) => parts.push(`no ${tagName(t)}`));
  q.picks.forEach((p) => parts.push(PICKS.find((x) => x.key === p)?.label ?? p));
  if (q.minRating != null || q.maxRating != null) parts.push(`${q.minRating ?? 0}-${q.maxRating ?? 5}★`);
  q.colorLabels.forEach((l) => parts.push(l));
  if (q.collapseBursts) parts.push("bursts collapsed");
  if (q.folderId != null) parts.push("one folder");
  if (q.missingOnly) parts.push("missing");
  if (q.sceneId != null) parts.push(`Scene ${sceneNumber?.(q.sceneId) || q.sceneId}`);
  return parts.join(", ");
}

/** 28 px summary shown instead of the filter bars outside the Grid. */
export function FilterSummary({ query, shown, total, sceneNumber, onEdit }: { query: Query; shown: number; total: number | null; sceneNumber: (id: number) => number; onEdit: () => void }) {
  const filtered = isFiltered(query);
  return (
    <div className="flex h-7 shrink-0 items-center gap-2 overflow-hidden whitespace-nowrap border-b border-neutral-800 px-3 text-xs text-neutral-300" data-testid="filter-summary">
      <Filter className="size-3.5 shrink-0 text-neutral-400" />
      <span className="truncate" data-testid="filter-summary-text">
        {filtered ? `Filtered: ${describeFilters(query, sceneNumber)}` : "No filters"} · {shown}
        {total != null ? ` of ${total}` : ""}
      </span>
      <button className="rounded bg-neutral-800 px-2 py-0.5 hover:bg-neutral-700" onClick={onEdit} data-testid="edit-filters">
        Edit filters
      </button>
    </div>
  );
}
