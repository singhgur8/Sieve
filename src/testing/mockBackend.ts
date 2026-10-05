// In-browser fake backend for UI tests and design work: `pnpm dev` then open `/?mock=5000`.
// Uses Tauri's official IPC mocks; image bytes are served by the test harness (Playwright route)
// or fall back to broken images when opened by hand. Loaded only in dev builds (see main.tsx).
import { mockIPC } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import { neutralAdjustments, copyFields } from "../lib/adjust";
import { completeAdjustments, lerpAdjustments, orientPoint } from "../ipc";
import type {
  EditBatchInfo,
  AiMaskRequest,
  AiMaskStatus,
  MaskCapabilities,
  MaskGroup,
  ModelDownloadFinished,
  ModelDownloadProgress,
  ModelDownloadStatus,
  MaskOverlayOptions,
  NormPoint,
  DevelopWarning,
  LookProfileInfo,
  BurstGroup,
  CatalogBackup,
  CatalogHealth,
  CatalogState,
  RelocateResult,
  CullSnapshot,
  CullTag,
  UiPrefs,
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
  ImageStats,
  MatchApplication,
  MatchOptions,
  MatchPreview,
  Scene,
  SceneDetectOptions,
  LutInfo,
  ParametricAdjustments,
  Preset,
  RenderOptions,
  PickFlag,
  PickOrigin,
  RawImageEntry,
  AutoToneValues,
  CreateProjectResult,
  EditPlan,
  ImportOptions,
  KeeperRule,
  Project,
  SceneEditEntry,
  SceneApplyOptions,
  SceneApplyOutcome,
  SkippedScene,
  ApplyScenesResult,
  EditBatchResult,
  EditPlanCounts,
  EditSource,
  ImageEditState,
  XmpFailure,
  ShootType,
  StyleGroup,
  StyleModelStatus,
  StylePreset,
  StyleProfile,
  WorkflowStep,
  ActivityEvent,
  ActivityKind,
  ActivityState,
  CullSummary,
  ImageFormat,
  MetadataFilter,
  MetadataFilterOptions,
  NumberRange,
  SuggestionReason,
  CaptureTimeEdit,
  CaptureTimeEditResult,
  CaptureTimeSnapshot,
  ImageMetadata,
  PreviewVariant,
  RejectStrictness,
  UprightMode,
  UprightResult,
} from "../ipc";
import { isKeeperValues, DEFAULT_SCENE_APPLY_OPTIONS as DEFAULT_APPLY, MINOR_SCENE_MAX_KEEPERS } from "../ipc";

const TAGS: CullTag[] = ["blink", "missed_focus", "motion_blur", "creative_blur", "underexposed", "overexposed", "duplicate_burst"];
const LABELS = [null, null, null, "red", "yellow", "green", "blue", "purple"] as const;

const MOCK_MASK_CAPABILITIES: MaskCapabilities = {
  ai: [
    { kind: "subject", available: true, model: "mock-segmenter@1", reason: null },
    { kind: "sky", available: true, model: "mock-segmenter@1", reason: null },
    { kind: "background", available: true, model: "mock-segmenter@1", reason: null },
    { kind: "people", available: true, model: "mock-segmenter@1", reason: null },
    { kind: "object", available: false, model: null, reason: "mock: no object model" },
    { kind: "landscape", available: false, model: null, reason: "mock: no landscape model" },
  ],
  personParts: ["face_skin", "body_skin", "eyebrows", "eye_sclera", "iris_pupil", "lips", "teeth", "hair", "clothes"],
  landscape: [],
};

// ---- model downloads (v12) ----
/** Mirror of Rust `model_fetch::SEGMENTATION` (name, bytes). */
const MOCK_SEGMENTATION_FILES: [string, number][] = [
  ["birefnet_lite.onnx", 224_005_088],
  ["skyseg.onnx", 175_997_079],
  ["yolox_m.onnx", 101_259_744],
  ["efficientsam_ti_encoder.onnx", 24_799_761],
  ["efficientsam_ti_decoder.onnx", 16_565_728],
  ["selfie_multiclass_256x256.onnx", 16_454_560],
];
/** Segmentation models installed in the mock; `?models=missing` starts without them. */
let mockModelsInstalled = true;

/** Capabilities with the families listed in `?noai=sky,people` (URL of the mock page) switched off. */
function mockCapabilities(): MaskCapabilities {
  const off = new Set((new URLSearchParams(typeof location === "undefined" ? "" : location.search).get("noai") ?? "").split(",").filter(Boolean));
  if (!mockModelsInstalled) for (const k of ["subject", "background", "sky", "people", "parts"]) off.add(k);
  return {
    ...MOCK_MASK_CAPABILITIES,
    ai: MOCK_MASK_CAPABILITIES.ai.map((c) => (off.has(c.kind) ? { ...c, available: false, model: null, reason: `mock: ${c.kind} model not installed` } : c)),
    personParts: off.has("parts") ? [] : MOCK_MASK_CAPABILITIES.personParts,
  };
}

/** Orientation of the mock image with this id: id 21 is a portrait frame stored rotated (EXIF 8). */
const mockOrientation = (id: number) => (id === 21 ? 8 : 1);

/**
 * Generates a grayscale PNG (data URL) approximating the mask of `target` in the displayed frame:
 * brush dabs, gradients, ellipses; AI/range masks are a soft centred blob. Only for the mock.
 */
function mockOverlayPng(groups: MaskGroup[], target: { groupId: string; componentId: string | null }, w: number, h: number, orientation: number): string {
  const c = document.createElement("canvas");
  c.width = Math.max(2, Math.round(w));
  c.height = Math.max(2, Math.round(h));
  const g = c.getContext("2d")!;
  g.fillStyle = "#000";
  g.fillRect(0, 0, c.width, c.height);
  const grp = groups.find((x) => x.id === target.groupId);
  if (!grp) return c.toDataURL("image/png");
  const P = (x: number, y: number) => {
    const d = orientPoint({ x, y }, orientation);
    return { x: d.x * c.width, y: d.y * c.height };
  };
  const comps = grp.components.filter((k) => k.active && (!target.componentId || k.id === target.componentId));
  for (const comp of comps) {
    const sh = comp.shape;
    g.save();
    g.globalAlpha = comp.opacity;
    g.fillStyle = "#fff";
    g.strokeStyle = "#fff";
    if (sh.kind === "brush") {
      for (const st of sh.strokes) {
        g.fillStyle = st.erase ? "#000" : "#fff";
        for (const d of st.dabs) {
          const p = P(d.x, d.y);
          g.beginPath();
          g.arc(p.x, p.y, Math.max(2, st.radius * c.width), 0, Math.PI * 2);
          g.fill();
        }
      }
    } else if (sh.kind === "linear") {
      const a = P(sh.full.x, sh.full.y);
      const b = P(sh.zero.x, sh.zero.y);
      const gr = g.createLinearGradient(a.x, a.y, b.x, b.y);
      gr.addColorStop(0, "#fff");
      gr.addColorStop(1, "#000");
      g.fillStyle = gr;
      g.fillRect(0, 0, c.width, c.height);
    } else if (sh.kind === "radial") {
      const p = P((sh.left + sh.right) / 2, (sh.top + sh.bottom) / 2);
      const swap = orientation >= 5;
      const rx = (((sh.right - sh.left) / 2) * (swap ? c.height : c.width)) || 1;
      const ry = (((sh.bottom - sh.top) / 2) * (swap ? c.width : c.height)) || 1;
      g.beginPath();
      g.ellipse(p.x, p.y, swap ? ry : rx, swap ? rx : ry, (sh.angle * Math.PI) / 180, 0, Math.PI * 2);
      g.fill();
    } else {
      const gr = g.createRadialGradient(c.width / 2, c.height / 2, 0, c.width / 2, c.height / 2, c.width * 0.4);
      gr.addColorStop(0, "#fff");
      gr.addColorStop(1, "#000");
      g.fillStyle = gr;
      g.fillRect(0, 0, c.width, c.height);
    }
    g.restore();
    if (comp.inverted) {
      g.globalCompositeOperation = "difference";
      g.fillStyle = "#fff";
      g.fillRect(0, 0, c.width, c.height);
      g.globalCompositeOperation = "source-over";
    }
  }
  return c.toDataURL("image/png");
}

/** Deterministic 32-hex "digest" for mock AI mattes. */
function mockDigest(s: string): string {
  let h = 2166136261;
  let out = "";
  for (let round = 0; round < 4; round++) {
    for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619) >>> 0;
    h = Math.imul(h ^ round, 16777619) >>> 0;
    out += h.toString(16).padStart(8, "0");
  }
  return out.toUpperCase();
}

/** Mock AI status: components with a digest are ready, others need an update. */
function mockAiStatus(groups: MaskGroup[]): AiMaskStatus[] {
  return groups.flatMap((g) =>
    g.components.flatMap((c) =>
      c.shape.kind === "ai"
        ? [{ groupId: g.id, componentId: c.id, state: c.shape.digest ? ("ready" as const) : ("needs_update" as const), info: null }]
        : [],
    ),
  );
}

export interface MockCall {
  cmd: string;
  args: Record<string, unknown>;
}

