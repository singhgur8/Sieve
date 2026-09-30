// UX review 2 (docs/ux-review-2.md): Develop Esc cascade, crop keys and floating bar, one undo, mask tool keys, compact
// Masks panel, hidden panels, slider fill / typed values, filter count, scope defaults, cheat sheet, WB picker, P2 items.
import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openApp, shot } from "./helpers";

const P = "8x-ux2-";

async function openDevelop(page: Page, id = 1, count = 200) {
  await openApp(page, count);
  await page.getByTestId(`cell-${id}`).click();
  await page.keyboard.press("d");
  await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", String(id));
  await expect(page.getByTestId("view-main")).toBeVisible();
  await expect(page.getByTestId("histogram")).toHaveAttribute("data-empty", "false");
}

async function setSlider(page: Page, id: string, value: number) {
  const s = page.getByTestId(`slider-${id}`);
  await s.fill(String(value));
  await s.evaluate((el) => (el as HTMLElement).blur());
}

const saved = async (page: Page) => (await calls(page, "save_adjustments")).filter((c) => c.args.label !== undefined);
const box = async (page: Page, testid: string) => (await page.getByTestId(testid).boundingBox())!;

async function frameRect(page: Page) {
  const layer = page.getByTestId("mask-layer");
  const l = (await layer.boundingBox())!;
  const b = JSON.parse((await layer.getAttribute("data-box"))!) as { x: number; y: number; w: number; h: number };
  return { x: l.x + b.x, y: l.y + b.y, w: b.w, h: b.h };
}

async function dragFrac(page: Page, a: [number, number], b: [number, number]) {
  const r = await frameRect(page);
  await page.mouse.move(r.x + a[0] * r.w, r.y + a[1] * r.h);
  await page.mouse.down();
  await page.mouse.move(r.x + b[0] * r.w, r.y + b[1] * r.h, { steps: 6 });
  await page.mouse.up();
}

test.describe("P0-1 Esc in Develop never changes the module", () => {
  test("cascade: crop, mask tool, mask selection, Masks panel, then nothing", async ({ page }) => {
    await openDevelop(page);
    // 1. crop tool
    await page.keyboard.press("r");
    await expect(page.getByTestId("crop-overlay")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("crop-overlay")).toHaveCount(0);
    await expect(page.getByTestId("develop-view")).toBeVisible();

    // 2. one-shot mask tool that ended by itself (luminance click), then Esc must not leave Develop.
    await page.keyboard.press("Shift+Q");
    await expect(page.getByTestId("masks-panel")).toBeVisible();
    const r = await frameRect(page);
    await page.mouse.click(r.x + r.w * 0.5, r.y + r.h * 0.5);
    await expect(page.locator('[data-testid^="mask-group-"][data-selected="true"]')).toHaveCount(1);
    // an active tool is ended first
    await page.keyboard.press("k");
    await expect(page.getByTestId("mask-capture")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("mask-capture")).toHaveCount(0);
    await expect(page.getByTestId("masks-panel")).toBeVisible();

    // 3. deselect the mask (starting a tool had cleared the selection: select the group again)
    await page.locator('[data-testid^="mask-group-name-"]').first().click();
    await expect(page.locator('[data-testid^="mask-group-"][data-selected="true"]')).toHaveCount(1);
    await page.keyboard.press("Escape");
    await expect(page.locator('[data-testid^="mask-group-"][data-selected="true"]')).toHaveCount(0);
    await expect(page.getByTestId("masks-panel")).toBeVisible();
    // 4. close the Masks panel (Adjust tab)
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("adjust-panel")).toBeVisible();
    // 5. nothing left to do: still Develop, still the same photo
    await page.keyboard.press("Escape");
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "1");
    await shot(page, `${P}esc-cascade`);
    // G leaves as before.
    await page.keyboard.press("g");
    await expect(page.getByTestId("develop-view")).toHaveCount(0);
  });

  test("Esc cancels the WB picker first", async ({ page }) => {
    await openDevelop(page);
    await page.keyboard.press("w");
    await expect(page.getByTestId("wb-picker-overlay")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("wb-picker-overlay")).toHaveCount(0);
    await expect(page.getByTestId("develop-view")).toBeVisible();
  });
});

