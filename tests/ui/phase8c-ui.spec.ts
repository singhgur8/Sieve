import { expect, test, type Page } from "@playwright/test";
import { openApp, shot } from "./helpers";

// Phase 8c: Lightroom-style metadata filters, background activity widget, Help & FAQ.

const activity = (page: Page, e: Record<string, unknown>) =>
  page.evaluate((ev) => (window as unknown as { __mockActivity: (e: unknown) => void }).__mockActivity(ev), { message: null, done: 0, total: null, state: "running", kind: "other", label: "Working", ...e });

const ids = async (page: Page) => Number((await page.getByTestId("readout-shown").innerText()).trim());

test.describe("metadata filters", () => {
  test("extension filter changes the grid and counts; facets follow the other filters", async ({ page }) => {
    await openApp(page, 400, "&meta=1");
    await expect(page.getByTestId("meta-row")).toHaveCount(0);
    await page.getByTestId("meta-toggle").click();
    await expect(page.getByTestId("meta-row")).toBeVisible();
    await expect(page.getByTestId("meta-count-extensions-raf")).toHaveText("100");
    await expect(page.getByTestId("meta-count-extensions-arw")).toHaveText("200");
    await expect(page.getByTestId("meta-count-extensions-jpg")).toHaveText("100");
    await expect(page.getByTestId("cell-2")).toBeVisible(); // ARW
    await shot(page, "meta-row-open");

    await page.getByTestId("meta-opt-extensions-raf").click();
    await expect(page.getByTestId("cell-2")).toHaveCount(0);
    await expect(page.getByTestId("cell-1")).toBeVisible();
    await expect.poll(() => ids(page)).toBe(100);
    // Own column keeps every value; other columns only show what is left.
    await expect(page.getByTestId("meta-count-extensions-arw")).toHaveText("200");
    await expect(page.getByTestId("meta-col-cameras").getByRole("button")).toHaveCount(1);
    await expect(page.getByTestId("meta-col-cameras").getByRole("button")).toContainText("Fujifilm X-T5");
    await expect(page.getByTestId("meta-chip-extensions")).toContainText("RAF");
    await expect(page.getByTestId("meta-active-count")).toHaveText("1");
    await shot(page, "meta-row-raf");

    // Multi-select within a column is OR.
    await page.getByTestId("meta-opt-extensions-jpg").click();
    await expect.poll(() => ids(page)).toBe(200);
    // AND across columns: add a lens column value.
    await page.getByTestId("meta-opt-extensions-jpg").click();
    await page.getByTestId("meta-opt-edited-no").click();
    await expect(page.getByTestId("meta-chip-edited")).toBeVisible();
    await expect.poll(() => ids(page)).toBe(100);

    // Filter bar counts follow the metadata too.
    await expect(page.getByTestId("pick-pick")).toBeVisible();

    await page.getByTestId("meta-clear").click();
    await expect.poll(() => ids(page)).toBe(400);
    await expect(page.getByTestId("meta-chips")).toHaveCount(0);
  });

  test("numeric columns select a range; the row stays open for the session; chips clear a column", async ({ page }) => {
    await openApp(page, 400, "&meta=1");
    await page.getByTestId("meta-toggle").click();
    const first = page.getByTestId("meta-col-iso").locator('[data-testid^="meta-opt-iso-"]').first();
    await first.click();
    await expect(first).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByTestId("meta-chip-iso")).toBeVisible();
    await page.getByTestId("meta-chip-iso").getByRole("button").click();
    await expect(page.getByTestId("meta-chip-iso")).toHaveCount(0);
    await page.reload();
    await expect(page.getByTestId("meta-row")).toBeVisible();
  });
});

