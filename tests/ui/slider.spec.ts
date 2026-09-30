import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openApp } from "./helpers";

// User report: "the sliders move but I don't see the change until I slow down". The backend keeps only the newest
// render per (image, slot) and answers superseded ones with null, so one render per input event starves the view.
// The mock reproduces that (see mockBackend `render`), here with a 40 ms render delay.

async function openDevelop(page: Page) {
  await openApp(page, 200);
  await page.getByTestId("cell-1").click();
  await page.keyboard.press("d");
  await expect(page.getByTestId("develop-view")).toBeVisible();
  await expect(page.getByTestId("view-main")).toBeVisible();
  await expect(page.getByTestId("histogram")).toHaveAttribute("data-empty", "false");
}

type W = { __shown: string[] };

test.describe("slider responsiveness", () => {
  test("a continuous 1 s drag displays many intermediate renders, then a full-quality render follows release", async ({ page }) => {
    await page.addInitScript(() => (window.__mockRenderDelay = () => 40));
    await openDevelop(page);

    // Record every distinct render (alt "Render <id> #<seq>") that reaches the main view.
    await page.evaluate(() => {
      const w = window as unknown as W;
      w.__shown = [];
      const seen = () => {
        const alt = document.querySelector('[data-testid="view-main"]')?.getAttribute("alt");
        if (alt && w.__shown[w.__shown.length - 1] !== alt) w.__shown.push(alt);
      };
      new MutationObserver(seen).observe(document.querySelector('[data-testid="viewer"]')!, {
        subtree: true,
        attributes: true,
        childList: true,
        attributeFilter: ["alt", "src"],
      });
    });
    await clearCalls(page);
    await page.evaluate(() => (window.__mockRenderStats = { inflight: 0, maxInflight: 0 }));

    const slider = page.getByTestId("slider-exposure");
    const box = (await slider.boundingBox())!;
    const y = box.y + box.height / 2;
    await page.mouse.move(box.x + box.width * 0.5, y);
    await page.mouse.down();
    const t0 = Date.now();
    let n = 0;
    while (Date.now() - t0 < 1000) {
      n++;
      const f = 0.5 + 0.45 * Math.abs(Math.sin(n / 12));
      await page.mouse.move(box.x + box.width * f, y);
      await page.waitForTimeout(12);
    }
    const shownDuringDrag = await page.evaluate(() => (window as unknown as W).__shown.length);
    await page.mouse.up();
    test.info().annotations.push({ type: "displayed-during-drag", description: String(shownDuringDrag) });
    console.log(`DISPLAYED_DURING_DRAG=${shownDuringDrag} moves=${n}`);

    // Intermediate renders were displayed while the pointer was still down.
    expect(shownDuringDrag).toBeGreaterThanOrEqual(8);
    const seqs = await page.evaluate(() => (window as unknown as W).__shown.map((a) => Number(/#(\d+)/.exec(a)![1])));
    expect(new Set(seqs).size).toBeGreaterThanOrEqual(8);
    expect(seqs).toEqual([...seqs].sort((a, b) => a - b)); // never goes backwards

    // One render in flight at a time.
    expect(await page.evaluate(() => window.__mockRenderStats!.maxInflight)).toBe(1);

    // Draft size during the drag, full quality after release.
    const edgesOf = async () => (await calls(page, "render_preview")).filter((c) => c.args.options.slot === "main").map((c) => c.args.options.maxEdge as number);
    await expect.poll(async () => {
      const e = await edgesOf();
      return e.length > 0 && e.at(-1)! > Math.min(...e);
    }).toBe(true);
    const edges = await edgesOf();
    const full = Math.max(...edges);
    const drafts = edges.filter((e) => e < full);
    expect(drafts.length).toBeGreaterThanOrEqual(6);
    expect(edges.at(-1)).toBe(full);

    // The final displayed render is the full-quality one for the final slider value.
    const finalValue = Number(await slider.inputValue());
    await expect(page.getByTestId("view-main")).toHaveAttribute("src", new RegExp(`e=${finalValue.toFixed(2)}`));
    const last = (await calls(page, "render_preview")).filter((c) => c.args.options.slot === "main").at(-1)!;
    expect(last.args.options.maxEdge).toBe(full);
    expect((last.args.adjustments as { exposure: number }).exposure).toBeCloseTo(finalValue, 2);
  });

  test("holding still for 150 ms mid-drag requests the full-quality render", async ({ page }) => {
    await page.addInitScript(() => (window.__mockRenderDelay = () => 20));
    await openDevelop(page);
    await clearCalls(page);
    const slider = page.getByTestId("slider-exposure");
    const box = (await slider.boundingBox())!;
    const y = box.y + box.height / 2;
    await page.mouse.move(box.x + box.width * 0.5, y);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width * 0.7, y, { steps: 4 });
    await page.mouse.move(box.x + box.width * 0.75, y, { steps: 2 });
    await page.waitForTimeout(500); // pointer still down, no input
    const edges = (await calls(page, "render_preview")).filter((c) => c.args.options.slot === "main").map((c) => c.args.options.maxEdge as number);
    expect(Math.min(...edges)).toBeLessThan(Math.max(...edges));
    expect(edges.at(-1)).toBe(Math.max(...edges));
    await page.mouse.up();
  });
});
