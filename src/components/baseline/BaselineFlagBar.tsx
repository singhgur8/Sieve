// Grid strip while the "needs a look" baseline filter is on: why those photos were flagged (grouped), and the reason
// for the active photo.
import { useEffect, useState } from "react";
import { AlertTriangle } from "lucide-react";
import { commands, unwrap, type BaselinePhotoResult } from "../../ipc";

interface Props {
  projectId: number;
  activeId: number | null;
  onClear: () => void;
  onOpenBaseline: () => void;
}

export function BaselineFlagBar({ projectId, activeId, onClear, onOpenBaseline }: Props) {
  const [rows, setRows] = useState<BaselinePhotoResult[]>([]);
  useEffect(() => {
    let dead = false;
    unwrap(commands.getBaselineResults(projectId, ["flagged"]))
      .then((r) => !dead && setRows(r))
      .catch(() => {});
    return () => {
      dead = true;
    };
  }, [projectId]);
  const groups = new Map<string, number>();
  rows.forEach((r) => {
    const t = r.reasons[0]?.text ?? "Needs a look";
    groups.set(t, (groups.get(t) ?? 0) + 1);
  });
  const mine = activeId != null ? rows.find((r) => r.imageId === activeId) : undefined;
  return (
    <div className="flex min-h-7 shrink-0 flex-wrap items-center gap-x-3 gap-y-0.5 border-b border-amber-900 bg-amber-950 px-3 py-1 text-xs text-amber-100" data-testid="baseline-flag-bar">
      <AlertTriangle className="size-3.5 shrink-0 text-amber-400" aria-hidden />
      <span data-testid="baseline-flag-groups">
        {rows.length} photo{rows.length === 1 ? "" : "s"} need a look after the baseline: {[...groups].map(([t, n]) => `${n} × ${t.charAt(0).toLowerCase()}${t.slice(1)}`).join("; ")}
      </span>
      {mine && (
        <span className="rounded bg-amber-900 px-1.5 py-0.5" data-testid="baseline-flag-reason" title="Why the baseline flagged the selected photo">
          This photo: {mine.reasons.map((r) => r.text).join("; ")}
        </span>
      )}
      <button className="rounded bg-amber-800 px-2 py-0.5 hover:bg-amber-700" data-testid="baseline-flag-clear" title="Show every photo again" onClick={onClear}>
        Show all
      </button>
      <button className="text-sky-300 hover:underline" data-testid="baseline-flag-open" title="Back to the baseline edit result" onClick={onOpenBaseline}>
        Baseline edit
      </button>
    </div>
  );
}
