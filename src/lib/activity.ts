// Background activity store: one subscription to `events.activityEvent` (IPC v18) shared by the corner widget and by
// every button that must not start conflicting work. Updates are batched (<= ~7 renders/s) so a 10 Hz event stream
// from several activities never floods React.
import { useSyncExternalStore } from "react";
import { events, type ActivityEvent, type ActivityKind } from "../ipc";

const FLUSH_MS = 150;
const FINISHED_MS = 2200;
const CANCELLED_MS = 3200;

const items = new Map<number, ActivityEvent>();
let snapshot: ActivityEvent[] = [];
const listeners = new Set<() => void>();
let started = false;
let flushTimer: ReturnType<typeof setTimeout> | undefined;
const expiry = new Map<number, ReturnType<typeof setTimeout>>();

function publish() {
  flushTimer = undefined;
  snapshot = [...items.values()];
  listeners.forEach((l) => l());
}
function schedule(immediate = false) {
  if (immediate) {
    clearTimeout(flushTimer);
    publish();
  } else if (flushTimer === undefined) flushTimer = setTimeout(publish, FLUSH_MS);
}

export function dismissActivity(id: number) {
  clearTimeout(expiry.get(id));
  expiry.delete(id);
  if (items.delete(id)) schedule(true);
}

/** Applies one backend event. */
export function applyActivityEvent(e: ActivityEvent) {
  const prev = items.get(e.id);
  // A late running event must not resurrect a finished activity.
  if (prev && prev.state !== "running" && e.state === "running") return;
  const terminal = e.state !== "running";
  const ms = e.state === "finished" ? FINISHED_MS : e.state === "cancelled" ? CANCELLED_MS : null;
  items.set(e.id, e);
  clearTimeout(expiry.get(e.id));
  if (terminal && ms != null) expiry.set(e.id, setTimeout(() => dismissActivity(e.id), ms));
  // Start and terminal events show at once; progress ticks are batched.
  schedule(terminal || !prev);
}

function start() {
  if (started) return;
  started = true;
  void events.activityEvent.listen((ev) => applyActivityEvent(ev.payload));
}

function subscribe(l: () => void) {
  start();
  listeners.add(l);
  return () => {
    listeners.delete(l);
  };
}
const get = () => snapshot;

export function useActivities(): ActivityEvent[] {
  return useSyncExternalStore(subscribe, get, get);
}

/** True while an activity of one of `kinds` is running (re-renders only when the answer changes). */
export function useActivityRunning(...kinds: ActivityKind[]): boolean {
  return useSyncExternalStore(
    subscribe,
    () => snapshot.some((a) => a.state === "running" && kinds.includes(a.kind)),
    () => false,
  );
}

/** Tooltip for a button disabled by a running activity. */
export const BUSY_WHY: Partial<Record<ActivityKind, string>> = {
  xmp_save: "Metadata is being saved. Wait for it to finish.",
  apply_scene: "A scene edit is being applied. Wait for it to finish.",
  paste_sync: "Settings are being pasted or synced. Wait for it to finish.",
  export: "An export is starting. Wait a moment.",
};

/** Non-reactive check for key handlers (e.g. Cmd+S while a save runs). */
export function isActivityRunning(...kinds: ActivityKind[]): boolean {
  return snapshot.some((a) => a.state === "running" && kinds.includes(a.kind));
}
