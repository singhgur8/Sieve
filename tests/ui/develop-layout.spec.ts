// Phase 8b "Lightroom-style Develop layout" (docs/ux-spec-8b.md section 5): panel order, single-line sliders, Copy Settings dialog,
// Copy / Paste / Previous / Sync / Reset bars and their shortcuts, viewer toolbar, filmstrip header.
import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openApp, openSection, shot } from "./helpers";

async function openDevelop(page: Page, id = 1) {
  await openApp(page, 200);
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

const y = async (page: Page, testid: string) => (await page.getByTestId(testid).boundingBox())!.y;
const saved = async (page: Page) => (await calls(page, "save_adjustments")).filter((c) => c.args.label !== undefined);

/** Every testid is below the previous one (top to bottom). */
async function expectOrder(page: Page, ids: string[]) {
  let prev = -1;
  for (const id of ids) {
    const v = await y(page, id);
    expect(v, `${id} must be below the previous entry`).toBeGreaterThan(prev);
    prev = v;
  }
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    if (!sessionStorage.getItem("seeded")) {
      localStorage.clear();
      sessionStorage.setItem("seeded", "1");
    }
  });
});

test.describe("panel layout", () => {
  test("right panel order: histogram, tool strip, Basic rows, then the closed sections; tabs are gone", async ({ page }) => {
    await openDevelop(page);
    await expect(page.getByTestId("panel-tab-adjust")).toHaveCount(0);
    await expect(page.getByTestId("panel-tab-masks")).toHaveCount(0);
    await expectOrder(page, ["histogram", "histogram-info", "tool-strip", "section-basic"]);
    await expectOrder(page, ["tool-crop", "section-basic"]);
    // Basic: Treatment, Profile, WB, Temp, Tint, Tone (Auto), tone sliders, Presence sliders.
    await expectOrder(page, ["bw-mode", "profile-panel", "wb-mode", "slider-row-temp", "slider-row-tint", "auto-tone", "slider-row-exposure", "slider-row-contrast", "slider-row-highlights", "slider-row-shadows", "slider-row-whites", "slider-row-blacks", "slider-row-texture", "slider-row-clarity", "slider-row-dehaze", "slider-row-vibrance", "slider-row-saturation"]);
    await expect(page.getByTestId("histogram-info")).toHaveText("ISO 100 · 85 mm · f/2.8 · 1/60s");
    // The closed sections follow in Lightroom order and start closed; Basic starts open.
    const ids = ["basic", "tone-curve", "hsl", "color-grading", "detail", "effects", "calibration"];
    await expectOrder(page, ids.map((i) => `section-${i}`));
    await expect(page.getByTestId("section-basic")).toHaveAttribute("data-open", "true");
    for (const i of ids.slice(1)) await expect(page.getByTestId(`section-${i}`)).toHaveAttribute("data-open", "false");
    // Removed sections.
    for (const i of ["profile", "presence", "lut", "crop"]) await expect(page.getByTestId(`section-${i}`)).toHaveCount(0);
    await expect(page.getByTestId("hsl-tabs")).toHaveCount(0);
    await openSection(page, "hsl");
    await expect(page.getByTestId("hsl-tab-hue")).toHaveText("Hue");
    await expect(page.getByTestId("hsl-tab-saturation")).toHaveText("Saturation");
    await expect(page.getByTestId("hsl-tab-luminance")).toHaveText("Luminance");
    // Bottom bar: Previous (disabled, nothing edited before) and Reset.
    await expect(page.getByTestId("previous-settings")).toBeDisabled();
    await expect(page.getByTestId("reset-all")).toHaveText("Reset");
    await expect(page.getByTestId("sync-settings")).toHaveCount(0);
  });

  test("tool strip: Crop opens the drawer under it, Masking replaces the section list", async ({ page }) => {
    await openDevelop(page);
    await page.getByTestId("tool-crop").click();
    await expect(page.getByTestId("crop-overlay")).toBeVisible();
    await expect(page.getByTestId("crop-panel")).toHaveAttribute("data-active", "true");
    await expectOrder(page, ["tool-strip", "crop-panel", "section-basic"]); // the sections stay below the drawer
    await expect(page.getByTestId("tool-crop")).toHaveAttribute("aria-pressed", "true");
    await page.getByTestId("tool-crop").click(); // again: apply (nothing changed, no history entry)
    await expect(page.getByTestId("crop-overlay")).toHaveCount(0);
    await expect(page.getByTestId("crop-panel")).toHaveCount(0);

    await page.getByTestId("tool-masking").click();
    await expect(page.getByTestId("masks-panel")).toBeVisible();
    await expect(page.getByTestId("section-basic")).toHaveCount(0);
    await page.keyboard.press("Shift+W"); // the shortcut closes it again
    await expect(page.getByTestId("masks-panel")).toHaveCount(0);
    await expect(page.getByTestId("section-basic")).toBeVisible();
  });

  test("left panel: Navigator, Presets, Snapshots, History with the Copy… / Paste bar", async ({ page }) => {
    await openDevelop(page);
    await expectOrder(page, ["section-navigator", "section-presets", "section-snapshots", "section-history", "left-bar"]);
    await expect(page.getByTestId("section-snapshots")).toHaveAttribute("data-open", "false");
    await expect(page.getByTestId("copy-settings")).toHaveText("Copy…");
    await expect(page.getByTestId("paste-settings")).toBeDisabled();
    // Snapshots need backend support: the + is disabled with a reason.
    await expect(page.getByTestId("snapshot-add")).toBeDisabled();
    // Navigator: FIT / 100% follow the zoom and the region rectangle shows at 100%.
    await expect(page.getByTestId("nav-fit")).toHaveAttribute("aria-pressed", "true");
    await page.getByTestId("nav-100").click();
    await expect(page.getByTestId("viewer")).toHaveAttribute("data-zoomed", "true");
    await expect(page.getByTestId("navigator-region")).toBeVisible();
    await expect(page.getByTestId("zoom-toggle")).toHaveAttribute("data-zoom", "100");
    await page.getByTestId("zoom-fit").click();
    await expect(page.getByTestId("navigator-region")).toHaveCount(0);
    // The preset + menu: Create Preset… works, Import is disabled until the v14 library is wired.
    await page.getByTestId("preset-add").click();
    await expect(page.getByTestId("preset-import")).toBeDisabled();
    await page.getByTestId("preset-save").click();
    await expect(page.getByTestId("fields-dialog")).toBeVisible();
    await expect(page.getByRole("dialog")).toContainText("New Develop Preset");
    await page.keyboard.press("Escape");
  });

  test("sliders are single 24 px rows: label, track, value", async ({ page }) => {
    await openDevelop(page);
    const row = (await page.getByTestId("slider-row-exposure").boundingBox())!;
    expect(row.height).toBe(24);
    const label = (await page.getByTestId("slider-label-exposure").boundingBox())!;
    const track = (await page.getByTestId("slider-exposure").boundingBox())!;
    const value = (await page.getByTestId("slider-value-exposure").boundingBox())!;
    expect(label.x + label.width).toBeLessThanOrEqual(track.x + 1);
    expect(track.x + track.width).toBeLessThanOrEqual(value.x + 1);
    expect(Math.abs(label.y + label.height / 2 - (track.y + track.height / 2))).toBeLessThan(3);
    // Changed values render the label brighter.
    await expect(page.getByTestId("slider-label-exposure")).toHaveClass(/text-neutral-300/);
    await setSlider(page, "exposure", 1);
    await expect(page.getByTestId("slider-label-exposure")).toHaveClass(/text-neutral-100/);
    // Basic stays compact: Exposure..Saturation rows fit in far less than the old two-row layout.
    const basic = (await page.getByTestId("section-basic").boundingBox())!;
    expect(basic.height).toBeLessThan(560);
  });

  test("viewer toolbar sits under the photo with clickable flags and stars; the readout moved to data-*", async ({ page }) => {
    await openDevelop(page);
    await expect(page.getByTestId("develop-back")).toHaveCount(0);
    await expect(page.getByTestId("render-ms")).toHaveCount(0);
    const tb = (await page.getByTestId("viewer-toolbar").boundingBox())!;
    const viewer = (await page.getByTestId("viewer").boundingBox())!;
    expect(tb.y).toBeGreaterThanOrEqual(viewer.y + viewer.height - 1);
    await expect(page.getByTestId("viewer-toolbar")).toHaveAttribute("data-render-ms", /^\d+$/);
    await expect(page.getByTestId("develop-filename")).toHaveText("DSC00001.ARW");
    await clearCalls(page);
    // Photo 1 starts picked: clicking the flag clears it, clicking again picks it.
    await expect(page.getByTestId("develop-flags")).toHaveAttribute("data-pick", "pick");
    await page.getByTestId("develop-pick").click();
    await expect.poll(async () => (await calls(page, "set_pick")).length).toBe(1);
    expect((await calls(page, "set_pick"))[0].args).toMatchObject({ ids: [1], pick: "unflagged" });
    await page.getByTestId("develop-pick").click();
    await expect(page.getByTestId("develop-flags")).toHaveAttribute("data-pick", "pick");
    await page.getByTestId("develop-reject").click();
    await expect(page.getByTestId("develop-flags")).toHaveAttribute("data-pick", "reject");
    await page.getByTestId("develop-stars").locator("button, svg").nth(2).click();
    await expect.poll(async () => (await calls(page, "set_rating")).length).toBe(1);
    await expect(page.getByTestId("filmstrip-header")).toContainText("1 of 200");
    await page.keyboard.press("Shift+Tab");
    await expect(page.getByTestId("viewer-toolbar")).toHaveCount(0);
  });
});

