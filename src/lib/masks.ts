// Masking UI model: tool kinds, local slider definitions (Lightroom ranges), naming and immutable update helpers.
import type { AiTarget, AiTargetKind, LocalAdjustments, MaskBlendMode, MaskComponent, MaskGroup, MaskShape, PersonPart } from "../ipc";

export type ToolKind = "brush" | "linear" | "radial" | "color" | "luminance" | "object";
/** Everything the Create buttons / Add menus offer. */
export type CreateKind = ToolKind | "subject" | "sky" | "background" | "people";

export const CREATE_LABEL: Record<CreateKind, string> = {
  subject: "Subject",
  sky: "Sky",
  background: "Background",
  people: "People",
  object: "Objects",
  brush: "Brush",
  linear: "Linear Gradient",
  radial: "Radial Gradient",
  color: "Color Range",
  luminance: "Luminance Range",
};

export const CREATE_ORDER: CreateKind[] = ["subject", "sky", "background", "people", "object", "brush", "linear", "radial", "color", "luminance"];

/** Capability family of an AI create kind (others need no model). */
export const AI_FAMILY: Partial<Record<CreateKind, AiTargetKind>> = { subject: "subject", sky: "sky", background: "background", people: "people", object: "object" };

export const PERSON_PART_LABEL: Record<PersonPart, string> = {
  face_skin: "Face skin",
  body_skin: "Body skin",
  eyebrows: "Eyebrows",
  eye_sclera: "Eye sclera",
  iris_pupil: "Iris & pupil",
  lips: "Lips",
  teeth: "Teeth",
  hair: "Hair",
  clothes: "Clothes",
};

/** Human name of a component's kind, used for default names ("Brush 1"). */
export function shapeLabel(s: MaskShape): string {
  switch (s.kind) {
    case "brush":
      return "Brush";
    case "linear":
      return "Linear Gradient";
    case "radial":
      return "Radial Gradient";
    case "luminance":
      return "Luminance Range";
    case "color":
      return "Color Range";
    case "unsupported":
      return "Unsupported";
    case "ai":
      return aiLabel(s.target);
  }
}

export function aiLabel(t: AiTarget): string {
  switch (t.kind) {
    case "subject":
      return "Subject";
    case "sky":
      return "Sky";
    case "background":
      return "Background";
    case "people":
      return "Person";
    case "object":
      return "Object";
    case "landscape":
      return "Landscape";
    default:
      return "Selection";
  }
}

/** First free "<base> <n>" among the components of all groups. */
export function nextComponentName(groups: MaskGroup[], base: string): string {
  const used = new Set(groups.flatMap((g) => g.components.map((c) => c.name)));
  for (let n = 1; ; n++) if (!used.has(`${base} ${n}`)) return `${base} ${n}`;
}

export const componentTitle = (c: MaskComponent) => c.name || shapeLabel(c.shape);

export const MODE_GLYPH: Record<MaskBlendMode, string> = { add: "+", subtract: "−", intersect: "∩" };

// ---- immutable updates ----

export function mapGroup(masks: MaskGroup[], gid: string, fn: (g: MaskGroup) => MaskGroup): MaskGroup[] {
  return masks.map((g) => (g.id === gid ? fn(g) : g));
}

export function mapComponent(masks: MaskGroup[], gid: string, cid: string, fn: (c: MaskComponent) => MaskComponent): MaskGroup[] {
  return mapGroup(masks, gid, (g) => ({ ...g, components: g.components.map((c) => (c.id === cid ? fn(c) : c)) }));
}

// ---- local adjustments (Lightroom ranges, UI units) ----

type NumericLocal = Exclude<keyof LocalAdjustments, "color" | "toneCurve">;

export interface LocalDef {
  key: NumericLocal;
  label: string;
  min: number;
  max: number;
  step: number;
  digits: number;
  accent?: string;
}

const pct = (key: NumericLocal, label: string, accent?: string): LocalDef => ({ key, label, min: -100, max: 100, step: 1, digits: 0, accent });

export const LOCAL_GROUPS: { id: string; title: string; defs: LocalDef[] }[] = [
  {
    id: "light",
    title: "Light",
    defs: [
      pct("temperature", "Temp", "#fbbf24"),
      pct("tint", "Tint", "#e879f9"),
      { key: "exposure", label: "Exposure", min: -4, max: 4, step: 0.01, digits: 2 },
      pct("contrast", "Contrast"),
      pct("highlights", "Highlights"),
      pct("shadows", "Shadows"),
      pct("whites", "Whites"),
      pct("blacks", "Blacks"),
    ],
  },
  {
    id: "presence",
    title: "Presence",
    defs: [
      pct("texture", "Texture"),
      pct("clarity", "Clarity"),
      pct("dehaze", "Dehaze"),
      { key: "hue", label: "Hue", min: -180, max: 180, step: 1, digits: 0 },
      pct("saturation", "Saturation"),
    ],
  },
  {
    id: "detail",
    title: "Detail",
    defs: [pct("sharpness", "Sharpness"), pct("noise", "Noise"), pct("moire", "Moire"), pct("defringe", "Defringe")],
  },
];

export const LOCAL_LABEL: Record<string, string> = Object.fromEntries(LOCAL_GROUPS.flatMap((g) => g.defs.map((d) => [d.key, d.label])));

// ---- brush ----

export interface BrushSettings {
  /** Lightroom "Size" slider, 1..100. */
  size: number;
  feather: number;
  flow: number;
  density: number;
  autoMask: boolean;
  erase: boolean;
}

export const DEFAULT_BRUSH: BrushSettings = { size: 20, feather: 50, flow: 50, density: 100, autoMask: false, erase: false };

/** Brush size slider -> stroke radius as a fraction of the sensor width. */
export const sizeToRadius = (size: number) => Math.max(0.002, size / 1000);

// ---- overlay ----

export interface OverlayStyle {
  id: string;
  label: string;
  /** "color" tints the mask with `color`; "gray" shows the matte translucently; "bw" shows it opaque. */
  mode: "color" | "gray" | "bw";
  color: string;
}

export const OVERLAY_STYLES: OverlayStyle[] = [
  { id: "red", label: "Red overlay", mode: "color", color: "#ff2d2d" },
  { id: "green", label: "Green overlay", mode: "color", color: "#22e05a" },
  { id: "blue", label: "Blue overlay", mode: "color", color: "#3b82f6" },
  { id: "yellow", label: "Yellow overlay", mode: "color", color: "#facc15" },
  { id: "gray", label: "Grayscale", mode: "gray", color: "#ffffff" },
  { id: "bw", label: "Black & white", mode: "bw", color: "#ffffff" },
];

/** Mirror of `MASK_LIMITS.maxDabs` guard: stop recording when a stroke gets absurdly long. */
export const MAX_STROKE_DABS = 5000;
