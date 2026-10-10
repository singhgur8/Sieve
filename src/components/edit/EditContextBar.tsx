// 32 px bar under the TopBar while Developing in the Edit step: which scene, its state, and Auto edit / Apply to scene.
import { ChevronDown, ChevronLeft, ChevronRight, ListChecks, X } from "lucide-react";
import type { SceneRow, Workflow } from "../../hooks/useWorkflow";
import { hint } from "../../lib/keymap";
import { BUSY_WHY, useActivityRunning } from "../../lib/activity";
import { Menu, menuItem } from "../Menu";
import { ApplyWhy, AutoEditButton, StatusIcon, applyBlock } from "./bits";

interface Props {
  wf: Workflow;
  rows: SceneRow[];
  activeId: number | null;
  fileName: (id: number) => string;
  onJump: (sceneId: number) => void;
  onStepScene: (dir: 1 | -1) => void;
  onPlan: () => void;
  onMakeRep: (imageId: number) => void;
  onApplyOptions: (sceneId: number) => void;
  onReview: (sceneId: number, ids?: number[]) => void;
  /** "Show" on the apply toast: the photos Apply skipped because the user edited them. */
  onShowIds: (ids: number[], label: string) => void;
  onNextReview: () => void;
  /** The Baseline bar is shown above (step 3 of the baseline edit): the old scene flow (Apply / Auto edit) stays out of the way. */
  baselineBar?: boolean;
  /** Representatives whose settings are still what a baseline run wrote: nothing to apply from them. */
  onBaseline?: Set<number>;
  /** Reviewing the photos the baseline flagged: Apply to scene is a neutral, explained action. */
  flaggedReview?: boolean;
}

const REP_CHIP: Record<SceneRow["ui"], { text: string; cls: string }> = {
  todo: { text: "To do · representative", cls: "bg-neutral-800 text-neutral-200" },
  auto: { text: "Auto edited, review · representative", cls: "bg-amber-950 text-amber-200" },
  edited: { text: "Edited · representative", cls: "bg-sky-950 text-sky-200" },
  applied: { text: "Applied · representative", cls: "bg-emerald-950 text-emerald-200" },
  stale: { text: "Changed since applied · representative", cls: "bg-amber-950 text-amber-200" },
  reset: { text: "Reset since applied · representative", cls: "bg-amber-950 text-amber-200" },
};

