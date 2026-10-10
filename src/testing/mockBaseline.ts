// Mock of the baseline edit commands (IPC v21) for `src/testing/mockBackend.ts`. Mirrors `db::baseline` (scope, skip /
// replace, provenance, results, the `baseline` edit batch) with a synthetic engine that follows the engine's model
// (without its smoothing): look groups copied from the anchor (`BASELINE_LOOK_FIELDS`), light = the photo's synthetic Auto
// + the anchor's offset from its own Auto, every 11th photo flagged low-key (kept 0.5 EV darker), every 19th "Auto
// failed" (the anchor's light), crop / transform / masks kept.
// Switches (URL of the mock page):
// - `?baseline=presets`: an imported preset group "Wedding Looks" (3 presets: Soft Film, Warm Matte, Clean B&W) in
//   `list_styles` / `list_presets`;
// - `?baseline=anchor`: + the anchor = project 1's first keeper with "Soft Film" applied and its light tweaked
//   (+0.3 EV and warmer than Auto) — the state after UI steps 1-2; no run yet;
// - `?baseline=1`: + a finished run on project 1 (scope keepers; results applied / flagged / skipped / anchor, one
//   undoable batch, provenance).
// v21.1: `auto_light` (the light-only Auto) returns the same synthetic Auto the engine uses (`autoLightOf`), so Develop's
// Auto on the anchor reads "same as Auto"; without a switch it is the legacy Develop Auto values (exposure +0.35, 5350 K
// / +6, as `auto_tone` / `auto_white_balance`). Runs report `live` counts and an undo message; results carry `state`;
// `planBaseline` feeds `EditPlan.baseline`.
// Without a switch: no presets beyond the user's, no run. `run_baseline` finishes after `window.__mockBaselineDelay` ms
// (default 300), reporting `activity-event` kind `baseline_edit` and one `baseline-run-finished`.
import { emit } from "@tauri-apps/api/event";
import { BASELINE_LOOK_FIELDS, MAX_BASELINE_SAMPLES, DEFAULT_BASELINE_SAMPLES } from "../ipc";
import type {
  ActivityEvent,
  AdjustmentField,
  AutoLightValues,
  BaselineAnchor,
  BaselineCounts,
  BaselineLiveCounts,
  BaselineOutcome,
  BaselinePhotoResult,
  BaselinePlanCounts,
  BaselinePreview,
  BaselinePreviewOptions,
  BaselineProvenance,
  BaselineReason,
  BaselineRun,
  BaselineRunFinished,
  BaselineSample,
  BaselineSettings,
  BaselineState,
  EditBatchInfo,
  EditBatchResult,
  EditPlanBaseline,
  ImageEditState,
  ImageQuery,
  LightOffset,
  LightValues,
  ParametricAdjustments,
  Preset,
  RawImageEntry,
  StyleGroup,
} from "../ipc";

declare global {
  interface Window {
    /** Test hook: ms before a mock `run_baseline` finishes (default 300). */
    __mockBaselineDelay?: number;
  }
}

/** What the mock backend gives the baseline mock. */
export interface MockBaselineContext {
  rows: RawImageEntry[];
  byId: Map<number, RawImageEntry>;
  projectOf: (r: RawImageEntry) => number | null;
  requireProject: (id: number) => unknown;
  keeper: (r: RawImageEntry) => boolean;
  getAdj: (id: number) => ParametricAdjustments;
  neutral: () => ParametricAdjustments;
  isNeutral: (a: ParametricAdjustments) => boolean;
  copyFields: (dst: ParametricAdjustments, src: ParametricAdjustments, fields: AdjustmentField[]) => ParametricAdjustments;
  /** A user edit (one history entry). */
  commitUser: (id: number, next: ParametricAdjustments, label: string) => void;
  /** One undoable batch of kind `baseline` (only changed photos recorded). */
  commitBatch: (label: string, items: { id: number; next: ParametricAdjustments; reviewReason: string | null }[]) => EditBatchResult;
  /** History cursor entry of a photo (`null` = never edited). */
  cursor: (id: number) => { entryId: number; batchId: number | null } | null;
  batchInfo: (batchId: number) => EditBatchInfo | null;
  /** v21.1: the batch item of `id` was kept by `undo_edit_batch(…, {keepLaterEdits: true})`. */
  keptOnUndo: (batchId: number, id: number) => boolean;
  /** `ImageEditState.needsReview` of a photo. */
  needsReview: (id: number) => boolean;
  presets: Preset[];
  styleGroups: StyleGroup[];
  nextPresetId: () => number;
  guardWrite: () => void;
}

