// IPC v21 (docs/ipc-changelog.md v21): baseline edit in the mock backend (`src/testing/mockBaseline.ts`), which mirrors
// `db::baseline` (scope, skip / replace, results, provenance, one `baseline` batch) with a synthetic engine. Invoke-level.
import { expect, test, type Page } from "@playwright/test";
import { openHome } from "./helpers";

const inv = <T = unknown>(page: Page, cmd: string, args: Record<string, unknown> = {}) =>
  page.evaluate(
    ([c, a]) =>
      (window as unknown as { __TAURI_INTERNALS__: { invoke: (c: string, a: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke(c as string, a),
    [cmd, args] as const,
  ) as Promise<T>;

/** The error a command rejects with (`null` when it resolves). */
const invErr = (page: Page, cmd: string, args: Record<string, unknown> = {}) =>
  page.evaluate(
    async ([c, a]) => {
      try {
        await (window as unknown as { __TAURI_INTERNALS__: { invoke: (c: string, a: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke(c as string, a);
        return null;
      } catch (e) {
        return e as { kind: string; message: string };
      }
    },
    [cmd, args] as const,
  );

type Light = { exposure: number; temperatureK: number; tint: number };
type Result = { imageId: number; outcome: string; reasons: { kind: string; text: string }[]; light: Light | null; auto: Light | null };
type Run = {
  id: number;
  state: string;
  settings: { anchorId: number; presetId: number | null };
  counts: { total: number; applied: number; flagged: number; skippedEdited: number; anchor: number; failed: number };
  anchor: { imageId: number; offset: { exposure: number; temperatureMired: number } } | null;
  batch: { batchId: number; kind: string; undoable: boolean } | null;
  message: string | null;
};
type Adj = { exposure: number; vibrance: number; effects: { grain: { amount: number } }; crop: { enabled: boolean } };
type Prov = { imageId: number; state: string; flagged: boolean };
type StyleLib = { groups: { name: string; presets: { id: number; name: string }[] }[] };

const query = (extra: Record<string, unknown>) => ({
  includeTags: [],
  excludeTags: [],
  tagMatch: "any",
  picks: [],
  minRating: null,
  maxRating: null,
  colorLabels: [],
  burstGroupId: null,
  folderId: null,
  projectId: 1,
  sort: "capture_time",
  sortDescending: false,
  offset: 0,
  limit: 1000,
  ...extra,
});

test.describe("IPC v21 mock contract", () => {
  test("?baseline=1: presets, anchor, finished run with results, provenance, flagged filter, batch undo", async ({ page }) => {
    await openHome(page, 201, "&baseline=1");
    const styles = await inv<StyleLib>(page, "list_styles");
    const looks = styles.groups.find((g) => g.name === "Wedding Looks")!;
    expect(looks.presets.map((p) => p.name)).toEqual(["Soft Film", "Warm Matte", "Clean B&W"]);

    const run = (await inv<Run>(page, "get_baseline_run", { projectId: 1 }))!;
    expect(run.state).toBe("finished");
    expect(run.settings.presetId).toBe(looks.presets[0].id);
    expect(run.batch).toMatchObject({ kind: "baseline", undoable: true });
    const c = run.counts;
    expect(c.total).toBe(c.applied + c.flagged + c.skippedEdited + c.anchor + c.failed);
    expect(c.anchor).toBe(1);
    expect(c.applied).toBeGreaterThan(0);
    expect(c.flagged).toBeGreaterThan(0);
    expect(run.anchor!.offset.exposure).toBeCloseTo(0.3, 5);
    expect(run.anchor!.offset.temperatureMired).toBeLessThan(0);

    const results = await inv<Result[]>(page, "get_baseline_results", { projectId: 1, outcomes: null });
    expect(results.length).toBe(c.total);
    const flagged = await inv<Result[]>(page, "get_baseline_results", { projectId: 1, outcomes: ["flagged"] });
    expect(flagged.length).toBe(c.flagged);
    expect(flagged.every((r) => r.reasons.length > 0)).toBe(true);
    expect(new Set(flagged.map((r) => r.reasons[0].kind))).toEqual(new Set(["low_key", "auto_failed"]));

    // Look copied from the anchor; light = Auto + offset (per photo, not the anchor's value).
    const anchorAdj = await inv<Adj>(page, "get_adjustments", { id: run.settings.anchorId });
    const applied = results.filter((r) => r.outcome === "applied").slice(0, 2);
    const adjs = await Promise.all(applied.map((r) => inv<Adj>(page, "get_adjustments", { id: r.imageId })));
    for (const [i, a] of adjs.entries()) {
      expect(a.vibrance).toBe(anchorAdj.vibrance);
      expect(a.effects.grain.amount).toBe(anchorAdj.effects.grain.amount);
      expect(a.exposure).toBe(applied[i].light!.exposure);
      expect(applied[i].light!.exposure).toBeCloseTo(applied[i].auto!.exposure + 0.3, 5);
    }

    // Grid filter + edit states: flagged photos need a look.
    const ids = await inv<number[]>(page, "list_image_ids", { query: query({ baselineOutcomes: ["flagged"] }) });
    expect(ids.sort()).toEqual(flagged.map((r) => r.imageId).sort());
    const states = await inv<{ editSource: string; needsReview: boolean }[]>(page, "get_edit_states", { imageIds: [flagged[0].imageId, applied[0].imageId] });
    expect(states[0]).toMatchObject({ editSource: "baseline", needsReview: true });
    expect(states[1]).toMatchObject({ editSource: "baseline", needsReview: false });

    // Provenance: a user edit after the baseline -> user_edited.
    const [p0] = await inv<Prov[]>(page, "get_baseline_provenance", { ids: [applied[1].imageId] });
    expect(p0.state).toBe("on_baseline");
    await inv(page, "save_adjustments", { id: applied[1].imageId, adjustments: { ...adjs[1], exposure: 1.5 }, label: "Exposure" });
    expect((await inv<Prov[]>(page, "get_baseline_provenance", { ids: [applied[1].imageId] }))[0].state).toBe("user_edited");
    expect(await inv<Prov[]>(page, "get_baseline_provenance", { ids: [run.settings.anchorId] })).toEqual([]);

    // A re-run skips the user-edited photo; the replaced provenance points at the new run.
    const rerun = await inv<Run>(page, "run_baseline", { projectId: 1, settings: { ...run.settings, scope: { kind: "keepers" }, replaceEdited: false } });
    expect(rerun.state).toBe("running");
    await expect.poll(async () => (await inv<Run>(page, "get_baseline_run", { projectId: 1 })).state).toBe("finished");
    const res2 = await inv<Result[]>(page, "get_baseline_results", { projectId: 1, outcomes: null });
    expect(res2.find((r) => r.imageId === applied[1].imageId)!.outcome).toBe("skipped_edited");
    expect(res2.find((r) => r.imageId === applied[0].imageId)!.outcome).toBe("applied");

    // Undo the re-run's batch: one call takes it back.
    const latest = await inv<Run>(page, "get_baseline_run", { projectId: 1 });
    if (latest.batch) {
      await inv(page, "undo_edit_batch", { batchId: latest.batch.batchId });
      expect((await inv<Run>(page, "get_baseline_run", { projectId: 1 })).batch!.undoable).toBe(false);
    }
  });

  test("?baseline=anchor: preview spread across scenes writes nothing; run + cancel; errors", async ({ page }) => {
    await openHome(page, 201, "&baseline=anchor");
    expect(await inv(page, "get_baseline_run", { projectId: 1 })).toBeNull();
    const looks = (await inv<StyleLib>(page, "list_styles")).groups.find((g) => g.name === "Wedding Looks")!;
    const keepers = await inv<number[]>(page, "list_image_ids", { query: query({ keepersOnly: true }) });
    const anchorId = keepers[0];
    const settings = { anchorId, presetId: looks.presets[0].id, scope: { kind: "keepers" }, replaceEdited: false };

    const pv = await inv<{
      samples: { photo: Result; before: Adj; after: Adj }[];
      counts: { inScope: number; toWrite: number; onBaseline: number; edited: number };
      anchor: { imageId: number };
    }>(page, "preview_baseline", { projectId: 1, settings, options: null });
    expect(pv.anchor.imageId).toBe(anchorId);
    expect(pv.samples.length).toBe(12);
    expect(pv.samples.some((s) => s.photo.imageId === anchorId)).toBe(false);
    expect(pv.counts.inScope).toBe(keepers.length);
    expect(pv.counts.toWrite).toBe(keepers.length - 1);
    expect(pv.samples.every((s) => JSON.stringify(s.before) !== JSON.stringify(s.after))).toBe(true);
    const states = await inv<{ editSource: string }[]>(page, "get_edit_states", { imageIds: pv.samples.map((s) => s.photo.imageId) });
    expect(states.every((s) => s.editSource === "none")).toBe(true);
    const pinned = await inv<{ samples: { photo: Result }[] }>(page, "preview_baseline", { projectId: 1, settings, options: { sampleCount: 12, imageIds: [keepers[3]] } });
    expect(pinned.samples.map((s) => s.photo.imageId)).toEqual([keepers[3]]);

    // Errors.
    expect(await invErr(page, "preview_baseline", { projectId: 1, settings: { ...settings, presetId: 9999 }, options: null })).toMatchObject({ kind: "not_found" });
    expect(await invErr(page, "preview_baseline", { projectId: 1, settings, options: { sampleCount: 0 } })).toMatchObject({ kind: "invalid_argument" });
    expect(await invErr(page, "preview_baseline", { projectId: 1, settings: { ...settings, anchorId: 999999 }, options: null })).toMatchObject({ kind: "not_found" });

    // Run + cancel: nothing written.
    await page.evaluate(() => ((window as unknown as { __mockBaselineDelay: number }).__mockBaselineDelay = 400));
    const r = await inv<Run>(page, "run_baseline", { projectId: 1, settings });
    expect(r.state).toBe("running");
    expect(await invErr(page, "run_baseline", { projectId: 1, settings })).toMatchObject({ kind: "invalid_argument" });
    await inv(page, "cancel_baseline");
    await expect.poll(async () => (await inv<Run>(page, "get_baseline_run", { projectId: 1 })).state).toBe("cancelled");
    const after = await inv<{ editSource: string }[]>(page, "get_edit_states", { imageIds: keepers.slice(1, 6) });
    expect(after.every((s) => s.editSource === "none")).toBe(true);

    // A real run.
    await inv(page, "run_baseline", { projectId: 1, settings: { ...settings, scope: { kind: "selection", ids: keepers.slice(0, 6) } } });
    await expect.poll(async () => (await inv<Run>(page, "get_baseline_run", { projectId: 1 })).state).toBe("finished");
    const done = await inv<Run>(page, "get_baseline_run", { projectId: 1 });
    expect(done.counts.total).toBe(6);
    expect(done.counts.anchor).toBe(1);
    expect(done.message).toMatch(/^Edited \d+ photos?/);
  });

  test("v21.1: auto_light = the engine's Auto; scenes on the baseline; plan summary; undo keeping later edits; live counts", async ({ page }) => {
    await openHome(page, 201, "&baseline=1");
    type LiveRun = Run & { live: { written: number; onBaseline: number; needsLook: number; userEdited: number; undone: number }; anchor: { imageId: number; auto: Light } };
    const run = (await inv<LiveRun>(page, "get_baseline_run", { projectId: 1 }))!;
    const anchorId = run.settings.anchorId;

    // P0-1: one Auto. auto_light on the anchor = the run's (and a preview's) Auto of the anchor.
    const al = await inv<{ light: Light; whiteBalanceEstimated: boolean }>(page, "auto_light", { id: anchorId, adjustments: null });
    expect(al.whiteBalanceEstimated).toBe(true);
    expect(al.light).toEqual(run.anchor.auto);
    const pv = await inv<{ anchor: { auto: Light } }>(page, "preview_baseline", { projectId: 1, settings: { ...run.settings, scope: { kind: "keepers" }, replaceEdited: false }, options: { sampleCount: 1 } });
    expect(pv.anchor.auto).toEqual(al.light);
    expect(await invErr(page, "auto_light", { id: 999999, adjustments: null })).toMatchObject({ kind: "not_found" });

    // Live counts of the finished run.
    expect(run.live.written).toBe(run.counts.applied + run.counts.flagged);
    expect(run.live.onBaseline).toBe(run.live.written);
    expect(run.live.needsLook).toBe(run.counts.flagged);

    // P0-2: every scene is on the baseline (representatives written by it, or the anchor); nothing to apply.
    type Plan = {
      keeperIds: number[];
      scenes: { sceneId: number; status: string; representativeId: number; baselineIds: number[]; imageIds: number[] }[];
      counts: { onBaseline: number; edited: number; applied: number; toEdit: number };
      editStates: { imageId: number; editSource: string; needsReview: boolean }[];
      baseline: { runId: number; anchorId: number; keepers: number; onBaseline: number; needsLook: number; editedSince: number; batch: { batchId: number } } | null;
    };
    await inv(page, "detect_scenes", { folderId: null, projectId: 1, options: null });
    let plan = await inv<Plan>(page, "get_edit_plan", { projectId: 1 });
    expect(plan.scenes.length).toBeGreaterThan(0);
    expect(plan.scenes.every((s) => s.status === "on_baseline")).toBe(true);
    expect(plan.counts).toMatchObject({ onBaseline: plan.scenes.length, edited: 0, applied: 0, toEdit: 0 });
    const onBaseline = plan.editStates.filter((s) => s.editSource === "baseline").length;
    expect(plan.baseline).toMatchObject({ runId: run.id, anchorId, keepers: plan.keeperIds.length, onBaseline, needsLook: run.counts.flagged, editedSince: 0 });
    expect(onBaseline).toBe(plan.keeperIds.length - 1);
    expect(plan.scenes.reduce((n, s) => n + s.baselineIds.length, 0)).toBe(onBaseline);

    // A representative edited after the baseline: its scene is "edited" again (refine scene by scene).
    const sc = plan.scenes.find((s) => s.representativeId !== anchorId)!;
    const repAdj = await inv<Adj>(page, "get_adjustments", { id: sc.representativeId });
    await inv(page, "save_adjustments", { id: sc.representativeId, adjustments: { ...repAdj, exposure: repAdj.exposure + 0.5 }, label: "Exposure" });
    plan = await inv<Plan>(page, "get_edit_plan", { projectId: 1 });
    const sc2 = plan.scenes.find((s) => s.sceneId === sc.sceneId)!;
    expect(sc2.status).toBe("edited");
    expect(sc2.baselineIds).not.toContain(sc.representativeId);
    expect(plan.baseline).toMatchObject({ onBaseline: onBaseline - 1, editedSince: 1 });

    // P1-2: linear undo refuses; "Undo the rest" restores the others and keeps the edit.
    const batchId = run.batch!.batchId;
    expect(await invErr(page, "undo_edit_batch", { batchId, options: null })).toMatchObject({ kind: "conflict" });
    const u = await inv<{ restoredIds: number[]; skippedIds: number[]; keptIds: number[] }>(page, "undo_edit_batch", { batchId, options: { keepLaterEdits: true } });
    expect(u.keptIds).toEqual([sc.representativeId]);
    expect(u.restoredIds.length).toBe(run.live.written - 1);
    expect((await inv<Adj>(page, "get_adjustments", { id: sc.representativeId })).exposure).toBeCloseTo(repAdj.exposure + 0.5, 5);
    const after = (await inv<LiveRun>(page, "get_baseline_run", { projectId: 1 }))!;
    expect(after.batch).toMatchObject({ undoable: false });
    expect(after.live).toEqual({ written: run.live.written, onBaseline: 0, needsLook: 0, userEdited: 1, undone: run.live.written - 1 });
    expect(after.message).toBe(`Undone on ${run.live.written - 1} photos; 1 photo you changed since was kept`);
    const prov = await inv<Prov[]>(page, "get_baseline_provenance", { ids: [sc.representativeId, u.restoredIds[0]] });
    expect(prov.map((p) => p.state)).toEqual(["user_edited", "undone"]);
    const res = await inv<(Result & { state: string | null })[]>(page, "get_baseline_results", { projectId: 1, outcomes: null });
    expect(res.find((r) => r.imageId === sc.representativeId)!.state).toBe("user_edited");
    expect(res.find((r) => r.imageId === u.restoredIds[0])!.state).toBe("undone");
    expect(res.find((r) => r.imageId === anchorId)!.state).toBeNull();
    plan = await inv<Plan>(page, "get_edit_plan", { projectId: 1 });
    expect(plan.baseline).toBeNull();
    expect(plan.scenes.some((s) => s.status === "on_baseline")).toBe(false);
    expect(await invErr(page, "undo_edit_batch", { batchId, options: { keepLaterEdits: true } })).toMatchObject({ kind: "invalid_argument" });
  });

  test("v21.1: a plain undo of a run reads undone (no stale summary)", async ({ page }) => {
    await openHome(page, 201, "&baseline=1");
    const run = (await inv<Run & { live: { written: number } }>(page, "get_baseline_run", { projectId: 1 }))!;
    const u = await inv<{ restoredIds: number[]; keptIds: number[] }>(page, "undo_edit_batch", { batchId: run.batch!.batchId, options: null });
    expect(u.keptIds).toEqual([]);
    const after = (await inv<Run & { live: { undone: number; onBaseline: number } }>(page, "get_baseline_run", { projectId: 1 }))!;
    expect(after.message).toBe("Undone: the photos are back to how they were");
    expect(after.live).toMatchObject({ undone: run.live.written, onBaseline: 0 });
  });

  test("partition constants are exported", async ({ page }) => {
    await openHome(page, 20, "");
    const fromBindings = await page.evaluate(async () => {
      const m = await import("/src/ipc/bindings.ts");
      return { light: m.BASELINE_LIGHT_FIELDS, never: m.BASELINE_NEVER_FIELDS, rows: m.BASELINE_PARTITION.length, label: m.BASELINE_LABEL };
    });
    expect(fromBindings.light).toEqual(["white_balance", "exposure", "contrast", "highlights", "shadows", "whites", "blacks"]);
    expect(fromBindings.never).toEqual(["crop", "masks", "transform"]);
    expect(fromBindings.rows).toBe(31);
    expect(fromBindings.label).toBe("Baseline Edit");
  });
});
