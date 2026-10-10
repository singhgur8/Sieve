// Guided workflow of one project (Cull, Edit, Export): the edit plan (scenes of keepers with one representative each),
// the personal style model, and the batch actions (auto edit, apply to scene, apply all, undo). Every call goes through the typed wrappers;
// Markers ("needs a look", "auto edited", "applied", skipped) come from the persisted plan (IPC v15), never from session state.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { commands, DEFAULT_SCENE_APPLY_OPTIONS, events, isEditPlanDone, unwrap, type EditBatchInfo, type EditPlan, type ImageEditState, type KeeperRule, type SceneApplyOptions, type SceneEditEntry, type StyleModelStatus } from "../ipc";
import { describeError } from "../lib/errors";
import type { ToastApi } from "../components/Toasts";
import { flushEdits } from "../lib/editFlush";

/** What the checklist shows for a scene (the backend status plus "auto edited, not reviewed yet"). */
export type SceneUi = "todo" | "auto" | "edited" | "applied" | "stale" | "reset" | "baseline";

export interface SceneRow {
  entry: SceneEditEntry;
  /** 1-based position in the plan (capture order). */
  number: number;
  ui: SceneUi;
  /** Keepers that receive the edit (keepers minus the representative). */
  targets: number;
  /** Keepers that need a look (persisted). */
  review: number[];
  /** Members whose current settings came from an apply (persisted). */
  applied: number;
  /** Keepers the last apply did not cover (`Apply to N new`). */
  unapplied: number[];
  skipped: boolean;
  minor: boolean;
}

export type PlanTab = "all" | "todo" | "edited" | "applied" | "baseline" | "skipped";
export const rowInTab = (r: SceneRow, t: PlanTab) =>
  t === "skipped" ? r.skipped : t === "all" ? true : r.skipped ? false : t === "todo" ? r.ui === "todo" || r.ui === "reset" : t === "applied" ? r.ui === "applied" : t === "baseline" ? r.ui === "baseline" : r.ui === "edited" || r.ui === "auto" || r.ui === "stale";

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
  /** Photos the batch wrote (a later edit to any of them makes its Undo unsafe). */
  imageIds: number[];
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

