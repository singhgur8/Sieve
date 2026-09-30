import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openApp, openHome, shot } from "./helpers";

// Mock rules: `?autosync=1` = auto-sync on (dirty photos stay pending until `__mockXmpFlush()` emulates a pass);
// `?errors=1` = photos with id % 10 === 5 cannot be written (read-only) or are missing (id % 10 === 3).
const pill = (page: Page) => page.getByTestId("xmp-status");

test.use({ viewport: { width: 1280, height: 800 } });

test("pending -> saved when the auto-sync pass lands", async ({ page }) => {
  await openApp(page, 200, "&autosync=1");
  await expect(pill(page)).toHaveAttribute("data-state", "pending");
  await expect(pill(page)).toContainText("Saving 5…");
  await page.getByTestId("xmp-status-button").click();
  await expect(page.getByTestId("xmp-headline")).toContainText("Saving 5 photos");
  await shot(page, "xmp-pending-popover");
  await page.keyboard.press("Escape");
  await page.evaluate(() => window.__mockXmpFlush!());
  await expect(pill(page)).toHaveAttribute("data-state", "saved");
  await expect(pill(page)).toHaveText("Saved");
  await shot(page, "xmp-saved");
});

test("running writer shows Saving… even without dirty photos", async ({ page }) => {
  await openApp(page, 200, "&autosync=1");
  await page.evaluate(() => {
    window.__mockXmpRunning = true;
    window.__mockXmpFlush!();
  });
  await expect(pill(page)).toHaveAttribute("data-state", "pending");
  await expect(pill(page)).toHaveText("Saving…");
});

test("off state offers Save now and turning auto-sync on", async ({ page }) => {
  await openApp(page, 200);
  await expect(pill(page)).toHaveAttribute("data-state", "off");
  await page.getByTestId("xmp-status-button").click();
  await shot(page, "xmp-off-popover");
  await clearCalls(page);
  await page.getByTestId("xmp-auto-toggle").check();
  await expect.poll(async () => (await calls(page, "set_xmp_auto_sync")).length).toBe(1);
  expect((await calls(page, "set_xmp_auto_sync"))[0].args).toEqual({ enabled: true });
  await expect(pill(page)).toHaveAttribute("data-state", "pending");
});

test("errors: count, failure list with reasons, Retry", async ({ page }) => {
  await openApp(page, 200, "&errors=1&autosync=1");
  await expect(pill(page)).toHaveAttribute("data-state", "error");
  const failed = Number(await pill(page).getAttribute("data-failed"));
  expect(failed).toBeGreaterThan(0);
  await expect(pill(page)).toContainText(`${failed} errors`);
  await page.getByTestId("xmp-status-button").click();
  const list = page.getByTestId("xmp-failures");
  await expect(list.locator("li")).toHaveCount(failed);
  await expect(page.getByTestId("xmp-failure-5")).toContainText("DSC00005");
  await expect(page.getByTestId("xmp-failure-5")).toContainText("read-only");
  await shot(page, "xmp-error-popover");
  await clearCalls(page);
  await page.getByTestId("xmp-retry").click();
  await expect.poll(async () => (await calls(page, "write_xmp_all_dirty")).length).toBe(1);
  // Unwritable photos stay failed.
  await expect(pill(page)).toHaveAttribute("data-state", "error");
});

test("one-time explanation: shown once when a project opens with auto-sync on, re-openable from the popover", async ({ page }) => {
  await openHome(page, 201, "&autosync=1");
  await page.getByTestId("project-open-1").click();
  await expect(page.getByTestId("grid-toolbar")).toBeVisible();
  const dlg = page.getByTestId("xmp-explainer");
  await expect(dlg).toBeVisible();
  await expect(dlg).toContainText("starting point");
  await expect(dlg).toContainText("never modified");
  await shot(page, "xmp-explainer");
  await page.getByTestId("xmp-explainer-dismiss").click();
  await expect(dlg).toHaveCount(0);
  await expect.poll(async () => (await calls(page, "set_ui_prefs")).length).toBe(1);
  expect((await calls(page, "set_ui_prefs"))[0].args).toEqual({ prefs: { xmpExplainerSeen: true } });
  // Re-open from the popover.
  await page.getByTestId("xmp-status-button").click();
  await page.getByTestId("xmp-explain").click();
  await expect(dlg).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dlg).toHaveCount(0);
});

test("no explanation when auto-sync is off (mock default)", async ({ page }) => {
  await openHome(page, 201);
  await page.getByTestId("project-open-1").click();
  await expect(page.getByTestId("grid-toolbar")).toBeVisible();
  await expect(page.getByTestId("xmp-explainer")).toHaveCount(0);
});