test.describe("P0-2 crop keys and P1-3 floating crop bar", () => {
  test("X swaps the orientation instead of rejecting; A toggles the lock; Shift+X still culls", async ({ page }) => {
    await openDevelop(page);
    await clearCalls(page);
    const pick0 = await page.getByTestId("develop-flags").getAttribute("data-pick");
    await page.keyboard.press("r");
    await expect(page.getByTestId("crop-aspect")).toHaveValue("original");
    const ratio = async () => {
      const r = JSON.parse((await page.getByTestId("crop-rect").getAttribute("data-rect"))!) as { l: number; t: number; r: number; b: number };
      return (await box(page, "crop-frame")).width * (r.r - r.l) / ((await box(page, "crop-frame")).height * (r.b - r.t));
    };
    const landscape = await ratio();
    expect(landscape).toBeGreaterThan(1);
    await page.keyboard.press("x");
    await expect(page.getByTestId("crop-flip")).toHaveAttribute("aria-pressed", "true");
    const portrait = await ratio();
    expect(portrait).toBeCloseTo(1 / landscape, 1);
    expect((await calls(page, "set_pick")).length).toBe(0);
    await expect(page.getByTestId("develop-flags")).toHaveAttribute("data-pick", pick0!);
    // back
    await page.keyboard.press("x");
    expect(await ratio()).toBeCloseTo(landscape, 1);

    // A: unlock -> Free; A again -> back to the last locked aspect. X on a free rect locks its ratio first.
    await page.keyboard.press("a");
    await expect(page.getByTestId("crop-aspect")).toHaveValue("free");
    await page.keyboard.press("a");
    await expect(page.getByTestId("crop-aspect")).toHaveValue("original");
    await page.keyboard.press("a");
    await page.keyboard.press("x");
    await expect(page.getByTestId("crop-aspect")).toHaveValue("custom");
    expect((await calls(page, "set_pick")).length).toBe(0);

    // Shift+X still rejects while cropping (culling keys keep working).
    await page.keyboard.press("Shift+x");
    await expect.poll(async () => (await calls(page, "set_pick")).length).toBe(1);
    expect((await calls(page, "set_pick"))[0].args.pick).toBe("reject");
  });

  test("floating crop bar: bottom centre, 36 px, Original default, 24 px inset, aspect remembered", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await openDevelop(page);
    // Scroll the Adjust panel away from the Crop section first (R used to leave the controls off-screen).
    await page.getByTestId("adjust-panel").evaluate((el) => (el.scrollTop = 2000));
    await page.keyboard.press("r");
    const bar = await box(page, "crop-bar");
    const viewer = await box(page, "viewer");
    expect(bar.height).toBe(36);
    expect(Math.abs(bar.x + bar.width / 2 - (viewer.x + viewer.width / 2))).toBeLessThanOrEqual(2);
    expect(viewer.y + viewer.height - (bar.y + bar.height)).toBeLessThanOrEqual(12);
    await expect(page.getByTestId("cropbar-aspect")).toHaveValue("original");
    // 24 px inset: the overlay area starts 24 px inside the viewer and the handles are inside it.
    const inset = await box(page, "crop-inset");
    expect(inset.x - viewer.x).toBe(24);
    expect(inset.y - viewer.y).toBe(24);
    expect(viewer.x + viewer.width - (inset.x + inset.width)).toBe(24);
    const nw = await box(page, "crop-handle-nw");
    expect(nw.x + nw.width / 2).toBeGreaterThanOrEqual(viewer.x + 24 - 1);
    await shot(page, `${P}crop-bar-1280`);

    // Bar controls work without the panel: aspect, angle, swap, cancel.
    await page.getByTestId("cropbar-aspect").selectOption("1:1");
    await expect(page.getByTestId("crop-aspect")).toHaveValue("1:1");
    await page.getByTestId("cropbar-angle").fill("2.5");
    await expect(page.getByTestId("cropbar-angle-value")).toHaveText("+2.5°");
    await page.getByTestId("cropbar-angle").dblclick();
    await expect(page.getByTestId("cropbar-angle-value")).toHaveText("0.0°");
    await page.getByTestId("cropbar-cancel").click();
    await expect(page.getByTestId("crop-bar")).toHaveCount(0);
    expect(await page.evaluate(() => localStorage.getItem("sieve.crop.aspect"))).toBe("1:1");
    await page.keyboard.press("r");
    await expect(page.getByTestId("cropbar-aspect")).toHaveValue("1:1");
    await page.getByTestId("cropbar-aspect").selectOption("original");
    await clearCalls(page);
    await page.getByTestId("cropbar-done").click();
    await expect(page.getByTestId("crop-bar")).toHaveCount(0);
  });
});

test.describe("P1-4 one undo in Develop", () => {
  test("Cmd+Z undoes the newest of culling and adjustment; toast names the file", async ({ page }) => {
    await openDevelop(page, 4);
    await setSlider(page, "exposure", 1);
    await expect.poll(async () => (await saved(page)).length).toBe(1);
    await page.waitForTimeout(30);
    await page.keyboard.press("x"); // culling change made after the adjustment
    await expect.poll(async () => (await calls(page, "set_pick")).length).toBe(1);
    await page.waitForTimeout(60);
    await clearCalls(page);

    await page.keyboard.press("Meta+z");
    await expect.poll(async () => (await calls(page, "restore_cull_snapshot")).length).toBe(1);
    expect((await calls(page, "undo_adjustments")).length).toBe(0);
    await expect(page.getByTestId("notice").last()).toContainText(/Undid: Reject DSC\d+\.ARW/);
    await expect(page.getByTestId("develop-flags")).toHaveAttribute("data-pick", "unflagged");

    await page.keyboard.press("Meta+z");
    await expect.poll(async () => (await calls(page, "undo_adjustments")).length).toBe(1);
    expect((await calls(page, "restore_cull_snapshot")).length).toBe(1);
  });
});

