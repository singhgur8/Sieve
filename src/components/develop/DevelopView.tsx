// Develop module: filmstrip + viewer (before/after, split, 100% detail) + presets/history + adjustment sliders.
import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { open } from "@tauri-apps/plugin-dialog";
import { ArrowLeft, ClipboardCopy, ClipboardPaste, Columns2, RefreshCw, RotateCcw, SplitSquareHorizontal, ZoomIn } from "lucide-react";
import { commands, convertFileSrc, unwrap, type AdjustmentField, type LutInfo, type NormRect, type ParametricAdjustments, type Preset } from "../../ipc";
import type { Library } from "../../hooks/useLibrary";
import type { SelectionApi } from "../../hooks/useSelection";
import { useEditor } from "../../hooks/useEditor";
import { SceneBadge } from "../Cell";
import { AdjustPanel } from "./AdjustPanel";
import { LeftPanel } from "./LeftPanel";
import { FieldsDialog } from "./FieldsDialog";
import { Viewer, visibleRegion, type Size, type Zoom } from "./Viewer";

export interface DevelopHandle {
  toggleBefore: () => void;
  toggleZoom: () => void;
  copy: () => void;
  paste: () => void;
  undo: () => void;
  redo: () => void;
}

interface Copied {
  adjustments: ParametricAdjustments;
  fields: AdjustmentField[];
}
// Copied settings survive leaving and re-entering the Develop module.
let clipboard: Copied | null = null;

type Dialog = { kind: "copy" | "sync" | "preset" } | null;

interface Props {
  lib: Library;
  sel: SelectionApi;
  onError: (e: unknown) => void;
  onNotice: (s: string) => void;
  onBack: () => void;
}

const FILM = 72;

