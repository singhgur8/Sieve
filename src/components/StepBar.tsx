// Guided-workflow step bar of the open project: 1 Cull > 2 Edit > 3 Export. Steps are never locked; "done" follows the
// persisted project step. Sub-counts: analysis / keepers, scenes applied, export progress.
import { Check, ChevronRight } from "lucide-react";
import type { WorkflowStep } from "../ipc";
import { hint } from "../lib/keymap";

export type StepState = "active" | "done" | "upcoming";

interface Props {
  /** Persisted step of the project. */
  step: WorkflowStep;
  /** The Export dialog is open: the Export pill is the active one. */
  exporting: boolean;
  cullSub: string;
  editSub: string | null;
  exportSub: string;
  /** 0-100 while an export runs (ring on the Export pill). */
  exportPct: number | null;
  /** Every scene applied: the Edit pill is done even while Edit is the current step. */
  editDone: boolean;
  onStep: (s: WorkflowStep) => void;
  /** The keeper formula ("Keepers 412 = ..."), appended to the hover text of every step that works on the keepers. */
  keeperNote?: string;
}

const ORDER: WorkflowStep[] = ["cull", "edit", "export"];
const LABEL: Record<WorkflowStep, string> = { cull: "Cull", edit: "Edit", export: "Export" };
const TITLE: Record<WorkflowStep, string> = {
  cull: "Cull: flag, star and reject",
  edit: "Edit: one photo per scene, then apply",
  export: "Export keepers",
};
const KEY: Record<WorkflowStep, "stepCull" | "stepEdit" | "stepExport"> = { cull: "stepCull", edit: "stepEdit", export: "stepExport" };

function Ring({ pct }: { pct: number }) {
  const r = 6;
  const c = 2 * Math.PI * r;
  return (
    <svg viewBox="0 0 16 16" className="size-4 -rotate-90" data-testid="export-ring" data-pct={Math.round(pct)}>
      <circle cx="8" cy="8" r={r} fill="none" stroke="currentColor" strokeOpacity="0.25" strokeWidth="2" />
      <circle cx="8" cy="8" r={r} fill="none" stroke="#fbbf24" strokeWidth="2" strokeDasharray={c} strokeDashoffset={c * (1 - pct / 100)} strokeLinecap="round" />
    </svg>
  );
}

export function StepBar(p: Props) {
  const current = p.exporting ? 2 : ORDER.indexOf(p.step);
  const subs: Record<WorkflowStep, string | null> = { cull: p.cullSub, edit: p.editSub, export: p.exportSub };
  return (
    <nav className="flex items-center gap-1" aria-label="Workflow steps" data-testid="step-bar">
      {ORDER.map((s, i) => {
        const state: StepState = i === current ? "active" : i < current || (s === "edit" && p.editDone && i < 2 && current > 0) ? "done" : "upcoming";
        const sub = subs[s];
        return (
          <div key={s} className="flex items-center gap-1">
            {i > 0 && <ChevronRight className="size-4 shrink-0 text-neutral-600" aria-hidden />}
            <button
              onClick={() => p.onStep(s)}
              data-testid={`step-${s}`}
              data-state={state}
              aria-current={state === "active" ? "step" : undefined}
              title={`${TITLE[s]}${hint(KEY[s])}${p.keeperNote ? `. ${p.keeperNote}` : ""}${state === "done" ? " (done)" : ""}`}
              className={`flex h-7 items-center gap-1.5 whitespace-nowrap rounded-full px-3 text-xs outline-none focus-visible:ring-2 focus-visible:ring-sky-400 ${
                state === "active" ? "bg-sky-800 text-sky-50" : "bg-neutral-800 hover:bg-neutral-700"
              } ${state === "done" ? "text-neutral-200" : state === "upcoming" ? "text-neutral-300" : ""}`}
            >
              <span
                className={`flex size-4 shrink-0 items-center justify-center rounded-full text-[10px] font-bold ${
                  state === "active" ? "bg-white text-sky-900" : state === "done" ? "" : "bg-neutral-600 text-neutral-200"
                }`}
              >
                {s === "export" && p.exportPct != null ? <Ring pct={p.exportPct} /> : state === "done" ? <Check className="size-4 text-emerald-400" strokeWidth={3} aria-label="Done" /> : i + 1}
              </span>
              <span className="font-medium">{LABEL[s]}</span>
              {sub && (
                <span className={`${state === "active" ? "text-sky-200" : "text-neutral-400"} max-[1599px]:hidden`} data-testid={`step-${s}-sub`}>
                  · {sub}
                </span>
              )}
            </button>
          </div>
        );
      })}
    </nav>
  );
}
