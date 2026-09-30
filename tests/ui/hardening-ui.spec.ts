// Phase 8 / IPC v13 UI: missing originals (badge, facet, Locate folder), catalog health banner and backup restore,
// remedies by error kind. Fixtures: `?missing=N` (images 1..N missing, folder 1 holds 1..25), `?health=read_only|replaced`.
import { expect, test } from "@playwright/test";
import { openApp, shot } from "./helpers";

const P = "10y-";

test.describe("missing originals", () => {
  test("badge from missingSinceMs, Missing facet filters and hides at zero", async ({ page }) => {
    await openApp(page, 50, "&missing=5");
    for (const id of [1, 5]) await expect(page.getByTestId(`health-${id}`)).toContainText("Missing");
    await expect(page.getByTestId("health-6")).toHaveCount(0);
    const chip = page.getByTestId("filter-missing");
    await expect(chip).toContainText("Missing");
    await expect(chip).toContainText("5");
    await shot(page, `${P}missing-facet`);
    await chip.click();
    await expect(chip).toHaveAttribute("data-state", "include");
    await expect(page.locator('[data-testid^="cell-"]')).toHaveCount(5);
    await page.getByTestId("clear-filters").click();
    await expect(page.locator('[data-testid^="cell-"]').nth(6)).toBeVisible();

    await openApp(page, 50);
    await expect(page.getByTestId("filter-missing")).toHaveCount(0);
  });

  test("Locate folder from Develop relinks, toasts, and reopens the image", async ({ page }) => {
    await openApp(page, 50, "&missing=5");
    await page.getByTestId("cell-1").click();
    await page.keyboard.press("d");
    await expect(page.getByTestId("original-unavailable")).toBeVisible();
    await expect(page.getByTestId("original-unavailable-message")).toContainText("Original file is missing");
    await expect(page.getByTestId("error-locate")).toBeVisible(); // the failure toast carries the remedy too
    await shot(page, `${P}locate-develop`);
    await page.getByTestId("original-locate").click();
    await expect(page.getByTestId("notice").filter({ hasText: "Relinked 25 photos" })).toBeVisible();
    await expect(page.getByTestId("notice")).not.toContainText("still missing");
    await expect(page.getByTestId("original-unavailable")).toHaveCount(0);
    await expect(page.getByTestId("view-main")).toBeVisible();
    await shot(page, `${P}locate-done`);
    await page.keyboard.press("g");
    await expect(page.getByTestId("health-1")).toHaveCount(0);
    await expect(page.getByTestId("filter-missing")).toHaveCount(0);
  });

  test("partial relink reports what is still missing; none found shows the message; cancelled picker does nothing", async ({ page }) => {
    await openApp(page, 50, "&missing=5");
    await page.evaluate(() => (window.__mockPickDir = null));
    await page.getByTestId("locate-folder").click();
    await expect(page.getByTestId("notice")).toHaveCount(0);
    await expect(page.getByTestId("error")).toHaveCount(0);

    await page.evaluate(() => (window.__mockPickDir = "/Volumes/empty"));
    await page.getByTestId("locate-folder").click();
    await expect(page.getByTestId("error")).toContainText("None of the 25 photos");
    await shot(page, `${P}locate-none`);
    await page.getByRole("button", { name: "Dismiss error" }).click();

    await page.evaluate(() => (window.__mockPickDir = "/Volumes/partial/ceremony"));
    await page.getByTestId("locate-folder").click();
    await expect(page.getByTestId("notice")).toContainText("Relinked 24 photos · 1 still missing");
    await expect(page.getByTestId("filter-missing")).toContainText("1");
    await expect(page.getByTestId("health-1")).toContainText("Missing");
    await expect(page.getByTestId("health-2")).toHaveCount(0);
  });

  test("More menu entry and the folder filter button locate a folder", async ({ page }) => {
    await openApp(page, 50, "&missing=5");
    await page.getByTestId("more-menu").click();
    await page.getByTestId("locate-folder-menu").click();
    await expect(page.getByTestId("notice")).toContainText("Relinked 25 photos");
    await page.getByTestId("more-menu").click();
    await expect(page.getByTestId("locate-folder-menu")).toBeVisible();
    await page.keyboard.press("Escape");
    await page.getByTestId("folder-select").selectOption("2");
    await expect(page.getByTestId("locate-selected-folder")).toBeVisible();
  });

  test("loupe shows the badge and a Locate button", async ({ page }) => {
    await openApp(page, 50, "&missing=5");
    await page.getByTestId("cell-2").dblclick();
    await expect(page.getByTestId("loupe-health-2")).toContainText("Missing");
    await shot(page, `${P}loupe-missing`);
    await page.getByTestId("loupe-locate").click();
    await expect(page.getByTestId("notice")).toContainText("Relinked 25 photos");
    await expect(page.getByTestId("loupe-health-2")).toHaveCount(0);
  });
});

