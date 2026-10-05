import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openApp, openHome, shot } from "./helpers";

// UX review 8d, P1-1 (frontend part: Auto Sync), P1-2, P1-3, P1-6, P1-7 (chips, see ux8c-recheck N4), P1-8.
const V18 = "&keepers=not_rejected";

async function openProject(page: Page, extra = "") {
  await openHome(page, 201, V18 + extra);
  await page.getByTestId("project-open-1").click();
  await expect(page.getByTestId("cull-summary")).toBeVisible();
}

async function setSlider(page: Page, id: string, value: number) {
  const s = page.getByTestId(`slider-${id}`);
  await s.fill(String(value));
  await s.evaluate((el) => (el as HTMLElement).blur());
}

const box = async (page: Page, id: string) => (await page.getByTestId(id).boundingBox())!;

test.describe("P1-2 the selection bar floats", () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  test("Shift-clicking a range does not move the grid (no project: bar shown, floating)", async ({ page }) => {
    await openApp(page, 60);
    await page.getByTestId("cell-2").click();
    const before = await box(page, "cell-2");
    await page.getByTestId("cell-6").click({ modifiers: ["Shift"] });
    await expect(page.getByTestId("selection-bar")).toBeVisible();
    const after = await box(page, "cell-2");
    expect(after.y).toBe(before.y);
    expect(after.x).toBe(before.x);
    const bar = await box(page, "selection-bar");
    const toolbar = await box(page, "grid-toolbar").catch(() => null);
    if (toolbar) expect(bar.y).toBeGreaterThanOrEqual(toolbar.y + toolbar.height);
    expect(bar.y + bar.height).toBeLessThanOrEqual(800);
    await shot(page, "p1-2-selection-bar");
  });

  test("Cull step with an empty clipboard and no scene filter shows no bar at all", async ({ page }) => {
    await openProject(page);
    await page.getByTestId("cell-2").click();
    const before = await box(page, "cell-2");
    await page.getByTestId("cell-6").click({ modifiers: ["Shift"] });
    await expect(page.getByTestId("selection-count")).toContainText("5 selected");
    await expect(page.getByTestId("selection-bar")).toHaveCount(0);
    expect(await box(page, "cell-2")).toEqual(before);
  });
});

test.describe("P1-6 strictness text", () => {
  test("each level shows the new explanation, and Help says the same", async ({ page }) => {
    await openProject(page);
    const T = {
      conservative: "Only unusable frames: nothing in focus, far too dark or blown out, or several defects at once. Closed eyes and burst duplicates are never rejected.",
      balanced: "Also missed focus or motion blur on the main subject, closed eyes on the main subject, and burst frames clearly worse than the best one.",
      aggressive: "Also any closed eyes or soft focus, and weaker burst frames even when the difference is small. Expect some keepers among the suggestions.",
    };
    for (const k of ["conservative", "balanced", "aggressive"] as const) {
      await page.getByTestId("reject-strictness").selectOption(k);
      await expect(page.getByTestId("strictness-explain")).toHaveText(T[k]);
    }
    await page.keyboard.press("F1");
    await page.getByTestId("help-nav-reject-strictness").click();
    const body = ((await page.getByTestId("help-panel").textContent()) ?? "").toLowerCase();
    for (const s of ["only unusable frames", "closed eyes and burst duplicates are never rejected", "closed eyes on the main subject, and burst frames clearly worse than the best one", "expect some keepers among the suggestions"]) expect(body).toContain(s);
  });
});

