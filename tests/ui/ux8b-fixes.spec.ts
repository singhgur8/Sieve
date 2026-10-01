// UX review 8b fix round (docs/ux-review-8b.md): P0-1, P0-2, P1-1, P1-6, P1-7, P1-8, P1-9 and the cheap P2s.
import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openApp, openHome, shot } from "./helpers";

const P = "ux8b-";

async function openProject(page: Page, query = "&style=ready") {
  await openHome(page, 201, query);
  await page.getByTestId("project-open-1").click();
  await expect(page.getByTestId("grid-toolbar")).toBeVisible();
}

async function openPlan(page: Page, query = "&style=ready") {
  await openProject(page, query);
  await page.getByTestId("continue-edit").click();
  await expect(page.getByTestId("plan-view")).toBeVisible();
  await expect(page.locator('[data-testid^="plan-scene-"]')).toHaveCount(3);
}

async function sceneIds(page: Page): Promise<number[]> {
  const t = await page.locator('article[data-testid^="plan-scene-"]').evaluateAll((els) => els.map((e) => e.getAttribute("data-testid")!));
  return t.map((x) => Number(x.replace("plan-scene-", "")));
}

const imageId = async (page: Page) => Number(await page.getByTestId("develop-view").getAttribute("data-image-id"));

/** Representative photo of every scene, read by opening each scene from the plan. */
async function representatives(page: Page, ids: number[]): Promise<number[]> {
  const reps: number[] = [];
  for (const id of ids) {
    await page.getByTestId(`plan-edit-${id}`).click();
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await expect(page.getByTestId("edit-chip")).toHaveAttribute("data-kind", "rep");
    reps.push(await imageId(page));
    await page.keyboard.press("g");
    await expect(page.getByTestId("plan-view")).toBeVisible();
  }
  return reps;
}

test.describe("P0-1 scene navigation lands on the representative", () => {
  test("next / previous scene, N / Shift+N and the checklist open the representative", async ({ page }) => {
    await openPlan(page);
    const ids = await sceneIds(page);
    const reps = await representatives(page, ids);
    expect(new Set(reps).size).toBe(3);
    const atRep = async (i: number) => {
      await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", String(reps[i]));
      await expect(page.getByTestId("edit-chip")).toHaveAttribute("data-kind", "rep");
    };
    await page.getByTestId(`plan-edit-${ids[0]}`).click();
    await atRep(0);
    await page.getByTestId("edit-next-scene").click();
    await expect(page.getByTestId("edit-scene-menu")).toHaveText("Scene 2 of 3");
    await atRep(1);
    await page.getByTestId("edit-prev-scene").click();
    await expect(page.getByTestId("edit-scene-menu")).toHaveText("Scene 1 of 3");
    await atRep(0);
    await page.keyboard.press("n");
    await expect(page.getByTestId("edit-scene-menu")).toHaveText("Scene 2 of 3");
    await atRep(1);
    await page.keyboard.press("Shift+n");
    await expect(page.getByTestId("edit-scene-menu")).toHaveText("Scene 1 of 3");
    await atRep(0);
    await page.getByTestId("edit-scene-menu").click();
    await page.getByTestId(`edit-checklist-${ids[2]}`).click();
    await expect(page.getByTestId("edit-scene-menu")).toHaveText("Scene 3 of 3");
    await atRep(2);
    await page.getByTestId("edit-scene-menu").click();
    await page.getByTestId(`edit-checklist-${ids[0]}`).click();
    await atRep(0);
  });
});

