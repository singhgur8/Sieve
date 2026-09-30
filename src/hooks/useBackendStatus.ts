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
import { formatError } from "../lib/format";

export interface AnalysisView {
  done: number;
  total: number;
  failed: number;
  running: boolean;
}

export function useBackendStatus(onLibraryChanged: () => void) {
  const [catalog, setCatalog] = useState<CatalogState | null>(null);
  const [progress, setProgress] = useState<ImportProgress | null>(null);
  const [analysis, setAnalysis] = useState<AnalysisView | null>(null);
  const [xmp, setXmp] = useState<XmpStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  const reportError = useCallback((e: unknown) => setError(formatError(e)), []);
  const refreshCatalog = useCallback(async () => {
    try {
      setCatalog(await unwrap(commands.getCatalogState()));
    } catch (e) {
      reportError(e);
    }
  }, [reportError]);
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
      events.analysisProgress.listen((ev) => setAnalysis({ ...ev.payload, running: ev.payload.done < ev.payload.total })),
      events.analysisFinished.listen(() => {
        setAnalysis((a) => (a ? { ...a, running: false } : a));
        void refreshCatalog();
        onLibraryChanged();
      }),
      events.xmpSynced.listen(refreshXmp),
      events.xmpWriteFailed.listen(refreshXmp),
    ];
    return () => {
      unlisten.forEach((u) => void u.then((f) => f()));
    };
  }, [refreshCatalog, refreshXmp, onLibraryChanged]);

  return { catalog, setCatalog, refreshCatalog, progress, analysis, setAnalysis, xmp, refreshXmp, error, setError, reportError };
}
