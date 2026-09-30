// Typed backend access. Components import from here, never from `@tauri-apps/api` directly.
export * from "./bindings";
export { convertFileSrc } from "@tauri-apps/api/core";

import { DEFAULT_ADJUSTMENTS, DEFAULT_ADJUSTMENTS_NON_RAW, DEFAULT_LOCAL_ADJUSTMENTS } from "./bindings";
import type {
  AdjustmentField,
  AppError,
  ColorGrading,
  ColorWheel,
  CropSettings,
  ExportFormatKind,
  HslChannels,
  ImageFormat,
  ImageQuery,
  LocalAdjustments,
  MaskComponent,
  MaskGroup,
  MaskShape,
  MatchOptions,
  NormPoint,
  ParametricAdjustments,
  PointCurves,
  ProfileSettings,
  WhiteBalance,
} from "./bindings";

type CommandResult<T> = { status: "ok"; data: T } | { status: "error"; error: AppError };

/** Unwraps a command result, throwing the `AppError` on failure. */
export async function unwrap<T>(result: Promise<CommandResult<T>>): Promise<T> {
  const r = await result;
  if (r.status === "error") throw r.error;
  return r.data;
}

/** Tone-curve point `[input, output]`, 0..=255 (Rust `CurvePoint`, inlined by the generator). */
export type CurvePoint = [number, number];

/** Pre-v9 name of `ImageFormat` (IPC v9 renamed it and added non-RAW formats). */
export type RawFormat = ImageFormat;

/** RAW formats (mirror of Rust `ImageFormat::RAW`). */
export const RAW_FORMATS: readonly ImageFormat[] = ["arw", "raf", "cr3"];

/** Mirror of Rust `ImageFormat::is_raw`. */
export function isRawFormat(format: ImageFormat): boolean {
  return RAW_FORMATS.includes(format);
}

/**
 * `ParametricAdjustments` with every v9 group present. The backend always sends complete
 * objects; the groups are optional in the generated type only for older callers.
 */
export type CompleteAdjustments = Required<ParametricAdjustments>;

/**
 * Neutral settings for a source format (Lightroom defaults; mirror of Rust
 * `ParametricAdjustments::defaults_for`, generated from Rust as `DEFAULT_ADJUSTMENTS*`).
 * RAW (or unknown format): profile "Adobe Color", sharpening 40, color NR 25; non-RAW: no
 * profile, no default sharpening / color NR.
 */
export function defaultAdjustments(format?: ImageFormat): CompleteAdjustments {
  const src = format !== undefined && !isRawFormat(format) ? DEFAULT_ADJUSTMENTS_NON_RAW : DEFAULT_ADJUSTMENTS;
  return structuredClone(src) as unknown as CompleteAdjustments;
}

/** Fills groups missing from `adj` (e.g. built by pre-v9 code) with the format's defaults. */
export function completeAdjustments(adj: ParametricAdjustments, format?: ImageFormat): CompleteAdjustments {
  const d = defaultAdjustments(format);
  return {
    ...adj,
    toneCurve: adj.toneCurve ?? d.toneCurve,
    colorGrading: adj.colorGrading ?? d.colorGrading,
    calibration: adj.calibration ?? d.calibration,
    detail: adj.detail ?? d.detail,
    effects: adj.effects ?? d.effects,
    blackAndWhite: adj.blackAndWhite ?? d.blackAndWhite,
    crop: adj.crop ?? d.crop,
    profile: adj.profile ?? d.profile,
    masks: adj.masks ?? d.masks,
  };
}

/** Identity point curve `[[0, 0], [255, 255]]` (mirror of Rust `PointCurves::IDENTITY`). */
export const IDENTITY_CURVE: readonly CurvePoint[] = [
  [0, 0],
  [255, 255],
];

/** Max points per curve (mirror of Rust `PointCurves::MAX_POINTS`). */
export const MAX_CURVE_POINTS = 32;

export const DEFAULT_QUERY: ImageQuery = {
  includeTags: [],
  excludeTags: [],
  tagMatch: "any",
  picks: [],
  minRating: null,
  maxRating: null,
  colorLabels: [],
  burstGroupId: null,
  sceneId: null,
  collapseBursts: false,
  folderId: null,
  sort: "capture_time",
  sortDescending: false,
  offset: 0,
  limit: 200,
};

