// Non-blocking export progress cards (top-right, below the top bar).
import { X } from "lucide-react";
import type { JobView } from "../../hooks/useExportJobs";

interface Props {
  jobs: JobView[];
  onCancel: (id: number) => void;
  onDismiss: (id: number) => void;
}

function Card({ job, onCancel, onDismiss }: { job: JobView; onCancel: (id: number) => void; onDismiss: (id: number) => void }) {
  const f = job.finished;
  const pct = job.total > 0 ? Math.round((job.done / job.total) * 100) : 0;
  const title = job.presetName ?? "Export";
  return (
    <div
      className="w-96 rounded-lg border border-neutral-700 bg-neutral-900/95 p-3 text-sm shadow-xl"
      data-testid={`export-job-${job.id}`}
      data-state={f ? (f.cancelled ? "cancelled" : "finished") : "running"}
    >
      <div className="flex items-center justify-between gap-2">
        <span className="truncate font-medium">{title}</span>
        {f ? (
          <button onClick={() => onDismiss(job.id)} aria-label="Dismiss export" data-testid="export-job-dismiss" className="text-neutral-400 hover:text-neutral-100">
            <X className="size-4" />
          </button>
        ) : (
          <button
            onClick={() => onCancel(job.id)}
            disabled={job.cancelling}
            data-testid="export-cancel"
            className="rounded bg-neutral-800 px-2 py-0.5 text-xs hover:bg-neutral-700 disabled:opacity-50"
          >
            {job.cancelling ? "Cancelling…" : "Cancel"}
          </button>
        )}
      </div>

      {!f && (
        <>
          <div className="mt-2 h-1.5 overflow-hidden rounded bg-neutral-800">
            <div className="h-full bg-amber-400 transition-[width]" style={{ width: `${pct}%` }} data-testid="export-bar" />
          </div>
          <p className="mt-1.5 text-xs text-neutral-400" data-testid="export-progress-text">
            {job.done}/{job.total}
            {job.failed > 0 && <span className="text-red-400"> · {job.failed} failed</span>}
            {job.skipped > 0 && <span> · {job.skipped} skipped</span>}
            {job.total === 0 && " · queued"}
          </p>
          {job.currentFile && (
            <p className="truncate text-xs text-neutral-500" data-testid="export-current-file">
              {job.currentFile}
            </p>
          )}
        </>
      )}

      {f && (
        <div className="mt-1.5 space-y-1 text-xs" data-testid="export-summary">
          <p className={f.failed.length > 0 ? "text-amber-300" : "text-emerald-300"} data-testid="export-summary-text">
            {f.cancelled ? "Cancelled: " : "Done: "}
            {f.succeeded} exported
            {f.skipped > 0 && `, ${f.skipped} skipped`}
            {f.failed.length > 0 && `, ${f.failed.length} failed`}
            {f.elapsedMs > 0 && ` in ${(f.elapsedMs / 1000).toFixed(1)}s`}
          </p>
          {job.outputDir ? (
            <p className="break-all font-mono text-neutral-400 select-text" data-testid="export-output-dir">
              {job.outputDir}
            </p>
          ) : (
            <p className="text-neutral-500">Written next to the original RAW files</p>
          )}
          {f.failed.length > 0 && (
            <ul className="max-h-28 overflow-auto rounded bg-neutral-950 p-1.5 text-red-300" data-testid="export-failures">
              {f.failed.map((x) => (
                <li key={x.imageId} data-testid={`export-failure-${x.imageId}`}>
                  <span className="font-mono">{x.fileName}</span>: {x.reason}
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}

export function ExportJobsPanel({ jobs, onCancel, onDismiss }: Props) {
  if (jobs.length === 0) return null;
  return (
    <div className="pointer-events-none fixed right-3 top-14 z-40 flex flex-col gap-2" data-testid="export-jobs">
      {jobs.map((j) => (
        <div key={j.id} className="pointer-events-auto">
          <Card job={j} onCancel={onCancel} onDismiss={onDismiss} />
        </div>
      ))}
    </div>
  );
}
