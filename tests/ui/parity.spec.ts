import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openApp, shot } from "./helpers";

// Phase 7b Lightroom-parity Develop panels: tone curve, color grading, profile, detail, effects, calibration, crop,
// warnings, copy/paste fields, import options.

async function openDevelop(page: Page, id = 1) {
  await openApp(page, 200);
  await page.getByTestId(`cell-${id}`).click();
  await page.keyboard.press("d");
  await expect(page.getByTestId("develop-view")).toBeVisible();
  await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", String(id));
  await expect(page.getByTestId("view-main")).toBeVisible();
  await expect(page.getByTestId("histogram")).toHaveAttribute("data-empty", "false");
}

async function openSection(page: Page, id: string) {
  if ((await page.getByTestId(`section-${id}`).getAttribute("data-open")) !== "true") await page.getByTestId(`section-toggle-${id}`).click();
  await expect(page.getByTestId(`section-${id}`)).toHaveAttribute("data-open", "true");
  await page.getByTestId(`section-${id}`).scrollIntoViewIfNeeded();
}

async function setSlider(page: Page, id: string, value: number) {
  const s = page.getByTestId(`slider-${id}`);
  await s.fill(String(value));
  await s.evaluate((el) => (el as HTMLElement).blur());
}

type Saved = { label: string; adjustments: any };
const saved = async (page: Page): Promise<Saved[]> => (await calls(page, "save_adjustments")).filter((c) => c.args.label !== undefined).map((c) => c.args as Saved);
const lastSave = async (page: Page) => (await saved(page)).pop()!;

/** Client position of a curve coordinate (0..255) inside the curve editor. */
async function curveXY(page: Page, x: number, y: number) {
  await page.getByTestId("curve-svg").scrollIntoViewIfNeeded();
  const b = (await page.getByTestId("curve-svg").boundingBox())!;
  const view = 255 + 16;
  return { x: b.x + ((x + 8) / view) * b.width, y: b.y + ((255 - y + 8) / view) * b.height };
}
const points = async (page: Page) => JSON.parse((await page.getByTestId("curve-svg").getAttribute("data-points"))!) as [number, number][];

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.clear());
});

