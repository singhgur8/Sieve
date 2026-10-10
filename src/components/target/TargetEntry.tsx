// Entry points of "Pick the best N" in the Cull step: the offer shown first for a project that has not been culled, and the
// toolbar button that reopens the flow.
import { useState } from "react";
import { useWide } from "../edit/bits";
import { Flag, Sparkles, X } from "lucide-react";
import type { TargetRun } from "../../ipc";
import { num, runProgress, suggestTarget } from "../../lib/target";

const key = (projectId: number) => `sieve.target.offer.dismissed.${projectId}`;
const dismissed = (projectId: number) => {
  try {
    return localStorage.getItem(key(projectId)) === "1";
  } catch {
    return false;
  }
};

/** True when the project has a finished target run with chosen photos. */
export const hasTargetRun = (run: TargetRun | null) => !!run && run.state === "finished" && run.counts.total > 0;

export function TargetOffer({ projectId, photoCount, onOpen }: { projectId: number; photoCount: number; onOpen: () => void }) {
  const [hidden, setHidden] = useState(() => dismissed(projectId));
  if (hidden || photoCount < 2) return null;
  return (
    <div className="flex shrink-0 items-center gap-3 border-b border-sky-900 bg-sky-950/70 px-3 py-1.5 text-xs text-sky-100" data-testid="target-offer">
      <Sparkles className="size-4 shrink-0 text-sky-300" />
      <span className="min-w-0 flex-1 truncate">
        Start here: let Sieve pick the best ~{suggestTarget(photoCount).toLocaleString("en-US")} of {photoCount.toLocaleString("en-US")} photos, then review only the picks and their alternatives instead of every frame.
      </span>
      <button className="whitespace-nowrap rounded bg-sky-700 px-2.5 py-1 font-medium text-white hover:bg-sky-600" data-testid="target-offer-open" title="Choose how many photos to deliver; Sieve picks them and you review by exception" onClick={onOpen}>
        Pick the best N…
      </button>
      <button
        className="rounded p-1 hover:bg-sky-900"
        data-testid="target-offer-dismiss"
        aria-label="Dismiss"
        title="Hide this suggestion for this project. Pick the best N stays in the toolbar"
        onClick={() => {
          try {
            localStorage.setItem(key(projectId), "1");
          } catch {
            /* ignore */
          }
          setHidden(true);
        }}
      >
        <X className="size-3.5" />
      </button>
    </div>
  );
}

/** The offer is for a project that has not been culled: no run and fewer than 5 % of the photos flagged. */
export const offerApplies = (run: TargetRun | null, photoCount: number, flagged: number) => !hasTargetRun(run) && run?.state !== "running" && flagged < 0.05 * photoCount;

/** Where the offer sits once a run exists (same height): live counts, and the stage with unfinished work (P1-9). */
export function BestRow({ projectId, run, onContinue, onApply }: { projectId: number; run: TargetRun; onContinue: () => void; onApply: () => void }) {
  const c = run.counts;
  const pr = runProgress(projectId, c);
  const applied = run.appliedAtMs != null ? new Date(run.appliedAtMs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) : null;
  return (
    <div className="flex h-8 shrink-0 items-center gap-3 whitespace-nowrap border-b border-sky-900 bg-sky-950/70 px-3 text-xs text-sky-100" data-testid="target-best-row" data-applied={applied != null} data-next={pr.next}>
      <Sparkles className="size-4 shrink-0 text-sky-300" />
      <span className="min-w-0 flex-1 truncate" data-testid="target-best-status">
        <b>Best {num(run.settings.targetCount)}</b>: {num(c.deliver)} picked · reviewed {num(pr.picksDone)} of {num(c.deliver)} · second look {num(pr.secondDone)} of {num(c.notSure)} · {applied ? `applied ${applied}` : "not applied yet"}
      </span>
      <button className="rounded bg-sky-700 px-2.5 py-0.5 font-medium text-white hover:bg-sky-600" data-testid="target-best-continue" title="Open Pick the best N at the stage with unfinished work (B)" onClick={onContinue}>
        Continue review
      </button>
      <button className="flex items-center gap-1 rounded bg-neutral-800 px-2.5 py-0.5 hover:bg-neutral-700" data-testid="target-best-apply" title="Write the picks to the photos' flags and XMP sidecars" onClick={onApply}>
        <Flag className="size-3" /> Apply flags…
      </button>
    </div>
  );
}

export function TargetButton({ run, onOpen }: { run: TargetRun | null; onOpen: () => void }) {
  const has = hasTargetRun(run);
  const wide = useWide("(min-width: 1440px)"); // the Cull toolbar is full at 1280: icon only there, plus "Best N" once a run exists
  return (
    <button
      className="flex h-7 items-center gap-1.5 whitespace-nowrap rounded-md bg-sky-800 px-3 text-xs font-medium text-sky-50 hover:bg-sky-700"
      data-testid="target-open"
      onClick={onOpen}
      aria-label={has ? `Best ${run!.counts.deliver} (B)` : "Pick the best N (B)"}
      title={has ? `Pick the best N (B): ${run!.counts.deliver} picked for a target of ${run!.settings.targetCount}. Review the picks and their alternatives` : "Pick the best N (B): Sieve chooses the delivery set, you review the picks and their alternatives"}
    >
      <Sparkles className="size-3.5" />
      {has ? `Best ${run!.counts.deliver}` : wide ? "Pick the best N" : ""}
    </button>
  );
}
