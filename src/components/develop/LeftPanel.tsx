import { Redo2, Save, Trash2, Undo2 } from "lucide-react";
import type { AdjustmentHistory, Preset } from "../../ipc";

interface Props {
  presets: Preset[];
  history: AdjustmentHistory | null;
  onApplyPreset: (p: Preset) => void;
  onSavePreset: () => void;
  onDeletePreset: (p: Preset) => void;
  onUndo: () => void;
  onRedo: () => void;
  onGoto: (entryId: number) => void;
}

export function LeftPanel({ presets, history, onApplyPreset, onSavePreset, onDeletePreset, onUndo, onRedo, onGoto }: Props) {
  const entries = history ? [...history.entries].reverse() : [];
  return (
    <div className="flex h-full flex-col overflow-y-auto text-xs" data-testid="left-panel">
      <section className="border-b border-neutral-800 px-3 py-2">
        <div className="mb-1 flex items-center justify-between">
          <h3 className="font-semibold uppercase tracking-wide text-neutral-300">Presets</h3>
          <button className="flex items-center gap-1 text-sky-400 hover:text-sky-300" onClick={onSavePreset} data-testid="preset-save" title="Save current settings as a preset">
            <Save className="size-3.5" /> Save
          </button>
        </div>
        {presets.length === 0 && <p className="text-neutral-600">No presets yet</p>}
        <ul data-testid="preset-list">
          {presets.map((p) => (
            <li key={p.id} className="group flex items-center justify-between rounded px-1.5 py-1 hover:bg-neutral-800">
              <button className="min-w-0 flex-1 truncate text-left" onClick={() => onApplyPreset(p)} data-testid={`preset-${p.id}`} title={`Apply ${p.name}`}>
                {p.name}
              </button>
              <button className="invisible text-neutral-500 hover:text-red-400 group-hover:visible" onClick={() => onDeletePreset(p)} data-testid={`preset-delete-${p.id}`} title="Delete preset">
                <Trash2 className="size-3.5" />
              </button>
            </li>
          ))}
        </ul>
      </section>
      <section className="px-3 py-2">
        <div className="mb-1 flex items-center justify-between">
          <h3 className="font-semibold uppercase tracking-wide text-neutral-300">History</h3>
          <div className="flex gap-2">
            <button disabled={!history?.canUndo} onClick={onUndo} title="Undo (Cmd+Z)" data-testid="undo" className="text-neutral-400 hover:text-white disabled:opacity-30">
              <Undo2 className="size-4" />
            </button>
            <button disabled={!history?.canRedo} onClick={onRedo} title="Redo (Shift+Cmd+Z)" data-testid="redo" className="text-neutral-400 hover:text-white disabled:opacity-30">
              <Redo2 className="size-4" />
            </button>
          </div>
        </div>
        {entries.length === 0 && <p className="text-neutral-600">No edits yet</p>}
        <ul data-testid="history-list">
          {entries.map((e) => (
            <li key={e.id}>
              <button
                className={`w-full truncate rounded px-1.5 py-1 text-left ${history?.currentEntryId === e.id ? "bg-sky-900/60 text-sky-100" : "text-neutral-400 hover:bg-neutral-800"}`}
                data-testid={`history-${e.id}`}
                data-current={history?.currentEntryId === e.id}
                onClick={() => onGoto(e.id)}
              >
                {e.label}
              </button>
            </li>
          ))}
        </ul>
      </section>
    </div>
  );
}
