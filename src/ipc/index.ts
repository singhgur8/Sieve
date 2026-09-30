// Typed backend access. Components import from here, never from `@tauri-apps/api` directly.
export * from "./bindings";
export { convertFileSrc } from "@tauri-apps/api/core";

import type { AdjustmentField, AppError, ExportFormatKind, ImageQuery } from "./bindings";

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
