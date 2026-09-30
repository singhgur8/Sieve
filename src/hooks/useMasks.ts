// Masking state for the Develop module: panel/tool/selection state and every mask edit.
//
// Masks live inside the image's `ParametricAdjustments` (`editor.adj.masks`), so every edit goes through the editor's
// existing path: `edit` (live: one render in flight, draft size, history label) + `commit` on release, or `change`.
// AI mattes are computed with `computeAiMask` first (progress state `busy`), then inserted as a component.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  commands,
  newMaskComponent,
  newMaskGroup,
  newMaskId,
  unwrap,
  type AiMaskState,
  type AiTarget,
  type MaskBlendMode,
  type MaskCapabilities,
  type MaskComponent,
  type MaskGroup,
  type MaskShape,
  type NormPoint,
  type NormRect,
  type PersonPart,
} from "../ipc";
import type { Editor } from "./useEditor";
import {
  AI_FAMILY,
  DEFAULT_BRUSH,
  mapComponent,
  mapGroup,
  nextComponentName,
  OVERLAY_STYLES,
  shapeLabel,
  sizeToRadius,
  type BrushSettings,
  type CreateKind,
  type ToolKind,
} from "../lib/masks";

export interface ToolState {
  kind: ToolKind;
  /** Existing group the new component joins (`null` = a new group is created by the first gesture). */
  groupId: string | null;
  /** Existing component the gesture continues (brush strokes, colour samples). */
  compId: string | null;
  mode: MaskBlendMode;
}

export interface Target {
  groupId: string | null;
  mode: MaskBlendMode;
}

export interface Busy {
  label: string;
}

/** How long the overlay stays up after the last mask edit (then it fades out unless "Show overlay" is on). */
export const OVERLAY_HOLD_MS = 600;

let capsPromise: Promise<MaskCapabilities> | null = null;
const capsListeners = new Set<() => void>();
/** Drops the cached capabilities (models were installed) and makes every mounted panel refetch. */
export function invalidateMaskCapabilities() {
  capsPromise = null;
  capsListeners.forEach((l) => l());
}

export interface MasksApi {
  open: boolean;
  setOpen: (v: boolean) => void;
  groups: MaskGroup[];
  selGroup: string | null;
  selComp: string | null;
  select: (gid: string | null, cid?: string | null) => void;
  tool: ToolState | null;
  beginTool: (kind: ToolKind, target?: Target, compId?: string | null) => void;
  endTool: () => void;
  brush: BrushSettings;
  patchBrush: (p: Partial<BrushSettings>) => void;
  /** "Show overlay" (O): pinned on. */
  overlayOn: boolean;
  /** Pinned on, or an edit happened within the last 600 ms (brush, handle drag, any mask slider): the overlay is shown. */
  overlayVisible: boolean;
  toggleOverlay: () => void;
  overlayStyle: number;
  cycleOverlayStyle: () => void;
  setOverlayStyle: (i: number) => void;
  pins: boolean;
  togglePins: () => void;
  hover: { groupId: string; componentId: string | null } | null;
  setHover: (h: { groupId: string; componentId: string | null } | null) => void;
  caps: MaskCapabilities | null;
  /** Reason a create kind is unavailable, or null. */
  unavailable: (kind: CreateKind) => string | null;
  aiState: Record<string, AiMaskState>;
  busy: Busy | null;
  picker: Target | null;
  openPicker: (t?: Target) => void;
  closePicker: () => void;
  create: (kind: CreateKind, target?: Target) => void;
  createPeople: (referencePoint: NormPoint | null, parts: PersonPart[], name?: string) => Promise<void>;
  createObject: (region: NormRect) => Promise<void>;
  updateAll: () => Promise<void>;
  needsUpdate: number;
  // group / component operations (one history entry each)
  renameGroup: (gid: string, name: string) => void;
  deleteGroup: (gid: string) => void;
  duplicateGroup: (gid: string) => void;
  invertGroup: (gid: string) => void;
  toggleGroup: (gid: string) => void;
  patchComponent: (gid: string, cid: string, patch: Partial<MaskComponent>, label: string) => void;
  deleteComponent: (gid: string, cid: string) => void;
  deleteSelected: () => void;
  /** Moves a group / component so it ends at array index `to` (clamped); one history entry. */
  reorderGroup: (gid: string, to: number) => void;
  reorderComponent: (gid: string, cid: string, to: number) => void;
  moveSelected: (delta: number) => void;
  liveGroup: (gid: string, fn: (g: MaskGroup) => MaskGroup, label: string) => void;
  commit: () => void;
  changeGroup: (gid: string, fn: (g: MaskGroup) => MaskGroup, label: string) => void;
  // canvas gestures
  strokeStart: (dab: NormPoint, opts?: { erase?: boolean }) => void;
  strokeMove: (dab: NormPoint) => void;
  shapeStart: (kind: ToolKind, shape: MaskShape) => void;
  shapeUpdate: (shape: MaskShape) => void;
  editShape: (gid: string, cid: string, fn: (s: MaskShape) => MaskShape, label: string) => void;
  addColorSample: (sample: { point: NormPoint; area: NormRect | null }) => void;
  setLuminance: (fn: (s: Extract<MaskShape, { kind: "luminance" }>) => Extract<MaskShape, { kind: "luminance" }>) => void;
}

