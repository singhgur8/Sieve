// Guided workflow of one project (Cull, Edit, Export): the edit plan (scenes of keepers with one representative each),
// the personal style model, and the batch actions (auto edit, apply to scene, apply all, undo). Every call goes through the typed wrappers;
// "needs a look" frames and "auto edited" marks come from the last result of an action and live for the session.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { commands, DEFAULT_SCENE_APPLY_OPTIONS, events, unwrap, type ApplyScenesResult, type EditPlan, type SceneApplyOptions, type SceneEditEntry, type StyleModelStatus } from "../ipc";
import { describeError } from "../lib/errors";
import type { ToastApi } from "../components/Toasts";

/** What the checklist shows for a scene (the backend status plus "auto edited, not reviewed yet"). */
export type SceneUi = "todo" | "auto" | "edited" | "applied" | "stale";

export interface SceneRow {
  entry: SceneEditEntry;
  /** 1-based position in the plan (capture order). */
  number: number;
  ui: SceneUi;
  /** Keepers that receive the edit (keepers minus the representative). */
  targets: number;
  /** Frames of the last apply whose match did not converge. */
  review: number[];
  /** Photos changed by the last apply in this session. */
  applied: number | null;
}

export interface Busy {
  kind: "scene" | "all" | "auto";
  sceneId?: number;
  done: number;
  total: number;
}

export interface LastBatch {
  batchId: number;
  label: string;
  sceneIds: number[];
  at: number;
}

export interface StyleGate {
  /** `ready`: run it; `learn`: ask to train first; `training`; `insufficient` (too few edited photos); `failed`. */
  kind: "ready" | "learn" | "training" | "insufficient" | "failed";
  tip: string;
}

const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;

export function styleGate(s: StyleModelStatus | null): StyleGate {
  if (!s) return { kind: "insufficient", tip: "Checking your style model" };
  if (s.state === "ready") return { kind: "ready", tip: "Predict your edit for the representative of each scene" };
  if (s.state === "training") return { kind: "training", tip: "Learning your style" };
  if (s.availableExamples < s.minExamples)
    return {
      kind: "insufficient",
      tip: `Edit at least ${s.minExamples} photos (you have ${s.availableExamples}) so Sieve can learn your style. Photos edited in Lightroom count after "Read metadata from file".`,
    };
  if (s.state === "failed") return { kind: "failed", tip: s.error ?? "The last training failed" };
  return { kind: "learn", tip: "Sieve learns your style from the photos you edited first" };
}

interface Deps {
  projectId: number | null;
  toasts: ToastApi;
  onError: (e: unknown) => void;
  /** The scene list / library cache need a re-read (scenes were detected, edits were written). */
  onChanged: (ids: number[]) => Promise<void> | void;
  /** Scenes were detected: refresh the scene strip too. */
  onScenesDetected: () => Promise<void> | void;
  fileName: (id: number) => string;
}