test.describe("P0-2 hover previews never outlive their hover", () => {
  async function openDevelop(page: Page) {
    await openApp(page, 200);
    await page.getByTestId("cell-1").click();
    await page.keyboard.press("d");
    await expect(page.getByTestId("histogram")).toHaveAttribute("data-empty", "false");
  }

  test("Esc, closing the browser, leaving the panel and window blur clear the preview", async ({ page }) => {
    await openDevelop(page);
    await page.getByTestId("profile-browse").click();
    const item = page.getByTestId("profile-item-Camera Standard");
    const label = page.getByTestId("hover-preview-label");
    // Esc with the pointer still on the row.
    await item.hover();
    await expect(label).toHaveText("Preview: Camera Standard");
    await page.keyboard.press("Escape");
    await expect(label).toHaveCount(0);
    await expect(page.getByTestId("profile-browser")).toHaveCount(0);
    // Close button while hovering a row.
    await page.getByTestId("profile-browse").click();
    await item.hover();
    await expect(label).toHaveCount(1);
    await page.getByTestId("profile-browser-close").click();
    await expect(label).toHaveCount(0);
    await page.waitForTimeout(400); // a late render must not resurrect it
    await expect(label).toHaveCount(0);
    // Pointer leaving the right panel.
    await page.getByTestId("profile-browse").click();
    await item.hover();
    await expect(label).toHaveCount(1);
    await page.mouse.move(500, 400);
    await expect(label).toHaveCount(0);
    // Window blur.
    await page.getByTestId("profile-item-none").hover();
    await item.hover();
    await expect(label).toHaveCount(1);
    await page.evaluate(() => window.dispatchEvent(new Event("blur")));
    await expect(label).toHaveCount(0);
    // A commit (click) replaces the preview with the applied render.
    await page.getByTestId("profile-item-none").hover();
    await item.hover();
    await expect(label).toHaveCount(1);
    await item.click();
    await expect(label).toHaveCount(0);
  });

  test("changing photo clears the preview", async ({ page }) => {
    await openDevelop(page);
    await page.getByTestId("profile-browse").click();
    await page.getByTestId("profile-item-Camera Standard").hover();
    await expect(page.getByTestId("hover-preview-label")).toHaveCount(1);
    await page.keyboard.press("ArrowRight");
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "2");
    await expect(page.getByTestId("hover-preview-label")).toHaveCount(0);
  });
});

test.describe("P1-1 / P1-6 apply toast and batch undo stack", () => {
  test("toast Review opens the first needs-look frame", async ({ page }) => {
    await openPlan(page);
    const [a] = await sceneIds(page);
    await page.getByTestId(`plan-auto-${a}`).click();
    await expect(page.getByTestId(`plan-scene-${a}`)).toHaveAttribute("data-status", "auto");
    await page.getByTestId(`plan-apply-${a}`).click();
    await expect(page.getByTestId("apply-review")).toBeVisible();
    await page.getByTestId("apply-review").click();
    await expect(page.getByTestId("develop-view")).toBeVisible();
    expect((await imageId(page)) % 7).toBe(0);
    await expect(page.getByTestId("edit-chip")).toHaveAttribute("data-kind", "review");
  });

  test("Cmd+Z undoes edit batches one by one and retires the toast Undo", async ({ page }) => {
    await openPlan(page);
    const [a] = await sceneIds(page);
    const row = page.getByTestId(`plan-scene-${a}`);
    await page.getByTestId(`plan-auto-${a}`).click();
    await expect(row).toHaveAttribute("data-status", "auto");
    await page.getByTestId(`plan-apply-${a}`).click();
    await expect(row).toHaveAttribute("data-status", "applied");
    await expect(page.getByTestId("apply-undo-batch")).toBeVisible();
    await clearCalls(page);
    await page.keyboard.press("Meta+z"); // the apply
    await expect(row).toHaveAttribute("data-status", "auto");
    await expect(page.getByTestId("apply-undo-batch")).toHaveCount(0); // its toast Undo is gone
    await page.keyboard.press("Meta+z"); // the auto edit
    await expect.poll(async () => (await calls(page, "undo_edit_batch")).length).toBe(2);
    await expect(row).toHaveAttribute("data-status", "todo");
  });
});

test.describe("P1-7 toolbar fits at 1280", () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  test("Cull grid: no horizontal overflow, View menu, no folder select for one folder", async ({ page }) => {
    await openProject(page);
    const fits = () => page.getByTestId("grid-toolbar").evaluate((e) => e.scrollWidth <= e.clientWidth);
    expect(await fits()).toBe(true);
    await expect(page.getByTestId("view-menu")).toBeVisible();
    await expect(page.getByTestId("sort-select")).toHaveCount(0);
    await expect(page.getByTestId("folder-select")).toHaveCount(0);
    await expect(page.getByTestId("selection-count")).toBeVisible();
    await shot(page, `${P}1280-cull-toolbar`);
    await page.getByTestId("view-menu").click();
    await expect(page.getByTestId("sort-select")).toBeVisible();
    await expect(page.getByTestId("thumb-size")).toBeVisible();
    await expect(page.getByTestId("auto-advance")).toBeVisible();
    await shot(page, `${P}1280-view-menu`);
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("sort-select")).toHaveCount(0);
  });
});

