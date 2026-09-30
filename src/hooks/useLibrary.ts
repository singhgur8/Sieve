// Lazily-loaded view of the catalog: the full ordered id list for a query (cheap, one call) plus an
// entry cache filled on demand for the ids the grid actually shows.
import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";
import { commands, DEFAULT_QUERY, events, unwrap, type ImageQuery, type RawImageEntry } from "../ipc";

export type Query = ImageQuery;
export const BASE_QUERY: Query = DEFAULT_QUERY;

const CHUNK = 120;

export interface Library {
  ids: number[];
  epoch: number;
  loaded: boolean;
  getEntry: (id: number) => RawImageEntry | undefined;
  version: (id: number) => number;
  ensure: (ids: number[]) => void;
  /** Ids that must stay loaded (active image, compare panes) regardless of grid scrolling. */
  pin: (ids: number[]) => void;
  patch: (ids: number[], fn: (e: RawImageEntry) => RawImageEntry) => void;
  refresh: (ids: number[]) => Promise<void>;
  /** Re-fetches every entry currently cached (after bulk changes such as scene edits). */
  refreshAll: () => Promise<void>;
  reload: () => Promise<void>;
  touch: () => void;
}

export function useLibrary(query: Query, onError: (e: unknown) => void): Library {
  const [ids, setIds] = useState<number[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [epoch, setEpoch] = useState(0);
  const [, force] = useReducer((x: number) => x + 1, 0);
  const entries = useRef(new Map<number, RawImageEntry>());
  const versions = useRef(new Map<number, number>());
  const eventThumbs = useRef(new Map<number, RawImageEntry["thumbnail"]>());
  const inflight = useRef(new Set<number>());
  const wanted = useRef<number[]>([]);
  const pinned = useRef<number[]>([]);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const touchTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const gen = useRef(0);
  const queryRef = useRef(query);
  queryRef.current = query;
  const queryKey = useMemo(() => JSON.stringify(query), [query]);

  const merge = useCallback((e: RawImageEntry): RawImageEntry => {
    const t = eventThumbs.current.get(e.id);
    return t && e.thumbnail.status === "pending" ? { ...e, thumbnail: t } : e;
  }, []);

  const touch = useCallback(() => {
    clearTimeout(touchTimer.current);
    touchTimer.current = setTimeout(() => setEpoch((n) => n + 1), 250);
  }, []);

  const reload = useCallback(async () => {
    const g = ++gen.current;
    try {
      const list = await unwrap(commands.listImageIds({ ...queryRef.current, offset: 0, limit: 200 }));
      if (g !== gen.current) return;
      setIds(list);
      setLoaded(true);
      setEpoch((n) => n + 1);
    } catch (e) {
      onError(e);
    }
  }, [onError]);

  useEffect(() => {
    void reload();
  }, [queryKey, reload]);

  const flush = useCallback(async () => {
    const want = [...pinned.current, ...wanted.current].filter((id) => !entries.current.has(id) && !inflight.current.has(id));
    const chunks: number[][] = [];
    for (let i = 0; i < want.length; i += CHUNK) chunks.push(want.slice(i, i + CHUNK));
    await Promise.all(
      chunks.map(async (chunk) => {
        chunk.forEach((id) => inflight.current.add(id));
        try {
          const rows = await unwrap(commands.getImages(chunk));
          rows.forEach((r) => entries.current.set(r.id, merge(r)));
        } catch (e) {
          onError(e);
        } finally {
          chunk.forEach((id) => inflight.current.delete(id));
          force();
        }
      }),
    );
  }, [merge, onError]);

  const ensure = useCallback(
    (list: number[]) => {
      wanted.current = list;
      clearTimeout(timer.current);
      timer.current = setTimeout(() => void flush(), 30);
    },
    [flush],
  );

  const pin = useCallback(
    (list: number[]) => {
      pinned.current = list;
      clearTimeout(timer.current);
      timer.current = setTimeout(() => void flush(), 0);
    },
    [flush],
  );

  const patch = useCallback((list: number[], fn: (e: RawImageEntry) => RawImageEntry) => {
    for (const id of list) {
      const e = entries.current.get(id);
      if (e) entries.current.set(id, fn(e));
    }
    force();
  }, []);

  const refresh = useCallback(
    async (list: number[]) => {
      if (list.length === 0) return;
      const rows = await unwrap(commands.getImages(list));
      rows.forEach((r) => entries.current.set(r.id, merge(r)));
      force();
      touch();
    },
    [merge, touch],
  );

  const refreshAll = useCallback(async () => {
    const all = [...entries.current.keys()];
    for (let i = 0; i < all.length; i += CHUNK * 4) await refresh(all.slice(i, i + CHUNK * 4));
  }, [refresh]);

  const refreshLoaded = useCallback(
    (list: number[]) => {
      const have = list.filter((id) => entries.current.has(id));
      refresh(have).catch(() => {});
    },
    [refresh],
  );

  useEffect(() => {
    const unlisten = [
      events.thumbnailReady.listen((ev) => {
        const p = ev.payload;
        const thumbnail = {
          status: "ready" as const,
          path: p.path,
          previewPath: p.previewPath,
          width: p.width,
          height: p.height,
        };
        eventThumbs.current.set(p.imageId, thumbnail);
        versions.current.set(p.imageId, (versions.current.get(p.imageId) ?? 0) + 1);
        patch([p.imageId], (e) => ({ ...e, thumbnail }));
        refreshLoaded([p.imageId]);
      }),
      events.thumbnailFailed.listen((ev) => {
        const thumbnail = { status: "failed" as const, reason: ev.payload.reason };
        eventThumbs.current.set(ev.payload.imageId, thumbnail);
        patch([ev.payload.imageId], (e) => ({ ...e, thumbnail }));
      }),
      events.analysisReady.listen((ev) => refreshLoaded([ev.payload.imageId])),
      events.xmpSynced.listen((ev) => refreshLoaded([...ev.payload.written, ...ev.payload.read])),
      events.xmpWriteFailed.listen((ev) => refreshLoaded([ev.payload.imageId])),
    ];
    return () => {
      unlisten.forEach((u) => void u.then((f) => f()));
    };
  }, [patch, refreshLoaded]);

  const getEntry = useCallback((id: number) => entries.current.get(id), []);
  const version = useCallback((id: number) => versions.current.get(id) ?? 0, []);

  return { ids, epoch, loaded, getEntry, version, ensure, pin, patch, refresh, refreshAll, reload, touch };
}
