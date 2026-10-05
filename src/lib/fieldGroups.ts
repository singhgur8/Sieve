// Layout of the Copy / Synchronize / New Preset settings dialog (docs/ux-spec-8b.md 5.7) and its remembered choice.
import { ALL_ADJUSTMENT_FIELDS, DEFAULT_SYNC_FIELDS, type AdjustmentField, type ParametricAdjustments } from "../ipc";
import { copyFields, sameAdjustments } from "./adjust";

export interface FieldLeaf {
  field: AdjustmentField;
  label: string;
}
export interface FieldGroup {
  /** `field-group-<id>` */
  id: string;
  label: string;
  children: FieldLeaf[];
}
export type FieldNode = ({ kind: "leaf" } & FieldLeaf) | ({ kind: "group" } & FieldGroup);

const leaf = (field: AdjustmentField, label: string): FieldNode => ({ kind: "leaf", field, label });
const group = (id: string, label: string, children: [AdjustmentField, string][]): FieldNode => ({
  kind: "group",
  id,
  label,
  children: children.map(([field, l]) => ({ field, label: l })),
});

/** Three columns, in the order of the spec table. */
export const FIELD_COLUMNS: FieldNode[][] = [
  [
    leaf("white_balance", "White Balance"),
    group("basic_tone", "Basic Tone", [
      ["exposure", "Exposure"],
      ["contrast", "Contrast"],
      ["highlights", "Highlights"],
      ["shadows", "Shadows"],
      ["whites", "White Clipping"],
      ["blacks", "Black Clipping"],
    ]),
    leaf("tone_curve", "Tone Curve"),
    leaf("texture", "Texture"),
    leaf("clarity", "Clarity"),
    leaf("dehaze", "Dehaze"),
    group("color", "Color", [
      ["vibrance", "Vibrance"],
      ["saturation", "Saturation"],
      ["hsl_hue", "Color Mixer: Hue"],
      ["hsl_saturation", "Color Mixer: Saturation"],
      ["hsl_luminance", "Color Mixer: Luminance"],
    ]),
    leaf("color_grading", "Color Grading"),
  ],
  [
    group("treatment", "Treatment & Profile", [
      ["profile", "Profile"],
      ["black_and_white", "Treatment (Black & White)"],
    ]),
    leaf("sharpening", "Sharpening"),
    leaf("noise_reduction", "Noise Reduction"),
    group("effects", "Effects", [
      ["vignette", "Post-Crop Vignetting"],
      ["grain", "Grain"],
    ]),
    leaf("calibration", "Calibration"),
    leaf("lut", "LUT"),
  ],
  [leaf("masks", "Masking"), leaf("crop", "Crop")],
];

/** Fields that have a checkbox in the dialog (the v14 subset fields and `process_version` have none). */
export const DISPLAYED_FIELDS: AdjustmentField[] = FIELD_COLUMNS.flat().flatMap((n) => (n.kind === "leaf" ? [n.field] : n.children.map((c) => c.field)));

/** Fields of the photo that differ from its neutral settings ("Check Modified"). */
export function modifiedFields(adj: ParametricAdjustments, defaults: ParametricAdjustments): AdjustmentField[] {
  return DISPLAYED_FIELDS.filter((f) => !sameAdjustments(copyFields(defaults, adj, [f]), defaults));
}

export const COPY_FIELDS_KEY = "sieve.copyFields.v1";
export const PRESET_FIELDS_KEY = "sieve.presetFields.v1";

/** The remembered choice, or null the first time (or when storage is unreadable). Unknown names are dropped. */
export function loadFields(key: string): AdjustmentField[] | null {
  try {
    const raw = localStorage.getItem(key);
    if (!raw) return null;
    const v = JSON.parse(raw);
    if (!Array.isArray(v)) return null;
    return ALL_ADJUSTMENT_FIELDS.filter((f) => v.includes(f));
  } catch {
    return null;
  }
}

export function saveFields(key: string, fields: readonly AdjustmentField[]) {
  try {
    localStorage.setItem(key, JSON.stringify(fields));
  } catch {
    /* private mode: not remembered */
  }
}

/** Fields a Copy / Sync uses without asking (remembered choice, else everything except crop and masks). */
export const rememberedCopyFields = (): AdjustmentField[] => loadFields(COPY_FIELDS_KEY) ?? DEFAULT_SYNC_FIELDS;

/** Fields whose value an Auto Sync would have to copy as-is. Exposure and white balance are held back until the relative
 * delta command (IPC v19.2 `sync_settings` `relative`) exists: copying them flattens per-photo matching. */
export const AUTO_SYNC_HELD_BACK: readonly AdjustmentField[] = ["exposure", "white_balance"];

/** Setting groups that differ between two states of the same photo (what one committed edit changed). Crop, masks and transform are never listed. */
export function changedFields(prev: ParametricAdjustments, next: ParametricAdjustments): AdjustmentField[] {
  return DEFAULT_SYNC_FIELDS.filter((f) => !sameAdjustments(copyFields(prev, next, [f]), prev));
}
