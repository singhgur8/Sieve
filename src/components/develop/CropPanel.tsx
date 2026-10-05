// Crop drawer of the adjust panel (right under the tool strip while the crop tool is active): start/finish the crop tool, aspect presets, straighten angle.
import { Loader2, Lock, LockOpen, Ruler, RectangleVertical, Wand2 } from "lucide-react";
import { hint } from "../../lib/keymap";
import { ASPECTS, OVERLAYS, saveOverlay, type AspectId, type OverlayId } from "../../lib/crop";
import { resetTool, setAspectTool, setCustomRatio, swapTool, toggleLockTool, type CropTool } from "./CropOverlay";
import { Slider } from "./Slider";

export interface CropApi {
  tool: CropTool | null;
  imageAspect: number;
  start: () => void;
  change: (t: CropTool | ((prev: CropTool) => CropTool)) => void;
  commit: () => void;
  cancel: () => void;
  /** Auto straighten (Upright Level -> crop angle). */
  autoStraighten: () => void;
  autoBusy: boolean;
  /** Inline note when no straight lines were found. */
  autoMessage: string | null;
}

const AUTO_TIP = "Auto straighten: level the horizon / verticals from the photo (Shift+double-click the Angle slider)";

/** W : H inputs of the Custom aspect. */
function CustomRatio({ tool, crop, testid }: { tool: CropTool; crop: CropApi; testid: string }) {
  if (tool.aspect !== "custom") return null;
  const r = tool.customRatio ?? crop.imageAspect;
  const w = +(r >= 1 ? r : 1).toFixed(2);
  const h = +(r >= 1 ? 1 : 1 / r).toFixed(2);
  const inp = "w-12 rounded bg-neutral-800 px-1 py-0.5 text-xs tabular-nums";
  const set = (nw: number, nh: number) => crop.change(setCustomRatio(tool, nw, nh, crop.imageAspect));
  return (
    <span className="flex items-center gap-1" data-testid={testid}>
      <input className={inp} type="number" min={0.1} step={0.1} defaultValue={w} key={`w${w}`} aria-label="Custom aspect width" data-testid={`${testid}-w`} onChange={(e) => set(Number(e.target.value), h)} />
      :
      <input className={inp} type="number" min={0.1} step={0.1} defaultValue={h} key={`h${h}`} aria-label="Custom aspect height" data-testid={`${testid}-h`} onChange={(e) => set(w, Number(e.target.value))} />
    </span>
  );
}

const AspectOptions = () => (
  <>
    {ASPECTS.map((a) => (
      <option key={a.id} value={a.id}>
        {a.label}
      </option>
    ))}
  </>
);

const b = "rounded bg-neutral-800 px-2 py-1 text-xs hover:bg-neutral-700 disabled:opacity-40";

