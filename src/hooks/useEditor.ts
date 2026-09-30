// Editing state for one image in the Develop module: live adjustments, history, and render streams.
//
// Render contract (docs/ipc-changelog.md v5): the backend keeps only the newest render per (image, slot) and
// answers superseded ones with `null`. Sending one render per input event therefore starves the display during
// a drag (every render is cancelled by the next). So: at most ONE `renderPreview` is in flight per (image, slot);
// when it settles and the adjustments changed meanwhile, the latest are sent immediately. While a drag is active
// the main slot is requested at draft size; on release (or after 150 ms without input) at full quality.
// Results that are `null` or older than the last shown `seq` for their (image, slot) are ignored;
// `saveAdjustments` runs once on release.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  commands,
  unwrap,
  completeAdjustments,
  defaultAdjustments,
  type AdjustmentHistory,
  type CompleteAdjustments,
  type DevelopInfo,
  type EditState,
  type Histogram,
  type ImageFormat,
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
  /** Rendered with the crop disabled (the crop tool shows the whole frame). */
  uncropped: boolean;
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
  /** Source format of the image (selects the neutral defaults); RAW when unknown. */
  format?: ImageFormat;
  /** Render the full, uncropped frame (crop tool active). */
  uncropped?: boolean;
}

export interface Editor {
  adj: CompleteAdjustments;
  /** Neutral settings for this image's format ("reset" values). */
  defaults: CompleteAdjustments;
  history: AdjustmentHistory | null;
  info: DevelopInfo | null;
  main: RenderView | null;
  detail: RenderView | null;
  before: RenderView | null;
  histogram: Histogram | null;
  loading: boolean;
  /** Live edit (slider input): updates state and schedules a render. `label` names the history entry. */
  edit: (mutate: (a: CompleteAdjustments) => ParametricAdjustments, label: string) => void;
  /** Persist the pending live edit (slider release). */
  commit: () => void;
  /** Persist any pending edit and wait for all queued saves/undos to finish (call before batch commands). */
  flush: () => Promise<void>;
  /** edit + commit in one go (buttons, dropdowns, resets). */
  change: (mutate: (a: CompleteAdjustments) => ParametricAdjustments, label: string) => void;
  undo: () => void;
  redo: () => void;
  goto: (entryId: number) => void;
  /** Re-read adjustments + history from the backend (after batch operations). */
  reload: () => Promise<void>;
}