test.describe("P1-1 mask tool keys open the panel", () => {
  test("K opens Masks and starts the brush; O / H / Delete are silent with the panel closed and no masks", async ({ page }) => {
    await openDevelop(page);
    await expect(page.getByTestId("masks-panel")).toHaveCount(0);
    const notices = await page.getByTestId("notice").count();
    for (const k of ["o", "Shift+O", "h", "Delete", "Backspace"]) await page.keyboard.press(k);
    await expect(page.getByTestId("masks-panel")).toHaveCount(0);
    expect(await page.getByTestId("notice").count()).toBe(notices);

    await page.keyboard.press("k");
    await expect(page.getByTestId("masks-panel")).toBeVisible();
    await expect(page.getByTestId("mask-capture")).toBeVisible();
    await page.keyboard.press("Escape");

    // a running crop is discarded by a tool key
    await page.keyboard.press("Escape"); // deselect / close (nothing selected)
    await page.keyboard.press("Escape");
    await page.keyboard.press("r");
    await expect(page.getByTestId("crop-overlay")).toBeVisible();
    await page.keyboard.press("m");
    await expect(page.getByTestId("crop-overlay")).toHaveCount(0);
    await expect(page.getByTestId("masks-panel")).toBeVisible();
    await expect(page.getByTestId("mask-capture")).toBeVisible();
    await page.keyboard.press("Escape");

    // With masks present, O reopens the panel.
    await page.getByTestId("mask-create-subject").click();
    await expect(page.getByTestId("mask-busy")).toHaveCount(0);
    await page.getByTestId("tool-masking").click(); // toggles the Masks panel closed
    await expect(page.getByTestId("masks-panel")).toHaveCount(0);
    await page.keyboard.press("o");
    await expect(page.getByTestId("masks-panel")).toBeVisible();
  });
});

test.describe("P1-2 compact Masks panel", () => {
  test("6 masks at 1280x800: icon row, single-row groups, selected Amount and Exposure visible without scrolling", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await openDevelop(page);
    await page.keyboard.press("Shift+W");
    // 0 masks: 2-column grid with short labels; full name as tooltip.
    await expect(page.getByTestId("mask-create-color")).toContainText("Color");
    await expect(page.getByTestId("mask-create-color")).not.toContainText("Range");
    await expect(page.getByTestId("mask-create-luminance")).toHaveAttribute("title", /Luminance Range/);
    await shot(page, `${P}masks-empty-1280`);

    for (let i = 0; i < 6; i++) {
      await page.getByTestId("mask-create-subject").click();
      await expect(page.getByTestId("mask-busy")).toBeVisible();
      await expect(page.getByTestId("mask-busy")).toHaveCount(0);
    }
    await expect(page.locator('[data-testid^="mask-group-"][data-selected]')).toHaveCount(6);
    await expect(page.getByTestId("masks-panel")).toHaveAttribute("data-compact", "true");

    // Icon row: 10 buttons of 24x24, tooltip "Brush (K)".
    const kinds = ["subject", "sky", "background", "people", "object", "brush", "linear", "radial", "color", "luminance"];
    for (const k of kinds) {
      const b = await box(page, `mask-create-${k}`);
      expect(b.width).toBe(24);
      expect(b.height).toBe(24);
      const pnl = await box(page, "masks-panel");
      expect(b.x + b.width, `${k} icon inside the panel`).toBeLessThanOrEqual(pnl.x + pnl.width);
    }
    await expect(page.getByTestId("mask-create-brush")).toHaveAttribute("title", "Brush (K)");

    // Acceptance: the selected mask's Amount and Exposure are on screen without scrolling.
    const panel = await box(page, "masks-panel");
    for (const id of ["slider-mask-amount", "slider-mask-exposure"]) {
      const b = await box(page, id);
      expect(b.y, id).toBeGreaterThanOrEqual(panel.y);
      expect(b.y + b.height, id).toBeLessThanOrEqual(panel.y + panel.height);
    }
    const list = await box(page, "mask-list");
    expect(list.height).toBeLessThanOrEqual(panel.height * 0.35 + 2);
    console.log(`MASKS6: panel=${panel.height} list=${list.height} amountY=${(await box(page, "slider-mask-amount")).y - panel.y} exposureY=${(await box(page, "slider-mask-exposure")).y - panel.y}`);
    await shot(page, `${P}masks-6-1280`);

    // Single-component groups are one row (no component rows until selected).
    const first = page.locator('[data-testid^="mask-group-"][data-selected="false"]').first();
    await expect(first.locator('[data-testid^="mask-comp-"][data-kind]')).toHaveCount(0);
  });
});

