// Develop saves slider edits asynchronously (debounced history entries). Anything that reads the stored adjustments
// of the active photo (Apply to scene, Apply all, Match scene) must wait for that save first, or it copies a stale
// representative (grain / clarity dropped). The mounted editor registers its `flush` here; callers `await flushEdits()`.
let current: (() => Promise<void>) | null = null;

/** Called by the mounted Develop editor; returns the unregister function. */
export function registerFlush(fn: () => Promise<void>): () => void {
  current = fn;
  return () => {
    if (current === fn) current = null;
  };
}

/** Resolves once every pending Develop edit is stored (no-op when Develop is not open). */
export async function flushEdits(): Promise<void> {
  await current?.();
}
