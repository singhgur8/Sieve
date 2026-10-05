import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openApp, openHome, openSection } from "./helpers";

// UX 8d re-check 1, part 2: R1-3 step 1, R1-4 and the P2s.

async function openDevelop(page: Page, query = "") {
  await openApp(page, 200, query);
  await page.getByTestId("cell-1").click();
  await page.keyboard.press("d");
  await expect(page.getByTestId("histogram")).toHaveAttribute("data-empty", "false");
}

test.describe("R1-3 step 1: the crop tool renders the whole warped frame", () => {
  test("while cropping, render_preview has crop.enabled=false and transform.constrainCrop=false", async ({ page }) => {
    await openDevelop(page);
    await openSection(page, "transform");
    await page.getByTestId("tf-constrain").check();
    await clearCalls(page);
    await page.keyboard.press("r");
    await expect(page.getByTestId("crop-overlay")).toBeVisible();
    await expect.poll(async () => (await calls(page, "render_preview")).length).toBeGreaterThan(0);
    const a = (await calls(page, "render_preview")).at(-1)!.args.adjustments as any;
    expect(a.crop.enabled).toBe(false);
    expect(a.transform.constrainCrop).toBe(false);
  });
});

test.describe("R1-3 v19.3 warp outline", () => {
  test("Constrain Crop on + Vertical: the tool starts at the constrained crop and a corner stops at the warp edge", async ({ page }) => {
    await openDevelop(page);
    await openSection(page, "transform");
    await page.getByTestId("upright-vertical").click();
    await page.getByTestId("tf-constrain").check();
    await clearCalls(page);
    await page.keyboard.press("r");
    await expect(page.getByTestId("crop-overlay")).toBeVisible();
    await expect.poll(async () => (await calls(page, "get_transform_bounds")).length).toBeGreaterThan(0);
    const rect = async () => JSON.parse((await page.getByTestId("crop-rect").getAttribute("data-rect"))!) as { l: number; t: number; r: number; b: number };
    await expect.poll(async () => (await rect()).l).toBeGreaterThan(0.04); // the auto-constrained frame, not the full frame
    await page.keyboard.press("a"); // free
    const h = (await page.getByTestId("crop-handle-nw").boundingBox())!;
    const f = (await page.getByTestId("crop-frame").boundingBox())!;
    await page.mouse.move(h.x + h.width / 2, h.y + h.height / 2);
    await page.mouse.down();
    await page.mouse.move(f.x + 1, f.y + 1, { steps: 6 });
    await page.mouse.up();
    const r = await rect();
    expect(r.l).toBeGreaterThan(0.04); // stopped at the warp edge (the left edge leans in by ~0.06 at the bottom)
  });
});

test.describe("R1-4 Edit Capture Time at 1280x800", () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  async function openPair(page: Page) {
    await openApp(page, 60, "&twocams=1");
    await page.getByTestId("cell-2").click();
    await page.getByTestId("cell-3").click({ modifiers: ["Meta"] });
    await page.keyboard.press("Meta+Shift+t");
    await expect(page.getByTestId("capture-pane-sync")).toBeVisible();
  }

  test("Apply is inside the viewport, initial focus is not the help icon, Enter applies once", async ({ page }) => {
    await openPair(page);
    const b = (await page.getByTestId("capture-apply").boundingBox())!;
    expect(b.y + b.height).toBeLessThanOrEqual(800);
    expect(b.y).toBeGreaterThan(0);
    expect(await page.evaluate(() => document.activeElement?.getAttribute("data-testid"))).toBe("capture-ref-frame-search");
    await page.getByTestId("capture-tab-sync").focus();
    await clearCalls(page);
    await page.keyboard.press("Enter");
    await expect.poll(async () => (await calls(page, "edit_capture_time")).length).toBe(1);
  });

  test("Esc cancels", async ({ page }) => {
    await openPair(page);
    await clearCalls(page);
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("capture-time-dialog")).toHaveCount(0);
    expect((await calls(page, "edit_capture_time")).length).toBe(0);
  });

  test("R1-P2-5: the model scope reads 'This camera model: every ... photo, any body'", async ({ page }) => {
    await openPair(page);
    await expect(page.getByTestId("capture-scope-model").locator("xpath=..")).toContainText(/This camera model: every .+ photo, any body \(\d+\)/);
  });
});