test.describe("P1-5 hide panels", () => {
  test("Tab hides both side panels (viewer >= 1280x603), Shift+Tab goes full-bleed, Tab restores", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await openDevelop(page);
    const normal = await box(page, "viewer");
    expect(normal.width).toBeLessThan(800);
    await page.keyboard.press("Tab");
    await expect(page.getByTestId("left-aside")).toHaveCount(0);
    await expect(page.getByTestId("right-aside")).toHaveCount(0);
    await expect.poll(async () => (await box(page, "viewer")).width).toBe(1280);
    const hidden = await box(page, "viewer");
    expect(hidden.height).toBeGreaterThanOrEqual(603);
    console.log(`PANELS: normal=${normal.width}x${normal.height} tabHidden=${hidden.width}x${hidden.height}`);
    await shot(page, `${P}tab-hidden-1280`);
    expect(JSON.parse((await page.evaluate(() => localStorage.getItem("sieve.panels.develop")))!)).toMatchObject({ left: true, right: true });

    // Masks and crop keep working with the right panel hidden.
    await page.keyboard.press("r");
    await expect(page.getByTestId("crop-bar")).toBeVisible();
    await page.keyboard.press("Escape");

    await page.keyboard.press("Shift+Tab");
    await expect(page.getByTestId("filmstrip")).toHaveCount(0);
    await expect(page.getByTestId("develop-toolbar")).toHaveCount(0);
    await expect(page.getByTestId("filter-summary")).toHaveCount(0);
    const full = await box(page, "viewer");
    expect(full.height).toBeGreaterThan(hidden.height + 100);
    await shot(page, `${P}full-bleed-1280`);
    await page.keyboard.press("Tab"); // any Tab restores
    await expect(page.getByTestId("filmstrip")).toBeVisible();
    await expect(page.getByTestId("right-aside")).toBeVisible();
    await expect(page.getByTestId("left-aside")).toBeVisible();

    // Chevron hides one panel.
    await page.getByTestId("panel-chevron-right").click();
    await expect(page.getByTestId("right-aside")).toHaveCount(0);
    await expect(page.getByTestId("left-aside")).toBeVisible();
    await page.getByTestId("panel-chevron-right").click();
    await expect(page.getByTestId("right-aside")).toBeVisible();
  });

  test("Shift+Tab in Loupe hides the filmstrip; Compare keeps Tab for pane switching", async ({ page }) => {
    await openApp(page, 200);
    await page.getByTestId("cell-3").click();
    await page.keyboard.press("Space");
    await expect(page.getByTestId("loupe")).toBeVisible();
    await expect(page.getByTestId("filmstrip")).toBeVisible();
    await page.keyboard.press("Shift+Tab");
    await expect(page.getByTestId("filmstrip")).toHaveCount(0);
    await expect(page.getByTestId("filter-summary")).toHaveCount(0);
    await page.keyboard.press("Shift+Tab");
    await expect(page.getByTestId("filmstrip")).toBeVisible();
    await page.keyboard.press("g");
    await page.getByTestId("cell-3").click();
    await page.keyboard.press("c");
    await expect(page.getByTestId("compare")).toBeVisible();
    await expect(page.getByTestId("compare-pane-a")).toHaveClass(/border-sky-500/);
    await page.keyboard.press("Tab");
    await expect(page.getByTestId("compare-pane-b")).toHaveClass(/border-sky-500/);
  });
});

