// Transient messages: bottom-centre, above the filmstrip, never shift the layout.
// Info toasts fade after 4 s (8 s when they carry an action); errors are sticky.
import { useCallback, useEffect, useRef, useState } from "react";
import { X } from "lucide-react";

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
}

export interface ToastApi {
  toasts: Toast[];
  push: (message: string, opts?: { action?: ToastAction; kind?: "info" | "error" }) => number;
  dismiss: (id: number) => void;
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

  const push = useCallback(
    (message: string, opts: { action?: ToastAction; kind?: "info" | "error" } = {}) => {
      const id = next.current++;
      const kind = opts.kind ?? "info";
      // A new message replaces plain older ones; toasts carrying an action (Undo) stay until they expire.
      setToasts((all) => [...all.filter((t) => t.action).slice(-1), { id, message, kind, action: opts.action }]);
      if (kind === "info") timers.current.set(id, setTimeout(() => dismiss(id), opts.action ? 8000 : 4000));
      return id;
    },
    [dismiss],
  );

  useEffect(() => {
    const t = timers.current;
    return () => t.forEach((h) => clearTimeout(h));
  }, []);

  return { toasts, push, dismiss };
}

export function Toasts({ api, error, onDismissError }: { api: ToastApi; error: string | null; onDismissError: () => void }) {
  return (
    <div className="pointer-events-none fixed bottom-24 left-1/2 z-40 flex w-[min(480px,92vw)] -translate-x-1/2 flex-col items-stretch gap-2" data-testid="toasts">
      {error && (
        <div role="alert" className="pointer-events-auto flex items-start justify-between gap-3 rounded-lg border border-red-800 bg-red-950 px-3 py-2 text-sm text-red-200 shadow-xl" data-testid="error">
          <span className="min-w-0 break-words">{error}</span>
          <button onClick={onDismissError} aria-label="Dismiss error" className="shrink-0">
            <X className="size-4" />
          </button>
        </div>
      )}
      {api.toasts.map((t) => (
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