test("P1-7 at 1440 the view controls stay inline", async ({ page }) => {
  await openProject(page);
  await expect(page.getByTestId("view-menu")).toHaveCount(0);
  await expect(page.getByTestId("sort-select")).toBeVisible();
});

test.describe("P1-8 cheat sheet keyboard", () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  test("scrolls with PgDn / Space, types to filter, Esc clears then closes, Workflow first in the Edit step", async ({ page }) => {
    await openPlan(page);
    await page.keyboard.press("?");
    const sheet = page.getByTestId("cheat-sheet");
    await expect(sheet).toBeVisible();
    await expect(page.getByTestId("cheat-subtitle")).toHaveText("Showing Edit step first");
    const first = await sheet.locator('[data-testid^="cheat-group-"]').first().getAttribute("data-testid");
    expect(first).toBe("cheat-group-Workflow");
    const cols = page.getByTestId("cheat-columns");
    await expect(page.getByTestId("cheat-fade")).toBeVisible();
    await shot(page, `${P}1280-cheatsheet`);
    const top = () => cols.evaluate((e) => e.scrollTop);
    expect(await top()).toBe(0);
    await cols.focus();
    await page.keyboard.press("PageDown");
    await expect.poll(top).toBeGreaterThan(100);
    const before = await top();
    await page.keyboard.press("Space");
    await expect(sheet).toBeVisible(); // Space scrolls, it does not close
    await expect.poll(top).toBeGreaterThan(before);
    await page.keyboard.press("PageUp");
    await page.keyboard.press("Home");
    await expect.poll(top).toBe(0);
    // Type to filter (a printable key on the content moves into the field).
    await page.keyboard.type("auto");
    await expect(page.getByTestId("cheat-filter")).toHaveValue("auto");
    await expect(page.getByTestId("cheat-autoEdit")).toBeVisible();
    await expect(page.getByTestId("cheat-pick")).toHaveCount(0);
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("cheat-filter")).toHaveValue("");
    await expect(sheet).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(sheet).toHaveCount(0);
  });

  test("PgDn scrolls straight after opening (filter field focused)", async ({ page }) => {
    await openProject(page);
    await page.keyboard.press("?");
    await expect(page.getByTestId("cheat-sheet")).toBeVisible();
    await page.keyboard.press("PageDown");
    await expect.poll(() => page.getByTestId("cheat-columns").evaluate((e) => e.scrollTop)).toBeGreaterThan(100);
  });
});

test.describe("P1-9 New project import options", () => {
  test("checkboxes drive the createProject options", async ({ page }) => {
    await openHome(page, 201, "&projects=0");
    await page.getByTestId("empty-new-project").click();
    const nonraw = page.getByTestId("new-project-nonraw");
    const pair = page.getByTestId("new-project-pair");
    await expect(nonraw).not.toBeChecked();
    await expect(pair).toBeDisabled();
    await nonraw.check();
    await expect(pair).toBeEnabled();
    await expect(pair).toBeChecked();
    await pair.uncheck();
    await page.getByTestId("new-project-pick").click();
    await clearCalls(page);
    await page.getByTestId("new-project-create").click();
    await expect.poll(async () => (await calls(page, "create_project")).length).toBe(1);
    const [c] = await calls(page, "create_project");
    expect(c.args.options).toMatchObject({ includeNonRaw: true, pairJpegWithRaw: false });
  });
});

test("P2: Snapshots hidden, labelled view buttons below 1440", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await openPlan(page);
  await expect(page.getByTestId("mode-plan")).toContainText("Plan");
  await expect(page.getByTestId("mode-develop")).toHaveAttribute("aria-label", "Develop");
  await expect(page.getByTestId("mode-develop").locator("span")).toBeHidden();
  await page.getByTestId("mode-develop").click();
  await expect(page.getByTestId("mode-develop")).toContainText("Develop");
  await expect(page.getByTestId("section-snapshots")).toHaveCount(0);
  await shot(page, `${P}1280-topbar`);
});