export function useWorkflow(d: Deps) {
  const { projectId, toasts, onError } = d;
  const [plan, setPlan] = useState<EditPlan | null>(null);
  const [loading, setLoading] = useState(false);
  const [grouping, setGrouping] = useState<{ done: number; total: number } | null>(null);
  const [planError, setPlanError] = useState<string | null>(null);
  const [style, setStyle] = useState<StyleModelStatus | null>(null);
  const [busy, setBusy] = useState<Busy | null>(null);
  const [review, setReview] = useState<Map<number, number[]>>(new Map());
  const [appliedCount, setAppliedCount] = useState<Map<number, number>>(new Map());
  const [autoAt, setAutoAt] = useState<Map<number, number>>(new Map());
  const [learnFor, setLearnFor] = useState<number[] | null>(null);
  const [replaceFor, setReplaceFor] = useState<number[] | null>(null);
  const [lastBatch, setLastBatch] = useState<LastBatch | null>(null);
  const seq = useRef(0);
  const dref = useRef(d);
  dref.current = d;
  const pendingAuto = useRef<number[] | null>(null);
  const busyRef = useRef<Busy | null>(null);
  busyRef.current = busy;
  const planRef = useRef(plan);
  planRef.current = plan;
  const labelOf = (id: number) => {
    const i = planRef.current?.scenes.findIndex((s) => s.sceneId === id) ?? -1;
    return i >= 0 ? `Scene ${i + 1}` : "the scene";
  };

  const fetchPlan = useCallback(async () => {
    if (projectId == null) return null;
    const my = ++seq.current;
    const p = await unwrap(commands.getEditPlan(projectId));
    if (my === seq.current) setPlan(p);
    return p;
  }, [projectId]);

  /** Re-reads the plan; with `group`, keepers outside every scene are grouped first (detect_scenes over the project). */
  const loadPlan = useCallback(
    async (group = false) => {
      if (projectId == null) return;
      setLoading(true);
      try {
        let p = await fetchPlan();
        if (group && p && p.keeperIds.length > 0 && p.unassignedKeeperIds.length > 0) {
          setGrouping({ done: 0, total: p.unassignedKeeperIds.length });
          try {
            await unwrap(commands.detectScenes(null, projectId, null));
          } finally {
            setGrouping(null);
          }
          await dref.current.onScenesDetected();
          p = await fetchPlan();
        }
        setPlanError(null);
      } catch (e) {
        setPlanError(describeError(e).message);
      } finally {
        setLoading(false);
      }
    },
    [projectId, fetchPlan],
  );

  /** "Rebuild plan": detect the project's scenes again (manual scenes and edits are kept), then re-read the plan. */
  const regroup = useCallback(async () => {
    if (projectId == null) return;
    setLoading(true);
    setGrouping({ done: 0, total: planRef.current?.keeperIds.length ?? 0 });
    try {
      await unwrap(commands.detectScenes(null, projectId, null));
      await dref.current.onScenesDetected();
      await fetchPlan();
      setPlanError(null);
    } catch (e) {
      setPlanError(describeError(e).message);
    } finally {
      setGrouping(null);
      setLoading(false);
    }
  }, [projectId, fetchPlan]);

  // Silent re-read (after edits / culling): keeps the current plan on screen and swallows errors.
  const refreshPlan = useCallback(() => void fetchPlan().catch(() => {}), [fetchPlan]);

  useEffect(() => {
    setPlan(null);
    setReview(new Map());
    setAppliedCount(new Map());
    setAutoAt(new Map());
    setLastBatch(null);
  }, [projectId]);

  // ---- style model ----
  useEffect(() => {
    if (projectId == null) return;
    unwrap(commands.styleModelStatus())
      .then(setStyle)
      .catch(() => {});
  }, [projectId]);

  const requestAutoEditRef = useRef<(ids: number[], st: StyleModelStatus) => void>(() => {});
  useEffect(() => {
    const un = [
      events.styleModelProgress.listen((ev) => {
        const p = ev.payload;
        setStyle((s) => (s ? { ...s, state: "training", progress: p.total > 0 ? p.done / p.total : 0 } : s));
      }),
      events.styleModelFinished.listen((ev) => {
        const f = ev.payload;
        setStyle(f.status);
        const pending = pendingAuto.current;
        pendingAuto.current = null;
        if (f.ok && pending) return requestAutoEditRef.current(pending, f.status);
        if (f.ok) return void toasts.push("Your style is learned. Auto edit (my style) is ready.");
        if (f.cancelled) return;
        toasts.push(f.error ?? "Learning your style failed", {
          kind: "error",
          action: { label: "Retry", testid: "style-retry", onClick: () => void train(pending ?? undefined) },
        });
      }),
      events.sceneProgress.listen((ev) => {
        const p = ev.payload;
        if (p.task === "detect") setGrouping((g) => (g ? { done: p.done, total: p.total } : g));
        if (p.task === "apply") setBusy((b) => (b && b.kind !== "auto" ? { ...b, done: p.done, total: p.total } : b));
      }),
    ];
    return () => un.forEach((u) => void u.then((f) => f()));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const train = useCallback(
    async (thenAuto?: number[]) => {
      pendingAuto.current = thenAuto ?? null;
      setStyle((s) => (s ? { ...s, state: "training", progress: 0, error: null } : s));
      try {
        await unwrap(commands.trainStyleModel());
      } catch (e) {
        pendingAuto.current = null;
        unwrap(commands.styleModelStatus())
          .then(setStyle)
          .catch(() => {});
        onError(e);
      }
    },
    [onError],
  );

  // ---- derived rows ----
  const rows = useMemo<SceneRow[]>(() => {
    if (!plan) return [];
    return plan.scenes.map((entry, i) => {
      const at = autoAt.get(entry.sceneId);
      const ui: SceneUi =
        entry.status === "to_edit" ? "todo" : entry.status === "applied" ? "applied" : entry.status === "outdated" ? "stale" : at != null && (entry.editedAtMs ?? 0) <= at ? "auto" : "edited";
      return { entry, number: i + 1, ui, targets: Math.max(0, entry.imageIds.length - 1), review: review.get(entry.sceneId) ?? [], applied: appliedCount.get(entry.sceneId) ?? null };
    });
  }, [plan, review, appliedCount, autoAt]);

  const afterChange = useCallback(
    async (ids: number[]) => {
      await dref.current.onChanged(ids);
      await fetchPlan().catch(() => {});
    },
    [fetchPlan],
  );

  // ---- undo ----
  const undoBatch = useCallback(
    async (batch: LastBatch) => {
      try {
        const r = await unwrap(commands.undoEditBatch(batch.batchId));
        setLastBatch((b) => (b?.batchId === batch.batchId ? null : b));
        setReview((m) => {
          const n = new Map(m);
          batch.sceneIds.forEach((s) => n.delete(s));
          return n;
        });
        setAppliedCount((m) => {
          const n = new Map(m);
          batch.sceneIds.forEach((s) => n.delete(s));
          return n;
        });
        await afterChange([...r.restoredIds, ...r.skippedIds]);
        toasts.push(`Undid ${batch.label} on ${plural(r.restoredIds.length, "photo")}${r.skippedIds.length > 0 ? ` · ${r.skippedIds.length} changed since, kept` : ""}`);
      } catch (e) {
        onError(e);
      }
    },
    [afterChange, toasts, onError],
  );

  const undoLast = useCallback(() => {
    if (lastBatch) void undoBatch(lastBatch);
  }, [lastBatch, undoBatch]);

  const remember = (batchId: number | null, label: string, sceneIds: number[]): LastBatch | null => {
    if (batchId == null) return null;
    const b = { batchId, label, sceneIds, at: Date.now() };
    setLastBatch(b);
    return b;
  };

  // ---- apply ----
  const noteOutcomes = (r: ApplyScenesResult) => {
    setReview((m) => {
      const n = new Map(m);
      r.scenes.forEach((s) => (s.notConvergedIds.length > 0 ? n.set(s.sceneId, s.notConvergedIds) : n.delete(s.sceneId)));
      return n;
    });
    setAppliedCount((m) => {
      const n = new Map(m);
      r.scenes.forEach((s) => n.set(s.sceneId, s.changedIds.length + s.skippedIds.length));
      return n;
    });
  };

  const applyScene = useCallback(
    async (sceneId: number, opts: "match" | "exact" = "match", onReview?: (sceneId: number) => void) => {
      if (busyRef.current) return;
      setBusy({ kind: "scene", sceneId, done: 0, total: 0 });
      try {
        const options: SceneApplyOptions | null =
          opts === "exact"
            ? ({ ...(DEFAULT_SCENE_APPLY_OPTIONS as unknown as SceneApplyOptions), matchOptions: { ...(DEFAULT_SCENE_APPLY_OPTIONS.matchOptions as unknown as SceneApplyOptions["matchOptions"]), matchExposure: false, matchWhiteBalance: false } } as SceneApplyOptions)
            : null;
        const r = await unwrap(commands.applySceneEdit(sceneId, options));
        noteOutcomes(r);
        const out = r.scenes[0];
        const n = out ? out.changedIds.length : 0;
        const need = out ? out.notConvergedIds.length : 0;
        const b = remember(r.batch.batchId, `Apply ${labelOf(sceneId)}`, [sceneId]);
        await afterChange(r.batch.changedIds);
        const label = labelOf(sceneId);
        toasts.push(`Applied ${label} to ${plural(n, "photo")}${need > 0 ? ` · ${need} need a look` : ""}`, {
          action: b ? { label: "Undo", testid: "apply-undo-batch", onClick: () => void undoBatch(b) } : undefined,
          secondary: need > 0 && onReview ? { label: "Review", testid: "apply-review", onClick: () => onReview(sceneId) } : undefined,
        });
      } catch (e) {
        onError(e);
      } finally {
        setBusy(null);
      }
    },
    [afterChange, toasts, onError, undoBatch],
  );

  const applyAll = useCallback(
    async (onReview?: (sceneId: number) => void) => {
      if (projectId == null || busyRef.current) return;
      setBusy({ kind: "all", done: 0, total: 0 });
      try {
        const r = await unwrap(commands.applyAllEditedScenes(projectId, null));
        if (r.scenes.length === 0) {
          toasts.push("No edited scenes to apply");
          return;
        }
        noteOutcomes(r);
        const n = r.scenes.reduce((a, s) => a + s.changedIds.length, 0);
        const need = r.scenes.filter((s) => s.notConvergedIds.length > 0);
        const needN = need.reduce((a, s) => a + s.notConvergedIds.length, 0);
        const b = remember(r.batch.batchId, `Apply ${plural(r.scenes.length, "scene")}`, r.scenes.map((s) => s.sceneId));
        await afterChange(r.batch.changedIds);
        toasts.push(`Applied ${plural(r.scenes.length, "scene")} to ${plural(n, "photo")}${needN > 0 ? ` · ${needN} need a look` : ""}`, {
          action: b ? { label: "Undo", testid: "apply-undo-batch", onClick: () => void undoBatch(b) } : undefined,
          secondary: needN > 0 && onReview ? { label: "Review", testid: "apply-review", onClick: () => onReview(need[0].sceneId) } : undefined,
        });
      } catch (e) {
        onError(e);
      } finally {
        setBusy(null);
      }
    },
    [projectId, afterChange, toasts, onError, undoBatch],
  );

  // ---- auto edit (my style) ----
  const runAuto = useCallback(
    async (sceneIds: number[]) => {
      const p = planRef.current;
      if (!p || busyRef.current) return;
      const reps = sceneIds.map((id) => p.scenes.find((s) => s.sceneId === id)?.representativeId).filter((x): x is number => x != null);
      if (reps.length === 0) return;
      setBusy({ kind: "auto", done: 0, total: reps.length });
      try {
        const r = await unwrap(commands.applyStylePrediction(reps));
        const b = remember(r.batchId, "Auto Edit (My Style)", []);
        await afterChange(reps);
        const fresh = planRef.current ?? p;
        setAutoAt((m) => {
          const n = new Map(m);
          sceneIds.forEach((id) => n.set(id, fresh.scenes.find((s) => s.sceneId === id)?.editedAtMs ?? Date.now()));
          return n;
        });
        toasts.push(`Auto edited ${plural(sceneIds.length, "scene")}. Review ${sceneIds.length === 1 ? "it" : "each one"}, then apply.`, {
          action: b ? { label: "Undo", testid: "auto-undo", onClick: () => void undoBatch(b) } : undefined,
        });
      } catch (e) {
        onError(e);
      } finally {
        setBusy(null);
      }
    },
    [afterChange, toasts, onError, undoBatch],
  );

  /** Entry point of every "Auto edit (my style)" button: gates on the model state, asks before replacing a hand edit. */
  const requestAutoEdit = useCallback(
    (sceneIds: number[], confirmed = false, st: StyleModelStatus | null = style) => {
      const p = planRef.current;
      if (!p || sceneIds.length === 0) return;
      const g = styleGate(st);
      if (g.kind === "insufficient" || g.kind === "training") return void toasts.push(g.tip);
      if (g.kind === "learn" || g.kind === "failed") return setLearnFor(sceneIds);
      const edited = sceneIds.filter((id) => {
        const s = p.scenes.find((x) => x.sceneId === id);
        return s?.edited && !autoAt.has(id);
      });
      if (edited.length > 0 && !confirmed) return setReplaceFor(sceneIds);
      void runAuto(sceneIds);
    },
    [style, autoAt, runAuto, toasts],
  );
  requestAutoEditRef.current = (ids, st) => requestAutoEdit(ids, false, st);

  const setRepresentative = useCallback(
    async (sceneId: number, imageId: number) => {
      const before = planRef.current?.scenes.find((s) => s.sceneId === sceneId)?.representativeId ?? null;
      if (before === imageId) return toasts.push("Already the representative");
      try {
        await unwrap(commands.setSceneRepresentative(sceneId, imageId));
        await fetchPlan();
        toasts.push(`${labelOf(sceneId)} representative: ${dref.current.fileName(imageId)}`, {
          action: {
            label: "Undo",
            testid: "rep-undo",
            onClick: () =>
              void unwrap(commands.setSceneRepresentative(sceneId, before))
                .then(() => fetchPlan())
                .catch(onError),
          },
        });
      } catch (e) {
        onError(e);
      }
    },
    [fetchPlan, toasts, onError],
  );

  const setKeeperRule = useCallback(
    async (rule: { minRating: number; useSuggestions: boolean }) => {
      try {
        await unwrap(commands.setKeeperRule(rule));
        await dref.current.onChanged([]);
        await loadPlan(true);
      } catch (e) {
        onError(e);
      }
    },
    [loadPlan, onError],
  );

  return {
    plan,
    rows,
    loading,
    grouping,
    planError,
    style,
    busy,
    lastBatch,
    learnFor,
    setLearnFor,
    replaceFor,
    setReplaceFor,
    loadPlan,
    regroup,
    refreshPlan,
    train,
    requestAutoEdit,
    runAuto,
    applyScene,
    applyAll,
    undoBatch,
    undoLast,
    setRepresentative,
    setKeeperRule,
  };
}

export type Workflow = ReturnType<typeof useWorkflow>;
