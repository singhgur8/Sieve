// Baseline edit, step 1: pick a preset from the style library. Every tile is the anchor photo rendered with that preset
// (`renderPreviewVariant`, slot `preview`, one at a time and only for tiles in view), plus "No preset".
import { useEffect, useMemo, useRef, useState } from "react";
import { Check, Search } from "lucide-react";
import { BASELINE_LOOK_FIELDS, commands, copyAdjustmentFields, defaultAdjustments, unwrap, type ParametricAdjustments, type StyleGroup, type StylePreset } from "../../ipc";

// One render at a time: all tiles share the anchor's `preview` slot, a newer request would supersede an older one.
let chain: Promise<unknown> = Promise.resolve();
function enqueue<T>(fn: () => Promise<T>): Promise<T> {
  const p = chain.then(fn, fn);
  chain = p.catch(() => undefined);
  return p;
}

async function renderTile(imageId: number, adj: ParametricAdjustments, presetId: number | null): Promise<string | null> {
  const options = { maxEdge: 240, slot: "preview" as const, region: null };
  const r = await unwrap(presetId == null ? commands.renderPreview(imageId, adj, options) : commands.renderPreviewVariant(imageId, adj, { kind: "preset", presetId }, options));
  if (!r) return null;
  // The slot keeps only its newest render: copy this one out before the next tile replaces it.
  try {
    const blob = await (await fetch(r.url)).blob();
    return URL.createObjectURL(blob);
  } catch {
    return r.url;
  }
}

function Tile({ imageId, adj, preset, label, sub, selected, onPick, testid }: { imageId: number; adj: ParametricAdjustments | null; preset: StylePreset | null; label: string; sub?: string; selected: boolean; onPick: () => void; testid: string }) {
  const ref = useRef<HTMLButtonElement>(null);
  const [visible, setVisible] = useState(typeof IntersectionObserver === "undefined");
  const [url, setUrl] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el || typeof IntersectionObserver === "undefined") return;
    const io = new IntersectionObserver((es) => es.some((e) => e.isIntersecting) && (setVisible(true), io.disconnect()), { rootMargin: "200px" });
    io.observe(el);
    return () => io.disconnect();
  }, []);
  useEffect(() => {
    if (!visible || !adj) return;
    let dead = false;
    let made: string | null = null;
    setUrl(null);
    setFailed(false);
    void enqueue(async () => {
      if (dead) return;
      try {
        made = await renderTile(imageId, adj, preset?.id ?? null);
        if (dead) {
          if (made?.startsWith("blob:")) URL.revokeObjectURL(made);
          return;
        }
        setUrl(made);
        setFailed(made == null);
      } catch {
        if (!dead) setFailed(true);
      }
    });
    return () => {
      dead = true;
      if (made?.startsWith("blob:")) URL.revokeObjectURL(made);
    };
  }, [visible, adj, imageId, preset?.id]);
  return (
    <button
      ref={ref}
      type="button"
      data-testid={testid}
      data-selected={selected}
      aria-pressed={selected}
      title={preset ? `${label}: use this preset as the look for the whole shoot${sub ? ` (${sub})` : ""}` : "No preset: keep the anchor photo's own look and only correct each photo's light"}
      onClick={onPick}
      className={`group relative flex flex-col overflow-hidden rounded-lg bg-neutral-900 text-left ring-1 ${selected ? "ring-2 ring-emerald-500" : "ring-neutral-800 hover:ring-neutral-600"}`}
    >
      <span className="block aspect-[3/2] w-full bg-neutral-800">
        {url ? <img src={url} alt="" draggable={false} className="size-full object-cover" data-testid={`${testid}-img`} /> : <span className="flex size-full items-center justify-center text-[10px] text-neutral-500">{failed ? "No preview" : "Rendering…"}</span>}
      </span>
      {selected && <Check className="absolute right-1 top-1 size-4 rounded-full bg-emerald-600 p-0.5 text-white" aria-label="Selected" />}
      <span className="truncate px-2 pt-1 text-xs font-medium text-neutral-100">{label}</span>
      <span className="truncate px-2 pb-1 text-[10px] text-neutral-400">{sub ?? " "}</span>
    </button>
  );
}

interface Props {
  groups: StyleGroup[];
  anchorId: number;
  anchorName: string;
  presetId: number | null;
  onPick: (p: StylePreset | null) => void;
  onChangeAnchor: () => void;
  onError: (e: unknown) => void;
}

