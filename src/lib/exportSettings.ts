// Pure helpers for the export dialog: format defaults, client-side template preview, validation.
import type { ExportFormat, ExportFormatKind, ExportSettings, RawImageEntry, ResizeMode } from "../ipc";
import { EXPORT_EXTENSIONS } from "../ipc";

export function defaultFormat(kind: ExportFormatKind): ExportFormat {
  switch (kind) {
    case "jpeg":
      return { kind, quality: 90, chromaSubsampling: "444" };
    case "tiff":
      return { kind, bitDepth: "16", compression: "lzw" };
    case "png":
      return { kind, bitDepth: "8" };
    case "webp":
      return { kind, quality: 85, lossless: false };
    case "heic":
      return { kind, quality: 85 };
  }
}

export function defaultResize(kind: ResizeMode["kind"]): ResizeMode {
  switch (kind) {
    case "none":
      return { kind };
    case "long_edge":
      return { kind, px: 2048 };
    case "short_edge":
      return { kind, px: 1080 };
    case "megapixels":
      return { kind, mp: 12 };
    case "width_height":
      return { kind, width: 2048, height: 2048 };
  }
}

export const DEFAULT_SETTINGS: ExportSettings = {
  format: defaultFormat("jpeg"),
  colorSpace: "srgb",
  resize: { mode: { kind: "none" }, dontEnlarge: true, resolutionPpi: 300 },
  sharpening: null,
  naming: { template: "{filename}", startNumber: 1, collision: "unique_suffix" },
  destination: { kind: "choose" },
  subfolder: null,
  metadata: { include: "all", removeLocation: false, includeKeywords: true, copyright: null, creator: null },
};

const pad = (n: number, w: number) => String(n).padStart(w, "0");

function formatDate(ms: number | null, fmt: string): string {
  const d = new Date(ms ?? 0);
  const map: [string, string][] = [
    ["YYYY", pad(d.getUTCFullYear(), 4)],
    ["YY", pad(d.getUTCFullYear() % 100, 2)],
    ["MM", pad(d.getUTCMonth() + 1, 2)],
    ["DD", pad(d.getUTCDate(), 2)],
    ["hh", pad(d.getUTCHours(), 2)],
    ["mm", pad(d.getUTCMinutes(), 2)],
    ["ss", pad(d.getUTCSeconds(), 2)],
  ];
  let out = "";
  for (let i = 0; i < fmt.length; ) {
    const hit = map.find(([k]) => fmt.startsWith(k, i));
    if (hit) {
      out += hit[1];
      i += hit[0].length;
    } else out += fmt[i++];
  }
  return out;
}

export interface TemplateResult {
  /** Expanded example file name incl. extension; `null` when the template is invalid. */
  example: string | null;
  error: string | null;
}

/** Client-side mirror of the Rust expander, for the live example only (the backend is authoritative). */
export function previewTemplate(template: string, startNumber: number, kind: ExportFormatKind, entry: RawImageEntry | null): TemplateResult {
  if (template.trim() === "") return { example: null, error: "Template is empty" };
  let out = "";
  const re = /\{([^{}]*)\}/g;
  let last = 0;
  let m: RegExpExecArray | null;
  const stem = (entry?.fileName ?? "DSC00001.ARW").replace(/\.[^.]+$/, "");
  const folder = (entry?.path ?? "/shoot/DSC00001.ARW").split("/").slice(-2, -1)[0] ?? "";
  while ((m = re.exec(template))) {
    out += template.slice(last, m.index);
    last = m.index + m[0].length;
    const [name, arg] = m[1].split(/:(.*)/s);
    switch (name) {
      case "filename":
        out += stem;
        break;
      case "seq": {
        const w = arg == null ? 1 : Number(arg);
        if (!Number.isInteger(w) || w < 1 || w > 9) return { example: null, error: `Bad {seq:${arg}} width` };
        out += pad(startNumber, w);
        break;
      }
      case "date":
        out += formatDate(entry?.capture.capturedAtMs ?? null, arg ?? "YYYYMMDD");
        break;
      case "rating":
        out += String(entry?.rating ?? 0);
        break;
      case "camera":
        out += entry?.camera.model ?? "Camera";
        break;
      case "folder":
        out += folder;
        break;
      case "id":
        out += String(entry?.id ?? 1);
        break;
      default:
        return { example: null, error: `Unknown token {${m[1]}}` };
    }
  }
  const rest = template.slice(last);
  if (/[{}]/.test(rest)) return { example: null, error: "Unbalanced braces" };
  out += rest;
  if (/[/\\:]/.test(out)) return { example: null, error: "File name cannot contain / \\ or :" };
  return { example: `${out}.${EXPORT_EXTENSIONS[kind]}`, error: null };
}

export function validateSubfolder(s: string | null): string | null {
  if (s == null || s === "") return null;
  if (/[\\:]/.test(s)) return "Subfolder cannot contain \\ or :";
  if (s.split("/").some((p) => p === "" || p === "." || p === "..")) return "Subfolder has an empty, . or .. component";
  return null;
}

/** Normalizes the draft for the backend: blank subfolder/overrides become null. */
export function normalizeSettings(s: ExportSettings): ExportSettings {
  const blank = (v: string | null) => (v == null || v.trim() === "" ? null : v.trim());
  return {
    ...s,
    subfolder: blank(s.subfolder),
    metadata: { ...s.metadata, copyright: blank(s.metadata.copyright), creator: blank(s.metadata.creator) },
  };
}