test.describe("catalog health", () => {
  test("read-only catalog: banner, restore dialog with backups, Enter confirms, relaunch notice", async ({ page }) => {
    await openApp(page, 50, "&health=read_only");
    const banner = page.getByTestId("health-banner");
    await expect(banner).toHaveAttribute("data-status", "read_only");
    await expect(page.getByTestId("health-message")).toContainText("opened read-only");
    await expect(page.getByTestId("health-dismiss")).toHaveCount(0); // persistent
    await shot(page, `${P}health-readonly`);

    await page.getByTestId("restore-backup").click();
    const dlg = page.getByTestId("restore-dialog");
    await expect(dlg).toBeVisible();
    await expect(page.getByTestId("restore-list").locator("li")).toHaveCount(3);
    await expect(page.getByTestId("restore-item-1")).toBeChecked();
    await expect(page.getByTestId("restore-list")).toContainText("MB");
    await shot(page, `${P}restore-dialog`);
    await page.keyboard.press("Escape");
    await expect(dlg).toHaveCount(0);

    await page.getByTestId("restore-backup").click();
    await page.getByTestId("restore-item-2").check();
    await page.keyboard.press("Enter");
    await expect(dlg).toHaveCount(0);
    const pending = page.getByTestId("restore-pending");
    await expect(pending).toContainText("Relaunch Sieve to finish restoring");
    await expect(page.getByTestId("health-banner")).toHaveCount(0);
    await shot(page, `${P}restore-pending`);
    const staged = await page.evaluate(async () => {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const ipc: any = await import("/src/ipc/index.ts");
      return (await ipc.unwrap(ipc.commands.getCatalogState())).health.restorePending;
    });
    expect(staged).toBe(true);
  });

  test("a refused write in a read-only catalog does not add a second banner", async ({ page }) => {
    await openApp(page, 50, "&health=read_only");
    await page.getByTestId("cell-2").click();
    await page.keyboard.press("p");
    await expect.poll(() => page.evaluate(() => window.__ipcLog.some((c) => c.cmd === "set_pick"))).toBe(true);
    await expect(page.getByTestId("issue-banner")).toHaveCount(0); // the health banner already says it
    await expect(page.getByTestId("health-banner")).toBeVisible();
  });

  test("replaced catalog: dismissible banner offering Restore backup", async ({ page }) => {
    await openApp(page, 50, "&health=replaced");
    const banner = page.getByTestId("health-banner");
    await expect(banner).toHaveAttribute("data-status", "replaced");
    await expect(banner).toContainText("a new, empty catalog was created");
    await shot(page, `${P}health-replaced`);
    await page.getByTestId("health-dismiss").click();
    await expect(banner).toHaveCount(0);
  });

  test("healthy catalog shows no banner", async ({ page }) => {
    await openApp(page, 50);
    await expect(page.getByTestId("health-banner")).toHaveCount(0);
    await expect(page.getByTestId("restore-pending")).toHaveCount(0);
  });

  test("a catalog_read_only error kind opens the restore flow from the banner", async ({ page }) => {
    await openApp(page, 50);
    await page.evaluate(() => {
      window.__mockFail = { set_pick: { kind: "catalog_read_only", message: "The catalog is read-only, so changes cannot be saved." } };
    });
    await page.getByTestId("cell-2").click();
    await page.keyboard.press("p");
    const b = page.getByTestId("issue-banner");
    await expect(b).toHaveAttribute("data-category", "catalog_readonly");
    await b.getByTestId("issue-restore").click();
    await expect(page.getByTestId("restore-dialog")).toBeVisible();
    await page.getByTestId("restore-confirm").click();
    await expect(page.getByTestId("restore-pending")).toBeVisible();
  });
});

test.describe("remedies by error kind", () => {
  test("disk_full / read_only kinds get their wording", async ({ page }) => {
    await openApp(page, 50);
    await page.evaluate(() => {
      window.__mockFail = { set_rating: { kind: "disk_full", message: "Could not write /x/DSC00002.xmp: no space." } };
    });
    await page.getByTestId("cell-2").click();
    await page.keyboard.press("3");
    const e = page.getByTestId("error");
    await expect(e).toHaveAttribute("data-category", "disk_full");
    await expect(e).not.toContainText("Locate");
    await page.evaluate(() => {
      window.__mockFail = { set_rating: { kind: "read_only", message: "Could not write /x/DSC00002.xmp: no." } };
    });
    await page.keyboard.press("4");
    await expect(page.getByTestId("error")).toHaveAttribute("data-category", "read_only");
  });
});
