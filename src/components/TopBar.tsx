// One 44 px application bar: import / shoot / analyze on the left, module switcher in the middle,
// XMP state, save and export on the right. Rarely used actions live in the Analyze and "more" menus.
import { Aperture, Check, ChevronDown, CloudUpload, Columns2, Cpu, DownloadCloud, FolderOpen, Grid3x3, Keyboard, Maximize, MoreHorizontal, ScanSearch, Share, SlidersHorizontal, Layers3 } from "lucide-react";
import type { CatalogState, ImportOptions, ShootType, XmpStatus } from "../ipc";
import type { AnalysisView } from "../hooks/useBackendStatus";
import { hint, type Mode } from "../lib/keymap";
import { Menu, menuItem } from "./Menu";

const SHOOT_TYPES: ShootType[] = ["wedding", "portrait", "sports", "event", "landscape", "general"];
const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);
const btn = "flex h-7 items-center gap-1.5 whitespace-nowrap rounded-md bg-neutral-800 px-2.5 text-sm hover:bg-neutral-700 disabled:opacity-50";
const seg = (on: boolean) => `flex h-7 items-center gap-1 whitespace-nowrap rounded px-2 text-xs ${on ? "bg-sky-800 text-sky-100" : "bg-neutral-800 hover:bg-neutral-700"}`;

interface Props {
  catalog: CatalogState | null;
  analysis: AnalysisView | null;
  xmp: XmpStatus | null;
  busy: boolean;
  mode: Mode;
  onMode: (m: Mode) => void;
  hasSelection: boolean;
  hasImages: boolean;
  detecting: boolean;
  onImport: () => void;
  importOptions: ImportOptions;
  onImportOptions: (patch: Partial<ImportOptions>) => void;
  onShootType: (t: ShootType) => void;
  onAnalyze: (kind: "pending" | "all") => void;
  onAutoAnalyze: (v: boolean) => void;
  onDetectScenes: () => void;
  onAutoXmp: (v: boolean) => void;
  onApplySuggestions: () => void;
  onWriteXmp: () => void;
  onSaveAllDirty: () => void;
  onReadXmp: () => void;
  onExport: () => void;
  onCheatSheet: () => void;
  onModels: () => void;
  /** 0-100 while export jobs run, otherwise null. */
  exportPct: number | null;
}

function Ring({ pct }: { pct: number }) {
  const r = 6;
  const c = 2 * Math.PI * r;
  return (
    <svg viewBox="0 0 16 16" className="size-4 -rotate-90" data-testid="export-ring" data-pct={Math.round(pct)}>
      <circle cx="8" cy="8" r={r} fill="none" stroke="currentColor" strokeOpacity="0.25" strokeWidth="2" />
      <circle cx="8" cy="8" r={r} fill="none" stroke="#fbbf24" strokeWidth="2" strokeDasharray={c} strokeDashoffset={c * (1 - pct / 100)} strokeLinecap="round" />
    </svg>
  );
}

