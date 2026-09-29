// Phase 1 contract smoke test: exercises catalog state, import and listing over IPC.
// The real grid / loupe UI replaces this in Phase 4.
import { useCallback, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Aperture, FolderOpen } from "lucide-react";
import {
  commands,
  DEFAULT_QUERY,
  unwrap,
  type AppError,
  type CatalogState,
  type ImagePage,
  type ImportSummary,
} from "./ipc";

export default function App() {
  const [catalog, setCatalog] = useState<CatalogState | null>(null);
  const [page, setPage] = useState<ImagePage | null>(null);
  const [lastImport, setLastImport] = useState<ImportSummary | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setCatalog(await unwrap(commands.getCatalogState()));
      setPage(await unwrap(commands.listImages(DEFAULT_QUERY)));
    } catch (e) {
      setError(formatError(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  async function importFolder() {
    const path = await open({ directory: true, title: "Import RAW folder" });
    if (typeof path !== "string") return;
    setBusy(true);
    setError(null);
    try {
      setLastImport(await unwrap(commands.importFolder(path, { recursive: true })));
      await refresh();
    } catch (e) {
      setError(formatError(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <main className="flex h-screen flex-col">
      <header className="flex items-center gap-3 border-b border-neutral-800 px-4 py-3">
        <Aperture className="size-5 text-amber-400" />
        <h1 className="font-semibold tracking-tight">LumenRAW</h1>
        <span className="text-xs text-neutral-500">
          {catalog ? `${catalog.imageCount} images · ${catalog.shootType} · ${catalog.catalogPath}` : "…"}
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

      {error && <p className="bg-red-950 px-4 py-2 text-sm text-red-300">{error}</p>}
      {lastImport && (
        <p className="px-4 py-2 text-sm text-neutral-400">
          Added {lastImport.added}, already in catalog {lastImport.skipped}, invalid {lastImport.invalid}
        </p>
      )}

      <div className="flex-1 overflow-auto">
        <table className="w-full text-left text-sm">
          <thead className="sticky top-0 bg-neutral-900 text-neutral-400">
            <tr>
              <th className="px-4 py-2 font-medium">File</th>
              <th className="px-4 py-2 font-medium">Format</th>
              <th className="px-4 py-2 font-medium">Make</th>
              <th className="px-4 py-2 font-medium">Sensor</th>
              <th className="px-4 py-2 font-medium">Thumbnail</th>
            </tr>
          </thead>
          <tbody>
            {page?.items.map((img) => (
              <tr key={img.id} className="border-b border-neutral-900">
                <td className="px-4 py-1.5" title={img.path}>{img.fileName}</td>
                <td className="px-4 py-1.5 uppercase">{img.format}</td>
                <td className="px-4 py-1.5">{img.camera.make}</td>
                <td className="px-4 py-1.5">{img.camera.sensorLayout}</td>
                <td className="px-4 py-1.5">{img.thumbnail.status}</td>
              </tr>
            ))}
          </tbody>
        </table>
        {page && page.total > page.items.length && (
          <p className="px-4 py-2 text-xs text-neutral-500">
            Showing {page.items.length} of {page.total}
          </p>
        )}
      </div>
    </main>
  );
}

function formatError(e: unknown): string {
  const err = e as Partial<AppError>;
  return err.kind ? `${err.kind}: ${err.message}` : String(e);
}
