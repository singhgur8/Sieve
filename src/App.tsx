// Phase 2 ingest view: simple list with thumbnails, EXIF and import progress.
// The virtualized grid / loupe replace this in Phase 4.
import { memo, useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { AlertTriangle, Aperture, Check, FolderOpen, ImageOff, Loader2, RotateCw, ScanSearch, Star, X } from "lucide-react";
import {
  commands,
  convertFileSrc,
  DEFAULT_QUERY,
  events,
  unwrap,
  type AnalysisStatus,
  type AppError,
  type CatalogState,
  type ImportProgress,
  type ImportStatus,
  type RawImageEntry,
  type ShootType,
} from "./ipc";

const PAGE_SIZE = 200;
const SHOOT_TYPES: ShootType[] = ["wedding", "portrait", "sports", "event", "landscape", "general"];

interface AnalysisView {
  done: number;
  total: number;
  failed: number;
  running: boolean;
}

export default function App() {
  const [catalog, setCatalog] = useState<CatalogState | null>(null);
  const [items, setItems] = useState<RawImageEntry[]>([]);
  const [total, setTotal] = useState(0);
  const [progress, setProgress] = useState<ImportProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [analysis, setAnalysis] = useState<AnalysisView | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [versions, setVersions] = useState<Record<number, number>>({});
  const loadedRef = useRef(0);
  // Thumbnail states delivered by events; they win over older list snapshots still marked pending.
  const eventThumbs = useRef(new Map<number, RawImageEntry["thumbnail"]>());

  const merge = useCallback((e: RawImageEntry): RawImageEntry => {
    const t = eventThumbs.current.get(e.id);
    return t && e.thumbnail.status === "pending" ? { ...e, thumbnail: t } : e;
  }, []);

  const patch = useCallback((id: number, fn: (e: RawImageEntry) => RawImageEntry) => {
    setItems((prev) => prev.map((e) => (e.id === id ? fn(e) : e)));
  }, []);

  const loadPage = useCallback(
    async (offset: number, limit = PAGE_SIZE) => {
      const page = await unwrap(commands.listImages({ ...DEFAULT_QUERY, offset, limit }));
      const fresh = page.items.map(merge);
      setTotal(page.total);
      setItems((prev) => (offset === 0 ? fresh : [...prev, ...fresh]));
      loadedRef.current = offset + page.items.length;
    },
    [merge],
  );

  const refresh = useCallback(async () => {
    try {
      setCatalog(await unwrap(commands.getCatalogState()));
      await loadPage(0);
    } catch (e) {
      setError(formatError(e));
    }
  }, [loadPage]);

  // Initial load + restore progress.
  useEffect(() => {
    void refresh();
    unwrap(commands.getImportStatus())
      .then((s: ImportStatus) => {
        const done = s.total - s.pending;
        if (s.running || s.pending > 0) setProgress({ done, total: s.total, failed: s.failed });
      })
      .catch((e) => setError(formatError(e)));
    unwrap(commands.getAnalysisStatus())
      .then((s: AnalysisStatus) => {
        if (s.running || s.pending > 0 || s.failed > 0) {
          setAnalysis({
            done: s.analyzed + s.failed,
            total: s.analyzed + s.failed + s.pending,
            failed: s.failed,
            running: s.running,
          });
        }
      })
      .catch((e) => setError(formatError(e)));
  }, [refresh]);

  // Live events.
  useEffect(() => {
    const unlisten = [
      events.importProgress.listen((ev) => {
        setProgress(ev.payload);
        if (ev.payload.done >= ev.payload.total) {
          void unwrap(commands.getCatalogState()).then(setCatalog).catch(() => {});
          // Re-fetch what is loaded so no row stays pending after a stale snapshot.
          void loadPage(0, Math.max(loadedRef.current, PAGE_SIZE)).catch(() => {});
        }
      }),
      events.thumbnailReady.listen((ev) => {
        const p = ev.payload;
        eventThumbs.current.set(p.imageId, {
          status: "ready",
          path: p.path,
          previewPath: p.previewPath,
          width: p.width,
          height: p.height,
        });
        setVersions((v) => ({ ...v, [p.imageId]: (v[p.imageId] ?? 0) + 1 }));
        patch(p.imageId, (e) => ({
          ...e,
          thumbnail: { status: "ready", path: p.path, previewPath: p.previewPath, width: p.width, height: p.height },
        }));
        // EXIF is written in the same pass; fetch just this entry.
        unwrap(commands.getImage(p.imageId))
          .then((fresh) => patch(p.imageId, () => merge(fresh)))
          .catch(() => {});
      }),
      events.thumbnailFailed.listen((ev) => {
        eventThumbs.current.set(ev.payload.imageId, { status: "failed", reason: ev.payload.reason });
        patch(ev.payload.imageId, (e) => ({
          ...e,
          thumbnail: { status: "failed", reason: ev.payload.reason },
        }));
      }),
      events.analysisProgress.listen((ev) => {
        setAnalysis({ ...ev.payload, running: ev.payload.done < ev.payload.total });
      }),
      events.analysisReady.listen((ev) => {
        const id = ev.payload.imageId;
        unwrap(commands.getImage(id))
          .then((fresh) => patch(id, () => merge(fresh)))
          .catch(() => {});
      }),
      events.analysisFinished.listen(() => {
        setAnalysis((a) => (a ? { ...a, running: false } : a));
        void unwrap(commands.getCatalogState()).then(setCatalog).catch(() => {});
        // Burst groups / duplicate tags may have changed on any row.
        void loadPage(0, Math.max(loadedRef.current, PAGE_SIZE)).catch(() => {});
      }),
    ];
    return () => {
      unlisten.forEach((u) => void u.then((f) => f()));
    };
  }, [patch, merge, loadPage]);

  async function importFolder() {
    const path = await open({ directory: true, title: "Import RAW folder" });
    if (typeof path !== "string") return;
    setBusy(true);
    setError(null);
    try {
      await unwrap(commands.importFolder(path, { recursive: true }));
      await refresh();
    } catch (e) {
      setError(formatError(e));
    } finally {
      setBusy(false);
    }
  }

  const retry = useCallback(
    async (id: number) => {
      eventThumbs.current.delete(id);
      patch(id, (e) => ({ ...e, thumbnail: { status: "pending" } }));
      try {
        await unwrap(commands.regenerateThumbnails([id]));
      } catch (e) {
        setError(formatError(e));
      }
    },
    [patch],
  );

  async function run(fn: () => Promise<unknown>) {
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError(formatError(e));
    }
  }

  const analyze = (kind: "pending" | "all") =>
    run(async () => {
      setAnalysis((a) => ({ done: 0, total: a?.total ?? 0, failed: 0, running: true }));
      await unwrap(commands.analyzeImages({ kind }));
    });

  const changeShootType = (t: ShootType) =>
    run(async () => {
      await unwrap(commands.setShootType(t));
      setCatalog(await unwrap(commands.getCatalogState()));
    });

  const toggleAuto = (enabled: boolean) =>
    run(async () => {
      await unwrap(commands.setAutoAnalyze(enabled));
      setCatalog((c) => (c ? { ...c, autoAnalyze: enabled } : c));
    });

  const applyAll = () =>
    run(async () => {
      const n = await unwrap(commands.applySuggestions(items.map((i) => i.id)));
      setNotice(`Applied suggestions to ${n} of ${items.length} loaded images`);
      await loadPage(0, Math.max(loadedRef.current, PAGE_SIZE));
    });

  const active = progress !== null && progress.done < progress.total;

  return (
    <main className="flex h-screen flex-col">
      <header className="flex items-center gap-3 border-b border-neutral-800 px-4 py-3">
        <Aperture className="size-5 text-amber-400" />
        <h1 className="font-semibold tracking-tight">LumenRAW</h1>
        <span className="truncate text-xs text-neutral-500">
          {catalog ? `${catalog.imageCount} images · ${catalog.shootType}` : "…"}
        </span>
        <button
          onClick={importFolder}
          disabled={busy}
          className="ml-auto flex items-center gap-2 rounded-md bg-neutral-800 px-3 py-1.5 text-sm hover:bg-neutral-700 disabled:opacity-50"
        >
          <FolderOpen className="size-4" />
          {busy ? "Importing…" : "Import folder"}
        </button>
      </header>

      <div className="flex flex-wrap items-center gap-3 border-b border-neutral-800 px-4 py-2 text-sm">
        <label className="flex items-center gap-2 text-xs text-neutral-400">
          Shoot type
          <select
            value={catalog?.shootType ?? "general"}
            onChange={(e) => void changeShootType(e.target.value as ShootType)}
            disabled={!catalog}
            className="rounded bg-neutral-800 px-2 py-1 text-sm text-neutral-200"
          >
            {SHOOT_TYPES.map((t) => (
              <option key={t} value={t}>
                {t}
              </option>
            ))}
          </select>
        </label>
        <button onClick={() => void analyze("pending")} disabled={analysis?.running} className={btn}>
          <ScanSearch className="size-4" />
          Analyze
        </button>
        <button onClick={() => void analyze("all")} disabled={analysis?.running} className={btn}>
          Re-analyze all
        </button>
        <label className="flex items-center gap-1.5 text-xs text-neutral-400">
          <input
            type="checkbox"
            checked={catalog?.autoAnalyze ?? false}
            disabled={!catalog}
            onChange={(e) => void toggleAuto(e.target.checked)}
          />
          Auto-analyze
        </label>
        <button onClick={applyAll} disabled={items.length === 0} className={`${btn} ml-auto`}>
          <Check className="size-4" />
          Apply suggestions
        </button>
      </div>

      {analysis && (analysis.running || analysis.failed > 0 || analysis.done < analysis.total) && (
        <AnalysisBar
          a={analysis}
          onCancel={() => void run(() => unwrap(commands.cancelAnalysis()))}
        />
      )}
      {notice && (
        <p className="flex items-center justify-between bg-neutral-900 px-4 py-1.5 text-xs text-neutral-300">
          {notice}
          <button onClick={() => setNotice(null)} aria-label="Dismiss">
            <X className="size-3.5" />
          </button>
        </p>
      )}

      {progress && (active || progress.failed > 0) && <ProgressBar progress={progress} active={active} />}
      {error && <p className="bg-red-950 px-4 py-2 text-sm text-red-300">{error}</p>}

      <ul className="flex-1 divide-y divide-neutral-900 overflow-auto">
        {items.map((img) => (
          <Row key={img.id} img={img} version={versions[img.id] ?? 0} onRetry={retry} />
        ))}
        {items.length < total && (
          <li className="px-4 py-3">
            <button
              onClick={() => loadPage(loadedRef.current).catch((e) => setError(formatError(e)))}
              className="rounded-md bg-neutral-800 px-3 py-1.5 text-sm hover:bg-neutral-700"
            >
              Load more ({items.length} of {total})
            </button>
          </li>
        )}
      </ul>
    </main>
  );
}

