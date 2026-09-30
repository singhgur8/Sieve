// In-browser fake backend for UI tests and design work: `pnpm dev` then open `/?mock=5000`.
// Uses Tauri's official IPC mocks; image bytes are served by the test harness (Playwright route)
// or fall back to broken images when opened by hand. Loaded only in dev builds (see main.tsx).
import { mockIPC } from "@tauri-apps/api/mocks";
import type {
  BurstGroup,
  CatalogState,
  CullTag,
  FaceInfo,
  FilterCounts,
  ImageQuery,
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
  }
}

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
    if (inBurst && i % 25 !== 1) tags.push({ tag: "duplicate_burst", source: "auto", confidence: 1, suppressed: false });
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
        default:
          return null;
      }
    },
    { shouldMockEvents: true },
  );
  // Files are served from the page origin as-is (no asset:// protocol in the browser).
  (window as unknown as { __TAURI_INTERNALS__: { convertFileSrc: (p: string) => string } }).__TAURI_INTERNALS__.convertFileSrc = (p) => p;
}
