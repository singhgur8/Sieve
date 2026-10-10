import { expect, test, type Page } from "@playwright/test";
import { openHome } from "./helpers";

// UX review 8c, Re-check 1: P1-1 / P1-8 remainders, N1, N2, N3, N4, N5, N6.
const V18 = "&keepers=not_rejected";

const activity = (page: Page, e: Record<string, unknown>) =>
  page.evaluate((ev) => (window as unknown as { __mockActivity: (e: unknown) => void }).__mockActivity(ev), { message: null, done: 0, total: null, state: "running", kind: "other", label: "Working", ...e });

async function openProject(page: Page, n = 201) {
  await openHome(page, n, V18);
  await page.getByTestId("project-open-1").click();
  await expect(page.getByTestId("cull-summary")).toBeVisible();
}

test.use({ viewport: { width: 1280, height: 800 } });

test("P1-1: the bar's numbers appear verbatim in the dialog; stars only wording", async ({ page }) => {
  await openProject(page);
  const bar = (await page.getByTestId("cull-sum-suggest").textContent())!;
  const parts = bar.replace(/^Suggestions: /, "").replace(/ — Apply…$/, "").split(" · ");
  expect(bar).toContain("stars only");
  expect(bar).not.toContain("star-rated");
  await page.getByTestId("cull-sum-suggest").click();
  await expect(page.getByTestId("apply-count-apply")).toBeVisible();
  const dialog = (await page.getByTestId("apply-counts").textContent())!;
  const n = (s: string) => Number(s.match(/^\d+/)![0]);
  const find = (re: RegExp) => parts.find((p) => re.test(p));
  const pk = find(/picks?$/), rj = find(/rejects?$/), st = find(/stars only$/);
  if (pk) expect(dialog).toContain(`${n(pk)} picked`);
  if (rj) expect(dialog).toContain(`${n(rj)} rejected`);
  if (st) expect(dialog).toContain(`${n(st)} stars only`);
  expect(dialog).not.toContain("star-rated");
});

test("P1-8 / N6: help and dialog copy name the button and say 'not flagged or rated'", async ({ page }) => {
  await openProject(page);
  await page.getByTestId("cull-sum-suggest").click();
  const explain = page.getByTestId("apply-explain");
  await expect(explain).toContainText("have not flagged or rated");
  await expect(explain).not.toContainText("not touched");
  await page.keyboard.press("Escape");
  await page.keyboard.press("F1");
  await page.getByTestId("help-nav-apply-suggestions").click();
  const entry = page.getByTestId("help-entry-apply-suggestions");
  await expect(entry).toContainText('"Suggestions: … — Apply…" button at the right of the Cull summary');
  await expect(entry).not.toContainText('"Sieve suggests … Apply…"');
  await expect(entry).toContainText("click N auto in the Cull summary to see only Auto's rejects");
});

test("N1: the activity stack sits behind Help and dialogs", async ({ page }) => {
  await openProject(page);
  await activity(page, { id: "t1", kind: "xmp_save", label: "Saving sidecars" });
  const widget = page.getByTestId("activity-widget");
  await expect(widget).toBeVisible();
  await expect(widget).toHaveClass(/z-\[65\]/);
  await page.keyboard.press("F1");
  const panel = page.getByTestId("help-panel");
  await expect(panel).toBeVisible();
  await expect(widget).toHaveClass(/z-40/);
  const box = (await panel.boundingBox())!;
  const x = box.x + box.width - 40;
  const y = box.y + box.height - 20;
  const wb = (await widget.boundingBox())!;
  expect(x).toBeGreaterThan(wb.x); // the point is under the stack's column
  const inside = await page.evaluate(([px, py]) => !!document.elementFromPoint(px, py)?.closest('[data-testid="help-panel"]'), [x, y]);
  expect(inside).toBe(true);
  await page.keyboard.press("Escape");
  await expect(widget).toHaveClass(/z-\[65\]/);
  await page.getByTestId("cull-sum-suggest").click();
  await expect(page.getByTestId("apply-dialog")).toBeVisible();
  await expect(widget).toHaveClass(/z-40/);
});

test("N2 / N5: with the origin chip at 1280 the filter bar does not overflow and Clear stays on screen", async ({ page }) => {
  await openProject(page, 2000);
  await page.getByTestId("cull-sum-suggest").click();
  await page.getByTestId("apply-confirm").click();
  await expect(page.getByTestId("cull-sum-auto")).not.toHaveText(/^Auto 0/);
  await page.getByTestId("cull-sum-auto").click();
  const chip = page.getByTestId("filter-origin");
  await expect(chip).toHaveText(/^Auto/);
  await expect(chip).not.toContainText("rejected");
  await expect(chip).toHaveAttribute("title", /Auto-rejected/);
  const bar = page.getByTestId("filter-bar");
  const { sw, cw } = await bar.evaluate((e) => ({ sw: e.scrollWidth, cw: e.clientWidth }));
  expect(sw).toBeLessThanOrEqual(cw);
  const clear = (await page.getByTestId("clear-filters").boundingBox())!;
  expect(clear.x + clear.width).toBeLessThanOrEqual(1280);
  const meta = (await page.getByTestId("meta-toggle").boundingBox())!;
  expect(meta.x + meta.width).toBeLessThanOrEqual(clear.x + 1);
  await expect(page.getByTestId("clear-filters")).toHaveAttribute("aria-label", "Clear");
  await expect(page.getByTestId("pick-reject")).toHaveAttribute("title", /auto only — clear "Auto ×" to see all/);
});

test("N4 / P1-7: the reject split is a chip group; a zero Auto chip stays a button and explains itself", async ({ page }) => {
  await openProject(page);
  const auto = page.getByTestId("cull-sum-auto");
  await expect(auto).toHaveText(/^Auto 0/);
  expect(await auto.evaluate((e) => e.tagName)).toBe("BUTTON");
  expect(await page.getByTestId("cull-sum-by-you").evaluate((e) => e.tagName)).toBe("BUTTON");
  await expect(page.getByTestId("cull-sum-reject-split")).toHaveText(/^Rejected \d+By you \d+Auto 0$/);
  for (const t of ["cull-sum-by-you", "cull-sum-auto"]) {
    const bg = await page.getByTestId(t).evaluate((e) => getComputedStyle(e).backgroundColor);
    const bgPicked = await page.getByTestId("cull-sum-picked").evaluate((e) => getComputedStyle(e).backgroundColor);
    expect(bg).toBe(bgPicked);
  }
  await auto.click();
  await expect(page.getByTestId("auto-zero-note")).toContainText("No photos were auto-rejected yet");
  await expect(page.getByTestId("auto-zero-note")).toContainText("Sieve suggests rejecting");
  await page.getByTestId("auto-zero-apply").click();
  await expect(page.getByTestId("filter-suggested")).toBeVisible();
});

test("N3: Help legend strips are bars without text", async ({ page }) => {
  await openProject(page);
  await page.keyboard.press("F1");
  await page.getByTestId("help-nav-icons").click();
  for (const l of ["Rejected strip", "Suggestion strip"]) {
    const row = page.getByTestId(`help-legend-${l}`);
    await expect(row).toBeVisible();
    const icon = row.locator("span.block").first();
    await expect(icon).toHaveText("");
    const b = (await icon.boundingBox())!;
    expect(b.width).toBeGreaterThanOrEqual(54);
  }
});
