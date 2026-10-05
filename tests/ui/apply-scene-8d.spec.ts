import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openApp, openHome, openSection, shot } from "./helpers";

// Phase 8d: Apply to scene flushes pending Develop edits, says what it did, every disabled Apply says why,
// and Cmd+C / Cmd+V / Sync work over a multi-selection as one undoable batch.
// Mock rules: project 1 "ceremony" = ids 1..101, 43 keepers in 3 scenes.

async function openPlan(page: Page) {
  await openHome(page, 201, "&style=ready");
  await page.getByTestId("project-open-1").click();
  await expect(page.getByTestId("step-bar")).toBeVisible();
  await page.getByTestId("continue-edit").click();
  await expect(page.getByTestId("plan-view")).toBeVisible();
  await expect(page.locator('[data-testid^="plan-scene-"]')).toHaveCount(3);
}

async function sceneIds(page: Page): Promise<number[]> {
  const t = await page.locator('article[data-testid^="plan-scene-"]').evaluateAll((els) => els.map((e) => e.getAttribute("data-testid")!));
  return t.map((x) => Number(x.replace("plan-scene-", "")));
}

async function setSlider(page: Page, id: string, value: number, blur = true) {
  const s = page.getByTestId(`slider-${id}`);
  await s.fill(String(value));
  if (blur) await s.evaluate((el) => (el as HTMLElement).blur());
}

const notice = (page: Page) => page.getByTestId("notice").last();

test.describe("apply to scene (8d)", () => {
  test("every disabled Apply shows its reason in place, with a one-click fix", async ({ page }) => {
    await openPlan(page);
    const [a] = await sceneIds(page);
    // Plan: the untouched scene says why, Apply all says why.
    await expect(page.getByTestId(`plan-apply-why-${a}`)).toContainText("Edit the representative first");
    await expect(page.getByTestId("plan-apply-all-why")).toContainText("Edit a scene's representative first");
    await shot(page, "8d-apply-why-plan");

    // Develop on a member photo: the bar explains and opens the representative.
    await page.getByTestId(`plan-edit-${a}`).click();
    await expect(page.getByTestId("develop-view")).toBeVisible();
    const rep = Number(await page.getByTestId("develop-view").getAttribute("data-image-id"));
    await expect(page.getByTestId("edit-apply")).toBeDisabled();
    await expect(page.getByTestId("edit-apply-why")).toContainText("Edit the representative first");
    await expect(page.getByTestId("edit-apply-why-fix")).toHaveCount(0); // already on it
    await page.getByTestId("film-2").click();
    await expect(page.getByTestId("edit-apply-why-fix")).toHaveText("Open representative");
    await page.getByTestId("edit-apply-why-fix").click();
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", String(rep));
    await shot(page, "8d-apply-why-develop");
  });

  test("Apply flushes the edit still in flight, then reports matched vs copied and skipped photos with Show", async ({ page }) => {
    await openPlan(page);
    const [a] = await sceneIds(page);
    await page.getByTestId(`plan-edit-${a}`).click();
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await setSlider(page, "exposure", 0.7);
    await expect.poll(async () => (await calls(page, "save_adjustments")).length).toBeGreaterThan(0);
    await expect(page.getByTestId("edit-apply")).toBeEnabled();

    // Grain, then Apply with no pause: the grain must be saved before the apply reads the representative.
    await openSection(page, "effects");
    await clearCalls(page);
    await setSlider(page, "grain-amount", 25, false);
    await page.getByTestId("edit-apply").click();
    await expect(page.getByTestId("edit-chip")).toContainText("Applied");
    const log = await page.evaluate(() => window.__ipcLog.map((x) => x.cmd));
    const saved = log.indexOf("save_adjustments");
    const applied = log.indexOf("apply_scene_edit");
    expect(saved).toBeGreaterThanOrEqual(0);
    expect(saved).toBeLessThan(applied);
    await expect(notice(page)).toContainText("Exposure and white balance matched per photo");
    await expect(notice(page)).toContainText("everything else copied");

    // The target got the grain.
    await page.getByTestId("film-2").click();
    await expect(page.getByTestId("slider-grain-amount")).toHaveValue("25");

    // The user edits that target; the next apply skips it, says so, and Show lists it.
    await setSlider(page, "exposure", -0.4);
    await expect.poll(async () => (await calls(page, "save_adjustments")).length).toBeGreaterThan(1);
    // Back on the representative (the filmstrip marks it).
    await page.locator('[data-testid^="film-rep-"]').first().click();
    await setSlider(page, "exposure", 0.9);
    await expect.poll(async () => (await calls(page, "save_adjustments")).length).toBeGreaterThan(2);
    await clearCalls(page);
    await expect(page.getByTestId("edit-apply")).toBeEnabled();
    await page.getByTestId("edit-apply").click();
    await expect(notice(page)).toContainText("skipped because you edited them");
    await shot(page, "8d-apply-skipped-toast");
    await page.getByTestId("apply-skipped-show-photos").click();
    await expect(page.getByTestId("id-filter-clear")).toBeVisible();
    await expect(page.getByTestId("selection-bar")).toBeVisible();
    await expect(page.getByTestId("selbar-count")).toHaveText("1 selected");
    await shot(page, "8d-apply-skipped-shown");
  });

  test("Cmd+C / Cmd+V pastes every setting to a 5-photo selection, one Undo reverts all", async ({ page }) => {
    await openApp(page, 60);
    // Edit photo 1 in Develop (exposure + grain), back to the grid.
    await page.getByTestId("cell-1").click();
    await page.keyboard.press("d");
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await setSlider(page, "exposure", 0.8);
    await openSection(page, "effects");
    await setSlider(page, "grain-amount", 30);
    await expect.poll(async () => (await calls(page, "save_adjustments")).length).toBeGreaterThan(1);
    await page.keyboard.press("g");
    await expect(page.getByTestId("cell-1")).toBeVisible();

    await page.getByTestId("cell-1").click();
    await page.keyboard.press("Meta+c");
    await expect(notice(page)).toContainText("Copied all settings");
    await page.getByTestId("cell-2").click();
    await page.getByTestId("cell-6").click({ modifiers: ["Shift"] });
    await expect(page.getByTestId("selbar-count")).toHaveText("5 selected");
    await expect(page.getByTestId("sel-paste")).toBeEnabled();
    await clearCalls(page);
    await page.keyboard.press("Meta+v");
    await expect.poll(async () => (await calls(page, "paste_settings")).length).toBe(1);
    expect((await calls(page, "paste_settings"))[0].args.ids).toHaveLength(5);
    await expect(notice(page)).toContainText("to 5 photos");

    await page.getByTestId("paste-undo-batch").click();
    await expect.poll(async () => (await calls(page, "undo_edit_batch")).length).toBe(1);
    await expect(notice(page)).toContainText("Undid");
    await expect(notice(page)).toContainText("5 photos");
    expect((await calls(page, "undo_edit_batch"))[0].args.batchId).toBeGreaterThan(0);
  });

  test("selection bar: every disabled button says why; Select scene + Sync from active", async ({ page }) => {
    await openApp(page, 60);
    await page.getByTestId("cell-1").click();
    await page.getByTestId("cell-3").click({ modifiers: ["Shift"] });
    await expect(page.getByTestId("selection-bar")).toBeVisible();
    await expect(page.getByTestId("sel-paste")).toBeDisabled();
    await expect(page.getByTestId("sel-paste-why")).toContainText("Copy settings first");
    await page.getByTestId("cell-1").click();
    await expect(page.getByTestId("selection-bar")).toHaveCount(0);
    await shot(page, "8d-selection-bar");
  });
});
