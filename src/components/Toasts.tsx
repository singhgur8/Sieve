// Transient messages: bottom-centre, above the filmstrip, never shift the layout.
// Info toasts fade after 4 s (10 s when they carry an action; Undo stays reachable from the scene row menu); errors are sticky.
import { useCallback, useEffect, useRef, useState } from "react";
import { AlertTriangle, X } from "lucide-react";
import type { ErrorInfo } from "../lib/errors";

export interface ToastAction {
  label: string;
  onClick: () => void;
  testid?: string;
}

export interface Toast {
  id: number;
  message: string;
  kind: "info" | "error";
  action?: ToastAction;
  /** Second button shown before `action` (e.g. Review next to Undo). */
  secondary?: ToastAction;
  /** Third button (Undo + Show + Review). */
  third?: ToastAction;
}

export interface ToastApi {
  toasts: Toast[];
  push: (message: string, opts?: { action?: ToastAction; secondary?: ToastAction; third?: ToastAction; kind?: "info" | "error"; ttl?: number }) => number;
  dismiss: (id: number) => void;
  /** Drops the toast's Undo (kept text), e.g. when a newer edit made it unsafe. */
  retract: (id: number) => void;
}

export function useToasts(): ToastApi {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const next = useRef(1);
  const timers = useRef(new Map<number, ReturnType<typeof setTimeout>>());

  const dismiss = useCallback((id: number) => {
    clearTimeout(timers.current.get(id));
    timers.current.delete(id);
    setToasts((all) => all.filter((t) => t.id !== id));
  }, []);

  const retract = useCallback((id: number) => {
    setToasts((all) => all.map((t) => (t.id === id ? { ...t, action: undefined } : t)));
  }, []);

  const push = useCallback(
    (message: string, opts: { action?: ToastAction; secondary?: ToastAction; third?: ToastAction; kind?: "info" | "error"; ttl?: number } = {}) => {
      const id = next.current++;
      const kind = opts.kind ?? "info";
      // A new message replaces plain older ones; toasts carrying an action (Undo) stay until they expire.
      setToasts((all) => [...all.filter((t) => t.action).slice(-1), { id, message, kind, action: opts.action, secondary: opts.secondary, third: opts.third }]);
      if (kind === "info") timers.current.set(id, setTimeout(() => dismiss(id), opts.ttl ?? (opts.action ? 10000 : 4000)));
      return id;
    },
    [dismiss],
  );

  useEffect(() => {
    const t = timers.current;
    return () => t.forEach((h) => clearTimeout(h));
  }, []);

  return { toasts, push, dismiss, retract };
}

/** Persistent inline banner (top of the window) for problems that do not go away by themselves. */
export function IssueBanner({ issue, onDismiss, onRestore }: { issue: ErrorInfo; onDismiss: () => void; onRestore?: () => void }) {
  return (
    <div role="alert" className="flex shrink-0 items-start gap-2 border-b border-amber-800 bg-amber-950 px-4 py-2 text-xs text-amber-100" data-testid="issue-banner" data-category={issue.category}>
      <AlertTriangle className="mt-0.5 size-4 shrink-0 text-amber-400" />
      <div className="min-w-0 flex-1">
        <p className="font-semibold">{issue.title}</p>
        <p className="break-words select-text">{issue.message}</p>
      </div>
      {issue.remedy === "restore" && onRestore && (
        <button onClick={onRestore} data-testid="issue-restore" className="shrink-0 rounded bg-amber-800 px-2 py-1 font-medium text-amber-50 hover:bg-amber-700">
          Restore backup…
        </button>
      )}
      <button onClick={onDismiss} aria-label="Dismiss banner" data-testid="issue-banner-dismiss" className="shrink-0 text-amber-300 hover:text-amber-100">
        <X className="size-4" />
      </button>
    </div>
  );
}

/** True while a modal dialog is open (toasts then move clear of its header and footer). */
function useDialogOpen(): boolean {
  const [open, setOpen] = useState(false);
  useEffect(() => {
    const check = () => setOpen(document.querySelector('[role="dialog"][aria-modal="true"]') != null);
    check();
    const mo = new MutationObserver(check);
    mo.observe(document.body, { childList: true, subtree: true });
    return () => mo.disconnect();
  }, []);
  return open;
}

