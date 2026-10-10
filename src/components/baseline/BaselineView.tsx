// Baseline edit (roadmap Phase 10): one preset + one adjusted photo -> the whole shoot, then finish in Lightroom.
// Five steps: 1 Preset, 2 Anchor, 3 Adjust in Develop (leaves this view; the Baseline bar brings you back), 4 Edit the
// rest, 5 Finish in Lightroom.
import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowLeft, ArrowRight, Check, Layers } from "lucide-react";
import { BASELINE_LIGHT_FIELDS, BASELINE_LOOK_FIELDS, commands, copyAdjustmentFields, defaultAdjustments, unwrap, type BaselineRun, type StylePreset } from "../../ipc";
import type { Library } from "../../hooks/useLibrary";
import { useStyleLibrary } from "../../hooks/useDevelopV14";
import { lightEditedByUser, lookEditedAfterPreset, startAnchorFromAuto, type BaselineSession } from "../../hooks/useBaseline";
import { modalCount } from "../../lib/modal";
import { HelpLink } from "../HelpLink";
import { PresetStep } from "./PresetStep";
import { AnchorStep } from "./AnchorStep";
import { RestStep } from "./RestStep";
import { FinishStep } from "./FinishStep";

export type BaselineStepId = 1 | 2 | 3 | 4 | 5;

const STEPS: { id: BaselineStepId; label: string; title: string }[] = [
  { id: 1, label: "Preset", title: "Step 1: pick the preset that gives the whole shoot its look" },
  { id: 2, label: "Anchor photo", title: "Step 2: choose the photo you will adjust by hand" },
  { id: 3, label: "Adjust it", title: "Step 3: adjust the anchor photo in Develop, light and white balance first" },
  { id: 4, label: "Edit the rest", title: "Step 4: choose which photos to edit, check the preview and apply" },
  { id: 5, label: "Finish in Lightroom", title: "Step 5: make sure the sidecars are saved and open the edit in Lightroom" },
];

interface Props {
  projectId: number;
  lib: Library;
  initialStep: BaselineStepId;
  keeperIds: number[];
  /** One keeper per scene (the anchor candidates), best first. */
  candidates: number[];
  defaultAnchor: number | null;
  activeId: number | null;
  selectedIds: number[];
  totalPhotos: number;
  sceneLabel: (sceneId: number | null) => string;
  session: BaselineSession;
  setSession: (fn: (s: BaselineSession) => BaselineSession) => void;
  run: BaselineRun | null;
  onRunStarted: (r: BaselineRun) => void;
  onApplied: (r: BaselineRun) => void;
  onUndo: (batch: { batchId: number; label: string }, opts?: { keepLaterEdits: boolean }) => Promise<void>;
  /** Read the run (and whether its batch can still be undone) again. */
  onRefresh: () => void;
  /** Flagged photos the user has not marked "Looks good" yet. */
  reviewLeft: Set<number>;
  projectName: string;
  /** The project's folders (the sidecars are written next to the RAWs). */
  folders: string[];
  onClose: () => void;
  /** Step 3: open Develop on the anchor (the preset has been applied to it). */
  onDevelop: (anchorId: number) => void;
  onShowFlagged: () => void;
  onExport: () => void;
  onError: (e: unknown) => void;
}

