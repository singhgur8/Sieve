// XMP sidecar sync indicator for the top bar (Saved / Saving / Error / Off), its popover (what is written where, failure
// list with Retry, auto-sync toggle) and the one-time "how Sieve reads and merges XMP" explanation.
import { useEffect, useRef } from "react";
import { AlertTriangle, CheckCircle2, CloudOff, HelpCircle, Loader2, RotateCw } from "lucide-react";
import type { XmpStatus } from "../ipc";
import { describeReason } from "../lib/errors";
import { Dialog } from "./Dialog";
import { Menu, menuItem } from "./Menu";
import { HelpLink } from "./HelpLink";

export interface XmpFailureRow {
  imageId: number;
  fileName: string;
  reason: string;
}

export type XmpUiState = "saved" | "pending" | "error" | "off";

export function xmpUiState(xmp: XmpStatus | null): XmpUiState {
  if (!xmp) return "saved";
  if (xmp.failed > 0) return "error";
  if (!xmp.autoSync) return "off";
  return xmp.dirty > 0 || xmp.running ? "pending" : "saved";
}

interface Props {
  xmp: XmpStatus | null;
  failures: XmpFailureRow[];
  onOpenErrors: () => void;
  onShow?: (imageId: number) => void;
  onRetry: () => void;
  onSaveAll: () => void;
  onAutoSync: (v: boolean) => void;
  onExplain: () => void;
}

const pillBase = "flex h-7 items-center gap-1.5 whitespace-nowrap rounded-full px-3 text-xs font-medium ring-1";
const pillClass: Record<XmpUiState, string> = {
  saved: "bg-neutral-900 text-neutral-300 ring-neutral-700 hover:bg-neutral-800",
  pending: "bg-amber-900/70 text-amber-100 ring-amber-600 hover:bg-amber-800",
  error: "bg-red-950 text-red-200 ring-red-700 hover:bg-red-900",
  off: "bg-neutral-900 text-neutral-400 ring-neutral-700 hover:bg-neutral-800",
};

export function XmpStatusPill({ xmp, failures, onOpenErrors, onShow, onRetry, onSaveAll, onAutoSync, onExplain }: Props) {
  const state = xmpUiState(xmp);
  const dirty = xmp?.dirty ?? 0;
  const failed = xmp?.failed ?? 0;
  const pending = Math.max(0, dirty - failed);
  const autoSync = xmp?.autoSync ?? false;
  const label =
    state === "error" ? `${failed} error${failed === 1 ? "" : "s"}`
    : state === "pending" ? (pending > 0 ? `Saving ${pending}…` : "Saving…")
    : state === "off" ? (dirty > 0 ? `${dirty} unsaved` : "Auto-save off")
    : "Saved";
  const icon =
    state === "error" ? <AlertTriangle className="size-3.5" />
    : state === "pending" ? <Loader2 className="size-3.5 animate-spin" />
    : state === "off" ? <CloudOff className="size-3.5" />
    : <CheckCircle2 className="size-3.5 text-emerald-400" />;
  const title =
    state === "error" ? "Some sidecars could not be written. Click for details."
    : state === "pending" ? "Writing changes to the XMP sidecars next to your RAW files"
    : state === "off" ? "Auto-save to XMP is off. Click to save or turn it on."
    : "All changes are written to the XMP sidecars";
  return (
    <Menu
      trigger={
        <span className="flex items-center gap-1.5" data-testid="xmp-status" data-state={state} data-dirty={dirty} data-failed={failed}>
          {icon}
          {label}
        </span>
      }
      triggerClass={`${pillBase} ${pillClass[state]}`}
      triggerTestId="xmp-status-button"
      title={title}
      align="right"
    >
      {(close) => (
        <div className="w-80 px-3 py-2 text-xs text-neutral-300" data-testid="xmp-popover" data-state={state}>
          <PopoverBody
            state={state}
            dirty={dirty}
            pending={pending}
            failed={failed}
            failures={failures}
            autoSync={autoSync}
            onOpenErrors={onOpenErrors}
            onShow={onShow && ((id) => (close(), onShow(id)))}
            onRetry={onRetry}
            onSaveAll={() => {
              close();
              onSaveAll();
            }}
            onAutoSync={onAutoSync}
            onExplain={() => {
              close();
              onExplain();
            }}
          />
        </div>
      )}
    </Menu>
  );
}