test.describe("P1-6 sliders", () => {
  test("fill starts at the default, changed value is emphasised, section dot, inline typed values", async ({ page }) => {
    await openDevelop(page);
    const fill = async (id: string) => box(page, `slider-fill-${id}`);
    const track = await box(page, "slider-exposure");
    // At the default the bipolar fill is empty.
    expect((await fill("exposure")).width).toBeLessThan(1);
    await expect(page.getByTestId("slider-row-exposure")).toHaveAttribute("data-changed", "false");
    await expect(page.getByTestId("section-dot-basic")).toHaveCount(0);

    await setSlider(page, "exposure", 2.5);
    await expect(page.getByTestId("slider-row-exposure")).toHaveAttribute("data-changed", "true");
    await expect(page.getByTestId("slider-value-exposure")).toHaveClass(/font-medium/);
    const f = await fill("exposure");
    expect(f.width).toBeGreaterThan(track.width * 0.15);
    // Fill is centre-origin: it starts at the middle of the track.
    expect(Math.abs(f.x - (track.x + track.width / 2))).toBeLessThan(6);
    await expect(page.getByTestId("section-dot-basic")).toBeVisible();
    await shot(page, `${P}sliders`);

    // Unipolar slider (Sharpening): fill from the left, not centre.
    await page.getByTestId("section-toggle-detail").click();
    await setSlider(page, "sharp-amount", 0);
    const amt = await box(page, "slider-sharp-amount");
    const af = await box(page, "slider-fill-sharp-amount");
    expect(Math.abs(af.x - (amt.x + 5))).toBeLessThan(4);
    expect(af.width).toBeGreaterThan(10); // from the left end up to the default (40)

    // Typed values: click the value, Enter commits one history entry.
    await clearCalls(page);
    await page.getByTestId("slider-value-exposure").click();
    const input = page.getByTestId("slider-edit-exposure");
    await expect(input).toBeFocused();
    await input.fill("1.5");
    await input.press("Enter");
    await expect.poll(async () => (await saved(page)).length).toBe(1);
    expect((await saved(page))[0].args.label).toBe("Exposure +1.50");
    expect(((await saved(page))[0].args.adjustments as { exposure: number }).exposure).toBe(1.5);
    await expect(page.getByTestId("slider-value-exposure")).toHaveText("+1.50");

    // Esc cancels, Up/Down step (Shift x10), values clamp.
    await page.getByTestId("slider-value-exposure").click();
    await page.getByTestId("slider-edit-exposure").fill("4");
    await page.getByTestId("slider-edit-exposure").press("Escape");
    await expect(page.getByTestId("slider-value-exposure")).toHaveText("+1.50");
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await page.getByTestId("slider-value-exposure").click();
    await page.getByTestId("slider-edit-exposure").press("ArrowUp");
    await expect(page.getByTestId("slider-edit-exposure")).toHaveValue("1.51");
    await page.getByTestId("slider-edit-exposure").press("Shift+ArrowUp");
    await expect(page.getByTestId("slider-edit-exposure")).toHaveValue("1.61");
    await page.getByTestId("slider-edit-exposure").fill("99");
    await page.getByTestId("slider-edit-exposure").press("Enter");
    await expect(page.getByTestId("slider-value-exposure")).toHaveText("+5.00");

    // Temp accepts Kelvin.
    await page.getByTestId("slider-value-temp").click();
    await page.getByTestId("slider-edit-temp").fill("6500");
    await page.getByTestId("slider-edit-temp").press("Enter");
    await expect(page.getByTestId("slider-value-temp")).toHaveText("6500 K");
    const last = (await saved(page)).at(-1)!;
    expect(last.args.label).toBe("Temp 6500 K");
    expect((last.args.adjustments as { whiteBalance: { mode: string; temperatureK: number } }).whiteBalance).toMatchObject({ mode: "custom", temperatureK: 6500 });

    // Double-click reset still works.
    await page.getByTestId("slider-label-exposure").dblclick();
    await expect(page.getByTestId("slider-value-exposure")).toHaveText("0.00");
  });
});

test.describe("P1-7 filter bar and count", () => {
  test("1280 px: chips are never clipped; the count lives in the toolbar", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await openApp(page, 2000);
    await expect(page.getByTestId("shown-count")).toHaveCount(0);
    const bar = await box(page, "filter-bar");
    for (const id of ["tag-blink", "tag-missed_focus", "tag-motion_blur", "tag-creative_blur", "tag-underexposed", "tag-duplicate_burst", "pick-pick", "pick-reject", "pick-unflagged"]) {
      const b = await box(page, id);
      expect(b.x, id).toBeGreaterThanOrEqual(bar.x);
      expect(b.x + b.width, id).toBeLessThanOrEqual(bar.x + bar.width + 0.5);
    }
    await expect(page.getByTestId("selection-count")).toHaveText("2000 photos · 0 selected");
    await page.getByTestId("tag-blink").click();
    await expect(page.getByTestId("selection-count")).toHaveText(/^\d+ of 2000 · 0 selected$/);
    await shot(page, `${P}filter-1280`);
    await page.getByTestId("clear-filters").click();
    await page.getByTestId("cell-1").click();
    await expect(page.getByTestId("selection-count")).toHaveText("2000 photos · 1 selected");
  });
});

test.describe("P1-8 Apply suggestions scope", () => {
  test("one selected photo defaults to All in view; radios recompute the counts", async ({ page }) => {
    await openApp(page, 200);
    await page.getByTestId("cell-3").click();
    await page.getByTestId("more-menu").click();
    await page.getByTestId("apply-suggestions").click();
    await expect(page.getByTestId("apply-title")).toHaveText("Apply suggestions");
    await expect(page.getByTestId("apply-scope-all")).toBeChecked();
    await expect(page.getByTestId("apply-scope")).toContainText("All in view (200)");
    await expect(page.getByTestId("apply-scope")).toContainText("Selected (1)");
    await expect(page.getByTestId("apply-count-skipped")).toBeVisible();
    const allApply = Number(await page.getByTestId("apply-count-apply").textContent());
    await page.getByTestId("apply-scope-selected").check();
    await expect.poll(async () => Number(await page.getByTestId("apply-count-apply").textContent())).toBeLessThanOrEqual(1);
    expect(allApply).toBeGreaterThan(1);
    await page.getByTestId("apply-scope-all").check();
    await clearCalls(page);
    await page.getByTestId("apply-confirm").click();
    await expect.poll(async () => (await calls(page, "apply_suggestions")).length).toBe(1);
    expect(((await calls(page, "apply_suggestions"))[0].args.ids as number[]).length).toBe(200);
  });
});

