import { useState } from "react";
import type { AdjustmentField } from "../../ipc";
import { ALL_ADJUSTMENT_FIELDS, DEFAULT_SYNC_FIELDS } from "../../ipc";
import { FIELD_LABEL } from "../../lib/adjust";
import { Dialog } from "../Dialog";

interface Props {
  title: string;
  confirm: string;
  /** Preset save asks for a name too. */
  withName?: boolean;
  initialName?: string;
  initial?: AdjustmentField[];
  /** Fields that differ from neutral are pre-ticked when no `initial` is given; otherwise everything. */
  onConfirm: (fields: AdjustmentField[], name: string) => void;
  onCancel: () => void;
}

/** Lightroom-style "which settings" checklist (fields mask) used by copy, sync and save-preset. */
export function FieldsDialog({ title, confirm, withName, initialName = "", initial, onConfirm, onCancel }: Props) {
  const [fields, setFields] = useState<Set<AdjustmentField>>(new Set(initial ?? DEFAULT_SYNC_FIELDS));
  const [name, setName] = useState(initialName);
  const toggle = (f: AdjustmentField) =>
    setFields((s) => {
      const n = new Set(s);
      if (n.has(f)) n.delete(f);
      else n.add(f);
      return n;
    });
  const ordered = ALL_ADJUSTMENT_FIELDS.filter((f) => fields.has(f));
  const valid = ordered.length > 0 && (!withName || name.trim().length > 0);
  return (
    <Dialog
      label={title}
      testid="fields-dialog"
      overlayClass="z-[60] bg-black/60"
      className="w-96 rounded-lg border border-neutral-700 bg-neutral-900 p-4 shadow-xl"
      onCancel={onCancel}
      onConfirm={() => onConfirm(ordered, name.trim())}
      canConfirm={() => valid}
    >
      <div>
        <h2 className="mb-3 text-sm font-semibold">{title}</h2>
        {withName && (
          <input
            data-autofocus
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Preset name"
            data-testid="preset-name"
            className="mb-3 w-full rounded bg-neutral-800 px-2 py-1 text-sm outline-none focus:ring-1 focus:ring-sky-500"
          />
        )}
        <div className="mb-2 flex gap-3 text-xs text-sky-400">
          <button data-testid="fields-all" onClick={() => setFields(new Set(ALL_ADJUSTMENT_FIELDS))}>
            Check all
          </button>
          <button data-testid="fields-none" onClick={() => setFields(new Set())}>
            Check none
          </button>
        </div>
        <div className="grid grid-cols-2 gap-x-4 gap-y-1 text-xs">
          {ALL_ADJUSTMENT_FIELDS.map((f) => (
            <label key={f} className="flex items-center gap-1.5">
              <input type="checkbox" data-autofocus={!withName && f === ALL_ADJUSTMENT_FIELDS[0] ? true : undefined} checked={fields.has(f)} onChange={() => toggle(f)} data-testid={`field-${f}`} />
              {FIELD_LABEL[f]}
            </label>
          ))}
        </div>
        <div className="mt-4 flex justify-end gap-2 text-xs">
          <button className="rounded bg-neutral-800 px-3 py-1 hover:bg-neutral-700" onClick={onCancel} data-testid="fields-cancel">
            Cancel
          </button>
          <button
            disabled={!valid}
            className="rounded bg-sky-700 px-3 py-1 text-white hover:bg-sky-600 disabled:opacity-40"
            onClick={() => onConfirm(ordered, name.trim())}
            data-testid="fields-confirm"
          >
            {confirm}
          </button>
        </div>
      </div>
    </Dialog>
  );
}
