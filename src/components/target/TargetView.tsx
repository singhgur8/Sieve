// "Pick the best N" (target-count culling, IPC v20): an overlay of the Cull step with four stages
// 1 Pick (count, shoot type, run, summary) -> 2 People -> 3 Review picks (pass 1) -> 4 Second look (pass 2), plus Apply.
// It owns the keyboard while open (keys come from the keymap rows of group "Pick the best N").
import { useCallback, useEffect, useRef, useState } from "react";
import { Check, ChevronRight, Flag, Undo2, X } from "lucide-react";
import { commands, unwrap, type KeeperRule, type ShootType, type TargetChoice, type TargetEditResult, type TargetRun, type TargetSnapshot } from "../../ipc";
import { useEntries } from "../../hooks/useTarget";
import { useKeyboard } from "../../hooks/useKeyboard";
import { hint, matchTargetKey, type TargetStage } from "../../lib/keymap";
import { modalCount } from "../../lib/modal";
import { plural } from "../../lib/target";
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
  keeperRule: KeeperRule | null;
  onKeeperRule: (r: KeeperRule) => Promise<void>;
  /** Flags / selection changed: refresh the library and the Cull summary. */
  onChanged: () => void;
  onClose: () => void;
  onShowInGrid: (choices: TargetChoice[]) => void;
  onContinueEdit: () => void;
  notify: (msg: string) => void;
  onError: (e: unknown) => void;
}

const num = (n: number) => n.toLocaleString("en-US");

export function TargetView(p: Props) {
  const [stage, setStage] = useState<Stage>(p.initialStage);
  const [rev, setRev] = useState(0);
  const [undoLabel, setUndoLabel] = useState<string | null>(null);
  const [applyOpen, setApplyOpen] = useState(false);
  const stack = useRef<{ label: string; previous: TargetSnapshot[] }[]>([]);
  const stageRef = useRef<StageRef>(null);
  const entries = useEntries(p.onError);
  const changedTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const changed = () => {
    clearTimeout(changedTimer.current);
    changedTimer.current = setTimeout(p.onChanged, 350);
  };
  useEffect(() => () => clearTimeout(changedTimer.current), []);

  const edit = useCallback(
    async (label: string, pr: Promise<TargetEditResult>) => {
      try {
        const r = await pr;
        if (r.previous.length > 0) {
          stack.current = [...stack.current, { label, previous: r.previous }].slice(-200);
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
          run={run}
          keeperRule={p.keeperRule}
          onClose={() => setApplyOpen(false)}
          onApply={async () => {
            try {
              const r = await unwrap(commands.applyTargetSelection(p.projectId));
              await p.refreshRun();
              p.onChanged();
              return r;
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

function ApplyDialog({
  run,
  keeperRule,
  onClose,
  onApply,
  onKeeperRule,
  onContinueEdit,
}: {
  run: TargetRun;
  keeperRule: KeeperRule | null;
  onClose: () => void;
  onApply: () => Promise<{ applied: number; skipped: number } | null>;
  onKeeperRule: (r: KeeperRule) => Promise<void>;
  onContinueEdit: () => void;
}) {
  const [result, setResult] = useState<{ applied: number; skipped: number } | null>(null);
  const [busy, setBusy] = useState(false);
  const [ruleSet, setRuleSet] = useState(false);
  const offer = !usesPicks(keeperRule) && !ruleSet;
  return (
    <Dialog label="Apply the picks" testid="target-apply-dialog" className="w-[30rem] rounded-xl border border-neutral-700 bg-neutral-900 p-5 text-sm text-neutral-200 shadow-2xl" onCancel={onClose}>
      {!result ? (
        <>
          <h2 className="text-base font-semibold text-neutral-50">Apply the picks?</h2>
          <p className="mt-2 text-neutral-300" data-testid="target-apply-text">
            This marks the {num(run.counts.deliver)} picked photos as Picked in the catalog and in their XMP sidecars. Photos you flagged or starred yourself are never changed, and nothing is rejected or deleted.
          </p>
          <div className="mt-4 flex justify-end gap-2">
            <ActionButton testid="target-apply-cancel" label="Cancel" keys={["Esc"]} title="Close without writing anything" onClick={onClose} />
            <ActionButton
              testid="target-apply-confirm"
              label="Apply"
              keys={[]}
              tone="primary"
              disabled={busy}
              title="Write the pick flags now"
              onClick={async () => {
                setBusy(true);
                const r = await onApply();
                setBusy(false);
                if (r) setResult(r);
              }}
            />
          </div>
        </>
      ) : (
        <>
          <h2 className="flex items-center gap-2 text-base font-semibold text-neutral-50">
            <Check className="size-4 text-emerald-400" /> Picks applied
          </h2>
          <p className="mt-2 text-neutral-300" data-testid="target-apply-result">
            Flagged {plural(result.applied, "photo")} as Picked{result.skipped > 0 ? `; ${num(result.skipped)} left as they were (your own flags, or already right)` : ""}.
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
          {!offer && usesPicks(keeperRule) && (
            <p className="mt-3 text-xs text-emerald-300" data-testid="target-keeper-done">
              Keepers are now the picks.
            </p>
          )}
          <div className="mt-4 flex justify-end gap-2">
            <ActionButton testid="target-apply-close" label="Close" keys={["Esc"]} title="Back to Pick the best N" onClick={onClose} />
            <ActionButton testid="target-continue-edit" label="Continue to Edit" keys={[]} tone="good" title="Group the keepers into scenes and edit one photo per scene" onClick={onContinueEdit} />
          </div>
        </>
      )}
    </Dialog>
  );
}
