// Phase 8: error and empty states, AI model download UI, error boundaries.
// Failure fixtures come from the mock (`?errors=1`: id % 10 === 3 original missing, 7 thumbnail decode failure,
// 5 sidecar not writable; `window.__mockFail` rejects a command; `window.__mockExportFail` fails export items).
import { expect, test, type Page } from "@playwright/test";
import { openApp, shot } from "./helpers";

const P = "10x-errors-";

async function openMasks(page: Page, id = 1) {
  await page.getByTestId(`cell-${id}`).click();
  await page.keyboard.press("d");
  await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", String(id));
  await expect(page.getByTestId("view-main")).toBeVisible();
  await page.keyboard.press("Shift+W");
  await expect(page.getByTestId("masks-panel")).toBeVisible();
}

test.describe("AI model download", () => {
  test("card, progress, cancel, error + retry, finish refreshes capabilities without a restart", async ({ page }) => {
    await page.addInitScript(() => (window.__mockModelDelay = 25));
    await openApp(page, 50, "&models=missing");
    await openMasks(page);
    const card = page.getByTestId("models-card");
    await expect(card).toContainText("AI masks need a one-time download (~559 MB)");
    await expect(page.getByTestId("mask-create-subject")).toBeDisabled();
    await shot(page, `${P}models-card`);

    // Progress with file i/n, MB and %, then cancel.
    await page.getByTestId("model-download").click();
    await expect(page.getByTestId("model-progress-text")).toContainText(/File \d of 6 · \d+ of 559 MB · \d+%/);
    await shot(page, `${P}models-progress`);
    await page.getByTestId("model-cancel").click();
    await expect(page.getByTestId("model-download")).toBeVisible();
    await expect(page.getByTestId("model-error")).toHaveCount(0); // a user cancel is not an error

    // Failure shows the message and a retry.
    await page.evaluate(() => (window.__mockModelFail = "yolox_m.onnx: SHA-256 mismatch (mock)"));
    await page.getByTestId("model-download").click();
    await expect(page.getByTestId("model-error")).toContainText("SHA-256 mismatch");
    await expect(page.getByTestId("model-download")).toContainText("Retry");
    await shot(page, `${P}models-error`);

    // Retry succeeds: card disappears and the AI tools become available without a reload.
    await page.evaluate(() => {
      window.__mockModelFail = undefined;
      window.__mockModelDelay = 2;
    });
    await page.getByTestId("model-download").click();
    await expect(card).toHaveCount(0);
    await expect(page.getByTestId("mask-create-subject")).toBeEnabled();
    await expect(page.getByTestId("mask-create-subject")).toHaveAttribute("data-unavailable", "false");
  });

  test("Manage AI models dialog in the More menu", async ({ page }) => {
    await page.addInitScript(() => (window.__mockModelDelay = 2));
    await openApp(page, 50, "&models=missing");
    await page.getByTestId("more-menu").click();
    await page.getByTestId("manage-models").click();
    const dlg = page.getByTestId("models-dialog");
    await expect(dlg).toBeVisible();
    await expect(page.getByTestId("models-summary")).toContainText("0 of 6 files installed");
    await expect(page.getByTestId("models-files").locator("li")).toHaveCount(6);
    await shot(page, `${P}models-dialog`);
    await page.getByTestId("model-download").click();
    await expect(page.getByTestId("models-summary")).toContainText("Installed (6 files", { timeout: 15_000 });
    await page.keyboard.press("Escape");
    await expect(dlg).toHaveCount(0);
  });
});

test.describe("missing and unreadable files", () => {
  test("Develop shows a placeholder for a missing original; the grid and loupe get a Missing badge", async ({ page }) => {
    await openApp(page, 200, "&errors=1");
    await page.getByTestId("cell-3").click();
    await page.keyboard.press("d");
    const ph = page.getByTestId("original-unavailable");
    await expect(ph).toBeVisible();
    await expect(ph).toHaveAttribute("data-kind", "missing");
    await expect(page.getByTestId("original-unavailable-message")).toContainText("Original file is missing or was moved: /shoot/DSC00003.ARW");
    await expect(page.getByTestId("original-locate")).toHaveCount(0); // needs IPC v13 relocate_folder
    await expect(page.getByTestId("error")).toBeVisible(); // transient toast alongside the inline state
    await shot(page, `${P}missing-develop`);
    await page.getByTestId("original-retry").click();
    await expect(ph).toBeVisible(); // still gone
    await page.keyboard.press("g");
    await expect(page.getByTestId("health-3")).toContainText("Missing");
    await expect(page.getByTestId("health-13")).toHaveCount(0);
    await shot(page, `${P}missing-cell`);
    await page.getByTestId("cell-3").dblclick();
    await expect(page.getByTestId("loupe-health-3")).toContainText("Missing");
    await shot(page, `${P}missing-loupe`);
  });

  test("decode failure shows a cell placeholder with the reason", async ({ page }) => {
    await page.route(/\/mock\/(thumb|preview)\/\d+\.jpg/, (r) => r.fulfill({ contentType: "image/svg+xml", body: "<svg xmlns='http://www.w3.org/2000/svg'/>" }));
    await page.goto("/?mock=200&errors=1");
    const cell = page.getByTestId("thumb-failed-7");
    await expect(cell).toBeVisible();
    await expect(cell).toHaveAttribute("title", /Could not decode \/shoot\/DSC00007\.ARW/);
    await expect(cell).toContainText("No preview");
    await shot(page, `${P}decode-cell`);
    await page.getByTestId("cell-7").dblclick();
    await expect(page.getByTestId("preview-unavailable")).toContainText("No preview for this photo");
    await shot(page, `${P}decode-loupe`);
  });

  test("a cached preview that fails to load shows a placeholder, not a broken image", async ({ page }) => {
    await page.route(/\/mock\/(thumb|preview)\/\d+\.jpg/, (r) => r.abort());
    await page.goto("/?mock=50");
    await expect(page.getByTestId("thumb-broken").first()).toContainText("Preview unavailable");
    await shot(page, `${P}thumb-broken`);
  });
});

