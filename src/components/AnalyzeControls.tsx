// Shoot-type select and the Analyze split button. They sit in the TopBar without a project and in the Cull context bar inside one.
import { ChevronDown, Layers3, ScanSearch } from "lucide-react";
import type { CatalogState, ShootType } from "../ipc";
import type { AnalysisView } from "../hooks/useBackendStatus";
import { Menu, menuItem } from "./Menu";

const SHOOT_TYPES: ShootType[] = ["wedding", "portrait", "sports", "event", "landscape", "general"];
const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);
const btn = "flex h-7 items-center gap-1.5 whitespace-nowrap rounded-md bg-neutral-800 px-2.5 text-sm hover:bg-neutral-700 disabled:opacity-50";

export function ShootSelect({ catalog, onShootType }: { catalog: CatalogState | null; onShootType: (t: ShootType) => void }) {
  return (
    <label className="flex items-center gap-1 text-xs text-neutral-400">
      <span className="max-[1400px]:hidden">Shoot</span>
      <select
        value={catalog?.shootType ?? "general"}
        onChange={(e) => onShootType(e.target.value as ShootType)}
        disabled={!catalog}
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
  );
}

interface AnalyzeProps {
  catalog: CatalogState | null;
  analysis: AnalysisView | null;
  hasImages: boolean;
  detecting: boolean;
  onAnalyze: (kind: "pending" | "all") => void;
  onAutoAnalyze: (v: boolean) => void;
  onDetectScenes: () => void;
}

export function AnalyzeSplit({ catalog: c, analysis, hasImages, detecting, onAnalyze, onAutoAnalyze, onDetectScenes }: AnalyzeProps) {
  return (
    <div className="flex" data-testid="analyze-split">
      <button onClick={() => onAnalyze("pending")} disabled={analysis?.running} className={`${btn} rounded-r-none`} data-testid="analyze-button" title="Analyze photos that have not been analyzed yet">
        <ScanSearch className="size-4" />
        Analyze
      </button>
      <Menu trigger={<ChevronDown className="size-4" />} triggerClass={`${btn} rounded-l-none border-l border-neutral-700 px-1.5`} triggerTestId="analyze-menu" title="More analysis options">
        {(close) => (
          <>
            <button
              className={menuItem}
              disabled={analysis?.running}
              data-testid="reanalyze-all"
              onClick={() => {
                close();
                onAnalyze("all");
              }}
            >
              Re-analyze all photos
            </button>
            <button
              className={menuItem}
              disabled={detecting || !hasImages}
              data-testid="scenes-detect"
              onClick={() => {
                close();
                onDetectScenes();
              }}
            >
              <Layers3 className="size-4" /> Detect scenes
            </button>
            <label className={`${menuItem} cursor-pointer`}>
              <input type="checkbox" data-testid="auto-analyze" checked={c?.autoAnalyze ?? false} disabled={!c} onChange={(e) => onAutoAnalyze(e.target.checked)} />
              Auto-analyze new photos
            </label>
          </>
        )}
      </Menu>
    </div>
  );
}