/** Returned by `handle` for commands it does not know. */
export const NOT_BASELINE = Symbol("not-baseline");

const LABEL = "Baseline Edit";
const ENGINE = "baseline-mock@1";

type Prov = { runId: number; batchId: number; entryId: number; flagged: boolean; appliedAtMs: number; anchorId: number; presetId: number | null };
type Run = Omit<BaselineRun, "batch" | "live"> & { batchId: number | null };

const round = (v: number, step: number) => Math.round(v / step) * step;
const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
const mired = (k: number) => 1e6 / Math.max(1, k);

/** Synthetic Auto of a photo (deterministic per id). */
export const mockAutoLight = (id: number): LightValues => ({
  exposure: round(((id * 37) % 21 - 10) / 20, 0.05),
  contrast: 6,
  highlights: -((id % 5) * 8),
  shadows: (id % 7) * 5,
  whites: 10,
  blacks: -8,
  temperatureK: 4800 + ((id * 53) % 1400),
  tint: (id * 7) % 16,
});

/** Legacy Develop Auto of the mock (`auto_tone` all keys + `auto_white_balance`), used without a `?baseline=` switch. */
export const LEGACY_AUTO_LIGHT: LightValues = { exposure: 0.35, contrast: 8, highlights: -42, shadows: 31, whites: 12, blacks: -9, temperatureK: 5350, tint: 6 };

const lightOf = (a: ParametricAdjustments, asShot: { temperatureK: number; tint: number }): LightValues => ({
  exposure: a.exposure,
  contrast: a.contrast,
  highlights: a.highlights,
  shadows: a.shadows,
  whites: a.whites,
  blacks: a.blacks,
  temperatureK: a.whiteBalance.mode === "custom" ? a.whiteBalance.temperatureK : asShot.temperatureK,
  tint: a.whiteBalance.mode === "custom" ? a.whiteBalance.tint : asShot.tint,
});

const offsetOf = (l: LightValues, a: LightValues): LightOffset => ({
  exposure: l.exposure - a.exposure,
  contrast: l.contrast - a.contrast,
  highlights: l.highlights - a.highlights,
  shadows: l.shadows - a.shadows,
  whites: l.whites - a.whites,
  blacks: l.blacks - a.blacks,
  temperatureMired: mired(l.temperatureK) - mired(a.temperatureK),
  tint: l.tint - a.tint,
});

/** Rust `LightOffset::add_to` + `develop::baseline::clamp_light`. */
const addClamp = (a: LightValues, o: LightOffset): LightValues => ({
  exposure: round(clamp(a.exposure + o.exposure, -5, 5), 0.05),
  contrast: Math.round(clamp(a.contrast + o.contrast, -100, 100)),
  highlights: Math.round(clamp(a.highlights + o.highlights, -100, 100)),
  shadows: Math.round(clamp(a.shadows + o.shadows, -100, 100)),
  whites: Math.round(clamp(a.whites + o.whites, -100, 100)),
  blacks: Math.round(clamp(a.blacks + o.blacks, -100, 100)),
  temperatureK: Math.round(clamp(1e6 / Math.max(1, mired(a.temperatureK) + o.temperatureMired), 2000, 50000)),
  tint: Math.round(clamp(a.tint + o.tint, -150, 150)),
});

const applyLight = (a: ParametricAdjustments, l: LightValues): ParametricAdjustments => ({
  ...a,
  exposure: l.exposure,
  contrast: l.contrast,
  highlights: l.highlights,
  shadows: l.shadows,
  whites: l.whites,
  blacks: l.blacks,
  whiteBalance: { mode: "custom", temperatureK: l.temperatureK, tint: l.tint },
});

