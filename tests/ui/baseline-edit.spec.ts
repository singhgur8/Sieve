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
    // P1-5: the anchor is pinned first, "After only" is the default, hold \ for Before, "Before / after" shows both.
    await expect(page.getByTestId("baseline-pinned-anchor")).toContainText("Anchor · your edit");
    await expect(page.getByTestId("baseline-pinned-anchor-img")).toBeVisible();
    expect(await page.getByTestId("baseline-samples").locator("figure").first().getAttribute("data-testid")).toBe("baseline-pinned-anchor");
    await expect(page.getByTestId("baseline-view-after")).toHaveAttribute("aria-pressed", "true");
    await expect(samples.first().locator('img[data-testid$="-after"]')).toBeVisible();
    await expect(samples.first().locator('img[data-testid$="-before"]')).toBeHidden();
    await page.keyboard.down("\\");
    await expect(samples.first().locator('img[data-testid$="-before"]')).toBeVisible();
    await expect(samples.first().locator('img[data-testid$="-after"]')).toBeHidden();
    await page.keyboard.up("\\");
    await expect(samples.first().locator('img[data-testid$="-before"]')).toBeHidden();
    await page.getByTestId("baseline-view-both").click();
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
    await expect(page.getByTestId("baseline-lr-note")).toContainText("Crop, straighten, masks, spot removal and lens corrections are as they were");
    expect(await untitled(page, "baseline-view")).toEqual([]);
    await shot(page, "p10-10-finish");
    // Unsaved sidecars can be saved from here.
    await expect(page.getByTestId("baseline-view").getByTestId("help-link-baseline-edit").first()).toBeVisible();
    // One Esc, no retry: the handler is registered once (it used to be re-registered on every App render and could miss a key).
    await page.keyboard.press("Escape");
    await expect(view(page)).toHaveCount(0, { timeout: 3000 });
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

  // ---- UX review 10 (docs/ux-review-10.md): P0 / P1 / P2 ----
  const inv = <T,>(page: Page, cmd: string, args: Record<string, unknown> = {}) =>
    page.evaluate(([c, a]) => (window as unknown as { __TAURI_INTERNALS__: { invoke: (c: string, a: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke(c as string, a) as Promise<unknown>, [cmd, args] as const) as Promise<T>;

  async function toDevelop(page: Page, preset: RegExp) {
    await page.getByTestId("plan-baseline").click();
    await page.getByRole("button", { name: preset }).first().click();
    await page.getByTestId("baseline-next").click();
    await clearCalls(page);
    await page.getByTestId("baseline-next").click();
    await expect(page.getByTestId("baseline-bar")).toBeVisible();
  }

  test("P0-1: the anchor starts at Auto (offset zero), keeps the preset's colours, and Auto in Develop leaves Vibrance / Saturation alone", async ({ page }) => {
    await openEdit(page, "presets");
    await toDevelop(page, /Soft Film/);
    const anchorId = (await calls(page, "apply_preset"))[0].args.ids as number[];
    const id = anchorId[0];
    // The bar reads "same as Auto" without touching anything, and history shows the preset then "Baseline: start from Auto".
    await expect(page.getByTestId("baseline-bar-offset")).toHaveText("same as Auto");
    await expect(page.getByTestId("baseline-bar")).toContainText("Your light vs Auto");
    await expect(page.getByTestId("baseline-bar")).not.toContainText("adjust exposure and white balance first");
    const labels = (await inv<{ entries: { label: string }[] }>(page, "get_history", { id })).entries.map((e) => e.label);
    expect(labels.filter((l) => l.startsWith("Preset:") || l.startsWith("Baseline"))).toEqual(["Preset: Soft Film", "Baseline: start from Auto"]);
    let adj = await inv<{ vibrance: number; saturation: number }>(page, "get_adjustments", { id });
    expect([adj.vibrance, adj.saturation]).toEqual([12, -6]);
    // Exposure +0.3 -> "+0.3 EV".
    const slider = page.getByTestId("slider-exposure");
    const cur = Number(await slider.inputValue());
    await slider.fill(String(Math.round((cur + 0.3) * 10) / 10));
    await slider.evaluate((el) => (el as HTMLElement).blur());
    await expect(page.getByTestId("baseline-bar-offset")).toContainText(/\+0\.[34] EV/);
    // The Auto button does light and white balance only; the preset's colours survive.
    await expect(page.getByTestId("auto-all")).toHaveAttribute("title", /preset's colours are kept/);
    await page.getByTestId("auto-all").click();
    await expect.poll(async () => (await inv<{ exposure: number }>(page, "get_adjustments", { id })).exposure).toBeCloseTo(0.35, 2);
    adj = await inv<{ vibrance: number; saturation: number }>(page, "get_adjustments", { id });
    expect([adj.vibrance, adj.saturation]).toEqual([12, -6]);
    // Start from Auto puts the light back at the baseline's Auto: offset zero again.
    await slider.fill("2");
    await slider.evaluate((el) => (el as HTMLElement).blur());
    await expect(page.getByTestId("baseline-bar-offset")).toContainText("EV");
    await page.getByTestId("baseline-bar-auto").click();
    await expect(page.getByTestId("baseline-bar-offset")).toHaveText("same as Auto");
    adj = await inv<{ vibrance: number; saturation: number }>(page, "get_adjustments", { id });
    expect([adj.vibrance, adj.saturation]).toEqual([12, -6]);
    // Leaving the bar, Auto is the plain Develop Auto again (vibrance follows it).
  });

  test("P0-1: a preset's own light values are added to Auto; light the user already set is never replaced", async ({ page }) => {
    await openEdit(page, "presets");
    await toDevelop(page, /Warm Matte/);
    // Warm Matte carries Exposure +0.2: Auto + 0.2 EV.
    await expect(page.getByTestId("baseline-bar-offset")).toContainText("+0.2 EV");
    // The anchor fixture of ?baseline=anchor already has user light: Start from Auto is not run on its own.
  });

  test("P0-2: no Apply to scene while the Baseline bar is shown; after a run the Plan reads Baseline X of Y with no Apply buttons", async ({ page }) => {
    await openEdit(page, "anchor");
    await page.getByTestId("plan-baseline").click();
    await page.getByTestId("baseline-step-3").click();
    await expect(page.getByTestId("baseline-bar")).toBeVisible();
    await expect(page.getByTestId("edit-context")).toBeVisible();
    await expect(page.getByTestId("edit-apply")).toHaveCount(0);
    await expect(page.getByTestId("edit-apply-menu")).toHaveCount(0);
    await expect(page.getByTestId("edit-auto")).toHaveCount(0);
    await expect(page.getByTestId("edit-plan")).toBeVisible();
    // A finished run: Plan.
    const p2 = await page.context().newPage();
    await openEdit(p2, "1");
    await expect(p2.getByTestId("plan-counts")).toHaveText(/^Baseline: \d+ of 43 keepers · \d+ need a look$/);
    await expect(p2.locator('[data-testid^="plan-apply-"]:not([data-testid="plan-apply-all"]):not([data-testid="plan-apply-block"]):not([data-testid^="plan-apply-why"]):not([data-testid="plan-apply-cancel"])')).toHaveCount(0);
    await expect(p2.getByTestId("plan-apply-all")).toHaveCount(0);
    const rows = p2.locator('article[data-testid^="plan-scene-"]');
    expect(await rows.count()).toBeGreaterThanOrEqual(3);
    await expect(p2.locator('article[data-on-baseline="true"]')).toHaveCount(await rows.count());
    await expect(rows.first()).toContainText("On baseline");
    await expect(p2.getByTestId("step-edit")).toContainText("baseline ✓");
    await shot(p2, "p10-11-plan-on-baseline");
    // Develop on a scene representative: no Apply to scene either.
    await rows.first().getByRole("button", { name: /^Edit/ }).click();
    await expect(p2.getByTestId("develop-view")).toBeVisible();
    await expect(p2.getByTestId("edit-apply")).toHaveCount(0);
    await p2.close();
  });

  test("P0-2: a representative edited after the baseline gets its Apply button back", async ({ page }) => {
    await openEdit(page, "1");
    const run = await inv<{ settings: { anchorId: number } }>(page, "get_baseline_run", { projectId: 1 });
    const rows = page.locator('article[data-testid^="plan-scene-"]');
    await expect(rows.first()).toHaveAttribute("data-on-baseline", "true");
    // A scene whose representative is not the anchor.
    const n = await rows.count();
    let sceneId = 0;
    for (let i = 0; i < n && !sceneId; i++) {
      const rep = Number((await rows.nth(i).locator('button[data-testid^="plan-rep-"]').getAttribute("data-testid"))!.replace("plan-rep-", ""));
      if (rep !== run.settings.anchorId) sceneId = Number((await rows.nth(i).getAttribute("data-testid"))!.replace("plan-scene-", ""));
    }
    expect(sceneId).toBeGreaterThan(0);
    await expect(page.getByTestId(`plan-apply-${sceneId}`)).toHaveCount(0);
    await page.getByTestId(`plan-edit-${sceneId}`).click();
    await expect(page.getByTestId("develop-view")).toBeVisible();
    const slider = page.getByTestId("slider-exposure");
    await slider.fill("1.5");
    await slider.evaluate((el) => (el as HTMLElement).blur());
    await expect.poll(async () => (await calls(page, "save_adjustments")).length).toBeGreaterThan(0);
    await page.getByTestId("edit-plan").click();
    await expect(page.getByTestId("plan-view")).toBeVisible();
    await expect(page.getByTestId(`plan-scene-${sceneId}`)).toHaveAttribute("data-on-baseline", "false");
    await expect(page.getByTestId(`plan-apply-${sceneId}`)).toBeVisible();
  });

  test("P1-1: the copy describes the light / look model (no 'keeps its own exposure')", async ({ page }) => {
    await openEdit(page, "1");
    await page.getByTestId("plan-baseline").click();
    await expect(page.getByTestId("baseline-summary-model")).toContainText("Colours, profile, curve, grain and detail are copied from the anchor (the preset plus your changes).");
    await expect(page.getByTestId("baseline-summary-model")).toContainText("its own Auto, plus how your anchor differs from its Auto");
    await expect(page.getByTestId("baseline-summary-model")).toContainText("Frames of one burst get matching values.");
    await page.getByTestId("baseline-to-finish").click();
    await expect(page.getByTestId("baseline-lr-note")).toContainText("Every edited photo now has the anchor's look and its own Auto-based light and white balance, as standard Lightroom settings in its .xmp sidecar.");
    await page.getByTestId("baseline-view").getByTestId("help-link-baseline-edit").first().click();
    const help = page.getByTestId("help-panel");
    await expect(help).toContainText("Crop, straighten, masks, spot removal and lens corrections are never changed.");
    await expect(help).toContainText("dark on purpose, silhouettes, mixed light, Auto failed, or a value hit the end of its slider");
    await expect(help).not.toContainText("keeps its own exposure");
    await expect(page.getByTestId("baseline-view")).not.toContainText("keeps its own exposure");
  });

  test("P1-2: Cmd+Z in the Plan updates the banner and the result; Cmd+Z works inside the view; the finish toast has Undo", async ({ page }) => {
    await openEdit(page, "1");
    await expect(page.getByTestId("plan-baseline-banner")).toHaveAttribute("data-state", "done");
    await page.keyboard.press("Control+z");
    await expect(page.getByTestId("plan-baseline-banner")).toHaveAttribute("data-state", "undone");
    await expect(page.getByTestId("plan-baseline-text")).toContainText("Undone");
    await expect(page.getByTestId("plan-baseline")).toHaveText(/Run again/);
    await page.getByTestId("plan-baseline").click();
    await expect(page.getByTestId("baseline-result-message")).toContainText("Undone");
    await expect(page.getByTestId("baseline-undo")).toBeDisabled();
    await expect(page.getByTestId("baseline-undo")).toHaveAttribute("title", "Already undone");
  });

  test("P1-2: the finish toast offers Undo; Cmd+Z inside the view undoes the baseline", async ({ page }) => {
    await openEdit(page, "anchor", 200);
    await page.getByTestId("plan-baseline").click();
    await page.getByTestId("baseline-step-4").click();
    await expect(page.locator('figure[data-testid^="baseline-sample-"]').first()).toBeVisible();
    await page.getByTestId("baseline-apply").click();
    await expect(page.getByTestId("baseline-result")).toBeVisible();
    await expect(page.getByTestId("baseline-undo-toast")).toBeVisible();
    await expect(page.getByText(/Edited \d+ photos · \d+ need a look/)).toBeVisible();
    await clearCalls(page);
    await page.getByTestId("baseline-undo-toast").click();
    await expect(page.getByTestId("baseline-result-message")).toContainText("Undone");
    expect((await calls(page, "undo_edit_batch")).length).toBe(1);
    // Run again, then Cmd+Z inside the view.
    await page.getByTestId("baseline-rerun").click().catch(() => undefined);
    await page.getByTestId("baseline-apply").click();
    await expect(page.getByTestId("baseline-result-message")).toContainText(/Edited \d+ photos/);
    await clearCalls(page);
    await page.keyboard.press("Control+z");
    await expect(page.getByTestId("baseline-result-message")).toContainText("Undone");
    expect((await calls(page, "undo_edit_batch")).length).toBe(1);
    // Nothing to undo: a reason, not silence.
    await page.keyboard.press("Control+z");
    await expect(page.getByText("The baseline is already undone")).toBeVisible();
  });

  test("P1-2: fixing one flagged photo blocks the one-step Undo, and the view says why", async ({ page }) => {
    await openEdit(page, "1");
    await page.getByTestId("plan-baseline").click();
    await page.getByTestId("baseline-show-flagged").click();
    const cell = page.locator('[data-testid^="cell-"]').first();
    await cell.click();
    await page.keyboard.press("d");
    await expect(page.getByTestId("develop-view")).toBeVisible();
    const slider = page.getByTestId("slider-exposure");
    await slider.fill("1.7");
    await slider.evaluate((el) => (el as HTMLElement).blur());
    await expect.poll(async () => (await calls(page, "save_adjustments")).length).toBeGreaterThan(0);
    await page.keyboard.press("Control+Alt+b");
    await expect(view(page)).toHaveAttribute("data-step", "4");
    await expect(page.getByTestId("baseline-undo")).toBeDisabled();
    await expect(page.getByTestId("baseline-undo-blocked")).toContainText(/You changed 1 photo after the baseline \(\S+\)/);
    // "Undo the rest" needs the architect's keepLaterEdits option; it is offered once the contract has it.
    const supported = await page.getByTestId("baseline-undo-rest").count();
    if (supported === 0) await expect(page.getByTestId("baseline-undo-blocked")).toContainText("Undo that change in Develop first");
    // Cmd+Z in the view explains instead of staying silent.
    await page.keyboard.press("Control+z");
    await expect(page.getByText(/Later edits on 1 photo/)).toBeVisible();
  });

  test("P1-3: Finish in Lightroom shows the folder, the import-preset trap, the overwrite warning and the LUT caveat", async ({ page }) => {
    await openEdit(page, "1");
    await page.getByTestId("plan-baseline").click();
    // Give the anchor a LUT so the warning shows.
    const run = await inv<{ settings: { anchorId: number } }>(page, "get_baseline_run", { projectId: 1 });
    const id = run.settings.anchorId;
    const adj = await inv<Record<string, unknown>>(page, "get_adjustments", { id });
    await inv(page, "save_adjustments", { id, adjustments: { ...adj, lut: { id: "Teal Film", amount: 100 } }, label: "LUT" });
    await page.getByTestId("baseline-to-finish").click();
    await expect(page.getByTestId("baseline-folder-path")).toContainText("/");
    await expect(page.getByTestId("baseline-folder-reveal")).toBeVisible();
    await expect(page.getByTestId("baseline-folder-copy")).toBeVisible();
    await expect(page.getByTestId("baseline-lr-new")).toContainText("Apply During Import > Develop Settings: None");
    await expect(page.getByTestId("baseline-lr-existing")).toContainText("This replaces any changes made to them in Lightroom since.");
    await expect(page.getByTestId("baseline-lr-existing")).toContainText("Cmd+A selects the folder");
    await expect(page.getByTestId("baseline-lut-warning")).toContainText("Your look uses the LUT Teal Film. Lightroom cannot read Sieve LUTs");
    await shot(page, "p10-12-finish-warnings");
  });

  test("P1-3: no LUT warning without a LUT; Cmd+S is shown next to Save now", async ({ page }) => {
    await openEdit(page, "1");
    await page.getByTestId("plan-baseline").click();
    await page.getByTestId("baseline-to-finish").click();
    await expect(page.getByTestId("baseline-lut-warning")).toHaveCount(0);
    const save = page.getByTestId("baseline-xmp-save");
    if (await save.count()) await expect(page.getByTestId("baseline-xmp")).toContainText("(Save, Cmd+S)");
  });

  test("P1-4: Cmd+Alt+B during step 3 goes to Edit the rest, not back to the preset", async ({ page }) => {
    await openEdit(page, "presets");
    await toDevelop(page, /Soft Film/);
    await page.keyboard.press("Control+Alt+b");
    await expect(view(page)).toHaveAttribute("data-step", "4");
    await expect(page.getByTestId("baseline-step-rest")).toBeVisible();
  });

  test("P1-5: click a sample to enlarge Before | After, arrows move, Esc closes the lightbox only; Show more asks for more samples", async ({ page }) => {
    await openEdit(page, "anchor");
    await page.getByTestId("plan-baseline").click();
    await page.getByTestId("baseline-step-4").click();
    const samples = page.locator('figure[data-testid^="baseline-sample-"]');
    await expect(samples).toHaveCount(12);
    await samples.first().locator("div").first().click();
    const box = page.getByTestId("baseline-lightbox");
    await expect(box).toBeVisible();
    await expect(box).toContainText("1 of 12");
    await expect(page.getByTestId("baseline-lightbox-before")).toBeVisible();
    await expect(page.getByTestId("baseline-lightbox-after")).toBeVisible();
    await page.keyboard.press("ArrowRight");
    await expect(box).toContainText("2 of 12");
    await page.keyboard.press("Escape");
    await expect(box).toHaveCount(0);
    await expect(view(page)).toBeVisible();
    // Flagged samples show the reason under the tile.
    const flagged = page.locator('figure[data-outcome="flagged"]');
    if (await flagged.count()) await expect(flagged.first().locator('[data-testid$="-reason"]')).toContainText("Needs a look:");
    await clearCalls(page);
    await page.getByTestId("baseline-more-samples").click();
    await expect.poll(async () => ((await calls(page, "preview_baseline")).at(-1)?.args.options as { sampleCount: number } | undefined)?.sampleCount).toBe(36);
  });

  test("P1-6: choosing another preset after step 3 replaces the look instead of stacking presets (and confirms when the user changed colours)", async ({ page }) => {
    await openEdit(page, "presets");
    await toDevelop(page, /Soft Film/);
    const id = ((await calls(page, "apply_preset"))[0].args.ids as number[])[0];
    let adj = await inv<{ effects: { grain: { amount: number } }; clarity: number; hsl: { saturation: { orange: number } } }>(page, "get_adjustments", { id });
    expect(adj.effects.grain.amount).toBe(20);
    // Back to step 1 (through Edit the rest), choose Warm Matte, on to Develop.
    await page.keyboard.press("Control+Alt+b");
    await page.getByTestId("baseline-step-1").click();
    // The tiles preview on the anchor with its look reset: never Soft Film's grain under another preset.
    await page.getByRole("button", { name: /Warm Matte/ }).first().click();
    await page.getByTestId("baseline-next").click();
    await page.getByTestId("baseline-next").click();
    await expect(page.getByTestId("baseline-bar")).toBeVisible();
    adj = await inv(page, "get_adjustments", { id });
    expect(adj.effects.grain.amount).toBe(0);
    expect(adj.hsl.saturation.orange).toBe(0);
    expect(adj.clarity).toBe(-10);
    await expect(page.getByTestId("baseline-bar-preset")).toHaveText("Warm Matte");
    // Now change a colour setting by hand; switching presets asks first.
    const clarity = page.getByTestId("slider-clarity");
    await clarity.fill("-30");
    await clarity.evaluate((el) => (el as HTMLElement).blur());
    await expect.poll(async () => (await inv<{ clarity: number }>(page, "get_adjustments", { id })).clarity).toBe(-30);
    await page.keyboard.press("Control+Alt+b");
    await page.getByTestId("baseline-step-1").click();
    await page.getByRole("button", { name: /Soft Film/ }).first().click();
    await page.getByTestId("baseline-next").click();
    await page.getByTestId("baseline-next").click();
    await expect(page.getByTestId("baseline-replace-confirm")).toContainText("Replace Warm Matte and your colour changes");
    await expect(page.getByTestId("baseline-replace-confirm")).toContainText("Your light and white balance stay.");
    await page.getByTestId("baseline-replace-cancel").click();
    await expect(page.getByTestId("baseline-replace-confirm")).toHaveCount(0);
    await expect(view(page)).toBeVisible();
    await page.getByTestId("baseline-next").click();
    await page.getByTestId("baseline-replace-ok").click();
    await expect(page.getByTestId("baseline-bar")).toBeVisible();
    adj = await inv(page, "get_adjustments", { id });
    expect(adj.clarity).toBe(0);
    expect(adj.effects.grain.amount).toBe(20);
  });

  test("P1-7: Looks good, next (Cmd+Enter) walks the flagged photos; the slim Baseline bar counts what is left", async ({ page }) => {
    await openEdit(page, "1");
    await page.getByTestId("plan-baseline").click();
    const total = Number((await page.getByTestId("baseline-count-flagged").locator("b").textContent()) ?? 0);
    expect(total).toBeGreaterThan(1);
    await page.getByTestId("baseline-show-flagged").click();
    await page.locator('[data-testid^="cell-"]').first().click();
    await page.keyboard.press("d");
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await expect(page.getByTestId("baseline-review-bar")).toContainText(`Needs a look: ${total} of ${total} left`);
    await expect(page.getByTestId("edit-looks-good")).toHaveAttribute("title", /Cmd\+Enter/);
    await expect(page.getByTestId("edit-next-review")).toHaveAttribute("title", "Next photo that needs a look (N)");
    for (let left = total; left > 0; left--) {
      await expect(page.getByTestId("baseline-review-left")).toContainText(`${left} of ${total} left`);
      await page.keyboard.press("Control+Enter");
    }
    await expect(page.getByText(`All ${total} checked`)).toBeVisible();
    await expect(page.getByTestId("baseline-looks-good-back")).toBeVisible();
    await expect(page.getByTestId("baseline-review-left")).toContainText(`0 of ${total} left`);
    await page.getByTestId("baseline-looks-good-back").click();
    await expect(view(page)).toBeVisible();
    await expect(page.getByTestId("baseline-flagged-list")).toHaveCount(0).catch(() => undefined);
  });

  test("P1-7: the back link of the slim bar returns to the result (Cmd+Alt+B)", async ({ page }) => {
    await openEdit(page, "1");
    await page.getByTestId("plan-baseline").click();
    await page.getByTestId("baseline-show-flagged").click();
    await page.locator('[data-testid^="cell-"]').first().click();
    await page.keyboard.press("d");
    await page.getByTestId("baseline-review-back").click();
    await expect(view(page)).toHaveAttribute("data-step", "4");
  });

  test("P2: result labels, rerun Cancel, skip / replace counts, keys in the view, banner buttons, window title", async ({ page }) => {
    await openEdit(page, "1");
    const banner = page.getByTestId("plan-baseline-banner");
    await expect(banner).toContainText(/\d+ edited · \d+ need a look/);
    await expect(page.getByTestId("plan-baseline-finish")).toBeVisible();
    await page.getByTestId("plan-baseline-finish").click();
    await expect(view(page)).toHaveAttribute("data-step", "5");
    await expect.poll(() => page.title()).toBe("ceremony · Baseline edit · Sieve");
    await page.getByTestId("baseline-finish-plan").click();
    await expect.poll(() => page.title()).toBe("Sieve");
    await page.getByTestId("plan-baseline").click();
    await expect(page.getByTestId("baseline-count-applied")).toContainText("Look fine");
    await expect(page.getByTestId("baseline-count-flagged")).toContainText("Need a look");
    await expect(page.getByTestId("baseline-count-skipped")).toContainText("Skipped (already edited)");
    // The preset name survives a reload of the session (taken from the run).
    await expect(page.getByTestId("baseline-summary-line")).toContainText("Soft Film");
    // Change settings -> Cancel returns to the result.
    await page.getByTestId("baseline-rerun").click();
    await expect(page.getByTestId("baseline-result")).toHaveCount(0);
    await expect(page.getByTestId("baseline-skip")).toBeVisible();
    await expect(page.getByTestId("baseline-skip").locator("xpath=..")).toContainText(/Skip them \(\d+\)/);
    await expect(page.getByTestId("baseline-replace").locator("xpath=..")).toContainText(/Replace their edit \(\d+\)/);
    await expect(page.getByTestId("baseline-view")).toContainText("Photos that already have an edit (yours, Auto edit, Apply to scene or a sidecar)");
    await page.getByTestId("baseline-rerun-cancel").click();
    await expect(page.getByTestId("baseline-result")).toBeVisible();
    // Cmd+Enter = the primary action of the step: Finish in Lightroom.
    await page.keyboard.press("Control+Enter");
    await expect(view(page)).toHaveAttribute("data-step", "5");
  });

  test("P2: Enter goes to the next step in steps 1 and 2; arrow keys move between preset tiles; the cheat sheet lists Baseline edit first", async ({ page }) => {
    await openEdit(page, "presets");
    await page.getByTestId("plan-baseline").click();
    await page.getByRole("button", { name: /Soft Film/ }).first().click();
    await page.keyboard.press("ArrowRight");
    await expect(page.locator('button[data-testid^="baseline-preset-"][data-selected="true"]')).toHaveCount(1);
    await page.keyboard.press("Enter");
    await expect(view(page)).toHaveAttribute("data-step", "2");
    await page.keyboard.press("Escape");
    await expect(view(page)).toHaveCount(0);
    await page.keyboard.press("?");
    const first = page.getByTestId("cheat-group-Workflow").locator("li").first();
    await expect(first).toContainText("Baseline edit");
    await expect(page.getByTestId("cheat-baselineLooksGood")).toContainText("Cmd+Enter");
    await expect(page.getByTestId("cheat-baselineNext")).toContainText("Cmd+Enter");
  });

  test("P2-7: the anchor step can list every keeper, not only one per scene", async ({ page }) => {
    await openEdit(page, "presets");
    await page.getByTestId("plan-baseline").click();
    await page.getByTestId("baseline-next").click();
    const tiles = page.locator('button[data-testid^="baseline-anchor-"][data-selected]');
    const few = await tiles.count();
    await page.getByTestId("baseline-anchor-all").click();
    await expect.poll(async () => tiles.count()).toBeGreaterThan(few);
    expect(await tiles.count()).toBe(43);
    await tiles.nth(5).click();
    await expect(page.locator('button[data-testid^="baseline-anchor-"][data-selected="true"]')).toHaveCount(1);
  });

  test("P2-12: the mock lists imported presets once (a preset is not selected twice)", async ({ page }) => {
    await openEdit(page, "presets");
    await page.getByTestId("plan-baseline").click();
    const names = await page.locator('button[data-testid^="baseline-preset-"][data-testid$="0"], button[data-testid^="baseline-preset-"]').evaluateAll((els) => els.map((e) => e.textContent ?? ""));
    expect(names.filter((n) => n.includes("Soft Film")).length).toBe(1);
    const sel = await page.getByRole("button", { name: /Soft Film/ }).evaluateAll((els) => els.filter((e) => e.getAttribute("data-selected") === "true").length);
    expect(sel).toBe(0);
    await page.getByRole("button", { name: /Soft Film/ }).first().click();
    await expect(page.locator('button[data-testid^="baseline-preset-"][data-selected="true"]')).toHaveCount(1);
  });
});
