import { expect, test } from "@playwright/test";
import { calls, clearCalls, openApp } from "./helpers";

// Loupe / Compare at 100%+ fetch a full-resolution region render (Develop's `detail` slot) for the visible area.

test("100% in the Loupe requests a full-res region render and swaps it in; panning re-requests (debounced)", async ({ page }) => {
  await openApp(page, 400);
  await page.getByTestId("cell-4").click();
  await page.keyboard.press("Space");
  await expect(page.getByTestId("zoom-a").locator("img").nth(1)).toBeVisible();
  await expect(page.getByTestId("zoom-detail")).toHaveCount(0);
  await clearCalls(page);
  await page.getByTestId("zoom-preset-100").click();
  await expect(page.getByTestId("zoom-label")).toContainText("100%");
  await expect(page.getByTestId("zoom-detail")).toBeVisible();
  const first = (await calls(page, "render_preview")).filter((c) => c.args.options.slot === "detail");
  expect(first.length).toBe(1);
  const r = first[0].args.options.region;
  expect(r.width).toBeGreaterThan(0);
  expect(r.width).toBeLessThanOrEqual(1);
  expect(first[0].args.id).toBe(4);
  expect(first[0].args.adjustments.crop.enabled).toBe(false);
  // Dragging pans; the tile is requested once the pan settles, not per move, and the old tile stays meanwhile.
  await clearCalls(page);
  const box = (await page.getByTestId("zoom-a").boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  for (let i = 1; i <= 10; i++) await page.mouse.move(box.x + box.width / 2 - i * 12, box.y + box.height / 2 - i * 6);
  await expect(page.getByTestId("zoom-detail")).toBeVisible();
  await page.mouse.up();
  await expect.poll(async () => (await calls(page, "render_preview")).filter((c) => c.args.options.slot === "detail").length).toBe(1);
  // Back to Fit drops the tile.
  await page.getByTestId("zoom-preset-fit").click();
  await expect(page.getByTestId("zoom-detail")).toHaveCount(0);
});