test.describe("tone curve", () => {
  test("point curve: click adds, drag moves, double-click and Backspace delete; saves with label", async ({ page }) => {
    await openDevelop(page);
    await openSection(page, "tone-curve");
    await expect(page.getByTestId("curve-svg")).toHaveAttribute("data-points", "[[0,0],[255,255]]");
    await clearCalls(page);

    // Click on empty space adds a point at that position.
    const p1 = await curveXY(page, 128, 160);
    await page.mouse.click(p1.x, p1.y);
    await expect.poll(async () => (await points(page)).length).toBe(3);
    const added = (await points(page))[1];
    expect(Math.abs(added[0] - 128)).toBeLessThanOrEqual(3);
    expect(Math.abs(added[1] - 160)).toBeLessThanOrEqual(3);
    await expect.poll(async () => (await lastSave(page))?.adjustments.toneCurve.point.master.length).toBe(3);
    expect((await lastSave(page)).label).toBe("Tone Curve");
    await shot(page, "6x-parity-01-tone-curve");

    // Drag the point up-right; neighbours keep strictly increasing input.
    const from = await curveXY(page, added[0], added[1]);
    const to = await curveXY(page, 170, 200);
    await page.mouse.move(from.x, from.y);
    await page.mouse.down();
    await page.mouse.move((from.x + to.x) / 2, (from.y + to.y) / 2, { steps: 3 });
    await page.mouse.move(to.x, to.y, { steps: 3 });
    await page.mouse.up();
    const moved = (await points(page))[1];
    expect(Math.abs(moved[0] - 170)).toBeLessThanOrEqual(3);
    expect(Math.abs(moved[1] - 200)).toBeLessThanOrEqual(3);
    await expect
      .poll(async () => (await lastSave(page)).adjustments.toneCurve.point.master[1][1])
      .toBe(moved[1]);
    expect((await lastSave(page)).adjustments.toneCurve.point.master).toEqual(await points(page));

    // Double-click deletes it again.
    const at = await curveXY(page, moved[0], moved[1]);
    await page.mouse.dblclick(at.x, at.y);
    await expect.poll(async () => (await points(page)).length).toBe(2);
    await expect.poll(async () => (await lastSave(page)).adjustments.toneCurve.point.master.length).toBe(2);

    // Backspace deletes the selected point; endpoints are never removed (min 2 points).
    const q = await curveXY(page, 60, 40);
    await page.mouse.click(q.x, q.y);
    await expect.poll(async () => (await points(page)).length).toBe(3);
    await page.keyboard.press("Backspace");
    await expect.poll(async () => (await points(page)).length).toBe(2);
    await page.getByTestId("curve-svg").focus();
    await page.mouse.dblclick((await curveXY(page, 0, 0)).x, (await curveXY(page, 0, 0)).y);
    expect((await points(page)).length).toBe(2);
  });

  test("channels are independent and the point count is capped", async ({ page }) => {
    await openDevelop(page);
    await openSection(page, "tone-curve");
    await page.getByTestId("curve-ch-red").click();
    const p = await curveXY(page, 100, 120);
    await page.mouse.click(p.x, p.y);
    await expect.poll(async () => (await points(page)).length).toBe(3);
    await expect.poll(async () => (await lastSave(page))?.adjustments.toneCurve.point.red.length).toBe(3);
    expect((await lastSave(page)).adjustments.toneCurve.point.master.length).toBe(2);
    await page.getByTestId("curve-ch-master").click();
    expect((await points(page)).length).toBe(2);

    // Fill to the maximum (32) with distinct inputs; extra clicks add nothing.
    for (let i = 1; i <= 30; i++) {
      const c = await curveXY(page, i * 8, i * 8);
      await page.mouse.click(c.x, c.y);
    }
    await expect.poll(async () => (await points(page)).length).toBe(32);
    const over = await curveXY(page, 4, 200);
    await page.mouse.click(over.x, over.y);
    await expect.poll(async () => (await points(page)).length).toBe(32);
    await page.getByTestId("curve-reset").click();
    await expect.poll(async () => (await points(page)).length).toBe(2);
  });

  test("parametric sliders and region split handles", async ({ page }) => {
    await openDevelop(page);
    await openSection(page, "tone-curve");
    await clearCalls(page);
    await setSlider(page, "curve-shadows", 30);
    await expect.poll(async () => (await lastSave(page))?.adjustments.toneCurve.parametric.shadows).toBe(30);
    expect((await lastSave(page)).label).toBe("Tone Curve: Shadows");
    await setSlider(page, "curve-highlights", -45);
    await expect.poll(async () => (await lastSave(page)).adjustments.toneCurve.parametric.highlights).toBe(-45);

    // Split handle: keyboard nudges, drag moves, order is preserved.
    const h0 = page.getByTestId("curve-split-0");
    await h0.focus();
    await page.keyboard.press("ArrowRight");
    await expect(h0).toHaveAttribute("aria-valuenow", "26");
    await expect.poll(async () => (await lastSave(page)).adjustments.toneCurve.parametric.shadowSplit).toBe(26);
    const h1 = page.getByTestId("curve-split-1");
    const box = (await h1.boundingBox())!;
    const track = (await page.getByTestId("curve-splits").boundingBox())!;
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(track.x + track.width * 0.9, box.y + box.height / 2, { steps: 4 });
    await page.mouse.up();
    // Clamped just below the highlight split (75).
    await expect.poll(async () => (await lastSave(page)).adjustments.toneCurve.parametric.midtoneSplit).toBe(74);

    // Double-click resets a slider to its default.
    await page.getByTestId("slider-label-curve-shadows").dblclick();
    await expect.poll(async () => (await lastSave(page)).adjustments.toneCurve.parametric.shadows).toBe(0);
  });
});