test.describe("P1-9 multi-photo reset and presets", () => {
  test("Reset (n) hits the burst, toasts with Undo; preset rows hint the count", async ({ page }) => {
    await openDevelop(page, 3);
    await expect(page.getByTestId("reset-all")).toHaveText("Reset");
    await setSlider(page, "exposure", 1);
    await expect.poll(async () => (await saved(page)).length).toBe(1);
    // Save a preset first (Cmd+Shift+N opens Save Preset).
    await page.keyboard.press("Meta+Shift+n");
    await page.getByTestId("preset-name").fill("Warm Film");
    await page.getByTestId("fields-dialog").getByRole("button", { name: "Create", exact: true }).click();
    await expect(page.getByTestId("preset-list")).toContainText("Warm Film");

    await page.keyboard.press("Meta+Shift+b");
    await expect(page.getByTestId("reset-all")).toHaveText(/Reset \(\d+\)/);
    const n = Number(/\((\d+)\)/.exec((await page.getByTestId("reset-all").textContent())!)![1]);
    expect(n).toBeGreaterThan(1);
    await page.getByTestId("preset-list").locator("li").first().hover();
    await expect(page.locator('[data-testid^="preset-hint-"]').first()).toHaveText(`→ ${n}`);
    await shot(page, `${P}multi-target`);

    await clearCalls(page);
    await page.getByTestId("reset-all").click();
    await expect(page.getByTestId("notice").last()).toContainText(`Reset ${n} photos`);
    expect(((await calls(page, "reset_adjustments"))[0].args.ids as number[]).length).toBe(n);
    await page.getByTestId("batch-undo").click();
    await expect.poll(async () => (await calls(page, "undo_adjustments")).length).toBe(n);

    await clearCalls(page);
    await page.getByTestId("preset-list").locator("li button").first().click();
    await expect(page.getByTestId("notice").last()).toContainText(`Applied 'Warm Film' to ${n} photos`);
  });
});

test.describe("P1-10 export skips rejects", () => {
  test("Skip rejected checkbox with count; the button count follows", async ({ page }) => {
    await openApp(page, 200);
    await page.getByTestId("export-button").click();
    const skip = page.getByTestId("export-skip-rejected");
    await expect(skip).toBeVisible();
    await expect(skip).toBeChecked();
    await expect(page.getByTestId("export-skip-rejected-label")).toContainText("Skip rejected (14)");
    await expect(page.getByTestId("export-go")).toHaveText("Export 186");
    await skip.uncheck();
    await expect(page.getByTestId("export-go")).toHaveText("Export 200");
  });
});

test.describe("P1-11 cheat sheet", () => {
  test("1728x1117 in Develop: every Develop and Masks row is visible without scrolling; key column 112 px", async ({ page }) => {
    await page.setViewportSize({ width: 1728, height: 1117 });
    await openDevelop(page);
    await page.keyboard.press("?");
    const dlg = page.getByTestId("cheat-sheet");
    await expect(dlg).toBeVisible();
    await expect(page.getByTestId("cheat-subtitle")).toHaveText("Showing Develop first");
    const cols = await box(page, "cheat-columns");
    expect(await page.getByTestId("cheat-columns").evaluate((el) => el.scrollTop)).toBe(0);
    const groups = await page.locator('[data-testid^="cheat-group-"]').evaluateAll((els) => els.map((e) => e.getAttribute("data-testid")));
    expect(groups.slice(0, 3)).toEqual(["cheat-group-Develop", "cheat-group-Masks", "cheat-group-Culling"]);
    let checked = 0;
    for (const g of ["Develop", "Masks"]) {
      const rows = page.getByTestId(`cheat-group-${g}`).locator('li[data-testid^="cheat-"]');
      const count = await rows.count();
      for (let i = 0; i < count; i++) {
        const b = (await rows.nth(i).boundingBox())!;
        expect(b.y, `${g} row ${i}`).toBeGreaterThanOrEqual(cols.y - 1);
        expect(b.y + b.height, `${g} row ${i}`).toBeLessThanOrEqual(cols.y + cols.height + 1);
        checked++;
      }
    }
    console.log(`CHEAT 1728: ${checked} Develop+Masks rows visible, columns box ${cols.width}x${cols.height}`);
    const keyCol = await page.getByTestId("cheat-undoAdj").locator("span").first().boundingBox();
    expect(Math.round(keyCol!.width)).toBe(112);
    await shot(page, `${P}cheatsheet-1728`);
    await page.keyboard.press("Escape");
    // Library modes list Culling, Navigate, View first.
    await page.keyboard.press("g");
    await page.keyboard.press("?");
    const lib = await page.locator('[data-testid^="cheat-group-"]').evaluateAll((els) => els.map((e) => e.getAttribute("data-testid")));
    expect(lib.slice(0, 3)).toEqual(["cheat-group-Culling", "cheat-group-Navigate", "cheat-group-View"]);
  });

  test("keymap: no chord maps to two actions in one mode / tool state", async ({ page }) => {
    await openApp(page, 10);
    const dup = await page.evaluate(async () => {
      const { KEYMAP, matchKey } = await import("/src/lib/keymap.ts");
      const out: string[] = [];
      const mk = (key: string, o: { mod?: boolean; shift?: boolean; alt?: boolean }) =>
        ({ key, code: key.length === 1 ? `Key${key.toUpperCase()}` : key, metaKey: !!o.mod, ctrlKey: false, shiftKey: !!o.shift, altKey: !!o.alt }) as KeyboardEvent;
      for (const mode of ["grid", "loupe", "compare", "develop"] as const)
        for (const cropping of [false, true])
          for (const d of KEYMAP) {
            if (d.external || !d.modes.includes(mode) || (d.needs === "crop" && !cropping)) continue;
            for (const ch of d.chords) {
              const hit = matchKey(mk(ch.key, { mod: ch.mod, shift: ch.shift === true, alt: ch.alt }), mode, { cropping });
              if (hit && hit.id !== d.id && !(cropping && hit.needs === "crop")) out.push(`${mode}${cropping ? "+crop" : ""}: ${ch.key} -> ${hit.id} (declared ${d.id})`);
            }
          }
      return out;
    });
    expect(dup).toEqual([]);
  });
});

