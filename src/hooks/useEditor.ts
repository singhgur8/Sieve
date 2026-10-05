// Editing state for one image in the Develop module: live adjustments, history, and render streams.
//
// Render contract (docs/ipc-changelog.md v5): the backend keeps only the newest render per (image, slot) and
// answers superseded ones with `null`. Sending one render per input event therefore starves the display during
// a drag (every render is cancelled by the next). So: at most ONE `renderPreview` is in flight per (image, slot);
// when it settles and the adjustments changed meanwhile, the latest are sent immediately. While a drag is active
// the main slot is requested at draft size; on release (or after 300 ms without input) at full quality.
// Input is decoupled from React (Phase 8d): `edit` updates `adjRef` and schedules the render immediately, while the
// React `adj` state is pushed at most once per animation frame (and synchronously on release / non-live changes).
// Finished renders are decoded off-screen before they are swapped in, so the main thread never decodes at paint.
// Results that are `null` or older than the last shown `seq` for their (image, slot) are ignored;
// `saveAdjustments` runs once on release.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  commands,
  unwrap,
  completeAdjustments,
  defaultAdjustments,
  type AdjustmentField,
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
import { labelWithValue, neutralAdjustments } from "../lib/adjust";
import { changedFields } from "../lib/fieldGroups";

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
  /** A user commit / undo / redo wrote history (not a plain reload): batch Undo offers become unsafe. */
  onCommitted?: (id: number) => void;
  /** After a commit was saved: the setting groups that edit changed (crop / masks / transform never listed). Auto Sync hangs here. */
  onSaved?: (id: number, changed: AdjustmentField[]) => Promise<void> | void;
  /** Source format of the image (selects the neutral defaults); RAW when unknown. */
  format?: ImageFormat;
  /** Render the full, uncropped frame (crop tool active). */
  uncropped?: boolean;
  /** Render without the Transform (Upright / manual): the Guided tool draws on the sensor frame. */
  untransformed?: boolean;
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
  /** URL for the Navigator: the last settled (non-draft) main render, frozen while a drag is in progress. */
  navUrl: string | null;
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
  /** Time (ms) of the adjustment Cmd+Z would undo; 0 when there is nothing to undo. */
  lastCommitAt: () => number;
  /** True when a redo entry exists. */
  canRedo: () => boolean;
}

/** Settings that change the develop warnings: the profile / look, and which AI masks have a computed matte. */
const warnKey = (a: CompleteAdjustments) =>
  JSON.stringify([a.profile, a.masks.flatMap((g) => g.components.map((c) => (c.shape.kind === "ai" ? (c.shape.digest ?? "") : "")))]);

/** Without input for this long mid-drag, the full-quality render is requested. */
const DRAFT_IDLE_MS = 300;

/** Decode an image off-screen so the later <img> swap paints from the decoded cache (never rejects). */
function decodeUrl(url: string): Promise<void> {
  const img = new Image();
  img.src = url;
  const timeout = new Promise<void>((r) => setTimeout(r, 400));
  return Promise.race([img.decode().catch(() => undefined), timeout]);
}

