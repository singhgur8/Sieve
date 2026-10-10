// Pass 2: a quick second look. The photos that are not delivered split into four piles by their first reason:
// Not sure (one at a time, by moment), Similar to a kept photo (one row per moment next to the kept photo), Weaker frames and
// Defects (plain grids, not part of the pass). Skip is the default and advances on its own after every action.
import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState, type ReactNode } from "react";
import { LayoutGrid } from "lucide-react";
import { commands, DEFAULT_QUERY, unwrap, type CoveredBy, type ImageSelection, type Moment, type RawImageEntry } from "../../ipc";
import { previewSrc } from "../../lib/entryImage";
import { hint } from "../../lib/keymap";
import { loadReviewed, num, ordinal, type Pile, PILE_HINT, PILE_LABEL, pileOf, PILES, saveReviewed, stemOf, type Pass } from "../../lib/target";
import { ActionButton, OverflowStrip, Pic, useFocusIntoView, Reasons, ShotBadge, useSyncedZoom, Windowed, ZoomPic } from "./bits";
import type { StageRef } from "./SetupStep";
import type { TargetCtx } from "./types";

interface Info {
  sel: ImageSelection | null;
  cov: CoveredBy | null;
}

/** Mirrors the engine's SIMILARITY_VERY_SIMILAR: below it a frame of another moment is not a near-duplicate. */
const VERY_SIMILAR = 0.7;
const CELL_W = 172;

async function selectionsOf(ids: number[]): Promise<ImageSelection[]> {
  const out: ImageSelection[] = [];
  for (let i = 0; i < ids.length; i += 500) out.push(...(await unwrap(commands.getImageSelections(ids.slice(i, i + 500)))));
  return out;
}

function Chip({ tone, children, testid }: { tone: "picked" | "notsure" | "aside" | "added"; children: ReactNode; testid?: string }) {
  const c = { picked: "bg-emerald-800 text-emerald-50", notsure: "bg-amber-800 text-amber-50", aside: "bg-neutral-700 text-neutral-100", added: "bg-sky-800 text-sky-50" }[tone];
  return (
    <span className={`shrink-0 rounded px-1.5 py-0.5 text-[11px] font-semibold ${c}`} data-testid={testid}>
      {children}
    </span>
  );
}

