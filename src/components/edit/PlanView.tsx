// Edit step overview: keepers grouped into scenes, one representative per scene, and a checklist
// (to do / edited / applied) with Auto edit (my style), Apply to scene and Apply all.
import { useEffect, useRef, useState } from "react";
import { ArrowRight, ChevronDown, ChevronRight, Layers, MoreHorizontal, X } from "lucide-react";
import type { BaselineRun, CullSummary } from "../../ipc";
import type { Library } from "../../hooks/useLibrary";
import { rowInTab, type PlanTab, type SceneRow, type Workflow } from "../../hooks/useWorkflow";
import { hint } from "../../lib/keymap";
import { BUSY_WHY, useActivityRunning } from "../../lib/activity";
import { Menu, menuItem } from "../Menu";
import { KeeperRuleMenu, RuleItems, ruleLabel } from "../KeeperRule";
import { keeperEquation } from "../../lib/cull";
import { Dialog } from "../Dialog";
import { AutoEditButton, ApplyWhy, applyBlock, StatusIcon, statusLine, Thumb, useWide } from "./bits";

const TABS: { id: PlanTab; label: string }[] = [
  { id: "all", label: "All" },
  { id: "todo", label: "To do" },
  { id: "edited", label: "Edited" },
  { id: "applied", label: "Applied" },
  { id: "skipped", label: "Skipped" },
];

const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;
const timeRange = (a: number | null | undefined, b: number | null | undefined) => {
  if (a == null) return "";
  const f = (t: number) => new Date(t).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  return b != null && f(a) !== f(b) ? `${f(a)}–${f(b)}` : f(a);
};

interface Props {
  wf: Workflow;
  lib: Library;
  /** Scene start / end times for the row subtitle. */
  sceneTimes: (sceneId: number) => { start: number | null; end: number | null };
  focusId: number | null;
  onFocus: (sceneId: number) => void;
  onEdit: (sceneId: number) => void;
  onReview: (sceneId: number, ids?: number[]) => void;
  onShowScene: (sceneId: number) => void;
  /** "Show" on the apply toast: list these photos in the grid, selected. */
  onShowIds: (ids: number[], label: string) => void;
  onChangeRep: (sceneId: number) => void;
  onApplyOptions: (sceneId: number) => void;
  onBackToCull: () => void;
  onContinueExport: () => void;
  onRegroup: () => void;
  /** Cull summary of the project: the keeper formula under the header. */
  summary?: CullSummary | null;
  /** Baseline edit (Phase 10): the main path of the Edit step. */
  onBaseline?: () => void;
  baselineRun?: BaselineRun | null;
  /** Representatives whose settings are still what a baseline run wrote (`get_baseline_provenance`): nothing to apply from them. */
  onBaselineIds?: Set<number>;
  /** Flagged photos the user has not marked "Looks good" yet (null = no baseline result). */
  flaggedLeft?: number | null;
  /** Banner: open the baseline view straight on "Finish in Lightroom". */
  onBaselineFinish?: () => void;
}

