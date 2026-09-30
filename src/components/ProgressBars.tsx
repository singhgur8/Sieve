import { Loader2 } from "lucide-react";
import type { ImportProgress } from "../ipc";
import type { AnalysisView } from "../hooks/useBackendStatus";

export function AnalysisBar({ a, onCancel }: { a: AnalysisView; onCancel: () => void }) {
  const pct = a.total > 0 ? Math.min(100, (a.done / a.total) * 100) : 0;
  return (
    <div className="border-b border-neutral-800 px-4 py-1.5" data-testid="analysis-bar">
      <div className="mb-1 flex items-center gap-2 text-xs text-neutral-400">
        {a.running && <Loader2 className="size-3 animate-spin" />}
        <span>
          {a.running ? "Analyzing" : "Analysis paused"}: {a.done} / {a.total}
        </span>
        {a.failed > 0 && <span className="text-red-400">{a.failed} failed</span>}
        {a.running && (
          <button onClick={onCancel} className="ml-auto rounded bg-neutral-800 px-2 py-0.5 hover:bg-neutral-700">
            Cancel
          </button>
        )}
      </div>
      <div className="h-1.5 overflow-hidden rounded bg-neutral-800">
        <div className="h-full bg-sky-400 transition-[width]" style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}

export function ImportBar({ progress, active }: { progress: ImportProgress; active: boolean }) {
  const pct = progress.total > 0 ? Math.min(100, (progress.done / progress.total) * 100) : 0;
  return (
    <div className="border-b border-neutral-800 px-4 py-1.5" data-testid="import-bar">
      <div className="mb-1 flex items-center gap-2 text-xs text-neutral-400">
        {active && <Loader2 className="size-3 animate-spin" />}
        <span>
          {active ? "Extracting thumbnails" : "Done"}: {progress.done} / {progress.total}
        </span>
        {progress.failed > 0 && <span className="text-red-400">{progress.failed} failed</span>}
      </div>
      <div className="h-1.5 overflow-hidden rounded bg-neutral-800">
        <div className="h-full bg-amber-400 transition-[width]" style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}
