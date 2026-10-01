// UX re-check 2 final round: P1-12 reset scenes (frontend), skippedScenes toast, and the P2 bugs #10-#13.
// Mock rules: project 1 = ids 1..101, 43 keepers in scenes 1 (16), 2 (17), 3 (10).
import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openHome, shot } from "./helpers";

const P = "p112-";
test.use({ viewport: { width: 1280, height: 800 } });

async function openPlan(page: Page) {
  await openHome(page, 201, "&style=ready");
  await page.getByTestId("project-open-1").click();
  await expect(page.getByTestId("grid-toolbar")).toBeVisible();
  await page.getByTestId("continue-edit").click();
  await expect(page.getByTestId("plan-view")).toBeVisible();
  await expect(page.locator('article[data-testid^="plan-scene-"]').first()).toBeVisible();
}
async function backToPlan(page: Page) {
  await page.getByTestId("home-button").click();
  await expect(page.getByTestId("home-page")).toBeVisible();
  await page.getByTestId("project-open-1").click();
  await expect(page.getByTestId("plan-view").or(page.getByTestId("grid-toolbar"))).toBeVisible();
  if (!(await page.getByTestId("plan-view").isVisible())) await page.getByTestId("continue-edit").click();
  await expect(page.getByTestId("plan-view")).toBeVisible();
}
const row = (page: Page, id: number) => page.getByTestId(`plan-scene-${id}`);
const notices = (page: Page) => page.getByTestId("notice");

/** Auto edit scene 1, apply it, open its representative in Develop and reset it. */
async function resetRep(page: Page) {
  await page.getByTestId("plan-auto-1").click();
  await expect(row(page, 1)).toHaveAttribute("data-status", "auto");
  await page.getByTestId("plan-apply-1").click();
  await expect(row(page, 1)).toHaveAttribute("data-status", "applied");
  await page.getByTestId("plan-edit-1").click();
  await expect(page.getByTestId("develop-view")).toBeVisible();
  await page.keyboard.press("Meta+Shift+r");
  await page.waitForTimeout(500);
  await page.keyboard.press("g");
  await expect(page.getByTestId("plan-view")).toBeVisible();
  await expect(row(page, 1)).toHaveAttribute("data-status", "reset");
}

test.describe("P1-12 reset scenes", () => {
  test("repro b: reset rep -> row says so, Edit + inline Undo, no Re-apply; Apply all skips it", async ({ page }) => {
    await openPlan(page);
    await resetRep(page);
    await expect(page.getByTestId("plan-status-1")).toContainText(/Representative reset · \d+ photos keep the earlier look/);
    await expect(page.getByTestId("plan-edit-1")).toBeVisible();
    await expect(page.getByTestId("plan-undo-inline-1")).toBeEnabled();
    await expect(page.getByTestId("plan-apply-1")).toHaveCount(0);
    await shot(page, `${P}1280-plan-reset`);
    // Auto edit the rest: Apply all counts scenes 2 and 3 only and succeeds.
    await page.getByTestId("plan-auto-2").click();
    await expect(row(page, 2)).toHaveAttribute("data-status", "auto");
    await page.getByTestId("plan-auto-3").click();
    await expect(row(page, 3)).toHaveAttribute("data-status", "auto");
    await expect(page.getByTestId("plan-apply-all")).toContainText("Apply 2 edited scenes");
    await page.getByTestId("plan-apply-all").click();
    await expect(row(page, 2)).toHaveAttribute("data-status", "applied");
    await expect(row(page, 3)).toHaveAttribute("data-status", "applied");
    await expect(row(page, 1)).toHaveAttribute("data-status", "reset");
    await expect(page.getByTestId("error")).toHaveCount(0);
    await expect(page.getByTestId("plan-continue-export")).toHaveCount(0);
    // Re-edit scene 1's representative (auto edit lands on the applied settings): the plan is done.
    await page.getByTestId("plan-auto-1").click();
    await expect(row(page, 1)).toHaveAttribute("data-status", "applied");
    await expect(page.getByTestId("plan-continue-export")).toBeVisible();
  });

  test("repro b: inline Undo apply restores the members and the scene becomes to do", async ({ page }) => {
    await openPlan(page);
    await resetRep(page);
    await clearCalls(page);
    await page.getByTestId("plan-undo-inline-1").click();
    await expect.poll(async () => (await calls(page, "undo_edit_batch")).length).toBe(1);
    await expect(row(page, 1)).toHaveAttribute("data-status", "todo");
    await expect(page.getByTestId("plan-undo-inline-1")).toHaveCount(0);
  });

  test("Develop: reset representative shows the chip and a disabled Apply to scene with the hint", async ({ page }) => {
    await openPlan(page);
    await page.getByTestId("plan-auto-1").click();
    await expect(row(page, 1)).toHaveAttribute("data-status", "auto");
    await page.getByTestId("plan-apply-1").click();
    await expect(row(page, 1)).toHaveAttribute("data-status", "applied");
    await page.getByTestId("plan-edit-1").click();
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await page.keyboard.press("Meta+Shift+r");
    await expect(page.getByTestId("edit-chip")).toContainText("Reset since applied · representative");
    await expect(page.getByTestId("edit-apply")).toBeDisabled();
    await expect(page.getByTestId("edit-apply")).toHaveAttribute("title", "Edit this photo first");
  });

  test("repro a: older Auto edit Undo is retired after Apply scene 1; Apply all then applies the rest", async ({ page }) => {
    await openPlan(page);
    await page.getByTestId("plan-auto-remaining").click();
    await expect(row(page, 3)).toHaveAttribute("data-status", "auto");
    await page.getByTestId("plan-apply-1").click();
    await expect(row(page, 1)).toHaveAttribute("data-status", "applied");
    await expect(page.getByTestId("auto-undo")).toHaveCount(0);
    await expect(page.getByTestId("apply-undo-batch")).toHaveCount(1);
    await page.getByTestId("plan-apply-all").click();
    await expect(row(page, 3)).toHaveAttribute("data-status", "applied");
    await expect(page.getByTestId("plan-continue-export")).toBeVisible();
  });
});