export function useEditor(id: number | null, opts: EditorOptions): Editor {
  const [adj, setAdj] = useState<CompleteAdjustments>(() => neutralAdjustments(opts.format));
  const [history, setHistory] = useState<AdjustmentHistory | null>(null);
  const [info, setInfo] = useState<DevelopInfo | null>(null);
  const [views, setViews] = useState<Record<RenderSlot, RenderView | null>>({ main: null, before: null, detail: null, mask: null });
  const [histogram, setHistogram] = useState<Histogram | null>(null);
  const [loading, setLoading] = useState(false);

  const optsRef = useRef(opts);
  optsRef.current = opts;
  const idRef = useRef(id);
  idRef.current = id;
  const adjRef = useRef(adj);
  const lastProfile = useRef("");
  const pending = useRef<{ id: number; label: string } | null>(null);
  const lastSeq = useRef(new Map<string, number>());
  const want = useRef<Record<RenderSlot, boolean>>({ main: false, before: false, detail: false, mask: false });
  const inflight = useRef(new Set<string>());
  const nullStreak = useRef(new Map<string, number>());
  const draft = useRef(false);
  const draftTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const chain = useRef<Promise<unknown>>(Promise.resolve());

  const enqueue = useCallback((fn: () => Promise<unknown>) => {
    chain.current = chain.current.then(fn).catch((e) => optsRef.current.onError(e));
  }, []);

  const pump = useCallback((slot: RenderSlot) => {
    const cur = idRef.current;
    const o = optsRef.current;
    if (cur == null || o.maxEdge <= 0 || !want.current[slot]) return;
    if (slot === "detail" && !o.region) {
      want.current.detail = false;
      return;
    }
    const key = `${cur}:${slot}`;
    if (inflight.current.has(key)) return; // its completion pumps again with the latest adjustments
    want.current[slot] = false;
    inflight.current.add(key);
    const cropOff = !!o.uncropped && slot !== "before";
    const base = slot === "before" ? neutralAdjustments(o.format) : adjRef.current;
    const a: ParametricAdjustments = cropOff ? { ...base, crop: { ...base.crop, enabled: false } } : base;
    const full = Math.min(2048, Math.max(64, Math.round(o.maxEdge)));
    const edge = draft.current && slot === "main" ? Math.max(256, Math.min(1024, Math.round(o.maxEdge / 2))) : full;
    const options = { maxEdge: Math.min(edge, full), slot, region: slot === "detail" ? o.region : null };
    unwrap(commands.renderPreview(cur, a, options))
      .then((r) => {
        if (!r) {
          // Superseded by another caller: retry a couple of times so the view never stays stale.
          const n = (nullStreak.current.get(key) ?? 0) + 1;
          nullStreak.current.set(key, n);
          if (n <= 3 && idRef.current === cur) want.current[slot] = true;
          return;
        }
        nullStreak.current.delete(key);
        if (r.imageId !== idRef.current) return; // image changed
        const k = `${r.imageId}:${r.slot}`;
        if (r.seq <= (lastSeq.current.get(k) ?? -1)) return; // older than what is shown
        lastSeq.current.set(k, r.seq);
        const v: RenderView = { imageId: r.imageId, url: r.url, width: r.width, height: r.height, seq: r.seq, renderMs: r.renderMs, lutMissing: r.lutMissing, uncropped: cropOff || !base.crop.enabled };
        setViews((prev) => ({ ...prev, [r.slot]: v }));
        if (r.slot === "main") setHistogram(r.histogram);
      })
      .catch((e) => optsRef.current.onError(e))
      .finally(() => {
        inflight.current.delete(key);
        (["main", "before", "detail"] as const).forEach((s) => pump(s));
      });
  }, []);

  const schedule = useCallback(
    (...slots: RenderSlot[]) => {
      for (const s of slots) want.current[s] = true;
      for (const s of slots) pump(s);
    },
    [pump],
  );

  /** Drag ended (release / idle): request the final full-quality render. */
  const endDraft = useCallback(() => {
    clearTimeout(draftTimer.current);
    if (!draft.current) return;
    draft.current = false;
    schedule("main");
  }, [schedule]);

  const setAdjBoth = useCallback((a0: ParametricAdjustments) => {
    const a = completeAdjustments(a0, optsRef.current.format);
    adjRef.current = a;
    lastProfile.current ||= JSON.stringify(a.profile);
    setAdj(a);
  }, []);

  const commitPending = useCallback(() => {
    const p = pending.current;
    if (!p) return;
    pending.current = null;
    endDraft();
    const snapshot = adjRef.current;
    enqueue(async () => {
      const h = await unwrap(commands.saveAdjustments(p.id, snapshot, p.label));
      if (idRef.current === p.id) setHistory(h);
      optsRef.current.onChanged(p.id);
      // Profile / look availability warnings depend on the saved settings.
      const pk = JSON.stringify(snapshot.profile);
      if (pk !== lastProfile.current && idRef.current === p.id) {
        lastProfile.current = pk;
        setInfo(await unwrap(commands.getDevelopInfo(p.id)));
      }
    });
  }, [enqueue, endDraft]);

  // Load on image change; persist a pending edit of the previous image first.
  useEffect(() => {
    if (id == null) return;
    let stale = false;
    setLoading(true);
    setViews({ main: null, before: null, detail: null, mask: null });
    setHistogram(null);
    setInfo(null);
    setHistory(null);
    lastProfile.current = "";
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
  const uncropped = !!opts.uncropped;
  useEffect(() => {
    if (id != null) schedule("main");
  }, [id, uncropped, schedule]);
  const regionKey = opts.region ? JSON.stringify(opts.region) : "";
  useEffect(() => {
    if (id != null && regionKey) schedule("detail");
    else setViews((v) => (v.detail ? { ...v, detail: null } : v));
  }, [id, regionKey, schedule]);

  useEffect(() => () => clearTimeout(draftTimer.current), []);

  const applyEdit = useCallback(
    (mutate: (a: CompleteAdjustments) => ParametricAdjustments, label: string, isDraft: boolean) => {
      const cur = idRef.current;
      if (cur == null) return;
      if (pending.current && pending.current.label !== label) commitPending();
      setAdjBoth(mutate(adjRef.current));
      pending.current = { id: cur, label };
      if (isDraft) {
        draft.current = true;
        clearTimeout(draftTimer.current);
        draftTimer.current = setTimeout(endDraft, 150);
      }
      schedule("main", ...(optsRef.current.region ? (["detail"] as const) : []));
    },
    [commitPending, setAdjBoth, schedule, endDraft],
  );

  const edit = useCallback((mutate: (a: CompleteAdjustments) => ParametricAdjustments, label: string) => applyEdit(mutate, label, true), [applyEdit]);

  const change = useCallback(
    (mutate: (a: CompleteAdjustments) => ParametricAdjustments, label: string) => {
      applyEdit(mutate, label, false);
      commitPending();
    },
    [applyEdit, commitPending],
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
    () => ({ adj, defaults: defaultAdjustments(opts.format), history, info, main: views.main, detail: views.detail, before: views.before, histogram, loading, edit, commit: commitPending, flush, change, undo, redo, goto, reload }),
    [adj, opts.format, history, info, views, histogram, loading, edit, commitPending, flush, change, undo, redo, goto, reload],
  );
}