test.describe("color grading", () => {
  test("dragging a wheel sets hue and saturation; Shift is fine control; double-click resets", async ({ page }) => {
    await openDevelop(page);
    await openSection(page, "color-grading");
    await clearCalls(page);
    const wheel = page.getByTestId("wheel-shadows");
    const b = (await wheel.boundingBox())!;
    const cx = b.x + b.width / 2;
    const cy = b.y + b.height / 2;
    // Right of centre at half radius: hue 90 (clockwise from top), saturation ~50.
    const r = (b.width / 2 - 6) * 0.5;
    await page.mouse.move(cx, cy);
    await page.mouse.down();
    await page.mouse.move(cx + r / 2, cy, { steps: 2 });
    await page.mouse.move(cx + r, cy, { steps: 2 });
    await page.mouse.up();
    await expect.poll(async () => Number(await wheel.getAttribute("data-hue"))).toBeGreaterThan(80);
    const hue = Number(await wheel.getAttribute("data-hue"));
    const sat = Number(await wheel.getAttribute("data-sat"));
    expect(hue).toBeLessThan(100);
    expect(sat).toBeGreaterThan(40);
    expect(sat).toBeLessThan(60);
    await expect.poll(async () => (await lastSave(page))?.adjustments.colorGrading.shadows.hue).toBe(hue);
    expect((await lastSave(page)).adjustments.colorGrading.shadows.saturation).toBe(sat);
    expect((await lastSave(page)).label).toBe("Color Grading: Shadows");
    await expect(page.getByTestId("slider-value-grading-hue")).toHaveText(String(hue));
    await shot(page, "6x-parity-02-color-grading");

    // Shift moves relative to the puck at 20% speed.
    await page.keyboard.down("Shift");
    await page.mouse.move(cx + r, cy);
    await page.mouse.down();
    await page.mouse.move(cx + r, cy + 50, { steps: 5 });
    await page.mouse.up();
    await page.keyboard.up("Shift");
    const hue2 = Number(await wheel.getAttribute("data-hue"));
    expect(hue2).toBeGreaterThan(hue); // moved clockwise, but only slightly
    expect(hue2 - hue).toBeLessThan(30);

    // Zone tabs, luminance, blending and balance.
    await page.getByTestId("grading-zone-highlights").click();
    await setSlider(page, "grading-lum", 20);
    await expect.poll(async () => (await lastSave(page)).adjustments.colorGrading.highlights.luminance).toBe(20);
    await setSlider(page, "grading-blending", 70);
    await setSlider(page, "grading-balance", -30);
    await expect.poll(async () => (await lastSave(page)).adjustments.colorGrading.balance).toBe(-30);
    expect((await lastSave(page)).adjustments.colorGrading.blending).toBe(70);

    await page.getByTestId("grading-zone-shadows").click();
    await wheel.dblclick();
    await expect.poll(async () => (await lastSave(page)).adjustments.colorGrading.shadows.saturation).toBe(0);
    expect((await lastSave(page)).adjustments.colorGrading.shadows.hue).toBe(0);
  });
});

test.describe("profile browser", () => {
  test("selecting a camera profile or a look edits profile.cameraProfile / profile.look; amount slider", async ({ page }) => {
    await openDevelop(page);
    await expect(page.getByTestId("profile-current")).toHaveText("Adobe Color");
    await page.getByTestId("profile-browse").click();
    await expect(page.getByTestId("profile-browser")).toBeVisible();
    await clearCalls(page);

    await page.getByTestId("profile-item-Camera Standard").click();
    await expect.poll(async () => (await lastSave(page))?.adjustments.profile.cameraProfile).toBe("Camera Standard");
    expect((await lastSave(page)).adjustments.profile.look).toBeNull();
    await page.getByTestId("profile-browser-close").click(); // Esc / Close returns to the sections
    await expect(page.getByTestId("profile-current")).toHaveText("Camera Standard");
    await page.getByTestId("profile-browse").click();

    await page.getByTestId("look-item-AAAA0000000000000000000000000001").click();
    await expect.poll(async () => (await lastSave(page)).adjustments.profile.look?.name).toBe("Vintage 01");
    expect((await lastSave(page)).adjustments.profile.look).toEqual({ name: "Vintage 01", uuid: "AAAA0000000000000000000000000001", amount: 1 });
    expect((await lastSave(page)).adjustments.profile.cameraProfile).toBe("Camera Standard");
    expect((await lastSave(page)).label).toBe("Profile: Vintage 01");

    await setSlider(page, "look-amount", 60);
    await expect.poll(async () => (await lastSave(page)).adjustments.profile.look.amount).toBeCloseTo(0.6, 2);

    // A look that is not installed cannot be picked.
    await expect(page.getByTestId("look-item-BBBB0000000000000000000000000001")).toBeDisabled();
    // A look that carries its own base profile switches it.
    await page.getByTestId("look-item-0CFE8F8AB5F63B2A73CE0B0077D20817").click();
    await expect.poll(async () => (await lastSave(page)).adjustments.profile.cameraProfile).toBe("Adobe Standard");
    await expect(page.getByTestId("slider-look-amount")).toHaveCount(0);
    await shot(page, "6x-parity-03-profile");
  });
});

