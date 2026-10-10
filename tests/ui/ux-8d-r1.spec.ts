import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openApp } from "./helpers";

// UX 8d re-check 1: R1-1 (crop commits on every exit but Esc / Cancel), R1-2 (typed angle), R1-3 step 1, R1-4 and the P2s.

async function openCrop(page: Page, cell = 1) {
  await openApp(page, 200);
  await page.getByTestId(`cell-${cell}`).click();
  await page.keyboard.press("d");
  await expect(page.getByTestId("view-main")).toBeVisible();
  await expect(page.getByTestId("histogram")).toHaveAttribute("data-empty", "false");
  await page.keyboard.press("r");
  await expect(page.getByTestId("crop-overlay")).toBeVisible();
}
const cropSaves = async (page: Page) => (await calls(page, "save_adjustments")).filter((c) => c.args.label === "Crop");

test.describe("R1-1 leaving the crop tool commits", () => {
  test("angle 4 then Right: one Crop save on photo 1, tool open on photo 2", async ({ page }) => {
    await openCrop(page);
    await clearCalls(page);
    await page.getByTestId("slider-crop-angle").fill("4");
    await page.getByTestId("slider-crop-angle").blur();
    await page.keyboard.press("ArrowRight");
    await expect.poll(async () => (await cropSaves(page)).length).toBe(1);
    const s = (await cropSaves(page))[0];
    expect(s.args.adjustments.crop.angle).toBe(4);
    await expect(page.getByTestId("crop-overlay")).toBeVisible();
    await page.waitForTimeout(300);
    expect((await cropSaves(page)).length).toBe(1);
  });

  test("angle 4 then G: one Crop save", async ({ page }) => {
    await openCrop(page);
    await clearCalls(page);
    await page.getByTestId("slider-crop-angle").fill("4");
    await page.keyboard.press("g");
    await expect(page.getByTestId("cell-1")).toBeVisible();
    await expect.poll(async () => (await cropSaves(page)).length).toBe(1);
  });

  test("angle 4 then Esc: no save", async ({ page }) => {
    await openCrop(page);
    await clearCalls(page);
    await page.getByTestId("slider-crop-angle").fill("4");
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("crop-overlay")).toHaveCount(0);
    await page.waitForTimeout(300);
    expect((await cropSaves(page)).length).toBe(0);
  });

  test("filmstrip click commits too", async ({ page }) => {
    await openCrop(page);
    await clearCalls(page);
    await page.getByTestId("slider-crop-angle").fill("3");
    await page.getByTestId("film-3").click();
    await expect.poll(async () => (await cropSaves(page)).length).toBe(1);
    await expect(page.getByTestId("crop-overlay")).toBeVisible();
  });
});

test.describe("R1-2 typed crop angle", () => {
  test("type 5 + Enter in the Angle value", async ({ page }) => {
    await openCrop(page);
    await page.getByTestId("slider-value-crop-angle").click();
    await page.getByTestId("slider-edit-crop-angle").fill("5");
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("slider-value-crop-angle")).toHaveText("+5.0°");
    const r = JSON.parse((await page.getByTestId("crop-rect").getAttribute("data-rect"))!);
    for (const [k, v] of [["l", 0.0563], ["t", 0.0563], ["r", 0.9437], ["b", 0.9437]] as const) expect(Math.abs(r[k] - v)).toBeLessThan(0.002);
    await expect(page.getByTestId("crop-overlay")).toBeVisible(); // Enter in the field did not commit the crop
  });

  test("cropbar angle -1.3", async ({ page }) => {
    await openApp(page, 200);
    await page.getByTestId("cell-1").click();
    await page.keyboard.press("d");
    await expect(page.getByTestId("histogram")).toHaveAttribute("data-empty", "false");
    await page.keyboard.press("Tab"); // hide the right panel: the crop bar shows
    await page.keyboard.press("r");
    await expect(page.getByTestId("crop-bar")).toBeVisible();
    await page.getByTestId("cropbar-angle").fill("-1.3");
    await expect(page.getByTestId("cropbar-angle-value")).toHaveText("-1.3°");
  });
});