declare global {
  interface Window {
    __ipcLog: MockCall[];
    /** Test hook: delay (ms) before a `render_preview` call with this per-slot sequence number resolves. */
    __mockRenderDelay?: (seq: number, slot: string) => number;
    /** Test hook (IPC v19.1): delay (ms) before the mock's edited preview of a changed photo is ready. */
    __mockEditedDelay?: number;
    /** Render concurrency observed by the mock (max renders awaiting a result at once). */
    __mockRenderStats?: { inflight: number; maxInflight: number };
    /** Test hook: when true, export jobs only advance through `__mockExportStep`. */
    __mockExportManual?: boolean;
    /** Advances the running mock export job by n files (default 1); finishes it when done. */
    __mockExportStep?: (n?: number) => void;
    /** v18: emits an arbitrary `activityEvent` (corner indicator tests). */
    __mockActivity?: (e: ActivityEvent) => void;
    /** Test hook: the mock auto-sync writer reports `running` in `get_xmp_status`. */
    __mockXmpRunning?: boolean;
    /** Test hook: emulates an auto-sync pass (writes every dirty photo it can, emits `xmp-synced` / `xmp-write-failed`). */
    __mockXmpFlush?: () => void;
    /** Test hook: ms per progress step of mock `detect_scenes` / `match_scene` (default 30). */
    __mockSceneDelay?: number;
    /** Test hook (v17): scene ids whose matching fails in apply (`file_missing` for one scene; reported in `skippedScenes` by apply all). */
    __mockApplyFailScenes?: number[];
    /** Makes get_edit_plan fail (plan error state). */
    __mockFailPlan?: boolean;
    /** Test hook: ms `compute_ai_mask` takes in the mock (default 250). */
    __mockAiDelay?: number;
    /** Test hook: ms per progress step of mock `download_models` (10 steps per file; default 40). */
    __mockModelDelay?: number;
    /** Folder the mock directory picker returns (default "/mock/export/Smith Wedding"). */
    __mockPickDir?: string | null;
    /** Test hook: when set, mock `download_models` fails with this error at the third file. */
    __mockModelFail?: string;
    /** Test hook: commands that reject with the given AppError (`{ cmd: { kind, message } }`); `once` entries are consumed. */
    __mockFail?: Record<string, { kind: string; message: string; once?: boolean }>;
    /** Test hook: `get_images` returns malformed entries (exercises the view error boundaries). */
    __mockCorruptImages?: boolean;
    /** Test hook: every export item fails with this reason (e.g. a disk-full message). */
    __mockExportFail?: string;
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

const MOCK_LOOKS: LookProfileInfo[] = [
  { uuid: "B952C231111CD8E0ECCF14B86BAA7077", name: "Adobe Color", group: "Adobe Raw", supportsAmount: false, monochrome: false, cameraProfile: "Adobe Standard", available: true, styleId: null },
  { uuid: "0CFE8F8AB5F63B2A73CE0B0077D20817", name: "Adobe Monochrome", group: "Adobe Raw", supportsAmount: false, monochrome: true, cameraProfile: "Adobe Standard", available: true, styleId: null },
  { uuid: "AAAA0000000000000000000000000001", name: "Vintage 01", group: "Vintage", supportsAmount: true, monochrome: false, cameraProfile: null, available: true, styleId: null },
  { uuid: "AAAA0000000000000000000000000002", name: "Vintage 02", group: "Vintage", supportsAmount: true, monochrome: false, cameraProfile: null, available: true, styleId: null },
  { uuid: "BBBB0000000000000000000000000001", name: "Modern 05", group: "Modern", supportsAmount: true, monochrome: false, cameraProfile: null, available: false, styleId: null },
];

const DEFAULT_PASTE_PREVIOUS: AdjustmentField[] = [
  "white_balance", "exposure", "contrast", "highlights", "shadows", "whites", "blacks", "texture", "clarity", "dehaze",
  "vibrance", "saturation", "hsl_hue", "hsl_saturation", "hsl_luminance", "lut", "tone_curve", "color_grading",
  "calibration", "sharpening", "noise_reduction", "vignette", "grain", "black_and_white", "crop", "profile", "process_version",
];

/** v18 `QualityScore.reasons` like the engine fills them: one per auto tag, plus a low score for
 *  reject suggestions without a tag. `keeperId` = the burst keeper (for `duplicate_burst`). */
function mockReasons(e: RawImageEntry, keeperId: number | null): SuggestionReason[] {
  const text: Record<CullTag, string> = {
    blink: "Eyes closed",
    missed_focus: "Missed focus on the face",
    motion_blur: "Motion blur",
    creative_blur: "Intentional blur (kept)",
    underexposed: "Too dark",
    overexposed: "Blown highlights on skin",
    duplicate_burst: "Duplicate in burst",
  };
  const out: SuggestionReason[] = e.tags.map((t) =>
    t.tag === "duplicate_burst" && keeperId != null && keeperId !== e.id
      ? { kind: "duplicate_burst", text: `Duplicate in burst (best DSC${String(keeperId).padStart(5, "0")})`, relatedImageId: keeperId }
      : { kind: t.tag, text: text[t.tag], relatedImageId: null },
  );
  out.sort((a, b) => Number(a.kind === "creative_blur") - Number(b.kind === "creative_blur")); // notes last, like the engine
  if (e.quality?.suggestedPick === "reject" && out.length === 0) out.push({ kind: "low_score", text: "Low overall quality", relatedImageId: null });
  return out;
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
      orientation: mockOrientation(id),
      fileSize: 30_000_000,
      fileMtimeMs: base,
      thumbnail: { status: "ready", path: `/mock/thumb/${id}.jpg`, previewPath: `/mock/preview/${id}.jpg`, width: 480, height: 320 },
      rating: 0,
      pick: "unflagged",
      colorLabel: null,
      burstGroupId: inBurst ? group : null,
      isBurstKeeper: inBurst && i % 25 === 1,
      sceneId: null,
      isSceneAnchor: false,
      // Every 9th frame has a camera JPEG sibling; every 10th carries Lightroom masks (unsupported).
      companionPath: id % 9 === 0 ? `/shoot/DSC${String(id).padStart(5, "0")}.JPG` : null,
      developWarnings: id % 10 === 0 ? [{ code: "masks_unsupported", detail: "2 mask groups" }] : [],
      missingSinceMs: null,
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
        reasons: [],
      },
      hasEdits: false,
      editedPreview: null,
      // v18: every 4th frame came with a sidecar.
      xmp: { dirty: false, syncedAtMs: null, error: null, hasSidecar: id % 4 === 0 },
      pickOrigin: null,
    };
    entry.quality!.reasons = mockReasons(entry, inBurst ? (group - 1) * 25 + 2 : null);
    rows.push(entry);
    if (inBurst) {
      const g = bursts.get(group) ?? { id: group, startedAtMs: entry.capture.capturedAtMs ?? 0, endedAtMs: 0, keeperImageId: null, imageIds: [] };
      g.imageIds.push(id);
      g.endedAtMs = entry.capture.capturedAtMs ?? 0;
      if (entry.isBurstKeeper) g.keeperImageId = id;
      bursts.set(group, g);
    }
  }
  // `?errors=1`: failure fixtures. id % 10 === 3 -> original missing (flagged like `?missing=N`, see below),
  // 7 -> thumbnail decode failure, 5 -> sidecar not writable.
  const errorsOn = new URLSearchParams(location.search).get("errors") === "1";
  const mockMissing = (id: number) => byId.get(id)?.missingSinceMs != null;
  const mockReadOnly = (id: number) => errorsOn && id % 10 === 5;
  const READ_ONLY = (id: number) => `Could not write /shoot/DSC${String(id).padStart(5, "0")}.xmp: the volume is read-only. Choose a writable location.`;
  if (errorsOn) {
    for (const r of rows) {
      if (r.id % 10 === 7) r.thumbnail = { status: "failed", reason: `Could not decode ${r.path}: unsupported RAW variant. The file may be damaged, still copying, or from an unsupported camera.` };
      if (mockReadOnly(r.id)) r.xmp = { ...r.xmp, dirty: true, syncedAtMs: null, error: READ_ONLY(r.id) };
      if (r.id % 10 === 3) r.missingSinceMs = base + 3_600_000;
    }
  }
  const params0 = new URLSearchParams(location.search);
  // A few pre-set flags so screenshots show something (v18: set by the user).
  for (let i = 0; i < count; i += 7) {
    rows[i].pick = i % 14 === 0 ? "pick" : "reject";
    rows[i].pickOrigin = "user";
  }
  // v18 `?meta=1`: varied file types / cameras / lenses for the metadata filter row (default data
  // stays all Sony ARW so existing suites are unaffected).
  if (new URLSearchParams(location.search).get("meta") === "1") {
    for (const r of rows) {
      const stem = r.fileName.replace(/\.[^.]+$/, "");
      if (r.id % 4 === 0) {
        Object.assign(r, { fileName: `${stem}.JPG`, path: r.path.replace(/\.[^.]+$/, ".JPG"), format: "jpeg" });
      } else if (r.id % 4 === 1) {
        Object.assign(r, { fileName: `${stem}.RAF`, path: r.path.replace(/\.[^.]+$/, ".RAF"), format: "raf" });
        r.camera = { make: "fujifilm", model: "X-T5", sensorLayout: "x_trans" };
        r.capture = { ...r.capture, lens: "XF33mmF1.4 R LM WR", focalLengthMm: 33, aperture: 1.4 };
      }
      if (r.id % 6 === 0) r.capture = { ...r.capture, lens: null };
    }
  }
  // v19: original EXIF time = corrected time until edited. `?twocams=1`: every 3rd frame is a Canon whose
  // clock ran 1 h ahead (Edit Capture Time > sync cameras fixture); default data stays one Sony body.
  for (const r of rows) {
    if (params0.get("twocams") === "1" && r.id % 3 === 0) {
      r.camera = { make: "canon", model: "EOS R5", sensorLayout: "bayer" };
      r.fileName = r.fileName.replace("DSC", "IMG_").replace(/\.ARW$/, ".CR3");
      r.path = `/shoot/${r.fileName}`;
      r.format = "cr3";
      r.capture = { ...r.capture, capturedAtMs: (r.capture.capturedAtMs ?? 0) + 3_600_000 };
    }
    r.capture = { ...r.capture, originalCapturedAtMs: r.capture.capturedAtMs, captureTimeSource: "exif" };
  }
  for (let i = 0; i < count; i += 5) rows[i].rating = (i / 5) % 6;
  for (let i = 0; i < count; i += 11) rows[i].colorLabel = LABELS[i % LABELS.length];
  for (let i = 3; i < count; i += 40) rows[i].xmp = { ...rows[i].xmp, dirty: true, syncedAtMs: null, error: null };
  void rand;

  // ---- Phase 8 hardening (v13): `?missing=N` flags images 1..N missing; `?health=read_only|replaced` ----
  const params = new URLSearchParams(location.search);
  const missingCount = Math.min(count, Math.max(0, Number(params.get("missing") ?? 0) || 0));
  for (let i = 0; i < missingCount; i++) rows[i].missingSinceMs = base + 3_600_000;
  const healthParam = params.get("health");
  const mockBackups: CatalogBackup[] = [1, 2, 3].map((index) => ({
    index,
    path: `/mock/catalog.sqlite.bak-${index}`,
    createdAtMs: base - index * 86_400_000,
    sizeBytes: 48_000_000 - index * 1_000_000,
  }));
  let health: CatalogHealth =
    healthParam === "read_only"
      ? {
          status: "read_only",
          message:
            "The catalog is damaged (database disk image is malformed) and was opened read-only, so changes cannot be saved. Quit Sieve and restore the backup /mock/catalog.sqlite.bak-1 (newest of 3), or copy it over /mock/catalog.sqlite.",
          backups: mockBackups,
          restorePending: false,
        }
      : healthParam === "replaced"
        ? {
            status: "replaced",
            message:
              "The catalog file was unreadable (file is not a database) and was moved to /mock/catalog.sqlite.corrupt-1; a new, empty catalog was created. Restore a backup to get your catalog back, or re-import your folders.",
            backups: mockBackups,
            restorePending: false,
          }
        : { status: "ok", message: null, backups: mockBackups, restorePending: false };
  const missingMessage = (r: RawImageEntry) =>
    `Original file is missing or was moved: ${r.path}. Reconnect the drive or move the file back, then try again.`;
  /** Like the backend: a damaged (read-only) catalog refuses writes. */
  const guardWrite = () => {
    if (health.status === "read_only") throw { kind: "catalog_read_only", message: health.message };
  };
  /** Like the backend: renders / develop info of a missing original fail with `file_missing`. */
  const guardOriginal = (id: number) => {
    const r = byId.get(id);
    if (r?.missingSinceMs != null) throw { kind: "file_missing", message: missingMessage(r) };
  };

  let catalog: CatalogState = {
    catalogPath: "/mock/catalog.sqlite",
    imageCount: count,
    shootType: "wedding",
    burstWindowMs: 1500,
    folders: [
      { id: 1, path: "/shoot/ceremony", imageCount: Math.floor(count / 2), projectId: 1 },
      { id: 2, path: "/shoot/reception", imageCount: count - Math.floor(count / 2), projectId: 2 },
    ],
    tagCounts: [],
    cacheDir: "/mock/cache",
    autoAnalyze: true,
    // Existing Playwright suites expect auto-sync off; `?autosync=1` gives the v14 default (on).
    xmpAutoSync: params.get("autosync") === "1",
    health,
    // Existing suites were written for the pre-v18 rule; `?keepers=not_rejected` gives the v18 default.
    keeperRule:
      params.get("keepers") === "not_rejected"
        ? { mode: "not_rejected", minRating: 1, useSuggestions: true }
        : { mode: "picks_and_ratings", minRating: 1, useSuggestions: true },
  };

  // ---- projects (v14): one project per mock folder; `?projects=0` starts with none ----
  interface MockProject {
    id: number;
    name: string;
    folderIds: number[];
    coverImageId: number | null;
    shootType: ShootType;
    workflowStep: WorkflowStep;
    createdAtMs: number;
    lastOpenedAtMs: number | null;
    /** v19 (`set_project_reject_strictness`); missing = balanced. */
    rejectStrictness?: RejectStrictness;
  }
  let projects: MockProject[] =
    params.get("projects") === "0"
      ? []
      : [
          { id: 1, name: "ceremony", folderIds: [1], coverImageId: null, shootType: "wedding", workflowStep: "cull", createdAtMs: base, lastOpenedAtMs: base + 86_400_000 },
          { id: 2, name: "reception", folderIds: [2], coverImageId: null, shootType: "wedding", workflowStep: "cull", createdAtMs: base + 3_600_000, lastOpenedAtMs: null },
        ];
  let projectSeq = projects.length;
  const projectOfFolder = (folderId: number) => projects.find((p) => p.folderIds.includes(folderId))?.id ?? null;
  const inScope = (r: RawImageEntry, folderId: number | null | undefined, projectId: number | null | undefined) =>
    (folderId == null || r.folderId === folderId) && (projectId == null || projectOfFolder(r.folderId) === projectId);
  const requireProject = (id: number) => {
    const p = projects.find((x) => x.id === id);
    if (!p) throw { kind: "not_found", message: `project ${id}` };
    return p;
  };
  const baseSuggest = new Map<number, PickFlag>();
  const keeper = (r: RawImageEntry) => params.get("nokeepers") !== "1" && isKeeperValues(catalog.keeperRule, r.pick, r.rating, r.quality?.suggestedPick);
  function projectDto(p: MockProject): Project {
    const photos = rows.filter((r) => p.folderIds.includes(r.folderId));
    const ranked = [...photos].sort(
      (a, b) =>
        Number(a.pick === "reject") - Number(b.pick === "reject") ||
        Number(b.pick === "pick") - Number(a.pick === "pick") ||
        b.rating - a.rating ||
        (a.capture.capturedAtMs ?? 0) - (b.capture.capturedAtMs ?? 0),
    );
    const cover = p.coverImageId ?? ranked[0]?.id ?? null;
    const coverRow = cover != null ? byId.get(cover) : undefined;
    const times = photos.map((r) => r.capture.capturedAtMs).filter((t): t is number => t != null);
    return {
      id: p.id,
      name: p.name,
      folders: catalog.folders
        .filter((f) => p.folderIds.includes(f.id))
        .map((f) => ({ id: f.id, path: f.path, imageCount: rows.filter((r) => r.folderId === f.id).length, exists: !f.path.startsWith("/missing") })),
      coverImageId: cover,
      coverChosen: p.coverImageId != null,
      coverThumbnailPath: coverRow?.thumbnail.status === "ready" ? coverRow.thumbnail.path : null,
      shootType: p.shootType,
      rejectStrictness: p.rejectStrictness ?? "balanced",
      workflowStep: p.workflowStep,
      createdAtMs: p.createdAtMs,
      lastOpenedAtMs: p.lastOpenedAtMs,
      photoCount: photos.length,
      keeperCount: photos.filter(keeper).length,
      editedCount: photos.filter((r) => r.hasEdits).length,
      pickedCount: photos.filter((r) => r.pick === "pick").length,
      rejectedCount: photos.filter((r) => r.pick === "reject").length,
      missingCount: photos.filter((r) => r.missingSinceMs != null).length,
      capturedFromMs: times.length ? Math.min(...times) : null,
      capturedToMs: times.length ? Math.max(...times) : null,
    };
  }
  const listProjects = () =>
    [...projects]
      .sort((a, b) => (b.lastOpenedAtMs ?? -1) - (a.lastOpenedAtMs ?? -1) || b.createdAtMs - a.createdAtMs || b.id - a.id)
      .map(projectDto);
  const validName = (n: string) => {
    const t = n.trim();
    if (!t || t.length > 200) throw { kind: "invalid_argument", message: "project name must not be empty" };
    return t;
  };

  // ---- style library, workflow, style model (v14) ----
  const styleGroups: StyleGroup[] = [];
  let styleSeq = 100;
  // `?style=ready` = a trained model, `?style=learnable` = untrained with enough edited photos, default = too few edits.
  const styleParam = params.get("style");
  let styleModel: StyleModelStatus = {
    state: styleParam === "ready" ? "ready" : "untrained",
    modelVersion: "style-mock@1",
    trainedAtMs: styleParam === "ready" ? Date.now() - 86_400_000 : null,
    trainingExamples: styleParam === "ready" ? 394 : 0,
    availableExamples: styleParam === "ready" || styleParam === "learnable" ? 394 : 0,
    minExamples: 20,
    progress: null,
    error: null,
    validation: null,
  };

  const byId = new Map(rows.map((r) => [r.id, r]));
  const visibleTags = (r: RawImageEntry) => r.tags.filter((t) => !t.suppressed).map((t) => t.tag);

  /** v18.1 `ImageQuery.pickOrigin` (mirror of Rust `repo::pick_origin_sql`): only flagged images match; no origin = the user's. */
  const originOk = (r: RawImageEntry, o: PickOrigin | null | undefined) =>
    o == null || (r.pick !== "unflagged" && (o === "auto" ? r.pickOrigin === "auto" : r.pickOrigin !== "auto"));

  function query(q: ImageQuery): number[] {
    let out = rows.filter((r) => {
      if (!inScope(r, q.folderId, q.projectId)) return false;
      const t = visibleTags(r);
      if (q.includeTags.length) {
        const ok = q.tagMatch === "all" ? q.includeTags.every((x) => t.includes(x)) : q.includeTags.some((x) => t.includes(x));
        if (!ok) return false;
      }
      if (q.excludeTags.some((x) => t.includes(x))) return false;
      if (q.picks.length && !q.picks.includes(r.pick)) return false;
      if (!originOk(r, q.pickOrigin)) return false;
      if (q.minRating != null && r.rating < q.minRating) return false;
      if (q.maxRating != null && r.rating > q.maxRating) return false;
      if (q.colorLabels.length && (!r.colorLabel || !q.colorLabels.includes(r.colorLabel))) return false;
      if (q.collapseBursts && r.burstGroupId != null && !r.isBurstKeeper) return false;
      if (q.sceneId != null && r.sceneId !== q.sceneId) return false;
      if (q.missingOnly && r.missingSinceMs == null) return false;
      if (q.keepersOnly && !keeper(r)) return false;
      if (!metaOk(r, q.metadata)) return false;
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

  // ---- v18 metadata filters (mirror of Rust `repo::metadata_clauses`) ----
  type Facet = keyof MetadataFilter;
  const extOf = (r: RawImageEntry) => (r.fileName.includes(".") ? r.fileName.slice(r.fileName.lastIndexOf(".") + 1).toLowerCase() : "");
  const blank = (s: string | null | undefined) => (s && s.trim() ? s.trim() : null);
  const round1 = (v: number | null | undefined) => (v == null ? null : Math.round(v * 10) / 10);
  const inRange = (v: number | null | undefined, range: NumberRange | null | undefined) => {
    if (!range) return true;
    if (v == null) return false;
    if (range.min != null && v < range.min - Math.abs(range.min) * 1e-6) return false;
    if (range.max != null && v > range.max + Math.abs(range.max) * 1e-6) return false;
    return true;
  };
  const DAY = 86_400_000;
  const dayOf = (ms: number | null | undefined) => (ms == null ? null : ms - (((ms % DAY) + DAY) % DAY));
  function metaOk(r: RawImageEntry, m: MetadataFilter | null | undefined, skip?: Facet): boolean {
    if (!m) return true;
    const on = (f: Facet) => skip !== f;
    if (on("formats") && m.formats?.length && !m.formats.includes(r.format)) return false;
    if (on("extensions") && m.extensions?.length) {
      for (const e of m.extensions) if (!/^[A-Za-z0-9]{1,10}$/.test(e)) throw { kind: "invalid_argument", message: `file extension "${e}" must be 1..=10 letters or digits` };
      if (!m.extensions.map((e) => e.toLowerCase()).includes(extOf(r))) return false;
    }
    if (on("cameras") && m.cameras?.length && !m.cameras.some((c) => c.make === r.camera.make && (blank(c.model) ?? null) === blank(r.camera.model))) return false;
    if (on("lenses") && m.lenses?.length && !m.lenses.some((l) => (l == null ? null : l.trim()) === blank(r.capture.lens))) return false;
    if (on("iso") && !inRange(r.capture.iso, m.iso)) return false;
    if (on("focalLengthMm") && !inRange(round1(r.capture.focalLengthMm), m.focalLengthMm)) return false;
    if (on("aperture") && !inRange(round1(r.capture.aperture), m.aperture)) return false;
    if (on("shutterSeconds") && !inRange(r.capture.shutterSeconds, m.shutterSeconds)) return false;
    if (on("captured") && m.captured) {
      const t = r.capture.capturedAtMs;
      if (t == null) return false;
      if (m.captured.fromMs != null && t < m.captured.fromMs) return false;
      if (m.captured.toMs != null && t >= m.captured.toMs) return false;
    }
    if (on("edited") && m.edited != null && r.hasEdits !== m.edited) return false;
    if (on("hasSidecar") && m.hasSidecar != null && r.xmp.hasSidecar !== m.hasSidecar) return false;
    return true;
  }
  function metadataOptions(q: ImageQuery): MetadataFilterOptions {
    // Each facet: the query without metadata, then every metadata constraint but the facet's own.
    const unfiltered = new Set(query({ ...q, metadata: {} }));
    const base = (skip: Facet) => rows.filter((r) => unfiltered.has(r.id) && metaOk(r, q.metadata, skip));
    const tally = <K,>(skip: Facet, key: (r: RawImageEntry) => K) => {
      const m = new Map<string, { key: K; count: number }>();
      for (const r of base(skip)) {
        const k = key(r);
        const s = JSON.stringify(k);
        const e = m.get(s);
        if (e) e.count++;
        else m.set(s, { key: k, count: 1 });
      }
      return [...m.values()];
    };
    const nullsLast = <T,>(a: T | null, b: T | null, cmp: (x: T, y: T) => number) => (a == null ? (b == null ? 0 : 1) : b == null ? -1 : cmp(a, b));
    const nums = (skip: Facet, key: (r: RawImageEntry) => number | null | undefined) =>
      tally(skip, (r) => key(r) ?? null)
        .map(({ key, count }) => ({ value: key, count }))
        .sort((a, b) => nullsLast(a.value, b.value, (x, y) => x - y));
    const yesNo = (skip: Facet, key: (r: RawImageEntry) => boolean) => {
      const t = tally(skip, key);
      return { yes: t.find((x) => x.key)?.count ?? 0, no: t.find((x) => !x.key)?.count ?? 0 };
    };
    const FORMATS: ImageFormat[] = ["arw", "raf", "cr3", "jpeg", "heic", "tiff", "png"];
    return {
      total: query(q).length,
      formats: tally("formats", (r) => r.format)
        .map(({ key, count }) => ({ format: key, count }))
        .sort((a, b) => FORMATS.indexOf(a.format) - FORMATS.indexOf(b.format)),
      extensions: tally("extensions", extOf)
        .map(({ key, count }) => ({ extension: key, count }))
        .sort((a, b) => (a.extension < b.extension ? -1 : 1)),
      cameras: tally("cameras", (r) => ({ make: r.camera.make, model: blank(r.camera.model) }))
        .map(({ key, count }) => ({ camera: key, count }))
        .sort((a, b) => Number(a.camera.model == null) - Number(b.camera.model == null) || a.camera.make.localeCompare(b.camera.make) || (a.camera.model ?? "").localeCompare(b.camera.model ?? "")),
      lenses: tally("lenses", (r) => blank(r.capture.lens))
        .map(({ key, count }) => ({ lens: key, count }))
        .sort((a, b) => nullsLast(a.lens, b.lens, (x, y) => x.localeCompare(y))),
      isos: nums("iso", (r) => r.capture.iso),
      focalLengths: nums("focalLengthMm", (r) => round1(r.capture.focalLengthMm)),
      apertures: nums("aperture", (r) => round1(r.capture.aperture)),
      shutterSpeeds: nums("shutterSeconds", (r) => r.capture.shutterSeconds),
      captureDays: tally("captured", (r) => dayOf(r.capture.capturedAtMs))
        .map(({ key, count }) => ({ dayStartMs: key, count }))
        .sort((a, b) => nullsLast(a.dayStartMs, b.dayStartMs, (x, y) => x - y)),
      edited: yesNo("edited", (r) => r.hasEdits),
      hasSidecar: yesNo("hasSidecar", (r) => r.xmp.hasSidecar),
    };
  }
  /** v18 `get_cull_summary` (mirror of Rust `repo::cull_summary`). */
  function cullSummary(projectId: number | null): CullSummary {
    if (projectId != null) requireProject(projectId);
    const scope = rows.filter((r) => inScope(r, null, projectId));
    const rule = catalog.keeperRule;
    const n = (f: (r: RawImageEntry) => boolean) => scope.filter(f).length;
    const unflaggedR = (r: RawImageEntry) => r.pick !== "pick" && r.pick !== "reject";
    const untouched = (r: RawImageEntry) => !!r.quality && r.pick === "unflagged" && r.rating === 0;
    const picked = n((r) => r.pick === "pick");
    const rejected = n((r) => r.pick === "reject");
    const rejectedAuto = n((r) => r.pick === "reject" && r.pickOrigin === "auto");
    const unflagged = scope.length - picked - rejected;
    const starredU = n((r) => unflaggedR(r) && r.rating >= rule.minRating);
    const suggestedU = n((r) => unflaggedR(r) && r.rating === 0 && r.quality?.suggestedPick === "pick");
    const notRejected = rule.mode === "not_rejected";
    return {
      total: scope.length,
      picked,
      pickedAuto: n((r) => r.pick === "pick" && r.pickOrigin === "auto"),
      unflagged,
      rejected,
      rejectedByUser: rejected - rejectedAuto,
      rejectedAuto,
      starred: n((r) => r.rating > 0),
      keepers: n(keeper),
      keeperBreakdown: notRejected
        ? { picked, unflagged, starred: 0, suggested: 0 }
        : { picked, unflagged: 0, starred: starredU, suggested: rule.useSuggestions ? suggestedU : 0 },
      keeperRule: rule,
      // v18.1: exactly what `apply_suggestions(onlyUnset)` changes (untouched = unflagged and 0 stars).
      suggestedRejectPending: n((r) => untouched(r) && r.quality?.suggestedPick === "reject"),
      suggestedPickPending: n((r) => untouched(r) && r.quality?.suggestedPick === "pick"),
      suggestedRatingPending: n((r) => untouched(r) && r.quality?.suggestedPick === "unflagged" && r.quality.suggestedRating > 0),
      unanalyzed: n((r) => !r.quality),
    };
  }
  /** v18 activity events: `window.__mockActivity(event)` emits any; the mock's own long work emits through `activity`. */
  let activitySeq = 0;
  const activity = (kind: ActivityKind, label: string, total: number | null) => {
    const id = ++activitySeq;
    const send = (done: number, state: ActivityState, message: string | null = null) => void emit("activity-event", { id, kind, label, done, total, state, message } satisfies ActivityEvent);
    send(0, "running");
    return { progress: (done: number) => send(done, "running"), end: (done: number, state: ActivityState, message: string | null) => send(done, state, message) };
  };
  window.__mockActivity = (e: ActivityEvent) => void emit("activity-event", e);
  const photos = (n: number) => `${n} photo${n === 1 ? "" : "s"}`;
  /** Rust `ipc::activity::xmp_message`. */
  const xmpMessage = (saved: number, failed: number) =>
    `Saved metadata for ${photos(saved)}` + (failed ? `; ${failed} sidecar${failed === 1 ? "" : "s"} could not be written` : "");

  function counts(folderId: number | null, projectId: number | null = null, keepersOnly = false, metadata: MetadataFilter | null = null, pickOrigin: PickOrigin | null = null): FilterCounts {
    if (projectId != null) requireProject(projectId);
    const scope = rows.filter((r) => inScope(r, folderId, projectId) && (!keepersOnly || keeper(r)) && metaOk(r, metadata) && originOk(r, pickOrigin));
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
      missing: scope.filter((r) => r.missingSinceMs != null).length,
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
    entries: (HistoryEntry & { snap: ParametricAdjustments; source: EditSource; batchId: number | null })[];
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
  // IPC v9: the backend always returns complete adjustments (every parity group present).
  const neutral = () => completeAdjustments(neutralAdjustments());
  const getAdj = (id: number): ParametricAdjustments => adjs.get(id) ?? neutral();
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
      appliedPresetId: appliedPresetOf(id),
    };
  };
  /** v19 (Rust `history::applied_preset`): the last applied preset while its fields still match the apply. */
  const appliedPresets = new Map<number, { presetId: number; snap: ParametricAdjustments }>();
  const appliedPresetOf = (id: number): number | null => {
    const a = appliedPresets.get(id);
    const p = a && presets.find((x) => x.id === a.presetId);
    if (!a || !p || p.fields.length === 0) return null;
    const cur = completeAdjustments(getAdj(id));
    return JSON.stringify(copyFields(cur, a.snap, p.fields)) === JSON.stringify(cur) ? p.id : null;
  };
  const isNeutral = (a: ParametricAdjustments) => JSON.stringify(completeAdjustments(a)) === JSON.stringify(neutral());
  /** IPC v15 `adjustment_history.source` mirror (Rust `history::source_for_label`). */
  const sourceForLabel = (label: string): EditSource => {
    if (label === "Original" || label === "Read from XMP") return "sidecar";
    if (label === "Apply to Scene" || label === "Match Scene") return "scene_apply";
    if (label === "Auto Edit (My Style)") return "auto_style";
    if (label === "Paste Settings" || label === "Sync Settings" || label === "Paste from Previous") return "pasted";
    return "user";
  };
  /**
   * IPC v19.1 edited previews: like the backend's background worker, renders the photo's edited preview after a
   * delay (`window.__mockEditedDelay`, default 120 ms) unless the settings changed meanwhile, then emits
   * `editedPreviewChanged`. URLs are content-addressed: `/mock/edited/<id>/<hash>/{thumb,preview}.jpg`.
   */
  function renderEdited(id: number) {
    const key = JSON.stringify(adjs.get(id) ?? null);
    let h = 0x811c9dc5;
    for (let i = 0; i < key.length; i++) h = Math.imul(h ^ key.charCodeAt(i), 0x01000193) >>> 0;
    const hash = h.toString(16).padStart(8, "0").repeat(2);
    setTimeout(() => {
      const r = byId.get(id);
      if (!r || JSON.stringify(adjs.get(id) ?? null) !== key) return;
      const preview = r.hasEdits ? { thumbUrl: `/mock/edited/${id}/${hash}/thumb.jpg`, previewUrl: `/mock/edited/${id}/${hash}/preview.jpg` } : null;
      if (JSON.stringify(preview) === JSON.stringify(r.editedPreview ?? null)) return;
      r.editedPreview = preview;
      void emit("edited-preview-changed", { imageId: id, preview });
    }, window.__mockEditedDelay ?? 120);
  }
  function commit(id: number, next0: ParametricAdjustments, label: string, coalesce = true) {
    const next = completeAdjustments(next0);
    const cur = getAdj(id);
    const h = histOf(id);
    if (JSON.stringify(cur) === JSON.stringify(next)) return;
    if (h.cursor < 0) {
      h.entries.push({ id: ++entryId, label: "Original", createdAtMs: Date.now(), snap: cur, source: "sidecar", batchId: null });
      h.cursor = 0;
    }
    const now = Date.now();
    h.entries.length = h.cursor + 1;
    const last = h.entries[h.cursor];
    if (coalesce && h.cursor > 0 && last.label === label && now - h.lastAt < 1500) {
      last.snap = next;
    } else {
      h.entries.push({ id: ++entryId, label, createdAtMs: now, snap: next, source: sourceForLabel(label), batchId: null });
      h.cursor++;
    }
    h.lastAt = now;
    adjs.set(id, next);
    const r = byId.get(id);
    if (r) r.hasEdits = !isNeutral(next);
    renderEdited(id);
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
    renderEdited(id);
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
    const stats = (window.__mockRenderStats ??= { inflight: 0, maxInflight: 0 });
    stats.inflight++;
    stats.maxInflight = Math.max(stats.maxInflight, stats.inflight);
    if (delay > 0) await new Promise((r) => setTimeout(r, delay));
    stats.inflight--;
    // Like the real backend: only the newest render per (image, slot) is kept; superseded ones resolve to null.
    if (seqs.get(key) !== seq) return null;
    const q = new URLSearchParams({ v: String(seq), e: a.exposure.toFixed(2), lut: a.lut?.id ?? "", region: o.region ? "1" : "" });
    return {
      imageId: id,
      slot: o.slot,
      seq,
      url: `/mock/render/${id}/${o.slot}?${q}${mockOrientation(id) >= 5 ? "&p=1" : ""}`,
      // Portrait frames (EXIF 8) are 2:3; the long edge is `maxEdge` either way.
      width: mockOrientation(id) >= 5 ? Math.round((o.maxEdge * 2) / 3) : o.maxEdge,
      // A crop changes the frame's aspect (mock frames are 3:2).
      height: mockOrientation(id) >= 5 ? o.maxEdge : Math.round(o.maxEdge * (o.region ? 1 : a.crop?.enabled ? (a.crop.bottom - a.crop.top) / (1.5 * (a.crop.right - a.crop.left)) : 2 / 3)),
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
    /** v18 activity of the job (started with its first progress). */
    act?: ReturnType<typeof activity>;
  }
  let activeRun: Run | null = null;
  const allExportPresets = () => [...BUILTIN_EXPORT_PRESETS, ...exportPresets];
  function stepRun(run: Run, n: number) {
    for (let k = 0; k < n && run.job.done < run.job.total && !run.cancelled; k++) {
      const id = run.ids[run.job.done];
      const r = byId.get(id);
      run.job.done++;
      if (window.__mockExportFail || id % 7 === 0) {
        run.job.failed++;
        run.job.failures.push({ imageId: id, fileName: r?.fileName ?? String(id), reason: window.__mockExportFail ?? "Decode error (mock)" });
      } else run.job.succeeded++;
      run.act ??= activity("export", `Exporting ${photos(run.job.total)}`, run.job.total);
      run.act.progress(run.job.done);
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
    run.act?.end(
      run.job.done,
      run.cancelled ? "cancelled" : "finished",
      `Exported ${photos(run.job.succeeded)}` + (failed.length ? `; ${failed.length} failed` : ""),
    );
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
  window.__mockXmpFlush = () => {
    const written: number[] = [];
    const dirty = rows.filter((r) => r.xmp.dirty);
    const a = dirty.length ? activity("xmp_save", "Saving metadata to XMP", dirty.length) : null;
    for (const r of dirty) {
      if (mockReadOnly(r.id) || mockMissing(r.id)) {
        const reason = mockMissing(r.id) ? missingMessage(r) : READ_ONLY(r.id);
        r.xmp = { ...r.xmp, dirty: true, syncedAtMs: null, error: reason };
        void emit("xmp-write-failed", { imageId: r.id, reason });
      } else {
        r.xmp = { dirty: false, syncedAtMs: Date.now(), error: null, hasSidecar: true };
        written.push(r.id);
      }
    }
    a?.end(dirty.length, "finished", xmpMessage(written.length, dirty.length - written.length));
    void emit("xmp-synced", { written, read: [] });
  };



  // ---- edit plan / apply-to-scene / style batches (v14) ----
  const sceneRep = new Map<number, number>();
  // v16: `repSnap` = the representative's settings the apply was made from (Rust
  // `applied_params_json`), `batchId` = the scene's last apply batch that changed something.
  const sceneApplied = new Map<number, { at: number; snaps: Map<number, string>; repSnap: string; batchId: number | null }>();
  interface MockBatchItem {
    id: number;
    before: string;
    after: string;
    beforeSource: EditSource;
    beforeBatch: number | null;
    sceneId: number | null;
    reviewReason: string | null;
    reviewed: boolean;
  }
  // v17: `bases` = representatives the batch (an apply) was made from, with the batch that wrote their settings.
  const batches = new Map<
    number,
    { label: string; items: MockBatchItem[]; undone: boolean; undoneAt: number | null; createdAt: number; sceneIds: number[]; bases: { imageId: number; base: number }[] }
  >();
  let batchSeq = 0;
  // IPC v15: skipped scenes, frames the last apply covered, apply cancel flag.
  const sceneSkipped = new Set<number>();
  const sceneCovered = new Map<number, number[]>();
  let applyCancel = false;
  const cursorEntry = (id: number) => {
    const h = hists.get(id);
    return h && h.cursor >= 0 ? h.entries[h.cursor] : undefined;
  };
  /** Per-photo state (Rust `batches::edit_states`): derived from the entry the history cursor points at. */
  function editState(id: number): ImageEditState {
    const none: ImageEditState = { imageId: id, editSource: "none", batchId: null, appliedSceneId: null, needsReview: false, reviewReason: null };
    if (isNeutral(getAdj(id))) return none;
    const e = cursorEntry(id);
    if (!e) return { ...none, editSource: "sidecar" };
    const item = e.batchId != null ? batches.get(e.batchId)?.items.find((i) => i.id === id) : undefined;
    const needs = e.source === "scene_apply" && e.batchId != null && !!item?.reviewReason && !item.reviewed;
    return {
      imageId: id,
      editSource: e.source,
      batchId: e.batchId,
      appliedSceneId: e.source === "scene_apply" ? (item?.sceneId ?? null) : null,
      needsReview: needs,
      reviewReason: needs ? (item?.reviewReason ?? null) : null,
    };
  }
  /** Commits `next` for a batch and returns its item (null when nothing changed). */
  function batchCommit(id: number, next: ParametricAdjustments, label: string, sceneId: number | null, reviewReason: string | null): MockBatchItem | null {
    const before = JSON.stringify(getAdj(id));
    const prev = cursorEntry(id);
    commit(id, next, label, false);
    const after = JSON.stringify(getAdj(id));
    if (before === after) return null;
    return { id, before, after, beforeSource: prev?.source ?? "sidecar", beforeBatch: prev?.batchId ?? null, sceneId, reviewReason, reviewed: false };
  }
  function recordItems(label: string, items: MockBatchItem[], sceneIds: number[]): number | null {
    if (items.length === 0) return null;
    const batchId = ++batchSeq;
    batches.set(batchId, { label, items, undone: false, undoneAt: null, createdAt: Date.now(), sceneIds, bases: [] });
    for (const it of items) {
      const e = cursorEntry(it.id);
      if (e) e.batchId = batchId;
    }
    return batchId;
  }
  /** v17 (Rust `batches::dependent_ids`): images of the batch a later apply (not undone) was made from. */
  function batchDependents(batchId: number): number[] {
    const b = batches.get(batchId);
    if (!b) return [];
    const used = new Set<number>();
    for (const o of batches.values()) if (!o.undone) for (const x of o.bases) if (x.base === batchId) used.add(x.imageId);
    return b.items.map((i) => i.id).filter((id) => used.has(id));
  }
  /** v17 (Rust `batches::conflict_ids`): edited after the batch, or a later apply was made from it. */
  function batchConflicts(batchId: number): number[] {
    const b = batches.get(batchId);
    if (!b) return [];
    const edited = new Set(batchEditedAfter(batchId));
    const deps = new Set(batchDependents(batchId));
    return b.items.map((i) => i.id).filter((id) => edited.has(id) || deps.has(id));
  }
  /** v16 (Rust `batches::edited_after_ids`): images of the batch edited after it. */
  function batchEditedAfter(batchId: number): number[] {
    const b = batches.get(batchId);
    if (!b) return [];
    const out: number[] = [];
    for (const it of b.items) {
      const h = hists.get(it.id);
      if (!h || h.cursor < 0) continue;
      const cur = h.entries[h.cursor];
      if (cur.batchId === batchId) continue;
      const own = h.entries.find((e) => e.batchId === batchId);
      const conflict = own ? cur.id > own.id : JSON.stringify(getAdj(it.id)) !== it.after;
      if (conflict) out.push(it.id);
    }
    return out;
  }
  function batchInfo(batchId: number): EditBatchInfo {
    const b = batches.get(batchId);
    if (!b) throw { kind: "not_found", message: `edit batch ${batchId}` };
    const conflictCount = b.undone ? 0 : batchConflicts(batchId).length;
    return {
      batchId,
      label: b.label,
      kind:
        b.label === "Auto Edit (My Style)"
          ? "style_prediction"
          : b.label === "Paste Settings" || b.label === "Sync Settings" || b.label === "Paste from Previous"
            ? "paste"
            : "scene_apply",
      createdAtMs: b.createdAt,
      undoneAtMs: b.undoneAt,
      imageCount: b.items.length,
      conflictCount,
      undoable: !b.undone && conflictCount === 0,
    };
  }
  /** Newest batch not undone that changed one of `ids` (Rust `batches::latest_batch_where`). */
  function latestBatch(ids: Set<number>): EditBatchInfo | null {
    const found = [...batches.entries()].filter(([, b]) => !b.undone && b.items.some((i) => ids.has(i.id))).map(([id]) => id);
    return found.length ? batchInfo(Math.max(...found)) : null;
  }
  const STYLE_FIELDS: AdjustmentField[] = ["exposure", "contrast", "highlights", "shadows", "vibrance"];
  const styleAdj = (a: ParametricAdjustments): ParametricAdjustments => ({ ...a, exposure: 0.3, contrast: 10, highlights: -25, shadows: 20, vibrance: 12 });
  const repOf = (scKeepers: RawImageEntry[], sceneId: number) => {
    const chosen = sceneRep.get(sceneId);
    const picked = chosen != null ? scKeepers.find((r) => r.id === chosen) : undefined;
    return { rep: picked ?? [...scKeepers].sort((a, b) => (b.quality?.overall ?? 0) - (a.quality?.overall ?? 0))[0], user: !!picked };
  };
  const lastEditAt = (id: number) => {
    const h = hists.get(id);
    return h && h.cursor > 0 ? h.entries[h.cursor].createdAtMs : null;
  };
  function planEntries(keepers: RawImageEntry[]): SceneEditEntry[] {
    const out: SceneEditEntry[] = [];
    for (const sc of scenes) {
      const ks = keepers.filter((r) => r.sceneId === sc.id);
      if (!ks.length) continue;
      const { rep, user } = repOf(ks, sc.id);
      const applied = sceneApplied.get(sc.id);
      const editedAt = rep.hasEdits ? lastEditAt(rep.id) : null;
      // Rust: applied while the representative's settings equal those the apply was made from.
      // v17: applied, then the representative went back to no edits -> "reset" (a to-do scene).
      const status: SceneEditEntry["status"] = applied
        ? !rep.hasEdits
          ? "reset"
          : JSON.stringify(getAdj(rep.id)) === applied.repSnap
            ? "applied"
            : "outdated"
        : rep.hasEdits
          ? "edited"
          : "to_edit";
      const appliedBatch = applied?.batchId != null ? batchInfo(applied.batchId) : null;
      const skipped = sceneSkipped.has(sc.id);
      const covered = sceneCovered.get(sc.id) ?? [];
      const src = (id: number) => editState(id).editSource;
      out.push({
        skipped,
        minor: ks.length <= MINOR_SCENE_MAX_KEEPERS,
        autoEdited: rep.hasEdits && src(rep.id) === "auto_style",
        appliedIds: sc.imageIds.filter((id) => id !== rep.id && src(id) === "scene_apply"),
        needsReviewIds: ks.filter((r) => editState(r.id).needsReview).map((r) => r.id),
        unappliedKeeperIds:
          applied && !skipped && status !== "reset" ? ks.filter((r) => r.id !== rep.id && !covered.includes(r.id) && (src(r.id) === "none" || src(r.id) === "auto_style")).map((r) => r.id) : [],
        sceneId: sc.id,
        imageIds: ks.map((r) => r.id),
        memberCount: sc.imageIds.length,
        representativeId: rep.id,
        representativeSource: user ? "user" : "auto",
        representativeReason: user ? "Chosen by you" : "Best-scored keeper of this scene",
        edited: rep.hasEdits,
        editedAtMs: editedAt,
        appliedAtMs: applied?.at ?? null,
        status,
        appliedBatch: appliedBatch && appliedBatch.undoneAtMs == null ? appliedBatch : null,
      });
    }
    return out;
  }
  /** Commits `fn` on every image as one undoable batch (only images that change are recorded). */
  function recordBatch(label: string, ids: number[], fn: (a: ParametricAdjustments) => ParametricAdjustments, sceneIds: number[] = []): EditBatchResult {
    const items: MockBatchItem[] = [];
    for (const id of ids) {
      const it = batchCommit(id, fn(getAdj(id)), label, null, null);
      if (it) items.push(it);
    }
    const batchId = recordItems(label, items, sceneIds);
    return { batchId, label, changedIds: items.map((i) => i.id) };
  }
  /** v17 (Rust `workflow::scene_label`): "Scene N" by plan position, "This scene" outside a plan. */
  function sceneLabel(sid: number): string {
    const sc = sceneOf(sid);
    const first = sc.imageIds.map((i) => byId.get(i)).find((r) => r != null);
    const pid = first ? projectOfFolder(first.folderId) : null;
    if (pid == null) return "This scene";
    const i = planEntries(rows.filter((r) => inScope(r, null, pid) && keeper(r))).findIndex((e) => e.sceneId === sid);
    return i >= 0 ? `Scene ${i + 1}` : "This scene";
  }
  /**
   * Rust `apply_scenes`. `lenient` (apply all, v17): scenes that cannot be applied are reported in
   * `skippedScenes` instead of failing; errors name the scene ("Scene 1: ...").
   */
  async function applyScenes(sceneIds: number[], o: SceneApplyOptions, label: string, lenient = false): Promise<ApplyScenesResult> {
    applyCancel = false;
    const exclude = new Set(o.excludeIds ?? []);
    const items: MockBatchItem[] = [];
    const outcomes: SceneApplyOutcome[] = [];
    const skippedScenes: SkippedScene[] = [];
    const bases: { imageId: number; base: number; forIds: number[] }[] = [];
    const plans = sceneIds.flatMap((sid) => {
      const sc = sceneOf(sid);
      const name = sceneLabel(sid);
      const ks = rows.filter((r) => r.sceneId === sid && (o.includeNonKeepers || keeper(r)));
      const { rep } = repOf(ks.filter(keeper), sid);
      const cannot = (reason: SkippedScene["reason"], message: string) => {
        if (!lenient) throw { kind: "invalid_argument", message };
        skippedScenes.push({ sceneId: sid, reason, message });
        return [];
      };
      if (!rep) return cannot("no_keepers", `${name} has no keepers.`);
      if (!rep.hasEdits)
        return cannot(
          "not_edited",
          sceneApplied.has(sid) ? `${name}: its representative was reset after the last apply. Edit it first, then apply.` : `${name}: edit its representative first, then apply.`,
        );
      const e = cursorEntry(rep.id);
      const base = e?.batchId != null && batches.get(e.batchId)?.undone === false ? e.batchId : null;
      return [{ sc, rep, name, base, prev: sceneApplied.get(sid), targets: ks.filter((r) => r.id !== rep.id) }];
    });
    const total = plans.reduce((n, p) => n + p.targets.length, 0);
    let done = 0;
    let cancelled = false;
    for (const { sc, rep, name, base, prev, targets } of plans) {
      // One progress step per scene; `cancel_scene_apply` takes effect between scenes.
      await sleep(window.__mockSceneDelay ?? 30);
      if (applyCancel) {
        cancelled = true;
        break;
      }
      if (targets.length && window.__mockApplyFailScenes?.includes(sc.id)) {
        const message = `${name}: ${rep.fileName} is missing`;
        if (!lenient) throw { kind: "file_missing", message };
        skippedScenes.push({ sceneId: sc.id, reason: "failed", message });
        done += targets.length;
        continue;
      }
      const changedIds: number[] = [];
      const skippedIds: number[] = [];
      const excludedIds: number[] = [];
      const notConvergedIds: number[] = [];
      const snaps = new Map<number, string>();
      for (const t of targets) {
        if (exclude.has(t.id)) {
          excludedIds.push(t.id);
          continue;
        }
        const cur = JSON.stringify(getAdj(t.id));
        if (o.skipUserEdited && prev?.snaps.has(t.id) && prev.snaps.get(t.id) !== cur) {
          skippedIds.push(t.id);
          continue;
        }
        const pv = solveMock([rep.id], t.id, o.matchOptions);
        if (!pv.converged) notConvergedIds.push(t.id);
        const reason = pv.converged ? null : (pv.notes[0] ?? "Exposure or white balance did not fully match the representative");
        const it = batchCommit(t.id, pv.adjustments, label, sc.id, reason);
        snaps.set(t.id, JSON.stringify(getAdj(t.id)));
        if (it) {
          changedIds.push(t.id);
          items.push(it);
        }
      }
      done += targets.length;
      void emit("scene-progress", { task: "apply", done, total });
      sceneApplied.set(sc.id, { at: Date.now(), snaps, repSnap: JSON.stringify(getAdj(rep.id)), batchId: prev?.batchId ?? null });
      if (base != null) bases.push({ imageId: rep.id, base, forIds: changedIds });
      sceneCovered.set(sc.id, [rep.id, ...targets.map((t) => t.id)]);
      sceneSkipped.delete(sc.id);
      outcomes.push({ sceneId: sc.id, representativeId: rep.id, changedIds, skippedIds, excludedIds, notConvergedIds, notes: [] });
    }
    const batchId = recordItems(label, items, outcomes.map((x) => x.sceneId));
    // Rust keeps the previous batch id when the apply changed nothing.
    if (batchId != null) outcomes.forEach((x) => { const a = sceneApplied.get(x.sceneId); if (a) a.batchId = batchId; });
    // v17: what the apply was built on (Rust `edit_batch_bases`), when it changed a frame of that scene.
    if (batchId != null) batches.get(batchId)!.bases = bases.filter((x) => x.forIds.length > 0).map(({ imageId, base }) => ({ imageId, base }));
    return { batch: { batchId, label, changedIds: items.map((i) => i.id) }, scenes: outcomes, cancelled, skippedScenes };
  }

  // ---- scenes (v7) emulation ----
  let scenes: Scene[] = [];
  let sceneId = 0;
  const sceneOrder = (imageIds: number[]) =>
    [...imageIds].sort((a, b) => (byId.get(a)?.capture.capturedAtMs ?? 0) - (byId.get(b)?.capture.capturedAtMs ?? 0) || a - b);
  function sceneBounds(sc: Scene) {
    const ts = sc.imageIds.map((i) => byId.get(i)?.capture.capturedAtMs).filter((t): t is number => t != null);
    sc.startedAtMs = ts.length ? Math.min(...ts) : null;
    sc.endedAtMs = ts.length ? Math.max(...ts) : null;
    const f = new Set(sc.imageIds.map((i) => byId.get(i)?.folderId));
    sc.folderId = f.size === 1 ? ((byId.get(sc.imageIds[0])?.folderId as number) ?? null) : null;
    sc.updatedAtMs = Date.now();
  }
  /** Re-derives images.scene_id / scene_anchor from the scene list (and drops emptied scenes). */
  function syncScenes() {
    scenes = scenes.filter((sc) => sc.imageIds.length > 0);
    rows.forEach((r) => {
      r.sceneId = null;
      r.isSceneAnchor = false;
    });
    scenes.forEach((sc) => {
      sc.imageIds = sceneOrder(sc.imageIds);
      sc.anchorIds = sceneOrder(sc.anchorIds.filter((a) => sc.imageIds.includes(a))).slice(0, 2);
      sceneBounds(sc);
      sc.imageIds.forEach((i) => {
        const r = byId.get(i)!;
        r.sceneId = sc.id;
        r.isSceneAnchor = sc.anchorIds.includes(i);
      });
    });
    scenes.sort((a, b) => (a.startedAtMs ?? 0) - (b.startedAtMs ?? 0) || a.id - b.id);
  }
  const sceneOf = (id: number) => {
    const sc = scenes.find((x) => x.id === id);
    if (!sc) throw { kind: "not_found", message: `scene ${id}` };
    return sc;
  };
  const newScene = (imageIds: number[], method: Scene["method"]): Scene => {
    const now = Date.now();
    const sc: Scene = { id: ++sceneId, folderId: null, startedAtMs: null, endedAtMs: null, imageIds, anchorIds: [], method, createdAtMs: now, updatedAtMs: now };
    scenes.push(sc);
    return sc;
  };
  const takeFromScenes = (imageIds: number[]) =>
    scenes.forEach((sc) => {
      sc.imageIds = sc.imageIds.filter((i) => !imageIds.includes(i));
    });
  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
  async function progress(task: "detect" | "match" | "apply", total: number) {
    const steps = 4;
    for (let k = 1; k <= steps; k++) {
      await sleep(window.__mockSceneDelay ?? 30);
      void emit("scene-progress", { task, done: Math.round((total * k) / steps), total });
    }
  }
  const mockStats = (imageId: number, luma: number, b: number): ImageStats => ({
    imageId,
    region: null,
    width: 640,
    height: 427,
    meanLuma: luma,
    logMeanLuma: Math.log2(luma),
    percentiles: { p1: 0.02, p10: 0.1, p50: luma, p90: 0.8, p99: 0.95 },
    clippedHighlights: 0,
    clippedShadows: 0,
    meanOklab: { l: luma, a: 0, b },
    neutral: { x: 0.5, y: 0.5, a: 0, b, coverage: 0.2 },
    whiteBalance: null,
    asShot: null,
    lutMissing: false,
  });
  /** Deterministic per-target correction so tests can assert exact values. */
  function solveMock(anchors: number[], target: number, o: MatchOptions): MatchPreview {
    const src = getAdj(anchors[0]);
    const cur = getAdj(target);
    const base = copyFields(cur, src, o.copyFields);
    const full = structuredClone(base);
    const dEv = o.matchExposure ? ((target % 5) - 2) * 0.25 + 0.1 : 0;
    const dT = o.matchWhiteBalance ? ((target % 3) - 1) * 300 + 150 : 0;
    const dTint = o.matchWhiteBalance ? (target % 4) - 1.5 : 0;
    const dCon = o.matchTone ? 6 : 0;
    full.exposure = base.exposure + dEv;
    if (o.matchWhiteBalance) {
      const w = base.whiteBalance;
      const t0 = w.mode === "custom" ? w.temperatureK : 5200;
      const tn0 = w.mode === "custom" ? w.tint : 8;
      base.whiteBalance = { mode: "custom", temperatureK: t0, tint: tn0 };
      full.whiteBalance = { mode: "custom", temperatureK: t0 + dT, tint: tn0 + dTint };
    }
    full.contrast = base.contrast + dCon;
    const adjustments = lerpAdjustments(base, full, o.strength);
    const ref = mockStats(anchors[0], 0.5, 0.01);
    const converged = target % 7 !== 0;
    const blended = anchors.length > 1 && target % 2 === 0;
    return {
      targetId: target,
      anchorIds: blended ? anchors : [anchors[0]],
      anchorWeight: blended ? 0.5 : 0,
      base,
      full,
      adjustments,
      delta: { exposure: dEv, temperatureK: dT, tint: dTint, contrast: dCon, whites: 0, blacks: 0 },
      reference: ref,
      before: mockStats(target, 0.5 * 2 ** -dEv, 0.01 + dTint / 100),
      predicted: mockStats(target, 0.5, 0.01),
      converged,
      notes: converged ? [] : ["Exposure correction clamped at +5 EV (mock)"],
    };
  }

  const ok = { succeeded: 0, skipped: 0, failed: [], changed: [] };
  let uiPrefs: UiPrefs = {};

  // ---- model downloads (v12): simulated progress, honours cancel ----
  mockModelsInstalled = new URLSearchParams(location.search).get("models") !== "missing";
  let modelRun: { cancelled: boolean } | null = null;
  function modelStatus(): ModelDownloadStatus {
    const files = MOCK_SEGMENTATION_FILES.map(([name, bytes]) => ({ name, installed: mockModelsInstalled, bytes }));
    return {
      groups: [{ id: "segmentation", label: "AI masking models", installed: mockModelsInstalled, bytesTotal: files.reduce((s, f) => s + f.bytes, 0), files }],
      downloading: modelRun ? "segmentation" : null,
    };
  }
  async function runModelDownload(run: { cancelled: boolean }) {
    const group = "segmentation";
    const total = MOCK_SEGMENTATION_FILES.reduce((s, [, b]) => s + b, 0);
    let before = 0;
    let error: string | null = null;
    outer: for (const [i, [name, bytes]] of MOCK_SEGMENTATION_FILES.entries()) {
      for (let k = 1; k <= 10; k++) {
        await sleep(window.__mockModelDelay ?? 40);
        if (run.cancelled) {
          error = "model download cancelled";
          break outer;
        }
        if (i === 2 && k === 10 && window.__mockModelFail) {
          error = window.__mockModelFail;
          break outer;
        }
        const payload: ModelDownloadProgress = { group, name, fileIndex: i, fileCount: MOCK_SEGMENTATION_FILES.length, bytesDone: before + Math.round((bytes * k) / 10), bytesTotal: total };
        void emit("model-download-progress", payload);
      }
      before += bytes;
    }
    modelRun = null;
    if (!error) mockModelsInstalled = true;
    const finished: ModelDownloadFinished = { group, ok: !error, cancelled: run.cancelled, error };
    void emit("model-download-finished", finished);
  }

  mockIPC(
    (cmd, payload) => {
      const args = (payload ?? {}) as Record<string, unknown>;
      window.__ipcLog.push({ cmd, args });
      const injected = window.__mockFail?.[cmd];
      if (injected) {
        if (injected.once) delete window.__mockFail![cmd];
        throw { kind: injected.kind, message: injected.message };
      }
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
          // Test hook for the error boundary: entries the grid cannot render.
          if (window.__mockCorruptImages) return ids.map((i) => ({ ...byId.get(i), tags: null }));
          return ids.map((i) => byId.get(i));
        case "get_image":
          return byId.get(args.id as number);
        case "get_filter_counts":
          return counts(
            args.folderId as number | null,
            (args.projectId as number | null) ?? null,
            (args.keepersOnly as boolean | null) ?? false,
            (args.metadata as MetadataFilter | null) ?? null,
            (args.pickOrigin as PickOrigin | null) ?? null,
          );
        // ---- IPC v18 ----
        case "get_cull_summary":
          return cullSummary((args.projectId as number | null) ?? null);
        case "get_metadata_filter_options":
          return metadataOptions(args.query as ImageQuery);
        case "refresh_sidecars":
          // No other app edits mock sidecars.
          if (args.projectId != null) requireProject(args.projectId as number);
          return [];
        case "get_import_status":
          return { total: count, pending: 0, ready: count, failed: 0, running: false };
        case "get_analysis_status":
          return { total: count, analyzed: count, failed: 0, pending: 0, waiting: 0, running: false };
        case "get_xmp_status":
          return { dirty: rows.filter((r) => r.xmp.dirty).length, failed: rows.filter((r) => r.xmp.error).length, running: !!window.__mockXmpRunning, autoSync: catalog.xmpAutoSync };
        case "set_pick":
          guardWrite();
          ids.forEach((i) => {
            const r = byId.get(i);
            if (r) {
              r.pick = args.pick as PickFlag;
              r.pickOrigin = r.pick === "unflagged" ? null : "user";
            }
          });
          return null;
        case "set_rating":
          guardWrite();
          ids.forEach((i) => {
            const r = byId.get(i);
            if (r) r.rating = args.rating as number;
          });
          return null;
        case "set_color_label":
          guardWrite();
          ids.forEach((i) => {
            const r = byId.get(i);
            if (r) r.colorLabel = args.label as RawImageEntry["colorLabel"];
          });
          return null;
        case "get_faces":
          return faces(args.id as number);
        case "list_burst_groups": {
          const fid = (args.folderId as number | null) ?? null;
          const pid = (args.projectId as number | null) ?? null;
          return [...bursts.values()].filter((g) => g.imageIds.some((i) => inScope(byId.get(i)!, fid, pid)));
        }
        case "write_xmp": {
          const bad = ids.filter((i) => mockReadOnly(i) || mockMissing(i));
          ids.forEach((i) => {
            const r = byId.get(i);
            if (r && !bad.includes(i)) r.xmp = { dirty: false, syncedAtMs: Date.now(), error: null, hasSidecar: true };
          });
          if (ids.length) {
            const a = activity("xmp_save", "Saving metadata to XMP", ids.length);
            a.end(ids.length, "finished", xmpMessage(ids.length - bad.length, bad.length));
          }
          return { ...ok, succeeded: ids.length - bad.length, failed: bad.map((i) => ({ imageId: i, reason: mockMissing(i) ? missingMessage(byId.get(i)!) : READ_ONLY(i) })), changed: [] };
        }
        case "read_xmp":
          return { ...ok, skipped: ids.length };
        case "apply_suggestions": {
          let applied = 0;
          ids.forEach((i) => {
            const r = byId.get(i);
            if (!r?.quality) return;
            if (args.onlyUnset && (r.pick !== "unflagged" || r.rating !== 0)) return;
            // v18.1: only real changes count as applied.
            if (r.pick === r.quality.suggestedPick && r.rating === r.quality.suggestedRating) return;
            r.rating = r.quality.suggestedRating;
            // v18: a flag "Auto" changes is `auto`; an unchanged flag keeps its origin.
            if (r.pick !== r.quality.suggestedPick) r.pickOrigin = r.quality.suggestedPick === "unflagged" ? null : "auto";
            r.pick = r.quality.suggestedPick;
            r.xmp = { ...r.xmp, dirty: true };
            applied++;
          });
          return { applied, skipped: ids.length - applied };
        }
        case "get_cull_snapshot":
          return ids.map((i) => {
            const r = byId.get(i)!;
            return { imageId: i, rating: r.rating, pick: r.pick, colorLabel: r.colorLabel, pickOrigin: r.pickOrigin };
          });
        case "restore_cull_snapshot": {
          const changed: number[] = [];
          for (const s of args.snapshots as CullSnapshot[]) {
            const r = byId.get(s.imageId);
            const origin = s.pick === "unflagged" ? null : (s.pickOrigin ?? "user");
            if (!r || (r.rating === s.rating && r.pick === s.pick && r.colorLabel === s.colorLabel && r.pickOrigin === origin)) continue;
            r.rating = s.rating;
            r.pick = s.pick;
            r.pickOrigin = origin;
            r.colorLabel = s.colorLabel;
            r.xmp = { ...r.xmp, dirty: true };
            changed.push(s.imageId);
          }
          return changed;
        }
        case "get_ui_prefs":
          return uiPrefs;
        case "set_ui_prefs":
          uiPrefs = { ...(args.prefs as UiPrefs) };
          return null;
        case "reveal_in_finder":
          return null;
        case "set_burst_keeper": {
          const g = bursts.get(args.groupId as number)!;
          const keeper = args.imageId as number;
          g.keeperImageId = keeper;
          for (const m of g.imageIds) {
            const r = byId.get(m)!;
            r.isBurstKeeper = m === keeper;
            const has = r.tags.some((t) => t.tag === "duplicate_burst");
            if (m === keeper) r.tags = r.tags.filter((t) => !(t.tag === "duplicate_burst" && t.source === "auto" && !t.suppressed));
            else if (!has) r.tags = [...r.tags, { tag: "duplicate_burst", source: "auto", confidence: 1, suppressed: false }];
          }
          return g;
        }
        case "write_xmp_all_dirty": {
          const folderId = args.folderId as number | null;
          const dirty = rows.filter((r) => r.xmp.dirty && (folderId == null || r.folderId === folderId));
          const bad = dirty.filter((r) => mockReadOnly(r.id) || mockMissing(r.id));
          dirty.filter((r) => !bad.includes(r)).forEach((r) => (r.xmp = { dirty: false, syncedAtMs: Date.now(), error: null, hasSidecar: true }));
          if (dirty.length) activity("xmp_save", "Saving metadata to XMP", dirty.length).end(dirty.length, "finished", xmpMessage(dirty.length - bad.length, bad.length));
          return {
            ...ok,
            succeeded: dirty.length - bad.length,
            failed: bad.map((r) => ({ imageId: r.id, reason: mockMissing(r.id) ? missingMessage(r) : READ_ONLY(r.id) })),
          };
        }
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
          guardWrite();
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
        case "get_develop_info": {
          guardOriginal(args.id as number);
          const look = completeAdjustments(getAdj(args.id as number)).profile.look;
          const warnings: DevelopWarning[] = [...(byId.get(args.id as number)?.developWarnings ?? [])];
          if (look && !MOCK_LOOKS.find((l) => l.uuid === look.uuid)?.available) warnings.push({ code: "look_unavailable", detail: look.name });
          const portrait = mockOrientation(args.id as number) >= 5;
          if (completeAdjustments(getAdj(args.id as number)).masks.some((g) => g.components.some((c) => c.shape.kind === "ai" && !c.shape.digest))) warnings.push({ code: "ai_mask_needs_update", detail: "1" });
          return { imageId: args.id, asShot: { temperatureK: 5200, tint: 8 }, sourceWidth: portrait ? 2000 : 3000, sourceHeight: portrait ? 3000 : 2000, fullWidth: portrait ? 4000 : 6000, fullHeight: portrait ? 6000 : 4000, warnings };
        }
        case "sample_white_balance": {
          // IPC v11 WB picker. Deterministic fake: temperature follows x, tint follows y
          // (sensor frame); the top 5 % of the frame is "clipped".
          const p = args.point as NormPoint;
          if (!(p.x >= 0 && p.x <= 1 && p.y >= 0 && p.y <= 1)) throw { kind: "invalid_argument", message: "white balance sample point must be within 0..=1" };
          if (p.y < 0.05) throw { kind: "invalid_argument", message: "the sampled area is clipped (overexposed); pick a neutral grey or white that is not blown out" };
          return { temperatureK: Math.round(3000 + p.x * 5000), tint: Math.round((p.y - 0.5) * 40) };
        }
        case "list_profiles":
          return {
            imageId: args.id,
            cameraModel: "Sony ILCE-7M4",
            cameraProfiles: [
              { name: "Adobe Standard", group: "Adobe Raw", styleId: null },
              { name: "Camera Standard", group: "Camera Matching", styleId: null },
              { name: "Camera Portrait", group: "Camera Matching", styleId: null },
              { name: "Camera Neutral", group: "Camera Matching", styleId: null },
            ],
            looks: MOCK_LOOKS,
            luts: luts.map((l, i) => ({ styleId: 900 + i, lutId: l.id, name: l.name, group: "LUTs", available: true })),
            searchDirs: ["/Library/Application Support/Adobe/CameraRaw/CameraProfiles"],
          };
        case "prepare_develop":
          guardOriginal(args.id as number);
          return null;
        // Masks (IPC v10): minimal fakes so the masking UI can be built and tested.
        case "list_masks": {
          const groups = completeAdjustments(getAdj(args.id as number)).masks;
          return { imageId: args.id, groups, ai: mockAiStatus(groups) };
        }
        case "save_masks":
          commit(args.id as number, { ...completeAdjustments(getAdj(args.id as number)), masks: args.masks as MaskGroup[] }, args.label as string);
          return historyDto(args.id as number);
        case "compute_ai_mask": {
          const r = args.request as AiMaskRequest;
          return sleep(window.__mockAiDelay ?? 250).then(() => ({
            digest: mockDigest(`${args.id}:${JSON.stringify(r.target)}:${JSON.stringify(r.referencePoint)}`),
            target: r.target,
            referencePoint: r.referencePoint,
            origin: "sieve",
            modelVersion: "mock-segmenter@1",
            width: 1920,
            height: 1280,
            bounds: { x: 0, y: 0, width: 1, height: 1 },
            coverage: 0.3,
          }));
        }
        case "detect_people":
          return [
            { referencePoint: { x: 0.35, y: 0.3 }, bbox: { x: 0.2, y: 0.1, width: 0.3, height: 0.85 }, face: { x: 0.3, y: 0.15, width: 0.1, height: 0.14 } },
            { referencePoint: { x: 0.65, y: 0.35 }, bbox: { x: 0.5, y: 0.15, width: 0.3, height: 0.8 }, face: { x: 0.6, y: 0.2, width: 0.1, height: 0.14 } },
          ];
        case "render_mask_overlay": {
          const o = args.options as MaskOverlayOptions;
          const id = args.id as number;
          const key = `${id}:mask`;
          const seq = (seqs.get(key) ?? 0) + 1;
          seqs.set(key, seq);
          const portrait = mockOrientation(id) >= 5;
          const width = portrait ? Math.round((o.maxEdge * 2) / 3) : o.maxEdge;
          const height = portrait ? o.maxEdge : Math.round((o.maxEdge * 2) / 3);
          const groups = ((args.adjustments as ParametricAdjustments).masks ?? []) as MaskGroup[];
          const t = args.target as { groupId: string; componentId: string | null };
          return {
            imageId: id,
            seq,
            url: mockOverlayPng(groups, t, width, height, mockOrientation(id)),
            width,
            height,
            coverage: 0.25,
            renderMs: 5,
          };
        }
        case "get_mask_capabilities":
          return mockCapabilities();
        case "model_downloads_status":
          return modelStatus();
        case "download_models": {
          if (args.group !== "segmentation") throw { kind: "invalid_argument", message: `unknown model group \`${String(args.group)}\`` };
          if (modelRun) throw { kind: "invalid_argument", message: "the segmentation models are already downloading" };
          const run = { cancelled: false };
          modelRun = run;
          void runModelDownload(run);
          return null;
        }
        case "cancel_model_download":
          if (modelRun) modelRun.cancelled = true;
          return null;
        // Phase 8 hardening (v13). Relocate finds every photo unless the path contains "empty"
        // (none found: nothing changes) or "partial" (the first missing image stays missing).
        case "relocate_folder": {
          guardWrite();
          const folderId = args.folderId as number;
          const newPath = String(args.newPath ?? "").replace(/\/+$/, "");
          const folder = catalog.folders.find((f) => f.id === folderId);
          if (!folder) throw { kind: "not_found", message: `folder ${folderId}` };
          if (!newPath.startsWith("/")) throw { kind: "invalid_argument", message: `${newPath}: not a folder` };
          const members = rows.filter((r) => r.folderId === folderId);
          if (newPath.includes("empty"))
            throw { kind: "invalid_argument", message: `None of the ${members.length} photos of ${folder.path} were found in ${newPath}. Choose the folder the shoot was moved to.` };
          const keepMissing = newPath.includes("partial") ? members.find((r) => r.missingSinceMs != null) : undefined;
          const result: RelocateResult = { matched: 0, stillMissing: 0 };
          for (const r of members) {
            if (r === keepMissing) {
              result.stillMissing++;
              continue;
            }
            r.path = `${newPath}/${r.fileName}`;
            r.missingSinceMs = null;
            result.matched++;
          }
          catalog = { ...catalog, folders: catalog.folders.map((f) => (f.id === folderId ? { ...f, path: newPath } : f)) };
          return result;
        }
        case "restore_catalog_backup": {
          const b = health.backups.find((x) => x.index === args.index);
          if (!b) throw { kind: "not_found", message: `backup /mock/catalog.sqlite.bak-${String(args.index)} does not exist` };
          health = { ...health, restorePending: true };
          catalog = { ...catalog, health };
          return health;
        }
        case "render_preview":
          guardOriginal(args.id as number);
          return render(args.id as number, args.adjustments as ParametricAdjustments, args.options as RenderOptions);
        // v19: Paste / Sync / Paste from Previous are one undoable batch (kind `paste`) over any selection.
        case "paste_settings":
          guardWrite();
          return recordBatch("Paste Settings", [...new Set(ids)], (a) => copyFields(a, args.adjustments as ParametricAdjustments, args.fields as AdjustmentField[]));
        case "sync_settings": {
          guardWrite();
          const src = getAdj(args.sourceId as number);
          return recordBatch("Sync Settings", [...new Set(args.targetIds as number[])], (a) => copyFields(a, src, args.fields as AdjustmentField[]));
        }
        case "reset_adjustments":
          return batch(ids, "Reset", () => neutral());
        case "apply_preset": {
          const p = presets.find((x) => x.id === args.presetId);
          if (!p) throw { kind: "not_found", message: "preset" };
          batch(ids, `Preset: ${p.name}`, (a) => copyFields(a, p.adjustments, p.fields));
          for (const i of ids) appliedPresets.set(i, { presetId: p.id, snap: completeAdjustments(getAdj(i)) });
          return null;
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
          const p: Preset = {
            id: ++presetId,
            name: args.name as string,
            adjustments: args.adjustments as ParametricAdjustments,
            fields: args.fields as AdjustmentField[],
            createdAtMs: now,
            updatedAtMs: now,
            groupId: 1,
            sourceFormat: "sieve",
            settingKeys: [],
          };
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
            projectId: (() => {
              const ps = new Set(uniq.map((i) => projectOfFolder(byId.get(i)?.folderId ?? -1)));
              return ps.size === 1 ? ([...ps][0] ?? null) : null;
            })(),
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
        case "detect_scenes": {
          const fid = args.folderId as number | null;
          const pid = (args.projectId as number | null) ?? null;
          const o = args.options as SceneDetectOptions | null;
          const scope = rows.filter((r) => inScope(r, fid, pid));
          return (async () => {
            await progress("detect", scope.length);
            const keepManual = !o?.replaceManual;
            const kept = new Set(keepManual ? scenes.filter((sc) => sc.method === "manual").flatMap((sc) => sc.imageIds) : []);
            const oldAnchors = new Set(scenes.flatMap((sc) => sc.anchorIds));
            scenes = scenes.filter((sc) => (keepManual && sc.method === "manual") || !sc.imageIds.some((i) => scope.some((r) => r.id === i)));
            // Mock rule: consecutive frames of a folder in chunks of 40 form a scene.
            const free = scope.filter((r) => !kept.has(r.id));
            for (let i = 0; i < free.length; ) {
              const chunk = free.slice(i, i + 40).filter((r) => r.folderId === free[i].folderId);
              const sc = newScene(chunk.map((r) => r.id), "auto");
              sc.anchorIds = chunk.map((r) => r.id).filter((x) => oldAnchors.has(x));
              i += chunk.length;
            }
            syncScenes();
            return scenes.filter((sc) => sc.imageIds.some((i) => inScope(byId.get(i)!, fid, pid)));
          })();
        }
        case "list_scenes": {
          const fid = args.folderId as number | null;
          const pid = (args.projectId as number | null) ?? null;
          return scenes.filter((sc) => sc.imageIds.some((i) => inScope(byId.get(i)!, fid, pid)));
        }
        case "get_scene":
          return sceneOf(args.id as number);
        case "create_scene": {
          const list = args.imageIds as number[];
          if (list.length === 0) throw { kind: "invalid_argument", message: "imageIds must not be empty" };
          takeFromScenes(list);
          const sc = newScene([...list], "manual");
          syncScenes();
          return sc;
        }
        case "set_scene_members": {
          const sc = sceneOf(args.id as number);
          const list = args.imageIds as number[];
          if (list.length === 0) throw { kind: "invalid_argument", message: "imageIds must not be empty" };
          takeFromScenes(list.filter((i) => !sc.imageIds.includes(i)));
          sc.imageIds = [...list];
          sc.method = "manual";
          syncScenes();
          return sc;
        }
        case "set_scene_anchors": {
          const sc = sceneOf(args.id as number);
          const list = args.anchorIds as number[];
          if (list.length > 2 || list.some((i) => !sc.imageIds.includes(i))) throw { kind: "invalid_argument", message: "anchors must be 0..=2 members" };
          sc.anchorIds = [...list];
          syncScenes();
          return sc;
        }
        case "merge_scenes": {
          const list = args.ids as number[];
          if (list.length < 2) throw { kind: "invalid_argument", message: "need at least 2 scenes" };
          const target = sceneOf(list[0]);
          for (const i of list.slice(1)) {
            const sc = sceneOf(i);
            target.imageIds.push(...sc.imageIds);
            target.anchorIds.push(...sc.anchorIds);
            sc.imageIds = [];
          }
          target.method = "manual";
          syncScenes();
          return target;
        }
        case "split_scene": {
          const sc = sceneOf(args.id as number);
          const at = sc.imageIds.indexOf(args.firstImageId as number);
          if (at <= 0) throw { kind: "invalid_argument", message: "firstImageId must be a member other than the first" };
          const tail = sc.imageIds.slice(at);
          sc.imageIds = sc.imageIds.slice(0, at);
          sc.method = "manual";
          const n = newScene(tail, "manual");
          n.anchorIds = sc.anchorIds.filter((a) => tail.includes(a));
          syncScenes();
          return [sc, n];
        }
        case "delete_scene":
          sceneOf(args.id as number).imageIds = [];
          syncScenes();
          return null;
        case "match_scene": {
          const anchors = [...new Set(args.anchorIds as number[])];
          const o = args.options as MatchOptions;
          if (anchors.length < 1 || anchors.length > 2) throw { kind: "invalid_argument", message: "1..=2 anchors" };
          const targets = [...new Set(args.targetIds as number[])].filter((t) => !anchors.includes(t));
          if (targets.length === 0) throw { kind: "invalid_argument", message: "no targets" };
          return (async () => {
            await progress("match", targets.length);
            return targets.map((t) => solveMock(anchors, t, o));
          })();
        }
        case "apply_scene_match": {
          const apps = args.applications as MatchApplication[];
          if (apps.some((a) => !byId.has(a.imageId))) throw { kind: "not_found", message: "image" };
          const changed: number[] = [];
          for (const a of apps) {
            if (JSON.stringify(getAdj(a.imageId)) !== JSON.stringify(a.adjustments)) changed.push(a.imageId);
            commit(a.imageId, a.adjustments, (args.label as string | null) ?? "Match Scene");
          }
          return changed;
        }
        case "import_folder": {
          const pid = (args.projectId as number | null) ?? null;
          if (pid != null) requireProject(pid);
          return { folderId: 1, projectId: pid ?? projectOfFolder(1) ?? 1, added: 0, skipped: rows.length, invalid: 0, sidecarsRead: 0, companions: 0 };
        }

        // ---- projects (v14) ----
        case "list_projects":
          return listProjects();
        case "get_project":
          return projectDto(requireProject(args.projectId as number));
        case "create_project": {
          guardWrite();
          const path = String(args.path).replace(/\/+$/, "");
          const known = catalog.folders.find((f) => f.path === path || path.startsWith(`${f.path}/`));
          const opts = args.options as ImportOptions;
          void opts;
          if (known) {
            const p = requireProject(projectOfFolder(known.id)!);
            const result: CreateProjectResult = {
              project: projectDto(p),
              import: { folderId: known.id, projectId: p.id, added: 0, skipped: known.imageCount, invalid: 0, sidecarsRead: 0, companions: 0 },
              existing: true,
            };
            return result;
          }
          // New folders import no photos in the mock (the grid fixture is fixed at startup).
          const folderId = Math.max(0, ...catalog.folders.map((f) => f.id)) + 1;
          catalog = { ...catalog, folders: [...catalog.folders, { id: folderId, path, imageCount: 0, projectId: ++projectSeq }] };
          const name = args.name != null ? validName(args.name as string) : path.split("/").pop() || path;
          const p: MockProject = {
            id: projectSeq,
            name,
            folderIds: [folderId],
            coverImageId: null,
            shootType: (args.shootType as ShootType | null) ?? catalog.shootType,
            workflowStep: "cull",
            createdAtMs: Date.now(),
            lastOpenedAtMs: null,
          };
          projects.push(p);
          const result: CreateProjectResult = {
            project: projectDto(p),
            import: { folderId, projectId: p.id, added: 0, skipped: 0, invalid: 0, sidecarsRead: 0, companions: 0 },
            existing: false,
          };
          return result;
        }
        case "open_project": {
          const p = requireProject(args.projectId as number);
          p.lastOpenedAtMs = Date.now();
          return projectDto(p);
        }
        case "rename_project": {
          guardWrite();
          const p = requireProject(args.projectId as number);
          p.name = validName(args.name as string);
          return projectDto(p);
        }
        case "set_project_cover": {
          guardWrite();
          const p = requireProject(args.projectId as number);
          const img = args.imageId as number | null;
          if (img != null && !p.folderIds.includes(byId.get(img)?.folderId ?? -1)) throw { kind: "invalid_argument", message: `image ${img} is not in project ${p.id}` };
          p.coverImageId = img;
          return projectDto(p);
        }
        case "set_project_shoot_type":
          guardWrite();
          requireProject(args.projectId as number).shootType = args.shootType as ShootType;
          return null;
        case "remove_project": {
          guardWrite();
          const p = requireProject(args.projectId as number);
          const removed = rows.filter((r) => p.folderIds.includes(r.folderId));
          for (const r of removed) byId.delete(r.id);
          for (let i = rows.length - 1; i >= 0; i--) if (p.folderIds.includes(rows[i].folderId)) rows.splice(i, 1);
          catalog = { ...catalog, folders: catalog.folders.filter((f) => !p.folderIds.includes(f.id)), imageCount: rows.length };
          projects = projects.filter((x) => x.id !== p.id);
          return { removedImages: removed.length, removedFolders: p.folderIds.length };
        }
        case "get_workflow_step":
          return requireProject(args.projectId as number).workflowStep;
        case "set_workflow_step":
          guardWrite();
          requireProject(args.projectId as number).workflowStep = args.step as WorkflowStep;
          return null;
        case "set_keeper_rule":
          catalog = { ...catalog, keeperRule: args.rule as KeeperRule };
          return null;
        case "get_edit_plan": {
          const pid = args.projectId as number;
          requireProject(pid);
          if (window.__mockFailPlan) throw { kind: "internal", message: "Could not read the scenes (mock)" };
          const keepers = rows.filter((r) => inScope(r, null, pid) && keeper(r));
          const entries = planEntries(keepers);
          const editStates = keepers.map((r) => editState(r.id));
          const unassignedKeeperIds = keepers.filter((r) => r.sceneId == null).map((r) => r.id);
          const needsReviewIds = editStates.filter((x) => x.needsReview).map((x) => x.imageId);
          const live = entries.filter((e) => !e.skipped);
          const counts: EditPlanCounts = {
            scenes: entries.length,
            toEdit: live.filter((e) => e.status === "to_edit" || e.status === "reset").length,
            edited: live.filter((e) => e.status === "edited").length,
            applied: live.filter((e) => e.status === "applied").length,
            outdated: live.filter((e) => e.status === "outdated").length,
            reset: live.filter((e) => e.status === "reset").length,
            skipped: entries.length - live.length,
            minor: entries.filter((e) => e.minor).length,
            needsReview: needsReviewIds.length,
            unappliedKeepers: live.reduce((n, e) => n + e.unappliedKeeperIds.length, 0),
            unassignedKeepers: unassignedKeeperIds.length,
          };
          const plan: EditPlan = {
            projectId: pid,
            keeperRule: catalog.keeperRule,
            keeperIds: keepers.map((r) => r.id),
            unassignedKeeperIds,
            scenes: entries,
            outdated: counts.unassignedKeepers > 0 || counts.unappliedKeepers > 0,
            counts,
            editStates,
            needsReviewIds,
            latestBatch: latestBatch(new Set(rows.filter((r) => inScope(r, null, pid)).map((r) => r.id))),
          };
          return plan;
        }
        case "set_scene_representative": {
          const sc = sceneOf(args.sceneId as number);
          const img = args.imageId as number | null;
          if (img == null) sceneRep.delete(sc.id);
          else {
            const r = byId.get(img);
            if (!r || !sc.imageIds.includes(img) || !keeper(r)) throw { kind: "invalid_argument", message: `image ${img} is not a keeper of scene ${sc.id}` };
            sceneRep.set(sc.id, img);
          }
          const e = planEntries(rows.filter((r) => r.sceneId === sc.id && keeper(r))).find((x) => x.sceneId === sc.id);
          if (!e) throw { kind: "not_found", message: `scene ${sc.id} has no keepers` };
          return e;
        }
        case "apply_scene_edit": {
          const o = (args.options as SceneApplyOptions | null) ?? (DEFAULT_APPLY as unknown as SceneApplyOptions);
          return applyScenes([args.sceneId as number], o, "Apply to Scene");
        }
        case "apply_all_edited_scenes": {
          const pid = args.projectId as number;
          requireProject(pid);
          const o = (args.options as SceneApplyOptions | null) ?? (DEFAULT_APPLY as unknown as SceneApplyOptions);
          const keepers = rows.filter((r) => inScope(r, null, pid) && keeper(r));
          const due = planEntries(keepers)
            .filter((e) => !e.skipped && e.edited && (e.status === "edited" || e.status === "outdated" || (e.status === "applied" && e.unappliedKeeperIds.length > 0)))
            .map((e) => e.sceneId);
          return applyScenes(due, o, "Apply to Scene", true);
        }
        case "list_styles": {
          const user: StylePreset[] = presets.map((p) => ({
            id: p.id,
            groupId: 1,
            name: p.name,
            sourceFormat: "sieve",
            sourcePath: null,
            fields: p.fields,
            settingKeys: [],
            supportsAmount: false,
            warnings: [],
          }));
          const lutProfiles: StyleProfile[] = luts.map((l, i) => ({
            id: 900 + i,
            groupId: 2,
            kind: "lut",
            name: l.name,
            sourceFormat: "sieve",
            sourcePath: l.path,
            available: true,
            supportsAmount: true,
            monochrome: false,
            cameraProfile: null,
            cameraModel: null,
            lookUuid: null,
            lutId: l.id,
          }));
          return {
            groups: [
              { id: 1, name: "User Presets", kind: "user", sourcePath: null, importedAtMs: null, presets: user, profiles: [] },
              ...styleGroups,
              { id: 2, name: "LUTs", kind: "luts", sourcePath: null, importedAtMs: null, presets: [], profiles: lutProfiles },
            ],
          };
        }
        case "import_style_folder": {
          const root = String(args.path);
          const gid = ++styleSeq;
          const name = root.split("/").pop() || root;
          const presetOf = (n: number): StylePreset => ({
            id: ++presetId,
            groupId: gid,
            name: `${name} ${String(n).padStart(2, "0")}`,
            sourceFormat: "xmp_preset",
            sourcePath: `${root}/${name} ${n}.xmp`,
            fields: ["exposure", "contrast"],
            settingKeys: ["Exposure2012", "Contrast2012"],
            supportsAmount: false,
            warnings: [],
          });
          const group: StyleGroup = { id: gid, name, kind: "imported", sourcePath: root, importedAtMs: Date.now(), presets: [presetOf(1), presetOf(2)], profiles: [] };
          const i = styleGroups.findIndex((g) => g.sourcePath === root);
          if (i >= 0) styleGroups.splice(i, 1, group);
          else styleGroups.push(group);
          return { root, groupIds: [gid], presets: 2, profiles: 0, skipped: [] };
        }
        case "remove_style_group": {
          const i = styleGroups.findIndex((g) => g.id === args.groupId);
          if (i < 0) throw { kind: (args.groupId as number) <= 2 ? "invalid_argument" : "not_found", message: "style group" };
          styleGroups.splice(i, 1);
          return null;
        }
        case "resolve_preset":
          return (args.adjustments as ParametricAdjustments | null) ?? getAdj(args.id as number);
        case "auto_tone": {
          const keys = (args.keys as AdjustmentField[] | null) ?? ["exposure", "contrast", "highlights", "shadows", "whites", "blacks", "vibrance", "saturation"];
          const all: Required<AutoToneValues> = { exposure: 0.35, contrast: 8, highlights: -42, shadows: 31, whites: 12, blacks: -9, vibrance: 10, saturation: 2 };
          const out: AutoToneValues = { exposure: null, contrast: null, highlights: null, shadows: null, whites: null, blacks: null, vibrance: null, saturation: null };
          for (const k of keys) if (k in all) (out as Record<string, number | null>)[k] = all[k as keyof AutoToneValues];
          return out;
        }
        case "auto_white_balance":
          return { temperatureK: 5350, tint: 6 };
        case "style_model_status":
          return styleModel;
        case "train_style_model": {
          if (styleModel.availableExamples < styleModel.minExamples) {
            styleModel = { ...styleModel, state: "failed", error: "Edit at least 20 photos first" };
            void emit("style-model-finished", { ok: false, cancelled: false, error: styleModel.error, status: styleModel });
            return null;
          }
          styleModel = { ...styleModel, state: "training", progress: 0, error: null };
          void (async () => {
            for (let k = 1; k <= 4; k++) {
              await sleep(window.__mockSceneDelay ?? 30);
              styleModel = { ...styleModel, progress: k / 4 };
              void emit("style-model-progress", { phase: "fitting", done: k * 100, total: 400 });
            }
            styleModel = { ...styleModel, state: "ready", progress: null, trainedAtMs: Date.now(), trainingExamples: styleModel.availableExamples };
            void emit("style-model-finished", { ok: true, cancelled: false, error: null, status: styleModel });
          })();
          return null;
        }
        case "cancel_style_training":
          return null;
        case "predict_style":
        case "apply_style_prediction": {
          if (styleModel.state !== "ready") throw { kind: "invalid_argument", message: "No style model yet: edit at least 20 photos, then train." };
          const ids = args.imageIds as number[];
          if (cmd === "predict_style") return ids.map((imageId) => ({ imageId, adjustments: styleAdj(getAdj(imageId)), fields: STYLE_FIELDS, confidence: 0.8, notes: [] }));
          return recordBatch("Auto Edit (My Style)", ids, (a) => styleAdj(a));
        }
        case "paste_previous":
          return recordBatch(
            "Paste from Previous",
            [...new Set((args.targetIds as number[]).filter((i) => i !== args.previousId))],
            (a) => copyFields(a, getAdj(args.previousId as number), (args.fields as AdjustmentField[] | null) ?? DEFAULT_PASTE_PREVIOUS),
          );
        case "undo_edit_batch": {
          const b = batches.get(args.batchId as number);
          if (!b) throw { kind: "not_found", message: `batch ${args.batchId}` };
          if (b.undone) throw { kind: "invalid_argument", message: "batch already undone" };
          // v16: linear undo; later edits on the batch's photos block it (nothing changes).
          const conflicts = batchConflicts(args.batchId as number);
          const edited = batchEditedAfter(args.batchId as number);
          if (edited.length)
            throw { kind: "conflict", message: `Later edits on ${conflicts.length} photo${conflicts.length === 1 ? "" : "s"}; undo those first` };
          // v17: an apply made from this batch's settings (Auto edit -> Apply to scene) is a later edit too.
          if (conflicts.length)
            throw {
              kind: "conflict",
              message:
                conflicts.length === 1
                  ? "A scene was applied from this edit since; undo that apply first"
                  : `${conflicts.length} scenes were applied from this edit since; undo those applies first`,
            };
          b.undone = true;
          b.undoneAt = Date.now();
          const restoredIds: number[] = [];
          const skippedIds: number[] = [];
          for (const it of b.items) {
            if (JSON.stringify(getAdj(it.id)) === it.after) {
              commit(it.id, JSON.parse(it.before) as ParametricAdjustments, `Undo ${b.label}`, false);
              // v15: the restored settings keep the provenance they had before the batch.
              const e = cursorEntry(it.id);
              if (e) {
                e.source = it.beforeSource;
                e.batchId = it.beforeBatch;
              }
              restoredIds.push(it.id);
            } else skippedIds.push(it.id);
          }
          // v16: scenes whose last apply was this batch are no longer applied.
          for (const [sid, a] of [...sceneApplied]) if (a.batchId === (args.batchId as number)) sceneApplied.delete(sid);
          return { restoredIds, skippedIds };
        }
        // ---- IPC v15 ----
        case "set_scene_skipped": {
          guardWrite();
          const sc = sceneOf(args.sceneId as number);
          const ks = rows.filter((r) => r.sceneId === sc.id && keeper(r));
          if (!ks.length) throw { kind: "invalid_argument", message: `scene ${sc.id} has no keepers` };
          if (args.skipped) sceneSkipped.add(sc.id);
          else sceneSkipped.delete(sc.id);
          return planEntries(ks).find((x) => x.sceneId === sc.id);
        }
        case "get_edit_states": {
          const ids = args.imageIds as number[];
          const unknown = ids.find((i) => !byId.has(i));
          if (unknown != null) throw { kind: "not_found", message: `image ${unknown}` };
          return ids.map(editState);
        }
        case "get_edit_batches":
          return (args.batchIds as number[]).map(batchInfo);
        case "mark_reviewed": {
          const ids = args.imageIds as number[];
          const unknown = ids.find((i) => !byId.has(i));
          if (unknown != null) throw { kind: "not_found", message: `image ${unknown}` };
          const cleared: number[] = [];
          for (const id of ids) {
            const st = editState(id);
            if (!st.needsReview || cleared.includes(id)) continue;
            const item = batches.get(st.batchId!)?.items.find((i) => i.id === id);
            if (item) item.reviewed = true;
            cleared.push(id);
          }
          return cleared;
        }
        case "cancel_scene_apply":
          applyCancel = true;
          return null;
        case "list_xmp_failures": {
          const pid = (args.projectId as number | null) ?? null;
          if (pid != null) requireProject(pid);
          const out: XmpFailure[] = rows.filter((r) => r.xmp.error && inScope(r, null, pid)).map((r) => ({ imageId: r.id, reason: r.xmp.error as string }));
          return out;
        }
        case "get_render_stats":
          return mockStats(args.id as number, 0.5, 0.01);
        // ---- IPC v19 ----
        case "edit_capture_time": {
          guardWrite();
          const list = args.ids as number[];
          const mode = args.mode as CaptureTimeEdit;
          if (new Set(list).size !== list.length) throw { kind: "invalid_argument", message: "image listed twice" };
          const rowOf = (i: number) => {
            const r = byId.get(i);
            if (!r) throw { kind: "not_found", message: `image ${i}` };
            return r;
          };
          const targets = list.map(rowOf);
          let offset: number | null = null;
          if (mode.kind === "shift") offset = mode.offsetMs;
          else if (mode.kind === "set_exact") {
            const ref = targets.find((r) => r.id === mode.referenceId);
            if (!ref) throw { kind: "invalid_argument", message: "the reference photo must be one of the selected photos" };
            offset = ref.capture.capturedAtMs != null ? mode.capturedAtMs - ref.capture.capturedAtMs : null;
          } else if (mode.kind === "sync_cameras") {
            const a = rowOf(mode.referenceId).capture.capturedAtMs;
            const b = rowOf(mode.targetId).capture.capturedAtMs;
            if (a == null || b == null) throw { kind: "invalid_argument", message: "both photos used to sync the cameras need a capture time" };
            offset = a - b;
          }
          const result: CaptureTimeEditResult = { changedIds: [], skippedIds: [], offsetMs: offset, previous: [] };
          for (const r of targets) {
            const cur = r.capture.capturedAtMs;
            let next: { ms: number | null; source: "exif" | "user" } | null = null;
            if (mode.kind === "revert") next = { ms: r.capture.originalCapturedAtMs ?? null, source: "exif" };
            else if (mode.kind === "set_exact" && mode.referenceId === r.id && cur == null) next = { ms: mode.capturedAtMs, source: "user" };
            else if (cur != null && offset != null) next = { ms: cur + offset, source: "user" };
            if (!next || (next.ms === cur && next.source === (r.capture.captureTimeSource ?? "exif"))) {
              result.skippedIds.push(r.id);
              continue;
            }
            result.previous.push({ imageId: r.id, capturedAtMs: cur, source: r.capture.captureTimeSource ?? "exif" });
            r.capture = { ...r.capture, capturedAtMs: next.ms, captureTimeSource: next.source };
            r.xmp = { ...r.xmp, dirty: true };
            result.changedIds.push(r.id);
          }
          return result;
        }
        case "restore_capture_times": {
          guardWrite();
          const snaps = args.snapshots as CaptureTimeSnapshot[];
          for (const sn of snaps) if (!byId.get(sn.imageId)) throw { kind: "not_found", message: `image ${sn.imageId}` };
          const changed: number[] = [];
          for (const sn of snaps) {
            const r = byId.get(sn.imageId)!;
            if (r.capture.capturedAtMs === sn.capturedAtMs && r.capture.captureTimeSource === sn.source) continue;
            r.capture = { ...r.capture, capturedAtMs: sn.capturedAtMs, captureTimeSource: sn.source };
            r.xmp = { ...r.xmp, dirty: true };
            changed.push(r.id);
          }
          return changed;
        }
        case "get_image_metadata": {
          const r = byId.get(args.id as number);
          if (!r) throw { kind: "not_found", message: `image ${args.id}` };
          const ext = r.fileName.split(".").pop()!.toLowerCase();
          const sidecarPath = r.format === "jpeg" ? `${r.path}.xmp` : r.path.replace(/\.[^.]+$/, ".xmp");
          const meta: ImageMetadata = {
            imageId: r.id,
            path: r.path,
            fileName: r.fileName,
            folderPath: r.path.slice(0, r.path.lastIndexOf("/")),
            format: r.format,
            extension: ext,
            fileSize: r.fileSize,
            fileMtimeMs: r.fileMtimeMs,
            capturedAtMs: r.capture.capturedAtMs,
            originalCapturedAtMs: r.capture.originalCapturedAtMs ?? null,
            captureTimeSource: r.capture.captureTimeSource ?? "exif",
            camera: r.camera,
            lens: r.capture.lens,
            iso: r.capture.iso,
            shutterSeconds: r.capture.shutterSeconds,
            aperture: r.capture.aperture,
            focalLengthMm: r.capture.focalLengthMm,
            focalLength35mm: r.capture.focalLengthMm,
            exposureCompensationEv: r.id % 4 === 0 ? -0.7 : 0,
            flashFired: false,
            cameraSerial: "1234567",
            width: r.width,
            height: r.height,
            orientation: r.orientation,
            // Every 5th frame carries GPS.
            gps: r.id % 5 === 0 ? { latitude: 51.500729, longitude: -0.124625, altitudeM: 12 } : null,
            sidecarPath,
            sidecarExists: r.xmp.hasSidecar,
            companionPath: r.companionPath,
            missing: r.missingSinceMs != null,
          };
          return meta;
        }
        case "auto_upright": {
          if (!byId.get(args.id as number)) throw { kind: "not_found", message: `image ${args.id}` };
          const mode = args.mode as UprightMode;
          const live = completeAdjustments((args.adjustments as ParametricAdjustments | null) ?? getAdj(args.id as number));
          // `?upright=none`: no usable lines (Lightroom leaves the photo as it is).
          if (mode === "off") return { mode, solution: null, message: null } satisfies UprightResult;
          if (params.get("upright") === "none" || (mode === "guided" && (live.transform.guides ?? []).length < 2))
            return { mode, solution: null, message: mode === "guided" ? "Draw at least two guides" : `No straight lines found for ${mode}` } satisfies UprightResult;
          const deg = mode === "level" ? 1.2 : mode === "vertical" ? 1.0 : 0.8;
          const rad = (deg * Math.PI) / 180;
          const k = mode === "level" ? 0 : 0.04;
          return {
            mode,
            solution: { mode, matrix: [Math.cos(rad), -Math.sin(rad), 0, Math.sin(rad), Math.cos(rad), 0, 0, k, 1], rotationDeg: deg, crs: [] },
            message: null,
          } satisfies UprightResult;
        }
        case "render_preview_variant": {
          guardOriginal(args.id as number);
          const v = args.variant as PreviewVariant;
          const live = completeAdjustments(args.adjustments as ParametricAdjustments);
          let resolved: ParametricAdjustments;
          if (v.kind === "preset") {
            const p = presets.find((x) => x.id === v.presetId);
            // Imported style presets are not stored in the mock (like `resolve_preset`): they preview as the live settings.
            resolved = p ? copyFields(live, p.adjustments, p.fields) : live;
          } else {
            if (!v.fields.length) throw { kind: "invalid_argument", message: "fields must not be empty" };
            resolved = copyFields(live, neutral(), v.fields);
          }
          return render(args.id as number, resolved, args.options as RenderOptions);
        }
        case "set_project_reject_strictness":
          guardWrite();
          requireProject(args.projectId as number).rejectStrictness = args.strictness as RejectStrictness;
          // The scorer re-evaluates suggestions: conservative drops borderline rejects, aggressive adds every non-best burst frame.
          for (const r of rows) {
            if (!r.quality || projectOfFolder(r.folderId) !== (args.projectId as number)) continue;
            const base = (baseSuggest.get(r.id) ?? (baseSuggest.set(r.id, r.quality.suggestedPick), r.quality.suggestedPick)) as PickFlag;
            const st = args.strictness as RejectStrictness;
            const next: PickFlag = st === "conservative" ? (base === "reject" && r.quality.overall >= 0.1 ? "unflagged" : base) : st === "aggressive" ? (base === "unflagged" && r.burstGroupId != null && !r.isBurstKeeper ? "reject" : base) : base;
            r.quality = { ...r.quality, suggestedPick: next };
          }
          return null;
        case "plugin:dialog|open":
          return (args.options as { directory?: boolean } | undefined)?.directory ? (window.__mockPickDir !== undefined ? window.__mockPickDir : "/mock/export/Smith Wedding") : "/mock/import/Moody Blue.cube";
        default:
          return null;
      }
    },
    { shouldMockEvents: true },
  );
  // Files are served from the page origin as-is (no asset:// protocol in the browser).
  (window as unknown as { __TAURI_INTERNALS__: { convertFileSrc: (p: string) => string } }).__TAURI_INTERNALS__.convertFileSrc = (p) => p;
}