test.describe("detail, effects, calibration, black and white", () => {
  test("sliders write their groups; double-click resets to the format default", async ({ page }) => {
    await openDevelop(page);
    await clearCalls(page);
    await openSection(page, "detail");
    // RAW default sharpening amount is 40.
    await expect(page.getByTestId("slider-value-sharp-amount")).toHaveText("40");
    await setSlider(page, "sharp-amount", 90);
    await setSlider(page, "sharp-radius", 1.6);
    await setSlider(page, "sharp-masking", 35);
    await setSlider(page, "nr-lum", 30);
    await setSlider(page, "nr-color-smooth", 70);
    await expect.poll(async () => (await lastSave(page))?.adjustments.detail.noiseReduction.colorSmoothness).toBe(70);
    let d = (await lastSave(page)).adjustments.detail;
    expect(d.sharpening).toMatchObject({ amount: 90, radius: 1.6, masking: 35 });
    expect(d.noiseReduction.luminance).toBe(30);
    await page.getByTestId("slider-label-sharp-amount").dblclick();
    await expect.poll(async () => (await lastSave(page)).adjustments.detail.sharpening.amount).toBe(40);
    await shot(page, "6x-parity-04-detail");

    await openSection(page, "effects");
    await setSlider(page, "vig-amount", -40);
    await setSlider(page, "grain-amount", 25);
    await page.getByTestId("vig-style").selectOption("color_priority");
    await expect.poll(async () => (await lastSave(page)).adjustments.effects.vignette.style).toBe("color_priority");
    const e = (await lastSave(page)).adjustments.effects;
    expect(e.vignette.amount).toBe(-40);
    expect(e.grain.amount).toBe(25);
    await page.getByTestId("vig-style").selectOption("paint_overlay");
    await expect(page.getByTestId("slider-vig-highlights")).toBeDisabled();

    await openSection(page, "calibration");
    await setSlider(page, "calib-shadow-tint", 12);
    await setSlider(page, "calib-red-hue", -20);
    await setSlider(page, "calib-blue-sat", 15);
    await expect.poll(async () => (await lastSave(page)).adjustments.calibration.blue.saturation).toBe(15);
    const c = (await lastSave(page)).adjustments.calibration;
    expect(c.shadowTint).toBe(12);
    expect(c.red.hue).toBe(-20);

    // Section reset returns the whole group to defaults.
    await page.getByTestId("reset-effects").click();
    await expect.poll(async () => (await lastSave(page)).adjustments.effects.vignette.amount).toBe(0);
    expect((await lastSave(page)).adjustments.effects.grain.amount).toBe(0);
    d = (await lastSave(page)).adjustments.detail;
    expect(d.sharpening.masking).toBe(35); // Detail was not reset

    // Black & White: toggle shows the gray mixer instead of the colour mixer.
    await openSection(page, "hsl");
    await expect(page.getByTestId("hsl-tabs")).toBeVisible();
    await page.getByTestId("bw-on").click();
    await expect.poll(async () => (await lastSave(page)).adjustments.blackAndWhite.enabled).toBe(true);
    await expect(page.getByTestId("hsl-tabs")).toHaveCount(0);
    await setSlider(page, "bw-red", 40);
    await expect.poll(async () => (await lastSave(page)).adjustments.blackAndWhite.mixer.red).toBe(40);
    await expect(page.getByTestId("section-toggle-hsl")).toContainText("B&W");
    await shot(page, "6x-parity-05-bw");
  });

  test("section collapse state is remembered; Alt-click solos a section", async ({ page }) => {
    await openDevelop(page);
    await openSection(page, "tone-curve");
    await page.getByTestId("section-toggle-basic").click();
    await expect(page.getByTestId("section-basic")).toHaveAttribute("data-open", "false");
    await expect(page.getByTestId("slider-exposure")).toHaveCount(0);
    // Remembered across a reload of the page (localStorage).
    const stored = await page.evaluate(() => localStorage.getItem("sieve.develop.sections.v2"));
    expect(JSON.parse(stored!)).toMatchObject({ basic: false, "tone-curve": true });
    await page.getByTestId("section-toggle-detail").click({ modifiers: ["Alt"] });
    await expect(page.getByTestId("section-detail")).toHaveAttribute("data-open", "true");
    for (const id of ["basic", "tone-curve", "hsl", "color-grading", "effects", "calibration"]) await expect(page.getByTestId(`section-${id}`)).toHaveAttribute("data-open", "false");
  });
});

