// Editing state for one image in the Develop module: live adjustments, history, and render streams.
//
// Render contract (docs/ipc-changelog.md v5): every input event schedules a `renderPreview` (throttled to
// one send per animation frame, always with the latest adjustments); results that are `null` or older than
// the last shown `seq` for their (image, slot) are ignored; `saveAdjustments` runs once on release.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  commands,
  unwrap,
  type AdjustmentHistory,
  type DevelopInfo,
  type EditState,
  type Histogram,
  type NormRect,
  type ParametricAdjustments,
  type RenderSlot,
} from "../ipc";
import { neutralAdjustments } from "../lib/adjust";

export interface RenderView {
  imageId: number;
  url: string;
  width: number;
  height: number;
  seq: number;
  renderMs: number;
  lutMissing: boolean;
}

export interface EditorOptions {
  /** Long edge in device pixels for the fitted render (0 = viewport not measured yet). */
  maxEdge: number;
  /** Visible region while zoomed to 100% (renders the `detail` slot); null when fitted. */
  region: NormRect | null;
  wantBefore: boolean;
  onError: (e: unknown) => void;
  /** Called after a history entry was saved / changed, so the library can refresh `hasEdits`. */
  onChanged: (id: number) => void;
}

export interface Editor {
  adj: ParametricAdjustments;
  history: AdjustmentHistory | null;
  info: DevelopInfo | null;
  main: RenderView | null;
  detail: RenderView | null;
  before: RenderView | null;
  histogram: Histogram | null;
  loading: boolean;
  /** Live edit (slider input): updates state and schedules a render. `label` names the history entry. */
  edit: (mutate: (a: ParametricAdjustments) => ParametricAdjustments, label: string) => void;
  /** Persist the pending live edit (slider release). */
  commit: () => void;
  /** Persist any pending edit and wait for all queued saves/undos to finish (call before batch commands). */
  flush: () => Promise<void>;
  /** edit + commit in one go (buttons, dropdowns, resets). */
  change: (mutate: (a: ParametricAdjustments) => ParametricAdjustments, label: string) => void;
  undo: () => void;
  redo: () => void;
  goto: (entryId: number) => void;
  /** Re-read adjustments + history from the backend (after batch operations). */
  reload: () => Promise<void>;
}

