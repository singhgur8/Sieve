// Detail (sharpening, noise reduction), Effects (post-crop vignette, grain) and Calibration panels.
import type { CompleteAdjustments, NoiseReduction, Sharpening, VignetteStyle } from "../../ipc";
import type { Editor } from "../../hooks/useEditor";
import { NumField } from "./fields";

type Props = { editor: Editor };

const SubHead = ({ children }: { children: string }) => <div className="mb-1 mt-1 text-[11px] font-semibold uppercase tracking-wide text-neutral-400">{children}</div>;

const sharp = (k: keyof Sharpening) => (a: CompleteAdjustments) => a.detail.sharpening[k];
const setSharp = (k: keyof Sharpening) => (a: CompleteAdjustments, v: number): CompleteAdjustments => ({ ...a, detail: { ...a.detail, sharpening: { ...a.detail.sharpening, [k]: v } } });
const nr = (k: keyof NoiseReduction) => (a: CompleteAdjustments) => a.detail.noiseReduction[k];
const setNr = (k: keyof NoiseReduction) => (a: CompleteAdjustments, v: number): CompleteAdjustments => ({ ...a, detail: { ...a.detail, noiseReduction: { ...a.detail.noiseReduction, [k]: v } } });

export function DetailPanel({ editor }: Props) {
  const S = (id: string, label: string, k: keyof Sharpening, min: number, max: number, step = 1, digits = 0) => (
    <NumField editor={editor} id={`sharp-${id}`} label={label} group="Sharpening" min={min} max={max} step={step} digits={digits} get={sharp(k)} set={setSharp(k)} />
  );
  const N = (id: string, label: string, k: keyof NoiseReduction) => (
    <NumField editor={editor} id={`nr-${id}`} label={label} group="Noise Reduction" min={0} max={100} step={1} get={nr(k)} set={setNr(k)} />
  );
  return (
    <>
      <SubHead>Sharpening</SubHead>
      {S("amount", "Amount", "amount", 0, 150)}
      {S("radius", "Radius", "radius", 0.5, 3, 0.1, 1)}
      {S("detail", "Detail", "detail", 0, 100)}
      {S("masking", "Masking", "masking", 0, 100)}
      <SubHead>Noise Reduction</SubHead>
      {N("lum", "Luminance", "luminance")}
      {N("lum-detail", "Detail", "luminanceDetail")}
      {N("lum-contrast", "Contrast", "luminanceContrast")}
      {N("color", "Color", "color")}
      {N("color-detail", "Color Detail", "colorDetail")}
      {N("color-smooth", "Smoothness", "colorSmoothness")}
    </>
  );
}

const STYLES: { id: VignetteStyle; label: string }[] = [
  { id: "highlight_priority", label: "Highlight Priority" },
  { id: "color_priority", label: "Color Priority" },
  { id: "paint_overlay", label: "Paint Overlay" },
];

export function EffectsPanel({ editor }: Props) {
  const v = editor.adj.effects.vignette;
  const V = (id: string, label: string, k: "amount" | "midpoint" | "roundness" | "feather" | "highlights", min: number, max: number, disabled = false) => (
    <NumField
      editor={editor}
      id={`vig-${id}`}
      label={label}
      group="Vignette"
      min={min}
      max={max}
      step={1}
      disabled={disabled}
      get={(a) => a.effects.vignette[k]}
      set={(a, x) => ({ ...a, effects: { ...a.effects, vignette: { ...a.effects.vignette, [k]: x } } })}
    />
  );
  const G = (id: string, label: string, k: "amount" | "size" | "roughness") => (
    <NumField
      editor={editor}
      id={`grain-${id}`}
      label={label}
      group="Grain"
      min={0}
      max={100}
      step={1}
      get={(a) => a.effects.grain[k]}
      set={(a, x) => ({ ...a, effects: { ...a.effects, grain: { ...a.effects.grain, [k]: x } } })}
    />
  );
  return (
    <>
      <SubHead>Post-Crop Vignetting</SubHead>
      <select
        className="mb-2 w-full rounded bg-neutral-800 px-1.5 py-1 text-xs"
        value={v.style}
        aria-label="Vignette style"
        data-testid="vig-style"
        onChange={(e) => editor.change((a) => ({ ...a, effects: { ...a.effects, vignette: { ...a.effects.vignette, style: e.target.value as VignetteStyle } } }), "Vignette: Style")}
      >
        {STYLES.map((s) => (
          <option key={s.id} value={s.id}>
            {s.label}
          </option>
        ))}
      </select>
      {V("amount", "Amount", "amount", -100, 100)}
      {V("midpoint", "Midpoint", "midpoint", 0, 100)}
      {V("roundness", "Roundness", "roundness", -100, 100)}
      {V("feather", "Feather", "feather", 0, 100)}
      {V("highlights", "Highlights", "highlights", 0, 100, v.style === "paint_overlay")}
      <SubHead>Grain</SubHead>
      {G("amount", "Amount", "amount")}
      {G("size", "Size", "size")}
      {G("roughness", "Roughness", "roughness")}
    </>
  );
}

const PRIMARIES = [
  ["red", "Red", "#ef4444"],
  ["green", "Green", "#22c55e"],
  ["blue", "Blue", "#3b82f6"],
] as const;

export function CalibrationPanel({ editor }: Props) {
  return (
    <>
      <NumField
        editor={editor}
        id="calib-shadow-tint"
        label="Shadows Tint"
        group="Calibration"
        min={-100}
        max={100}
        step={1}
        accent="#e879f9"
        get={(a) => a.calibration.shadowTint}
        set={(a, v) => ({ ...a, calibration: { ...a.calibration, shadowTint: v } })}
      />
      {PRIMARIES.map(([k, name, color]) => (
        <div key={k}>
          <SubHead>{`${name} Primary`}</SubHead>
          <NumField
            editor={editor}
            id={`calib-${k}-hue`}
            label="Hue"
            group={`Calibration: ${name}`}
            min={-100}
            max={100}
            step={1}
            accent={color}
            get={(a) => a.calibration[k].hue}
            set={(a, v) => ({ ...a, calibration: { ...a.calibration, [k]: { ...a.calibration[k], hue: v } } })}
          />
          <NumField
            editor={editor}
            id={`calib-${k}-sat`}
            label="Saturation"
            group={`Calibration: ${name}`}
            min={-100}
            max={100}
            step={1}
            accent={color}
            get={(a) => a.calibration[k].saturation}
            set={(a, v) => ({ ...a, calibration: { ...a.calibration, [k]: { ...a.calibration[k], saturation: v } } })}
          />
        </div>
      ))}
    </>
  );
}
