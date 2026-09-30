// Lightroom-style "which settings" dialog shared by Copy Settings, Synchronize Settings, New Develop Preset and
// Match's "Copy from anchor" (docs/ux-spec-8b.md 5.7): three columns, tri-state parents, remembered choice.
import { useEffect, useRef, useState } from "react";
import type { AdjustmentField } from "../../ipc";
import { ALL_ADJUSTMENT_FIELDS, DEFAULT_SYNC_FIELDS } from "../../ipc";
import { DISPLAYED_FIELDS, FIELD_COLUMNS, loadFields, saveFields, type FieldGroup } from "../../lib/fieldGroups";
import { Dialog } from "../Dialog";

interface Props {
  title: string;
  confirm: string;
  /** Preset creation asks for a name too (and starts from "Check Modified"). */
  withName?: boolean;
  initialName?: string;
  /** Explicit starting selection (Match's anchor fields); nothing is remembered for it. */
  initial?: AdjustmentField[];
  /** localStorage key of the remembered choice (Copy / Sync share one, presets another). */
  storageKey?: string;
  /** Fields that differ from the photo's defaults: "Check Modified" and the first-time preset selection. */
  modified?: AdjustmentField[];
  /** The photo has a legacy LUT reference: the LUT row is offered. */
  hasLut?: boolean;
  /** The photo has masks: otherwise Masking is disabled with "(none)". */
  hasMasks?: boolean;
  onConfirm: (fields: AdjustmentField[], name: string) => void;
  onCancel: () => void;
}

export function SettingsFieldsDialog({ title, confirm, withName, initialName = "", initial, storageKey, modified, hasLut = true, hasMasks = true, onConfirm, onCancel }: Props) {
  const [fields, setFields] = useState<Set<AdjustmentField>>(() => {
    if (initial) return new Set(initial);
    const saved = storageKey ? loadFields(storageKey) : null;
    if (saved) return new Set(saved);
    return new Set(withName ? (modified ?? DEFAULT_SYNC_FIELDS) : DEFAULT_SYNC_FIELDS);
  });
  const [name, setName] = useState(initialName);
  const set = (list: AdjustmentField[], on: boolean) =>
    setFields((s) => {
      const n = new Set(s);
      for (const f of list) {
        if (on) n.add(f);
        else n.delete(f);
      }
      return n;
    });
  const disabledField = (f: AdjustmentField) => f === "masks" && !hasMasks;
  const ordered = ALL_ADJUSTMENT_FIELDS.filter((f) => fields.has(f) && !disabledField(f));
  const valid = ordered.length > 0 && (!withName || name.trim().length > 0);
  const go = () => {
    if (storageKey) saveFields(storageKey, ordered);
    onConfirm(ordered, name.trim());
  };

  return (
    <Dialog
      label={title}
      testid="fields-dialog"
      overlayClass="z-[60] bg-black/60"
      className="w-[680px] max-w-full rounded-lg border border-neutral-700 bg-neutral-900 shadow-xl"
      onCancel={onCancel}
      onConfirm={go}
      canConfirm={() => valid}
    >
      <div className="px-5 pt-4">
        <h2 className="mb-3 text-sm font-semibold">{title}</h2>
        {withName && (
          <label className="mb-3 flex items-center gap-2 text-xs">
            <span className="w-24 shrink-0 text-neutral-300">Preset Name</span>
            <input
              data-autofocus
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="Preset name"
              data-testid="preset-name"
              className="min-w-0 flex-1 rounded bg-neutral-800 px-2 py-1 text-sm outline-none focus:ring-1 focus:ring-sky-500"
            />
          </label>
        )}
        <div className="grid grid-cols-3 gap-x-5 text-xs">
          {FIELD_COLUMNS.map((col, ci) => (
            <div key={ci} className="flex flex-col">
              {col.map((n) =>
                n.kind === "leaf" ? (
                  (n.field !== "lut" || hasLut) && (
                    <Row
                      key={n.field}
                      field={n.field}
                      label={n.label}
                      checked={fields.has(n.field) && !disabledField(n.field)}
                      disabled={disabledField(n.field)}
                      note={n.field === "masks" && !hasMasks ? "(none)" : undefined}
                      onChange={(on) => set([n.field], on)}
                    />
                  )
                ) : (
                  <Group key={n.id} g={n} fields={fields} onSet={set} />
                ),
              )}
            </div>
          ))}
        </div>
      </div>
      <div className="mt-3 flex h-[52px] items-center justify-between border-t border-neutral-800 px-5 text-xs">
        <div className="flex gap-3 text-sky-400">
          <button data-testid="fields-all" onClick={() => setFields(new Set<AdjustmentField>([...DISPLAYED_FIELDS.filter((f) => !disabledField(f) && (f !== "lut" || hasLut)), "process_version"]))}>
            Check All
          </button>
          <button data-testid="fields-none" onClick={() => setFields(new Set())}>
            Check None
          </button>
          <button data-testid="fields-modified" onClick={() => setFields(new Set(modified ?? []))} title="Only the settings that differ from this photo's defaults">
            Check Modified
          </button>
        </div>
        <div className="flex gap-2">
          <button className="rounded bg-neutral-800 px-3 py-1 hover:bg-neutral-700" onClick={onCancel} data-testid="fields-cancel">
            Cancel
          </button>
          <button
            data-autofocus={withName ? undefined : true}
            disabled={!valid}
            className="rounded bg-sky-700 px-3 py-1 text-white hover:bg-sky-600 disabled:opacity-40"
            onClick={go}
            data-testid="fields-confirm"
          >
            {confirm}
          </button>
        </div>
      </div>
    </Dialog>
  );
}

function Row({ field, label, checked, disabled, note, onChange, indent }: { field: AdjustmentField; label: string; checked: boolean; disabled?: boolean; note?: string; onChange: (on: boolean) => void; indent?: boolean }) {
  return (
    <label className={`flex h-[22px] items-center gap-1.5 ${indent ? "pl-5" : ""} ${disabled ? "text-neutral-500" : ""}`}>
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} data-testid={`field-${field}`} />
      <span className="truncate">{label}</span>
      {note && <span className="text-neutral-500">{note}</span>}
    </label>
  );
}

function Group({ g, fields, onSet }: { g: FieldGroup; fields: Set<AdjustmentField>; onSet: (list: AdjustmentField[], on: boolean) => void }) {
  const ref = useRef<HTMLInputElement>(null);
  const list = g.children.map((c) => c.field);
  const n = list.filter((f) => fields.has(f)).length;
  const all = n === list.length;
  useEffect(() => {
    if (ref.current) ref.current.indeterminate = n > 0 && !all;
  }, [n, all]);
  return (
    <>
      <label className="flex h-[22px] items-center gap-1.5 font-semibold">
        <input ref={ref} type="checkbox" checked={all} onChange={() => onSet(list, !all)} data-testid={`field-group-${g.id}`} data-state={all ? "checked" : n > 0 ? "mixed" : "unchecked"} />
        <span className="truncate">{g.label}</span>
      </label>
      {g.children.map((c) => (
        <Row key={c.field} indent field={c.field} label={c.label} checked={fields.has(c.field)} onChange={(on) => onSet([c.field], on)} />
      ))}
    </>
  );
}
