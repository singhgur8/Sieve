// IPC v15 wired into the guided workflow (docs/ipc-changelog.md v15, docs/ux-review-8b.md P1-2..P1-5, P2 cancel / keepers / XMP failures).
// Mock rules: project 1 = ids 1..101, 43 keepers in scenes 1 (16 keepers), 2 (17), 3 (10); applies do not converge for ids % 7 == 0.
import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openApp, openHome, shot } from "./helpers";

const W = "v15-";

async function openProject(page: Page, query = "&style=ready", id = 1) {
  await openHome(page, 201, query);
  await page.getByTestId(`project-open-${id}`).click();
  await expect(page.getByTestId("grid-toolbar")).toBeVisible();
}

async function openPlan(page: Page, query = "&style=ready") {
  await openProject(page, query);
  await page.getByTestId("continue-edit").click();
  await expect(page.getByTestId("plan-view")).toBeVisible();
  await expect(page.locator('article[data-testid^="plan-scene-"]').first()).toBeVisible();
}

const row = (page: Page, id: number) => page.getByTestId(`plan-scene-${id}`);
const toast = (page: Page) => page.getByTestId("notice").last();
const inv = (page: Page, cmd: string, args: Record<string, unknown>) =>
  page.evaluate(([c, a]) => (window as unknown as { __TAURI_INTERNALS__: { invoke: (c: string, a: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke(c as string, a), [cmd, args] as const);

async function setSlider(page: Page, id: string, value: number) {
  const s = page.getByTestId(`slider-${id}`);
  await s.fill(String(value));
  await s.evaluate((el) => (el as HTMLElement).blur());
}

/** Auto edits every scene, then applies scene `id` (match). */
async function autoAndApply(page: Page, id: number) {
  await page.getByTestId("plan-auto-remaining").click();
  await expect(row(page, id)).toHaveAttribute("data-status", "auto");
  await page.getByTestId(`plan-apply-${id}`).click();
  await expect(row(page, id)).toHaveAttribute("data-status", "applied");
}

/** Cull grid: select the photo (scrolling to it with the keyboard) and Pick it. */
async function pickInCull(page: Page, id: number) {
  const cell = page.getByTestId(`cell-${id}`);
  if (!(await cell.isVisible())) await page.getByTestId("grid-scroll").evaluate((el, bottom) => (el.scrollTop = bottom ? el.scrollHeight : 0), id > 50);
  await cell.click();
  await page.keyboard.press("p");
}

const imageId = async (page: Page) => Number(await page.getByTestId("develop-view").getAttribute("data-image-id"));

test.describe("P1-2 persisted workflow state", () => {
  test("needs-a-look and auto-edited survive a project switch; Looks good clears the mark", async ({ page }) => {
    await openPlan(page);
    await page.getByTestId("plan-auto-remaining").click();
    await expect(row(page, 1)).toHaveAttribute("data-status", "auto");
    await page.getByTestId("plan-apply-1").click();
    await expect(row(page, 1)).toHaveAttribute("data-status", "applied");
    await expect(page.getByTestId("plan-status-1")).toContainText("1 need a look");
    await expect(page.getByTestId("plan-review-1")).toHaveText("Review 1");

    // Project switch (home and back): the marks come from the catalog, not from the session.
    await page.getByTestId("home-button").click();
    await expect(page.getByTestId("home-page")).toBeVisible();
    await page.getByTestId("project-open-1").click();
    await expect(page.getByTestId("plan-view")).toBeVisible();
    await expect(row(page, 1)).toHaveAttribute("data-status", "applied");
    await expect(page.getByTestId("plan-status-1")).toContainText("1 need a look");
    await expect(page.getByTestId("plan-review-1")).toHaveText("Review 1");
    // The other scenes were auto edited and still read "auto".
    await expect(row(page, 2)).toHaveAttribute("data-status", "auto");
    await expect(row(page, 3)).toHaveAttribute("data-status", "auto");

    // "Looks good" clears it through mark_reviewed.
    await page.getByTestId("plan-review-1").click();
    await expect(page.getByTestId("edit-chip")).toHaveAttribute("data-kind", "review");
    expect(await imageId(page)).toBe(21);
    await clearCalls(page);
    await page.getByTestId("edit-looks-good").click();
    expect((await calls(page, "mark_reviewed")).map((c) => c.args)).toEqual([{ imageIds: [21] }]);
    await expect(page.getByTestId("edit-chip")).toHaveAttribute("data-kind", "applied");
    await expect(page.getByTestId("film-review-21")).toHaveCount(0);
    await page.keyboard.press("g");
    await expect(page.getByTestId("plan-view")).toBeVisible();
    await expect(page.getByTestId("plan-status-1")).not.toContainText("need a look");
    await expect(page.getByTestId("plan-review-1")).toHaveCount(0);
  });

  test("editing a needs-a-look frame clears its mark; N walks plan.needsReviewIds", async ({ page }) => {
    await openPlan(page);
    await page.getByTestId("plan-auto-remaining").click();
    await expect(row(page, 2)).toHaveAttribute("data-status", "auto");
    await page.getByTestId("plan-apply-2").click();
    await expect(row(page, 2)).toHaveAttribute("data-status", "applied");
    await expect(page.getByTestId("plan-status-2")).toContainText("4 need a look"); // 49, 56, 70, 77
    await page.getByTestId("plan-review-2").click();
    await expect(page.getByTestId("edit-chip")).toHaveAttribute("data-kind", "review");
    expect(await imageId(page)).toBe(49);
    await expect(page.getByTestId("film-review-56")).toBeVisible();

    // Fix 49 by hand: the "!" goes away and N moves on to 56.
    await setSlider(page, "exposure", 0.9);
    await expect(page.getByTestId("film-review-49")).toHaveCount(0);
    await expect(page.getByTestId("edit-chip")).not.toHaveAttribute("data-kind", "review");
    await page.keyboard.press("n");
    await expect.poll(() => imageId(page)).toBe(56);
    await expect(page.getByTestId("edit-chip")).toHaveAttribute("data-kind", "review");
    await page.keyboard.press("n");
    await expect.poll(() => imageId(page)).toBe(70);
    // Cmd+Z returns 49 to the applied settings: the mark is back (it follows undo).
    await page.keyboard.press("g");
    await expect(page.getByTestId("plan-status-2")).toContainText("3 need a look");
    await shot(page, `${W}1-needs-look`);
  });

  test("Undo apply restores the previous provenance (marks vanish with the batch)", async ({ page }) => {
    await openPlan(page);
    await autoAndApply(page, 1);
    await expect(page.getByTestId("plan-review-1")).toBeVisible();
    await page.getByTestId("apply-undo-batch").click();
    await expect(row(page, 1)).toHaveAttribute("data-status", "auto");
    await expect(page.getByTestId("plan-review-1")).toHaveCount(0);
  });
});

test.describe("P1-3 outdated plans", () => {
  test("keepers added after an apply: Applied to N, new keepers not edited, Apply to M new, allDone", async ({ page }) => {
    await openPlan(page);
    await page.getByTestId("plan-auto-remaining").click();
    await expect(row(page, 3)).toHaveAttribute("data-status", "auto");
    await page.getByTestId("plan-apply-all").click();
    await expect(page.getByTestId("plan-continue-export")).toBeVisible();
    await expect(page.getByTestId("step-edit-sub")).toHaveText("· All scenes applied");

    // Cull: pick 82 (inside scene 3, not a keeper yet) and 42 (scene 2).
    await page.getByTestId("step-cull").click();
    await pickInCull(page, 82);
    await pickInCull(page, 4);
    await page.getByTestId("step-edit").click();
    await expect(page.getByTestId("plan-view")).toBeVisible();
    await expect(page.getByTestId("plan-status-3")).toContainText("1 new keeper not edited");
    await expect(page.getByTestId("plan-status-1")).toContainText("1 new keeper not edited");
    await expect(page.getByTestId("plan-apply-3")).toHaveText("Apply to 1 new");
    await expect(page.getByTestId("plan-continue-export")).toHaveCount(0);
    await expect(page.getByTestId("step-edit-sub")).not.toHaveText("· All scenes applied");
    await shot(page, `${W}2-new-keepers`);

    await clearCalls(page);
    await page.getByTestId("plan-apply-3").click();
    await expect(page.getByTestId("plan-status-3")).not.toContainText("new keeper");
    const [a] = await calls(page, "apply_scene_edit");
    expect(a.args.sceneId).toBe(3);
    await expect(page.getByTestId("plan-status-1")).toContainText("1 new keeper not edited");
    // Apply all handles the remaining scene, then everything is done again.
    await page.getByTestId("plan-apply-all").click();
    await expect(page.getByTestId("plan-continue-export")).toBeVisible();
    await expect(page.getByTestId("step-edit-sub")).toHaveText("· All scenes applied");
  });

  test("keepers outside every scene show a banner with Group them", async ({ page }) => {
    await openPlan(page);
    // Shrink scene 3 to two members: its other keepers lose their scene. Re-reading the plan (open a scene and come back) shows the banner.
    await inv(page, "set_scene_members", { id: 3, imageIds: [81, 85] });
    await page.getByTestId("plan-edit-1").click();
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await page.keyboard.press("g");
    await expect(page.getByTestId("plan-unassigned")).toContainText("8 keepers are not in a scene yet");
    await clearCalls(page);
    await page.getByTestId("plan-group-new").click();
    await expect(page.getByTestId("plan-unassigned")).toHaveCount(0);
    expect((await calls(page, "detect_scenes")).length).toBe(1);
  });
});

test.describe("P1-4 skipped and minor scenes", () => {
  test("skip / include from the row menu and key S; persists; Skipped tab; allDone", async ({ page }) => {
    await openPlan(page);
    await expect(page.getByTestId("plan-tab-skipped")).toContainText("0");
    await page.getByTestId("plan-menu-3").click();
    await page.getByTestId("plan-skip-3").click();
    await expect(row(page, 3)).toHaveAttribute("data-skipped", "true");
    await expect(page.getByTestId("plan-status-3")).toHaveText("Skipped, no edit copied");
    await expect(page.getByTestId("plan-tab-skipped")).toContainText("1");
    await expect(page.getByTestId("plan-tab-todo")).toContainText("2");
    await expect(page.getByTestId("plan-counts")).toContainText("1 skipped");
    await expect(page.getByTestId("plan-auto-3")).toHaveCount(0);
    await expect(row(page, 3)).toHaveClass(/opacity-60/);

    // Persisted: home and back.
    await page.getByTestId("home-button").click();
    await page.getByTestId("project-open-1").click();
    await expect(row(page, 3)).toHaveAttribute("data-skipped", "true");
    await page.getByTestId("plan-tab-skipped").click();
    await expect(page.locator('article[data-testid^="plan-scene-"]')).toHaveCount(1);
    await page.getByTestId("plan-tab-all").click();

    // Auto edit + apply the other two: skipped scene 3 counts as done.
    await page.getByTestId("plan-auto-remaining").click();
    await expect(row(page, 2)).toHaveAttribute("data-status", "auto");
    await expect(page.getByTestId("plan-auto-remaining")).toHaveCount(0);
    await page.getByTestId("plan-apply-all").click();
    await expect(page.getByTestId("plan-continue-export")).toBeVisible();
    await expect(page.getByTestId("step-edit-sub")).toHaveText("· All scenes applied");
    expect((await calls(page, "apply_all_edited_scenes")).length).toBe(1);

    // S includes it again (the focused row), and the plan is no longer done.
    await row(page, 3).click();
    await page.keyboard.press("s");
    await expect(row(page, 3)).toHaveAttribute("data-skipped", "false");
    await expect(page.getByTestId("plan-continue-export")).toHaveCount(0);
    expect((await calls(page, "set_scene_skipped")).map((c) => c.args)).toEqual([
      { sceneId: 3, skipped: true },
      { sceneId: 3, skipped: false },
    ]);
  });

  test("minor scenes are folded last under Small scenes", async ({ page }) => {
    await openPlan(page);
    // Scene 3 -> two keepers (a minor scene); its other keepers get regrouped by the plan.
    await page.getByTestId("step-cull").click();
    await inv(page, "set_scene_members", { id: 3, imageIds: [81, 85] });
    await page.getByTestId("step-edit").click();
    await expect(page.getByTestId("plan-view")).toBeVisible();
    await expect(page.getByTestId("plan-minor-toggle")).toContainText("Small scenes (1 scene, 2 photos)");
    await expect(row(page, 3)).toHaveCount(0);
    const order = await page.locator('article[data-testid^="plan-scene-"]').count();
    expect(order).toBe(3); // 1, 2 and the regrouped scene; the minor one is folded
    await page.getByTestId("plan-minor-toggle").click();
    await expect(row(page, 3)).toBeVisible();
    const ids = await page.locator('article[data-testid^="plan-scene-"]').evaluateAll((els) => els.map((e) => e.getAttribute("data-testid")));
    expect(ids.at(-1)).toBe("plan-scene-3");
    await expect(row(page, 3)).toHaveAttribute("data-skipped", "false");
    await shot(page, `${W}3-plan-minor`);
  });
});

test.describe("P1-5 MatchPanel apply is a batch", () => {
  test("Apply with options calls apply_scene_edit with matchOptions + excludeIds and undoes as one step", async ({ page }) => {
    await openPlan(page);
    await page.getByTestId("plan-auto-3").click();
    await expect(row(page, 3)).toHaveAttribute("data-status", "auto");
    await page.getByTestId("plan-menu-3").click();
    await page.getByTestId("plan-options-3").click();
    await expect(page.getByTestId("match-panel")).toBeVisible();
    await page.getByTestId("match-run").click();
    await expect(page.getByTestId("match-card-86")).toBeVisible();
    await page.getByTestId("match-include-86").uncheck();
    await page.getByTestId("match-strength").fill("50");
    await clearCalls(page);
    await page.getByTestId("match-apply").click();
    await expect(page.getByTestId("match-panel")).toHaveCount(0);
    await expect(row(page, 3)).toHaveAttribute("data-status", "applied");
    expect(await calls(page, "apply_scene_match")).toHaveLength(0);
    const [ap] = await calls(page, "apply_scene_edit");
    const o = ap.args.options as { excludeIds: number[]; matchOptions: { strength: number } };
    expect(ap.args.sceneId).toBe(3);
    expect(o.excludeIds).toContain(86);
    expect(o.matchOptions.strength).toBe(0.5);
    await expect(toast(page)).toContainText("Applied Scene 3 to");
    await clearCalls(page);
    await page.getByTestId("apply-undo-batch").click();
    await expect(row(page, 3)).toHaveAttribute("data-status", "auto");
    expect(await calls(page, "undo_adjustments")).toHaveLength(0);
    expect(await calls(page, "undo_edit_batch")).toHaveLength(1);
  });
});

test.describe("P2 cancel apply", () => {
  async function slowApplyAll(page: Page) {
    await openPlan(page);
    await page.getByTestId("plan-auto-remaining").click();
    await expect(row(page, 3)).toHaveAttribute("data-status", "auto");
    await page.evaluate(() => ((window as unknown as { __mockSceneDelay: number }).__mockSceneDelay = 700));
    await clearCalls(page);
    await page.getByTestId("plan-apply-all").click();
    await expect(page.getByTestId("plan-applying")).toBeVisible();
    // After the first scene (16 targets) progress moves past 1.
    await expect(page.getByTestId("plan-applying")).toContainText(/Applying… (1\d|[2-9]\d)\//);
  }

  test("x on the Applying pill stops after the finished scenes and offers Undo", async ({ page }) => {
    await slowApplyAll(page);
    await page.getByTestId("plan-apply-cancel").click();
    await expect(toast(page)).toContainText(/Stopped after [12] scene/);
    expect(await calls(page, "cancel_scene_apply")).toHaveLength(1);
    await expect(row(page, 1)).toHaveAttribute("data-status", "applied");
    await expect(row(page, 3)).toHaveAttribute("data-status", "auto");
    await expect(page.getByTestId("plan-applying")).toHaveCount(0);
    await clearCalls(page);
    await page.getByTestId("apply-undo-batch").click();
    await expect(row(page, 1)).toHaveAttribute("data-status", "auto");
    expect(await calls(page, "undo_edit_batch")).toHaveLength(1);
  });

  test("Esc in the Plan cancels the apply", async ({ page }) => {
    await slowApplyAll(page);
    await page.keyboard.press("Escape");
    await expect(toast(page)).toContainText("Stopped after");
    expect(await calls(page, "cancel_scene_apply")).toHaveLength(1);
    await expect(page.getByTestId("plan-view")).toBeVisible();
  });
});

test.describe("P2 keepers filter and XMP failures", () => {
  test("Edit and Export grids query keepersOnly and count over keepers", async ({ page }) => {
    await openPlan(page);
    await clearCalls(page);
    await page.getByTestId("plan-menu-1").click();
    await page.getByTestId("plan-show-1").click();
    await expect(page.getByTestId("grid-toolbar")).toBeVisible();
    await expect(page.getByTestId("selection-count")).toContainText("16 of 43 keepers");
    const ids = await calls(page, "list_image_ids");
    expect(ids.length).toBeGreaterThan(0);
    for (const c of ids) expect((c.args.query as { keepersOnly?: boolean }).keepersOnly).toBe(true);
    const fc = await calls(page, "get_filter_counts");
    expect(fc.at(-1)!.args).toMatchObject({ folderId: null, projectId: 1, keepersOnly: true });
    // Develop's filmstrip header: "16 of 43 keepers".
    await page.getByTestId("cell-1").dblclick();
    await expect(page.getByTestId("filter-summary-text")).toContainText("16 of 43 keepers");
    // The Cull step is unchanged: whole project.
    await page.getByTestId("step-cull").click();
    await clearCalls(page);
    await expect(page.getByTestId("selection-count")).toContainText("101 photos");
    expect((await calls(page, "get_filter_counts")).at(-1)?.args.keepersOnly ?? null).toBeNull();
  });

  test("XMP failure popover uses list_xmp_failures and Show lists the failed photos", async ({ page }) => {
    await openApp(page, 200, "&errors=1&autosync=1");
    await clearCalls(page);
    await page.getByTestId("xmp-status-button").click();
    await expect.poll(async () => (await calls(page, "list_xmp_failures")).length).toBeGreaterThan(0);
    // No paging scan of the catalog (list_images with limit 1000).
    expect((await calls(page, "list_images")).filter((c) => (c.args.query as { limit: number }).limit === 1000)).toHaveLength(0);
    const failed = Number(await page.getByTestId("xmp-status").getAttribute("data-failed"));
    await expect(page.getByTestId("xmp-failures").locator("li")).toHaveCount(failed);
    await page.getByTestId("xmp-failure-show-5").click();
    await expect(page.getByTestId("id-filter-bar")).toContainText(`${failed} photos with sidecar errors`);
    await expect(page.getByTestId("cell-5")).toBeVisible();
    await expect(page.getByTestId("cell-6")).toHaveCount(0);
    await page.getByTestId("id-filter-clear").click();
    await expect(page.getByTestId("id-filter-bar")).toHaveCount(0);
    await expect(page.getByTestId("cell-6")).toBeVisible();
  });
});

test("screenshots: the plan with skipped, minor, new keepers and needs-a-look at 1280x800", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await openPlan(page);
  await page.getByTestId("plan-auto-remaining").click();
  await expect(row(page, 3)).toHaveAttribute("data-status", "auto");
  await page.getByTestId("plan-apply-1").click();
  await expect(row(page, 1)).toHaveAttribute("data-status", "applied");
  await page.getByTestId("plan-apply-3").click();
  await expect(row(page, 3)).toHaveAttribute("data-status", "applied");
  // Skip scene 2, make scene 3 gain a keeper (pick 82 in Cull), and shrink a scene to a minor one.
  await page.getByTestId("plan-menu-2").click();
  await page.getByTestId("plan-skip-2").click();
  await page.getByTestId("step-cull").click();
  await pickInCull(page, 82);
  await inv(page, "create_scene", { imageIds: [11, 15] });
  await page.getByTestId("step-edit").click();
  await expect(page.getByTestId("plan-view")).toBeVisible();
  await expect(page.getByTestId("plan-minor-toggle")).toBeVisible();
  await page.getByTestId("plan-minor-toggle").click();
  await expect(page.getByTestId("plan-status-3")).toContainText("new keeper");
  await shot(page, `${W}4-plan-1280`);
});
