import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openApp, openSection } from "./helpers";

// UX 8d re-check 3: R3-P2-1 (scroll restore), R3-P2-2 (hold Right keeps the tool), R3-P2-3 (undo order, Cmd+S keeps the tool).

async function openDevelop(page: Page) {
  await openApp(page, 200);
  await page.getByTestId("cell-1").click();
  await page.keyboard.press("d");
  await expect(page.getByTestId("histogram")).toHaveAttribute("data-empty", "false");
}
const cropSaves = async (page: Page) => (await calls(page, "save_adjustments")).filter((c) => c.args.label === "Crop");
async function setAngle(page: Page, v: string) {
  await page.getByTestId("slider-crop-angle").fill(v);
  await page.getByTestId("slider-crop-angle").blur();
}
const angle = (page: Page) => page.getByTestId("slider-value-crop-angle");

test("R3-P2-1: R then Esc restores the right-panel scroll", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await openDevelop(page);
  await openSection(page, "transform");
  const sc = page.getByTestId("adjust-scroll");
  await sc.evaluate((el) => (el.scrollTop = 517));
  const before = await sc.evaluate((el) => el.scrollTop);
  expect(before).toBeGreaterThan(0);
  await page.keyboard.press("r");
  await expect(page.getByTestId("crop-overlay")).toBeVisible();
  await page.waitForTimeout(200);
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("crop-overlay")).toHaveCount(0);
  await page.waitForTimeout(200);
  const after = await sc.evaluate((el) => el.scrollTop);
  expect(Math.abs(after - before)).toBeLessThanOrEqual(1);
});

test("R3-P2-2: R, angle 3, three quick Right: one Crop, tool open on photo 4 at 0.0", async ({ page }) => {
  await openDevelop(page);
  await page.keyboard.press("r");
  await expect(page.getByTestId("crop-overlay")).toBeVisible();
  await clearCalls(page);
  await setAngle(page, "3");
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("ArrowRight");
  await expect(page.getByTestId("crop-overlay")).toBeVisible();
  await expect(angle(page)).toHaveText("0.0°");
  await page.waitForTimeout(400);
  expect((await cropSaves(page)).length).toBe(1);
  await expect(page.getByTestId("crop-overlay")).toBeVisible();
});

test("R3-P2-3a: angle 3, Exposure +1, Cmd+Z undoes Exposure and keeps the tool; next Cmd+Z reverts it", async ({ page }) => {
  await openDevelop(page);
  await page.keyboard.press("r");
  await expect(page.getByTestId("crop-overlay")).toBeVisible();
  await setAngle(page, "3");
  await page.waitForTimeout(50);
  await page.getByTestId("slider-exposure").fill("1");
  await page.getByTestId("slider-exposure").blur();
  await page.waitForTimeout(300);
  await page.keyboard.press("Meta+z");
  await expect(page.getByTestId("slider-value-exposure")).toHaveText(/^0(\.0+)?$/);
  await expect(angle(page)).toHaveText("+3.0°");
  await page.keyboard.press("Meta+z");
  await expect(page.getByText("Crop changes undone")).toBeVisible();
  await expect(angle(page)).toHaveText("0.0°");
});

test("R3-P2-3b: R, angle 2, Cmd+S: one Crop and the tool stays open at +2.0", async ({ page }) => {
  await openDevelop(page);
  await page.keyboard.press("r");
  await expect(page.getByTestId("crop-overlay")).toBeVisible();
  await setAngle(page, "2");
  await clearCalls(page);
  await page.keyboard.press("Meta+s");
  await expect.poll(async () => (await cropSaves(page)).length).toBe(1);
  await expect(page.getByTestId("crop-overlay")).toBeVisible();
  await expect(angle(page)).toHaveText("+2.0°");
});
