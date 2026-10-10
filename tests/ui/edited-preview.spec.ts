import { expect, test, type Page } from "@playwright/test";
import { deflateSync } from "node:zlib";
import { openApp } from "./helpers";

// IPC v19.1 edited previews: edited photos show their edited render everywhere, and switching between edited photos
// in Develop never paints the unedited (embedded) preview of an edited photo while its render is pending.

function crc32(buf: Buffer): number {
  let c = ~0;
  for (const b of buf) {
    c ^= b;
    for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1));
  }
  return ~c >>> 0;
}
function chunk(type: string, data: Buffer): Buffer {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
}
const pngCache = new Map<string, Buffer>();
/** Raster stand-in (an SVG <img> never paints synchronously, which would hide flashes). */
function png(w: number, h: number, rgb: [number, number, number]): Buffer {
  const key = `${w}x${h}:${rgb.join(",")}`;
  const hit = pngCache.get(key);
  if (hit) return hit;
  const row = Buffer.alloc(1 + w * 3);
  for (let x = 0; x < w; x++) row.set(rgb, 1 + x * 3);
  const raw = Buffer.concat(Array.from({ length: h }, () => row));
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8;
  ihdr[9] = 2;
  const out = Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk("IHDR", ihdr), chunk("IDAT", deflateSync(raw)), chunk("IEND", Buffer.alloc(0))]);
  pngCache.set(key, out);
  return out;
}

async function rasterRoutes(page: Page) {
  // Interception disables the HTTP cache; the real app (asset protocol, immutable sieve:// URLs) has it on.
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("Network.enable");
  await cdp.send("Network.setCacheDisabled", { cacheDisabled: false });
  const cache = { "cache-control": "max-age=3600" };
  await page.route(/\/mock\/(thumb|preview)\/\d+\.jpg/, (route) => {
    const m = /\/mock\/(thumb|preview)\/(\d+)\.jpg/.exec(route.request().url())!;
    const [w, h] = m[1] === "thumb" ? [480, 320] : [2048, 1365];
    return route.fulfill({ contentType: "image/png", headers: cache, body: png(w, h, [60, 60, 60]) });
  });
  await page.route(/\/mock\/edited\/\d+\//, (route) => {
    const m = /edited\/\d+\/\w+\/(thumb|preview)\.jpg/.exec(route.request().url())!;
    const [w, h] = m[1] === "thumb" ? [480, 320] : [2048, 1365];
    return route.fulfill({ contentType: "image/png", headers: cache, body: png(w, h, [200, 160, 90]) });
  });
  await page.route(/\/mock\/render\//, (route) => route.fulfill({ contentType: "image/png", body: png(1200, 800, [190, 150, 80]) }));
}

