import { useEffect, useRef } from "react";
import { convertFileSrc } from "../ipc";
import type { Library } from "./useLibrary";

const AHEAD = 4;
const BEHIND = 2;
const KEEP = 24;

/** URLs a viewer paints for an entry: the small thumbnail first, then the 2048px preview. */
export function entryUrls(lib: Library, id: number): string[] {
  const t = lib.getEntry(id)?.thumbnail;
  if (!t || t.status !== "ready") return [];
  const v = lib.version(id);
  const out = [`${convertFileSrc(t.path)}?v=${v}`];
  if (t.previewPath && t.previewPath !== t.path) out.push(`${convertFileSrc(t.previewPath)}?v=${v}`);
  return out;
}

/**
 * Warms the browser's decoded-image cache for the photos around `activeId` in the current (filtered) order, so
 * arrow-key navigation never waits for a fetch + decode. The direction of travel is remembered: more photos are
 * fetched ahead of it than behind it. Holds the last KEEP Image objects so the decoded bitmaps are not collected.
 */
export function usePrefetchNeighbours(lib: Library, activeId: number | null, enabled = true) {
  const held = useRef(new Map<string, HTMLImageElement>());
  const lastIndex = useRef(-1);
  const { ids } = lib;
  useEffect(() => {
    if (!enabled || activeId == null) return;
    const i = ids.indexOf(activeId);
    if (i < 0) return;
    const dir = lastIndex.current >= 0 && i < lastIndex.current ? -1 : 1;
    lastIndex.current = i;
    const order: number[] = [];
    for (let k = 1; k <= AHEAD; k++) order.push(i + dir * k);
    for (let k = 1; k <= BEHIND; k++) order.push(i - dir * k);
    const cache = held.current;
    for (const j of order) {
      const id = ids[j];
      if (id == null) continue;
      for (const url of entryUrls(lib, id)) {
        if (cache.has(url)) {
          // Re-insert to mark as recently used.
          const im = cache.get(url)!;
          cache.delete(url);
          cache.set(url, im);
          continue;
        }
        const im = new Image();
        im.decoding = "async";
        im.src = url;
        void im.decode().catch(() => {});
        cache.set(url, im);
      }
    }
    while (cache.size > KEEP) cache.delete(cache.keys().next().value as string);
  }, [ids, activeId, enabled, lib]);
}
