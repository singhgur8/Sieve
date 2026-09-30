// Library shell: virtualized grid, filter bars, loupe / compare / develop, and the single keymap-driven shortcut handler.
import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { commands, unwrap, type ColorLabel, type Scene, type PickFlag, type RawImageEntry, type ShootType } from "./ipc";
import { BASE_QUERY, useLibrary, type Query } from "./hooks/useLibrary";
import { useSelection } from "./hooks/useSelection";
import { useBackendStatus } from "./hooks/useBackendStatus";
import { useKeyboard } from "./hooks/useKeyboard";
import { useCullUndo } from "./hooks/useCullUndo";
import { TopBar } from "./components/TopBar";
import { FilterBar, FilterExtras, FilterSummary, isFiltered, useFilterCounts } from "./components/FilterBar";
import { GridToolbar, type Mode } from "./components/GridToolbar";
import { PhotoGrid } from "./components/PhotoGrid";
import { LoupeLayer, type CompareState, type LoupeHandle } from "./components/LoupeLayer";
import { DevelopView, type DevelopHandle } from "./components/develop/DevelopView";
import { AnalysisBar, ImportBar } from "./components/ProgressBars";
import { useImportOptions } from "./lib/importOptions";
import { ExportDialog } from "./components/export/ExportDialog";
import { ExportJobsPanel } from "./components/export/ExportJobsPanel";
import { useExportJobs } from "./hooks/useExportJobs";
import { useScenes } from "./hooks/useScenes";
import { SceneStrip } from "./components/scenes/SceneStrip";
import { MatchPanel } from "./components/scenes/MatchPanel";
import { Toasts, useToasts } from "./components/Toasts";
import { CheatSheet } from "./components/CheatSheet";
import { ApplySuggestionsDialog } from "./components/ApplySuggestionsDialog";
import { matchKey } from "./lib/keymap";
import { modalCount } from "./lib/modal";
import { getClipboard } from "./lib/clipboard";
import { toggleChrome, toggleSidePanels, usePanels } from "./lib/panels";

const LABEL_KEYS: Record<string, ColorLabel> = { "6": "red", "7": "yellow", "8": "green", "9": "blue" };

const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;

