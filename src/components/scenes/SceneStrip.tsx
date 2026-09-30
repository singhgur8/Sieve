// Scene strip (Library and Develop): scene chips that filter the grid, Detect scenes, scene editing,
// anchor toggle and the entry point to the Match panel.
import { Anchor, Combine, Loader2, Scissors, ScanSearch, SquarePlus, Trash2, Unlink, Wand2 } from "lucide-react";
import { MAX_SCENE_ANCHORS, type Scene } from "../../ipc";
import type { ScenesApi } from "../../hooks/useScenes";

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

const btn = "flex items-center gap-1 rounded bg-neutral-800 px-2 py-1 text-xs hover:bg-neutral-700 disabled:opacity-40 disabled:hover:bg-neutral-800";

export function SceneStrip({ api, filterId, onFilter, targets, activeId, onMatch }: Props) {
  const { scenes, progress, detecting } = api;
  const activeScene = api.sceneOfImage(activeId);
  const focus = scenes.find((s) => s.id === filterId) ?? activeScene;
  const isAnchor = activeId != null && !!activeScene?.anchorIds.includes(activeId);
  const detectPct = progress && progress.task === "detect" && progress.total > 0 ? Math.min(100, (progress.done / progress.total) * 100) : 0;
  const distinctScenes = new Set(targets.map((t) => api.sceneOfImage(t)?.id).filter((x) => x != null)).size;
  const canSplit = !!activeScene && activeId != null && activeScene.imageIds[0] !== activeId;

  return (
    <div className="flex flex-wrap items-center gap-2 border-b border-neutral-800 bg-neutral-950 px-3 py-1.5 text-xs text-neutral-400" data-testid="scene-strip">
      <span className="font-medium text-neutral-300">Scenes</span>
      <button className={btn} disabled={detecting} onClick={() => void api.detect()} data-testid="scenes-detect" title="Group the shoot into lighting scenes">
        {detecting ? <Loader2 className="size-3.5 animate-spin" /> : <ScanSearch className="size-3.5" />} Detect scenes
      </button>
      {detecting && (
        <div className="h-1.5 w-32 overflow-hidden rounded bg-neutral-800" data-testid="scene-progress" data-pct={Math.round(detectPct)}>
          <div className="h-full bg-emerald-400 transition-[width]" style={{ width: `${detectPct}%` }} />
        </div>
      )}
      <div className="flex max-w-full items-center gap-1 overflow-x-auto" data-testid="scene-chips">
        <button
          className={`whitespace-nowrap rounded px-2 py-1 ${filterId == null ? "bg-sky-800 text-sky-100" : "bg-neutral-800 hover:bg-neutral-700"}`}
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
            className={`flex items-center gap-1 whitespace-nowrap rounded px-2 py-1 ${filterId === s.id ? "bg-sky-800 text-sky-100" : "bg-neutral-800 hover:bg-neutral-700"}`}
          >
            Scene {api.number(s.id)} <span className="text-neutral-500">· {s.imageIds.length}</span>
            {s.anchorIds.length > 0 && (
              <span className="flex items-center text-amber-400" data-testid={`scene-anchors-${s.id}`}>
                <Anchor className="size-3" />
                {s.anchorIds.length}
              </span>
            )}
            {s.method === "manual" && <span className="text-[9px] uppercase text-neutral-500">manual</span>}
          </button>
        ))}
        {scenes.length === 0 && !detecting && <span className="px-1 text-neutral-600">No scenes yet</span>}
      </div>
      <div className="ml-auto flex flex-wrap items-center gap-1">
        <button className={btn} disabled={targets.length === 0} onClick={() => void api.createFromImages(targets)} data-testid="scene-new" title="New scene from the selected photos">
          <SquarePlus className="size-3.5" /> New scene
        </button>
        <button className={btn} disabled={distinctScenes < 2} onClick={() => void api.mergeOf(targets)} data-testid="scene-merge" title="Merge the scenes of the selected photos">
          <Combine className="size-3.5" /> Merge
        </button>
        <button className={btn} disabled={!canSplit} onClick={() => void api.splitAt(activeId)} data-testid="scene-split" title="Start a new scene at the current photo">
          <Scissors className="size-3.5" /> Split here
        </button>
        <button className={btn} disabled={!targets.some((t) => api.sceneOfImage(t))} onClick={() => void api.removeFromScene(targets)} data-testid="scene-remove" title="Remove the selected photos from their scene">
          <Unlink className="size-3.5" /> Remove
        </button>
        <button className={btn} disabled={!focus} onClick={() => focus && void api.deleteScene(focus.id)} data-testid="scene-delete" title="Delete the current scene (photos are kept)">
          <Trash2 className="size-3.5" /> Delete
        </button>
        <button
          className={`${btn} ${isAnchor ? "!bg-amber-800 text-amber-100" : ""}`}
          disabled={!activeScene}
          onClick={() => void api.toggleAnchor(activeId)}
          data-testid="scene-anchor"
          data-on={isAnchor}
          title={`Mark the current photo as a graded anchor (Shift+A, max ${MAX_SCENE_ANCHORS} per scene)`}
        >
          <Anchor className="size-3.5" /> {isAnchor ? "Anchor" : "Mark as anchor"}
        </button>
        <button
          className={`${btn} !bg-emerald-800 text-emerald-100 hover:!bg-emerald-700 disabled:!bg-neutral-800`}
          disabled={!focus || focus.anchorIds.length === 0 || focus.imageIds.length <= focus.anchorIds.length}
          onClick={() => focus && onMatch(focus)}
          data-testid="scene-match"
          title={focus && focus.anchorIds.length === 0 ? "Mark 1-2 graded anchors first" : "Match the rest of the scene to its anchors"}
        >
          <Wand2 className="size-3.5" /> Match scene
        </button>
      </div>
    </div>
  );
}
