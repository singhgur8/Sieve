// Lightroom-style Library Filter "Metadata" columns: turns `MetadataFilterOptions` (values with counts) and the active
// `MetadataFilter` into columns, toggles, chips and one-line descriptions. Multi-select within a list column (OR), AND across
// columns. Numeric columns and the capture day are ranges on the wire, so selecting several values selects the range
// from the lowest to the highest one.
import { useSyncExternalStore } from "react";
import type { CameraFilter, MetadataFilter, MetadataFilterOptions, NumberCount, NumberRange } from "../ipc";

export type ColumnKey = "extensions" | "cameras" | "lenses" | "iso" | "focalLengthMm" | "aperture" | "shutterSeconds" | "captured" | "edited" | "hasSidecar";

export interface ColOption {
  id: string;
  label: string;
  count: number;
  selected: boolean;
  /** Unknown values cannot be part of a range. */
  disabled?: boolean;
}

export interface Column {
  key: ColumnKey;
  title: string;
  options: ColOption[];
  /** The filter after clicking option `id`. */
  toggle: (m: MetadataFilter, id: string) => MetadataFilter;
}

const DAY = 86_400_000;
const MAKE: Record<string, string> = { sony: "Sony", fujifilm: "Fujifilm", canon: "Canon", other: "" };

export const cameraLabel = (c: CameraFilter) => [MAKE[c.make] ?? c.make, c.model ?? (c.make === "other" ? "Unknown camera" : "")].filter(Boolean).join(" ") || "Unknown camera";
export const shutterLabel = (s: number) => (s >= 0.4 ? `${Math.round(s * 10) / 10}s` : `1/${Math.round(1 / s)}`);
export const dayLabel = (ms: number) => new Date(ms).toISOString().slice(0, 10);

/** Drops empty constraints; `undefined` when nothing is left (so queries stay identical to unfiltered ones). */
export function cleanMeta(m: MetadataFilter | undefined): MetadataFilter | undefined {
  if (!m) return undefined;
  const out: MetadataFilter = {};
  if (m.formats?.length) out.formats = m.formats;
  if (m.extensions?.length) out.extensions = m.extensions;
  if (m.cameras?.length) out.cameras = m.cameras;
  if (m.lenses?.length) out.lenses = m.lenses;
  if (m.iso) out.iso = m.iso;
  if (m.focalLengthMm) out.focalLengthMm = m.focalLengthMm;
  if (m.aperture) out.aperture = m.aperture;
  if (m.shutterSeconds) out.shutterSeconds = m.shutterSeconds;
  if (m.captured) out.captured = m.captured;
  if (m.edited != null) out.edited = m.edited;
  if (m.hasSidecar != null) out.hasSidecar = m.hasSidecar;
  return Object.keys(out).length ? out : undefined;
}

export const metaActive = (m: MetadataFilter | undefined) => cleanMeta(m) != null;

const toggleIn = <T,>(list: T[] | undefined, v: T, same: (a: T, b: T) => boolean): T[] => {
  const cur = list ?? [];
  return cur.some((x) => same(x, v)) ? cur.filter((x) => !same(x, v)) : [...cur, v];
};

function numberColumn(key: "iso" | "focalLengthMm" | "aperture" | "shutterSeconds", title: string, values: NumberCount[], m: MetadataFilter, label: (v: number) => string): Column {
  const range: NumberRange | null | undefined = m[key];
  const inRange = (v: number) => !!range && (range.min == null || v >= range.min - Math.abs(range.min) * 1e-6) && (range.max == null || v <= range.max + Math.abs(range.max) * 1e-6);
  return {
    key,
    title,
    options: values.map((o) => ({ id: String(o.value), label: o.value == null ? "Unknown" : label(o.value), count: o.count, selected: o.value != null && inRange(o.value), disabled: o.value == null })),
    toggle: (cur, id) => {
      const v = Number(id);
      const sel = values.map((o) => o.value).filter((x): x is number => x != null && inRange(x));
      const next = sel.includes(v) ? sel.filter((x) => x !== v) : [...sel, v];
      return { ...cur, [key]: next.length ? { min: Math.min(...next), max: Math.max(...next) } : null };
    },
  };
}