export default function App() {
  const [query, setQuery] = useState<Query>(BASE_QUERY);
  const [mode, setMode] = useState<Mode>("grid");
  const [cmp, setCmp] = useState<CompareState | null>(null);
  const [size, setSize] = useState(200);
  const [autoAdvance, setAutoAdvance] = useState(false);
  const [busy, setBusy] = useState(false);
  const [importOpts, setImportOpts] = useImportOptions();
  const importOptsRef = useRef(importOpts);
  importOptsRef.current = importOpts;
  const [filtersOpen, setFiltersOpen] = useState(true);
  const [exportOpen, setExportOpen] = useState<number[] | null>(null);
  const [applyOpen, setApplyOpen] = useState<{ selected: number[]; all: number[] } | null>(null);
  const [cheatOpen, setCheatOpen] = useState(false);
  const [matchOpen, setMatchOpen] = useState<number | null>(null);
  const [devEpoch, setDevEpoch] = useState(0);
  const [caps, setCaps] = useState(false);
  const devPanels = usePanels("develop");
  const loupePanels = usePanels("loupe");
  const lastUndone = useRef<"cull" | "adj">("adj");
  const colsRef = useRef({ cols: 1, page: 1 });
  const loupe = useRef<LoupeHandle>(null);
  const develop = useRef<DevelopHandle>(null);
  const reloadRef = useRef<() => void>(() => {});

  const toasts = useToasts();
  const { push } = toasts;
  const setNotice = useCallback((m: string) => void push(m), [push]);

  const onLibraryChanged = useCallback(() => reloadRef.current(), []);
  const status = useBackendStatus(onLibraryChanged);
  const { reportError, setError } = status;
  const lib = useLibrary(query, reportError);
  reloadRef.current = () => void lib.reload();
  const { ids } = lib;
  const exportJobs = useExportJobs(reportError);
  const sel = useSelection(ids);
  const scenes = useScenes(query.folderId, query.sceneId ?? null, lib, reportError, setNotice);
  const counts = useFilterCounts(query.folderId, lib.epoch);
  const matchScene: Scene | undefined = matchOpen != null ? scenes.scenes.find((s) => s.id === matchOpen) : undefined;

  const active = mode === "compare" && cmp ? cmp[cmp.focus] : sel.active;
  const membershipSensitive =
    query.picks.length > 0 || query.minRating != null || query.maxRating != null || query.colorLabels.length > 0 || query.sort === "rating";

  // Caps Lock acts as auto-advance while on (Lightroom).
  useEffect(() => {
    const sync = (e: KeyboardEvent) => e.getModifierState && setCaps(e.getModifierState("CapsLock"));
    window.addEventListener("keydown", sync, true);
    window.addEventListener("keyup", sync, true);
    return () => {
      window.removeEventListener("keydown", sync, true);
      window.removeEventListener("keyup", sync, true);
    };
  }, []);

  // Keep entries needed outside the grid range (active image, compare panes) loaded.
  const { pin } = lib;
  useEffect(() => {
    pin([sel.active, cmp?.a, cmp?.b].filter((x): x is number => x != null));
  }, [pin, sel.active, cmp?.a, cmp?.b]);

  // If the active image left the result set (filter/rating change), move to its neighbour.
  const prevIds = useRef<number[]>([]);
  const { active: selActive, set: selSet, clear: selClear } = sel;
  useEffect(() => {
    const prev = prevIds.current;
    prevIds.current = ids;
    if (selActive != null && lib.loaded && !ids.includes(selActive)) {
      const at = Math.min(Math.max(prev.indexOf(selActive), 0), ids.length - 1);
      if (ids[at] != null) selSet([ids[at]], ids[at]);
      else selClear();
    }
  }, [ids, lib.loaded, selActive, selSet, selClear]);

  // A scene filter that no longer exists (deleted, merged away, other folder) is dropped.
  const { scenes: sceneList } = scenes;
  useEffect(() => {
    if (query.sceneId != null && !sceneList.some((s) => s.id === query.sceneId)) setQuery((q) => (q.sceneId == null ? q : { ...q, sceneId: null }));
  }, [sceneList, query.sceneId]);

  const targets = useCallback((): number[] => {
    if (mode === "compare" && cmp) return [cmp[cmp.focus]];
    if (mode === "loupe" || mode === "develop") return sel.active != null ? [sel.active] : [];
    if (sel.selected.size > 0) return [...sel.selected];
    return sel.active != null ? [sel.active] : [];
  }, [mode, cmp, sel.active, sel.selected]);

  const step = useCallback(
    (delta: number, extend = false) => {
      if (ids.length === 0) return;
      const i = sel.active != null ? ids.indexOf(sel.active) : -1;
      const n = i < 0 ? 0 : Math.min(ids.length - 1, Math.max(0, i + delta));
      sel.moveTo(ids[n], extend);
    },
    [ids, sel],
  );

  const stepCompare = useCallback(
    (dir: 1 | -1) => {
      if (!cmp) return;
      const other = cmp.focus === "a" ? cmp.b : cmp.a;
      let i = cmp.pool.indexOf(cmp[cmp.focus]) + dir;
      while (i >= 0 && i < cmp.pool.length && cmp.pool[i] === other) i += dir;
      if (i < 0 || i >= cmp.pool.length) return;
      const id = cmp.pool[i];
      setCmp({ ...cmp, [cmp.focus]: id });
      sel.set([id], id);
    },
    [cmp, sel],
  );

  const openLoupe = useCallback(
    (id?: number) => {
      const target = id ?? sel.active ?? ids[0];
      if (target == null) return;
      sel.set([target], target);
      setCmp(null);
      setMode("loupe");
    },
    [sel, ids],
  );

  const enterCompare = useCallback(async () => {
    try {
      let a: number | undefined;
      let b: number | undefined;
      let pool = ids;
      if (mode === "grid" && sel.selected.size === 2) {
        [a, b] = [...sel.selected];
      } else {
        a = sel.active ?? ids[0];
        if (a == null) return;
        const entry = lib.getEntry(a);
        if (entry?.burstGroupId != null) {
          const groups = await unwrap(commands.listBurstGroups(query.folderId));
          const g = groups.find((x) => x.id === entry.burstGroupId);
          if (g) {
            pool = g.imageIds;
            b = g.keeperImageId != null && g.keeperImageId !== a ? g.keeperImageId : g.imageIds.find((x) => x !== a);
          }
        }
        if (b == null) {
          const i = ids.indexOf(a);
          b = ids[i + 1] ?? ids[i - 1];
        }
      }
      if (a == null || b == null) {
        setNotice("Select two photos (or a burst member) to compare");
        return;
      }
      setCmp({ pool, a, b, focus: "a" });
      sel.set([a], a);
      setMode("compare");
    } catch (e) {
      reportError(e);
    }
  }, [ids, mode, sel, lib, query.folderId, reportError, setNotice]);

  const openDevelop = useCallback(() => {
    const target = sel.active ?? ids[0];
    if (target == null) return;
    // Keep a multi-selection (for sync / paste); otherwise select just the image.
    if (!sel.selected.has(target)) sel.set([target], target);
    setCmp(null);
    setMode("develop");
  }, [sel, ids]);

  const changeMode = useCallback(
    (m: Mode) => {
      if (m === "grid") {
        setMode("grid");
        setCmp(null);
      } else if (m === "develop") openDevelop();
      else if (m === "loupe") openLoupe();
      else void enterCompare();
    },
    [openLoupe, openDevelop, enterCompare],
  );

  // ---- culling actions (batch over targets), all undoable ----
  const onRestored = useCallback(
    (changed: number[]) => {
      void lib.refresh(changed.filter((id) => lib.getEntry(id)).slice(0, 2000)).catch(reportError);
      if (membershipSensitive) void lib.reload();
      status.refreshXmp();
    },
    [lib, reportError, membershipSensitive, status],
  );
  const cull = useCullUndo({ getEntry: lib.getEntry, onRestored, toast: setNotice, onError: reportError });

  const advanceIf = useCallback(
    (t: number[], force: boolean) => {
      if ((force || autoAdvance || caps) && t.length === 1) {
        if (mode === "compare") stepCompare(1);
        else step(1);
      }
    },
    [autoAdvance, caps, mode, step, stepCompare],
  );

  const mutate = useCallback(
    async (label: string, t: number[], optimistic: (e: RawImageEntry) => RawImageEntry, call: () => Promise<unknown>) => {
      let before = null;
      try {
        before = await cull.capture(t);
      } catch (e) {
        reportError(e);
      }
      const changes = before?.some((s) => {
        const e = lib.getEntry(s.imageId);
        if (!e) return true;
        const n = optimistic(e);
        return n.pick !== s.pick || n.rating !== s.rating || n.colorLabel !== s.colorLabel;
      });
      lib.patch(t, optimistic);
      try {
        await call();
        if (before && changes) cull.record(label, before);
      } catch (e) {
        reportError(e);
      }
      try {
        await lib.refresh(t);
      } catch (e) {
        reportError(e);
      }
      if (membershipSensitive) void lib.reload();
    },
    [lib, cull, reportError, membershipSensitive],
  );

  /** "DSC00010.ARW" for one photo, "3 photos" otherwise (undo toasts: "Undid: Reject DSC00010.ARW"). */
  const what = useCallback((t: number[]) => (t.length === 1 ? (lib.getEntry(t[0])?.fileName ?? "1 photo") : plural(t.length, "photo")), [lib]);

  const doPick = useCallback(
    (pick: PickFlag, advance: boolean) => {
      const t = targets();
      if (t.length === 0) return;
      const label = `${{ pick: "Pick", reject: "Reject", unflagged: "Unflag" }[pick]} ${what(t)}`;
      void mutate(label, t, (e) => ({ ...e, pick }), () => unwrap(commands.setPick(t, pick)));
      advanceIf(t, advance);
    },
    [targets, mutate, advanceIf, what],
  );

  const doRating = useCallback(
    (rating: number) => {
      const t = targets();
      if (t.length === 0) return;
      void mutate(`Rate ${what(t)} ${rating}★`, t, (e) => ({ ...e, rating }), () => unwrap(commands.setRating(t, rating)));
      advanceIf(t, false);
    },
    [targets, mutate, advanceIf, what],
  );

  const doLabel = useCallback(
    (label: ColorLabel) => {
      const t = targets();
      if (t.length === 0) return;
      const allHave = t.every((id) => lib.getEntry(id)?.colorLabel === label);
      const next = allHave ? null : label;
      void mutate(`Label ${what(t)} ${next ?? "none"}`, t, (e) => ({ ...e, colorLabel: next }), () => unwrap(commands.setColorLabel(t, next)));
      advanceIf(t, false);
    },
    [targets, mutate, advanceIf, lib, what],
  );

  /** Make `id` the keeper of its burst (optionally Pick it too: Compare's K). */
  const setKeeper = useCallback(
    async (id: number | null, alsoPick: boolean) => {
      if (id == null) return;
      const entry = lib.getEntry(id);
      if (!entry || entry.burstGroupId == null) return setNotice("This photo is not part of a burst");
      try {
        const g = await unwrap(commands.setBurstKeeper(entry.burstGroupId, id));
        if (alsoPick) {
          void mutate(`Pick ${entry.fileName}`, [id], (e) => ({ ...e, pick: "pick" }), () => unwrap(commands.setPick([id], "pick")));
        }
        await lib.refresh(g.imageIds.filter((x) => lib.getEntry(x)));
        setNotice(`${entry.fileName} is now the burst keeper`);
      } catch (e) {
        reportError(e);
      }
    },
    [lib, mutate, reportError, setNotice],
  );

  // ---- XMP ----
  const writeXmp = useCallback(async () => {
    const t = targets();
    if (t.length === 0) return;
    try {
      const r = await unwrap(commands.writeXmp(t));
      setNotice(`Saved metadata for ${r.succeeded} photo${r.succeeded === 1 ? "" : "s"}${r.failed.length ? `, ${r.failed.length} failed: ${r.failed[0].reason}` : ""}`);
      await lib.refresh(t);
      status.refreshXmp();
    } catch (e) {
      reportError(e);
    }
  }, [targets, lib, status, reportError, setNotice]);

  const saveAllDirty = useCallback(async () => {
    try {
      const r = await unwrap(commands.writeXmpAllDirty(null));
      setNotice(`Saved XMP for ${plural(r.succeeded, "photo")}${r.failed.length ? `, ${r.failed.length} failed: ${r.failed[0].reason}` : ""}`);
      status.refreshXmp();
      await lib.refreshAll();
    } catch (e) {
      reportError(e);
    }
  }, [lib, status, reportError, setNotice]);

  const readXmp = useCallback(async () => {
    const t = targets();
    if (t.length === 0) return;
    try {
      const r = await unwrap(commands.readXmp(t));
      setNotice(`Read ${r.changed.length} changed, ${r.skipped} without sidecar`);
      await lib.refresh(t);
      if (membershipSensitive) void lib.reload();
      status.refreshXmp();
    } catch (e) {
      reportError(e);
    }
  }, [targets, lib, status, reportError, membershipSensitive, setNotice]);

  const openExport = useCallback(() => {
    if (ids.length === 0) return;
    setExportOpen(targets());
  }, [ids.length, targets]);

  // ---- top-bar actions ----
  const run = useCallback(
    async (fn: () => Promise<unknown>) => {
      setError(null);
      try {
        await fn();
      } catch (e) {
        reportError(e);
      }
    },
    [setError, reportError],
  );

  const importFolder = useCallback(
    () =>
      void run(async () => {
        const path = await open({ directory: true, title: "Import RAW folder" });
        if (typeof path !== "string") return;
        setBusy(true);
        try {
          const s = await unwrap(commands.importFolder(path, importOptsRef.current));
          setNotice(`Imported ${s.added} new (${s.skipped} already known, ${s.sidecarsRead} sidecars read${s.companions > 0 ? `, ${s.companions} JPEG pairs` : ""})`);
          await status.refreshCatalog();
          await lib.reload();
        } finally {
          setBusy(false);
        }
      }),
    [run, status, lib, setNotice],
  );

  const analyze = (kind: "pending" | "all") =>
    void run(async () => {
      status.setAnalysis((a) => ({ done: 0, total: a?.total ?? 0, failed: 0, running: true }));
      await unwrap(commands.analyzeImages({ kind }));
    });

  const askApplySuggestions = () => {
    if (ids.length === 0) return;
    setApplyOpen({ selected: [...sel.selected], all: ids });
  };

  const applySuggestions = (t: number[], onlyUnset: boolean) =>
    void run(async () => {
      const before = await unwrap(commands.getCullSnapshot(t));
      const { applied, skipped } = await unwrap(commands.applySuggestions(t, onlyUnset));
      await lib.refresh(t.filter((id) => lib.getEntry(id)).slice(0, 2000));
      if (membershipSensitive) void lib.reload();
      status.refreshXmp();
      if (applied === 0) {
        setNotice(`Nothing to apply (${skipped} skipped)`);
        return;
      }
      const entry = cull.record(`Apply suggestions to ${plural(applied, "photo")}`, before);
      push(`Applied suggestions to ${applied} of ${t.length} photos${skipped ? ` (${skipped} skipped)` : ""}`, {
        action: { label: "Undo", testid: "apply-undo", onClick: () => void cull.undoEntry(entry) },
      });
    });

  const revealInFinder = (path: string) => void run(() => unwrap(commands.revealInFinder(path)));

  const pasteToSelection = useCallback(async () => {
    const c = getClipboard();
    if (!c) return setNotice("Nothing copied yet (Cmd+Shift+C in Develop)");
    const t = targets();
    if (t.length === 0) return;
    try {
      await unwrap(commands.pasteSettings(t, c.adjustments, c.fields));
      setNotice(`Pasted ${plural(c.fields.length, "setting group")} to ${plural(t.length, "photo")}`);
      await lib.refresh(t.filter((id) => lib.getEntry(id)).slice(0, 2000));
    } catch (e) {
      reportError(e);
    }
  }, [targets, lib, reportError, setNotice]);

  const selectBurst = useCallback(async () => {
    const id = active;
    const entry = id != null ? lib.getEntry(id) : undefined;
    if (id == null || entry?.burstGroupId == null) return setNotice("The active photo is not part of a burst");
    try {
      const groups = await unwrap(commands.listBurstGroups(query.folderId));
      const g = groups.find((x) => x.id === entry.burstGroupId);
      const members = g ? g.imageIds.filter((x) => ids.includes(x)) : [];
      if (members.length === 0) return setNotice("Burst members are hidden by the current filters");
      sel.set(members, id);
      setNotice(`Selected ${plural(members.length, "photo")} of the burst`);
    } catch (e) {
      reportError(e);
    }
  }, [active, lib, query.folderId, ids, sel, reportError, setNotice]);

  // ---- keyboard: one handler driven by the shared keymap ----
  useKeyboard((e) => {
    if (modalCount() > 0) return; // dialogs and menus own the keyboard
    const def = matchKey(e, mode, { cropping: mode === "develop" && !!develop.current?.isCropping() });
    if (!def) return;
    e.preventDefault();
    const k = e.key;
    switch (def.id) {
      case "pick":
        return doPick("pick", e.shiftKey);
      case "reject":
        return doPick("reject", e.shiftKey);
      case "unflag":
        return doPick("unflagged", false);
      case "rate":
        return doRating(Number(k));
      case "label":
        return doLabel(LABEL_KEYS[k]);
      case "keeper":
        return void setKeeper(cmp ? cmp[cmp.focus] : null, true);
      case "keeperSet":
        return void setKeeper(active ?? null, false);
      case "undoCull":
        return void cull.undo();
      case "redoCull":
        return void cull.redo();
      case "anchor":
        return void scenes.toggleAnchor(active ?? null);
      case "selectBurst":
        return void selectBurst();
      case "navH": {
        const dir = k === "ArrowRight" ? 1 : -1;
        if (mode === "compare") stepCompare(dir);
        else step(dir, e.shiftKey && mode === "grid");
        return;
      }
      case "navV":
        return step(k === "ArrowDown" ? colsRef.current.cols : -colsRef.current.cols, e.shiftKey);
      case "gridJump": {
        const page = colsRef.current.page;
        const delta = { Home: -Infinity, End: Infinity, PageUp: -page, PageDown: page }[k] ?? 0;
        return step(Math.max(-1e9, Math.min(1e9, delta)), e.shiftKey);
      }
      case "toggleLoupe":
        if (mode === "grid") openLoupe();
        else changeMode("grid");
        return;
      case "devToLoupe":
        return openLoupe();
      case "toGrid":
        return changeMode("grid");
      case "escape":
        if (mode !== "grid") changeMode("grid");
        else sel.clear();
        return;
      case "developEscape":
        return develop.current?.escape();
      case "cropSwap":
        return develop.current?.cropSwap();
      case "cropLock":
        return develop.current?.cropLock();
      case "panelsToggle":
        return toggleSidePanels();
      case "panelsHide":
        return toggleChrome(mode === "loupe" ? "loupe" : "develop");
      case "bwToggle":
        return develop.current?.toggleBw();
      case "wbPicker":
        return develop.current?.togglePicker();
      case "faceDevelop":
        return develop.current?.faceZoom(e.shiftKey ? -1 : 1);
      case "pastePrev":
        return develop.current?.pastePrevious();
      case "savePreset":
        return develop.current?.savePreset();
      case "develop":
        return openDevelop();
      case "compare":
        if (mode === "compare") openLoupe(cmp ? cmp[cmp.focus] : undefined);
        else void enterCompare();
        return;
      case "tab":
        if (cmp) {
          const focus = cmp.focus === "a" ? "b" : "a";
          setCmp({ ...cmp, focus });
          sel.set([cmp[focus]], cmp[focus]);
        }
        return;
      case "selectAll":
        return sel.selectAll();
      case "selectNone":
        return sel.clear();
      case "filterBar":
        if (mode === "grid") setFiltersOpen((v) => !v);
        else {
          changeMode("grid");
          setFiltersOpen(true);
        }
        return;
      case "zoomLoupe":
        if (mode === "grid") openLoupe();
        setTimeout(() => loupe.current?.toggleZoom(), mode === "grid" ? 120 : 0);
        return;
      case "zoomDevelop":
        return develop.current?.toggleZoom();
      case "face":
        return loupe.current?.cycleFace(e.shiftKey ? -1 : 1);
      case "info":
        return loupe.current?.cycleInfo();
      case "crop":
        return develop.current?.toggleCrop();
      case "cropCommit":
        return void develop.current?.commitCrop();
      case "before":
        return develop.current?.toggleBefore();
      case "split":
        return develop.current?.toggleSplit();
      case "undoAdj": {
        // One undo in Develop: the newest of the last culling change and the last adjustment.
        const d = develop.current;
        if (cull.undoAt() > (d?.lastCommitAt() ?? 0)) {
          lastUndone.current = "cull";
          return void cull.undo();
        }
        lastUndone.current = "adj";
        return d?.undo();
      }
      case "redoAdj": {
        const d = develop.current;
        if (cull.redoAt() > 0 && (!d?.canRedo() || lastUndone.current === "cull")) return void cull.redo();
        return d?.redo();
      }
      case "copy":
        return develop.current?.copy();
      case "paste":
        if (mode === "develop") develop.current?.paste();
        else void pasteToSelection();
        return;
      case "sync":
        return develop.current?.sync();
      case "reset":
        return develop.current?.reset();
      case "maskPanel":
      case "maskBrush":
      case "maskLinear":
      case "maskRadial":
      case "maskColor":
      case "maskLuminance":
      case "maskOverlay":
      case "maskOverlayStyle":
      case "maskPins":
      case "maskSize":
      case "maskFeather":
      case "maskAuto":
      case "maskDelete":
        return develop.current?.maskKey(def.id, e);
      case "saveXmp":
        return void writeXmp();
      case "export":
        return openExport();
      case "import":
        return importFolder();
      case "cheatSheet":
        return setCheatOpen(true);
    }
  });

  const importActive = status.progress !== null && status.progress.done < status.progress.total;
  const catalog = status.catalog;
  const running = exportJobs.jobs.filter((j) => j.running);
  const exportPct = running.length ? Math.round((running.reduce((a, j) => a + j.done, 0) / Math.max(1, running.reduce((a, j) => a + j.total, 0))) * 100) : null;
  const filtered = isFiltered(query);
  const clearFilters = () => setQuery((q) => ({ ...BASE_QUERY, sort: q.sort, sortDescending: q.sortDescending }));

  return (
    <main className="flex h-screen flex-col">
      <TopBar
        catalog={catalog}
        analysis={status.analysis}
        xmp={status.xmp}
        busy={busy}
        mode={mode}
        onMode={changeMode}
        hasSelection={targets().length > 0}
        hasImages={ids.length > 0}
        detecting={scenes.detecting}
        onImport={importFolder}
        importOptions={importOpts}
        onImportOptions={setImportOpts}
        onShootType={(t: ShootType) =>
          void run(async () => {
            await unwrap(commands.setShootType(t));
            await status.refreshCatalog();
          })
        }
        onAnalyze={analyze}
        onAutoAnalyze={(v) =>
          void run(async () => {
            await unwrap(commands.setAutoAnalyze(v));
            status.setCatalog((c) => (c ? { ...c, autoAnalyze: v } : c));
          })
        }
        onDetectScenes={() => void scenes.detect()}
        onAutoXmp={(v) =>
          void run(async () => {
            await unwrap(commands.setXmpAutoSync(v));
            status.setCatalog((c) => (c ? { ...c, xmpAutoSync: v } : c));
            status.refreshXmp();
          })
        }
        onApplySuggestions={askApplySuggestions}
        onWriteXmp={() => void writeXmp()}
        onSaveAllDirty={() => void saveAllDirty()}
        onReadXmp={() => void readXmp()}
        onExport={openExport}
        onCheatSheet={() => setCheatOpen(true)}
        exportPct={exportPct}
      />
      <ExportJobsPanel jobs={exportJobs.jobs} onCancel={(id) => void exportJobs.cancel(id)} onDismiss={exportJobs.dismiss} onReveal={revealInFinder} />
      {exportOpen && (
        <ExportDialog
          selectionIds={exportOpen}
          filteredIds={ids}
          sampleEntry={(id) => lib.getEntry(id)}
          onClose={() => setExportOpen(null)}
          onStarted={exportJobs.track}
        />
      )}
      {applyOpen && (
        <ApplySuggestionsDialog
          selected={applyOpen.selected}
          all={applyOpen.all}
          onCancel={() => setApplyOpen(null)}
          onConfirm={(t, onlyUnset) => {
            setApplyOpen(null);
            applySuggestions(t, onlyUnset);
          }}
        />
      )}
      {cheatOpen && <CheatSheet mode={mode} onClose={() => setCheatOpen(false)} />}
      {status.analysis && (status.analysis.running || status.analysis.failed > 0 || status.analysis.done < status.analysis.total) && (
        <AnalysisBar a={status.analysis} onCancel={() => void run(() => unwrap(commands.cancelAnalysis()))} />
      )}
      {status.progress && (importActive || status.progress.failed > 0) && <ImportBar progress={status.progress} active={importActive} />}

      {mode === "grid" ? (
        catalog != null && catalog.imageCount === 0 ? null : (
        <>
          {filtersOpen ? (
            <FilterBar query={query} setQuery={setQuery} counts={counts} />
          ) : (
            <FilterSummary query={query} shown={ids.length} total={counts?.total ?? null} sceneNumber={scenes.number} onEdit={() => setFiltersOpen(true)} />
          )}
          <GridToolbar
            query={query}
            setQuery={setQuery}
            size={size}
            onSize={setSize}
            selectedCount={sel.selected.size}
            total={ids.length}
            catalogTotal={counts?.total ?? null}
            capsLock={caps}
            autoAdvance={autoAdvance}
            onAutoAdvance={setAutoAdvance}
            filters={filtersOpen ? <FilterExtras query={query} setQuery={setQuery} counts={counts} catalog={catalog} /> : null}
          />
        </>
        )
      ) : (mode === "develop" && devPanels.chrome) || (mode === "loupe" && loupePanels.chrome) ? null : (
        <FilterSummary
          query={query}
          shown={ids.length}
          total={counts?.total ?? null}
          sceneNumber={scenes.number}
          onEdit={() => {
            changeMode("grid");
            setFiltersOpen(true);
          }}
        />
      )}

      <SceneStrip
        api={scenes}
        filterId={query.sceneId ?? null}
        onFilter={(id) => setQuery((q) => ({ ...q, sceneId: id }))}
        targets={targets()}
        activeId={active ?? null}
        onMatch={(s) => setMatchOpen(s.id)}
      />

      <div className="relative flex min-h-0 flex-1 flex-col" data-mode={mode}>
        <PhotoGrid
          lib={lib}
          targetSize={size}
          selected={sel.selected}
          active={sel.active}
          onColsChange={(cols, page) => (colsRef.current = { cols, page })}
          onCellClick={(id, e) => sel.click(id, { shift: e.shiftKey, meta: e.metaKey || e.ctrlKey })}
          onCellDoubleClick={openLoupe}
          catalogEmpty={catalog != null && catalog.imageCount === 0}
          filtered={filtered}
          onImport={importFolder}
          onClearFilters={clearFilters}
        />
        {mode === "develop" && (
          <DevelopView
            key={devEpoch}
            ref={develop}
            lib={lib}
            sel={sel}
            onError={reportError}
            onNotice={setNotice}
            onUndoToast={(msg, undo) => push(msg, { action: { label: "Undo", testid: "batch-undo", onClick: undo } })}
            onBack={() => changeMode("grid")}
          />
        )}
        {(mode === "loupe" || mode === "compare") && (
          <LoupeLayer
            ref={loupe}
            mode={mode}
            lib={lib}
            activeId={active}
            compare={cmp}
            onFocusPane={(k) => {
              if (!cmp) return;
              setCmp({ ...cmp, focus: k });
              sel.set([cmp[k]], cmp[k]);
            }}
            onOpen={(id) => sel.set([id], id)}
          />
        )}
      </div>
      {matchScene && (
        <MatchPanel
          scene={matchScene}
          sceneNumber={scenes.number(matchScene.id)}
          progress={scenes.progress}
          fileName={(id) => lib.getEntry(id)?.fileName ?? `#${id}`}
          onClose={() => setMatchOpen(null)}
          onApplied={(changed, attempted) => {
            setMatchOpen(null);
            void lib.refresh(attempted.filter((id) => lib.getEntry(id))).catch(reportError);
            setDevEpoch((n) => n + 1);
            if (changed.length === 0) return setNotice(`Applied Match Scene to 0 of ${plural(attempted.length, "photo")}`);
            push(`Applied Match Scene to ${changed.length} of ${plural(attempted.length, "photo")} (history: Match Scene)`, {
              action: {
                label: "Undo",
                testid: "match-undo",
                onClick: () =>
                  void run(async () => {
                    for (const id of changed) await unwrap(commands.undoAdjustments(id));
                    await lib.refresh(changed.filter((id) => lib.getEntry(id)));
                    setDevEpoch((n) => n + 1);
                    setNotice(`Undid Match Scene on ${plural(changed.length, "photo")}`);
                  }),
              },
            });
          }}
        />
      )}
      <Toasts api={toasts} error={status.error} onDismissError={() => setError(null)} />
    </main>
  );
}