// Exhaustive by construction: adding a Rust `AdjustmentField` variant fails the type check here.
const ADJUSTMENT_FIELD_SET: Record<AdjustmentField, true> = {
  white_balance: true,
  exposure: true,
  contrast: true,
  highlights: true,
  shadows: true,
  whites: true,
  blacks: true,
  texture: true,
  clarity: true,
  dehaze: true,
  vibrance: true,
  saturation: true,
  hsl_hue: true,
  hsl_saturation: true,
  hsl_luminance: true,
  lut: true,
  tone_curve: true,
  color_grading: true,
  calibration: true,
  sharpening: true,
  noise_reduction: true,
  vignette: true,
  grain: true,
  black_and_white: true,
  crop: true,
  profile: true,
  masks: true,
};

/** Every `AdjustmentField`, in panel order (fields mask "select all"). */
export const ALL_ADJUSTMENT_FIELDS = Object.keys(ADJUSTMENT_FIELD_SET) as AdjustmentField[];

/** Display names for fields-mask checkboxes (Lightroom's Copy Settings wording). */
export const ADJUSTMENT_FIELD_LABELS: Record<AdjustmentField, string> = {
  white_balance: "White balance",
  exposure: "Exposure",
  contrast: "Contrast",
  highlights: "Highlights",
  shadows: "Shadows",
  whites: "Whites",
  blacks: "Blacks",
  texture: "Texture",
  clarity: "Clarity",
  dehaze: "Dehaze",
  vibrance: "Vibrance",
  saturation: "Saturation",
  hsl_hue: "HSL hue",
  hsl_saturation: "HSL saturation",
  hsl_luminance: "HSL luminance",
  lut: "LUT",
  tone_curve: "Tone curve",
  color_grading: "Color grading",
  calibration: "Calibration",
  sharpening: "Sharpening",
  noise_reduction: "Noise reduction",
  vignette: "Post-crop vignetting",
  grain: "Grain",
  black_and_white: "Black & white",
  crop: "Crop",
  profile: "Profile",
  masks: "Masking",
};

/**
 * Copies the `fields` groups of `src` over `dst` (mirror of Rust
 * `ParametricAdjustments::copy_fields`; keep in sync). Returns a new, complete object.
 */
export function copyAdjustmentFields(
  dst: ParametricAdjustments,
  src: ParametricAdjustments,
  fields: readonly AdjustmentField[],
): CompleteAdjustments {
  const out = structuredClone(completeAdjustments(dst));
  const s = structuredClone(completeAdjustments(src));
  for (const f of fields) {
    switch (f) {
      case "white_balance":
        out.whiteBalance = s.whiteBalance;
        break;
      case "hsl_hue":
        out.hsl.hue = s.hsl.hue;
        break;
      case "hsl_saturation":
        out.hsl.saturation = s.hsl.saturation;
        break;
      case "hsl_luminance":
        out.hsl.luminance = s.hsl.luminance;
        break;
      case "lut":
        out.lut = s.lut;
        break;
      case "tone_curve":
        out.toneCurve = s.toneCurve;
        break;
      case "color_grading":
        out.colorGrading = s.colorGrading;
        break;
      case "calibration":
        out.calibration = s.calibration;
        break;
      case "sharpening":
        out.detail.sharpening = s.detail.sharpening;
        break;
      case "noise_reduction":
        out.detail.noiseReduction = s.detail.noiseReduction;
        break;
      case "vignette":
        out.effects.vignette = s.effects.vignette;
        break;
      case "grain":
        out.effects.grain = s.effects.grain;
        break;
      case "black_and_white":
        out.blackAndWhite = s.blackAndWhite;
        break;
      case "crop":
        out.crop = s.crop;
        break;
      case "profile":
        out.profile = s.profile;
        break;
      case "masks":
        out.masks = s.masks.map(transferableMaskGroup);
        break;
      default:
        out[f] = s[f];
    }
  }
  return out;
}

