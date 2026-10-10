// Baseline edit, step 4: edit the rest. Scope, skip / replace, a preview of sample photos across the scenes (the anchor
// pinned first, "After only" by default, hold \ for Before, click to enlarge), plan counts, Apply with progress + Stop,
// the result summary, the flagged photos and one Undo.
import { useEffect, useMemo, useState } from "react";
import { AlertTriangle, ArrowRight, Loader2, Undo2, X } from "lucide-react";
import { commands, events, unwrap, type BaselinePhotoResult, type BaselinePreview, type BaselineRun, type BaselineScope, type ParametricAdjustments } from "../../ipc";
import type { Library } from "../../hooks/useLibrary";
import { baselineSettings, describeOffset, KEEP_LATER_SUPPORTED, undoneText, type BaselineSession } from "../../hooks/useBaseline";
import { Thumb } from "../edit/bits";

type ScopeKind = BaselineScope["kind"];

interface Props {
  projectId: number;
  lib: Library;
  session: BaselineSession;
  anchorName: string;
  keeperCount: number;
  totalCount: number;
  selectedIds: number[];
  sceneLabel: (sceneId: number | null) => string;
  run: BaselineRun | null;
  /** Flagged photos the user has not marked "Looks good" yet. */
  reviewLeft: Set<number>;
  onRunStarted: (r: BaselineRun) => void;
  onUndo: (opts?: { keepLaterEdits: boolean }) => Promise<void>;
  /** Read the run (and its undo state) again. */
  onRefresh: () => void;
  onShowFlagged: () => void;
  onFinish: () => void;
  onError: (e: unknown) => void;
}

const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;
const DEFAULT_SAMPLES = 12;
const MAX_SAMPLES = 48;

async function renderSlot(id: number, adj: ParametricAdjustments, slot: "preview" | "before", maxEdge = 360): Promise<string | null> {
  try {
    const r = await unwrap(commands.renderPreview(id, adj, { maxEdge, slot, region: null }));
    return r?.url ?? null;
  } catch {
    return null;
  }
}