export function PresetStep({ groups, anchorId, anchorName, presetId, onPick, onChangeAnchor, onError }: Props) {
  const [adj, setAdj] = useState<ParametricAdjustments | null>(null);
  const [q, setQ] = useState("");
  const [group, setGroup] = useState<number | null>(null);
  useEffect(() => {
    let dead = false;
    setAdj(null);
    unwrap(commands.getAdjustments(anchorId))
      // The tiles show each preset on the anchor with its look reset to neutral (light stays), never on top of an earlier preset.
      .then((a) => !dead && setAdj(copyAdjustmentFields(a, defaultAdjustments(), BASELINE_LOOK_FIELDS)))
      .catch(onError);
    return () => {
      dead = true;
    };
  }, [anchorId, onError]);
  const shown = useMemo(() => groups.filter((g) => g.kind !== "luts" && g.presets.length > 0), [groups]);
  const needle = q.trim().toLowerCase();
  const list = useMemo(
    () =>
      shown
        .filter((g) => group == null || g.id === group)
        .map((g) => ({ g, presets: g.presets.filter((p) => !needle || p.name.toLowerCase().includes(needle) || g.name.toLowerCase().includes(needle)) }))
        .filter((x) => x.presets.length > 0),
    [shown, group, needle],
  );
  // Arrow-key navigation: the tiles re-render when the selection changes, so put focus back on the selected one.
  const gridRef = useRef<HTMLDivElement>(null);
  const keyNav = useRef(false);
  useEffect(() => {
    if (!keyNav.current) return;
    const g = gridRef.current;
    const sel = g?.querySelector<HTMLButtonElement>('button[data-testid^="baseline-preset-"][data-selected="true"]');
    if (g && sel && !g.contains(document.activeElement)) sel.focus();
  });
  const total = list.reduce((n, x) => n + x.presets.length, 0);
  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="baseline-step-preset">
      <div className="flex flex-wrap items-center gap-2 border-b border-neutral-800 px-4 py-2 text-xs">
        <label className="flex h-7 items-center gap-1.5 rounded-md bg-neutral-800 px-2" title="Search presets by name or group">
          <Search className="size-3.5 text-neutral-400" aria-hidden />
          <input data-testid="baseline-preset-search" aria-label="Search presets" title="Search presets by name or group" placeholder="Search presets" value={q} onChange={(e) => setQ(e.target.value)} className="w-48 bg-transparent outline-none placeholder:text-neutral-500" />
        </label>
        <div className="flex flex-wrap items-center gap-1" role="group" aria-label="Preset groups">
          <button type="button" data-testid="baseline-group-all" aria-pressed={group == null} title="Show presets from every group" onClick={() => setGroup(null)} className={`rounded px-2 py-1 ${group == null ? "bg-sky-800 text-sky-100" : "bg-neutral-800 text-neutral-300 hover:bg-neutral-700"}`}>
            All groups
          </button>
          {shown.map((g) => (
            <button key={g.id} type="button" data-testid={`baseline-group-${g.id}`} aria-pressed={group === g.id} title={`Only presets from ${g.name} (${g.presets.length})`} onClick={() => setGroup(group === g.id ? null : g.id)} className={`rounded px-2 py-1 ${group === g.id ? "bg-sky-800 text-sky-100" : "bg-neutral-800 text-neutral-300 hover:bg-neutral-700"}`}>
              {g.name} <span className="opacity-70">{g.presets.length}</span>
            </button>
          ))}
        </div>
        <span className="ml-auto text-neutral-400" data-testid="baseline-preview-on">
          Previews on <b className="text-neutral-200">{anchorName}</b>{" "}
          <button type="button" className="text-sky-300 hover:underline" data-testid="baseline-change-anchor" title="Choose a different photo to preview the presets on and to use as the anchor" onClick={onChangeAnchor}>
            change
          </button>
        </span>
      </div>
      <div
        className="min-h-0 flex-1 overflow-y-auto p-4"
        data-testid="baseline-preset-grid"
        ref={gridRef}
        onKeyDown={(e) => {
          // Arrow keys move between the tiles (and choose them); Enter then goes to the next step.
          const dir = e.key === "ArrowRight" || e.key === "ArrowDown" ? 1 : e.key === "ArrowLeft" || e.key === "ArrowUp" ? -1 : 0;
          if (!dir) return;
          const tiles = [...e.currentTarget.querySelectorAll<HTMLButtonElement>('button[data-testid^="baseline-preset-"]')];
          const at = tiles.indexOf(document.activeElement as HTMLButtonElement);
          const next = tiles[Math.min(tiles.length - 1, Math.max(0, (at < 0 ? tiles.findIndex((t) => t.dataset.selected === "true") : at) + dir))];
          if (!next) return;
          e.preventDefault();
          keyNav.current = true;
          next.focus();
          next.click();
        }}
      >
        <div className="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-3">
          {!needle && <Tile testid="baseline-preset-none" imageId={anchorId} adj={adj} preset={null} label="No preset" sub="your photo as it is" selected={presetId == null} onPick={() => onPick(null)} />}
        </div>
        {list.map(({ g, presets }) => (
          <section key={g.id} className="mt-4" data-testid={`baseline-preset-group-${g.id}`}>
            <h3 className="mb-2 text-xs font-semibold uppercase tracking-wide text-neutral-400">{g.name}</h3>
            <div className="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-3">
              {presets.map((p) => (
                <Tile key={p.id} testid={`baseline-preset-${p.id}`} imageId={anchorId} adj={adj} preset={p} label={p.name} sub={g.name} selected={presetId === p.id} onPick={() => onPick(p)} />
              ))}
            </div>
          </section>
        ))}
        {needle && total === 0 && (
          <p className="py-10 text-center text-sm text-neutral-400" data-testid="baseline-preset-empty">
            No preset matches &quot;{q}&quot;.
          </p>
        )}
        {shown.length === 0 && (
          <p className="py-6 text-center text-sm text-neutral-400" data-testid="baseline-preset-none-hint">
            No presets imported yet. Use Import Presets in Develop (Presets panel, +) to bring in your Lightroom presets, or continue with &quot;No preset&quot;.
          </p>
        )}
      </div>
    </div>
  );
}