export function PlanView(p: Props) {
  const { wf } = p;
  const wide = useWide();
  const { tab, setTab } = wf;
  const [confirmRegroup, setConfirmRegroup] = useState(false);
  const rows = wf.rows;
  const keeperCount = wf.plan?.keeperIds.length ?? 0;
  const counts = wf.plan?.counts;
  const todo = rows.filter((r) => (r.ui === "todo" || r.ui === "reset") && !r.skipped);
  // Scenes the "Apply" button handles: edited ones, and applied ones that gained keepers since.
  const onBase = p.onBaselineIds;
  const isOnBase = (r: SceneRow) => !!onBase?.has(r.entry.representativeId);
  const pending = rows.filter((r) => !r.skipped && !isOnBase(r) && (r.ui === "edited" || r.ui === "auto" || r.ui === "stale" || (r.ui === "applied" && r.unapplied.length > 0)));
  const bRun = p.baselineRun;
  const baselineDone = bRun?.state === "finished" && bRun.batch != null && bRun.batch.undoneAtMs == null;
  const pendingTargets = pending.reduce((a, r) => a + (r.ui === "applied" ? r.unapplied.length : r.targets), 0);
  const allDone = wf.done;
  const visible = wf.layout.visible;
  const unassigned = wf.plan?.unassignedKeeperIds.length ?? 0;
  const memberShown = wide ? 8 : 6;

  // Keep the thumbnails of every visible row loaded.
  const { ensure } = p.lib;
  useEffect(() => {
    ensure(visible.flatMap((r) => [r.entry.representativeId, ...r.entry.imageIds.filter((i) => i !== r.entry.representativeId).slice(0, memberShown)]));
  }, [visible, memberShown, ensure]);

  // Scroll the focused row into view (Up / Down).
  const listRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (p.focusId != null) listRef.current?.querySelector(`[data-testid="plan-scene-${p.focusId}"]`)?.scrollIntoView({ block: "nearest" });
  }, [p.focusId]);

  const busy = wf.busy;
  const applying = useActivityRunning("apply_scene");
  const style = wf.style;

  let body: React.ReactNode;
  if (wf.grouping || (wf.loading && !wf.plan)) {
    body = (
      <div className="space-y-2 p-3" data-testid="plan-skeleton">
        {Array.from({ length: 6 }, (_, i) => (
          <div key={i} className="h-[112px] animate-pulse rounded-lg bg-neutral-900" />
        ))}
      </div>
    );
  } else if (wf.planError) {
    body = (
      <Empty testid="plan-error" title="Could not group the keepers into scenes." detail={wf.planError}>
        <button className="rounded-md bg-sky-700 px-3 py-1.5 text-sm hover:bg-sky-600" data-testid="plan-retry" onClick={() => void wf.loadPlan(true)}>
          Try again
        </button>
      </Empty>
    );
  } else if (wf.plan && wf.plan.keeperIds.length === 0) {
    body = (
      <Empty testid="plan-empty" title="No keepers yet." detail="Pick photos in Cull (Z), or choose which photos count as keepers.">
        <button className="rounded-md bg-neutral-800 px-3 py-1.5 text-sm hover:bg-neutral-700" data-testid="plan-back-cull" onClick={p.onBackToCull}>
          Back to Cull
        </button>
        <Menu trigger={<span className="flex items-center gap-1">Change keeper rule <ChevronDown className="size-4" /></span>} triggerClass="rounded-md bg-neutral-800 px-3 py-1.5 text-sm hover:bg-neutral-700" triggerTestId="plan-empty-rule">
          {(close) => <RuleItems current={wf.plan!.keeperRule} onPick={(r) => (close(), void wf.setKeeperRule(r))} />}
        </Menu>
      </Empty>
    );
  } else if (wf.plan && rows.length === 0) {
    body = (
      <Empty testid="plan-ungrouped" title="Keepers are not grouped into scenes yet." detail={`${plural(keeperCount, "keeper")} to group by time and lighting.`}>
        <button className="rounded-md bg-sky-700 px-3 py-1.5 text-sm hover:bg-sky-600" data-testid="plan-group" onClick={() => void wf.loadPlan(true)}>
          Group keepers into scenes
        </button>
      </Empty>
    );
  } else {
    body = (
      <div ref={listRef} className="min-h-0 flex-1 space-y-2 overflow-y-auto p-3" data-testid="plan-list">
        {visible.length === 0 && wf.layout.minor.length === 0 && <p className="py-10 text-center text-sm text-neutral-400">No scenes in this tab.</p>}
        {wf.layout.main.map((r) => (
          <SceneRowView key={r.entry.sceneId} r={r} p={p} memberShown={memberShown} focused={p.focusId === r.entry.sceneId} />
        ))}
        {wf.layout.minor.length > 0 && (
          <>
            <button
              className="flex h-6 w-full items-center gap-1 rounded text-left text-xs text-neutral-300 hover:text-neutral-100"
              data-testid="plan-minor-toggle"
              aria-expanded={wf.minorOpen}
              onClick={() => wf.setMinorOpen(!wf.minorOpen)}
            >
              {wf.minorOpen ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
              Small scenes ({plural(wf.layout.minor.length, "scene")}, {plural(wf.layout.minor.reduce((a, r) => a + r.entry.imageIds.length, 0), "photo")})
              <span className="h-px flex-1 bg-neutral-800" />
            </button>
            {wf.minorOpen && wf.layout.minor.map((r) => <SceneRowView key={r.entry.sceneId} r={r} p={p} memberShown={memberShown} focused={p.focusId === r.entry.sceneId} />)}
          </>
        )}
      </div>
    );
  }

  const groupingPct = wf.grouping && wf.grouping.total > 0 ? Math.round((wf.grouping.done / wf.grouping.total) * 100) : 0;

  return (
    <section className="absolute inset-0 z-10 flex flex-col bg-neutral-950" data-testid="plan-view" aria-label="Edit plan">
      <header className="flex h-12 shrink-0 items-center gap-3 border-b border-neutral-800 px-3" data-testid="plan-header">
        {wf.grouping ? (
          <div className="flex min-w-0 items-center gap-3 text-sm" data-testid="plan-grouping">
            <span className="truncate">Grouping {plural(keeperCount, "keeper")} into lighting scenes… {groupingPct}%</span>
            <span className="h-1.5 w-[200px] overflow-hidden rounded-full bg-neutral-800">
              <span className="block h-full bg-emerald-500 transition-all" style={{ width: `${groupingPct}%` }} />
            </span>
          </div>
        ) : (
          <>
            <div className="shrink-0 whitespace-nowrap text-sm">
              <span className="font-semibold">Edit</span>
              <span className="text-neutral-400" data-testid="plan-summary">
                {" "}
                · {plural(rows.length, "scene")} · {plural(keeperCount, "keeper")}
              </span>
            </div>
            {wf.plan && (
              <Menu
                trigger={
                  <>
                    Keepers: {ruleLabel(wf.plan.keeperRule)} <ChevronDown className="size-3.5" />
                  </>
                }
                triggerClass="flex h-7 items-center gap-1 whitespace-nowrap rounded-md bg-neutral-800 px-2 text-xs hover:bg-neutral-700"
                triggerTestId="plan-keepers-rule"
                title="Which photos count as keepers (used by the Edit and Export steps)"
              >
                {(close) => <RuleItems current={wf.plan!.keeperRule} onPick={(r) => (close(), void wf.setKeeperRule(r))} />}
              </Menu>
            )}
            {rows.length > 0 && (
              <div className="flex min-w-0 shrink items-center gap-2 text-xs text-neutral-300" data-testid="plan-progress">
                <span className="flex h-1.5 w-24 shrink-0 overflow-hidden rounded-full bg-neutral-800 min-[1600px]:w-[200px]" aria-hidden>
                  <span className="h-full bg-emerald-500" style={{ width: `${((counts?.applied ?? 0) / rows.length) * 100}%` }} />
                  <span className="h-full bg-sky-500" style={{ width: `${(((counts?.edited ?? 0) + (counts?.outdated ?? 0)) / rows.length) * 100}%` }} />
                </span>
                <span className="min-w-0 truncate whitespace-nowrap" data-testid="plan-counts" data-kind={baselineDone ? "baseline" : "scenes"}>
                  {baselineDone && bRun ? `Baseline: ${bRun.counts.applied + bRun.counts.flagged} of ${keeperCount} keepers${(p.flaggedLeft ?? bRun.counts.flagged) > 0 ? ` · ${p.flaggedLeft ?? bRun.counts.flagged} need a look` : ""}` : `${(counts?.edited ?? 0) + (counts?.outdated ?? 0)} edited · ${counts?.applied ?? 0} applied · ${counts?.toEdit ?? 0} to do${(counts?.skipped ?? 0) > 0 ? ` · ${counts?.skipped} skipped` : ""}`}
                </span>
              </div>
            )}
          </>
        )}
        <div className="ml-auto flex shrink-0 items-center gap-2">
          {(busy?.kind === "all" || busy?.kind === "scene") && (
            <span className="flex items-center gap-1 rounded-full bg-emerald-950 py-0.5 pl-2.5 pr-1 text-xs text-emerald-300" data-testid="plan-applying">
              {wf.cancelling ? "Stopping…" : (busy.total > 0 ? `Applying… ${busy.done}/${busy.total}` : "Applying…")}
              <button className="rounded-full p-0.5 hover:bg-emerald-900 disabled:opacity-40" data-testid="plan-apply-cancel" aria-label="Stop applying (Esc)" title="Stop applying (Esc). Scenes already applied stay applied" disabled={wf.cancelling} onClick={wf.cancelApply}>
                <X className="size-3" />
              </button>
            </span>
          )}
          {todo.length > 0 && (
            <AutoEditButton
              style={style}
              testid="plan-auto-remaining"
              label={`Auto edit ${todo.length} remaining (my style)`}
              disabled={busy != null}
              onClick={() => wf.requestAutoEdit(todo.map((r) => r.entry.sceneId))}
            />
          )}
          {allDone ? (
            <button className="flex h-7 items-center gap-1.5 rounded-md bg-emerald-700 px-3 text-xs font-medium text-white hover:bg-emerald-600" data-testid="plan-continue-export" onClick={p.onContinueExport}>
              Continue to Export <ArrowRight className="size-3.5" /> {keeperCount}
            </button>
          ) : (pending.length === 0 && baselineDone) || (pending.length === 0 && (counts?.toEdit ?? 0) === 0) || (pending.length === 0 && rows.some((r) => r.ui === "applied" || r.ui === "reset")) ? null : (
            <div className="flex shrink-0 items-center gap-2 max-[1439px]:flex-col-reverse max-[1439px]:items-end max-[1439px]:gap-0.5" data-testid="plan-apply-block">
            {pending.length === 0 && busy == null && !applying && (
              <ApplyWhy
                block={{ reason: "Edit a scene's representative first", fix: todo.length > 0 ? "open" : undefined }}
                testid="plan-apply-all-why"
                onFix={() => todo.length > 0 && p.onEdit(todo[0].entry.sceneId)}
              />
            )}
            <button
              className="flex h-7 items-center gap-1.5 rounded-md bg-neutral-800 px-3 text-xs font-medium text-neutral-100 hover:bg-neutral-700 disabled:opacity-40"
              data-testid="plan-apply-all"
              disabled={pending.length === 0 || busy != null || applying}
              title={applying ? BUSY_WHY.apply_scene : pending.length === 0 ? "Edit or auto edit a scene first" : "Copies each edited scene's edit to the rest of its scene, matching exposure and white balance per photo"}
              onClick={() => void wf.applyAll(p.onReview, p.onFocus, p.onShowIds)}
            >
              Apply {plural(pending.length, "edited scene")} ({pendingTargets})
              <ArrowRight className="size-3.5" />
            </button>
            </div>
          )}
          <Menu trigger={<MoreHorizontal className="size-4" />} triggerClass="flex h-7 shrink-0 items-center rounded-md bg-neutral-800 px-2 hover:bg-neutral-700" triggerTestId="plan-more" title="More" align="right">
            {(close) => (
              <div className="w-72 py-1">
                <button
                  className={menuItem}
                  data-testid="plan-retrain"
                  disabled={style?.state === "training" || (style?.availableExamples ?? 0) < (style?.minExamples ?? 0)}
                  onClick={() => {
                    close();
                    void wf.train();
                  }}
                >
                  Retrain my style
                </button>
                <p className="px-3 py-1.5 text-xs text-neutral-400" data-testid="plan-style-info">
                  {style?.state === "ready" && style.trainedAtMs != null
                    ? `Style: learned from ${style.trainingExamples} photos · ${new Date(style.trainedAtMs).toLocaleDateString([], { day: "numeric", month: "short" })}`
                    : style && style.availableExamples < style.minExamples
                      ? `Style: not learned yet (${style.availableExamples} of ${style.minExamples} edited photos)`
                      : "Style: not learned yet"}
                </p>
                <button
                  className={`${menuItem} border-t border-neutral-800`}
                  data-testid="plan-rebuild"
                  onClick={() => {
                    close();
                    setConfirmRegroup(true);
                  }}
                >
                  Rebuild plan…
                </button>
              </div>
            )}
          </Menu>
        </div>
      </header>

      {p.onBaseline && !wf.grouping && wf.plan && wf.plan.keeperIds.length > 0 && <BaselineBanner run={p.baselineRun ?? null} flaggedLeft={p.flaggedLeft ?? null} onOpen={p.onBaseline} onFinish={p.onBaselineFinish} />}

      {p.summary && !wf.grouping && (
        <div className="flex min-h-7 shrink-0 flex-wrap items-center gap-x-2 border-b border-neutral-800 px-3 py-1 text-xs text-neutral-300" data-testid="plan-keeper-formula">
          <span title="Only keepers are grouped into scenes, edited and exported">
            Keepers <b>{p.summary.keepers}</b> {keeperEquation(p.summary)}
          </span>
          <KeeperRuleMenu current={p.summary.keeperRule} onPick={(r) => void wf.setKeeperRule(r)} testid="plan-keeper-rule-link" />
          <button className="text-sky-300 hover:underline" data-testid="plan-review-rejected" onClick={p.onBackToCull} title="Go back to Cull to look at what was left out">
            Back to Cull
          </button>
        </div>
      )}

      {unassigned > 0 && !wf.grouping && (
        <div className="flex h-8 shrink-0 items-center gap-3 border-b border-amber-900 bg-amber-950 px-3 text-xs text-amber-100" data-testid="plan-unassigned">
          <span>
            {unassigned === 1 ? "1 keeper is not in a scene yet." : `${unassigned} keepers are not in a scene yet.`}
          </span>
          <button className="rounded bg-amber-800 px-2 py-0.5 font-medium hover:bg-amber-700" data-testid="plan-group-new" onClick={() => void wf.loadPlan(true)}>
            Group them
          </button>
        </div>
      )}

      <div className="flex h-8 shrink-0 items-center gap-1 border-b border-neutral-800 px-3 text-xs" role="tablist">
        {TABS.map((t) => {
          const n = rows.filter((r) => rowInTab(r, t.id)).length;
          return (
            <button
              key={t.id}
              role="tab"
              aria-selected={tab === t.id}
              data-testid={`plan-tab-${t.id}`}
              onClick={() => setTab(t.id)}
              className={`rounded px-2 py-1 ${tab === t.id ? "bg-neutral-800 text-neutral-100" : "text-neutral-400 hover:text-neutral-200"}`}
            >
              {t.label} <span className="text-neutral-400">{n}</span>
            </button>
          );
        })}
        {allDone && (
          <span className="ml-3 text-emerald-300" data-testid="plan-all-done">
            All {rows.length} scenes done
          </span>
        )}
      </div>

      {body}

      {confirmRegroup && (
        <Dialog
          label="Rebuild plan"
          testid="plan-rebuild-dialog"
          className="w-[420px] max-w-full rounded-lg border border-neutral-700 bg-neutral-900 p-4 shadow-xl"
          onCancel={() => setConfirmRegroup(false)}
          onConfirm={() => {
            setConfirmRegroup(false);
            p.onRegroup();
          }}
        >
          <h2 className="mb-1 text-sm font-semibold">Rebuild the plan?</h2>
          <p className="mb-4 text-xs text-neutral-300">Scenes and representatives are chosen again. Edits are kept.</p>
          <div className="flex justify-end gap-2">
            <button className="rounded-md bg-neutral-800 px-3 py-1 text-sm hover:bg-neutral-700" onClick={() => setConfirmRegroup(false)}>
              Cancel
            </button>
            <button className="rounded-md bg-sky-700 px-3 py-1 text-sm hover:bg-sky-600" data-testid="plan-rebuild-confirm" onClick={() => (setConfirmRegroup(false), p.onRegroup())}>
              Rebuild
            </button>
          </div>
        </Dialog>
      )}
    </section>
  );
}

