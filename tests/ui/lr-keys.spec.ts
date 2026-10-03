import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openApp } from "./helpers";

// Lightroom one-hand culling: Z = Pick, X = Reject, U = Unflag, Space = open Loupe (Grid) / zoom Fit<->1:1 (Loupe, Compare, Develop).

const lastPick = async (page: Page) => (await calls(page, "set_pick")).at(-1)?.args;

async function expectPick(page: Page, key: string, ids: number[], pick: string) {
  await clearCalls(page);
  await page.keyboard.press(key);
  await expect.poll(async () => (await calls(page, "set_pick")).length).toBe(1);
  expect(await lastPick(page)).toEqual({ ids, pick });
}

test.describe("Lightroom one-hand keys", () => {
  test("Grid: Z pick, X reject, U unflag, P alias, Shift+Z advances, Space opens the Loupe, Z does not zoom", async ({ page }) => {
    await openApp(page, 400);
    await page.getByTestId("cell-3").click();
    await expectPick(page, "z", [3], "pick");
    await expectPick(page, "x", [3], "reject");
    await expectPick(page, "u", [3], "unflagged");
    await expectPick(page, "p", [3], "pick");
    await expectPick(page, "Shift+z", [3], "pick");
    await expect(page.getByTestId("cell-4")).toHaveAttribute("data-active", "true");
    await expectPick(page, "Shift+x", [4], "reject");
    await expect(page.getByTestId("cell-5")).toHaveAttribute("data-active", "true");
    await expect(page.getByTestId("loupe")).toHaveCount(0);
    await page.keyboard.press("Space");
    await expect(page.getByTestId("loupe")).toBeVisible();
    await expect(page.getByTestId("zoom-label")).toContainText("Fit"); // Space opens at Fit
  });

  test("Loupe: Space toggles Fit / 1:1 and never leaves; Z / X flag; Enter, E, G, Esc return", async ({ page }) => {
    await openApp(page, 400);
    await page.getByTestId("cell-3").click();
    await page.keyboard.press("Space");
    await expect(page.getByTestId("zoom-label")).toContainText("Fit");
    await page.keyboard.press("Space");
    await expect(page.getByTestId("zoom-label")).toContainText("100%");
    await expect(page.getByTestId("loupe")).toBeVisible();
    await page.keyboard.press("Space");
    await expect(page.getByTestId("zoom-label")).toContainText("Fit");
    await expect(page.getByTestId("loupe")).toBeVisible();
    await expectPick(page, "z", [3], "pick");
    await expect(page.getByTestId("zoom-label")).toContainText("Fit"); // Z no longer zooms
    await expectPick(page, "x", [3], "reject");
    for (const key of ["Enter", "e", "g", "Escape"]) {
      await page.keyboard.press(key);
      await expect(page.getByTestId("loupe")).toHaveCount(0);
      await page.keyboard.press("Space");
      await expect(page.getByTestId("loupe")).toBeVisible();
    }
  });

  test("Compare: Space zooms both panes and stays; Z picks the active pane", async ({ page }) => {
    await openApp(page, 400);
    await page.getByTestId("cell-3").click();
    await page.keyboard.press("c");
    await expect(page.getByTestId("compare")).toBeVisible();
    await page.keyboard.press("Space");
    await expect(page.getByTestId("zoom-label")).toContainText("100%");
    await expect(page.getByTestId("compare")).toBeVisible();
    await page.keyboard.press("Space");
    await expect(page.getByTestId("zoom-label")).toContainText("Fit");
    await expect(page.getByTestId("compare")).toBeVisible();
    await clearCalls(page);
    await page.keyboard.press("z");
    await expect.poll(async () => (await calls(page, "set_pick")).length).toBe(1);
    await expect(page.getByTestId("zoom-label")).toContainText("Fit");
    await expect(page.getByTestId("compare")).toBeVisible();
  });

  test("Develop: Space toggles Fit / 100% and stays; Z / X flag; Z no longer zooms; crop keeps X = swap", async ({ page }) => {
    await openApp(page, 200);
    await page.getByTestId("cell-3").click();
    await page.keyboard.press("d");
    await expect(page.getByTestId("view-main")).toBeVisible();
    const viewer = page.getByTestId("viewer");
    await expect(viewer).toHaveAttribute("data-zoomed", "false");
    await page.keyboard.press("Space");
    await expect(viewer).toHaveAttribute("data-zoomed", "true");
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await page.keyboard.press("Space");
    await expect(viewer).toHaveAttribute("data-zoomed", "false");
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await expectPick(page, "z", [3], "pick");
    await expect(viewer).toHaveAttribute("data-zoomed", "false");
    await expectPick(page, "x", [3], "reject");
    await expect(page.getByTestId("zoom-100")).toHaveAttribute("title", /Space/);
    await expect(page.getByTestId("develop-pick")).toHaveAttribute("title", /Z/);
    // Crop tool: X swaps the orientation instead of rejecting.
    await page.keyboard.press("r");
    await clearCalls(page);
    await page.keyboard.press("x");
    await expect(page.getByTestId("crop-flip").or(page.getByTestId("cropbar-flip")).first()).toHaveAttribute("aria-pressed", "true");
    expect((await calls(page, "set_pick")).length).toBe(0);
    await page.keyboard.press("Escape");
  });

  test("cheat sheet lists Z / X / Space and no longer Z for zoom", async ({ page }) => {
    await openApp(page, 200);
    await page.keyboard.press("?");
    const sheet = page.getByTestId("cheat-sheet");
    await expect(sheet).toBeVisible();
    await expect(sheet).toContainText("Pick");
    await expect(sheet).toContainText("Zoom Fit / 1:1");
    await expect(sheet).not.toContainText("Z, Space");
    await expect(sheet).not.toContainText("Zoom to 1:1");
  });
});
