// Edit step overview: keepers grouped into scenes, one representative per scene, and a checklist
// (to do / edited / applied) with Auto edit (my style), Apply to scene and Apply all.
import { useEffect, useMemo, useRef, useState } from "react";
import { ArrowRight, ChevronDown, MoreHorizontal } from "lucide-react";
import type { KeeperRule } from "../../ipc";
import type { Library } from "../../hooks/useLibrary";
import type { SceneRow, Workflow } from "../../hooks/useWorkflow";
import { Menu, menuItem } from "../Menu";
import { Dialog } from "../Dialog";
import { AutoEditButton, StatusIcon, statusLine, Thumb, useWide } from "./bits";

type Tab = "all" | "todo" | "edited" | "applied";
const TABS: { id: Tab; label: string }[] = [
  { id: "all", label: "All" },
  { id: "todo", label: "To do" },
  { id: "edited", label: "Edited" },
  { id: "applied", label: "Applied" },
];
const inTab = (r: SceneRow, t: Tab) => t === "all" || (t === "todo" ? r.ui === "todo" : t === "applied" ? r.ui === "applied" : r.ui === "edited" || r.ui === "auto" || r.ui === "stale");

export const KEEPER_RULES: { rule: KeeperRule; label: string; long: string }[] = [
  { rule: { minRating: 1, useSuggestions: true }, label: "Picks, 1★+, suggested", long: "Picks, anything rated 1★ and up, and photos Sieve suggests" },
  { rule: { minRating: 1, useSuggestions: false }, label: "Picks, 1★+", long: "Picks and anything rated 1★ and up" },
  { rule: { minRating: 3, useSuggestions: false }, label: "Picks, 3★+", long: "Picks and photos rated 3★ and up" },
  { rule: { minRating: 5, useSuggestions: false }, label: "Picks, 5★", long: "Picks and 5★ photos" },
];
const ruleLabel = (r: KeeperRule) => KEEPER_RULES.find((x) => x.rule.minRating === r.minRating && x.rule.useSuggestions === r.useSuggestions)?.label ?? `${r.minRating}★+`;

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
  onReview: (sceneId: number) => void;
  onShowScene: (sceneId: number) => void;
  onChangeRep: (sceneId: number) => void;
  onApplyOptions: (sceneId: number) => void;
  onBackToCull: () => void;
  onContinueExport: () => void;
  onRegroup: () => void;
}