export function CropPanel({ crop }: { crop: CropApi }) {
  const { tool } = crop;
  if (!tool) return null; // the drawer exists only while the tool is active (tool strip, R)
  const setAspect = (id: AspectId) => crop.change(setAspectTool(tool, id, crop.imageAspect));
  const flip = () => crop.change(swapTool(tool, crop.imageAspect));
  return (
    <div className="border-b border-neutral-800 py-2" data-testid="crop-panel" data-active="true">
      <div className="mb-2 flex gap-1">
        <select
          className="min-w-0 flex-1 rounded bg-neutral-800 px-1.5 py-1 text-xs"
          value={tool.aspect}
          aria-label="Crop aspect ratio"
          data-testid="crop-aspect"
          onChange={(e) => setAspect(e.target.value as AspectId)}
        >
          <AspectOptions />
        </select>
        <button className={`${b} ${tool.aspect !== "free" ? "bg-sky-800 text-sky-100" : ""}`} onClick={() => crop.change(toggleLockTool(tool, crop.imageAspect))} title={`Lock the aspect ratio${hint("cropLock")}`} aria-pressed={tool.aspect !== "free"} data-testid="crop-lock">
          {tool.aspect !== "free" ? <Lock className="size-3.5" /> : <LockOpen className="size-3.5" />}
        </button>
        <button className={`${b} ${tool.flip ? "bg-sky-800 text-sky-100" : ""}`} onClick={flip} title={`Swap landscape / portrait${hint("cropSwap")}`} data-testid="crop-flip" aria-pressed={tool.flip}>
          <RectangleVertical className="size-3.5" />
        </button>
      </div>
      {tool.aspect === "custom" && (
        <div className="mb-2 text-xs">
          <CustomRatio tool={tool} crop={crop} testid="crop-custom" />
        </div>
      )}
      <div className="mb-2 flex flex-wrap gap-1">
        <button className={`${b} ${tool.angleTool ? "bg-sky-800 text-sky-100" : ""}`} onClick={() => crop.change({ ...tool, angleTool: !tool.angleTool })} title="Angle tool: drag a line along the horizon (or Cmd-drag)" aria-pressed={!!tool.angleTool} data-testid="crop-angle-tool">
          <Ruler className="inline size-3.5" /> Angle tool
        </button>
        <button className={b} disabled={crop.autoBusy} onClick={crop.autoStraighten} title={AUTO_TIP} data-testid="crop-auto-straighten">
          {crop.autoBusy ? <Loader2 className="inline size-3.5 animate-spin" /> : <Wand2 className="inline size-3.5" />} Auto
        </button>
      </div>
      {crop.autoMessage && (
        <p className="mb-2 rounded bg-amber-900/40 px-1.5 py-1 text-[11px] text-amber-200" role="status" data-testid="crop-auto-message">
          {crop.autoMessage}
        </p>
      )}
      <label className="mb-2 flex items-center gap-1.5 text-xs text-neutral-300" title="Keep the crop inside the straightened image">
        <input type="checkbox" checked={tool.constrain} onChange={(e) => crop.change({ ...tool, constrain: e.target.checked })} data-testid="crop-constrain" />
        Constrain to image
      </label>
      <label className="mb-2 flex items-center gap-1.5 text-xs text-neutral-300" title="Cycle with O, rotate with Shift+O">
        Overlay
        <select
          className="min-w-0 flex-1 rounded bg-neutral-800 px-1.5 py-1 text-xs"
          value={tool.overlay}
          aria-label="Crop guide overlay"
          data-testid="crop-overlay-select"
          onChange={(e) => {
            saveOverlay(e.target.value as OverlayId);
            crop.change({ ...tool, overlay: e.target.value as OverlayId });
          }}
        >
          {OVERLAYS.map((o) => (
            <option key={o.id} value={o.id}>
              {o.label}
            </option>
          ))}
        </select>
      </label>
      <Slider
        id="crop-angle"
        label="Angle"
        value={tool.angle}
        min={-45}
        max={45}
        step={0.1}
        digits={1}
        display={(v) => `${v > 0 ? "+" : ""}${v.toFixed(1)}°`}
        onInput={(v) => crop.change((t) => ({ ...t, angle: v, rotating: true }))}
        onCommit={() => crop.change((t) => ({ ...t, rotating: false }))}
        onReset={() => crop.change((t) => ({ ...t, angle: 0, rotating: false }))}
        onAuto={crop.autoStraighten}
      />
      <div className="mt-2 flex gap-1">
        <button className={`${b} bg-sky-700 text-white hover:bg-sky-600`} onClick={crop.commit} data-testid="crop-done" title="Apply the crop (Enter)">
          Done
        </button>
        <button className={b} onClick={crop.cancel} data-testid="crop-cancel" title="Discard changes (Esc)">
          Cancel
        </button>
        <button className={b} onClick={() => crop.change(resetTool(tool))} title={`Reset the crop${hint("cropReset")}`} data-testid="crop-reset">
          Reset
        </button>
      </div>
      <p className="mt-1 text-[11px] text-neutral-400">Drag the handles or the frame; drag outside to rotate. R / Enter applies, Esc cancels, O cycles guides.</p>
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
        onChange={(e) => crop.change(setAspectTool(tool, e.target.value as AspectId, crop.imageAspect))}
      >
        <AspectOptions />
      </select>
      <CustomRatio tool={tool} crop={crop} testid="cropbar-custom" />
      <button className={`${ibtn} ${locked ? "bg-sky-800" : ""}`} onClick={() => crop.change(toggleLockTool(tool, crop.imageAspect))} title={`Lock the aspect ratio${hint("cropLock")}`} aria-pressed={locked} data-testid="cropbar-lock">
        {locked ? <Lock className="size-3.5" /> : <LockOpen className="size-3.5" />}
      </button>
      <button className={`${ibtn} ${tool.flip ? "bg-sky-800" : ""}`} onClick={() => crop.change(swapTool(tool, crop.imageAspect))} title={`Swap landscape / portrait${hint("cropSwap")}`} aria-pressed={tool.flip} data-testid="cropbar-flip">
        <RectangleVertical className="size-3.5" />
      </button>
      <button className={`${ibtn} ${tool.angleTool ? "bg-sky-800" : ""}`} onClick={() => crop.change({ ...tool, angleTool: !tool.angleTool })} title="Angle tool: drag a line along the horizon (or Cmd-drag)" aria-pressed={!!tool.angleTool} data-testid="cropbar-angle-tool">
        <Ruler className="size-3.5" />
      </button>
      <button className={`${ibtn} disabled:opacity-40`} disabled={crop.autoBusy} onClick={crop.autoStraighten} title={AUTO_TIP} data-testid="cropbar-auto-straighten">
        {crop.autoBusy ? <Loader2 className="size-3.5 animate-spin" /> : <Wand2 className="size-3.5" />}
      </button>
      {crop.autoMessage && (
        <span className="max-w-48 truncate text-amber-300" title={crop.autoMessage} role="status" data-testid="cropbar-auto-message">
          {crop.autoMessage}
        </span>
      )}
      <label className="flex items-center gap-1" title="Keep the crop inside the straightened image">
        <input type="checkbox" checked={tool.constrain} onChange={(e) => crop.change({ ...tool, constrain: e.target.checked })} data-testid="cropbar-constrain" />
        Constrain
      </label>
      <label className="flex items-center gap-1.5" title="Straighten angle (double-click to reset)">
        Angle
        <input
          type="range"
          className="sieve-range h-3 w-40 cursor-pointer"
          style={{ background: "linear-gradient(#525252, #525252) center / 100% 2px no-repeat" }}
          min={-45}
          max={45}
          step={0.1}
          value={tool.angle}
          aria-label="Crop angle"
          data-testid="cropbar-angle"
          onChange={(e) => crop.change((t) => ({ ...t, angle: Number(e.target.value), rotating: true }))}
          onPointerUp={() => crop.change((t) => ({ ...t, rotating: false }))}
          onBlur={() => crop.change((t) => (t.rotating ? { ...t, rotating: false } : t))}
          onKeyUp={() => crop.change((t) => ({ ...t, rotating: false }))}
          onDoubleClick={(e) => (e.shiftKey ? crop.autoStraighten() : crop.change((t) => ({ ...t, angle: 0, rotating: false })))}
        />
        <span className="w-10 tabular-nums text-neutral-300" data-testid="cropbar-angle-value">
          {`${tool.angle > 0 ? "+" : ""}${tool.angle.toFixed(1)}°`}
        </span>
      </label>
      <button className="rounded px-2 py-1 hover:bg-white/15" onClick={() => crop.change(resetTool(tool))} title={`Reset the crop${hint("cropReset")}`} data-testid="cropbar-reset">
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