test.describe("crop tool", () => {
  test("R opens the tool, dragging a handle then Enter writes the crop; Esc cancels without saving", async ({ page }) => {
    await openDevelop(page);
    await clearCalls(page);
    await page.keyboard.press("r");
    await expect(page.getByTestId("crop-overlay")).toBeVisible();
    await expect(page.getByTestId("crop-panel")).toHaveAttribute("data-active", "true");
    await expect(page.getByTestId("crop-aspect")).toHaveValue("original"); // UX2 P1-3: starts locked to Original
    await page.keyboard.press("a"); // A unlocks (Free) for this free-form drag
    await expect(page.getByTestId("crop-aspect")).toHaveValue("free");
    // The tool asks for the uncropped frame.
    await expect.poll(async () => (await calls(page, "render_preview")).some((c) => c.args.adjustments.crop.enabled === false)).toBe(true);

    const se = page.getByTestId("crop-handle-se");
    const b = (await se.boundingBox())!;
    const frame = (await page.getByTestId("crop-frame").boundingBox())!;
    await page.mouse.move(b.x + b.width / 2, b.y + b.height / 2);
    await page.mouse.down();
    await page.mouse.move(frame.x + frame.width * 0.9, frame.y + frame.height * 0.8, { steps: 4 });
    await page.mouse.up();
    const rect = JSON.parse((await page.getByTestId("crop-rect").getAttribute("data-rect"))!);
    expect(rect.r).toBeGreaterThan(0.85);
    expect(rect.r).toBeLessThan(0.95);
    expect(rect.b).toBeGreaterThan(0.75);
    expect(rect.b).toBeLessThan(0.85);
    await setCropAngle(page, 3.5);
    await shot(page, "6x-parity-06-crop-tool");

    expect((await saved(page)).length).toBe(0); // nothing is written until Enter
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("crop-overlay")).toHaveCount(0);
    await expect.poll(async () => (await saved(page)).length).toBe(1);
    const s = await lastSave(page);
    expect(s.label).toBe("Crop");
    // Straightened: the stored corners are those of the rotated frame in the source (see polish.spec for the round trip).
    expect(s.adjustments.crop).toMatchObject({ enabled: true, angle: 3.5 });
    expect(s.adjustments.crop.right).toBeGreaterThan(s.adjustments.crop.left);
    expect(s.adjustments.crop.bottom).toBeGreaterThan(s.adjustments.crop.top);
    expect(s.adjustments.crop.right).toBeLessThanOrEqual(1);
    // The render of the cropped frame follows (mock: aspect changes).
    await expect.poll(async () => (await calls(page, "render_preview")).filter((c) => c.args.options.slot === "main").pop()!.args.adjustments.crop.enabled).toBe(true);
    await expect(page.getByTestId("tool-crop-dot")).toBeVisible(); // the strip marks a cropped photo

    // Esc cancels: the first Esc only leaves the tool (Develop stays open), nothing is saved.
    await clearCalls(page);
    await page.keyboard.press("r");
    await expect(page.getByTestId("crop-overlay")).toBeVisible();
    const nw = (await page.getByTestId("crop-handle-nw").boundingBox())!;
    await page.mouse.move(nw.x + nw.width / 2, nw.y + nw.height / 2);
    await page.mouse.down();
    await page.mouse.move(nw.x + 80, nw.y + 60, { steps: 3 });
    await page.mouse.up();
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("crop-overlay")).toHaveCount(0);
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await page.waitForTimeout(200);
    expect((await saved(page)).length).toBe(0);
    // UX2 P0-1: a second Esc does nothing in Develop (G goes back to the Grid).
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await page.keyboard.press("g");
    await expect(page.getByTestId("develop-view")).toHaveCount(0);
  });

  test("aspect presets lock the ratio; Remove crop restores the full frame", async ({ page }) => {
    await openDevelop(page);
    await page.keyboard.press("r");
    await expect(page.getByTestId("crop-overlay")).toBeVisible();
    await page.getByTestId("crop-aspect").selectOption("1:1");
    const frame = (await page.getByTestId("crop-frame").boundingBox())!;
    const rect = JSON.parse((await page.getByTestId("crop-rect").getAttribute("data-rect"))!);
    expect(((rect.r - rect.l) * frame.width) / ((rect.b - rect.t) * frame.height)).toBeCloseTo(1, 1);
    // Dragging a corner keeps the ratio.
    const ne = (await page.getByTestId("crop-handle-ne").boundingBox())!;
    await page.mouse.move(ne.x + ne.width / 2, ne.y + ne.height / 2);
    await page.mouse.down();
    await page.mouse.move(ne.x - 40, ne.y + 60, { steps: 4 });
    await page.mouse.up();
    const r2 = JSON.parse((await page.getByTestId("crop-rect").getAttribute("data-rect"))!);
    expect(((r2.r - r2.l) * frame.width) / ((r2.b - r2.t) * frame.height)).toBeCloseTo(1, 1);
    await clearCalls(page);
    await page.getByTestId("crop-done").click();
    await expect.poll(async () => (await saved(page)).length).toBe(1);
    expect((await lastSave(page)).adjustments.crop.enabled).toBe(true);

    // Remove the crop: open the tool, Reset to the full frame, Done.
    await page.getByTestId("tool-crop").click();
    await expect(page.getByTestId("crop-panel")).toHaveAttribute("data-active", "true");
    await page.getByTestId("crop-reset").click();
    await page.getByTestId("crop-done").click();
    await expect.poll(async () => (await lastSave(page)).adjustments.crop.enabled).toBe(false);
    await expect(page.getByTestId("tool-crop-dot")).toHaveCount(0);

    // A full-frame commit with no angle stores no crop at all.
    await clearCalls(page);
    await page.keyboard.press("r");
    await page.keyboard.press("Enter");
    await page.waitForTimeout(200);
    expect((await saved(page)).length).toBe(0);
  });
});

