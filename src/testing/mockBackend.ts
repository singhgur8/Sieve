// In-browser fake backend for UI tests and design work: `pnpm dev` then open `/?mock=5000`.
// Uses Tauri's official IPC mocks; image bytes are served by the test harness (Playwright route)
// or fall back to broken images when opened by hand. Loaded only in dev builds (see main.tsx).
import { mockIPC } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import { neutralAdjustments, copyFields } from "../lib/adjust";
import type {
  BurstGroup,
  CatalogState,
  CullTag,
  FaceInfo,
  AdjustmentField,
  AdjustmentHistory,
  FilterCounts,
  HistoryEntry,
  ExportFailure,
  ExportJob,
  ExportPreset,
  ExportSettings,
  ImageQuery,
  LutInfo,
  ParametricAdjustments,
  Preset,
  RenderOptions,
  PickFlag,
  RawImageEntry,
} from "../ipc";

const TAGS: CullTag[] = ["blink", "missed_focus", "motion_blur", "creative_blur", "underexposed", "overexposed", "duplicate_burst"];
const LABELS = [null, null, null, "red", "yellow", "green", "blue", "purple"] as const;

export interface MockCall {
  cmd: string;
  args: Record<string, unknown>;
}

declare global {
  interface Window {
    __ipcLog: MockCall[];
    /** Test hook: delay (ms) before a `render_preview` call with this per-slot sequence number resolves. */
    __mockRenderDelay?: (seq: number, slot: string) => number;
    /** Test hook: when true, export jobs only advance through `__mockExportStep`. */
    __mockExportManual?: boolean;
    /** Advances the running mock export job by n files (default 1); finishes it when done. */
    __mockExportStep?: (n?: number) => void;
  }
}


// ---- export (v6) emulation ----
const BASE_EXPORT: ExportSettings = {
  format: { kind: "jpeg", quality: 90, chromaSubsampling: "444" },
  colorSpace: "srgb",
  resize: { mode: { kind: "none" }, dontEnlarge: true, resolutionPpi: 300 },
  sharpening: null,
  naming: { template: "{filename}", startNumber: 1, collision: "unique_suffix" },
  destination: { kind: "choose" },
  subfolder: null,
  metadata: { include: "all", removeLocation: false, includeKeywords: true, copyright: null, creator: null },
};
const BUILTIN_EXPORT_PRESETS: ExportPreset[] = [
  { id: -1, name: "Client JPEG full-res sRGB q90", builtIn: true, settings: BASE_EXPORT, createdAtMs: 0, updatedAtMs: 0 },
  {
    id: -2,
    name: "Web 2048 sRGB",
    builtIn: true,
    createdAtMs: 0,
    updatedAtMs: 0,
    settings: {
      ...BASE_EXPORT,
      format: { kind: "jpeg", quality: 80, chromaSubsampling: "420" },
      resize: { mode: { kind: "long_edge", px: 2048 }, dontEnlarge: true, resolutionPpi: 72 },
      sharpening: { media: "screen", amount: "standard" },
      metadata: { include: "copyright_only", removeLocation: true, includeKeywords: true, copyright: null, creator: null },
    },
  },
  {
    id: -3,
    name: "Print TIFF 16-bit Adobe RGB",
    builtIn: true,
    createdAtMs: 0,
    updatedAtMs: 0,
    settings: {
      ...BASE_EXPORT,
      format: { kind: "tiff", bitDepth: "16", compression: "lzw" },
      colorSpace: "adobe_rgb",
      sharpening: { media: "glossy", amount: "standard" },
    },
  },
];

function rng(seed: number) {
  let s = seed >>> 0;
  return () => {
    s = (Math.imul(s, 1664525) + 1013904223) >>> 0;
    return s / 4294967296;
  };
}

