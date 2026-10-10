// Develop module: filmstrip + viewer (before/after, split, 100% detail) + presets/history + adjustment sliders.
import { forwardRef, useCallback, useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ChevronLeft, ChevronRight, Columns2, Columns3, Flag, SplitSquareHorizontal, X } from "lucide-react";
import { usePrefetchNeighbours } from "../../hooks/usePrefetch";
import { applyAutoTone, commands, convertFileSrc, unwrap, type AdjustmentField, type ColorLabel, type FaceInfo, type ImportStyleReport, type NormRect, type ParametricAdjustments, type StyleGroup, type StylePreset, type SyncDeltaResult } from "../../ipc";
import type { Library } from "../../hooks/useLibrary";
import type { SelectionApi } from "../../hooks/useSelection";
import { useEditor, type Editor } from "../../hooks/useEditor";
import { clearFileHealth, useEntryHealth } from "../../lib/errors";
import { OriginalUnavailable } from "./OriginalUnavailable";
import { Stars } from "../Cell";
import { Menu, menuItem } from "../Menu";
import { CompareBar, CompareTag } from "../CompareBar";
import type { CompareState } from "../LoupeLayer";
import { Filmstrip } from "../Filmstrip";
import { setPanelHidden, usePanels } from "../../lib/panels";
import { dispToSensor, screenToDisp } from "../../lib/maskGeom";
import { hint, type ActionId } from "../../lib/keymap";
import { useMasks } from "../../hooks/useMasks";
import { MasksPanel } from "./MasksPanel";
import { MaskLayer } from "./MaskLayer";
import { PeoplePicker } from "./PeoplePicker";
import type { Frame } from "../../lib/maskGeom";
import { formatShutter, LABEL_COLOR, trimNum } from "../../lib/format";
import { getClipboard, setClipboard, useClipboard } from "../../lib/clipboard";
import { registerFlush } from "../../lib/editFlush";
import { AdjustPanel, type AutoApi } from "./AdjustPanel";
import { useHoverPreview, useStyleLibrary } from "../../hooks/useDevelopV14";
import { Dialog } from "../Dialog";
import { LeftPanel } from "./LeftPanel";
import { SettingsFieldsDialog } from "./SettingsFieldsDialog";
import { COPY_FIELDS_KEY, modifiedFields, PRESET_FIELDS_KEY, rememberedCopyFields } from "../../lib/fieldGroups";
import { setPreviousPhoto, getPreviousPhoto, usePreviousPhoto } from "../../lib/previousPhoto";
import { CropOverlay, cropChanged, constrainTool, newTool, refit, resetTool, swapTool, toggleLockTool, type CropTool, type Quad } from "./CropOverlay";
import { CropBar, type CropApi } from "./CropPanel";
import { GuideOverlay } from "./GuideOverlay";
import { useUpright } from "../../hooks/useUpright";
import { cropAngleForRotation } from "../../lib/transform";
import { WarningsChip } from "./WarningsChip";
import { FULL, fromStored, isFull, loadCropAspect, loadOverlay, nextOverlay, previewRotation, saveOverlay, toStored } from "../../lib/crop";
import { getLastZoomIn, rememberZoom, ZOOM_PRESETS, type ZoomPreset } from "../../lib/zoom";
import { Viewer, frameBox, visibleRegion, type Size, type Zoom } from "./Viewer";
import { developPlaceholder } from "../../lib/entryImage";

export interface DevelopHandle {
  toggleBefore: () => void;
  toggleZoom: () => void;
  copy: () => void;
  paste: () => void;
  /** Sync… dialog, or (quiet) sync with the remembered fields straight away. */
  sync: (quiet?: boolean) => void;
  /** Cmd+Alt+Shift+A: Auto Sync on / off. */
  toggleAutoSync: () => void;
  reset: () => void;
  toggleSplit: () => void;
  /** R: start the crop tool, or apply it when already active. */
  toggleCrop: () => void;
  /** Enter: apply the crop (no-op when the tool is inactive). */
  commitCrop: () => void;
  /** Leaving the crop tool any way but Esc / Cancel commits it (Lightroom); resolves after the saves landed. A photo change within Develop re-opens the tool on the next photo. */
  commitPendingTool: (stay?: boolean) => Promise<void>;
  /** Esc: discard the crop tool; true when it was active (so the caller does not also leave Develop). */
  cancelCrop: () => boolean;
  undo: () => void;
  redo: () => void;
  /** Masking shortcuts (Shift+W toggles the panel; the tool keys act only while it is open). */
  maskKey: (action: ActionId, e: KeyboardEvent) => void;
  /** Esc: finish the active mask tool; true when one was active. */
  cancelMaskTool: () => boolean;
  /** Esc in Develop: cancel crop / picker, end the mask tool, deselect the mask, close the Masks panel; never leaves Develop. */
  escape: () => void;
  isCropping: () => boolean;
  /** First Cmd+Z with uncommitted crop tool changes reverts the tool; true when it did. */
  revertTool: (newest?: number) => boolean;
  cropSwap: () => void;
  cropLock: () => void;
  /** O: next crop guide overlay; Shift+O: rotate it; Cmd+Alt+R: reset the crop tool. */
  cropOverlay: () => void;
  cropOverlayRotate: () => void;
  cropReset: () => void;
  /** Shift+T: toggle the Guided Upright tool. */
  toggleGuided: () => void;
  /** Time (ms) of the adjustment Cmd+Z would undo (0 = none) and whether an adjustment redo exists. */
  lastCommitAt: () => number;
  canRedo: () => boolean;
  toggleBw: () => void;
  togglePicker: () => void;
  faceZoom: (dir: 1 | -1) => void;
  pastePrevious: () => void;
  savePreset: () => void;
  /** Cmd+U / Cmd+Shift+U: Lightroom Auto tone / Auto white balance for the active photo. */
  autoTone: () => void;
  autoWb: () => void;
}

type Dialog = { kind: "copy" | "sync" | "preset" } | null;

interface Props {
  /** Auto Sync (Lightroom): while on, every committed edit goes to the other selected photos (state lives in App, session-long). */
  autoSync?: boolean;
  /** A crop or mask tool is active (the app moves toasts away from the handles). */
  onToolActive?: (on: boolean) => void;
  onAutoSync?: (on: boolean) => void;
  lib: Library;
  sel: SelectionApi;
  onError: (e: unknown) => void;
  onNotice: (s: string) => void;
  /** Toast with an Undo action (multi-photo reset / preset). */
  onUndoToast: (msg: string, undo: () => void) => void;
  /** Paste / sync result (v19 `EditBatchResult`): the workflow refreshes and shows the one-step Undo toast. */
  onBatch: (r: { batchId: number | null; label: string; changedIds: number[] }, text: string, attempted: number, soft?: boolean) => Promise<boolean>;
  /** Photos whose history was just written by the user (commit, undo, batch edit): scene batch Undo offers must not outlive it. */
  onCommitted?: (ids: number[]) => void;
  onBack: () => void;
  /** "Locate folder…" for the folder of image `imageId` (IPC v13 relocate_folder). */
  onLocate: (imageId: number) => void;
  /** Compare view (two panes side by side). `focus` is the active pane: the sliders, crop and masks edit it. */
  compare?: CompareState | null;
  onFocusPane?: (k: "a" | "b") => void;
  /** Filmstrip click in Compare: choose the Candidate. */
  onCandidate?: (id: number) => void;
  onSwap?: () => void;
  onMakeSelect?: () => void;
  onToggleCompare?: () => void;
  /** Click on a star: rate that photo (0 clears). */
  onRate?: (id: number, rating: number) => void;
  /** Pick / reject button on the viewer toolbar (clicking the current flag clears it). */
  onFlag?: (id: number, flag: "pick" | "reject") => void;
  /** Color label menu on the viewer toolbar (null clears). */
  onLabel?: (id: number, label: ColorLabel | null) => void;
  /** Filter summary shown in the filmstrip header (Develop has no separate filter row). */
  filterSummary?: { text: string; onEdit: () => void };
  /** Edit step: the context bar shown above the workspace. */
  topSlot?: React.ReactNode;
  /** Edit step: per-cell filmstrip markers (representative ring, applied check, needs-a-look). */
  filmBadge?: (id: number) => React.ReactNode;
  /** Edit step: "This scene only" chip in the filmstrip header. */
  sceneOnly?: { on: boolean; toggle: () => void };
}

const COLOR_LABELS: ColorLabel[] = ["red", "yellow", "green", "blue", "purple"];

/** "ISO 800 · 85 mm · f/1.8 · 1/250 s" for the histogram info line. */
function exifLine(c: { iso: number | null; shutterSeconds: number | null; aperture: number | null; focalLengthMm: number | null } | undefined): string | null {
  if (!c) return null;
  const parts = [c.iso != null ? `ISO ${c.iso}` : null, c.focalLengthMm != null ? `${trimNum(c.focalLengthMm)} mm` : null, c.aperture != null ? `f/${trimNum(c.aperture)}` : null, c.shutterSeconds != null ? formatShutter(c.shutterSeconds) : null].filter(Boolean);
  return parts.length ? parts.join(" · ") : null;
}

const stem = (name: string | undefined) => (name ?? "").replace(/\.[^.]+$/, "");

const FILM = 72;
/** Margin around the image while cropping, so the handles do not sit on the panel borders. */
const CROP_INSET = 24;

