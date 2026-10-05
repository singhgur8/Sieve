// Capture times are "naive" wall-clock ms (camera local time read as UTC); format and parse them in UTC so no time zone leaks in.
import type { CaptureTimeSource } from "../ipc";

const SEC = 1000;
const MIN = 60 * SEC;
const HOUR = 60 * MIN;

/** "2026-10-05T14:03:22", the value format of `<input type="datetime-local" step="1">`. */
export const toInputValue = (ms: number) => new Date(ms).toISOString().slice(0, 19);

/** Inverse of `toInputValue`; null when empty or invalid. */
export function fromInputValue(v: string): number | null {
  if (!v) return null;
  const ms = Date.parse(v.length === 16 ? `${v}:00Z` : `${v}Z`);
  return Number.isFinite(ms) ? ms : null;
}

/** "+1 h 5 min", "-30 s", "no change". */
export function formatOffset(ms: number): string {
  if (ms === 0) return "no change";
  const sign = ms < 0 ? "-" : "+";
  let a = Math.abs(ms);
  const d = Math.floor(a / (24 * HOUR));
  a -= d * 24 * HOUR;
  const h = Math.floor(a / HOUR);
  a -= h * HOUR;
  const m = Math.floor(a / MIN);
  a -= m * MIN;
  const s = Math.round(a / SEC);
  const parts = [d && `${d} d`, h && `${h} h`, m && `${m} min`, s && `${s} s`].filter(Boolean);
  return `${sign}${parts.join(" ")}`;
}

/** Offset in ms from a sign and h / m / s fields. */
export const offsetFrom = (sign: 1 | -1, h: number, m: number, s: number) => sign * (h * HOUR + m * MIN + s * SEC);

export const sourceLabel = (s: CaptureTimeSource | undefined): string =>
  s === "user" ? "corrected in Sieve" : s === "sidecar" ? "read from the XMP sidecar (e.g. corrected in Lightroom)" : "from the file (EXIF)";
