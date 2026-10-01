// IPC v17 (docs/ipc-changelog.md v17, docs/ux-review-8b.md "Re-check 2" P1-12): the mock backend emulates the
// contract the same way as the Rust tests in src-tauri/src/scene/workflow.rs. Invoke-level only (no UI assertions).
// Mock rules: project 1 = ids 1..101, keepers in scenes 1, 2, 3; `style=ready` enables Auto edit.
import { expect, test, type Page } from "@playwright/test";
import { openHome } from "./helpers";

type Entry = { sceneId: number; representativeId: number; status: string; edited: boolean; unappliedKeeperIds: number[]; appliedBatch: { batchId: number; undoable: boolean } | null };
type Plan = { scenes: Entry[]; counts: { toEdit: number; edited: number; outdated: number; reset: number }; latestBatch: { batchId: number; undoable: boolean } | null };
type Err = { kind: string; message: string };

const inv = <T>(page: Page, cmd: string, args: Record<string, unknown>) =>
  page.evaluate(([c, a]) => (window as unknown as { __TAURI_INTERNALS__: { invoke: (c: string, a: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke(c as string, a), [cmd, args] as const) as Promise<T>;
const fail = (page: Page, cmd: string, args: Record<string, unknown>) =>
  page.evaluate(
    ([c, a]) =>
      (window as unknown as { __TAURI_INTERNALS__: { invoke: (c: string, a: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke(c as string, a).then(
        () => null,
        (e: unknown) => e,
      ),
    [cmd, args] as const,
  ) as Promise<Err | null>;
const plan = (page: Page) => inv<Plan>(page, "get_edit_plan", { projectId: 1 });

test.describe("IPC v17 mock contract", () => {
  test.beforeEach(async ({ page }) => {
    // Scenes exist once the Edit step has opened (the mock detects them on the way in).
    await openHome(page, 201, "&style=ready");
    await page.getByTestId("project-open-1").click();
    await expect(page.getByTestId("grid-toolbar")).toBeVisible();
    await page.getByTestId("continue-edit").click();
    await expect(page.locator('article[data-testid^="plan-scene-"]').first()).toBeVisible();
    expect((await plan(page)).scenes).toHaveLength(3);
  });

  test("apply from an auto edit blocks the auto edit's undo until the apply is undone (repro a)", async ({ page }) => {
    const reps = (await plan(page)).scenes.map((s) => s.representativeId);
    const auto = (await inv<{ batchId: number }>(page, "apply_style_prediction", { imageIds: reps })).batchId;
    const apply = (await inv<{ batch: { batchId: number }; skippedScenes: unknown[] }>(page, "apply_scene_edit", { sceneId: (await plan(page)).scenes[0].sceneId, options: null }));
    expect(apply.skippedScenes).toEqual([]);
    expect(await fail(page, "undo_edit_batch", { batchId: auto })).toEqual({ kind: "conflict", message: "A scene was applied from this edit since; undo that apply first" });
    const [info] = await inv<{ undoable: boolean; conflictCount: number }[]>(page, "get_edit_batches", { batchIds: [auto] });
    expect(info).toMatchObject({ undoable: false, conflictCount: 1 });
    expect((await plan(page)).scenes[0].status).toBe("applied");
    await inv(page, "undo_edit_batch", { batchId: apply.batch.batchId });
    expect(await fail(page, "undo_edit_batch", { batchId: auto })).toBeNull();
    expect((await plan(page)).scenes.map((s) => s.status)).toEqual(["to_edit", "to_edit", "to_edit"]);
  });

  test("a reset representative is a to-do scene; apply all applies the others (repro b)", async ({ page }) => {
    const p0 = await plan(page);
    const reps = p0.scenes.map((s) => s.representativeId);
    await inv(page, "apply_style_prediction", { imageIds: [reps[0]] });
    const apply = await inv<{ batch: { batchId: number } }>(page, "apply_scene_edit", { sceneId: p0.scenes[0].sceneId, options: null });
    await inv(page, "reset_adjustments", { ids: [reps[0]] });
    let p = await plan(page);
    expect(p.scenes[0]).toMatchObject({ status: "reset", edited: false, unappliedKeeperIds: [], appliedBatch: { batchId: apply.batch.batchId, undoable: true } });
    expect(p.counts).toMatchObject({ toEdit: 3, reset: 1, edited: 0, outdated: 0 });
    expect(await fail(page, "apply_scene_edit", { sceneId: p0.scenes[0].sceneId, options: null })).toEqual({
      kind: "invalid_argument",
      message: "Scene 1: its representative was reset after the last apply. Edit it first, then apply.",
    });
    expect(await fail(page, "apply_scene_edit", { sceneId: p0.scenes[1].sceneId, options: null })).toEqual({
      kind: "invalid_argument",
      message: "Scene 2: edit its representative first, then apply.",
    });
    await inv(page, "apply_style_prediction", { imageIds: reps.slice(1) });
    expect((await plan(page)).counts).toMatchObject({ edited: 2, toEdit: 1, reset: 1 });
    const all = await inv<{ scenes: { sceneId: number }[]; skippedScenes: unknown[] }>(page, "apply_all_edited_scenes", { projectId: 1, options: null });
    expect(all.scenes.map((s) => s.sceneId)).toEqual([p0.scenes[1].sceneId, p0.scenes[2].sceneId]);
    expect(all.skippedScenes).toEqual([]);
    p = await plan(page);
    expect(p.scenes.map((s) => s.status)).toEqual(["reset", "applied", "applied"]);
    // Scene 1's apply is still undoable; undoing it makes the scene plain "to edit".
    await inv(page, "undo_edit_batch", { batchId: apply.batch.batchId });
    expect((await plan(page)).scenes[0]).toMatchObject({ status: "to_edit", appliedBatch: null });
  });

  test("apply all reports a scene whose matching fails and applies the rest", async ({ page }) => {
    const p0 = await plan(page);
    await inv(page, "apply_style_prediction", { imageIds: p0.scenes.map((s) => s.representativeId) });
    const failing = p0.scenes[1].sceneId;
    await page.evaluate((id) => ((window as unknown as { __mockApplyFailScenes: number[] }).__mockApplyFailScenes = [id]), failing);
    const all = await inv<{ scenes: { sceneId: number }[]; skippedScenes: { sceneId: number; reason: string; message: string }[] }>(page, "apply_all_edited_scenes", {
      projectId: 1,
      options: null,
    });
    expect(all.scenes.map((s) => s.sceneId)).toEqual([p0.scenes[0].sceneId, p0.scenes[2].sceneId]);
    expect(all.skippedScenes).toHaveLength(1);
    expect(all.skippedScenes[0]).toMatchObject({ sceneId: failing, reason: "failed" });
    expect(all.skippedScenes[0].message).toMatch(/^Scene 2: .+ is missing$/);
    const err = await fail(page, "apply_scene_edit", { sceneId: failing, options: null });
    expect(err?.kind).toBe("file_missing");
    expect(err?.message).toMatch(/^Scene 2: /);
  });
});
