// Library shell: virtualized grid, filter bars, loupe / compare / develop, and the single keymap-driven shortcut handler.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { commands, DEFAULT_SYNC_FIELDS, unwrap, type ActivityKind, type CaptureTimeEdit, type ColorLabel, type RejectStrictness, type KeeperRule, type Project, type Scene, type PickFlag, type RawImageEntry, type ShootType, type UiPrefs, type WorkflowStep } from "./ipc";
import { BASE_QUERY, useLibrary, type Library, type Query } from "./hooks/useLibrary";
import { useSelection } from "./hooks/useSelection";
import { useBackendStatus } from "./hooks/useBackendStatus";
import { useKeyboard } from "./hooks/useKeyboard";
import { useCullUndo } from "./hooks/useCullUndo";
import { useCullSummary } from "./hooks/useCullSummary";
import { useBurstSizes } from "./hooks/useBurstSizes";
import { CullSummaryBar } from "./components/CullSummaryBar";
import { keeperFormula } from "./lib/cull";
import { TopBar } from "./components/TopBar";
import { XmpExplainer } from "./components/XmpStatus";
import { MetadataRow } from "./components/MetadataFilterRow";
import { useMetaRowOpen } from "./lib/metaFilter";
import { FilterBar, FilterExtras, filterSummaryText, FilterSummary, isFiltered, useFilterCounts } from "./components/FilterBar";
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
import { SelectionBar } from "./components/scenes/SelectionBar";
import { StepBar } from "./components/StepBar";
import { AnalyzeSplit, ShootSelect } from "./components/AnalyzeControls";
import { PlanView } from "./components/edit/PlanView";
import { EditContextBar } from "./components/edit/EditContextBar";
import { StyleDialogs } from "./components/edit/StyleDialogs";
import { useWorkflow } from "./hooks/useWorkflow";
import { MatchPanel } from "./components/scenes/MatchPanel";
import { IssueBanner, Toasts, useToasts } from "./components/Toasts";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { ModelsDialog } from "./components/ModelsCard";
import { useModels } from "./lib/models";
import { CheatSheet } from "./components/CheatSheet";
import { ActivityWidget } from "./components/ActivityWidget";
import { HelpPanel } from "./components/HelpPanel";
import { openHelp, useHelpState } from "./lib/helpStore";
import { isActivityRunning } from "./lib/activity";
import { ChevronRight } from "lucide-react";
import { PhotoInfoPanel } from "./components/PhotoInfoPanel";
import { CaptureTimeDialog } from "./components/CaptureTimeDialog";
import { formatOffset } from "./lib/captureTime";
import { ApplySuggestionsDialog } from "./components/ApplySuggestionsDialog";
import { matchKey } from "./lib/keymap";
import { modalCount, useModalCount } from "./lib/modal";
import { getClipboard, setClipboard } from "./lib/clipboard";
import { flushEdits } from "./lib/editFlush";
import { clearFileHealth, describeReason, noteFailure } from "./lib/errors";
import { HealthBanner, RestoreBackupDialog } from "./components/CatalogHealth";
import { toggleChrome, toggleSidePanels, usePanels } from "./lib/panels";

const LABEL_KEYS: Record<string, ColorLabel> = { "6": "red", "7": "yellow", "8": "green", "9": "blue" };

const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;

/** Optimistic flag change: a flag you set is yours (`pickOrigin: user`), clearing it leaves no origin. */
const withPick = (e: RawImageEntry, pick: PickFlag): RawImageEntry => ({ ...e, pick, pickOrigin: pick === "unflagged" ? null : "user" });

interface AppProps {
  /** The open project; every query, count, scene list, import, analysis and export job is scoped to it (null = no scope, dev mock only). */
  project: Project | null;
  onHome: () => void;
  onOpenProject: (id: number) => Promise<void>;
}