function Empty({ testid, title, detail, children }: { testid: string; title: string; detail: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-1 items-center justify-center p-6" data-testid={testid}>
      <div className="max-w-[420px] text-center">
        <p className="text-sm font-semibold text-neutral-200">{title}</p>
        <p className="mt-1 text-xs text-neutral-400">{detail}</p>
        <div className="mt-4 flex justify-center gap-2">{children}</div>
      </div>
    </div>
  );
}

function SceneRowView({ r, p, memberShown, focused }: { r: SceneRow; p: Props; memberShown: number; focused: boolean }) {
  const { wf, lib } = p;
  const e = r.entry;
  const id = e.sceneId;
  const onBase = !!p.onBaselineIds?.has(e.representativeId);
  const line = onBase && !r.skipped ? { text: "On baseline", cls: "text-emerald-300", extra: r.review.length > 0 ? ` · ${r.review.length} need a look` : undefined } : statusLine(r);
  const t = p.sceneTimes(id);
  const repEntry = lib.getEntry(e.representativeId);
  const others = e.imageIds.filter((i) => i !== e.representativeId);
  // Size the strip from the width left after the action buttons: whole thumbs plus the "+N" chip.
  const stripRef = useRef<HTMLDivElement>(null);
  const [stripW, setStripW] = useState<number | null>(null);
  useEffect(() => {
    const el = stripRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setStripW(el.clientWidth));
    ro.observe(el);
    setStripW(el.clientWidth);
    return () => ro.disconnect();
  }, []);
  const cell = (typeof window !== "undefined" && window.innerWidth >= 1600 ? 90 : 72) + 4;
  const fit = stripW == null ? memberShown : others.length * cell <= stripW ? others.length : Math.max(0, Math.floor((stripW - 48) / cell));
  const shown = others.slice(0, Math.min(memberShown, fit));
  const more = others.length - shown.length;
  const busyHere = wf.busy?.kind === "scene" && wf.busy.sceneId === id;
  const applying = useActivityRunning("apply_scene");
  const anyBusy = wf.busy != null || applying;
  const undo = wf.sceneUndo(id);
  const newKeepers = r.ui === "applied" ? r.unapplied.length : 0;
  const applyLabel = newKeepers > 0 ? `Apply to ${newKeepers} new` : r.ui === "stale" ? `Re-apply to ${r.targets}` : `Apply to ${r.targets}`;
  const canApply = r.ui !== "todo" && r.ui !== "reset" && r.targets > 0 && !r.skipped;
  const reviewSet = new Set(r.review);
  const appliedSet = new Set(e.appliedIds);

  return (
    <article
      data-testid={`plan-scene-${id}`}
      data-status={r.ui}
      data-on-baseline={onBase}
      data-focused={focused}
      onClick={() => p.onFocus(id)}
      data-skipped={r.skipped}
      className={`flex h-[112px] items-center gap-3 rounded-lg border bg-neutral-900 px-3 min-[1600px]:h-[140px] ${r.skipped ? "opacity-60" : ""} ${focused ? "border-sky-500 ring-2 ring-sky-500" : "border-neutral-800"}`}
    >
      <StatusIcon ui={r.ui} />
      <button
        className="relative h-[96px] w-[144px] shrink-0 overflow-hidden rounded ring-2 ring-amber-400 min-[1600px]:h-[120px] min-[1600px]:w-[180px]"
        onDoubleClick={() => p.onEdit(id)}
        onClick={() => p.onFocus(id)}
        title="The representative: edit this photo (double-click to open it)"
        data-testid={`plan-rep-${id}`}
      >
        <Thumb entry={repEntry} version={lib.version(e.representativeId)} className="size-full" />
        <span className="absolute left-1 top-1 rounded bg-black/70 px-1 text-[10px] font-bold text-amber-400">R</span>
        {repEntry?.hasEdits && <span className="absolute right-1 top-1 size-2 rounded-full bg-sky-400" />}
      </button>
      <div className="w-[220px] shrink-0 min-[1600px]:w-[260px]">
        <p className="text-[13px] font-semibold text-neutral-100">Scene {r.number}</p>
        <p className="text-xs text-neutral-400">
          {timeRange(t.start, t.end)}
          {t.start != null ? " · " : ""}
          {plural(e.imageIds.length, "keeper")}
        </p>
        <p className={`text-xs ${line.cls}`} data-testid={`plan-status-${id}`}>
          {busyHere ? (
            wf.busy!.total > 0 ? `Applying… ${wf.busy!.done}/${wf.busy!.total}` : "Applying…"
          ) : r.ui === "todo" && !r.skipped ? (
            <>
              To do:{" "}
              <button className="text-sky-300 underline-offset-2 hover:underline" data-testid={`plan-todo-edit-${id}`} onClick={(ev) => (ev.stopPropagation(), p.onEdit(id))}>
                edit this photo
              </button>
            </>
          ) : (
            line.text
          )}
          {!busyHere && line.extra && <span className="text-amber-300">{line.extra}</span>}
        </p>
        {!busyHere && !onBase && !(r.ui === "todo" && !r.skipped) && (
          <ApplyWhy
            block={applyBlock(r)}
            testid={`plan-apply-why-${id}`}
            onFix={(fix) => (fix === "open" ? p.onEdit(id) : void wf.setSkipped(id, false))}
          />
        )}
        <p className="truncate text-[11px] text-neutral-400" title={e.representativeReason}>
          {repEntry?.fileName ?? `#${e.representativeId}`}
        </p>
      </div>
      <div ref={stripRef} className="flex min-w-0 flex-1 items-center gap-1 overflow-hidden" data-testid={`plan-members-${id}`}>
        {shown.map((i) => (
          <div key={i} className="relative h-12 w-[72px] shrink-0 overflow-hidden rounded min-[1600px]:h-[60px] min-[1600px]:w-[90px]">
            <Thumb entry={lib.getEntry(i)} version={lib.version(i)} className="size-full" />
            {reviewSet.has(i) ? <span className="absolute bottom-0.5 right-0.5 rounded bg-black/70 px-1 text-[10px] font-bold text-amber-400" data-testid={`plan-mark-review-${i}`}>!</span> : appliedSet.has(i) && <span className="absolute bottom-0.5 right-0.5 rounded bg-black/60 px-0.5 text-[10px] text-emerald-400">✓</span>}
          </div>
        ))}
        {more > 0 && (
          <button
            className="flex h-12 shrink-0 items-center rounded bg-neutral-800 px-2 text-xs text-neutral-300 hover:bg-neutral-700 min-[1600px]:h-[60px]"
            onClick={() => p.onShowScene(id)}
            data-testid={`plan-more-${id}`}
            title="Show this scene in the grid"
          >
            +{more}
          </button>
        )}
      </div>
      <div className="flex shrink-0 items-center justify-end gap-2" onClick={(ev) => ev.stopPropagation()}>
        {!r.skipped && !onBase && (r.ui === "edited" || r.ui === "auto" || r.ui === "stale" || newKeepers > 0) && (
          <button
            className="h-7 whitespace-nowrap rounded-md bg-emerald-700 px-3 text-xs font-medium text-white hover:bg-emerald-600 disabled:opacity-40"
            data-testid={`plan-apply-${id}`}
            disabled={!canApply || anyBusy}
            title={applying ? BUSY_WHY.apply_scene : canApply ? (newKeepers > 0 ? `Copies this edit to the ${newKeepers} keepers added since it was applied.` : `Copies this edit to ${r.targets} photos, matching exposure and white balance to each.`) : "No other keepers in this scene"}
            onClick={() => void wf.applyScene(id, "match", p.onReview, p.onShowIds)}
          >
            {applyLabel}
          </button>
        )}
        {r.review.length > 0 && !r.skipped && r.ui !== "reset" && (
          <button className="h-7 whitespace-nowrap rounded-md bg-amber-900/70 px-3 text-xs font-medium text-amber-100 hover:bg-amber-800" data-testid={`plan-review-${id}`} onClick={() => p.onReview(id)}>
            Review {r.review.length}
          </button>
        )}
        <button
          className={`h-7 whitespace-nowrap rounded-md px-3 text-xs font-medium ${r.ui === "todo" || r.ui === "reset" ? "bg-sky-700 text-white hover:bg-sky-600" : "bg-neutral-800 text-neutral-200 hover:bg-neutral-700"}`}
          data-testid={`plan-edit-${id}`}
          onClick={() => p.onEdit(id)}
        >
          {r.ui === "auto" ? "Review" : "Edit"} ▸
        </button>
        {r.ui === "reset" && !r.skipped && undo.batch && (
          <button
            className="h-7 whitespace-nowrap rounded-md bg-neutral-800 px-3 text-xs font-medium text-neutral-200 hover:bg-neutral-700 disabled:opacity-40"
            data-testid={`plan-undo-inline-${id}`}
            disabled={!undo.enabled || anyBusy}
            title={undo.reason ?? "Restore the photos to how they were before the apply"}
            onClick={() => void wf.undoBatch(undo.batch!)}
          >
            Undo apply
          </button>
        )}
        {!r.skipped && (r.ui === "todo" || r.ui === "reset" || r.ui === "edited") && (
          <AutoEditButton style={wf.style} testid={`plan-auto-${id}`} label="Auto edit" disabled={anyBusy} onClick={() => wf.requestAutoEdit([id])} className="px-2" />
        )}
        <Menu trigger={<MoreHorizontal className="size-4" />} triggerClass="flex h-7 shrink-0 items-center rounded-md bg-neutral-800 px-1.5 hover:bg-neutral-700" triggerTestId={`plan-menu-${id}`} title="More for this scene" align="right">
          {(close) => {
            const item = (testid: string, label: string, fn: () => void, disabled = false, why?: string) => (
              <button
                className={menuItem}
                data-testid={testid}
                disabled={disabled}
                title={why}
                onClick={() => {
                  close();
                  fn();
                }}
              >
                {label}
              </button>
            );
            return (
              <div className="w-64 py-1">
                {item(`plan-change-rep-${id}`, "Change representative…", () => p.onChangeRep(id))}
                {item(`plan-options-${id}`, "Apply with options…", () => p.onApplyOptions(id), r.ui === "todo" || r.ui === "reset")}
                {item(`plan-exact-${id}`, "Copy exactly (no matching)", () => void wf.applyScene(id, "exact", p.onReview), r.ui === "todo" || r.ui === "reset" || anyBusy || r.targets === 0)}
                {item(`plan-skip-${id}`, r.skipped ? `Include this scene${hint("planSkip")}` : `Skip this scene${hint("planSkip")}`, () => void wf.setSkipped(id, !r.skipped))}
                {item(`plan-show-${id}`, `Show all ${e.memberCount} photos`, () => p.onShowScene(id))}
                {item(`plan-undo-${id}`, "Undo apply", () => undo?.batch && void wf.undoBatch(undo.batch), !undo?.enabled, undo?.reason)}
              </div>
            );
          }}
        </Menu>
      </div>
    </article>
  );
}