export function createMockBaseline(ctx: MockBaselineContext) {
  const params = new URLSearchParams(typeof location === "undefined" ? "" : location.search);
  const mode = params.get("baseline");
  const runs: Run[] = [];
  /** Results of each project's latest finished run. */
  const results = new Map<number, BaselinePhotoResult[]>();
  const prov = new Map<number, Prov>();
  let running: { runId: number; cancelled: boolean } | null = null;
  let runSeq = 0;
  let activitySeq = 200_000;
  const AS_SHOT = { temperatureK: 5200, tint: 8 };
  /** Rust `develop::baseline::measure_light`: the one light-only Auto (`auto_light` and the engine). */
  const autoLightOf = (id: number): LightValues => (mode ? mockAutoLight(id) : { ...LEGACY_AUTO_LIGHT });

  const projectRows = (pid: number) =>
    ctx.rows.filter((r) => ctx.projectOf(r) === pid).sort((a, b) => (a.capture.capturedAtMs ?? 0) - (b.capture.capturedAtMs ?? 0) || a.id - b.id);
  const reason = (kind: BaselineReason["kind"], text: string): BaselineReason => ({ kind, text });

  /** Rust `db::baseline::photo_states`. */
  const stateOf = (id: number): "unedited" | "on_baseline" | "edited" => {
    const p = prov.get(id);
    const c = ctx.cursor(id);
    if (p && c && c.entryId === p.entryId && c.batchId === p.batchId) return "on_baseline";
    return ctx.isNeutral(ctx.getAdj(id)) ? "unedited" : "edited";
  };

  /** Rust `db::baseline::resolve_settings`. */
  function resolve(projectId: number, s: BaselineSettings): BaselineSettings {
    ctx.requireProject(projectId);
    const inProject = (id: number) => {
      const r = ctx.byId.get(id);
      if (!r) throw { kind: "not_found", message: `image ${id}` };
      if (ctx.projectOf(r) !== projectId) throw { kind: "invalid_argument", message: `image ${id} is not in project ${projectId}` };
    };
    inProject(s.anchorId);
    if (s.presetId != null && !ctx.presets.some((p) => p.id === s.presetId)) throw { kind: "not_found", message: `preset ${s.presetId}` };
    if (s.scope.kind === "selection") {
      s.scope.ids.forEach(inProject);
      return { ...s, scope: { kind: "selection", ids: [...new Set(s.scope.ids)] } };
    }
    return { ...s, scope: { ...s.scope } };
  }

  const scopeRows = (projectId: number, s: BaselineSettings) => {
    const all = projectRows(projectId);
    if (s.scope.kind === "all") return all;
    if (s.scope.kind === "keepers") return all.filter(ctx.keeper);
    const ids = new Set(s.scope.ids);
    return all.filter((r) => ids.has(r.id));
  };

  /** Rust `db::baseline::look_source`. */
  const lookOf = (s: BaselineSettings): ParametricAdjustments => {
    const cur = ctx.getAdj(s.anchorId);
    const p = s.presetId != null ? ctx.presets.find((x) => x.id === s.presetId) : undefined;
    return p && ctx.isNeutral(cur) ? ctx.copyFields(cur, p.adjustments, p.fields) : cur;
  };

  const anchorOf = (s: BaselineSettings, look: ParametricAdjustments): BaselineAnchor => {
    const auto = autoLightOf(s.anchorId);
    const light = lightOf(look, AS_SHOT);
    return { imageId: s.anchorId, auto, light, offset: offsetOf(light, auto) };
  };

  const counts = (rs: RawImageEntry[], s: BaselineSettings): BaselinePlanCounts => {
    const c: BaselinePlanCounts = { inScope: rs.length, toWrite: 0, onBaseline: 0, edited: 0 };
    for (const r of rs) {
      if (r.id === s.anchorId) continue;
      const st = stateOf(r.id);
      if (st === "edited") {
        c.edited++;
        if (s.replaceEdited) c.toWrite++;
      } else {
        c.toWrite++;
        if (st === "on_baseline") c.onBaseline++;
      }
    }
    return c;
  };

  /** Rust `develop::baseline::compute` (target model, see the header). */
  function plan(r: RawImageEntry, s: BaselineSettings, look: ParametricAdjustments, anchor: BaselineAnchor): { result: BaselinePhotoResult; next: ParametricAdjustments | null } {
    const base = { imageId: r.id, sceneId: r.sceneId ?? null, burstGroupId: r.burstGroupId ?? null, auto: null, light: null, reasons: [] as BaselineReason[], state: null };
    if (r.id === s.anchorId) return { result: { ...base, outcome: "anchor" }, next: null };
    if (stateOf(r.id) === "edited" && !s.replaceEdited) return { result: { ...base, outcome: "skipped_edited" }, next: null };
    if (r.missingSinceMs != null) return { result: { ...base, outcome: "failed", reasons: [reason("unreadable", "The original is missing")] }, next: null };
    const reasons: BaselineReason[] = [];
    let auto: LightValues | null = autoLightOf(r.id);
    let light: LightValues;
    if (r.id % 19 === 0) {
      auto = null;
      light = anchor.light;
      reasons.push(reason("auto_failed", "Auto could not be computed; used the anchor's light"));
    } else {
      light = addClamp(auto, anchor.offset);
      if (r.id % 11 === 0) {
        light = { ...light, exposure: round(clamp(light.exposure - 0.5, -5, 5), 0.05) };
        reasons.push(reason("low_key", "Dark on purpose: kept darker than Auto"));
      }
    }
    const cur = ctx.getAdj(r.id);
    const next = applyLight(ctx.copyFields(cur, look, [...BASELINE_LOOK_FIELDS]), light);
    return { result: { ...base, outcome: reasons.length ? "flagged" : "applied", reasons, auto, light }, next };
  }

  /** Rust `develop::baseline::pick_samples` (scenes round-robin, evenly spaced). */
  function pickSamples(rs: RawImageEntry[], n: number): RawImageEntry[] {
    const groups = new Map<string, RawImageEntry[]>();
    for (const r of rs) {
      const k = String(r.sceneId ?? "none");
      groups.set(k, [...(groups.get(k) ?? []), r]);
    }
    const gs = [...groups.values()];
    const quota = gs.map(() => 0);
    let left = Math.min(n, rs.length);
    while (left > 0) gs.forEach((g, i) => left > 0 && quota[i] < g.length && (quota[i]++, left--));
    const out = gs.flatMap((g, i) => Array.from({ length: quota[i] }, (_, j) => g[Math.floor(((2 * j + 1) * g.length) / (2 * quota[i]))]));
    return out.sort((a, b) => rs.indexOf(a) - rs.indexOf(b));
  }

  const countOf = (rs: BaselinePhotoResult[]): BaselineCounts => {
    const c: BaselineCounts = { total: rs.length, applied: 0, flagged: 0, skippedEdited: 0, anchor: 0, failed: 0 };
    const key: Record<BaselineOutcome, keyof BaselineCounts> = { applied: "applied", flagged: "flagged", skipped_edited: "skippedEdited", anchor: "anchor", failed: "failed" };
    for (const r of rs) c[key[r.outcome]]++;
    return c;
  };

  /** Rust `develop::baseline::summary`. */
  const summary = (c: BaselineCounts) => {
    const w = c.applied + c.flagged;
    let m = `Edited ${w} photo${w === 1 ? "" : "s"}`;
    if (c.flagged) m += `; ${c.flagged} need a look`;
    if (c.skippedEdited) m += `; ${c.skippedEdited} already edited were skipped`;
    if (c.failed) m += `; ${c.failed} could not be read`;
    return m;
  };

  /** Rust `db::baseline::provenance` state of one photo (v21.1: kept-on-undo photos read `user_edited`). */
  const provState = (id: number, p: Prov): BaselineState => {
    const c = ctx.cursor(id);
    if (c && c.entryId === p.entryId && c.batchId === p.batchId) return "on_baseline";
    return ctx.batchInfo(p.batchId)?.undoneAtMs != null && !ctx.keptOnUndo(p.batchId, id) ? "undone" : "user_edited";
  };

  /** Rust `db::baseline::live_counts`. */
  const liveOf = (runId: number): BaselineLiveCounts => {
    const c: BaselineLiveCounts = { written: 0, onBaseline: 0, needsLook: 0, userEdited: 0, undone: 0 };
    for (const [id, p] of prov) {
      if (p.runId !== runId) continue;
      c.written++;
      const st = provState(id, p);
      if (st === "on_baseline") {
        c.onBaseline++;
        if (p.flagged && ctx.needsReview(id)) c.needsLook++;
      } else if (st === "user_edited") c.userEdited++;
      else c.undone++;
    }
    return c;
  };

  /** Rust `db::baseline::undone_message`. */
  const undoneMessage = (l: BaselineLiveCounts) => {
    const photos = (n: number) => (n === 1 ? "1 photo" : `${n} photos`);
    if (l.userEdited === 0) return "Undone: the photos are back to how they were";
    return `Undone on ${photos(l.undone)}; ${l.userEdited === 1 ? "1 photo you changed since was kept" : `${l.userEdited} photos you changed since were kept`}`;
  };

  const dto = (r: Run): BaselineRun => {
    const { batchId, ...rest } = r;
    const batch = batchId != null ? ctx.batchInfo(batchId) : null;
    const live = liveOf(r.id);
    return { ...rest, settings: structuredClone(r.settings), batch, live, message: batch?.undoneAtMs != null ? undoneMessage(live) : r.message };
  };

  /** The run's write (Rust `db::baseline::store_results`). */
  function store(run: Run, projectId: number) {
    const s = run.settings;
    const look = lookOf(s);
    const anchor = anchorOf(s, look);
    run.anchor = anchor;
    const planned = scopeRows(projectId, s).map((r) => plan(r, s, look, anchor));
    const items = planned.filter((p) => p.next).map((p) => ({ id: p.result.imageId, next: p.next!, reviewReason: p.result.outcome === "flagged" ? p.result.reasons[0].text : null }));
    const batch = ctx.commitBatch(LABEL, items);
    if (batch.batchId != null)
      for (const id of batch.changedIds) {
        const c = ctx.cursor(id)!;
        const flagged = planned.some((p) => p.result.imageId === id && p.result.outcome === "flagged");
        prov.set(id, { runId: run.id, batchId: batch.batchId, entryId: c.entryId, flagged, appliedAtMs: Date.now(), anchorId: s.anchorId, presetId: s.presetId });
      }
    const rs = planned.map((p) => p.result);
    results.set(projectId, rs);
    run.batchId = batch.batchId;
    run.counts = countOf(rs);
    run.state = "finished";
    run.finishedAtMs = Date.now();
    run.message = summary(run.counts);
  }

  function begin(projectId: number, s: BaselineSettings): Run {
    const run: Run = {
      id: ++runSeq,
      projectId,
      settings: s,
      state: "running",
      startedAtMs: Date.now(),
      finishedAtMs: null,
      message: null,
      engineVersion: ENGINE,
      anchor: null,
      counts: { total: 0, applied: 0, flagged: 0, skippedEdited: 0, anchor: 0, failed: 0 },
      batchId: null,
    };
    runs.push(run);
    return run;
  }

  // ---- switches ----
  if (mode === "presets" || mode === "anchor" || mode === "1") {
    const gid = 700;
    const mk = (name: string, adj: (a: ParametricAdjustments) => ParametricAdjustments, fields: AdjustmentField[], keys: string[]): Preset => {
      const p: Preset = {
        id: ctx.nextPresetId(),
        name,
        adjustments: adj(ctx.neutral()),
        fields,
        createdAtMs: 0,
        updatedAtMs: 0,
        groupId: gid,
        sourceFormat: "xmp_preset",
        settingKeys: keys,
      };
      ctx.presets.push(p);
      return p;
    };
    const soft = mk(
      "Soft Film",
      (a) => ({ ...a, vibrance: 12, saturation: -6, hsl: { ...a.hsl, saturation: { ...a.hsl.saturation, orange: -15, blue: -20 } }, effects: { ...a.effects!, grain: { amount: 20, size: 25, roughness: 50 } } }),
      ["vibrance", "saturation", "hsl_saturation", "grain"],
      ["Vibrance", "Saturation", "SaturationAdjustmentOrange", "SaturationAdjustmentBlue", "GrainAmount", "GrainSize", "GrainFrequency"],
    );
    mk("Warm Matte", (a) => ({ ...a, clarity: -10, vibrance: 8, exposure: 0.2 }), ["clarity", "vibrance", "exposure"], ["Clarity2012", "Vibrance", "Exposure2012"]);
    mk("Clean B&W", (a) => ({ ...a, blackAndWhite: { ...a.blackAndWhite!, enabled: true } }), ["black_and_white"], ["ConvertToGrayscale"]);
    ctx.styleGroups.push({
      id: gid,
      name: "Wedding Looks",
      kind: "imported",
      sourcePath: "/Users/mock/Presets/Wedding Looks",
      importedAtMs: 0,
      presets: ctx.presets
        .filter((p) => p.groupId === gid)
        .map((p) => ({ id: p.id, groupId: gid, name: p.name, sourceFormat: "xmp_preset", sourcePath: `/Users/mock/Presets/Wedding Looks/${p.name}.xmp`, fields: p.fields, settingKeys: p.settingKeys, supportsAmount: false, warnings: [] })),
      profiles: [],
    });
    if (mode !== "presets") {
      const anchorRow = projectRows(1).find(ctx.keeper);
      if (anchorRow) {
        const auto = mockAutoLight(anchorRow.id);
        const withPreset = ctx.copyFields(ctx.getAdj(anchorRow.id), soft.adjustments, soft.fields);
        ctx.commitUser(anchorRow.id, withPreset, `Preset: ${soft.name}`);
        const tweaked = applyLight(withPreset, { ...auto, exposure: round(auto.exposure + 0.3, 0.05), temperatureK: auto.temperatureK + 400, tint: auto.tint + 2 });
        ctx.commitUser(anchorRow.id, tweaked, "Exposure");
        if (mode === "1") {
          const s: BaselineSettings = { anchorId: anchorRow.id, presetId: soft.id, scope: { kind: "keepers" }, replaceEdited: false };
          store(begin(1, s), 1);
        }
      }
    }
  }

  function preview(projectId: number, settings: BaselineSettings, options: BaselinePreviewOptions | null): BaselinePreview {
    const s = resolve(projectId, settings);
    const n = options?.sampleCount ?? DEFAULT_BASELINE_SAMPLES;
    if (n < 1 || n > MAX_BASELINE_SAMPLES) throw { kind: "invalid_argument", message: `sampleCount must be 1..=${MAX_BASELINE_SAMPLES}` };
    const rs = scopeRows(projectId, s);
    let sample: RawImageEntry[];
    if (options?.imageIds) {
      sample = options.imageIds.map((id) => {
        const r = rs.find((x) => x.id === id);
        if (!r) throw { kind: "invalid_argument", message: `image ${id} is not in the baseline's scope` };
        return r;
      });
    } else sample = pickSamples(rs.filter((r) => r.id !== s.anchorId && (s.replaceEdited || stateOf(r.id) !== "edited")), n);
    const look = lookOf(s);
    const anchor = anchorOf(s, look);
    const samples: BaselineSample[] = sample.map((r) => {
      const p = plan(r, s, look, anchor);
      const before = ctx.getAdj(r.id);
      return { photo: p.result, before, after: p.next ?? before };
    });
    return { projectId, settings: s, anchor, samples, counts: counts(rs, s), engineVersion: ENGINE };
  }

  function run(projectId: number, settings: BaselineSettings): BaselineRun {
    if (running) throw { kind: "invalid_argument", message: "A baseline edit is already running" };
    const s = resolve(projectId, settings);
    ctx.guardWrite();
    const r = begin(projectId, s);
    const me = { runId: r.id, cancelled: false };
    running = me;
    const total = scopeRows(projectId, s).length;
    const id = ++activitySeq;
    const send = (done: number, state: ActivityEvent["state"], message: string | null = null) =>
      void emit("activity-event", { id, kind: "baseline_edit", label: "Baseline edit", done, total, state, message } satisfies ActivityEvent);
    send(0, "running");
    const delay = window.__mockBaselineDelay ?? 300;
    setTimeout(() => send(Math.floor(total / 2), "running"), delay / 2);
    setTimeout(() => {
      running = null;
      if (me.cancelled) {
        Object.assign(r, { state: "cancelled", finishedAtMs: Date.now(), message: "Stopped; nothing was changed" });
        send(0, "cancelled", r.message);
      } else {
        store(r, projectId);
        send(total, "finished", r.message);
      }
      void emit("baseline-run-finished", { run: dto(r) } satisfies BaselineRunFinished);
    }, delay);
    return dto(r);
  }

  function provenanceOf(ids: number[]): BaselineProvenance[] {
    const unknown = ids.find((i) => !ctx.byId.has(i));
    if (unknown != null) throw { kind: "not_found", message: `image ${unknown}` };
    return ids.flatMap((id) => {
      const p = prov.get(id);
      if (!p) return [];
      const state = provState(id, p);
      return [{ imageId: id, runId: p.runId, batchId: p.batchId, anchorId: p.anchorId, presetId: p.presetId, appliedAtMs: p.appliedAtMs, state, flagged: p.flagged }];
    });
  }

  /** `ImageQuery.baselineOutcomes` (Rust `repo::query_filter`). */
  const queryOk = (id: number, q: Pick<ImageQuery, "baselineOutcomes">) => {
    if (!q.baselineOutcomes?.length) return true;
    const r = ctx.byId.get(id);
    const pid = r ? ctx.projectOf(r) : null;
    const res = pid != null ? results.get(pid)?.find((x) => x.imageId === id) : undefined;
    return !!res && q.baselineOutcomes.includes(res.outcome);
  };

  function handle(cmd: string, args: Record<string, unknown>): unknown {
    switch (cmd) {
      case "preview_baseline":
        return preview(args.projectId as number, args.settings as BaselineSettings, (args.options as BaselinePreviewOptions | null) ?? null);
      case "run_baseline":
        return run(args.projectId as number, args.settings as BaselineSettings);
      case "cancel_baseline":
        if (running) running.cancelled = true;
        return null;
      case "get_baseline_run": {
        const pid = args.projectId as number;
        ctx.requireProject(pid);
        const r = [...runs].reverse().find((x) => x.projectId === pid);
        return r ? dto(r) : null;
      }
      case "get_baseline_results": {
        const pid = args.projectId as number;
        ctx.requireProject(pid);
        const outcomes = (args.outcomes as BaselineOutcome[] | null) ?? [];
        const latest = [...runs].reverse().find((x) => x.projectId === pid && x.state === "finished");
        return (results.get(pid) ?? [])
          .filter((r) => !outcomes.length || outcomes.includes(r.outcome))
          .map((r) => {
            const p = prov.get(r.imageId);
            return { ...r, state: p && latest && p.runId === latest.id ? provState(r.imageId, p) : null };
          });
      }
      case "auto_light": {
        const id = args.id as number;
        if (!ctx.byId.has(id)) throw { kind: "not_found", message: `image ${id}` };
        return { light: autoLightOf(id), whiteBalanceEstimated: true } satisfies AutoLightValues;
      }
      case "get_baseline_provenance":
        return provenanceOf(args.ids as number[]);
      default:
        return NOT_BASELINE;
    }
  }

  /** The project's latest run while finished with a batch that is not undone (Rust `db::baseline::live_anchor`). */
  function liveRun(projectId: number): { run: Run; batch: EditBatchInfo } | null {
    const r = [...runs].reverse().find((x) => x.projectId === projectId);
    if (!r || r.state !== "finished" || r.batchId == null) return null;
    const batch = ctx.batchInfo(r.batchId);
    return batch && batch.undoneAtMs == null ? { run: r, batch } : null;
  }
  const liveAnchor = (projectId: number | null): number | null => (projectId == null ? null : (liveRun(projectId)?.run.settings.anchorId ?? null));

  /** Rust `db::baseline::plan_baseline` (`EditPlan.baseline`). */
  function planBaseline(projectId: number, keeperStates: ImageEditState[]): EditPlanBaseline | null {
    const live = liveRun(projectId);
    if (!live) return null;
    const { run: r, batch } = live;
    const on = keeperStates.filter((s) => s.editSource === "baseline");
    const editedSince = keeperStates.filter((s) => {
      const p = prov.get(s.imageId);
      return !!p && p.runId === r.id && provState(s.imageId, p) === "user_edited";
    }).length;
    return {
      runId: r.id,
      batch,
      anchorId: r.settings.anchorId,
      presetId: r.settings.presetId,
      keepers: keeperStates.length,
      onBaseline: on.length,
      needsLook: on.filter((s) => s.needsReview).length,
      editedSince,
    };
  }

  return { handle, queryOk, planBaseline, liveAnchor };
}