export function EditContextBar(p: Props) {
  const { wf, rows } = p;
  const row = p.activeId != null ? rows.find((r) => r.entry.imageIds.includes(p.activeId!)) : undefined;
  const idx = row ? rows.indexOf(row) : -1;
  const isRep = !!row && row.entry.representativeId === p.activeId;
  const applying = useActivityRunning("apply_scene");
  const busy = wf.busy != null || applying;
  const undo = row ? wf.sceneUndo(row.entry.sceneId) : null;
  const canApply = !!row && row.ui !== "todo" && row.ui !== "reset" && row.targets > 0 && !row.skipped;
  const block0 = applyBlock(row);
  // Opening the representative is pointless while it is the photo on screen.
  const block = block0 && !block0.soft ? (isRep && block0.fix === "open" ? { ...block0, fix: undefined } : block0) : null;
  const newKeepers = row && row.ui === "applied" ? row.unapplied.length : 0;
  const repOnBaseline = !!row && !!p.onBaseline?.has(row.entry.representativeId);
  const sceneFlow = !p.baselineBar;
  const showApply = sceneFlow && !repOnBaseline;

  let chip: React.ReactNode = <span className="rounded-full bg-neutral-800 px-2 py-0.5 text-xs text-neutral-300">Not in a scene</span>;
  let hintText: string | null = null;
  if (row && p.activeId != null) {
    const num = row.number;
    if (isRep) {
      const c = REP_CHIP[row.ui];
      chip = repOnBaseline ? (
        <span className="rounded-full bg-emerald-950 px-2 py-0.5 text-xs text-emerald-200" data-testid="edit-chip" data-kind="rep">
          On baseline · representative
        </span>
      ) : (
        <span className={`rounded-full px-2 py-0.5 text-xs ${c.cls}`} data-testid="edit-chip" data-kind="rep">
          {c.text}
        </span>
      );
      if (!sceneFlow || repOnBaseline) hintText = null;
      else if (row.ui === "reset") hintText = "Edit this photo first";
      else if (row.ui === "todo" || row.ui === "edited") hintText = `Edit this photo, then apply it to the other ${row.targets}.`;
    } else if (wf.needsReviewSet.has(p.activeId)) {
      const reason = wf.stateById.get(p.activeId)?.reviewReason;
      chip = (
        <>
          <span className="max-w-[420px] truncate rounded-full bg-amber-950 px-2 py-0.5 text-xs text-amber-200" data-testid="edit-chip" data-kind="review" title={reason ?? undefined}>
            Needs a look: {reason ? reason.charAt(0).toLowerCase() + reason.slice(1) : "exposure did not match"}
          </span>
          <button className="rounded bg-neutral-800 px-2 py-0.5 text-xs text-emerald-200 hover:bg-neutral-700" onClick={() => void wf.markReviewed([p.activeId!])} data-testid="edit-looks-good" title="Keep the settings and clear the mark. Cmd+Enter does this and goes to the next photo that needs a look">
            Looks good
          </button>
          <button className="text-xs text-sky-300 hover:underline" onClick={p.onNextReview} data-testid="edit-next-review" title="Next photo that needs a look (N)">
            Next to review ›
          </button>
        </>
      );
    } else if (row.entry.appliedIds.includes(p.activeId)) {
      chip = (
        <span className="rounded-full bg-emerald-950 px-2 py-0.5 text-xs text-emerald-200" data-testid="edit-chip" data-kind="applied">
          From Scene {num}&apos;s edit ✓
        </span>
      );
    } else {
      chip = (
        <>
          <span className="rounded-full bg-neutral-800 px-2 py-0.5 text-xs text-neutral-300" data-testid="edit-chip" data-kind="member">
            Not the representative
          </span>
          <button className="text-xs text-sky-300 hover:underline" onClick={() => p.onMakeRep(p.activeId!)} data-testid="edit-make-rep" title="Make this photo the one you edit for the scene (Shift+A)">
            Make it the representative (Shift+A)
          </button>
        </>
      );
    }
  }

  return (
    <div className="flex h-8 shrink-0 items-center gap-2 border-b border-neutral-800 bg-neutral-900 px-3" data-testid="edit-context">
      <button className="rounded p-0.5 text-neutral-300 hover:bg-neutral-800 disabled:opacity-30" disabled={rows.length < 2} onClick={() => p.onStepScene(-1)} aria-label="Previous scene" title="Previous scene (Shift+N)" data-testid="edit-prev-scene">
        <ChevronLeft className="size-4" />
      </button>
      <Menu
        trigger={
          <>
            {row ? `Scene ${row.number} of ${rows.length}` : `${rows.length} scenes`} <ChevronDown className="size-3.5" />
          </>
        }
        triggerClass="flex h-6 w-[150px] items-center justify-center gap-1 rounded bg-neutral-800 px-2 text-xs hover:bg-neutral-700"
        triggerTestId="edit-scene-menu"
        title="Scene checklist"
      >
        {(close) => (
          <div className="max-h-[480px] w-[360px] overflow-y-auto py-1" data-testid="edit-checklist">
            {rows.map((r) => (
              <button
                key={r.entry.sceneId}
                className={`${menuItem} h-9`}
                data-testid={`edit-checklist-${r.entry.sceneId}`}
                data-status={r.ui}
                onClick={() => {
                  close();
                  p.onJump(r.entry.sceneId);
                }}
              >
                <StatusIcon ui={r.ui} className="size-4" />
                <span className="w-16 shrink-0">Scene {r.number}</span>
                <span className="w-8 shrink-0 text-right text-xs text-neutral-400">{r.entry.imageIds.length}</span>
                <span className="min-w-0 flex-1 truncate pl-2 text-xs text-neutral-400">{p.fileName(r.entry.representativeId)}</span>
              </button>
            ))}
            <button
              className={`${menuItem} border-t border-neutral-800`}
              onClick={() => {
                close();
                p.onPlan();
              }}
            >
              <ListChecks className="size-4" /> Open plan <span className="ml-auto text-xs text-neutral-400">G</span>
            </button>
          </div>
        )}
      </Menu>
      <button className="rounded p-0.5 text-neutral-300 hover:bg-neutral-800 disabled:opacity-30" disabled={rows.length < 2} onClick={() => p.onStepScene(1)} aria-label="Next scene to edit" title="Next scene to edit (N)" data-testid="edit-next-scene">
        <ChevronRight className="size-4" />
      </button>
      <div className="flex min-w-0 items-center gap-2 overflow-hidden whitespace-nowrap">{chip}</div>
      {hintText && <span className={`min-w-0 truncate text-xs text-neutral-400 ${row?.ui === "reset" ? "" : "max-[1439px]:hidden"}`}>{hintText}</span>}
      <div className="ml-auto flex shrink-0 items-center gap-2">
        {wf.busy?.kind === "scene" && (
          <span className="flex h-6 items-center rounded bg-emerald-950 px-2 text-xs text-emerald-200" data-testid="edit-applying">
            {wf.cancelling ? "Stopping…" : (wf.busy.total > 0 ? `Applying… ${wf.busy.done}/${wf.busy.total}` : "Applying…")}
            <button className="ml-1 rounded p-0.5 hover:bg-emerald-900 disabled:opacity-40" data-testid="edit-apply-cancel" aria-label="Stop applying" title="Stop applying. Scenes already applied stay applied" disabled={wf.cancelling} onClick={wf.cancelApply}>
              <X className="size-3" />
            </button>
          </span>
        )}
        {sceneFlow && (
          <AutoEditButton
            style={wf.style}
            testid="edit-auto"
            label="Auto edit (my style)"
            disabled={!row || busy}
            onClick={() => row && wf.requestAutoEdit([row.entry.sceneId])}
          />
        )}
        {showApply && (
          <ApplyWhy
            block={block && !applying ? block : null}
            testid="edit-apply-why"
            className="max-w-[300px]"
            onFix={(fix) => row && (fix === "open" ? p.onJump(row.entry.sceneId) : void wf.setSkipped(row.entry.sceneId, false))}
          />
        )}
        {showApply && <div className="flex">
          <button
            className={`flex h-6 items-center whitespace-nowrap rounded-l-md px-3 text-xs font-medium text-white disabled:opacity-40 ${p.flaggedReview ? "bg-neutral-700 hover:bg-neutral-600" : "bg-emerald-700 hover:bg-emerald-600"}`}
            data-testid="edit-apply"
            disabled={!canApply || busy}
            title={p.flaggedReview ? "Copies this photo's edit over its scene and replaces the baseline light of those photos" : applying ? BUSY_WHY.apply_scene : row?.ui === "reset" ? "Edit this photo first" : row?.ui === "todo" ? "Edit this photo or auto edit it first" : `Apply this scene's edit to the other keepers, matching exposure and white balance${hint("applyScene")}`}
            onClick={() => row && void wf.applyScene(row.entry.sceneId, "match", p.onReview, p.onShowIds)}
          >
            {newKeepers > 0 ? `Apply to ${newKeepers} new` : `Apply to scene${row ? ` (${row.targets})` : ""}`}
          </button>
          <Menu
            trigger={<ChevronDown className="size-3.5" />}
            triggerClass={`flex h-6 items-center rounded-r-md border-l px-1.5 text-white disabled:opacity-40 ${p.flaggedReview ? "border-neutral-600 bg-neutral-700 hover:bg-neutral-600" : "border-emerald-800 bg-emerald-700 hover:bg-emerald-600"}`}
            triggerTestId="edit-apply-menu"
            title="More ways to apply"
            align="right"
            disabled={!row}
          >
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
                <div className="w-60 py-1">
                  {item("edit-apply-options", "Apply with options…", () => row && p.onApplyOptions(row.entry.sceneId), !canApply)}
                  {item("edit-apply-exact", "Copy exactly (no matching)", () => row && void wf.applyScene(row.entry.sceneId, "exact", p.onReview, p.onShowIds), !canApply || busy)}
                  {item("edit-undo-apply", "Undo apply", () => undo?.batch && void wf.undoBatch(undo.batch), !undo?.enabled, undo?.reason)}
                </div>
              );
            }}
          </Menu>
        </div>}
        <button className="flex h-6 items-center gap-1 rounded bg-neutral-800 px-2 text-xs hover:bg-neutral-700" onClick={p.onPlan} data-testid="edit-plan" title="Back to the plan (G)">
          <ListChecks className="size-3.5" /> Plan
        </button>
      </div>
      {idx >= 0 && <span className="sr-only">Scene {idx + 1}</span>}
    </div>
  );
}