export function metaColumns(o: MetadataFilterOptions, m: MetadataFilter): Column[] {
  const days = o.captureDays;
  const cap = m.captured;
  const dayIn = (d: number) => !!cap && (cap.fromMs == null || d >= cap.fromMs) && (cap.toMs == null || d < cap.toMs);
  const yesNo = (key: "edited" | "hasSidecar", title: string, c: { yes: number; no: number }, yes: string, no: string): Column => ({
    key,
    title,
    options: [
      { id: "yes", label: yes, count: c.yes, selected: m[key] === true },
      { id: "no", label: no, count: c.no, selected: m[key] === false },
    ],
    toggle: (cur, id) => ({ ...cur, [key]: m[key] === (id === "yes") ? null : id === "yes" }),
  });
  return [
    {
      key: "extensions",
      title: "File type",
      options: o.extensions.map((e) => ({ id: e.extension, label: e.extension.toUpperCase(), count: e.count, selected: !!m.extensions?.some((x) => x.toLowerCase() === e.extension) })),
      toggle: (cur, id) => ({ ...cur, extensions: toggleIn(cur.extensions, id, (a, b) => a.toLowerCase() === b.toLowerCase()) }),
    },
    {
      key: "cameras",
      title: "Camera",
      options: o.cameras.map((c) => ({
        id: JSON.stringify(c.camera),
        label: cameraLabel(c.camera),
        count: c.count,
        selected: !!m.cameras?.some((x) => x.make === c.camera.make && x.model === c.camera.model),
      })),
      toggle: (cur, id) => ({ ...cur, cameras: toggleIn(cur.cameras, JSON.parse(id) as CameraFilter, (a, b) => a.make === b.make && a.model === b.model) }),
    },
    {
      key: "lenses",
      title: "Lens",
      options: o.lenses.map((l) => ({ id: JSON.stringify(l.lens), label: l.lens ?? "Unknown lens", count: l.count, selected: !!m.lenses?.some((x) => x === l.lens) })),
      toggle: (cur, id) => ({ ...cur, lenses: toggleIn(cur.lenses, JSON.parse(id) as string | null, (a, b) => a === b) }),
    },
    numberColumn("iso", "ISO", o.isos, m, (v) => String(Math.round(v))),
    numberColumn("focalLengthMm", "Focal length", o.focalLengths, m, (v) => `${v} mm`),
    numberColumn("aperture", "Aperture", o.apertures, m, (v) => `f/${v}`),
    numberColumn("shutterSeconds", "Shutter", o.shutterSpeeds, m, shutterLabel),
    {
      key: "captured",
      title: "Capture date",
      options: days.map((d) => ({ id: String(d.dayStartMs), label: d.dayStartMs == null ? "No date" : dayLabel(d.dayStartMs), count: d.count, selected: d.dayStartMs != null && dayIn(d.dayStartMs), disabled: d.dayStartMs == null })),
      toggle: (cur, id) => {
        const v = Number(id);
        const all = days.map((d) => d.dayStartMs).filter((x): x is number => x != null);
        const sel = all.filter(dayIn);
        const next = sel.includes(v) ? sel.filter((x) => x !== v) : [...sel, v];
        return { ...cur, captured: next.length ? { fromMs: Math.min(...next), toMs: Math.max(...next) + DAY } : null };
      },
    },
    yesNo("edited", "Edited", o.edited, "Edited", "Unedited"),
    yesNo("hasSidecar", "Sidecar", o.hasSidecar, "Has sidecar", "No sidecar"),
  ];
}

export interface MetaChip {
  key: ColumnKey;
  text: string;
}

/** One chip per active column ("File type: ARW, RAF"); `cols` supplies the labels when the options are loaded. */
export function metaChips(m: MetadataFilter | undefined, cols: Column[] | null): MetaChip[] {
  const c = cleanMeta(m);
  if (!c) return [];
  const out: MetaChip[] = [];
  const list = (key: ColumnKey, title: string, fallback: string[]) => {
    const col = cols?.find((x) => x.key === key);
    const sel = col?.options.filter((o) => o.selected).map((o) => o.label);
    const items = sel && sel.length ? sel : fallback;
    out.push({ key, text: `${title}: ${items.length > 3 ? `${items.slice(0, 3).join(", ")} +${items.length - 3}` : items.join(", ")}` });
  };
  if (c.extensions) list("extensions", "File type", c.extensions.map((e) => e.toUpperCase()));
  if (c.cameras) list("cameras", "Camera", c.cameras.map(cameraLabel));
  if (c.lenses) list("lenses", "Lens", c.lenses.map((l) => l ?? "Unknown lens"));
  const rng = (key: "iso" | "focalLengthMm" | "aperture" | "shutterSeconds", title: string, f: (v: number) => string) => {
    const r = c[key];
    if (!r) return;
    const a = r.min != null ? f(r.min) : "…";
    const b = r.max != null ? f(r.max) : "…";
    out.push({ key, text: `${title}: ${a === b ? a : `${a} – ${b}`}` });
  };
  rng("iso", "ISO", (v) => String(Math.round(v)));
  rng("focalLengthMm", "Focal length", (v) => `${v} mm`);
  rng("aperture", "Aperture", (v) => `f/${v}`);
  rng("shutterSeconds", "Shutter", shutterLabel);
  if (c.captured) {
    const a = c.captured.fromMs != null ? dayLabel(c.captured.fromMs) : "…";
    const b = c.captured.toMs != null ? dayLabel(c.captured.toMs - DAY) : "…";
    out.push({ key: "captured", text: `Date: ${a === b ? a : `${a} – ${b}`}` });
  }
  if (c.edited != null) out.push({ key: "edited", text: c.edited ? "Edited" : "Unedited" });
  if (c.hasSidecar != null) out.push({ key: "hasSidecar", text: c.hasSidecar ? "Has sidecar" : "No sidecar" });
  return out;
}

export const describeMeta = (m: MetadataFilter | undefined): string[] => metaChips(m, null).map((c) => c.text);

export function clearColumn(m: MetadataFilter, key: ColumnKey): MetadataFilter | undefined {
  const next: MetadataFilter = { ...m };
  if (key === "extensions") delete next.extensions;
  else if (key === "cameras") delete next.cameras;
  else if (key === "lenses") delete next.lenses;
  else delete next[key];
  return cleanMeta(next);
}

// ---- row open / closed, remembered for the session ----
const OPEN_KEY = "sieve.metaRowOpen";
const listeners = new Set<() => void>();
let open = (() => {
  try {
    return sessionStorage.getItem(OPEN_KEY) === "1";
  } catch {
    return false;
  }
})();
export function setMetaRowOpen(v: boolean) {
  open = v;
  try {
    sessionStorage.setItem(OPEN_KEY, v ? "1" : "0");
  } catch {
    /* private mode */
  }
  listeners.forEach((l) => l());
}
export function useMetaRowOpen(): boolean {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => {
        listeners.delete(l);
      };
    },
    () => open,
    () => false,
  );
}
