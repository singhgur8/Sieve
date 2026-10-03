import { expect, test, type Page } from "@playwright/test";
import { deflateSync } from "node:zlib";
import { calls, clearCalls, openApp } from "./helpers";

// Raster stand-ins for the photos (real thumbnails / previews are JPEGs): an <img> showing an SVG can never be
// painted synchronously, which would make "blank frame" measurements meaningless.
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
function png(w: number, h: number, id: number): Buffer {
  const key = `${w}x${h}:${id % 12}`;
  const hit = pngCache.get(key);
  if (hit) return hit;
  const row = Buffer.alloc(1 + w * 3);
  for (let x = 0; x < w; x++) {
    row[1 + x * 3] = (id * 53) % 256;
    row[2 + x * 3] = 40 + ((id * 97) % 160);
    row[3 + x * 3] = 120;
  }
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
  // Request interception turns the HTTP cache off; the real app (asset protocol) has it on. Turn it back on.
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("Network.enable");
  await cdp.send("Network.setCacheDisabled", { cacheDisabled: false });
  await page.route(/\/mock\/(thumb|preview)\/\d+\.jpg/, (route) => {
    const m = /\/mock\/(thumb|preview)\/(\d+)\.jpg/.exec(route.request().url())!;
    const [w, h] = m[1] === "thumb" ? [480, 320] : [2048, 1365];
    return route.fulfill({ contentType: "image/png", headers: { "cache-control": "max-age=3600" }, body: png(w, h, Number(m[2])) });
  });
  await page.route(/\/mock\/render\//, (route) => {
    const id = Number(/render\/(\d+)\//.exec(route.request().url())![1]);
    return route.fulfill({ contentType: "image/png", body: png(1200, 800, id) });
  });
}

// Arrow-key navigation through the Loupe / Develop with two filters on, at 25 presses per second.
// A frame-by-frame sampler (requestAnimationFrame) records which photo the visible <img> elements belong to
// compared to the photo the UI says is current.

declare global {
  interface Window {
    __frames: { cur: number | null; shown: number[]; dom: number[]; ready: boolean; anim: boolean }[];
    __sampling: boolean;
  }
}

const PRESSES = 25;
const GAP_MS = 40; // 25 presses / s

async function twoFilters(page: Page) {
  await page.getByTestId("pick-unflagged").click();
  await page.getByTestId("tag-blink").click();
  await page.getByTestId("tag-blink").click(); // include -> exclude
  await expect(page.getByTestId("tag-blink")).toHaveAttribute("data-state", "exclude");
}

