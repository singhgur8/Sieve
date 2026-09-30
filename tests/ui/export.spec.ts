import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openApp, shot } from "./helpers";

/** Selects photos 5..8 (id 7 fails in the mock) and opens the dialog with Cmd+Shift+E. */
async function openDialog(page: Page, { manual = false }: { manual?: boolean } = {}) {
  if (manual) await page.addInitScript(() => (window.__mockExportManual = true));
  await openApp(page, 200);
  await page.getByTestId("cell-5").click();
  for (let i = 0; i < 3; i++) await page.keyboard.press("Shift+ArrowRight");
  await page.keyboard.press("Meta+Shift+E");
  await expect(page.getByTestId("export-dialog")).toBeVisible();
  await expect(page.getByTestId("export-count")).toHaveText("4 photos");
}

async function pickFolder(page: Page) {
  await page.getByTestId("export-choose-folder").click();
  await expect(page.getByTestId("export-dest-path")).toHaveText("/mock/export/Smith Wedding");
}

const step = (page: Page, n = 1) => page.evaluate((k) => window.__mockExportStep!(k), n);

test.describe("export", () => {
  test("dialog lists built-in presets, gates export on a destination, capabilities grey out HEIC", async ({ page }) => {
    await openDialog(page);
    await expect(page.getByTestId("export-preset--1")).toBeVisible();
    await expect(page.getByTestId("export-preset--2")).toContainText("Web 2048");
    await expect(page.getByTestId("export-preset--3")).toContainText("Print TIFF");
    await expect(page.getByTestId("export-go")).toBeDisabled();
    await expect(page.getByTestId("export-format").locator("option[value=heic]")).toHaveAttribute("disabled", "");
    await expect(page.getByTestId("export-format").locator("option[value=webp]")).not.toHaveAttribute("disabled", "");
    await page.getByTestId("export-preset--2").click();
    await expect(page.getByTestId("export-resize-px")).toHaveValue("2048");
    await expect(page.getByTestId("export-sharpen-media")).toHaveValue("screen");
    await expect(page.getByTestId("export-metadata")).toHaveValue("copyright_only");
    await page.getByTestId("export-preset--3").click();
    await expect(page.getByTestId("export-format")).toHaveValue("tiff");
    await expect(page.getByTestId("export-bitdepth")).toHaveValue("16");
    await expect(page.getByTestId("export-colorspace")).toHaveValue("adobe_rgb");
    await pickFolder(page);
    await expect(page.getByTestId("export-go")).toBeEnabled();
    await shot(page, "3x-export-01-dialog");
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("export-dialog")).toHaveCount(0);
  });

  test("editing every setting calls exportImages with the exact settings", async ({ page }) => {
    await openDialog(page);
    await pickFolder(page);
    await page.getByTestId("export-quality").fill("72");
    await expect(page.getByTestId("export-quality-value")).toHaveText("72");
    await page.getByTestId("export-chroma").selectOption("420");
    await page.getByTestId("export-colorspace").selectOption("display_p3");
    await page.getByTestId("export-resize-mode").selectOption("long_edge");
    await page.getByTestId("export-resize-px").fill("3000");
    await page.getByTestId("export-dont-enlarge").uncheck();
    await page.getByTestId("export-ppi").fill("240");
    await page.getByTestId("export-sharpen-media").selectOption("matte");
    await page.getByTestId("export-sharpen-amount").selectOption("high");
    await page.getByTestId("export-template").fill("Smith-{seq:4}_{filename}");
    await page.getByTestId("export-start-number").fill("10");
    await page.getByTestId("export-collision").selectOption("skip");
    await page.getByTestId("export-subfolder").fill("Web/Batch 1");
    await page.getByTestId("export-metadata").selectOption("all");
    await page.getByTestId("export-remove-location").check();
    await page.getByTestId("export-copyright").fill("(c) Smith Photo");
    await expect(page.getByTestId("export-template-example")).toContainText("Smith-0010_DSC00005.jpg");
    await shot(page, "3x-export-02-edited");

    await clearCalls(page);
    await page.getByTestId("export-go").click();
    await expect(page.getByTestId("export-dialog")).toHaveCount(0);
    const [call] = await calls(page, "export_images");
    expect(call.args.ids).toEqual([5, 6, 7, 8]);
    expect(call.args.presetName).toBe("Client JPEG full-res sRGB q90");
    expect(call.args.settings).toEqual({
      format: { kind: "jpeg", quality: 72, chromaSubsampling: "420" },
      colorSpace: "display_p3",
      resize: { mode: { kind: "long_edge", px: 3000 }, dontEnlarge: false, resolutionPpi: 240 },
      sharpening: { media: "matte", amount: "high" },
      naming: { template: "Smith-{seq:4}_{filename}", startNumber: 10, collision: "skip" },
      destination: { kind: "folder", path: "/mock/export/Smith Wedding" },
      subfolder: "Web/Batch 1",
      metadata: { include: "all", removeLocation: true, includeKeywords: true, copyright: "(c) Smith Photo", creator: null },
    });
  });

  test("format variants: TIFF, PNG, WebP; megapixels and width x height resize; source folder", async ({ page }) => {
    await openDialog(page);
    await page.getByTestId("export-dest-kind").selectOption("source_folder");
    await expect(page.getByTestId("export-go")).toBeEnabled();

    await page.getByTestId("export-format").selectOption("tiff");
    await page.getByTestId("export-bitdepth").selectOption("8");
    await page.getByTestId("export-tiff-compression").selectOption("zip");
    await page.getByTestId("export-resize-mode").selectOption("megapixels");
    await page.getByTestId("export-resize-mp").fill("8.5");
    await clearCalls(page);
    await page.getByTestId("export-go").click();
    let [call] = await calls(page, "export_images");
    expect(call.args.settings.format).toEqual({ kind: "tiff", bitDepth: "8", compression: "zip" });
    expect(call.args.settings.resize.mode).toEqual({ kind: "megapixels", mp: 8.5 });
    expect(call.args.settings.destination).toEqual({ kind: "source_folder" });

    await page.getByTestId("export-button").click();
    await page.getByTestId("export-dest-kind").selectOption("source_folder");
    await page.getByTestId("export-format").selectOption("png");
    await page.getByTestId("export-bitdepth").selectOption("16");
    await page.getByTestId("export-resize-mode").selectOption("width_height");
    await page.getByTestId("export-resize-width").fill("1920");
    await page.getByTestId("export-resize-height").fill("1080");
    await clearCalls(page);
    await page.getByTestId("export-go").click();
    [call] = await calls(page, "export_images");
    expect(call.args.settings.format).toEqual({ kind: "png", bitDepth: "16" });
    expect(call.args.settings.resize.mode).toEqual({ kind: "width_height", width: 1920, height: 1080 });

    await page.getByTestId("export-button").click();
    await page.getByTestId("export-dest-kind").selectOption("source_folder");
    await page.getByTestId("export-format").selectOption("webp");
    await page.getByTestId("export-quality").fill("60");
    await clearCalls(page);
    await page.getByTestId("export-go").click();
    [call] = await calls(page, "export_images");
    expect(call.args.settings.format).toEqual({ kind: "webp", quality: 60, lossless: false });
  });

  test("filename template chips insert tokens and validate live", async ({ page }) => {
    await openDialog(page);
    await page.getByTestId("export-template").fill("");
    await expect(page.getByTestId("export-template-example")).toContainText("empty");
    await page.getByTestId("export-token-date").click();
    await page.getByTestId("export-token-filename").click();
    await expect(page.getByTestId("export-template")).toHaveValue("{date}{filename}");
    await expect(page.getByTestId("export-template-example")).toContainText("20260601DSC00005.jpg");
    await page.getByTestId("export-template").fill("{nope}");
    await expect(page.getByTestId("export-template-example")).toContainText("Unknown token");
    await page.getByTestId("export-dest-kind").selectOption("source_folder");
    await expect(page.getByTestId("export-go")).toBeDisabled();
    await page.getByTestId("export-template").fill("{filename}-{rating}");
    await page.getByTestId("export-format").selectOption("tiff");
    await expect(page.getByTestId("export-template-example")).toContainText("DSC00005-");
    await expect(page.getByTestId("export-template-example")).toContainText(".tif");
    await expect(page.getByTestId("export-go")).toBeEnabled();
  });

  test("warns about existing files from planExport", async ({ page }) => {
    await page.addInitScript(() => (window.__mockExportManual = true));
    await openApp(page, 200);
    await page.getByTestId("cell-9").click();
    for (let i = 0; i < 2; i++) await page.keyboard.press("Shift+ArrowRight");
    await page.keyboard.press("Meta+Shift+E");
    await page.getByTestId("export-dest-kind").selectOption("source_folder");
    await expect(page.getByTestId("export-plan-warning")).toContainText("1 file already exist");
    await page.getByTestId("export-collision").selectOption("overwrite");
    await expect(page.getByTestId("export-plan-warning")).toContainText("overwritten");
    expect((await calls(page, "plan_export")).length).toBeGreaterThan(0);
  });

  test("save as, update and delete user presets", async ({ page }) => {
    await openDialog(page);
    await page.getByTestId("export-quality").fill("55");
    await page.getByTestId("export-preset-name").fill("Proofs q55");
    await page.getByTestId("export-preset-save-as").click();
    const saved = (await calls(page, "save_export_preset")).at(-1)!;
    expect(saved.args.id).toBeNull();
    expect(saved.args.name).toBe("Proofs q55");
    expect(saved.args.settings.format).toEqual({ kind: "jpeg", quality: 55, chromaSubsampling: "444" });
    const userPreset = page.getByTestId("export-presets").getByRole("button", { name: /Proofs q55/ });
    await expect(userPreset).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByTestId("export-preset-update")).toBeDisabled();

    // Modify + update the selected user preset.
    await page.getByTestId("export-quality").fill("40");
    await expect(page.getByTestId("export-preset-update")).toBeEnabled();
    await page.getByTestId("export-preset-update").click();
    const upd = (await calls(page, "save_export_preset")).at(-1)!;
    expect(upd.args.id).toBe(1);
    expect(upd.args.settings.format.quality).toBe(40);

    // Built-ins are read only.
    await page.getByTestId("export-preset--1").click();
    await expect(page.getByTestId("export-preset-delete")).toBeDisabled();
    await expect(page.getByTestId("export-preset-update")).toBeDisabled();
    await expect(page.getByTestId("export-quality-value")).toHaveText("90");

    // Reload the saved preset, then delete with confirmation.
    await userPreset.click();
    await expect(page.getByTestId("export-quality-value")).toHaveText("40");
    await shot(page, "3x-export-03-presets");
    await page.getByTestId("export-preset-delete").click();
    expect((await calls(page, "delete_export_preset")).length).toBe(0);
    await page.getByTestId("export-preset-delete").click();
    expect((await calls(page, "delete_export_preset")).at(-1)!.args.id).toBe(1);
    await expect(userPreset).toHaveCount(0);
    await expect(page.getByTestId("export-preset--1")).toHaveAttribute("aria-pressed", "true");

    // Duplicate names surface the backend error.
    await page.getByTestId("export-preset-name").fill("web 2048 SRGB");
    await page.getByTestId("export-preset-save-as").click();
    await expect(page.getByTestId("export-error")).toContainText("already exists");
  });

  test("progress events update the panel, finish shows failures and output path", async ({ page }) => {
    await openDialog(page, { manual: true });
    await pickFolder(page);
    await page.getByTestId("export-preset-name").fill("");
    await page.getByTestId("export-go").click();
    const job = page.getByTestId("export-job-1");
    await expect(job).toBeVisible();
    await expect(page.getByTestId("export-progress-text")).toHaveText("0/4");
    await expect(page.getByTestId("export-ring")).toBeVisible();

    await step(page, 1);
    await expect(page.getByTestId("export-progress-text")).toHaveText("1/4");
    await expect(page.getByTestId("export-current-file")).toHaveText("DSC00006.ARW");
    await expect(page.getByTestId("export-bar")).toHaveAttribute("style", /width: 25%/);
    await step(page, 1);
    await expect(page.getByTestId("export-progress-text")).toContainText("2/4");
    await step(page, 1); // id 7 fails
    await expect(page.getByTestId("export-progress-text")).toContainText("1 failed");
    await shot(page, "3x-export-04-progress");

    await step(page, 1);
    await expect(job).toHaveAttribute("data-state", "finished");
    await expect(page.getByTestId("export-summary-text")).toContainText("3 exported, 1 failed");
    await expect(page.getByTestId("export-failure-7")).toContainText("DSC00007.ARW: Decode error (mock)");
    await expect(page.getByTestId("export-output-dir")).toHaveText("/mock/export/Smith Wedding");
    await expect(page.getByTestId("export-ring")).toHaveCount(0);
    await shot(page, "3x-export-05-finished");

    await page.getByTestId("export-job-dismiss").click();
    await expect(page.getByTestId("export-jobs")).toHaveCount(0);
  });

  test("cancel calls cancelExport and shows the cancelled summary", async ({ page }) => {
    await openDialog(page, { manual: true });
    await pickFolder(page);
    await page.getByTestId("export-go").click();
    await step(page, 1);
    await expect(page.getByTestId("export-progress-text")).toHaveText("1/4");
    await clearCalls(page);
    await page.getByTestId("export-cancel").click();
    const [c] = await calls(page, "cancel_export");
    expect(c.args.jobId).toBe(1);
    await expect(page.getByTestId("export-job-1")).toHaveAttribute("data-state", "cancelled");
    await expect(page.getByTestId("export-summary-text")).toContainText("Cancelled: 1 exported");
    await shot(page, "3x-export-06-cancelled");
  });

  test("filtered scope exports every photo in the current filter", async ({ page }) => {
    await openApp(page, 30);
    await page.getByTestId("cell-2").click();
    await page.keyboard.press("Shift+ArrowRight");
    await page.getByTestId("export-button").click();
    await expect(page.getByTestId("export-count")).toHaveText("2 photos");
    await page.getByTestId("export-scope-filtered").check();
    await expect(page.getByTestId("export-count")).toHaveText("30 photos");
    await page.getByTestId("export-dest-kind").selectOption("source_folder");
    await clearCalls(page);
    await page.getByTestId("export-go").click();
    const [call] = await calls(page, "export_images");
    expect(call.args.ids).toHaveLength(30);
  });
});
