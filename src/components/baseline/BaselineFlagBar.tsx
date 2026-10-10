// Grid strip while the "needs a look" baseline filter is on: why those photos were flagged (grouped), and the reason
// for the active photo. In Develop the same filter shows a slim bar with how many are left and the way back.
import { useEffect, useState } from "react";
import { AlertTriangle, CheckCircle2 } from "lucide-react";
import { commands, unwrap, type BaselinePhotoResult } from "../../ipc";

/** The photos the project's latest baseline run flagged (reasons included). */
export function useFlaggedRows(projectId: number | null, tick?: unknown): BaselinePhotoResult[] {
  const [rows, setRows] = useState<BaselinePhotoResult[]>([]);
  useEffect(() => {
    if (projectId == null) return;
    let dead = false;
    unwrap(commands.getBaselineResults(projectId, ["flagged"]))
      .then((r) => !dead && setRows(r))
      .catch(() => {});
    return () => {
      dead = true;
    };
  }, [projectId, tick]);
  return rows;
}

interface Props {
  projectId: number;
  activeId: number | null;
  /** Flagged photos not yet marked "Looks good": only these count. */
  reviewLeft: Set<number>;
  tick?: unknown;
  onClear: () => void;
  onOpenBaseline: () => void;
}

export function BaselineFlagBar({ projectId, activeId, reviewLeft, tick, onClear, onOpenBaseline }: Props) {
  const rows = useFlaggedRows(projectId, tick);
  const open = rows.filter((r) => reviewLeft.has(r.imageId));
  const groups = new Map<string, number>();
  open.forEach((r) => {
    const t = r.reasons[0]?.text ?? "Needs a look";
    groups.set(t, (groups.get(t) ?? 0) + 1);
  });
  const mine = activeId != null ? rows.find((r) => r.imageId === activeId) : undefined;
  const checked = rows.length - open.length;
  return (
    <div className="flex min-h-7 shrink-0 flex-wrap items-center gap-x-3 gap-y-0.5 border-b border-amber-900 bg-amber-950 px-3 py-1 text-xs text-amber-100" data-testid="baseline-flag-bar">
      <AlertTriangle className="size-3.5 shrink-0 text-amber-400" aria-hidden />
      <span data-testid="baseline-flag-groups">
        {open.length === 0 && rows.length > 0
          ? `All ${rows.length} photos that needed a look are checked`
          : `${open.length} photo${open.length === 1 ? "" : "s"} need a look after the baseline: ${[...groups].map(([t, n]) => `${n} × ${t.charAt(0).toLowerCase()}${t.slice(1)}`).join("; ")}`}
        {checked > 0 && open.length > 0 ? ` · ${checked} checked` : ""}
      </span>
      {mine && (
        <span className="rounded bg-amber-900 px-1.5 py-0.5" data-testid="baseline-flag-reason" title="Why the baseline flagged the selected photo">
          This photo: {mine.reasons.map((r) => r.text).join("; ")}
        </span>
      )}
      <button className="rounded bg-amber-800 px-2 py-0.5 hover:bg-amber-700" data-testid="baseline-flag-clear" title="Show every photo again" onClick={onClear}>
        Show all
      </button>
      <button className="text-sky-300 hover:underline" data-testid="baseline-flag-open" title="Back to the baseline edit result (Cmd+Alt+B)" onClick={onOpenBaseline}>
        Baseline edit
      </button>
    </div>
  );
}

/** Develop, while reviewing the flagged photos: where you are and the way back to the result. */
export function BaselineReviewBar({ projectId, reviewLeft, tick, onBack }: { projectId: number; reviewLeft: Set<number>; tick?: unknown; onBack: () => void }) {
  const rows = useFlaggedRows(projectId, tick);
  const left = rows.filter((r) => reviewLeft.has(r.imageId)).length;
  const done = left === 0 && rows.length > 0;
  return (
    <div className={`flex h-8 shrink-0 items-center gap-2 border-b px-3 text-xs ${done ? "border-emerald-900 bg-emerald-950/70" : "border-amber-900 bg-amber-950/70"}`} data-testid="baseline-review-bar" data-done={done ? "1" : undefined} role="region" aria-label="Baseline review">
      {done ? <CheckCircle2 className="size-3.5 shrink-0 text-emerald-400" aria-hidden /> : <AlertTriangle className="size-3.5 shrink-0 text-amber-400" aria-hidden />}
      <span className={`font-semibold ${done ? "text-emerald-200" : "text-amber-200"}`}>Baseline</span>
      <span className={done ? "text-emerald-200" : "text-neutral-200"} data-testid="baseline-review-left">
        {done ? `All ${rows.length} checked` : `Needs a look: ${left} of ${rows.length} left`}
      </span>
      <button className="ml-auto text-sky-300 hover:underline" data-testid="baseline-review-back" title="Back to the baseline edit result (Cmd+Alt+B)" onClick={onBack}>
        Back to the result (Cmd+Alt+B)
      </button>
    </div>
  );
}
