// Pass 2: a quick second look at Not sure + Set aside. Each photo shows the similar one that is already kept; skip is the default
// and advances on its own after every action.
import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState } from "react";
import { LayoutGrid } from "lucide-react";
import { commands, DEFAULT_QUERY, unwrap, type CoveredBy, type ImageSelection } from "../../ipc";
import { previewSrc } from "../../lib/entryImage";
import { hint } from "../../lib/keymap";
import { CHOICE_LABEL, loadReviewed, saveReviewed, stemOf } from "../../lib/target";
import { ActionButton, Pic, Reasons, ShotBadge } from "./bits";
import type { StageRef } from "./SetupStep";
import type { TargetCtx } from "./types";

interface Info {
  sel: ImageSelection | null;
  cov: CoveredBy | null;
}
const num = (n: number) => n.toLocaleString("en-US");

export const SecondStep = forwardRef<StageRef, { ctx: TargetCtx }>(function SecondStep({ ctx }, ref) {
  const { projectId, entries } = ctx;
  const [ids, setIds] = useState<number[] | null>(null);
  const [at, setAt] = useState(0);
  const [reviewed, setReviewed] = useState(() => loadReviewed(projectId, "second"));
  const [info, setInfo] = useState<Map<number, Info>>(new Map());
  const loading = useRef(new Set<number>());
  const busy = useRef(false);
  const stamp = ctx.run?.finishedAtMs ?? 0;

  useEffect(() => {
    let stale = false;
    unwrap(commands.listImageIds({ ...DEFAULT_QUERY, projectId, targetChoices: ["not_sure", "set_aside"] }))
      .then((l) => {
        if (stale) return;
        setIds(l);
        // Continue at the first photo not looked at yet.
        const r = loadReviewed(projectId, "second");
        const first = l.findIndex((i) => !r.has(i));
        setAt(first >= 0 ? first : 0);
      })
      .catch(ctx.onError);
    return () => {
      stale = true;
    };
  }, [projectId, stamp]); // eslint-disable-line react-hooks/exhaustive-deps

  // Selection + covered-by of the current and the next photos (prefetch). Cleared after every edit.
  useEffect(() => {
    setInfo(new Map());
    loading.current.clear();
  }, [ctx.rev]);
  const list = ids ?? [];
  const loadInfo = useCallback(
    (want: number[]) => {
      const todo = want.filter((i) => !info.has(i) && !loading.current.has(i));
      if (todo.length === 0) return;
      todo.forEach((i) => loading.current.add(i));
      void (async () => {
        try {
          const sels = await unwrap(commands.getImageSelections(todo));
          const covs = await Promise.all(todo.map((i) => unwrap(commands.getCoveredBy(i))));
          const covIds = covs.filter((c): c is CoveredBy => !!c).map((c) => c.coveredById);
          entries.need([...todo, ...covIds]);
          setInfo((m) => {
            const n = new Map(m);
            todo.forEach((i, k) => n.set(i, { sel: sels.find((s) => s.imageId === i) ?? null, cov: covs[k] }));
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
  const cur = list[at] ?? null;
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

  const markReviewed = useCallback(
    (id: number | null) => {
      if (id == null) return;
      setReviewed((s) => {
        if (s.has(id)) return s;
        const n = new Set(s).add(id);
        saveReviewed(projectId, "second", n);
        return n;
      });
    },
    [projectId],
  );
  const total = list.length;
  const done = useMemo(() => list.filter((i) => reviewed.has(i)).length, [list, reviewed]);
  const here = cur != null ? info.get(cur) : undefined;
  const cov = here?.cov ?? null;
  const entry = cur != null ? entries.get(cur) : undefined;
  const covEntry = cov ? entries.get(cov.coveredById) : undefined;
  const kept = here?.sel?.choice === "deliver";

  const go = (d: number) => setAt((a) => Math.max(0, Math.min(total - 1, a + d)));
  const advance = () => {
    markReviewed(cur);
    go(1);
  };
  const act = async (fn: () => Promise<boolean>) => {
    if (busy.current || cur == null) return;
    busy.current = true;
    try {
      if (await fn()) advance();
    } finally {
      busy.current = false;
    }
  };
  const skip = () => {
    if (cur != null) advance();
  };
  const swap = () =>
    act(async () => {
      if (!cov) return false;
      const r = await ctx.edit(`Swap ${stemOf(covEntry, cov.coveredById)} for ${stemOf(entry, cur!)}`, unwrap(commands.swapAlternative(cov.coveredById, cur!)));
      if (r) ctx.notify(`Kept ${stemOf(entry, cur!)} instead of ${stemOf(covEntry, cov.coveredById)}. Cmd+Z undoes it`);
      return !!r;
    });
  const add = () =>
    act(async () => {
      const r = await ctx.edit(`${cov ? "Add both" : "Keep"} ${stemOf(entry, cur!)}`, cov ? unwrap(commands.addAlternative(cur!)) : unwrap(commands.setTargetChoice([cur!], "deliver")));
      if (r) ctx.notify(cov ? `Kept both ${stemOf(entry, cur!)} and ${stemOf(covEntry, cov.coveredById)}. Cmd+Z undoes it` : `Kept ${stemOf(entry, cur!)}. Cmd+Z undoes it`);
      return !!r;
    });

  useImperativeHandle(
    ref,
    () => ({
      key: (id) => {
        switch (id) {
          case "targetPrev":
            return go(-1), true;
          case "targetNext":
          case "targetSkip":
            return skip(), true;
          case "targetSwap":
            return void swap(), true;
          case "targetAdd":
          case "targetKeep":
            return void add(), true;
          default:
            return false;
        }
      },
    }),
    [cur, cov, entry, covEntry, total], // eslint-disable-line react-hooks/exhaustive-deps
  );

  if (ids == null) return <div className="p-6 text-sm text-neutral-400">Loading…</div>;
  if (total === 0)
    return (
      <div className="mx-auto max-w-xl p-8 text-center text-sm text-neutral-300" data-testid="target-second-empty">
        Nothing to look at twice: no photos are Not sure or Set aside.
      </div>
    );

  const pct = Math.round((done / total) * 100);
  const sim = cov ? Math.round(cov.similarity * 100) : null;
  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="target-second" data-current={cur ?? ""} data-total={total} data-reviewed={done}>
      <div className="flex shrink-0 items-center gap-3 border-b border-neutral-800 px-4 py-2 text-sm">
        <span className="font-medium text-neutral-100" data-testid="target-second-count" title="Not sure and Set aside photos you have looked at">
          {num(done)} of {num(total)} reviewed
        </span>
        <div className="h-1.5 w-48 overflow-hidden rounded bg-neutral-800" role="progressbar" aria-valuenow={pct} aria-label="Second look progress">
          <div className="h-full bg-emerald-500" style={{ width: `${pct}%` }} />
        </div>
        <span className="text-xs text-neutral-400" data-testid="target-second-position">
          Photo {at + 1} of {num(total)}
        </span>
        <button className="ml-auto flex h-7 items-center gap-1.5 rounded bg-neutral-800 px-2 text-xs hover:bg-neutral-700" data-testid="target-show-rest" title="Close this view and show the Not sure and Set aside photos in the grid" onClick={() => ctx.showInGrid(["not_sure", "set_aside"])}>
          <LayoutGrid className="size-3.5" /> Show in grid
        </button>
      </div>
      {done >= total && (
        <p className="shrink-0 bg-emerald-950 px-4 py-1.5 text-xs text-emerald-100" data-testid="target-second-complete">
          Second look done. Apply the flags from the top right when you are happy.
        </p>
      )}

      <div className="flex min-h-0 flex-1 gap-3 p-3">
        <figure className="relative flex min-w-0 flex-1 flex-col" data-testid="target-second-cur" data-id={cur ?? ""}>
          <Pic entry={entry} big className="min-h-0 flex-1 rounded-lg" testid="target-second-pic" />
          <div className="pointer-events-none absolute left-2 top-2 flex items-center gap-1.5">
            <ShotBadge type={here?.sel?.shotType ?? null} />
            <span className="rounded bg-black/70 px-1.5 py-0.5 text-[11px] text-neutral-100" data-testid="target-second-choice">
              {kept ? "Kept" : here?.sel ? CHOICE_LABEL[here.sel.choice] : ""}
            </span>
          </div>
          <figcaption className="mt-2 flex items-start gap-3 text-xs">
            <span className="shrink-0 text-neutral-200">{entry?.fileName ?? ""}</span>
            <span className="min-w-0 text-neutral-300">
              <Reasons sel={here?.sel ?? undefined} />
            </span>
          </figcaption>
        </figure>

        {cov ? (
          <figure className="relative flex min-w-0 flex-1 flex-col" data-testid="target-covered" data-id={cov.coveredById} data-similarity={cov.similarity}>
            <Pic entry={covEntry} big className="min-h-0 flex-1 rounded-lg ring-1 ring-emerald-700" testid="target-covered-pic" />
            <figcaption className="mt-2 text-xs">
              <div className="flex items-center gap-3">
                <span className="shrink-0 rounded bg-emerald-800 px-1.5 py-0.5 font-semibold text-emerald-50">Picked</span>
                <span className="font-medium text-neutral-100" data-testid="target-covered-text">
                  {cov.text}
                </span>
              </div>
              <div className="mt-1 text-neutral-400" data-testid="target-similarity">
                {sim}% similar{cov.sameMoment ? ", same moment" : ""}
              </div>
            </figcaption>
          </figure>
        ) : (
          <div className="flex min-w-0 flex-1 items-center justify-center rounded-lg border border-dashed border-neutral-800 text-sm text-neutral-400" data-testid="target-no-cover">
            {kept ? "Kept." : "No similar photo is kept. Keep this one, or skip."}
          </div>
        )}
      </div>

      <div className="flex shrink-0 flex-wrap items-center gap-2 border-t border-neutral-800 bg-neutral-950 px-3 py-2" data-testid="target-second-actions">
        <ActionButton testid="target-skip" label="Skip" keys={["Space"]} tone="primary" title={`Leave it as it is and go to the next photo${hint("targetSkip")}`} onClick={skip} />
        {cov ? (
          <>
            <ActionButton testid="target-second-swap" label={`Swap: keep this instead of ${stemOf(covEntry, cov.coveredById)}`} keys={["S"]} title={`Deliver this photo instead of the similar one that is kept; that one becomes its alternative${hint("targetSwap")}`} onClick={() => void swap()} />
            <ActionButton testid="target-second-add" label="Add both" keys={["A"]} tone="good" title={`Keep this photo as well as the similar one${hint("targetAdd")}`} onClick={() => void add()} />
          </>
        ) : (
          <ActionButton testid="target-second-keep" label="Keep" keys={["A", "Z"]} tone="good" disabled={kept} title={`Add this photo to the delivery set${hint("targetKeep")}`} onClick={() => void add()} />
        )}
        <span className="ml-auto text-xs text-neutral-400">Left: back</span>
      </div>
    </div>
  );
});
