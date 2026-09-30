import { Aperture, Check, CloudUpload, DownloadCloud, FolderOpen, Share, ScanSearch } from "lucide-react";
import type { CatalogState, ShootType, XmpStatus } from "../ipc";
import type { AnalysisView } from "../hooks/useBackendStatus";

const SHOOT_TYPES: ShootType[] = ["wedding", "portrait", "sports", "event", "landscape", "general"];
const btn = "flex items-center gap-1.5 rounded-md bg-neutral-800 px-2.5 py-1 text-sm hover:bg-neutral-700 disabled:opacity-50";

interface Props {
  catalog: CatalogState | null;
  analysis: AnalysisView | null;
  xmp: XmpStatus | null;
  busy: boolean;
  hasSelection: boolean;
  hasImages: boolean;
  onImport: () => void;
  onShootType: (t: ShootType) => void;
  onAnalyze: (kind: "pending" | "all") => void;
  onAutoAnalyze: (v: boolean) => void;
  onAutoXmp: (v: boolean) => void;
  onApplySuggestions: () => void;
  onWriteXmp: () => void;
  onReadXmp: () => void;
  onExport: () => void;
  exportsRunning: number;
}

export function TopBar(p: Props) {
  const c = p.catalog;
  const xmp = p.xmp;
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-neutral-800 px-4 py-2" data-testid="top-bar">
      <Aperture className="size-5 text-amber-400" />
      <h1 className="font-semibold tracking-tight">Sieve</h1>
      <span className="text-xs text-neutral-500">{c ? `${c.imageCount} images` : "…"}</span>
      <button onClick={p.onImport} disabled={p.busy} className={btn}>
        <FolderOpen className="size-4" />
        {p.busy ? "Importing…" : "Import"}
      </button>
      <label className="flex items-center gap-1.5 text-xs text-neutral-400">
        Shoot
        <select
          value={c?.shootType ?? "general"}
          onChange={(e) => p.onShootType(e.target.value as ShootType)}
          disabled={!c}
          className="rounded bg-neutral-800 px-2 py-1 text-sm text-neutral-200"
        >
          {SHOOT_TYPES.map((t) => (
            <option key={t} value={t}>
              {t}
            </option>
          ))}
        </select>
      </label>
      <button onClick={() => p.onAnalyze("pending")} disabled={p.analysis?.running} className={btn}>
        <ScanSearch className="size-4" />
        Analyze
      </button>
      <button onClick={() => p.onAnalyze("all")} disabled={p.analysis?.running} className={btn}>
        Re-analyze
      </button>
      <label className="flex items-center gap-1.5 text-xs text-neutral-400">
        <input type="checkbox" checked={c?.autoAnalyze ?? false} disabled={!c} onChange={(e) => p.onAutoAnalyze(e.target.checked)} />
        Auto-analyze
      </label>
      <button onClick={p.onApplySuggestions} disabled={!p.hasImages} className={btn} title="Copy suggested rating/pick to the selection (or all when nothing is selected)">
        <Check className="size-4" />
        Apply suggestions
      </button>

      <div className="ml-auto flex items-center gap-3">
        <span className="text-xs text-neutral-500" data-testid="xmp-status">
          {xmp ? `${xmp.dirty} unsaved${xmp.failed ? ` · ${xmp.failed} failed` : ""}` : ""}
        </span>
        <button onClick={p.onWriteXmp} disabled={!p.hasSelection} className={btn} title="Write XMP sidecars for the selection (Cmd+S)">
          <CloudUpload className="size-4" />
          Save metadata
        </button>
        <button onClick={p.onReadXmp} disabled={!p.hasSelection} className={btn} title="Read XMP sidecars of the selection into the catalog">
          <DownloadCloud className="size-4" />
          Read from file
        </button>
        <button onClick={p.onExport} disabled={!p.hasImages} data-testid="export-button" className={btn} title="Export the selection or the filtered set (Cmd+Shift+E)">
          <Share className="size-4" />
          Export{p.exportsRunning > 0 ? ` (${p.exportsRunning} running)` : ""}
        </button>
        <label className="flex items-center gap-1.5 text-xs text-neutral-400">
          <input type="checkbox" data-testid="xmp-auto" checked={c?.xmpAutoSync ?? false} disabled={!c} onChange={(e) => p.onAutoXmp(e.target.checked)} />
          Auto-sync XMP
        </label>
      </div>
    </div>
  );
}