test.describe("R1-P2-1 / 2 / 3 Auto Sync copy and selection bar", () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  test("notice, tooltip, switch label, no wrapping, grid bottom padding", async ({ page }) => {
    await openApp(page, 60);
    await page.getByTestId("cell-2").click();
    await page.getByTestId("cell-4").click({ modifiers: ["Shift"] });
    await expect(page.getByTestId("selection-bar")).toBeVisible();
    const count = (await page.getByTestId("selbar-count").boundingBox())!;
    expect(count.height).toBeLessThan(20); // one line
    await expect(page.getByTestId("sel-edit-all")).toHaveAttribute("title", "Open Develop with the whole selection, Auto Sync on (the whole scene when nothing is selected)");
    expect(await page.getByTestId("grid-scroll").evaluate((el) => getComputedStyle(el).paddingBottom)).toBe("56px");
    await page.getByTestId("sel-edit-all").click();
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await expect(page.getByTestId("auto-sync-switch")).toContainText("Auto Sync · 3");
    await expect(page.getByTestId("sync-settings")).toHaveText("Sync…");
    const sw = (await page.getByTestId("auto-sync-switch").boundingBox())!;
    expect(sw.height).toBeLessThan(30); // one line
  });
});

test.describe("R1-P2-4 / 6 / 7 and P2-12 in the Cull step", () => {
  async function openProject(page: Page, w = 1440) {
    await page.setViewportSize({ width: w, height: 900 });
    await openHome(page, 201, "&keepers=not_rejected");
    await page.getByTestId("project-open-1").click();
    await expect(page.getByTestId("cull-summary")).toBeVisible();
  }

  test("Auto 0 popover closes on Esc", async ({ page }) => {
    await openProject(page);
    await page.getByTestId("cull-sum-auto").click();
    await expect(page.getByTestId("auto-zero-note")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("auto-zero-note")).toHaveCount(0);
  });

  test("P2-12: strictness is inline in the summary bar at 1440, a second row at 1280", async ({ page }) => {
    await openProject(page, 1440);
    await expect(page.getByTestId("strictness-row")).toHaveAttribute("data-inline", "true");
    const s = (await page.getByTestId("reject-strictness").boundingBox())!;
    const bar = (await page.getByTestId("cull-summary").boundingBox())!;
    expect(s.y).toBeGreaterThanOrEqual(bar.y);
    expect(s.y + s.height).toBeLessThanOrEqual(bar.y + bar.height);
    await page.setViewportSize({ width: 1280, height: 800 });
    await expect(page.getByTestId("strictness-row")).toHaveAttribute("data-inline", "false");
  });

  test("R1-P2-6 / 7: Apply under the review filter lands on Rejected by Auto with a toast; counts have no stray spaces", async ({ page }) => {
    await openProject(page);
    const n = Number(await page.getByTestId("strictness-count").getAttribute("data-rejects"));
    await page.getByTestId("strictness-count").click();
    await page.getByTestId("cull-sum-suggest").click();
    await expect(page.getByTestId("apply-kind-rejects").locator("xpath=..")).toHaveText(`Rejects (${n})`);
    await page.getByTestId("apply-confirm").click();
    await expect(page.getByTestId("filter-suggested")).toHaveCount(0);
    await expect(page.getByTestId("cull-sum-auto")).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByTestId("toasts")).toContainText(`Rejected ${n} photo`);
    await expect(page.getByTestId("toasts")).toContainText("shown here");
    await expect(page.getByTestId("strictness-count")).toHaveText("No reject suggestions");
  });
});

test.describe("R1-P2-8 / 9 Transform", () => {
  test("Guided: only Guided is pressed, the stored mode is a hint, an on-image badge shows", async ({ page }) => {
    await openDevelop(page);
    await openSection(page, "transform");
    await page.getByTestId("upright-vertical").click();
    await expect(page.getByTestId("upright-vertical")).toHaveAttribute("aria-pressed", "true");
    await page.keyboard.press("Shift+T");
    await expect(page.getByTestId("upright-guided")).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByTestId("upright-vertical")).toHaveAttribute("aria-pressed", "false");
    await expect(page.getByTestId("upright-guided-current")).toContainText("Current: Vertical, kept until 2 guides are drawn");
    await expect(page.getByTestId("guided-badge")).toBeVisible();
  });

  test("a fine grid shows while a Transform slider changes and goes away after it stops", async ({ page }) => {
    await openDevelop(page);
    await openSection(page, "transform");
    await expect(page.getByTestId("transform-grid")).toHaveCount(0);
    await page.getByTestId("slider-tf-vertical").fill("20");
    await expect(page.getByTestId("transform-grid")).toBeVisible();
    await page.getByTestId("slider-tf-vertical").evaluate((el) => (el as HTMLElement).blur());
    await page.mouse.move(5, 5);
    await expect(page.getByTestId("transform-grid")).toHaveCount(0, { timeout: 3000 });
  });
});