export function useEditor(id: number | null, opts: EditorOptions): Editor {
  const [adj, setAdj] = useState<ParametricAdjustments>(neutralAdjustments);
  const [history, setHistory] = useState<AdjustmentHistory | null>(null);
  const [info, setInfo] = useState<DevelopInfo | null>(null);
  const [views, setViews] = useState<Record<RenderSlot, RenderView | null>>({ main: null, before: null, detail: null });
  const [histogram, setHistogram] = useState<Histogram | null>(null);
  const [loading, setLoading] = useState(false);

  const optsRef = useRef(opts);
  optsRef.current = opts;
  const idRef = useRef(id);
  idRef.current = id;
  const adjRef = useRef(adj);
  const pending = useRef<{ id: number; label: string } | null>(null);
  const lastSeq = useRef(new Map<string, number>());
  const dirty = useRef({ main: false, before: false, detail: false });
  const raf = useRef(0);
  const chain = useRef<Promise<unknown>>(Promise.resolve());

  const enqueue = useCallback((fn: () => Promise<unknown>) => {
    chain.current = chain.current.then(fn).catch((e) => optsRef.current.onError(e));
  }, []);

  const send = useCallback((slot: RenderSlot) => {
    const cur = idRef.current;
    const o = optsRef.current;
    if (cur == null || o.maxEdge <= 0) return;
    if (slot === "detail" && !o.region) return;
    const a = slot === "before" ? neutralAdjustments() : adjRef.current;
    const options = { maxEdge: Math.min(8192, Math.max(64, Math.round(o.maxEdge))), slot, region: slot === "detail" ? o.region : null };
    unwrap(commands.renderPreview(cur, a, options))
      .then((r) => {
        if (!r || r.imageId !== idRef.current) return; // superseded by the backend / image changed
        const key = `${r.imageId}:${r.slot}`;
        if (r.seq <= (lastSeq.current.get(key) ?? -1)) return; // older than what is shown
        lastSeq.current.set(key, r.seq);
        const v: RenderView = { imageId: r.imageId, url: r.url, width: r.width, height: r.height, seq: r.seq, renderMs: r.renderMs, lutMissing: r.lutMissing };
        setViews((prev) => ({ ...prev, [r.slot]: v }));
        if (r.slot === "main") setHistogram(r.histogram);
      })
      .catch((e) => optsRef.current.onError(e));
  }, []);

  const schedule = useCallback(
    (...slots: RenderSlot[]) => {
      for (const s of slots) dirty.current[s] = true;
      if (raf.current) return;
      raf.current = requestAnimationFrame(() => {
        raf.current = 0;
        const d = dirty.current;
        dirty.current = { main: false, before: false, detail: false };
        (["main", "before", "detail"] as const).forEach((s) => d[s] && send(s));
      });
    },
    [send],
  );

  const setAdjBoth = useCallback((a: ParametricAdjustments) => {
    adjRef.current = a;
    setAdj(a);
  }, []);

  const commitPending = useCallback(() => {
    const p = pending.current;
    if (!p) return;
    pending.current = null;
    const snapshot = adjRef.current;
    enqueue(async () => {
      const h = await unwrap(commands.saveAdjustments(p.id, snapshot, p.label));
      if (idRef.current === p.id) setHistory(h);
      optsRef.current.onChanged(p.id);
    });
  }, [enqueue]);

  // Load on image change; persist a pending edit of the previous image first.
  useEffect(() => {
    if (id == null) return;
    let stale = false;
    setLoading(true);
    setViews({ main: null, before: null, detail: null });
    setHistogram(null);
    setInfo(null);
    setHistory(null);
    Promise.all([unwrap(commands.getAdjustments(id)), unwrap(commands.getHistory(id)), unwrap(commands.getDevelopInfo(id))])
      .then(([a, h, i]) => {
        if (stale) return;
        setAdjBoth(a);
        setHistory(h);
        setInfo(i);
        setLoading(false);
        schedule("main", ...(optsRef.current.wantBefore ? (["before"] as const) : []), ...(optsRef.current.region ? (["detail"] as const) : []));
      })
      .catch((e) => {
        if (!stale) optsRef.current.onError(e);
      });
    return () => {
      stale = true;
      commitPending();
    };
  }, [id, setAdjBoth, schedule, commitPending]);

  // Viewport changes.
  useEffect(() => {
    if (id != null && opts.maxEdge > 0) schedule("main");
  }, [id, opts.maxEdge, schedule]);
  useEffect(() => {
    if (id != null && opts.wantBefore) schedule("before");
  }, [id, opts.wantBefore, schedule]);
  const regionKey = opts.region ? JSON.stringify(opts.region) : "";
  useEffect(() => {
    if (id != null && regionKey) schedule("detail");
    else setViews((v) => (v.detail ? { ...v, detail: null } : v));
  }, [id, regionKey, schedule]);

  useEffect(() => () => cancelAnimationFrame(raf.current), []);

  const edit = useCallback(
    (mutate: (a: ParametricAdjustments) => ParametricAdjustments, label: string) => {
      const cur = idRef.current;
      if (cur == null) return;
      if (pending.current && pending.current.label !== label) commitPending();
      setAdjBoth(mutate(adjRef.current));
      pending.current = { id: cur, label };
      schedule("main", ...(optsRef.current.region ? (["detail"] as const) : []));
    },
    [commitPending, setAdjBoth, schedule],
  );

  const change = useCallback(
    (mutate: (a: ParametricAdjustments) => ParametricAdjustments, label: string) => {
      edit(mutate, label);
      commitPending();
    },
    [edit, commitPending],
  );

  const applyState = useCallback(
    (s: EditState, forId: number) => {
      if (idRef.current !== forId) return;
      setAdjBoth(s.adjustments);
      setHistory(s.history);
      schedule("main", ...(optsRef.current.region ? (["detail"] as const) : []));
      optsRef.current.onChanged(forId);
    },
    [setAdjBoth, schedule],
  );

  const stepper = useCallback(
    (fn: (id: number) => Promise<EditState>) => {
      const cur = idRef.current;
      if (cur == null) return;
      commitPending();
      enqueue(async () => applyState(await fn(cur), cur));
    },
    [commitPending, enqueue, applyState],
  );

  const undo = useCallback(() => stepper((i) => unwrap(commands.undoAdjustments(i))), [stepper]);
  const redo = useCallback(() => stepper((i) => unwrap(commands.redoAdjustments(i))), [stepper]);
  const goto = useCallback((entryId: number) => stepper((i) => unwrap(commands.gotoHistory(i, entryId))), [stepper]);

  const flush = useCallback(async () => {
    commitPending();
    await chain.current;
  }, [commitPending]);

  const reload = useCallback(async () => {
    const cur = idRef.current;
    if (cur == null) return;
    commitPending();
    await chain.current;
    const [a, h] = await Promise.all([unwrap(commands.getAdjustments(cur)), unwrap(commands.getHistory(cur))]);
    if (idRef.current !== cur) return;
    setAdjBoth(a);
    setHistory(h);
    schedule("main", ...(optsRef.current.region ? (["detail"] as const) : []));
    optsRef.current.onChanged(cur);
  }, [commitPending, setAdjBoth, schedule]);

  return useMemo(
    () => ({ adj, history, info, main: views.main, detail: views.detail, before: views.before, histogram, loading, edit, commit: commitPending, flush, change, undo, redo, goto, reload }),
    [adj, history, info, views, histogram, loading, edit, commitPending, flush, change, undo, redo, goto, reload],
  );
}
