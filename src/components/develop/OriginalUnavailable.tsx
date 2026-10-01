// Develop placeholder for a photo whose original cannot be read (moved / drive disconnected) or decoded.
import { ImageOff, RefreshCw, Unplug } from "lucide-react";
import type { FileHealth } from "../../lib/errors";

interface Props {
  health: FileHealth;
  fileName: string;
  onRetry: () => void;
  /** "Locate folder…" (IPC v13 relocate_folder); absent while the backend cannot relink. */
  onLocate?: () => void;
}

export function OriginalUnavailable({ health, fileName, onRetry, onLocate }: Props) {
  const missing = health.kind === "missing";
  return (
    <div className="absolute inset-0 z-10 flex flex-col items-center justify-center gap-3 bg-neutral-950 p-8 text-center" data-testid="original-unavailable" data-kind={health.kind}>
      {missing ? <Unplug className="size-9 text-amber-400" /> : <ImageOff className="size-9 text-red-400" />}
      <h2 className="text-base font-semibold text-neutral-100">{missing ? "The original file is missing" : "This file could not be decoded"}</h2>
      <p className="max-w-lg break-words text-xs text-neutral-400 select-text" data-testid="original-unavailable-message">
        {health.message}
      </p>
      <p className="text-xs text-neutral-400">
        {missing
          ? `Your edits to ${fileName} are kept in the catalog. Reconnect the drive or move the file back, then retry.`
          : "The file may be damaged, still copying, or from a camera Sieve does not support yet. Culling data and the cached preview are unaffected."}
      </p>
      <div className="flex gap-2">
        <button onClick={onRetry} data-testid="original-retry" className="flex items-center gap-1.5 rounded bg-sky-700 px-3 py-1.5 text-sm font-medium text-white hover:bg-sky-600">
          <RefreshCw className="size-4" /> Retry
        </button>
        {missing && onLocate && (
          <button onClick={onLocate} data-testid="original-locate" className="rounded bg-neutral-800 px-3 py-1.5 text-sm hover:bg-neutral-700">
            Locate folder…
          </button>
        )}
      </div>
    </div>
  );
}
