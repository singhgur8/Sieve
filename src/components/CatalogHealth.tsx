// Catalog health (IPC v13): the startup banner for a read-only / replaced catalog, the "relaunch to finish
// restoring" notice, and the Restore backup dialog.
import { useState } from "react";
import { AlertTriangle, DatabaseBackup, RefreshCw, X } from "lucide-react";
import type { CatalogBackup, CatalogHealth } from "../ipc";
import { Dialog } from "./Dialog";

const fmtDate = (ms: number) => new Date(ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
const fmtSize = (b: number) => (b >= 1e9 ? `${(b / 1e9).toFixed(1)} GB` : `${Math.max(1, Math.round(b / 1e6))} MB`);

/** Persistent banner from `CatalogState.health`. Nothing when the catalog is fine and no restore is staged. */
export function HealthBanner({ health, onRestore, onDismiss }: { health: CatalogHealth; onRestore: () => void; onDismiss: () => void }) {
  if (health.restorePending) {
    return (
      <div role="alert" className="flex shrink-0 items-center gap-2 border-b border-sky-800 bg-sky-950 px-4 py-2 text-xs text-sky-100" data-testid="restore-pending" data-category="restore_pending">
        <RefreshCw className="size-4 shrink-0 text-sky-300" />
        <p className="font-semibold">Relaunch Sieve to finish restoring</p>
        <p className="text-sky-200/80">The selected backup replaces the catalog the next time Sieve starts.</p>
      </div>
    );
  }
  if (health.status === "ok") return null;
  const readOnly = health.status === "read_only";
  return (
    <div role="alert" className="flex shrink-0 items-start gap-2 border-b border-amber-800 bg-amber-950 px-4 py-2 text-xs text-amber-100" data-testid="health-banner" data-status={health.status} data-category={readOnly ? "catalog_readonly" : "catalog_replaced"}>
      <AlertTriangle className="mt-0.5 size-4 shrink-0 text-amber-400" />
      <div className="min-w-0 flex-1">
        <p className="font-semibold">{readOnly ? "Catalog is read-only" : "Catalog was replaced"}</p>
        <p className="break-words select-text" data-testid="health-message">
          {health.message}
        </p>
      </div>
      {health.backups.length > 0 && (
        <button onClick={onRestore} data-testid="restore-backup" className="flex shrink-0 items-center gap-1 rounded bg-amber-800 px-2 py-1 font-medium text-amber-50 hover:bg-amber-700">
          <DatabaseBackup className="size-3.5" /> Restore backup…
        </button>
      )}
      {!readOnly && (
        <button onClick={onDismiss} aria-label="Dismiss banner" data-testid="health-dismiss" className="shrink-0 text-amber-300 hover:text-amber-100">
          <X className="size-4" />
        </button>
      )}
    </div>
  );
}

/** Lists the backups (newest first) and stages the chosen one; it takes effect at the next launch. */
export function RestoreBackupDialog({ backups, onCancel, onRestore }: { backups: CatalogBackup[]; onCancel: () => void; onRestore: (index: number) => Promise<void> }) {
  const sorted = [...backups].sort((a, b) => a.index - b.index);
  const [index, setIndex] = useState(sorted[0]?.index ?? 0);
  const [busy, setBusy] = useState(false);
  const confirm = async () => {
    if (busy || sorted.length === 0) return;
    setBusy(true);
    try {
      await onRestore(index);
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog label="Restore catalog backup" testid="restore-dialog" className="w-[28rem] rounded-lg border border-neutral-700 bg-neutral-900 p-4 shadow-xl" onCancel={onCancel} onConfirm={() => void confirm()} canConfirm={() => sorted.length > 0 && !busy}>
      <h2 className="mb-1 text-sm font-semibold">Restore catalog backup</h2>
      <p className="mb-3 text-xs text-neutral-400">The chosen backup replaces your catalog the next time Sieve starts. Your RAW files and sidecars are not touched.</p>
      {sorted.length === 0 ? (
        <p className="mb-3 rounded bg-neutral-950 p-3 text-xs text-neutral-400" data-testid="restore-empty">
          No backups yet.
        </p>
      ) : (
        <ul className="mb-4 space-y-1" role="radiogroup" aria-label="Backups" data-testid="restore-list">
          {sorted.map((b, i) => (
            <li key={b.index}>
              <label className="flex cursor-pointer items-center gap-2 rounded bg-neutral-950 px-3 py-2 text-xs hover:bg-neutral-800">
                <input type="radio" name="backup" data-autofocus={i === 0 ? true : undefined} checked={index === b.index} onChange={() => setIndex(b.index)} data-testid={`restore-item-${b.index}`} />
                <span className="flex-1">{fmtDate(b.createdAtMs)}</span>
                <span className="text-neutral-400">{fmtSize(b.sizeBytes)}</span>
                {i === 0 && <span className="rounded bg-neutral-800 px-1 text-[10px] text-neutral-300">newest</span>}
              </label>
            </li>
          ))}
        </ul>
      )}
      <div className="flex justify-end gap-2 text-xs">
        <button className="rounded bg-neutral-800 px-3 py-1.5 hover:bg-neutral-700" onClick={onCancel} data-testid="restore-cancel">
          Cancel
        </button>
        <button className="rounded bg-sky-700 px-3 py-1.5 text-white hover:bg-sky-600 disabled:opacity-40" disabled={sorted.length === 0 || busy} onClick={() => void confirm()} data-testid="restore-confirm">
          Restore
        </button>
      </div>
    </Dialog>
  );
}
