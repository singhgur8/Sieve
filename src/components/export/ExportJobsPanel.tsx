// Non-blocking export progress cards (bottom-right). A finished job collapses to a 28 px pill after 8 s.
import { AlertTriangle, CheckCircle2, FolderSearch, X } from "lucide-react";
import { describeReason, type ErrorCategory } from "../../lib/errors";
import type { JobView } from "../../hooks/useExportJobs";

interface Props {
  jobs: JobView[];
  onCancel: (id: number) => void;
  onDismiss: (id: number) => void;
  onReveal: (path: string) => void;
}

function Pill({ job, onDismiss, onReveal }: { job: JobView; onDismiss: (id: number) => void; onReveal: (path: string) => void }) {
  const f = job.finished!;
  return (
    <div
      className="flex h-7 items-center gap-2 rounded-full border border-neutral-700 bg-neutral-900/95 px-3 text-xs shadow-xl"
      data-testid={`export-job-${job.id}`}
      data-state="pill"
    >
      <CheckCircle2 className="size-3.5 text-emerald-400" />
      <span data-testid="export-pill-text">
        {f.cancelled ? "Export cancelled" : "Export done"} · {f.succeeded}
      </span>
      {job.outputDir && (
        <button className="flex items-center gap-1 text-sky-300 hover:text-sky-200" onClick={() => onReveal(job.outputDir!)} data-testid="export-reveal">
          <FolderSearch className="size-3.5" /> Reveal
        </button>
      )}
      <button onClick={() => onDismiss(job.id)} aria-label="Dismiss export" data-testid="export-job-dismiss" className="text-neutral-400 hover:text-neutral-100">
        <X className="size-3.5" />
      </button>
    </div>
  );
}

const FAILURE_HINT: Partial<Record<ErrorCategory, string>> = {
  disk_full: "The destination disk is full. Free up space or pick another folder, then export the remaining photos again.",
  read_only: "The destination is read-only. Choose a writable folder in the export dialog.",
  permission: "Sieve is not allowed to write there. Check the folder's permissions or choose another folder.",
  folder_gone: "The destination folder no longer exists (moved, or its drive was disconnected). Choose another folder.",
  missing: "Some originals are missing. Reconnect the drive or move the files back, then export them again.",
  decode: "Some files could not be decoded. They may be damaged or still copying.",
};

/** One-line guidance for the most actionable failure category of a finished job. */
function failureHint(failed: { reason: string }[]): string | null {
  const cats = new Set(failed.map((f) => describeReason(f.reason).category));
  for (const c of ["disk_full", "read_only", "permission", "folder_gone", "missing", "decode"] as const) if (cats.has(c)) return FAILURE_HINT[c] ?? null;
  return null;
}

function Card({ job, onCancel, onDismiss, onReveal }: { job: JobView; onCancel: (id: number) => void; onDismiss: (id: number) => void; onReveal: (path: string) => void }) {
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
            <p className="truncate text-xs text-neutral-400" data-testid="export-current-file">
              {job.currentFile}
            </p>
          )}
        </>
      )}

      {f && (
        <div className="mt-1.5 space-y-1 text-xs" data-testid="export-summary">
          <p className={f.failed.length > 0 ? (f.succeeded === 0 ? "text-red-300" : "text-amber-300") : "text-emerald-300"} data-testid="export-summary-text">
            {f.cancelled ? "Cancelled: " : f.failed.length > 0 && f.succeeded === 0 ? "Export failed: " : "Done: "}
            {f.succeeded} exported
            {f.skipped > 0 && `, ${f.skipped} skipped`}
            {f.failed.length > 0 && `, ${f.failed.length} failed`}
            {f.elapsedMs > 0 && ` in ${(f.elapsedMs / 1000).toFixed(1)}s`}
          </p>
          {job.outputDir ? (
            <div className="flex items-center gap-2">
              <p className="min-w-0 flex-1 break-all font-mono text-neutral-400 select-text" data-testid="export-output-dir">
                {job.outputDir}
              </p>
              <button className="flex shrink-0 items-center gap-1 rounded bg-neutral-800 px-2 py-0.5 text-sky-300 hover:bg-neutral-700" onClick={() => onReveal(job.outputDir!)} data-testid="export-reveal">
                <FolderSearch className="size-3.5" /> Reveal in Finder
              </button>
            </div>
          ) : (
            <p className="text-neutral-400">Written next to the original RAW files</p>
          )}
          {f.failed.length > 0 && failureHint(f.failed) && (
            <p className="flex items-start gap-1.5 rounded bg-red-950/60 p-1.5 text-red-200" data-testid="export-failure-hint">
              <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
              {failureHint(f.failed)}
            </p>
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

export function ExportJobsPanel({ jobs, onCancel, onDismiss, onReveal }: Props) {
  if (jobs.length === 0) return null;
  return (
    <div className="pointer-events-none fixed bottom-24 right-3 z-40 flex flex-col items-end gap-2" data-testid="export-jobs">
      {jobs.map((j) => (
        <div key={j.id} className="pointer-events-auto">
          {j.finished && j.collapsed ? <Pill job={j} onDismiss={onDismiss} onReveal={onReveal} /> : <Card job={j} onCancel={onCancel} onDismiss={onDismiss} onReveal={onReveal} />}
        </div>
      ))}
    </div>
  );
}
