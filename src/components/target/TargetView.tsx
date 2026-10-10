// "Pick the best N" (target-count culling, IPC v20): an overlay of the Cull step with four stages
// 1 Pick (count, shoot type, run, summary) -> 2 People -> 3 Review picks (pass 1) -> 4 Second look (pass 2), plus Apply.
// It owns the keyboard while open (keys come from the keymap rows of group "Pick the best N").
import { useCallback, useEffect, useRef, useState } from "react";
import { Check, ChevronRight, Flag, Undo2, X } from "lucide-react";
import {
  commands,
  unwrap,
  type CullSnapshot,
  type KeeperRule,
  type ShootType,
  type TargetApplyOptions,
  type TargetApplyPlan,
  type TargetApplyResult,
  type TargetChoice,
  type TargetEditResult,
  type TargetRun,
  type TargetSnapshot,
} from "../../ipc";
import { useEntries } from "../../hooks/useTarget";
import { useKeyboard } from "../../hooks/useKeyboard";
import { hint, matchTargetKey, type TargetStage } from "../../lib/keymap";
import { modalCount } from "../../lib/modal";
import { num, plural } from "../../lib/target";
import { Dialog } from "../Dialog";
import { HelpLink } from "../HelpLink";
import { ActionButton } from "./bits";
import { PeopleStep } from "./PeopleStep";
import { ReviewStep } from "./ReviewStep";
import { SecondStep } from "./SecondStep";
import { SetupStep, type StageRef } from "./SetupStep";
import type { TargetCtx } from "./types";

export type Stage = TargetStage;

export const PICKS_ONLY_RULE: KeeperRule = { mode: "picks_and_ratings", minRating: 1, useSuggestions: false };
const usesPicks = (r: KeeperRule | null) => !!r && r.mode === "picks_and_ratings" && !r.useSuggestions;

interface Props {
  projectId: number;
  shootType: ShootType;
  photoCount: number;
  run: TargetRun | null;
  refreshRun: () => Promise<void>;
  initialStage: Stage;
  /** Open with the Apply dialog (the Cull step's Best N row). */
  initialApply?: boolean;
  keeperRule: KeeperRule | null;
  onKeeperRule: (r: KeeperRule) => Promise<void>;
  /** Flags / selection changed: refresh the library and the Cull summary. */
  onChanged: () => void;
  onClose: () => void;
  onShowInGrid: (choices: TargetChoice[]) => void;
  onContinueEdit: () => void;
  /** v20.1: puts an apply on the Cull undo stack ("Apply Pick the best N"); returns its undo. */
  recordApply?: (previous: CullSnapshot[]) => () => Promise<void>;
  notify: (msg: string) => void;
  onError: (e: unknown) => void;
}