test.describe("P1-3 Sync two cameras", () => {
  test("one selected photo: both frame pickers are empty and Apply is disabled; the list has no cap and is searchable", async ({ page }) => {
    await openApp(page, 1500, "&twocams=1");
    await page.getByTestId("cell-2").click();
    await page.keyboard.press("Meta+Shift+t");
    await page.getByTestId("capture-tab-sync").click();
    await expect(page.getByTestId("capture-ref-frame-empty")).toBeVisible();
    await expect(page.getByTestId("capture-tgt-frame-empty")).toBeVisible();
    await expect(page.getByTestId("capture-apply")).toBeDisabled();
    await expect(page.getByTestId("capture-sync-hint")).toContainText("Cmd-click one photo from each camera");
    // No 500-entry cap: the list holds every frame of the camera (virtualized: few DOM rows).
    const n = Number(await page.getByTestId("capture-tgt-frame-list").getAttribute("data-count")); // the photo's own camera (1000 of 1500)
    expect(n).toBeGreaterThan(500);
    expect(await page.locator('[data-testid^="capture-tgt-frame-opt-"]').count()).toBeLessThan(60);
    await page.getByTestId("capture-tgt-frame-search").fill("DSC00002");
    expect(Number(await page.getByTestId("capture-tgt-frame-list").getAttribute("data-count"))).toBeLessThan(n);
    await page.getByTestId("capture-tgt-frame-opt-2").click();
    await expect(page.getByTestId("capture-tgt-frame-chosen")).toBeVisible();
    await expect(page.getByTestId("capture-tgt-frame-time")).not.toBeEmpty();
    await expect(page.getByTestId("capture-apply")).toBeDisabled(); // the second frame is still missing
  });

  test("one frame of each camera selected: opens on Sync with the pair and both thumbnails", async ({ page }) => {
    await openApp(page, 60, "&twocams=1");
    await page.getByTestId("cell-2").click();
    await page.getByTestId("cell-3").click({ modifiers: ["Meta"] });
    await page.keyboard.press("Meta+Shift+t");
    await expect(page.getByTestId("capture-pane-sync")).toBeVisible();
    await expect(page.getByTestId("capture-ref-frame-chosen")).toHaveAttribute("data-id", "2");
    await expect(page.getByTestId("capture-tgt-frame-chosen")).toHaveAttribute("data-id", "3");
    await expect(page.getByTestId("capture-ref-frame-chosen").locator("img")).toBeVisible();
    await expect(page.getByTestId("capture-tgt-frame-chosen").locator("img")).toBeVisible();
    await expect(page.getByTestId("capture-scope")).toContainText("of this project moves");
    await shot(page, "p1-3-sync-pair");
  });
});

test.describe("P1-1 Auto Sync (frontend)", () => {
  async function editThree(page: Page) {
    await openApp(page, 60);
    await page.getByTestId("cell-1").click();
    await page.getByTestId("cell-3").click({ modifiers: ["Shift"] });
    await page.getByTestId("sel-edit-all").click();
    await expect(page.getByTestId("develop-view")).toBeVisible();
  }

  test("Edit N selected turns Auto Sync on; a committed change is ONE sync_delta (source + 2 targets), one Cmd+Z reverts the whole batch", async ({ page }) => {
    await editThree(page);
    await expect(page.getByTestId("auto-sync-switch")).toHaveAttribute("aria-checked", "true");
    await expect(page.getByTestId("sync-settings")).toContainText("Auto Sync · 3");
    await clearCalls(page);
    await setSlider(page, "contrast", 25);
    await expect.poll(async () => (await calls(page, "sync_delta")).length).toBe(1);
    const c = (await calls(page, "sync_delta"))[0].args as any;
    expect(c.targetIds).toHaveLength(2);
    expect([c.sourceId, ...c.targetIds].sort()).toEqual([1, 2, 3]);
    expect(c.before.contrast).toBe(0);
    expect(c.after.contrast).toBe(25);
    expect((await calls(page, "save_adjustments")).length).toBe(0);
    expect((await calls(page, "sync_settings")).length).toBe(0);
    await expect(page.getByTestId("paste-undo-batch")).toBeVisible();
    await page.keyboard.press("Meta+z");
    await expect.poll(async () => (await calls(page, "undo_edit_batch")).length).toBe(1);
    expect((await calls(page, "undo_adjustments")).length).toBe(0); // atomic: the batch includes the source
  });

  test("exposure and white balance sync too, as a change (relative), through the same single call", async ({ page }) => {
    await editThree(page);
    await expect(page.getByTestId("auto-sync-note")).toContainText("Exposure and white balance");
    await clearCalls(page);
    await setSlider(page, "exposure", 0.5);
    await expect.poll(async () => (await calls(page, "sync_delta")).length).toBe(1);
    expect(((await calls(page, "sync_delta"))[0].args as any).after.exposure).toBe(0.5);
    expect((await calls(page, "sync_settings")).length).toBe(0);
  });

  test("Cmd+Alt+Shift+A flips the switch; off means no sync_settings", async ({ page }) => {
    await editThree(page);
    await page.keyboard.press("Meta+Alt+Shift+A");
    await expect(page.getByTestId("auto-sync-switch")).toHaveAttribute("aria-checked", "false");
    await clearCalls(page);
    await setSlider(page, "contrast", 10);
    await expect.poll(async () => (await calls(page, "save_adjustments")).length).toBe(1);
    expect((await calls(page, "sync_settings")).length).toBe(0);
    await page.keyboard.press("Meta+Alt+Shift+A");
    await expect(page.getByTestId("auto-sync-switch")).toHaveAttribute("aria-checked", "true");
  });
});

