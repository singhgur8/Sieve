// Corner widget (bottom-right) listing background work: spinner or progress ring with done / total, a brief check when it
// finishes, a sticky message on errors. Fed by `events.activityEvent` through src/lib/activity.ts.
import { AlertTriangle, Check, Loader2, X } from "lucide-react";
import type { ActivityEvent } from "../ipc";
import { dismissActivity, useActivities } from "../lib/activity";

function Ring({ pct }: { pct: number }) {
  const r = 7;
  const c = 2 * Math.PI * r;
  return (
    <svg viewBox="0 0 18 18" className="size-[18px] shrink-0 -rotate-90" data-testid="activity-ring" data-pct={Math.round(pct)}>
      <circle cx="9" cy="9" r={r} fill="none" stroke="currentColor" strokeOpacity="0.25" strokeWidth="2" />
      <circle cx="9" cy="9" r={r} fill="none" stroke="#fbbf24" strokeWidth="2" strokeDasharray={c} strokeDashoffset={c * (1 - Math.min(1, Math.max(0, pct / 100)))} strokeLinecap="round" />
    </svg>
  );
}

const fmt = (n: number) => n.toLocaleString("en-US");

export function activityText(a: ActivityEvent): string {
  if (a.state === "running") return a.total != null ? `${a.label} ${fmt(a.done)} / ${fmt(a.total)}` : a.label;
  return a.message ? a.message : a.state === "finished" ? `${a.label}: done` : a.state === "cancelled" ? `${a.label}: cancelled` : a.label;
}

function Row({ a }: { a: ActivityEvent }) {
  const icon =
    a.state === "running" ? (a.total != null && a.total > 0 ? <Ring pct={(a.done / a.total) * 100} /> : <Loader2 className="size-[18px] shrink-0 animate-spin text-amber-300" data-testid="activity-spinner" />)
    : a.state === "finished" ? <Check className="size-[18px] shrink-0 text-emerald-400" />
    : a.state === "error" ? <AlertTriangle className="size-[18px] shrink-0 text-red-400" />
    : <X className="size-[18px] shrink-0 text-neutral-400" />;
  const tone = a.state === "error" ? "border-red-800 bg-red-950 text-red-100" : "border-neutral-700 bg-neutral-900 text-neutral-100";
  return (
    <div
      className={`pointer-events-auto flex items-start gap-2 rounded-lg border px-3 py-2 text-xs shadow-xl ${tone}`}
      data-testid="activity"
      data-kind={a.kind}
      data-state={a.state}
      role={a.state === "error" ? "alert" : "status"}
    >
      <span className="mt-px">{icon}</span>
      <div className="min-w-0 flex-1">
        {a.state === "running" && a.total != null && a.total > 0 ? (
          <>
            <div className="truncate" data-testid="activity-text">{activityText(a)}</div>
            <div className="mt-1 h-1 overflow-hidden rounded bg-neutral-700">
              <div className="h-full bg-amber-400" style={{ width: `${Math.min(100, (a.done / a.total) * 100)}%` }} />
            </div>
          </>
        ) : (
          <div className={a.state === "error" ? "break-words select-text" : "truncate"} data-testid="activity-text">
            {a.state === "error" ? <><span className="font-medium">{a.label}</span>{a.message ? `: ${a.message}` : ""}</> : activityText(a)}
          </div>
        )}
      </div>
      {a.state === "error" && (
        <button onClick={() => dismissActivity(a.id)} aria-label="Dismiss" data-testid="activity-dismiss" className="shrink-0 text-red-300 hover:text-red-100">
          <X className="size-3.5" />
        </button>
      )}
    </div>
  );
}

export function ActivityWidget() {
  const list = useActivities();
  if (list.length === 0) return null;
  return (
    <div className="pointer-events-none fixed bottom-3 right-3 z-[65] flex w-[min(320px,92vw)] flex-col items-stretch gap-2" data-testid="activity-widget" aria-live="polite">
      {list.map((a) => (
        <Row key={a.id} a={a} />
      ))}
    </div>
  );
}