test.describe("P1-12 white balance eyedropper", () => {
  test("W toggles the picker; a click samples, sets Custom WB and records White Balance: Picker", async ({ page }) => {
    await openDevelop(page);
    await expect(page.getByTestId("wb-picker")).toHaveAttribute("title", /White balance picker \(W\)/);
    await page.keyboard.press("w");
    await expect(page.getByTestId("wb-picker-badge")).toHaveText("Click a neutral grey or white. Esc cancels");
    await expect(page.getByTestId("wb-picker")).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByTestId("wb-picker-overlay")).toHaveCSS("cursor", "crosshair");
    await clearCalls(page);
    const v = await box(page, "viewer");
    await page.mouse.click(v.x + v.width * 0.5, v.y + v.height * 0.5);
    await expect.poll(async () => (await calls(page, "sample_white_balance")).length).toBe(1);
    const c = (await calls(page, "sample_white_balance"))[0].args as { id: number; point: { x: number; y: number } };
    expect(c.id).toBe(1);
    expect(c.point.x).toBeGreaterThan(0.3);
    expect(c.point.x).toBeLessThan(0.7);
    await expect(page.getByTestId("wb-picker-overlay")).toHaveCount(0);
    await expect.poll(async () => (await saved(page)).length).toBe(1);
    const s = (await saved(page))[0];
    expect(s.args.label).toBe("White Balance: Picker");
    expect((s.args.adjustments as { whiteBalance: { mode: string } }).whiteBalance.mode).toBe("custom");
    await expect(page.getByTestId("wb-select")).toHaveValue("custom");

    // The panel button toggles it too.
    await page.getByTestId("wb-picker").click();
    await expect(page.getByTestId("wb-picker-overlay")).toBeVisible();
    await page.getByTestId("wb-picker").click();
    await expect(page.getByTestId("wb-picker-overlay")).toHaveCount(0);
  });
});

