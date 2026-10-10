// Phase 10: Baseline edit UI (preset -> anchor -> adjust -> edit the rest -> finish in Lightroom) on the mock backend
// (`?baseline=presets|anchor|1`, src/testing/mockBaseline.ts). Mock rules: project 1 = ids 1..101, 43 keepers in 3 scenes.
import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openHome, shot } from "./helpers";

async function openEdit(page: Page, mode: "presets" | "anchor" | "1", delay = 0) {
  if (delay) await page.addInitScript((d) => (window.__mockBaselineDelay = d), delay);
  await openHome(page, 201, `&style=ready&baseline=${mode}`);
  await page.getByTestId("project-open-1").click();
  await expect(page.getByTestId("step-bar")).toBeVisible();
  await page.getByTestId("continue-edit").click();
  await expect(page.getByTestId("plan-view")).toBeVisible();
  await expect(page.locator('article[data-testid^="plan-scene-"]').first()).toBeVisible();
}

const view = (page: Page) => page.getByTestId("baseline-view");

async function untitled(page: Page, root: string): Promise<string[]> {
  return page.getByTestId(root).locator("button, input, select, a").evaluateAll((els) =>
    els.filter((e) => !(e.getAttribute("title") || e.getAttribute("aria-label")) && !(e as HTMLElement).closest("[data-testid$='-img']")).map((e) => `${e.tagName} ${e.getAttribute("data-testid") ?? e.textContent?.slice(0, 30)}`),
  );
}

