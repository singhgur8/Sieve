import { expect, type Page } from "@playwright/test";
import { copyFileSync, mkdirSync } from "node:fs";
import path from "node:path";

export const SHOTS = path.resolve("test-results/screens");
// Key screenshots are also copied here for orchestrator review (gitignored).
const REVIEW_DIR = path.resolve(process.env.UI_SCREENS_DIR ?? "test-data/ui-screens");

export async function shot(page: Page, name: string) {
  mkdirSync(SHOTS, { recursive: true });
  mkdirSync(REVIEW_DIR, { recursive: true });
  const file = path.join(SHOTS, `${name}.png`);
  await page.screenshot({ path: file });
  copyFileSync(file, path.join(REVIEW_DIR, `${name}.png`));
}

function svg(kind: "thumb" | "preview", id: number): string {
  const hue = (id * 47) % 360;
  const w = kind === "thumb" ? 480 : 2048;
  const h = kind === "thumb" ? 320 : 1365;
  const faces =
    kind === "preview" && id % 3 !== 0
      ? [
          [0.3, 0.3],
          [0.65, 0.4],
        ]
          .map(
            ([x, y]) =>
              `<ellipse cx="${x * w}" cy="${y * h}" rx="${0.06 * w}" ry="${0.09 * h}" fill="hsl(${hue},40%,75%)"/><circle cx="${(x - 0.02) * w}" cy="${(y - 0.02) * h}" r="${0.008 * w}" fill="#222"/><circle cx="${(x + 0.02) * w}" cy="${(y - 0.02) * h}" r="${0.008 * w}" fill="#222"/>`,
          )
          .join("")
      : "";
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="hsl(${hue},55%,35%)"/><stop offset="1" stop-color="hsl(${(hue + 60) % 360},60%,18%)"/></linearGradient></defs><rect width="${w}" height="${h}" fill="url(#g)"/>${faces}<text x="50%" y="${kind === "thumb" ? "58%" : "92%"}" text-anchor="middle" font-family="Helvetica" font-size="${h * (kind === "thumb" ? 0.28 : 0.1)}" fill="white" fill-opacity="0.85">${id}</text></svg>`;
}

/** Opens the Projects home page of the mock catalog (no project open). */
export async function openHome(page: Page, count = 201, query = "") {
  await routeImages(page);
  await page.goto(`/?mock=${count}${query}`);
  await expect(page.getByTestId("home-page")).toBeVisible();
  await expect(page.getByTestId("home-loading")).toHaveCount(0);
}

/** Opens the app against the in-browser mock catalog (all photos, no project scope) and serves placeholder images. */
export async function openApp(page: Page, count = 5000, query = "") {
  await routeImages(page);
  await page.goto(`/?mock=${count}&scope=all${query}`);
  await expect(page.getByTestId("cell-1")).toBeVisible();
  await expect(page.getByTestId("cell-1").locator("img")).toBeVisible();
}

async function routeImages(page: Page) {
  await page.route(/\/mock\/(thumb|preview)\/\d+\.jpg/, (route) => {
    const m = /\/mock\/(thumb|preview)\/(\d+)\.jpg/.exec(route.request().url())!;
    return route.fulfill({ contentType: "image/svg+xml", body: svg(m[1] as "thumb" | "preview", Number(m[2])) });
  });
  await page.route(/\/mock\/render\//, (route) => {
    const u = new URL(route.request().url());
    const m = /render\/(\d+)\/(\w+)/.exec(u.pathname)!;
    const e = Number(u.searchParams.get("e") ?? 0);
    const light = Math.max(5, Math.min(95, 40 + e * 12));
    const hue = (Number(m[1]) * 47) % 360;
    const lut = u.searchParams.get("lut");
    const W = u.searchParams.get("p") ? 800 : 1200;
    const H = u.searchParams.get("p") ? 1200 : 800;
    const body = `<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}" viewBox="0 0 ${W} ${H}"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="hsl(${m[2] === "before" ? 200 : hue},50%,${m[2] === "before" ? 40 : light}%)"/><stop offset="1" stop-color="hsl(${(hue + 60) % 360},55%,${light * 0.5}%)"/></linearGradient></defs><rect width="${W}" height="${H}" fill="url(#g)"/><text x="50%" y="55%" text-anchor="middle" font-family="Helvetica" font-size="90" fill="white" fill-opacity="0.9">${m[1]} ${m[2]} EV ${e}${lut ? " " + lut : ""}</text></svg>`;
    return route.fulfill({ contentType: "image/svg+xml", body });
  });
}

/** Analyze menu -> Detect scenes. */
export async function detectScenes(page: Page) {
  await page.getByTestId("analyze-menu").click();
  await page.getByTestId("scenes-detect").click();
}

/** Opens the Scene menu when needed and returns the item; call `closeMenus` after assertions on it. */
export async function sceneItem(page: Page, testid: string) {
  if (!(await page.getByTestId(testid).isVisible())) await page.getByTestId("scene-menu").click();
  return page.getByTestId(testid);
}

export async function closeMenus(page: Page) {
  if ((await page.getByRole("menu").count()) > 0) await page.keyboard.press("Escape");
  await expect(page.getByRole("menu")).toHaveCount(0);
}

export async function calls(page: Page, cmd: string) {
  return page.evaluate((c) => window.__ipcLog.filter((x) => x.cmd === c), cmd) as Promise<{ cmd: string; args: Record<string, any> }[]>;
}

export async function clearCalls(page: Page) {
  await page.evaluate(() => (window.__ipcLog.length = 0));
}

declare global {
  interface Window {
    __ipcLog: { cmd: string; args: Record<string, unknown> }[];
  }
}

/** Opens a Develop right-panel section (Basic is open by default; the others start closed). */
export async function openSection(page: Page, id: string) {
  const sec = page.getByTestId(`section-${id}`);
  if ((await sec.getAttribute("data-open")) !== "true") await page.getByTestId(`section-toggle-${id}`).click();
  await expect(sec).toHaveAttribute("data-open", "true");
}