export const DevelopView = forwardRef<DevelopHandle, Props>(function DevelopView({ lib, sel, onError, onNotice, onBack }, ref) {
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
  const [copied, setCopied] = useState<Copied | null>(clipboard);
  const dpr = typeof window === "undefined" ? 1 : window.devicePixelRatio || 1;

  const { refresh } = lib;
  const onChanged = useCallback((changed: number) => void refresh([changed]).catch(() => {}), [refresh]);
  const editor = useEditor(id, {
    maxEdge: Math.ceil(Math.max(size.w, size.h) * dpr),
    region,
    wantBefore: showBefore || split,
    onError,
    onChanged,
  });
  const { info } = editor;
  const fw = info?.fullWidth ?? 0;
  const fh = info?.fullHeight ?? 0;

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

  const toggleZoom = useCallback((at?: { x: number; y: number }) => setZoom((z) => (z.on ? { on: false, cx: 0.5, cy: 0.5 } : { on: true, cx: at?.x ?? 0.5, cy: at?.y ?? 0.5 })), []);

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
    if (!clipboard) return onNotice("Nothing copied yet (Cmd+Shift+C)");
    const t = targets();
    if (!t.length) return;
    const c = clipboard;
    void run(async () => {
      await editor.flush();
      await unwrap(commands.pasteSettings(t, c.adjustments, c.fields));
      onNotice(`Pasted ${c.fields.length} setting group${c.fields.length === 1 ? "" : "s"} to ${t.length} photo${t.length === 1 ? "" : "s"}`);
      await afterBatch(t);
    });
  }, [targets, run, editor, afterBatch, onNotice]);

  const doReset = () =>
    void run(async () => {
      const t = targets();
      await editor.flush();
      await unwrap(commands.resetAdjustments(t));
      await afterBatch(t);
    });

  const doApplyPreset = (p: Preset) =>
    void run(async () => {
      const t = targets();
      await editor.flush();
      await unwrap(commands.applyPreset(t, p.id));
      await afterBatch(t);
    });

  const importLut = () =>
    void run(async () => {
      const path = await open({ title: "Import .cube LUT", filters: [{ name: "Cube LUT", extensions: ["cube"] }] });
      if (typeof path !== "string") return;
      const l = await unwrap(commands.importLut(path));
      await loadLuts();
      editor.change((a) => ({ ...a, lut: { id: l.id, amount: 100 } }), "LUT");
    });

  const syncTargets = useMemo(() => [...sel.selected].filter((x) => x !== id), [sel.selected, id]);

  useImperativeHandle(
    ref,
    () => ({
      toggleBefore: () => setShowBefore((v) => !v),
      toggleZoom: () => toggleZoom(),
      copy: () => setDialog({ kind: "copy" }),
      paste: doPaste,
      undo: editor.undo,
      redo: editor.redo,
    }),
    [toggleZoom, doPaste, editor.undo, editor.redo],
  );

  // ---- filmstrip ----
  const stripRef = useRef<HTMLDivElement>(null);
  const virt = useVirtualizer({ count: lib.ids.length, horizontal: true, getScrollElement: () => stripRef.current, estimateSize: () => FILM + 4, overscan: 6 });
  const vitems = virt.getVirtualItems();
  const { ensure } = lib;
  const firstV = vitems[0]?.index ?? 0;
  const lastV = vitems.length ? vitems[vitems.length - 1].index + 1 : 0;
  useEffect(() => ensure(lib.ids.slice(firstV, lastV)), [ensure, lib.ids, firstV, lastV]);
  const activeIndex = id == null ? -1 : lib.ids.indexOf(id);
  useEffect(() => {
    if (activeIndex >= 0) virt.scrollToIndex(activeIndex, { align: "auto" });
  }, [activeIndex, virt]);

  const entry = id != null ? lib.getEntry(id) : undefined;
  const btn = (on = false) => `flex items-center gap-1 rounded px-2 py-1 text-xs ${on ? "bg-sky-800 text-sky-100" : "bg-neutral-800 hover:bg-neutral-700"} disabled:opacity-40`;

  return (
    <div className="absolute inset-0 z-10 flex flex-col bg-neutral-950" data-testid="develop-view" data-image-id={id ?? ""}>
      <div className="flex flex-wrap items-center gap-2 border-b border-neutral-800 px-3 py-1.5 text-neutral-300">
        <button className={btn()} onClick={onBack} title="Back to Library (G)" data-testid="develop-back">
          <ArrowLeft className="size-3.5" /> Library
        </button>
        <span className="text-xs text-neutral-500" data-testid="develop-filename">
          {entry?.fileName ?? ""}
        </span>
        <div className="ml-4 flex gap-1">
          <button className={btn(showBefore)} onClick={() => setShowBefore((v) => !v)} title="Before / after (\)" data-testid="before-toggle">
            <Columns2 className="size-3.5" /> Before
          </button>
          <button className={btn(split)} onClick={() => setSplit((v) => !v)} title="Split view" data-testid="split-toggle">
            <SplitSquareHorizontal className="size-3.5" /> Split
          </button>
          <button className={btn(zoom.on)} onClick={() => toggleZoom()} title="Zoom to 100% (Z)" data-testid="zoom-toggle">
            <ZoomIn className="size-3.5" /> 100%
          </button>
        </div>
        <div className="flex gap-1">
          <button className={btn()} onClick={() => setDialog({ kind: "copy" })} title="Copy settings (Cmd+Shift+C)" data-testid="copy-settings">
            <ClipboardCopy className="size-3.5" /> Copy
          </button>
          <button className={btn()} disabled={!copied} onClick={doPaste} title="Paste settings (Cmd+Shift+V)" data-testid="paste-settings">
            <ClipboardPaste className="size-3.5" /> Paste
          </button>
          <button className={btn()} disabled={syncTargets.length === 0} onClick={() => setDialog({ kind: "sync" })} title="Sync settings to the other selected photos" data-testid="sync-settings">
            <RefreshCw className="size-3.5" /> Sync
          </button>
          <button className={btn()} onClick={doReset} title="Reset all adjustments" data-testid="reset-all">
            <RotateCcw className="size-3.5" /> Reset
          </button>
        </div>
        <span className="ml-auto text-[11px] tabular-nums text-neutral-600" data-testid="render-ms">
          {editor.main ? `${editor.main.width}x${editor.main.height} · ${Math.round(editor.main.renderMs)} ms` : ""}
        </span>
      </div>

      <div className="flex min-h-0 flex-1">
        <aside className="w-56 shrink-0 border-r border-neutral-800">
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
          />
        </aside>
        <div className="min-w-0 flex-1">
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
          />
        </div>
        <aside className="w-72 shrink-0 border-l border-neutral-800">
          <AdjustPanel editor={editor} luts={luts} onImportLut={importLut} />
        </aside>
      </div>

      <div ref={stripRef} className="h-[88px] shrink-0 overflow-x-auto overflow-y-hidden border-t border-neutral-800 bg-neutral-900" data-testid="filmstrip">
        <div style={{ width: virt.getTotalSize(), height: "100%", position: "relative" }}>
          {vitems.map((v) => {
            const fid = lib.ids[v.index];
            const e = lib.getEntry(fid);
            const t = e?.thumbnail;
            const active = fid === id;
            return (
              <button
                key={v.key}
                data-testid={`film-${fid}`}
                data-active={active}
                data-selected={sel.selected.has(fid)}
                onClick={(ev) => sel.click(fid, { shift: ev.shiftKey, meta: ev.metaKey || ev.ctrlKey })}
                className={`absolute top-2 overflow-hidden rounded bg-neutral-800 ${sel.selected.has(fid) ? "ring-2 ring-sky-500" : ""} ${active ? "outline outline-2 outline-white" : ""}`}
                style={{ left: v.start, width: FILM, height: FILM }}
              >
                {t?.status === "ready" && <img src={`${convertFileSrc(t.path)}?v=${lib.version(fid)}`} alt="" draggable={false} className="size-full object-cover" />}
                {e && e.sceneId != null && (
                  <span className="absolute bottom-0.5 left-0.5">
                    <SceneBadge entry={e} testPrefix="film-scene" />
                  </span>
                )}
                {e?.hasEdits && <span className="absolute right-0.5 top-0.5 size-2 rounded-full bg-sky-400" title="Edited" data-testid={`film-edited-${fid}`} />}
              </button>
            );
          })}
        </div>
      </div>

      {dialog?.kind === "copy" && (
        <FieldsDialog
          title="Copy Settings"
          confirm="Copy"
          onCancel={() => setDialog(null)}
          onConfirm={(fields) => {
            clipboard = { adjustments: structuredClone(editor.adj), fields };
            setCopied(clipboard);
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