function PopoverBody(p: {
  state: XmpUiState;
  dirty: number;
  pending: number;
  failed: number;
  failures: XmpFailureRow[];
  autoSync: boolean;
  onOpenErrors: () => void;
  onShow?: (imageId: number) => void;
  onRetry: () => void;
  onSaveAll: () => void;
  onAutoSync: (v: boolean) => void;
  onExplain: () => void;
}) {
  const { state, onOpenErrors } = p;
  const scan = useRef(onOpenErrors);
  scan.current = onOpenErrors;
  useEffect(() => {
    if (state === "error") scan.current();
  }, [state, p.failed]);
  const headline =
    state === "error" ? `${p.failed} sidecar${p.failed === 1 ? "" : "s"} could not be written`
    : state === "pending" ? (p.pending > 0 ? `Saving ${p.pending} photo${p.pending === 1 ? "" : "s"}…` : "Saving…")
    : state === "off" ? "Auto-save is off"
    : "All changes saved";
  return (
    <>
      <div className="mb-1 text-sm font-medium text-neutral-100" data-testid="xmp-headline">{headline}</div>
      <p className="text-neutral-400">
        {state === "off"
          ? "Changes stay in the catalog until you save. Sidecars next to the RAWs are not touched meanwhile."
          : "Ratings, flags, color labels, Sieve tags and edits are written to the .xmp sidecar next to each RAW a moment after you change them. Every other field in the sidecar is kept. The RAW files are never modified."}
      </p>
      {state === "error" && (
        <>
          <ul className="mt-2 max-h-40 space-y-1.5 overflow-y-auto" data-testid="xmp-failures">
            {p.failures.map((f) => (
              <li key={f.imageId} className="rounded bg-neutral-800/70 px-2 py-1" data-testid={`xmp-failure-${f.imageId}`}>
                <div className="flex items-center gap-2">
                  <span className="min-w-0 flex-1 truncate font-medium text-neutral-200">{f.fileName}</span>
                  {p.onShow && (
                    <button className="shrink-0 text-sky-300 hover:underline" data-testid={`xmp-failure-show-${f.imageId}`} onClick={() => p.onShow!(f.imageId)}>
                      Show
                    </button>
                  )}
                </div>
                <div className="text-neutral-400">{describeReason(f.reason).message}</div>
              </li>
            ))}
            {p.failures.length < p.failed && <li className="text-neutral-400">{p.failed - p.failures.length} more, retry to list them.</li>}
          </ul>
          <button className="mt-2 flex items-center gap-1.5 rounded-md bg-neutral-700 px-2.5 py-1 text-neutral-100 hover:bg-neutral-600" data-testid="xmp-retry" onClick={p.onRetry}>
            <RotateCw className="size-3.5" /> Retry
          </button>
        </>
      )}
      {state === "off" && p.dirty > 0 && (
        <button className="mt-2 rounded-md bg-amber-600 px-2.5 py-1 font-medium text-black hover:bg-amber-500" data-testid="xmp-save-all" onClick={p.onSaveAll}>
          Save {p.dirty} photo{p.dirty === 1 ? "" : "s"} now
        </button>
      )}
      <label className="mt-2 flex cursor-pointer items-center gap-2 text-neutral-200">
        <input type="checkbox" data-testid="xmp-auto-toggle" checked={p.autoSync} onChange={(e) => p.onAutoSync(e.target.checked)} />
        Save to XMP automatically
      </label>
      <button className={`${menuItem} -mx-3 mt-1 w-[calc(100%+1.5rem)] text-xs text-neutral-300`} data-testid="xmp-explain" onClick={p.onExplain}>
        <HelpCircle className="size-3.5" /> How XMP sync works
      </button>
      <HelpLink id="saved" label="Where is my work saved?" className="mt-1" />
    </>
  );
}

export function XmpExplainer({ autoSync, onClose }: { autoSync: boolean; onClose: () => void }) {
  return (
    <Dialog label="How Sieve syncs XMP sidecars" testid="xmp-explainer" className="w-[30rem] rounded-lg border border-neutral-700 bg-neutral-900 p-5 text-sm text-neutral-300 shadow-2xl" onCancel={onClose} onConfirm={onClose}>
      <h2 className="mb-2 text-base font-semibold text-neutral-100">How Sieve syncs XMP sidecars</h2>
      <ul className="list-disc space-y-1.5 pl-5">
        <li>Sieve reads any existing <code>.xmp</code> sidecars next to your RAW files (from Lightroom or Bridge) as the starting point for ratings, flags, labels and edits.</li>
        <li>Your changes are written back into the same sidecar: ratings, pick/reject flags, color labels, Sieve tags and develop settings.</li>
        <li>Every other field in the sidecar is kept as it is. Your RAW files are never modified.</li>
        <li>{autoSync ? "Saving happens automatically a moment after each change. " : "Auto-save is off; use Save to write sidecars. "}The status pill in the top bar shows Saved, Saving or Error and lets you turn auto-save on or off.</li>
      </ul>
      <div className="mt-4 flex justify-end">
        <button className="rounded-md bg-amber-600 px-3 py-1.5 font-medium text-black hover:bg-amber-500" data-testid="xmp-explainer-dismiss" onClick={onClose} autoFocus>
          Got it
        </button>
      </div>
    </Dialog>
  );
}