/** The obvious way to edit: one preset + one adjusted photo -> the rest. Scene by scene (below) stays for refinements. */
function BaselineBanner({ run, flaggedLeft, onOpen, onFinish }: { run: BaselineRun | null; flaggedLeft: number | null; onOpen: () => void; onFinish?: () => void }) {
  const running = run?.state === "running";
  const undone = run?.state === "finished" && run.batch?.undoneAtMs != null;
  const finished = run?.state === "finished" && run.batch != null && run.batch.undoneAtMs == null;
  const left = flaggedLeft ?? run?.counts.flagged ?? 0;
  const checked = Math.max(0, (run?.counts.flagged ?? 0) - left);
  return (
    <div className="flex shrink-0 items-center gap-3 border-b border-emerald-900 bg-emerald-950/60 px-3 py-2 text-xs" data-testid="plan-baseline-banner" data-state={running ? "running" : finished ? "done" : undone ? "undone" : "new"}>
      <Layers className="size-5 shrink-0 text-emerald-400" aria-hidden />
      <div className="min-w-0 flex-1 text-neutral-200">
        <div className="text-sm font-semibold text-emerald-100">Baseline edit</div>
        <div className="truncate text-neutral-300" data-testid="plan-baseline-text">
          {running
            ? "Running now…"
            : finished
              ? `${run!.counts.applied + run!.counts.flagged} edited${left > 0 ? ` · ${left} need a look` : ""}${checked > 0 ? ` · ${checked} checked` : ""}. Open it to review the result, undo it or finish in Lightroom.`
              : undone
                ? "Undone. The photos are back to how they were."
                : "Pick a preset, adjust one photo, and Sieve edits the rest of the shoot the same way, each photo with its own light. Then finish in Lightroom."}
        </div>
      </div>
      {finished && onFinish && (
        <button className="flex h-8 shrink-0 items-center gap-1.5 whitespace-nowrap rounded-md bg-neutral-800 px-3 text-xs font-medium text-neutral-100 hover:bg-neutral-700" data-testid="plan-baseline-finish" title="Make sure the sidecars are saved and open the edit in Lightroom" onClick={onFinish}>
          Finish in Lightroom <ArrowRight className="size-3.5" />
        </button>
      )}
      <button
        className="flex h-8 shrink-0 items-center gap-1.5 whitespace-nowrap rounded-md bg-emerald-700 px-4 text-sm font-semibold text-white hover:bg-emerald-600"
        data-testid="plan-baseline"
        title="Start here: pick a preset, adjust one photo, then edit the rest of the shoot in one step (Cmd+Alt+B). Editing scene by scene below is for refinements"
        onClick={onOpen}
      >
        {finished || running ? "Open baseline edit" : undone ? "Run again" : "Start baseline edit"} <ArrowRight className="size-4" />
      </button>
    </div>
  );
}
