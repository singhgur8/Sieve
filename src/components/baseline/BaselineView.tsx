// Baseline edit (roadmap Phase 10): one preset + one adjusted photo -> the whole shoot, then finish in Lightroom.
// Five steps: 1 Preset, 2 Anchor, 3 Adjust in Develop (leaves this view; the Baseline bar brings you back), 4 Edit the
// rest, 5 Finish in Lightroom.
import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowLeft, ArrowRight, Check, Layers } from "lucide-react";
import { commands, unwrap, type BaselineRun, type StylePreset } from "../../ipc";
import type { Library } from "../../hooks/useLibrary";
import { useStyleLibrary } from "../../hooks/useDevelopV14";
import type { BaselineSession } from "../../hooks/useBaseline";
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
  onUndo: (batch: { batchId: number; label: string }) => Promise<void>;
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
  // A finished run refreshes the library once (not for the run that was already finished when this view opened).
  const seen = useRef<number | null>(run?.state === "finished" ? run.id : null);
  useEffect(() => {
    if (run?.state === "finished" && seen.current !== run.id) {
      seen.current = run.id;
      p.onApplied(run);
    }
  }, [run]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || running || modalCount() > 0 || document.querySelector('[role="dialog"], [role="menu"]')) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      p.onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [running, p.onClose]); // eslint-disable-line react-hooks/exhaustive-deps

  const pickPreset = useCallback(
    (pr: StylePreset | null) => setSession((s) => ({ ...s, presetId: pr?.id ?? null, presetName: pr?.name ?? null })),
    [setSession],
  );

  const goDevelop = async () => {
    if (anchorId == null) return;
    setPreparing(true);
    try {
      const key = `${anchorId}:${session.presetId}`;
      if (session.presetId != null && session.appliedKey !== key) {
        const h = await unwrap(commands.getHistory(anchorId));
        if (h.appliedPresetId !== session.presetId) await unwrap(commands.applyPreset([anchorId], session.presetId));
      }
      setSession((s) => ({ ...s, anchorId, stage: "setup", appliedKey: s.presetId != null ? key : undefined }));
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
        <AnchorStep lib={p.lib} candidates={p.candidates} anchorId={anchorId} activeId={p.activeId} onPick={(id) => setSession((s) => ({ ...s, anchorId: id }))} />
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
          onUndo={() => (run?.batch ? p.onUndo({ batchId: run.batch.batchId, label: "Baseline Edit" }) : Promise.resolve())}
          onShowFlagged={p.onShowFlagged}
          onFinish={() => setStep(5)}
          onError={p.onError}
        />
      )}
      {step === 5 && <FinishStep onExport={p.onExport} onPlan={p.onClose} onError={p.onError} />}
      {anchorId == null && <p className="p-8 text-center text-sm text-neutral-400" data-testid="baseline-no-anchor">There are no keepers to use as the anchor yet. Pick photos in Cull first.</p>}

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
