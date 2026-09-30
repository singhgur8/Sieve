// Develop module: filmstrip + viewer (before/after, split, 100% detail) + presets/history + adjustment sliders.
import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { ArrowLeft, ChevronLeft, ChevronRight, ClipboardCopy, ClipboardPaste, Columns2, Flag, RefreshCw, RotateCcw, SplitSquareHorizontal, X, ZoomIn } from "lucide-react";
import { commands, convertFileSrc, unwrap, type FaceInfo, type LutInfo, type NormRect, type ParametricAdjustments, type Preset } from "../../ipc";
import type { Library } from "../../hooks/useLibrary";
import type { SelectionApi } from "../../hooks/useSelection";
import { useEditor } from "../../hooks/useEditor";
import { Stars } from "../Cell";
import { Filmstrip } from "../Filmstrip";
import { setPanelHidden, usePanels } from "../../lib/panels";
import { dispToSensor, screenToDisp } from "../../lib/maskGeom";
import { copyFields, FIELD_LABEL } from "../../lib/adjust";
import { hint, type ActionId } from "../../lib/keymap";
import { useMasks } from "../../hooks/useMasks";
import { MasksPanel } from "./MasksPanel";
import { MaskLayer } from "./MaskLayer";
import { PeoplePicker } from "./PeoplePicker";
import type { Frame } from "../../lib/maskGeom";
import { LABEL_COLOR } from "../../lib/format";
import { getClipboard, setClipboard, useClipboard } from "../../lib/clipboard";
import { AdjustPanel } from "./AdjustPanel";
import { LeftPanel } from "./LeftPanel";
import { FieldsDialog } from "./FieldsDialog";
import { CropOverlay, swapTool, toggleLockTool, type CropTool } from "./CropOverlay";
import { CropBar, type CropApi } from "./CropPanel";
import { WarningsChip } from "./WarningsChip";
import { FULL, fromStored, isFull, loadCropAspect, toStored } from "../../lib/crop";
import { setSectionOpen } from "../../lib/sections";
import { Viewer, frameBox, visibleRegion, type Size, type Zoom } from "./Viewer";

export interface DevelopHandle {
  toggleBefore: () => void;
  toggleZoom: () => void;
  copy: () => void;
  paste: () => void;
  sync: () => void;
  reset: () => void;
  toggleSplit: () => void;
  /** R: start the crop tool, or apply it when already active. */
  toggleCrop: () => void;
  /** Enter: apply the crop (no-op when the tool is inactive). */
  commitCrop: () => void;
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
  cropSwap: () => void;
  cropLock: () => void;
  /** Time (ms) of the adjustment Cmd+Z would undo (0 = none) and whether an adjustment redo exists. */
  lastCommitAt: () => number;
  canRedo: () => boolean;
  toggleBw: () => void;
  togglePicker: () => void;
  faceZoom: (dir: 1 | -1) => void;
  pastePrevious: () => void;
  savePreset: () => void;
}

/** Last photo edited in Develop (for Paste from previous); survives the module being re-entered. */
let previousId: number | null = null;

type Dialog = { kind: "copy" | "sync" | "preset" } | null;

interface Props {
  lib: Library;
  sel: SelectionApi;
  onError: (e: unknown) => void;
  onNotice: (s: string) => void;
  /** Toast with an Undo action (multi-photo reset / preset). */
  onUndoToast: (msg: string, undo: () => void) => void;
  onBack: () => void;
}

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