export function TargetView(p: Props) {
  const [stage, setStage] = useState<Stage>(p.initialStage);
  const [rev, setRev] = useState(0);
  const [undoLabel, setUndoLabel] = useState<string | null>(null);
  const [applyOpen, setApplyOpen] = useState(!!p.initialApply);
  const [focus, setFocus] = useState<{ id: number; stage: Stage; n: number } | null>(null);
  const stage_ = useRef(stage);
  stage_.current = stage;
  const stack = useRef<{ label: string; previous: TargetSnapshot[]; at?: number; stage: Stage }[]>([]);
  const stageRef = useRef<StageRef>(null);
  const entries = useEntries(p.onError);
  const changedTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const changed = () => {
    clearTimeout(changedTimer.current);
    changedTimer.current = setTimeout(p.onChanged, 350);
  };
  useEffect(() => () => clearTimeout(changedTimer.current), []);

  const edit = useCallback(
    async (label: string, pr: Promise<TargetEditResult>, at?: number) => {
      try {
        const stageNow = stage_.current;
        const r = await pr;
        if (r.previous.length > 0) {
          stack.current = [...stack.current, { label, previous: r.previous, at, stage: stageNow }].slice(-200);
          setUndoLabel(label);
        }
        entries.drop(r.previous.map((s) => s.imageId));
        setRev((n) => n + 1);
        void p.refreshRun();
        changed();
        return r;
      } catch (e) {
        p.onError(e);
        return null;
      }
    },
    [entries, p.refreshRun, p.onError], // eslint-disable-line react-hooks/exhaustive-deps
  );

  const undo = useCallback(async () => {
    if (stage_.current === "people") return p.notify("Change an answer with Y / N"); // people answers are not on the undo stack
    const top = stack.current[stack.current.length - 1];
    if (!top) return p.notify("Nothing to undo");
    stack.current = stack.current.slice(0, -1);
    setUndoLabel(stack.current[stack.current.length - 1]?.label ?? null);
    try {
      await unwrap(commands.restoreTargetSnapshot(top.previous));
      entries.drop(top.previous.map((s) => s.imageId));
      setRev((n) => n + 1);
      void p.refreshRun();
      changed();
      p.notify(`Undid: ${top.label}`);
      // Back to what was undone (P1-3): switch to its stage and select the photo.
      if (top.at != null && (top.stage === "review" || top.stage === "second")) {
        setStage(top.stage);
        setFocus({ id: top.at, stage: top.stage, n: Date.now() });
      }
    } catch (e) {
      p.onError(e);
    }
  }, [entries, p.refreshRun, p.notify, p.onError]); // eslint-disable-line react-hooks/exhaustive-deps

  useKeyboard((e) => {
    if (modalCount() > 0) return;
    const id = matchTargetKey(e, stage);
    if (!id) return;
    e.preventDefault();
    if (id === "targetUndo") return void undo();
    const used = stageRef.current?.key(id, e) ?? false;
    if (!used && id === "targetClose") p.onClose();
  });

  const run = p.run;
  const finished = run?.state === "finished" && run.counts.total > 0;
  const ctx: TargetCtx = {
    projectId: p.projectId,
    run,
    refreshRun: p.refreshRun,
    entries,
    edit,
    undo: () => void undo(),
    canUndo: undoLabel != null,
    rev,
    focus,
    focusDone: () => setFocus(null),
    notify: p.notify,
    onError: p.onError,
    showInGrid: p.onShowInGrid,
    go: setStage,
  };

  const tabs: { id: Stage; label: string; n: string; title: string; needsRun: boolean; badge?: number }[] = [
    { id: "setup", n: "1", label: "Pick", title: "Choose how many photos to deliver and run the selection", needsRun: false },
    { id: "people", n: "2", label: "People", title: "Confirm the couple and say which recurring people are important", needsRun: true, badge: run?.peopleQuestions },
    { id: "review", n: "3", label: "Review picks", title: "Go through the delivery set with the alternatives of each moment", needsRun: true },
    { id: "second", n: "4", label: "Second look", title: "Quickly check the Not sure and Set aside photos", needsRun: true },
  ];
  const c = run?.counts;

  return (
    <section className="absolute inset-0 z-10 flex flex-col bg-neutral-950" data-testid="target-view" data-stage={stage} aria-label="Pick the best N">
      <header className="flex h-12 shrink-0 items-center gap-2 border-b border-neutral-800 px-3" data-testid="target-header">
        <nav className="flex items-center gap-1" aria-label="Steps">
          {tabs.map((t, i) => {
            const disabled = t.needsRun && !finished;
            return (
              <span key={t.id} className="flex items-center gap-1">
                {i > 0 && <ChevronRight className="size-3.5 text-neutral-600" aria-hidden />}
                <button
                  className={`flex h-8 items-center gap-1.5 rounded-md px-2.5 text-sm ${stage === t.id ? "bg-neutral-800 text-neutral-50" : "text-neutral-300 hover:bg-neutral-900"} disabled:opacity-40`}
                  data-testid={`target-tab-${t.id}`}
                  data-active={stage === t.id}
                  aria-current={stage === t.id ? "step" : undefined}
                  disabled={disabled}
                  title={disabled ? "Run the selection first" : t.title}
                  onClick={() => setStage(t.id)}
                >
                  <span className="flex size-4 items-center justify-center rounded-full bg-neutral-700 text-[10px]">{t.n}</span>
                  {t.label}
                  {t.badge ? (
                    <span className="rounded-full bg-sky-700 px-1.5 text-[10px] text-white" data-testid="target-people-badge" title={`${plural(t.badge, "question")} still open`}>
                      {t.badge}
                    </span>
                  ) : null}
                </button>
              </span>
            );
          })}
        </nav>
        <HelpLink id="pick-best-n" title="How Pick the best N works" />
        <div className="ml-auto flex items-center gap-2">
          {c && finished && (
            <span className="text-xs text-neutral-300" data-testid="target-header-count" data-deliver={c.deliver} title="Photos in the delivery set now, and the target you asked for (a guideline)">
              Picked <b className="text-neutral-50">{num(c.deliver)}</b> · target {num(run!.settings.targetCount)}
            </span>
          )}
          <button
            className="flex h-8 items-center gap-1.5 rounded-md bg-neutral-800 px-2.5 text-sm hover:bg-neutral-700 disabled:opacity-40"
            data-testid="target-undo"
            disabled={undoLabel == null}
            title={undoLabel ? `Undo: ${undoLabel}${hint("targetUndo")}` : `Nothing to undo${hint("targetUndo")}`}
            onClick={() => void undo()}
          >
            <Undo2 className="size-4" /> Undo
          </button>
          <button
            className="flex h-8 items-center gap-1.5 rounded-md bg-emerald-700 px-3 text-sm font-medium text-white hover:bg-emerald-600 disabled:opacity-40"
            data-testid="target-apply"
            disabled={!finished}
            title="Write the picks to the photos' flags and XMP sidecars. Your own flags and stars are never changed; you can undo it"
            onClick={() => setApplyOpen(true)}
          >
            <Flag className="size-4" /> Apply flags…
          </button>
          <button className="flex size-8 items-center justify-center rounded-md hover:bg-neutral-800" data-testid="target-close" title={`Back to the grid${hint("targetClose")}`} aria-label="Back to the grid" onClick={p.onClose}>
            <X className="size-4" />
          </button>
        </div>
      </header>

      <div className="flex min-h-0 flex-1 flex-col">
        {stage === "setup" && <SetupStep ref={stageRef} ctx={ctx} shootType={p.shootType} photoCount={p.photoCount} />}
        {stage === "people" && <PeopleStep ref={stageRef} ctx={ctx} />}
        {stage === "review" && <ReviewStep ref={stageRef} ctx={ctx} />}
        {stage === "second" && <SecondStep ref={stageRef} ctx={ctx} />}
      </div>

      {applyOpen && run && (
        <ApplyDialog
          keeperRule={p.keeperRule}
          onClose={() => setApplyOpen(false)}
          onPlan={async (opts) => {
            try {
              return await unwrap(commands.planTargetApply(p.projectId, opts));
            } catch (e) {
              p.onError(e);
              return null;
            }
          }}
          onApply={async (opts) => {
            try {
              const r = await unwrap(commands.applyTargetSelection(p.projectId, opts));
              const undoApply = r.previous.length > 0 ? p.recordApply?.(r.previous) : undefined;
              await p.refreshRun();
              p.onChanged();
              const undo = async () => {
                try {
                  if (undoApply) await undoApply();
                  else await unwrap(commands.restoreCullSnapshot(r.previous));
                  await p.refreshRun();
                  p.onChanged();
                  return true;
                } catch (e) {
                  p.onError(e);
                  return false;
                }
              };
              return { result: r, undo };
            } catch (e) {
              p.onError(e);
              return null;
            }
          }}
          onKeeperRule={p.onKeeperRule}
          onContinueEdit={() => {
            setApplyOpen(false);
            p.onContinueEdit();
          }}
        />
      )}
    </section>
  );
}

