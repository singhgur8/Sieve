// Pass 1: review the delivery set. One picked photo at a time with its whole moment in a strip (the other picks, then the
// alternatives); everything on keys. Opens at the first pick not looked at yet; Shift+Right accepts the rest of a moment.
import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState } from "react";
import { ArrowRight, LayoutGrid } from "lucide-react";
import { commands, DEFAULT_QUERY, unwrap, type Alternatives, type ImageSelection, type Moment } from "../../ipc";
import { previewSrc } from "../../lib/entryImage";
import { hint } from "../../lib/keymap";
import { clockOf, loadReviewed, momentLabel, num, saveReviewed, SHOT_LABEL, stemOf } from "../../lib/target";
import { ActionButton, OverflowStrip, Pic, useFocusIntoView, Reasons, ShotBadge, useSyncedZoom, Windowed, ZoomPic } from "./bits";
import type { StageRef } from "./SetupStep";
import type { TargetCtx } from "./types";

export const ReviewStep = forwardRef<StageRef, { ctx: TargetCtx }>(function ReviewStep({ ctx }, ref) {
  const { projectId, entries } = ctx;
  const [ids, setIds] = useState<number[] | null>(null);
  const [cur, setCur] = useState<number | null>(null);
  const [alts, setAlts] = useState<Alternatives | null>(null);
  const [altIdx, setAltIdx] = useState(0);
  const [moments, setMoments] = useState<Map<number, Moment>>(new Map());
  const [reviewed, setReviewed] = useState(() => loadReviewed(projectId, "review"));
  const [flash, setFlash] = useState(false);
  const zoom = useSyncedZoom();
  const idx = useRef(0);
  const busy = useRef(false);
  const first = useRef(true);
  const pending = useRef<number | null>(null);
  const [pendTick, setPendTick] = useState(0);
  const stamp = ctx.run?.finishedAtMs ?? 0;

  const markReviewed = useCallback(
    (...list: (number | null)[]) => {
      setReviewed((s) => {
        const add = list.filter((i): i is number => i != null && !s.has(i));
        if (add.length === 0) return s;
        const n = new Set(s);
        add.forEach((i) => n.add(i));
        saveReviewed(projectId, "review", n);
        return n;
      });
    },
    [projectId],
  );
  const unmarkReviewed = useCallback(
    (list: number[]) => {
      setReviewed((s) => {
        if (!list.some((i) => s.has(i))) return s;
        const n = new Set(s);
        list.forEach((i) => n.delete(i));
        saveReviewed(projectId, "review", n);
        return n;
      });
    },
    [projectId],
  );
  // N3-5: the reviewed marks each undoable action added (ids that were not reviewed before), so its undo takes them away.
  const actionMarks = useRef<{ at: number; ids: number[] }[]>([]);
  const markFor = (at: number, ...ids: number[]) => {
    actionMarks.current = [...actionMarks.current, { at, ids: [...new Set(ids)].filter((i) => !reviewed.has(i)) }].slice(-200);
    markReviewed(...ids);
  };

  const loadList = useCallback(
    async (prefer?: number | null) => {
      try {
        const list = await unwrap(commands.listImageIds({ ...DEFAULT_QUERY, projectId, targetChoices: ["deliver"] }));
        setIds(list);
        // P1-2: the first load resumes at the first pick not looked at yet (everything reviewed: photo 1). Decided outside the
        // state updater, which StrictMode runs twice.
        const resume = first.current && list.length > 0 ? (list.find((i) => !loadReviewed(projectId, "review").has(i)) ?? list[0]) : null;
        if (resume != null) first.current = false;
        setCur((prev) => {
          if (resume != null) return resume;
          if (pending.current != null && list.includes(pending.current)) return pending.current; // undo: back on the photo
          const want = prefer ?? prev;
          if (want != null && list.includes(want)) return want;
          return list[Math.min(idx.current, list.length - 1)] ?? null; // a set-aside photo: the next one slides in
        });
      } catch (e) {
        ctx.onError(e);
      }
    },
    [projectId, ctx.onError], // eslint-disable-line react-hooks/exhaustive-deps
  );
  useEffect(() => {
    void loadList();
  }, [loadList, ctx.rev, stamp]);
  useEffect(() => {
    let stale = false;
    unwrap(commands.listMoments(projectId))
      .then((ms) => {
        if (stale) return;
        const m = new Map<number, Moment>();
        ms.forEach((x) => x.imageIds.forEach((i) => m.set(i, x)));
        setMoments(m);
      })
      .catch(ctx.onError);
    return () => {
      stale = true;
    };
  }, [projectId, stamp, ctx.rev]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (ids && cur != null) idx.current = Math.max(0, ids.indexOf(cur));
  }, [ids, cur]);

  // Undo moved here (P1-3): select the photo the undone action was made on once the list has it.
  useEffect(() => {
    if (ctx.focus && ctx.focus.stage === "review") {
      const id = ctx.focus.id;
      const k = actionMarks.current.map((m) => m.at).lastIndexOf(id);
      if (k >= 0) {
        unmarkReviewed(actionMarks.current[k].ids);
        actionMarks.current = actionMarks.current.filter((_, n) => n !== k);
      }
      pending.current = id;
      setPendTick((n) => n + 1);
      ctx.focusDone();
    }
  }, [ctx.focus]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    if (pending.current != null && ids?.includes(pending.current)) {
      setCur(pending.current);
      setAltIdx(0);
      pending.current = null;
    }
  }, [ids, ctx.rev, pendTick]);

  // The alternatives strip of the current photo.
  useEffect(() => {
    if (cur == null) return setAlts(null);
    let stale = false;
    unwrap(commands.getAlternatives(cur))
      .then((a) => {
        if (stale) return;
        setAlts(a);
        setAltIdx((i) => Math.min(i, Math.max(0, a.alternatives.length - 1)));
        entries.need([cur, ...a.alternatives.map((x) => x.imageId)]);
      })
      .catch(ctx.onError);
    return () => {
      stale = true;
    };
  }, [cur, ctx.rev]); // eslint-disable-line react-hooks/exhaustive-deps

  // Prefetch the next photos (entries, then their previews into the browser cache).
  const list = ids ?? [];
  useEffect(() => {
    const next = list.slice(idx.current + 1, idx.current + 4);
    entries.need(next);
    next.forEach((i) => {
      const src = previewSrc(entries.get(i));
      if (src) new Image().src = src;
    });
  });

  const total = list.length;
  const done = useMemo(() => list.filter((i) => reviewed.has(i)).length, [list, reviewed]);
  const at = cur != null ? list.indexOf(cur) : -1;
  const entry = cur != null ? entries.get(cur) : undefined;
  const sel: ImageSelection | null = alts?.delivered ?? null;
  const list2 = alts?.alternatives ?? [];
  const alt = list2[Math.min(altIdx, list2.length - 1)];
  const altEntry = alt ? entries.get(alt.imageId) : undefined;
  const moment = cur != null ? moments.get(cur) : undefined;
  const keyOf = (i: number) => moments.get(i)?.id ?? -i;
  // Moments in list order: "Moment 12 of 180".
  const order = useMemo(() => {
    const seen = new Map<number, number>();
    list.forEach((i) => {
      const k = moments.get(i)?.id ?? -i;
      if (!seen.has(k)) seen.set(k, seen.size);
    });
    return seen;
  }, [list, moments]);
  const momentNo = cur != null ? (order.get(keyOf(cur)) ?? -1) + 1 : 0;
  const otherPicks = (moment?.deliveredIds ?? []).filter((i) => i !== cur);
  useEffect(() => {
    entries.need(otherPicks);
  });

  const jump = (to: number) => {
    if (list.length === 0) return;
    markReviewed(cur);
    setCur(list[Math.max(0, Math.min(list.length - 1, to))]);
    setAltIdx(0);
  };
  const go = (d: number) => jump(Math.max(0, at) + d);
  const nextMoment = () => {
    if (cur == null) return;
    const k = keyOf(cur);
    const mine = list.filter((i) => keyOf(i) === k);
    markReviewed(...mine);
    const j = list.findIndex((_, n) => n > at && keyOf(list[n]) !== k);
    if (j >= 0) {
      setCur(list[j]);
      setAltIdx(0);
    }
  };
  const prevMoment = () => {
    if (cur == null) return;
    const k = keyOf(cur);
    let j = at;
    while (j >= 0 && keyOf(list[j]) === k) j--;
    if (j < 0) return;
    const pk = keyOf(list[j]);
    while (j > 0 && keyOf(list[j - 1]) === pk) j--;
    jump(j);
  };
  const nextOpen = (d: 1 | -1) => {
    for (let n = at + d; n >= 0 && n < list.length; n += d) {
      if (!reviewed.has(list[n])) return jump(n);
    }
  };
  const act = async (fn: () => Promise<void>) => {
    if (busy.current) return;
    busy.current = true;
    try {
      await fn();
    } finally {
      busy.current = false;
    }
  };
  const nudge = () => {
    setFlash(true);
    setTimeout(() => setFlash(false), 600);
  };
  const keep = () =>
    act(async () => {
      if (cur == null) return;
      const r = await ctx.edit(`Keep ${stemOf(entry, cur)}`, unwrap(commands.setTargetChoice([cur], "deliver")), cur);
      if (r) {
        markFor(cur, cur);
        go(1);
      }
    });
  const reject = () =>
    act(async () => {
      if (cur == null) return;
      markFor(cur, cur);
      const r = await ctx.edit(`Set aside ${stemOf(entry, cur)}`, unwrap(commands.setTargetChoice([cur], "set_aside")), cur);
      if (r) ctx.notify(`Set aside ${stemOf(entry, cur)}. Cmd+Z undoes it`);
    });
  const swap = () =>
    act(async () => {
      if (cur == null) return;
      if (!alt) return nudge();
      const r = await ctx.edit(`Swap ${stemOf(entry, cur)} for ${stemOf(altEntry, alt.imageId)}`, unwrap(commands.swapAlternative(cur, alt.imageId)), cur);
      if (r) {
        markFor(cur, alt.imageId, cur);
        idx.current = Math.max(0, at);
        setCur(alt.imageId);
        setAltIdx(0);
        ctx.notify(`Swapped in ${stemOf(altEntry, alt.imageId)}. Cmd+Z undoes it`);
      }
    });
  const add = () =>
    act(async () => {
      if (!alt) return nudge();
      const r = await ctx.edit(`Add ${stemOf(altEntry, alt.imageId)}`, unwrap(commands.addAlternative(alt.imageId)), cur ?? undefined);
      if (r) ctx.notify(`Added ${stemOf(altEntry, alt.imageId)} too. Cmd+Z undoes it`);
    });
  const cycle = (d: number) => (list2.length === 0 ? nudge() : setAltIdx((i) => (i + d + list2.length) % list2.length));


  // ---- Moment grid (R1-2): one row per moment, the picks big on the left, the alternatives on the right ----
  const [view, setView] = useState<"two" | "grid">("two");
  const [g, setG] = useState<{ key: number; col: number } | null>(null);
  const [rowSel, setRowSel] = useState<Map<number, ImageSelection[]>>(new Map());
  const loadingRows = useRef(new Set<number>());
  const rowRev = useRef(0);
  const lastRow = useRef(0);
  useEffect(() => {
    rowRev.current++;
    loadingRows.current.clear();
    setRowSel(new Map());
  }, [ctx.rev, stamp]);
  const rows = useMemo(() => {
    const out: { key: number; picks: number[]; moment: Moment | undefined }[] = [];
    const by = new Map<number, (typeof out)[number]>();
    list.forEach((i) => {
      const k = moments.get(i)?.id ?? -i;
      let r = by.get(k);
      if (!r) {
        r = { key: k, picks: [], moment: moments.get(i) };
        by.set(k, r);
        out.push(r);
      }
      r.picks.push(i);
    });
    return out;
  }, [list, moments]);
  const altsOf = (key: number) => (rowSel.get(key) ?? []).filter((s) => s.choice === "alternative").sort((a, b) => (a.rank ?? 1e9) - (b.rank ?? 1e9) || a.imageId - b.imageId);
  const needRow = (r: { key: number; moment: Moment | undefined }) => {
    if (rowSel.has(r.key) || loadingRows.current.has(r.key)) return;
    if (!r.moment) return void setRowSel((m) => (m.has(r.key) ? m : new Map(m).set(r.key, [])));
    loadingRows.current.add(r.key);
    const stampRev = rowRev.current;
    unwrap(commands.getImageSelections(r.moment.imageIds))
      .then((rowsSel) => {
        if (stampRev !== rowRev.current) return;
        entries.need(rowsSel.filter((s) => s.choice === "alternative").map((s) => s.imageId));
        setRowSel((m) => new Map(m).set(r.key, rowsSel));
      })
      .catch(ctx.onError)
      .finally(() => loadingRows.current.delete(r.key));
  };
  const cellsOf = (r: { key: number; picks: number[] }) => [...r.picks, ...altsOf(r.key).map((s) => s.imageId)];
  const gRow = (() => {
    const at = g ? rows.findIndex((r) => r.key === g.key) : -1;
    return at >= 0 ? at : Math.max(0, Math.min(rows.length - 1, lastRow.current));
  })();
  const gCells = view === "grid" && rows[gRow] ? cellsOf(rows[gRow]) : [];
  const gCol = g ? Math.max(0, Math.min(gCells.length - 1, g.col)) : 0;
  const gid = gCells[gCol] ?? null;
  useFocusIntoView(view === "grid" && gid != null ? `target-grid-cell-${gid}` : null);
  useEffect(() => {
    if (view === "grid") lastRow.current = gRow;
  }, [view, gRow]);
  useEffect(() => {
    if (view === "grid" && rows[gRow]) needRow(rows[gRow]);
  });
  const addedFocus = useRef<number | null>(null);
  useEffect(() => {
    const want = addedFocus.current;
    if (want == null) return;
    const r = rows.find((x) => x.picks.includes(want));
    if (!r) return;
    addedFocus.current = null;
    setG({ key: r.key, col: r.picks.indexOf(want) });
  }, [rows]);
  const focusRow = (n: number, col = 0) => {
    const r = rows[Math.max(0, Math.min(rows.length - 1, n))];
    if (r) setG({ key: r.key, col });
  };
  const openGrid = () => {
    const first = rows.findIndex((r) => r.picks.some((i) => !reviewed.has(i)));
    const curRow = cur != null ? rows.findIndex((r) => r.key === keyOf(cur)) : 0;
    focusRow(first >= 0 ? first : Math.max(0, curRow));
    setView("grid");
  };
  const openCell = () => {
    if (gid == null) return;
    if (list.includes(gid)) {
      setCur(gid);
      setAltIdx(0);
    } else {
      const s = altsOf(rows[gRow].key).find((a) => a.imageId === gid);
      const pick = s?.alternativeOf;
      if (pick != null && list.includes(pick)) {
        setCur(pick);
        setAltIdx(Math.max(0, altsOf(rows[gRow].key).filter((a) => a.alternativeOf === pick).findIndex((a) => a.imageId === gid)));
      }
    }
    setView("two");
  };
  const gridAside = () =>
    act(async () => {
      if (gid == null || !list.includes(gid)) return;
      const en = entries.get(gid);
      markFor(gid, gid);
      const r = await ctx.edit(`Set aside ${stemOf(en, gid)}`, unwrap(commands.setTargetChoice([gid], "set_aside")), gid);
      if (r) ctx.notify(`Set aside ${stemOf(en, gid)}. Cmd+Z undoes it`);
    });
  const gridAdd = () =>
    act(async () => {
      if (gid == null || list.includes(gid)) return;
      const en = entries.get(gid);
      const r = await ctx.edit(`Add ${stemOf(en, gid)}`, unwrap(commands.addAlternative(gid)), cur ?? undefined);
      if (r) {
        addedFocus.current = gid; // N3-5: the ring stays on the added photo, which moves among the picks
        ctx.notify(`Added ${stemOf(en, gid)} too. Cmd+Z undoes it`);
      }
    });
  const gridNextMoment = () => {
    const r = rows[gRow];
    if (!r) return;
    markReviewed(...r.picks);
    if (gRow + 1 < rows.length) focusRow(gRow + 1);
  };
  const gridOpenRow = (d: 1 | -1) => {
    for (let n = gRow + d; n >= 0 && n < rows.length; n += d) if (rows[n].picks.some((i) => !reviewed.has(i))) return focusRow(n);
  };
  const gridKey = (id: string, e: { key: string; shiftKey: boolean }): boolean => {
    switch (id) {
      case "targetPrev":
        return setG({ key: rows[gRow].key, col: gCol - 1 }), true;
      case "targetNext":
        return setG({ key: rows[gRow].key, col: gCol + 1 }), true;
      case "targetCycle":
        if (e.key === "Tab") return setG({ key: rows[gRow].key, col: gCol + (e.shiftKey ? -1 : 1) }), true;
        return focusRow(gRow + (e.key === "ArrowUp" ? -1 : 1), gCol), true;
      case "targetNextMoment":
        return gridNextMoment(), true;
      case "targetPrevMoment":
        return focusRow(gRow - 1), true;
      case "targetNextOpen":
        return gridOpenRow(1), true;
      case "targetPrevOpen":
        return gridOpenRow(-1), true;
      case "targetFirst":
        return focusRow(0), true;
      case "targetLast":
        return focusRow(rows.length - 1), true;
      case "targetReject":
        return void gridAside(), true;
      case "targetAdd":
        return void gridAdd(), true;
      case "targetEnter":
      case "targetGrid":
        return openCell(), true;
      default:
        return true; // Keep / Swap / Zoom mean nothing in the grid
    }
  };

  useImperativeHandle(
    ref,
    () => ({
      key: (id, e) => {
        if (id === "targetGrid" && view === "two") return openGrid(), true;
        if (view === "grid" && rows.length > 0) return gridKey(id, e);
        switch (id) {
          case "targetPrev":
            return go(-1), true;
          case "targetNext":
            return go(1), true;
          case "targetNextMoment":
            return nextMoment(), true;
          case "targetPrevMoment":
            return prevMoment(), true;
          case "targetNextOpen":
            return nextOpen(1), true;
          case "targetPrevOpen":
            return nextOpen(-1), true;
          case "targetFirst":
            return jump(0), true;
          case "targetLast":
            return jump(list.length - 1), true;
          case "targetKeep":
          case "targetEnter":
            return void keep(), true;
          case "targetReject":
            return void reject(), true;
          case "targetSwap":
            return void swap(), true;
          case "targetAdd":
            return void add(), true;
          case "targetZoom":
            return zoom.toggle(), true;
          case "targetCycle":
            return cycle(e.key === "ArrowUp" || (e.key === "Tab" && e.shiftKey) ? -1 : 1), true;
          default:
            return false;
        }
      },
    }),
    [ids, cur, at, alt, altIdx, list2.length, entry, altEntry, reviewed, moments, view, g, rows, rowSel], // eslint-disable-line react-hooks/exhaustive-deps
  );

  if (ids == null) return <div className="p-6 text-sm text-neutral-400">Loading the picks…</div>;
  if (total === 0)
    return (
      <div className="mx-auto max-w-xl p-8 text-center text-sm text-neutral-300" data-testid="target-review-empty">
        There are no picked photos yet. Run &ldquo;Pick the best N&rdquo; first.
        <div className="mt-3 flex justify-center">
          <ActionButton testid="target-review-to-setup" label="Pick the best N" keys={[]} tone="primary" title="Choose the delivery set" onClick={() => ctx.go("setup")} />
        </div>
      </div>
    );

  const pct = total > 0 ? Math.round((done / total) * 100) : 0;
  const altNo = Math.min(altIdx, list2.length - 1);
  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="target-review" data-current={cur ?? ""} data-total={total} data-reviewed={done} data-moment={momentNo} data-view={view}>
      <div className="flex shrink-0 items-center gap-3 border-b border-neutral-800 px-4 py-2 text-sm">
        <span className="font-medium text-neutral-100" data-testid="target-review-count" title="Photos of the delivery set you have looked at (kept, swapped or moved past)">
          {num(done)} of {num(total)} reviewed
        </span>
        <div className="h-1.5 w-48 overflow-hidden rounded bg-neutral-800" role="progressbar" aria-valuenow={pct} aria-label="Review progress">
          <div className="h-full bg-emerald-500" style={{ width: `${pct}%` }} />
        </div>
        {view === "two" && (
          <span className="text-xs text-neutral-400" data-testid="target-review-position">
            Photo {num(at + 1)} of {num(total)}
          </span>
        )}
        <span className="text-xs text-neutral-400" data-testid="target-review-moment-no">
          {/* N3-4: in the grid the position follows the grid focus */}
          Moment {num(view === "grid" ? gRow + 1 : momentNo)} of {num(view === "grid" ? rows.length : order.size)}
        </span>
        <span className="ml-auto flex items-center gap-2">
          <button className="flex h-7 items-center gap-1.5 rounded bg-neutral-800 px-2 text-xs hover:bg-neutral-700" data-testid="target-show-picks" title="Close this view and show the picked photos in the grid" onClick={() => ctx.showInGrid(["deliver"])}>
            <LayoutGrid className="size-3.5" /> Show in grid
          </button>
          <button
            className={`flex h-7 items-center gap-1.5 rounded px-2 text-xs ${done >= total ? "bg-sky-700 text-white hover:bg-sky-600" : "bg-neutral-800 hover:bg-neutral-700"}`}
            data-testid="target-to-second"
            title="Go on to the second look: the Not sure photos first, then similar, weaker and defective frames"
            onClick={() => ctx.go("second")}
          >
            Second look <ArrowRight className="size-3.5" />
          </button>
        </span>
      </div>

      {done >= total && (
        <p className="shrink-0 bg-emerald-950 px-4 py-1.5 text-xs text-emerald-100" data-testid="target-review-complete">
          All {num(total)} picks reviewed. Next: the second look, then Apply.
        </p>
      )}

      {view === "two" ? (
        <>
      <div className="flex min-h-0 flex-1 gap-3 p-3">
        <figure className="relative flex min-w-0 flex-1 flex-col" data-testid="target-cur" data-id={cur ?? ""}>
          <ZoomPic entry={entry} zoom={zoom} primary className="min-h-0 flex-1 rounded-lg" testid="target-cur-pic" />
          <div className="pointer-events-none absolute left-2 top-2 flex flex-wrap items-center gap-1.5">
            <ShotBadge type={sel?.shotType ?? moment?.shotType ?? null} testid="target-cur-shot" />
            <span className="rounded bg-black/70 px-1.5 py-0.5 text-[11px] text-neutral-100" data-testid="target-cur-moment">
              {momentLabel(moment, cur ?? -1, false) || "No moment"}
            </span>
          </div>
          <figcaption className="mt-2 flex items-start gap-3 text-xs">
            <span className="shrink-0 rounded bg-emerald-800 px-1.5 py-0.5 font-semibold text-emerald-50">Picked</span>
            <span className="shrink-0 text-neutral-200" data-testid="target-cur-name">
              {entry?.fileName ?? ""}
            </span>
            <span className="min-w-0 text-neutral-300">
              <Reasons sel={sel ?? undefined} />
            </span>
          </figcaption>
        </figure>

        {alt && (
          <figure className="relative flex min-w-0 flex-1 flex-col" data-testid="target-alt-view" data-id={alt.imageId}>
            <ZoomPic entry={altEntry} zoom={zoom} className="min-h-0 flex-1 rounded-lg ring-1 ring-sky-700" testid="target-alt-pic" />
            <figcaption className="mt-2 flex items-start gap-3 text-xs">
              <span className="shrink-0 rounded bg-sky-800 px-1.5 py-0.5 font-semibold text-sky-50" data-testid="target-alt-rank">
                Alternative {altNo + 1} of {list2.length}
              </span>
              <span className="shrink-0 text-neutral-200">{altEntry?.fileName ?? ""}</span>
              <span className="min-w-0 text-neutral-300" data-testid="target-alt-reason">
                <Reasons sel={alt} max={1} />
              </span>
            </figcaption>
          </figure>
        )}
      </div>

      <div className="shrink-0 border-t border-neutral-800 px-3 py-2" data-testid="target-strip">
        <div className="mb-1 flex items-center gap-2 text-[11px] uppercase tracking-wide text-neutral-400" data-testid="target-strip-header">
          <span data-testid="target-strip-title">
            This moment: {otherPicks.length + 1} picked · {list2.length} {list2.length === 1 ? "alternative" : "alternatives"}
          </span>
          <span className="hidden normal-case text-neutral-500" data-testid="target-strip-count">
            {list2.length}
          </span>
        </div>
        <ul className="flex gap-2 overflow-x-auto p-1">
          {cur != null && (
            <li className="w-52 shrink-0" data-testid="target-showing">
              <div className="flex w-full gap-2 rounded bg-neutral-900 p-1 text-left ring-2 ring-white">
                <Pic entry={entry} className="h-14 w-20 shrink-0 rounded" />
                <span className="min-w-0 text-[11px] leading-4">
                  <b className="rounded bg-white px-1 text-neutral-900">Showing</b> <span className="text-neutral-300">{entry ? stemOf(entry, cur) : ""}</span>
                </span>
              </div>
            </li>
          )}
          {otherPicks.map((pid) => {
            const e = entries.get(pid);
            return (
              <li key={`p${pid}`} className="w-52 shrink-0">
                <button
                  className="flex w-full gap-2 rounded bg-neutral-900 p-1 text-left ring-2 ring-emerald-700 hover:ring-emerald-500"
                  data-testid={`target-pick-${pid}`}
                  title={`${e?.fileName ?? ""}: also picked in this moment. Click to review it`}
                  onClick={() => {
                    if (list.includes(pid)) jump(list.indexOf(pid));
                  }}
                >
                  <Pic entry={e} className="h-14 w-20 shrink-0 rounded" />
                  <span className="min-w-0 text-[11px] leading-4">
                    <b className="rounded bg-emerald-800 px-1 text-emerald-50">Picked</b> <span className="text-neutral-300">{e ? stemOf(e, pid) : ""}</span>
                  </span>
                </button>
              </li>
            );
          })}
          {list2.map((a, i) => {
            const e = entries.get(a.imageId);
            const on = i === altNo;
            return (
              <li key={a.imageId} className="w-52 shrink-0">
                <button
                  className={`flex w-full gap-2 rounded p-1 text-left ${on ? "bg-sky-950 ring-2 ring-sky-500" : "bg-neutral-900 ring-1 ring-sky-900 hover:ring-sky-600"}`}
                  data-testid={`target-alt-${a.imageId}`}
                  data-selected={on}
                  data-rank={a.rank ?? ""}
                  title={`${e?.fileName ?? ""}: ${a.reasons[0]?.text ?? "alternative"}. Click to compare; S swaps it in (double-click also swaps)${hint("targetSwap")}`}
                  onClick={() => setAltIdx(i)}
                  onDoubleClick={() => void swap()}
                >
                  <Pic entry={e} className="h-14 w-20 shrink-0 rounded" />
                  <span className="min-w-0 text-[11px] leading-4">
                    <b className="text-sky-200">#{a.rank ?? i + 1}</b> <span className="text-neutral-300">{e ? stemOf(e, a.imageId) : ""}</span>
                    <span className="line-clamp-2 text-neutral-400">{a.reasons[0]?.text}</span>
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
        {list2.length === 0 && (
          <p className={`text-xs ${flash ? "font-semibold text-amber-300" : "text-neutral-400"}`} data-testid="target-strip-empty" data-flash={flash}>
            No alternatives for this moment.
          </p>
        )}
      </div>

        </>
      ) : (
        <>
      <Windowed
        testid="target-grid"
        rowH={200}
        count={rows.length}
        focusRow={gRow}
        renderRow={(n) => {
          const r = rows[n];
          if (!r) return null;
          needRow(r);
          const alts = altsOf(r.key);
          entries.need([...r.picks, ...alts.map((a) => a.imageId)]);
          const done = r.picks.every((i) => reviewed.has(i));
          const ring = "outline outline-2 outline-offset-2 outline-sky-400";
          return (
            <div className={`h-full border-b border-neutral-900 px-3 ${done ? "opacity-60" : ""}`} data-testid={`target-grid-row-${n}`} data-moment={r.key} data-reviewed={done}>
              <div className="flex h-5 items-center text-[11px] text-neutral-400" data-testid={`target-grid-head-${n}`}>
                Moment {num(n + 1)} of {num(rows.length)}
                {r.moment ? ` · ${[clockOf(r.moment.startedAtMs), SHOT_LABEL[r.moment.shotType]].filter(Boolean).join(" · ")}` : ""} · {r.picks.length} picked · {alts.length} {alts.length === 1 ? "alternative" : "alternatives"}
              </div>
              <div className="flex items-start gap-3">
                <div className="flex shrink-0 gap-2 px-1 py-1">
                  {r.picks.map((id) => {
                    const on = id === gid;
                    const e = entries.get(id);
                    return (
                      <button key={id} className={`relative h-[160px] w-[240px] shrink-0 rounded ${on ? ring : ""}`} data-testid={`target-grid-cell-${id}`} data-kind="pick" data-focused={on} title={`${stemOf(e, id)}: picked. X sets it aside, Enter opens it`} onClick={() => setG({ key: r.key, col: r.picks.indexOf(id) })} onDoubleClick={() => { setG({ key: r.key, col: r.picks.indexOf(id) }); setCur(id); setAltIdx(0); setView("two"); }}>
                        <Pic entry={e} className="size-full rounded ring-1 ring-emerald-700" />
                        <span className="absolute left-1 top-1 rounded bg-emerald-800 px-1.5 py-0.5 text-[11px] font-semibold text-emerald-50">Picked</span>
                        <span className="absolute bottom-1 left-1 rounded bg-black/70 px-1 text-[11px] text-neutral-100">{stemOf(e, id)}</span>
                      </button>
                    );
                  })}
                </div>
                <div className="mt-1 h-40 w-px shrink-0 bg-neutral-700" />
                <OverflowStrip testid={`target-grid-alts-${n}`}>
                  {alts.map((a, k) => {
                    const on = a.imageId === gid;
                    const e = entries.get(a.imageId);
                    return (
                      <button key={a.imageId} className={`w-40 shrink-0 rounded text-left ${on ? ring : ""}`} data-testid={`target-grid-cell-${a.imageId}`} data-kind="alt" data-focused={on} title={`${stemOf(e, a.imageId)}: ${a.reasons[0]?.text ?? "alternative"} (#${a.rank ?? k + 1} alternative of ${a.alternativeOf != null ? stemOf(entries.get(a.alternativeOf), a.alternativeOf) : "this moment"}). A adds it, Enter opens it`} onClick={() => setG({ key: r.key, col: r.picks.length + k })}>
                        <Pic entry={e} className="h-[112px] w-40 rounded ring-1 ring-sky-900" />
                        <div className="mt-0.5 flex items-center gap-1 text-[11px] leading-4">
                          <b className="shrink-0 text-sky-300">#{a.rank ?? k + 1}</b>
                          <span className="truncate text-neutral-400" data-testid="target-alt-caption">≈ {a.alternativeOf != null ? stemOf(entries.get(a.alternativeOf), a.alternativeOf) : stemOf(e, a.imageId)}</span>
                        </div>
                      </button>
                    );
                  })}
                </OverflowStrip>
              </div>
            </div>
          );
        }}
      />

        </>
      )}

      <div className="flex shrink-0 flex-wrap items-center gap-2 border-t border-neutral-800 bg-neutral-950 px-3 py-2" data-testid="target-review-actions">
{view === "grid" ? (
<>
        <ActionButton testid="target-reject" label="Set aside" keys={["X"]} tone="bad" disabled={gid == null || !list.includes(gid)} title={`Take the focused pick out of the delivery set. Nothing is rejected or deleted${hint("targetReject")}`} onClick={() => void gridAside()} />
        <ActionButton testid="target-add" label="Add too" keys={["A"]} disabled={gid == null || list.includes(gid)} title={`Deliver the focused alternative as well${hint("targetAdd")}`} onClick={() => void gridAdd()} />
        <ActionButton testid="target-next-moment" label="Accept row" keys={["⇧→"]} title={`Looks good: mark every pick of this row reviewed and go to the next moment${hint("targetNextMoment")}`} onClick={gridNextMoment} />
        <ActionButton testid="target-open-cell" label="Open" keys={["Enter"]} title="Open the focused photo in the two-up view, where S swaps" onClick={openCell} />
        <ActionButton testid="target-grid-toggle" label="Two-up" keys={["G"]} tone="primary" title={`Back to the two-up view${hint("targetGrid")}`} onClick={openCell} />
        <span className="ml-auto text-xs text-neutral-400">Arrows move · Up / Down: moment · Shift+→/↓: accept row · ] / [: next / previous unreviewed</span>
</>
) : (
<>
        <ActionButton testid="target-keep" label="Keep" keys={["Z"]} tone="good" title={`Keep this photo in the delivery set and go on${hint("targetKeep")}. Enter does the same`} onClick={() => void keep()} />
        <ActionButton testid="target-reject" label="Set aside" keys={["X"]} tone="bad" title={`Take it out of the delivery set. Nothing is rejected or deleted${hint("targetReject")}`} onClick={() => void reject()} />
        <ActionButton testid="target-swap" label="Swap in" keys={["S"]} tone="primary" disabled={!alt} title={`Use the shown alternative instead of this photo; this one becomes its alternative${hint("targetSwap")}`} onClick={() => void swap()} />
        <ActionButton testid="target-add" label="Add too" keys={["A"]} disabled={!alt} title={`Deliver the shown alternative as well${hint("targetAdd")}`} onClick={() => void add()} />
        <ActionButton testid="target-cycle" label="Next alternative" keys={["Tab"]} disabled={list2.length < 2} title={`Show the next alternative of this moment (Shift+Tab: previous)${hint("targetCycle")}`} onClick={() => cycle(1)} />
        <ActionButton testid="target-next-moment" label="Next moment" keys={["⇧→"]} title={`Looks good: mark every pick of this moment reviewed and go to the next moment${hint("targetNextMoment")}`} onClick={nextMoment} />
        <ActionButton testid="target-zoom" label={zoom.view.scale > 1.001 ? "Fit" : "100%"} keys={["Space"]} title={`Zoom both photos to 100% at the same point, or back to fit${hint("targetZoom")}`} onClick={() => zoom.toggle()} />
        <span className="ml-auto text-xs text-neutral-400">Left / Right: photo · ] / [: next / previous unreviewed</span>
        <ActionButton testid="target-grid-toggle" label="Moment grid" keys={["G"]} title={`Show every moment as a row: its picks big, its alternatives next to them${hint("targetGrid")}`} onClick={openGrid} />
</>
)}
      </div>
    </div>
  );
});
