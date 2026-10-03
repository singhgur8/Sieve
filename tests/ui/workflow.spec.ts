import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openHome, shot } from "./helpers";

// Mock rules (src/testing/mockBackend.ts): project 1 "ceremony" = ids 1..101, 43 keepers in 3 scenes (40-photo chunks).
// `?style=ready` = a trained style model, `?style=learnable` = untrained with enough edited photos (default: too few).
// Mock apply: targets with id % 7 == 0 do not converge ("need a look").
const W = "workflow-";

async function openProject(page: Page, query = "&style=ready", id = 1) {
  await openHome(page, 201, query);
  await page.getByTestId(`project-open-${id}`).click();
  await expect(page.getByTestId("grid-toolbar")).toBeVisible();
  await expect(page.getByTestId("step-bar")).toBeVisible();
}

/** Cull -> "Continue to Edit": the plan with its three scenes. */
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

const row = (page: Page, id: number) => page.getByTestId(`plan-scene-${id}`);
const toast = (page: Page) => page.getByTestId("notice").last();

async function setSlider(page: Page, id: string, value: number) {
  const s = page.getByTestId(`slider-${id}`);
  await s.fill(String(value));
  await s.evaluate((el) => (el as HTMLElement).blur());
}

/** Manually edits the representative of a scene in Develop (Exposure), then returns to the plan with G. */
async function editScene(page: Page, id: number, exposure = 0.7) {
  await page.getByTestId(`plan-edit-${id}`).click();
  await expect(page.getByTestId("develop-view")).toBeVisible();
  await expect(page.getByTestId("edit-context")).toBeVisible();
  await setSlider(page, "exposure", exposure);
  await expect.poll(async () => (await calls(page, "save_adjustments")).length).toBeGreaterThan(0);
  await page.keyboard.press("g");
  await expect(page.getByTestId("plan-view")).toBeVisible();
}

