// Slider definitions, neutral adjustments and helpers for the Develop module.
import type { AdjustmentField, HslChannels, ParametricAdjustments } from "../ipc";
import { ADJUSTMENT_FIELD_LABELS, copyAdjustmentFields } from "../ipc";

export type Band = keyof HslChannels;
export const BANDS: Band[] = ["red", "orange", "yellow", "green", "aqua", "blue", "purple", "magenta"];
export const BAND_COLOR: Record<Band, string> = {
  red: "#ef4444",
  orange: "#f97316",
  yellow: "#eab308",
  green: "#22c55e",
  aqua: "#06b6d4",
  blue: "#3b82f6",
  purple: "#a855f7",
  magenta: "#ec4899",
};
export type HslKind = "hue" | "saturation" | "luminance";
export const HSL_FIELD: Record<HslKind, AdjustmentField> = { hue: "hsl_hue", saturation: "hsl_saturation", luminance: "hsl_luminance" };

const zeroBands = (): HslChannels => ({ red: 0, orange: 0, yellow: 0, green: 0, aqua: 0, blue: 0, purple: 0, magenta: 0 });

export const PROCESS_VERSION = 1;

export function neutralAdjustments(): ParametricAdjustments {
  return {
    processVersion: PROCESS_VERSION,
    whiteBalance: { mode: "as_shot" },
    exposure: 0,
    contrast: 0,
    highlights: 0,
    shadows: 0,
    whites: 0,
    blacks: 0,
    texture: 0,
    clarity: 0,
    dehaze: 0,
    vibrance: 0,
    saturation: 0,
    hsl: { hue: zeroBands(), saturation: zeroBands(), luminance: zeroBands() },
    lut: null,
  };
}

export type SimpleKey = "exposure" | "contrast" | "highlights" | "shadows" | "whites" | "blacks" | "texture" | "clarity" | "dehaze" | "vibrance" | "saturation";

export interface SliderDef {
  key: SimpleKey;
  label: string;
  min: number;
  max: number;
  step: number;
  digits: number;
}

const pct = (key: SimpleKey, label: string): SliderDef => ({ key, label, min: -100, max: 100, step: 1, digits: 0 });

export const BASIC: SliderDef[] = [
  { key: "exposure", label: "Exposure", min: -5, max: 5, step: 0.01, digits: 2 },
  pct("contrast", "Contrast"),
  pct("highlights", "Highlights"),
  pct("shadows", "Shadows"),
  pct("whites", "Whites"),
  pct("blacks", "Blacks"),
];
export const PRESENCE: SliderDef[] = [
  pct("texture", "Texture"),
  pct("clarity", "Clarity"),
  pct("dehaze", "Dehaze"),
  pct("vibrance", "Vibrance"),
  pct("saturation", "Saturation"),
];

export const TEMP_MIN = 2000;
export const TEMP_MAX = 50000;
/** Temperature sliders are logarithmic: 0..1 position <-> Kelvin. */
export const tempToPos = (k: number) => (Math.log(k) - Math.log(TEMP_MIN)) / (Math.log(TEMP_MAX) - Math.log(TEMP_MIN));
export const posToTemp = (t: number) => Math.round(Math.exp(Math.log(TEMP_MIN) + t * (Math.log(TEMP_MAX) - Math.log(TEMP_MIN))) / 10) * 10;

// IPC v9 (architect): labels + copy semantics live next to the contract in `src/ipc`.
export const FIELD_LABEL: Record<AdjustmentField, string> = ADJUSTMENT_FIELD_LABELS;

/** Field groups per panel section (section resets). */
export const BASIC_FIELDS: AdjustmentField[] = ["white_balance", "exposure", "contrast", "highlights", "shadows", "whites", "blacks"];
export const PRESENCE_FIELDS: AdjustmentField[] = ["texture", "clarity", "dehaze", "vibrance", "saturation"];
export const HSL_FIELDS: AdjustmentField[] = ["hsl_hue", "hsl_saturation", "hsl_luminance"];

/** Copies the `fields` groups of `src` over `dst` (mirrors `ParametricAdjustments::copy_fields`). */
export function copyFields(dst: ParametricAdjustments, src: ParametricAdjustments, fields: AdjustmentField[]): ParametricAdjustments {
  return copyAdjustmentFields(dst, src, fields);
}

export const sameAdjustments = (a: ParametricAdjustments, b: ParametricAdjustments) => JSON.stringify(a) === JSON.stringify(b);
