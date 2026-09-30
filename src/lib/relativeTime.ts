const rtf = new Intl.RelativeTimeFormat("en", { numeric: "auto" });
const STEPS: [Intl.RelativeTimeFormatUnit, number][] = [
  ["year", 365 * 86_400_000],
  ["month", 30 * 86_400_000],
  ["week", 7 * 86_400_000],
  ["day", 86_400_000],
  ["hour", 3_600_000],
  ["minute", 60_000],
];

/** "2 days ago", "yesterday", "just now". */
export function relativeTime(ms: number, now = Date.now()): string {
  const d = ms - now;
  if (Math.abs(d) < 60_000) return "just now";
  for (const [unit, size] of STEPS) if (Math.abs(d) >= size) return rtf.format(Math.round(d / size), unit);
  return "just now";
}

const dateFmt = new Intl.DateTimeFormat("en", { year: "numeric", month: "short", day: "numeric" });
export const formatDate = (ms: number) => dateFmt.format(new Date(ms));
