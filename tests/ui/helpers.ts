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

/** Opens the app against the in-browser mock catalog and serves placeholder images. */
export async function openApp(page: Page, count = 5000) {
  await page.route(/\/mock\/(thumb|preview)\/\d+\.jpg/, (route) => {
    const m = /\/mock\/(thumb|preview)\/(\d+)\.jpg/.exec(route.request().url())!;
    return route.fulfill({ contentType: "image/svg+xml", body: svg(m[1] as "thumb" | "preview", Number(m[2])) });
  });
  await page.goto(`/?mock=${count}`);
  await expect(page.getByTestId("cell-1")).toBeVisible();
  await expect(page.getByTestId("cell-1").locator("img")).toBeVisible();
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
