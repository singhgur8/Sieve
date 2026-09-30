// Scenes of the current folder: list, detect, edit membership, anchors. Every mutation goes through
// the typed wrappers, then re-reads the scene list and the cached library rows (sceneId / isSceneAnchor).
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { commands, events, MAX_SCENE_ANCHORS, unwrap, type Scene, type SceneProgress } from "../ipc";
import type { Library } from "./useLibrary";

export interface ScenesApi {
  scenes: Scene[];
  /** 1-based position in the strip, used for "Scene N" labels. */
  number: (id: number) => number;
  sceneOfImage: (imageId: number | null) => Scene | undefined;
  progress: SceneProgress | null;
  detecting: boolean;
  detect: () => Promise<void>;
  createFromImages: (imageIds: number[]) => Promise<void>;
  mergeOf: (imageIds: number[]) => Promise<void>;
  splitAt: (imageId: number | null) => Promise<void>;
  removeFromScene: (imageIds: number[]) => Promise<void>;
  deleteScene: (id: number) => Promise<void>;
  toggleAnchor: (imageId: number | null) => Promise<void>;
  /** Re-reads scenes + cached rows (after an external change such as an applied match). */
  sync: () => Promise<void>;
}

export function useScenes(
  folderId: number | null,
  sceneFilter: number | null,
  lib: Library,
  onError: (e: unknown) => void,
  notify: (msg: string) => void,
): ScenesApi {
  const [scenes, setScenes] = useState<Scene[]>([]);
  const [progress, setProgress] = useState<SceneProgress | null>(null);
  const [detecting, setDetecting] = useState(false);
  const scenesRef = useRef(scenes);
  scenesRef.current = scenes;
  const filterRef = useRef(sceneFilter);
  filterRef.current = sceneFilter;
  // Order in which anchors were marked per scene (oldest first) for the replace policy.
  const marked = useRef(new Map<number, number[]>());
  const libRef = useRef(lib);
  libRef.current = lib;

  const reloadScenes = useCallback(async () => {
    const list = await unwrap(commands.listScenes(folderId));
    setScenes(list);
    return list;
  }, [folderId]);

  useEffect(() => {
    reloadScenes().catch(onError);
  }, [reloadScenes, onError]);

  useEffect(() => {
    const un = events.sceneProgress.listen((ev) => setProgress(ev.payload));
    return () => void un.then((f) => f());
  }, []);

  const sync = useCallback(async () => {
    await reloadScenes();
    await libRef.current.refreshAll();
    if (filterRef.current != null) await libRef.current.reload();
  }, [reloadScenes]);

  const guard = useCallback(
    async (fn: () => Promise<void>) => {
      try {
        await fn();
        await sync();
      } catch (e) {
        onError(e);
      }
    },
    [sync, onError],
  );

  const number = useCallback((id: number) => scenesRef.current.findIndex((s) => s.id === id) + 1, []);
  const byImage = useMemo(() => {
    const m = new Map<number, Scene>();
    scenes.forEach((s) => s.imageIds.forEach((i) => m.set(i, s)));
    return m;
  }, [scenes]);
  const sceneOfImage = useCallback((imageId: number | null) => (imageId == null ? undefined : byImage.get(imageId)), [byImage]);

  const detect = useCallback(async () => {
    setDetecting(true);
    setProgress({ task: "detect", done: 0, total: 0 });
    try {
      const found = await unwrap(commands.detectScenes(folderId, null));
      notify(found.length === 0 ? "No scenes found. Scenes need photos taken close together in similar light; make one from a selection with Scene > New scene from selection." : `Detected ${found.length} scene${found.length === 1 ? "" : "s"}`);
      await sync();
    } catch (e) {
      onError(e);
    } finally {
      setDetecting(false);
      setProgress(null);
    }
  }, [folderId, sync, notify, onError]);

  const createFromImages = useCallback(
    (imageIds: number[]) =>
      guard(async () => {
        if (imageIds.length === 0) return notify("Select photos first");
        await unwrap(commands.createScene(imageIds));
        notify(`Created a scene from ${imageIds.length} photo${imageIds.length === 1 ? "" : "s"}`);
      }),
    [guard, notify],
  );

  const mergeOf = useCallback(
    (imageIds: number[]) =>
      guard(async () => {
        const ids: number[] = [];
        for (const i of imageIds) {
          const s = byImage.get(i);
          if (s && !ids.includes(s.id)) ids.push(s.id);
        }
        if (ids.length < 2) return notify("Select photos from at least two scenes to merge");
        await unwrap(commands.mergeScenes(ids));
        notify(`Merged ${ids.length} scenes`);
      }),
    [guard, notify, byImage],
  );

  const splitAt = useCallback(
    (imageId: number | null) =>
      guard(async () => {
        const s = sceneOfImage(imageId);
        if (imageId == null || !s) return notify("The current photo is not in a scene");
        if (s.imageIds[0] === imageId) return notify("Cannot split before the first photo of a scene");
        await unwrap(commands.splitScene(s.id, imageId));
        notify("Scene split");
      }),
    [guard, notify, sceneOfImage],
  );

  const removeFromScene = useCallback(
    (imageIds: number[]) =>
      guard(async () => {
        const per = new Map<number, number[]>();
        for (const i of imageIds) {
          const s = byImage.get(i);
          if (s) per.set(s.id, [...(per.get(s.id) ?? []), i]);
        }
        if (per.size === 0) return notify("None of the selected photos is in a scene");
        for (const [sid, gone] of per) {
          const s = scenesRef.current.find((x) => x.id === sid)!;
          const rest = s.imageIds.filter((i) => !gone.includes(i));
          if (rest.length === 0) await unwrap(commands.deleteScene(sid));
          else await unwrap(commands.setSceneMembers(sid, rest));
        }
        notify("Removed from scene");
      }),
    [guard, notify, byImage],
  );

  const deleteScene = useCallback(
    (id: number) =>
      guard(async () => {
        await unwrap(commands.deleteScene(id));
        notify("Scene deleted");
      }),
    [guard, notify],
  );

  const toggleAnchor = useCallback(
    (imageId: number | null) =>
      guard(async () => {
        const s = sceneOfImage(imageId);
        if (imageId == null || !s) return notify("Add the photo to a scene before marking it as an anchor");
        let next: number[];
        if (s.anchorIds.includes(imageId)) {
          next = s.anchorIds.filter((a) => a !== imageId);
        } else {
          // Replace policy: at the limit the anchor marked longest ago is dropped.
          const order = (marked.current.get(s.id) ?? []).filter((a) => s.anchorIds.includes(a));
          const known = [...order, ...s.anchorIds.filter((a) => !order.includes(a))];
          next = [...known];
          if (next.length >= MAX_SCENE_ANCHORS) {
            const dropped = next.shift()!;
            notify(`A scene has at most ${MAX_SCENE_ANCHORS} anchors: replaced photo #${dropped}`);
          }
          next.push(imageId);
        }
        marked.current.set(s.id, next);
        await unwrap(commands.setSceneAnchors(s.id, next));
      }),
    [guard, notify, sceneOfImage],
  );

  return { scenes, number, sceneOfImage, progress, detecting, detect, createFromImages, mergeOf, splitAt, removeFromScene, deleteScene, toggleAnchor, sync };
}
