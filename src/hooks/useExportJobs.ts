// Export job tracking: seeds from get_export_jobs, then follows exportProgress / exportFinished.
import { useCallback, useEffect, useRef, useState } from "react";
import { commands, events, unwrap, type ExportFinished, type ExportJob } from "../ipc";
import { noteFailure } from "../lib/errors";

export interface JobView {
  id: number;
  presetName: string | null;
  total: number;
  done: number;
  failed: number;
  skipped: number;
  currentFile: string | null;
  outputDir: string | null;
  running: boolean;
  cancelling: boolean;
  finished: ExportFinished | null;
  /** Finished job collapsed into the 28 px pill (8 s after completion, unless something failed). */
  collapsed: boolean;
}

const blank = (id: number): JobView => ({
  id,
  presetName: null,
  total: 0,
  done: 0,
  failed: 0,
  skipped: 0,
  currentFile: null,
  outputDir: null,
  running: true,
  cancelling: false,
  finished: null,
  collapsed: false,
});

const fromJob = (j: ExportJob): Partial<JobView> => ({
  presetName: j.presetName,
  total: j.total,
  done: j.done,
  failed: j.failed,
  skipped: j.skipped,
  outputDir: j.outputDir,
});

const COLLAPSE_MS = 8000;

/** `projectId` scopes the list to that project's jobs (null = every job). */
export function useExportJobs(onError: (e: unknown) => void, projectId: number | null = null) {
  const [jobs, setJobs] = useState<JobView[]>([]);
  const timers = useRef(new Set<ReturnType<typeof setTimeout>>());

  const upsert = useCallback((id: number, patch: (j: JobView) => Partial<JobView>) => {
    setJobs((all) => {
      const i = all.findIndex((j) => j.id === id);
      const cur = i >= 0 ? all[i] : blank(id);
      const next = { ...cur, ...patch(cur) };
      if (i >= 0) return all.map((j, k) => (k === i ? next : j));
      return [...all, next];
    });
  }, []);

  // Jobs of this project (started here, or confirmed by `get_export_jobs`) and of other projects (ignored).
  const mine = useRef(new Set<number>());
  const foreign = useRef(new Set<number>());
  const checking = useRef(new Set<number>());
  const inScope = useCallback((j: ExportJob) => projectId == null || j.projectId == null || j.projectId === projectId, [projectId]);

  /** Runs `fn` when `jobId` belongs to this project; unknown jobs are looked up once (events carry no project). */
  const ifMine = useCallback(
    (jobId: number, fn: () => void) => {
      if (projectId == null || mine.current.has(jobId)) return fn();
      if (foreign.current.has(jobId) || checking.current.has(jobId)) return;
      checking.current.add(jobId);
      unwrap(commands.getExportJobs())
        .then((list) => {
          const j = list.find((x) => x.id === jobId);
          if (!j || inScope(j)) {
            mine.current.add(jobId);
            fn();
          } else foreign.current.add(jobId);
        })
        .catch(() => {})
        .finally(() => checking.current.delete(jobId));
    },
    [projectId, inScope],
  );

  useEffect(() => {
    unwrap(commands.getExportJobs())
      .then((list) =>
        list
          .filter((j) => (j.state === "queued" || j.state === "running") && inScope(j))
          .forEach((j) => {
            mine.current.add(j.id);
            upsert(j.id, (c) => (c.finished ? {} : { ...fromJob(j), running: true }));
          }),
      )
      .catch(() => {});
    // Progress may arrive before export_images resolves; unknown jobs are created on the fly.
    const unlisten = [
      events.exportProgress.listen((ev) => {
        const p = ev.payload;
        ifMine(p.jobId, () =>
          upsert(p.jobId, (c) =>
            c.finished ? {} : { total: p.total, done: p.done, failed: p.failed, skipped: p.skipped, currentFile: p.currentFile, running: true },
          ),
        );
      }),
      events.exportFinished.listen((ev) => {
        const f = ev.payload;
        f.failed.forEach((x) => noteFailure(x.reason));
        ifMine(f.jobId, () => {
        upsert(f.jobId, (c) => ({
          finished: f,
          running: false,
          cancelling: false,
          currentFile: null,
          outputDir: f.outputDir ?? c.outputDir,
          done: f.succeeded + f.skipped + f.failed.length,
          failed: f.failed.length,
          skipped: f.skipped,
        }));
        if (f.failed.length === 0) {
          const t = setTimeout(() => {
            timers.current.delete(t);
            upsert(f.jobId, () => ({ collapsed: true }));
          }, COLLAPSE_MS);
          timers.current.add(t);
        }
        });
      }),
    ];
    const pending = timers.current;
    return () => {
      unlisten.forEach((u) => void u.then((fn) => fn()));
      pending.forEach((t) => clearTimeout(t));
    };
  }, [upsert, ifMine, inScope]);

  const track = useCallback(
    (j: ExportJob) => {
      mine.current.add(j.id);
      upsert(j.id, (c) => (c.finished ? {} : fromJob(j)));
    },
    [upsert],
  );

  const cancel = useCallback(
    async (id: number) => {
      upsert(id, () => ({ cancelling: true }));
      try {
        await unwrap(commands.cancelExport(id));
      } catch (e) {
        upsert(id, () => ({ cancelling: false }));
        onError(e);
      }
    },
    [upsert, onError],
  );

  const dismiss = useCallback((id: number) => setJobs((all) => all.filter((j) => j.id !== id)), []);

  return { jobs, track, cancel, dismiss };
}
