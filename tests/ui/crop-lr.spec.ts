import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openApp } from "./helpers";

// Lightroom-style crop interactions: rotate by dragging outside, aspect lock, X swap, O overlays, Esc / Enter, angle tool.

async function openCrop(page: Page) {
  await openApp(page, 200);
  await page.getByTestId("cell-1").click();
  await page.keyboard.press("d");
  await expect(page.getByTestId("view-main")).toBeVisible();
  await expect(page.getByTestId("histogram")).toHaveAttribute("data-empty", "false");
  await page.keyboard.press("r");
  await expect(page.getByTestId("crop-overlay")).toBeVisible();
  await expect(page.getByTestId("crop-frame")).toBeVisible();
}

type R = { l: number; t: number; r: number; b: number };
const rectOf = async (page: Page): Promise<R> => JSON.parse((await page.getByTestId("crop-rect").getAttribute("data-rect"))!);
const rotation = async (page: Page) => Number(await page.getByTestId("crop-overlay").getAttribute("data-rotation"));
const saves = async (page: Page) => (await calls(page, "save_adjustments")).filter((c) => c.args.label !== undefined);
/** Pixel w / h of the crop rectangle. */
async function pixelRatio(page: Page) {
  const b = (await page.getByTestId("crop-rect").boundingBox())!;
  return b.width / b.height;
}