export function BaselineView(p: Props) {
  const { session, setSession, run } = p;
  const [step, setStep] = useState<BaselineStepId>(p.initialStep);
  const [preparing, setPreparing] = useState(false);
  const [confirm, setConfirm] = useState<{ from: string; to: string } | null>(null);
  const styles = useStyleLibrary(p.onError);
  const anchorId = session.anchorId ?? p.defaultAnchor;
  const anchorName = anchorId != null ? (p.lib.getEntry(anchorId)?.fileName ?? `#${anchorId}`) : "";
  const running = run?.state === "running";

  // Fix the anchor in the session as soon as there is one.
  useEffect(() => {
    if (session.anchorId == null && p.defaultAnchor != null) setSession((s) => ({ ...s, anchorId: p.defaultAnchor }));
  }, [session.anchorId, p.defaultAnchor, setSession]);
  useEffect(() => p.lib.ensure(anchorId != null ? [anchorId] : []), [anchorId, p.lib.ensure]); // eslint-disable-line react-hooks/exhaustive-deps

  // A run that was set up earlier (or by an earlier session): show its preset by name.
  useEffect(() => {
    if (session.presetId == null || session.presetName != null) return;
    const name = styles.groups.flatMap((g) => g.presets).find((x) => x.id === session.presetId)?.name;
    if (name) setSession((s) => ({ ...s, presetName: name }));
  }, [styles.groups, session.presetId, session.presetName, setSession]);
  // Opened straight on "Edit the rest" with no session: the preset is the run's, else the one the anchor carries.
  useEffect(() => {
    if (p.initialStep < 4 || session.presetId != null || session.appliedKey || anchorId == null) return;
    const fromRun = run && run.settings.anchorId === anchorId ? run.settings.presetId : null;
    if (fromRun != null) return void setSession((x) => (x.presetId == null ? { ...x, presetId: fromRun } : x));
    let dead = false;
    unwrap(commands.getHistory(anchorId))
      .then((h) => !dead && h.appliedPresetId != null && setSession((x) => (x.presetId == null && !x.appliedKey ? { ...x, presetId: h.appliedPresetId ?? null } : x)))
      .catch(() => {});
    return () => {
      dead = true;
    };
  }, [anchorId]); // eslint-disable-line react-hooks/exhaustive-deps
  // The window title tells projects apart in Mission Control and the Window menu.
  useEffect(() => {
    const before = document.title;
    document.title = `${p.projectName} · Baseline edit · Sieve`;
    return () => {
      document.title = before;
    };
  }, [p.projectName]);
  // A finished run refreshes the library once (not for the run that was already finished when this view opened).
  const seen = useRef<number | null>(run?.state === "finished" ? run.id : null);
  useEffect(() => {
    if (run?.state === "finished" && seen.current !== run.id) {
      seen.current = run.id;
      p.onApplied(run);
    }
  }, [run]); // eslint-disable-line react-hooks/exhaustive-deps

  const confirmRef = useRef(confirm);
  confirmRef.current = confirm;
  const stepRef = useRef(step);
  stepRef.current = step;
  // The key handler is registered ONCE and reads the changing values through refs. Re-registering it on every render
  // (it used to depend on `onClose`, a new function each App render) lost a keypress now and then: another capture
  // listener's setState (App's Caps Lock sync) lets React flush the pending passive effects in the microtask between two
  // listeners of the same keydown, which removes this handler before it is reached and adds the new one too late to be
  // called for that event (a removed listener is skipped, a listener added during dispatch is not run for it).
  const runningRef = useRef(running);
  runningRef.current = running;
  const closeRef = useRef(p.onClose);
  closeRef.current = p.onClose;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const running = runningRef.current;
      const overlay = modalCount() > 0 || document.querySelector('[role="dialog"], [role="menu"]');
      if (e.key === "Escape") {
        if (running || overlay) return;
        e.preventDefault();
        e.stopImmediatePropagation();
        if (confirmRef.current) return void setConfirm(null);
        return closeRef.current();
      }
      if (e.key !== "Enter" || e.altKey || e.shiftKey || running || overlay) return;
      // Enter = the next step; Cmd+Enter = the primary action of the step (Edit N photos / Finish in Lightroom).
      const t = e.target as HTMLElement | null;
      const mod = e.metaKey || e.ctrlKey;
      const onTile = !!t?.closest("button[data-selected]");
      if (!mod && !onTile && t && t.closest("button, a, input, select, textarea, [role='radio']")) return;
      const click = (id: string) => {
        const el = document.querySelector<HTMLButtonElement>(`[data-testid="${id}"]`);
        if (!el || el.disabled) return false;
        e.preventDefault();
        e.stopPropagation();
        el.click();
        return true;
      };
      const st = stepRef.current;
      if (st === 1 || st === 2) void click("baseline-next");
      else if (st === 4 && mod) void (click("baseline-apply") || click("baseline-to-finish"));
      else if (st === 4) void click("baseline-to-finish");
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);

  const pickPreset = useCallback(
    (pr: StylePreset | null) => setSession((s) => ({ ...s, presetId: pr?.id ?? null, presetName: pr?.name ?? null })),
    [setSession],
  );

  // Changing the preset after one was applied: the look goes back to neutral first (confirmed when the user changed it).
  const goDevelop = async (confirmed = false) => {
    if (anchorId == null) return;
    setPreparing(true);
    try {
      const key = `${anchorId}:${session.presetId}`;
      const all = styles.groups.flatMap((g) => g.presets);
      const preset = session.presetId != null ? all.find((x) => x.id === session.presetId) : undefined;
      let presetLight = session.presetLight;
      let appliedPresetId = session.appliedPresetId;
      if (session.appliedKey !== key) {
        const h = await unwrap(commands.getHistory(anchorId));
        const sameAnchor = session.appliedKey?.startsWith(`${anchorId}:`) ?? false;
        const prev = (sameAnchor ? session.appliedPresetId : undefined) ?? h.appliedPresetId ?? null;
        if (session.presetId == null ? prev != null : h.appliedPresetId !== session.presetId) {
          if (prev != null && prev !== session.presetId) {
            if (!confirmed && lookEditedAfterPreset(h.entries)) {
              setConfirm({ from: all.find((x) => x.id === prev)?.name ?? "the earlier preset", to: session.presetName ?? "no preset" });
              return;
            }
            const cur = await unwrap(commands.getAdjustments(anchorId));
            // The user's own light stays; untouched light goes back to neutral so the new preset's values start clean.
            const fields = lightEditedByUser(h.entries) ? [...BASELINE_LOOK_FIELDS] : [...BASELINE_LOOK_FIELDS, ...BASELINE_LIGHT_FIELDS];
            await unwrap(commands.saveAdjustments(anchorId, copyAdjustmentFields(cur, defaultAdjustments(), fields), `Baseline preset: ${session.presetName ?? "none"}`));
          }
          if (session.presetId != null) await unwrap(commands.applyPreset([anchorId], session.presetId));
        }
        appliedPresetId = session.presetId;
        // The light starts at the anchor's own Auto (+ the preset's light values), so "same as Auto" is the zero point.
        const pl = await startAnchorFromAuto(p.projectId, anchorId, session.presetId, !!preset?.fields.includes("white_balance"));
        if (pl) presetLight = pl;
      }
      setConfirm(null);
      setSession((s) => ({ ...s, anchorId, stage: "setup", appliedKey: key, appliedPresetId, presetLight }));
      p.onDevelop(anchorId);
    } catch (e) {
      p.onError(e);
    } finally {
      setPreparing(false);
    }
  };

  const go = (s: BaselineStepId) => {
    if (s === 3) return void goDevelop();
    if (s >= 4) setSession((x) => ({ ...x, anchorId, stage: "rest" }));
    setStep(s);
  };

  const ready = anchorId != null;
  const sessionForRest: BaselineSession = { ...session, anchorId };

  return (
    <section className="absolute inset-0 z-20 flex flex-col bg-neutral-950" data-testid="baseline-view" data-step={step} aria-label="Baseline edit">
      <header className="flex h-12 shrink-0 items-center gap-4 border-b border-neutral-800 px-3">
        <h1 className="flex items-center gap-2 whitespace-nowrap text-sm font-semibold">
          <Layers className="size-4 text-emerald-400" aria-hidden /> Baseline edit
        </h1>
        <nav className="flex items-center gap-1" aria-label="Baseline edit steps" data-testid="baseline-steps">
          {STEPS.map((s, i) => {
            const on = step === s.id;
            const past = s.id < step || (s.id === 3 && step > 3);
            return (
              <div key={s.id} className="flex items-center gap-1">
                {i > 0 && <span className="h-px w-3 bg-neutral-700" aria-hidden />}
                <button
                  type="button"
                  data-testid={`baseline-step-${s.id}`}
                  aria-current={on ? "step" : undefined}
                  disabled={!ready || running || preparing}
                  title={s.title}
                  onClick={() => go(s.id)}
                  className={`flex h-7 items-center gap-1.5 whitespace-nowrap rounded-full px-2.5 text-xs disabled:opacity-40 ${on ? "bg-emerald-700 font-medium text-white" : "bg-neutral-900 text-neutral-300 hover:bg-neutral-800"}`}
                >
                  <span className={`flex size-4 items-center justify-center rounded-full text-[10px] ${on ? "bg-emerald-900" : "bg-neutral-800"}`}>{past ? <Check className="size-3" /> : s.id}</span>
                  {s.label}
                </button>
              </div>
            );
          })}
        </nav>
        <HelpLink id="baseline-edit" title="Help: how the baseline edit works" className="ml-1" />
        <button type="button" className="ml-auto flex h-7 items-center gap-1 rounded-md bg-neutral-800 px-2.5 text-xs hover:bg-neutral-700" data-testid="baseline-close" title="Back to the Edit plan (Esc)" onClick={p.onClose}>
          <ArrowLeft className="size-3.5" /> Plan
        </button>
      </header>

      {step === 1 && anchorId != null && (
        <PresetStep groups={styles.groups} anchorId={anchorId} anchorName={anchorName} presetId={session.presetId} onPick={pickPreset} onChangeAnchor={() => setStep(2)} onError={p.onError} />
      )}
      {step === 2 && anchorId != null && (
        <AnchorStep lib={p.lib} candidates={p.candidates} allIds={p.keeperIds} anchorId={anchorId} activeId={p.activeId} onPick={(id) => setSession((s) => ({ ...s, anchorId: id }))} />
      )}
      {step === 4 && anchorId != null && (
        <RestStep
          projectId={p.projectId}
          lib={p.lib}
          session={sessionForRest}
          anchorName={anchorName}
          keeperCount={p.keeperIds.length}
          totalCount={p.totalPhotos}
          selectedIds={p.selectedIds}
          sceneLabel={p.sceneLabel}
          run={run}
          onRunStarted={p.onRunStarted}
          reviewLeft={p.reviewLeft}
          onRefresh={p.onRefresh}
          onUndo={(o) => (run?.batch ? p.onUndo({ batchId: run.batch.batchId, label: "Baseline Edit" }, o) : Promise.resolve())}
          onShowFlagged={p.onShowFlagged}
          onFinish={() => setStep(5)}
          onError={p.onError}
        />
      )}
      {step === 5 && <FinishStep folders={p.folders} anchorId={anchorId} onExport={p.onExport} onPlan={p.onClose} onError={p.onError} />}
      {anchorId == null && <p className="p-8 text-center text-sm text-neutral-400" data-testid="baseline-no-anchor">There are no keepers to use as the anchor yet. Pick photos in Cull first.</p>}

      {confirm && (
        <div className="flex shrink-0 items-center gap-3 border-t border-amber-900 bg-amber-950 px-4 py-2 text-xs text-amber-100" role="alert" data-testid="baseline-replace-confirm">
          <span>
            Replace {confirm.from} and your colour changes on {anchorName} with {confirm.to}? Your light and white balance stay.
          </span>
          <button type="button" className="ml-auto h-7 rounded-md bg-amber-700 px-3 font-medium text-white hover:bg-amber-600" data-testid="baseline-replace-ok" title="Replace the look on the anchor photo; you can still undo each step in Develop" onClick={() => void goDevelop(true)}>
            Replace
          </button>
          <button type="button" className="h-7 rounded-md bg-neutral-800 px-3 hover:bg-neutral-700" data-testid="baseline-replace-cancel" title="Keep the look the anchor has now (Esc)" onClick={() => setConfirm(null)}>
            Cancel
          </button>
        </div>
      )}

      {(step === 1 || step === 2) && anchorId != null && (
        <footer className="flex h-14 shrink-0 items-center gap-3 border-t border-neutral-800 px-4 text-xs">
          <span className="text-neutral-400" data-testid="baseline-choice">
            Preset: <b className="text-neutral-200">{session.presetName ?? "none"}</b> · Anchor: <b className="text-neutral-200">{anchorName}</b>
          </span>
          {step === 2 && (
            <button type="button" className="ml-auto h-8 rounded-md bg-neutral-800 px-3 hover:bg-neutral-700" data-testid="baseline-back" title="Back to the preset" onClick={() => setStep(1)}>
              Back
            </button>
          )}
          <button
            type="button"
            className={`${step === 1 ? "ml-auto" : ""} flex h-8 items-center gap-1.5 rounded-md bg-emerald-700 px-4 text-sm font-medium text-white hover:bg-emerald-600 disabled:opacity-40`}
            data-testid="baseline-next"
            disabled={preparing}
            title={step === 1 ? "Next: choose the photo you will adjust" : "Open the anchor photo in Develop with the preset applied. Adjust light and white balance first"}
            onClick={() => (step === 1 ? setStep(2) : void goDevelop())}
          >
            {step === 1 ? "Next: choose the anchor photo" : preparing ? "Applying the preset…" : "Adjust it in Develop"} <ArrowRight className="size-4" />
          </button>
        </footer>
      )}
    </section>
  );
}