/**
 * Everything except `crop` and `masks` (mirror of Rust `AdjustmentField::DEFAULT_SYNC`): the
 * default selection for Sync / Copy Settings and scene matching (per-frame geometry).
 */
export const DEFAULT_SYNC_FIELDS = ALL_ADJUSTMENT_FIELDS.filter((f) => f !== "crop" && f !== "masks");

/** File-name template tokens (mirror of Rust `FILENAME_TOKENS` / `parse_filename_template`). */
export const EXPORT_FILENAME_TOKENS: { token: string; description: string }[] = [
  { token: "{filename}", description: "Original file name without extension" },
  { token: "{seq}", description: "Sequence number from Start number ({seq:4} pads to 4 digits)" },
  { token: "{date}", description: "Capture date ({date:YYYY-MM-DD_hhmmss}; default YYYYMMDD)" },
  { token: "{rating}", description: "Star rating 0-5" },
  { token: "{camera}", description: "Camera model" },
  { token: "{folder}", description: "Name of the RAW's folder" },
  { token: "{id}", description: "Catalog image id" },
];

/** File extension written per format (mirror of Rust `ExportFormatKind::extension`). */
export const EXPORT_EXTENSIONS: Record<ExportFormatKind, string> = {
  jpeg: "jpg",
  tiff: "tif",
  png: "png",
  webp: "webp",
  heic: "heic",
};

/** Mirror of Rust `MatchOptions::default()` (scene matching). */
export const DEFAULT_MATCH_OPTIONS: MatchOptions = {
  matchExposure: true,
  matchWhiteBalance: true,
  matchTone: false,
  strength: 1,
  copyFields: DEFAULT_SYNC_FIELDS,
};

/** Mirror of Rust `scene::TOLERANCE_EV` / `scene::TOLERANCE_AB` (a match is "within tolerance"). */
export const SCENE_MATCH_TOLERANCE = { ev: 0.15, ab: 0.012 } as const;

/** At most this many anchors per scene / `matchScene` call (Rust `Scene::MAX_ANCHORS`). */
export const MAX_SCENE_ANCHORS = 2;

function lerpHsl(a: HslChannels, b: HslChannels, t: number): HslChannels {
  const out = { ...a };
  for (const k of Object.keys(a) as (keyof HslChannels)[]) out[k] = a[k] + (b[k] - a[k]) * t;
  return out;
}

/** Linear interpolation of every numeric field of a flat record (other fields: nearer side). */
function lerpFlat<T extends object>(a: T, b: T, t: number): T {
  const out = { ...a } as Record<string, unknown>;
  const bb = b as Record<string, unknown>;
  for (const [k, v] of Object.entries(a)) {
    const w = bb[k];
    out[k] = typeof v === "number" && typeof w === "number" ? v + (w - v) * t : t >= 0.5 ? w : v;
  }
  return out as T;
}

function lerpWheel(a: ColorWheel, b: ColorWheel, t: number): ColorWheel {
  if (t <= 0) return a;
  if (t >= 1) return b;
  let hue: number;
  if (a.saturation === 0) hue = b.hue;
  else if (b.saturation === 0) hue = a.hue;
  else {
    let d = (b.hue - a.hue) % 360;
    if (d > 180) d -= 360;
    else if (d < -180) d += 360;
    hue = (((a.hue + d * t) % 360) + 360) % 360;
  }
  return {
    hue,
    saturation: a.saturation + (b.saturation - a.saturation) * t,
    luminance: a.luminance + (b.luminance - a.luminance) * t,
  };
}

function lerpCurve(a: CurvePoint[], b: CurvePoint[], t: number): CurvePoint[] {
  if (a.length === b.length) {
    return a.map((p, i) => [p[0] + (b[i][0] - p[0]) * t, p[1] + (b[i][1] - p[1]) * t] as CurvePoint);
  }
  return t >= 0.5 ? b : a;
}

function lerpPointCurves(a: PointCurves, b: PointCurves, t: number): PointCurves {
  return {
    master: lerpCurve(a.master, b.master, t),
    red: lerpCurve(a.red, b.red, t),
    green: lerpCurve(a.green, b.green, t),
    blue: lerpCurve(a.blue, b.blue, t),
  };
}