test.describe("R1-P2-10 crop handle modifiers", () => {
  async function openCrop(page: Page) {
    await openDevelop(page);
    await page.keyboard.press("r");
    await expect(page.getByTestId("crop-overlay")).toBeVisible();
  }
  const rectOf = async (page: Page) => JSON.parse((await page.getByTestId("crop-rect").getAttribute("data-rect"))!) as { l: number; t: number; r: number; b: number };

  test("Alt-drag on a handle resizes about the centre", async ({ page }) => {
    await openCrop(page);
    await page.keyboard.press("a"); // free
    const h = (await page.getByTestId("crop-handle-e").boundingBox())!;
    const x = h.x + h.width / 2;
    const y = h.y + h.height / 2;
    await page.keyboard.down("Alt");
    await page.mouse.move(x, y);
    await page.mouse.down();
    await page.mouse.move(x - 80, y, { steps: 5 });
    await page.mouse.up();
    await page.keyboard.up("Alt");
    const r = await rectOf(page);
    expect(Math.abs(r.l - (1 - r.r))).toBeLessThan(0.005); // symmetric about the centre
    expect(r.r - r.l).toBeLessThan(0.95);
  });

  test("Shift-drag on a corner of a free crop keeps its ratio", async ({ page }) => {
    await openCrop(page);
    await page.keyboard.press("a"); // free
    const h = (await page.getByTestId("crop-handle-se").boundingBox())!;
    const x = h.x + h.width / 2;
    const y = h.y + h.height / 2;
    const r0 = await rectOf(page);
    const ratio0 = (r0.r - r0.l) / (r0.b - r0.t);
    await page.keyboard.down("Shift");
    await page.mouse.move(x, y);
    await page.mouse.down();
    await page.mouse.move(x - 150, y - 150, { steps: 5 });
    await page.mouse.up();
    await page.keyboard.up("Shift");
    const r = await rectOf(page);
    expect((r.r - r.l) / (r.b - r.t)).toBeCloseTo(ratio0, 2);
    expect(r.r - r.l).toBeLessThan(0.95);
  });
});

test.describe("R1-P2-11 cheat sheet and Cmd+Alt+R", () => {
  test("rows exist; Cmd+Alt+R outside the tool resets only the crop (one 'Reset Crop' entry)", async ({ page }) => {
    await openDevelop(page);
    await page.keyboard.press("r");
    await expect(page.getByTestId("crop-overlay")).toBeVisible();
    await page.getByTestId("slider-crop-angle").fill("4");
    await page.getByTestId("crop-done").click();
    await expect(page.getByTestId("crop-overlay")).toHaveCount(0);
    await clearCalls(page);
    await page.keyboard.press("Meta+Alt+r");
    await expect.poll(async () => (await calls(page, "save_adjustments")).filter((c) => c.args.label === "Reset Crop").length).toBe(1);
    const a = (await calls(page, "save_adjustments")).find((c) => c.args.label === "Reset Crop")!.args.adjustments as any;
    expect(a.crop.enabled).toBe(false);
    await page.keyboard.press("Shift+?");
    const sheet = page.getByTestId("cheat-sheet");
    await expect(sheet).toContainText("Draw a straighten line");
    await expect(sheet).toContainText("Cmd-drag");
    await expect(sheet).toContainText("Shift+double-click Angle");
  });
});

test.describe("P2-11 Plan rows", () => {
  test("a to-do row has no repeated reason; 'edit this photo' is the link", async ({ page }) => {
    await openHome(page, 201, "&style=ready");
    await page.getByTestId("project-open-1").click();
    await page.getByTestId("continue-edit").click();
    const row = page.locator('article[data-status="todo"]').first();
    await expect(row).toBeVisible();
    const id = (await row.getAttribute("data-testid"))!.replace("plan-scene-", "");
    await expect(page.getByTestId(`plan-todo-edit-${id}`)).toHaveText("edit this photo");
    await expect(page.getByTestId(`plan-apply-why-${id}`)).toHaveCount(0);
    await page.getByTestId(`plan-todo-edit-${id}`).click();
    await expect(page.getByTestId("develop-view")).toBeVisible();
  });
});
