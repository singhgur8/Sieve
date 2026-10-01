// One 44 px application bar: import / shoot / analyze on the left, module switcher in the middle,
// XMP state, save and export on the right. Rarely used actions live in the Analyze and "more" menus.
import { Aperture, Check, ChevronDown, CloudUpload, Columns2, Cpu, DownloadCloud, FolderOpen, FolderSearch, Grid3x3, Keyboard, LayoutList, Maximize, MoreHorizontal, RefreshCw, Share, SlidersHorizontal, Layers3 } from "lucide-react";
import type { CatalogState, ImportOptions, Project, ShootType, XmpStatus } from "../ipc";
import type { AnalysisView } from "../hooks/useBackendStatus";
import { hint, type Mode } from "../lib/keymap";
import { Menu, menuItem } from "./Menu";
import { XmpStatusPill, type XmpFailureRow } from "./XmpStatus";
import { ProjectSwitcher } from "./ProjectSwitcher";
import { AnalyzeSplit, ShootSelect } from "./AnalyzeControls";
import type { ReactNode } from "react";

const btn = "flex h-7 items-center gap-1.5 whitespace-nowrap rounded-md bg-neutral-800 px-2.5 text-sm hover:bg-neutral-700 disabled:opacity-50";
const labelHide = "max-[1439px]:hidden";
const seg = (on: boolean) => `flex h-7 items-center gap-1 whitespace-nowrap rounded px-2 text-xs ${on ? "bg-sky-800 text-sky-100" : "bg-neutral-800 hover:bg-neutral-700"}`;

