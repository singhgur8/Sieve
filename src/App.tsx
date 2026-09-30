// Phase 4 culling UI: virtualized grid, filter bar, loupe / compare, Lightroom-style keyboard.
import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Undo2, X } from "lucide-react";
import { commands, unwrap, type ColorLabel, type Scene, type PickFlag, type RawImageEntry, type ShootType } from "./ipc";
import { BASE_QUERY, useLibrary, type Query } from "./hooks/useLibrary";
import { useSelection } from "./hooks/useSelection";
import { useBackendStatus } from "./hooks/useBackendStatus";
import { useKeyboard } from "./hooks/useKeyboard";
import { TopBar } from "./components/TopBar";
import { FilterBar } from "./components/FilterBar";
import { GridToolbar, type Mode } from "./components/GridToolbar";
import { PhotoGrid } from "./components/PhotoGrid";
import { LoupeLayer, type CompareState, type LoupeHandle } from "./components/LoupeLayer";
import { DevelopView, type DevelopHandle } from "./components/develop/DevelopView";
import { AnalysisBar, ImportBar } from "./components/ProgressBars";
import { ExportDialog } from "./components/export/ExportDialog";
import { ExportJobsPanel } from "./components/export/ExportJobsPanel";
import { useExportJobs } from "./hooks/useExportJobs";
import { useScenes } from "./hooks/useScenes";
import { SceneStrip } from "./components/scenes/SceneStrip";
import { MatchPanel } from "./components/scenes/MatchPanel";

const LABEL_KEYS: Record<string, ColorLabel> = { "6": "red", "7": "yellow", "8": "green", "9": "blue" };

