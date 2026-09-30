// Export job tracking: seeds from get_export_jobs, then follows exportProgress / exportFinished.
import { useCallback, useEffect, useRef, useState } from "react";
import { commands, events, unwrap, type ExportFinished, type ExportJob } from "../ipc";

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

export function useExportJobs(onError: (e: unknown) => void) {
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

  useEffect(() => {
    unwrap(commands.getExportJobs())
      .then((list) =>
        list
          .filter((j) => j.state === "queued" || j.state === "running")
          .forEach((j) => upsert(j.id, (c) => (c.finished ? {} : { ...fromJob(j), running: true }))),
      )
      .catch(() => {});
    // Progress may arrive before export_images resolves; unknown jobs are created on the fly.
    const unlisten = [
      events.exportProgress.listen((ev) => {
        const p = ev.payload;
        upsert(p.jobId, (c) =>
          c.finished ? {} : { total: p.total, done: p.done, failed: p.failed, skipped: p.skipped, currentFile: p.currentFile, running: true },
        );
      }),
      events.exportFinished.listen((ev) => {
        const f = ev.payload;
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
      }),
    ];
    const pending = timers.current;
    return () => {
      unlisten.forEach((u) => void u.then((fn) => fn()));
      pending.forEach((t) => clearTimeout(t));
    };
  }, [upsert]);

  const track = useCallback((j: ExportJob) => upsert(j.id, (c) => (c.finished ? {} : fromJob(j))), [upsert]);

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
