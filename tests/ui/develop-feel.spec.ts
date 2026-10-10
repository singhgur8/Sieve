// Phase 8d "Develop editing feel": typed / arrow-key slider values, Cmd/Ctrl+C / V, generic Auto, hold the changed dot,
// preset highlight and hover preview on the main image.
import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openApp } from "./helpers";

async function openDevelop(page: Page, id = 1) {
  await openApp(page, 200);
  await page.getByTestId(`cell-${id}`).click();
  await page.keyboard.press("d");
  await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", String(id));
  await expect(page.getByTestId("view-main")).toBeVisible();
  await expect(page.getByTestId("histogram")).toHaveAttribute("data-empty", "false");
}
const saved = async (page: Page) => (await calls(page, "save_adjustments")).filter((c) => c.args.label !== undefined);
const val = (page: Page, id: string) => page.getByTestId(`slider-value-${id}`);

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    if (!sessionStorage.getItem("seeded")) {
      localStorage.clear();
      sessionStorage.setItem("seeded", "1");
    }
  });
});

test.describe("slider values", () => {
  test("click the number, type, Enter commits; Esc cancels; Tab moves on", async ({ page }) => {
    await openDevelop(page);
    await clearCalls(page);
    await val(page, "exposure").click();
    await page.getByTestId("slider-edit-exposure").fill("1.5");
    await page.keyboard.press("Enter");
    await expect(val(page, "exposure")).toHaveText("+1.50");
    await expect.poll(async () => (await saved(page)).length).toBe(1);
    await val(page, "exposure").click();
    await page.getByTestId("slider-edit-exposure").fill("3");
    await page.keyboard.press("Escape");
    await expect(val(page, "exposure")).toHaveText("+1.50");
    // Tab commits and opens the next slider's value.
    await val(page, "contrast").click();
    await page.getByTestId("slider-edit-contrast").fill("25");
    await page.keyboard.press("Tab");
    await expect(val(page, "contrast")).toHaveText("+25");
    await expect(page.getByTestId("slider-edit-highlights")).toBeFocused();
  });

  test("hover + Up / Down, Shift x10, Alt fine; Left / Right still change photo; double-click label resets", async ({ page }) => {
    await openDevelop(page);
    await page.getByTestId("slider-row-contrast").hover();
    await page.keyboard.press("ArrowUp");
    await expect(val(page, "contrast")).toHaveText("+1");
    await page.keyboard.press("Shift+ArrowUp");
    await expect(val(page, "contrast")).toHaveText("+11");
    await page.keyboard.press("ArrowDown");
    await expect(val(page, "contrast")).toHaveText("+10");
    await page.getByTestId("slider-row-temp").hover();
    const t0 = parseInt((await val(page, "temp").textContent())!);
    await page.keyboard.press("Alt+ArrowUp");
    await expect.poll(async () => parseInt((await val(page, "temp").textContent())!) - t0).toBeGreaterThan(0);
    await expect.poll(async () => parseInt((await val(page, "temp").textContent())!) - t0).toBe(10);
    await expect.poll(async () => (await saved(page)).length).toBeGreaterThan(0);
    // One history entry for a burst of nudges on one slider.
    await page.getByTestId("slider-row-contrast").hover();
    await clearCalls(page);
    for (let i = 0; i < 3; i++) await page.keyboard.press("ArrowUp");
    await expect(val(page, "contrast")).toHaveText("+13");
    await expect.poll(async () => (await saved(page)).length).toBe(1);
    // Left / Right (hovering, not focused) navigate photos.
    await page.keyboard.press("ArrowRight");
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "2");
    await page.keyboard.press("ArrowLeft");
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "1");
    await page.getByTestId("slider-label-contrast").dblclick();
    await expect(val(page, "contrast")).toHaveText("0");
  });

  test("focused slider: arrows step, Shift x10", async ({ page }) => {
    await openDevelop(page);
    await page.getByTestId("slider-saturation").focus();
    await page.keyboard.press("ArrowRight");
    await expect(val(page, "saturation")).toHaveText("+1");
    await page.keyboard.press("Shift+ArrowRight");
    await expect(val(page, "saturation")).toHaveText("+11");
    await page.keyboard.press("ArrowLeft");
    await expect(val(page, "saturation")).toHaveText("+10");
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "1");
  });
});

test.describe("copy / paste keys", () => {
  for (const mod of ["Meta", "Control"]) {
    test(`${mod}+C then next photo ${mod}+V pastes as one undoable step`, async ({ page }) => {
      await openDevelop(page);
      await page.getByTestId("slider-exposure").fill("1.25");
      await page.getByTestId("slider-exposure").evaluate((el) => (el as HTMLElement).blur());
      await expect.poll(async () => (await saved(page)).length).toBe(1);
      await page.keyboard.press(`${mod}+c`);
      await page.keyboard.press("ArrowRight");
      await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "2");
      await expect(val(page, "exposure")).toHaveText("0.00");
      await clearCalls(page);
      await page.keyboard.press(`${mod}+v`);
      await expect(val(page, "exposure")).toHaveText("+1.25");
      expect((await calls(page, "paste_settings")).length).toBe(1);
      await expect(page.getByTestId("paste-undo-batch")).toBeVisible();
      await expect(page.getByTestId("paste-undo-batch").locator("../..")).toContainText("Pasted");
      await page.getByTestId("paste-undo-batch").click();
      await expect(val(page, "exposure")).toHaveText("0.00");
    });
  }

  test("Cmd+C is left alone while typing a slider value", async ({ page }) => {
    await openDevelop(page);
    await val(page, "exposure").click();
    await page.getByTestId("slider-edit-exposure").fill("2");
    await clearCalls(page);
    await page.keyboard.press("Meta+c");
    await expect(page.getByTestId("slider-edit-exposure")).toHaveValue("2");
    await page.keyboard.press("Escape");
  });
});

