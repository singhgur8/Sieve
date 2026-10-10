// Entry points of "Pick the best N" in the Cull step: the offer shown first for a project that has not been culled, and the
// toolbar button that reopens the flow.
import { useState } from "react";
import { useWide } from "../edit/bits";
import { Sparkles, X } from "lucide-react";
import type { TargetRun } from "../../ipc";
import { suggestTarget } from "../../lib/target";

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

export function TargetButton({ run, onOpen }: { run: TargetRun | null; onOpen: () => void }) {
  const has = hasTargetRun(run);
  const wide = useWide("(min-width: 1440px)"); // the Cull toolbar is full at 1280: icon only there
  return (
    <button
      className="flex h-7 items-center gap-1.5 whitespace-nowrap rounded-md bg-sky-800 px-3 text-xs font-medium text-sky-50 hover:bg-sky-700"
      data-testid="target-open"
      onClick={onOpen}
      aria-label={has ? `Best ${run!.counts.deliver}` : "Pick the best N"}
      title={has ? `Pick the best N: ${run!.counts.deliver} picked for a target of ${run!.settings.targetCount}. Review the picks and their alternatives` : "Pick the best N: Sieve chooses the delivery set, you review the picks and their alternatives"}
    >
      <Sparkles className="size-3.5" />
      {wide && (has ? `Best ${run!.counts.deliver}` : "Pick the best N")}
    </button>
  );
}