test.describe("Copy Settings dialog", () => {
  test("every group is a checkbox; parents are tri-state; Check All / None / Modified", async ({ page }) => {
    await openDevelop(page);
    await setSlider(page, "exposure", 1);
    await page.getByTestId("copy-settings").click();
    const dlg = page.getByTestId("fields-dialog");
    await expect(dlg).toBeVisible();
    await expect(page.getByRole("dialog")).toContainText("Copy Settings");
    const box = (await page.getByRole("dialog").boundingBox())!;
    expect(box.width).toBe(680);
    // The primary button has the focus: Enter copies straight away.
    await expect(page.getByTestId("fields-confirm")).toBeFocused();
    // Default: everything but Masking (no masks here) and Crop.
    await expect(page.getByTestId("field-masks")).toBeDisabled();
    await expect(page.getByTestId("field-crop")).not.toBeChecked();
    await expect(page.getByTestId("field-white_balance")).toBeChecked();
    await expect(page.getByTestId("field-group-basic_tone")).toHaveAttribute("data-state", "checked");
    await shot(page, "develop-layout-copy-dialog");

    await page.getByTestId("fields-none").click();
    await expect(page.getByTestId("field-group-basic_tone")).toHaveAttribute("data-state", "unchecked");
    await page.getByTestId("field-exposure").check();
    await expect(page.getByTestId("field-group-basic_tone")).toHaveAttribute("data-state", "mixed");
    expect(await page.getByTestId("field-group-basic_tone").evaluate((el) => (el as HTMLInputElement).indeterminate)).toBe(true);
    // Clicking an indeterminate parent checks every child.
    await page.getByTestId("field-group-basic_tone").click();
    for (const f of ["exposure", "contrast", "highlights", "shadows", "whites", "blacks"]) await expect(page.getByTestId(`field-${f}`)).toBeChecked();
    await page.getByTestId("field-group-basic_tone").click(); // checked parent: clears the children
    await expect(page.getByTestId("field-exposure")).not.toBeChecked();
    // Other parents exist.
    for (const g of ["color", "treatment", "effects"]) await expect(page.getByTestId(`field-group-${g}`)).toBeVisible();

    await page.getByTestId("fields-modified").click(); // only the settings that differ from the photo's defaults
    await expect(page.getByTestId("field-exposure")).toBeChecked();
    await expect(page.getByTestId("field-contrast")).not.toBeChecked();
    await expect(page.getByTestId("field-white_balance")).not.toBeChecked();
    await page.getByTestId("fields-all").click();
    await expect(page.getByTestId("field-crop")).toBeChecked();
    await expect(page.getByTestId("field-group-color")).toHaveAttribute("data-state", "checked");
    await page.getByTestId("fields-cancel").click();
    await expect(dlg).toHaveCount(0);
  });

  test("remembers the last choice; Enter copies; Alt-click Copy… skips the dialog", async ({ page }) => {
    await openDevelop(page);
    await setSlider(page, "exposure", 1.5);
    await page.getByTestId("copy-settings").click();
    await page.getByTestId("fields-none").click();
    await page.getByTestId("field-exposure").check();
    await page.getByTestId("field-tone_curve").check();
    await page.getByTestId("fields-confirm").click();
    await expect(page.getByTestId("notice").last()).toContainText("Copied 2 settings from DSC00001");
    expect(JSON.parse((await page.evaluate(() => localStorage.getItem("sieve.copyFields.v1")))!)).toEqual(["exposure", "tone_curve"]);

    // Reopen: same boxes.
    await page.getByTestId("copy-settings").click();
    await expect(page.getByTestId("field-exposure")).toBeChecked();
    await expect(page.getByTestId("field-tone_curve")).toBeChecked();
    await expect(page.getByTestId("field-contrast")).not.toBeChecked();
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("fields-dialog")).toHaveCount(0);

    // Cmd+Shift+C then Enter: two key presses.
    await page.keyboard.press("Meta+Shift+c");
    await expect(page.getByTestId("fields-dialog")).toBeVisible();
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("fields-dialog")).toHaveCount(0);
    await expect(page.getByTestId("notice").last()).toContainText("Copied 2 settings");

    // Alt-click Copy…: no dialog, same fields.
    await page.getByTestId("copy-settings").click({ modifiers: ["Alt"] });
    await expect(page.getByTestId("fields-dialog")).toHaveCount(0);
    await expect(page.getByTestId("notice").last()).toContainText("Copied 2 settings from DSC00001");
  });

  test("the preset variant asks for a name and starts from the modified settings", async ({ page }) => {
    await openDevelop(page);
    await setSlider(page, "contrast", 20);
    await page.keyboard.press("Meta+Shift+n");
    await expect(page.getByRole("dialog")).toContainText("New Develop Preset");
    await expect(page.getByTestId("preset-name")).toBeFocused();
    await expect(page.getByTestId("field-contrast")).toBeChecked();
    await expect(page.getByTestId("field-exposure")).not.toBeChecked();
    await expect(page.getByTestId("fields-confirm")).toBeDisabled(); // needs a name
    await page.getByTestId("preset-name").fill("Punchy");
    await page.getByTestId("fields-confirm").click();
    const [p] = await calls(page, "save_preset");
    expect(p.args.name).toBe("Punchy");
    expect(p.args.fields).toEqual(["contrast"]);
    expect(JSON.parse((await page.evaluate(() => localStorage.getItem("sieve.presetFields.v1")))!)).toEqual(["contrast"]);
    await expect(page.getByTestId("preset-list")).toContainText("Punchy");
  });
});

