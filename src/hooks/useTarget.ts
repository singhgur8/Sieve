// Data hooks of the "Pick the best N" flow (IPC v20).
import { useCallback, useEffect, useRef, useState } from "react";
import { commands, events, unwrap, type RawImageEntry, type TargetRun } from "../ipc";

/** The project's target run (null = never run) and a refresh; follows `targetRunFinished`. */
export function useTargetRun(projectId: number | null) {
  const [run, setRun] = useState<{ projectId: number; run: TargetRun | null } | null>(null);
  const [loaded, setLoaded] = useState(false);
  const refresh = useCallback(async () => {
    if (projectId == null) return;
    try {
      const r = await unwrap(commands.getTargetRun(projectId));
      setRun({ projectId, run: r });
    } catch {
      /* no run to show */
    } finally {
      setLoaded(true);
    }
  }, [projectId]);
  useEffect(() => {
    setLoaded(false);
    void refresh();
    const un = events.targetRunFinished.listen((ev) => {
      if (ev.payload.run.projectId === projectId) setRun({ projectId: projectId!, run: ev.payload.run });
    });
    return () => void un.then((f) => f());
  }, [projectId, refresh]);
  return { run: run && run.projectId === projectId ? run.run : null, loaded, refresh };
}

/** Photo entries by id, fetched on demand (thumbnails and names of alternatives, covered-by photos, neighbours). */
export function useEntries(onError: (e: unknown) => void) {
  const cache = useRef(new Map<number, RawImageEntry>());
  const inflight = useRef(new Set<number>());
  const [, setTick] = useState(0);
  const need = useCallback(
    (ids: number[]) => {
      const want = [...new Set(ids)].filter((i) => !cache.current.has(i) && !inflight.current.has(i));
      if (want.length === 0) return;
      want.forEach((i) => inflight.current.add(i));
      unwrap(commands.getImages(want))
        .then((rows) => rows.forEach((r) => cache.current.set(r.id, r)))
        .catch(onError)
        .finally(() => {
          want.forEach((i) => inflight.current.delete(i));
          setTick((n) => n + 1);
        });
    },
    [onError],
  );
  /** Drops entries so the next `need` refetches them (flags changed). */
  const drop = useCallback((ids: number[]) => ids.forEach((i) => cache.current.delete(i)), []);
  const get = useCallback((id: number) => cache.current.get(id), []);
  return { get, need, drop };
}