interface Opts {
  editor: Editor;
  id: number | null;
  onError: (e: unknown) => void;
  onNotice: (s: string) => void;
}

/** Appends `comp` to `gid`'s group, or creates a new group (id `newGid`, named after the component) when `gid` is null/unknown. */
function insert(m: MaskGroup[], gid: string | null, newGid: string, comp: MaskComponent): MaskGroup[] {
  if (gid && m.some((g) => g.id === gid)) return mapGroup(m, gid, (g) => ({ ...g, components: [...g.components, comp] }));
  return [...m, { ...newMaskGroup(comp.name, [comp]), id: newGid }];
}

const EMPTY_STATES: Record<string, AiMaskState> = {};

const clampIndex = (i: number, len: number) => Math.max(0, Math.min(len - 1, i));
/** `list` with the item at `from` moved so that it ends at index `to`. */
function moveItem<T>(list: T[], from: number, to: number): T[] {
  const out = list.slice();
  const [it] = out.splice(from, 1);
  out.splice(to, 0, it);
  return out;
}

export function useMasks({ editor, id, onError, onNotice }: Opts): MasksApi {
  const [open, setOpen] = useState(false);
  const [selGroup, setSelGroup] = useState<string | null>(null);
  const [selComp, setSelComp] = useState<string | null>(null);
  const [tool, setTool] = useState<ToolState | null>(null);
  const [brush, setBrush] = useState<BrushSettings>(DEFAULT_BRUSH);
  const [overlayOn, setOverlayOn] = useState(false);
  const [overlayStyle, setOverlayStyle] = useState(0);
  // Auto-show: any mask edit shows the overlay and keeps it up until OVERLAY_HOLD_MS after the last one.
  const [flash, setFlash] = useState(false);
  const flashTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const poke = useCallback(() => {
    setFlash(true);
    clearTimeout(flashTimer.current);
    flashTimer.current = setTimeout(() => setFlash(false), OVERLAY_HOLD_MS);
  }, []);
  useEffect(() => () => clearTimeout(flashTimer.current), []);
  const [pins, setPins] = useState(true);
  const [hover, setHover] = useState<{ groupId: string; componentId: string | null } | null>(null);
  const [caps, setCaps] = useState<MaskCapabilities | null>(null);
  const [aiState, setAiState] = useState<Record<string, AiMaskState>>(EMPTY_STATES);
  const [busy, setBusy] = useState<Busy | null>(null);
  const [picker, setPicker] = useState<Target | null>(null);

  const groups = editor.adj.masks;
  const idRef = useRef(id);
  idRef.current = id;
  const toolRef = useRef(tool);
  toolRef.current = tool;
  const brushRef = useRef(brush);
  brushRef.current = brush;
  const groupsRef = useRef(groups);
  groupsRef.current = groups;
  const selRef = useRef({ g: selGroup, c: selComp });
  selRef.current = { g: selGroup, c: selComp };
  const edRef = useRef(editor);
  edRef.current = editor;
  const gesture = useRef<{ gid: string; cid: string } | null>(null);

  // Capabilities once per session (and again after the AI models were downloaded).
  const [capsEpoch, setCapsEpoch] = useState(0);
  useEffect(() => {
    const bump = () => setCapsEpoch((n) => n + 1);
    capsListeners.add(bump);
    return () => void capsListeners.delete(bump);
  }, []);
  useEffect(() => {
    capsPromise ??= unwrap(commands.getMaskCapabilities());
    let stale = false;
    capsPromise.then((c) => !stale && setCaps(c)).catch((e) => {
      capsPromise = null;
      onError(e);
    });
    return () => {
      stale = true;
    };
  }, [onError, capsEpoch]);

  // New image: reset selection and tool.
  useEffect(() => {
    setSelGroup(null);
    setSelComp(null);
    setTool(null);
    setHover(null);
    setPicker(null);
    setBusy(null);
    setAiState(EMPTY_STATES);
    gesture.current = null;
  }, [id]);

  // Selection follows undo/redo/delete.
  useEffect(() => {
    if (selGroup && !groups.some((g) => g.id === selGroup)) {
      // After undo / delete the selection moves to the last remaining mask so Delete keeps working.
      setSelGroup(groups.length > 0 ? groups[groups.length - 1].id : null);
      setSelComp(null);
    } else if (selGroup && selComp && !groups.find((g) => g.id === selGroup)?.components.some((c) => c.id === selComp)) setSelComp(null);
  }, [groups, selGroup, selComp]);

  // AI status: refetch when the set of AI components / digests changes.
  const aiSig = useMemo(
    () =>
      groups
        .flatMap((g) => g.components.filter((c) => c.shape.kind === "ai").map((c) => `${c.id}:${c.shape.kind === "ai" ? (c.shape.digest ?? "") : ""}`))
        .join("|"),
    [groups],
  );
  useEffect(() => {
    if (id == null || !aiSig) {
      setAiState(EMPTY_STATES);
      return;
    }
    let stale = false;
    unwrap(commands.listMasks(id))
      .then((l) => {
        if (stale) return;
        setAiState(Object.fromEntries(l.ai.map((s) => [s.componentId, s.state])));
      })
      .catch(onError);
    return () => {
      stale = true;
    };
  }, [id, aiSig, onError]);

  const select = useCallback((gid: string | null, cid: string | null = null) => {
    setSelGroup(gid);
    setSelComp(cid);
  }, []);

  const endTool = useCallback(() => {
    setTool(null);
    gesture.current = null;
  }, []);

  const beginTool = useCallback((kind: ToolKind, target?: Target, compId: string | null = null) => {
    gesture.current = null;
    setTool({ kind, groupId: target?.groupId ?? null, compId, mode: target?.mode ?? "add" });
    if (!target?.groupId) {
      setSelGroup(null);
      setSelComp(null);
    }
  }, []);

  const patchBrush = useCallback((p: Partial<BrushSettings>) => setBrush((b) => ({ ...b, ...p })), []);

  const unavailable = useCallback(
    (kind: CreateKind): string | null => {
      const fam = AI_FAMILY[kind];
      if (!fam) return null;
      if (!caps) return "Checking which AI models are installed...";
      const c = caps.ai.find((x) => x.kind === fam);
      if (!c) return "This AI selection is not offered by the backend";
      return c.available ? null : (c.reason ?? "The model for this selection is not installed");
    },
    [caps],
  );

  // ---- editing primitives ----
  const edit = useCallback(
    (fn: (m: MaskGroup[]) => MaskGroup[], label: string) => {
      poke();
      edRef.current.edit((a) => ({ ...a, masks: fn(a.masks) }), label);
    },
    [poke],
  );
  const change = useCallback(
    (fn: (m: MaskGroup[]) => MaskGroup[], label: string) => {
      poke();
      edRef.current.change((a) => ({ ...a, masks: fn(a.masks) }), label);
    },
    [poke],
  );
  const commit = useCallback(() => {
    poke();
    edRef.current.commit();
  }, [poke]);

  const addComponent = useCallback(
    (target: Target | undefined, shape: MaskShape, baseName: string, label: string) => {
      const name = nextComponentName(groupsRef.current, baseName);
      const comp = { ...newMaskComponent(shape, name), mode: target?.groupId ? target.mode : "add" };
      const newGid = newMaskId();
      change((m) => insert(m, target?.groupId ?? null, newGid, comp), label);
      const gid = target?.groupId && groupsRef.current.some((g) => g.id === target.groupId) ? target.groupId : newGid;
      setSelGroup(gid);
      setSelComp(comp.id);
      return { gid, cid: comp.id };
    },
    [change],
  );

  // ---- AI ----
  const computeAndAdd = useCallback(
    async (aiTarget: AiTarget, referencePoint: NormPoint | null, target: Target | undefined, baseName: string, busyLabel: string) => {
      const cur = idRef.current;
      if (cur == null) return;
      setBusy({ label: busyLabel });
      try {
        await edRef.current.flush();
        const info = await unwrap(commands.computeAiMask(cur, { target: aiTarget, referencePoint, force: false }));
        if (idRef.current !== cur) return;
        addComponent(target, { kind: "ai", target: aiTarget, referencePoint, digest: info.digest }, baseName, `Mask: ${baseName}`);
        if (info.coverage === 0) onNotice(`No ${baseName.toLowerCase()} found in this photo`);
      } catch (e) {
        onError(e);
      } finally {
        setBusy((b) => (idRef.current === cur ? null : b));
      }
    },
    [addComponent, onError, onNotice],
  );

  const createPeople = useCallback(
    async (referencePoint: NormPoint | null, parts: PersonPart[], name = "Person") => {
      const target = picker ?? undefined;
      setPicker(null);
      await computeAndAdd({ kind: "people", parts }, referencePoint, target, name, "Finding people...");
    },
    [computeAndAdd, picker],
  );

  const createObject = useCallback(
    async (region: NormRect) => {
      const t = toolRef.current;
      const target = t ? { groupId: t.groupId, mode: t.mode } : undefined;
      setTool(null);
      await computeAndAdd({ kind: "object", region }, null, target, "Object", "Selecting object...");
    },
    [computeAndAdd],
  );

  const create = useCallback(
    (kind: CreateKind, target?: Target) => {
      if (unavailable(kind)) return;
      switch (kind) {
        case "subject":
          return void computeAndAdd({ kind: "subject" }, null, target, "Subject", "Finding subject...");
        case "sky":
          return void computeAndAdd({ kind: "sky" }, null, target, "Sky", "Finding sky...");
        case "background":
          return void computeAndAdd({ kind: "background" }, null, target, "Background", "Finding background...");
        case "people":
          return setPicker(target ?? { groupId: null, mode: "add" });
        default:
          return beginTool(kind, target);
      }
    },
    [unavailable, computeAndAdd, beginTool],
  );

  const needsUpdate = useMemo(() => Object.values(aiState).filter((s) => s === "needs_update").length, [aiState]);

  const updateAll = useCallback(async () => {
    const cur = idRef.current;
    if (cur == null) return;
    const todo = groupsRef.current.flatMap((g) => g.components.filter((c) => c.shape.kind === "ai" && !c.shape.digest).map((c) => c));
    if (!todo.length) return;
    setBusy({ label: `Updating ${todo.length} AI mask${todo.length === 1 ? "" : "s"}...` });
    try {
      await edRef.current.flush();
      const digests = new Map<string, string>();
      for (const c of todo) {
        if (c.shape.kind !== "ai") continue;
        const info = await unwrap(commands.computeAiMask(cur, { target: c.shape.target, referencePoint: c.shape.referencePoint, force: false }));
        digests.set(c.id, info.digest);
      }
      if (idRef.current !== cur) return;
      change((m) => m.map((g) => ({ ...g, components: g.components.map((c) => (c.shape.kind === "ai" && digests.has(c.id) ? { ...c, shape: { ...c.shape, digest: digests.get(c.id)! } } : c)) })), "Mask: Update AI masks");
    } catch (e) {
      onError(e);
    } finally {
      setBusy((b) => (idRef.current === cur ? null : b));
    }
  }, [change, onError]);

  // ---- group / component operations ----
  const renameGroup = useCallback((gid: string, name: string) => change((m) => mapGroup(m, gid, (g) => ({ ...g, name: name.slice(0, 64) })), "Mask: Rename"), [change]);
  const deleteGroup = useCallback(
    (gid: string) => {
      const g = groupsRef.current.find((x) => x.id === gid);
      change((m) => m.filter((x) => x.id !== gid), `Mask: Delete ${g?.name ?? "mask"}`);
    },
    [change],
  );
  const duplicateGroup = useCallback(
    (gid: string) => {
      const g = groupsRef.current.find((x) => x.id === gid);
      if (!g) return;
      const copy: MaskGroup = { ...structuredClone(g), id: newMaskId(), name: `${g.name} copy`.slice(0, 64) };
      copy.components = copy.components.map((c) => ({ ...c, id: newMaskId() }));
      change((m) => [...m, copy], `Mask: Duplicate ${g.name}`);
      setSelGroup(copy.id);
      setSelComp(null);
    },
    [change],
  );
  const invertGroup = useCallback(
    (gid: string) => change((m) => mapGroup(m, gid, (g) => ({ ...g, components: g.components.map((c) => ({ ...c, inverted: !c.inverted })) })), "Mask: Invert"),
    [change],
  );
  const toggleGroup = useCallback(
    (gid: string) => change((m) => mapGroup(m, gid, (g) => ({ ...g, active: !g.active })), "Mask: Visibility"),
    [change],
  );
  const patchComponent = useCallback(
    (gid: string, cid: string, patch: Partial<MaskComponent>, label: string) => change((m) => mapComponent(m, gid, cid, (c) => ({ ...c, ...patch })), label),
    [change],
  );
  const deleteComponent = useCallback(
    (gid: string, cid: string) =>
      change((m) => m.map((g) => (g.id === gid ? { ...g, components: g.components.filter((c) => c.id !== cid) } : g)).filter((g) => g.components.length > 0 || g.id !== gid), "Mask: Delete component"),
    [change],
  );
  const deleteSelected = useCallback(() => {
    const { g, c } = selRef.current;
    if (!g) return;
    const grp = groupsRef.current.find((x) => x.id === g);
    if (c && grp && grp.components.length > 1) deleteComponent(g, c);
    else deleteGroup(g);
  }, [deleteComponent, deleteGroup]);

  // Order is array order in `adjustments.masks` / `components` (later groups apply after earlier ones).
  const reorderGroup = useCallback(
    (gid: string, to: number) => {
      const from = groupsRef.current.findIndex((x) => x.id === gid);
      if (from < 0) return;
      const dest = clampIndex(to, groupsRef.current.length);
      if (dest === from) return;
      change((m) => moveItem(m, from, dest), `Mask: Reorder ${groupsRef.current[from].name}`);
    },
    [change],
  );
  const reorderComponent = useCallback(
    (gid: string, cid: string, to: number) => {
      const g = groupsRef.current.find((x) => x.id === gid);
      const from = g ? g.components.findIndex((c) => c.id === cid) : -1;
      if (!g || from < 0) return;
      const dest = clampIndex(to, g.components.length);
      if (dest === from) return;
      change((m) => mapGroup(m, gid, (x) => ({ ...x, components: moveItem(x.components, from, dest) })), "Mask: Reorder component");
    },
    [change],
  );
  /** Alt+Up / Alt+Down: moves the selected component within its group, else the selected group in the stack. */
  const moveSelected = useCallback(
    (delta: number) => {
      const { g, c } = selRef.current;
      if (!g) return;
      const grp = groupsRef.current.find((x) => x.id === g);
      if (!grp) return;
      if (c && grp.components.length > 1) reorderComponent(g, c, grp.components.findIndex((x) => x.id === c) + delta);
      else reorderGroup(g, groupsRef.current.findIndex((x) => x.id === g) + delta);
    },
    [reorderComponent, reorderGroup],
  );

  const liveGroup = useCallback((gid: string, fn: (g: MaskGroup) => MaskGroup, label: string) => edit((m) => mapGroup(m, gid, fn), label), [edit]);
  const changeGroup = useCallback((gid: string, fn: (g: MaskGroup) => MaskGroup, label: string) => change((m) => mapGroup(m, gid, fn), label), [change]);

  // ---- canvas gestures ----
  const strokeStart = useCallback(
    (dab: NormPoint, opts?: { erase?: boolean }) => {
      const t = toolRef.current;
      if (!t || t.kind !== "brush") return;
      const b = brushRef.current;
      const stroke = {
        radius: sizeToRadius(b.size),
        flow: b.flow / 100,
        feather: b.feather / 100,
        density: b.density / 100,
        erase: b.erase !== !!opts?.erase,
        autoMask: b.autoMask,
        dabs: [dab],
      };
      const label = "Mask: Brush";
      const existing = t.groupId && t.compId && groupsRef.current.find((g) => g.id === t.groupId)?.components.some((c) => c.id === t.compId);
      if (existing) {
        gesture.current = { gid: t.groupId!, cid: t.compId! };
        edit((m) => mapComponent(m, t.groupId!, t.compId!, (c) => (c.shape.kind === "brush" ? { ...c, shape: { ...c.shape, strokes: [...c.shape.strokes, stroke] } } : c)), label);
        return;
      }
      const name = nextComponentName(groupsRef.current, "Brush");
      const comp = { ...newMaskComponent({ kind: "brush", strokes: [stroke] }, name), mode: t.groupId ? t.mode : ("add" as MaskBlendMode) };
      const newGid = newMaskId();
      gesture.current = { gid: t.groupId && groupsRef.current.some((g) => g.id === t.groupId) ? t.groupId : newGid, cid: comp.id };
      edit((m) => insert(m, t.groupId, newGid, comp), label);
      setSelGroup(gesture.current.gid);
      setSelComp(comp.id);
      setTool({ ...t, groupId: gesture.current.gid, compId: comp.id });
    },
    [edit],
  );

  const strokeMove = useCallback(
    (dab: NormPoint) => {
      const g = gesture.current;
      if (!g) return;
      edit(
        (m) =>
          mapComponent(m, g.gid, g.cid, (c) => {
            if (c.shape.kind !== "brush" || c.shape.strokes.length === 0) return c;
            const strokes = c.shape.strokes.slice();
            const last = strokes[strokes.length - 1];
            strokes[strokes.length - 1] = { ...last, dabs: [...last.dabs, dab] };
            return { ...c, shape: { ...c.shape, strokes } };
          }),
        "Mask: Brush",
      );
    },
    [edit],
  );

  const shapeStart = useCallback(
    (kind: ToolKind, shape: MaskShape) => {
      const t = toolRef.current;
      const base = kind === "linear" ? "Linear Gradient" : kind === "radial" ? "Radial Gradient" : shapeLabel(shape);
      const name = nextComponentName(groupsRef.current, base);
      const comp = { ...newMaskComponent(shape, name), mode: t?.groupId ? t.mode : ("add" as MaskBlendMode) };
      const newGid = newMaskId();
      const gid = t?.groupId && groupsRef.current.some((g) => g.id === t.groupId) ? t.groupId : newGid;
      gesture.current = { gid, cid: comp.id };
      edit((m) => insert(m, t?.groupId ?? null, newGid, comp), `Mask: ${base}`);
      setSelGroup(gid);
      setSelComp(comp.id);
    },
    [edit],
  );

  const shapeUpdate = useCallback(
    (shape: MaskShape) => {
      const g = gesture.current;
      if (!g) return;
      const label = `Mask: ${shapeLabel(shape)}`;
      edit((m) => mapComponent(m, g.gid, g.cid, (c) => ({ ...c, shape })), label);
    },
    [edit],
  );

  const editShape = useCallback(
    (gid: string, cid: string, fn: (s: MaskShape) => MaskShape, label: string) => edit((m) => mapComponent(m, gid, cid, (c) => ({ ...c, shape: fn(c.shape) })), label),
    [edit],
  );

  const addColorSample = useCallback(
    (sample: { point: NormPoint; area: NormRect | null }) => {
      const t = toolRef.current;
      if (!t || t.kind !== "color") return;
      const s = { ...sample, lightroomModel: null };
      const cid = t.compId;
      const gid = t.groupId;
      if (gid && cid && groupsRef.current.find((g) => g.id === gid)?.components.some((c) => c.id === cid)) {
        change((m) => mapComponent(m, gid, cid, (c) => (c.shape.kind === "color" ? { ...c, shape: { ...c.shape, samples: [...c.shape.samples, s].slice(-5) } } : c)), "Mask: Color Range");
        return;
      }
      const r = addComponent(t.groupId ? { groupId: t.groupId, mode: t.mode } : undefined, { kind: "color", samples: [s], amount: 50 }, "Color Range", "Mask: Color Range");
      setTool({ ...t, groupId: r.gid, compId: r.cid });
    },
    [addComponent, change],
  );

  const setLuminance = useCallback(
    (fn: (s: Extract<MaskShape, { kind: "luminance" }>) => Extract<MaskShape, { kind: "luminance" }>) => {
      const t = toolRef.current;
      const { g, c } = selRef.current;
      const cur = g && c ? groupsRef.current.find((x) => x.id === g)?.components.find((x) => x.id === c) : null;
      if (cur && cur.shape.kind === "luminance") {
        const gid = g!;
        const cid = c!;
        change((m) => mapComponent(m, gid, cid, (x) => (x.shape.kind === "luminance" ? { ...x, shape: fn(x.shape) } : x)), "Mask: Luminance Range");
        return;
      }
      const shape = fn({ kind: "luminance", featherLow: 0, low: 0, high: 1, featherHigh: 1, smoothness: 50 });
      const r = addComponent(t?.groupId ? { groupId: t.groupId, mode: t.mode } : undefined, shape, "Luminance Range", "Mask: Luminance Range");
      if (t) setTool({ ...t, groupId: r.gid, compId: r.cid });
    },
    [addComponent, change],
  );

  return {
    open,
    setOpen,
    groups,
    selGroup,
    selComp,
    select,
    tool,
    beginTool,
    endTool,
    brush,
    patchBrush,
    overlayOn,
    overlayVisible: overlayOn || flash,
    toggleOverlay: () => setOverlayOn((v) => !v),
    overlayStyle,
    cycleOverlayStyle: () => setOverlayStyle((i) => (i + 1) % OVERLAY_STYLES.length),
    setOverlayStyle,
    pins,
    togglePins: () => setPins((v) => !v),
    hover,
    setHover,
    caps,
    unavailable,
    aiState,
    busy,
    picker,
    openPicker: (t) => setPicker(t ?? { groupId: null, mode: "add" }),
    closePicker: () => setPicker(null),
    create,
    createPeople,
    createObject,
    updateAll,
    needsUpdate,
    renameGroup,
    deleteGroup,
    duplicateGroup,
    invertGroup,
    toggleGroup,
    patchComponent,
    deleteComponent,
    deleteSelected,
    reorderGroup,
    reorderComponent,
    moveSelected,
    liveGroup,
    commit,
    changeGroup,
    strokeStart,
    strokeMove,
    shapeStart,
    shapeUpdate,
    editShape,
    addColorSample,
    setLuminance,
  };
}