export const DevelopView = forwardRef<DevelopHandle, Props>(function DevelopView({ lib, sel, onError, onNotice, onUndoToast, onBack }, ref) {
  const id = sel.active;
  const [size, setSize] = useState<Size>({ w: 0, h: 0 });
  const [zoom, setZoom] = useState<Zoom>({ on: false, cx: 0.5, cy: 0.5 });
  const [region, setRegion] = useState<NormRect | null>(null);
  const [showBefore, setShowBefore] = useState(false);
  const [split, setSplit] = useState(false);
  const [splitPos, setSplitPos] = useState(0.5);
  const [luts, setLuts] = useState<LutInfo[]>([]);
  const [presets, setPresets] = useState<Preset[]>([]);
  const [dialog, setDialog] = useState<Dialog>(null);
  const [cropTool, setCropTool] = useState<CropTool | null>(null);
  const cropRef = useRef<CropTool | null>(null);
  cropRef.current = cropTool;
  const [picking, setPicking] = useState(false);
  const pickerRef = useRef(false);
  pickerRef.current = picking;
  const panels = usePanels("develop");
  const copied = useClipboard();
  const dpr = typeof window === "undefined" ? 1 : window.devicePixelRatio || 1;

  const { refresh } = lib;
  const onChanged = useCallback((changed: number) => void refresh([changed]).catch(() => {}), [refresh]);
  const entry = id != null ? lib.getEntry(id) : undefined;
  const editor = useEditor(id, {
    format: entry?.format,
    uncropped: cropTool !== null,
    maxEdge: Math.ceil(Math.max(size.w, size.h) * dpr),
    region,
    wantBefore: showBefore || split,
    onError,
    onChanged,
  });
  const { info } = editor;
  const fw = info?.fullWidth ?? 0;
  const fh = info?.fullHeight ?? 0;
  const masks = useMasks({ editor, id, onError, onNotice });
  const masksRef = useRef(masks);
  masksRef.current = masks;

  // Region of the frame visible at 100%: committed immediately on toggle/resize, debounced after a pan.
  const latest = useRef({ zoom, size, fw, fh });
  latest.current = { zoom, size, fw, fh };
  const commitRegion = useCallback(() => {
    const l = latest.current;
    setRegion(visibleRegion(l.zoom, l.size, l.fw, l.fh));
  }, []);
  useEffect(() => {
    commitRegion();
  }, [zoom.on, size, fw, fh, id, commitRegion]);
  const panTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const onPanEnd = useCallback(() => {
    clearTimeout(panTimer.current);
    panTimer.current = setTimeout(commitRegion, 120);
  }, [commitRegion]);
  useEffect(() => () => clearTimeout(panTimer.current), []);
  // New image: back to fit.
  useEffect(() => setZoom({ on: false, cx: 0.5, cy: 0.5 }), [id]);

  // ---- crop tool ----
  useEffect(() => setCropTool(null), [id]);
  const orientation = entry?.orientation ?? 1;
  const imageAspect = editor.main && editor.main.uncropped && editor.main.height > 0 ? editor.main.width / editor.main.height : 0;
  const startCrop = useCallback(() => {
    if (id == null) return;
    const c = editor.adj.crop;
    masksRef.current.endTool();
    setZoom({ on: false, cx: 0.5, cy: 0.5 });
    setShowBefore(false);
    setSplit(false);
    setSectionOpen("crop", true);
    setPicking(false);
    setCropTool({ rect: c.enabled ? fromStored(c, orientation) : FULL, angle: c.angle, aspect: loadCropAspect(), flip: false });
    // Make sure the crop controls are on screen (the panel may be scrolled to another section).
    setTimeout(() => document.querySelector('[data-testid="section-crop"]')?.scrollIntoView({ block: "nearest" }), 0);
  }, [id, editor.adj.crop, orientation]);
  const commitCrop = useCallback(() => {
    const t = cropRef.current;
    if (!t) {
      masksRef.current.endTool(); // Enter also finishes a mask tool
      return;
    }
    setCropTool(null);
    const next = isFull(t.rect) && t.angle === 0 ? { ...editor.defaults.crop } : toStored(t.rect, orientation, t.angle);
    if (JSON.stringify(next) === JSON.stringify(editor.adj.crop)) return; // nothing changed: no history entry
    editor.change((a) => ({ ...a, crop: next }), "Crop");
  }, [editor, orientation]);
  const cancelCrop = useCallback(() => {
    if (!cropRef.current) return false;
    setCropTool(null);
    return true;
  }, []);
  const imageAspectRef = useRef(imageAspect);
  imageAspectRef.current = imageAspect;
  const cropApi: CropApi = { tool: cropTool, imageAspect: imageAspect || 1.5, start: startCrop, change: setCropTool, commit: commitCrop, cancel: cancelCrop };

  const toggleZoom = useCallback((at?: { x: number; y: number }) => cropRef.current || setZoom((z) => (z.on ? { on: false, cx: 0.5, cy: 0.5 } : { on: true, cx: at?.x ?? 0.5, cy: at?.y ?? 0.5 })), []);

  // Presets and LUTs.
  const loadPresets = useCallback(() => unwrap(commands.listPresets()).then(setPresets).catch(onError), [onError]);
  const loadLuts = useCallback(() => unwrap(commands.listLuts()).then(setLuts).catch(onError), [onError]);
  useEffect(() => {
    void loadPresets();
    void loadLuts();
  }, [loadPresets, loadLuts]);

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

  const afterBatch = useCallback(
    async (t: number[]) => {
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

  const doPaste = useCallback(() => {
    const c = getClipboard();
    if (!c) return onNotice("Nothing copied yet (Cmd+Shift+C)");
    const t = targets();
    if (!t.length) return;
    void run(async () => {
      await editor.flush();
      await unwrap(commands.pasteSettings(t, c.adjustments, c.fields));
      onNotice(`Pasted ${c.fields.length} setting group${c.fields.length === 1 ? "" : "s"} to ${t.length} photo${t.length === 1 ? "" : "s"}`);
      await afterBatch(t);
    });
  }, [targets, run, editor, afterBatch, onNotice]);

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

  const doApplyPreset = (p: Preset) =>
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
      previousId = id;
    };
  }, [id]);
  const pastePrevious = useCallback(
    () =>
      void run(async () => {
        const from = previousId;
        if (from == null || from === id) return onNotice("No previous photo to paste from");
        const src = await unwrap(commands.getAdjustments(from));
        const fields = (Object.keys(FIELD_LABEL) as (keyof typeof FIELD_LABEL)[]).filter((f) => f !== "crop" && (f as string) !== "masks");
        editor.change((a) => copyFields(a, src, fields), "Paste Settings");
        onNotice("Pasted settings from the previous photo");
      }),
    [run, id, editor, onNotice],
  );

  const importLut = () =>
    void run(async () => {
      const path = await open({ title: "Import .cube LUT", filters: [{ name: "Cube LUT", extensions: ["cube"] }] });
      if (typeof path !== "string") return;
      const l = await unwrap(commands.importLut(path));
      await loadLuts();
      editor.change((a) => ({ ...a, lut: { id: l.id, amount: 100 } }), "LUT");
    });

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
        if (cropRef.current) setCropTool(null);
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
  const escape = useCallback(() => {
    if (pickerRef.current) return setPicking(false);
    if (cropRef.current) return setCropTool(null);
    const m = masksRef.current;
    if (m.tool) return m.endTool();
    if (m.open && (m.selGroup || m.selComp)) return m.select(null);
    if (m.open) m.setOpen(false);
  }, []);

  const syncTargets = useMemo(() => [...sel.selected].filter((x) => x !== id), [sel.selected, id]);

  useImperativeHandle(
    ref,
    () => ({
      toggleBefore: () => setShowBefore((v) => !v),
      toggleZoom: () => toggleZoom(),
      copy: () => setDialog({ kind: "copy" }),
      paste: doPaste,
      sync: () => (syncTargets.length > 0 ? setDialog({ kind: "sync" }) : onNotice("Cmd/Shift-click other photos in the filmstrip to sync to them")),
      reset: doReset,
      toggleSplit: () => setSplit((v) => !v),
      toggleCrop: () => (cropRef.current ? commitCrop() : startCrop()),
      commitCrop,
      cancelCrop,
      undo: editor.undo,
      redo: editor.redo,
      maskKey: (action, e) => maskKey(action, e),
      cancelMaskTool: () => {
        if (!masksRef.current.tool) return false;
        masksRef.current.endTool();
        return true;
      },
      escape,
      isCropping: () => cropRef.current !== null,
      cropSwap: () => cropRef.current && setCropTool(swapTool(cropRef.current, imageAspectRef.current || 1.5)),
      cropLock: () => cropRef.current && setCropTool(toggleLockTool(cropRef.current, imageAspectRef.current || 1.5)),
      lastCommitAt: editor.lastCommitAt,
      canRedo: editor.canRedo,
      toggleBw,
      togglePicker,
      faceZoom,
      pastePrevious,
      savePreset: () => setDialog({ kind: "preset" }),
    }),
    [toggleZoom, doPaste, doReset, syncTargets.length, onNotice, editor.undo, editor.redo, editor.lastCommitAt, editor.canRedo, commitCrop, cancelCrop, startCrop, maskKey, escape, toggleBw, togglePicker, faceZoom, pastePrevious],
  );

  const box = frameBox(zoom, size, fw, fh, editor.main);
  const frame: Frame | null = useMemo(() => (fw > 0 && fh > 0 ? { orientation, crop: editor.adj.crop, w: fw, h: fh } : null), [orientation, editor.adj.crop, fw, fh]);
  const boxRef = useRef(box);
  boxRef.current = box;
  const frameRef = useRef(frame);
  frameRef.current = frame;
  const thumb = entry?.thumbnail;
  const thumbUrl = thumb?.status === "ready" ? `${convertFileSrc(thumb.path)}?v=${id != null ? lib.version(id) : 0}` : null;

  const btn = (on = false) => `flex items-center gap-1 rounded px-2 py-1 text-xs ${on ? "bg-sky-800 text-sky-100" : "bg-neutral-800 hover:bg-neutral-700"} disabled:opacity-40`;

  return (
    <div className="absolute inset-0 z-10 flex flex-col bg-neutral-950" data-testid="develop-view" data-image-id={id ?? ""}>
      {!panels.chrome && (
      <div className="flex flex-wrap items-center gap-2 border-b border-neutral-800 px-3 py-1.5 text-neutral-300" data-testid="develop-toolbar">
        <button className={btn()} onClick={onBack} title={`Back to Library${hint("toGrid")}`} data-testid="develop-back">
          <ArrowLeft className="size-3.5" /> Library
        </button>
        <span className="text-xs text-neutral-300" data-testid="develop-filename">
          {entry?.fileName ?? ""}
        </span>
        {entry && (
          <span className="flex items-center gap-1.5" data-testid="develop-flags" data-pick={entry.pick} data-rating={entry.rating}>
            {entry.pick === "pick" && <Flag className="size-3.5 fill-green-500 text-green-500" aria-label="Picked" />}
            {entry.pick === "reject" && <X className="size-4 text-red-500" strokeWidth={3} aria-label="Rejected" />}
            <Stars n={entry.rating} />
            {entry.colorLabel && <span className={`size-2.5 rounded-full ${LABEL_COLOR[entry.colorLabel]}`} title={entry.colorLabel} />}
          </span>
        )}
        <div className="ml-4 flex gap-1">
          <button className={btn(showBefore)} onClick={() => setShowBefore((v) => !v)} title={`Before / after${hint("before")}`} data-testid="before-toggle">
            <Columns2 className="size-3.5" /> Before
          </button>
          <button className={btn(split)} onClick={() => setSplit((v) => !v)} title={`Split view${hint("split")}`} data-testid="split-toggle">
            <SplitSquareHorizontal className="size-3.5" /> Split
          </button>
          <button className={btn(zoom.on)} onClick={() => toggleZoom()} title={`Zoom to 100%${hint("zoomDevelop")}`} data-testid="zoom-toggle">
            <ZoomIn className="size-3.5" /> 100%
          </button>
        </div>
        <div className="flex gap-1">
          <button className={btn()} onClick={() => setDialog({ kind: "copy" })} title={`Copy settings${hint("copy")}`} data-testid="copy-settings">
            <ClipboardCopy className="size-3.5" /> Copy
          </button>
          <button className={btn()} disabled={!copied} onClick={doPaste} title={`Paste settings${hint("paste")}`} data-testid="paste-settings">
            <ClipboardPaste className="size-3.5" /> Paste
          </button>
          <span title={syncTargets.length === 0 ? "Cmd/Shift-click other photos in the filmstrip to sync to them" : `Sync settings to the other selected photos${hint("sync")}`}>
            <button className={`${btn()} disabled:pointer-events-none`} disabled={syncTargets.length === 0} onClick={() => setDialog({ kind: "sync" })} data-testid="sync-settings">
              <RefreshCw className="size-3.5" /> Sync
            </button>
          </span>
          <button className={btn()} onClick={doReset} title={`Reset all adjustments${hint("reset")}`} data-testid="reset-all">
            <RotateCcw className="size-3.5" /> {targets().length > 1 ? `Reset (${targets().length})` : "Reset"}
          </button>
        </div>
        <WarningsChip
          warnings={info?.warnings ?? []}
          actions={{
            ai_mask_needs_update: { label: "Update AI masks", run: () => void masksRef.current.updateAll() },
            masks_unsupported: { label: "Show in Masks panel", run: () => masksRef.current.setOpen(true) },
          }}
        />
        <span className="ml-auto text-[11px] tabular-nums text-neutral-400" data-testid="render-ms">
          {editor.main ? `${editor.main.width}x${editor.main.height} · ${Math.round(editor.main.renderMs)} ms` : ""}
        </span>
      </div>
      )}

      <div className="flex min-h-0 flex-1">
        {!panels.left && (
        <aside className="w-56 shrink-0 border-r border-neutral-800" data-testid="left-aside">
          <LeftPanel
            presets={presets}
            history={editor.history}
            onApplyPreset={doApplyPreset}
            onSavePreset={() => setDialog({ kind: "preset" })}
            onDeletePreset={(p) =>
              void run(async () => {
                await unwrap(commands.deletePreset(p.id));
                await loadPresets();
              })
            }
            onUndo={editor.undo}
            onRedo={editor.redo}
            onGoto={editor.goto}
            targetCount={targets().length}
          />
        </aside>
        )}
        <div className="relative min-w-0 flex-1">
          <Viewer
            main={editor.main}
            before={editor.before}
            detail={editor.detail}
            detailRegion={region}
            showBefore={showBefore}
            split={split}
            splitPos={splitPos}
            onSplitPos={setSplitPos}
            zoom={zoom}
            size={size}
            fw={fw}
            fh={fh}
            loading={editor.loading}
            onSize={setSize}
            onPan={setZoom}
            onPanEnd={onPanEnd}
            onToggleZoom={toggleZoom}
            inset={cropTool ? CROP_INSET : 0}
          />
          {masks.open && !cropTool && <MaskLayer masks={masks} editor={editor} id={id} frame={frame} box={box} onError={onError} />}
          {masks.open && masks.tool && !cropTool && (
            <span className="pointer-events-none absolute left-2 top-2 rounded bg-black/60 px-1.5 text-xs text-white" data-testid="mask-tool-badge" data-tool={masks.tool.kind}>
              {TOOL_HELP[masks.tool.kind]}
            </span>
          )}
          {cropTool && imageAspect > 0 && (
            <div className="absolute" style={{ inset: CROP_INSET }} data-testid="crop-inset">
              <CropOverlay tool={cropTool} size={{ w: Math.max(0, size.w - 2 * CROP_INSET), h: Math.max(0, size.h - 2 * CROP_INSET) }} imageAspect={imageAspect} onChange={setCropTool} />
            </div>
          )}
          {cropTool && <CropBar crop={cropApi} />}
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
          <PanelChevron side="left" hidden={panels.left} />
          <PanelChevron side="right" hidden={panels.right} />
        </div>
        {!panels.right && (
        <aside className="flex w-72 shrink-0 flex-col border-l border-neutral-800 min-[1600px]:w-80" data-testid="right-aside">
          <div className="flex gap-1 border-b border-neutral-800 px-3 py-1.5" role="tablist" aria-label="Develop panels">
            <button role="tab" aria-selected={!masks.open} className={`flex-1 rounded px-2 py-1 text-xs ${!masks.open ? "bg-sky-800 text-sky-100" : "bg-neutral-800 hover:bg-neutral-700"}`} onClick={() => { masks.endTool(); masks.setOpen(false); }} data-testid="panel-tab-adjust">
              Adjust
            </button>
            <button role="tab" aria-selected={masks.open} className={`flex-1 rounded px-2 py-1 text-xs ${masks.open ? "bg-sky-800 text-sky-100" : "bg-neutral-800 hover:bg-neutral-700"}`} onClick={() => masks.setOpen(true)} title={`Masks: local adjustments${hint("maskPanel")}`} data-testid="panel-tab-masks">
              Masks{masks.groups.length > 0 ? ` (${masks.groups.length})` : ""}
            </button>
          </div>
          <div className="min-h-0 flex-1">
            {masks.open ? <MasksPanel masks={masks} /> : <AdjustPanel editor={editor} luts={luts} onImportLut={importLut} imageId={id} onError={onError} crop={cropApi} picker={{ active: picking, toggle: togglePicker }} />}
          </div>
        </aside>
        )}
      </div>

      {!panels.chrome && (
        <Filmstrip lib={lib} activeId={id} selected={sel.selected} onPick={(fid, ev) => sel.click(fid, { shift: ev.shiftKey, meta: ev.metaKey || ev.ctrlKey })} cellW={FILM} cellH={FILM} height={88} scenePrefix="film-scene" />
      )}

      {masks.picker && id != null && (
        <PeoplePicker imageId={id} thumbUrl={thumbUrl} caps={masks.caps} onCreate={(pt, parts, name) => void masks.createPeople(pt, parts, name)} onCancel={masks.closePicker} onError={onError} />
      )}
      {dialog?.kind === "copy" && (
        <FieldsDialog
          title="Copy Settings"
          confirm="Copy"
          onCancel={() => setDialog(null)}
          onConfirm={(fields) => {
            setClipboard({ adjustments: structuredClone(editor.adj), fields });
            setDialog(null);
            onNotice(`Copied ${fields.length} setting group${fields.length === 1 ? "" : "s"}`);
          }}
        />
      )}
      {dialog?.kind === "sync" && id != null && (
        <FieldsDialog
          title={`Sync Settings to ${syncTargets.length} photo${syncTargets.length === 1 ? "" : "s"}`}
          confirm="Sync"
          onCancel={() => setDialog(null)}
          onConfirm={(fields) => {
            setDialog(null);
            void run(async () => {
              await editor.flush();
              await unwrap(commands.syncSettings(id, syncTargets, fields));
              onNotice(`Synced settings to ${syncTargets.length} photo${syncTargets.length === 1 ? "" : "s"}`);
              await afterBatch(syncTargets);
            });
          }}
        />
      )}
      {dialog?.kind === "preset" && (
        <FieldsDialog
          title="Save Preset"
          confirm="Save"
          withName
          onCancel={() => setDialog(null)}
          onConfirm={(fields, name) => {
            setDialog(null);
            void run(async () => {
              await unwrap(commands.savePreset(null, name, editor.adj, fields));
              await loadPresets();
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