/** The Apply step (v20.1): exact counts from `plan_target_apply`, an optional reject of clear defects, one-step undo. */
function ApplyDialog({
  keeperRule,
  onClose,
  onPlan,
  onApply,
  onKeeperRule,
  onContinueEdit,
}: {
  keeperRule: KeeperRule | null;
  onClose: () => void;
  onPlan: (opts: TargetApplyOptions) => Promise<TargetApplyPlan | null>;
  onApply: (opts: TargetApplyOptions) => Promise<{ result: TargetApplyResult; undo: () => Promise<boolean> } | null>;
  onKeeperRule: (r: KeeperRule) => Promise<void>;
  onContinueEdit: () => void;
}) {
  const [rejects, setRejects] = useState(true);
  const [plan, setPlan] = useState<TargetApplyPlan | null>(null);
  const [applied, setApplied] = useState<{ result: TargetApplyResult; undo: () => Promise<boolean> } | null>(null);
  const [undone, setUndone] = useState(false);
  const [busy, setBusy] = useState(false);
  const [ruleSet, setRuleSet] = useState(false);
  const offer = !usesPicks(keeperRule) && !ruleSet && !undone;
  useEffect(() => {
    let live = true;
    setPlan(null);
    void onPlan({ rejects }).then((r) => {
      if (live) setPlan(r);
    });
    return () => {
      live = false;
    };
  }, [rejects]); // eslint-disable-line react-hooks/exhaustive-deps
  const writes = plan ? plan.picks + plan.rejects + plan.unflags : 0;
  const result = applied?.result;
  return (
    <Dialog label="Apply the picks" testid="target-apply-dialog" className="w-[32rem] rounded-xl border border-neutral-700 bg-neutral-900 p-5 text-sm text-neutral-200 shadow-2xl" onCancel={onClose}>
      {!result ? (
        <>
          <h2 className="text-base font-semibold text-neutral-50">Apply the picks?</h2>
          <div className="mt-2 space-y-1.5 text-neutral-300" data-testid="target-apply-text" data-loading={plan == null}>
            {plan == null ? (
              <p>Counting…</p>
            ) : (
              <>
                <p data-testid="target-apply-picks" data-n={plan.picks}>
                  • <b className="text-neutral-50">{num(plan.picks)}</b> {plan.picks === 1 ? "photo gets" : "photos get"} the Picked flag.
                </p>
                {plan.unflags > 0 && (
                  <p data-testid="target-apply-unflags" data-n={plan.unflags}>
                    • {plural(plan.unflags, "photo")} {plan.unflags === 1 ? "loses" : "lose"} an earlier Sieve flag.
                  </p>
                )}
              </>
            )}
          </div>
          <label className="mt-2 flex items-start gap-2 text-neutral-300" title="Reject the photos Sieve is confident are defects (closed eyes, missed focus, blur). Untick to leave them unflagged">
            <input
              type="checkbox"
              className="mt-0.5"
              data-testid="target-apply-rejects"
              checked={rejects}
              title="Also reject the photos with clear defects"
              onChange={(e) => setRejects(e.target.checked)}
            />
            <span data-testid="target-apply-rejects-label" data-n={plan?.rejectable ?? ""}>
              Also reject <b className="text-neutral-50">{plan ? num(plan.rejectable) : "…"}</b> {plan?.rejectable === 1 ? "photo" : "photos"} with clear defects (closed eyes, missed focus, blur).
            </span>
          </label>
          <p className="mt-3 text-xs text-neutral-400" data-testid="target-apply-note">
            Your own flags{plan && plan.userFlagged > 0 ? ` (${num(plan.userFlagged)})` : ""} and all stars stay as they are. Flags go to the catalog and to the XMP sidecars next to your RAWs, where Lightroom reads them. You can undo it.
          </p>
          <div className="mt-4 flex justify-end gap-2">
            <ActionButton testid="target-apply-cancel" label="Cancel" keys={["Esc"]} title="Close without writing anything" onClick={onClose} />
            <ActionButton
              testid="target-apply-confirm"
              label={plan == null ? "Apply" : writes === 0 ? "Nothing to change" : `Apply to ${plural(writes, "photo")}`}
              keys={[]}
              tone="primary"
              disabled={busy || plan == null || writes === 0}
              title="Write these flags now (catalog and XMP sidecars)"
              onClick={async () => {
                setBusy(true);
                const r = await onApply({ rejects });
                setBusy(false);
                if (r) setApplied(r);
              }}
            />
          </div>
        </>
      ) : (
        <>
          <h2 className="flex items-center gap-2 text-base font-semibold text-neutral-50">
            {undone ? <Undo2 className="size-4 text-neutral-300" /> : <Check className="size-4 text-emerald-400" />} {undone ? "Apply undone" : "Picks applied"}
          </h2>
          <p className="mt-2 text-neutral-300" data-testid="target-apply-result" data-picks={result.picks} data-rejects={result.rejects} data-unflags={result.unflags}>
            {undone
              ? "The flags are back as they were before the apply."
              : `Picked ${num(result.picks)} · Rejected ${num(result.rejects)} · Unflagged ${num(result.unflags)}. ${plural(result.unchanged + result.userFlagged, "photo")} left as they were.`}
          </p>
          {offer && (
            <div className="mt-3 rounded-lg border border-sky-900 bg-sky-950/60 p-3" data-testid="target-keeper-offer">
              <p className="font-medium text-sky-100">Make the picks your keepers?</p>
              <p className="mt-1 text-xs text-sky-200/90" data-testid="target-keeper-explain">
                Keepers are what Edit and Export work on. Today that is everything not rejected; with &ldquo;Picks, 1★+&rdquo; it is just the picks (and anything you rated).
              </p>
              <div className="mt-2 flex gap-2">
                <ActionButton
                  testid="target-keeper-set"
                  label="Use picks as keepers"
                  keys={[]}
                  tone="primary"
                  title="Set the keeper rule to Picks, 1★+: the delivery set becomes the keepers"
                  onClick={async () => {
                    await onKeeperRule(PICKS_ONLY_RULE);
                    setRuleSet(true);
                  }}
                />
                <ActionButton testid="target-keeper-skip" label="Not now" keys={[]} title="Keep the current keeper rule" onClick={() => setRuleSet(true)} />
              </div>
            </div>
          )}
          {!offer && !undone && usesPicks(keeperRule) && (
            <p className="mt-3 text-xs text-emerald-300" data-testid="target-keeper-done">
              Keepers are now the picks.
            </p>
          )}
          <div className="mt-4 flex justify-end gap-2">
            {!undone && result.changed.length > 0 && (
              <ActionButton
                testid="target-apply-undo"
                label="Undo"
                keys={[]}
                disabled={busy}
                title="Put every flag this apply changed back as it was (also on the Cull undo stack: Cmd+Z in the grid)"
                onClick={async () => {
                  setBusy(true);
                  const ok = await applied!.undo();
                  setBusy(false);
                  if (ok) setUndone(true);
                }}
              />
            )}
            <ActionButton testid="target-apply-close" label="Close" keys={["Esc"]} title="Back to Pick the best N" onClick={onClose} />
            <ActionButton testid="target-continue-edit" label="Continue to Edit" keys={[]} tone="good" title="Group the keepers into scenes and edit one photo per scene" onClick={onContinueEdit} />
          </div>
        </>
      )}
    </Dialog>
  );
}