export function RestStep(p: Props) {
  const { projectId, session } = p;
  const anchorId = session.anchorId!;
  const [scope, setScope] = useState<ScopeKind>("keepers");
  const [replace, setReplace] = useState(false);
  const [rerun, setRerun] = useState(false);
  const [sampleCount, setSampleCount] = useState(DEFAULT_SAMPLES);
  const [preview, setPreview] = useState<BaselinePreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [urls, setUrls] = useState<Record<number, { before: string | null; after: string | null }>>({});
  const [anchorUrl, setAnchorUrl] = useState<string | null>(null);
  const [mode, setMode] = useState<"after" | "both">("after");
  const [holdBefore, setHoldBefore] = useState(false);
  const [lightbox, setLightbox] = useState<number | null>(null);
  const [big, setBig] = useState<{ before: string | null; after: string | null } | null>(null);
  const [prog, setProg] = useState<{ done: number; total: number } | null>(null);
  const [starting, setStarting] = useState(false);
  const [undoing, setUndoing] = useState(false);
  const [flagged, setFlagged] = useState<BaselinePhotoResult[]>([]);
  const [later, setLater] = useState<number[]>([]);
  const selKey = p.selectedIds.join(",");
  const settings = useMemo(
    () => baselineSettings(anchorId, session.presetId, scope === "selection" ? { kind: "selection", ids: p.selectedIds } : { kind: scope }, replace),
    [anchorId, session.presetId, scope, replace, selKey], // eslint-disable-line react-hooks/exhaustive-deps
  );
  const running = p.run?.state === "running";
  const finished = p.run?.state === "finished" && p.run.settings.anchorId === anchorId;
  const done = finished && !rerun;
  const { onRefresh } = p;

  // The batch may have been undone (or blocked) elsewhere since this run finished.
  useEffect(() => onRefresh(), []); // eslint-disable-line react-hooks/exhaustive-deps

  // Progress of the run (activity events) -- the corner widget shows it too.
  useEffect(() => {
    let off: (() => void) | undefined;
    let dead = false;
    void events.activityEvent.listen((e) => e.payload.kind === "baseline_edit" && setProg({ done: e.payload.done ?? 0, total: e.payload.total ?? 0 })).then((u) => (dead ? u() : (off = u)));
    return () => {
      dead = true;
      off?.();
    };
  }, []);

  // Plan counts + the sample grid for the chosen scope.
  useEffect(() => {
    if (done || running) return;
    if (scope === "selection" && p.selectedIds.length === 0) {
      setPreview(null);
      setPreviewError("Select some photos in the grid first.");
      return;
    }
    let stale = false;
    const t = setTimeout(() => {
      setPreviewError(null);
      unwrap(commands.previewBaseline(projectId, settings, { sampleCount, imageIds: null }))
        .then((pv) => !stale && setPreview(pv))
        .catch((e) => !stale && (setPreview(null), setPreviewError(String((e as { message?: string })?.message ?? e))));
    }, 250);
    return () => {
      stale = true;
      clearTimeout(t);
    };
  }, [projectId, settings, sampleCount, done, running]); // eslint-disable-line react-hooks/exhaustive-deps

  // Before / after renders: three at a time, before in slot `before`, after in slot `preview`.
  useEffect(() => {
    setUrls({});
    if (!preview) return;
    let stale = false;
    let next = 0;
    const items = preview.samples;
    const work = async () => {
      while (!stale) {
        const s = items[next++];
        if (!s) return;
        const id = s.photo.imageId;
        const [before, after] = await Promise.all([renderSlot(id, s.before, "before"), renderSlot(id, s.after, "preview")]);
        if (!stale) setUrls((u) => ({ ...u, [id]: { before, after } }));
      }
    };
    void Promise.all([work(), work(), work()]);
    return () => {
      stale = true;
    };
  }, [preview]);
  const { ensure } = p.lib;
  useEffect(() => ensure(preview?.samples.map((s) => s.photo.imageId) ?? []), [preview, ensure]);

  // The anchor's current render, pinned first: the consistency check is "do all of them look like this".
  const anchorKey = preview ? `${anchorId}:${preview.anchor.light.exposure}:${preview.anchor.light.temperatureK}:${session.presetId}` : null;
  useEffect(() => {
    if (!anchorKey) return;
    let dead = false;
    unwrap(commands.getAdjustments(anchorId))
      .then((a) => renderSlot(anchorId, a, "preview"))
      .then((u) => !dead && setAnchorUrl(u))
      .catch(() => {});
    return () => {
      dead = true;
    };
  }, [anchorKey, anchorId]);

  // Hold \ = Before (Lightroom's key); releasing it shows After again.
  useEffect(() => {
    const isBs = (e: KeyboardEvent) => e.key === "\\" && !e.metaKey && !e.ctrlKey && !e.altKey;
    const down = (e: KeyboardEvent) => {
      if (isBs(e) && !(e.target instanceof HTMLInputElement) && !(e.target instanceof HTMLTextAreaElement)) {
        e.preventDefault();
        setHoldBefore(true);
      }
    };
    const up = (e: KeyboardEvent) => e.key === "\\" && setHoldBefore(false);
    const blur = () => setHoldBefore(false);
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    window.addEventListener("blur", blur);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
      window.removeEventListener("blur", blur);
    };
  }, []);

  const samples = preview?.samples ?? [];
  // Enlarged view: Before | After side by side at fit size, <- / -> through the samples, Esc closes.
  useEffect(() => {
    if (lightbox == null) return;
    const s = samples[lightbox];
    if (!s) return setLightbox(null);
    let dead = false;
    setBig(null);
    void Promise.all([renderSlot(s.photo.imageId, s.before, "before", 1400), renderSlot(s.photo.imageId, s.after, "preview", 1400)]).then(([before, after]) => !dead && setBig({ before, after }));
    return () => {
      dead = true;
    };
  }, [lightbox, preview]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    if (lightbox == null) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setLightbox(null);
      else if (e.key === "ArrowRight") setLightbox((i) => (i == null ? i : Math.min(samples.length - 1, i + 1)));
      else if (e.key === "ArrowLeft") setLightbox((i) => (i == null ? i : Math.max(0, i - 1)));
      else return;
      e.preventDefault();
      e.stopImmediatePropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [lightbox, samples.length]);

  // The photos that need a look, after a run.
  const runId = p.run?.id;
  useEffect(() => {
    if (!done || !p.run || p.run.counts.flagged === 0) return setFlagged([]);
    let dead = false;
    unwrap(commands.getBaselineResults(projectId, ["flagged"]))
      .then((r) => {
        if (dead) return;
        ensure(r.slice(0, 6).map((x) => x.imageId));
        setFlagged(r);
      })
      .catch(p.onError);
    return () => {
      dead = true;
    };
  }, [done, runId, projectId]); // eslint-disable-line react-hooks/exhaustive-deps

  // Photos the user changed after the baseline: they block the one-step Undo.
  const batch = p.run?.batch;
  const undone = batch?.undoneAtMs != null;
  const conflicts = !undone ? (batch?.conflictCount ?? 0) : 0;
  useEffect(() => {
    if (!done || (conflicts === 0 && !undone)) return void setLater([]);
    let dead = false;
    void (async () => {
      try {
        const all = await unwrap(commands.getBaselineResults(projectId, null));
        const prov = await unwrap(commands.getBaselineProvenance(all.map((x) => x.imageId)));
        const ids = prov.filter((x) => x.state === "user_edited").map((x) => x.imageId);
        if (dead) return;
        ensure(ids.slice(0, 3));
        setLater(ids);
      } catch {
        /* the message works without the names */
      }
    })();
    return () => {
      dead = true;
    };
  }, [done, runId, conflicts, undone, projectId]); // eslint-disable-line react-hooks/exhaustive-deps

  const apply = async () => {
    setStarting(true);
    setProg(null);
    try {
      const r = await unwrap(commands.runBaseline(projectId, settings));
      setRerun(false);
      p.onRunStarted(r);
    } catch (e) {
      p.onError(e);
    } finally {
      setStarting(false);
    }
  };
  const undo = async (keepLaterEdits = false) => {
    setUndoing(true);
    try {
      await p.onUndo(keepLaterEdits ? { keepLaterEdits: true } : undefined);
      p.onRefresh();
    } finally {
      setUndoing(false);
    }
  };

  const c = preview?.counts;
  const scopeCount = scope === "keepers" ? p.keeperCount : scope === "all" ? p.totalCount : p.selectedIds.length;
  const radio = (name: string, value: string, checked: boolean, set: () => void, label: string, title: string, testid: string, disabled = false) => (
    <label className={`flex items-center gap-2 rounded px-2 py-1 ${checked ? "bg-neutral-800" : "hover:bg-neutral-900"} ${disabled ? "opacity-40" : "cursor-pointer"}`} title={title}>
      <input type="radio" name={name} value={value} checked={checked} disabled={disabled} onChange={set} data-testid={testid} title={title} className="accent-emerald-500" />
      <span className="text-xs">{label}</span>
    </label>
  );
  const nm = (id: number) => p.lib.getEntry(id)?.fileName ?? "…";
  const stubEngine = p.run?.engineVersion.startsWith("baseline-stub") ?? false;
  const left = flagged.filter((f) => p.reviewLeft.has(f.imageId)).length;
  const laterNames = later.slice(0, 3).map(nm).join(", ") + (later.length > 3 ? ` and ${later.length - 3} more` : "");
  const restCount = Math.max(0, (batch?.imageCount ?? 0) - (batch?.conflictCount ?? 0));
  const unitFor = (sc: ScopeKind) => (sc === "keepers" ? "keeper" : "photo");

  // Samples grouped by scene (After only) or in plan order (Before / after).
  const grouped = useMemo(() => {
    const m = new Map<number | null, typeof samples>();
    samples.forEach((s) => m.set(s.photo.sceneId, [...(m.get(s.photo.sceneId) ?? []), s]));
    return [...m];
  }, [preview]); // eslint-disable-line react-hooks/exhaustive-deps

  const tile = (s: (typeof samples)[number], idx: number) => {
    const u = urls[s.photo.imageId];
    const entry = p.lib.getEntry(s.photo.imageId);
    const flaggedS = s.photo.outcome === "flagged";
    const skipped = s.photo.outcome === "skipped_edited";
    const img = (k: "before" | "after", show: boolean) =>
      u?.[k] ? (
        <img src={u[k]!} alt={`${k === "before" ? "Before" : "After"}: ${entry?.fileName ?? ""}`} draggable={false} className={`size-full object-cover ${show ? "" : "hidden"}`} data-testid={`baseline-sample-${s.photo.imageId}-${k}`} />
      ) : show ? (
        <span className="flex size-full items-center justify-center text-[10px] text-neutral-500">{u ? "No preview" : "Rendering…"}</span>
      ) : null;
    return (
      <figure key={s.photo.imageId} className="rounded-lg bg-neutral-900 p-1.5 ring-1 ring-neutral-800" data-testid={`baseline-sample-${s.photo.imageId}`} data-outcome={s.photo.outcome}>
        {mode === "both" ? (
          <div className="grid cursor-zoom-in grid-cols-2 gap-1" onClick={() => setLightbox(idx)} title="Click to enlarge (Before | After)">
            {(["before", "after"] as const).map((k) => (
              <div key={k} className="relative aspect-[3/2] overflow-hidden rounded bg-neutral-800">
                {img(k, true)}
                <span className="absolute left-1 top-1 rounded bg-black/70 px-1 text-[10px] text-neutral-200">{k === "before" ? "Before" : "After"}</span>
              </div>
            ))}
          </div>
        ) : (
          <div className="relative aspect-[3/2] cursor-zoom-in overflow-hidden rounded bg-neutral-800" onClick={() => setLightbox(idx)} title="Click to enlarge (Before | After). Hold \ to see the Before">
            {img("after", !holdBefore)}
            {img("before", holdBefore)}
            <span className="absolute left-1 top-1 rounded bg-black/70 px-1 text-[10px] text-neutral-200">{holdBefore ? "Before" : "After"}</span>
          </div>
        )}
        <figcaption className="px-1 pt-1 text-[11px] text-neutral-300">
          <span className="flex items-center gap-1.5">
            <span data-testid={`baseline-sample-${s.photo.imageId}-scene`}>{p.sceneLabel(s.photo.sceneId)}</span>
            <span className="min-w-0 truncate text-neutral-500">{entry?.fileName}</span>
            {skipped && (
              <span className="ml-auto shrink-0 rounded bg-neutral-800 px-1 text-neutral-300" title="You edited this photo yourself; it is left as it is unless you choose Replace">
                kept
              </span>
            )}
          </span>
          {flaggedS && (
            <span className="block truncate text-amber-300" data-testid={`baseline-sample-${s.photo.imageId}-reason`} title={s.photo.reasons.map((r) => r.text).join("; ")}>
              Needs a look: {s.photo.reasons.map((r) => r.text).join("; ") || "check it"}
            </span>
          )}
        </figcaption>
      </figure>
    );
  };

  return (
    <div className="min-h-0 flex-1 overflow-y-auto" data-testid="baseline-step-rest">
      <div className="mx-auto max-w-[1100px] space-y-4 p-4">
        <div className="rounded-lg bg-neutral-900 p-3 text-xs text-neutral-300" data-testid="baseline-summary-line">
          Preset <b className="text-neutral-100">{session.presetName ?? "none"}</b> · anchor <b className="text-neutral-100">{p.anchorName}</b>
          {p.run?.anchor && (
            <>
              {" "}
              · your photo: <b className="text-neutral-100">{describeOffset(p.run.anchor.offset)}</b>
            </>
          )}
          <span className="block pt-1 text-neutral-400" data-testid="baseline-summary-model">
            Colours, profile, curve, grain and detail are copied from the anchor (the preset plus your changes). Exposure, contrast, highlights, shadows, whites, blacks and white balance are set for each photo: its own Auto, plus how your anchor differs from its Auto ({describeOffset(p.run?.anchor?.offset ?? preview?.anchor.offset) || "measuring…"}). Frames of one burst get matching values.
          </span>
        </div>

        {(running || starting) && (
          <div className="rounded-lg bg-emerald-950 p-3" data-testid="baseline-progress" role="status">
            <div className="mb-2 flex items-center gap-2 text-sm text-emerald-100">
              <Loader2 className="size-4 animate-spin" aria-hidden />
              <span data-testid="baseline-progress-text">{prog && prog.total > 0 ? `Editing photos… ${prog.done} of ${prog.total}` : "Editing photos…"}</span>
              <button type="button" className="ml-auto flex items-center gap-1 rounded-md bg-neutral-800 px-2 py-1 text-xs hover:bg-neutral-700" data-testid="baseline-cancel" title="Stop now. Nothing is changed until the run is complete" onClick={() => void unwrap(commands.cancelBaseline()).catch(p.onError)}>
                <X className="size-3.5" /> Stop
              </button>
            </div>
            <div className="h-1.5 overflow-hidden rounded-full bg-emerald-900">
              <div className="h-full bg-emerald-400 transition-all" style={{ width: `${prog && prog.total > 0 ? Math.round((prog.done / prog.total) * 100) : 0}%` }} />
            </div>
          </div>
        )}

        {p.run && p.run.state !== "running" && !starting && (p.run.state !== "finished" || done) && (
          <div className={`rounded-lg p-3 ${p.run.state === "finished" ? "bg-neutral-900 ring-1 ring-emerald-800" : "bg-amber-950 ring-1 ring-amber-800"}`} data-testid="baseline-result" data-state={p.run.state}>
            <p className="text-sm font-medium text-neutral-100" data-testid="baseline-result-message">
              {p.run.state === "cancelled" ? "Stopped. Nothing was changed." : p.run.state === "failed" ? `The run failed: ${p.run.message ?? "unknown error"}` : undone ? undoneText(later.map(nm), batch?.imageCount ?? 0) : (p.run.message ?? "Done")}
            </p>
            {p.run.state === "finished" && stubEngine && !undone && (
              <p className="mt-1 text-xs text-amber-300" data-testid="baseline-stub-note">
                Preview engine: light is plain Auto for now (your anchor&apos;s offset is not applied yet).
              </p>
            )}
            {p.run.state === "finished" && !undone && (
              <div className="mt-2 flex flex-wrap items-center gap-2 text-xs">
                <span className="rounded bg-emerald-950 px-2 py-0.5 text-emerald-200" data-testid="baseline-count-applied" title="Photos edited with the baseline that look fine">
                  Look fine <b>{p.run.counts.applied}</b>
                </span>
                <span className="rounded bg-amber-950 px-2 py-0.5 text-amber-200" data-testid="baseline-count-flagged" title="Edited, but worth a second look (dark on purpose, silhouettes, mixed light, Auto failed, a value at the end of its slider)">
                  Need a look <b>{p.run.counts.flagged}</b>
                </span>
                <span className="rounded bg-neutral-800 px-2 py-0.5 text-neutral-300" data-testid="baseline-count-skipped" title="Photos that already had an edit; they were left alone">
                  Skipped (already edited) <b>{p.run.counts.skippedEdited}</b>
                </span>
                {p.run.counts.failed > 0 && (
                  <span className="rounded bg-red-950 px-2 py-0.5 text-red-200" data-testid="baseline-count-failed" title="Photos whose original could not be read">
                    Could not read <b>{p.run.counts.failed}</b>
                  </span>
                )}
              </div>
            )}
            <div className="mt-3 flex flex-wrap items-center gap-2">
              {p.run.state === "finished" && (
                <button
                  type="button"
                  className="flex h-7 items-center gap-1.5 rounded-md bg-neutral-800 px-3 text-xs hover:bg-neutral-700 disabled:opacity-40"
                  data-testid="baseline-undo"
                  disabled={!batch || !batch.undoable || undone || undoing}
                  title={undone ? "Already undone" : !batch ? "Nothing was written" : batch.undoable ? `Put all ${plural(batch.imageCount, "photo")} back to how they were before the baseline (Cmd+Z)` : "Some photos were edited after the baseline, so it cannot be undone as one step"}
                  onClick={() => void undo()}
                >
                  <Undo2 className="size-3.5" /> {undone ? "Undone" : "Undo baseline"}
                </button>
              )}
              {conflicts > 0 && KEEP_LATER_SUPPORTED && (
                <button type="button" className="flex h-7 items-center gap-1.5 rounded-md bg-neutral-800 px-3 text-xs font-medium hover:bg-neutral-700 disabled:opacity-40" data-testid="baseline-undo-rest" disabled={undoing} title={`Put the other ${plural(restCount, "photo")} back and keep the ${plural(later.length, "change")} you made since`} onClick={() => void undo(true)}>
                  <Undo2 className="size-3.5" /> Undo the rest ({restCount})
                </button>
              )}
              {p.run.counts.flagged > 0 && !undone && (
                <button type="button" className="flex h-7 items-center gap-1.5 rounded-md bg-amber-800 px-3 text-xs font-medium text-white hover:bg-amber-700" data-testid="baseline-show-flagged" title="List only the photos that need a look in the grid, each with its reason" onClick={p.onShowFlagged}>
                  <AlertTriangle className="size-3.5" /> Show the {flagged.length > 0 ? left : p.run.counts.flagged} that need a look
                </button>
              )}
              <button type="button" className="h-7 rounded-md bg-neutral-800 px-3 text-xs hover:bg-neutral-700" data-testid="baseline-rerun" title="Change the scope or replace setting and run the baseline again (only photos still on the baseline are updated)" onClick={() => setRerun(true)}>
                Change settings and run again
              </button>
              {p.run.state === "finished" && !undone && (
                <button type="button" className="ml-auto flex h-7 items-center gap-1.5 rounded-md bg-emerald-700 px-3 text-xs font-medium text-white hover:bg-emerald-600" data-testid="baseline-to-finish" title="Last step: make sure the sidecars are saved and open them in Lightroom (Cmd+Enter)" onClick={p.onFinish}>
                  Finish in Lightroom <ArrowRight className="size-3.5" />
                </button>
              )}
            </div>
            {conflicts > 0 && p.run.state === "finished" && (
              <p className="mt-2 text-xs text-amber-300" data-testid="baseline-undo-blocked">
                You changed {plural(later.length || conflicts, "photo")} after the baseline{later.length > 0 ? ` (${laterNames})` : ""}.{" "}
                {KEEP_LATER_SUPPORTED ? (
                  <>
                    <b>Undo the rest</b> puts the other {restCount} back and keeps your change.
                  </>
                ) : (
                  "Undo that change in Develop first, then Undo baseline."
                )}
              </p>
            )}
            {flagged.length > 0 && !undone && (
              <ul className="mt-3 space-y-1" data-testid="baseline-flagged-list">
                {flagged.slice(0, 6).map((f) => (
                  <li key={f.imageId} className="flex items-center gap-2 text-xs" data-testid={`baseline-flagged-${f.imageId}`} data-reviewed={!p.reviewLeft.has(f.imageId)}>
                    <Thumb entry={p.lib.getEntry(f.imageId)} version={p.lib.version(f.imageId)} className="h-8 w-12 shrink-0 rounded" />
                    <span className="w-32 shrink-0 truncate text-neutral-200">{p.lib.getEntry(f.imageId)?.fileName ?? "…"}</span>
                    <span className="min-w-0 truncate text-amber-200">{f.reasons.map((r) => r.text).join("; ")}</span>
                    {!p.reviewLeft.has(f.imageId) && <span className="ml-auto shrink-0 text-emerald-300">checked</span>}
                  </li>
                ))}
                {flagged.length > 6 && <li className="text-xs text-neutral-400">and {flagged.length - 6} more</li>}
              </ul>
            )}
          </div>
        )}

        {!done && !running && (
          <>
            <div className="flex flex-wrap gap-x-8 gap-y-2" data-testid="baseline-options">
              <fieldset className="space-y-0.5" role="radiogroup" aria-label="Which photos">
                <legend className="mb-1 text-xs font-semibold uppercase tracking-wide text-neutral-400">Which photos</legend>
                {radio("scope", "keepers", scope === "keepers", () => setScope("keepers"), `Keepers (${p.keeperCount})`, "The photos that count as keepers: the set you deliver after culling", "baseline-scope-keepers")}
                {radio("scope", "all", scope === "all", () => setScope("all"), `All photos (${p.totalCount})`, "Every photo in the project, rejects included", "baseline-scope-all")}
                {radio("scope", "selection", scope === "selection", () => setScope("selection"), `Selected photos (${p.selectedIds.length})`, p.selectedIds.length === 0 ? "Select photos in the grid first (close this and select some)" : "Only the photos currently selected in the grid", "baseline-scope-selection", p.selectedIds.length === 0)}
              </fieldset>
              <fieldset className="space-y-0.5" role="radiogroup" aria-label="Photos that already have an edit">
                <legend className="mb-1 text-xs font-semibold uppercase tracking-wide text-neutral-400" title="Yours, Auto edit, Apply to scene or a sidecar">
                  Photos that already have an edit (yours, Auto edit, Apply to scene or a sidecar)
                </legend>
                {radio("replace", "skip", !replace, () => setReplace(false), c ? `Skip them (${c.edited})` : "Skip them", "Leave photos that already have an edit alone (recommended)", "baseline-skip")}
                {radio("replace", "replace", replace, () => setReplace(true), c ? `Replace their edit (${c.edited})` : "Replace their edit", "Overwrite their existing edit with the baseline. You can undo this in one step", "baseline-replace")}
              </fieldset>
            </div>

            <div>
              <div className="mb-2 flex flex-wrap items-baseline gap-3">
                <h3 className="text-xs font-semibold uppercase tracking-wide text-neutral-400">Preview across your scenes</h3>
                <span className="text-xs text-neutral-400" data-testid="baseline-plan-counts">
                  {c ? (
                    <>
                      Will edit <b className="text-neutral-200">{c.toWrite}</b> of {plural(c.inScope, "photo")} in scope
                      {c.edited > 0 && <> · {replace ? `${c.edited} already edited will be replaced` : `${c.edited} already edited will be skipped`}</>}
                      {c.onBaseline > 0 && <> · {c.onBaseline} already on a baseline will be updated</>}
                    </>
                  ) : (
                    (previewError ?? "Measuring…")
                  )}
                </span>
                <span className="ml-auto flex items-center gap-2 text-xs text-neutral-400">
                  <span className="flex overflow-hidden rounded-md bg-neutral-800" role="group" aria-label="Preview style" data-testid="baseline-view-toggle">
                    {(["after", "both"] as const).map((m) => (
                      <button key={m} type="button" aria-pressed={mode === m} data-testid={`baseline-view-${m}`} title={m === "after" ? "One tile per photo with the baseline applied: check that they all look alike" : "Each photo before and after the baseline"} onClick={() => setMode(m)} className={`px-2 py-1 ${mode === m ? "bg-sky-800 text-sky-100" : "text-neutral-300 hover:bg-neutral-700"}`}>
                        {m === "after" ? "After only" : "Before / after"}
                      </button>
                    ))}
                  </span>
                  <span>Hold \ for Before</span>
                </span>
              </div>
              {previewError && !c && (
                <p className="text-xs text-amber-300" data-testid="baseline-preview-error">
                  {previewError}
                </p>
              )}
              <div className={`grid gap-3 ${mode === "after" ? "grid-cols-[repeat(auto-fill,minmax(220px,1fr))]" : "grid-cols-[repeat(auto-fill,minmax(260px,1fr))]"}`} data-testid="baseline-samples">
                {preview && (
                  <figure className="rounded-lg bg-neutral-900 p-1.5 ring-2 ring-emerald-500" data-testid="baseline-pinned-anchor">
                    <div className="relative aspect-[3/2] overflow-hidden rounded bg-neutral-800">
                      {anchorUrl ? <img src={anchorUrl} alt={`Anchor: ${p.anchorName}`} draggable={false} className="size-full object-cover" data-testid="baseline-pinned-anchor-img" /> : <span className="flex size-full items-center justify-center text-[10px] text-neutral-500">Rendering…</span>}
                    </div>
                    <figcaption className="px-1 pt-1 text-[11px] text-emerald-200">
                      Anchor · your edit <span className="text-neutral-500">{p.anchorName}</span>
                    </figcaption>
                  </figure>
                )}
                {mode === "both"
                  ? samples.map((s, i) => tile(s, i))
                  : grouped.map(([sceneId, list]) => (
                      <div key={sceneId ?? "none"} className="contents" data-testid={`baseline-scene-${sceneId}`}>
                        <h4 className="col-span-full -mb-1 mt-1 text-[11px] font-semibold uppercase tracking-wide text-neutral-400">{p.sceneLabel(sceneId)}</h4>
                        {list.map((s) => tile(s, samples.indexOf(s)))}
                      </div>
                    ))}
              </div>
              {preview && sampleCount < MAX_SAMPLES && samples.length >= sampleCount && (
                <button type="button" className="mt-3 h-7 rounded-md bg-neutral-800 px-3 text-xs hover:bg-neutral-700" data-testid="baseline-more-samples" title="Preview more photos from across the shoot" onClick={() => setSampleCount((n) => Math.min(MAX_SAMPLES, n + 24))}>
                  Show {Math.min(24, MAX_SAMPLES - sampleCount)} more
                </button>
              )}
            </div>

            <div className="sticky bottom-0 flex items-center gap-3 border-t border-neutral-800 bg-neutral-950/95 py-3">
              <button
                type="button"
                className="flex h-9 items-center gap-2 rounded-md bg-emerald-700 px-5 text-sm font-semibold text-white hover:bg-emerald-600 disabled:opacity-40"
                data-testid="baseline-apply"
                disabled={starting || !c || c.toWrite === 0}
                title={!c ? "Waiting for the preview" : c.toWrite === 0 ? "There is nothing to edit with these settings" : `Edit ${plural(c.toWrite, "photo")} now. Everything can be undone in one step (Cmd+Enter)`}
                onClick={() => void apply()}
              >
                {c ? `Edit ${plural(c.toWrite, "photo")}` : "Edit the rest"} <ArrowRight className="size-4" />
              </button>
              {rerun && finished && (
                <button type="button" className="h-9 rounded-md bg-neutral-800 px-4 text-sm hover:bg-neutral-700" data-testid="baseline-rerun-cancel" title="Keep the result as it is and go back to it" onClick={() => setRerun(false)}>
                  Cancel
                </button>
              )}
              <span className="text-xs text-neutral-400">{scope === "selection" ? `${plural(scopeCount, "selected photo")}` : `${plural(scopeCount, unitFor(scope))}`} in scope. Crop, straighten, masks, spot removal and lens corrections are never changed.</span>
            </div>
          </>
        )}
      </div>

      {lightbox != null && samples[lightbox] && (
        <div className="fixed inset-0 z-50 flex flex-col bg-black/90" role="dialog" aria-modal="true" aria-label="Before and after" data-testid="baseline-lightbox">
          <div className="flex h-10 shrink-0 items-center gap-3 px-4 text-xs text-neutral-200">
            <span>
              {p.sceneLabel(samples[lightbox].photo.sceneId)} · {nm(samples[lightbox].photo.imageId)} · {lightbox + 1} of {samples.length}
            </span>
            <span className="text-neutral-400">Left / Right: next photo · Esc: close</span>
            <button type="button" className="ml-auto rounded p-1 hover:bg-neutral-800" aria-label="Close" title="Close (Esc)" data-testid="baseline-lightbox-close" onClick={() => setLightbox(null)}>
              <X className="size-4" />
            </button>
          </div>
          <div className="grid min-h-0 flex-1 grid-cols-2 gap-2 px-4 pb-4">
            {(["before", "after"] as const).map((k) => (
              <div key={k} className="relative flex min-h-0 items-center justify-center overflow-hidden rounded bg-neutral-900">
                {big?.[k] ? <img src={big[k]!} alt={k === "before" ? "Before" : "After"} className="max-h-full max-w-full object-contain" data-testid={`baseline-lightbox-${k}`} /> : <span className="text-xs text-neutral-500">{big ? "No preview" : "Rendering…"}</span>}
                <span className="absolute left-2 top-2 rounded bg-black/70 px-1.5 py-0.5 text-[11px] text-neutral-200">{k === "before" ? "Before" : "After"}</span>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

export { type ScopeKind };