export function useEditor(id: number | null, opts: EditorOptions): Editor {
  const [adj, setAdj] = useState<CompleteAdjustments>(() => neutralAdjustments(opts.format));
  const [history, setHistory] = useState<AdjustmentHistory | null>(null);
  const [info, setInfo] = useState<DevelopInfo | null>(null);
  const [views, setViews] = useState<Record<RenderSlot, RenderView | null>>({ main: null, before: null, detail: null, mask: null, navigator: null, preview: null });
  const [histogram, setHistogram] = useState<Histogram | null>(null);
  const [navUrl, setNavUrl] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const optsRef = useRef(opts);
  optsRef.current = opts;
  const idRef = useRef(id);
  idRef.current = id;
  const adjRef = useRef(adj);
  const lastProfile = useRef("");
  /** Settings as last loaded / saved: the next commit is diffed against it. */
  const baseRef = useRef<CompleteAdjustments | null>(null);
  const pending = useRef<{ id: number; label: string } | null>(null);
  const lastSeq = useRef(new Map<string, number>());
  const want = useRef<Record<RenderSlot, boolean>>({ main: false, before: false, detail: false, mask: false, navigator: false, preview: false });
  const inflight = useRef(new Set<string>());
  const nullStreak = useRef(new Map<string, number>());
  const draft = useRef(false);
  const draftTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const adjRaf = useRef(0);
  const lastHist = useRef(0);
  const chain = useRef<Promise<unknown>>(Promise.resolve());
  const historyRef = useRef<AdjustmentHistory | null>(null);
  historyRef.current = history;

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
    const flat = !!o.untransformed && slot !== "before";
    let a: ParametricAdjustments = cropOff || flat ? { ...base, crop: { ...base.crop, enabled: false } } : base;
    if (flat) a = { ...a, transform: neutralAdjustments(o.format).transform };
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
        const wasDraft = draft.current && r.slot === "main";
        const v: RenderView = { imageId: r.imageId, url: r.url, width: r.width, height: r.height, seq: r.seq, renderMs: r.renderMs, lutMissing: r.lutMissing, uncropped: cropOff || flat || !base.crop.enabled };
        // Decode off the display path (not awaited: the next render may start meanwhile), then swap in.
        void decodeUrl(r.url).then(() => {
          if (r.imageId !== idRef.current) return;
          const k = `${r.imageId}:${r.slot}`;
          if (r.seq <= (lastSeq.current.get(k) ?? -1)) return; // older than what is shown
          lastSeq.current.set(k, r.seq);
          const now = performance.now();
          const withHist = r.slot === "main" && (!draft.current || now - lastHist.current >= 120);
          if (withHist) lastHist.current = now;
          setViews((prev) => ({ ...prev, [r.slot]: v }));
          if (withHist) setHistogram(r.histogram);
          if (r.slot === "main" && !wasDraft && !draft.current) setNavUrl(r.url);
        });
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

  /** Push the live adjustments into React state now (cancels a pending frame push). */
  const flushAdj = useCallback(() => {
    if (adjRaf.current) {
      cancelAnimationFrame(adjRaf.current);
      adjRaf.current = 0;
    }
    setAdj(adjRef.current);
  }, []);

  const setAdjBoth = useCallback(
    (a0: ParametricAdjustments, live = false) => {
      const a = completeAdjustments(a0, optsRef.current.format);
      adjRef.current = a;
      lastProfile.current ||= warnKey(a);
      if (!live) return flushAdj();
      // Live input: React state at most once per frame; renders already read `adjRef`.
      adjRaf.current ||= requestAnimationFrame(() => {
        adjRaf.current = 0;
        setAdj(adjRef.current);
      });
    },
    [flushAdj],
  );

  const commitPending = useCallback(() => {
    const p = pending.current;
    if (!p) return;
    pending.current = null;
    flushAdj();
    endDraft();
    const snapshot = adjRef.current;
    const prev = baseRef.current;
    baseRef.current = snapshot;
    enqueue(async () => {
      const h = await unwrap(commands.saveAdjustments(p.id, snapshot, labelWithValue(p.label, snapshot)));
      if (idRef.current === p.id) setHistory(h);
      optsRef.current.onChanged(p.id);
      optsRef.current.onCommitted?.(p.id);
      if (optsRef.current.onSaved) await optsRef.current.onSaved(p.id, prev ? changedFields(prev, snapshot) : []);
      // Profile / look availability warnings depend on the saved settings.
      const pk = warnKey(snapshot);
      if (pk !== lastProfile.current && idRef.current === p.id) {
        lastProfile.current = pk;
        setInfo(await unwrap(commands.getDevelopInfo(p.id)));
      }
    });
  }, [enqueue, endDraft, flushAdj]);

  // Load on image change; persist a pending edit of the previous image first.
  useEffect(() => {
    if (id == null) return;
    let stale = false;
    setLoading(true);
    setViews({ main: null, before: null, detail: null, mask: null, navigator: null, preview: null });
    setHistogram(null);
    setNavUrl(null);
    setInfo(null);
    setHistory(null);
    lastProfile.current = "";
    Promise.all([unwrap(commands.getAdjustments(id)), unwrap(commands.getHistory(id)), unwrap(commands.getDevelopInfo(id))])
      .then(([a, h, i]) => {
        if (stale) return;
        setAdjBoth(a);
        baseRef.current = adjRef.current;
        setHistory(h);
        setInfo(i);
        setLoading(false);
        schedule("main", ...(optsRef.current.wantBefore ? (["before"] as const) : []), ...(optsRef.current.region ? (["detail"] as const) : []));
      })
      .catch((e) => {
        if (stale) return;
        setLoading(false);
        optsRef.current.onError(e);
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
  const untransformed = !!opts.untransformed;
  useEffect(() => {
    if (id != null) schedule("main");
  }, [id, uncropped, untransformed, schedule]);
  const regionKey = opts.region ? JSON.stringify(opts.region) : "";
  useEffect(() => {
    if (id != null && regionKey) schedule("detail");
    else setViews((v) => (v.detail ? { ...v, detail: null } : v));
  }, [id, regionKey, schedule]);

  useEffect(
    () => () => {
      clearTimeout(draftTimer.current);
      cancelAnimationFrame(adjRaf.current);
    },
    [],
  );

  const applyEdit = useCallback(
    (mutate: (a: CompleteAdjustments) => ParametricAdjustments, label: string, isDraft: boolean) => {
      const cur = idRef.current;
      if (cur == null) return;
      if (pending.current && pending.current.label !== label) commitPending();
      // The first edit of a gesture reaches React at once (callers select / create things in the same tick);
      // its continuation (same label, pending release) is coalesced to one state push per frame.
      const continuation = pending.current?.label === label;
      setAdjBoth(mutate(adjRef.current), isDraft && continuation);
      pending.current = { id: cur, label };
      if (isDraft) {
        draft.current = true;
        clearTimeout(draftTimer.current);
        draftTimer.current = setTimeout(endDraft, DRAFT_IDLE_MS);
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
      baseRef.current = adjRef.current;
      setHistory(s.history);
      schedule("main", ...(optsRef.current.region ? (["detail"] as const) : []));
      optsRef.current.onChanged(forId);
      optsRef.current.onCommitted?.(forId);
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
    const [a, h, i] = await Promise.all([unwrap(commands.getAdjustments(cur)), unwrap(commands.getHistory(cur)), unwrap(commands.getDevelopInfo(cur))]);
    if (idRef.current !== cur) return;
    setAdjBoth(a);
    baseRef.current = adjRef.current;
    setHistory(h);
    setInfo(i);
    setLoading(false);
    schedule("main", ...(optsRef.current.region ? (["detail"] as const) : []));
    optsRef.current.onChanged(cur);
  }, [commitPending, setAdjBoth, schedule]);

  const lastCommitAt = useCallback(() => {
    if (pending.current) return Date.now();
    const h = historyRef.current;
    if (!h || !h.canUndo) return 0;
    return h.entries.find((e) => e.id === h.currentEntryId)?.createdAtMs ?? 0;
  }, []);
  const canRedo = useCallback(() => !!historyRef.current?.canRedo, []);

  const defaults = useMemo(() => defaultAdjustments(opts.format), [opts.format]);
  return useMemo(
    () => ({ adj, defaults, history, info, main: views.main, detail: views.detail, before: views.before, histogram, navUrl, loading, edit, commit: commitPending, flush, change, undo, redo, goto, reload, lastCommitAt, canRedo }),
    [adj, defaults, history, info, views, histogram, navUrl, loading, edit, commitPending, flush, change, undo, redo, goto, reload, lastCommitAt, canRedo],
  );
}
