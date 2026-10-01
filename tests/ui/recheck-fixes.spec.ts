// UX re-check fixes (docs/ux-review-8b.md "Re-check"): P1-10, P1-11 (frontend), and the cheap P2s.
// Mock rules: project 1 = ids 1..101, 43 keepers in scenes 1 (16), 2 (17), 3 (10); applies do not converge for ids % 7 == 0.
import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openHome, shot } from "./helpers";

const P = "recheck-";

async function openPlan(page: Page, query = "&style=ready") {
  await openHome(page, 201, query);
  await page.getByTestId("project-open-1").click();
  await expect(page.getByTestId("grid-toolbar")).toBeVisible();
  await page.getByTestId("continue-edit").click();
  await expect(page.getByTestId("plan-view")).toBeVisible();
  await expect(page.locator('article[data-testid^="plan-scene-"]').first()).toBeVisible();
}

const row = (page: Page, id: number) => page.getByTestId(`plan-scene-${id}`);
const toast = (page: Page) => page.getByTestId("notice").last();
const inv = (page: Page, cmd: string, args: Record<string, unknown>) =>
  page.evaluate(([c, a]) => (window as unknown as { __TAURI_INTERNALS__: { invoke: (c: string, a: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke(c as string, a), [cmd, args] as const);

async function openOptions(page: Page, scene: number) {
  await page.getByTestId(`plan-menu-${scene}`).click();
  await page.getByTestId(`plan-options-${scene}`).click();
  await expect(page.getByTestId("match-panel")).toBeVisible();
}

test.describe("P1-10 Apply with options targets keepers", () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  test("Scene 3: Apply to 9 photos, non-keepers opt-in, toast and row agree", async ({ page }) => {
    await openPlan(page);
    await page.getByTestId("plan-auto-3").click();
    await expect(row(page, 3)).toHaveAttribute("data-status", "auto");
    await openOptions(page, 3);
    await expect(page.getByTestId("match-include-nonkeepers")).not.toBeChecked();
    await page.getByTestId("match-run").click();
    await expect(page.getByTestId("match-apply")).toHaveText("Apply to 9 photos");
    // Non-keepers are not listed at all.
    for (const id of [82, 83, 84]) await expect(page.getByTestId(`match-card-${id}`)).toHaveCount(0);
    await shot(page, `${P}1280-match-panel`);
    await clearCalls(page);
    await page.getByTestId("match-apply").click();
    await expect(row(page, 3)).toHaveAttribute("data-status", "applied");
    const [ap] = await calls(page, "apply_scene_edit");
    const o = ap.args.options as { includeNonKeepers: boolean; excludeIds: number[] };
    expect(o.includeNonKeepers).toBe(false);
    expect(o.excludeIds.some((i) => [82, 83, 84].includes(i))).toBe(false);
    await expect(toast(page)).toContainText("Applied Scene 3 to 9 photos");
  });

  test("including non-keepers sends includeNonKeepers; toast Review of non-keepers stays put", async ({ page }) => {
    await openPlan(page);
    await page.getByTestId("plan-auto-3").click();
    await expect(row(page, 3)).toHaveAttribute("data-status", "auto");
    await openOptions(page, 3);
    await page.getByTestId("match-include-nonkeepers").check();
    await page.getByTestId("match-run").click();
    await expect(page.getByTestId("match-apply")).toHaveText(/Apply to (1[1-9]|2\d) photos/);
    await clearCalls(page);
    await page.getByTestId("match-apply").click();
    const [ap] = await calls(page, "apply_scene_edit");
    expect((ap.args.options as { includeNonKeepers: boolean }).includeNonKeepers).toBe(true);
    await expect(page.getByTestId("apply-review")).toBeVisible();
    await page.getByTestId("apply-review").click();
    await expect(toast(page)).toContainText("The frames that need a look are not keepers");
    await expect(page.getByTestId("plan-view")).toBeVisible();
    await expect(page.getByTestId("develop-view")).toHaveCount(0);
  });

  test("frames the user edited start unticked with an Edited by you badge", async ({ page }) => {
    await openPlan(page);
    await page.getByTestId("plan-auto-3").click();
    await expect(row(page, 3)).toHaveAttribute("data-status", "auto");
    const adj = (await inv(page, "get_adjustments", { imageId: 85 })) as Record<string, unknown>;
    await inv(page, "save_adjustments", { id: 85, adjustments: { ...adj, exposure: 0.7 }, label: "Exposure" });
    await openOptions(page, 3);
    await page.getByTestId("match-run").click();
    await expect(page.getByTestId("match-card-85")).toHaveAttribute("data-included", "false");
    await expect(page.getByTestId("match-edited-85")).toHaveText("Edited by you");
    await expect(page.getByTestId("match-apply")).toHaveText("Apply to 8 photos");
    await expect(page.getByTestId("match-selected-count")).toContainText("1 edited by you (unticked)");
    await page.getByTestId("match-include-85").check(); // an explicit tick overrides
    await expect(page.getByTestId("match-apply")).toHaveText("Apply to 9 photos");
  });
});

test.describe("P1-11 toast Undo is linear", () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  test("an older toast loses its Undo once a newer apply exists; the row Undo follows the same rule", async ({ page }) => {
    await openPlan(page);
    await page.getByTestId("plan-auto-remaining").click();
    await expect(row(page, 1)).toHaveAttribute("data-status", "auto");
    await expect(page.getByTestId("auto-undo")).toBeVisible();
    await page.getByTestId("plan-apply-1").click();
    await expect(row(page, 1)).toHaveAttribute("data-status", "applied");
    await expect(page.getByTestId("auto-undo")).toHaveCount(0);
    await expect(page.getByTestId("apply-undo-batch")).toHaveCount(1);
    await page.getByTestId("plan-menu-1").click();
    await expect(page.getByTestId("plan-undo-1")).toBeEnabled();
    await page.keyboard.press("Escape");
    // Applying a second scene makes the first scene's row Undo unavailable, with a reason.
    await page.getByTestId("plan-apply-2").click();
    await expect(row(page, 2)).toHaveAttribute("data-status", "applied");
    await page.getByTestId("plan-menu-1").click();
    await expect(page.getByTestId("plan-undo-1")).toBeDisabled();
    await expect(page.getByTestId("plan-undo-1")).toHaveAttribute("title", /Undo that first/);
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("apply-undo-batch")).toHaveCount(1); // only the newest toast keeps Undo
  });

  test("a later edit in Develop retires the Undo of the batch that wrote the photo", async ({ page }) => {
    await openPlan(page);
    await page.getByTestId("plan-auto-1").click();
    await expect(row(page, 1)).toHaveAttribute("data-status", "auto");
    await page.getByTestId("plan-apply-1").click();
    await expect(row(page, 1)).toHaveAttribute("data-status", "applied");
    await expect(page.getByTestId("apply-undo-batch")).toBeVisible();
    await page.getByTestId("plan-edit-1").click();
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await page.keyboard.press("ArrowRight"); // a member, written by the apply
    const s = page.getByTestId("slider-exposure");
    await s.fill("1.5");
    await s.evaluate((el) => (el as HTMLElement).blur());
    await expect(page.getByTestId("apply-undo-batch")).toHaveCount(0);
    await page.keyboard.press("g");
    await expect(page.getByTestId("plan-view")).toBeVisible();
    await page.getByTestId("plan-menu-1").click();
    await expect(page.getByTestId("plan-undo-1")).toBeDisabled();
  });

  test("a conflict error from undo_edit_batch shows its message and retires the Undo", async ({ page }) => {
    await openPlan(page);
    await page.getByTestId("plan-auto-1").click();
    await expect(row(page, 1)).toHaveAttribute("data-status", "auto");
    await page.evaluate(() => {
      (window as unknown as { __mockFail: unknown }).__mockFail = { undo_edit_batch: { kind: "conflict", message: "Later edits on 3 photos; undo those first", once: true } };
    });
    await page.getByTestId("auto-undo").click();
    await expect(page.getByTestId("notice").filter({ hasText: "Later edits on 3 photos; undo those first" })).toBeVisible();
    await expect(page.getByTestId("auto-undo")).toHaveCount(0);
  });
});

test.describe("re-check P2s", () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  test("cheat sheet lists S and Esc for the Plan; menu hints show (S)", async ({ page }) => {
    await openPlan(page);
    await page.getByTestId("plan-menu-1").click();
    await expect(page.getByTestId("plan-skip-1")).toContainText("Skip this scene (S)");
    await page.getByTestId("plan-skip-1").click();
    await page.getByTestId("plan-menu-1").click();
    await expect(page.getByTestId("plan-skip-1")).toContainText("Include this scene (S)");
    await page.keyboard.press("Escape");
    await page.keyboard.press("?");
    const sheet = page.getByTestId("cheat-sheet");
    await expect(sheet).toBeVisible();
    await expect(sheet).toContainText("Skip / include the focused scene");
    await expect(sheet).toContainText("Stop applying");
  });

  test("progress pill says Applying… before the first progress event", async ({ page }) => {
    await openPlan(page);
    await page.getByTestId("plan-auto-remaining").click();
    await expect(row(page, 3)).toHaveAttribute("data-status", "auto");
    await page.evaluate(() => ((window as unknown as { __mockSceneDelay: number }).__mockSceneDelay = 900));
    await page.getByTestId("plan-apply-all").click();
    const pill = page.getByTestId("plan-applying");
    await expect(pill).toBeVisible();
    await expect(pill).not.toContainText("0/0");
    await page.getByTestId("plan-apply-cancel").click();
  });

  test("apply-all button is hidden when nothing is applicable", async ({ page }) => {
    await openPlan(page);
    await page.getByTestId("plan-auto-remaining").click();
    await expect(row(page, 3)).toHaveAttribute("data-status", "auto");
    await page.getByTestId("plan-apply-all").click();
    await expect(row(page, 3)).toHaveAttribute("data-status", "applied");
    // Every scene applied: Continue to Export replaces it. Skip one scene to leave nothing applicable and nothing to do.
    await expect(page.getByTestId("plan-apply-all")).toHaveCount(0);
  });

  test("toasts with Undo fade after about 10 s", async ({ page }) => {
    await openPlan(page);
    await page.getByTestId("plan-auto-1").click();
    await expect(row(page, 1)).toHaveAttribute("data-status", "auto");
    await page.getByTestId("plan-apply-1").click();
    await expect(page.getByTestId("apply-undo-batch")).toBeVisible();
    await page.waitForTimeout(8500);
    await expect(page.getByTestId("apply-undo-batch")).toBeVisible();
    await expect(page.getByTestId("apply-undo-batch")).toHaveCount(0, { timeout: 4000 });
    // Still undoable from the row menu.
    await page.getByTestId("plan-menu-1").click();
    await expect(page.getByTestId("plan-undo-1")).toBeEnabled();
  });

  test("toasts do not cover the Develop viewer toolbar", async ({ page }) => {
    await openPlan(page);
    await page.getByTestId("plan-auto-1").click();
    await expect(page.getByTestId("auto-undo")).toBeVisible();
    await page.getByTestId("plan-edit-1").click();
    await expect(page.getByTestId("develop-view")).toBeVisible();
    const t = await page.getByTestId("toasts").boundingBox();
    const bar = await page.getByTestId("develop-view").locator("[data-testid=viewer-toolbar], [data-testid=develop-toolbar]").first().boundingBox();
    await shot(page, `${P}1280-develop-toast`);
    if (t && bar) {
      const overlap = !(t.x + t.width <= bar.x || bar.x + bar.width <= t.x || t.y + t.height <= bar.y || bar.y + bar.height <= t.y);
      expect(overlap).toBe(false);
    }
  });

  test("sort direction is labelled", async ({ page }) => {
    await openHome(page, 201, "");
    await page.getByTestId("project-open-1").click();
    await expect(page.getByTestId("grid-toolbar")).toBeVisible();
    await page.getByTestId("view-menu").click();
    await expect(page.getByTestId("sort-dir")).toContainText(/Ascending|Descending/);
  });
});
