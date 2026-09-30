import type { AppError, CullTag } from "../ipc";

export function formatError(e: unknown): string {
  const err = e as Partial<AppError>;
  return err && err.kind ? `${err.kind}: ${err.message}` : String(e);
}

export const TAG_SHORT: Record<CullTag, string> = {
  blink: "BL",
  missed_focus: "MF",
  motion_blur: "MB",
  creative_blur: "CB",
  underexposed: "UE",
  overexposed: "OE",
  duplicate_burst: "DB",
};

export const TAG_STYLE: Record<CullTag, string> = {
  blink: "bg-rose-900 text-rose-200",
  missed_focus: "bg-orange-900 text-orange-200",
  motion_blur: "bg-amber-900 text-amber-200",
  creative_blur: "bg-violet-900 text-violet-200",
  underexposed: "bg-slate-700 text-slate-200",
  overexposed: "bg-yellow-800 text-yellow-100",
  duplicate_burst: "bg-sky-900 text-sky-200",
};

export const ALL_TAGS = Object.keys(TAG_SHORT) as CullTag[];

export const tagName = (t: CullTag) => t.replace("_", " ");

export const LABEL_COLOR: Record<string, string> = {
  red: "bg-red-500",
  yellow: "bg-yellow-400",
  green: "bg-green-500",
  blue: "bg-blue-500",
  purple: "bg-purple-500",
};

const timeFmt = new Intl.DateTimeFormat(undefined, { timeZone: "UTC", dateStyle: "medium", timeStyle: "medium" });
export const formatTime = (ms: number) => timeFmt.format(new Date(ms));

export function formatShutter(s: number): string {
  return s >= 1 ? `${trimNum(s)}s` : `1/${Math.round(1 / s)}s`;
}
export function trimNum(n: number): string {
  return String(Math.round(n * 10) / 10);
}
