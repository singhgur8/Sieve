// Batch editing over the Grid selection (also a scene's photos while the scene filter is on): select the scene, copy the active
// photo's settings, paste them to every selected photo, sync, or edit the whole selection in Develop. Every disabled button says why.
import { ClipboardCopy, ClipboardPaste, MousePointerSquareDashed, RefreshCw, SlidersHorizontal } from "lucide-react";
import { hint } from "../../lib/keymap";
import { useClipboard } from "../../lib/clipboard";

interface Props {
  selected: number;
  /** Photos shown by the current scene filter (null = no scene filter). */
  sceneCount: number | null;
  hasActive: boolean;
  onSelectAll: () => void;
  onCopy: () => void;
  onPaste: () => void;
  onSync: () => void;
  onEditAll: () => void;
}

const btn = "flex h-6 items-center gap-1 whitespace-nowrap rounded bg-neutral-800 px-2 text-xs hover:bg-neutral-700 disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-neutral-800";

export function SelectionBar({ selected, sceneCount, hasActive, onSelectAll, onCopy, onPaste, onSync, onEditAll }: Props) {
  const clip = useClipboard();
  const why = (testid: string, text: string | null) =>
    text ? (
      <span className="whitespace-nowrap text-[11px] text-amber-300" data-testid={testid}>
        {text}
      </span>
    ) : null;
  const pasteWhy = !clip ? "Copy settings first (Cmd+C)" : selected === 0 ? "Select photos to paste to" : null;
  const syncWhy = !hasActive ? "Select a source photo" : selected < 2 ? "Shift / Cmd-click more photos" : null;
  const editWhy = selected === 0 && sceneCount == null ? "Select photos first" : null;
  return (
    <div className="flex h-8 shrink-0 items-center gap-2 overflow-x-auto border-b border-neutral-800 bg-neutral-950 px-3 text-xs text-neutral-300" data-testid="selection-bar">
      <span className="font-medium" data-testid="selbar-count">
        {selected} selected
      </span>
      {sceneCount != null && (
        <button className={btn} onClick={onSelectAll} data-testid="sel-select-all" title={`Select every photo of the scene${hint("selectAll")}`}>
          <MousePointerSquareDashed className="size-3.5" /> Select scene ({sceneCount})
        </button>
      )}
      <button className={btn} onClick={onCopy} disabled={!hasActive} data-testid="sel-copy" title={`Copy every setting of the active photo${hint("copyAll")}`}>
        <ClipboardCopy className="size-3.5" /> Copy settings
      </button>
      <button className={btn} onClick={onPaste} disabled={!!pasteWhy} data-testid="sel-paste" title={`Paste the copied settings to every selected photo (one Undo)${hint("pasteAll")}`}>
        <ClipboardPaste className="size-3.5" /> Paste{selected > 0 ? ` to ${selected}` : ""}
      </button>
      {why("sel-paste-why", pasteWhy)}
      <button className={btn} onClick={onSync} disabled={!!syncWhy} data-testid="sel-sync" title="Copy the active photo's settings to the other selected photos (one Undo)">
        <RefreshCw className="size-3.5" /> Sync from active
      </button>
      {why("sel-sync-why", syncWhy)}
      <button className={btn} onClick={onEditAll} disabled={!!editWhy} data-testid="sel-edit-all" title="Open Develop with the whole selection (the whole scene when nothing is selected). Cmd+Alt+S syncs, reset and presets apply to all">
        <SlidersHorizontal className="size-3.5" /> {sceneCount != null && selected < 2 ? "Edit all in scene" : `Edit ${selected} selected`}
      </button>
      {why("sel-edit-why", editWhy)}
    </div>
  );
}