async function setCropAngle(page: Page, v: number) {
  await page.getByTestId("slider-crop-angle").fill(String(v));
}

test.describe("warnings", () => {
  test("frames with masks show a chip with a details popover; clean frames show none", async ({ page }) => {
    await openDevelop(page, 1);
    await expect(page.getByTestId("warnings-chip")).toHaveCount(0);
    await page.getByTestId("film-10").click();
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "10");
    const chip = page.getByTestId("warnings-chip");
    await expect(chip).toBeVisible();
    await expect(chip).toContainText("Some masks can't be rendered");
    await chip.click();
    await expect(page.getByTestId("warning-masks_unsupported")).toContainText("mask types");
    await expect(page.getByTestId("warning-masks_unsupported")).toContainText("2 mask groups");
    await shot(page, "6x-parity-07-warnings");
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("warnings-popover")).toHaveCount(0);
    await expect(page.getByTestId("develop-view")).toBeVisible();
  });
});

test.describe("copy / paste and import", () => {
  test("copy dialog offers the new groups (crop unticked by default is not required); paste carries them", async ({ page }) => {
    await openDevelop(page);
    await openSection(page, "detail");
    await setSlider(page, "sharp-amount", 77);
    await openSection(page, "tone-curve");
    await page.getByTestId("curve-ch-master").click();
    const p = await curveXY(page, 128, 170);
    await page.mouse.click(p.x, p.y);
    await expect.poll(async () => (await lastSave(page))?.adjustments.toneCurve.point.master.length).toBe(3);
    await page.getByTestId("section-toggle-tone-curve").focus();
    await page.keyboard.press("Meta+Shift+c");
    await expect(page.getByTestId("fields-dialog")).toBeVisible();
    for (const f of ["tone_curve", "color_grading", "calibration", "sharpening", "noise_reduction", "vignette", "grain", "black_and_white", "crop", "profile"]) {
      await expect(page.getByTestId(`field-${f}`), f).toBeVisible();
    }
    await page.getByTestId("fields-none").click();
    await page.getByTestId("field-tone_curve").check();
    await page.getByTestId("field-sharpening").check();
    await shot(page, "6x-parity-08-copy-fields");
    await page.getByTestId("fields-confirm").click();

    await page.getByTestId("film-2").click();
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "2");
    await clearCalls(page);
    await page.keyboard.press("Meta+Shift+v");
    await expect.poll(async () => (await calls(page, "paste_settings")).length).toBe(1);
    const [paste] = await calls(page, "paste_settings");
    expect(paste.args.fields).toEqual(["tone_curve", "sharpening"]);
    expect(paste.args.adjustments.toneCurve.point.master.length).toBe(3);
    expect(paste.args.adjustments.detail.sharpening.amount).toBe(77);
    await expect(page.getByTestId("slider-value-sharp-amount")).toHaveText("77");
    await expect(page.getByTestId("curve-svg")).toHaveAttribute("data-points", /^\[\[0,0\],\[\d+,\d+\],\[255,255\]\]$/);
  });

  test("Import options popover passes includeNonRaw / pairJpegWithRaw to importFolder and is remembered", async ({ page }) => {
    await openApp(page, 200);
    await page.getByTestId("import-options").click();
    await expect(page.getByTestId("import-options-popover")).toBeVisible();
    await expect(page.getByTestId("import-include-nonraw")).not.toBeChecked();
    await expect(page.getByTestId("import-pair-jpeg")).toBeDisabled();
    await page.getByTestId("import-include-nonraw").check();
    await page.getByTestId("import-pair-jpeg").uncheck();
    await shot(page, "6x-parity-09-import-options");
    await clearCalls(page);
    await page.getByTestId("import-choose").click();
    await expect.poll(async () => (await calls(page, "import_folder")).length).toBe(1);
    expect((await calls(page, "import_folder"))[0].args.options).toEqual({ recursive: true, includeNonRaw: true, pairJpegWithRaw: false });

    // Plain Import reuses the remembered choice.
    await clearCalls(page);
    await page.getByTestId("import-button").click();
    await expect.poll(async () => (await calls(page, "import_folder")).length).toBe(1);
    expect((await calls(page, "import_folder"))[0].args.options.includeNonRaw).toBe(true);
  });

  test("companion JPEG shows +JPG on the cell", async ({ page }) => {
    await openApp(page, 200);
    await expect(page.getByTestId("companion-9")).toHaveText("+JPG");
    await expect(page.getByTestId("companion-8")).toHaveCount(0);
  });
});

test.describe("keymap", () => {
  test("R and Enter are in the cheat sheet", async ({ page }) => {
    await openDevelop(page);
    await page.keyboard.press("?");
    await expect(page.getByRole("dialog")).toContainText("Crop tool");
    await expect(page.getByRole("dialog")).toContainText("Apply crop");
  });
});