const TOOL_HELP: Record<string, string> = {
  brush: "Brush: drag to paint, Alt erases, [ ] size, Esc done",
  linear: "Linear gradient: drag from the strong side to the weak side",
  radial: "Radial gradient: drag from the centre outwards",
  color: "Color range: click or drag to sample colors, Esc done",
  luminance: "Luminance range: click to sample a brightness",
  object: "Objects: drag a rectangle around the object",
};

/** >= 1600 px: bigger filmstrip cells and wider panels (Tailwind's min-[1600px]). */
function useWide(): boolean {
  const q = typeof window === "undefined" ? null : window.matchMedia("(min-width: 1600px)");
  const [wide, setWide] = useState(q?.matches ?? false);
  useEffect(() => {
    if (!q) return;
    const on = () => setWide(q.matches);
    q.addEventListener("change", on);
    return () => q.removeEventListener("change", on);
  }, [q]);
  return wide;
}

export const DevelopView = forwardRef<DevelopHandle, Props>(function DevelopView({ autoSync = false, onToolActive, onAutoSync, onCommitted, lib, sel, onError, onNotice, onUndoToast, onBatch, onLocate, compare = null, onFocusPane, onCandidate, onSwap, onMakeSelect, onToggleCompare, onRate, onFlag, onLabel, filterSummary, topSlot, filmBadge, sceneOnly }, ref) {
  const id = compare ? compare[compare.focus] : sel.active;
  const [size, setSize] = useState<Size>({ w: 0, h: 0 });
  const [zoom, setZoom] = useState<Zoom>({ on: false, cx: 0.5, cy: 0.5 });
  const [region, setRegion] = useState<NormRect | null>(null);
  const [showBefore, setShowBefore] = useState(false);
  const [split, setSplit] = useState(false);
  const [splitPos, setSplitPos] = useState(0.5);
  const styles = useStyleLibrary(onError);
  const [importReport, setImportReport] = useState<ImportStyleReport | null>(null);
  const [autoBusy, setAutoBusy] = useState(false);
  const [autoWb, setAutoWb] = useState<{ id: number; t: number; tint: number } | null>(null);
  const [dialog, setDialog] = useState<Dialog>(null);
  const [cropTool, setCropTool] = useState<CropTool | null>(null);
  const cropRef = useRef<CropTool | null>(null);
  cropRef.current = cropTool;
  // Guided Upright tool (Shift+T): the photo is shown without its transform and guide lines are drawn on it.
  const [guideOn, setGuideOn] = useState(false);
  const guideRef = useRef(false);
  guideRef.current = guideOn;
  const [browsing, setBrowsing] = useState(false);
  const [picking, setPicking] = useState(false);
  const pickerRef = useRef(false);
  pickerRef.current = picking;
  const panels = usePanels("develop");
  const copied = useClipboard();
  const wide = useWide();
  const dpr = typeof window === "undefined" ? 1 : window.devicePixelRatio || 1;

  const { refresh } = lib;
  const onChanged = useCallback((changed: number) => void refresh([changed]).catch(() => {}), [refresh]);
  const onCommittedRef = useRef(onCommitted);
  onCommittedRef.current = onCommitted;
  const noteCommitted = useCallback((i: number) => onCommittedRef.current?.([i]), []);
  const entry = id != null ? lib.getEntry(id) : undefined;
  // Compare: one editor per pane (A = Select, B = Candidate); `editor` is the active pane's, so every panel edits it.
  const focusB = !!compare && compare.focus === "b";
  const idA = compare ? compare.a : id;
  const idB = compare ? compare.b : null;
  const maxEdge = Math.ceil(Math.max(size.w, size.h) * dpr);
  const beforeOn = (showBefore || split) && !compare;
  // Auto Sync hook of editor A (filled in below, once the selection helpers exist).
  const syncedRef = useRef<(id: number, targets: number[], r: SyncDeltaResult) => Promise<void>>(async () => {});
  const editorA = useEditor(idA, {
    autoSyncTargets: () => (autoOnRef.current ? syncTargetsRef.current : null),
    onSynced: (i, t, r) => syncedRef.current(i, t, r),
    format: (idA != null ? lib.getEntry(idA) : undefined)?.format,
    uncropped: (cropTool !== null || guideOn) && !focusB,
    untransformed: guideOn && !focusB,
    maxEdge,
    region,
    wantBefore: beforeOn && !focusB,
    onError,
    onChanged,
    onCommitted: noteCommitted,
  });
  const editorB = useEditor(idB, {
    format: (idB != null ? lib.getEntry(idB) : undefined)?.format,
    uncropped: (cropTool !== null || guideOn) && focusB,
    untransformed: guideOn && focusB,
    maxEdge,
    region,
    wantBefore: false,
    onError,
    onChanged,
    onCommitted: noteCommitted,
  });
  const editor = focusB ? editorB : editorA;
  const onBatchRef = useRef(onBatch);
  onBatchRef.current = onBatch;
  const editorRef = useRef(editor);
  editorRef.current = editor;
  const { info } = editor;
  // Apply to scene / Match scene read the stored settings: they wait for these saves (lib/editFlush).
  const flushA = editorA.flush;
  const flushB = editorB.flush;
  useEffect(
    () =>
      registerFlush(async () => {
        await flushA();
        await flushB();
      }),
    [flushA, flushB],
  );
  const health = useEntryHealth(entry);
  // The original became reachable again (relocated folder): load the image that failed to open.
  const hadHealth = useRef(false);
  useEffect(() => {
    if (health) hadHealth.current = true;
    else if (hadHealth.current) {
      hadHealth.current = false;
      if (!editor.main) void editor.reload().catch(onError);
    }
  });
  const fw = info?.fullWidth ?? 0;
  const fh = info?.fullHeight ?? 0;
  const masks = useMasks({ editor, id, onError, onNotice });
  const toolActive = cropTool !== null || (masks.open && !!masks.tool);
  useEffect(() => {
    onToolActive?.(toolActive);
    return () => onToolActive?.(false);
  }, [toolActive]); // eslint-disable-line react-hooks/exhaustive-deps
  const masksRef = useRef(masks);
  masksRef.current = masks;

  // Region of the frame visible at 100%: committed immediately on toggle/resize, debounced after a pan.
  const zs = zoom.s ?? 1;
  const latest = useRef({ zoom, size, fw, fh, main: editor.main });
  latest.current = { zoom, size, fw, fh, main: editor.main };
  const commitRegion = useCallback(() => {
    const l = latest.current;
    const k = l.zoom.s ?? 1;
    setRegion(visibleRegion(l.zoom, l.size, l.fw * k, l.fh * k));
  }, []);
  useEffect(() => {
    commitRegion();
  }, [zoom.on, zs, size, fw, fh, id, commitRegion]);
  const panTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const onPanEnd = useCallback(() => {
    clearTimeout(panTimer.current);
    panTimer.current = setTimeout(commitRegion, 120);
  }, [commitRegion]);
  useEffect(() => () => clearTimeout(panTimer.current), []);
  // Zoom level and position persist while stepping through photos (Lightroom); the crop tool and Esc still go back to Fit.

  // ---- crop tool ----
  // The tool (aspect lock, overlay) that was committed by a photo change: it re-opens on the next photo once that is loaded.
  const carryRef = useRef<CropTool | null>(null);
  const [reopen, setReopen] = useState<CropTool | null>(null);
  useEffect(() => {
    setCropTool(null);
    setGuideOn(false);
    setCropMsg(null);
    // a tool that was open (untouched, re-opened between two quick steps) is carried on too
    const carried = carryRef.current ?? (cropRef.current && !cropRef.current.dirty ? cropRef.current : null); // read now: the updater runs later
    setReopen((prev) => carried ?? prev); // R3-P2-2: a pending re-open survives a quick second step
    carryRef.current = null;
  }, [id]);
  const upright = useUpright(editor, id, onError);
  const [cropMsg, setCropMsg] = useState<string | null>(null);
  const [cropBusy, setCropBusy] = useState(false);
  const adjRef = useRef(editor.adj);
  adjRef.current = editor.adj;
  const orientation = entry?.orientation ?? 1;
  const imageAspect = editor.main && editor.main.uncropped && editor.main.height > 0 ? editor.main.width / editor.main.height : 0;
  // Oriented, uncropped aspect (w / h) of the photo: what crop rectangles are fractions of.
  const frameAspect = info && info.fullWidth > 0 && info.fullHeight > 0 ? info.fullWidth / info.fullHeight : imageAspect || 1.5;
  const frameAspectRef = useRef(frameAspect);
  frameAspectRef.current = frameAspect;
  const orientationRef = useRef(orientation);
  orientationRef.current = orientation;
  /** Every crop tool change goes through here: the rectangle stays inside the straightened image. */
  const changeCrop = useCallback(
    (t: CropTool | ((prev: CropTool) => CropTool)) =>
      setCropTool((prev) => {
        const next = typeof t === "function" ? (prev ? t(prev) : prev) : t;
        if (!next) return next;
        const dirty = !!(prev?.dirty || (prev && cropChanged(prev, next)) || next.dirty);
        return { ...constrainTool(next, frameAspectRef.current, orientationRef.current, quadRef.current), dirty };
      }),
    [],
  );
  // Warped image outline (v19.3 get_transform_bounds) while the crop tool is open: "Constrain to image" keeps the rectangle inside it.
  const [quad, setQuad] = useState<Quad | null>(null);
  const quadRef = useRef<Quad | null>(null);
  quadRef.current = quad;
  const cropOpen = cropTool !== null;
  const transformKey = JSON.stringify(editor.adj.transform);
  const adjLive = useRef(editor.adj);
  adjLive.current = editor.adj;
  useEffect(() => {
    if (!cropOpen || id == null) {
      setQuad(null);
      return;
    }
    let live = true;
    const timer = setTimeout(() => {
      unwrap(commands.getTransformBounds(id, adjLive.current))
        .then((b) => {
          if (!live) return;
          quadRef.current = b.validQuad;
          setQuad(b.validQuad);
          changeCrop((t) => t); // re-fit the rectangle to the new outline
        })
        .catch(() => {});
    }, 150); // debounced: Transform sliders may be dragged while the tool is open
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [cropOpen, id, transformKey, changeCrop]);
  /** JSON of the stored crop the open tool was seeded from: a different stored crop means a change from outside the tool (R2-1). */
  const toolBaseline = useRef("");
  const panelScroll = useRef<{ el: Element; top: number } | null>(null);
  /** R3-P2-3: when the tool first became dirty (0 = clean). */
  const dirtyAt = useRef(0);
  const toolDirty = !!cropTool?.dirty;
  useEffect(() => {
    dirtyAt.current = toolDirty ? Date.now() : 0;
  }, [toolDirty]);
  const startCrop = useCallback(async (carry?: CropTool | null) => {
    if (id == null) return;
    // R3-P2-1: capture the panel scroll before the crop panel is inserted (user-opened tool only).
    if (!carry && !cropRef.current && !panelScroll.current) {
      const sc = document.querySelector<HTMLElement>('[data-testid="adjust-scroll"]');
      if (sc) panelScroll.current = { el: sc, top: sc.scrollTop };
    }
    let c = editor.adj.crop;
    let q: Quad | null = null;
    const tf = editor.adj.transform;
    const nonNeutral = tf.upright !== "off" || tf.vertical !== 0 || tf.horizontal !== 0 || tf.rotate !== 0 || tf.scale !== 100 || tf.offsetX !== 0 || tf.offsetY !== 0 || tf.aspect !== 0;
    toolBaseline.current = JSON.stringify(editor.adj.crop);
    if (tf.constrainCrop || nonNeutral) {
      // Await the outline so the first visible frame is the fitted one (R1-3, R2-2).
      // The photo currently shows the auto-constrained crop: start from that rectangle (R1-3).
      try {
        const b = await unwrap(commands.getTransformBounds(id, editor.adj));
        q = b.validQuad;
        if (q && tf.constrainCrop && b.constrainedCrop?.enabled) c = b.constrainedCrop;
      } catch {
        /* no outline: today's behaviour */
      }
      if (idRefDev.current !== id) return;
    }
    quadRef.current = q;
    setQuad(q);
    masksRef.current.endTool();
    setGuideOn(false);
    setCropMsg(null);
    setZoom({ on: false, cx: 0.5, cy: 0.5 });
    setShowBefore(false);
    setSplit(false);
    setPicking(false);
    let t = newTool(c.enabled ? fromStored(c, orientation, frameAspect) : FULL, c.enabled ? c.angle : 0, loadCropAspect(), loadOverlay());
    if (carry) {
      t = { ...t, overlay: carry.overlay, overlayOrient: carry.overlayOrient, constrain: carry.constrain };
      if (!c.enabled) t = refit({ ...t, aspect: carry.aspect, flip: carry.flip, customRatio: carry.customRatio }, frameAspect);
    }
    setCropTool(constrainTool(t, frameAspect, orientation, q));
  }, [id, editor.adj, orientation, frameAspect]);
  // Re-open the crop tool on the next photo (R1-1) as soon as it is loaded.
  useEffect(() => {
    if (!reopen || id == null || editor.loading || !editor.info || editor.info.imageId !== id || !(frameAspect > 0)) return;
    setReopen(null);
    startCrop(reopen);
  }, [reopen, id, editor.loading, editor.info, frameAspect, startCrop]);
  // R2-1: the stored crop changed from outside the tool (undo, History, reset, paste, preset, sync): re-seed it.
  const storedCropKey = JSON.stringify(editor.adj.crop);
  useEffect(() => {
    const t = cropRef.current;
    if (!t || editor.loading || storedCropKey === toolBaseline.current) return;
    toolBaseline.current = storedCropKey;
    void startCrop({ ...t, dirty: false });
  }, [storedCropKey, editor.loading, startCrop]);
  /** First Cmd+Z with uncommitted tool changes: back to the seed, history untouched (R2-1). */
  const revertTool = useCallback((newest = 0) => {
    const t = cropRef.current;
    if (!t?.dirty || dirtyAt.current < newest) return false; // a later action (slider, cull, batch) is undone first
    void startCrop({ ...t, dirty: false });
    onNotice("Crop changes undone");
    return true;
  }, [startCrop, onNotice]);
  const commitCrop = useCallback(() => {
    const t = cropRef.current;
    if (!t) {
      masksRef.current.endTool(); // Enter also finishes a mask tool
      return;
    }
    setCropTool(null);
    const next = isFull(t.rect) && t.angle === 0 ? { ...editor.defaults.crop } : toStored(t.rect, orientation, t.angle, frameAspect);
    if (JSON.stringify(next) === JSON.stringify(editor.adj.crop)) return; // nothing changed: no history entry
    toolBaseline.current = JSON.stringify(next); // our own commit must not re-seed a tool
    editor.change((a) => ({ ...a, crop: next }), "Crop");
  }, [editor, orientation, frameAspect]);
  // R2-P2-1: bring the crop panel into view when the tool opens (Transform may push it below the fold); restore the scroll on close.
  useLayoutEffect(() => {
    if (cropOpen) return;
    const s = panelScroll.current;
    panelScroll.current = null;
    if (s && s.el.isConnected) (s.el as HTMLElement).scrollTop = s.top;
  }, [cropOpen]);
  useEffect(() => {
    if (!cropOpen) return;
    const t = window.setTimeout(() => {
      const panel = document.querySelector('[data-testid="crop-panel"]');
      if (!panel) return;
      if (!panelScroll.current) {
        let sc: HTMLElement | null = panel.parentElement;
        while (sc && sc.scrollHeight <= sc.clientHeight + 1) sc = sc.parentElement;
        if (sc) panelScroll.current = { el: sc, top: sc.scrollTop };
      }
      panel.scrollIntoView({ block: "nearest" });
    }, 0);
    return () => window.clearTimeout(t);
  }, [cropOpen]);
  const cancelCrop = useCallback(() => {
    if (!cropRef.current) return false;
    setCropTool(null);
    return true;
  }, []);
  const imageAspectRef = useRef(imageAspect);
  imageAspectRef.current = imageAspect;
  /** Auto straighten (Lightroom): `auto_upright(level)` -> the crop angle (mirrored orientations flip it), re-fit by `changeCrop`. */
  const autoStraighten = useCallback(async () => {
    const forId = id;
    if (forId == null || !cropRef.current) return;
    setCropBusy(true);
    setCropMsg(null);
    try {
      const r = await unwrap(commands.autoUpright(forId, "level", adjRef.current));
      const t = cropRef.current;
      if (!t || forId !== idRefDev.current) return;
      if (!r.solution) return setCropMsg(r.message ?? "No straight lines found to level");
      changeCrop({ ...t, angle: cropAngleForRotation(r.solution.rotationDeg, orientationRef.current), rotating: false });
    } catch (e) {
      onError(e);
    } finally {
      setCropBusy(false);
    }
  }, [id, changeCrop, onError]);
  const idRefDev = useRef(id);
  idRefDev.current = id;
  const cropApi: CropApi = { tool: cropTool, imageAspect: imageAspect || 1.5, start: () => void startCrop(), change: changeCrop, commit: commitCrop, cancel: cancelCrop, autoStraighten: () => void autoStraighten(), autoBusy: cropBusy, autoMessage: cropMsg };
  const toggleGuided = useCallback(() => {
    if (guideRef.current) return setGuideOn(false);
    if (idRefDev.current == null) return;
    masksRef.current.endTool();
    commitCrop();
    setPicking(false);
    setShowBefore(false);
    setSplit(false);
    setZoom({ on: false, cx: 0.5, cy: 0.5 });
    setGuideOn(true);
  }, []);
  const guided = { active: guideOn, toggle: toggleGuided };
  /** Every exit from the crop tool except Esc / Cancel commits it; `carry` re-opens it on the next photo. */
  const commitPendingTool = useCallback(async (stay = false) => {
    const t = cropRef.current;
    if (t) {
      if (stay) setReopen(t); // R3-P2-3b: Cmd+S keeps the tool open on this photo
      else carryRef.current = t;
      window.setTimeout(() => {
        if (carryRef.current === t) carryRef.current = null; // the exit was not a photo change
      }, 1500);
      if (t.dirty) commitCrop();
      else setCropTool(null); // untouched (R2-2): navigating never edits the photo; the tool is still carried
    }
    await Promise.all([flushA(), flushB()]);
  }, [commitCrop, flushA, flushB]);

  // Zoom to a preset keeping the image point under `at` (viewer px; the viewer centre by default) fixed.
  const hoverRef = useRef<{ x: number; y: number } | null>(null);
  const zoomTo = useCallback((p: ZoomPreset, at?: { x: number; y: number } | null) => {
    const l = latest.current;
    if (cropRef.current) return;
    // Frame size not known yet (photo still opening): only the plain Fit / 100% toggles can be honoured.
    if (l.fw <= 0 || l.size.w <= 0) return setZoom(p === "fit" ? { on: false, cx: 0.5, cy: 0.5 } : p === 100 ? { on: true, cx: 0.5, cy: 0.5 } : l.zoom);
    const fitS = Math.min(l.size.w / l.fw, l.size.h / l.fh);
    const s = p === "fit" ? 0 : p === "fill" ? Math.max(l.size.w / l.fw, l.size.h / l.fh) : p / 100;
    if (s <= fitS + 0.001) return setZoom({ on: false, cx: 0.5, cy: 0.5 });
    const k = l.zoom.s ?? 1;
    const box = frameBox(l.zoom, l.size, l.fw * k, l.fh * k, l.main);
    const a = at ?? { x: l.size.w / 2, y: l.size.h / 2 };
    const px = box && box.w > 0 ? (a.x - box.x) / box.w : 0.5;
    const py = box && box.h > 0 ? (a.y - box.y) / box.h : 0.5;
    const axis = (pp: number, am: number, vp: number, full: number) => {
      const d = full * s;
      if (d <= vp) return 0.5;
      const lo = vp / 2 / d;
      return Math.min(1 - lo, Math.max(lo, pp + (vp / 2 - am) / d));
    };
    setZoom({ on: true, cx: axis(px, a.x, l.size.w, l.fw), cy: axis(py, a.y, l.size.h, l.fh), s: Math.abs(s - 1) < 0.0005 ? undefined : s });
  }, []);
  /** Space / double click: Fit <-> 100% at the cursor. */
  const toggleZoom = useCallback(
    (at?: { x: number; y: number }) => {
      if (cropRef.current) return;
      if (latest.current.zoom.on) setZoom({ on: false, cx: 0.5, cy: 0.5 });
      else zoomTo(getLastZoomIn(), at ?? hoverRef.current);
    },
    [zoomTo],
  );
  const activePreset: ZoomPreset | null = !zoom.on
    ? "fit"
    : ((): ZoomPreset | null => {
        if (fw > 0 && size.w > 0 && Math.abs(zs - Math.max(size.w / fw, size.h / fh)) < 0.001) return "fill";
        return ([50, 100, 200, 400] as const).find((n) => Math.abs(zs * 100 - n) < 0.5) ?? null;
      })();

  // Warm the develop cache for filmstrip neighbours.
  useEffect(() => {
    if (id == null) return;
    const i = lib.ids.indexOf(id);
    const near = [lib.ids[i + 1], lib.ids[i - 1], lib.ids[i + 2]].filter((x): x is number => x != null);
    if (near.length) void commands.prepareDevelop(near);
  }, [id, lib.ids]);

  const targets = useCallback((): number[] => {
    if (id == null) return [];
    return sel.selected.size > 1 && sel.selected.has(id) ? [...sel.selected] : [id];
  }, [id, sel.selected]);

  const syncTargets = useMemo(() => [...sel.selected].filter((x) => x !== id), [sel.selected, id]);
  const syncTargetsRef = useRef(syncTargets);
  syncTargetsRef.current = syncTargets;
  const autoOn = autoSync && !compare && syncTargets.length > 0;
  const autoOnRef = useRef(autoOn);
  autoOnRef.current = autoOn;

  const afterBatch = useCallback(
    async (t: number[]) => {
      onCommittedRef.current?.(t);
      await lib.refresh(t.filter((x) => lib.getEntry(x)).slice(0, 2000)).catch(() => {});
      if (id != null && t.includes(id)) await editor.reload();
    },
    [lib, id, editor],
  );

  const run = useCallback(
    async (fn: () => Promise<unknown>) => {
      try {
        await fn();
      } catch (e) {
        onError(e);
      }
    },
    [onError],
  );

  /** Undo of a multi-photo batch: one `undoAdjustments` per photo that changed. */
  const undoBatch = useCallback(
    (t: number[], what: string) => () =>
      void run(async () => {
        for (const x of t) await unwrap(commands.undoAdjustments(x));
        await afterBatch(t);
        onNotice(`Undid ${what}`);
      }),
    [run, afterBatch, onNotice],
  );


  const doPaste = useCallback(() => {
    const c = getClipboard();
    if (!c) return onNotice("Nothing copied yet (Cmd+C copies this photo's settings)");
    const t = targets();
    if (!t.length) return;
    void run(async () => {
      await editor.flush();
      // v19: one undoable batch (`EditBatchResult`); the workflow reports it with an Undo toast.
      const r = await unwrap(commands.pasteSettings(t, c.adjustments, c.fields));
      await afterBatch(t);
      await onBatchRef.current(r, "Pasted settings", t.length, true);
    });
  }, [targets, run, editor, afterBatch, onNotice]);

  /** Copy with `fields` (dialog confirm, or Alt-click with the remembered ones). */
  const copyWith = useCallback(
    (fields: AdjustmentField[]) => {
      setClipboard({ adjustments: structuredClone(editor.adj), fields, fromName: entry?.fileName });
      onNotice(`Copied ${fields.length} settings from ${stem(entry?.fileName)}`);
    },
    [editor.adj, entry?.fileName, onNotice],
  );

  const syncTo = useCallback(
    (fields: AdjustmentField[]) => {
      if (id == null) return;
      const t = syncTargets;
      void run(async () => {
        await editor.flush();
        const r = await unwrap(commands.syncSettings(id, t, fields));
        await afterBatch(t);
        await onBatchRef.current(r, `Synchronized ${fields.length} settings`, t.length, true);
      });
    },
    [id, run, editor, afterBatch, syncTargets],
  );

  // ---- Auto Sync: each committed edit goes through `sync_delta` (useEditor): the other selected photos get the change (exposure / white
  // balance relative), one batch that includes this photo; the batch Undo (toast, Cmd+Z) reverts all of them ----
  syncedRef.current = async (_id, t, r) => {
    try {
      await afterBatch(t);
      if (r.absoluteWbIds.length > 0) onNotice(`White balance was copied as is to ${r.absoluteWbIds.length} ${r.absoluteWbIds.length === 1 ? "photo" : "photos"} (their as-shot values could not be read)`);
      await onBatchRef.current(r.batch, "Auto Sync", t.length + 1, true);
    } catch (e) {
      onError(e);
    }
  };

  const doReset = useCallback(
    () =>
      void run(async () => {
        const t = targets();
        await editor.flush();
        await unwrap(commands.resetAdjustments(t));
        await afterBatch(t);
        if (t.length > 1) onUndoToast(`Reset ${t.length} photos`, undoBatch(t, `reset of ${t.length} photos`));
      }),
    [run, targets, editor, afterBatch, onUndoToast, undoBatch],
  );

  const doApplyPreset = (p: { id: number; name: string }) =>
    void run(async () => {
      const t = targets();
      await editor.flush();
      await unwrap(commands.applyPreset(t, p.id));
      await afterBatch(t);
      if (t.length > 1) onUndoToast(`Applied '${p.name}' to ${t.length} photos`, undoBatch(t, `'${p.name}' on ${t.length} photos`));
    });

  // Paste from previous (Cmd+Alt+V): everything except crop and masks, from the photo edited before this one.
  useEffect(() => {
    if (id == null) return;
    return () => {
      setPreviousPhoto(id);
    };
  }, [id]);
  const pastePrevious = useCallback(
    () =>
      void run(async () => {
        const from = getPreviousPhoto();
        const t = targets();
        if (from == null || t.every((x) => x === from)) return onNotice("No previous photo to paste from");
        await editor.flush();
        const r = await unwrap(commands.pastePrevious(t, from, null));
        await afterBatch(t);
        await onBatchRef.current(r, `Pasted settings from ${stem(lib.getEntry(from)?.fileName) || "the previous photo"}`, t.length, true);
      }),
    [run, targets, editor, afterBatch, lib, onNotice],
  );

  // ---- Auto tone / auto white balance (v14 `auto_tone`, `auto_white_balance`): one history entry each ----
  const runAuto = useCallback(
    async (what: "all" | "tone" | "wb" | "temp" | "tint" | "key", key?: AdjustmentField, label?: string) => {
      if (id == null || autoBusy) return;
      setAutoBusy(true);
      try {
        await editor.flush();
        const cur = editor.adj;
        if (what === "all") {
          // Generic Auto (works without a learned style): tone and white balance from this photo, one history entry.
          const [v, w] = await Promise.all([unwrap(commands.autoTone(id, cur, null)), unwrap(commands.autoWhiteBalance(id, cur))]);
          editor.change((a) => {
            const t = applyAutoTone(a, v);
            return { ...t, whiteBalance: { mode: "custom", temperatureK: w.temperatureK, tint: w.tint } };
          }, "Auto");
          setAutoWb({ id, t: w.temperatureK, tint: w.tint });
        } else if (what === "tone" || what === "key") {
          const v = await unwrap(commands.autoTone(id, cur, what === "key" && key ? [key] : null));
          editor.change((a) => applyAutoTone(a, v), label ?? "Auto Tone");
        } else {
          const w = await unwrap(commands.autoWhiteBalance(id, cur));
          editor.change(
            (a) => {
              const as = info?.asShot ?? { temperatureK: w.temperatureK, tint: w.tint };
              const base = a.whiteBalance.mode === "custom" ? a.whiteBalance : { temperatureK: as.temperatureK, tint: as.tint };
              const next = what === "wb" ? { temperatureK: w.temperatureK, tint: w.tint } : what === "temp" ? { temperatureK: w.temperatureK, tint: base.tint } : { temperatureK: base.temperatureK, tint: w.tint };
              return { ...a, whiteBalance: { mode: "custom", ...next } };
            },
            label ?? "Auto White Balance",
          );
          if (what === "wb") setAutoWb({ id, t: w.temperatureK, tint: w.tint });
        }
      } catch (e) {
        onError(e);
      } finally {
        setAutoBusy(false);
      }
    },
    [id, autoBusy, editor, info, onError],
  );
  const wbNow = editor.adj.whiteBalance;
  const auto: AutoApi = {
    busy: autoBusy,
    all: () => void runAuto("all"),
    tone: () => void runAuto("tone"),
    wb: () => void runAuto("wb"),
    slider: (k) => (k === "temp" ? void runAuto("temp", undefined, "Auto: Temp") : k === "tint" ? void runAuto("tint", undefined, "Auto: Tint") : void runAuto("key", k, `Auto: ${k[0].toUpperCase()}${k.slice(1)}`)),
    wbIsAuto: !!autoWb && autoWb.id === id && wbNow.mode === "custom" && wbNow.temperatureK === autoWb.t && wbNow.tint === autoWb.tint,
  };
  const autoRef = useRef(auto);
  autoRef.current = auto;

  // ---- style library: import, removal, hover previews ----
  const importStyles = useCallback(async () => {
    const r = await styles.importFolder();
    if (!r) return;
    const groups = r.groupIds.length;
    onNotice(`Imported ${r.presets} preset${r.presets === 1 ? "" : "s"} in ${groups} group${groups === 1 ? "" : "s"} and ${r.profiles} profile${r.profiles === 1 ? "" : "s"}${r.skipped.length ? ` · ${r.skipped.length} file${r.skipped.length === 1 ? "" : "s"} skipped` : ""}`);
    if (r.skipped.length > 0) setImportReport(r);
  }, [styles, onNotice]);
  const edgeFor = useCallback((to: "navigator" | "viewer") => (to === "navigator" ? 480 : maxEdge), [maxEdge]);
  const hover = useHoverPreview(id, edgeFor);
  // Any edit (commit, slider, auto, paste) replaces a hover preview with the real render.
  const { stop: stopHover, startVariant } = hover;
  useEffect(() => stopHover(), [editor.adj, stopHover]);
  const hoverPreset = useCallback(
    (p: StylePreset | null) => {
      if (!p || id == null) return stopHover();
      // 120 ms dwell, then one `renderPreviewVariant` (slot `preview`) shown on the main image and the Navigator;
      // latest wins, nothing is applied or saved until the click.
      startVariant(p.name, "preset", { kind: "preset", presetId: p.id }, () => editor.adj, 120);
    },
    [stopHover, startVariant, id, editor.adj],
  );
  // Press-and-hold a panel's "changed" dot: the photo without that panel's changes while held.
  const holdingRef = useRef(false);
  const holdApi = useMemo(
    () => ({
      start: (title: string, fields: AdjustmentField[]) => {
        holdingRef.current = true;
        startVariant(`Without ${title} changes`, "hold", { kind: "without_fields", fields }, () => editorRef.current.adj, 0);
      },
      stop: () => {
        holdingRef.current = false;
        stopHover();
      },
    }),
    [startVariant, stopHover],
  );
  const profileHover = useMemo(
    () => ({
      start: (label: string, adj: ParametricAdjustments) => hover.start(label, "viewer", async () => adj),
      stop: hover.stop,
    }),
    [hover],
  );

  const maskKey = useCallback(
    (action: ActionId, e: KeyboardEvent) => {
      const m = masksRef.current;
      if (action === "maskPanel") {
        if (m.open) m.endTool();
        m.setOpen(!m.open);
        return;
      }
      const tool = action === "maskBrush" || action === "maskLinear" || action === "maskRadial" || action === "maskColor" || action === "maskLuminance";
      if (tool) {
        // Lightroom: the tool key opens Masking and starts the tool in one go (a running crop is discarded first).
        if (cropRef.current) {
          commitCrop(); // leaving the tool applies it (Lightroom)
          cropRef.current = null; // the checks below run before React re-renders
        }
        setPicking(false);
        if (!m.open) m.setOpen(true);
      } else if (!m.open) {
        // O / Shift+O / H only make sense with masks; the panel opens for them, otherwise silent.
        const viewKey = action === "maskOverlay" || action === "maskOverlayStyle" || action === "maskPins";
        if (viewKey && m.groups.length > 0 && !cropRef.current) m.setOpen(true);
        else return;
      }
      if (cropRef.current) return;
      const sel = m.groups.find((g) => g.id === m.selGroup);
      const comp = sel?.components.find((c) => c.id === m.selComp);
      const brushOn = m.tool?.kind === "brush" || comp?.shape.kind === "brush";
      switch (action) {
        case "maskBrush":
          if (sel && comp?.shape.kind === "brush") m.beginTool("brush", { groupId: sel.id, mode: comp.mode }, comp.id);
          else m.create("brush");
          return;
        case "maskLinear":
          return m.create("linear");
        case "maskRadial":
          return m.create("radial");
        case "maskColor":
          return m.create("color");
        case "maskLuminance":
          return m.create("luminance");
        case "maskOverlay":
          return m.toggleOverlay();
        case "maskOverlayStyle":
          if (!m.overlayOn) m.toggleOverlay();
          return m.cycleOverlayStyle();
        case "maskPins":
          return m.togglePins();
        case "maskSize":
          if (brushOn) m.patchBrush({ size: Math.max(1, Math.min(100, m.brush.size + (e.key === "[" ? -5 : 5))) });
          return;
        case "maskFeather":
          if (brushOn) m.patchBrush({ feather: Math.max(0, Math.min(100, m.brush.feather + (e.key === "[" || e.key === "{" ? -10 : 10))) });
          return;
        case "maskAuto":
          if (brushOn) m.patchBrush({ autoMask: !m.brush.autoMask });
          return;
        case "maskDelete":
          return m.deleteSelected();
        case "maskMoveUp":
          return m.moveSelected(-1);
        case "maskMoveDown":
          return m.moveSelected(1);
      }
    },
    [],
  );

  // ---- white balance picker (W) ----
  const togglePicker = useCallback(() => {
    if (cropRef.current) return;
    masksRef.current.endTool();
    setPicking((v) => !v);
  }, []);
  const pickWb = useCallback(
    (clientX: number, clientY: number, rect: DOMRect) => {
      setPicking(false);
      if (id == null || !boxRef.current || !frameRef.current) return;
      const disp = screenToDisp(clientX - rect.left, clientY - rect.top, boxRef.current);
      const pt = dispToSensor({ x: Math.min(1, Math.max(0, disp.x)), y: Math.min(1, Math.max(0, disp.y)) }, frameRef.current);
      void run(async () => {
        const r = await unwrap(commands.sampleWhiteBalance(id, pt, editor.adj as ParametricAdjustments));
        editor.change((a) => ({ ...a, whiteBalance: { mode: "custom", temperatureK: r.temperatureK, tint: r.tint } }), "White Balance: Picker");
      });
    },
    [id, run, editor],
  );

  // ---- Black & White (V) ----
  const toggleBw = useCallback(() => {
    const on = !editor.adj.blackAndWhite.enabled;
    editor.change((a) => ({ ...a, blackAndWhite: { ...a.blackAndWhite, enabled: on } }), on ? "Black & White" : "Color");
  }, [editor]);

  // ---- face zoom (F): 100% on each detected face ----
  const facesRef = useRef<{ id: number | null; faces: FaceInfo[] }>({ id: null, faces: [] });
  const faceIdx = useRef(-1);
  useEffect(() => {
    faceIdx.current = -1;
    facesRef.current = { id, faces: [] };
    if (id == null) return;
    let stale = false;
    unwrap(commands.getFaces(id))
      .then((f) => !stale && (facesRef.current = { id, faces: f }))
      .catch(() => {});
    return () => {
      stale = true;
    };
  }, [id]);
  const faceZoom = useCallback((dir: 1 | -1) => {
    const faces = facesRef.current.faces;
    if (cropRef.current || faces.length === 0) return;
    const order = [...faces.keys()].sort((p, q) => Number(faces[q].primary) - Number(faces[p].primary) || p - q);
    let next = faceIdx.current + dir;
    if (next >= order.length || next < -1) next = dir > 0 ? -1 : order.length - 1;
    faceIdx.current = next;
    if (next < 0) return setZoom({ on: false, cx: 0.5, cy: 0.5 });
    const b = faces[order[next]].bbox;
    setZoom({ on: true, cx: b.x + b.width / 2, cy: b.y + b.height / 2 });
  }, []);

  // ---- Esc cascade: never changes the module ----
  const browsingRef = useRef(false);
  browsingRef.current = browsing;
  const escape = useCallback(() => {
    if (pickerRef.current) return setPicking(false);
    if (browsingRef.current) return setBrowsing(false);
    if (guideRef.current) return setGuideOn(false);
    if (cropRef.current) return setCropTool(null);
    const m = masksRef.current;
    if (m.tool) return m.endTool();
    if (m.open && (m.selGroup || m.selComp)) return m.select(null);
    if (m.open) m.setOpen(false);
  }, []);


  useImperativeHandle(
    ref,
    () => ({
      toggleBefore: () => setShowBefore((v) => !v),
      toggleZoom: () => toggleZoom(),
      copy: () => setDialog({ kind: "copy" }),
      paste: doPaste,
      sync: (quiet) => {
        if (syncTargets.length === 0) return onNotice("Cmd/Shift-click other photos in the filmstrip to sync to them");
        if (quiet) syncTo(rememberedCopyFields());
        else setDialog({ kind: "sync" });
      },
      reset: doReset,
      toggleSplit: () => setSplit((v) => !v),
      toggleCrop: () => (cropRef.current ? commitCrop() : startCrop()),
      commitCrop,
      commitPendingTool,
      cancelCrop,
      undo: editor.undo,
      toggleAutoSync: () => {
        if (syncTargetsRef.current.length === 0) return onNotice("Auto Sync needs 2 or more selected photos (Cmd / Shift-click in the filmstrip)");
        onAutoSync?.(!autoOnRef.current);
      },
      redo: editor.redo,
      maskKey: (action, e) => maskKey(action, e),
      cancelMaskTool: () => {
        if (!masksRef.current.tool) return false;
        masksRef.current.endTool();
        return true;
      },
      escape,
      isCropping: () => cropRef.current !== null,
      revertTool,
      cropSwap: () => cropRef.current && changeCrop(swapTool(cropRef.current, imageAspectRef.current || 1.5)),
      cropOverlay: () => {
        const t = cropRef.current;
        if (!t) return;
        const overlay = nextOverlay(t.overlay);
        saveOverlay(overlay);
        changeCrop({ ...t, overlay });
      },
      cropOverlayRotate: () => cropRef.current && changeCrop({ ...cropRef.current, overlayOrient: (cropRef.current.overlayOrient + 1) % 4 }),
      cropReset: () => {
        if (cropRef.current) return changeCrop(resetTool(cropRef.current));
        // Outside the tool (Lightroom): only the crop goes back, as one history entry (none when it is already neutral).
        const d = editorRef.current;
        if (JSON.stringify(d.adj.crop) === JSON.stringify(d.defaults.crop)) return;
        d.change((a) => ({ ...a, crop: { ...d.defaults.crop } }), "Reset Crop");
      },
      toggleGuided,
      cropLock: () => cropRef.current && changeCrop(toggleLockTool(cropRef.current, imageAspectRef.current || 1.5)),
      lastCommitAt: editor.lastCommitAt,
      canRedo: editor.canRedo,
      toggleBw,
      togglePicker,
      faceZoom,
      pastePrevious,
      savePreset: () => setDialog({ kind: "preset" }),
      autoTone: () => autoRef.current.tone(),
      autoWb: () => autoRef.current.wb(),
    }),
    [revertTool, commitPendingTool, toggleZoom, doPaste, doReset, syncTargets.length, syncTo, onNotice, editor.undo, editor.redo, editor.lastCommitAt, editor.canRedo, commitCrop, cancelCrop, startCrop, toggleGuided, maskKey, escape, toggleBw, togglePicker, faceZoom, pastePrevious],
  );

  const box = frameBox(zoom, size, fw * zs, fh * zs, editor.main);
  const frame: Frame | null = useMemo(() => (fw > 0 && fh > 0 ? { orientation, crop: editor.adj.crop, w: fw, h: fh } : null), [orientation, editor.adj.crop, fw, fh]);
  // Lightroom shows a fine grid while a Transform slider is dragged or nudged; it goes 300 ms after the last change (R1-P2-9).
  const [tfGrid, setTfGrid] = useState(false);
  const tfHold = useRef(false);
  const tfTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const tfBump = useCallback(() => {
    setTfGrid(true);
    clearTimeout(tfTimer.current);
    if (!tfHold.current) tfTimer.current = setTimeout(() => setTfGrid(false), 300);
  }, []);
  useEffect(() => {
    const isTf = (t: EventTarget | null) => t instanceof Element && !!t.closest('[data-testid^="slider-tf-"], [data-testid^="slider-row-tf-"]');
    const down = (e: PointerEvent) => {
      if (!isTf(e.target)) return;
      tfHold.current = true;
      tfBump();
    };
    const up = () => {
      if (!tfHold.current) return;
      tfHold.current = false;
      tfBump();
    };
    document.addEventListener("pointerdown", down, true);
    window.addEventListener("pointerup", up, true);
    window.addEventListener("pointercancel", up, true);
    return () => {
      document.removeEventListener("pointerdown", down, true);
      window.removeEventListener("pointerup", up, true);
      window.removeEventListener("pointercancel", up, true);
      clearTimeout(tfTimer.current);
    };
  }, [tfBump]);
  const tfKey = (({ vertical, horizontal, rotate, aspect, scale, offsetX, offsetY }) => JSON.stringify([vertical, horizontal, rotate, aspect, scale, offsetX, offsetY]))(editor.adj.transform);
  const tfPrev = useRef({ id, tfKey });
  useEffect(() => {
    const p = tfPrev.current;
    tfPrev.current = { id, tfKey };
    if (p.id !== id || p.tfKey === tfKey) return;
    // Only when the change comes from a Transform slider (drag, hover nudge, typing), not from Upright / undo / paste.
    if (document.querySelector('[data-testid^="slider-row-tf-"]:hover, [data-testid^="slider-tf-"]:focus, [data-testid^="slider-edit-tf-"]')) tfBump();
  }, [tfKey, id, tfBump]);
  const boxRef = useRef(box);
  boxRef.current = box;
  const frameRef = useRef(frame);
  frameRef.current = frame;
  const thumb = entry?.thumbnail;
  const thumbUrl = thumb?.status === "ready" ? `${convertFileSrc(thumb.path)}?v=${id != null ? lib.version(id) : 0}` : null;

  /**
   * Shown (fit view) until a photo's first render arrives, so switching photos never goes blank: its cached edited
   * render (IPC v19.1), or the embedded preview of an unedited photo - never the embedded (unedited) preview of an
   * edited photo, which would flash it without its edits.
   */
  const placeholderFor = (pid: number | null): string | null => (pid != null ? developPlaceholder(lib.getEntry(pid), lib.version(pid)) : null);
  usePrefetchNeighbours(lib, id);

  const mkViewer = (ed: Editor, active: boolean) => {
    const pid = ed === editorB ? idB : idA;
    return (
    <Viewer
      imageId={pid}
      main={ed.main}
      before={active ? ed.before : null}
      detail={ed.detail}
      detailRegion={region}
      showBefore={active && showBefore && !compare}
      split={active && split && !compare}
      splitPos={splitPos}
      onSplitPos={setSplitPos}
      zoom={zoom}
      size={size}
      fw={(ed.info?.fullWidth ?? 0) * zs}
      fh={(ed.info?.fullHeight ?? 0) * zs}
      hoverRef={hoverRef}
      loading={ed.loading}
      placeholder={placeholderFor(pid)}
      onSize={setSize}
      onPan={setZoom}
      onPanEnd={onPanEnd}
      onToggleZoom={toggleZoom}
      inset={active && cropTool ? CROP_INSET : 0}
      rotate={active && cropTool ? previewRotation(cropTool.angle, orientation) : 0}
    />
    );
  };
  const activeLayers = (
    <>
      {hover.preview?.to === "viewer" && (
        <div className="pointer-events-none absolute inset-0 z-10" data-testid="hover-preview">
          <img src={hover.preview.url} alt="" className="size-full object-contain" draggable={false} />
          <span className="absolute left-2 top-2 rounded bg-black/70 px-1.5 text-xs text-white" data-testid="hover-preview-label">
            Preview: {hover.preview.label}
          </span>
        </div>
      )}
          {health && !editor.main && entry && (
        <OriginalUnavailable
          health={health}
          fileName={entry.fileName}
          onLocate={() => onLocate(entry.id)}
          onRetry={() => {
            clearFileHealth(entry.path);
            void editor.reload().catch(onError);
          }}
        />
      )}
      {masks.open && !cropTool && <MaskLayer masks={masks} editor={editor} id={id} frame={frame} box={box} region={zoom.on ? region : null} onError={onError} />}
      {masks.open && masks.tool && !cropTool && (
        <span className="pointer-events-none absolute left-2 top-2 rounded bg-black/60 px-1.5 text-xs text-white" data-testid="mask-tool-badge" data-tool={masks.tool.kind}>
          {TOOL_HELP[masks.tool.kind]}
        </span>
      )}
      {cropTool && imageAspect > 0 && (
        <div className="absolute" style={{ inset: CROP_INSET }} data-testid="crop-inset">
          <CropOverlay tool={cropTool} size={{ w: Math.max(0, size.w - 2 * CROP_INSET), h: Math.max(0, size.h - 2 * CROP_INSET) }} imageAspect={imageAspect} orientation={orientation} onChange={changeCrop} validQuad={quad} />
        </div>
      )}
      {/* The right panel is the home of the crop controls (Lightroom); the floating bar only stands in while that panel is hidden (Tab). */}
      {cropTool && (panels.right || masks.open) && <CropBar crop={cropApi} />}
      {guideOn && !compare && (
        <span className="pointer-events-none absolute left-2 top-2 z-20 rounded bg-black/70 px-1.5 text-xs text-white" data-testid="guided-badge">
          Guided Upright: drag 2 to 4 lines along straight edges. x deletes, Esc done
        </span>
      )}
      {tfGrid && box && !cropTool && (
        <div className="pointer-events-none absolute z-10" style={{ left: box.x, top: box.y, width: box.w, height: box.h }} data-testid="transform-grid">
          {[1, 2, 3, 4, 5, 6, 7, 8, 9].map((i) => (
            <div key={i}>
              <div className="absolute inset-y-0 w-px bg-white/45" style={{ left: `${i * 10}%` }} />
              <div className="absolute inset-x-0 h-px bg-white/45" style={{ top: `${i * 10}%` }} />
            </div>
          ))}
        </div>
      )}
      {guideOn && imageAspect > 0 && !compare && (
        <GuideOverlay guides={editor.adj.transform.guides ?? []} size={size} imageAspect={imageAspect} orientation={orientation} onChange={upright.setGuides} />
      )}
      {picking && (
        <div
          className="absolute inset-0 z-20 cursor-crosshair"
          data-testid="wb-picker-overlay"
          onClick={(e) => pickWb(e.clientX, e.clientY, e.currentTarget.getBoundingClientRect())}
        >
          <span className="pointer-events-none absolute left-2 top-2 rounded bg-black/70 px-1.5 text-xs text-white" data-testid="wb-picker-badge">
            Click a neutral grey or white. Esc cancels
          </span>
        </div>
      )}
    </>
  );

  const vbtn = (on = false) => `flex h-6 items-center gap-1 rounded px-2 text-xs ${on ? "bg-sky-800 text-sky-100" : "bg-neutral-800 hover:bg-neutral-700"} disabled:opacity-40`;
  const nTargets = targets().length;
  const prevId = usePreviousPhoto();
  const filmCell = wide ? 88 : FILM;
  // Walking ~30 fields through completeAdjustments is costly; only redo it when the settings actually change.
  // Only the settings dialogs need it: not recomputed per slider frame.
  const dialogOpen = dialog != null;
  const modified = useMemo(() => (dialogOpen ? modifiedFields(editor.adj, editor.defaults) : []), [dialogOpen, editor.adj, editor.defaults]);
  const dialogProps = { modified, hasLut: !!editor.adj.lut, hasMasks: editor.adj.masks.length > 0 };
  const filmIdx = id != null ? lib.ids.indexOf(id) : -1;

  const viewerToolbar = !panels.chrome && (
    <div
      className="flex h-9 shrink-0 items-center gap-2 border-t border-neutral-800 bg-neutral-950 px-2 text-neutral-300"
      data-testid="viewer-toolbar"
      data-render-ms={editor.main ? Math.round(editor.main.renderMs) : ""}
      data-render-size={editor.main ? `${editor.main.width}x${editor.main.height}` : ""}
    >
      <button className={vbtn(showBefore)} disabled={!!compare} onClick={() => setShowBefore((v) => !v)} title={`Before / after${hint("before")}`} data-testid="before-toggle">
        <Columns2 className="size-3.5" /> Before
      </button>
      <button className={vbtn(split)} disabled={!!compare} onClick={() => setSplit((v) => !v)} title={`Split view${hint("split")}`} data-testid="split-toggle">
        <SplitSquareHorizontal className="size-3.5" /> Split
      </button>
      <button className={vbtn(!!compare)} onClick={() => onToggleCompare?.()} title={`Compare two photos side by side${hint("compare")}`} aria-pressed={!!compare} data-testid="develop-compare">
        <Columns3 className="size-3.5" /> Compare
      </button>
      <div className="flex gap-px" role="group" aria-label="Zoom" data-testid="zoom-toggle" data-zoom={zoom.on ? String(Math.round(zs * 100)) : "fit"}>
        {ZOOM_PRESETS.map((z, i) => (
          <button
            key={String(z.id)}
            className={`${vbtn(activePreset === z.id)} ${i === 0 ? "rounded-r-none" : i === ZOOM_PRESETS.length - 1 ? "rounded-l-none" : "rounded-none"}`}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => (rememberZoom(z.id), zoomTo(z.id))}
            title={`${z.title}${z.id === "fit" || z.id === 100 ? hint("zoomDevelop") : ""}`}
            aria-pressed={activePreset === z.id}
            data-testid={`zoom-${z.id}`}
          >
            {z.label}
          </button>
        ))}
      </div>
      {entry && (
        <span className="ml-2 flex items-center gap-1" data-testid="develop-flags" data-pick={entry.pick} data-rating={entry.rating}>
          <button className="rounded p-0.5 hover:bg-neutral-800" onClick={() => onFlag?.(entry.id, "pick")} title={`Pick${hint("pick")}`} aria-pressed={entry.pick === "pick"} data-testid="develop-pick">
            <Flag className={`size-3.5 ${entry.pick === "pick" ? "fill-green-500 text-green-500" : "text-neutral-400"}`} />
          </button>
          <button className="rounded p-0.5 hover:bg-neutral-800" onClick={() => onFlag?.(entry.id, "reject")} title={`Reject${hint("reject")}`} aria-pressed={entry.pick === "reject"} data-testid="develop-reject">
            <X className={`size-4 ${entry.pick === "reject" ? "text-red-500" : "text-neutral-400"}`} strokeWidth={entry.pick === "reject" ? 3 : 2} />
          </button>
          <Stars n={entry.rating} className="size-3.5" onRate={onRate && ((r) => onRate(entry.id, r))} testId="develop-stars" />
          <Menu
            trigger={<span className={`block size-2.5 rounded-full ${entry.colorLabel ? LABEL_COLOR[entry.colorLabel] : "border border-neutral-500"}`} data-label={entry.colorLabel ?? ""} />}
            triggerClass="flex size-5 items-center justify-center rounded hover:bg-neutral-800"
            triggerTestId="develop-label"
            title={entry.colorLabel ? `Color label: ${entry.colorLabel}` : "Color label"}
          >
            {(close) => (
              <>
                {COLOR_LABELS.map((l) => (
                  <button
                    key={l}
                    role="menuitem"
                    className={menuItem}
                    data-testid={`develop-label-${l}`}
                    onClick={() => {
                      close();
                      onLabel?.(entry.id, entry.colorLabel === l ? null : l);
                    }}
                  >
                    <span className={`size-2.5 rounded-full ${LABEL_COLOR[l]}`} /> <span className="capitalize">{l}</span>
                  </button>
                ))}
                <button
                  role="menuitem"
                  className={menuItem}
                  data-testid="develop-label-none"
                  onClick={() => {
                    close();
                    onLabel?.(entry.id, null);
                  }}
                >
                  No label
                </button>
              </>
            )}
          </Menu>
        </span>
      )}
      <span className="min-w-0 flex-1 truncate text-xs text-neutral-300" title={entry?.path ?? ""} data-testid="develop-filename">
        {entry?.fileName ?? ""}
      </span>
      <WarningsChip
        warnings={info?.warnings ?? []}
        actions={{
          ai_mask_needs_update: { label: "Update AI masks", run: () => void masksRef.current.updateAll() },
          masks_unsupported: { label: "Show in Masks panel", run: () => masksRef.current.setOpen(true) },
        }}
      />
    </div>
  );

  return (
    <div className="absolute inset-0 z-10 flex flex-col bg-neutral-950" data-testid="develop-view" data-image-id={id ?? ""}>
      {topSlot}
      {compare && !panels.chrome && (
        <CompareBar
          focus={compare.focus}
          aName={lib.getEntry(compare.a)?.fileName ?? ""}
          bName={lib.getEntry(compare.b)?.fileName ?? ""}
          onSwap={() => onSwap?.()}
          onMakeSelect={() => onMakeSelect?.()}
          onFocus={(k) => onFocusPane?.(k)}
          onExit={onToggleCompare}
        />
      )}

      <div className="flex min-h-0 flex-1">
        {!panels.left && (
          <aside className="w-56 shrink-0 border-r border-neutral-800 min-[1600px]:w-60" data-testid="left-aside" onMouseLeave={hover.stop}>
            <LeftPanel
              groups={styles.groups}
              importing={styles.importing}
              onImport={() => void importStyles()}
              onRemoveGroup={(g: StyleGroup) => void styles.removeGroup(g.id)}
              onHoverPreset={hoverPreset}
              navPreview={hover.preview?.to === "navigator" || hover.preview?.source === "preset" ? hover.preview : null}
              appliedPresetId={editor.history?.appliedPresetId ?? null}
              history={editor.history}
              imageId={id}
              onApplyPreset={doApplyPreset}
              onSavePreset={() => setDialog({ kind: "preset" })}
              onDeletePreset={(p) =>
                void run(async () => {
                  await unwrap(commands.deletePreset(p.id));
                  await styles.reload();
                })
              }
              onUndo={editor.undo}
              onRedo={editor.redo}
              onGoto={editor.goto}
              targetCount={nTargets}
              navUrl={editor.navUrl ?? editor.main?.url ?? thumbUrl}
              zoom={zoom}
              region={region}
              onPreset={(pr) => (rememberZoom(pr), zoomTo(pr))}
              activePreset={activePreset}
              onZoom={(z) => cropRef.current || setZoom(z)}
              onCopy={(alt) => (alt ? copyWith(rememberedCopyFields()) : setDialog({ kind: "copy" }))}
              onPaste={doPaste}
              copied={copied}
            />
          </aside>
        )}
        <div className="flex min-w-0 flex-1 flex-col">
          {compare ? (
            <div className="relative flex min-h-0 min-w-0 flex-1 gap-1" data-testid="dev-compare">
              {(["a", "b"] as const).map((k) => {
                const active = compare.focus === k;
                const pid = compare[k];
                const pe = lib.getEntry(pid);
                return (
                  <div
                    key={k}
                    className={`relative min-w-0 flex-1 border-2 ${active ? "border-sky-500" : "border-transparent"}`}
                    data-testid={`dev-compare-pane-${k}`}
                    data-active={active}
                    data-image-id={pid}
                    onPointerDown={() => !active && onFocusPane?.(k)}
                  >
                    {mkViewer(k === "a" ? editorA : editorB, active)}
                    {active && activeLayers}
                    <div className="absolute bottom-2 left-2 flex items-center gap-2 rounded bg-black/70 px-2 py-0.5 text-xs text-neutral-100" data-testid={`dev-compare-label-${k}`}>
                      <span className={`rounded px-1 font-semibold ${k === "a" ? "bg-sky-700" : "bg-amber-400 text-black"}`}>{k === "a" ? "Select" : "Candidate"}</span>
                      {pe?.fileName}
                      {pe && <Stars n={pe.rating} className="size-3" onRate={onRate && ((r) => onRate(pid, r))} testId={`dev-compare-stars-${k}`} />}
                      {active && <span className="text-sky-300">editing</span>}
                    </div>
                  </div>
                );
              })}
              <PanelChevron side="left" hidden={panels.left} />
              <PanelChevron side="right" hidden={panels.right} />
            </div>
          ) : (
            <div className="relative min-h-0 min-w-0 flex-1">
              {mkViewer(editor, true)}
              {activeLayers}
              <PanelChevron side="left" hidden={panels.left} />
              <PanelChevron side="right" hidden={panels.right} />
            </div>
          )}
          {viewerToolbar}
        </div>
        {!panels.right && (
          <aside className="flex w-72 shrink-0 flex-col border-l border-neutral-800 min-[1600px]:w-80" data-testid="right-aside" onMouseLeave={() => !holdingRef.current && hover.stop()}>
            {compare && (
              <div className="truncate border-b border-neutral-800 px-3 py-1 text-[11px] text-sky-300" data-testid="compare-editing">
                Editing the {compare.focus === "a" ? "Select" : "Candidate"}: {entry?.fileName}
              </div>
            )}
            <div className="min-h-0 flex-1">
              <AdjustPanel
                editor={editor}
                styleVersion={styles.version}
                importing={styles.importing}
                onImportStyles={() => void importStyles()}
                hover={profileHover}
                hold={holdApi}
                auto={auto}
                imageId={id}
                onError={onError}
                crop={cropApi}
                upright={upright}
                guided={guided}
                picker={{ active: picking, toggle: togglePicker }}
                masks={{
                  open: masks.open,
                  count: masks.groups.length,
                  toggle: () => {
                    if (masks.open) masks.endTool();
                    masks.setOpen(!masks.open);
                  },
                  panel: <MasksPanel masks={masks} />,
                }}
                browser={{ open: browsing, setOpen: setBrowsing }}
                exif={exifLine(entry?.capture)}
                bar={{
                  count: nTargets,
                  hasPrevious: prevId != null && prevId !== id,
                  onPrevious: pastePrevious,
                  autoSync: autoOn,
                  onAutoSync: (v) => onAutoSync?.(v),
                  onSync: (alt) => (alt ? syncTo(rememberedCopyFields()) : setDialog({ kind: "sync" })),
                  onReset: doReset,
                }}
              />
            </div>
          </aside>
        )}
      </div>

      {!panels.chrome && (
        <>
          <div className="flex h-[22px] shrink-0 items-center gap-3 overflow-hidden whitespace-nowrap border-t border-neutral-800 bg-neutral-900 px-3 text-[11px] text-neutral-400" data-testid="filmstrip-header">
            <span>{filmIdx >= 0 ? `${filmIdx + 1} of ${lib.ids.length}` : `${lib.ids.length} photos`}</span>
            {sel.selected.size > 1 && <span className="text-sky-300">{sel.selected.size} selected</span>}
            {sceneOnly && (
              <button
                className={`shrink-0 rounded px-1.5 ${sceneOnly.on ? "bg-sky-800 text-sky-100" : "bg-neutral-800 text-neutral-200 hover:bg-neutral-700"}`}
                aria-pressed={sceneOnly.on}
                onClick={sceneOnly.toggle}
                title="Show only the photos of this scene (Shift+S)"
                data-testid="scene-only"
              >
                This scene only
              </button>
            )}
            {filterSummary && (
              <>
                <span className="ml-auto min-w-0 truncate" data-testid="filter-summary-text">
                  {filterSummary.text}
                </span>
                <button className="shrink-0 rounded bg-neutral-800 px-1.5 text-neutral-200 hover:bg-neutral-700" onClick={filterSummary.onEdit} data-testid="edit-filters">
                  Edit filters
                </button>
              </>
            )}
          </div>
          <Filmstrip
            lib={lib}
            activeId={compare ? compare.b : id}
            selected={compare ? new Set([compare.a]) : sel.selected}
            marked={compare ? new Set([compare.b]) : undefined}
            onPick={(fid, ev) => {
              const mods = { shift: ev.shiftKey, meta: ev.metaKey || ev.ctrlKey };
              void commitPendingTool().then(() => (compare ? onCandidate?.(fid) : sel.click(fid, mods)));
            }}
            onRate={onRate}
            badge={compare ? (fid) => <CompareTag id={fid} a={compare.a} b={compare.b} /> : filmBadge}
            cellW={filmCell}
            cellH={filmCell}
            height={filmCell + 8}
            top={4}
            scenePrefix="film-scene"
          />
        </>
      )}

      {importReport && (
        <Dialog label="Import report" testid="import-report" className="w-[460px] max-w-full rounded-lg border border-neutral-700 bg-neutral-900 p-4 shadow-xl" onCancel={() => setImportReport(null)} onConfirm={() => setImportReport(null)}>
          <h2 className="mb-1 text-sm font-semibold">Import report</h2>
          <p className="mb-2 text-xs text-neutral-300">
            Imported {importReport.presets} presets and {importReport.profiles} profiles. {importReport.skipped.length} files were skipped:
          </p>
          <ul className="max-h-56 overflow-y-auto rounded bg-neutral-950 p-2 text-xs" data-testid="import-skipped">
            {importReport.skipped.map((k) => (
              <li key={k.path} className="py-0.5">
                <span className="text-neutral-200">{k.path.split("/").pop()}</span> <span className="text-neutral-400">{k.reason}</span>
              </li>
            ))}
          </ul>
          <div className="mt-3 flex justify-end">
            <button data-autofocus className="rounded bg-neutral-800 px-3 py-1 text-xs hover:bg-neutral-700" onClick={() => setImportReport(null)} data-testid="import-report-close">
              Close
            </button>
          </div>
        </Dialog>
      )}
      {masks.picker && id != null && (
        <PeoplePicker imageId={id} thumbUrl={thumbUrl} caps={masks.caps} onCreate={(pt, parts, name) => void masks.createPeople(pt, parts, name)} onCancel={masks.closePicker} onError={onError} />
      )}
      {dialog?.kind === "copy" && (
        <SettingsFieldsDialog
          title="Copy Settings"
          confirm="Copy"
          storageKey={COPY_FIELDS_KEY}
          {...dialogProps}
          onCancel={() => setDialog(null)}
          onConfirm={(fields) => {
            setDialog(null);
            copyWith(fields);
          }}
        />
      )}
      {dialog?.kind === "sync" && id != null && (
        <SettingsFieldsDialog
          title="Synchronize Settings"
          confirm="Synchronize"
          storageKey={COPY_FIELDS_KEY}
          {...dialogProps}
          onCancel={() => setDialog(null)}
          onConfirm={(fields) => {
            setDialog(null);
            syncTo(fields);
          }}
        />
      )}
      {dialog?.kind === "preset" && (
        <SettingsFieldsDialog
          title="New Develop Preset"
          confirm="Create"
          withName
          storageKey={PRESET_FIELDS_KEY}
          {...dialogProps}
          onCancel={() => setDialog(null)}
          onConfirm={(fields, name) => {
            setDialog(null);
            void run(async () => {
              await unwrap(commands.savePreset(null, name, editor.adj, fields));
              await styles.reload();
            });
          }}
        />
      )}
    </div>
  );
});

/** 16 px chevron on a panel's inner edge: hides / shows that panel (Tab does both). */
function PanelChevron({ side, hidden }: { side: "left" | "right"; hidden: boolean }) {
  const Icon = (side === "left") === hidden ? ChevronRight : ChevronLeft;
  return (
    <button
      className={`absolute top-1/2 z-30 flex h-12 w-4 -translate-y-1/2 items-center justify-center bg-neutral-900/70 text-neutral-300 hover:bg-neutral-800 hover:text-white ${side === "left" ? "left-0 rounded-r" : "right-0 rounded-l"}`}
      title={hidden ? "Show panel (Tab)" : "Hide panel (Tab)"}
      aria-label={hidden ? `Show ${side} panel` : `Hide ${side} panel`}
      data-testid={`panel-chevron-${side}`}
      onClick={() => setPanelHidden(side, !hidden)}
    >
      <Icon className="size-3.5" />
    </button>
  );
}