function lerpGrading(a: ColorGrading, b: ColorGrading, t: number): ColorGrading {
  return {
    shadows: lerpWheel(a.shadows, b.shadows, t),
    midtones: lerpWheel(a.midtones, b.midtones, t),
    highlights: lerpWheel(a.highlights, b.highlights, t),
    global: lerpWheel(a.global, b.global, t),
    blending: a.blending + (b.blending - a.blending) * t,
    balance: a.balance + (b.balance - a.balance) * t,
  };
}

function lerpCrop(a: CropSettings, b: CropSettings, t: number): CropSettings {
  return a.enabled && b.enabled ? lerpFlat(a, b, t) : t >= 0.5 ? b : a;
}

function lerpProfile(a: ProfileSettings, b: ProfileSettings, t: number): ProfileSettings {
  if (a.look && b.look && a.look.uuid === b.look.uuid && a.cameraProfile === b.cameraProfile) {
    return { cameraProfile: a.cameraProfile, look: { ...a.look, amount: a.look.amount + (b.look.amount - a.look.amount) * t } };
  }
  return t >= 0.5 ? b : a;
}

/**
 * Mirror of Rust `ParametricAdjustments::lerp` (keep in sync): interpolates a (t = 0) -> b (t = 1).
 * Scene-match strength slider: `lerpAdjustments(preview.base, preview.full, strength)`.
 * Missing v9 groups are treated as RAW defaults; the result is always complete.
 */
export function lerpAdjustments(a0: ParametricAdjustments, b0: ParametricAdjustments, t: number): CompleteAdjustments {
  const a = completeAdjustments(a0);
  const b = completeAdjustments(b0);
  t = Number.isFinite(t) ? Math.min(1, Math.max(0, t)) : 0;
  const l = (x: number, y: number) => x + (y - x) * t;
  const nearB = t >= 0.5;
  let whiteBalance: WhiteBalance;
  if (a.whiteBalance.mode === "custom" && b.whiteBalance.mode === "custom") {
    const mired = l(1e6 / a.whiteBalance.temperatureK, 1e6 / b.whiteBalance.temperatureK);
    whiteBalance = {
      mode: "custom",
      temperatureK: Math.min(50000, Math.max(2000, 1e6 / mired)),
      tint: l(a.whiteBalance.tint, b.whiteBalance.tint),
    };
  } else {
    whiteBalance = nearB ? b.whiteBalance : a.whiteBalance;
  }
  const lut =
    a.lut && b.lut && a.lut.id === b.lut.id
      ? { id: a.lut.id, amount: l(a.lut.amount, b.lut.amount) }
      : nearB
        ? b.lut
        : a.lut;
  return {
    processVersion: a.processVersion,
    whiteBalance,
    exposure: l(a.exposure, b.exposure),
    contrast: l(a.contrast, b.contrast),
    highlights: l(a.highlights, b.highlights),
    shadows: l(a.shadows, b.shadows),
    whites: l(a.whites, b.whites),
    blacks: l(a.blacks, b.blacks),
    texture: l(a.texture, b.texture),
    clarity: l(a.clarity, b.clarity),
    dehaze: l(a.dehaze, b.dehaze),
    vibrance: l(a.vibrance, b.vibrance),
    saturation: l(a.saturation, b.saturation),
    hsl: {
      hue: lerpHsl(a.hsl.hue, b.hsl.hue, t),
      saturation: lerpHsl(a.hsl.saturation, b.hsl.saturation, t),
      luminance: lerpHsl(a.hsl.luminance, b.hsl.luminance, t),
    },
    lut,
    toneCurve: {
      parametric: lerpFlat(a.toneCurve.parametric, b.toneCurve.parametric, t),
      point: lerpPointCurves(a.toneCurve.point, b.toneCurve.point, t),
    },
    colorGrading: lerpGrading(a.colorGrading, b.colorGrading, t),
    calibration: {
      red: lerpFlat(a.calibration.red, b.calibration.red, t),
      green: lerpFlat(a.calibration.green, b.calibration.green, t),
      blue: lerpFlat(a.calibration.blue, b.calibration.blue, t),
      shadowTint: l(a.calibration.shadowTint, b.calibration.shadowTint),
    },
    detail: {
      sharpening: lerpFlat(a.detail.sharpening, b.detail.sharpening, t),
      noiseReduction: lerpFlat(a.detail.noiseReduction, b.detail.noiseReduction, t),
    },
    effects: {
      vignette: lerpFlat(a.effects.vignette, b.effects.vignette, t),
      grain: lerpFlat(a.effects.grain, b.effects.grain, t),
    },
    blackAndWhite: {
      enabled: nearB ? b.blackAndWhite.enabled : a.blackAndWhite.enabled,
      mixer: lerpHsl(a.blackAndWhite.mixer, b.blackAndWhite.mixer, t),
    },
    crop: lerpCrop(a.crop, b.crop, t),
    profile: lerpProfile(a.profile, b.profile, t),
    masks: nearB ? b.masks : a.masks,
  };
}

