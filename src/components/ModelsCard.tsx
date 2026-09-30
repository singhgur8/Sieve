// AI model download UI: the inline card in the Masks panel and the "Manage AI models" dialog (More menu).
import { Download, RefreshCw, X } from "lucide-react";
import { Dialog } from "./Dialog";
import { cancelModelDownload, downloadModels, mb, segmentation, useModels } from "../lib/models";

/** Progress line and bar shared by the card and the dialog. */
function Progress({ compact }: { compact?: boolean }) {
  const m = useModels();
  const p = m.progress;
  const pct = p && p.bytesTotal > 0 ? Math.min(100, Math.round((p.bytesDone / p.bytesTotal) * 100)) : 0;
  return (
    <div data-testid="model-progress" data-pct={pct}>
      <div className="mb-1 flex items-center justify-between gap-2 text-[11px] text-neutral-300">
        <span data-testid="model-progress-text" className="min-w-0 truncate">
          {p ? `File ${p.fileIndex + 1} of ${p.fileCount} · ${mb(p.bytesDone)} of ${mb(p.bytesTotal)} MB · ${pct}%` : "Starting download..."}
        </span>
        <button
          className="shrink-0 rounded bg-neutral-800 px-2 py-0.5 hover:bg-neutral-700 disabled:opacity-50"
          disabled={m.cancelling}
          onClick={() => void cancelModelDownload()}
          data-testid="model-cancel"
        >
          {m.cancelling ? "Cancelling..." : "Cancel"}
        </button>
      </div>
      <div className="h-1.5 overflow-hidden rounded bg-neutral-800">
        <div className="h-full bg-sky-400 transition-[width]" style={{ width: `${pct}%` }} data-testid="model-bar" />
      </div>
      {!compact && p && <p className="mt-1 truncate text-[11px] text-neutral-400">{p.name}</p>}
    </div>
  );
}

function ErrorLine() {
  const m = useModels();
  if (!m.error) return null;
  return (
    <p role="alert" className="mt-2 break-words text-[11px] text-red-300" data-testid="model-error">
      {m.error}
    </p>
  );
}

/** Masks panel card; renders nothing once the segmentation models are installed. */
export function ModelsCard() {
  const m = useModels();
  const g = segmentation(m.status);
  if (!g || (g.installed && !m.downloading)) return null;
  return (
    <div className="mb-2 rounded-lg border border-sky-900 bg-sky-950/40 p-2.5" data-testid="models-card">
      <h3 className="text-xs font-semibold text-sky-100">AI masks need a one-time download (~{mb(g.bytesTotal)} MB)</h3>
      {m.downloading ? (
        <div className="mt-2">
          <Progress compact />
        </div>
      ) : (
        <>
          <p className="mt-1 text-[11px] text-neutral-300">Subject, sky, background and people selections run on models stored on this Mac. Everything stays offline after this.</p>
          <button
            className="mt-2 flex items-center gap-1.5 rounded bg-sky-700 px-2.5 py-1 text-xs font-medium text-white hover:bg-sky-600"
            onClick={() => void downloadModels()}
            data-testid="model-download"
          >
            {m.error ? <RefreshCw className="size-3.5" /> : <Download className="size-3.5" />}
            {m.error ? "Retry download" : "Download"}
          </button>
        </>
      )}
      <ErrorLine />
    </div>
  );
}

/** More menu > Manage AI models. */
export function ModelsDialog({ onClose }: { onClose: () => void }) {
  const m = useModels();
  const g = segmentation(m.status);
  return (
    <Dialog label="AI models" testid="models-dialog" className="w-[480px] max-w-full rounded-xl border border-neutral-700 bg-neutral-900 p-4 text-sm shadow-2xl" onCancel={onClose} backdropClose>
      <header className="mb-3 flex items-center justify-between">
        <h2 className="font-semibold">AI models</h2>
        <button onClick={onClose} aria-label="Close" data-testid="models-close" className="text-neutral-400 hover:text-neutral-100">
          <X className="size-4" />
        </button>
      </header>
      {!g ? (
        <p className="text-xs text-neutral-400">Checking installed models...</p>
      ) : (
        <>
          <div className="flex items-center justify-between gap-2">
            <div>
              <p className="font-medium">{g.label}</p>
              <p className="text-xs text-neutral-400" data-testid="models-summary">
                {g.installed ? `Installed (${g.files.length} files, ${mb(g.bytesTotal)} MB)` : `${g.files.filter((f) => f.installed).length} of ${g.files.length} files installed · ${mb(g.bytesTotal)} MB download`}
              </p>
            </div>
            {!g.installed && !m.downloading && (
              <button className="flex items-center gap-1.5 rounded bg-sky-700 px-3 py-1 text-xs font-medium text-white hover:bg-sky-600" onClick={() => void downloadModels()} data-testid="model-download">
                <Download className="size-3.5" /> {m.error ? "Retry" : "Download"}
              </button>
            )}
          </div>
          {m.downloading && (
            <div className="mt-3">
              <Progress />
            </div>
          )}
          <ErrorLine />
          <ul className="mt-3 max-h-48 overflow-auto rounded bg-neutral-950 p-2 font-mono text-[11px]" data-testid="models-files">
            {g.files.map((f) => (
              <li key={f.name} className="flex justify-between gap-2">
                <span className="truncate">{f.name}</span>
                <span className={f.installed ? "text-emerald-400" : "text-neutral-400"}>{f.installed ? "installed" : `${mb(f.bytes)} MB`}</span>
              </li>
            ))}
          </ul>
        </>
      )}
    </Dialog>
  );
}
