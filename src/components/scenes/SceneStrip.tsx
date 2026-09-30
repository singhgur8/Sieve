// Scene strip (Library and Develop): one 32 px non-wrapping row, shown once scenes exist (or are being detected).
// Scene chips filter the grid; the "Scene" menu holds the editing actions; Match scene is the primary action.
import { Anchor, ChevronDown, Combine, Loader2, Scissors, ScanSearch, SquarePlus, Trash2, Unlink, Wand2 } from "lucide-react";
import { MAX_SCENE_ANCHORS, type Scene } from "../../ipc";
import type { ScenesApi } from "../../hooks/useScenes";
import { hint } from "../../lib/keymap";
import { Menu, menuItem } from "../Menu";

interface Props {
  api: ScenesApi;
  /** Scene currently used as grid filter (ImageQuery.sceneId). */
  filterId: number | null;
  onFilter: (id: number | null) => void;
  /** Photos the actions apply to (selection, or the active photo in loupe/develop). */
  targets: number[];
  activeId: number | null;
  onMatch: (scene: Scene) => void;
}

const btn = "flex h-6 items-center gap-1 whitespace-nowrap rounded bg-neutral-800 px-2 text-xs hover:bg-neutral-700 disabled:opacity-40 disabled:hover:bg-neutral-800";

export function SceneStrip({ api, filterId, onFilter, targets, activeId, onMatch }: Props) {
  const { scenes, progress, detecting } = api;
  if (scenes.length === 0 && !detecting) return null;
  const activeScene = api.sceneOfImage(activeId);
  const focus = scenes.find((s) => s.id === filterId) ?? activeScene;
  const isAnchor = activeId != null && !!activeScene?.anchorIds.includes(activeId);
  const detectPct = progress && progress.task === "detect" && progress.total > 0 ? Math.min(100, (progress.done / progress.total) * 100) : 0;
  const distinctScenes = new Set(targets.map((t) => api.sceneOfImage(t)?.id).filter((x) => x != null)).size;
  const canSplit = !!activeScene && activeId != null && activeScene.imageIds[0] !== activeId;

  return (
    <div className="flex h-8 shrink-0 items-center gap-2 border-b border-neutral-800 bg-neutral-950 px-3 text-xs text-neutral-300" data-testid="scene-strip">
      <span className="font-medium">Scenes</span>
      {detecting && (
        <>
          <Loader2 className="size-3.5 animate-spin" />
          <div className="h-1.5 w-32 shrink-0 overflow-hidden rounded bg-neutral-800" data-testid="scene-progress" data-pct={Math.round(detectPct)}>
            <div className="h-full bg-emerald-400 transition-[width]" style={{ width: `${detectPct}%` }} />
          </div>
        </>
      )}
      <div className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto overflow-y-hidden" data-testid="scene-chips">
        <button
          className={`whitespace-nowrap rounded px-2 py-0.5 ${filterId == null ? "bg-sky-800 text-sky-100" : "bg-neutral-800 hover:bg-neutral-700"}`}
          onClick={() => onFilter(null)}
          data-testid="scene-chip-all"
        >
          All photos
        </button>
        {scenes.map((s) => (
          <button
            key={s.id}
            onClick={() => onFilter(filterId === s.id ? null : s.id)}
            title={`${s.method === "manual" ? "Manual" : "Auto"} scene, ${s.imageIds.length} photos, ${s.anchorIds.length} anchor(s)`}
            data-testid={`scene-chip-${s.id}`}
            data-active={filterId === s.id}
            data-method={s.method}
            className={`flex shrink-0 items-center gap-1 whitespace-nowrap rounded px-2 py-0.5 ${filterId === s.id ? "bg-sky-800 text-sky-100" : "bg-neutral-800 hover:bg-neutral-700"}`}
          >
            Scene {api.number(s.id)} <span className="text-neutral-400">· {s.imageIds.length}</span>
            {s.anchorIds.length > 0 && (
              <span className="flex items-center text-amber-400" data-testid={`scene-anchors-${s.id}`}>
                <Anchor className="size-3" />
                {s.anchorIds.length}
              </span>
            )}
            {s.method === "manual" && <span className="text-[10px] uppercase text-neutral-400">manual</span>}
          </button>
        ))}
      </div>
      <Menu trigger={<>Scene <ChevronDown className="size-3.5" /></>} triggerClass={btn} triggerTestId="scene-menu" title="Scene actions" align="right">
        {(close) => {
          const item = (testid: string, icon: React.ReactNode, label: string, disabled: boolean, run: () => void, title?: string) => (
            <button
              className={menuItem}
              disabled={disabled}
              data-testid={testid}
              title={title}
              onClick={() => {
                close();
                run();
              }}
            >
              {icon} {label}
            </button>
          );
          return (
            <>
              {item("scenes-detect", <ScanSearch className="size-4" />, "Detect scenes again", detecting, () => void api.detect(), "Group the shoot into lighting scenes")}
              {item("scene-new", <SquarePlus className="size-4" />, "New scene from selection", targets.length === 0, () => void api.createFromImages(targets))}
              {item("scene-merge", <Combine className="size-4" />, "Merge scenes of selection", distinctScenes < 2, () => void api.mergeOf(targets))}
              {item("scene-split", <Scissors className="size-4" />, "Split here", !canSplit, () => void api.splitAt(activeId), "Start a new scene at the current photo")}
              {item("scene-remove", <Unlink className="size-4" />, "Remove selection from scene", !targets.some((t) => api.sceneOfImage(t)), () => void api.removeFromScene(targets))}
              {item("scene-delete", <Trash2 className="size-4" />, "Delete scene", !focus, () => focus && void api.deleteScene(focus.id), "Photos are kept")}
              <div className={isAnchor ? "bg-amber-950/60" : ""} data-on={isAnchor} data-testid="scene-anchor-row">
                {item(
                  "scene-anchor",
                  <Anchor className="size-4" />,
                  isAnchor ? "Unmark anchor" : "Mark as anchor",
                  !activeScene,
                  () => void api.toggleAnchor(activeId),
                  `Mark the current photo as a graded anchor${hint("anchor")}, max ${MAX_SCENE_ANCHORS} per scene`,
                )}
              </div>
            </>
          );
        }}
      </Menu>
      <button
        className="flex h-6 items-center gap-1 whitespace-nowrap rounded bg-emerald-800 px-2.5 text-xs font-medium text-emerald-100 hover:bg-emerald-700 disabled:bg-neutral-800 disabled:font-normal disabled:text-neutral-400"
        disabled={!focus || focus.anchorIds.length === 0 || focus.imageIds.length <= focus.anchorIds.length}
        onClick={() => focus && onMatch(focus)}
        data-testid="scene-match"
        data-anchor-on={isAnchor}
        title={focus && focus.anchorIds.length === 0 ? "Mark 1-2 graded anchors first (Shift+A)" : "Match the rest of the scene to its anchors"}
      >
        <Wand2 className="size-3.5" /> Match scene
      </button>
    </div>
  );
}