test.describe("guided workflow", () => {
  test("step bar switches Cull / Edit / Export, persists the step and is keyboard driven", async ({ page }) => {
    await openProject(page);
    const bar = page.getByTestId("step-bar");
    await expect(page.getByTestId("step-cull")).toHaveAttribute("data-state", "active");
    await expect(page.getByTestId("step-edit")).toHaveAttribute("data-state", "upcoming");
    await expect(page.getByTestId("step-export")).toHaveAttribute("data-state", "upcoming");
    await expect(page.getByTestId("step-cull-sub")).toHaveText("· 43 keepers");
    await expect(page.getByTestId("plan-view")).toHaveCount(0);
    // Shoot type and Analyze moved out of the TopBar into the Cull bar; the old Export button is gone (step 3 replaces it).
    await expect(page.getByTestId("top-bar").getByTestId("shoot-select")).toHaveCount(0);
    await expect(page.getByTestId("grid-toolbar").getByTestId("shoot-select")).toBeVisible();
    await expect(page.getByTestId("grid-toolbar").getByTestId("analyze-button")).toBeVisible();
    await expect(page.getByTestId("export-button")).toHaveCount(0);
    await shot(page, `${W}1-cull`);

    await clearCalls(page);
    await page.getByTestId("step-edit").click();
    await expect(page.getByTestId("plan-view")).toBeVisible();
    await expect(page.getByTestId("step-edit")).toHaveAttribute("data-state", "active");
    await expect(page.getByTestId("step-cull")).toHaveAttribute("data-state", "done");
    await expect(page.getByTestId("grid-toolbar")).toHaveCount(0);
    expect((await calls(page, "set_workflow_step")).map((c) => c.args)).toEqual([{ projectId: 1, step: "edit" }]);
    expect((await calls(page, "get_edit_plan")).length).toBeGreaterThan(0);
    await expect(page.getByTestId("step-edit-sub")).toHaveText("· 0 of 3 scenes");

    await page.getByTestId("step-cull").click();
    await expect(page.getByTestId("plan-view")).toHaveCount(0);
    await expect(page.getByTestId("cell-1")).toBeVisible();
    await expect(page.getByTestId("step-cull")).toHaveAttribute("data-state", "active");
    expect((await calls(page, "set_workflow_step")).map((c) => c.args.step)).toEqual(["edit", "cull"]);

    // Keys: Cmd+Alt+2 = Edit (again = Plan), Cmd+Alt+1 = Cull, Cmd+Alt+3 = Export.
    await page.keyboard.press("Meta+Alt+2");
    await expect(page.getByTestId("plan-view")).toBeVisible();
    await page.keyboard.press("Meta+Alt+1");
    await expect(page.getByTestId("plan-view")).toHaveCount(0);
    await page.keyboard.press("Meta+Alt+3");
    await expect(page.getByTestId("export-dialog")).toBeVisible();
    await expect(page.getByTestId("step-export")).toHaveAttribute("data-state", "active");
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("export-dialog")).toHaveCount(0);
    await expect(bar).toBeVisible();

    // The step is stored per project: back on the home page the card shows it.
    await page.getByTestId("home-button").click();
    await expect(page.getByTestId("project-step-1")).toHaveText("Cull");
  });

  test("Continue to Edit builds the plan: scenes, representatives, keepers only", async ({ page }) => {
    await openProject(page);
    await clearCalls(page);
    await page.getByTestId("continue-edit").click();
    await expect(page.getByTestId("plan-view")).toBeVisible();
    await expect(page.locator('article[data-testid^="plan-scene-"]')).toHaveCount(3);
    // Keepers with no scene were grouped first (detect_scenes over the project), then the plan was re-read.
    const detect = await calls(page, "detect_scenes");
    expect(detect).toHaveLength(1);
    expect(detect[0].args).toMatchObject({ folderId: null, projectId: 1 });
    await expect(page.getByTestId("plan-summary")).toContainText("3 scenes · 43 keepers");
    await expect(page.getByTestId("plan-counts")).toHaveText("0 edited · 0 applied · 3 to do");
    for (const id of await sceneIds(page)) await expect(row(page, id)).toHaveAttribute("data-status", "todo");
    await expect(page.getByTestId("plan-tab-all")).toContainText("3");
    await expect(page.getByTestId("plan-tab-todo")).toContainText("3");
    await expect(page.getByTestId("plan-tab-applied")).toContainText("0");
    await expect(page.getByTestId("plan-apply-all")).toBeDisabled();
    await expect(page.getByTestId("plan-auto-remaining")).toContainText("Auto edit 3 remaining");
    // Each row shows its representative (with the R badge) and only keepers.
    const first = (await sceneIds(page))[0];
    await expect(page.getByTestId(`plan-rep-${first}`)).toBeVisible();
    await expect(page.getByTestId(`plan-status-${first}`)).toHaveText("To do: edit this photo");
    await page.getByTestId("plan-tab-applied").click();
    await expect(page.locator('article[data-testid^="plan-scene-"]')).toHaveCount(0);
    await page.getByTestId("plan-tab-all").click();
    await shot(page, `${W}2-plan-fresh`);

    // Edit-step scope: the grid lists the project's keepers only.
    await page.getByTestId("mode-grid").click();
    await expect(page.getByTestId("plan-view")).toHaveCount(0);
    await expect(page.getByTestId("selection-count")).toContainText("43");
  });

  test("empty and error states of the plan", async ({ page }) => {
    await openProject(page, "&style=ready&nokeepers=1");
    await expect(page.getByTestId("step-cull-sub")).toHaveText("· No keepers yet");
    await page.getByTestId("continue-edit").click();
    await expect(page.getByTestId("plan-empty")).toContainText("No keepers yet.");
    await expect(page.getByTestId("plan-back-cull")).toBeVisible();
    await page.getByTestId("plan-back-cull").click();
    await expect(page.getByTestId("plan-view")).toHaveCount(0);
    await expect(page.getByTestId("step-cull")).toHaveAttribute("data-state", "active");
  });

  test("a plan that cannot be built offers Try again", async ({ page }) => {
    await openProject(page);
    await page.evaluate(() => (window.__mockFailPlan = true));
    await page.getByTestId("continue-edit").click();
    await expect(page.getByTestId("plan-error")).toContainText("Could not group the keepers into scenes.");
    await expect(page.getByTestId("plan-error")).toContainText("Could not read the scenes (mock)");
    await page.evaluate(() => (window.__mockFailPlan = false));
    await page.getByTestId("plan-retry").click();
    await expect(page.locator('article[data-testid^="plan-scene-"]')).toHaveCount(3);
  });

  test("keeper rule menu changes who counts as a keeper and re-reads the plan", async ({ page }) => {
    await openPlan(page);
    await clearCalls(page);
    await page.getByTestId("plan-keepers-rule").click();
    await page.getByTestId("keeper-rule-3").click(); // Picks, 5 stars
    const [call] = await calls(page, "set_keeper_rule");
    expect(call.args.rule).toEqual({ mode: "picks_and_ratings", minRating: 5, useSuggestions: false });
    await expect(page.getByTestId("plan-keepers-rule")).toContainText("Picks, 5★");
    await expect(page.getByTestId("plan-summary")).not.toContainText("43 keepers");
  });

  test("Auto edit (my style): ready model fills the representative, marked for review", async ({ page }) => {
    await openPlan(page);
    const [a, b] = await sceneIds(page);
    await clearCalls(page);
    await page.getByTestId(`plan-auto-${a}`).click();
    await expect(row(page, a)).toHaveAttribute("data-status", "auto");
    await expect(page.getByTestId(`plan-status-${a}`)).toHaveText("Auto edited (my style), review it");
    await expect(row(page, b)).toHaveAttribute("data-status", "todo");
    const [pred] = await calls(page, "apply_style_prediction");
    expect(pred.args.imageIds).toHaveLength(1);
    await expect(toast(page)).toContainText("Auto edited 1 scene");
    await expect(page.getByTestId(`plan-apply-${a}`)).toBeEnabled();
    await expect(page.getByTestId("plan-counts")).toHaveText("1 edited · 0 applied · 2 to do");
    await expect(page.getByTestId("plan-auto-remaining")).toContainText("Auto edit 2 remaining");

    // Undo from the toast restores the representative.
    await page.getByTestId("auto-undo").click();
    await expect(row(page, a)).toHaveAttribute("data-status", "todo");
    expect((await calls(page, "undo_edit_batch")).length).toBe(1);

    // All remaining at once, then a manual tweak turns "auto" into "edited".
    await page.getByTestId("plan-auto-remaining").click();
    for (const id of await sceneIds(page)) await expect(row(page, id)).toHaveAttribute("data-status", "auto");
    await expect(page.getByTestId("plan-auto-remaining")).toHaveCount(0);
    await page.getByTestId(`plan-edit-${a}`).click(); // "Review"
    await expect(page.getByTestId("edit-chip")).toContainText("Auto edited, review");
    await setSlider(page, "exposure", 1.2);
    await expect.poll(async () => (await calls(page, "save_adjustments")).length).toBeGreaterThan(0);
    await page.keyboard.press("g");
    await expect(row(page, a)).toHaveAttribute("data-status", "edited");
    await expect(row(page, b)).toHaveAttribute("data-status", "auto");
  });

  test("Auto edit asks before replacing a hand edit", async ({ page }) => {
    await openPlan(page);
    const [a] = await sceneIds(page);
    await editScene(page, a);
    await expect(row(page, a)).toHaveAttribute("data-status", "edited");
    await clearCalls(page);
    await page.getByTestId(`plan-auto-${a}`).click();
    await expect(page.getByTestId("replace-edit-dialog")).toContainText("Replace your edit on DSC");
    await page.getByTestId("replace-cancel").click();
    expect(await calls(page, "apply_style_prediction")).toHaveLength(0);
    await page.getByTestId(`plan-auto-${a}`).click();
    await page.getByTestId("replace-confirm").click();
    await expect(row(page, a)).toHaveAttribute("data-status", "auto");
    expect(await calls(page, "apply_style_prediction")).toHaveLength(1);
  });

  test("style model states: too few edits, learn first, training", async ({ page }) => {
    // Default: fewer than 20 edited photos -> disabled with the reason.
    await openPlan(page, "");
    const [a] = await sceneIds(page);
    await expect(page.getByTestId(`plan-auto-${a}`)).toBeDisabled();
    await expect(page.getByTestId(`plan-auto-${a}`)).toHaveAttribute("title", /Edit at least 20 photos \(you have 0\)/);
    await expect(page.getByTestId(`plan-auto-${a}`)).toHaveAttribute("data-style-state", "insufficient");
    await shot(page, `${W}3-plan-no-style`);
  });

  test("style model states: untrained with enough edits asks to learn, then auto edits", async ({ page }) => {
    await openPlan(page, "&style=learnable");
    const [a] = await sceneIds(page);
    await expect(page.getByTestId(`plan-auto-${a}`)).toHaveAttribute("data-style-state", "learn");
    await clearCalls(page);
    await page.getByTestId(`plan-auto-${a}`).click();
    await expect(page.getByTestId("learn-style-dialog")).toContainText("Learn your style first?");
    await expect(page.getByTestId("learn-style-dialog")).toContainText("394 photos");
    await page.getByTestId("learn-cancel").click();
    expect(await calls(page, "train_style_model")).toHaveLength(0);
    await page.getByTestId(`plan-auto-${a}`).click();
    await page.getByTestId("learn-confirm").click();
    await expect(row(page, a)).toHaveAttribute("data-status", "auto", { timeout: 10_000 });
    expect(await calls(page, "train_style_model")).toHaveLength(1);
    expect(await calls(page, "apply_style_prediction")).toHaveLength(1);
    // Now trained: the plan menu says so.
    await page.getByTestId("plan-more").click();
    await expect(page.getByTestId("plan-style-info")).toContainText("learned from 394 photos");
  });

  test("Apply to scene is reviewable and undoable", async ({ page }) => {
    await openPlan(page);
    const ids = await sceneIds(page);
    await page.getByTestId("plan-auto-remaining").click();
    for (const id of ids) await expect(row(page, id)).toHaveAttribute("data-status", "auto");
    await expect(page.getByTestId("plan-counts")).toHaveText("3 edited · 0 applied · 0 to do");
    await clearCalls(page);

    const [a, b] = ids;
    await page.getByTestId(`plan-apply-${a}`).click();
    await expect(row(page, a)).toHaveAttribute("data-status", "applied");
    await expect(row(page, b)).toHaveAttribute("data-status", "auto");
    const [ap] = await calls(page, "apply_scene_edit");
    expect(ap.args.sceneId).toBe(a);
    expect(ap.args.options).toBeNull(); // backend default: relative matching, exposure + white balance per frame
    await expect(toast(page)).toContainText("Applied Scene 1 to");
    await expect(page.getByTestId(`plan-status-${a}`)).toContainText("Applied to");
    await expect(page.getByTestId("plan-counts")).toHaveText("2 edited · 1 applied · 0 to do");
    await expect(page.getByTestId("step-edit-sub")).toHaveText("· 1 of 3 scenes");
    await shot(page, `${W}4-plan-applied`);

    // Frames that did not converge are flagged for review.
    await expect(page.getByTestId(`plan-status-${a}`)).toContainText("need a look");
    await expect(page.getByTestId(`plan-review-${a}`)).toBeVisible();

    // Undo from the toast.
    await page.getByTestId("apply-undo-batch").click();
    await expect(row(page, a)).toHaveAttribute("data-status", "auto");
    const undo = await calls(page, "undo_edit_batch");
    expect(undo).toHaveLength(1);
    await expect(toast(page)).toContainText("Undid Apply Scene 1");
    await expect(page.getByTestId("plan-counts")).toHaveText("3 edited · 0 applied · 0 to do");

    // Apply again, then undo with Cmd+Z (a batch is the newest undoable action).
    await page.getByTestId(`plan-apply-${a}`).click();
    await expect(row(page, a)).toHaveAttribute("data-status", "applied");
    await page.keyboard.press("Meta+z");
    await expect(row(page, a)).toHaveAttribute("data-status", "auto");
    expect(await calls(page, "undo_edit_batch")).toHaveLength(2);

    // Menu: Copy exactly (no matching) applies with matching off; Undo apply is available afterwards.
    await page.getByTestId(`plan-menu-${a}`).click();
    await expect(page.getByTestId(`plan-undo-${a}`)).toBeDisabled();
    await page.getByTestId(`plan-exact-${a}`).click();
    await expect(row(page, a)).toHaveAttribute("data-status", "applied");
    const exact = (await calls(page, "apply_scene_edit")).at(-1)!;
    expect((exact.args.options as { matchOptions: { matchExposure: boolean; matchWhiteBalance: boolean } }).matchOptions).toMatchObject({ matchExposure: false, matchWhiteBalance: false });
    await page.getByTestId(`plan-menu-${a}`).click();
    await page.getByTestId(`plan-undo-${a}`).click();
    await expect(row(page, a)).toHaveAttribute("data-status", "auto");
  });

  test("Review walks the frames that need a look", async ({ page }) => {
    await openPlan(page);
    const [a] = await sceneIds(page);
    await page.getByTestId(`plan-auto-${a}`).click();
    await expect(row(page, a)).toHaveAttribute("data-status", "auto");
    await page.getByTestId(`plan-apply-${a}`).click();
    await expect(row(page, a)).toHaveAttribute("data-status", "applied");
    await page.getByTestId(`plan-review-${a}`).click();
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await expect(page.getByTestId("edit-chip")).toHaveAttribute("data-kind", "review");
    await expect(page.getByTestId("edit-chip")).toContainText("Needs a look");
    await expect(page.getByTestId("scene-only")).toHaveAttribute("aria-pressed", "true");
    const open = Number(await page.getByTestId("develop-view").getAttribute("data-image-id"));
    expect(open % 7).toBe(0); // the mock does not converge ids divisible by 7
    await expect(page.getByTestId(`film-review-${open}`)).toBeVisible();
    await shot(page, `${W}5-develop-review`);
  });

  test("Apply all edited scenes is one undoable batch", async ({ page }) => {
    await openPlan(page);
    const ids = await sceneIds(page);
    await page.getByTestId("plan-auto-remaining").click();
    for (const id of ids) await expect(row(page, id)).toHaveAttribute("data-status", "auto");
    await clearCalls(page);
    await expect(page.getByTestId("plan-apply-all")).toContainText("Apply 3 edited scenes");
    await page.getByTestId("plan-apply-all").click();
    for (const id of ids) await expect(row(page, id)).toHaveAttribute("data-status", "applied");
    const [all] = await calls(page, "apply_all_edited_scenes");
    expect(all.args.projectId).toBe(1);
    expect(await calls(page, "apply_scene_edit")).toHaveLength(0);
    await expect(toast(page)).toContainText("Applied 3 scenes to");
    await expect(page.getByTestId("plan-all-done")).toHaveText("All 3 scenes done");
    await expect(page.getByTestId("plan-continue-export")).toBeVisible();
    await expect(page.getByTestId("plan-apply-all")).toHaveCount(0);
    await expect(page.getByTestId("step-edit-sub")).toHaveText("· All scenes applied");
    await expect(page.getByTestId("step-edit")).toHaveAttribute("data-state", "active"); // the current step stays active; it turns done once Export is entered

    await page.getByTestId("apply-undo-batch").click();
    for (const id of ids) await expect(row(page, id)).toHaveAttribute("data-status", "auto");
    expect(await calls(page, "undo_edit_batch")).toHaveLength(1);
    await expect(page.getByTestId("plan-apply-all")).toBeVisible();

    // With everything applied again, Continue to Export opens the export step.
    await page.getByTestId("plan-apply-all").click();
    await expect(page.getByTestId("plan-continue-export")).toBeVisible();
    await page.getByTestId("plan-continue-export").click();
    await expect(page.getByTestId("export-dialog")).toBeVisible();
  });

  test("Develop in the Edit step: context bar, representative, apply, scene navigation", async ({ page }) => {
    await openPlan(page);
    const [a, b] = await sceneIds(page);
    await page.getByTestId(`plan-edit-${a}`).click();
    await expect(page.getByTestId("develop-view")).toBeVisible();
    const ctx = page.getByTestId("edit-context");
    await expect(ctx).toBeVisible();
    await expect(page.getByTestId("edit-scene-menu")).toHaveText("Scene 1 of 3");
    await expect(page.getByTestId("edit-chip")).toContainText("To do");
    await expect(page.getByTestId("edit-apply")).toBeDisabled();
    await expect(page.getByTestId("edit-apply")).toHaveAttribute("title", /Edit this photo or auto edit it first/);
    await expect(page.getByTestId("scene-only")).toHaveAttribute("aria-pressed", "true");
    const rep = Number(await page.getByTestId("develop-view").getAttribute("data-image-id"));
    await expect(page.getByTestId(`film-rep-${rep}`)).toBeVisible();
    await shot(page, `${W}6-develop-todo`);

    // Auto edit from the bar, then apply from the bar.
    await clearCalls(page);
    await page.getByTestId("edit-auto").click();
    await expect(page.getByTestId("edit-chip")).toContainText("Auto edited, review");
    await expect(page.getByTestId("edit-apply")).toBeEnabled();
    await page.getByTestId("edit-apply").click();
    await expect(page.getByTestId("edit-chip")).toContainText("Applied");
    expect(await calls(page, "apply_scene_edit")).toHaveLength(1);

    // A member shows where its edit came from; "Make it the representative" replaces the representative.
    await page.getByTestId("film-2").click();
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "2");
    await expect(page.getByTestId("edit-chip")).toHaveAttribute("data-kind", /applied|member/);
    await clearCalls(page);
    await page.keyboard.press("Shift+A");
    const [setRep] = await calls(page, "set_scene_representative");
    expect(setRep.args).toEqual({ sceneId: a, imageId: 2 });
    await expect(page.getByTestId("film-rep-2")).toBeVisible();
    await expect(toast(page)).toContainText("Scene 1 representative: DSC00002");
    await page.keyboard.press("Shift+A");
    await expect(toast(page)).toContainText("Already the representative");

    // Scene navigation: N skips applied scenes, the checklist jumps anywhere, G returns to the plan.
    await page.keyboard.press("n");
    await expect(page.getByTestId("edit-scene-menu")).toHaveText("Scene 2 of 3");
    await page.getByTestId("edit-scene-menu").click();
    await expect(page.getByTestId(`edit-checklist-${a}`)).toHaveAttribute("data-status", /outdated|stale|applied/);
    await page.getByTestId(`edit-checklist-${b}`).click();
    await expect(page.getByTestId("edit-scene-menu")).toHaveText("Scene 2 of 3");
    await page.keyboard.press("Shift+N");
    await expect(page.getByTestId("edit-scene-menu")).toHaveText("Scene 1 of 3");
    await page.getByTestId("edit-plan").click();
    await expect(page.getByTestId("plan-view")).toBeVisible();
    await expect(page.getByTestId("plan-scene-" + a)).toHaveAttribute("data-status", /applied|stale/);
  });

  test("plan keyboard: arrows move focus, Enter opens the representative, Cmd+Alt+U auto edits, Cmd+Shift+Enter applies", async ({ page }) => {
    await openPlan(page);
    const [a, b] = await sceneIds(page);
    await expect(row(page, a)).toHaveAttribute("data-focused", "true");
    await page.keyboard.press("ArrowDown");
    await expect(row(page, b)).toHaveAttribute("data-focused", "true");
    await page.keyboard.press("Meta+Alt+u");
    await expect(row(page, b)).toHaveAttribute("data-status", "auto");
    await page.keyboard.press("Meta+Shift+Enter");
    await expect(row(page, b)).toHaveAttribute("data-status", "applied");
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await expect(page.getByTestId("edit-scene-menu")).toHaveText("Scene 2 of 3");
    await page.keyboard.press("g");
    await expect(page.getByTestId("plan-view")).toBeVisible();
  });

  test("Export step: dialog scoped to the project's keepers", async ({ page }) => {
    await openProject(page);
    await clearCalls(page);
    await page.getByTestId("step-export").click();
    await expect(page.getByTestId("export-dialog")).toBeVisible();
    await expect(page.getByTestId("export-title")).toHaveText("Export 43 keepers");
    await expect(page.getByTestId("export-scope-keepers")).toBeChecked();
    await expect(page.getByTestId("export-count")).toHaveText("43 photos");
    await expect(page.getByTestId("step-export")).toHaveAttribute("data-state", "active");
    await shot(page, `${W}7-export`);
    await page.getByTestId("export-choose-folder").click();
    await page.getByTestId("export-go").click();
    await expect(page.getByTestId("export-dialog")).toHaveCount(0);
    const [call] = await calls(page, "export_images");
    expect(call.args.ids).toHaveLength(43);
    // Only keepers: every id is a pick / rated / suggested photo of project 1 (ids 1..101).
    expect((call.args.ids as number[]).every((i) => i >= 1 && i <= 101)).toBe(true);
    expect((await calls(page, "set_workflow_step")).map((c) => c.args.step)).toContain("export");
    await expect(page.getByTestId("step-export")).toHaveAttribute("data-state", "active");

    // Other scopes stay available.
    await page.getByTestId("step-cull").click();
    await page.keyboard.press("Meta+Shift+E");
    await expect(page.getByTestId("export-dialog")).toBeVisible();
    await expect(page.getByTestId("export-scope-selection")).toBeVisible();
    await expect(page.getByTestId("export-scope-filtered")).toBeVisible();
  });

  test("screenshots of every step at both sizes", async ({ page }) => {
    for (const [w, h, tag] of [
      [1280, 800, "1280"],
      [1728, 1117, "1728"],
    ] as const) {
      await page.setViewportSize({ width: w, height: h });
      await openPlan(page);
      const ids = await sceneIds(page);
      await page.getByTestId(`plan-auto-${ids[0]}`).click();
      await expect(row(page, ids[0])).toHaveAttribute("data-status", "auto");
      await page.getByTestId(`plan-apply-${ids[0]}`).click();
      await expect(row(page, ids[0])).toHaveAttribute("data-status", "applied");
      await editScene(page, ids[1]);
      await expect(page.getByTestId("plan-view")).toBeVisible();
      await page.waitForTimeout(400);
      await shot(page, `${W}${tag}-plan`);
      await page.getByTestId(`plan-edit-${ids[0]}`).click();
      await expect(page.getByTestId("edit-context")).toBeVisible();
      await page.waitForTimeout(400);
      await shot(page, `${W}${tag}-develop-context`);
      await page.getByTestId("step-cull").click();
      await expect(page.getByTestId("cell-1")).toBeVisible();
      await page.waitForTimeout(300);
      await shot(page, `${W}${tag}-cull`);
      await page.getByTestId("step-export").click();
      await expect(page.getByTestId("export-dialog")).toBeVisible();
      await shot(page, `${W}${tag}-export`);
    }
  });
});
