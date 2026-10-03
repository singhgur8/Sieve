// Library Filter "Metadata" row (Lightroom style): one column per attribute, each value with the number of photos it would
// show given every other filter. Click to select (several values in a column = OR), columns combine with AND.
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { X } from "lucide-react";
import { commands, unwrap, type MetadataFilter, type MetadataFilterOptions } from "../ipc";
import type { Query } from "../hooks/useLibrary";
import { cleanMeta, clearColumn, metaActive, metaChips, metaColumns, type Column, type ColumnKey } from "../lib/metaFilter";

/** Facet values for the grid's current query; refetched when the query or the library changes. */
export function useMetadataOptions(query: Query, epoch: number, enabled: boolean): MetadataFilterOptions | null {
  const [opts, setOpts] = useState<MetadataFilterOptions | null>(null);
  const key = useMemo(() => JSON.stringify({ ...query, offset: 0, limit: 0 }), [query]);
  useEffect(() => {
    if (!enabled) return;
    let stale = false;
    unwrap(commands.getMetadataFilterOptions({ ...query, offset: 0 }))
      .then((o) => !stale && setOpts(o))
      .catch(() => {});
    return () => {
      stale = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, epoch, enabled]);
  return opts;
}

/** Minimum widths (px) per column; the row stretches them (`flex-1`) so everything fits at 1280 px. */
const MIN_W: Record<string, number> = { extensions: 80, cameras: 136, lenses: 160, iso: 72, focalLengthMm: 80, aperture: 72, shutterSeconds: 80, captured: 120, status: 112 };

function Options({ c, meta, setMeta, label }: { c: Column; meta: MetadataFilter; setMeta: (m: MetadataFilter | undefined) => void; label?: (o: Column["options"][number]) => string }) {
  return (
    <ul>
      {c.options.length === 0 && <li className="px-2 py-1 text-neutral-500">No values</li>}
      {c.options.map((o) => (
        <li key={o.id}>
          <button
            disabled={o.disabled}
            aria-pressed={o.selected}
            data-testid={`meta-opt-${c.key}-${o.id}`}
            onClick={() => setMeta(c.toggle(meta, o.id))}
            title={o.disabled ? "Unknown values cannot be filtered" : undefined}
            className={`flex w-full items-center justify-between gap-2 px-2 py-0.5 text-left disabled:opacity-40 ${o.selected ? "bg-sky-800 text-sky-100" : "text-neutral-300 hover:bg-neutral-800"}`}
          >
            <span className="min-w-0 truncate">{label ? label(o) : o.label}</span>
            <span className="shrink-0 tabular-nums opacity-70" data-testid={`meta-count-${c.key}-${o.id}`}>
              {o.count}
            </span>
          </button>
        </li>
      ))}
    </ul>
  );
}

export function MetadataRow({ query, setQuery, epoch }: { query: Query; setQuery: (fn: (q: Query) => Query) => void; epoch: number }) {
  const opts = useMetadataOptions(query, epoch, true);
  const meta: MetadataFilter = query.metadata ?? {};
  const cols = useMemo(() => (opts ? metaColumns(opts, meta) : null), [opts, query.metadata]); // eslint-disable-line react-hooks/exhaustive-deps
  const setMeta = (next: MetadataFilter | undefined) => setQuery((q) => ({ ...q, metadata: cleanMeta(next) }));
  const ref = useRef<HTMLDivElement>(null);
  const [overflow, setOverflow] = useState(false);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setOverflow(el.scrollWidth > el.clientWidth + 1);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [cols]);
  const main = cols?.filter((c) => c.key !== "edited" && c.key !== "hasSidecar");
  const status = cols?.filter((c) => c.key === "edited" || c.key === "hasSidecar");
  const statusActive = !!status?.some((c) => c.options.some((o) => o.selected));
  const head = "flex items-center justify-between border-b border-neutral-800 px-2 py-1 text-[11px] font-medium uppercase tracking-wide text-neutral-400";
  return (
    <div
      ref={ref}
      className="flex h-28 shrink-0 items-stretch gap-px overflow-x-auto border-b border-neutral-800 bg-neutral-800 text-xs"
      style={overflow ? { maskImage: "linear-gradient(to right, #000 92%, transparent)", WebkitMaskImage: "linear-gradient(to right, #000 92%, transparent)" } : undefined}
      data-testid="meta-row"
      data-overflow={overflow}
    >
      {!cols && <div className="flex-1 bg-neutral-950 px-3 py-2 text-neutral-400">Loading…</div>}
      {main?.map((c) => {
        const active = c.options.some((o) => o.selected);
        return (
          <div key={c.key} className="flex min-w-0 flex-1 basis-0 flex-col bg-neutral-950" style={{ minWidth: MIN_W[c.key] }} data-testid={`meta-col-${c.key}`} data-active={active}>
            <div className={head}>
              {c.title}
              {active && (
                <button className="text-neutral-400 hover:text-neutral-100" aria-label={`Clear ${c.title}`} data-testid={`meta-clear-${c.key}`} onClick={() => setMeta(clearColumn(meta, c.key))}>
                  <X className="size-3" />
                </button>
              )}
            </div>
            <div className="min-h-0 flex-1 overflow-y-auto py-0.5">
              <Options c={c} meta={meta} setMeta={setMeta} />
            </div>
          </div>
        );
      })}
      {status && status.length > 0 && (
        <div className="flex min-w-0 flex-1 basis-0 flex-col bg-neutral-950" style={{ minWidth: MIN_W.status }} data-testid="meta-col-status" data-active={statusActive}>
          <div className={head}>
            Status
            {statusActive && (
              <button
                className="text-neutral-400 hover:text-neutral-100"
                aria-label="Clear Status"
                data-testid="meta-clear-status"
                onClick={() => setMeta(status.reduce<MetadataFilter | undefined>((m, c) => clearColumn(m ?? {}, c.key), meta))}
              >
                <X className="size-3" />
              </button>
            )}
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto py-0.5">
            {status.map((c, i) => (
              <div key={c.key} data-testid={`meta-col-${c.key}`} data-active={c.options.some((o) => o.selected)} className={i > 0 ? "mt-0.5 border-t border-neutral-800 pt-0.5" : ""}>
                <Options c={c} meta={meta} setMeta={setMeta} />
              </div>
            ))}
          </div>
        </div>
      )}
      {metaActive(query.metadata) && (
        <div className="flex shrink-0 items-start bg-neutral-950 px-2 py-1.5">
          <button className="rounded bg-neutral-800 px-2 py-0.5 text-neutral-300 hover:bg-neutral-700" data-testid="meta-clear" onClick={() => setMeta(undefined)}>
            Clear metadata
          </button>
        </div>
      )}
    </div>
  );
}

/** Chips for the active metadata filters, each with a button that clears its column. */
export function MetaChips({ query, setQuery }: { query: Query; setQuery: (fn: (q: Query) => Query) => void }) {
  const chips = metaChips(query.metadata, null);
  if (chips.length === 0) return null;
  const clear = (key: ColumnKey) => setQuery((q) => ({ ...q, metadata: clearColumn(q.metadata ?? {}, key) }));
  return (
    <div className="flex shrink-0 items-center gap-1" data-testid="meta-chips">
      {chips.map((c) => (
        <span key={c.key} className="flex items-center gap-1 rounded bg-sky-900/70 px-1.5 py-0.5 text-sky-100" data-testid={`meta-chip-${c.key}`}>
          {c.text}
          <button aria-label={`Remove ${c.text}`} className="text-sky-300 hover:text-white" onClick={() => clear(c.key)}>
            <X className="size-3" />
          </button>
        </span>
      ))}
    </div>
  );
}
