import { expect, test, type Page } from "@playwright/test";
import { openApp } from "./helpers";

// Phase 8d "slider friction": input is handled independently of rendering. During a 2 s drag with a slow render
// (mock delay), measure pointer-to-value-text latency, main-thread long tasks and renders shown per second.

async function openDevelop(page: Page) {
  await openApp(page, 200);
  await page.getByTestId("cell-1").click();
  await page.keyboard.press("d");
  await expect(page.getByTestId("develop-view")).toBeVisible();
  await expect(page.getByTestId("view-main")).toBeVisible();
  await expect(page.getByTestId("histogram")).toHaveAttribute("data-empty", "false");
}

interface Probe {
  lat: number[];
  long: number[];
  shown: number;
  alt?: string;
}

for (const delay of [40, 80]) {
  test(`2 s drag with ${delay} ms renders: thumb follows the pointer, renders keep landing`, async ({ page }) => {
    await page.addInitScript((d) => (window.__mockRenderDelay = () => d), delay);
    await openDevelop(page);
    await page.evaluate(() => {
      const w = window as unknown as { __p: Probe };
      const p: Probe = (w.__p = { lat: [], long: [], shown: 0 });
      let moves: number[] = [];
      const text = document.querySelector('[data-testid="slider-value-exposure"]')!;
      let last = text.textContent;
      // Time at which the value text shows the new pointer position, measured from the input event timestamp.
      new MutationObserver(() => {
        if (text.textContent === last) return;
        last = text.textContent;
        const now = performance.now();
        for (const t of moves) p.lat.push(now - t);
        moves = [];
      }).observe(text, { childList: true, characterData: true, subtree: true });
      window.addEventListener("pointermove", (e) => moves.push(e.timeStamp), { capture: true });
      new PerformanceObserver((l) => l.getEntries().forEach((e) => p.long.push(e.duration))).observe({ type: "longtask" });
      new MutationObserver(() => {
        const alt = document.querySelector('[data-testid="view-main"]')?.getAttribute("alt");
        if (alt && alt !== p.alt) {
          p.alt = alt;
          p.shown++;
        }
      }).observe(document.querySelector('[data-testid="viewer"]')!, { subtree: true, attributes: true, childList: true, attributeFilter: ["alt", "src"] });
    });
    const box = (await page.getByTestId("slider-exposure").boundingBox())!;
    const y = box.y + box.height / 2;
    await page.mouse.move(box.x + box.width * 0.5, y);
    await page.mouse.down();
    const t0 = Date.now();
    let n = 0;
    while (Date.now() - t0 < 2000) {
      n++;
      await page.mouse.move(box.x + box.width * (0.5 + 0.4 * Math.sin(n / 14)), y);
      await page.waitForTimeout(8);
    }
    const secs = (Date.now() - t0) / 1000;
    const p = await page.evaluate(() => (window as unknown as { __p: Probe }).__p);
    await page.mouse.up();
    const sorted = [...p.lat].sort((a, b) => a - b);
    const p95 = sorted[Math.floor(sorted.length * 0.95)] ?? -1;
    const p50 = sorted[Math.floor(sorted.length * 0.5)] ?? -1;
    const longTotal = p.long.reduce((a, b) => a + b, 0);
    console.log(`SMOOTH delay=${delay} moves=${n} samples=${sorted.length} latP50=${p50.toFixed(1)} latP95=${p95.toFixed(1)} longTasks=${p.long.length} longMs=${longTotal.toFixed(0)} shown/s=${(p.shown / secs).toFixed(1)}`);
    expect(p95).toBeLessThan(20);
    if (delay === 40) expect(p.shown / secs).toBeGreaterThanOrEqual(15);
  });
}

test("the Navigator image stays frozen while a slider is dragged and follows the settled render after release", async ({ page }) => {
  await page.addInitScript(() => (window.__mockRenderDelay = () => 30));
  await openDevelop(page);
  const nav = page.getByTestId("navigator-img");
  await expect(nav).toBeVisible();
  const before = await nav.getAttribute("src");
  const box = (await page.getByTestId("slider-exposure").boundingBox())!;
  const y = box.y + box.height / 2;
  await page.mouse.move(box.x + box.width * 0.5, y);
  await page.mouse.down();
  for (let i = 1; i <= 25; i++) {
    await page.mouse.move(box.x + box.width * (0.5 + 0.004 * i), y);
    await page.waitForTimeout(10);
  }
  expect(await nav.getAttribute("src")).toBe(before);
  await page.mouse.up();
  await expect.poll(() => nav.getAttribute("src")).not.toBe(before);
});
