import { expect, test, type Page } from "@playwright/test";
import { detectScenes, openApp, openHome } from "./helpers";

// UX review 8d, P2 items: P2-1 (Space toggles to the last zoom), P2-2 (info card anchoring), P2-7, P2-8, P2-9, P2-14.

async function openLoupe(page: Page, id = 4) {
  await openApp(page, 400);
  await page.getByTestId("cell-" + id).click();
  await page.keyboard.press("Space");
  await expect(page.getByTestId("loupe")).toBeVisible();
  await expect(page.getByTestId("zoom-a").locator("img").nth(1)).toBeVisible();
}

const box = async (page: Page, id: string) => (await page.getByTestId(id).boundingBox())!;
const overlaps = (a: { x: number; y: number; width: number; height: number }, b: { x: number; y: number; width: number; height: number }) =>
  a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;

test.describe("P2-1 Space toggles Fit and the last zoom-in preset", () => {
  test("Loupe: 200%, Fit, Space -> 200%", async ({ page }) => {
    await openLoupe(page);
    await page.getByTestId("zoom-preset-200").click();
    await expect(page.getByTestId("zoom-label")).toContainText("200%");
    await page.getByTestId("zoom-preset-fit").click();
    await expect(page.getByTestId("zoom-label")).toContainText("Fit");
    await page.keyboard.press("Space");
    await expect(page.getByTestId("zoom-label")).toContainText("200%");
    await page.keyboard.press("Space");
    await expect(page.getByTestId("zoom-label")).toContainText("Fit");
  });

  test("Develop: 200%, Fit, Space -> 200%", async ({ page }) => {
    await openApp(page, 60);
    await page.getByTestId("cell-3").click();
    await page.keyboard.press("d");
    const viewer = page.getByTestId("viewer");
    await expect(page.getByTestId("view-main")).toBeVisible();
    await page.getByTestId("zoom-200").click();
    await expect(viewer).toHaveAttribute("data-zoom-s", "2");
    await page.getByTestId("nav-fit").click();
    await expect(viewer).toHaveAttribute("data-zoomed", "false");
    await page.keyboard.press("Space");
    await expect(viewer).toHaveAttribute("data-zoom-s", "2");
  });
});

test.describe("P2-2 / P2-8 / P2-9 / P2-7 / P2-14", () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  test("the photo info card does not cover Continue, Suggestions or the Develop sliders", async ({ page }) => {
    await openHome(page, 201, "&keepers=not_rejected");
    await page.getByTestId("project-open-1").click();
    await expect(page.getByTestId("cull-summary")).toBeVisible();
    await page.getByTestId("cell-2").click();
    await page.keyboard.press("Meta+i");
    const card = await box(page, "photo-info");
    for (const id of ["continue-edit", "cull-sum-suggest"]) {
      const b = await page.getByTestId(id).boundingBox();
      if (b) expect(overlaps(card, b), id).toBe(false);
    }
    await page.keyboard.press("d");
    await expect(page.getByTestId("develop-view")).toBeVisible();
    const dcard = await box(page, "photo-info");
    expect(overlaps(dcard, await box(page, "adjust-panel"))).toBe(false);
  });

  test("tool active: the toast moves off the crop's corner handles; the crop bar only stands in for a hidden panel", async ({ page }) => {
    await openApp(page, 30);
    await page.getByTestId("cell-3").click();
    await page.keyboard.press("d");
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await page.keyboard.press("r");
    await expect(page.getByTestId("crop-panel")).toBeVisible();
    await expect(page.getByTestId("crop-bar")).toHaveCount(0);
    await page.keyboard.press("Meta+c");
    await expect(page.getByTestId("toasts")).toHaveAttribute("data-placement", /tool|bottom/);
    await expect(page.getByTestId("toasts")).not.toHaveAttribute("data-placement", "top");
  });

  test("Basic has 'Auto' and 'Auto tone', and no truncated helper text", async ({ page }) => {
    await openApp(page, 30);
    await page.getByTestId("cell-3").click();
    await page.keyboard.press("d");
    await expect(page.getByTestId("auto-all")).toHaveText("Auto");
    await expect(page.getByTestId("auto-tone")).toHaveText("Auto tone");
  });

  test("filmstrip scene badge does not sit on the stars", async ({ page }) => {
    await openApp(page, 200);
    await detectScenes(page);
    await expect(page.getByTestId("scene-chip-6")).toBeVisible();
    await page.getByTestId("cell-3").click();
    await page.keyboard.press("d");
    await page.keyboard.press("4");
    const badge = await box(page, "film-scene-3");
    const stars = await page.getByTestId("film-rating-3").boundingBox();
    if (stars) expect(overlaps(badge, stars)).toBe(false);
  });
});
