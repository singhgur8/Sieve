// Phase 2 ingest view: simple list with thumbnails, EXIF and import progress.
// The virtualized grid / loupe replace this in Phase 4.
import { memo, useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { AlertTriangle, Aperture, FolderOpen, ImageOff, Loader2, RotateCw } from "lucide-react";
import {
  commands,
  convertFileSrc,
  DEFAULT_QUERY,
  events,
  unwrap,
  type AppError,
  type CatalogState,
  type ImportProgress,
  type ImportStatus,
  type RawImageEntry,
} from "./ipc";

const PAGE_SIZE = 200;

export default function App() {
  const [catalog, setCatalog] = useState<CatalogState | null>(null);
  const [items, setItems] = useState<RawImageEntry[]>([]);
  const [total, setTotal] = useState(0);
  const [progress, setProgress] = useState<ImportProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const loadedRef = useRef(0);

  const patch = useCallback((id: number, fn: (e: RawImageEntry) => RawImageEntry) => {
    setItems((prev) => prev.map((e) => (e.id === id ? fn(e) : e)));
  }, []);

  const loadPage = useCallback(async (offset: number) => {
    const page = await unwrap(commands.listImages({ ...DEFAULT_QUERY, offset, limit: PAGE_SIZE }));
    setTotal(page.total);
    setItems((prev) => (offset === 0 ? page.items : [...prev, ...page.items]));
    loadedRef.current = offset + page.items.length;
  }, []);

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
  }, [refresh]);

  // Live events.
  useEffect(() => {
    const unlisten = [
      events.importProgress.listen((ev) => {
        setProgress(ev.payload);
        if (ev.payload.done >= ev.payload.total) {
          void unwrap(commands.getCatalogState()).then(setCatalog).catch(() => {});
        }
      }),
      events.thumbnailReady.listen((ev) => {
        const p = ev.payload;
        patch(p.imageId, (e) => ({
          ...e,
          thumbnail: { status: "ready", path: p.path, previewPath: p.previewPath, width: p.width, height: p.height },
        }));
        // EXIF is written in the same pass; fetch just this entry.
        unwrap(commands.getImage(p.imageId))
          .then((fresh) => patch(p.imageId, () => fresh))
          .catch(() => {});
      }),
      events.thumbnailFailed.listen((ev) => {
        patch(ev.payload.imageId, (e) => ({
          ...e,
          thumbnail: { status: "failed", reason: ev.payload.reason },
        }));
      }),
    ];
    return () => {
      unlisten.forEach((u) => void u.then((f) => f()));
    };
  }, [patch]);

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
      patch(id, (e) => ({ ...e, thumbnail: { status: "pending" } }));
      try {
        await unwrap(commands.regenerateThumbnails([id]));
      } catch (e) {
        setError(formatError(e));
      }
    },
    [patch],
  );

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

      {progress && (active || progress.failed > 0) && <ProgressBar progress={progress} active={active} />}
      {error && <p className="bg-red-950 px-4 py-2 text-sm text-red-300">{error}</p>}

      <ul className="flex-1 divide-y divide-neutral-900 overflow-auto">
        {items.map((img) => (
          <Row key={img.id} img={img} onRetry={retry} />
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

const Row = memo(function Row({ img, onRetry }: { img: RawImageEntry; onRetry: (id: number) => void }) {
  const t = img.thumbnail;
  const c = img.capture;
  return (
    <li className="flex items-center gap-4 px-4 py-2">
      <div className="flex size-20 shrink-0 items-center justify-center overflow-hidden rounded bg-neutral-900">
        {t.status === "ready" ? (
          <img
            src={convertFileSrc(t.path)}
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