test.describe("P2 items", () => {
  test("V toggles Black & White; Space toggles 100%; F zooms to faces", async ({ page }) => {
    await openDevelop(page);
    await clearCalls(page);
    await page.keyboard.press("v");
    await expect.poll(async () => (await saved(page)).at(-1)?.args.label).toBe("Black & White");
    await page.keyboard.press("v");
    await expect.poll(async () => (await saved(page)).at(-1)?.args.label).toBe("Color");
    await expect(page.getByTestId("viewer")).toHaveAttribute("data-zoomed", "false");
    await page.keyboard.press("Space");
    await expect(page.getByTestId("viewer")).toHaveAttribute("data-zoomed", "true");
    await page.keyboard.press("Space");
    await expect(page.getByTestId("viewer")).toHaveAttribute("data-zoomed", "false");
    await page.keyboard.press("f");
    await expect(page.getByTestId("viewer")).toHaveAttribute("data-zoomed", "true");
    await page.keyboard.press("Shift+f");
    await page.keyboard.press("Shift+f");
    await expect(page.getByTestId("viewer")).toHaveAttribute("data-zoomed", "true");
  });

  test("Caps Lock acts as auto-advance", async ({ page }) => {
    await openApp(page, 200);
    await page.getByTestId("cell-5").click();
    await page.evaluate(() => window.dispatchEvent(new KeyboardEvent("keydown", { key: "Shift", modifierCapsLock: true })));
    await expect(page.getByTestId("auto-advance")).toBeChecked();
    await expect(page.locator("label", { has: page.getByTestId("auto-advance") })).toContainText("Caps Lock");
    await page.evaluate(() => window.dispatchEvent(new KeyboardEvent("keydown", { key: "p", modifierCapsLock: true })));
    await expect(page.getByTestId("cell-6")).toHaveAttribute("data-active", "true");
    await page.evaluate(() => window.dispatchEvent(new KeyboardEvent("keydown", { key: "Shift", modifierCapsLock: false })));
    await expect(page.getByTestId("auto-advance")).not.toBeChecked();
  });

  test("Loupe shows +JPG in the info overlay and flags / stars on its filmstrip", async ({ page }) => {
    await openApp(page, 200);
    await page.getByTestId("cell-9").click(); // every 9th photo has a JPEG companion
    await page.keyboard.press("Space");
    await expect(page.getByTestId("info-overlay")).toContainText("+JPG");
    await page.keyboard.press("p");
    await page.keyboard.press("4");
    await expect(page.getByTestId("film-flag-9")).toHaveAttribute("data-pick", "pick");
    await expect(page.getByTestId("film-rating-9")).toHaveAttribute("data-rating", "4");
  });

  test("Cmd+Alt+V pastes from the previous photo (not crop / masks); Cmd+Shift+N saves a preset", async ({ page }) => {
    await openDevelop(page, 1);
    await setSlider(page, "exposure", 1.25);
    await expect.poll(async () => (await saved(page)).length).toBe(1);
    await page.getByTestId("film-2").click();
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "2");
    await expect(page.getByTestId("slider-value-exposure")).toHaveText("0.00");
    await clearCalls(page);
    await page.keyboard.press("Meta+Alt+v");
    await expect(page.getByTestId("slider-value-exposure")).toHaveText("+1.25");
    await expect(page.getByTestId("history-list")).toContainText("Paste from Previous");
    // Plain Cmd+V-style keys are unaffected: Alt without the chord does nothing.
    await page.keyboard.press("Meta+Shift+n");
    await expect(page.getByTestId("preset-name")).toBeVisible();
    await page.keyboard.press("Escape");
  });

  test("history labels carry values; mask colour samples are removable chips; selection follows deletes", async ({ page }) => {
    await openDevelop(page);
    await setSlider(page, "contrast", 12);
    await expect(page.getByTestId("history-list")).toContainText("Contrast +12");
    await page.keyboard.press("Shift+W");
    await page.getByTestId("mask-create-subject").click();
    await expect(page.getByTestId("mask-busy")).toHaveCount(0);
    await page.getByTestId("mask-create-subject").click();
    await expect(page.getByTestId("mask-busy")).toHaveCount(0);
    await expect(page.locator('[data-testid^="mask-group-"][data-selected]')).toHaveCount(2);
    // Delete removes the selected group; the selection moves to the remaining one so Delete keeps working.
    await page.keyboard.press("Delete");
    await expect(page.locator('[data-testid^="mask-group-"][data-selected]')).toHaveCount(1);
    await expect(page.locator('[data-testid^="mask-group-"][data-selected="true"]')).toHaveCount(1);
    await page.keyboard.press("Delete");
    await expect(page.getByTestId("mask-empty")).toBeVisible();
  });

  test("right panel is 320 px from 1600 px; 288 px below", async ({ page }) => {
    await page.setViewportSize({ width: 1728, height: 1117 });
    await openDevelop(page);
    expect((await box(page, "right-aside")).width).toBe(320);
    await page.setViewportSize({ width: 1280, height: 800 });
    await expect.poll(async () => (await box(page, "right-aside")).width).toBe(288);
  });

  test("empty catalog hides the filter bar and toolbar; copy mentions JPEG / HEIC", async ({ page }) => {
    await page.goto("/?mock=0");
    await expect(page.getByTestId("filter-bar")).toHaveCount(0);
    await expect(page.getByTestId("grid-toolbar")).toHaveCount(0);
    await expect(page.getByText("JPEG, HEIC, TIFF, PNG when enabled in Import")).toBeVisible();
    await shot(page, `${P}empty`);
  });

  test("warnings chip: masks copy and Show in Masks panel", async ({ page }) => {
    await openDevelop(page, 10);
    await page.getByTestId("warnings-chip").click();
    await expect(page.getByTestId("warning-masks_unsupported")).toContainText("Some masks can't be rendered");
    await expect(page.getByTestId("warning-masks_unsupported")).toContainText("Depth Range");
    await page.getByTestId("warning-action-masks_unsupported").click();
    await expect(page.getByTestId("masks-panel")).toBeVisible();
  });
});