export function installMockBackend(count: number) {
  window.__ipcLog = [];
  const rand = rng(42);
  const base = Date.UTC(2026, 5, 1, 14, 0, 0);
  const rows: RawImageEntry[] = [];
  const bursts = new Map<number, BurstGroup>();

  for (let i = 0; i < count; i++) {
    const id = i + 1;
    const inBurst = i % 25 < 4;
    const group = Math.floor(i / 25) + 1;
    const tags = TAGS.filter(() => rand() < 0.08).map((tag) => ({ tag, source: "auto" as const, confidence: 0.8, suppressed: false }));
    if (inBurst && i % 25 !== 1 && !tags.some((t) => t.tag === "duplicate_burst")) tags.push({ tag: "duplicate_burst", source: "auto", confidence: 1, suppressed: false });
    const overall = rand();
    const entry: RawImageEntry = {
      id,
      folderId: i < count / 2 ? 1 : 2,
      path: `/shoot/DSC${String(id).padStart(5, "0")}.ARW`,
      fileName: `DSC${String(id).padStart(5, "0")}.ARW`,
      format: "arw",
      camera: { make: "sony", model: "ILCE-7M4", sensorLayout: "bayer" },
      capture: {
        capturedAtMs: base + i * 300 + group * 20000,
        iso: 100 * (1 + (i % 6)),
        shutterSeconds: 1 / (60 + (i % 5) * 60),
        aperture: 2.8,
        focalLengthMm: 85,
        lens: "FE 85mm F1.8",
      },
      width: 6000,
      height: 4000,
      orientation: 1,
      fileSize: 30_000_000,
      fileMtimeMs: base,
      thumbnail: { status: "ready", path: `/mock/thumb/${id}.jpg`, previewPath: `/mock/preview/${id}.jpg`, width: 480, height: 320 },
      rating: 0,
      pick: "unflagged",
      colorLabel: null,
      burstGroupId: inBurst ? group : null,
      isBurstKeeper: inBurst && i % 25 === 1,
      tags,
      quality: {
        overall,
        faceSharpness: rand(),
        globalSharpness: rand(),
        eyesOpen: 1,
        composition: 0.5,
        faceCount: id % 3 === 0 ? 0 : 2,
        exposure: { clippedHighlightsPct: 0, clippedShadowsPct: 0, meanLuma: 0.5 },
        modelVersion: "mock",
        suggestedRating: Math.round(overall * 5),
        suggestedPick: overall > 0.8 ? "pick" : overall < 0.15 ? "reject" : "unflagged",
      },
      hasEdits: false,
      xmp: { dirty: false, syncedAtMs: null, error: null },
    };
    rows.push(entry);
    if (inBurst) {
      const g = bursts.get(group) ?? { id: group, startedAtMs: entry.capture.capturedAtMs ?? 0, endedAtMs: 0, keeperImageId: null, imageIds: [] };
      g.imageIds.push(id);
      g.endedAtMs = entry.capture.capturedAtMs ?? 0;
      if (entry.isBurstKeeper) g.keeperImageId = id;
      bursts.set(group, g);
    }
  }
  // A few pre-set flags so screenshots show something.
  for (let i = 0; i < count; i += 7) rows[i].pick = i % 14 === 0 ? "pick" : "reject";
  for (let i = 0; i < count; i += 5) rows[i].rating = (i / 5) % 6;
  for (let i = 0; i < count; i += 11) rows[i].colorLabel = LABELS[i % LABELS.length];
  for (let i = 3; i < count; i += 40) rows[i].xmp = { dirty: true, syncedAtMs: null, error: null };
  void rand;

  let catalog: CatalogState = {
    catalogPath: "/mock/catalog.sqlite",
    imageCount: count,
    shootType: "wedding",
    burstWindowMs: 1500,
    folders: [
      { id: 1, path: "/shoot/ceremony", imageCount: Math.floor(count / 2) },
      { id: 2, path: "/shoot/reception", imageCount: count - Math.floor(count / 2) },
    ],
    tagCounts: [],
    cacheDir: "/mock/cache",
    autoAnalyze: true,
    xmpAutoSync: false,
  };

  const byId = new Map(rows.map((r) => [r.id, r]));
  const visibleTags = (r: RawImageEntry) => r.tags.filter((t) => !t.suppressed).map((t) => t.tag);

  function query(q: ImageQuery): number[] {
    let out = rows.filter((r) => {
      if (q.folderId != null && r.folderId !== q.folderId) return false;
      const t = visibleTags(r);
      if (q.includeTags.length) {
        const ok = q.tagMatch === "all" ? q.includeTags.every((x) => t.includes(x)) : q.includeTags.some((x) => t.includes(x));
        if (!ok) return false;
      }
      if (q.excludeTags.some((x) => t.includes(x))) return false;
      if (q.picks.length && !q.picks.includes(r.pick)) return false;
      if (q.minRating != null && r.rating < q.minRating) return false;
      if (q.maxRating != null && r.rating > q.maxRating) return false;
      if (q.colorLabels.length && (!r.colorLabel || !q.colorLabels.includes(r.colorLabel))) return false;
      if (q.collapseBursts && r.burstGroupId != null && !r.isBurstKeeper) return false;
      return true;
    });
    const key: Record<string, (r: RawImageEntry) => number | string> = {
      capture_time: (r) => r.capture.capturedAtMs ?? 0,
      file_name: (r) => r.fileName,
      quality: (r) => r.quality?.overall ?? 0,
      rating: (r) => -r.rating,
    };
    const k = key[q.sort];
    out = [...out].sort((a, b) => (k(a) < k(b) ? -1 : k(a) > k(b) ? 1 : a.id - b.id));
    if (q.sortDescending) out.reverse();
    return out.map((r) => r.id);
  }

  function counts(folderId: number | null): FilterCounts {
    const scope = rows.filter((r) => folderId == null || r.folderId === folderId);
    const tags = TAGS.map((tag) => ({ tag, count: scope.filter((r) => visibleTags(r).includes(tag)).length })).filter((t) => t.count > 0);
    const ratings = [0, 0, 0, 0, 0, 0];
    scope.forEach((r) => ratings[r.rating]++);
    return {
      total: scope.length,
      tags,
      picked: scope.filter((r) => r.pick === "pick").length,
      rejected: scope.filter((r) => r.pick === "reject").length,
      unflagged: scope.filter((r) => r.pick === "unflagged").length,
      ratings,
      burstGroups: new Set(scope.map((r) => r.burstGroupId).filter((g) => g != null)).size,
      burstNonKeepers: scope.filter((r) => r.burstGroupId != null && !r.isBurstKeeper).length,
    };
  }

  const faces = (id: number): FaceInfo[] =>
    id % 3 === 0
      ? []
      : [
          { x: 0.3, y: 0.3 },
          { x: 0.65, y: 0.4 },
        ].map((p, i) => ({
          bbox: { x: p.x - 0.06, y: p.y - 0.09, width: 0.12, height: 0.18 },
          leftEye: { x: p.x - 0.02, y: p.y - 0.02 },
          rightEye: { x: p.x + 0.02, y: p.y - 0.02 },
          detectionScore: 0.9,
          ear: 0.3,
          eyesOpen: 1,
          sharpness: 0.7,
          blink: false,
          inFocus: true,
          primary: i === 1,
          considered: true,
        }));

  // ---- develop (v5) emulation: adjustments, linear history with cursor, presets, LUTs, renders ----
  interface Hist {
    entries: (HistoryEntry & { snap: ParametricAdjustments })[];
    cursor: number; // index into entries, -1 = never edited
    lastAt: number;
  }
  const adjs = new Map<number, ParametricAdjustments>();
  const hists = new Map<number, Hist>();
  const seqs = new Map<string, number>();
  let entryId = 0;
  let presetId = 0;
  const presets: Preset[] = [];
  const luts: LutInfo[] = [
    { id: "film-warm", name: "Film Warm", kind: "lut_3d", size: 33, path: "/mock/luts/film-warm.cube" },
    { id: "teal-orange", name: "Teal Orange", kind: "lut_3d", size: 33, path: "/mock/luts/teal-orange.cube" },
  ];
  const getAdj = (id: number) => adjs.get(id) ?? neutralAdjustments();
  const histOf = (id: number): Hist => {
    let h = hists.get(id);
    if (!h) hists.set(id, (h = { entries: [], cursor: -1, lastAt: 0 }));
    return h;
  };
  const historyDto = (id: number): AdjustmentHistory => {
    const h = histOf(id);
    return {
      imageId: id,
      entries: h.entries.map(({ id: i, label, createdAtMs }) => ({ id: i, label, createdAtMs })),
      currentEntryId: h.cursor < 0 ? null : h.entries[h.cursor].id,
      canUndo: h.cursor > 0,
      canRedo: h.cursor >= 0 && h.cursor < h.entries.length - 1,
    };
  };
  const isNeutral = (a: ParametricAdjustments) => JSON.stringify(a) === JSON.stringify(neutralAdjustments());
  function commit(id: number, next: ParametricAdjustments, label: string) {
    const cur = getAdj(id);
    const h = histOf(id);
    if (JSON.stringify(cur) === JSON.stringify(next)) return;
    if (h.cursor < 0) {
      h.entries.push({ id: ++entryId, label: "Original", createdAtMs: Date.now(), snap: cur });
      h.cursor = 0;
    }
    const now = Date.now();
    h.entries.length = h.cursor + 1;
    const last = h.entries[h.cursor];
    if (h.cursor > 0 && last.label === label && now - h.lastAt < 1500) {
      last.snap = next;
    } else {
      h.entries.push({ id: ++entryId, label, createdAtMs: now, snap: next });
      h.cursor++;
    }
    h.lastAt = now;
    adjs.set(id, next);
    const r = byId.get(id);
    if (r) r.hasEdits = !isNeutral(next);
  }
  function jump(id: number, cursor: number) {
    const h = histOf(id);
    if (h.cursor < 0) return { adjustments: getAdj(id), history: historyDto(id) };
    h.cursor = Math.max(0, Math.min(h.entries.length - 1, cursor));
    h.lastAt = 0;
    const snap = h.entries[h.cursor].snap;
    adjs.set(id, snap);
    const r = byId.get(id);
    if (r) r.hasEdits = !isNeutral(snap);
    return { adjustments: snap, history: historyDto(id) };
  }
  function histogram(a: ParametricAdjustments) {
    const shift = a.exposure * 18 + a.contrast * 0.1;
    const bump = (mu: number, sd: number) => Array.from({ length: 256 }, (_, i) => Math.round(4000 * Math.exp(-((i - mu) ** 2) / (2 * sd * sd))));
    return { red: bump(120 + shift + 8, 40), green: bump(110 + shift, 36), blue: bump(95 + shift - 8, 44), luma: bump(112 + shift, 38) };
  }
  async function render(id: number, a: ParametricAdjustments, o: RenderOptions) {
    const key = `${id}:${o.slot}`;
    const seq = (seqs.get(key) ?? 0) + 1;
    seqs.set(key, seq);
    const delay = window.__mockRenderDelay?.(seq, o.slot) ?? 0;
    if (delay > 0) await new Promise((r) => setTimeout(r, delay));
    const q = new URLSearchParams({ v: String(seq), e: a.exposure.toFixed(2), lut: a.lut?.id ?? "", region: o.region ? "1" : "" });
    return {
      imageId: id,
      slot: o.slot,
      seq,
      url: `/mock/render/${id}/${o.slot}?${q}`,
      width: o.maxEdge,
      height: Math.round(o.maxEdge * (o.region ? 1 : 2 / 3)),
      histogram: histogram(a),
      renderMs: 7 + (seq % 5),
      lutMissing: !!a.lut && !luts.some((l) => l.id === a.lut!.id),
    };
  }
  function batch(ids: number[], label: string, fn: (a: ParametricAdjustments) => ParametricAdjustments) {
    ids.forEach((i) => commit(i, fn(getAdj(i)), label));
    return null;
  }

  // ---- export (v6) state: fake jobs that emit progress over time and honour cancel ----
  const exportPresets: ExportPreset[] = [];
  let exportPresetId = 0;
  let exportJobId = 0;
  const exportJobs: ExportJob[] = [];
  interface Run {
    job: ExportJob;
    ids: number[];
    timer: ReturnType<typeof setInterval> | null;
    cancelled: boolean;
  }
  let activeRun: Run | null = null;
  const allExportPresets = () => [...BUILTIN_EXPORT_PRESETS, ...exportPresets];
  function stepRun(run: Run, n: number) {
    for (let k = 0; k < n && run.job.done < run.job.total && !run.cancelled; k++) {
      const id = run.ids[run.job.done];
      const r = byId.get(id);
      run.job.done++;
      if (id % 7 === 0) {
        run.job.failed++;
        run.job.failures.push({ imageId: id, fileName: r?.fileName ?? String(id), reason: "Decode error (mock)" });
      } else run.job.succeeded++;
      void emit("export-progress", {
        jobId: run.job.id,
        done: run.job.done,
        total: run.job.total,
        failed: run.job.failed,
        skipped: 0,
        currentFile: run.job.done < run.job.total ? (byId.get(run.ids[run.job.done])?.fileName ?? null) : null,
      });
    }
    if (run.cancelled || run.job.done >= run.job.total) finishRun(run);
  }
  function finishRun(run: Run) {
    if (run.timer) clearInterval(run.timer);
    run.job.state = run.cancelled ? "cancelled" : "completed";
    run.job.finishedAtMs = Date.now();
    if (activeRun === run) activeRun = null;
    const failed: ExportFailure[] = run.job.failures;
    void emit("export-finished", {
      jobId: run.job.id,
      succeeded: run.job.succeeded,
      skipped: 0,
      failed,
      cancelled: run.cancelled,
      outputDir: run.job.outputDir,
      elapsedMs: 1234,
    });
  }
  window.__mockExportStep = (n = 1) => {
    if (activeRun) stepRun(activeRun, n);
  };

  const ok = { succeeded: 0, skipped: 0, failed: [], changed: [] };

  mockIPC(
    (cmd, payload) => {
      const args = (payload ?? {}) as Record<string, unknown>;
      window.__ipcLog.push({ cmd, args });
      const ids = (args.ids as number[] | undefined) ?? [];
      switch (cmd) {
        case "get_catalog_state":
          return catalog;
        case "list_image_ids":
          return query(args.query as ImageQuery);
        case "list_images": {
          const q = args.query as ImageQuery;
          const all = query(q);
          return { items: all.slice(q.offset, q.offset + q.limit).map((i) => byId.get(i)), total: all.length };
        }
        case "get_images":
          return ids.map((i) => byId.get(i));
        case "get_image":
          return byId.get(args.id as number);
        case "get_filter_counts":
          return counts(args.folderId as number | null);
        case "get_import_status":
          return { total: count, pending: 0, ready: count, failed: 0, running: false };
        case "get_analysis_status":
          return { total: count, analyzed: count, failed: 0, pending: 0, waiting: 0, running: false };
        case "get_xmp_status":
          return { dirty: rows.filter((r) => r.xmp.dirty).length, failed: 0, running: false, autoSync: catalog.xmpAutoSync };
        case "set_pick":
          ids.forEach((i) => {
            const r = byId.get(i);
            if (r) r.pick = args.pick as PickFlag;
          });
          return null;
        case "set_rating":
          ids.forEach((i) => {
            const r = byId.get(i);
            if (r) r.rating = args.rating as number;
          });
          return null;
        case "set_color_label":
          ids.forEach((i) => {
            const r = byId.get(i);
            if (r) r.colorLabel = args.label as RawImageEntry["colorLabel"];
          });
          return null;
        case "get_faces":
          return faces(args.id as number);
        case "list_burst_groups":
          return [...bursts.values()];
        case "write_xmp":
          ids.forEach((i) => {
            const r = byId.get(i);
            if (r) r.xmp = { dirty: false, syncedAtMs: Date.now(), error: null };
          });
          return { ...ok, succeeded: ids.length, changed: [] };
        case "read_xmp":
          return { ...ok, skipped: ids.length };
        case "apply_suggestions":
          return ids.length;
        case "set_xmp_auto_sync":
          catalog = { ...catalog, xmpAutoSync: args.enabled as boolean };
          return null;
        case "set_auto_analyze":
          catalog = { ...catalog, autoAnalyze: args.enabled as boolean };
          return null;
        case "set_shoot_type":
          catalog = { ...catalog, shootType: args.shootType as CatalogState["shootType"] };
          return null;
        case "get_adjustments":
          return getAdj(args.id as number);
        case "save_adjustments":
          commit(args.id as number, args.adjustments as ParametricAdjustments, args.label as string);
          return historyDto(args.id as number);
        case "get_history":
          return historyDto(args.id as number);
        case "undo_adjustments": {
          const h = histOf(args.id as number);
          return jump(args.id as number, h.cursor - 1);
        }
        case "redo_adjustments": {
          const h = histOf(args.id as number);
          return jump(args.id as number, h.cursor + 1);
        }
        case "goto_history": {
          const h = histOf(args.id as number);
          return jump(args.id as number, h.entries.findIndex((e) => e.id === args.entryId));
        }
        case "get_develop_info":
          return { imageId: args.id, asShot: { temperatureK: 5200, tint: 8 }, sourceWidth: 3000, sourceHeight: 2000, fullWidth: 6000, fullHeight: 4000 };
        case "prepare_develop":
          return null;
        case "render_preview":
          return render(args.id as number, args.adjustments as ParametricAdjustments, args.options as RenderOptions);
        case "paste_settings":
          return batch(ids, "Paste Settings", (a) => copyFields(a, args.adjustments as ParametricAdjustments, args.fields as AdjustmentField[]));
        case "sync_settings": {
          const src = getAdj(args.sourceId as number);
          return batch(args.targetIds as number[], "Sync Settings", (a) => copyFields(a, src, args.fields as AdjustmentField[]));
        }
        case "reset_adjustments":
          return batch(ids, "Reset", () => neutralAdjustments());
        case "apply_preset": {
          const p = presets.find((x) => x.id === args.presetId);
          if (!p) throw { kind: "not_found", message: "preset" };
          return batch(ids, `Preset: ${p.name}`, (a) => copyFields(a, p.adjustments, p.fields));
        }
        case "list_presets":
          return [...presets].sort((a, b) => a.name.localeCompare(b.name));
        case "save_preset": {
          const now = Date.now();
          const existing = presets.find((x) => x.id === args.id);
          if (existing) {
            Object.assign(existing, { name: args.name, adjustments: args.adjustments, fields: args.fields, updatedAtMs: now });
            return existing;
          }
          const p: Preset = { id: ++presetId, name: args.name as string, adjustments: args.adjustments as ParametricAdjustments, fields: args.fields as AdjustmentField[], createdAtMs: now, updatedAtMs: now };
          presets.push(p);
          return p;
        }
        case "delete_preset":
          presets.splice(presets.findIndex((x) => x.id === args.id), 1);
          return null;
        case "list_luts":
          return luts;
        case "import_lut": {
          const name = (args.path as string).split("/").pop()!.replace(/\.cube$/i, "");
          const l: LutInfo = { id: name.toLowerCase().replace(/[^a-z0-9]+/g, "-"), name, kind: "lut_3d", size: 33, path: args.path as string };
          if (!luts.some((x) => x.id === l.id)) luts.push(l);
          return l;
        }
        case "delete_lut":
          luts.splice(luts.findIndex((x) => x.id === args.id), 1);
          return null;
        case "get_export_capabilities":
          return {
            formats: [
              { kind: "jpeg", available: true, reason: null, bitDepths: ["8"], supportsMetadata: true },
              { kind: "tiff", available: true, reason: null, bitDepths: ["8", "16"], supportsMetadata: true },
              { kind: "png", available: true, reason: null, bitDepths: ["8", "16"], supportsMetadata: true },
              { kind: "webp", available: true, reason: null, bitDepths: ["8"], supportsMetadata: true },
              { kind: "heic", available: false, reason: "HEIC encoder not available", bitDepths: ["8"], supportsMetadata: true },
            ],
            maxParallel: 4,
            memoryBudgetMb: 4096,
          };
        case "list_export_presets":
          return allExportPresets();
        case "save_export_preset": {
          const name = ((args.name as string) ?? "").trim();
          if (!name || name.length > 100) throw { kind: "invalid_argument", message: "Preset name must be 1-100 characters" };
          const clash = allExportPresets().find((p) => p.name.toLowerCase() === name.toLowerCase() && p.id !== args.id);
          if (clash) throw { kind: "invalid_argument", message: `A preset named "${name}" already exists` };
          const now = Date.now();
          if (args.id != null) {
            const ex = exportPresets.find((p) => p.id === args.id);
            if (!ex) throw { kind: (args.id as number) < 0 ? "invalid_argument" : "not_found", message: "Built-in presets are read-only" };
            Object.assign(ex, { name, settings: args.settings, updatedAtMs: now });
            return ex;
          }
          const p: ExportPreset = { id: ++exportPresetId, name, builtIn: false, settings: args.settings as ExportSettings, createdAtMs: now, updatedAtMs: now };
          exportPresets.push(p);
          return p;
        }
        case "delete_export_preset": {
          const i = exportPresets.findIndex((p) => p.id === args.id);
          if (i < 0) throw { kind: "not_found", message: "preset" };
          exportPresets.splice(i, 1);
          return null;
        }
        case "plan_export": {
          const st = args.settings as ExportSettings;
          if (st.destination.kind === "choose") throw { kind: "invalid_argument", message: "Choose a destination" };
          const dir = st.destination.kind === "folder" ? st.destination.path : null;
          return {
            outputDir: dir,
            // Mock: every 10th planned file "already exists".
            files: ids.map((imageId) => ({ imageId, path: dir ? `${dir}/${imageId}.jpg` : null, exists: imageId % 10 === 0 })),
            existing: ids.filter((i) => i % 10 === 0).length,
          };
        }
        case "export_images": {
          const st = args.settings as ExportSettings;
          if (st.destination.kind === "choose") throw { kind: "invalid_argument", message: "Choose a destination folder" };
          if (ids.length === 0) throw { kind: "invalid_argument", message: "Nothing to export" };
          const uniq = [...new Set(ids)];
          const job: ExportJob = {
            id: ++exportJobId,
            state: "running",
            presetName: (args.presetName as string | null) ?? null,
            format: st.format.kind,
            total: uniq.length,
            done: 0,
            succeeded: 0,
            failed: 0,
            skipped: 0,
            outputDir: st.destination.kind === "folder" ? st.destination.path + (st.subfolder ? `/${st.subfolder}` : "") : null,
            failures: [],
            createdAtMs: Date.now(),
            finishedAtMs: null,
          };
          exportJobs.unshift(job);
          const run: Run = { job, ids: uniq, timer: null, cancelled: false };
          activeRun = run;
          if (!window.__mockExportManual) run.timer = setInterval(() => stepRun(run, 1), 120);
          return job;
        }
        case "cancel_export": {
          if (!activeRun || activeRun.job.id !== args.jobId) throw { kind: "not_found", message: "no such running job" };
          const run = activeRun;
          run.cancelled = true;
          // Like the real engine: finishes asynchronously.
          setTimeout(() => finishRun(run), 30);
          return null;
        }
        case "get_export_jobs":
          return exportJobs.map((j) => ({ ...j }));
        case "plugin:dialog|open":
          return (args.options as { directory?: boolean } | undefined)?.directory ? "/mock/export/Smith Wedding" : "/mock/import/Moody Blue.cube";
        default:
          return null;
      }
    },
    { shouldMockEvents: true },
  );
  // Files are served from the page origin as-is (no asset:// protocol in the browser).
  (window as unknown as { __TAURI_INTERNALS__: { convertFileSrc: (p: string) => string } }).__TAURI_INTERNALS__.convertFileSrc = (p) => p;
}