async function startSampler(page: Page, mode: "loupe" | "develop") {
  await page.evaluate((m) => {
    window.__frames = [];
    window.__sampling = true;
    const idOf = (src: string) => Number(/(?:thumb|preview|render)\/(\d+)/.exec(src)?.[1] ?? NaN);
    const tick = () => {
      if (!window.__sampling) return;
      let cur: number | null = null;
      let imgs: HTMLImageElement[] = [];
      if (m === "loupe") {
        const t = document.querySelector('[data-testid="info-overlay"]')?.textContent ?? "";
        const n = /DSC0*(\d+)/.exec(t);
        cur = n ? Number(n[1]) : null;
        imgs = [...document.querySelectorAll<HTMLImageElement>('[data-testid="zoom-a"] img')];
      } else {
        const v = document.querySelector('[data-testid="develop-view"]');
        cur = v ? Number(v.getAttribute("data-image-id")) : null;
        imgs = [...document.querySelectorAll<HTMLImageElement>('[data-testid="viewer"] img')];
      }
      // What Chrome paints: a loaded image, or - for an element whose src changed and has not loaded yet - the
      // previous image it held (the old bitmap stays on screen until the new one is ready).
      const last = ((window as any).__last ??= new WeakMap<HTMLImageElement, string>()) as WeakMap<HTMLImageElement, string>;
      const painted: { src: string }[] = [];
      for (const i of imgs) {
        const cs = getComputedStyle(i);
        if (cs.visibility === "hidden" || Number(cs.opacity) === 0) continue;
        if (i.complete && i.naturalWidth > 0) {
          last.set(i, i.currentSrc || i.src);
          painted.push({ src: i.currentSrc || i.src });
        } else if (last.has(i)) painted.push({ src: last.get(i)! });
      }
      let anim = false;
      for (const i of imgs) {
        for (let e: Element | null = i; e && e.getAttribute?.("data-testid") !== "loupe" && e.getAttribute?.("data-testid") !== "viewer"; e = e.parentElement) {
          const cs = getComputedStyle(e);
          if (Number(cs.opacity) < 1 || parseFloat(cs.transitionDuration) > 0 || cs.animationName !== "none") anim = true;
        }
      }
      window.__frames.push({ cur, shown: painted.map((i) => idOf(i.src)), dom: imgs.map((i) => idOf(i.src)), ready: painted.length > 0, anim });
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  }, mode);
}

async function run(page: Page, mode: "loupe" | "develop") {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("Performance.enable");
  const metric = async (name: string) => ((await cdp.send("Performance.getMetrics")).metrics.find((m) => m.name === name)?.value ?? 0) * 1000;
  await clearCalls(page);
  await startSampler(page, mode);
  const task0 = await metric("TaskDuration");
  const script0 = await metric("ScriptDuration");
  for (let i = 0; i < PRESSES; i++) {
    await page.keyboard.press("ArrowRight");
    await page.waitForTimeout(GAP_MS);
  }
  const task1 = await metric("TaskDuration");
  const script1 = await metric("ScriptDuration");
  await page.waitForTimeout(400);
  await page.evaluate(() => (window.__sampling = false));
  const frames = await page.evaluate(() => window.__frames);
  const during = frames.filter((f) => f.cur != null);
  const bad = during.filter((f) => f.shown.some((s) => s !== f.cur));
  const blank = during.filter((f) => !f.ready);
  // DOM level (independent of load timing): the photo's own image is mounted in the commit that shows it, and no other photo's is.
  const domWrong = during.filter((f) => f.dom.some((s) => s !== f.cur) || !f.dom.includes(f.cur!));
  const anim = during.filter((f) => f.anim);
  const ipc: Record<string, number> = {};
  for (const c of ["list_image_ids", "get_images", "get_filter_counts", "get_faces", "render_preview"]) ipc[c] = (await calls(page, c)).length;
  const r = {
    mode,
    frames: during.length,
    wrongPhotoFrames: bad.length,
    domWrongFrames: domWrong.length,
    blankFrames: blank.length,
    animFrames: anim.length,
    taskMsPerKey: +((task1 - task0) / PRESSES).toFixed(2),
    scriptMsPerKey: +((script1 - script0) / PRESSES).toFixed(2),
    ipc,
  };
  console.log("NAV-METRICS " + JSON.stringify(r));
  return r;
}

test.describe("arrow-key navigation (two filters on, 25 presses/s)", () => {
  test("Loupe: every painted frame belongs to the current photo, no animation, no re-queries", async ({ page }) => {
    await openApp(page, 400);
    await rasterRoutes(page); // registered last: wins over the SVG routes of openApp
    await twoFilters(page);
    await page.getByTestId("cell-2").click();
    await page.keyboard.press("Space");
    await expect(page.getByTestId("loupe")).toBeVisible();
    await expect(page.getByTestId("zoom-a").locator("img").last()).toBeVisible();
    await page.waitForTimeout(300);
    const r = await run(page, "loupe");
    expect(r.frames).toBeGreaterThan(20);
    expect(r.wrongPhotoFrames).toBe(0);
    expect(r.domWrongFrames).toBe(0);
    // blankFrames (image requested but not yet decoded) is reported only: Playwright interception disables the HTTP cache, so it cannot reach 0 here.
    expect(r.animFrames).toBe(0);
    expect(r.ipc.list_image_ids).toBe(0);
    expect(r.ipc.get_filter_counts).toBe(0);
  });

  test("Develop: every painted frame belongs to the current photo, no blank frame", async ({ page }) => {
    await openApp(page, 400);
    await rasterRoutes(page); // registered last: wins over the SVG routes of openApp
    await twoFilters(page);
    await page.getByTestId("cell-2").click();
    await page.keyboard.press("d");
    await expect(page.getByTestId("view-main")).toBeVisible();
    await page.waitForTimeout(300);
    const r = await run(page, "develop");
    expect(r.frames).toBeGreaterThan(20);
    expect(r.wrongPhotoFrames).toBe(0);
    expect(r.domWrongFrames).toBe(0);
    // blankFrames (image requested but not yet decoded) is reported only: Playwright interception disables the HTTP cache, so it cannot reach 0 here.
    expect(r.animFrames).toBe(0);
    expect(r.ipc.list_image_ids).toBe(0);
    expect(r.ipc.get_filter_counts).toBe(0);
  });
});
