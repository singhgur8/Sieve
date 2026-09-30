// Typed backend access. Components import from here, never from `@tauri-apps/api` directly.
export * from "./bindings";
export { convertFileSrc } from "@tauri-apps/api/core";

import type {
  AdjustmentField,
  AppError,
  ExportFormatKind,
  HslChannels,
  ImageQuery,
  MatchOptions,
  ParametricAdjustments,
  WhiteBalance,
} from "./bindings";

type CommandResult<T> = { status: "ok"; data: T } | { status: "error"; error: AppError };

/** Unwraps a command result, throwing the `AppError` on failure. */
export async function unwrap<T>(result: Promise<CommandResult<T>>): Promise<T> {
  const r = await result;
  if (r.status === "error") throw r.error;
  return r.data;
}

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
};

/** Every `AdjustmentField`, in panel order (fields mask "select all"). */
export const ALL_ADJUSTMENT_FIELDS = Object.keys(ADJUSTMENT_FIELD_SET) as AdjustmentField[];

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
  copyFields: ALL_ADJUSTMENT_FIELDS,
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

/**
 * Mirror of Rust `ParametricAdjustments::lerp` (keep in sync): interpolates a (t = 0) -> b (t = 1).
 * Scene-match strength slider: `lerpAdjustments(preview.base, preview.full, strength)`.
 */
export function lerpAdjustments(a: ParametricAdjustments, b: ParametricAdjustments, t: number): ParametricAdjustments {
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
  };
}