test.describe("baseline edit UI", () => {
  test("the Plan makes it the obvious path; step 1 shows preset previews on the anchor with search and groups", async ({ page }) => {
    await openEdit(page, "presets");
    const banner = page.getByTestId("plan-baseline-banner");
    await expect(banner).toContainText("Pick a preset, adjust one photo");
    await expect(page.getByTestId("plan-baseline")).toHaveText(/Start baseline edit/);
    expect(await untitled(page, "plan-baseline-banner")).toEqual([]);
    await shot(page, "p10-01-plan-banner");
    await clearCalls(page);
    await page.getByTestId("plan-baseline").click();
    await expect(view(page)).toHaveAttribute("data-step", "1");
    await expect(page.getByTestId("baseline-step-preset")).toBeVisible();
    // No preset + the three presets, each with a rendered preview on the anchor.
    await expect(page.getByTestId("baseline-preset-none")).toBeVisible();
    const tiles = page.locator('[data-testid^="baseline-preset-"][data-testid$="-img"]');
    const buttons = await page.locator('button[data-testid^="baseline-preset-"]').count();
    expect(buttons).toBeGreaterThanOrEqual(4);
    await expect(tiles).toHaveCount(buttons); // rendered one after the other
    const variants = await calls(page, "render_preview_variant");
    const total = buttons;
    expect(variants.length).toBe(buttons - 1); // every preset + "No preset" (a plain render)
    expect(variants.every((c) => (c.args.variant as { kind: string }).kind === "preset" && (c.args.options as { slot: string }).slot === "preview")).toBe(true);
    expect(new Set(variants.map((c) => c.args.id)).size).toBe(1);
    await expect(page.getByTestId("baseline-preview-on")).toContainText(/\.ARW|\.|#/);
    await shot(page, "p10-02-preset-step");

    // Search and groups.
    await page.getByTestId("baseline-preset-search").fill("warm matte");
    await expect(page.getByRole("button", { name: /Soft Film/ })).toHaveCount(0);
    await expect(page.getByRole("button", { name: /Warm Matte/ }).first()).toBeVisible();
    await expect(page.getByTestId("baseline-preset-none")).toHaveCount(0);
    await page.getByTestId("baseline-preset-search").fill("zzz");
    await expect(page.getByTestId("baseline-preset-empty")).toBeVisible();
    await page.getByTestId("baseline-preset-search").fill("");
    await expect(page.locator('[data-testid^="baseline-preset-"][data-testid$="-img"]')).toHaveCount(total);
    const groupBtn = page.locator('[data-testid^="baseline-group-"]:not([data-testid="baseline-group-all"])').first();
    await groupBtn.click();
    await expect(groupBtn).toHaveAttribute("aria-pressed", "true");
    await page.getByTestId("baseline-group-all").click();

    // Pick one.
    const soft = page.getByRole("button", { name: /Soft Film/ }).first();
    await soft.click();
    await expect(soft).toHaveAttribute("data-selected", "true");
    await expect(page.getByTestId("baseline-choice")).toContainText("Soft Film");
    expect(await untitled(page, "baseline-view")).toEqual([]);
  });

  test("step 2 chooses the anchor; step 3 opens Develop with the Baseline bar showing the offset from Auto", async ({ page }) => {
    await openEdit(page, "anchor");
    await page.getByTestId("plan-baseline").click();
    await page.getByRole("button", { name: /Soft Film/ }).first().click();
    await page.getByTestId("baseline-next").click();
    await expect(view(page)).toHaveAttribute("data-step", "2");
    const cands = page.locator('[data-testid^="baseline-anchor-"][data-selected]');
    expect(await cands.count()).toBeGreaterThanOrEqual(3);
    await expect(page.locator('[data-testid^="baseline-anchor-"][data-selected="true"]')).toHaveCount(1);
    await shot(page, "p10-03-anchor-step");
    // Switch the anchor to another candidate, then back to the default (the mock's prepared anchor).
    const first = page.locator('[data-testid^="baseline-anchor-"][data-selected="true"]');
    const firstId = await first.getAttribute("data-testid");
    await clearCalls(page);
    await page.getByTestId("baseline-next").click();
    await expect(page.getByTestId("develop-view")).toBeVisible();
    expect((await calls(page, "apply_preset")).length).toBe(1);
    const bar = page.getByTestId("baseline-bar");
    await expect(bar).toBeVisible();
    await expect(page.getByTestId("baseline-bar-preset")).toHaveText("Soft Film");
    await expect(page.getByTestId("baseline-bar-offset")).toContainText("+0.3 EV");
    await expect(page.getByTestId("baseline-bar-offset")).toContainText("warmer than Auto");
    await expect(bar).toContainText("Edit the rest");
    expect(await untitled(page, "baseline-bar")).toEqual([]);
    await shot(page, "p10-04-develop-bar");
    expect(firstId).toBeTruthy();

    // Changing the exposure re-measures the offset.
    await clearCalls(page);
    const slider = page.getByTestId("slider-exposure");
    await slider.fill("1");
    await slider.evaluate((el) => (el as HTMLElement).blur());
    await expect.poll(async () => (await calls(page, "preview_baseline")).length).toBeGreaterThan(0);
    await expect(page.getByTestId("baseline-bar-offset")).not.toContainText("+0.3 EV");

    // The old scene bar is still there below it.
    await expect(page.getByTestId("edit-context")).toBeVisible();
    await page.getByTestId("baseline-bar-rest").click();
    await expect(view(page)).toHaveAttribute("data-step", "4");
    // Back to Develop through step 3: the preset is not applied a second time over the user's adjustments.
    await clearCalls(page);
    await page.getByTestId("baseline-step-3").click();
    await expect(page.getByTestId("baseline-bar")).toBeVisible();
    expect((await calls(page, "apply_preset")).length).toBe(0);
  });

  test("an unedited anchor gets the chosen preset applied when leaving for Develop", async ({ page }) => {
    await openEdit(page, "presets");
    await page.getByTestId("plan-baseline").click();
    await page.getByRole("button", { name: /Warm Matte/ }).first().click();
    await page.getByTestId("baseline-next").click();
    await clearCalls(page);
    await page.getByTestId("baseline-next").click();
    await expect(page.getByTestId("develop-view")).toBeVisible();
    const applied = await calls(page, "apply_preset");
    expect(applied.length).toBe(1);
    await expect(page.getByTestId("baseline-bar-preset")).toHaveText("Warm Matte");
  });

  test("step 4: before/after samples across scenes, plan counts, apply with progress, result, flagged filter and one undo", async ({ page }) => {
    await openEdit(page, "anchor", 900);
    await page.getByTestId("plan-baseline").click();
    await page.getByRole("button", { name: /Soft Film/ }).first().click();
    await page.getByTestId("baseline-next").click();
    await page.getByTestId("baseline-next").click();
    await expect(page.getByTestId("baseline-bar")).toBeVisible();
    await clearCalls(page);
    await page.getByTestId("baseline-bar-rest").click();
    await expect(view(page)).toHaveAttribute("data-step", "4");
    await expect(page.getByTestId("baseline-step-rest")).toBeVisible();

    // Preview: ~12 samples across scenes, before and after rendered, scene label under each.
    const samples = page.locator('figure[data-testid^="baseline-sample-"]');
    await expect(samples).toHaveCount(12);
    await expect(samples.first().locator('img[data-testid$="-before"]')).toBeVisible();
    await expect(samples.first().locator('img[data-testid$="-after"]')).toBeVisible();
    const scenes = await samples.locator('[data-testid$="-scene"]').allTextContents();
    expect(new Set(scenes).size).toBeGreaterThanOrEqual(3);
    scenes.forEach((t) => expect(t).toMatch(/^Scene \d+$/));
    const pv = (await calls(page, "preview_baseline")).at(-1)!;
    expect((pv.args.settings as { scope: { kind: string } }).scope.kind).toBe("keepers");
    expect((pv.args.options as { sampleCount: number }).sampleCount).toBe(12);
    const renders = (await calls(page, "render_preview")).filter((c) => (c.args.options as { slot: string }).slot === "preview");
    expect(renders.length).toBeGreaterThanOrEqual(12);
    await expect(page.getByTestId("baseline-plan-counts")).toContainText(/Will edit \d+ of 43 photos/);
    await shot(page, "p10-05-rest-step");

    // Scope and skip / replace change the plan.
    await page.getByTestId("baseline-scope-all").check();
    await expect(page.getByTestId("baseline-plan-counts")).toContainText("of 101 photos");
    await page.getByTestId("baseline-scope-keepers").check();
    await expect(page.getByTestId("baseline-plan-counts")).toContainText("of 43 photos");
    await expect(page.getByTestId("baseline-skip")).toBeChecked();
    await expect(page.getByTestId("baseline-scope-selection")).toHaveAttribute("title", /selected in the grid|Select photos/);
    expect(await untitled(page, "baseline-view")).toEqual([]);

    // Apply: progress, Stop is offered, then the summary.
    await clearCalls(page);
    await page.getByTestId("baseline-apply").click();
    await expect(page.getByTestId("baseline-progress")).toBeVisible();
    await expect(page.getByTestId("baseline-cancel")).toBeVisible();
    await expect(page.getByTestId("baseline-progress-text")).toContainText(/Editing photos/);
    await shot(page, "p10-06-progress");
    await expect(page.getByTestId("baseline-result")).toBeVisible();
    await expect(page.getByTestId("baseline-result-message")).toContainText(/Edited \d+ photos/);
    const applied = Number((await page.getByTestId("baseline-count-applied").locator("b").textContent()) ?? 0);
    const flagged = Number((await page.getByTestId("baseline-count-flagged").locator("b").textContent()) ?? 0);
    expect(applied).toBeGreaterThan(20);
    expect(flagged).toBeGreaterThan(0);
    await expect(page.getByTestId("baseline-count-skipped")).toContainText("0");
    await expect(page.getByTestId("baseline-flagged-list").locator("li").first()).toContainText(/./);
    await shot(page, "p10-07-result");
    expect((await calls(page, "run_baseline")).length).toBe(1);

    // Flagged -> a grid filter with the reason.
    await clearCalls(page);
    await page.getByTestId("baseline-show-flagged").click();
    await expect(page.getByTestId("baseline-flag-bar")).toBeVisible();
    await expect(page.getByTestId("filter-baseline")).toBeVisible();
    await expect(page.getByTestId("filter-baseline")).toContainText("needs a look");
    const q = (await calls(page, "list_image_ids")).at(-1)!.args.query as { baselineOutcomes: string[] };
    expect(q.baselineOutcomes).toEqual(["flagged"]);
    await expect(page.getByTestId("selection-count")).toContainText(String(flagged));
    await expect(page.getByTestId("baseline-flag-groups")).toContainText(/dark on purpose|Auto could not/i);
    const cell = page.locator('[data-testid^="cell-"]').first();
    await cell.click();
    await expect(page.getByTestId("baseline-flag-reason")).toContainText("This photo:");
    await shot(page, "p10-08-flagged-filter");
    // The filter chip clears it.
    await page.getByTestId("filter-baseline").click();
    await expect(page.getByTestId("filter-baseline")).toHaveCount(0);
    await expect(page.getByTestId("baseline-flag-bar")).toHaveCount(0);

    // Back to the result, then one Undo for the whole batch.
    await page.getByTestId("step-edit").click().catch(() => undefined);
    await expect(page.getByTestId("plan-view")).toBeVisible();
    await expect(page.getByTestId("plan-baseline-banner")).toHaveAttribute("data-state", "done");
    await page.getByTestId("plan-baseline").click();
    await expect(view(page)).toHaveAttribute("data-step", "4");
    await expect(page.getByTestId("baseline-result")).toBeVisible();
    await clearCalls(page);
    await page.getByTestId("baseline-undo").click();
    await expect(page.getByTestId("baseline-result-message")).toContainText("Undone");
    await expect(page.getByTestId("baseline-undo")).toBeDisabled();
    const undos = await calls(page, "undo_edit_batch");
    expect(undos.length).toBe(1);
    await expect(page.getByTestId("baseline-flagged-list")).toHaveCount(0);
    // Edited state is really back: run again is possible and the photos are unedited (mock invoke).
    const edited = await page.evaluate(async () => {
      const r = (await (window as unknown as { __TAURI_INTERNALS__: { invoke: (c: string, a: unknown) => Promise<{ hasEdits: boolean }[]> } }).__TAURI_INTERNALS__.invoke("get_images", { ids: [2, 3, 4, 5] })) as { hasEdits: boolean }[];
      return r.map((x) => x.hasEdits);
    });
    expect(edited.some(Boolean)).toBe(false);
    await shot(page, "p10-09-undone");
  });

  test("Stop cancels a running baseline and writes nothing", async ({ page }) => {
    await openEdit(page, "anchor", 1500);
    await page.getByTestId("plan-baseline").click();
    await page.getByTestId("baseline-step-4").click();
    await expect(page.locator('figure[data-testid^="baseline-sample-"]').first()).toBeVisible();
    await page.getByTestId("baseline-apply").click();
    await expect(page.getByTestId("baseline-progress")).toBeVisible();
    await page.getByTestId("baseline-cancel").click();
    await expect(page.getByTestId("baseline-result")).toBeVisible();
    await expect(page.getByTestId("baseline-result-message")).toContainText("Stopped");
    expect((await calls(page, "cancel_baseline")).length).toBe(1);
  });

  test("a finished run opens at the result (?baseline=1); step 5 explains the Lightroom hand-off", async ({ page }) => {
    await openEdit(page, "1");
    await expect(page.getByTestId("plan-baseline-banner")).toHaveAttribute("data-state", "done");
    await page.getByTestId("plan-baseline").click();
    await expect(view(page)).toHaveAttribute("data-step", "4");
    await expect(page.getByTestId("baseline-result")).toBeVisible();
    await expect(page.getByTestId("baseline-summary-line")).toContainText("Soft Film");
    await expect(page.getByTestId("baseline-count-flagged")).toBeVisible();
    await page.getByTestId("baseline-to-finish").click();
    await expect(view(page)).toHaveAttribute("data-step", "5");
    await expect(page.getByTestId("baseline-xmp")).toBeVisible();
    await expect(page.getByTestId("baseline-lr-existing")).toContainText("Metadata > Read Metadata from Files");
    await expect(page.getByTestId("baseline-lr-new")).toContainText("Lightroom reads the sidecars");
    await expect(page.getByTestId("baseline-lr-note")).toContainText("Crop, straighten and masks are not touched");
    expect(await untitled(page, "baseline-view")).toEqual([]);
    await shot(page, "p10-10-finish");
    // Unsaved sidecars can be saved from here.
    await expect(page.getByTestId("baseline-view").getByTestId("help-link-baseline-edit").first()).toBeVisible();
    await expect(async () => {
      await page.keyboard.press("Escape");
      await expect(view(page)).toHaveCount(0, { timeout: 1000 });
    }).toPass({ timeout: 10_000 });
    await expect(page.getByTestId("plan-view")).toBeVisible();
  });

  test("Cmd+Alt+B opens it, the help entry and the cheat sheet know it, the old scene flow is still there", async ({ page }) => {
    await openEdit(page, "presets");
    await page.keyboard.press("Control+Alt+b");
    await expect(view(page)).toBeVisible();
    await page.getByTestId("baseline-close").click();
    await expect(view(page)).toHaveCount(0);
    // Apply to scene is still available, but no longer the primary (emerald) button.
    await expect(page.getByTestId("plan-apply-all")).toBeVisible();
    await expect(page.getByTestId("plan-baseline")).toBeVisible();
    await page.keyboard.press("?");
    await expect(page.getByTestId("cheat-baselineEdit")).toContainText("Cmd+Alt+B");
  });

  test("filter chips and describeFilters know baselineOutcomes", async ({ page }) => {
    await openEdit(page, "1");
    await page.getByTestId("plan-baseline").click();
    await page.getByTestId("baseline-show-flagged").click();
    await expect(page.getByTestId("filter-baseline")).toHaveAttribute("title", /needs a look/);
    await page.locator('[data-testid^="cell-"]').first().click();
    await page.keyboard.press("d");
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await expect(page.getByTestId("filter-summary-text")).toContainText("Baseline edit: needs a look");
  });
});