test.describe("crop like Lightroom", () => {
  test("drag outside the crop rotates the photo; rotating back restores the full crop", async ({ page }) => {
    await openCrop(page);
    const full = await rectOf(page);
    const f = (await page.getByTestId("crop-frame").boundingBox())!;
    const cx = f.x + f.width / 2;
    const cy = f.y + f.height / 2;
    // Start just outside the left edge, near the top (the side-panel chevron sits at mid height).
    const v0 = { x: -(f.width / 2 + 12), y: -(f.height / 2 - 40) };
    const rot = (deg: number) => {
      const a = (deg * Math.PI) / 180;
      return { x: cx + v0.x * Math.cos(a) - v0.y * Math.sin(a), y: cy + v0.x * Math.sin(a) + v0.y * Math.cos(a) };
    };
    await page.mouse.move(cx + v0.x, cy + v0.y);
    await page.mouse.down();
    const p1 = rot(10);
    await page.mouse.move(p1.x, p1.y, { steps: 6 });
    expect(Math.abs(await rotation(page))).toBeGreaterThan(5);
    await expect(page.getByTestId("crop-grid-line")).toHaveCount(9 * 1); // fine grid while rotating
    const tilted = await rectOf(page);
    expect((tilted.r - tilted.l) * (tilted.b - tilted.t)).toBeLessThan(0.85);
    // Back to level: the crop grows back to the full frame (it never only shrinks).
    await page.mouse.move(cx + v0.x, cy + v0.y, { steps: 6 });
    await page.mouse.up();
    expect(Math.abs(await rotation(page))).toBeLessThan(0.6);
    const back = await rectOf(page);
    expect((back.r - back.l) * (back.b - back.t)).toBeGreaterThan(0.97 * (full.r - full.l) * (full.b - full.t));
  });

  test("angle slider 12 then 0 restores the rect", async ({ page }) => {
    await openCrop(page);
    const before = await rectOf(page);
    await page.getByTestId("slider-crop-angle").fill("12");
    const mid = await rectOf(page);
    expect(mid.r - mid.l).toBeLessThan(before.r - before.l - 0.1);
    await page.getByTestId("slider-crop-angle").fill("0");
    expect(await rectOf(page)).toEqual(before);
  });

  test("aspect lock holds while dragging a corner; A unlocks; X swaps and swaps back", async ({ page }) => {
    await openCrop(page);
    await page.getByTestId("crop-aspect").selectOption("1:1");
    expect(await pixelRatio(page)).toBeCloseTo(1, 1);
    const se = (await page.getByTestId("crop-handle-nw").boundingBox())!;
    const f = (await page.getByTestId("crop-frame").boundingBox())!;
    await page.mouse.move(se.x + se.width / 2, se.y + se.height / 2);
    await page.mouse.down();
    await page.mouse.move(f.x + f.width * 0.3, f.y + f.height * 0.1, { steps: 4 });
    await page.mouse.up();
    expect(await pixelRatio(page)).toBeCloseTo(1, 1);
    await page.getByTestId("crop-aspect").selectOption("original");
    const r0 = await pixelRatio(page);
    expect(r0).toBeGreaterThan(1.3);
    await page.keyboard.press("x");
    expect(await pixelRatio(page)).toBeCloseTo(1 / r0, 1);
    await page.keyboard.press("x");
    expect(await pixelRatio(page)).toBeCloseTo(r0, 1);
    await page.keyboard.press("a");
    await expect(page.getByTestId("crop-aspect")).toHaveValue("free");
    await page.keyboard.press("a");
    await expect(page.getByTestId("crop-aspect")).not.toHaveValue("free");
    // Preset list matches Lightroom's.
    const labels = await page.getByTestId("crop-aspect").locator("option").allTextContents();
    expect(labels).toEqual(["Free", "As Shot", "Original", "1 x 1", "4 x 5 / 8 x 10", "5 x 7", "2 x 3 / 4 x 6", "16 x 9", "Custom..."]);
    await page.getByTestId("crop-aspect").selectOption("custom");
    await expect(page.getByTestId("crop-custom")).toBeVisible();
    await page.getByTestId("crop-custom-w").fill("2");
    await page.getByTestId("crop-custom-h").fill("1");
    expect(await pixelRatio(page)).toBeCloseTo(2, 1);
  });

  test("O cycles the overlays, Shift+O rotates the guide, fine grid shows while rotating", async ({ page }) => {
    await openCrop(page);
    const g = page.getByTestId("crop-guide");
    await expect(g).toHaveAttribute("data-overlay", "thirds");
    const order = ["grid", "goldenRatio", "goldenSpiral", "diagonal", "triangle", "aspects", "thirds"];
    for (const id of order) {
      await page.keyboard.press("o");
      await expect(g).toHaveAttribute("data-overlay", id);
    }
    await page.keyboard.press("o");
    await page.keyboard.press("o");
    await page.keyboard.press("o"); // goldenRatio -> spiral
    await expect(g).toHaveAttribute("data-orient", "0");
    await page.keyboard.press("Shift+O");
    await expect(g).toHaveAttribute("data-orient", "1");
    await page.keyboard.press("Shift+O");
    await expect(g).toHaveAttribute("data-orient", "2");
    // O did not toggle the (absent) mask overlay or anything else: still cropping.
    await expect(page.getByTestId("crop-overlay")).toBeVisible();
  });

  test("Esc cancels without saving; Enter commits one Crop edit; Cmd+Alt+R resets", async ({ page }) => {
    await openCrop(page);
    await clearCalls(page);
    await page.getByTestId("slider-crop-angle").fill("4");
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("crop-overlay")).toHaveCount(0);
    expect((await saves(page)).length).toBe(0);

    await page.keyboard.press("r");
    await expect(page.getByTestId("crop-overlay")).toBeVisible();
    await page.getByTestId("slider-crop-angle").fill("4");
    await page.keyboard.press("Meta+Alt+r");
    expect(await rotation(page)).toBe(0);
    await expect(page.getByTestId("crop-aspect")).toHaveValue("original");
    await page.getByTestId("slider-crop-angle").fill("4");
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("crop-overlay")).toHaveCount(0);
    await expect.poll(async () => (await saves(page)).length).toBe(1);
    const s = (await saves(page))[0].args;
    expect(s.label).toBe("Crop");
    expect(s.adjustments.crop).toMatchObject({ enabled: true, angle: 4 });
    // The stored crop is the largest rect at 4 degrees (not over-cropped): re-open and the rect is unchanged.
    await page.keyboard.press("r");
    const again = await rectOf(page);
    await page.keyboard.press("Escape");
    expect((again.r - again.l) * (again.b - again.t)).toBeGreaterThan(0.8); // analytic 0.823 for 3:2 at 4 degrees
  });

  test("angle tool: a line drawn along a tilted horizon levels it; Auto straighten is disabled with a reason", async ({ page }) => {
    await openCrop(page);
    await expect(page.getByTestId("crop-auto-straighten")).toBeDisabled();
    await expect(page.getByTestId("crop-auto-straighten")).toHaveAttribute("title", /Upright/);
    await page.getByTestId("crop-angle-tool").click();
    await expect(page.getByTestId("crop-angle-tool")).toHaveAttribute("aria-pressed", "true");
    const f = (await page.getByTestId("crop-frame").boundingBox())!;
    const x0 = f.x + f.width * 0.2;
    const y0 = f.y + f.height * 0.5;
    const x1 = f.x + f.width * 0.8;
    const y1 = y0 + (x1 - x0) * Math.tan((3 * Math.PI) / 180); // horizon sloping down to the right by 3 degrees
    await page.mouse.move(x0, y0);
    await page.mouse.down();
    await page.mouse.move(x1, y1, { steps: 6 });
    await expect(page.getByTestId("crop-straighten-line")).toBeVisible();
    await page.mouse.up();
    await expect(page.getByTestId("crop-angle-tool")).toHaveAttribute("aria-pressed", "false");
    // The photo is rotated 3 degrees counter-clockwise on screen to level the horizon (crs:CropAngle +3).
    expect(Math.abs(await rotation(page))).toBeGreaterThan(2.8);
    expect(Math.abs(await rotation(page))).toBeLessThan(3.2);
    const v = Number(await page.getByTestId("slider-crop-angle").inputValue());
    expect(Math.abs(Math.abs(v) - 3)).toBeLessThan(0.2);
  });

  test("Constrain to image off lets the angle go without shrinking the crop", async ({ page }) => {
    await openCrop(page);
    const before = await rectOf(page);
    await page.getByTestId("crop-constrain").uncheck();
    await page.getByTestId("slider-crop-angle").fill("10");
    expect(await rectOf(page)).toEqual(before);
    await page.getByTestId("crop-constrain").check();
    const after = await rectOf(page);
    expect(after.r - after.l).toBeLessThan(before.r - before.l - 0.05);
  });
});
