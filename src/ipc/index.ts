// Typed backend access. Components import from here, never from `@tauri-apps/api` directly.
export * from "./bindings";

import type { AppError, ImageQuery } from "./bindings";

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
  pick: null,
  minRating: null,
  burstGroupId: null,
  folderId: null,
  sort: "capture_time",
  offset: 0,
  limit: 200,
};