const invoke = <T>(page: Page, cmd: string, args: Record<string, unknown>) =>
  page.evaluate(([c, a]) => (window as unknown as { __TAURI_INTERNALS__: { invoke: (c: string, a: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke(c as string, a), [cmd, args] as const) as Promise<T>;

async function editViaBackend(page: Page, ids: number[]) {
  for (const id of ids) {
    const a = await invoke<Record<string, unknown>>(page, "get_adjustments", { id });
    await invoke(page, "save_adjustments", { id, adjustments: { ...a, exposure: 1 }, label: "Exposure +1.00" });
  }
}

declare global {
  interface Window {
    __edFrames: { cur: number; shown: string[] }[];
    __edSampling: boolean;
  }
}

test.describe("edited previews (IPC v19.1)", () => {
  test("Develop: switching between edited photos on a slow backend never paints an unedited preview", async ({ page }) => {
    await openApp(page, 400);
    await rasterRoutes(page);
    const edited = Array.from({ length: 12 }, (_, i) => i + 1);
    await editViaBackend(page, edited);
    await page.waitForTimeout(400); // mock edited renders land (120 ms)
    // Slow renders: every switch shows the placeholder for a while.
    await page.evaluate(() => (window.__mockRenderDelay = () => 700));
    await page.getByTestId("cell-2").click();
    await page.keyboard.press("d");
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "2");

    await page.evaluate(() => {
      window.__edFrames = [];
      window.__edSampling = true;
      const last = new WeakMap<HTMLImageElement, string>();
      const tick = () => {
        if (!window.__edSampling) return;
        const v = document.querySelector('[data-testid="develop-view"]');
        const cur = Number(v?.getAttribute("data-image-id"));
        const shown: string[] = [];
        for (const i of document.querySelectorAll<HTMLImageElement>('[data-testid="viewer"] img')) {
          const cs = getComputedStyle(i);
          if (cs.visibility === "hidden" || Number(cs.opacity) === 0) continue;
          // What is painted: a loaded image, or the previous bitmap of an element whose new src is still loading.
          if (i.complete && i.naturalWidth > 0) {
            last.set(i, i.currentSrc || i.src);
            shown.push(i.currentSrc || i.src);
          } else if (last.has(i)) shown.push(last.get(i)!);
        }
        window.__edFrames.push({ cur, shown });
        requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    });
    // Forward and back through edited photos, faster than renders land, then slower than them.
    for (const [key, gap] of [["ArrowRight", 250], ["ArrowRight", 250], ["ArrowRight", 250], ["ArrowLeft", 250], ["ArrowLeft", 900], ["ArrowRight", 900], ["ArrowRight", 120], ["ArrowRight", 120]] as const) {
      await page.keyboard.press(key);
      await page.waitForTimeout(gap);
    }
    await page.waitForTimeout(900);
    await page.evaluate(() => (window.__edSampling = false));
    const frames = await page.evaluate(() => window.__edFrames);
    const kind = (src: string) => (/\/mock\/edited\//.test(src) ? "edited" : /\/mock\/render\//.test(src) ? "render" : /\/mock\/(thumb|preview)\//.test(src) ? "unedited" : "other");
    const during = frames.filter((f) => edited.includes(f.cur));
    const unedited = during.filter((f) => f.shown.some((s) => kind(s) === "unedited"));
    const editedPlaceholder = during.filter((f) => f.shown.some((s) => kind(s) === "edited"));
    const blank = during.filter((f) => f.shown.length === 0);
    console.log("EDITED-NAV " + JSON.stringify({ frames: during.length, unedited: unedited.length, editedPlaceholder: editedPlaceholder.length, blank: blank.length }));
    expect(during.length).toBeGreaterThan(60);
    expect(unedited.length).toBe(0);
    // The cached edited preview is what fills the gap while the slow render is pending.
    expect(editedPlaceholder.length).toBeGreaterThan(10);
    // Never the previous photo's edited preview either.
    const wrong = during.filter((f) => f.shown.some((s) => kind(s) === "edited" && !s.includes(`/edited/${f.cur}/`)));
    expect(wrong.length).toBe(0);
  });

  test("an edited photo whose edited render is not ready yet shows no unedited placeholder in Develop", async ({ page }) => {
    await openApp(page, 400);
    await rasterRoutes(page);
    await page.evaluate(() => {
      window.__mockEditedDelay = 60_000; // edited renders never land in this test
      window.__mockRenderDelay = () => 800;
    });
    await editViaBackend(page, [3]);
    await page.getByTestId("cell-2").click();
    await page.keyboard.press("d");
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "2");
    await page.keyboard.press("ArrowRight");
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "3");
    // Until the render lands: spinner, no placeholder (an embedded preview would show the photo without its edits).
    await expect(page.getByTestId("view-placeholder")).toHaveCount(0);
    await expect(page.getByTestId("view-main")).toBeVisible({ timeout: 5000 });
  });

  test("Library grid, filmstrip and Loupe show the edited render after an edit in Develop, live", async ({ page }) => {
    await openApp(page, 200);
    await rasterRoutes(page);
    const cellImg = page.getByTestId("cell-3").locator("img");
    await expect(cellImg).toHaveAttribute("src", /\/mock\/thumb\/3\.jpg/);
    await page.getByTestId("cell-3").click();
    await page.keyboard.press("d");
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "3");
    await expect(page.getByTestId("view-main")).toBeVisible();
    const s = page.getByTestId("slider-exposure");
    await s.fill("1.2");
    await s.evaluate((el) => (el as HTMLElement).blur());
    // The filmstrip follows live (event), while still in Develop.
    await expect(page.getByTestId("film-3").locator("img")).toHaveAttribute("src", /\/mock\/edited\/3\/\w+\/thumb\.jpg/);
    await page.keyboard.press("g");
    await expect(cellImg).toHaveAttribute("src", /\/mock\/edited\/3\/\w+\/thumb\.jpg/);
    const first = await cellImg.getAttribute("src");
    // Loupe: the edited 2048 render.
    await page.getByTestId("cell-3").click();
    await page.keyboard.press("Space");
    await expect(page.getByTestId("zoom-a").locator("img").last()).toHaveAttribute("src", /\/mock\/edited\/3\/\w+\/preview\.jpg/);
    await page.keyboard.press("g");
    // Another edit gives a new content-addressed URL; resetting goes back to the embedded thumbnail.
    const a = await invoke<Record<string, unknown>>(page, "get_adjustments", { id: 3 });
    await invoke(page, "save_adjustments", { id: 3, adjustments: { ...a, exposure: -0.5 }, label: "Exposure -0.50" });
    await expect(cellImg).not.toHaveAttribute("src", first!);
    await expect(cellImg).toHaveAttribute("src", /\/mock\/edited\/3\//);
    await invoke(page, "save_adjustments", { id: 3, adjustments: { ...a, exposure: 0 }, label: "Reset" });
    await expect(cellImg).toHaveAttribute("src", /\/mock\/thumb\/3\.jpg/);
  });
});