test.describe("failures while saving and exporting", () => {
  test("XMP write failure: cell badge, persistent toast with the reason", async ({ page }) => {
    await openApp(page, 200, "&errors=1");
    await expect(page.getByTestId("xmp-error-5")).toBeVisible();
    await page.getByTestId("cell-5").click();
    await page.getByTestId("save-metadata").click();
    const t = page.getByTestId("notice");
    await expect(t).toContainText("1 sidecar could not be written");
    await expect(t).toContainText("the volume is read-only");
    await expect(page.getByTestId("xmp-error-5")).toBeVisible();
    await shot(page, `${P}xmp-readonly`);
  });

  test("export job card explains a full disk", async ({ page }) => {
    await page.addInitScript(() => {
      window.__mockExportManual = true;
      window.__mockExportFail = "Could not write /Volumes/Card/Export/DSC00005.jpg: the disk is full. Free up space or choose another destination.";
    });
    await openApp(page, 200);
    await page.getByTestId("cell-5").click();
    await page.keyboard.press("Meta+Shift+E");
    await expect(page.getByTestId("export-dialog")).toBeVisible();
    await page.getByTestId("export-choose-folder").click();
    await page.getByTestId("export-go").click();
    await page.evaluate(() => window.__mockExportStep!(1000));
    const card = page.getByTestId("export-summary");
    await expect(card).toBeVisible({ timeout: 10_000 });
    await expect(page.getByTestId("export-summary-text")).toContainText("Export failed");
    await expect(page.getByTestId("export-failure-hint")).toContainText("disk is full");
    await shot(page, `${P}export-disk-full`);
  });

  test("catalog read-only error becomes a persistent banner", async ({ page }) => {
    await openApp(page, 50);
    await page.evaluate(() => {
      window.__mockFail = {
        set_pick: {
          kind: "database",
          message: "The catalog is damaged (mock) and was opened read-only, so changes cannot be saved. Quit Sieve and restore the backup /cache/catalog.sqlite.bak-1 (newest of 2), or copy it over /cache/catalog.sqlite.",
        },
      };
    });
    await page.getByTestId("cell-2").click();
    await page.keyboard.press("p");
    const b = page.getByTestId("issue-banner");
    await expect(b).toBeVisible();
    await expect(b).toHaveAttribute("data-category", "catalog_readonly");
    await expect(b).toContainText("restore the backup");
    await shot(page, `${P}catalog-readonly`);
    await page.getByTestId("issue-banner-dismiss").click();
    await expect(b).toHaveCount(0);
  });

  test("analysis failures are explained on the progress bar", async ({ page }) => {
    await openApp(page, 50);
    await page.evaluate(async () => {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const { events }: any = await import("/src/ipc/index.ts");
      await events.analysisProgress.emit({ done: 2, total: 10, failed: 1 });
      await events.analysisFailed.emit({ imageId: 4, reason: "face model could not run: out of memory" });
      await events.analysisProgress.emit({ done: 10, total: 10, failed: 1 });
    });
    const bar = page.getByTestId("analysis-failed");
    await expect(bar).toContainText("1 failed to analyze: face model could not run");
    await shot(page, `${P}analysis-failed`);
    await page.getByTestId("analysis-dismiss").click();
    await expect(page.getByTestId("analysis-bar")).toHaveCount(0);
  });
});

test.describe("error boundary", () => {
  test("a view that throws shows the fallback and recovers with Reload view; the rest of the app stays usable", async ({ page }) => {
    const logged: string[] = [];
    page.on("console", (m) => m.type() === "error" && logged.push(m.text()));
    await page.addInitScript(() => (window.__mockCorruptImages = true));
    await openApp(page, 20).catch(() => {}); // cells never render, so openApp's cell wait fails
    const fb = page.getByTestId("error-boundary-library");
    await expect(fb).toBeVisible();
    await expect(fb).toContainText("Something went wrong");
    await expect(fb.getByTestId("error-boundary-reload")).toContainText("Reload view");
    await expect(page.getByTestId("top-bar")).toBeVisible();
    expect(logged.some((l) => l.includes("[sieve] Library view crashed"))).toBe(true);
    await shot(page, `${P}boundary`);
    await page.evaluate(() => (window.__mockCorruptImages = false));
    await fb.getByTestId("error-boundary-reload").click();
    await expect(page.getByTestId("cell-1")).toBeVisible();
    await expect(fb).toHaveCount(0);
  });
});

test.describe("empty states", () => {
  test("no results for a filter, no presets, no history, no masks", async ({ page }) => {
    await openApp(page, 5);
    await page.getByTestId("pick-reject").click();
    await expect(page.getByTestId("grid-empty-text")).toContainText("No photos match the current filters");
    await expect(page.getByTestId("empty-clear-filters")).toBeVisible();
    await shot(page, `${P}empty-filter`);
    await page.getByTestId("empty-clear-filters").click();
    await page.getByTestId("cell-1").click();
    await page.keyboard.press("d");
    await expect(page.getByTestId("left-panel")).toContainText("No presets yet. Save the current settings");
    await expect(page.getByTestId("left-panel")).toContainText("No edits yet");
    await page.keyboard.press("Shift+W");
    await expect(page.getByTestId("mask-empty")).toContainText("No masks yet");
    await shot(page, `${P}empty-develop`);
  });
});
