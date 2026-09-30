// Crop section of the adjust panel: start/finish the crop tool, aspect presets, straighten angle.
import { Crop as CropIcon, Lock, LockOpen, RectangleVertical } from "lucide-react";
import type { Editor } from "../../hooks/useEditor";
import { hint } from "../../lib/keymap";
import { ASPECTS, FULL, saveCropAspect, type AspectId } from "../../lib/crop";
import { refit, swapTool, toggleLockTool, type CropTool } from "./CropOverlay";
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
    saveCropAspect(id);
    crop.change(refit({ ...tool, aspect: id, customRatio: undefined }, crop.imageAspect));
  };
  const flip = () => crop.change(swapTool(tool, crop.imageAspect));
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
          {tool.aspect === "custom" && <option value="custom">Custom</option>}
          {ASPECTS.map((a) => (
            <option key={a.id} value={a.id}>
              {a.label}
            </option>
          ))}
        </select>
        <button className={`${b} ${tool.flip ? "bg-sky-800 text-sky-100" : ""}`} onClick={flip} title={`Swap landscape / portrait${hint("cropSwap")}`} data-testid="crop-flip" aria-pressed={tool.flip}>
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
        <button className={b} onClick={() => crop.change({ ...tool, rect: FULL, angle: 0, aspect: "original", flip: false, customRatio: undefined })} data-testid="crop-reset">
          Reset
        </button>
      </div>
      <p className="mt-1 text-[11px] text-neutral-400">Drag the handles or the frame. Enter applies, Esc cancels.</p>
    </div>
  );
}

/** Floating crop bar (bottom centre of the viewer): every crop control stays reachable with the side panels hidden. */
export function CropBar({ crop }: { crop: CropApi }) {
  const { tool } = crop;
  if (!tool) return null;
  const locked = tool.aspect !== "free";
  const ibtn = "flex size-6 items-center justify-center rounded text-neutral-200 hover:bg-white/15";
  return (
    <div
      className="absolute bottom-2 left-1/2 z-20 flex h-9 -translate-x-1/2 items-center gap-2 rounded-lg bg-black/70 px-2 text-xs text-neutral-100 shadow-lg"
      data-testid="crop-bar"
      onPointerDown={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
    >
      <select
        className="w-24 rounded bg-neutral-800 px-1 py-0.5 text-xs"
        value={tool.aspect}
        aria-label="Crop aspect ratio"
        data-testid="cropbar-aspect"
        onChange={(e) => {
          const id = e.target.value as AspectId;
          saveCropAspect(id);
          crop.change(refit({ ...tool, aspect: id, customRatio: undefined }, crop.imageAspect));
        }}
      >
        {tool.aspect === "custom" && <option value="custom">Custom</option>}
        {ASPECTS.map((a) => (
          <option key={a.id} value={a.id}>
            {a.label}
          </option>
        ))}
      </select>
      <button className={`${ibtn} ${locked ? "bg-sky-800" : ""}`} onClick={() => crop.change(toggleLockTool(tool, crop.imageAspect))} title={`Lock the aspect ratio${hint("cropLock")}`} aria-pressed={locked} data-testid="cropbar-lock">
        {locked ? <Lock className="size-3.5" /> : <LockOpen className="size-3.5" />}
      </button>
      <button className={`${ibtn} ${tool.flip ? "bg-sky-800" : ""}`} onClick={() => crop.change(swapTool(tool, crop.imageAspect))} title={`Swap landscape / portrait${hint("cropSwap")}`} aria-pressed={tool.flip} data-testid="cropbar-flip">
        <RectangleVertical className="size-3.5" />
      </button>
      <label className="flex items-center gap-1.5" title="Straighten angle (double-click to reset)">
        Angle
        <input
          type="range"
          className="sieve-range h-3 w-40 cursor-pointer"
          min={-45}
          max={45}
          step={0.1}
          value={tool.angle}
          aria-label="Crop angle"
          data-testid="cropbar-angle"
          onChange={(e) => crop.change({ ...tool, angle: Number(e.target.value) })}
          onDoubleClick={() => crop.change({ ...tool, angle: 0 })}
        />
        <span className="w-10 tabular-nums text-neutral-300" data-testid="cropbar-angle-value">
          {`${tool.angle > 0 ? "+" : ""}${tool.angle.toFixed(1)}°`}
        </span>
      </label>
      <button className="rounded px-2 py-1 hover:bg-white/15" onClick={() => crop.change({ ...tool, rect: FULL, angle: 0, aspect: "original", flip: false, customRatio: undefined })} data-testid="cropbar-reset">
        Reset
      </button>
      <button className="rounded px-2 py-1 hover:bg-white/15" onClick={crop.cancel} title="Discard changes (Esc)" data-testid="cropbar-cancel">
        Cancel
      </button>
      <button className="rounded bg-sky-700 px-2.5 py-1 text-white hover:bg-sky-600" onClick={crop.commit} title="Apply the crop (Enter)" data-testid="cropbar-done">
        Done
      </button>
    </div>
  );
}
