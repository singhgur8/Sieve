// Crop section of the adjust panel: start/finish the crop tool, aspect presets, straighten angle.
import { Crop as CropIcon, RectangleVertical } from "lucide-react";
import type { Editor } from "../../hooks/useEditor";
import { hint } from "../../lib/keymap";
import { ASPECTS, FULL, fitRatio, type AspectId } from "../../lib/crop";
import { lockRatio, type CropTool } from "./CropOverlay";
import { Slider } from "./Slider";

export interface CropApi {
  tool: CropTool | null;
  imageAspect: number;
  start: () => void;
  change: (t: CropTool) => void;
  commit: () => void;
  cancel: () => void;
}

const b = "rounded bg-neutral-800 px-2 py-1 text-xs hover:bg-neutral-700 disabled:opacity-40";

export function CropPanel({ editor, crop }: { editor: Editor; crop: CropApi }) {
  const { tool } = crop;
  const c = editor.adj.crop;
  if (!tool) {
    return (
      <div data-testid="crop-panel" data-active="false">
        <div className="flex items-center gap-1">
          <button className={`${b} flex items-center gap-1`} onClick={crop.start} title={`Crop and straighten${hint("crop")}`} data-testid="crop-start">
            <CropIcon className="size-3.5" /> Crop{hint("crop")}
          </button>
          <button className={b} disabled={!c.enabled} onClick={() => editor.change((a) => ({ ...a, crop: editor.defaults.crop }), "Crop: Remove")} data-testid="crop-remove">
            Remove crop
          </button>
        </div>
        <p className="mt-1 text-[11px] text-neutral-400" data-testid="crop-status">
          {c.enabled ? `Cropped ${Math.round((c.right - c.left) * 100)}% x ${Math.round((c.bottom - c.top) * 100)}%${c.angle ? `, ${c.angle.toFixed(1)} deg` : ""}` : "Not cropped"}
        </p>
      </div>
    );
  }
  const setAspect = (id: AspectId) => {
    const next = { ...tool, aspect: id };
    const fr = lockRatio(next, crop.imageAspect);
    crop.change({ ...next, rect: fr == null ? tool.rect : fitRatio(tool.rect, fr) });
  };
  const flip = () => {
    const next = { ...tool, flip: !tool.flip };
    const fr = lockRatio(next, crop.imageAspect);
    crop.change({ ...next, rect: fr == null ? tool.rect : fitRatio(tool.rect, fr) });
  };
  return (
    <div data-testid="crop-panel" data-active="true">
      <div className="mb-2 flex gap-1">
        <select
          className="min-w-0 flex-1 rounded bg-neutral-800 px-1.5 py-1 text-xs"
          value={tool.aspect}
          aria-label="Crop aspect ratio"
          data-testid="crop-aspect"
          onChange={(e) => setAspect(e.target.value as AspectId)}
        >
          {ASPECTS.map((a) => (
            <option key={a.id} value={a.id}>
              {a.label}
            </option>
          ))}
        </select>
        <button className={`${b} ${tool.flip ? "bg-sky-800 text-sky-100" : ""}`} disabled={tool.aspect === "free"} onClick={flip} title="Swap landscape / portrait" data-testid="crop-flip" aria-pressed={tool.flip}>
          <RectangleVertical className="size-3.5" />
        </button>
      </div>
      <Slider
        id="crop-angle"
        label="Angle"
        value={tool.angle}
        min={-45}
        max={45}
        step={0.1}
        digits={1}
        display={(v) => `${v > 0 ? "+" : ""}${v.toFixed(1)}°`}
        onInput={(v) => crop.change({ ...tool, angle: v })}
        onCommit={() => {}}
        onReset={() => crop.change({ ...tool, angle: 0 })}
      />
      <div className="mt-2 flex gap-1">
        <button className={`${b} bg-sky-700 text-white hover:bg-sky-600`} onClick={crop.commit} data-testid="crop-done" title="Apply the crop (Enter)">
          Done
        </button>
        <button className={b} onClick={crop.cancel} data-testid="crop-cancel" title="Discard changes (Esc)">
          Cancel
        </button>
        <button className={b} onClick={() => crop.change({ ...tool, rect: FULL, angle: 0, aspect: "free", flip: false })} data-testid="crop-reset">
          Reset
        </button>
      </div>
      <p className="mt-1 text-[11px] text-neutral-400">Drag the handles or the frame. Enter applies, Esc cancels.</p>
    </div>
  );
}