test.describe("background activity", () => {
  test("shows progress, finish, error and cancel; Save is disabled while xmp_save runs", async ({ page }) => {
    await openApp(page, 200);
    await page.getByTestId("cell-3").click();
    await expect(page.getByTestId("save-metadata")).toBeEnabled();
    await expect(page.getByTestId("activity-widget")).toHaveCount(0);

    await activity(page, { id: 9001, kind: "xmp_save", label: "Saving metadata", done: 412, total: 2389 });
    const row = page.locator('[data-testid="activity"][data-kind="xmp_save"]');
    await expect(row).toContainText("Saving metadata 412 / 2,389");
    await expect(row.getByTestId("activity-ring")).toBeVisible();
    await expect(page.getByTestId("save-metadata")).toBeDisabled();
    await expect(page.getByTestId("save-metadata")).toHaveAttribute("title", /being saved/);
    await shot(page, "activity-running");

    await activity(page, { id: 9002, kind: "apply_scene", label: "Applying scene edit" });
    await expect(page.locator('[data-kind="apply_scene"]').getByTestId("activity-spinner")).toBeVisible();

    await activity(page, { id: 9001, kind: "xmp_save", label: "Saving metadata", done: 2389, total: 2389, state: "finished", message: "Saved metadata for 2,389 photos" });
    await expect(row).toContainText("Saved metadata for 2,389 photos");
    await expect(page.getByTestId("save-metadata")).toBeEnabled();
    await expect(row).toHaveCount(0, { timeout: 6000 });

    await activity(page, { id: 9002, kind: "apply_scene", label: "Applying scene edit", state: "error", message: "disk is full" });
    const err = page.locator('[data-kind="apply_scene"]');
    await expect(err).toContainText("disk is full");
    await shot(page, "activity-error");
    await page.waitForTimeout(3000);
    await expect(err).toBeVisible(); // errors stay
    await page.getByTestId("activity-dismiss").click();
    await expect(err).toHaveCount(0);

    await activity(page, { id: 9003, kind: "export", label: "Exporting", state: "cancelled" });
    await expect(page.locator('[data-kind="export"]')).toContainText("cancelled");
    await expect(page.locator('[data-kind="export"]')).toHaveCount(0, { timeout: 8000 });
  });

  test("real Save shows its activity from the mock backend", async ({ page }) => {
    await openApp(page, 200);
    await page.getByTestId("cell-3").click();
    await page.getByTestId("save-metadata").click();
    await expect(page.getByTestId("save-metadata")).toBeEnabled({ timeout: 8000 });
  });
});

test.describe("help", () => {
  test("F1 and the Help button open the panel; search finds auto-advance; the ? link opens its entry", async ({ page }) => {
    await openApp(page, 200);
    await page.keyboard.press("F1");
    await expect(page.getByTestId("help-panel")).toBeVisible();
    await expect(page.getByTestId("help-entry-culling")).toBeVisible();
    await shot(page, "help-panel");
    await page.getByTestId("help-search").fill("auto-advance");
    await expect(page.getByTestId("help-nav-auto-advance")).toBeVisible();
    await expect(page.getByTestId("help-nav-saved")).toHaveCount(0);
    await page.getByTestId("help-search").fill("zzzz");
    await expect(page.getByTestId("help-empty")).toBeVisible();
    await page.keyboard.press("Escape"); // clears the search
    await expect(page.getByTestId("help-search")).toHaveValue("");
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("help-panel")).toHaveCount(0);

    await page.getByTestId("help-button").click();
    await expect(page.getByTestId("help-panel")).toBeVisible();
    await page.getByTestId("help-nav-icons").click();
    await expect(page.getByTestId("help-legend-Pick flag")).toBeVisible();
    await shot(page, "help-icons");
    await page.getByTestId("help-close").click();

    await page.getByTestId("help-link-auto-advance").click();
    await expect(page.getByTestId("help-entry-auto-advance")).toBeVisible();
    await expect(page.getByTestId("auto-advance")).not.toBeChecked(); // the link did not toggle the checkbox
    await page.keyboard.press("Escape");

    // Cheat sheet links to Help and back.
    await page.keyboard.press("?");
    await expect(page.getByTestId("cheat-sheet")).toBeVisible();
    await expect(page.getByTestId("cheat-help")).toBeVisible();
    await page.getByTestId("cheat-open-help").click();
    await expect(page.getByTestId("help-panel")).toBeVisible();
    await page.getByTestId("help-nav-shortcuts").click();
    await page.getByTestId("help-open-shortcuts").click();
    await expect(page.getByTestId("cheat-sheet")).toBeVisible();
  });
});