/**
 * `top`: Develop. Without a dialog the stack sits at the viewer's top-right (left of the adjustment panel) so it stays off the
 * photo's centre and the viewer toolbar; while a dialog is open it moves to the bottom-left, clear of the dialog.
 */
export function Toasts({ api, error, onDismissError, onLocate, placement = "bottom" }: { api: ToastApi; error: ErrorInfo | null; onDismissError: () => void; onLocate?: () => void; placement?: "bottom" | "top" | "tool" | "target" }) {
  const modal = useDialogOpen();
  const pos = modal ? "bottom-3 left-4 w-[min(400px,92vw)]" : placement === "target" ? "top-[100px] right-3 w-[min(400px,92vw)]" : placement === "tool" ? "bottom-14 left-[272px] w-[min(400px,92vw)]" : placement === "top" ? "top-[88px] right-[300px] w-[min(400px,92vw)] min-[1600px]:right-[332px]" : "bottom-24 left-1/2 w-[min(480px,92vw)] -translate-x-1/2";
  return (
    <div className={`pointer-events-none fixed ${pos} z-[60] flex flex-col items-stretch gap-2`} data-testid="toasts" data-placement={modal ? "modal" : placement}>
      {error && (
        <div role="alert" className="pointer-events-auto flex items-start justify-between gap-3 rounded-lg border border-red-800 bg-red-950 px-3 py-2 text-sm text-red-200 shadow-xl" data-testid="error" data-category={error.category}>
          <span className="min-w-0 break-words">{error.message}</span>
          {error.remedy === "locate" && onLocate && (
            <button
              onClick={() => {
                onDismissError();
                onLocate();
              }}
              data-testid="error-locate"
              className="shrink-0 rounded bg-red-800 px-2 py-0.5 text-xs font-medium text-red-50 hover:bg-red-700"
            >
              Locate folder…
            </button>
          )}
          <button onClick={onDismissError} aria-label="Dismiss error" className="shrink-0">
            <X className="size-4" />
          </button>
        </div>
      )}
      {(placement === "target" ? api.toasts.slice(-1) : api.toasts).map((t) => (
        <div
          key={t.id}
          role="status"
          className={`pointer-events-auto flex items-center justify-between gap-3 rounded-lg border px-3 py-2 text-sm shadow-xl ${
            t.kind === "error" ? "border-red-800 bg-red-950 text-red-200" : "border-neutral-700 bg-neutral-900 text-neutral-100"
          }`}
          data-testid="notice"
        >
          <span className="min-w-0 break-words">{t.message}</span>
          <span className="flex shrink-0 items-center gap-2">
            {t.third && (
              <button
                className="rounded bg-neutral-700 px-2 py-0.5 text-xs font-medium text-neutral-100 hover:bg-neutral-600"
                data-testid={t.third.testid ?? "toast-third"}
                onClick={() => {
                  t.third?.onClick();
                  api.dismiss(t.id);
                }}
              >
                {t.third.label}
              </button>
            )}
            {t.secondary && (
              <button
                className="rounded bg-neutral-700 px-2 py-0.5 text-xs font-medium text-neutral-100 hover:bg-neutral-600"
                data-testid={t.secondary.testid ?? "toast-secondary"}
                onClick={() => {
                  t.secondary?.onClick();
                  api.dismiss(t.id);
                }}
              >
                {t.secondary.label}
              </button>
            )}
            {t.action && (
              <button
                className="rounded bg-sky-800 px-2 py-0.5 text-xs font-medium text-sky-100 hover:bg-sky-700"
                data-testid={t.action.testid ?? "toast-action"}
                onClick={() => {
                  t.action?.onClick();
                  api.dismiss(t.id);
                }}
              >
                {t.action.label}
              </button>
            )}
            <button onClick={() => api.dismiss(t.id)} aria-label="Dismiss" className="text-neutral-400 hover:text-neutral-100">
              <X className="size-3.5" />
            </button>
          </span>
        </div>
      ))}
    </div>
  );
}