// ---------------------------------------------------------------------------
// Masks / local adjustments (IPC v10). Coordinates are in the sensor frame (un-oriented,
// uncropped, normalized): see `src-tauri/src/ipc/masks.rs` and docs/architecture.md "Masks".
// ---------------------------------------------------------------------------

/** Limits enforced by the backend (mirror of Rust `MaskLimits`). */
export const MASK_LIMITS = {
  maxGroups: 100,
  maxComponents: 64,
  maxDabs: 200_000,
  maxColorSamples: 5,
  maxName: 64,
} as const;

/** Lightroom-style id for a new group / component: 32 upper-case hex digits. */
export function newMaskId(): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0"))
    .join("")
    .toUpperCase();
}

/** Default local slider set (all 0, refine saturation 100, identity curves). */
export function defaultLocalAdjustments(): LocalAdjustments {
  return structuredClone(DEFAULT_LOCAL_ADJUSTMENTS) as unknown as LocalAdjustments;
}

/** A new, active component (opacity 1, add, not inverted). */
export function newMaskComponent(shape: MaskShape, name = ""): MaskComponent {
  return { id: newMaskId(), name, active: true, mode: "add", inverted: false, opacity: 1, shape };
}

/** A new, active mask group at 100 % with default local sliders. */
export function newMaskGroup(name: string, components: MaskComponent[] = []): MaskGroup {
  return { id: newMaskId(), name, active: true, amount: 1, adjustments: defaultLocalAdjustments(), components };
}

/**
 * Copy of a group for another image (paste / sync / presets): AI components lose their
 * `digest` so the target recomputes its own matte (mirror of Rust `MaskGroup::transferable`).
 */
export function transferableMaskGroup(g: MaskGroup): MaskGroup {
  const out = structuredClone(g);
  for (const c of out.components) if (c.shape.kind === "ai") c.shape.digest = null;
  return out;
}

/** Sensor frame -> displayed (EXIF-oriented) frame (mirror of Rust `orient_point`). */
export function orientPoint(p: NormPoint, orientation: number | null | undefined): NormPoint {
  const { x: u, y: v } = p;
  switch (orientation) {
    case 2:
      return { x: 1 - u, y: v };
    case 3:
      return { x: 1 - u, y: 1 - v };
    case 4:
      return { x: u, y: 1 - v };
    case 5:
      return { x: v, y: u };
    case 6:
      return { x: 1 - v, y: u };
    case 7:
      return { x: 1 - v, y: 1 - u };
    case 8:
      return { x: v, y: 1 - u };
    default:
      return { x: u, y: v };
  }
}

/** Displayed (EXIF-oriented) frame -> sensor frame (mirror of Rust `unorient_point`). */
export function unorientPoint(p: NormPoint, orientation: number | null | undefined): NormPoint {
  const { x, y } = p;
  switch (orientation) {
    case 2:
      return { x: 1 - x, y };
    case 3:
      return { x: 1 - x, y: 1 - y };
    case 4:
      return { x, y: 1 - y };
    case 5:
      return { x: y, y: x };
    case 6:
      return { x: y, y: 1 - x };
    case 7:
      return { x: 1 - y, y: 1 - x };
    case 8:
      return { x: 1 - y, y: x };
    default:
      return { x, y };
  }
}
