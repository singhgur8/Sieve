// Catalog state, ingest/analysis progress and XMP status, kept in sync with backend events.
import { useCallback, useEffect, useState } from "react";
import {
  commands,
  events,
  unwrap,
  type AnalysisStatus,
  type CatalogState,
  type ImportProgress,
  type ImportStatus,
  type XmpStatus,
} from "../ipc";
import { describeError, noteFailure, type ErrorInfo } from "../lib/errors";

export interface AnalysisView {
  done: number;
  total: number;
  failed: number;
  running: boolean;
  /** Reason of the most recent failure (analysisFailed), for the bar's tooltip. */
  lastReason?: string;
}

export function useBackendStatus(onLibraryChanged: () => void) {
  const [catalog, setCatalog] = useState<CatalogState | null>(null);
  const [progress, setProgress] = useState<ImportProgress | null>(null);
  const [analysis, setAnalysis] = useState<AnalysisView | null>(null);
  const [xmp, setXmp] = useState<XmpStatus | null>(null);
  const [error, setError] = useState<ErrorInfo | null>(null);
  /** Persistent catalog problem (read-only / damaged): shown as an inline banner until the app restarts. */
  const [catalogIssue, setCatalogIssue] = useState<ErrorInfo | null>(null);

  const reportError = useCallback((e: unknown) => {
    const info = describeError(e);
    noteFailure(info.message);
    if (info.persistent) setCatalogIssue(info);
    else setError(info);
  }, []);
  const refreshCatalog = useCallback(async () => {
    try {
      setCatalog(await unwrap(commands.getCatalogState()));
    } catch (e) {
      reportError(e);
    }
  }, [reportError]);
  /** Sidecar write failures seen this session (events and reports), keyed by image id. Cleared when the image is written. */
  const [xmpFailures, setXmpFailures] = useState<Map<number, string>>(new Map());
  const noteXmpFailures = useCallback((failed: { imageId: number; reason: string }[], written: number[] = [], replace = false) => {
    setXmpFailures((m) => {
      const n = new Map(replace ? [] : m);
      written.forEach((id) => n.delete(id));
      failed.forEach((f) => n.set(f.imageId, f.reason));
      return n;
    });
  }, []);
  const refreshXmp = useCallback(() => {
    unwrap(commands.getXmpStatus())
      .then(setXmp)
      .catch(() => {});
  }, []);

  useEffect(() => {
    void refreshCatalog();
    refreshXmp();
    unwrap(commands.getImportStatus())
      .then((s: ImportStatus) => {
        if (s.running || s.pending > 0) setProgress({ done: s.total - s.pending, total: s.total, failed: s.failed });
      })
      .catch(reportError);
    unwrap(commands.getAnalysisStatus())
      .then((s: AnalysisStatus) => {
        if (s.running || s.pending > 0 || s.failed > 0) {
          setAnalysis({ done: s.analyzed + s.failed, total: s.analyzed + s.failed + s.pending, failed: s.failed, running: s.running });
        }
      })
      .catch(reportError);
  }, [refreshCatalog, refreshXmp, reportError]);

  useEffect(() => {
    const unlisten = [
      events.importProgress.listen((ev) => {
        setProgress(ev.payload);
        if (ev.payload.done >= ev.payload.total) {
          void refreshCatalog();
          onLibraryChanged();
        }
      }),
      events.analysisProgress.listen((ev) => setAnalysis((a) => ({ ...ev.payload, running: ev.payload.done < ev.payload.total, lastReason: a?.lastReason }))),
      events.analysisFinished.listen(() => {
        setAnalysis((a) => (a ? { ...a, running: false } : a));
        void refreshCatalog();
        onLibraryChanged();
      }),
      events.analysisFailed.listen((ev) => setAnalysis((a) => (a ? { ...a, lastReason: ev.payload.reason } : a))),
      events.thumbnailFailed.listen((ev) => noteFailure(ev.payload.reason)),
      events.xmpSynced.listen((ev) => {
        noteXmpFailures([], ev.payload.written);
        refreshXmp();
      }),
      events.xmpWriteFailed.listen((ev) => {
        noteXmpFailures([ev.payload]);
        refreshXmp();
      }),
    ];
    return () => {
      unlisten.forEach((u) => void u.then((f) => f()));
    };
  }, [refreshCatalog, refreshXmp, onLibraryChanged, noteXmpFailures]);

  return { catalog, setCatalog, refreshCatalog, progress, analysis, setAnalysis, xmp, refreshXmp, xmpFailures, noteXmpFailures, error, setError, reportError, catalogIssue, setCatalogIssue };
}
