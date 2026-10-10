// Transform section of the adjust panel (Lightroom Classic): Upright buttons, Guided tool, manual sliders, Constrain Crop.
import { Loader2 } from "lucide-react";
import type { CompleteAdjustments, TransformSettings } from "../../ipc";
import type { Editor } from "../../hooks/useEditor";
import type { UprightApi } from "../../hooks/useUpright";
import { hint } from "../../lib/keymap";
import { UPRIGHT_BUTTONS, uprightLabel } from "../../lib/transform";
import { NumField, seg } from "./fields";

export interface GuidedApi {
  active: boolean;
  toggle: () => void;
}

type Num = Exclude<keyof TransformSettings, "upright" | "guides" | "constrainCrop" | "solution">;
const set = (k: Num) => (a: CompleteAdjustments, v: number): CompleteAdjustments => ({ ...a, transform: { ...a.transform, [k]: v } });
const get = (k: Num) => (a: CompleteAdjustments) => a.transform[k] as number;

export function TransformPanel({ editor, upright, guided }: { editor: Editor; upright: UprightApi; guided: GuidedApi }) {
  const t = editor.adj.transform;
  const S = (id: string, label: string, k: Num, min: number, max: number, step = 1, digits = 0) => (
    <NumField editor={editor} id={`tf-${id}`} label={label} group="Transform" min={min} max={max} step={step} digits={digits} get={get(k)} set={set(k)} />
  );
  return (
    <div data-testid="transform-panel" data-upright={t.upright}>
      <div className="mb-1 flex items-center gap-1.5 text-[11px] text-neutral-400">
        Upright {upright.busy && <Loader2 className="size-3 animate-spin" data-testid="upright-busy" />}
      </div>
      <div className="mb-1 grid grid-cols-3 gap-1" data-testid="upright-buttons">
        {UPRIGHT_BUTTONS.map((b) => {
          const on = guided.active ? b.mode === "guided" : b.mode === t.upright; // while the Guided tool is armed only it is pressed
          return (
            <button
              key={b.mode}
              className={seg(on)}
              aria-pressed={on}
              title={b.tip + (b.mode === "guided" ? hint("guided") : "")}
              data-testid={`upright-${b.mode}`}
              onClick={() => (b.mode === "guided" ? guided.toggle() : upright.run(b.mode))}
            >
              {b.label}
            </button>
          );
        })}
      </div>
      {guided.active && (
        <p className="mb-1 text-[11px] text-sky-300" data-testid="upright-guided-help">
          Guided: drag on the photo to draw up to 4 lines along things that should be straight. Drag an end to adjust, x deletes, Esc exits.
          {t.upright !== "off" && t.upright !== "guided" && (
            <span className="block text-neutral-400" data-testid="upright-guided-current">
              Current: {uprightLabel(t.upright)}, kept until 2 guides are drawn
            </span>
          )}
        </p>
      )}
      {upright.message && (
        <p className="mb-1 rounded bg-amber-900/40 px-1.5 py-1 text-[11px] text-amber-200" role="status" data-testid="upright-message">
          {upright.message}
        </p>
      )}
      <label className="mb-1 flex items-center gap-1.5 text-xs text-neutral-300" title="Keep the crop inside the corrected image (otherwise the gaps show white)">
        <input
          type="checkbox"
          checked={t.constrainCrop}
          onChange={(e) => editor.change((a) => ({ ...a, transform: { ...a.transform, constrainCrop: e.target.checked } }), "Transform: Constrain Crop")}
          data-testid="tf-constrain"
        />
        Constrain Crop
      </label>
      {S("vertical", "Vertical", "vertical", -100, 100)}
      {S("horizontal", "Horizontal", "horizontal", -100, 100)}
      {S("rotate", "Rotate", "rotate", -10, 10, 0.1, 1)}
      {S("aspect", "Aspect", "aspect", -100, 100)}
      {S("scale", "Scale", "scale", 50, 150)}
      {S("offset-x", "Offset X", "offsetX", -100, 100)}
      {S("offset-y", "Offset Y", "offsetY", -100, 100)}
    </div>
  );
}
