// Baseline edit, step 4: edit the rest. Scope, skip / replace, a before / after grid of sample photos across the
// scenes, plan counts, Apply with progress + Stop, the result summary, the flagged photos and one Undo.
import { useEffect, useMemo, useState } from "react";
import { AlertTriangle, ArrowRight, Loader2, Undo2, X } from "lucide-react";
import { commands, events, unwrap, type BaselinePhotoResult, type BaselinePreview, type BaselineRun, type BaselineScope, type ParametricAdjustments } from "../../ipc";
import type { Library } from "../../hooks/useLibrary";
import { baselineSettings, describeOffset, type BaselineSession } from "../../hooks/useBaseline";
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
  onRunStarted: (r: BaselineRun) => void;
  onUndo: () => Promise<void>;
  onShowFlagged: () => void;
  onFinish: () => void;
  onError: (e: unknown) => void;
}

const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;

async function renderSlot(id: number, adj: ParametricAdjustments, slot: "preview" | "before"): Promise<string | null> {
  try {
    const r = await unwrap(commands.renderPreview(id, adj, { maxEdge: 360, slot, region: null }));
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
  const [preview, setPreview] = useState<BaselinePreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [urls, setUrls] = useState<Record<number, { before: string | null; after: string | null }>>({});
  const [prog, setProg] = useState<{ done: number; total: number } | null>(null);
  const [starting, setStarting] = useState(false);
  const [undoing, setUndoing] = useState(false);
  const [flagged, setFlagged] = useState<BaselinePhotoResult[]>([]);
  const selKey = p.selectedIds.join(",");
  const settings = useMemo(
    () => baselineSettings(anchorId, session.presetId, scope === "selection" ? { kind: "selection", ids: p.selectedIds } : { kind: scope }, replace),
    [anchorId, session.presetId, scope, replace, selKey], // eslint-disable-line react-hooks/exhaustive-deps
  );
  const running = p.run?.state === "running";
  const finished = p.run?.state === "finished" && p.run.settings.anchorId === anchorId;
  const done = finished && !rerun;

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
      unwrap(commands.previewBaseline(projectId, settings, { sampleCount: 12, imageIds: null }))
        .then((pv) => !stale && setPreview(pv))
        .catch((e) => !stale && (setPreview(null), setPreviewError(String((e as { message?: string })?.message ?? e))));
    }, 250);
    return () => {
      stale = true;
      clearTimeout(t);
    };
  }, [projectId, settings, done, running]); // eslint-disable-line react-hooks/exhaustive-deps

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

  // The photos that need a look, after a run.
  const runId = p.run?.id;
  useEffect(() => {
    if (!done || !p.run || p.run.counts.flagged === 0) return setFlagged([]);
    unwrap(commands.getBaselineResults(projectId, ["flagged"]))
      .then((r) => {
        setFlagged(r);
        ensure(r.slice(0, 6).map((x) => x.imageId));
      })
      .catch(p.onError);
  }, [done, runId, projectId]); // eslint-disable-line react-hooks/exhaustive-deps

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
  const undo = async () => {
    setUndoing(true);
    try {
      await p.onUndo();
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
  const batch = p.run?.batch;
  const undone = batch?.undoneAtMs != null;

  return (
    <div className="min-h-0 flex-1 overflow-y-auto" data-testid="baseline-step-rest">
      <div className="mx-auto max-w-[1100px] space-y-4 p-4">
        <div className="rounded-lg bg-neutral-900 p-3 text-xs text-neutral-300" data-testid="baseline-summary-line">
          Preset <b className="text-neutral-100">{session.presetName ?? "none"}</b> · anchor <b className="text-neutral-100">{p.anchorName}</b>
          {p.run?.anchor && <> · your photo: <b className="text-neutral-100">{describeOffset(p.run.anchor.offset)}</b></>}
          <span className="block pt-1 text-neutral-400">The preset&apos;s look is copied as it is. Each photo keeps its own exposure and white balance, nudged the same way you nudged the anchor.</span>
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
              {p.run.state === "cancelled" ? "Stopped. Nothing was changed." : p.run.state === "failed" ? `The run failed: ${p.run.message ?? "unknown error"}` : undone ? "Undone. The photos are back to how they were." : (p.run.message ?? "Done")}
            </p>
            {p.run.state === "finished" && (
              <div className="mt-2 flex flex-wrap items-center gap-2 text-xs">
                <span className="rounded bg-emerald-950 px-2 py-0.5 text-emerald-200" data-testid="baseline-count-applied" title="Photos edited with the baseline">
                  Applied <b>{p.run.counts.applied}</b>
                </span>
                <span className="rounded bg-amber-950 px-2 py-0.5 text-amber-200" data-testid="baseline-count-flagged" title="Edited, but worth a second look (dark on purpose, silhouettes, Auto failed)">
                  Needs a look <b>{p.run.counts.flagged}</b>
                </span>
                <span className="rounded bg-neutral-800 px-2 py-0.5 text-neutral-300" data-testid="baseline-count-skipped" title="Photos you had already edited; they were left alone">
                  Already edited, skipped <b>{p.run.counts.skippedEdited}</b>
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
                  title={undone ? "Already undone" : !batch ? "Nothing was written" : batch.undoable ? `Put all ${plural(batch.imageCount, "photo")} back to how they were before the baseline` : "Some photos were edited after the baseline, so it cannot be undone as one step"}
                  onClick={() => void undo()}
                >
                  <Undo2 className="size-3.5" /> {undone ? "Undone" : "Undo baseline"}
                </button>
              )}
              {p.run.counts.flagged > 0 && !undone && (
                <button type="button" className="flex h-7 items-center gap-1.5 rounded-md bg-amber-800 px-3 text-xs font-medium text-white hover:bg-amber-700" data-testid="baseline-show-flagged" title="List only the photos that need a look in the grid, each with its reason" onClick={p.onShowFlagged}>
                  <AlertTriangle className="size-3.5" /> Show the {p.run.counts.flagged} that need a look
                </button>
              )}
              <button type="button" className="h-7 rounded-md bg-neutral-800 px-3 text-xs hover:bg-neutral-700" data-testid="baseline-rerun" title="Change the scope or replace setting and run the baseline again (only photos still on the baseline are updated)" onClick={() => setRerun(true)}>
                Change settings and run again
              </button>
              {p.run.state === "finished" && !undone && (
                <button type="button" className="ml-auto flex h-7 items-center gap-1.5 rounded-md bg-emerald-700 px-3 text-xs font-medium text-white hover:bg-emerald-600" data-testid="baseline-to-finish" title="Last step: make sure the sidecars are saved and open them in Lightroom" onClick={p.onFinish}>
                  Finish in Lightroom <ArrowRight className="size-3.5" />
                </button>
              )}
            </div>
            {flagged.length > 0 && !undone && (
              <ul className="mt-3 space-y-1" data-testid="baseline-flagged-list">
                {flagged.slice(0, 6).map((f) => (
                  <li key={f.imageId} className="flex items-center gap-2 text-xs" data-testid={`baseline-flagged-${f.imageId}`}>
                    <Thumb entry={p.lib.getEntry(f.imageId)} version={p.lib.version(f.imageId)} className="h-8 w-12 shrink-0 rounded" />
                    <span className="w-32 shrink-0 truncate text-neutral-200">{p.lib.getEntry(f.imageId)?.fileName ?? `#${f.imageId}`}</span>
                    <span className="min-w-0 truncate text-amber-200">{f.reasons.map((r) => r.text).join("; ")}</span>
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
              <fieldset className="space-y-0.5" role="radiogroup" aria-label="Photos you already edited">
                <legend className="mb-1 text-xs font-semibold uppercase tracking-wide text-neutral-400">Photos you already edited</legend>
                {radio("replace", "skip", !replace, () => setReplace(false), "Skip them", "Leave photos that have your own edits alone (recommended)", "baseline-skip")}
                {radio("replace", "replace", replace, () => setReplace(true), "Replace their edit", "Overwrite your own edits with the baseline. You can undo this in one step", "baseline-replace")}
              </fieldset>
            </div>

            <div>
              <div className="mb-2 flex items-baseline gap-3">
                <h3 className="text-xs font-semibold uppercase tracking-wide text-neutral-400">Preview across your scenes</h3>
                <span className="text-xs text-neutral-400" data-testid="baseline-plan-counts">
                  {c ? (
                    <>
                      Will edit <b className="text-neutral-200">{c.toWrite}</b> of {plural(c.inScope, "photo")} in scope
                      {c.edited > 0 && <> · {replace ? `${c.edited} already edited will be replaced` : `${c.edited} already edited will be skipped`}</>}
                      {c.onBaseline > 0 && <> · {c.onBaseline} already on a baseline will be updated</>}
                    </>
                  ) : (
                    previewError ?? "Measuring…"
                  )}
                </span>
              </div>
              {previewError && !c && (
                <p className="text-xs text-amber-300" data-testid="baseline-preview-error">
                  {previewError}
                </p>
              )}
              <div className="grid grid-cols-[repeat(auto-fill,minmax(260px,1fr))] gap-3" data-testid="baseline-samples">
                {preview?.samples.map((s) => {
                  const u = urls[s.photo.imageId];
                  const entry = p.lib.getEntry(s.photo.imageId);
                  const flaggedS = s.photo.outcome === "flagged";
                  const skipped = s.photo.outcome === "skipped_edited";
                  return (
                    <figure key={s.photo.imageId} className="rounded-lg bg-neutral-900 p-1.5 ring-1 ring-neutral-800" data-testid={`baseline-sample-${s.photo.imageId}`} data-outcome={s.photo.outcome}>
                      <div className="grid grid-cols-2 gap-1">
                        {(["before", "after"] as const).map((k) => (
                          <div key={k} className="relative aspect-[3/2] overflow-hidden rounded bg-neutral-800">
                            {u?.[k] ? <img src={u[k]!} alt={`${k === "before" ? "Before" : "After"}: ${entry?.fileName ?? ""}`} draggable={false} className="size-full object-cover" data-testid={`baseline-sample-${s.photo.imageId}-${k}`} /> : <span className="flex size-full items-center justify-center text-[10px] text-neutral-500">{u ? "No preview" : "Rendering…"}</span>}
                            <span className="absolute left-1 top-1 rounded bg-black/70 px-1 text-[10px] text-neutral-200">{k === "before" ? "Before" : "After"}</span>
                          </div>
                        ))}
                      </div>
                      <figcaption className="flex items-center gap-1.5 px-1 pt-1 text-[11px] text-neutral-300">
                        <span data-testid={`baseline-sample-${s.photo.imageId}-scene`}>{p.sceneLabel(s.photo.sceneId)}</span>
                        <span className="min-w-0 truncate text-neutral-500">{entry?.fileName}</span>
                        {flaggedS && (
                          <span className="ml-auto shrink-0 rounded bg-amber-950 px-1 text-amber-300" title={s.photo.reasons.map((r) => r.text).join("; ")}>
                            needs a look
                          </span>
                        )}
                        {skipped && (
                          <span className="ml-auto shrink-0 rounded bg-neutral-800 px-1 text-neutral-300" title="You edited this photo yourself; it is left as it is unless you choose Replace">
                            kept
                          </span>
                        )}
                      </figcaption>
                    </figure>
                  );
                })}
              </div>
            </div>

            <div className="sticky bottom-0 flex items-center gap-3 border-t border-neutral-800 bg-neutral-950/95 py-3">
              <button
                type="button"
                className="flex h-9 items-center gap-2 rounded-md bg-emerald-700 px-5 text-sm font-semibold text-white hover:bg-emerald-600 disabled:opacity-40"
                data-testid="baseline-apply"
                disabled={starting || !c || c.toWrite === 0}
                title={!c ? "Waiting for the preview" : c.toWrite === 0 ? "There is nothing to edit with these settings" : `Edit ${plural(c.toWrite, "photo")} now. Everything can be undone in one step`}
                onClick={() => void apply()}
              >
                {c ? `Edit ${plural(c.toWrite, "photo")}` : "Edit the rest"} <ArrowRight className="size-4" />
              </button>
              <span className="text-xs text-neutral-400">{scope === "selection" ? `${plural(scopeCount, "selected photo")}` : `${plural(scopeCount, scope === "keepers" ? "keeper" : "photo")}`} in scope. Crop, straighten and masks are never touched.</span>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

export { type ScopeKind };
