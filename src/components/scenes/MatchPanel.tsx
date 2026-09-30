// Scene match panel: options -> matchScene (one solve) -> per-target before / predicted previews.
// The strength slider only re-blends locally (lerpAdjustments) and re-renders; it never re-solves.
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AlertTriangle, CheckCircle2, Loader2, X } from "lucide-react";
import {
  ALL_ADJUSTMENT_FIELDS,
  commands,
  convertFileSrc,
  DEFAULT_MATCH_OPTIONS,
  lerpAdjustments,
  unwrap,
  type AdjustmentField,
  type MatchOptions,
  type MatchPreview,
  type RawImageEntry,
  type ParametricAdjustments,
  type Scene,
  type SceneProgress,
} from "../../ipc";
import { FieldsDialog } from "../develop/FieldsDialog";
import { Dialog } from "../Dialog";
import { formatError } from "../../lib/format";

interface Props {
  scene: Scene;
  sceneNumber: number;
  progress: SceneProgress | null;
  fileName: (id: number) => string;
  onClose: () => void;
  /** Called after apply_scene_match resolved with the changed image ids. */
  onApplied: (changed: number[], attempted: number[]) => void;
}

const RENDER_EDGE = 360;

export function MatchPanel({ scene, sceneNumber, progress, fileName: libName, onClose, onApplied }: Props) {
  const [rows, setRows] = useState<Map<number, RawImageEntry>>(new Map());
  const fileName = useCallback((id: number) => rows.get(id)?.fileName ?? libName(id), [rows, libName]);
  useEffect(() => {
    let live = true;
    (async () => {
      const m = new Map<number, RawImageEntry>();
      for (let i = 0; i < scene.imageIds.length; i += 200) {
        try {
          const list = await unwrap(commands.getImages(scene.imageIds.slice(i, i + 200)));
          list.forEach((r) => m.set(r.id, r));
        } catch {
          return;
        }
      }
      if (live) setRows(m);
    })();
    return () => {
      live = false;
    };
  }, [scene.imageIds]);

  const [opts, setOpts] = useState<MatchOptions>({ ...DEFAULT_MATCH_OPTIONS });
  const [previews, setPreviews] = useState<MatchPreview[] | null>(null);
  const [excluded, setExcluded] = useState<Set<number>>(new Set());
  const [solving, setSolving] = useState(false);
  const [applying, setApplying] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [fieldsOpen, setFieldsOpen] = useState(false);
  const [stale, setStale] = useState(false);
  const strength = opts.strength;

  const [includeRejected, setIncludeRejected] = useState(false);
  const nonAnchors = useMemo(() => scene.imageIds.filter((i) => !scene.anchorIds.includes(i)), [scene]);
  const rejectedCount = useMemo(() => nonAnchors.filter((i) => rows.get(i)?.pick === "reject").length, [nonAnchors, rows]);
  const targetIds = useMemo(() => (includeRejected ? nonAnchors : nonAnchors.filter((i) => rows.get(i)?.pick !== "reject")), [nonAnchors, rows, includeRejected]);

  const change = (patch: Partial<MatchOptions>, needsSolve = true) => {
    setOpts((o) => ({ ...o, ...patch }));
    if (needsSolve && previews) setStale(true);
  };

  const solve = async () => {
    setSolving(true);
    setError(null);
    try {
      const res = await unwrap(commands.matchScene(scene.anchorIds, targetIds, opts));
      setPreviews(res);
      setExcluded(new Set());
      setStale(false);
    } catch (e) {
      setError(formatError(e));
    } finally {
      setSolving(false);
    }
  };

  // Re-blended locally; stable identities keep the cards from re-rendering on unrelated state changes.
  const blended = useMemo(() => new Map((previews ?? []).map((p) => [p.targetId, lerpAdjustments(p.base, p.full, strength)])), [previews, strength]);
  const predicted = (p: MatchPreview) => blended.get(p.targetId)!;
  const selected = previews?.filter((p) => !excluded.has(p.targetId)) ?? [];

  const apply = async () => {
    if (selected.length === 0) return;
    setApplying(true);
    setError(null);
    try {
      const apps = selected.map((p) => ({ imageId: p.targetId, adjustments: predicted(p) }));
      const changed = await unwrap(commands.applySceneMatch(apps, "Match Scene"));
      onApplied(changed, apps.map((a) => a.imageId));
    } catch (e) {
      setError(formatError(e));
      setApplying(false);
    }
  };

  const toggle = (id: number) =>
    setExcluded((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });

  const pct = solving && progress?.task === "match" && progress.total > 0 ? Math.min(100, (progress.done / progress.total) * 100) : 0;
  const check = (label: string, key: "matchExposure" | "matchWhiteBalance" | "matchTone", testid: string) => (
    <label className="flex items-center gap-1.5">
      <input type="checkbox" checked={opts[key]} onChange={(e) => change({ [key]: e.target.checked })} data-testid={testid} />
      {label}
    </label>
  );

  return (
    <Dialog
      label={`Match Scene ${sceneNumber}`}
      testid="match-panel"
      overlayClass="z-40 bg-black/70"
      className="flex h-[88vh] w-[min(1200px,94vw)] flex-col rounded-lg border border-neutral-700 bg-neutral-900 shadow-xl"
      onCancel={onClose}
    >
      <>
        <div className="flex items-center gap-3 border-b border-neutral-800 px-4 py-2.5">
          <h2 className="text-sm font-semibold">Match Scene {sceneNumber}</h2>
          <div className="flex items-center gap-2" data-testid="match-anchors">
            {scene.anchorIds.map((id) => {
              const t = rows.get(id)?.thumbnail;
              return (
                <div key={id} className="flex items-center gap-1.5" data-testid={`match-anchor-${id}`}>
                  <span className="block size-16 shrink-0 overflow-hidden rounded bg-neutral-800">
                    {t?.status === "ready" && <img src={convertFileSrc(t.path)} alt="" className="size-full object-cover" draggable={false} />}
                  </span>
                  <span className="max-w-28 truncate text-xs text-neutral-300">{fileName(id)}</span>
                </div>
              );
            })}
          </div>
          <span className="text-xs text-neutral-400" data-testid="match-summary">
            {scene.anchorIds.length} anchor{scene.anchorIds.length === 1 ? "" : "s"} ({scene.anchorIds.map(fileName).join(", ")}) · {targetIds.length} target
            {targetIds.length === 1 ? "" : "s"}
          </span>
          <button className="ml-auto text-neutral-400 hover:text-white" onClick={onClose} aria-label="Close" data-testid="match-close">
            <X className="size-4" />
          </button>
        </div>

        <div className="flex flex-wrap items-center gap-x-5 gap-y-2 border-b border-neutral-800 px-4 py-2 text-xs text-neutral-300">
          {check("Exposure", "matchExposure", "match-exposure")}
          {check("White balance", "matchWhiteBalance", "match-wb")}
          {check("Tone", "matchTone", "match-tone")}
          <button className="text-sky-400 hover:underline" onClick={() => setFieldsOpen(true)} data-testid="match-copy-fields">
            Also copy from anchor: {opts.copyFields.length === ALL_ADJUSTMENT_FIELDS.length ? "All settings" : opts.copyFields.length === 0 ? "Nothing" : `${opts.copyFields.length} groups`} ▾
          </button>
          {rejectedCount > 0 && (
            <label className="flex items-center gap-1.5">
              <input
                type="checkbox"
                checked={includeRejected}
                data-testid="match-include-rejected"
                onChange={(e) => {
                  setIncludeRejected(e.target.checked);
                  if (previews) setStale(true);
                }}
              />
              Include rejected ({rejectedCount})
            </label>
          )}
          <label className="flex items-center gap-2">
            Strength
            <input
              type="range"
              min={0}
              max={100}
              step={1}
              value={Math.round(strength * 100)}
              onChange={(e) => change({ strength: Number(e.target.value) / 100 }, false)}
              data-testid="match-strength"
            />
            <span className="w-9 tabular-nums" data-testid="match-strength-value">
              {Math.round(strength * 100)}%
            </span>
          </label>
          <button
            className={`rounded px-3 py-1 text-white disabled:opacity-40 ${stale || !previews ? "bg-emerald-700 hover:bg-emerald-600" : "bg-neutral-700 hover:bg-neutral-600"}`}
            disabled={solving || targetIds.length === 0 || (!opts.matchExposure && !opts.matchWhiteBalance && !opts.matchTone && opts.copyFields.length === 0)}
            onClick={() => void solve()}
            data-testid="match-run"
          >
            {solving ? "Matching..." : previews ? (stale ? "Re-run (options changed)" : "Re-run") : "Preview match"}
          </button>
          {solving && (
            <div className="h-1.5 w-40 overflow-hidden rounded bg-neutral-800" data-testid="match-progress" data-pct={Math.round(pct)}>
              <div className="h-full bg-emerald-400 transition-[width]" style={{ width: `${pct}%` }} />
            </div>
          )}
        </div>

        {error && (
          <p className="bg-red-950 px-4 py-1.5 text-xs text-red-300" data-testid="match-error">
            {error}
          </p>
        )}

        <div className="min-h-0 flex-1 overflow-y-auto p-3" data-testid="match-results">
          {!previews && !solving && <p className="p-6 text-center text-sm text-neutral-400">Choose what to match, then press Preview match. Nothing is saved until you apply.</p>}
          {solving && !previews && <Loader2 className="mx-auto mt-10 size-6 animate-spin text-neutral-400" />}
          {previews && (
            <div className="grid gap-3" style={{ gridTemplateColumns: "repeat(auto-fill, minmax(340px, 1fr))", opacity: stale ? 0.5 : 1 }}>
              {previews.map((p) => (
                <MatchCard key={p.targetId} p={p} strength={strength} adjustments={predicted(p)} name={fileName(p.targetId)} included={!excluded.has(p.targetId)} onToggle={toggle} />
              ))}
            </div>
          )}
        </div>

        <div className="flex items-center gap-3 border-t border-neutral-800 px-4 py-2.5 text-xs text-neutral-400">
          {previews && (
            <>
              <span data-testid="match-selected-count">
                {selected.length} of {previews.length} selected
              </span>
              <button className="text-sky-400 hover:underline" onClick={() => setExcluded(new Set())} data-testid="match-select-all">
                Select all
              </button>
              <button className="text-sky-400 hover:underline" onClick={() => setExcluded(new Set(previews.filter((p) => !p.converged).map((p) => p.targetId)))} data-testid="match-deselect-unconverged">
                Deselect not converged
              </button>
              <span data-testid="match-unconverged-count">{previews.filter((p) => !p.converged).length} not converged</span>
            </>
          )}
          <button className="ml-auto rounded bg-neutral-800 px-3 py-1.5 hover:bg-neutral-700" onClick={onClose} data-testid="match-cancel">
            Cancel
          </button>
          <button
            className="rounded bg-emerald-700 px-3 py-1.5 text-white hover:bg-emerald-600 disabled:opacity-40"
            disabled={selected.length === 0 || applying || stale || solving}
            onClick={() => void apply()}
            data-testid="match-apply"
          >
            {applying ? "Applying..." : `Apply to ${selected.length} photo${selected.length === 1 ? "" : "s"}`}
          </button>
        </div>
      </>
      {fieldsOpen && (
        <FieldsDialog
          title="Copy from anchor"
          confirm="Use"
          initial={opts.copyFields}
          onCancel={() => setFieldsOpen(false)}
          onConfirm={(fields: AdjustmentField[]) => {
            change({ copyFields: fields });
            setFieldsOpen(false);
          }}
        />
      )}
    </Dialog>
  );
}