test.describe("copy / paste / previous / sync shortcuts", () => {
  test("Cmd+Shift+C / Cmd+Shift+V copy and paste with a tooltip that names the source", async ({ page }) => {
    await openDevelop(page, 1);
    await setSlider(page, "exposure", 1.25);
    await page.keyboard.press("Meta+Shift+c");
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("paste-settings")).toBeEnabled();
    await expect(page.getByTestId("paste-settings")).toHaveAttribute("title", /^Paste \d+ settings from DSC00001 to 1 photo \(Cmd\+Shift\+V\)$/);
    await page.getByTestId("film-2").click();
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "2");
    await clearCalls(page);
    await page.keyboard.press("Meta+Shift+v");
    await expect(page.getByTestId("slider-value-exposure")).toHaveText("+1.25");
    const [paste] = await calls(page, "paste_settings");
    expect(paste.args.ids).toEqual([2]);
    await expect(page.getByTestId("notice").last()).toContainText("settings to 1 photo");
    await expect(page.getByTestId("batch-undo")).toBeVisible();
    await page.getByTestId("batch-undo").click();
    await expect(page.getByTestId("slider-value-exposure")).toHaveText("0.00");
  });

  test("Previous pastes the photo edited before this one (Cmd+Alt+V too) and is disabled until there is one", async ({ page }) => {
    await openDevelop(page, 1);
    await expect(page.getByTestId("previous-settings")).toBeDisabled();
    await expect(page.getByTestId("previous-settings")).toHaveAttribute("title", "No previous photo yet");
    await setSlider(page, "exposure", 0.75);
    await page.getByTestId("film-2").click();
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "2");
    await expect(page.getByTestId("previous-settings")).toBeEnabled();
    await clearCalls(page);
    await page.getByTestId("previous-settings").click();
    await expect(page.getByTestId("slider-value-exposure")).toHaveText("+0.75");
    await expect.poll(async () => (await saved(page)).at(-1)?.args.label).toBe("Paste Settings");
    await page.getByTestId("film-3").click();
    await expect(page.getByTestId("slider-value-exposure")).toHaveText("0.00");
    await page.keyboard.press("Meta+Alt+v");
    await expect(page.getByTestId("slider-value-exposure")).toHaveText("+0.75");
  });

  test("a multi-selection turns Previous into Sync…; Cmd+Alt+S and Alt-click sync without the dialog; Reset shows the count", async ({ page }) => {
    await openDevelop(page, 1);
    await setSlider(page, "vibrance", 30);
    await page.getByTestId("film-2").click({ modifiers: ["Meta"] });
    await page.getByTestId("film-3").click({ modifiers: ["Meta"] });
    await page.getByTestId("film-1").click({ modifiers: ["Meta"] }); // re-activate 1 (toggle off + on keeps the others)
    await page.getByTestId("film-1").click({ modifiers: ["Meta"] });
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "1");
    await expect(page.getByTestId("previous-settings")).toHaveCount(0);
    await expect(page.getByTestId("sync-settings")).toHaveText("Sync…");
    await expect(page.getByTestId("reset-all")).toHaveText(/Reset \(\d+\)/);
    await expect(page.getByTestId("filmstrip-header")).toContainText("selected");
    await shot(page, "develop-layout-sync-bar");

    await clearCalls(page);
    await page.getByTestId("sync-settings").click();
    await expect(page.getByRole("dialog")).toContainText("Synchronize Settings");
    await expect(page.getByTestId("fields-confirm")).toHaveText("Synchronize");
    await page.getByTestId("fields-none").click();
    await page.getByTestId("field-vibrance").check();
    await page.getByTestId("fields-confirm").click();
    await expect.poll(async () => (await calls(page, "sync_settings")).length).toBe(1);
    expect((await calls(page, "sync_settings"))[0].args.fields).toEqual(["vibrance"]);

    // Cmd+Alt+S and Alt-click use the remembered fields (shared with Copy) without asking.
    await clearCalls(page);
    await page.keyboard.press("Meta+Alt+s");
    await expect.poll(async () => (await calls(page, "sync_settings")).length).toBe(1);
    expect((await calls(page, "sync_settings"))[0].args.fields).toEqual(["vibrance"]);
    await expect(page.getByTestId("fields-dialog")).toHaveCount(0);
    await page.getByTestId("sync-settings").click({ modifiers: ["Alt"] });
    await expect.poll(async () => (await calls(page, "sync_settings")).length).toBe(2);
    await expect(page.getByTestId("fields-dialog")).toHaveCount(0);
  });
});

