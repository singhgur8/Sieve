import { expect, test, type Page } from "@playwright/test";
import { openApp } from "./helpers";

// Lightroom-style loupe zoom: Space / click zoom at the cursor, zoom + relative position persist while stepping
// with the arrow keys (Loupe, Compare, Develop), presets, drag to pan.

async function openLoupe(page: Page, id = 4) {
  await openApp(page, 400);
  await page.getByTestId("cell-" + id).click();
  await page.keyboard.press("Space");
  await expect(page.getByTestId("loupe")).toBeVisible();
  await expect(page.getByTestId("zoom-a").locator("img").nth(1)).toBeVisible();
}

const attr = async (page: Page, id: string, name: string) => Number(await page.getByTestId(id).getAttribute(name));

test.describe("loupe zoom like Lightroom", () => {
  test("Space zooms to 100% under the cursor and the same relative point stays through 5 arrow steps", async ({ page }) => {
    await openLoupe(page);
    const pane = page.getByTestId("zoom-a");
    const box = (await pane.boundingBox())!;
    // Bottom-right corner of the pane.
    await page.mouse.move(box.x + box.width - 6, box.y + box.height - 6);
    await page.keyboard.press("Space");
    await expect(page.getByTestId("zoom-label")).toContainText("100%");
    const cx = await attr(page, "zoom-a", "data-cx");
    const cy = await attr(page, "zoom-a", "data-cy");
    expect(cx).toBeGreaterThan(0.6);
    expect(cy).toBeGreaterThan(0.6);
    const scale = await attr(page, "zoom-a", "data-scale");
    for (let i = 0; i < 5; i++) {
      await page.keyboard.press("ArrowRight");
      await expect(page.getByTestId("info-overlay")).toContainText(`DSC0000${5 + i}`);
    }
    await expect(page.getByTestId("zoom-label")).toContainText("100%");
    expect(await attr(page, "zoom-a", "data-scale")).toBeCloseTo(scale, 2);
    expect(await attr(page, "zoom-a", "data-cx")).toBeCloseTo(cx, 3);
    expect(await attr(page, "zoom-a", "data-cy")).toBeCloseTo(cy, 3);
    // Space goes back to Fit, and Left keeps Fit.
    await page.keyboard.press("Space");
    await expect(page.getByTestId("zoom-label")).toContainText("Fit");
    await page.keyboard.press("ArrowLeft");
    await expect(page.getByTestId("zoom-label")).toContainText("Fit");
  });

  test("Space with the cursor at different places zooms there; click toggles too", async ({ page }) => {
    await openLoupe(page);
    const pane = page.getByTestId("zoom-a");
    const box = (await pane.boundingBox())!;
    await page.mouse.move(box.x + 8, box.y + 8);
    await page.keyboard.press("Space");
    await expect(page.getByTestId("zoom-label")).toContainText("100%");
    const tl = { x: await attr(page, "zoom-a", "data-cx"), y: await attr(page, "zoom-a", "data-cy") };
    await page.keyboard.press("Space");
    await expect(page.getByTestId("zoom-label")).toContainText("Fit");
    await page.mouse.move(box.x + box.width - 8, box.y + box.height - 8);
    await page.keyboard.press("Space");
    await expect(page.getByTestId("zoom-label")).toContainText("100%");
    const br = { x: await attr(page, "zoom-a", "data-cx"), y: await attr(page, "zoom-a", "data-cy") };
    expect(tl.x).toBeLessThan(0.45);
    expect(tl.y).toBeLessThan(0.45);
    expect(br.x).toBeGreaterThan(0.55);
    expect(br.y).toBeGreaterThan(0.55);
    // Click: back to Fit, click again zooms in at the click point.
    await pane.click({ position: { x: 10, y: 10 } });
    await expect(page.getByTestId("zoom-label")).toContainText("Fit");
    await pane.click({ position: { x: 10, y: 10 } });
    await expect(page.getByTestId("zoom-label")).toContainText("100%");
    expect(await attr(page, "zoom-a", "data-cx")).toBeLessThan(0.45);
  });

  test("presets Fit / Fill / 50 / 100 / 200 / 400 and drag to pan", async ({ page }) => {
    await openLoupe(page);
    const label = page.getByTestId("zoom-label");
    await page.getByTestId("zoom-preset-100").click();
    await expect(label).toContainText("100%");
    await expect(page.getByTestId("zoom-preset-100")).toHaveAttribute("aria-pressed", "true");
    const s100 = await attr(page, "zoom-a", "data-scale");
    await page.getByTestId("zoom-preset-200").click();
    await expect(label).toContainText("200%");
    expect(await attr(page, "zoom-a", "data-scale")).toBeCloseTo(s100 * 2, 2);
    await page.getByTestId("zoom-preset-400").click();
    await expect(label).toContainText("400%");
    await page.getByTestId("zoom-preset-fill").click();
    await expect(page.getByTestId("zoom-preset-fill")).toHaveAttribute("aria-pressed", "true");
    expect(await attr(page, "zoom-a", "data-scale")).toBeGreaterThanOrEqual(1);
    await page.getByTestId("zoom-preset-fit").click();
    await expect(label).toContainText("Fit");
    // Space returns to the last zoom-in preset chosen (Fill here); with 100% chosen it is 100%.
    await page.keyboard.press("Space");
    await expect(page.getByTestId("zoom-preset-fill")).toHaveAttribute("aria-pressed", "true");
    await page.getByTestId("zoom-preset-100").click();
    await page.getByTestId("zoom-preset-fit").click();
    await expect(label).toContainText("Fit");
    await page.keyboard.press("Space");
    await expect(label).toContainText("100%");
    // Drag pans, and a drag does not toggle zoom.
    const pane = page.getByTestId("zoom-a");
    const box = (await pane.boundingBox())!;
    const cx0 = await attr(page, "zoom-a", "data-cx");
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width / 2 - 120, box.y + box.height / 2, { steps: 6 });
    await page.mouse.up();
    await expect(label).toContainText("100%");
    await expect.poll(async () => attr(page, "zoom-a", "data-cx")).not.toBeCloseTo(cx0, 3);
  });

  test("Compare keeps the shared zoom while the Candidate steps", async ({ page }) => {
    await openLoupe(page);
    await page.keyboard.press("c");
    await expect(page.getByTestId("compare")).toBeVisible();
    await page.mouse.move(400, 300);
    await page.keyboard.press("Space");
    await expect(page.getByTestId("zoom-label")).toContainText("100%");
    const cx = await attr(page, "zoom-a", "data-cx");
    await page.keyboard.press("ArrowRight");
    await page.keyboard.press("ArrowRight");
    await expect(page.getByTestId("zoom-label")).toContainText("100%");
    expect(await attr(page, "zoom-a", "data-cx")).toBeCloseTo(cx, 3);
    expect(await attr(page, "zoom-b", "data-cx")).toBeCloseTo(cx, 3);
  });
});