const sign = (n: number, digits = 2) => `${n >= 0 ? "+" : ""}${n.toFixed(digits)}`;

const MatchCard = memo(function MatchCard({
  p,
  strength,
  adjustments,
  name,
  included,
  onToggle,
}: {
  p: MatchPreview;
  strength: number;
  adjustments: ParametricAdjustments;
  name: string;
  included: boolean;
  onToggle: (id: number) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [visible, setVisible] = useState(false);
  const [beforeUrl, setBeforeUrl] = useState<string | null>(null);
  const [afterUrl, setAfterUrl] = useState<string | null>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const io = new IntersectionObserver((es) => es.some((e) => e.isIntersecting) && setVisible(true), { rootMargin: "200px" });
    io.observe(el);
    return () => io.disconnect();
  }, []);

  // Before: the target's stored adjustments (rendered once).
  useEffect(() => {
    if (!visible) return;
    let live = true;
    (async () => {
      try {
        const stored = await unwrap(commands.getAdjustments(p.targetId));
        const r = await unwrap(commands.renderPreview(p.targetId, stored, { maxEdge: RENDER_EDGE, slot: "before", region: null }));
        if (live && r) setBeforeUrl(r.url);
      } catch {
        /* preview is best-effort; the numbers below still show the correction */
      }
    })();
    return () => {
      live = false;
    };
  }, [visible, p.targetId]);

  // Predicted: re-rendered (debounced) when the strength changes; latest-wins on the backend.
  useEffect(() => {
    if (!visible) return;
    let live = true;
    const t = setTimeout(() => {
      unwrap(commands.renderPreview(p.targetId, adjustments, { maxEdge: RENDER_EDGE, slot: "main", region: null }))
        .then((r) => live && r && setAfterUrl(r.url))
        .catch(() => {});
    }, 120);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [visible, p.targetId, adjustments]);

  const d = p.delta;
  const wb = adjustments.whiteBalance;
  return (
    <div
      ref={ref}
      data-testid={`match-card-${p.targetId}`}
      data-included={included}
      data-converged={p.converged}
      data-base-exposure={p.base.exposure}
      data-full-exposure={p.full.exposure}
      data-predicted-exposure={adjustments.exposure}
      data-predicted-temp={wb.mode === "custom" ? wb.temperatureK : ""}
      className={`rounded border p-2 text-xs ${included ? "border-neutral-700 bg-neutral-950" : "border-neutral-800 bg-neutral-950/40 opacity-50"}`}
      style={{ contentVisibility: "auto", containIntrinsicSize: "260px" }}
    >
      <div className="mb-1.5 flex items-center gap-2">
        <input type="checkbox" checked={included} onChange={() => onToggle(p.targetId)} data-testid={`match-include-${p.targetId}`} aria-label={`Apply to ${name}`} />
        <span className="truncate font-medium text-neutral-200">{name}</span>
        {p.anchorIds.length > 1 && <span className="text-[10px] text-neutral-400">blend {Math.round(p.anchorWeight * 100)}%</span>}
        {p.converged ? (
          <span className="ml-auto flex items-center gap-0.5 text-emerald-400" title="Within tolerance of the anchor at full strength">
            <CheckCircle2 className="size-3.5" /> converged
          </span>
        ) : (
          <span className="ml-auto flex items-center gap-0.5 text-amber-400" title="Does not reach the anchor's look at full strength" data-testid={`match-warn-${p.targetId}`}>
            <AlertTriangle className="size-3.5" /> not converged
          </span>
        )}
      </div>
      <div className="grid grid-cols-2 gap-1.5">
        <Frame label="Before" url={beforeUrl} testid={`match-before-${p.targetId}`} />
        <Frame label={`Predicted ${Math.round(strength * 100)}%`} url={afterUrl} testid={`match-after-${p.targetId}`} />
      </div>
      <div className="mt-1.5 flex flex-wrap gap-x-3 gap-y-0.5 tabular-nums text-neutral-400" data-testid={`match-delta-${p.targetId}`}>
        {d.exposure !== 0 && <span>Exp {sign(d.exposure * strength)} EV</span>}
        {d.temperatureK !== 0 && <span>Temp {sign(d.temperatureK * strength, 0)} K</span>}
        {d.tint !== 0 && <span>Tint {sign(d.tint * strength, 1)}</span>}
        {d.contrast !== 0 && <span>Contrast {sign(d.contrast * strength, 1)}</span>}
        {d.whites !== 0 && <span>Whites {sign(d.whites * strength, 1)}</span>}
        {d.blacks !== 0 && <span>Blacks {sign(d.blacks * strength, 1)}</span>}
      </div>
      {p.notes.length > 0 && (
        <ul className="mt-1 list-disc pl-4 text-amber-300/90" data-testid={`match-notes-${p.targetId}`}>
          {p.notes.map((n, i) => (
            <li key={i}>{n}</li>
          ))}
        </ul>
      )}
    </div>
  );
});

function Frame({ label, url, testid }: { label: string; url: string | null; testid: string }) {
  return (
    <div className="relative aspect-[3/2] overflow-hidden rounded bg-neutral-900">
      {url ? <img src={url} alt={label} draggable={false} className="size-full object-contain" data-testid={testid} /> : <Loader2 className="absolute inset-0 m-auto size-4 animate-spin text-neutral-700" />}
      <span className="absolute left-1 top-1 rounded bg-black/60 px-1 text-[10px] text-neutral-200">{label}</span>
    </div>
  );
}