export default function App({ project: projectProp, onHome, onOpenProject }: AppProps) {
  const modalOpenCount = useModalCount();
  const helpState = useHelpState();
  const [project, setProject] = useState(projectProp);
  const projectId = project?.id ?? null;
  const [queryState, setQuery] = useState<Query>(BASE_QUERY);
  // The project scope is applied here, so resetting filters (BASE_QUERY) can never leave the project.
  // The Edit and Export steps list the project's keepers only (server-side, so counts and ids agree).
  const keepersStep = projectId != null && (project?.workflowStep === "edit" || project?.workflowStep === "export");
  const query = useMemo<Query>(() => ({ ...queryState, projectId, keepersOnly: keepersStep || !!queryState.keepersOnly }), [queryState, projectId, keepersStep, queryState.keepersOnly]);
  // What the filter bars show: the Edit / Export steps' built-in keepers scope is not a filter the user set.
  const uiQuery = useMemo<Query>(() => ({ ...query, keepersOnly: !!queryState.keepersOnly }), [query, queryState.keepersOnly]);
  const [mode, setMode] = useState<Mode>("grid");
  const [cmp, setCmp] = useState<CompareState | null>(null);
  const [size, setSize] = useState(200);
  const [autoAdvance, setAutoAdvance] = useState(false);
  const [busy, setBusy] = useState(false);
  const [importOpts, setImportOpts] = useImportOptions();
  const importOptsRef = useRef(importOpts);
  importOptsRef.current = importOpts;
  const [filtersOpen, setFiltersOpen] = useState(true);
  const metaOpen = useMetaRowOpen();
  const [exportOpen, setExportOpen] = useState<{ sel: number[]; keepers?: number[] } | null>(null);
  const [exportedCount, setExportedCount] = useState<number | null>(null);
  const [applyOpen, setApplyOpen] = useState<{ selected: number[]; all: number[] } | null>(null);
  // Scenes are an Edit-step concept inside a project: the strip starts hidden there (Shift+S shows it).
  const [scenesOpen, setScenesOpen] = useState(projectProp == null);
  // Edit step overview (the plan) replaces the grid while open; a project that was left in the Edit step reopens on it.
  const [planOpen, setPlanOpen] = useState(projectProp?.workflowStep === "edit");
  const [planFocus, setPlanFocus] = useState<number | null>(null);
  // Review mode after an apply: N walks the frames that need a look.
  const [reviewScene, setReviewScene] = useState<number | null>(null);
  const [cheatOpen, setCheatOpen] = useState(false);
  const [restoreOpen, setRestoreOpen] = useState(false);
  const [healthDismissed, setHealthDismissed] = useState(false);
  const [modelsOpen, setModelsOpen] = useState(false);
  const [infoOpen, setInfoOpen] = useState(false);
  const [captureOpen, setCaptureOpen] = useState<number[] | null>(null);
  useModels(); // keeps the download listeners alive so mask capabilities refresh even when no panel is open
  const [matchOpen, setMatchOpen] = useState<number | null>(null);
  const [devEpoch, setDevEpoch] = useState(0);
  const [caps, setCaps] = useState(false);
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
  const rawLib = useLibrary(query, reportError);
  reloadRef.current = () => void rawLib.reload();
  const exportJobs = useExportJobs(reportError, projectId);
  const scenes = useScenes(query.folderId, projectId, query.sceneId ?? null, rawLib, reportError, setNotice);
  const step: WorkflowStep | null = project?.workflowStep ?? null;
  const changedRef = useRef<(ids: number[]) => Promise<void>>(async () => {});
  const wf = useWorkflow({
    projectId,
    toasts,
    onError: reportError,
    onChanged: (changed) => changedRef.current(changed),
    onScenesDetected: () => scenes.sync(),
    fileName: (id) => rawLib.getEntry(id)?.fileName ?? `#${id}`,
  });
  // "Show" on an XMP failure: the grid lists just those photos (failures are few, so the id list is cut client-side).
  const [idFilter, setIdFilter] = useState<{ ids: Set<number>; label: string } | null>(null);
  useEffect(() => setIdFilter(null), [projectId]);
  const scopedIds = useMemo(() => (idFilter ? rawLib.ids.filter((i) => idFilter.ids.has(i)) : rawLib.ids), [idFilter, rawLib.ids]);
  const lib: Library = idFilter ? { ...rawLib, ids: scopedIds } : rawLib;
  const { ids } = lib;
  const sel = useSelection(ids);
  const counts = useFilterCounts(query.folderId, projectId, lib.epoch, keepersStep, query.metadata, query.pickOrigin);
  // "of M" in the readouts is the unfiltered total (the metadata filter changes `counts`, not this).
  const totals = useFilterCounts(query.folderId, projectId, lib.epoch, keepersStep);
  // Cull summary (picked / unflagged / rejected / keepers): follows culling changes, analysis and the keeper rule.
  const analysisRunning = status.analysis?.running ?? false;
  const cullSum = useCullSummary(projectId, [rawLib.epoch, analysisRunning, status.catalog?.keeperRule]);
  const burstSizes = useBurstSizes(query.folderId, projectId, analysisRunning);
  const matchScene: Scene | undefined = matchOpen != null ? scenes.scenes.find((s) => s.id === matchOpen) : undefined;

  const active = mode === "compare" && cmp ? cmp[cmp.focus] : sel.active;
  const membershipSensitive =
    query.picks.length > 0 || query.pickOrigin != null || query.minRating != null || query.maxRating != null || query.colorLabels.length > 0 || query.sort === "rating" || query.metadata?.edited != null || query.metadata?.hasSidecar != null;

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
  /** Scene navigation target: selected once the (re-queried) result set contains it; meanwhile the neighbour fallback is skipped. */
  const pendingActive = useRef<number | null>(null);
  const [pendingTick, setPendingTick] = useState(0);
  useEffect(() => {
    const prev = prevIds.current;
    prevIds.current = ids;
    const pending = pendingActive.current;
    if (pending != null) {
      if (ids.includes(pending)) {
        pendingActive.current = null;
        selSet([pending], pending);
      }
      return;
    }
    if (selActive != null && lib.loaded && !ids.includes(selActive)) {
      const at = Math.min(Math.max(prev.indexOf(selActive), 0), ids.length - 1);
      if (ids[at] != null) selSet([ids[at]], ids[at]);
      else selClear();
    }
  }, [ids, lib.loaded, selActive, selSet, selClear, pendingTick]);

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

  const stepPhoto = useCallback(
    (delta: number, extend = false) => {
      if (ids.length === 0) return;
      const i = sel.active != null ? ids.indexOf(sel.active) : -1;
      const n = i < 0 ? 0 : Math.min(ids.length - 1, Math.max(0, i + delta));
      sel.moveTo(ids[n], extend);
    },
    [ids, sel],
  );

  /** Compare: move the Candidate through the filtered gallery (the Select never changes; it is skipped). */
  const stepCompare = useCallback(
    (dir: 1 | -1) => {
      if (!cmp || ids.length === 0) return;
      let i = ids.indexOf(cmp.b);
      if (i < 0) i = ids.indexOf(cmp.a);
      i += dir;
      while (i >= 0 && i < ids.length && ids[i] === cmp.a) i += dir;
      if (i < 0 || i >= ids.length) return;
      const id = ids[i];
      setCmp({ ...cmp, b: id });
      if (cmp.focus === "b") sel.set([id], id);
    },
    [cmp, ids, sel],
  );

  /** Pick the Candidate (filmstrip click). Clicking the Select swaps the two. */
  const setCandidate = useCallback(
    (id: number) => {
      if (!cmp || id === cmp.b) return;
      if (id === cmp.a) return swapCompareRef.current();
      setCmp({ ...cmp, b: id });
      if (cmp.focus === "b") sel.set([id], id);
    },
    [cmp, sel],
  );

  /** Swap the panes. The active photo stays active (so the focus flips with it). */
  const swapCompare = useCallback(() => {
    if (!cmp) return;
    const next: CompareState = { a: cmp.b, b: cmp.a, focus: cmp.focus === "a" ? "b" : "a" };
    setCmp(next);
    sel.set([next[next.focus]], next[next.focus]);
  }, [cmp, sel]);
  const swapCompareRef = useRef(swapCompare);
  swapCompareRef.current = swapCompare;

  /** Candidate becomes the Select; the next photo after it becomes the new Candidate. */
  const makeSelect = useCallback(() => {
    if (!cmp) return;
    let i = ids.indexOf(cmp.b) + 1;
    if (ids[i] === cmp.b) i++;
    let next = ids[i];
    if (next == null || next === cmp.b) next = ids[ids.indexOf(cmp.b) - 1];
    if (next == null || next === cmp.b) return;
    const n: CompareState = { a: cmp.b, b: next, focus: cmp.focus };
    setCmp(n);
    sel.set([n[n.focus]], n[n.focus]);
  }, [cmp, ids, sel]);

  const focusPane = useCallback(
    (k: "a" | "b") => {
      if (!cmp) return;
      setCmp({ ...cmp, focus: k });
      sel.set([cmp[k]], cmp[k]);
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
      if ((mode === "grid" || mode === "develop") && sel.selected.size === 2) {
        [a, b] = [...sel.selected];
      } else {
        a = sel.active ?? ids[0];
        if (a == null) return;
        const entry = lib.getEntry(a);
        if (entry?.burstGroupId != null) {
          const groups = await unwrap(commands.listBurstGroups(query.folderId, projectId));
          const g = groups.find((x) => x.id === entry.burstGroupId);
          if (g) {
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
      setCmp({ a, b, focus: "a" });
      sel.set([a], a);
      if (mode !== "develop") setMode("compare"); // inside Develop, Compare is a view of Develop (editing stays available)
    } catch (e) {
      reportError(e);
    }
  }, [ids, mode, sel, lib, query.folderId, projectId, reportError, setNotice]);

  const openDevelop = useCallback(() => {
    const target = sel.active ?? ids[0];
    if (target == null) return;
    // Keep a multi-selection (for sync / paste); otherwise select just the image.
    if (!cmp && !sel.selected.has(target)) sel.set([target], target);
    setMode("develop"); // a Compare pair stays open: Develop Compare edits the active pane
    if (!cmp) setCmp(null);
  }, [sel, ids, cmp]);

  const changeMode = useCallback(
    (m: Mode) => {
      setPlanOpen(false);
      if (m === "grid") {
        setMode("grid");
        setCmp(null);
      } else if (m === "develop") openDevelop();
      else if (m === "loupe") openLoupe();
      else if (mode === "compare" || (mode === "develop" && cmp)) return;
      else void enterCompare();
    },
    [openLoupe, openDevelop, enterCompare, mode, cmp],
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
        else stepPhoto(1);
      }
    },
    [autoAdvance, caps, mode, stepPhoto, stepCompare],
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
      void mutate(label, t, (e) => withPick(e, pick), () => unwrap(commands.setPick(t, pick)));
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

  /** Star click in the grid, loupe, Develop or a filmstrip: rates that photo only; clicking its current rating clears it. */
  const ratePhoto = useCallback(
    (id: number, rating: number) => {
      void mutate(`Rate ${what([id])} ${rating}★`, [id], (e) => ({ ...e, rating }), () => unwrap(commands.setRating([id], rating)));
    },
    [mutate, what],
  );

  /** Flag / X click on the Develop toolbar: flags that photo only; clicking its current flag clears it. */
  const flagPhoto = useCallback(
    (id: number, flag: "pick" | "reject") => {
      const pick: PickFlag = lib.getEntry(id)?.pick === flag ? "unflagged" : flag;
      const name = { pick: "Pick", reject: "Reject", unflagged: "Unflag" }[pick];
      void mutate(`${name} ${what([id])}`, [id], (e) => withPick(e, pick), () => unwrap(commands.setPick([id], pick)));
    },
    [lib, mutate, what],
  );

  /** Color label menu on the Develop toolbar (null clears). */
  const labelPhoto = useCallback(
    (id: number, label: ColorLabel | null) => {
      void mutate(`Label ${what([id])} ${label ?? "none"}`, [id], (e) => ({ ...e, colorLabel: label }), () => unwrap(commands.setColorLabel([id], label)));
    },
    [mutate, what],
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
          void mutate(`Pick ${entry.fileName}`, [id], (e) => withPick(e, "pick"), () => unwrap(commands.setPick([id], "pick")));
        }
        await lib.refresh(g.imageIds.filter((x) => lib.getEntry(x)));
        setNotice(`${entry.fileName} is now the best of its burst`);
      } catch (e) {
        reportError(e);
      }
    },
    [lib, mutate, reportError, setNotice],
  );

  // ---- XMP ----
  /** Success summary, or a persistent error toast (read-only folder, moved originals...) when some sidecars failed. */
  const xmpToast = useCallback(
    (prefix: string, r: { succeeded: number; failed: { imageId: number; reason: string }[] }) => {
      status.noteXmpFailures(r.failed);
      if (r.failed.length === 0) return setNotice(`${prefix} ${plural(r.succeeded, "photo")}`);
      r.failed.forEach((f) => noteFailure(f.reason));
      const why = describeReason(r.failed[0].reason);
      const lead = r.succeeded > 0 ? `${prefix} ${plural(r.succeeded, "photo")}; ` : "";
      push(`${lead}${plural(r.failed.length, "sidecar")} could not be written. ${why.message}`, { kind: "error" });
    },
    [push, setNotice, status.noteXmpFailures],
  );

  const writeXmp = useCallback(async () => {
    const t = targets();
    if (t.length === 0 || isActivityRunning("xmp_save")) return;
    try {
      const r = await unwrap(commands.writeXmp(t));
      status.noteXmpFailures([], t.filter((i) => !r.failed.some((f) => f.imageId === i)));
      xmpToast("Saved metadata for", r);
      await lib.refresh(t);
      status.refreshXmp();
    } catch (e) {
      reportError(e);
    }
  }, [targets, lib, status, reportError, setNotice]);

  const saveAllDirty = useCallback(async () => {
    try {
      const r = await unwrap(commands.writeXmpAllDirty(null));
      xmpToast("Saved XMP for", r);
      status.refreshXmp();
      await lib.refreshAll();
    } catch (e) {
      reportError(e);
    }
  }, [lib, status, reportError, setNotice]);

  // One-time explanation of how Sieve reads and merges XMP sidecars (shown when a project is opened with auto-sync on).
  const [uiPrefs, setUiPrefs] = useState<UiPrefs | null>(null);
  const [explainOpen, setExplainOpen] = useState(false);
  useEffect(() => {
    unwrap(commands.getUiPrefs())
      .then(setUiPrefs)
      .catch(() => setUiPrefs({}));
  }, []);
  const explainerDue = project != null && uiPrefs != null && !uiPrefs.xmpExplainerSeen && status.catalog?.xmpAutoSync === true;
  const closeExplainer = useCallback(() => {
    setExplainOpen(false);
    if (uiPrefs?.xmpExplainerSeen) return;
    const next = { ...uiPrefs, xmpExplainerSeen: true };
    setUiPrefs(next);
    unwrap(commands.setUiPrefs(next)).catch(reportError);
  }, [uiPrefs, reportError]);

  // Failure list for the status popover: session failures plus the catalog's list (`list_xmp_failures`) when it is opened.
  const [xmpNames, setXmpNames] = useState<Map<number, string>>(new Map());
  const xmpFailureRows = useMemo(
    () => [...status.xmpFailures].map(([imageId, reason]) => ({ imageId, reason, fileName: rawLib.getEntry(imageId)?.fileName ?? xmpNames.get(imageId) ?? `Photo #${imageId}` })),
    [status.xmpFailures, xmpNames, rawLib],
  );
  const { noteXmpFailures } = status;
  const openXmpErrors = useCallback(async () => {
    try {
      const found = await unwrap(commands.listXmpFailures(projectId));
      noteXmpFailures(found, [], true);
      const entries = await unwrap(commands.getImages(found.slice(0, 200).map((f) => f.imageId)));
      setXmpNames(new Map(entries.map((e) => [e.id, e.fileName])));
    } catch {
      /* the session failures stay listed */
    }
  }, [noteXmpFailures, projectId]);
  /** Show: leave the Plan / loupe and list the failed photos (all of them, not just this row) in the grid. */
  const showXmpFailure = useCallback(
    (imageId: number) => {
      const failed = new Set([...status.xmpFailures.keys(), imageId]);
      setQuery((q) => ({ ...BASE_QUERY, sort: q.sort, sortDescending: q.sortDescending }));
      setIdFilter({ ids: failed, label: `${plural(failed.size, "photo")} with sidecar errors` });
      setPlanOpen(false);
      setCmp(null);
      setMode("grid");
      sel.set([imageId], imageId);
    },
    [status.xmpFailures, sel],
  );

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

  /** Export dialog. In a project's Edit / Export step (or from step 3) it is scoped to the project's keepers. */
  const openExport = useCallback(
    async (keepers = false) => {
      if (projectId != null && (keepers || step === "edit" || step === "export")) {
        try {
          const plan = await unwrap(commands.getEditPlan(projectId));
          if (plan.keeperIds.length === 0) return setNotice("No keepers yet. Pick photos in Cull (Z) first.");
          setExportOpen({ sel: targets(), keepers: plan.keeperIds });
        } catch (e) {
          reportError(e);
        }
        return;
      }
      if (ids.length === 0) return;
      setExportOpen({ sel: targets() });
    },
    [projectId, step, ids.length, targets, reportError, setNotice],
  );

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

  /** The catalog as this project sees it: its folders and shoot type (shoot type is per project since v14). */
  const catalog = status.catalog;
  const scopedCatalog = useMemo(
    () => (catalog && project ? { ...catalog, folders: catalog.folders.filter((f) => f.projectId === project.id), shootType: project.shootType } : catalog),
    [catalog, project],
  );

  // The Edit step re-reads the plan whenever something that feeds it changed (culling, edits, returning to it).
  const activeEntry = active != null ? lib.getEntry(active) : undefined;
  const activeStamp = active != null ? `${lib.version(active)}:${activeEntry?.hasEdits ? 1 : 0}` : "";
  const { refreshPlan, loadPlan } = wf;
  useEffect(() => {
    if (step !== "edit") return;
    const t = setTimeout(refreshPlan, 200);
    return () => clearTimeout(t);
  }, [step, mode, planOpen, rawLib.epoch, activeStamp, refreshPlan]);
  useEffect(() => {
    if (projectId == null) return;
    refreshPlan(); // the Edit pill shows scene counts in every step
  }, [projectId, refreshPlan]);
  useEffect(() => {
    if (step === "edit" && planOpen && !wf.plan && !wf.loading) void loadPlan(true);
  }, [step, planOpen, wf.plan, wf.loading, loadPlan]);
  useEffect(() => {
    if (mode !== "grid") setPlanOpen(false);
  }, [mode]);
  useEffect(() => {
    if (step === "edit" && planOpen && planFocus == null && wf.rows[0]) setPlanFocus(wf.rows[0].entry.sceneId);
  }, [step, planOpen, planFocus, wf.rows]);

  // "Exported N" on the Export pill: the last finished job of this project.
  const jobsDone = exportJobs.jobs.filter((j) => j.finished).length;
  useEffect(() => {
    if (projectId == null) return;
    unwrap(commands.getExportJobs())
      .then((list) => {
        const done = list.filter((j) => j.projectId === projectId && j.state === "completed").sort((a, b) => (b.finishedAtMs ?? 0) - (a.finishedAtMs ?? 0));
        setExportedCount(done[0] ? done[0].succeeded : null);
      })
      .catch(() => {});
  }, [projectId, jobsDone]);

  // Header counts / step follow the library (imports, culling, edits all bump `lib.epoch`).
  const refreshProject = useCallback(async () => {
    if (projectId == null) return;
    try {
      setProject(await unwrap(commands.getProject(projectId)));
    } catch (e) {
      reportError(e);
    }
  }, [projectId, reportError]);
  useEffect(() => {
    void refreshProject();
  }, [lib.epoch, refreshProject]);

  changedRef.current = async (changed) => {
    if (changed.length > 0) await rawLib.refresh(changed.filter((id) => rawLib.getEntry(id)).slice(0, 2000)).catch(reportError);
    else await rawLib.refreshAll().catch(reportError);
    setDevEpoch((n) => n + 1);
    status.refreshXmp();
    void refreshProject();
  };

  // ---- workflow steps ----
  const setStep = useCallback(
    async (s: WorkflowStep) => {
      if (projectId == null) return;
      setProject((p) => (p ? { ...p, workflowStep: s } : p));
      try {
        await unwrap(commands.setWorkflowStep(projectId, s));
      } catch (e) {
        reportError(e);
        void refreshProject();
      }
    },
    [projectId, reportError, refreshProject],
  );

  const openPlan = useCallback(() => {
    setPlanOpen(true);
    setMode("grid");
    setCmp(null);
    setReviewScene(null);
    setQuery((q) => (q.sceneId == null ? q : { ...q, sceneId: null }));
  }, []);

  /** Project scope of the library: rows of the plan by scene and by photo. */
  const rowOfImage = useMemo(() => {
    const m = new Map<number, (typeof wf.rows)[number]>();
    wf.rows.forEach((r) => r.entry.imageIds.forEach((i) => m.set(i, r)));
    return m;
  }, [wf.rows]);
  const rowOfScene = useCallback((sceneId: number) => wf.rows.find((r) => r.entry.sceneId === sceneId), [wf.rows]);

  /** Develop on a photo of a scene with the filmstrip narrowed to that scene. */
  const openScene = useCallback(
    (sceneId: number, photo?: number) => {
      const row = rowOfScene(sceneId);
      const id = photo ?? row?.entry.representativeId;
      if (id == null) return;
      setQuery((q) => (q.sceneId === sceneId ? q : { ...q, sceneId }));
      pendingActive.current = id;
      setPendingTick((t) => t + 1);
      // Safety: a target that never shows up (hidden by another filter) must not freeze the neighbour fallback.
      window.setTimeout(() => {
        if (pendingActive.current === id) {
          pendingActive.current = null;
          setPendingTick((t) => t + 1);
        }
      }, 2000);
      setCmp(null);
      setPlanOpen(false);
      setMode("develop");
    },
    [rowOfScene],
  );

  const goStep = useCallback(
    (s: WorkflowStep) => {
      if (projectId == null) return;
      if (s === "cull") {
        void setStep("cull");
        setQuery((q) => (q.sceneId == null ? q : { ...q, sceneId: null }));
        setPlanOpen(false);
        setReviewScene(null);
        setMode("grid");
        setCmp(null);
      } else if (s === "edit") {
        if (step !== "edit") void setStep("edit");
        void wf.loadPlan(true);
        openPlan();
      } else {
        void openExport(true);
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [projectId, step, setStep, openPlan, wf.loadPlan],
  );

  /** G / Esc: back to the overview of the step (the Plan in the Edit step, the Grid otherwise). */
  const goOverview = useCallback(() => {
    if (projectId != null && step === "edit") openPlan();
    else changeMode("grid");
  }, [projectId, step, openPlan, changeMode]);

  /** N / Shift+N: next / previous scene (N skips scenes that are already applied). */
  const stepScene = useCallback(
    (dir: 1 | -1, skipDone: boolean) => {
      const rows = wf.rows;
      if (rows.length === 0) return;
      const cur = planOpen ? rows.findIndex((r) => r.entry.sceneId === planFocus) : active != null ? rows.findIndex((r) => r.entry.imageIds.includes(active)) : -1;
      for (let k = 1; k <= rows.length; k++) {
        const r = rows[(((cur < 0 ? (dir === 1 ? -1 : 0) : cur) + dir * k) % rows.length + rows.length) % rows.length];
        if (skipDone && (r.ui === "applied" || r.skipped)) continue;
        setReviewScene(null);
        if (planOpen) setPlanFocus(r.entry.sceneId);
        else openScene(r.entry.sceneId);
        return;
      }
      setNotice("Every scene is applied or skipped");
    },
    [wf.rows, planOpen, planFocus, active, openScene, setNotice],
  );

  /** Review an apply: Develop on the first frame that needs a look; N walks the rest of `plan.needsReviewIds`. */
  const reviewFrames = useCallback(
    (sceneId: number, reviewIds?: number[]) => {
      const row = rowOfScene(sceneId);
      // Develop never opens on a frame outside the plan's keepers (an apply with options can report others).
      const keepers = new Set(wf.plan?.keeperIds ?? []);
      const wanted = reviewIds && reviewIds.length > 0 ? reviewIds : row?.review;
      const kept = wanted?.filter((i) => keepers.has(i));
      if (wanted && wanted.length > 0 && (kept?.length ?? 0) === 0) return setNotice("The frames that need a look are not keepers");
      const first = kept?.[0];
      if (first == null) return setNotice("Nothing to review in this scene");
      setReviewScene(sceneId);
      openScene(sceneId, first);
    },
    [rowOfScene, openScene, setNotice, wf.plan],
  );
  const nextReview = useCallback(() => {
    const plan = wf.plan;
    const list = plan?.needsReviewIds ?? [];
    if (!plan || list.length === 0) return setNotice("No more frames to review");
    // The frame being fixed leaves the list, so "next" is the first one after it in capture order.
    const order = new Map(plan.keeperIds.map((id, i) => [id, i]));
    const at = active != null ? (order.get(active) ?? -1) : -1;
    const next = list.find((id) => (order.get(id) ?? -1) > at);
    if (next == null) return setNotice("That was the last frame to review");
    const row = rowOfImage.get(next);
    if (!row) return;
    setReviewScene(row.entry.sceneId);
    if (query.sceneId != null && query.sceneId !== row.entry.sceneId) openScene(row.entry.sceneId, next);
    else sel.set([next], next);
  }, [wf.plan, active, rowOfImage, query.sceneId, openScene, sel, setNotice]);

  /** Shift+A in the Edit step: the photo becomes the representative of its scene. */
  const makeRepresentative = useCallback(
    (id: number | null) => {
      const row = id != null ? rowOfImage.get(id) : undefined;
      if (id == null || !row) return setNotice("Open a photo that belongs to a scene first");
      void wf.setRepresentative(row.entry.sceneId, id);
    },
    [rowOfImage, wf, setNotice],
  );

  const setCover = useCallback(
    async (imageId: number) => {
      if (projectId == null) return;
      try {
        setProject(await unwrap(commands.setProjectCover(projectId, imageId)));
        setNotice("Project cover updated");
      } catch (e) {
        reportError(e);
      }
    },
    [projectId, reportError, setNotice],
  );

  const importFolder = useCallback(
    () =>
      void run(async () => {
        const path = await open({ directory: true, title: "Import RAW folder" });
        if (typeof path !== "string") return;
        setBusy(true);
        try {
          const s = await unwrap(commands.importFolder(path, importOptsRef.current, projectId));
          setNotice(`Imported ${s.added} new (${s.skipped} already known, ${s.sidecarsRead} sidecars read${s.companions > 0 ? `, ${s.companions} JPEG pairs` : ""})`);
          await status.refreshCatalog();
          await lib.reload();
        } finally {
          setBusy(false);
        }
      }),
    [run, status, lib, setNotice, projectId],
  );

  /** "Locate folder…": pick the moved folder, relink the catalog folder to it, refresh everything. */
  const locateFolder = useCallback(
    (imageId?: number) =>
      void run(async () => {
        let folderId: number | null | undefined = imageId != null ? lib.getEntry(imageId)?.folderId : query.folderId;
        if (folderId == null) {
          const [first] = await unwrap(commands.listImageIds({ ...query, missingOnly: true, folderId: null, sceneId: null, offset: 0, limit: 1 }));
          if (first != null) folderId = (await unwrap(commands.getImage(first))).folderId;
        }
        folderId ??= scopedCatalog?.folders[0]?.id;
        if (folderId == null) return;
        const path = await open({ directory: true, title: "Locate folder" });
        if (typeof path !== "string") return;
        const r = await unwrap(commands.relocateFolder(folderId, path));
        push(`Relinked ${plural(r.matched, "photo")}${r.stillMissing > 0 ? ` · ${r.stillMissing} still missing` : ""}`);
        clearFileHealth();
        await status.refreshCatalog();
        await lib.reset();
      }),
    [run, lib, query, status, push, scopedCatalog],
  );

  const restoreBackup = useCallback(
    async (index: number) => {
      try {
        const h = await unwrap(commands.restoreCatalogBackup(index));
        status.setCatalog((c) => (c ? { ...c, health: h } : c));
        setRestoreOpen(false);
      } catch (e) {
        reportError(e);
      }
    },
    [status, reportError],
  );

  const analyze = (kind: "pending" | "all") =>
    void run(async () => {
      status.setAnalysis((a) => ({ done: 0, total: a?.total ?? 0, failed: 0, running: true }));
      await unwrap(commands.analyzeImages(kind === "all" && projectId != null ? { kind: "project", projectId } : { kind }));
    });

  const onShootTypeChange = (t: ShootType) =>
    void run(async () => {
      if (projectId != null) {
        await unwrap(commands.setProjectShootType(projectId, t));
        await refreshProject();
      } else {
        await unwrap(commands.setShootType(t));
        await status.refreshCatalog();
      }
    });
  const onAutoAnalyzeChange = (v: boolean) =>
    void run(async () => {
      await unwrap(commands.setAutoAnalyze(v));
      status.setCatalog((c) => (c ? { ...c, autoAnalyze: v } : c));
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
      push(`Applied suggestions to ${applied} of ${t.length} photos${skipped ? ` (${skipped} skipped)` : ""}. Review the result in the Rejected view`, {
        action: { label: "Undo", testid: "apply-undo", onClick: () => void cull.undoEntry(entry) },
      });
    });

  /** Keeper rule changed from the Cull summary / Export dialog (the Edit plan has its own path through `wf`). */
  /** Edit Capture Time: apply, refresh order / entries / panel, and offer Undo (restore_capture_times). */
  const applyCaptureTime = useCallback(
    async (target: number[], mode: CaptureTimeEdit, label: string) => {
      try {
        const res = await unwrap(commands.editCaptureTime(target, mode));
        setCaptureOpen(null);
        await rawLib.reload();
        await rawLib.refreshAll();
        status.refreshXmp();
        if (res.changedIds.length === 0) return setNotice("No capture times changed");
        const undo = async () => {
          try {
            await unwrap(commands.restoreCaptureTimes(res.previous));
            await rawLib.reload();
            await rawLib.refreshAll();
            status.refreshXmp();
            setNotice(`Undid: ${label}`);
          } catch (e) {
            reportError(e);
          }
        };
        const off = res.offsetMs != null ? ` (${formatOffset(res.offsetMs)})` : "";
        push(`${mode.kind === "revert" ? "Reverted" : "Changed"} capture time of ${plural(res.changedIds.length, "photo")}${mode.kind === "shift" || mode.kind === "revert" ? "" : off}${mode.kind === "shift" ? off : ""}`, {
          action: { label: "Undo", testid: "capture-undo", onClick: () => void undo() },
        });
      } catch (e) {
        reportError(e);
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [rawLib, status.refreshXmp, setNotice, push, reportError],
  );

  const changeRejectStrictness = useCallback(
    async (strictness: RejectStrictness) => {
      if (projectId == null) return;
      try {
        await unwrap(commands.setProjectRejectStrictness(projectId, strictness));
        await refreshProject();
        cullSum.refresh();
        await rawLib.refreshAll();
        setNotice(`Reject strictness: ${strictness}. Suggestions are being updated`);
      } catch (e) {
        reportError(e);
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [projectId, refreshProject, cullSum.refresh, rawLib, setNotice, reportError],
  );

  const changeKeeperRule = useCallback(
    async (rule: KeeperRule) => {
      try {
        await unwrap(commands.setKeeperRule(rule));
        status.setCatalog((c) => (c ? { ...c, keeperRule: rule } : c));
        cullSum.refresh();
        wf.refreshPlan();
        await refreshProject();
        void rawLib.reload();
      } catch (e) {
        reportError(e);
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [status.setCatalog, cullSum.refresh, wf.refreshPlan, refreshProject, rawLib, reportError],
  );

  // Lightroom (or anything else) may have edited sidecars: re-read the changed ones when the project opens and whenever
  // the window regains focus (at most every 2 s), then refetch the photos that changed.
  const lastSidecarRefresh = useRef(0);
  const sidecarProject = useRef<number | null>(null);
  useEffect(() => {
    if (projectId == null) return;
    if (sidecarProject.current !== projectId) {
      sidecarProject.current = projectId;
      lastSidecarRefresh.current = 0;
    }
    const run = async () => {
      const now = Date.now();
      if (now - lastSidecarRefresh.current < 2000) return;
      lastSidecarRefresh.current = now;
      try {
        const changed = await unwrap(commands.refreshSidecars(projectId));
        if (changed.length === 0) return;
        await rawLib.refresh(changed.filter((id) => rawLib.getEntry(id)).slice(0, 2000));
        void rawLib.reload();
        cullSum.refresh();
        status.refreshXmp();
        setNotice(`${plural(changed.length, "photo")} changed in sidecars by another app (Lightroom?) and were reloaded`);
      } catch (e) {
        reportError(e);
      }
    };
    void run();
    const onFocus = () => void run();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectId]);

  const revealInFinder = (path: string) => void run(() => unwrap(commands.revealInFinder(path)));

  /** Cmd+V / Cmd+Shift+V in the Library: the copied settings go to every selected photo as one undoable batch. */
  const pasteToSelection = useCallback(async () => {
    const c = getClipboard();
    if (!c) return setNotice("Nothing copied yet. Cmd+C copies all settings of the active photo");
    const t = targets();
    if (t.length === 0) return;
    try {
      await flushEdits();
      const r = await unwrap(commands.pasteSettings(t, c.adjustments, c.fields));
      await wf.reportBatch(r, `Pasted ${plural(c.fields.length, "setting group")}${c.fromName ? ` from ${c.fromName}` : ""}`, t.length);
    } catch (e) {
      reportError(e);
    }
  }, [targets, wf, reportError, setNotice]);

  /** Cmd+C in the Library: every setting of the active photo (not crop / masks) onto the clipboard. */
  const copyActive = useCallback(async () => {
    const id = active;
    if (id == null) return setNotice("Select a photo first");
    try {
      await flushEdits();
      const adjustments = await unwrap(commands.getAdjustments(id));
      const fromName = lib.getEntry(id)?.fileName;
      setClipboard({ adjustments, fields: [...DEFAULT_SYNC_FIELDS], fromName });
      const n = Math.max(0, targets().filter((t) => t !== id).length);
      setNotice(`Copied all settings of ${fromName ?? "the photo"} (not crop / masks). ${n > 0 ? `Cmd+V pastes to the ${plural(n, "other selected photo")}` : "Select photos and press Cmd+V to paste"}`);
    } catch (e) {
      reportError(e);
    }
  }, [active, lib, targets, reportError, setNotice]);

  /** Sync: the active photo's settings onto the rest of the selection (one undoable batch). */
  const syncSelection = useCallback(async () => {
    const id = active;
    const t = targets().filter((x) => x !== id);
    if (id == null || t.length === 0) return setNotice("Select the photos to sync to (Shift / Cmd-click), the active photo is the source");
    try {
      await flushEdits();
      const r = await unwrap(commands.syncSettings(id, t, [...DEFAULT_SYNC_FIELDS]));
      await wf.reportBatch(r, `Synchronized settings of ${lib.getEntry(id)?.fileName ?? "the photo"}`, t.length);
    } catch (e) {
      reportError(e);
    }
  }, [active, targets, wf, lib, reportError, setNotice]);

  /** "Show" on an apply toast: leave the Plan and list exactly these photos, selected. */
  const showIds = useCallback(
    (list: number[], label: string) => {
      if (list.length === 0) return;
      setPlanOpen(false);
      setCmp(null);
      setMode("grid");
      setIdFilter({ ids: new Set(list), label });
      sel.set(list, list[0]);
    },
    [sel],
  );

  /** "Edit all in scene": the whole scene selected, Develop on `startId`. Reset / presets apply to all, Cmd+Alt+S syncs the rest. */
  const editAllInScene = useCallback(
    (sceneIds: number[], startId: number) => {
      if (sceneIds.length === 0) return;
      sel.set(sceneIds, startId);
      setCmp(null);
      setPlanOpen(false);
      setMode("develop");
      setNotice(`Editing 1 of ${sceneIds.length}: Cmd+Alt+S syncs this photo's settings to the other ${sceneIds.length - 1}; reset and presets apply to all ${sceneIds.length}`);
    },
    [sel, setNotice],
  );

  const selectBurst = useCallback(async () => {
    const id = active;
    const entry = id != null ? lib.getEntry(id) : undefined;
    if (id == null || entry?.burstGroupId == null) return setNotice("The active photo is not part of a burst");
    try {
      const groups = await unwrap(commands.listBurstGroups(query.folderId, projectId));
      const g = groups.find((x) => x.id === entry.burstGroupId);
      const members = g ? g.imageIds.filter((x) => ids.includes(x)) : [];
      if (members.length === 0) return setNotice("Burst members are hidden by the current filters");
      sel.set(members, id);
      setNotice(`Selected ${plural(members.length, "photo")} of the burst`);
    } catch (e) {
      reportError(e);
    }
  }, [active, lib, query.folderId, projectId, ids, sel, reportError, setNotice]);

  const toggleScenes = useCallback(() => {
    if (scenesOpen) {
      setScenesOpen(false);
      setQuery((q) => (q.sceneId == null ? q : { ...q, sceneId: null }));
    } else setScenesOpen(true);
  }, [scenesOpen]);

  // ---- keyboard: one handler driven by the shared keymap ----
  useKeyboard((e) => {
    if (modalCount() > 0) return; // dialogs and menus own the keyboard
    const inPlan = planOpen && projectId != null && step === "edit";
    if (inPlan && !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey) {
      // Plan: Up / Down move the row focus, Enter / D open the representative.
      const shown = wf.layout.visible;
      const at = shown.findIndex((r) => r.entry.sceneId === planFocus);
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const n = shown[Math.min(shown.length - 1, Math.max(0, at + (e.key === "ArrowDown" ? 1 : -1)))];
        if (n) setPlanFocus(n.entry.sceneId);
        return;
      }
      if (e.key === "Escape" && wf.busy && wf.busy.kind !== "auto") {
        e.preventDefault();
        return wf.cancelApply();
      }
      if (e.key.toLowerCase() === "s" && planFocus != null) {
        e.preventDefault();
        const r = rowOfScene(planFocus);
        return r ? void wf.setSkipped(planFocus, !r.skipped) : undefined;
      }
      if ((e.key === "Enter" || e.key.toLowerCase() === "d") && planFocus != null) {
        e.preventDefault();
        return openScene(planFocus);
      }
    }
    const def = matchKey(e, mode, { cropping: mode === "develop" && !!develop.current?.isCropping(), comparing: cmp != null });
    if (!def) return;
    const PLAN_INERT = ["pick", "reject", "unflag", "rate", "label", "keeper", "keeperSet", "anchor", "selectBurst", "navH", "navV", "gridJump", "toggleLoupe", "gridLoupe", "zoomLoupe", "selectAll", "selectNone", "filterBar", "scenesToggle", "develop", "compare", "paste", "copyAll", "pasteAll"];
    if (planOpen && PLAN_INERT.includes(def.id)) return;
    e.preventDefault();
    const k = e.key;
    switch (def.id) {
      case "stepCull":
        return goStep("cull");
      case "stepEdit":
        return goStep("edit");
      case "stepExport":
        return goStep("export");
      case "nextScene":
        if (step !== "edit") return;
        if (mode === "develop" && (reviewScene != null || (active != null && wf.needsReviewSet.has(active)))) return nextReview();
        return stepScene(1, true);
      case "prevScene":
        return step === "edit" ? stepScene(-1, false) : undefined;
      case "applyScene": {
        if (step !== "edit") return;
        const row = planOpen ? rowOfScene(planFocus ?? -1) : active != null ? rowOfImage.get(active) : undefined;
        if (!row || row.ui === "todo" || row.ui === "reset" || row.targets === 0) return setNotice("Edit this scene's representative first");
        if (row.skipped) return setNotice("This scene is skipped. Include it first (S in the Plan)");
        return void wf.applyScene(row.entry.sceneId, "match", reviewFrames, showIds);
      }
      case "autoEdit": {
        if (step !== "edit") return;
        const row = planOpen ? rowOfScene(planFocus ?? -1) : active != null ? rowOfImage.get(active) : undefined;
        return row ? wf.requestAutoEdit([row.entry.sceneId]) : undefined;
      }
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
        if (wf.lastBatch && wf.lastBatch.at > cull.undoAt()) return wf.undoLast();
        {
          // A newer batch exists but later edits block it: say why instead of "Nothing to undo".
          const blocked = wf.blockedUndo();
          if (blocked && blocked.at > cull.undoAt()) return void toasts.push(blocked.reason);
        }
        return void cull.undo();
      case "redoCull":
        return void cull.redo();
      case "anchor":
        if (projectId != null && step === "edit") return makeRepresentative(active ?? null);
        return void scenes.toggleAnchor(active ?? null);
      case "selectBurst":
        return void selectBurst();
      case "navH": {
        const dir = k === "ArrowRight" ? 1 : -1;
        if (cmp) stepCompare(dir);
        else stepPhoto(dir, e.shiftKey && mode === "grid");
        return;
      }
      case "navV":
        return stepPhoto(k === "ArrowDown" ? colsRef.current.cols : -colsRef.current.cols, e.shiftKey);
      case "gridJump": {
        const page = colsRef.current.page;
        const delta = { Home: -Infinity, End: Infinity, PageUp: -page, PageDown: page }[k] ?? 0;
        return stepPhoto(Math.max(-1e9, Math.min(1e9, delta)), e.shiftKey);
      }
      case "toggleLoupe":
        if (mode === "grid") openLoupe();
        else changeMode("grid");
        return;
      case "devToLoupe":
        return openLoupe();
      case "toGrid":
        return goOverview();
      case "escape":
        if (mode !== "grid") goOverview();
        else sel.clear();
        return;
      case "developEscape":
        return develop.current?.escape();
      case "cropSwap":
        return develop.current?.cropSwap();
      case "cropLock":
        return develop.current?.cropLock();
      case "cropOverlay":
        return develop.current?.cropOverlay();
      case "cropOverlayRotate":
        return develop.current?.cropOverlayRotate();
      case "cropReset":
        return develop.current?.cropReset();
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
        else if (mode === "develop" && cmp) setCmp(null);
        else void enterCompare();
        return;
      case "compareSwap":
        return swapCompare();
      case "compareMakeSelect":
        return makeSelect();
      case "compareFocus":
        return cmp ? focusPane(cmp.focus === "a" ? "b" : "a") : undefined;
      case "scenesToggle":
        return toggleScenes();
      case "photoInfo":
        return setInfoOpen((v) => !v);
      case "captureTime": {
        const t = targets();
        if (t.length > 0) setCaptureOpen(t);
        return;
      }
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
      case "gridLoupe":
        return openLoupe();
      case "zoomLoupe":
        return loupe.current?.toggleZoom();
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
        if (wf.lastBatch && wf.lastBatch.at > Math.max(cull.undoAt(), d?.lastCommitAt() ?? 0)) return wf.undoLast();
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
      case "copyAll":
        return void copyActive();
      case "pasteAll":
        return void pasteToSelection();
      case "sync":
        return develop.current?.sync();
      case "syncQuiet":
        return develop.current?.sync(true);
      case "autoTone":
        return develop.current?.autoTone();
      case "autoWb":
        return develop.current?.autoWb();
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
      case "maskMoveUp":
      case "maskMoveDown":
        return develop.current?.maskKey(def.id, e);
      case "saveXmp":
        return void writeXmp();
      case "export":
        return void openExport();
      case "import":
        return importFolder();
      case "cheatSheet":
        return setCheatOpen(true);
      case "help":
        return openHelp();
    }
  });

  const importActive = status.progress !== null && status.progress.done < status.progress.total;
  const analysisBarShown = !!status.analysis && (status.analysis.running || status.analysis.failed > 0 || status.analysis.done < status.analysis.total);
  const importBarShown = !!status.progress && (importActive || status.progress.failed > 0);
  // One progress display per job: the corner stack skips what the export cards / top bars already show.
  const hiddenActivityKinds: ActivityKind[] = [...(exportJobs.jobs.length > 0 ? (["export"] as const) : []), ...(analysisBarShown ? (["analysis"] as const) : []), ...(importBarShown ? (["import"] as const) : [])];
  const running = exportJobs.jobs.filter((j) => j.running);
  const exportPct = running.length ? Math.round((running.reduce((a, j) => a + j.done, 0) / Math.max(1, running.reduce((a, j) => a + j.total, 0))) * 100) : null;
  const catalogEmpty = project ? project.photoCount === 0 : catalog != null && catalog.imageCount === 0;
  const filtered = isFiltered(uiQuery);
  const clearFilters = () => setQuery((q) => ({ ...BASE_QUERY, sort: q.sort, sortDescending: q.sortDescending }));

  // While Loupe / Compare / Develop cover the grid, it receives the exact props it had when it was last visible
  // (memoised, so it does not re-render): arrow-key navigation would otherwise re-render every visible cell per key.
  const liveGridProps = {
    lib,
    targetSize: size,
    selected: sel.selected,
    active: sel.active,
    onColsChange: (cols: number, page: number) => (colsRef.current = { cols, page }),
    onCellClick: (id: number, e: React.MouseEvent) => sel.click(id, { shift: e.shiftKey, meta: e.metaKey || e.ctrlKey }),
    onCellDoubleClick: openLoupe,
    onRate: ratePhoto,
    burstSizes,
    catalogEmpty,
    filtered,
    onImport: importFolder,
    onClearFilters: clearFilters,
  };
  const frozenGrid = useRef(liveGridProps);
  if (mode === "grid") frozenGrid.current = liveGridProps;
  const gridProps = frozenGrid.current;

  return (
    <main className="flex h-screen flex-col">
      {catalog && (catalog.health.restorePending || !healthDismissed) && <HealthBanner health={catalog.health} onRestore={() => setRestoreOpen(true)} onDismiss={() => setHealthDismissed(true)} />}
      {status.catalogIssue && catalog?.health.status !== "read_only" && (
        <IssueBanner issue={status.catalogIssue} onDismiss={() => status.setCatalogIssue(null)} onRestore={catalog && catalog.health.backups.length > 0 ? () => setRestoreOpen(true) : undefined} />
      )}
      <TopBar
        catalog={scopedCatalog}
        project={project}
        onHome={onHome}
        onOpenProject={onOpenProject}
        onSetCover={active != null && project ? () => void setCover(active) : null}
        analysis={status.analysis}
        xmp={status.xmp}
        xmpFailures={xmpFailureRows}
        onOpenXmpErrors={openXmpErrors}
        onShowXmpFailure={showXmpFailure}
        onXmpExplain={() => setExplainOpen(true)}
        busy={busy}
        mode={planOpen ? "plan" : mode}
        onMode={changeMode}
        compareOn={cmp != null}
        scenesCount={step === "edit" ? 0 : scenes.scenes.length}
        scenesOpen={scenesOpen}
        onToggleScenes={toggleScenes}
        hasSelection={targets().length > 0}
        hasImages={ids.length > 0}
        detecting={scenes.detecting}
        steps={
          project ? (
            <StepBar
              step={step ?? "cull"}
              exporting={exportOpen != null}
              cullSub={
                status.analysis?.running && status.analysis.total > 0
                  ? `Analyzing ${Math.round((status.analysis.done / status.analysis.total) * 100)}%`
                  : project.keeperCount > 0
                    ? `${project.keeperCount} keepers`
                    : "No keepers yet"
              }
              editSub={
                wf.grouping
                  ? "Grouping…"
                  : wf.rows.length > 0
                    ? wf.done
                      ? "All scenes applied"
                      : wf.plan?.outdated
                        ? "Needs a look"
                        : `${wf.rows.filter((r) => r.ui === "applied" || r.skipped).length} of ${wf.rows.length} scenes`
                    : null
              }
              exportSub={exportPct != null ? `Exporting ${exportPct}%` : exportedCount != null ? `Exported ${exportedCount}` : `${project.keeperCount} photos`}
              exportPct={exportPct}
              editDone={wf.done}
              keeperNote={cullSum.summary ? keeperFormula(cullSum.summary) : undefined}
              onStep={goStep}
            />
          ) : undefined
        }
        legacyControls={catalogEmpty}
        plan={project && step === "edit" ? { on: planOpen, onPlan: openPlan } : undefined}
        onImport={importFolder}
        importOptions={importOpts}
        onImportOptions={setImportOpts}
        onShootType={onShootTypeChange}
        onAnalyze={analyze}
        onAutoAnalyze={onAutoAnalyzeChange}
        onDetectScenes={() => {
          setScenesOpen(true);
          void scenes.detect();
        }}
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
        onExport={() => void openExport()}
        onCheatSheet={() => setCheatOpen(true)}
        onModels={() => setModelsOpen(true)}
        infoOpen={infoOpen}
        onInfo={() => setInfoOpen((v) => !v)}
        onCaptureTime={() => {
          const t = targets();
          if (t.length > 0) setCaptureOpen(t);
        }}
        onLocate={() => locateFolder()}
        onRegenerate={() =>
          void run(async () => {
            const t = targets();
            await unwrap(commands.regenerateThumbnails(t));
            setNotice(`Regenerating previews for ${plural(t.length, "photo")}`);
          })
        }
        exportPct={exportPct}
      />
      {exportOpen && (
        <ErrorBoundary view="Export" overlay onExit={() => setExportOpen(null)}>
          <ExportDialog
            selectionIds={exportOpen.sel}
            filteredIds={ids}
            keeperIds={exportOpen.keepers}
            summary={cullSum.summary}
            onKeeperRule={async (r) => {
              await changeKeeperRule(r);
              void openExport(true);
            }}
            sampleEntry={(id) => lib.getEntry(id)}
            onClose={() => setExportOpen(null)}
            onStarted={(job) => {
              exportJobs.track(job);
              if (exportOpen.keepers) void setStep("export");
            }}
          />
        </ErrorBoundary>
      )}
      {restoreOpen && catalog && <RestoreBackupDialog backups={catalog.health.backups} onCancel={() => setRestoreOpen(false)} onRestore={restoreBackup} />}
      {modelsOpen && <ModelsDialog onClose={() => setModelsOpen(false)} />}
      {infoOpen && (
        <PhotoInfoPanel
          imageId={mode === "compare" && cmp ? cmp[cmp.focus] : (active ?? null)}
          refetchKey={`${lib.epoch}:${active != null ? lib.version(active) : 0}`}
          onClose={() => setInfoOpen(false)}
          onEditTime={() => {
            const t = targets();
            if (t.length > 0) setCaptureOpen(t);
          }}
          onRevert={(id) => void applyCaptureTime([id], { kind: "revert" }, "Reverted capture time to original")}
        />
      )}
      {captureOpen && <CaptureTimeDialog targetIds={captureOpen} activeId={active ?? null} viewIds={ids} onApply={applyCaptureTime} onCancel={() => setCaptureOpen(null)} />}
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
      {project && <StyleDialogs wf={wf} fileName={(id) => lib.getEntry(id)?.fileName ?? `#${id}`} />}
      {(explainOpen || explainerDue) && <XmpExplainer autoSync={status.catalog?.xmpAutoSync ?? false} onClose={closeExplainer} />}
      {cheatOpen && <CheatSheet mode={mode} editStep={projectId != null && step === "edit" && (planOpen || mode === "develop")} onClose={() => setCheatOpen(false)} />}
      {analysisBarShown && status.analysis && (
        <AnalysisBar a={status.analysis} onCancel={() => void run(() => unwrap(commands.cancelAnalysis()))} onDismiss={() => status.setAnalysis(null)} />
      )}
      {importBarShown && status.progress && <ImportBar progress={status.progress} active={importActive} />}

      {idFilter && !planOpen && (
        <div className="flex h-7 shrink-0 items-center gap-3 border-b border-amber-900 bg-amber-950 px-3 text-xs text-amber-100" data-testid="id-filter-bar">
          <span>Showing {idFilter.label}</span>
          <button className="rounded bg-amber-800 px-2 py-0.5 hover:bg-amber-700" data-testid="id-filter-clear" onClick={() => setIdFilter(null)}>
            Show all
          </button>
        </div>
      )}
      {planOpen ? null : mode === "grid" ? (
        catalogEmpty ? null : (
        <>
          {filtersOpen ? (
            <>
              <FilterBar query={uiQuery} setQuery={setQuery} counts={counts} onLocate={() => locateFolder()} />
              {metaOpen && <MetadataRow query={query} setQuery={setQuery} epoch={lib.epoch} />}
            </>
          ) : (
            <FilterSummary query={uiQuery} shown={ids.length} total={totals?.total ?? null} sceneNumber={scenes.number} onEdit={() => setFiltersOpen(true)} unit={keepersStep ? "keepers" : ""} />
          )}
          <GridToolbar
            query={query}
            setQuery={setQuery}
            size={size}
            onSize={setSize}
            selectedCount={sel.selected.size}
            total={ids.length}
            catalogTotal={totals?.total ?? null}
            unit={keepersStep ? "keepers" : "photos"}
            capsLock={caps}
            autoAdvance={autoAdvance}
            onAutoAdvance={setAutoAdvance}
            leading={
              project ? (
                <>
                  <ShootSelect catalog={scopedCatalog} onShootType={onShootTypeChange} />
                  <AnalyzeSplit
                    catalog={scopedCatalog}
                    analysis={status.analysis}
                    hasImages={ids.length > 0}
                    detecting={scenes.detecting}
                    onAnalyze={analyze}
                    onAutoAnalyze={onAutoAnalyzeChange}
                    onDetectScenes={() => {
                      setScenesOpen(true);
                      void scenes.detect();
                    }}
                  />
                </>
              ) : undefined
            }
            trailing={
              project && step === "cull" ? (
                <button
                  className="flex h-7 items-center gap-1.5 whitespace-nowrap rounded-md bg-emerald-700 px-3 text-xs font-medium text-white hover:bg-emerald-600"
                  data-testid="continue-edit"
                  onClick={() => goStep("edit")}
                  title={`Group the keepers into scenes and edit one photo per scene.${cullSum.summary ? ` ${keeperFormula(cullSum.summary)}. Change the keeper rule in the Cull summary bar.` : ""}`}
                >
                  Continue to Edit{project.keeperCount > 0 ? ` · ${project.keeperCount} keepers` : ""} <ChevronRight className="size-3.5" />
                </button>
              ) : undefined
            }
            filters={filtersOpen ? <FilterExtras query={uiQuery} setQuery={setQuery} counts={counts} catalog={scopedCatalog} onLocate={() => locateFolder()} /> : null}
          />
          {project && step === "cull" && cullSum.summary && (
            <CullSummaryBar summary={cullSum.summary} query={uiQuery} setQuery={setQuery} onKeeperRule={(r) => void changeKeeperRule(r)} onApplySuggestions={askApplySuggestions} strictness={project.rejectStrictness} onStrictness={(v) => void changeRejectStrictness(v)} />
          )}
        </>
        )
      ) : mode === "develop" || (mode === "loupe" && loupePanels.chrome) ? null : (
        <FilterSummary
          query={uiQuery}
          shown={ids.length}
          total={totals?.total ?? null}
          unit={keepersStep ? "keepers" : ""}
          sceneNumber={scenes.number}
          onEdit={() => {
            changeMode("grid");
            setFiltersOpen(true);
          }}
        />
      )}

      {scenesOpen && !(project && step === "edit") && (
      <SceneStrip
        api={scenes}
        filterId={query.sceneId ?? null}
        onFilter={(id) => setQuery((q) => ({ ...q, sceneId: id }))}
        targets={targets()}
        activeId={active ?? null}
        onMatch={(s) => setMatchOpen(s.id)}
        onHide={toggleScenes}
      />
      )}

      {mode === "grid" && !planOpen && (query.sceneId != null || sel.selected.size > 1) && (
        <SelectionBar
          selected={sel.selected.size}
          sceneCount={query.sceneId != null ? ids.length : null}
          hasActive={active != null}
          onSelectAll={sel.selectAll}
          onCopy={() => void copyActive()}
          onPaste={() => void pasteToSelection()}
          onSync={() => void syncSelection()}
          onEditAll={() => {
            const list = sel.selected.size > 1 ? ids.filter((i) => sel.selected.has(i)) : ids;
            if (list.length > 0) editAllInScene(list, active != null && list.includes(active) ? active : list[0]);
          }}
        />
      )}

      <div className="relative flex min-h-0 flex-1 flex-col" data-mode={mode}>
        <ErrorBoundary view="Library" onReload={() => void lib.reset()}>
        <PhotoGrid {...gridProps} />
        </ErrorBoundary>
        {planOpen && project && step === "edit" && (
          <ErrorBoundary view="Plan" overlay onExit={() => setPlanOpen(false)}>
            <PlanView
              wf={wf}
              lib={lib}
              sceneTimes={(id) => {
                const sc = scenes.scenes.find((x) => x.id === id);
                return { start: sc?.startedAtMs ?? null, end: sc?.endedAtMs ?? null };
              }}
              focusId={planFocus}
              onFocus={setPlanFocus}
              onEdit={(id) => openScene(id)}
              onReview={reviewFrames}
              onShowScene={(id) => {
                setPlanOpen(false);
                setQuery((q) => ({ ...q, sceneId: id }));
              }}
              onChangeRep={(id) => {
                openScene(id);
                setNotice("Press Shift+A on the photo you want as the representative");
              }}
              onApplyOptions={(id) => setMatchOpen(id)}
              onShowIds={showIds}
              onBackToCull={() => goStep("cull")}
              onContinueExport={() => goStep("export")}
              onRegroup={() => void wf.regroup()}
              summary={cullSum.summary}
            />
          </ErrorBoundary>
        )}
        {mode === "develop" && (
          <ErrorBoundary view="Develop" overlay onExit={() => changeMode("grid")}>
          <DevelopView
            key={devEpoch}
            ref={develop}
            lib={lib}
            sel={sel}
            onError={reportError}
            onNotice={setNotice}
            onCommitted={wf.noteCommit}
            onUndoToast={(msg, undo) => push(msg, { action: { label: "Undo", testid: "batch-undo", onClick: undo } })}
            onBatch={wf.reportBatch}
            onBack={() => changeMode("grid")}
            onLocate={locateFolder}
            compare={cmp}
            onFocusPane={focusPane}
            onCandidate={setCandidate}
            onSwap={swapCompare}
            onMakeSelect={makeSelect}
            onToggleCompare={() => (cmp ? setCmp(null) : void enterCompare())}
            onRate={ratePhoto}
            onFlag={flagPhoto}
            filterSummary={{
              text: filterSummaryText(uiQuery, ids.length, totals?.total ?? null, scenes.number, keepersStep ? "keepers" : ""),
              onEdit: () => {
                changeMode("grid");
                setFiltersOpen(true);
              },
            }}
            onLabel={labelPhoto}
            topSlot={
              project && step === "edit" ? (
                <EditContextBar
                  wf={wf}
                  rows={wf.rows}
                  activeId={active ?? null}
                  fileName={(id) => lib.getEntry(id)?.fileName ?? `#${id}`}
                  onJump={(id) => openScene(id)}
                  onStepScene={(dir) => stepScene(dir, dir === 1)}
                  onPlan={openPlan}
                  onMakeRep={makeRepresentative}
                  onApplyOptions={(id) => setMatchOpen(id)}
                  onShowIds={showIds}
                  onReview={reviewFrames}
                  onNextReview={nextReview}
                />
              ) : undefined
            }
            sceneOnly={
              project && step === "edit"
                ? {
                    on: query.sceneId != null,
                    toggle: () => {
                      const row = active != null ? rowOfImage.get(active) : undefined;
                      setQuery((q) => (q.sceneId != null ? { ...q, sceneId: null } : row ? { ...q, sceneId: row.entry.sceneId } : q));
                    },
                  }
                : undefined
            }
            filmBadge={
              project && step === "edit"
                ? (fid) => {
                    const row = rowOfImage.get(fid);
                    if (!row) return null;
                    const isRep = row.entry.representativeId === fid;
                    return (
                      <>
                        {isRep && (
                          <>
                            <span className="pointer-events-none absolute inset-0 rounded ring-2 ring-inset ring-amber-400" data-testid={`film-rep-${fid}`} aria-hidden />
                            <span className="absolute left-0.5 top-4 rounded bg-black/70 px-0.5 text-[9px] font-bold text-amber-400" title="Representative: the photo you edit for this scene; Apply to scene copies its look to the others" aria-label="Representative of the scene">R</span>
                          </>
                        )}
                        {!isRep && wf.needsReviewSet.has(fid) && <span className="absolute right-3.5 top-0 text-[11px] font-bold text-amber-400" title="Needs a look: the applied edit did not match this frame well" aria-label="Needs a look" data-testid={`film-review-${fid}`}>!</span>}
                        {!isRep && !wf.needsReviewSet.has(fid) && row.entry.appliedIds.includes(fid) && <span className="absolute right-3.5 top-0 text-[11px] font-bold text-emerald-400" title="The scene edit has been applied to this photo" aria-label="Edit applied" data-testid={`film-applied-${fid}`}>✓</span>}
                      </>
                    );
                  }
                : undefined
            }
          />
          </ErrorBoundary>
        )}
        {(mode === "loupe" || mode === "compare") && (
          <ErrorBoundary view="Library" overlay onExit={() => changeMode("grid")}>
          <LoupeLayer
            ref={loupe}
            mode={mode}
            lib={lib}
            activeId={active}
            compare={cmp}
            onFocusPane={focusPane}
            onOpen={(id) => sel.set([id], id)}
            onCandidate={setCandidate}
            onSwap={swapCompare}
            onMakeSelect={makeSelect}
            onEditCompare={openDevelop}
            onExitCompare={() => openLoupe(cmp ? cmp[cmp.focus] : undefined)}
            onRate={ratePhoto}
            onLocate={locateFolder}
            burstSizes={burstSizes}
          />
          </ErrorBoundary>
        )}
      </div>
      {matchScene && (
        <MatchPanel
          // In the Edit step the scene's representative is the single anchor of the preview (no anchors are graded there).
          scene={step === "edit" && matchScene.anchorIds.length === 0 ? { ...matchScene, anchorIds: [rowOfScene(matchScene.id)?.entry.representativeId ?? matchScene.imageIds[0]] } : matchScene}
          sceneNumber={scenes.number(matchScene.id)}
          keeperIds={step === "edit" ? wf.plan?.keeperIds : undefined}
          progress={scenes.progress}
          fileName={(id) => lib.getEntry(id)?.fileName ?? `#${id}`}
          onClose={() => setMatchOpen(null)}
          onApplied={
            step === "edit"
              ? undefined
              : (changed, attempted) => {
                  setMatchOpen(null);
                  void lib.refresh(attempted.filter((id) => lib.getEntry(id))).catch(reportError);
                  setDevEpoch((n) => n + 1);
                  wf.refreshPlan();
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
                }
          }
          onApply={(options) => {
            const id = matchScene.id;
            setMatchOpen(null);
            void wf.applyScene(id, options, reviewFrames, showIds).then(() => {
              setDevEpoch((n) => n + 1);
            });
          }}
        />
      )}
      <Toasts api={toasts} placement={mode === "develop" ? "top" : "bottom"} error={status.error} onDismissError={() => setError(null)} onLocate={() => locateFolder(active ?? undefined)} />
      <HelpPanel onShortcuts={() => setCheatOpen(true)} />
      <ActivityWidget behindDialogs={exportOpen != null || modalOpenCount > 0 || helpState.open} hasChildren={exportJobs.jobs.length > 0} hideKinds={hiddenActivityKinds}>
        <ExportJobsPanel jobs={exportJobs.jobs} onCancel={(id) => void exportJobs.cancel(id)} onDismiss={exportJobs.dismiss} onReveal={revealInFinder} />
      </ActivityWidget>
    </main>
  );
}