test.describe("P1-8 1280 px layout", () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  async function openPlan(page: Page) {
    await openHome(page, 201, "&style=ready");
    await page.getByTestId("project-open-1").click();
    await page.getByTestId("continue-edit").click();
    await expect(page.getByTestId("plan-view")).toBeVisible();
    await expect(page.locator('[data-testid^="plan-scene-"]').first()).toBeVisible();
  }
  const inside = async (page: Page, id: string, w = 1280) => {
    const b = await box(page, id);
    expect(b.x + b.width, id).toBeLessThanOrEqual(w);
    expect(b.x, id).toBeGreaterThanOrEqual(0);
  };

  test("Plan header: Apply all and the menu are on screen, the reason sits under the button", async ({ page }) => {
    await openPlan(page);
    await inside(page, "plan-apply-all");
    await inside(page, "plan-more");
    await expect(page.getByTestId("plan-apply-all-why")).toBeVisible();
    await inside(page, "plan-apply-all-why");
    const a = await box(page, "plan-apply-all");
    const w = await box(page, "plan-apply-all-why");
    expect(w.y).toBeGreaterThan(a.y);
    await shot(page, "p1-8-plan-1280");
  });

  test("Edit step: the top bar's More menu is not clipped, in the Plan and in Develop", async ({ page }) => {
    await openPlan(page);
    await inside(page, "more-menu");
    const scene = (await page.locator('article[data-testid^="plan-scene-"]').first().getAttribute("data-testid"))!.replace("plan-scene-", "");
    await page.getByTestId(`plan-edit-${scene}`).click();
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await inside(page, "more-menu");
  });

  for (const w of [1280, 1728]) {
    test(`Navigator zoom buttons stay inside the left panel at ${w}`, async ({ page }) => {
      await page.setViewportSize({ width: w, height: 900 });
      await openApp(page, 30);
      await page.getByTestId("cell-1").dblclick();
      await page.keyboard.press("d");
      await expect(page.getByTestId("develop-view")).toBeVisible();
      const panel = await box(page, "left-panel");
      const title = await box(page, "section-toggle-navigator");
      for (const id of ["nav-fit", "nav-fill", "nav-100", "nav-200"]) {
        const b = await box(page, id);
        expect(b.x + b.width, id).toBeLessThanOrEqual(panel.x + panel.width);
        expect(b.x, id).toBeGreaterThanOrEqual(title.x + title.width + 4);
      }
      await expect(page.getByTestId("nav-fit")).toHaveText("Fit");
    });
  }
});
