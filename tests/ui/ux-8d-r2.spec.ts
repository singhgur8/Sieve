import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openApp, openSection } from "./helpers";

// UX 8d re-check 2: R2-1 (tool follows history), R2-2 (implicit exits only commit touched tools), R2-P2-1..3.

async function openDevelop(page: Page) {
  await openApp(page, 200);
  await page.getByTestId("cell-1").click();
  await page.keyboard.press("d");
  await expect(page.getByTestId("histogram")).toHaveAttribute("data-empty", "false");
}
const cropSaves = async (page: Page) => (await calls(page, "save_adjustments")).filter((c) => c.args.label === "Crop");
const rect = async (page: Page) => JSON.parse((await page.getByTestId("crop-rect").getAttribute("data-rect"))!) as { l: number; t: number; r: number; b: number };
async function setAngle(page: Page, v: string) {
  await page.getByTestId("slider-crop-angle").fill(v);
  await page.getByTestId("slider-crop-angle").blur();
}
/** Commits a 4 degree crop on photo 1 and re-opens the tool. */
async function committed4(page: Page) {
  await openDevelop(page);
  await page.keyboard.press("r");
  await expect(page.getByTestId("crop-overlay")).toBeVisible();
  await setAngle(page, "4");
  await page.keyboard.press("r");
  await expect(page.getByTestId("crop-overlay")).toHaveCount(0);
  await expect.poll(async () => (await cropSaves(page)).length).toBe(1);
  await page.keyboard.press("r");
  await expect(page.getByTestId("crop-overlay")).toBeVisible();
  await expect(page.getByTestId("slider-value-crop-angle")).toHaveText("+4.0°");
  await clearCalls(page);
}

test.describe("R2-1 the crop tool follows history", () => {
  for (const how of ["Cmd+Z", "History Original", "Cmd+Shift+R"]) {
    test(`commit 4, R, ${how}: tool at 0.0, no Crop save on exit`, async ({ page }) => {
      await committed4(page);
      if (how === "Cmd+Z") await page.keyboard.press("Meta+z");
      else if (how === "Cmd+Shift+R") await page.keyboard.press("Meta+Shift+r");
      else await page.getByTestId("history-list").getByRole("button", { name: "Original" }).click();
      await expect(page.getByTestId("slider-value-crop-angle")).toHaveText("0.0°");
      expect(await rect(page)).toEqual({ l: 0, t: 0, r: 1, b: 1 });
      await page.keyboard.press("ArrowRight");
      await page.waitForTimeout(400);
      expect((await cropSaves(page)).length).toBe(0);
    });
  }

  test("R, angle 3, Cmd+Z: back to the seed, history unchanged; next Cmd+Z is a normal undo", async ({ page }) => {
    await committed4(page);
    await setAngle(page, "3");
    await expect(page.getByTestId("slider-value-crop-angle")).toHaveText("+3.0°");
    await page.keyboard.press("Meta+z");
    await expect(page.getByTestId("slider-value-crop-angle")).toHaveText("+4.0°");
    expect((await calls(page, "undo_adjustments")).length).toBe(0);
    await expect(page.getByText("Crop changes undone")).toBeVisible();
    await page.keyboard.press("Meta+z");
    await expect(page.getByTestId("slider-value-crop-angle")).toHaveText("0.0°");
    expect((await calls(page, "undo_adjustments")).length).toBe(1);
  });
});

test.describe("R2-2 implicit exits commit only touched tools", () => {
  async function uprightOnPhoto2(page: Page) {
    await openDevelop(page);
    await page.getByTestId("film-2").click();
    await expect(page.getByTestId("histogram")).toHaveAttribute("data-empty", "false");
    await openSection(page, "transform");
    await page.getByTestId("upright-vertical").click();
    await page.waitForTimeout(500);
    await page.getByTestId("film-1").click();
    await expect(page.getByTestId("histogram")).toHaveAttribute("data-empty", "false");
    await clearCalls(page);
  }

  test("R then Right through an Upright photo: no save, never the full frame once visible", async ({ page }) => {
    await uprightOnPhoto2(page);
    await page.keyboard.press("r");
    await expect(page.getByTestId("crop-overlay")).toBeVisible();
    await page.keyboard.press("ArrowRight");
    const seen: string[] = [];
    for (let i = 0; i < 30; i++) {
      if (await page.getByTestId("crop-rect").count()) seen.push((await page.getByTestId("crop-rect").getAttribute("data-rect")) ?? "");
      await page.waitForTimeout(50);
    }
    expect(seen.length).toBeGreaterThan(0);
    for (const s of seen) expect(JSON.parse(s)).not.toEqual({ l: 0, t: 0, r: 1, b: 1 });
    await page.keyboard.press("ArrowRight");
    await page.waitForTimeout(500);
    expect((await calls(page, "save_adjustments")).length).toBe(0);
  });

  test("R, Enter on an Upright photo: one Crop (Enter is explicit)", async ({ page }) => {
    await uprightOnPhoto2(page);
    await page.getByTestId("film-2").click();
    await expect(page.getByTestId("histogram")).toHaveAttribute("data-empty", "false");
    await clearCalls(page);
    await page.keyboard.press("r");
    await expect(page.getByTestId("crop-overlay")).toBeVisible();
    await page.waitForTimeout(400);
    await page.keyboard.press("Enter");
    await expect.poll(async () => (await cropSaves(page)).length).toBe(1);
  });

  test("R, angle 2, Right: still one Crop", async ({ page }) => {
    await openDevelop(page);
    await page.keyboard.press("r");
    await expect(page.getByTestId("crop-overlay")).toBeVisible();
    await clearCalls(page);
    await setAngle(page, "2");
    await page.keyboard.press("ArrowRight");
    await expect.poll(async () => (await cropSaves(page)).length).toBe(1);
  });

  test("R2-P2-3: Cmd+S applies the on-screen crop first", async ({ page }) => {
    await openDevelop(page);
    await page.keyboard.press("r");
    await expect(page.getByTestId("crop-overlay")).toBeVisible();
    await setAngle(page, "2");
    await clearCalls(page);
    await page.keyboard.press("Meta+s");
    await expect.poll(async () => (await cropSaves(page)).length).toBe(1);
  });
});