test.describe("skipped scenes toast", () => {
  test("Apply all names the scene that could not be applied and offers Show", async ({ page }) => {
    await openPlan(page);
    await page.getByTestId("plan-auto-remaining").click();
    await expect(row(page, 3)).toHaveAttribute("data-status", "auto");
    const sceneId = Number((await row(page, 2).getAttribute("data-testid"))!.replace("plan-scene-", ""));
    await page.evaluate((id) => ((window as unknown as { __mockApplyFailScenes: number[] }).__mockApplyFailScenes = [id]), sceneId);
    await page.getByTestId("plan-apply-all").click();
    const skipped = notices(page).filter({ hasText: "Not applied: Scene 2:" });
    await expect(skipped).toBeVisible();
    await expect(skipped).toContainText("is missing");
    await expect(skipped.getByTestId("apply-skipped-show")).toBeVisible();
    await expect(row(page, 3)).toHaveAttribute("data-status", "applied");
    await expect(row(page, 2)).not.toHaveAttribute("data-status", "applied");
    await expect(page.getByTestId("error")).toHaveCount(0);
  });
});

test.describe("P2 bugs from re-check 2", () => {
  async function applyThenEditMember(page: Page) {
    await page.getByTestId("plan-auto-2").click();
    await expect(row(page, 2)).toHaveAttribute("data-status", "auto");
    await page.getByTestId("plan-apply-2").click();
    await expect(row(page, 2)).toHaveAttribute("data-status", "applied");
    await page.getByTestId("plan-edit-2").click();
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await page.keyboard.press("ArrowRight");
    const s = page.getByTestId("slider-exposure");
    await s.fill("1.5");
    await s.evaluate((el) => (el as HTMLElement).blur());
    await expect(page.getByTestId("apply-undo-batch")).toHaveCount(0);
    await page.waitForTimeout(700);
  }

  test("#10 per-image undo of a member edit re-enables the row Undo apply", async ({ page }) => {
    await openPlan(page);
    await applyThenEditMember(page);
    await page.keyboard.press("Meta+z"); // per-image undo back to the applied value
    await page.waitForTimeout(700);
    await page.keyboard.press("g");
    await expect(page.getByTestId("plan-view")).toBeVisible();
    await page.getByTestId("plan-menu-2").click();
    await expect(page.getByTestId("plan-undo-2")).toBeEnabled();
  });

  test("#11 Cmd+Z in the Plan explains a blocked undo instead of Nothing to undo", async ({ page }) => {
    await openPlan(page);
    await applyThenEditMember(page);
    await page.keyboard.press("g");
    await expect(page.getByTestId("plan-view")).toBeVisible();
    await page.keyboard.press("Meta+z");
    await expect(notices(page).last()).toContainText(/Later edits on 1 photo\. Undo those in Develop first/);
    await expect(page.getByText("Nothing to undo")).toHaveCount(0);
  });

  test("#12 the undo toast names the scene after Home and back", async ({ page }) => {
    await openPlan(page);
    await page.getByTestId("plan-auto-2").click();
    await expect(row(page, 2)).toHaveAttribute("data-status", "auto");
    await page.getByTestId("plan-apply-2").click();
    await expect(row(page, 2)).toHaveAttribute("data-status", "applied");
    await backToPlan(page);
    await expect(row(page, 2)).toHaveAttribute("data-status", "applied");
    await page.getByTestId("plan-menu-2").click();
    await page.getByTestId("plan-undo-2").click();
    await expect(notices(page).last()).toContainText(/Undid Apply Scene 2 on \d+ photos/);
  });

  test("#13 Develop toasts sit top-right, clear of the photo centre and the adjustment panel", async ({ page }) => {
    await openPlan(page);
    await page.getByTestId("plan-auto-1").click();
    await expect(page.getByTestId("auto-undo")).toBeVisible();
    await page.getByTestId("plan-edit-1").click();
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await expect(page.getByTestId("toasts")).toHaveAttribute("data-placement", "top");
    const t = (await page.getByTestId("toasts").boundingBox())!;
    const aside = (await page.getByTestId("right-aside").boundingBox())!;
    expect(t.x + t.width).toBeLessThanOrEqual(aside.x);
    expect(t.x).toBeGreaterThan(560);
    await shot(page, `${P}1280-develop-toast`);
  });

  test("#13 toasts move bottom-left while the Match panel is open", async ({ page }) => {
    await openPlan(page);
    await page.getByTestId("plan-auto-1").click();
    await expect(page.getByTestId("auto-undo")).toBeVisible();
    await page.getByTestId("plan-menu-1").click();
    await page.getByTestId("plan-options-1").click();
    await expect(page.getByTestId("match-panel")).toBeVisible();
    await expect(page.getByTestId("toasts")).toHaveAttribute("data-placement", "modal");
    const t = (await page.getByTestId("toasts").boundingBox())!;
    const m = (await page.getByRole("dialog", { name: /Match Scene/ }).boundingBox())!;
    const overlap = !(t.x + t.width <= m.x || m.x + m.width <= t.x || t.y + t.height <= m.y || m.y + m.height <= t.y);
    expect(overlap).toBe(false);
  });
});
