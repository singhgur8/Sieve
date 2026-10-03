import { expect, test, type Page } from "@playwright/test";
import { calls, openHome, shot } from "./helpers";

// UX review 8c P1-1 (summary's suggestion counts = what Apply with defaults changes), P1-6 ("by you" / "auto"
// are real filters, IPC v18.1 `ImageQuery.pickOrigin`) and P2 #10 (summary bar stays one row at 1280).
// Mock: project 1 = ids 1..101; every 7th photo is pre-flagged by the user.
const V18 = "&keepers=not_rejected";

async function openProject(page: Page) {
  await openHome(page, 201, V18);
  await page.getByTestId("project-open-1").click();
  await expect(page.getByTestId("grid-toolbar")).toBeVisible();
  await expect(page.getByTestId("cull-summary")).toBeVisible();
}

/** "Suggestions: 20 picks · 8 rejects · 3 stars only — Apply…" -> the sum of the parts. */
async function suggestedTotal(page: Page): Promise<number> {
  const text = (await page.getByTestId("cull-sum-suggest").textContent()) ?? "";
  expect(text).toMatch(/^Suggestions: .+ — Apply…$/);
  expect(text).not.toMatch(/(^|[ :·])0 /); // zero parts are dropped
  return [...text.matchAll(/(\d+) (picks?|rejects?|stars only)/g)].reduce((a, m) => a + Number(m[1]), 0);
}

async function applyWithDefaults(page: Page) {
  const n = await suggestedTotal(page);
  expect(n).toBeGreaterThan(0);
  await page.getByTestId("cull-sum-suggest").click();
  await expect(page.getByTestId("apply-count-apply")).toHaveText(String(n));
  await expect(page.getByTestId("apply-confirm")).toHaveText(`Apply to ${n}`);
  await page.getByTestId("apply-confirm").click();
  await expect(page.getByTestId("notice").last()).toContainText(`Applied suggestions to ${n} of`);
}

test.describe("cull summary: suggestions and flag origin (v18.1)", () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  test("the suggestion button counts exactly what Apply changes and is gone after one Apply", async ({ page }) => {
    await openProject(page);
    const bar = page.getByTestId("cull-summary");
    await expect(page.getByTestId("cull-sum-suggest")).toBeVisible();
    // One row at 1280 (P2 #10): no wrap, the formula truncates with the full text in its title.
    expect((await bar.boundingBox())!.height).toBeLessThan(34);
    const formula = page.getByTestId("cull-sum-formula");
    expect(await formula.getAttribute("title")).toContain((await formula.textContent())!);
    await shot(page, "cull-origin-1280-before-apply");
    await applyWithDefaults(page);
    await expect(page.getByTestId("cull-sum-suggest")).toHaveCount(0);
    expect((await bar.boundingBox())!.height).toBeLessThan(34);
    // The More menu still opens the dialog; with defaults there is nothing left to do.
    await page.getByTestId("more-menu").click();
    await page.getByTestId("apply-suggestions").click();
    await expect(page.getByTestId("apply-count-apply")).toHaveText("0");
    await expect(page.getByTestId("apply-confirm")).toBeDisabled();
  });

  test("'by you' and 'auto' filter the rejects by origin, with an active state and a filter chip", async ({ page }) => {
    await openProject(page);
    await applyWithDefaults(page); // creates auto-rejects next to the user's own
    const byYou = page.getByTestId("cull-sum-by-you");
    const auto = page.getByTestId("cull-sum-auto");
    await expect(auto).not.toHaveText(/^0 auto/);
    const nAuto = Number((await auto.textContent())!.match(/^(\d+)/)![1]);
    const nYou = Number((await byYou.textContent())!.match(/^(\d+)/)![1]);
    const nRejected = Number(await page.getByTestId("cull-sum-rejected").locator("b").textContent());
    expect(nAuto + nYou).toBe(nRejected);
    const readout = page.getByTestId("selection-count");

    await auto.click();
    await expect(auto).toHaveAttribute("aria-pressed", "true");
    await expect(byYou).toHaveAttribute("aria-pressed", "false");
    await expect(readout).toContainText(`Showing ${nAuto} of 101`);
    await expect(page.getByTestId("filter-origin")).toHaveText(/^Auto/);
    await expect(page.getByTestId("pick-reject")).toContainText(String(nAuto)); // get_filter_counts honours the origin
    expect((await calls(page, "get_filter_counts")).some((c) => c.args.pickOrigin === "auto")).toBe(true);
    expect((await calls(page, "list_image_ids")).at(-1)!.args.query).toMatchObject({ picks: ["reject"], pickOrigin: "auto" });
    for (const el of await page.locator('[data-testid^="reject-reason-"]').all()) await expect(el).toHaveAttribute("data-origin", "auto");
    await shot(page, "cull-origin-auto-rejects");

    await byYou.click();
    await expect(byYou).toHaveAttribute("aria-pressed", "true");
    await expect(auto).toHaveAttribute("aria-pressed", "false");
    await expect(readout).toContainText(`Showing ${nYou} of 101`);
    await expect(page.getByTestId("filter-origin")).toHaveText(/^By you/);
    for (const el of await page.locator('[data-testid^="reject-reason-"]').all()) await expect(el).toHaveAttribute("data-origin", "user");

    // The chip's × drops the origin only: every reject again.
    await page.getByTestId("filter-origin").click();
    await expect(page.getByTestId("filter-origin")).toHaveCount(0);
    await expect(readout).toContainText(`Showing ${nRejected} of 101`);
    await expect(page.getByTestId("cull-sum-rejected")).toHaveClass(/bg-sky-800/);

    // Clicking an active origin toggle turns the filter off.
    await auto.click();
    await expect(readout).toContainText(`Showing ${nAuto} of 101`);
    await auto.click();
    await expect(auto).toHaveAttribute("aria-pressed", "false");
    await expect(readout).toContainText("Showing 101 of 101");
  });
});