export default function App() {
  const [query, setQuery] = useState<Query>(BASE_QUERY);
  const [mode, setMode] = useState<Mode>("grid");
  const [cmp, setCmp] = useState<CompareState | null>(null);
  const [size, setSize] = useState(200);
  const [autoAdvance, setAutoAdvance] = useState(false);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [exportOpen, setExportOpen] = useState<number[] | null>(null);
  const [matchOpen, setMatchOpen] = useState<number | null>(null);
  const [matchUndo, setMatchUndo] = useState<number[] | null>(null);
  const [devEpoch, setDevEpoch] = useState(0);
  const colsRef = useRef(1);
  const loupe = useRef<LoupeHandle>(null);
  const develop = useRef<DevelopHandle>(null);
  const reloadRef = useRef<() => void>(() => {});

  const onLibraryChanged = useCallback(() => reloadRef.current(), []);
  const status = useBackendStatus(onLibraryChanged);
  const { reportError, setError } = status;
  const lib = useLibrary(query, reportError);
  reloadRef.current = () => void lib.reload();
  const { ids } = lib;
  const exportJobs = useExportJobs(reportError);
  const sel = useSelection(ids);
  const scenes = useScenes(query.folderId, query.sceneId ?? null, lib, reportError, setNotice);
  const matchScene: Scene | undefined = matchOpen != null ? scenes.scenes.find((s) => s.id === matchOpen) : undefined;

  const active = mode === "compare" && cmp ? cmp[cmp.focus] : sel.active;
  const membershipSensitive =
    query.picks.length > 0 || query.minRating != null || query.maxRating != null || query.colorLabels.length > 0 || query.sort === "rating";

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
  }, [ids, mode, sel, lib, query.folderId, reportError]);

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

  // ---- culling actions (batch over targets) ----
  const advanceIf = useCallback(
    (t: number[], force: boolean) => {
      if ((force || autoAdvance) && t.length === 1) {
        if (mode === "compare") stepCompare(1);
        else step(1);
      }
    },
    [autoAdvance, mode, step, stepCompare],
  );

  const mutate = useCallback(
    async (t: number[], optimistic: (e: RawImageEntry) => RawImageEntry, call: () => Promise<unknown>) => {
      lib.patch(t, optimistic);
      try {
        await call();
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
    [lib, reportError, membershipSensitive],
  );

  const doPick = useCallback(
    (pick: PickFlag, advance: boolean) => {
      const t = targets();
      if (t.length === 0) return;
      void mutate(t, (e) => ({ ...e, pick }), () => unwrap(commands.setPick(t, pick)));
      advanceIf(t, advance);
    },
    [targets, mutate, advanceIf],
  );

  const doRating = useCallback(
    (rating: number) => {
      const t = targets();
      if (t.length === 0) return;
      void mutate(t, (e) => ({ ...e, rating }), () => unwrap(commands.setRating(t, rating)));
      advanceIf(t, false);
    },
    [targets, mutate, advanceIf],
  );

  const doLabel = useCallback(
    (label: ColorLabel) => {
      const t = targets();
      if (t.length === 0) return;
      const allHave = t.every((id) => lib.getEntry(id)?.colorLabel === label);
      const next = allHave ? null : label;
      void mutate(t, (e) => ({ ...e, colorLabel: next }), () => unwrap(commands.setColorLabel(t, next)));
      advanceIf(t, false);
    },
    [targets, mutate, advanceIf, lib],
  );

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
  }, [targets, lib, status, reportError]);

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
  }, [targets, lib, status, reportError, membershipSensitive]);

  const openExport = useCallback(() => {
    if (ids.length === 0) return;
    setExportOpen(targets());
  }, [ids.length, targets]);

  // ---- keyboard ----
  useKeyboard((e) => {
    const k = e.key;
    const lower = k.toLowerCase();
    if (exportOpen || matchOpen != null) return;
    if ((e.metaKey || e.ctrlKey) && e.shiftKey && lower === "e") {
      e.preventDefault();
      openExport();
      return;
    }
    if ((e.metaKey || e.ctrlKey) && mode === "develop") {
      if (lower === "z") {
        e.preventDefault();
        if (e.shiftKey) develop.current?.redo();
        else develop.current?.undo();
        return;
      }
      if (e.shiftKey && lower === "c") {
        e.preventDefault();
        develop.current?.copy();
        return;
      }
      if (e.shiftKey && lower === "v") {
        e.preventDefault();
        develop.current?.paste();
        return;
      }
    }
    if (e.metaKey || e.ctrlKey) {
      if (lower === "a") {
        e.preventDefault();
        sel.selectAll();
      } else if (lower === "s") {
        e.preventDefault();
        void writeXmp();
      }
      return;
    }
    if (e.altKey) return;
    if (e.shiftKey && lower === "a") {
      e.preventDefault();
      void scenes.toggleAnchor(active ?? null);
      return;
    }
    const used = () => e.preventDefault();
    if (mode === "develop") {
      if (k === "\\") { used(); develop.current?.toggleBefore(); return; }
      if (lower === "d") { used(); return; }
      if (lower === "z") { used(); develop.current?.toggleZoom(); return; }
      if (k === " " || k === "Enter" || k === "Tab" || lower === "c" || lower === "f" || lower === "e") {
        if (lower === "e") { used(); changeMode("grid"); }
        return;
      }
    }
    if (lower === "p") { used(); doPick("pick", e.shiftKey); return; }
    if (lower === "x") { used(); doPick("reject", e.shiftKey); return; }
    if (lower === "u") { used(); doPick("unflagged", false); return; }
    if (/^[0-5]$/.test(k)) { used(); doRating(Number(k)); return; }
    if (LABEL_KEYS[k]) { used(); doLabel(LABEL_KEYS[k]); return; }
    switch (k) {
      case "ArrowLeft":
      case "ArrowRight": {
        used();
        const dir = k === "ArrowRight" ? 1 : -1;
        if (mode === "compare") stepCompare(dir);
        else step(dir, e.shiftKey && mode === "grid");
        return;
      }
      case "ArrowUp":
      case "ArrowDown":
        if (mode === "grid") {
          used();
          step(k === "ArrowDown" ? colsRef.current : -colsRef.current, e.shiftKey);
        }
        return;
      case " ":
        used();
        if (mode === "grid") openLoupe();
        else changeMode("grid");
        return;
      case "Enter":
        used();
        if (mode === "grid") openLoupe();
        return;
      case "Escape":
        if (mode !== "grid") changeMode("grid");
        else sel.clear();
        return;
      case "Tab":
        if (mode === "compare" && cmp) {
          used();
          const focus = cmp.focus === "a" ? "b" : "a";
          setCmp({ ...cmp, focus });
          sel.set([cmp[focus]], cmp[focus]);
        }
        return;
    }
    switch (lower) {
      case "e":
        used();
        if (mode === "grid") openLoupe();
        else changeMode("grid");
        return;
      case "g":
        used();
        changeMode("grid");
        return;
      case "d":
        used();
        openDevelop();
        return;
      case "c":
        used();
        if (mode === "compare") openLoupe(cmp ? cmp[cmp.focus] : undefined);
        else void enterCompare();
        return;
      case "z":
        used();
        if (mode === "grid") openLoupe();
        setTimeout(() => loupe.current?.toggleZoom(), mode === "grid" ? 120 : 0);
        return;
      case "f":
        used();
        if (mode !== "grid") loupe.current?.cycleFace(e.shiftKey ? -1 : 1);
        return;
    }
  });

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

  const importFolder = () =>
    void run(async () => {
      const path = await open({ directory: true, title: "Import RAW folder" });
      if (typeof path !== "string") return;
      setBusy(true);
      try {
        const s = await unwrap(commands.importFolder(path, { recursive: true }));
        setNotice(`Imported ${s.added} new (${s.skipped} already known, ${s.sidecarsRead} sidecars read)`);
        await status.refreshCatalog();
        await lib.reload();
      } finally {
        setBusy(false);
      }
    });

  const analyze = (kind: "pending" | "all") =>
    void run(async () => {
      status.setAnalysis((a) => ({ done: 0, total: a?.total ?? 0, failed: 0, running: true }));
      await unwrap(commands.analyzeImages({ kind }));
    });

  const applyAll = () =>
    void run(async () => {
      const t = sel.selected.size > 0 ? [...sel.selected] : ids;
      const n = await unwrap(commands.applySuggestions(t));
      setNotice(`Applied suggestions to ${n} of ${t.length} photos`);
      await lib.refresh(t.filter((id) => lib.getEntry(id)).slice(0, 2000));
      if (membershipSensitive) void lib.reload();
    });

  const importActive = status.progress !== null && status.progress.done < status.progress.total;
  const catalog = status.catalog;

  return (
    <main className="flex h-screen flex-col">
      <TopBar
        catalog={catalog}
        analysis={status.analysis}
        xmp={status.xmp}
        busy={busy}
        hasSelection={targets().length > 0}
        hasImages={ids.length > 0}
        onImport={importFolder}
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
        onAutoXmp={(v) =>
          void run(async () => {
            await unwrap(commands.setXmpAutoSync(v));
            status.setCatalog((c) => (c ? { ...c, xmpAutoSync: v } : c));
            status.refreshXmp();
          })
        }
        onApplySuggestions={applyAll}
        onWriteXmp={() => void writeXmp()}
        onReadXmp={() => void readXmp()}
        onExport={openExport}
        exportsRunning={exportJobs.jobs.filter((j) => j.running).length}
      />
      <ExportJobsPanel jobs={exportJobs.jobs} onCancel={(id) => void exportJobs.cancel(id)} onDismiss={exportJobs.dismiss} />
      {exportOpen && (
        <ExportDialog
          selectionIds={exportOpen}
          filteredIds={ids}
          sampleEntry={(id) => lib.getEntry(id)}
          onClose={() => setExportOpen(null)}
          onStarted={exportJobs.track}
        />
      )}
      {status.analysis && (status.analysis.running || status.analysis.failed > 0 || status.analysis.done < status.analysis.total) && (
        <AnalysisBar a={status.analysis} onCancel={() => void run(() => unwrap(commands.cancelAnalysis()))} />
      )}
      {status.progress && (importActive || status.progress.failed > 0) && <ImportBar progress={status.progress} active={importActive} />}
      {notice && (
        <p className="flex items-center justify-between bg-neutral-900 px-4 py-1 text-xs text-neutral-300" data-testid="notice">
          {notice}
          <button onClick={() => setNotice(null)} aria-label="Dismiss">
            <X className="size-3.5" />
          </button>
        </p>
      )}
      {status.error && (
        <p className="flex items-center justify-between bg-red-950 px-4 py-1.5 text-sm text-red-300" data-testid="error">
          {status.error}
          <button onClick={() => setError(null)} aria-label="Dismiss">
            <X className="size-3.5" />
          </button>
        </p>
      )}

      <FilterBar query={query} setQuery={setQuery} catalog={catalog} epoch={lib.epoch} shown={ids.length} />
      <GridToolbar
        mode={mode}
        query={query}
        setQuery={setQuery}
        size={size}
        onSize={setSize}
        selectedCount={sel.selected.size}
        total={ids.length}
        autoAdvance={autoAdvance}
        onAutoAdvance={setAutoAdvance}
        onMode={changeMode}
      />

      <SceneStrip
        api={scenes}
        filterId={query.sceneId ?? null}
        onFilter={(id) => setQuery((q) => ({ ...q, sceneId: id }))}
        targets={targets()}
        activeId={active ?? null}
        onMatch={(s) => setMatchOpen(s.id)}
      />
      {matchUndo && (
        <p className="flex items-center gap-3 bg-emerald-950 px-4 py-1 text-xs text-emerald-200" data-testid="match-undo-bar">
          Matched {matchUndo.length} photo{matchUndo.length === 1 ? "" : "s"} (history: Match Scene)
          <button
            className="flex items-center gap-1 rounded bg-emerald-900 px-2 py-0.5 hover:bg-emerald-800"
            data-testid="match-undo"
            onClick={() =>
              void run(async () => {
                const done = matchUndo;
                setMatchUndo(null);
                for (const id of done) await unwrap(commands.undoAdjustments(id));
                await lib.refresh(done.filter((id) => lib.getEntry(id)));
                setDevEpoch((n) => n + 1);
                setNotice(`Undid Match Scene on ${done.length} photo${done.length === 1 ? "" : "s"}`);
              })
            }
          >
            <Undo2 className="size-3" /> Undo
          </button>
          <button className="ml-auto" onClick={() => setMatchUndo(null)} aria-label="Dismiss">
            <X className="size-3.5" />
          </button>
        </p>
      )}

      <div className="relative flex min-h-0 flex-1 flex-col" data-mode={mode}>
        <PhotoGrid
          lib={lib}
          targetSize={size}
          selected={sel.selected}
          active={sel.active}
          onColsChange={(c) => (colsRef.current = c)}
          onCellClick={(id, e) => sel.click(id, { shift: e.shiftKey, meta: e.metaKey || e.ctrlKey })}
          onCellDoubleClick={openLoupe}
        />
        {mode === "develop" && (
          <DevelopView key={devEpoch} ref={develop} lib={lib} sel={sel} onError={reportError} onNotice={setNotice} onBack={() => changeMode("grid")} />
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
            setNotice(`Applied Match Scene to ${changed.length} of ${attempted.length} photo${attempted.length === 1 ? "" : "s"}`);
            setMatchUndo(changed.length ? changed : null);
            void lib.refresh(attempted.filter((id) => lib.getEntry(id))).catch(reportError);
            setDevEpoch((n) => n + 1);
          }}
        />
      )}
    </main>
  );
}