/** What an apply does per frame vs. copies, in words (the toast after Apply to scene). */
export function describeApply(o: SceneApplyOptions | null): string {
  const m = (o ?? (DEFAULT_SCENE_APPLY_OPTIONS as unknown as SceneApplyOptions)).matchOptions;
  const matched = [m.matchExposure && "exposure", m.matchWhiteBalance && "white balance", m.matchTone && "tone"].filter(Boolean) as string[];
  const copied = m.copyFields.length;
  const list = matched.length > 1 ? `${matched.slice(0, -1).join(", ")} and ${matched[matched.length - 1]}` : (matched[0] ?? "");
  const head = matched.length > 0 ? `${list[0].toUpperCase()}${list.slice(1)} matched per photo` : "Nothing matched per photo";
  return copied > 0 ? `${head}; everything else copied (grain, clarity, HSL, curves…)` : head;
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
  const [tab, setTab] = useState<PlanTab>("all");
  const [minorOpen, setMinorOpen] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [learnFor, setLearnFor] = useState<number[] | null>(null);
  const [replaceFor, setReplaceFor] = useState<number[] | null>(null);
  const [batchStack, setBatchStack] = useState<LastBatch[]>([]);
  /** Batch id -> toast offering its Undo (dismissed once the batch is undone). */
  const batchToast = useRef(new Map<number, number>());
  const bindToast = (b: LastBatch | null, toastId: number) => {
    if (b) batchToast.current.set(b.batchId, toastId);
  };
  /** Backend truth (`get_edit_batches`) for the session's batches: whether `undo_edit_batch` would succeed now. */
  const [infos, setInfos] = useState<Map<number, EditBatchInfo>>(new Map());
  /** Batches a later user edit touched: an immediate hint until the next backend refresh says otherwise. */
  const [touched, setTouched] = useState<Set<number>>(new Set());
  const touchedRef = useRef(new Set<number>());
  const batchStackRef = useRef<LastBatch[]>([]);
  batchStackRef.current = batchStack;
  const reasonOf = (i: EditBatchInfo): string | null =>
    i.undoneAtMs != null ? "Already undone" : i.conflictCount > 0 ? `Later edits on ${plural(i.conflictCount, "photo")}. Undo those first` : null;
  /** Why a session batch cannot be undone right now; null = it can. */
  const undoReason = (b: Pick<LastBatch, "batchId">): string | null => {
    const i = infos.get(b.batchId);
    if (i) return reasonOf(i);
    return touched.has(b.batchId) ? "Later edits touched these photos. Undo those first" : null;
  };
  /** Row menus: the scene's persisted last apply (works after Home and back). */
  const sceneUndo = (sceneId: number): { batch: Pick<LastBatch, "batchId" | "label"> | null; enabled: boolean; reason: string | undefined } => {
    const ab = planRef.current?.scenes.find((x) => x.sceneId === sceneId)?.appliedBatch ?? null;
    if (!ab) return { batch: null, enabled: false, reason: "Nothing to undo" };
    const i = infos.get(ab.batchId) ?? ab;
    const hint = touched.has(ab.batchId) && i.undoable;
    return { batch: { batchId: ab.batchId, label: batchLabel(ab.batchId, ab.label) }, enabled: i.undoable && !hint, reason: reasonOf(i) ?? (hint ? "Later edits touched these photos. Undo those first" : undefined) };
  };
  /** Scene-aware label for a persisted batch ("Apply Scene 2"), so it reads the same after Home and back. */
  const batchLabel = (batchId: number, fallback: string): string => {
    const ss = plan?.scenes ?? [];
    const hit = ss.map((x, i) => ({ x, i })).filter(({ x }) => x.appliedBatch?.batchId === batchId);
    if (hit.length === 1) return `Apply Scene ${hit[0].i + 1}`;
    if (hit.length > 1) return `Apply ${plural(hit.length, "scene")}`;
    return fallback;
  };
  const latest = plan?.latestBatch ?? null;
  const stackTop = batchStack.length > 0 ? batchStack[batchStack.length - 1] : null;
  /** What Cmd+Z undoes: the session's newest batch, else the project's newest batch from the plan. */
  const lastBatch: LastBatch | null = stackTop
    ? (infos.get(stackTop.batchId)?.undoable ?? !touched.has(stackTop.batchId))
      ? stackTop
      : null
    : latest && latest.undoable && !touched.has(latest.batchId)
      ? { batchId: latest.batchId, label: batchLabel(latest.batchId, latest.label), sceneIds: [], imageIds: [], at: latest.createdAtMs }
      : null;
  /** Newest batch (session, else the plan's) that exists but cannot be undone: Cmd+Z explains why instead of "Nothing to undo". */
  const blockedUndo = (): { at: number; reason: string } | null => {
    const top = stackTop ? { batchId: stackTop.batchId, at: stackTop.at } : latest ? { batchId: latest.batchId, at: latest.createdAtMs } : null;
    if (!top || lastBatch) return null;
    const i = infos.get(top.batchId) ?? (latest && latest.batchId === top.batchId ? latest : null);
    if (i && i.undoneAtMs != null) return null;
    const n = i?.conflictCount ?? 0;
    const reason = n > 0 ? `Later edits on ${plural(n, "photo")}. Undo those in Develop first` : "Later edits touched these photos. Undo those in Develop first";
    return { at: top.at, reason };
  };
  /** The user committed an adjustment (or batch-edited photos) in Develop: batches covering them lose their Undo (hint; the backend confirms on the next refresh). */
  const noteCommit = useCallback((ids: number[]) => {
    const set = new Set(ids);
    const hit = batchStackRef.current.filter((b) => b.imageIds.some((i) => set.has(i)) && !touchedRef.current.has(b.batchId));
    if (hit.length > 0) {
      hit.forEach((b) => {
        touchedRef.current.add(b.batchId);
        const tid = batchToast.current.get(b.batchId);
        if (tid != null) dref.current.toasts.retract(tid);
      });
      setTouched(new Set(touchedRef.current));
    }
    clearTimeout(commitTimer.current);
    commitTimer.current = setTimeout(() => refreshRef.current(), 300);
  }, []);
  const commitTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const refreshRef = useRef<() => void>(() => {});
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

  /** Refreshes the undoability of the batches the UI holds (session stack + toasts) from the backend. */
  const syncBatches = async (p: EditPlan) => {
    const ids = new Set<number>([...batchStackRef.current.map((b) => b.batchId), ...batchToast.current.keys()]);
    if (ids.size === 0) return setInfos(new Map());
    try {
      const list = await unwrap(commands.getEditBatches([...ids]));
      const m = new Map(list.map((i) => [i.batchId, i]));
      p.scenes.forEach((x) => x.appliedBatch && !m.has(x.appliedBatch.batchId) && m.set(x.appliedBatch.batchId, x.appliedBatch));
      touchedRef.current = new Set([...touchedRef.current].filter((id) => !m.has(id)));
      setTouched(new Set(touchedRef.current));
      setInfos(m);
      list.forEach((i) => {
        const tid = batchToast.current.get(i.batchId);
        if (tid == null) return;
        if (i.undoneAtMs != null) {
          dref.current.toasts.dismiss(tid);
          batchToast.current.delete(i.batchId);
        } else if (!i.undoable) dref.current.toasts.retract(tid);
      });
      setBatchStack((st) => st.filter((b) => m.get(b.batchId)?.undoneAtMs == null));
    } catch {
      // Keep the client-side hints when the refresh fails.
    }
  };

  const fetchPlan = useCallback(async () => {
    if (projectId == null) return null;
    const my = ++seq.current;
    const p = await unwrap(commands.getEditPlan(projectId));
    if (my === seq.current) {
      setPlan(p);
      void syncBatches(p);
    }
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
  refreshRef.current = refreshPlan;

  useEffect(() => {
    setPlan(null);
    setTab("all");
    setMinorOpen(false);
    setBatchStack([]);
    setTouched(new Set());
    touchedRef.current = new Set();
    batchToast.current.clear();
    setInfos(new Map());
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
      const ui: SceneUi = entry.status === "to_edit" ? "todo" : entry.status === "reset" ? "reset" : entry.status === "applied" ? "applied" : entry.status === "outdated" ? "stale" : entry.status === "on_baseline" ? "baseline" : entry.autoEdited ? "auto" : "edited";
      return {
        entry,
        number: i + 1,
        ui,
        targets: Math.max(0, entry.imageIds.length - 1),
        review: entry.needsReviewIds,
        applied: entry.appliedIds.length,
        unapplied: entry.unappliedKeeperIds,
        skipped: entry.skipped,
        minor: entry.minor,
      };
    });
  }, [plan]);

  /** Rows of the active tab: regular scenes in plan order, then the folded "small scenes" (not in the Skipped tab). */
  const layout = useMemo(() => {
    const inTab = rows.filter((r) => rowInTab(r, tab));
    const fold = tab !== "skipped";
    const main = fold ? inTab.filter((r) => !r.minor) : inTab;
    const minor = fold ? inTab.filter((r) => r.minor) : [];
    return { main, minor, visible: [...main, ...(minorOpen ? minor : [])] };
  }, [rows, tab, minorOpen]);
  const done = useMemo(() => (plan ? isEditPlanDone(plan) : false), [plan]);
  const stateById = useMemo(() => new Map<number, ImageEditState>((plan?.editStates ?? []).map((x) => [x.imageId, x])), [plan]);
  const needsReviewSet = useMemo(() => new Set(plan?.needsReviewIds ?? []), [plan]);

  const afterChange = useCallback(
    async (ids: number[]) => {
      await dref.current.onChanged(ids);
      await fetchPlan().catch(() => {});
    },
    [fetchPlan],
  );

  // ---- undo ----
  const undoBatch = useCallback(
    async (batch: Pick<LastBatch, "batchId" | "label">, opts?: { keepLaterEdits?: boolean }) => {
      try {
        const r = await unwrap(commands.undoEditBatch(batch.batchId, opts?.keepLaterEdits ? { keepLaterEdits: true } : null));
        setBatchStack((st) => st.filter((b) => b.batchId !== batch.batchId));
        const tid = batchToast.current.get(batch.batchId);
        if (tid != null) {
          toasts.dismiss(tid);
          batchToast.current.delete(batch.batchId);
        }
        await afterChange([...r.restoredIds, ...r.skippedIds]);
        toasts.push(`Undid ${batch.label} on ${plural(r.restoredIds.length, "photo")}${r.skippedIds.length > 0 ? ` · ${r.skippedIds.length} changed since, kept` : ""}`);
      } catch (e) {
        // v16 `conflict`: later edits touched these photos. Say so (info, nothing changed) and re-read the plan.
        if ((e as { kind?: unknown } | null)?.kind === "conflict") {
          const tid = batchToast.current.get(batch.batchId);
          if (tid != null) toasts.retract(tid);
          void fetchPlan().catch(() => {});
          return void toasts.push(describeError(e).message);
        }
        onError(e);
      }
    },
    [afterChange, fetchPlan, toasts, onError],
  );

  const undoLast = useCallback(() => {
    if (lastBatch) void undoBatch(lastBatch);
  }, [lastBatch, undoBatch]);

  const remember = (batchId: number | null, label: string, sceneIds: number[], imageIds: number[] = []): LastBatch | null => {
    if (batchId == null) return null;
    // Older toasts keep their Undo until the backend says it would conflict (`get_edit_batches` after the plan refetch).
    const b = { batchId, label, sceneIds, imageIds, at: Date.now() };
    setBatchStack((st) => [...st, b].slice(-20));
    return b;
  };

  /**
   * A paste / sync batch (`EditBatchResult`): refresh what changed and offer one Undo that reverts the whole batch.
   * Returns false when nothing changed.
   */
  const reportBatch = useCallback(
    async (r: { batchId: number | null; label: string; changedIds: number[] }, text: string, attempted: number, soft = false): Promise<boolean> => {
      if (r.batchId == null || r.changedIds.length === 0) {
        toasts.push(`${text}: no change (${plural(attempted, "photo")} already matched)`);
        return false;
      }
      const b = remember(r.batchId, r.label, [], r.changedIds);
      // `soft`: the caller (Develop) already refreshed its own photos; a full `onChanged` would remount the Develop view.
      if (soft) await fetchPlan().catch(() => {});
      else await afterChange(r.changedIds);
      const tid = toasts.push(`${text} to ${plural(r.changedIds.length, "photo")}${attempted > r.changedIds.length ? ` (${attempted - r.changedIds.length} already matched)` : ""}`, {
        action: b ? { label: "Undo", testid: "paste-undo-batch", onClick: () => void undoBatch(b) } : undefined,
      });
      bindToast(b, tid);
      return true;
    },
    [afterChange, fetchPlan, toasts, undoBatch],
  );

  // ---- apply ----
  const applyScene = useCallback(
    async (
      sceneId: number,
      opts: "match" | "exact" | SceneApplyOptions = "match",
      onReview?: (sceneId: number, ids?: number[]) => void,
      onShowIds?: (ids: number[], label: string) => void,
    ) => {
      if (busyRef.current) return;
      setBusy({ kind: "scene", sceneId, done: 0, total: 0 });
      setCancelling(false);
      try {
        // The representative is read from the catalog: store the slider edit still in flight first.
        await flushEdits();
        const options: SceneApplyOptions | null =
          opts === "exact"
            ? ({ ...(DEFAULT_SCENE_APPLY_OPTIONS as unknown as SceneApplyOptions), matchOptions: { ...(DEFAULT_SCENE_APPLY_OPTIONS.matchOptions as unknown as SceneApplyOptions["matchOptions"]), matchExposure: false, matchWhiteBalance: false } } as SceneApplyOptions)
            : opts === "match"
              ? null
              : opts;
        const r = await unwrap(commands.applySceneEdit(sceneId, options));
        const out = r.scenes[0];
        const n = out ? out.changedIds.length : 0;
        const need = out ? out.notConvergedIds.length : 0;
        const label = labelOf(sceneId);
        const b = remember(r.batch.batchId, `Apply ${label}`, [sceneId], r.batch.changedIds);
        await afterChange(r.batch.changedIds);
        if (r.cancelled) {
          const tid = toasts.push(r.scenes.length === 0 ? `Stopped. Nothing was applied to ${label}` : `Stopped after ${plural(r.scenes.length, "scene")}`, {
            action: b ? { label: "Undo", testid: "apply-undo-batch", onClick: () => void undoBatch(b) } : undefined,
          });
          bindToast(b, tid);
          return;
        }
        const skippedIds = out?.skippedIds ?? [];
        const sk = skippedIds.length;
        const tid = toasts.push(
          `Applied ${label} to ${plural(n, "photo")}. ${describeApply(options)}.${sk > 0 ? ` ${sk} skipped because you edited them.` : ""}${need > 0 ? ` ${need} ${need === 1 ? "needs" : "need"} a look.` : ""}`,
          {
            action: b ? { label: "Undo", testid: "apply-undo-batch", onClick: () => void undoBatch(b) } : undefined,
            secondary:
              sk > 0 && onShowIds
                ? { label: "Show", testid: "apply-skipped-show-photos", onClick: () => onShowIds(skippedIds, `${plural(sk, "photo")} you edited (skipped by Apply)`) }
                : need > 0 && onReview
                  ? { label: "Review", testid: "apply-review", onClick: () => onReview(sceneId, out?.notConvergedIds) }
                  : undefined,
            third: sk > 0 && need > 0 && onReview ? { label: "Review", testid: "apply-review", onClick: () => onReview(sceneId, out?.notConvergedIds) } : undefined,
          },
        );
        bindToast(b, tid);
      } catch (e) {
        onError(e);
      } finally {
        setBusy(null);
        setCancelling(false);
      }
    },
    [afterChange, toasts, onError, undoBatch],
  );

  const applyAll = useCallback(
    async (onReview?: (sceneId: number, ids?: number[]) => void, onShow?: (sceneId: number) => void, onShowIds?: (ids: number[], label: string) => void) => {
      if (projectId == null || busyRef.current) return;
      setBusy({ kind: "all", done: 0, total: 0 });
      setCancelling(false);
      try {
        await flushEdits();
        const r = await unwrap(commands.applyAllEditedScenes(projectId, null));
        const skipped = r.skippedScenes ?? [];
        const noteSkipped = () => {
          if (skipped.length === 0) return;
          const first = skipped[0];
          const known = planRef.current?.scenes.some((x) => x.sceneId === first.sceneId);
          toasts.push(`Not applied: ${skipped.map((x) => x.message).join(" ")}`, {
            action: known && onShow ? { label: "Show", testid: "apply-skipped-show", onClick: () => onShow(first.sceneId) } : undefined,
          });
        };
        if (r.scenes.length === 0) {
          toasts.push(r.cancelled ? "Stopped. Nothing was applied" : "No edited scenes to apply");
          noteSkipped();
          return;
        }
        const n = r.scenes.reduce((a, s) => a + s.changedIds.length, 0);
        const need = r.scenes.filter((s) => s.notConvergedIds.length > 0);
        const needN = need.reduce((a, s) => a + s.notConvergedIds.length, 0);
        const b = remember(r.batch.batchId, `Apply ${plural(r.scenes.length, "scene")}`, r.scenes.map((s) => s.sceneId), r.batch.changedIds);
        await afterChange(r.batch.changedIds);
        const head = r.cancelled ? `Stopped after ${plural(r.scenes.length, "scene")} (${plural(n, "photo")})` : `Applied ${plural(r.scenes.length, "scene")} to ${plural(n, "photo")}`;
        const skippedAll = r.scenes.flatMap((s) => s.skippedIds);
        const tid = toasts.push(
          `${head}. ${describeApply(null)}.${skippedAll.length > 0 ? ` ${skippedAll.length} skipped because you edited them.` : ""}${needN > 0 ? ` ${needN} ${needN === 1 ? "needs" : "need"} a look.` : ""}`,
          {
            action: b ? { label: "Undo", testid: "apply-undo-batch", onClick: () => void undoBatch(b) } : undefined,
            secondary:
              skippedAll.length > 0 && onShowIds
                ? { label: "Show", testid: "apply-skipped-show-photos", onClick: () => onShowIds(skippedAll, `${plural(skippedAll.length, "photo")} you edited (skipped by Apply)`) }
                : needN > 0 && onReview
                  ? { label: "Review", testid: "apply-review", onClick: () => onReview(need[0].sceneId, need[0].notConvergedIds) }
                  : undefined,
            third: skippedAll.length > 0 && needN > 0 && onReview ? { label: "Review", testid: "apply-review", onClick: () => onReview(need[0].sceneId, need[0].notConvergedIds) } : undefined,
          },
        );
        bindToast(b, tid);
        noteSkipped();
      } catch (e) {
        onError(e);
      } finally {
        setBusy(null);
        setCancelling(false);
      }
    },
    [projectId, afterChange, toasts, onError, undoBatch],
  );

  /** × on the Applying pill / Esc in the Plan: stops between steps; scenes already matched stay applied. */
  const cancelApply = useCallback(() => {
    const b = busyRef.current;
    if (!b || b.kind === "auto") return;
    setCancelling(true);
    unwrap(commands.cancelSceneApply()).catch(onError);
  }, [onError]);

  const setSkipped = useCallback(
    async (sceneId: number, skipped: boolean) => {
      try {
        await unwrap(commands.setSceneSkipped(sceneId, skipped));
        await fetchPlan();
        toasts.push(`${skipped ? "Skipped" : "Included"} ${labelOf(sceneId)}`, {
          action: { label: "Undo", testid: "skip-undo", onClick: () => void unwrap(commands.setSceneSkipped(sceneId, !skipped)).then(() => fetchPlan()).catch(onError) },
        });
      } catch (e) {
        onError(e);
      }
    },
    [fetchPlan, toasts, onError],
  );

  /** "Looks good": clears needs-a-look on the frames without touching their settings. */
  const markReviewed = useCallback(
    async (ids: number[]) => {
      try {
        await unwrap(commands.markReviewed(ids));
        await fetchPlan();
      } catch (e) {
        onError(e);
      }
    },
    [fetchPlan, onError],
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
        const b = remember(r.batchId, "Auto Edit (My Style)", [], reps);
        await afterChange(reps);
        const tid = toasts.push(`Auto edited ${plural(sceneIds.length, "scene")}. Review ${sceneIds.length === 1 ? "it" : "each one"}, then apply.`, {
          action: b ? { label: "Undo", testid: "auto-undo", onClick: () => void undoBatch(b) } : undefined,
        });
        bindToast(b, tid);
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
        return s?.edited && !s.autoEdited;
      });
      if (edited.length > 0 && !confirmed) return setReplaceFor(sceneIds);
      void runAuto(sceneIds);
    },
    [style, runAuto, toasts],
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
    async (rule: KeeperRule) => {
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
    layout,
    done,
    stateById,
    needsReviewSet,
    tab,
    setTab,
    minorOpen,
    setMinorOpen,
    cancelling,
    cancelApply,
    setSkipped,
    markReviewed,
    loading,
    grouping,
    planError,
    style,
    busy,
    lastBatch,
    blockedUndo,
    sceneUndo,
    undoReason,
    noteCommit,
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
    reportBatch,
    undoBatch,
    undoLast,
    setRepresentative,
    setKeeperRule,
  };
}

export type Workflow = ReturnType<typeof useWorkflow>;