test.describe("develop zoom like Lightroom", () => {
  test("zoom persists across arrows, Space zooms at the cursor, presets work", async ({ page }) => {
    await openApp(page, 200);
    await page.getByTestId("cell-3").click();
    await page.keyboard.press("d");
    await expect(page.getByTestId("view-main")).toBeVisible();
    const viewer = page.getByTestId("viewer");
    const box = (await viewer.boundingBox())!;
    await page.mouse.move(box.x + box.width - 10, box.y + box.height - 10);
    await page.keyboard.press("Space");
    await expect(viewer).toHaveAttribute("data-zoomed", "true");
    const cx = Number(await viewer.getAttribute("data-zoom-cx"));
    const cy = Number(await viewer.getAttribute("data-zoom-cy"));
    expect(cx).toBeGreaterThan(0.5);
    expect(cy).toBeGreaterThan(0.5);
    await page.keyboard.press("ArrowRight");
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await expect(viewer).toHaveAttribute("data-zoomed", "true");
    expect(Number(await viewer.getAttribute("data-zoom-cx"))).toBeCloseTo(cx, 3);
    expect(Number(await viewer.getAttribute("data-zoom-cy"))).toBeCloseTo(cy, 3);
    await page.getByTestId("zoom-200").click();
    await expect(viewer).toHaveAttribute("data-zoom-s", "2");
    await page.getByTestId("zoom-400").click();
    await expect(viewer).toHaveAttribute("data-zoom-s", "4");
    await page.getByTestId("nav-fit").click();
    await expect(viewer).toHaveAttribute("data-zoomed", "false");
  });
});
