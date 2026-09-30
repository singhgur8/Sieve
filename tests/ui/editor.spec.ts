import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openApp, openSection, shot } from "./helpers";

async function openDevelop(page: Page, id = 1) {
  await openApp(page, 200);
  await page.getByTestId(`cell-${id}`).click();
  await page.keyboard.press("d");
  await expect(page.getByTestId("develop-view")).toBeVisible();
  await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", String(id));
  await expect(page.getByTestId("view-main")).toBeVisible();
  await expect(page.getByTestId("histogram")).toHaveAttribute("data-empty", "false");
}

/** Sets a range input the way a completed drag would: input events, then release (blur). */
async function setSlider(page: Page, id: string, value: number) {
  const s = page.getByTestId(`slider-${id}`);
  await s.fill(String(value));
  await s.evaluate((el) => (el as HTMLElement).blur());
}

const saves = async (page: Page) => (await calls(page, "save_adjustments")).filter((c) => c.args.label !== undefined);

test.describe("develop", () => {
  test("opens with D / Develop tab, filmstrip switches images, G returns", async ({ page }) => {
    await openDevelop(page, 3);
    await expect(page.getByTestId("develop-filename")).toContainText("DSC00003");
    await expect(page.getByTestId("adjust-panel")).toBeVisible();
    await expect(page.getByTestId("left-panel")).toBeVisible();
    // As-shot WB seeds the sliders.
    await expect(page.getByTestId("slider-value-temp")).toHaveText("5200 K");
    await expect(page.getByTestId("slider-value-tint")).toHaveText("+8");
    await shot(page, "2x-editor-01-open");

    await page.getByTestId("film-4").click();
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "4");
    await expect(page.getByTestId("view-main")).toHaveAttribute("alt", /Render 4 /);
    expect((await calls(page, "get_develop_info")).map((c) => c.args.id)).toContain(4);
    expect((await calls(page, "prepare_develop")).length).toBeGreaterThan(0);

    await page.keyboard.press("ArrowLeft");
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "3");
    await page.keyboard.press("g");
    await expect(page.getByTestId("develop-view")).toHaveCount(0);
    await page.getByTestId("mode-develop").click();
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await page.getByTestId("mode-grid").click();
    await expect(page.getByTestId("develop-view")).toHaveCount(0);
  });

  test("dragging Exposure renders per input with latest values and saves once on release", async ({ page }) => {
    await openDevelop(page);
    await clearCalls(page);
    const slider = page.getByTestId("slider-exposure");
    const box = (await slider.boundingBox())!;
    const y = box.y + box.height / 2;
    await page.mouse.move(box.x + box.width / 2, y);
    await page.mouse.down();
    for (const f of [0.58, 0.66, 0.74, 0.82]) {
      await page.mouse.move(box.x + box.width * f, y, { steps: 2 });
      await page.waitForTimeout(40);
    }
    // Not saved while the drag is in progress.
    expect(await saves(page)).toHaveLength(0);
    await page.mouse.up();

    const renders = (await calls(page, "render_preview")).filter((c) => c.args.options.slot === "main");
    expect(renders.length).toBeGreaterThanOrEqual(3);
    const exps = renders.map((c) => c.args.adjustments.exposure as number);
    expect(new Set(exps).size).toBeGreaterThanOrEqual(3);
    expect(renders.every((c) => c.args.id === 1 && c.args.options.maxEdge >= 64)).toBe(true);

    await expect.poll(async () => (await saves(page)).length).toBe(1);
    const [save] = await saves(page);
    expect(save.args.label).toMatch(/^Exposure [+-]\d/); // history labels carry the value (UX2 P2-5)
    expect(save.args.id).toBe(1);
    const finalValue = Number(await slider.inputValue());
    expect(finalValue).toBeGreaterThan(1);
    expect(save.args.adjustments.exposure).toBeCloseTo(finalValue, 2);
    expect(exps[exps.length - 1]).toBeCloseTo(finalValue, 2);
    await expect(page.getByTestId("history-list")).toContainText("Exposure");
    await expect(page.getByTestId("view-main")).toHaveAttribute("src", new RegExp(`e=${finalValue.toFixed(2)}`));
    await expect(page.getByTestId("viewer-toolbar")).toHaveAttribute("data-render-ms", /^\d+$/);
    await shot(page, "2x-editor-02-exposure");

    // The filmstrip shows the edited badge after the library refresh.
    await expect(page.getByTestId("film-edited-1")).toBeVisible();
  });

  test("double-clicking a slider label resets it and records the reset", async ({ page }) => {
    await openDevelop(page);
    await setSlider(page, "contrast", 40);
    await expect(page.getByTestId("slider-value-contrast")).toHaveText("+40");
    await clearCalls(page);
    await page.getByTestId("slider-label-contrast").dblclick();
    await expect(page.getByTestId("slider-value-contrast")).toHaveText("0");
    await expect.poll(async () => (await saves(page)).length).toBe(1);
    const [save] = await saves(page);
    expect(save.args.label).toBe("Contrast 0");
    expect(save.args.adjustments.contrast).toBe(0);
    // Double-clicking the slider itself also resets.
    await setSlider(page, "shadows", -30);
    await page.getByTestId("slider-shadows").dblclick();
    await expect(page.getByTestId("slider-value-shadows")).toHaveText("0");
  });

  test("Cmd+Z / Shift+Cmd+Z undo and redo update the sliders and history", async ({ page }) => {
    await openDevelop(page);
    await setSlider(page, "exposure", 1.5);
    await setSlider(page, "contrast", 25);
    await expect.poll(async () => (await saves(page)).length).toBe(2);
    await expect(page.getByTestId("slider-value-contrast")).toHaveText("+25");

    await page.keyboard.press("Meta+z");
    await expect(page.getByTestId("slider-value-contrast")).toHaveText("0");
    await expect(page.getByTestId("slider-value-exposure")).toHaveText("+1.50");
    expect((await calls(page, "undo_adjustments")).length).toBe(1);
    await page.keyboard.press("Meta+z");
    await expect(page.getByTestId("slider-value-exposure")).toHaveText("0.00");
    await shot(page, "2x-editor-03-undo");
    await page.keyboard.press("Meta+Shift+z");
    await expect(page.getByTestId("slider-value-exposure")).toHaveText("+1.50");
    expect((await calls(page, "redo_adjustments")).length).toBe(1);

    // Clicking a history entry jumps there; the panel buttons work too.
    const original = page.getByTestId("history-list").getByRole("button", { name: "Original" });
    await original.click();
    await expect(page.getByTestId("slider-value-exposure")).toHaveText("0.00");
    expect((await calls(page, "goto_history")).length).toBe(1);
    await page.getByTestId("redo").click();
    await expect(page.getByTestId("slider-value-exposure")).toHaveText("+1.50");
    // The undone render is requested with the restored adjustments.
    await expect
      .poll(async () => (await calls(page, "render_preview")).filter((c) => c.args.options.slot === "main").pop()!.args.adjustments.exposure)
      .toBeCloseTo(1.5, 2);
  });

  test("copy with a fields mask, paste onto another image", async ({ page }) => {
    await openDevelop(page);
    await setSlider(page, "exposure", 2);
    await setSlider(page, "contrast", 30);
    await page.keyboard.press("Meta+Shift+c");
    await expect(page.getByTestId("fields-dialog")).toBeVisible();
    await page.getByTestId("fields-none").click();
    await page.getByTestId("field-exposure").check();
    await shot(page, "2x-editor-04-fields-dialog");
    await page.getByTestId("fields-confirm").click();
    await expect(page.getByTestId("fields-dialog")).toHaveCount(0);

    await page.getByTestId("film-2").click();
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "2");
    await expect(page.getByTestId("slider-value-exposure")).toHaveText("0.00");
    await clearCalls(page);
    await page.keyboard.press("Meta+Shift+v");
    await expect.poll(async () => (await calls(page, "paste_settings")).length).toBe(1);
    const [paste] = await calls(page, "paste_settings");
    expect(paste.args.ids).toEqual([2]);
    expect(paste.args.fields).toEqual(["exposure"]);
    expect(paste.args.adjustments.exposure).toBe(2);
    await expect(page.getByTestId("slider-value-exposure")).toHaveText("+2.00");
    await expect(page.getByTestId("slider-value-contrast")).toHaveText("0");
    await expect(page.getByTestId("history-list")).toContainText("Paste Settings");
  });

  test("sync settings to a multi-selection and reset", async ({ page }) => {
    await openDevelop(page);
    await setSlider(page, "vibrance", 35);
    await expect(page.getByTestId("sync-settings")).toHaveCount(0); // one photo: Previous instead of Sync…
    await expect(page.getByTestId("previous-settings")).toBeVisible();
    await page.getByTestId("film-1").click({ modifiers: ["Meta"] });
    await page.getByTestId("film-2").click({ modifiers: ["Meta"] });
    await page.getByTestId("film-3").click({ modifiers: ["Meta"] });
    // Meta-toggling image 1 off would change the active image; active is the last click. Re-activate 1.
    await page.getByTestId("film-1").click({ modifiers: ["Meta"] });
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "1");
    await expect(page.getByTestId("sync-settings")).toBeVisible();
    await expect(page.getByTestId("reset-all")).toHaveText(/Reset \(\d+\)/);
    await clearCalls(page);
    await page.getByTestId("sync-settings").click();
    await page.getByTestId("fields-confirm").click();
    await expect.poll(async () => (await calls(page, "sync_settings")).length).toBe(1);
    const [sync] = await calls(page, "sync_settings");
    expect(sync.args.sourceId).toBe(1);
    expect([...sync.args.targetIds].sort()).toEqual([2, 3]);
    expect(sync.args.fields.length).toBeGreaterThan(10);

    await page.getByTestId("reset-all").click();
    await expect(page.getByTestId("slider-value-vibrance")).toHaveText("0");
    expect((await calls(page, "reset_adjustments"))[0].args.ids.length).toBeGreaterThan(0);
  });

  test("save and apply a preset, delete it", async ({ page }) => {
    await openDevelop(page);
    await setSlider(page, "exposure", 1.25);
    await setSlider(page, "saturation", -20);
    await page.getByTestId("preset-add").click();
    await page.getByTestId("preset-save").click();
    await page.getByTestId("preset-name").fill("Warm Look");
    await page.getByTestId("fields-confirm").click();
    await expect(page.getByTestId("preset-list")).toContainText("Warm Look");
    const [saved] = await calls(page, "save_preset");
    expect(saved.args.name).toBe("Warm Look");
    expect(saved.args.id).toBeNull();
    expect(saved.args.adjustments.exposure).toBe(1.25);
    await shot(page, "2x-editor-05-presets");

    await page.getByTestId("film-2").click();
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "2");
    await expect(page.getByTestId("slider-value-exposure")).toHaveText("0.00");
    await clearCalls(page);
    await page.getByTestId("preset-list").getByText("Warm Look").click();
    await expect(page.getByTestId("slider-value-exposure")).toHaveText("+1.25");
    await expect(page.getByTestId("slider-value-saturation")).toHaveText("-20");
    const [apply] = await calls(page, "apply_preset");
    expect(apply.args.ids).toEqual([2]);
    await expect(page.getByTestId("history-list")).toContainText("Preset: Warm Look");

    await page.getByTestId("preset-list").locator("li").first().hover();
    await page.locator('[data-testid^="preset-delete-"]:not([data-testid*="confirm"]):not([data-testid*="cancel"])').first().click();
    await page.locator('[data-testid^="preset-delete-confirm-"]').click();
    await expect(page.getByTestId("preset-list")).toHaveCount(1);
    await expect(page.getByText("No presets yet")).toBeVisible();
  });

  test("LUT profiles set lut {id, amount 0..200}; picking another profile removes the LUT", async ({ page }) => {
    await openDevelop(page);
    await page.getByTestId("profile-browse").click();
    await expect(page.getByTestId("profile-browser")).toBeVisible();
    await expect(page.locator('[data-testid^="lut-item-"]')).toHaveCount(2);
    await clearCalls(page);
    await page.getByTestId("lut-item-film-warm").click();
    await expect.poll(async () => (await saves(page)).length).toBe(1);
    let [save] = await saves(page);
    expect(save.args.label).toBe("LUT");
    expect(save.args.adjustments.lut).toEqual({ id: "film-warm", amount: 100 });
    await expect(page.getByTestId("view-main")).toHaveAttribute("src", /lut=film-warm/);

    await setSlider(page, "lut-amount", 60);
    await expect.poll(async () => (await saves(page)).length).toBe(2);
    [, save] = await saves(page);
    expect(save.args.adjustments.lut).toEqual({ id: "film-warm", amount: 60 });
    await shot(page, "2x-editor-06-lut");

    // v14: Amount goes up to 200.
    await setSlider(page, "lut-amount", 150);
    await expect.poll(async () => (await saves(page)).pop()?.args.adjustments.lut.amount).toBe(150);
    await page.getByTestId("lut-item-teal-orange").click();
    await expect(page.getByTestId("lut-item-teal-orange")).toHaveClass(/bg-sky-800/);
    expect((await saves(page)).pop()!.args.adjustments.lut).toEqual({ id: "teal-orange", amount: 100 });
    // A camera profile replaces the LUT.
    await page.getByTestId("profile-item-Camera Standard").click();
    await expect(page.getByTestId("slider-lut-amount")).toHaveCount(0);
    const lastSave = (await saves(page)).pop()!;
    expect(lastSave.args.adjustments.lut).toBeNull();
    expect(lastSave.args.adjustments.profile.cameraProfile).toBe("Camera Standard");
  });

  test("white balance mode: Custom seeds from as-shot, Temp edit switches to custom", async ({ page }) => {
    await openDevelop(page);
    await page.getByTestId("wb-select").selectOption("custom");
    await expect.poll(async () => (await saves(page)).length).toBe(1);
    expect((await saves(page))[0].args.adjustments.whiteBalance).toEqual({ mode: "custom", temperatureK: 5200, tint: 8 });
    await setSlider(page, "tint", 40);
    const last = (await saves(page)).pop()!;
    expect(last.args.label).toMatch(/^Tint [+-]?\d/);
    expect(last.args.adjustments.whiteBalance).toMatchObject({ mode: "custom", tint: 40, temperatureK: 5200 });
    await page.getByTestId("wb-select").selectOption("as_shot");
    await expect(page.getByTestId("slider-value-tint")).toHaveText("+8");
  });

  test("resetting Temp/Tint returns to As Shot when values match as-shot", async ({ page }) => {
    await openDevelop(page);
    await setSlider(page, "tint", 40);
    await setSlider(page, "temp", 0.7);
    await page.getByTestId("slider-label-tint").dblclick();
    let wb = (await saves(page)).pop()!.args.adjustments.whiteBalance;
    expect(wb).toMatchObject({ mode: "custom", tint: 8 });
    await page.getByTestId("slider-label-temp").dblclick();
    wb = (await saves(page)).pop()!.args.adjustments.whiteBalance;
    expect(wb).toEqual({ mode: "as_shot" });
    await expect(page.getByTestId("slider-value-temp")).toHaveText("5200 K");
  });

  test("End/PageDown keys commit the slider", async ({ page }) => {
    await openDevelop(page);
    const s = page.getByTestId("slider-exposure");
    await s.focus();
    await clearCalls(page);
    await page.keyboard.press("End");
    await expect.poll(async () => (await saves(page)).length).toBe(1);
    expect((await saves(page))[0].args.adjustments.exposure).toBeGreaterThan(0);
    await page.keyboard.press("PageDown");
    await expect.poll(async () => (await saves(page)).pop()?.args.adjustments.exposure).toBeLessThan(5);
  });

  test("Color Mixer edits a band and section reset clears it", async ({ page }) => {
    await openDevelop(page);
    await openSection(page, "hsl");
    await setSlider(page, "hsl-hue-blue", 30);
    expect((await saves(page)).pop()!.args.adjustments.hsl.hue.blue).toBe(30);
    await page.getByTestId("hsl-tab-luminance").click();
    await setSlider(page, "hsl-luminance-orange", -25);
    await expect(page.getByTestId("slider-value-hsl-luminance-orange")).toHaveText("-25");
    await shot(page, "2x-editor-07-color-mixer");
    await page.getByTestId("reset-hsl").click();
    await expect(page.getByTestId("slider-value-hsl-luminance-orange")).toHaveText("0");
    const a = (await saves(page)).pop()!.args.adjustments;
    expect(a.hsl.hue.blue).toBe(0);
    expect(a.hsl.luminance.orange).toBe(0);
  });

  test("before/after toggle (\\) requests the before slot", async ({ page }) => {
    await openDevelop(page);
    await setSlider(page, "exposure", 2);
    await clearCalls(page);
    await page.keyboard.press("\\");
    await expect(page.getByTestId("before-badge")).toBeVisible();
    await expect(page.getByTestId("view-before")).toBeVisible();
    const before = (await calls(page, "render_preview")).filter((c) => c.args.options.slot === "before");
    expect(before.length).toBeGreaterThan(0);
    expect(before[0].args.adjustments.exposure).toBe(0);
    await shot(page, "2x-editor-08-before");
    await page.keyboard.press("\\");
    await expect(page.getByTestId("before-badge")).toHaveCount(0);
    await expect(page.getByTestId("view-main")).toBeVisible();

    // Split view shows both.
    await page.getByTestId("split-toggle").click();
    await expect(page.getByTestId("split-handle")).toBeVisible();
    await expect(page.getByTestId("view-before")).toBeVisible();
    await expect(page.getByTestId("view-main")).toBeVisible();
    await shot(page, "2x-editor-09-split");
  });

  test("zoom to 100% renders the detail slot for the visible region, again after panning", async ({ page }) => {
    await openDevelop(page);
    await clearCalls(page);
    await page.keyboard.press("z");
    await expect(page.getByTestId("viewer")).toHaveAttribute("data-zoomed", "true");
    await expect(page.getByTestId("view-detail")).toBeVisible();
    const first = (await calls(page, "render_preview")).filter((c) => c.args.options.slot === "detail");
    expect(first.length).toBeGreaterThan(0);
    const r1 = first[first.length - 1].args.options.region;
    expect(r1.width).toBeGreaterThan(0);
    expect(r1.width).toBeLessThan(1);
    expect(r1.x + r1.width).toBeLessThanOrEqual(1.0001);

    const box = (await page.getByTestId("viewer").boundingBox())!;
    await page.mouse.move(box.x + 300, box.y + 300);
    await page.mouse.down();
    await page.mouse.move(box.x + 100, box.y + 200, { steps: 5 });
    await page.mouse.up();
    await expect.poll(async () => (await calls(page, "render_preview")).filter((c) => c.args.options.slot === "detail").length).toBeGreaterThan(first.length);
    const all = (await calls(page, "render_preview")).filter((c) => c.args.options.slot === "detail");
    const r2 = all[all.length - 1].args.options.region;
    expect(r2.x).toBeGreaterThan(r1.x);
    expect(r2.y).toBeGreaterThan(r1.y);
    await shot(page, "2x-editor-10-zoom");
    await page.keyboard.press("z");
    await expect(page.getByTestId("viewer")).toHaveAttribute("data-zoomed", "false");
  });

  test("stale (out-of-order) render results are ignored", async ({ page }) => {
    await openDevelop(page);
    const alt = await page.getByTestId("view-main").getAttribute("alt");
    const base = Number(/#(\d+)/.exec(alt!)![1]);
    // The next render (seq base+1) answers slowly; the following one answers immediately.
    await page.evaluate((b) => (window.__mockRenderDelay = (seq, slot) => (slot === "main" && seq === b + 1 ? 500 : 0)), base);
    const s = page.getByTestId("slider-exposure");
    await s.fill("1");
    await page.waitForTimeout(60);
    await s.fill("3");
    await expect(page.getByTestId("view-main")).toHaveAttribute("src", /e=3\.00/);
    await expect(page.getByTestId("view-main")).toHaveAttribute("alt", `Render 1 #${base + 2}`);
    await page.waitForTimeout(700); // the slow, older result has arrived by now and must not win
    await expect(page.getByTestId("view-main")).toHaveAttribute("src", /e=3\.00/);
    await expect(page.getByTestId("view-main")).toHaveAttribute("alt", `Render 1 #${base + 2}`);
    const mains = (await calls(page, "render_preview")).filter((c) => c.args.options.slot === "main");
    expect(mains.length).toBeGreaterThanOrEqual(2);
  });
});