interface Props {
  catalog: CatalogState | null;
  /** Open project (null = all photos, dev mock only). */
  project?: Project | null;
  onHome?: () => void;
  onOpenProject?: (id: number) => Promise<void>;
  onSetCover?: (() => void) | null;
  analysis: AnalysisView | null;
  xmp: XmpStatus | null;
  xmpFailures: XmpFailureRow[];
  onOpenXmpErrors: () => void;
  onShowXmpFailure?: (imageId: number) => void;
  onXmpExplain: () => void;
  busy: boolean;
  mode: Mode | "plan";
  onMode: (m: Mode) => void;
  /** Compare view is open (also inside Develop): highlights the Compare button. */
  compareOn?: boolean;
  /** Scene strip toggle: shown once scenes exist (or are being detected). */
  scenesCount: number;
  scenesOpen: boolean;
  onToggleScenes: () => void;
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
  onLocate: () => void;
  onRegenerate: () => void;
  /** 0-100 while export jobs run, otherwise null. */
  exportPct: number | null;
  /** Workflow step bar (project open): replaces Shoot / Analyze / Export, which move to the Cull bar and step 3. */
  steps?: ReactNode;
  /** Plan view segment (Edit step). */
  plan?: { on: boolean; onPlan: () => void };
  /** Project without photos (no Cull bar to hold them): keep Shoot type and Analyze in the TopBar. */
  legacyControls?: boolean;
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
  const modes: { m: Mode; label: string; icon: typeof Grid3x3; title: string; testid: string }[] = [
    { m: "grid", label: "Grid", icon: Grid3x3, title: `Grid${hint("toGrid")}`, testid: "mode-grid" },
    { m: "loupe", label: "Loupe", icon: Maximize, title: `Loupe${hint("toggleLoupe")}`, testid: "mode-loupe" },
    { m: "compare", label: "Compare", icon: Columns2, title: `Compare${hint("compare")}`, testid: "mode-compare" },
    { m: "develop", label: "Develop", icon: SlidersHorizontal, title: `Develop${hint("develop")}`, testid: "mode-develop" },
  ];
  return (
    <div className="flex h-11 shrink-0 items-center gap-2 border-b border-neutral-800 px-3" data-testid="top-bar">
      <button onClick={p.onHome} disabled={!p.onHome} className="flex items-center gap-2 rounded-md hover:opacity-80 disabled:hover:opacity-100" data-testid="home-button" title="All projects">
        <Aperture className="size-5 shrink-0 text-amber-400" />
        <h1 className={`font-semibold tracking-tight ${p.steps ? "max-[1439px]:hidden" : ""}`}>Sieve</h1>
      </button>
      {p.project && p.onHome && p.onOpenProject && <ProjectSwitcher project={p.project} onHome={p.onHome} onOpenProject={p.onOpenProject} onSetCover={p.onSetCover ?? null} />}
      <div className="flex" data-testid="import-split">
        <button onClick={p.onImport} disabled={p.busy} className={`${btn} rounded-r-none`} data-testid="import-button" title={`${p.project ? "Add a folder to this project" : "Import a shoot folder"}${hint("import")}`}>
          <FolderOpen className="size-4" />
          <span className={p.steps ? "max-[1439px]:hidden" : ""}>{p.busy ? "Importing…" : "Import"}</span>
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
      {(!p.steps || p.legacyControls) && (
        <>
          <ShootSelect catalog={c} onShootType={p.onShootType} />
          <AnalyzeSplit catalog={c} analysis={p.analysis} hasImages={p.hasImages} detecting={p.detecting} onAnalyze={p.onAnalyze} onAutoAnalyze={p.onAutoAnalyze} onDetectScenes={p.onDetectScenes} />
        </>
      )}
      {p.steps && <div className="mx-auto">{p.steps}</div>}

      <div className={`${p.steps ? "" : "mx-auto"} flex gap-1`} data-testid="mode-switcher">
        {p.plan && (
          <button className={seg(p.plan.on)} aria-label="Plan" aria-pressed={p.plan.on} onClick={p.plan.onPlan} title={`Plan: the scene checklist${hint("toGrid")}`} data-testid="mode-plan">
            <LayoutList className="size-3.5" /> <span className={p.steps && !p.plan.on ? labelHide : ""}>Plan</span>
          </button>
        )}
        {modes.map(({ m, label, icon: Icon, title, testid }) => (
          <button key={m} className={seg(p.mode === m || (m === "compare" && !!p.compareOn))} aria-label={label} aria-pressed={m === "compare" ? !!p.compareOn || p.mode === m : p.mode === m} onClick={() => p.onMode(m)} title={title} data-testid={testid}>
            <Icon className="size-3.5" /> <span className={p.steps && !(p.mode === m && !(p.plan?.on)) ? labelHide : ""}>{label}</span>
          </button>
        ))}
      </div>

      {(p.scenesCount > 0 || p.detecting) && (
        <button className={seg(p.scenesOpen)} onClick={p.onToggleScenes} aria-pressed={p.scenesOpen} title={`${p.scenesOpen ? "Hide" : "Show"} the scene strip (hiding clears the scene filter)${hint("scenesToggle")}`} data-testid="scenes-toggle">
          <Layers3 className="size-3.5" /> Scenes
        </button>
      )}

      <XmpStatusPill xmp={xmp} failures={p.xmpFailures} onOpenErrors={p.onOpenXmpErrors} onShow={p.onShowXmpFailure} onRetry={p.onSaveAllDirty} onSaveAll={p.onSaveAllDirty} onAutoSync={p.onAutoXmp} onExplain={p.onXmpExplain} />
      <button onClick={p.onWriteXmp} disabled={!p.hasSelection} className={btn} data-testid="save-metadata" title={`Write XMP sidecars for the selection${hint("saveXmp")}`}>
        <CloudUpload className="size-4" />
        Save
      </button>
      {!p.steps && <button onClick={p.onExport} disabled={!p.hasImages} data-testid="export-button" className={btn} title={`Export the selection or the filtered set${hint("export")}`}>
        {p.exportPct != null ? <Ring pct={p.exportPct} /> : <Share className="size-4" />}
        Export
      </button>}
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
              disabled={!p.hasSelection}
              data-testid="regenerate-previews"
              title="Extract the thumbnails and previews of the selection again (when they look broken or are missing)"
              onClick={() => {
                close();
                p.onRegenerate();
              }}
            >
              <RefreshCw className="size-4" /> Regenerate previews
            </button>
            <button
              className={menuItem}
              data-testid="locate-folder-menu"
              disabled={!p.catalog || p.catalog.folders.length === 0}
              title="Point a folder at where its photos were moved"
              onClick={() => {
                close();
                p.onLocate();
              }}
            >
              <FolderSearch className="size-4" /> Locate folder…
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
