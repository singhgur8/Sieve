import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, closeMenus, detectScenes, openApp, sceneItem, shot } from "./helpers";

// Mock rule (src/testing/mockBackend.ts): detect_scenes chunks each folder into scenes of 40.
// 200 photos -> scenes 1..6 = ids 1-40, 41-80, 81-100, 101-140, 141-180, 181-200.
async function detect(page: Page) {
  await detectScenes(page);
  await expect(page.getByTestId("scene-chip-6")).toBeVisible();
}

async function setup(page: Page) {
  await openApp(page, 200);
  await detect(page);
}

test.describe("scenes", () => {
  test("detect scenes shows progress, lists chips, badges cells and filters the grid", async ({ page }) => {
    await page.addInitScript(() => (window.__mockSceneDelay = 300));
    await openApp(page, 200);
    await expect(page.getByTestId("scene-strip")).toHaveCount(0); // hidden until scenes exist
    await detectScenes(page);
    await expect(page.getByTestId("scene-progress")).toBeVisible();
    await expect(page.getByTestId("scene-chip-6")).toBeVisible();
    await expect(page.getByTestId("scene-progress")).toHaveCount(0);
    expect((await calls(page, "detect_scenes"))[0].args).toEqual({ folderId: null, projectId: null, options: null });
    await expect(page.getByTestId("scene-chips").locator("button")).toHaveCount(7);
    await expect(page.getByTestId("scene-chip-1")).toContainText("Scene 1");
    await expect(page.getByTestId("scene-chip-1")).toContainText("40");
    await expect(page.getByTestId("scene-badge-1")).toHaveText("S1");
    await expect(page.getByTestId("scene-badge-1")).toHaveAttribute("data-anchor", "false");

    await clearCalls(page);
    await page.getByTestId("scene-chip-2").click();
    await expect(page.getByTestId("selection-count")).toContainText("40 of 200");
    await expect(page.getByTestId("cell-41")).toBeVisible();
    await expect(page.getByTestId("cell-1")).toHaveCount(0);
    const q = (await calls(page, "list_image_ids")).at(-1)!.args.query as { sceneId: number };
    expect(q.sceneId).toBe(2);
    await shot(page, "4x-scenes-01-strip-filter");

    await page.getByTestId("scene-chip-all").click();
    await expect(page.getByTestId("selection-count")).toContainText("200 photos");
    // The strip also shows in Develop, with filmstrip badges.
    await page.getByTestId("cell-3").click();
    await page.keyboard.press("d");
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await expect(page.getByTestId("scene-strip")).toBeVisible();
    await expect(page.getByTestId("film-scene-3")).toHaveText("S1");
    await shot(page, "4x-scenes-02-develop-strip");
  });

  test("anchors: Shift+A toggles, at most two, oldest replaced with a notice", async ({ page }) => {
    await setup(page);
    await expect(await sceneItem(page, "scene-anchor")).toBeDisabled();
    await closeMenus(page);
    await page.getByTestId("cell-3").click();
    await clearCalls(page);
    await page.keyboard.press("Shift+A");
    await expect(page.getByTestId("scene-badge-3")).toHaveAttribute("data-anchor", "true");
    expect((await calls(page, "set_scene_anchors")).at(-1)!.args).toEqual({ id: 1, anchorIds: [3] });
    await expect(page.getByTestId("scene-anchors-1")).toContainText("1");

    await page.getByTestId("cell-5").click();
    await (await sceneItem(page, "scene-anchor")).click();
    expect((await calls(page, "set_scene_anchors")).at(-1)!.args).toEqual({ id: 1, anchorIds: [3, 5] });
    await expect(page.getByTestId("scene-anchors-1")).toContainText("2");

    await page.getByTestId("cell-7").click();
    await page.keyboard.press("Shift+A");
    expect((await calls(page, "set_scene_anchors")).at(-1)!.args).toEqual({ id: 1, anchorIds: [5, 7] });
    await expect(page.getByTestId("notice")).toContainText("at most 2 anchors");
    await expect(page.getByTestId("scene-badge-3")).toHaveAttribute("data-anchor", "false");
    await expect(page.getByTestId("scene-badge-7")).toHaveAttribute("data-anchor", "true");
    await shot(page, "4x-scenes-03-anchors");

    // Toggle off.
    await page.keyboard.press("Shift+A");
    expect((await calls(page, "set_scene_anchors")).at(-1)!.args).toEqual({ id: 1, anchorIds: [5] });
    await expect(page.getByTestId("scene-badge-7")).toHaveAttribute("data-anchor", "false");

    // A photo outside any scene cannot be an anchor.
    await page.getByTestId("scene-chip-2").click();
    await page.getByTestId("cell-50").click();
    await (await sceneItem(page, "scene-remove")).click();
    await page.getByTestId("scene-chip-all").click();
    await page.getByTestId("cell-50").click();
    await expect(page.getByTestId("scene-badge-50")).toHaveCount(0);
    await expect(await sceneItem(page, "scene-anchor")).toBeDisabled();
    await closeMenus(page);
    await page.keyboard.press("Shift+A");
    await expect(page.getByTestId("notice")).toContainText("before marking it as an anchor");
  });

  test("match: options, one solve, strength slider re-blends locally with lerp", async ({ page }) => {
    await setup(page);
    await expect(page.getByTestId("scene-match")).toBeDisabled();
    await page.getByTestId("cell-1").click();
    await page.keyboard.press("Shift+A");
    await expect(page.getByTestId("scene-badge-1")).toHaveAttribute("data-anchor", "true");
    await expect(page.getByTestId("scene-match")).toBeEnabled();
    await clearCalls(page);
    await page.getByTestId("scene-match").click();
    await expect(page.getByTestId("match-panel")).toBeVisible();
    await expect(page.getByTestId("match-summary")).toContainText("1 anchor");
    // Rejected photos are excluded by default (ids 8, 22, 36 in scene 1); the toggle brings them back.
    await expect(page.getByTestId("match-summary")).toContainText("36 targets");
    await expect(page.getByTestId("match-include-rejected")).toBeVisible();
    await expect(page.getByTestId("match-anchor-1")).toContainText("DSC00001.ARW");
    await expect(page.getByTestId("match-copy-fields")).toContainText("Also copy from anchor: All settings except crop");
    await page.getByTestId("match-include-rejected").check();
    await expect(page.getByTestId("match-summary")).toContainText("39 targets");
    await expect(page.getByTestId("match-exposure")).toBeChecked();
    await expect(page.getByTestId("match-wb")).toBeChecked();
    await expect(page.getByTestId("match-tone")).not.toBeChecked();
    await expect(page.getByTestId("match-apply")).toBeDisabled();

    await page.getByTestId("match-tone").check();
    await page.getByTestId("match-run").click();
    await expect(page.getByTestId("match-progress")).toBeVisible();
    await expect(page.getByTestId("match-card-3")).toBeVisible();
    const m = await calls(page, "match_scene");
    expect(m).toHaveLength(1);
    expect(m[0].args.anchorIds).toEqual([1]);
    expect((m[0].args.targetIds as number[]).length).toBe(39);
    expect(m[0].args.options).toMatchObject({ matchExposure: true, matchWhiteBalance: true, matchTone: true, strength: 1 });

    // Mock solve for target 3: full - base = +0.35 EV.
    const card = page.getByTestId("match-card-3");
    await expect(card).toHaveAttribute("data-base-exposure", "0");
    await expect(card).toHaveAttribute("data-full-exposure", "0.35");
    await expect(card).toHaveAttribute("data-predicted-exposure", "0.35");
    await expect(page.getByTestId("match-before-3")).toBeVisible();
    await expect(page.getByTestId("match-after-3")).toBeVisible();

    await clearCalls(page);
    await page.getByTestId("match-strength").fill("50");
    await expect(page.getByTestId("match-strength-value")).toHaveText("50%");
    await expect(card).toHaveAttribute("data-predicted-exposure", "0.175");
    await expect(page.getByTestId("match-delta-3")).toContainText("Exp +0.17 EV");
    await expect.poll(async () => {
      const r = (await calls(page, "render_preview")).filter((c) => c.args.id === 3 && (c.args.options as { slot: string }).slot === "main");
      return (r.at(-1)?.args.adjustments as { exposure: number } | undefined)?.exposure;
    }).toBeCloseTo(0.175, 6);
    expect(await calls(page, "match_scene")).toHaveLength(0);

    await page.getByTestId("match-strength").fill("0");
    await expect(card).toHaveAttribute("data-predicted-exposure", "0");
    expect(await calls(page, "match_scene")).toHaveLength(0);

    // Not-converged targets (ids divisible by 7) carry a warning and a note.
    await expect(page.getByTestId("match-warn-7")).toBeVisible();
    await expect(page.getByTestId("match-notes-7")).toContainText("clamped");
    await expect(page.getByTestId("match-unconverged-count")).toHaveText("5 not converged");
    await page.getByTestId("match-strength").fill("100");
    await shot(page, "4x-scenes-04-match-preview");
  });

  test("match apply sends the exact blended adjustments, respects deselection, and undoes", async ({ page }) => {
    await setup(page);
    await page.getByTestId("cell-1").click();
    await page.keyboard.press("Shift+A");
    await expect(page.getByTestId("scene-badge-1")).toHaveAttribute("data-anchor", "true");
    await page.getByTestId("scene-match").click();
    await page.getByTestId("match-include-rejected").check();
    await page.getByTestId("match-run").click();
    await expect(page.getByTestId("match-card-2")).toBeVisible();
    await expect(page.getByTestId("match-selected-count")).toHaveText("39 of 39 selected");

    await page.getByTestId("match-include-3").uncheck();
    await expect(page.getByTestId("match-selected-count")).toHaveText("38 of 39 selected");
    await page.getByTestId("match-deselect-unconverged").click();
    await expect(page.getByTestId("match-selected-count")).toHaveText("34 of 39 selected");
    await page.getByTestId("match-include-3").uncheck();
    await page.getByTestId("match-include-3").check();
    await page.getByTestId("match-include-2").uncheck();
    await page.getByTestId("match-strength").fill("50");
    await expect(page.getByTestId("match-apply")).toHaveText("Apply to 33 photos");

    await clearCalls(page);
    await page.getByTestId("match-apply").click();
    await expect(page.getByTestId("match-panel")).toHaveCount(0);
    const a = await calls(page, "apply_scene_match");
    expect(a).toHaveLength(1);
    expect(a[0].args.label).toBe("Match Scene");
    const apps = a[0].args.applications as { imageId: number; adjustments: { exposure: number; contrast: number; whiteBalance: { mode: string; temperatureK: number; tint: number } } }[];
    expect(apps).toHaveLength(33);
    const ids = apps.map((x) => x.imageId);
    expect(ids).not.toContain(2);
    expect(ids).not.toContain(7);
    expect(ids).toContain(3);
    // id 3: dEv 0.35, dT 150 (3%3=0 -> -300+150 = -150), tint (3%4)-1.5 = 1.5, contrast 0 (tone off).
    const t3 = apps.find((x) => x.imageId === 3)!.adjustments;
    expect(t3.exposure).toBeCloseTo(0.175, 6);
    expect(t3.whiteBalance.mode).toBe("custom");
    expect(t3.whiteBalance.temperatureK).toBeGreaterThan(0);
    await expect(page.getByTestId("notice")).toContainText("Applied Match Scene to 33 of 33 photos");
    await expect(page.getByTestId("match-undo")).toBeVisible();
    await expect(page.getByTestId("cell-3")).toBeVisible();

    // History entry per image via the existing history; Undo reverts them.
    const h = await page.evaluate(() => (window as unknown as { __TAURI_INTERNALS__: { invoke: (c: string, a: unknown) => Promise<{ entries: { label: string }[] }> } }).__TAURI_INTERNALS__.invoke("get_history", { id: 3 }));
    expect(h.entries.at(-1)!.label).toBe("Match Scene");
    await shot(page, "4x-scenes-05-applied");
    await clearCalls(page);
    await page.getByTestId("match-undo").click();
    await expect(page.getByTestId("match-undo")).toHaveCount(0);
    expect((await calls(page, "undo_adjustments")).length).toBe(33);
  });

  test("split, merge, new scene, remove and delete", async ({ page }) => {
    await setup(page);
    // Split at 10: scene 1 keeps 1-9, new scene 7 gets 10-40.
    await expect(await sceneItem(page, "scene-split")).toBeDisabled();
    await closeMenus(page);
    await page.getByTestId("cell-10").click();
    await clearCalls(page);
    await (await sceneItem(page, "scene-split")).click();
    await expect(page.getByTestId("scene-chip-7")).toBeVisible();
    expect((await calls(page, "split_scene"))[0].args).toEqual({ id: 1, firstImageId: 10 });
    await expect(page.getByTestId("scene-chip-1")).toContainText("9");
    await expect(page.getByTestId("scene-chip-7")).toContainText("31");
    await expect(page.getByTestId("scene-chip-7")).toHaveAttribute("data-method", "manual");
    await expect(page.getByTestId("scene-badge-10")).toHaveText("S7");
    // The first frame of a scene cannot be a split point.
    await page.getByTestId("cell-1").click();
    await expect(await sceneItem(page, "scene-split")).toBeDisabled();
    await closeMenus(page);

    // Merge the scenes of photo 1 (scene 1) and photo 10 (scene 7).
    await expect(await sceneItem(page, "scene-merge")).toBeDisabled();
    await closeMenus(page);
    await page.getByTestId("cell-10").click({ modifiers: ["Meta"] });
    await expect(await sceneItem(page, "scene-merge")).toBeEnabled();
    await closeMenus(page);
    await clearCalls(page);
    await (await sceneItem(page, "scene-merge")).click();
    await expect(page.getByTestId("scene-chip-7")).toHaveCount(0);
    expect((await calls(page, "merge_scenes"))[0].args).toEqual({ ids: [1, 7] });
    await expect(page.getByTestId("scene-chip-1")).toContainText("40");
    await expect(page.getByTestId("scene-badge-10")).toHaveText("S1");

    // New scene from 3 selected photos (moved out of scene 1).
    await page.getByTestId("cell-11").click();
    await page.getByTestId("cell-13").click({ modifiers: ["Shift"] });
    await clearCalls(page);
    await (await sceneItem(page, "scene-new")).click();
    await expect(page.getByTestId("scene-chip-8")).toBeVisible();
    expect((await calls(page, "create_scene"))[0].args).toEqual({ imageIds: [11, 12, 13] });
    await expect(page.getByTestId("scene-chip-8")).toContainText("3");
    await expect(page.getByTestId("scene-chip-1")).toContainText("37");
    await expect(page.getByTestId("scene-badge-12")).toHaveText("S8");

    // Remove one photo from the scene.
    await page.getByTestId("cell-12").click();
    await clearCalls(page);
    await (await sceneItem(page, "scene-remove")).click();
    await expect(page.getByTestId("scene-badge-12")).toHaveCount(0);
    expect((await calls(page, "set_scene_members"))[0].args).toEqual({ id: 8, imageIds: [11, 13] });

    // Delete the filtered scene; the filter falls back to all photos.
    await page.getByTestId("scene-chip-8").click();
    await expect(page.getByTestId("selection-count")).toContainText("2 of 200");
    await clearCalls(page);
    await (await sceneItem(page, "scene-delete")).click();
    await expect(page.getByTestId("scene-chip-8")).toHaveCount(0);
    expect((await calls(page, "delete_scene"))[0].args).toEqual({ id: 8 });
    await expect(page.getByTestId("selection-count")).toContainText("200 photos");
    await expect(page.getByTestId("scene-badge-11")).toHaveCount(0);
    await shot(page, "4x-scenes-06-edited");
  });
});
