// One place that turns backend AppErrors / failure reasons into user-facing wording, and remembers which
// originals are known to be missing or undecodable so the grid, loupe and Develop can say so (session-only
// until IPC v13's `missingSinceMs` lands; then the entry field is authoritative).
//
// Classification branches on the v13 error kinds (file_missing, disk_full, read_only, decode_failed,
// catalog_read_only) and falls back to the message text for kinds that carry no detail (not_found, io,
// database, ...), which is what the backend emits today.
import { useSyncExternalStore } from "react";

export type ErrorCategory = "missing" | "disk_full" | "read_only" | "permission" | "decode" | "folder_gone" | "catalog_readonly" | "catalog_full" | "other";

export interface ErrorInfo {
  category: ErrorCategory;
  title: string;
  message: string;
  /** Persistent problems get an inline banner instead of a dismissible toast. */
  persistent: boolean;
}

const TITLES: Record<ErrorCategory, string> = {
  missing: "Original file is missing",
  disk_full: "Disk is full",
  read_only: "Folder is read-only",
  permission: "Permission denied",
  decode: "Could not decode the file",
  folder_gone: "Folder not found",
  catalog_readonly: "Catalog is read-only",
  catalog_full: "Catalog could not be saved",
  other: "Something went wrong",
};

const BY_KIND: Record<string, ErrorCategory> = {
  file_missing: "missing",
  disk_full: "disk_full",
  read_only: "read_only",
  decode_failed: "decode",
  catalog_read_only: "catalog_readonly",
};

export function categorize(kind: string | undefined, message: string): ErrorCategory {
  if (kind && BY_KIND[kind]) return BY_KIND[kind];
  const m = message.toLowerCase();
  if (kind === "database" && m.includes("opened read-only")) return "catalog_readonly";
  if (kind === "database" && m.includes("full or unavailable")) return "catalog_full";
  if (m.startsWith("original file is missing")) return "missing";
  if (m.includes("the disk is full") || m.startsWith("not enough disk space")) return "disk_full";
  if (m.includes("the volume is read-only") || m.includes("read-only file system")) return "read_only";
  if (m.includes("permission denied")) return "permission";
  if (m.includes("could not decode")) return "decode";
  if (m.includes("no longer exists")) return "folder_gone";
  return "other";
}

/** Classifies an AppError (or any thrown value / failure-reason string). */
export function describeError(e: unknown): ErrorInfo {
  const err = e as { kind?: unknown; message?: unknown } | null;
  const isObj = err !== null && typeof err === "object";
  const message = isObj && typeof err.message === "string" ? err.message : String(e);
  const kind = isObj && typeof err.kind === "string" ? err.kind : undefined;
  const category = categorize(kind, message);
  return { category, title: TITLES[category], message, persistent: category === "catalog_readonly" };
}

/** A failure reason string (XMP write, export item, analysis). */
export function describeReason(reason: string): ErrorInfo {
  return describeError({ message: reason });
}

// ---- per-file health registry ----
export interface FileHealth {
  kind: "missing" | "decode";
  message: string;
}

let files = new Map<string, FileHealth>();
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((l) => l());

const MISSING_RE = /Original file is missing or was moved: (.+?)\. Reconnect/;
const DECODE_RE = /Could not decode (.+?): /;

/** Records missing / undecodable originals mentioned in an error or failure reason. Returns true when it did. */
export function noteFailure(text: string): boolean {
  const miss = MISSING_RE.exec(text);
  const dec = miss ? null : DECODE_RE.exec(text);
  const path = (miss ?? dec)?.[1];
  if (!path) return false;
  const kind = miss ? "missing" : "decode";
  if (files.get(path)?.kind === kind) return true;
  files = new Map(files).set(path, { kind, message: text });
  emit();
  return true;
}

export function clearFileHealth(path?: string) {
  if (path == null) files = new Map();
  else if (files.has(path)) {
    files = new Map(files);
    files.delete(path);
  } else return;
  emit();
}

export function fileHealth(path: string | undefined): FileHealth | undefined {
  return path ? files.get(path) : undefined;
}

export function useFileHealth(path: string | undefined): FileHealth | undefined {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => (path ? files.get(path) : undefined),
  );
}