for (const vp of [
  { w: 1280, h: 800 },
  { w: 1728, h: 1117 },
]) {
  test(`screenshots ${vp.w}x${vp.h}: nothing clipped or overlapping`, async ({ page }) => {
    await page.setViewportSize({ width: vp.w, height: vp.h });
    await openDevelop(page, 1);
    await shot(page, `develop-layout-${vp.w}-basic`);
    // The panels fit the window and the bars stay on screen.
    for (const id of ["left-bar", "right-bar", "viewer-toolbar", "filmstrip-header", "filmstrip"]) {
      const b = (await page.getByTestId(id).boundingBox())!;
      expect(b.y + b.height, id).toBeLessThanOrEqual(vp.h + 0.5);
      expect(b.x + b.width, id).toBeLessThanOrEqual(vp.w + 0.5);
    }
    const left = (await page.getByTestId("left-aside").boundingBox())!;
    const right = (await page.getByTestId("right-aside").boundingBox())!;
    expect(left.width).toBe(vp.w >= 1600 ? 240 : 224);
    expect(right.width).toBe(vp.w >= 1600 ? 320 : 288);
    expect((await page.getByTestId("filmstrip").boundingBox())!.height).toBe(vp.w >= 1600 ? 96 : 80);
    await openSection(page, "tone-curve");
    await openSection(page, "hsl");
    await shot(page, `develop-layout-${vp.w}-sections`);
    await page.getByTestId("tool-masking").click();
    await shot(page, `develop-layout-${vp.w}-masks`);
    await page.getByTestId("tool-masking").click();
    await page.getByTestId("profile-browse").click();
    await expect(page.getByTestId("profile-browser")).toBeVisible();
    await shot(page, `develop-layout-${vp.w}-profile-browser`);
    await page.keyboard.press("Escape"); // Esc closes the browser, Develop stays open
    await expect(page.getByTestId("profile-browser")).toHaveCount(0);
    await expect(page.getByTestId("develop-view")).toBeVisible();
  });
}