export const SecondStep = forwardRef<StageRef, { ctx: TargetCtx }>(function SecondStep({ ctx }, ref) {
  const { projectId, entries } = ctx;
  const [loaded, setLoaded] = useState(false);
  // Selection of every Not sure / Set aside row at load (the piles are fixed by it) and refreshed copies of the grid piles.
  const [sels, setSels] = useState<Map<number, ImageSelection>>(new Map());
  const [moments, setMoments] = useState<Moment[]>([]);
  const [pile, setPile] = useState<Pile>("not_sure");
  const [at, setAt] = useState(0);
  const [fid, setFid] = useState<number | null>(null);
  const [revNS, setRevNS] = useState(() => loadReviewed(projectId, "second"));
  const [revSim, setRevSim] = useState(() => loadReviewed(projectId, "second_similar"));
  const [info, setInfo] = useState<Map<number, Info>>(new Map());
  const [flash, setFlash] = useState<string | null>(null);
  const [peek, setPeek] = useState(false);
  const [large, setLarge] = useState(false);
  const [moreRow, setMoreRow] = useState<number | null>(null);
  const zoom = useSyncedZoom();
  const loading = useRef(new Set<number>());
  const busy = useRef(false);
  const initial = useRef<Map<number, ImageSelection>>(new Map());
  const stamp = ctx.run?.finishedAtMs ?? 0;

  // ---- load: ids, selections, moments ----
  useEffect(() => {
    let stale = false;
    void (async () => {
      try {
        const ids = await unwrap(commands.listImageIds({ ...DEFAULT_QUERY, projectId, targetChoices: ["not_sure", "set_aside"], sort: "target_moment" }));
        const [rows, ms] = await Promise.all([selectionsOf(ids), unwrap(commands.listMoments(projectId))]);
        if (stale) return;
        const byId = new Map(rows.map((r) => [r.imageId, r]));
        initial.current = new Map(ids.filter((i) => byId.has(i)).map((i) => [i, byId.get(i)!]));
        setSels(new Map(initial.current));
        setMoments(ms);
        setLoaded(true);
      } catch (e) {
        ctx.onError(e);
      }
    })();
    return () => {
      stale = true;
    };
  }, [projectId, stamp]); // eslint-disable-line react-hooks/exhaustive-deps

  const momentOf = useMemo(() => {
    const m = new Map<number, Moment>();
    moments.forEach((x) => x.imageIds.forEach((i) => m.set(i, x)));
    return m;
  }, [moments]);
  const keyOf = useCallback((i: number) => initial.current.get(i)?.momentId ?? momentOf.get(i)?.id ?? -i, [momentOf, loaded]); // eslint-disable-line react-hooks/exhaustive-deps

  // The piles (fixed at load), each ordered by moment then score.
  const piles = useMemo(() => {
    const out: Record<Pile, number[]> = { not_sure: [], similar: [], weaker: [], defects: [] };
    const rows = [...initial.current.values()];
    // The backend orders by moment, then score (sort "target_moment") and assigns the pile.
    rows.forEach((r) => out[r.pile ?? pileOf(r)].push(r.imageId));
    return out;
  }, [loaded, momentOf]); // eslint-disable-line react-hooks/exhaustive-deps
  const list = piles[pile];
  const simRows = useMemo(() => {
    const rows: { key: number; moment: Moment | undefined; frames: number[] }[] = [];
    const byKey = new Map<number, (typeof rows)[number]>();
    piles.similar.forEach((i) => {
      const k = keyOf(i);
      let r = byKey.get(k);
      if (!r) {
        r = { key: k, moment: momentOf.get(i), frames: [] };
        byKey.set(k, r);
        rows.push(r);
      }
      r.frames.push(i);
    });
    return rows;
  }, [piles, keyOf, momentOf]);
  const flat = useMemo(() => simRows.flatMap((r) => r.frames), [simRows]);
  const rowOfFrame = useMemo(() => {
    const m = new Map<number, number>();
    simRows.forEach((r, n) => r.frames.forEach((f) => m.set(f, n)));
    return m;
  }, [simRows]);

  const mark = useCallback(
    (pass: Pass, add: number[]) => {
      const set = pass === "second" ? setRevNS : setRevSim;
      set((s) => {
        const fresh = add.filter((i) => !s.has(i));
        if (fresh.length === 0) return s;
        const n = new Set(s);
        fresh.forEach((i) => n.add(i));
        saveReviewed(projectId, pass, n);
        return n;
      });
    },
    [projectId],
  );
  const unmark = useCallback(
    (id: number) => {
      for (const [pass, set] of [["second", setRevNS], ["second_similar", setRevSim]] as const)
        set((s) => {
          if (!s.has(id)) return s;
          const n = new Set(s);
          n.delete(id);
          saveReviewed(projectId, pass, n);
          return n;
        });
    },
    [projectId],
  );

  // Open a pile at its first unreviewed photo (or moment).
  const open = useCallback(
    (p: Pile) => {
      setPile(p);
      if (p === "not_sure") {
        const r = loadReviewed(projectId, "second");
        const first = piles.not_sure.findIndex((i) => !r.has(i));
        setAt(first >= 0 ? first : 0);
      } else {
        const r = loadReviewed(projectId, "second_similar");
        const f = p === "similar" ? (flat.find((i) => !r.has(i)) ?? flat[0]) : piles[p][0];
        setFid(f ?? null);
      }
    },
    [projectId, piles, flat],
  );
  useEffect(() => setLarge(false), [pile]);
  const opened = useRef(false);
  useEffect(() => {
    if (loaded && !opened.current) {
      opened.current = true;
      open("not_sure");
    }
  }, [loaded, open]);

  // Undo moved here (P1-3): switch to the pile of the photo, select it and take its reviewed mark away.
  useEffect(() => {
    if (!ctx.focus || ctx.focus.stage !== "second" || !loaded) return;
    const id = ctx.focus.id;
    const p = (Object.keys(piles) as Pile[]).find((k) => piles[k].includes(id));
    ctx.focusDone();
    if (!p) return;
    unmark(id);
    setPile(p);
    if (p === "not_sure") setAt(Math.max(0, piles.not_sure.indexOf(id)));
    else setFid(id);
  }, [ctx.focus, loaded]); // eslint-disable-line react-hooks/exhaustive-deps

  // ---- refreshed data after every edit: moments (kept photos), the current pile's selections, covered-by ----
  useEffect(() => {
    setInfo(new Map());
    loading.current.clear();
    if (!loaded) return;
    let stale = false;
    unwrap(commands.listMoments(projectId))
      .then((ms) => !stale && setMoments(ms))
      .catch(ctx.onError);
    if (pile !== "not_sure" && piles[pile].length > 0)
      selectionsOf(piles[pile])
        .then((rows) => !stale && setSels((m) => new Map([...m, ...rows.map((r) => [r.imageId, r] as const)])))
        .catch(ctx.onError);
    return () => {
      stale = true;
    };
  }, [ctx.rev, pile, loaded]); // eslint-disable-line react-hooks/exhaustive-deps

  const cur = pile === "not_sure" ? (list[at] ?? null) : null;
  const target = pile === "not_sure" ? cur : fid;
  const loadInfo = useCallback(
    (want: number[]) => {
      const todo = want.filter((i) => !info.has(i) && !loading.current.has(i));
      if (todo.length === 0) return;
      todo.forEach((i) => loading.current.add(i));
      void (async () => {
        try {
          const rows = await unwrap(commands.getImageSelections(todo));
          const covs = await Promise.all(todo.map((i) => unwrap(commands.getCoveredBy(i))));
          const covIds = covs.filter((c): c is CoveredBy => !!c).map((c) => c.coveredById);
          entries.need([...todo, ...covIds]);
          setInfo((m) => {
            const n = new Map(m);
            todo.forEach((i, k) => n.set(i, { sel: rows.find((s) => s.imageId === i) ?? null, cov: covs[k] }));
            return n;
          });
        } catch (e) {
          ctx.onError(e);
        } finally {
          todo.forEach((i) => loading.current.delete(i));
        }
      })();
    },
    [info, entries, ctx.onError], // eslint-disable-line react-hooks/exhaustive-deps
  );
  useEffect(() => {
    if (cur == null) return;
    loadInfo(list.slice(at, at + 4));
    entries.need(list.slice(at, at + 4));
    // Prefetch the previews of the next photos and of their kept twins.
    list.slice(at + 1, at + 4).forEach((i) => {
      const s = previewSrc(entries.get(i));
      if (s) new Image().src = s;
      const c = info.get(i)?.cov;
      const cs = c ? previewSrc(entries.get(c.coveredById)) : null;
      if (cs) new Image().src = cs;
    });
  });

  // The kept photos of the current moment and what the user added (P0-3).
  const here = cur != null ? info.get(cur) : undefined;
  const cov = here?.cov ?? null;
  const curMoment = target != null ? momentOf.get(target) : undefined;
  const keptIds = curMoment?.deliveredIds ?? [];
  const userIds = new Set(curMoment?.userDeliveredIds ?? []);
  // R1-1: a cover from another moment is not a duplicate: it never offers the swap and shows small unless it is very similar.
  const cross = !!cov && cov.tier === "another_moment";
  const strong = !!cov && (cov.sameMoment || cov.similarity >= VERY_SIMILAR);
  const showCover = !!cov && (strong || peek);
  useEffect(() => setPeek(false), [cur]);
  useEffect(() => {
    entries.need(keptIds);
  }, [keptIds.join(",")]); // eslint-disable-line react-hooks/exhaustive-deps
  const addedIds = keptIds.filter((i) => i !== target && userIds.has(i));

  const entry = cur != null ? entries.get(cur) : undefined;
  const covEntry = cov ? entries.get(cov.coveredById) : undefined;
  const kept = here?.sel?.choice === "deliver";

  // ---- progress ----
  const doneNS = useMemo(() => piles.not_sure.filter((i) => revNS.has(i)).length, [piles, revNS]);
  const doneSim = useMemo(() => simRows.filter((r) => r.frames.every((f) => revSim.has(f))).length, [simRows, revSim]);

  // ---- navigation: Not sure ----
  const noteFlash = (msg = "Nothing kept in this moment to swap with") => {
    setFlash(msg);
    setTimeout(() => setFlash(null), 2200);
  };
  const jumpNS = (to: number, markCur = true) => {
    if (list.length === 0) return;
    if (markCur && cur != null) mark("second", [cur]);
    setAt(Math.max(0, Math.min(list.length - 1, to)));
  };
  const nextMomentNS = () => {
    if (cur == null) return;
    const k = keyOf(cur);
    const rest = list.filter((i, n) => n >= at && keyOf(i) === k);
    mark("second", rest);
    const j = list.findIndex((i, n) => n > at && keyOf(i) !== k);
    if (j >= 0) setAt(j);
  };
  const prevMomentNS = () => {
    if (cur == null) return;
    const k = keyOf(cur);
    let j = at;
    while (j >= 0 && keyOf(list[j]) === k) j--;
    if (j < 0) return;
    const pk = keyOf(list[j]);
    while (j > 0 && keyOf(list[j - 1]) === pk) j--;
    jumpNS(j);
  };
  const openNS = (d: 1 | -1) => {
    for (let n = at + d; n >= 0 && n < list.length; n += d) if (!revNS.has(list[n])) return jumpNS(n);
  };
  const advance = () => jumpNS(at + 1);

  // ---- navigation: Similar / grids ----
  useFocusIntoView(pile === "similar" && fid != null ? `target-sim-frame-${fid}` : null);
  const flatList = pile === "similar" ? flat : pile === "not_sure" ? [] : list;
  const fAt = fid != null ? flatList.indexOf(fid) : -1;
  const moveF = (to: number) => {
    if (flatList.length === 0) return;
    const nf = flatList[Math.max(0, Math.min(flatList.length - 1, to))];
    if (pile === "similar" && fid != null && rowOfFrame.get(nf) !== rowOfFrame.get(fid)) mark("second_similar", simRows[rowOfFrame.get(fid)!].frames);
    setFid(nf);
  };
  const rowMove = (d: number) => {
    if (pile === "similar") {
      const r = (fid != null ? rowOfFrame.get(fid) : 0) ?? 0;
      const to = simRows[Math.max(0, Math.min(simRows.length - 1, r + d))];
      if (to && to !== simRows[r]) moveF(flatList.indexOf(to.frames[0]));
    } else {
      const cols = Math.max(1, Math.floor(gridW / CELL_W));
      moveF(fAt + d * cols);
    }
  };
  const [gridW, setGridW] = useState(900);
  const nextMomentSim = () => {
    if (fid == null) return;
    const r = rowOfFrame.get(fid)!;
    mark("second_similar", simRows[r].frames);
    if (simRows[r + 1]) setFid(simRows[r + 1].frames[0]);
  };
  const prevMomentSim = () => {
    if (fid == null) return;
    const r = rowOfFrame.get(fid)!;
    if (simRows[r - 1]) moveF(flatList.indexOf(simRows[r - 1].frames[0]));
  };
  const openSim = (d: 1 | -1) => {
    for (let n = fAt + d; n >= 0 && n < flat.length; n += d) if (!revSim.has(flat[n])) return moveF(n);
  };

  // ---- actions ----
  const act = async (fn: () => Promise<boolean>) => {
    if (busy.current) return;
    busy.current = true;
    try {
      if (await fn()) {
        if (pile === "not_sure") advance();
        else moveF(fAt + 1);
      }
    } finally {
      busy.current = false;
    }
  };
  const targetEntry = target != null ? entries.get(target) : undefined;
  /** The kept photo a swap replaces: the covered-by one in Not sure, else the moment's kept photo nearest to the frame. */
  const swapWith = (): number | null => {
    if (pile === "not_sure") return cross ? null : (cov?.coveredById ?? null);
    if (target == null) return null;
    const m = momentOf.get(target);
    const cb = sels.get(target)?.coveredBy;
    return cb != null && m?.deliveredIds.includes(cb) ? cb : (m?.deliveredIds[0] ?? null);
  };
  const swap = () =>
    act(async () => {
      if (target == null) return false;
      if (pile === "not_sure" && cross && cov) return noteFlash(`${cov.coveredByName} is the pick of another moment. A keeps this one too`), false;
      const other = swapWith();
      if (other == null || other === target) return noteFlash(), false;
      entries.need([other]);
      const r = await ctx.edit(`Swap ${stemOf(entries.get(other), other)} for ${stemOf(targetEntry, target)}`, unwrap(commands.swapAlternative(other, target)), target);
      if (r) ctx.notify(`Kept ${stemOf(targetEntry, target)} instead of ${stemOf(entries.get(other), other)}. Cmd+Z undoes it`);
      return !!r;
    });
  const hasKeptNear = pile === "not_sure" ? !!cov && !cross : target != null && !!momentOf.get(target)?.deliveredIds.length;
  const add = () =>
    act(async () => {
      if (target == null) return false;
      const both = pile !== "defects" && pile !== "weaker" && hasKeptNear;
      const label = both ? "Add both" : "Keep";
      const r = await ctx.edit(`${label} ${stemOf(targetEntry, target)}`, both ? unwrap(commands.addAlternative(target)) : unwrap(commands.setTargetChoice([target], "deliver")), target);
      if (r) ctx.notify(both ? `Kept ${stemOf(targetEntry, target)} as well. Cmd+Z undoes it` : `Kept ${stemOf(targetEntry, target)}. Cmd+Z undoes it`);
      return !!r;
    });
  /** X (P2-5): Not sure -> Set aside and on. */
  const asideNow = () =>
    act(async () => {
      if (pile !== "not_sure" || cur == null || here?.sel?.choice !== "not_sure") return false;
      const r = await ctx.edit(`Set aside ${stemOf(entry, cur)}`, unwrap(commands.setTargetChoice([cur], "set_aside")), cur);
      if (r) ctx.notify(`Set aside ${stemOf(entry, cur)}. Cmd+Z undoes it`);
      return !!r;
    });

  useImperativeHandle(
    ref,
    () => ({
      key: (id, e) => {
        const ns = pile === "not_sure";
        const sim = pile === "similar";
        switch (id) {
          case "targetPrev":
            return ns ? jumpNS(at - 1, false) : moveF(fAt - 1), true;
          case "targetNext":
            return ns ? jumpNS(at + 1, false) : moveF(fAt + 1), true;
          case "targetNextMoment":
          case "targetSkipMoment":
            return ns ? nextMomentNS() : sim ? nextMomentSim() : undefined, true;
          case "targetPrevMoment":
            return ns ? prevMomentNS() : sim ? prevMomentSim() : undefined, true;
          case "targetNextOpen":
            return ns ? openNS(1) : sim ? openSim(1) : undefined, true;
          case "targetPrevOpen":
            return ns ? openNS(-1) : sim ? openSim(-1) : undefined, true;
          case "targetFirst":
            return ns ? jumpNS(0, false) : moveF(0), true;
          case "targetLast":
            return ns ? jumpNS(list.length - 1, false) : moveF(flatList.length - 1), true;
          case "targetCycle":
            return ns ? undefined : rowMove(e.key === "ArrowUp" || (e.key === "Tab" && e.shiftKey) ? -1 : 1), true;
          case "targetSkip":
          case "targetEnter":
            return ns ? cur != null && advance() : moveF(fAt + 1), true;
          case "targetSwap":
            return void swap(), true;
          case "targetAdd":
          case "targetKeep":
            return void add(), true;
          case "targetReject":
            return void asideNow(), true;
          case "targetLarge":
            return sim && fid != null && setLarge((v) => !v), true;
          case "targetClose":
            return large ? (setLarge(false), true) : false;
          default:
            return false;
        }
      },
    }),
    [pile, at, fid, cur, cov, list, flat, simRows, piles, revNS, revSim, keptIds.length, here, gridW, large, cross], // eslint-disable-line react-hooks/exhaustive-deps
  );

  if (!loaded) return <div className="p-6 text-sm text-neutral-400">Loading…</div>;
  const total = list.length;
  const all = PILES.reduce((n, p) => n + piles[p].length, 0);
  if (all === 0)
    return (
      <div className="mx-auto max-w-xl p-8 text-center text-sm text-neutral-300" data-testid="target-second-empty">
        Nothing to look at twice: no photos are Not sure or Set aside.
      </div>
    );

  const done = pile === "not_sure" ? doneNS : pile === "similar" ? doneSim : 0;
  const denom = pile === "not_sure" ? total : pile === "similar" ? simRows.length : 0;
  const pct = denom > 0 ? Math.round((done / denom) * 100) : 0;
  const sim = cov ? Math.round(cov.similarity * 100) : null;
  const nextN = keptIds.length + 1;
  // R1-9: the kept photo the large view compares the focused frame with (its nearest one, else the moment's first).
  const fMoment = fid != null ? momentOf.get(fid) : undefined;
  const fCover = fid != null ? sels.get(fid)?.coveredBy : null;
  const largeKept = fid == null ? null : fCover != null && fMoment?.deliveredIds.includes(fCover) ? fCover : (fMoment?.deliveredIds[0] ?? null);
  if (largeKept != null) entries.need([largeKept]);
  const hasAdded = addedIds.length > 0;
  const addedName = hasAdded ? stemOf(entries.get(addedIds[addedIds.length - 1]), addedIds[addedIds.length - 1]) : "";
  const gridShow = pile === "similar" ? (["set_aside"] as const) : pile === "not_sure" ? (["not_sure"] as const) : (["set_aside"] as const);

  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="target-second" data-pile={pile} data-current={pile === "not_sure" ? (cur ?? "") : (fid ?? "")} data-total={total} data-rows={pile === "similar" ? simRows.length : ""} data-reviewed={done}>
      <div className="flex shrink-0 flex-wrap items-center gap-3 border-b border-neutral-800 px-4 py-2 text-sm">
        <div className="flex items-center gap-1" role="tablist" aria-label="Piles" data-testid="target-piles">
          {PILES.map((p) => (
            <button
              key={p}
              role="tab"
              aria-selected={pile === p}
              className={`flex h-8 items-center gap-1.5 rounded-md px-2.5 text-xs ${pile === p ? "bg-sky-800 text-white" : "bg-neutral-800 text-neutral-200 hover:bg-neutral-700"}`}
              data-testid={`target-pile-${p}`}
              data-active={pile === p}
              data-count={piles[p].length}
              title={PILE_HINT[p]}
              onClick={() => open(p)}
            >
              {PILE_LABEL[p]} <b>{num(piles[p].length)}</b>
            </button>
          ))}
        </div>
        {denom > 0 && (
          <>
            <span className="font-medium text-neutral-100" data-testid="target-second-count" title="What you have looked at in this pile">
              {num(done)} of {num(denom)} {pile === "similar" ? "moments " : ""}reviewed
            </span>
            <div className="h-1.5 w-32 overflow-hidden rounded bg-neutral-800" role="progressbar" aria-valuenow={pct} aria-label="Second look progress">
              <div className="h-full bg-emerald-500" style={{ width: `${pct}%` }} />
            </div>
          </>
        )}
        {pile === "not_sure" && total > 0 && (
          <span className="text-xs text-neutral-400" data-testid="target-second-position">
            Photo {num(at + 1)} of {num(total)}
          </span>
        )}
        <button className="ml-auto flex h-7 items-center gap-1.5 rounded bg-neutral-800 px-2 text-xs hover:bg-neutral-700" data-testid="target-show-rest" title="Close this view and show these photos in the grid" onClick={() => ctx.showInGrid([...gridShow])}>
          <LayoutGrid className="size-3.5" /> Show in grid
        </button>
      </div>
      {(pile === "defects" || pile === "weaker") && (
        <p className="shrink-0 bg-neutral-900 px-4 py-1.5 text-xs text-neutral-300" data-testid="target-pile-note">
          Defects are rejected when you apply. Weaker frames stay unflagged.
        </p>
      )}
      {pile === "not_sure" && done >= total && total > 0 && (
        <p className="shrink-0 bg-emerald-950 px-4 py-1.5 text-xs text-emerald-100" data-testid="target-second-complete">
          Second look done. Apply the flags from the top right when you are happy.
        </p>
      )}

      {pile === "not_sure" &&
        (total === 0 ? (
          <div className="flex flex-1 items-center justify-center p-8 text-center text-sm text-neutral-300" data-testid="target-pile-empty">
            No close calls: nothing is Not sure. Look at the other piles above if you want to.
          </div>
        ) : (
          <>
            <div className="flex min-h-0 flex-1 gap-3 p-3">
              <figure className="relative flex min-w-0 flex-1 flex-col" data-testid="target-second-cur" data-id={cur ?? ""}>
                <ZoomPic entry={entry} zoom={zoom} primary className="min-h-0 flex-1 rounded-lg" testid="target-second-pic" />
                <div className="pointer-events-none absolute left-2 top-2 flex items-center gap-1.5">
                  <ShotBadge type={here?.sel?.shotType ?? null} />
                </div>
                <figcaption className="mt-2 flex items-start gap-3 text-xs">
                  <span className="flex shrink-0 items-center gap-2 text-neutral-200">
                    {entry?.fileName ?? ""}
                    {here?.sel && (
                      <span data-testid="target-second-choice">
                        <Chip tone={kept ? "picked" : here.sel.choice === "not_sure" ? "notsure" : "aside"}>{kept ? "Kept" : here.sel.choice === "not_sure" ? "Not sure" : "Set aside"}</Chip>
                      </span>
                    )}
                  </span>
                  <span className="min-w-0 text-neutral-300">
                    <Reasons sel={here?.sel ?? undefined} />
                  </span>
                  {cov && !strong && (
                    <button
                      className="ml-auto flex shrink-0 items-center gap-2 rounded bg-neutral-900 p-1 text-left hover:bg-neutral-800"
                      data-testid="target-cover-thumb"
                      data-peek={peek}
                      title={peek ? "Hide the kept photo" : "Show the two photos side by side"}
                      onClick={() => setPeek((v) => !v)}
                    >
                      <Pic entry={covEntry} className="h-8 w-12 shrink-0 rounded" />
                      <span className="text-[11px] text-neutral-300" data-testid="target-cover-note">
                        {cov.text} · {sim}%
                      </span>
                    </button>
                  )}
                </figcaption>
              </figure>

              {cov && showCover && (
                <figure className="relative flex min-w-0 flex-1 flex-col" data-testid="target-covered" data-id={cov.coveredById} data-similarity={cov.similarity}>
                  <ZoomPic entry={covEntry} zoom={zoom} className="min-h-0 flex-1 rounded-lg ring-1 ring-emerald-700" testid="target-covered-pic" />
                  <figcaption className="mt-2 text-xs">
                    <div className="flex items-center gap-3">
                      <Chip tone="picked">Picked</Chip>
                      <span className="font-medium text-neutral-100" data-testid="target-covered-text">
                        {cov.text}
                      </span>
                    </div>
                    <div className="mt-1 text-neutral-400" data-testid="target-similarity">
                      {sim}% similar{cov.sameMoment ? ", same moment" : ""}
                    </div>
                  </figcaption>
                </figure>
              )}
            </div>
            {!cov && (
              <p className="shrink-0 px-4 pb-1 text-xs text-neutral-400" data-testid="target-no-cover">
                {kept ? "Kept." : "No similar photo is kept. Keep this one, or skip."}
              </p>
            )}
            <div className="shrink-0 border-t border-neutral-800 px-3 py-2" data-testid="target-kept-strip" data-count={keptIds.length}>
              <div className="mb-1 flex flex-wrap items-center gap-x-3 text-[11px] text-neutral-400">
                <span className="uppercase tracking-wide">Kept from this moment ({keptIds.length})</span>
                {hasAdded && (
                  <span className="text-xs normal-case text-amber-200" data-testid="target-already-added">
                    You already added {addedName} from this moment{cov ? `. Already kept: ${stemOf(covEntry, cov.coveredById)} (${sim}%)` : ""}
                  </span>
                )}
              </div>
              {keptIds.length === 0 ? (
                <p className="text-xs text-neutral-500">Nothing from this moment is kept yet.</p>
              ) : (
                <ul className="flex gap-2 overflow-x-auto pb-1">
                  {keptIds.map((k) => {
                    const added = userIds.has(k);
                    return (
                      <li key={k} className="shrink-0" data-testid={`target-kept-${k}`} data-added={added} title={`${stemOf(entries.get(k), k)}: ${added ? "you added this" : "kept by Sieve"}`}>
                        <Pic entry={entries.get(k)} className={`h-12 w-16 rounded ${added ? "ring-2 ring-sky-400" : "ring-1 ring-emerald-700"}`} />
                        {added && <div className="text-center text-[10px] text-sky-300">you added</div>}
                      </li>
                    );
                  })}
                </ul>
              )}
            </div>
            <div className="flex shrink-0 flex-wrap items-center gap-2 border-t border-neutral-800 bg-neutral-950 px-3 py-2" data-testid="target-second-actions">
              <ActionButton testid="target-skip" label="Skip" keys={["Space"]} tone="primary" title={`Leave it as it is and go to the next photo${hint("targetSkip")}. Enter does the same`} onClick={() => cur != null && advance()} />
              <ActionButton testid="target-skip-moment" label="Skip moment" keys={["⇧Space"]} title={`Mark the rest of this moment reviewed and go to the next moment${hint("targetSkipMoment")}`} onClick={nextMomentNS} />
              {cov && (
                <ActionButton
                  testid="target-second-swap"
                  label={`Swap: keep this instead of ${stemOf(covEntry, cov.coveredById)}`}
                  keys={["S"]}
                  disabled={cross}
                  title={cross ? `${cov.coveredByName} is the pick of another moment, so there is nothing to swap. A keeps this photo too` : `Deliver this photo instead of the similar one that is kept; that one becomes its alternative${hint("targetSwap")}`}
                  onClick={() => void swap()}
                />
              )}
              {hasAdded ? (
                <button
                  className="flex h-9 items-center gap-2 whitespace-nowrap rounded-md bg-amber-700 px-3 text-sm text-white hover:bg-amber-600"
                  data-testid="target-second-add"
                  data-amber="true"
                  title={`You already added ${addedName} from this moment. Keep this photo as well${hint("targetAdd")}`}
                  onClick={() => void add()}
                >
                  Add a {ordinal(nextN)} from this moment <kbd className="rounded border border-amber-300 bg-amber-800 px-1 text-[10px] font-semibold">A</kbd>
                </button>
              ) : cov && !cross ? (
                <ActionButton testid="target-second-add" label="Add both" keys={["A"]} tone="good" title={`Keep this photo as well as the similar one${hint("targetAdd")}`} onClick={() => void add()} />
              ) : (
                <ActionButton testid="target-second-keep" label="Keep" keys={["A", "Z"]} tone="good" disabled={kept} title={`Add this photo to the delivery set${hint("targetKeep")}`} onClick={() => void add()} />
              )}
              <ActionButton testid="target-second-aside" label="Set aside" keys={["X"]} tone="bad" disabled={here?.sel?.choice !== "not_sure"} title={`Take it out for good: set it aside and go on. Nothing is rejected${hint("targetReject")}`} onClick={() => void asideNow()} />
              <span className={`ml-auto text-xs ${flash ? "font-semibold text-amber-300" : "text-neutral-400"}`} data-testid="target-second-hint">
                {flash ?? "Left: back · click a photo to zoom both · ] / [: next / previous unreviewed"}
              </span>
            </div>
          </>
        ))}

      {pile === "similar" &&
        (simRows.length === 0 ? (
          <div className="flex flex-1 items-center justify-center p-8 text-sm text-neutral-300" data-testid="target-pile-empty">
            No frames look like a kept photo.
          </div>
        ) : (
          <>
            {large && fid != null ? (
              <div className="flex min-h-0 flex-1 gap-3 p-3" data-testid="target-sim-large" data-id={fid}>
                <figure className="relative flex min-w-0 flex-1 flex-col">
                  <ZoomPic entry={entries.get(fid)} zoom={zoom} primary className="min-h-0 flex-1 rounded-lg" testid="target-sim-large-pic" />
                  <figcaption className="mt-2 flex items-center gap-3 text-xs">
                    <Chip tone={sels.get(fid)?.choice === "deliver" ? "picked" : "aside"}>{sels.get(fid)?.choice === "deliver" ? "Kept" : "Set aside"}</Chip>
                    <span className="text-neutral-200">{stemOf(entries.get(fid), fid)}</span>
                    <span className="min-w-0 text-neutral-300">{sels.get(fid)?.reasons[0]?.text}</span>
                  </figcaption>
                </figure>
                {largeKept != null && (
                  <figure className="relative flex min-w-0 flex-1 flex-col" data-id={largeKept}>
                    <ZoomPic entry={entries.get(largeKept)} zoom={zoom} className="min-h-0 flex-1 rounded-lg ring-1 ring-emerald-700" testid="target-sim-large-kept" />
                    <figcaption className="mt-2 flex items-center gap-3 text-xs">
                      <Chip tone="picked">Picked</Chip>
                      <span className="text-neutral-200">{stemOf(entries.get(largeKept), largeKept)}</span>
                      <span className="text-neutral-400">{sels.get(fid)?.coveredSimilarity != null ? `${Math.round(sels.get(fid)!.coveredSimilarity! * 100)}% similar` : ""}</span>
                    </figcaption>
                  </figure>
                )}
              </div>
            ) : (
              <Windowed
                testid="target-similar"
                count={simRows.length}
                focusRow={fid != null ? (rowOfFrame.get(fid) ?? -1) : -1}
                renderRow={(n) => {
                  const r = simRows[n];
                  if (!r) return null;
                  const keptHere = r.moment?.deliveredIds ?? [];
                  const added = new Set(r.moment?.userDeliveredIds ?? []);
                  // R1-3: up to 3 kept tiles (the ones you added first), then "+N more".
                  const shown = [...keptHere.filter((k) => added.has(k)), ...keptHere.filter((k) => !added.has(k))].slice(0, 3);
                  const more = keptHere.filter((k) => !shown.includes(k));
                  entries.need([...keptHere, ...r.frames]);
                  const reviewedRow = r.frames.every((f) => revSim.has(f));
                  return (
                    <div className={`flex h-full items-start gap-4 border-b border-neutral-900 px-4 py-2 ${reviewedRow ? "opacity-60" : ""}`} data-testid={`target-sim-row-${n}`} data-moment={r.key} data-reviewed={reviewedRow}>
                      <div className="flex shrink-0 gap-2 p-1" data-testid={`target-sim-kept-${n}`} data-count={keptHere.length}>
                        {keptHere.length === 0 ? (
                          <div className="flex h-[112px] w-40 items-center justify-center rounded bg-neutral-900 text-[11px] text-neutral-500">No kept photo</div>
                        ) : (
                          shown.map((k) => {
                            const mine = added.has(k);
                            return (
                              <div key={k} className="w-40" data-testid="target-sim-kept-photo" data-id={k} data-added={mine}>
                                <Pic entry={entries.get(k)} className={`h-[112px] w-40 rounded ${mine ? "ring-2 ring-sky-400" : "ring-2 ring-emerald-700"}`} />
                                <div className="mt-1 flex items-center gap-1 text-[11px]">
                                  <Chip tone={mine ? "added" : "picked"}>{mine ? "you added" : "Picked"}</Chip>
                                  <span className="truncate text-neutral-300">{stemOf(entries.get(k), k)}</span>
                                </div>
                              </div>
                            );
                          })
                        )}
                        {more.length > 0 && (
                          <div className="relative w-40" onMouseEnter={() => setMoreRow(n)} onMouseLeave={() => setMoreRow((m) => (m === n ? null : m))}>
                            <button
                              className="flex h-[112px] w-40 items-center justify-center rounded bg-neutral-900 text-sm font-semibold text-neutral-200 ring-1 ring-emerald-800 hover:bg-neutral-800"
                              data-testid={`target-sim-kept-more-${n}`}
                              title="Show all the kept photos of this moment"
                              onClick={() => setMoreRow(n)}
                            >
                              +{more.length} more
                            </button>
                            {moreRow === n && (
                              <div className="absolute left-full top-0 z-20 ml-1 flex w-72 flex-wrap gap-1 rounded border border-neutral-600 bg-neutral-900 p-2 shadow-xl" data-testid={`target-sim-kept-all-${n}`}>
                                {keptHere.map((k) => (
                                  <div key={k} title={stemOf(entries.get(k), k)} data-id={k}>
                                    <Pic entry={entries.get(k)} className={`size-16 rounded ${added.has(k) ? "ring-2 ring-sky-400" : "ring-1 ring-emerald-700"}`} />
                                  </div>
                                ))}
                              </div>
                            )}
                          </div>
                        )}
                      </div>
                      <div className="mt-1 h-[112px] w-px shrink-0 bg-neutral-700" />
                      <OverflowStrip testid={`target-sim-frames-${n}`}>
                        {r.frames.map((f) => {
                          const s = sels.get(f);
                          const on = f === fid;
                          const nowKept = s?.choice === "deliver";
                          const near = keptHere.length >= 2 && s?.coveredBy != null && s.coveredSimilarity != null ? s.coveredBy : null;
                          if (near != null) entries.need([near]);
                          return (
                            <button
                              key={f}
                              className={`w-40 shrink-0 rounded text-left ${on ? "outline outline-2 outline-offset-2 outline-sky-400" : ""}`}
                              data-testid={`target-sim-frame-${f}`}
                              data-focused={on}
                              data-kept={nowKept}
                              title={`${stemOf(entries.get(f), f)}: ${s?.reasons[0]?.text ?? ""}. A adds it, S swaps it with the kept photo, E shows it large`}
                              onClick={() => setFid(f)}
                            >
                              <Pic entry={entries.get(f)} className={`h-[112px] w-40 rounded ${nowKept ? "ring-2 ring-emerald-700" : ""}`} />
                              <div className={`mt-1 flex items-center gap-1 rounded px-0.5 text-[11px] text-neutral-300 ${on ? "bg-sky-950" : ""}`}>
                                {nowKept ? (
                                  <Chip tone="picked">Kept</Chip>
                                ) : (
                                  <span className="shrink-0" data-testid="target-sim-pct">
                                    {s?.coveredSimilarity != null ? `${Math.round(s.coveredSimilarity * 100)}%` : ""}
                                    {near != null ? ` · like ${stemOf(entries.get(near), near)}` : ""}
                                  </span>
                                )}
                                <span className="truncate text-neutral-400">{stemOf(entries.get(f), f)}</span>
                              </div>
                            </button>
                          );
                        })}
                      </OverflowStrip>
                    </div>
                  );
                }}
              />
            )}
            <div className="flex shrink-0 flex-wrap items-center gap-2 border-t border-neutral-800 bg-neutral-950 px-3 py-2" data-testid="target-second-actions">
              {hasAdded && fid != null && sels.get(fid)?.choice !== "deliver" ? (
                <>
                  <span className="text-xs text-amber-200" data-testid="target-already-added">
                    You already added {addedName} from this moment
                  </span>
                  <button
                    className="flex h-9 items-center gap-2 whitespace-nowrap rounded-md bg-amber-700 px-3 text-sm text-white hover:bg-amber-600"
                    data-testid="target-second-add"
                    data-amber="true"
                    title={`You already added ${addedName} from this moment. Keep the highlighted frame as well${hint("targetAdd")}`}
                    onClick={() => void add()}
                  >
                    Add a {ordinal(nextN)} from this moment <kbd className="rounded border border-amber-300 bg-amber-800 px-1 text-[10px] font-semibold">A</kbd>
                  </button>
                </>
              ) : (
                <ActionButton testid="target-sim-add" label="Add it too" keys={["A"]} tone="good" disabled={fid == null || sels.get(fid)?.choice === "deliver"} title={`Keep the highlighted frame as well as the kept photo${hint("targetAdd")}`} onClick={() => void add()} />
              )}
              <ActionButton testid="target-sim-swap" label="Swap with the kept photo" keys={["S"]} tone="primary" disabled={fid == null || sels.get(fid)?.choice === "deliver"} title={`Deliver the highlighted frame instead of the kept photo of this moment${hint("targetSwap")}`} onClick={() => void swap()} />
              <ActionButton testid="target-sim-large-toggle" label={large ? "Back to rows" : "Large view"} keys={["E"]} title={`Show the highlighted frame large next to its nearest kept photo; click a photo to zoom both${hint("targetLarge")}. E or Esc goes back`} onClick={() => fid != null && setLarge((v) => !v)} />
              <ActionButton testid="target-sim-next-moment" label="Next moment" keys={["⇧→"]} title={`Mark this row reviewed and go to the next moment${hint("targetNextMoment")}`} onClick={nextMomentSim} />
              <span className={`ml-auto text-xs ${flash ? "font-semibold text-amber-300" : "text-neutral-400"}`}>Arrows move · Up / Down: row · E: large view · ] / [: next / previous unreviewed</span>
            </div>
          </>
        ))}

      {(pile === "weaker" || pile === "defects") &&
        (total === 0 ? (
          <div className="flex flex-1 items-center justify-center p-8 text-sm text-neutral-300" data-testid="target-pile-empty">
            Nothing here.
          </div>
        ) : (
          <>
            <Windowed
              testid="target-grid"
              onWidth={setGridW}
              count={Math.ceil(total / Math.max(1, Math.floor(gridW / CELL_W)))}
              focusRow={fid != null ? Math.floor(fAt / Math.max(1, Math.floor(gridW / CELL_W))) : -1}
              renderRow={(n) => {
                const cols = Math.max(1, Math.floor(gridW / CELL_W));
                const ids = list.slice(n * cols, n * cols + cols);
                entries.need(ids);
                return (
                  <div className="flex gap-2 px-4 py-2">
                    {ids.map((f) => {
                      const s = sels.get(f);
                      const e: RawImageEntry | undefined = entries.get(f);
                      const nowKept = s?.choice === "deliver";
                      return (
                        <button
                          key={f}
                          className={`w-40 shrink-0 text-left ${f === fid ? "rounded ring-2 ring-sky-500" : ""}`}
                          data-testid={`target-cell-${f}`}
                          data-focused={f === fid}
                          data-kind={s?.reasons[0]?.kind}
                          title={`${stemOf(e, f)}: ${s?.reasons[0]?.text ?? ""}. A keeps it`}
                          onClick={() => setFid(f)}
                        >
                          <Pic entry={e} className={`h-[112px] w-40 rounded ${nowKept ? "ring-2 ring-emerald-700" : ""}`} />
                          <div className="mt-1 text-[11px] leading-4">
                            {nowKept ? <Chip tone="picked">Kept</Chip> : <Chip tone="aside">Set aside</Chip>}
                            <span className="ml-1 line-clamp-2 text-neutral-300">{s?.reasons[0]?.text}</span>
                          </div>
                        </button>
                      );
                    })}
                  </div>
                );
              }}
            />
            <div className="flex shrink-0 flex-wrap items-center gap-2 border-t border-neutral-800 bg-neutral-950 px-3 py-2" data-testid="target-second-actions">
              <ActionButton testid="target-grid-keep" label="Keep it" keys={["A"]} tone="good" disabled={fid == null || sels.get(fid)?.choice === "deliver"} title={`Add the highlighted photo to the delivery set${hint("targetAdd")}`} onClick={() => void add()} />
              <span className="ml-auto text-xs text-neutral-400">Arrows move · Up / Down: row. These piles are not part of the pass</span>
            </div>
          </>
        ))}
    </div>
  );
});