export function PlanView(p: Props) {
  const { wf } = p;
  const wide = useWide();
  const [tab, setTab] = useState<Tab>("all");
  const [confirmRegroup, setConfirmRegroup] = useState(false);
  const rows = wf.rows;
  const keeperCount = wf.plan?.keeperIds.length ?? 0;
  const todo = rows.filter((r) => r.ui === "todo");
  const pending = rows.filter((r) => r.ui === "edited" || r.ui === "auto" || r.ui === "stale");
  const applied = rows.filter((r) => r.ui === "applied");
  const pendingTargets = pending.reduce((a, r) => a + r.targets, 0);
  const allDone = rows.length > 0 && applied.length === rows.length;
  const visible = useMemo(() => rows.filter((r) => inTab(r, tab)), [rows, tab]);
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
      <Empty testid="plan-empty" title="No keepers yet." detail="Pick photos in Cull (P), or choose which photos count as keepers.">
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
        {visible.length === 0 && <p className="py-10 text-center text-sm text-neutral-400">No scenes in this tab.</p>}
        {visible.map((r) => (
          <SceneRowView key={r.entry.sceneId} r={r} p={p} memberShown={memberShown} focused={p.focusId === r.entry.sceneId} />
        ))}
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
              <div className="flex shrink-0 items-center gap-2 text-xs text-neutral-300" data-testid="plan-progress">
                <span className="flex h-1.5 w-24 shrink-0 overflow-hidden rounded-full bg-neutral-800 min-[1600px]:w-[200px]" aria-hidden>
                  <span className="h-full bg-emerald-500" style={{ width: `${(applied.length / rows.length) * 100}%` }} />
                  <span className="h-full bg-sky-500" style={{ width: `${(pending.length / rows.length) * 100}%` }} />
                </span>
                <span className="whitespace-nowrap" data-testid="plan-counts">
                  {pending.length} edited · {applied.length} applied · {todo.length} to do
                </span>
              </div>
            )}
          </>
        )}
        <div className="ml-auto flex shrink-0 items-center gap-2">
          {busy?.kind === "all" && (
            <span className="text-xs text-emerald-300" data-testid="plan-applying">
              Applying… {busy.done}/{busy.total}
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
          ) : (
            <button
              className="flex h-7 items-center gap-1.5 rounded-md bg-emerald-700 px-3 text-xs font-medium text-white hover:bg-emerald-600 disabled:opacity-40"
              data-testid="plan-apply-all"
              disabled={pending.length === 0 || busy != null}
              title={pending.length === 0 ? "Edit or auto edit a scene first" : "Copies each edited scene's edit to the rest of its scene, matching exposure and white balance per photo"}
              onClick={() => void wf.applyAll(p.onReview)}
            >
              Apply {plural(pending.length, "edited scene")} ({pendingTargets})
              <ArrowRight className="size-3.5" />
            </button>
          )}
          <Menu trigger={<MoreHorizontal className="size-4" />} triggerClass="flex h-7 items-center rounded-md bg-neutral-800 px-2 hover:bg-neutral-700" triggerTestId="plan-more" title="More" align="right">
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

      <div className="flex h-8 shrink-0 items-center gap-1 border-b border-neutral-800 px-3 text-xs" role="tablist">
        {TABS.map((t) => {
          const n = rows.filter((r) => inTab(r, t.id)).length;
          return (
            <button
              key={t.id}
              role="tab"
              aria-selected={tab === t.id}
              data-testid={`plan-tab-${t.id}`}
              onClick={() => setTab(t.id)}
              className={`rounded px-2 py-1 ${tab === t.id ? "bg-neutral-800 text-neutral-100" : "text-neutral-400 hover:text-neutral-200"}`}
            >
              {t.label} <span className="text-neutral-500">{n}</span>
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

function RuleItems({ current, onPick }: { current: KeeperRule; onPick: (r: KeeperRule) => void }) {
  return (
    <div className="w-80 py-1" data-testid="keeper-rule-menu">
      {KEEPER_RULES.map((k, i) => {
        const on = k.rule.minRating === current.minRating && k.rule.useSuggestions === current.useSuggestions;
        return (
          <button key={i} className={menuItem} data-testid={`keeper-rule-${i}`} aria-checked={on} role="menuitemradio" onClick={() => onPick(k.rule)}>
            <span className="w-4">{on ? "✓" : ""}</span>
            {k.long}
          </button>
        );
      })}
    </div>
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
  const line = statusLine(r);
  const t = p.sceneTimes(id);
  const repEntry = lib.getEntry(e.representativeId);
  const others = e.imageIds.filter((i) => i !== e.representativeId);
  const shown = others.slice(0, memberShown);
  const more = others.length - shown.length;
  const busyHere = wf.busy?.kind === "scene" && wf.busy.sceneId === id;
  const anyBusy = wf.busy != null;
  const last = wf.lastBatch && wf.lastBatch.sceneIds.includes(id) ? wf.lastBatch : null;
  const applyLabel = r.ui === "stale" ? `Re-apply to ${r.targets}` : `Apply to ${r.targets}`;
  const canApply = r.ui !== "todo" && r.targets > 0;
  const reviewSet = new Set(r.review);

  return (
    <article
      data-testid={`plan-scene-${id}`}
      data-status={r.ui}
      data-focused={focused}
      onClick={() => p.onFocus(id)}
      className={`flex h-[112px] items-center gap-3 rounded-lg border bg-neutral-900 px-3 min-[1600px]:h-[140px] ${focused ? "border-sky-500 ring-2 ring-sky-500" : "border-neutral-800"}`}
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
          {busyHere ? `Applying… ${wf.busy!.done}/${wf.busy!.total}` : line.text}
          {!busyHere && line.extra && <span className="text-amber-300">{line.extra}</span>}
        </p>
        <p className="truncate text-[11px] text-neutral-500" title={e.representativeReason}>
          {repEntry?.fileName ?? `#${e.representativeId}`}
        </p>
      </div>
      <div className="flex min-w-0 flex-1 items-center gap-1 overflow-hidden" data-testid={`plan-members-${id}`}>
        {shown.map((i) => (
          <div key={i} className="relative h-12 w-[72px] shrink-0 overflow-hidden rounded min-[1600px]:h-[60px] min-[1600px]:w-[90px]">
            <Thumb entry={lib.getEntry(i)} version={lib.version(i)} className="size-full" />
            {reviewSet.has(i) ? <span className="absolute bottom-0.5 right-0.5 rounded bg-black/70 px-1 text-[10px] font-bold text-amber-400">!</span> : r.ui === "applied" && <span className="absolute bottom-0.5 right-0.5 rounded bg-black/60 px-0.5 text-[10px] text-emerald-400">✓</span>}
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
        {(r.ui === "edited" || r.ui === "auto" || r.ui === "stale") && (
          <button
            className="h-7 whitespace-nowrap rounded-md bg-emerald-700 px-3 text-xs font-medium text-white hover:bg-emerald-600 disabled:opacity-40"
            data-testid={`plan-apply-${id}`}
            disabled={!canApply || anyBusy}
            title={canApply ? `Copies this edit to ${r.targets} photos, matching exposure and white balance to each.` : "No other keepers in this scene"}
            onClick={() => void wf.applyScene(id, "match", p.onReview)}
          >
            {applyLabel}
          </button>
        )}
        {r.ui === "applied" && r.review.length > 0 && (
          <button className="h-7 whitespace-nowrap rounded-md bg-amber-900/70 px-3 text-xs font-medium text-amber-100 hover:bg-amber-800" data-testid={`plan-review-${id}`} onClick={() => p.onReview(id)}>
            Review {r.review.length}
          </button>
        )}
        <button
          className={`h-7 whitespace-nowrap rounded-md px-3 text-xs font-medium ${r.ui === "todo" ? "bg-sky-700 text-white hover:bg-sky-600" : "bg-neutral-800 text-neutral-200 hover:bg-neutral-700"}`}
          data-testid={`plan-edit-${id}`}
          onClick={() => p.onEdit(id)}
        >
          {r.ui === "auto" ? "Review" : "Edit"} ▸
        </button>
        {(r.ui === "todo" || r.ui === "edited") && (
          <AutoEditButton style={wf.style} testid={`plan-auto-${id}`} label="Auto edit" disabled={anyBusy} onClick={() => wf.requestAutoEdit([id])} className="px-2" />
        )}
        <Menu trigger={<MoreHorizontal className="size-4" />} triggerClass="flex h-7 items-center rounded-md bg-neutral-800 px-1.5 hover:bg-neutral-700" triggerTestId={`plan-menu-${id}`} title="More for this scene" align="right">
          {(close) => {
            const item = (testid: string, label: string, fn: () => void, disabled = false) => (
              <button
                className={menuItem}
                data-testid={testid}
                disabled={disabled}
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
                {item(`plan-options-${id}`, "Apply with options…", () => p.onApplyOptions(id), r.ui === "todo")}
                {item(`plan-exact-${id}`, "Copy exactly (no matching)", () => void wf.applyScene(id, "exact", p.onReview), r.ui === "todo" || anyBusy || r.targets === 0)}
                {item(`plan-show-${id}`, `Show all ${e.memberCount} photos`, () => p.onShowScene(id))}
                {item(`plan-undo-${id}`, "Undo apply", () => last && void wf.undoBatch(last), !last)}
              </div>
            );
          }}
        </Menu>
      </div>
    </article>
  );
}
