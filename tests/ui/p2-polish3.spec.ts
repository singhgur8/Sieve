// UX re-check 3 P2 polish: #10 header Apply hidden, #11 strip fits, #13 no Review on reset, #14 Develop hint at 1280.
import { expect, test, type Page } from "@playwright/test";
import { openHome, shot } from "./helpers";

test.use({ viewport: { width: 1280, height: 800 } });
const row = (page: Page, id: number) => page.getByTestId(`plan-scene-${id}`);

async function openPlan(page: Page) {
  await openHome(page, 201, "&style=ready");
  await page.getByTestId("project-open-1").click();
  await expect(page.getByTestId("grid-toolbar")).toBeVisible();
  await page.getByTestId("continue-edit").click();
  await expect(page.getByTestId("plan-view")).toBeVisible();
  await expect(page.locator('article[data-testid^="plan-scene-"]').first()).toBeVisible();
}
async function resetRep(page: Page, stayInDevelop = false) {
  await page.getByTestId("plan-auto-1").click();
  await expect(row(page, 1)).toHaveAttribute("data-status", "auto");
  await page.getByTestId("plan-apply-1").click();
  await expect(row(page, 1)).toHaveAttribute("data-status", "applied");
  await page.getByTestId("plan-edit-1").click();
  await expect(page.getByTestId("develop-view")).toBeVisible();
  await page.keyboard.press("Meta+Shift+r");
  await page.waitForTimeout(500);
  if (stayInDevelop) return;
  await page.keyboard.press("g");
  await expect(page.getByTestId("plan-view")).toBeVisible();
  await expect(row(page, 1)).toHaveAttribute("data-status", "reset");
}

test("#10 header Apply is hidden once a scene was applied and nothing is pending", async ({ page }) => {
  await openPlan(page);
  await expect(page.getByTestId("plan-apply-all")).toBeVisible(); // fresh plan keeps the disabled preview
  await page.getByTestId("plan-auto-2").click();
  await page.getByTestId("plan-auto-3").click();
  await expect(row(page, 3)).toHaveAttribute("data-status", "auto");
  await page.getByTestId("plan-apply-all").click();
  await expect(row(page, 3)).toHaveAttribute("data-status", "applied");
  await expect(page.getByTestId("plan-apply-all")).toHaveCount(0);
});

test("#10/#11/#13 reset row: no header Apply, strip ends on whole thumb, no Review", async ({ page }) => {
  await openPlan(page);
  await resetRep(page);
  await expect(page.getByTestId("plan-apply-all")).toHaveCount(0);
  await expect(page.getByTestId("plan-review-1")).toHaveCount(0);
  const ok = await page.getByTestId("plan-members-1").evaluate((el) => {
    const box = el.getBoundingClientRect();
    return Array.from(el.children).every((c) => c.getBoundingClientRect().right <= box.right + 0.5);
  });
  expect(ok).toBe(true);
  await expect(page.getByTestId("plan-more-1")).toBeVisible();
  await shot(page, "p2-polish3-plan-reset-1280");
});

test("#14 Develop shows the reset hint at 1280", async ({ page }) => {
  await openPlan(page);
  await resetRep(page, true);
  await expect(page.getByTestId("edit-chip")).toContainText("Reset since applied");
  await expect(page.getByText("Edit this photo first", { exact: true })).toBeVisible();
});
