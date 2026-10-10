// Step 1 of "Pick the best N": target count + shoot type, a run with progress and cancel, and the summary of the result.
import { forwardRef, useEffect, useImperativeHandle, useState } from "react";
import { Loader2, Sparkles, Users, X } from "lucide-react";
import { commands, unwrap, type ShootType } from "../../ipc";
import { useActivities } from "../../lib/activity";
import { cap, plural, rememberedTarget, rememberTarget, SHOOT_TYPES, SHOT_LABEL, SHOT_ORDER, suggestTarget } from "../../lib/target";
import { ActionButton, ShotBadge } from "./bits";
import type { StageHandler } from "./bits";
import type { TargetCtx } from "./types";

export interface StageRef {
  key: StageHandler;
}

const num = (n: number) => n.toLocaleString("en-US");

export const SetupStep = forwardRef<StageRef, { ctx: TargetCtx; shootType: ShootType; photoCount: number }>(function SetupStep({ ctx, shootType, photoCount }, ref) {
  const { run } = ctx;
  const initialShoot = run?.settings.shootType ?? shootType;
  const [shoot, setShoot] = useState<ShootType>(initialShoot);
  const [count, setCount] = useState<string>(String(run?.settings.targetCount ?? rememberedTarget(initialShoot) ?? suggestTarget(photoCount)));
  const [starting, setStarting] = useState(false);
  const activities = useActivities();
  const act = activities.find((a) => a.kind === "target_selection" && a.state === "running");
  const running = run?.state === "running" || !!act || starting;
  useImperativeHandle(ref, () => ({ key: () => false }), []);
  useEffect(() => {
    if (run?.state === "running" || act) setStarting(false);
  }, [run?.state, act]);

  const n = Number(count);
  const valid = Number.isInteger(n) && n >= 1 && n <= 100_000;
  const pickShoot = (t: ShootType) => {
    setShoot(t);
    setCount(String(rememberedTarget(t) ?? suggestTarget(photoCount)));
  };
  const start = async () => {
    if (!valid || running) return;
    setStarting(true);
    rememberTarget(shoot, n);
    try {
      await unwrap(commands.runTargetSelection(ctx.projectId, { targetCount: n, shootType: shoot }));
      await ctx.refreshRun();
    } catch (e) {
      setStarting(false);
      ctx.onError(e);
    }
  };
  const cancel = () => void unwrap(commands.cancelTargetSelection()).catch(ctx.onError);

  const c = run?.counts;
  const done = run?.state === "finished" && !running && c && c.total > 0;
  const pct = act && act.total ? Math.round((act.done / act.total) * 100) : null;

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-5 overflow-y-auto p-6" data-testid="target-setup">
      <div>
        <h2 className="text-lg font-semibold text-neutral-50">Pick the best {valid ? num(n) : "N"}</h2>
        <p className="mt-1 text-sm text-neutral-300">
          Sieve chooses the photos to deliver: many variations of the couple, one per group setup and per detail, and candids where a face is clearly visible. Then you review the picks with their alternatives at hand, and only glance at the rest. The count is a guideline, not a limit.
        </p>
      </div>

      <div className="flex flex-wrap items-end gap-4 rounded-lg border border-neutral-800 bg-neutral-900 p-4" data-testid="target-form">
        <label className="flex flex-col gap-1 text-xs text-neutral-300">
          How many photos do you want to deliver?
          <input
            type="number"
            min={1}
            max={100000}
            value={count}
            onChange={(e) => setCount(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && void start()}
            disabled={running}
            className="h-9 w-40 rounded bg-neutral-800 px-2 text-base text-neutral-50 disabled:opacity-50"
            data-testid="target-count"
            title={`Target number of photos to deliver (a guideline). Suggested: about a third of the ${num(photoCount)} photos`}
            aria-label="Target count"
          />
        </label>
        <label className="flex flex-col gap-1 text-xs text-neutral-300">
          Shoot type
          <select
            value={shoot}
            onChange={(e) => pickShoot(e.target.value as ShootType)}
            disabled={running}
            className="h-9 rounded bg-neutral-800 px-2 text-sm text-neutral-50 disabled:opacity-50"
            data-testid="target-shoot"
            title="Decides who counts as the main subject and how the delivery is balanced. The target is remembered per shoot type"
            aria-label="Shoot type"
          >
            {SHOOT_TYPES.map((t) => (
              <option key={t} value={t}>
                {cap(t)}
              </option>
            ))}
          </select>
        </label>
        {running ? (
          <button className="flex h-9 items-center gap-2 rounded-md bg-neutral-800 px-3 text-sm text-neutral-50 hover:bg-neutral-700" data-testid="target-cancel" title="Stop choosing. The previous selection is kept" onClick={cancel}>
            <X className="size-4" /> Cancel
          </button>
        ) : (
          <button
            className="flex h-9 items-center gap-2 rounded-md bg-sky-700 px-4 text-sm font-medium text-white hover:bg-sky-600 disabled:opacity-40"
            data-testid="target-run"
            title={valid ? `Choose the best ${num(n)} of ${num(photoCount)} photos. Nothing is flagged until you apply` : "Enter a number between 1 and 100,000"}
            disabled={!valid}
            onClick={() => void start()}
          >
            <Sparkles className="size-4" /> {run && run.counts.total > 0 ? "Run again" : `Pick the best ${valid ? num(n) : "N"}`}
          </button>
        )}
        <p className="basis-full text-xs text-neutral-400" data-testid="target-suggest">
          {num(photoCount)} photos in this project. Suggested: {num(suggestTarget(photoCount))} (about a third).
        </p>
      </div>

      {running && (
        <div className="rounded-lg border border-sky-900 bg-sky-950/60 p-4" data-testid="target-progress" data-pct={pct ?? ""}>
          <div className="mb-2 flex items-center gap-2 text-sm text-sky-100">
            <Loader2 className="size-4 animate-spin" /> <span data-testid="target-progress-label">{act?.label ?? "Choosing the best photos"}</span>
            {act && act.total ? (
              <span className="text-sky-300">
                step {Math.min(act.done + 1, act.total)} of {act.total}
              </span>
            ) : null}
          </div>
          <div className="h-1.5 overflow-hidden rounded bg-neutral-800" role="progressbar" aria-valuenow={pct ?? undefined} aria-label="Choosing the delivery set">
            <div className="h-full bg-sky-500 transition-all" style={{ width: `${pct ?? 15}%` }} />
          </div>
        </div>
      )}

      {!running && run && (run.state === "failed" || run.state === "cancelled") && (
        <p className="rounded border border-amber-900 bg-amber-950/60 p-3 text-sm text-amber-100" data-testid="target-run-message">
          {run.state === "cancelled" ? "Stopped. " : "Could not finish. "}
          {run.message ?? ""}
          {c && c.total > 0 ? " The previous selection is kept." : ""}
        </p>
      )}

      {!running && run?.state === "finished" && c && c.total === 0 && (
        <p className="rounded border border-amber-900 bg-amber-950/60 p-3 text-sm text-amber-100" data-testid="target-run-message">
          {run.message ?? "No photos were chosen."}
        </p>
      )}

      {done && c && (
        <section className="rounded-lg border border-neutral-800 bg-neutral-900 p-4" data-testid="target-summary" data-deliver={c.deliver} data-not-sure={c.notSure} data-set-aside={c.setAside}>
          <h3 className="text-sm font-semibold text-neutral-100">
            Picked <b data-testid="target-sum-deliver">{num(c.deliver)}</b> of {num(c.total)} photos
            <span className="ml-2 font-normal text-neutral-400">target {num(run.settings.targetCount)}</span>
          </h3>
          <ul className="mt-3 grid grid-cols-5 gap-2" data-testid="target-sum-shots">
            {SHOT_ORDER.map((s) => {
              const row = c.perShotType.find((x) => x.shotType === s);
              if (!row && s === "other") return null;
              return (
                <li key={s} className="rounded bg-neutral-800 p-2 text-center" data-testid={`target-sum-shot-${s}`} data-deliver={row?.deliver ?? 0} title={`${SHOT_LABEL[s]}: ${row?.deliver ?? 0} picked of ${row?.total ?? 0} photos`}>
                  <ShotBadge type={s} />
                  <div className="mt-1 text-xl font-semibold text-neutral-50">{row?.deliver ?? 0}</div>
                  <div className="text-[11px] text-neutral-400">of {row?.total ?? 0}</div>
                </li>
              );
            })}
          </ul>
          <dl className="mt-3 flex flex-wrap gap-x-6 gap-y-1 text-sm text-neutral-300">
            <div title="Almost made it: a quick second look decides" data-testid="target-sum-notsure">
              Not sure <b className="text-neutral-50">{num(c.notSure)}</b>
            </div>
            <div title="Near-duplicates, defects and weaker frames. Nothing is rejected" data-testid="target-sum-setaside">
              Set aside <b className="text-neutral-50">{num(c.setAside)}</b>
            </div>
            <div title="Alternatives are kept next to their pick for a one-key swap" data-testid="target-sum-alts">
              Alternatives <b className="text-neutral-50">{num(c.alternative)}</b>
            </div>
          </dl>
          <div className="mt-4 flex flex-wrap items-center gap-2">
            {run.peopleQuestions > 0 && (
              <ActionButton testid="target-next-people" label={`Check the people (${plural(run.peopleQuestions, "question")})`} keys={[]} tone="primary" title="Confirm the couple and say who else matters, then re-run" onClick={() => ctx.go("people")} />
            )}
            <ActionButton testid="target-next-review" label={`Review the ${num(c.deliver)} picks`} keys={[]} tone={run.peopleQuestions > 0 ? "neutral" : "primary"} title="Go through the delivery set with the alternatives of each moment" onClick={() => ctx.go("review")} />
            <ActionButton testid="target-next-second" label="Second look" keys={[]} title="Quickly check the Not sure and Set aside photos" onClick={() => ctx.go("second")} />
            {run.peopleQuestions === 0 && (
              <span className="flex items-center gap-1 text-xs text-neutral-400" title="People you answered about are kept for every re-run">
                <Users className="size-3.5" /> People confirmed
              </span>
            )}
          </div>
        </section>
      )}
    </div>
  );
});