test.describe("Auto", () => {
  test("generic Auto sits at the top of Basic, always enabled, runs auto_light + auto_tone (vibrance / saturation) as one entry", async ({ page }) => {
    await openDevelop(page);
    const auto = page.getByTestId("auto-all");
    await expect(auto).toBeEnabled();
    const ya = (await auto.boundingBox())!.y;
    expect(ya).toBeLessThan((await page.getByTestId("bw-mode").boundingBox())!.y);
    await expect(auto).toHaveAttribute("title", /my style/);
    await clearCalls(page);
    await auto.click();
    await expect(val(page, "exposure")).toHaveText("+0.35");
    await expect.poll(async () => (await saved(page)).length).toBe(1);
    expect((await saved(page))[0].args.label).toBe("Auto");
    // v21.1: light (WB + six tone sliders) from `auto_light`, then vibrance / saturation on the merged settings.
    expect((await calls(page, "auto_light")).length).toBe(1);
    const tone = await calls(page, "auto_tone");
    expect(tone.length).toBe(1);
    expect(tone[0].args.keys).toEqual(["vibrance", "saturation"]);
    expect((tone[0].args.adjustments as { exposure: number }).exposure).toBe(0.35);
    expect((await calls(page, "auto_white_balance")).length).toBe(0);
    await expect(val(page, "vibrance")).toHaveText("+10");
  });
});

test.describe("hold the changed dot", () => {
  test("holding shows a without_fields render on the main image; release restores it", async ({ page }) => {
    await openDevelop(page);
    await page.getByTestId("slider-exposure").fill("2");
    await page.getByTestId("slider-exposure").evaluate((el) => (el as HTMLElement).blur());
    await expect.poll(async () => (await saved(page)).length).toBe(1);
    const dot = page.getByTestId("section-dot-basic");
    await expect(dot).toBeVisible();
    await clearCalls(page);
    const box = (await dot.boundingBox())!;
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await expect(page.getByTestId("hover-preview-label")).toHaveText(/Without Basic/);
    const [c] = await calls(page, "render_preview_variant");
    expect(c.args.variant.kind).toBe("without_fields");
    expect(c.args.variant.fields).toContain("exposure");
    expect(c.args.options.slot).toBe("preview");
    await page.mouse.up();
    await expect(page.getByTestId("hover-preview")).toHaveCount(0);
    expect(await saved(page)).toHaveLength(0); // nothing saved, no history
  });

  test("keyboard: hold Space on the dot", async ({ page }) => {
    await openDevelop(page);
    await page.getByTestId("slider-exposure").fill("2");
    await page.getByTestId("slider-exposure").evaluate((el) => (el as HTMLElement).blur());
    await page.getByTestId("section-dot-basic").focus();
    await page.keyboard.down(" ");
    await expect(page.getByTestId("hover-preview-label")).toHaveText(/Without Basic/);
    await page.keyboard.up(" ");
    await expect(page.getByTestId("hover-preview")).toHaveCount(0);
  });
});

test.describe("presets", () => {
  async function makePresets(page: Page) {
    await openDevelop(page);
    for (const [name, ev] of [["Warm Look", 1.25], ["Cool Look", -0.5]] as const) {
      await page.getByTestId("slider-exposure").fill(String(ev));
      await page.getByTestId("slider-exposure").evaluate((el) => (el as HTMLElement).blur());
      await page.getByTestId("preset-add").click();
      await page.getByTestId("preset-save").click();
      await page.getByTestId("preset-name").fill(name);
      await page.getByTestId("fields-confirm").click();
      await expect(page.getByTestId("preset-list")).toContainText(name);
    }
    await page.getByTestId("film-2").click();
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "2");
  }

  test("hover previews on the main image, mouse-out restores, click applies and highlights, a slider clears it", async ({ page }) => {
    await makePresets(page);
    const warm = page.getByTestId("preset-list").getByText("Warm Look");
    const mainSrc = await page.getByTestId("view-main").getAttribute("src");
    await clearCalls(page);
    await warm.hover();
    await expect(page.getByTestId("hover-preview-label")).toHaveText("Preview: Warm Look");
    await expect(page.getByTestId("navigator-preview-label")).toContainText("Warm Look");
    const [c] = await calls(page, "render_preview_variant");
    expect(c.args.variant.kind).toBe("preset");
    expect(await page.getByTestId("view-main").getAttribute("src")).toBe(mainSrc);
    expect((await calls(page, "apply_preset")).length).toBe(0);
    await page.mouse.move(700, 400);
    await expect(page.getByTestId("hover-preview")).toHaveCount(0);
    // Quick pass across both presets: only the last one is shown.
    await warm.hover();
    await page.getByTestId("preset-list").getByText("Cool Look").hover();
    await expect(page.getByTestId("hover-preview-label")).toHaveText("Preview: Cool Look");
    await page.mouse.move(700, 400);
    // Click applies, highlights.
    await warm.click();
    await expect(val(page, "exposure")).toHaveText("+1.25");
    await expect(page.locator('[data-applied="true"]')).toContainText("Warm Look");
    await expect(page.getByTestId("hover-preview")).toHaveCount(0);
    // Changing a preset-owned slider clears the highlight.
    await page.getByTestId("slider-exposure").fill("0.5");
    await page.getByTestId("slider-exposure").evaluate((el) => (el as HTMLElement).blur());
    await expect(page.locator('[data-applied="true"]')).toHaveCount(0);
  });
});