export function TopBar(p: Props) {
  const c = p.catalog;
  const xmp = p.xmp;
  const dirty = xmp?.dirty ?? 0;
  const modes: { m: Mode; label: string; icon: typeof Grid3x3; title: string; testid: string }[] = [
    { m: "grid", label: "Grid", icon: Grid3x3, title: `Grid${hint("toGrid")}`, testid: "mode-grid" },
    { m: "loupe", label: "Loupe", icon: Maximize, title: `Loupe${hint("toggleLoupe")}`, testid: "mode-loupe" },
    { m: "compare", label: "Compare", icon: Columns2, title: `Compare${hint("compare")}`, testid: "mode-compare" },
    { m: "develop", label: "Develop", icon: SlidersHorizontal, title: `Develop${hint("develop")}`, testid: "mode-develop" },
  ];
  return (
    <div className="flex h-11 shrink-0 items-center gap-2 border-b border-neutral-800 px-3" data-testid="top-bar">
      <Aperture className="size-5 shrink-0 text-amber-400" />
      <h1 className="mr-1 font-semibold tracking-tight">Sieve</h1>
      <div className="flex" data-testid="import-split">
        <button onClick={p.onImport} disabled={p.busy} className={`${btn} rounded-r-none`} data-testid="import-button" title={`Import a shoot folder${hint("import")}`}>
          <FolderOpen className="size-4" />
          {p.busy ? "Importing…" : "Import"}
        </button>
        <Menu trigger={<ChevronDown className="size-4" />} triggerClass={`${btn} rounded-l-none border-l border-neutral-700 px-1.5`} triggerTestId="import-options" title="Import options" disabled={p.busy}>
          {(close) => (
            <div className="w-64 py-1" data-testid="import-options-popover">
              <label className={`${menuItem} cursor-pointer`}>
                <input type="checkbox" data-testid="import-include-nonraw" checked={!!p.importOptions.includeNonRaw} onChange={(e) => p.onImportOptions({ includeNonRaw: e.target.checked })} />
                Include JPEG, HEIC, TIFF, PNG
              </label>
              <label className={`${menuItem} cursor-pointer ${p.importOptions.includeNonRaw ? "" : "opacity-40"}`} title="A camera JPEG next to a RAW with the same name is shown as +JPG on the RAW instead of a separate photo">
                <input type="checkbox" data-testid="import-pair-jpeg" disabled={!p.importOptions.includeNonRaw} checked={!!p.importOptions.pairJpegWithRaw} onChange={(e) => p.onImportOptions({ pairJpegWithRaw: e.target.checked })} />
                Pair JPEG with its RAW
              </label>
              <button
                className={`${menuItem} border-t border-neutral-800`}
                data-testid="import-choose"
                onClick={() => {
                  close();
                  p.onImport();
                }}
              >
                <FolderOpen className="size-4" /> Choose folder...
              </button>
            </div>
          )}
        </Menu>
      </div>
      <label className="flex items-center gap-1 text-xs text-neutral-400">
        Shoot
        <select
          value={c?.shootType ?? "general"}
          onChange={(e) => p.onShootType(e.target.value as ShootType)}
          disabled={!c}
          className="h-7 rounded bg-neutral-800 px-1.5 text-sm text-neutral-200"
          data-testid="shoot-select"
        >
          {SHOOT_TYPES.map((t) => (
            <option key={t} value={t}>
              {cap(t)}
            </option>
          ))}
        </select>
      </label>
      <div className="flex" data-testid="analyze-split">
        <button onClick={() => p.onAnalyze("pending")} disabled={p.analysis?.running} className={`${btn} rounded-r-none`} data-testid="analyze-button" title="Analyze photos that have not been analyzed yet">
          <ScanSearch className="size-4" />
          Analyze
        </button>
        <Menu trigger={<ChevronDown className="size-4" />} triggerClass={`${btn} rounded-l-none border-l border-neutral-700 px-1.5`} triggerTestId="analyze-menu" title="More analysis options">
          {(close) => (
            <>
              <button
                className={menuItem}
                disabled={p.analysis?.running}
                data-testid="reanalyze-all"
                onClick={() => {
                  close();
                  p.onAnalyze("all");
                }}
              >
                Re-analyze all photos
              </button>
              <button
                className={menuItem}
                disabled={p.detecting || !p.hasImages}
                data-testid="scenes-detect"
                onClick={() => {
                  close();
                  p.onDetectScenes();
                }}
              >
                <Layers3 className="size-4" /> Detect scenes
              </button>
              <label className={`${menuItem} cursor-pointer`}>
                <input type="checkbox" data-testid="auto-analyze" checked={c?.autoAnalyze ?? false} disabled={!c} onChange={(e) => p.onAutoAnalyze(e.target.checked)} />
                Auto-analyze new photos
              </label>
            </>
          )}
        </Menu>
      </div>

      <div className="mx-auto flex gap-1" data-testid="mode-switcher">
        {modes.map(({ m, label, icon: Icon, title, testid }) => (
          <button key={m} className={seg(p.mode === m)} onClick={() => p.onMode(m)} title={title} data-testid={testid}>
            <Icon className="size-3.5" /> {label}
          </button>
        ))}
      </div>

      {dirty > 0 && (
        <button
          onClick={p.onSaveAllDirty}
          className="flex h-7 items-center gap-1.5 whitespace-nowrap rounded-full bg-amber-900/70 px-3 text-xs font-medium text-amber-100 ring-1 ring-amber-600 hover:bg-amber-800"
          data-testid="xmp-status"
          title="Write XMP sidecars for every photo with unsaved changes"
        >
          <CloudUpload className="size-3.5" />
          {dirty} photo{dirty === 1 ? "" : "s"} not saved to XMP{xmp && xmp.failed > 0 ? ` · ${xmp.failed} failed` : ""}
        </button>
      )}
      {dirty === 0 && xmp && xmp.failed > 0 && (
        <span className="rounded-full bg-red-950 px-3 py-1 text-xs text-red-200" data-testid="xmp-status">
          {xmp.failed} XMP failed
        </span>
      )}
      <button onClick={p.onWriteXmp} disabled={!p.hasSelection} className={btn} data-testid="save-metadata" title={`Write XMP sidecars for the selection${hint("saveXmp")}`}>
        <CloudUpload className="size-4" />
        Save
      </button>
      <button onClick={p.onExport} disabled={!p.hasImages} data-testid="export-button" className={btn} title={`Export the selection or the filtered set${hint("export")}`}>
        {p.exportPct != null ? <Ring pct={p.exportPct} /> : <Share className="size-4" />}
        Export
      </button>
      <Menu trigger={<MoreHorizontal className="size-4" />} triggerClass={btn} triggerTestId="more-menu" title="More actions" align="right">
        {(close) => (
          <>
            <button
              className={menuItem}
              disabled={!p.hasImages}
              data-testid="apply-suggestions"
              title="Copy suggested rating and pick to the selection (or all photos when nothing is selected)"
              onClick={() => {
                close();
                p.onApplySuggestions();
              }}
            >
              <Check className="size-4" /> Apply suggestions…
            </button>
            <button
              className={menuItem}
              disabled={!p.hasSelection}
              data-testid="read-xmp"
              onClick={() => {
                close();
                p.onReadXmp();
              }}
            >
              <DownloadCloud className="size-4" /> Read metadata from file
            </button>
            <label className={`${menuItem} cursor-pointer`}>
              <input type="checkbox" data-testid="xmp-auto" checked={c?.xmpAutoSync ?? false} disabled={!c} onChange={(e) => p.onAutoXmp(e.target.checked)} />
              Auto-sync XMP
            </label>
            <button
              className={menuItem}
              data-testid="open-cheat-sheet"
              onClick={() => {
                close();
                p.onCheatSheet();
              }}
            >
              <Keyboard className="size-4" /> Keyboard shortcuts <span className="ml-auto text-xs text-neutral-400">?</span>
            </button>
            <button
              className={menuItem}
              data-testid="manage-models"
              onClick={() => {
                close();
                p.onModels();
              }}
            >
              <Cpu className="size-4" /> Manage AI models…
            </button>
          </>
        )}
      </Menu>
    </div>
  );
}