const btn =
  "flex items-center gap-2 rounded-md bg-neutral-800 px-3 py-1 text-sm hover:bg-neutral-700 disabled:opacity-50";

function AnalysisBar({ a, onCancel }: { a: AnalysisView; onCancel: () => void }) {
  const pct = a.total > 0 ? Math.min(100, (a.done / a.total) * 100) : 0;
  return (
    <div className="border-b border-neutral-800 px-4 py-2">
      <div className="mb-1 flex items-center gap-2 text-xs text-neutral-400">
        {a.running && <Loader2 className="size-3 animate-spin" />}
        <span>
          {a.running ? "Analyzing" : "Analysis paused"}: {a.done} / {a.total}
        </span>
        {a.failed > 0 && <span className="text-red-400">{a.failed} failed</span>}
        {a.running && (
          <button onClick={onCancel} className="ml-auto rounded bg-neutral-800 px-2 py-0.5 hover:bg-neutral-700">
            Cancel
          </button>
        )}
      </div>
      <div className="h-1.5 overflow-hidden rounded bg-neutral-800">
        <div className="h-full bg-sky-400 transition-[width]" style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}

function ProgressBar({ progress, active }: { progress: ImportProgress; active: boolean }) {
  const pct = progress.total > 0 ? Math.min(100, (progress.done / progress.total) * 100) : 0;
  return (
    <div className="border-b border-neutral-800 px-4 py-2">
      <div className="mb-1 flex items-center gap-2 text-xs text-neutral-400">
        {active && <Loader2 className="size-3 animate-spin" />}
        <span>
          {active ? "Extracting thumbnails" : "Done"}: {progress.done} / {progress.total}
        </span>
        {progress.failed > 0 && <span className="text-red-400">{progress.failed} failed</span>}
      </div>
      <div className="h-1.5 overflow-hidden rounded bg-neutral-800">
        <div className="h-full bg-amber-400 transition-[width]" style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}

const Row = memo(function Row({
  img,
  version,
  onRetry,
}: {
  img: RawImageEntry;
  version: number;
  onRetry: (id: number) => void;
}) {
  const t = img.thumbnail;
  const c = img.capture;
  return (
    <li className="flex items-center gap-4 px-4 py-2">
      <div className="flex size-20 shrink-0 items-center justify-center overflow-hidden rounded bg-neutral-900">
        {t.status === "ready" ? (
          <img
            src={`${convertFileSrc(t.path)}?v=${version}`}
            loading="lazy"
            decoding="async"
            alt={img.fileName}
            className="size-full object-contain"
          />
        ) : t.status === "pending" ? (
          <Loader2 className="size-5 animate-spin text-neutral-600" />
        ) : (
          <ImageOff className="size-5 text-red-500" />
        )}
      </div>
      <div className="min-w-0 flex-1 text-sm">
        <div className="flex items-center gap-2">
          <span className="truncate font-medium" title={img.path}>
            {img.fileName}
          </span>
          <span className="text-xs uppercase text-neutral-500">{img.format}</span>
        </div>
        <div className="text-xs text-neutral-400">
          {[img.camera.model, c.lens].filter(Boolean).join(" · ") || "—"}
        </div>
        <div className="text-xs text-neutral-500">
          {[
            c.capturedAtMs != null ? formatTime(c.capturedAtMs) : null,
            c.iso != null ? `ISO ${c.iso}` : null,
            c.shutterSeconds != null ? formatShutter(c.shutterSeconds) : null,
            c.aperture != null ? `f/${trimNum(c.aperture)}` : null,
            c.focalLengthMm != null ? `${trimNum(c.focalLengthMm)} mm` : null,
          ]
            .filter(Boolean)
            .join(" · ") || "No EXIF yet"}
        </div>
        <CullInfo img={img} />
        {t.status === "failed" && (
          <div className="mt-1 flex items-center gap-2 text-xs text-red-400">
            <AlertTriangle className="size-3.5 shrink-0" />
            <span className="rounded bg-red-950 px-1.5 py-0.5">Failed</span>
            <span className="truncate" title={t.reason}>
              {t.reason}
            </span>
            <button
              onClick={() => onRetry(img.id)}
              className="ml-1 flex items-center gap-1 rounded bg-neutral-800 px-2 py-0.5 text-neutral-200 hover:bg-neutral-700"
            >
              <RotateCw className="size-3" />
              Retry
            </button>
          </div>
        )}
      </div>
    </li>
  );
});

function CullInfo({ img }: { img: RawImageEntry }) {
  const q = img.quality;
  if (!q && img.tags.length === 0 && img.burstGroupId == null) return null;
  return (
    <div className="mt-1 flex flex-wrap items-center gap-1.5 text-xs">
      {img.tags.map((t) => (
        <span
          key={t.tag}
          title={`${t.source}, confidence ${t.confidence.toFixed(2)}${t.suppressed ? " (dismissed)" : ""}`}
          className={
            t.suppressed
              ? "rounded bg-neutral-900 px-1.5 py-0.5 text-neutral-600 line-through"
              : "rounded bg-amber-950 px-1.5 py-0.5 text-amber-300"
          }
        >
          {t.tag.replace("_", " ")}
        </span>
      ))}
      {q && (
        <>
          <span className="text-neutral-300" title="Overall quality score">
            Q {Math.round(q.overall * 100)}
          </span>
          <span className="flex items-center gap-0.5 text-neutral-400" title="Suggested rating">
            <Star className="size-3" />
            {q.suggestedRating}
          </span>
          {q.suggestedPick !== "unflagged" && (
            <span
              className={`rounded px-1.5 py-0.5 ${q.suggestedPick === "pick" ? "bg-green-950 text-green-300" : "bg-red-950 text-red-300"}`}
            >
              suggest {q.suggestedPick}
            </span>
          )}
        </>
      )}
      {img.burstGroupId != null && (
        <span className="rounded bg-neutral-800 px-1.5 py-0.5 text-neutral-300">
          burst #{img.burstGroupId}
          {img.isBurstKeeper ? " · keeper" : ""}
        </span>
      )}
    </div>
  );
}

const timeFmt = new Intl.DateTimeFormat(undefined, {
  timeZone: "UTC",
  dateStyle: "medium",
  timeStyle: "medium",
});

function formatTime(ms: number): string {
  return timeFmt.format(new Date(ms));
}

function formatShutter(s: number): string {
  return s >= 1 ? `${trimNum(s)}s` : `1/${Math.round(1 / s)}s`;
}

function trimNum(n: number): string {
  return String(Math.round(n * 10) / 10);
}

function formatError(e: unknown): string {
  const err = e as Partial<AppError>;
  return err.kind ? `${err.kind}: ${err.message}` : String(e);
}
