// Phase 8b frontend fixes: Compare view (Library + Develop), masks UX, clickable stars, scenes toggle, size slider.
// Mock ratings: id 11 = 2 stars, id 16 = 3, id 21 = 4, id 26 = 5 (every fifth photo, see mockBackend).
import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, detectScenes, openApp, shot } from "./helpers";

const P = "11a-";
const N = 2000;

async function setSlider(page: Page, id: string, value: number) {
  const s = page.getByTestId(`slider-${id}`);
  await s.fill(String(value));
  await s.evaluate((el) => (el as HTMLElement).blur());
}
const saves = async (page: Page) => (await calls(page, "save_adjustments")).filter((c) => c.args.label !== undefined);

test.describe("clickable stars", () => {
  test("grid cell: click sets, current rating clears, hover previews, selection does not change", async ({ page }) => {
    await openApp(page, N);
    await page.getByTestId("cell-7").click();
    await expect(page.getByTestId("cell-7")).toHaveAttribute("data-active", "true");
    await clearCalls(page);

    // Hover preview: 4 stars light up while the rating is still 2.
    await page.getByTestId("stars-cell-11-4").hover();
    await expect(page.getByTestId("stars-cell-11-4")).toHaveAttribute("data-filled", "true");
    await expect(page.getByTestId("stars-cell-11-5")).toHaveAttribute("data-filled", "false");
    await expect(page.getByTestId("cell-11")).toHaveAttribute("data-rating", "2");
    await shot(page, `${P}stars-hover`);

    await page.getByTestId("stars-cell-11-4").click();
    await expect.poll(async () => (await calls(page, "set_rating")).length).toBe(1);
    expect((await calls(page, "set_rating"))[0].args).toEqual({ ids: [11], rating: 4 });
    await expect(page.getByTestId("cell-11")).toHaveAttribute("data-rating", "4");
    // No accidental selection change / loupe.
    await expect(page.getByTestId("cell-7")).toHaveAttribute("data-active", "true");
    await expect(page.getByTestId("cell-11")).toHaveAttribute("data-selected", "false");
    await expect(page.getByTestId("grid-scroll")).toBeVisible();

    // Clicking the current rating clears it: the stars disappear from the cell.
    await clearCalls(page);
    await page.getByTestId("stars-cell-11-4").click();
    await expect.poll(async () => (await calls(page, "set_rating")).length).toBe(1);
    expect((await calls(page, "set_rating"))[0].args).toEqual({ ids: [11], rating: 0 });
    await expect(page.getByTestId("cell-11")).toHaveAttribute("data-rating", "0");
    await expect(page.getByTestId("stars-cell-11")).toHaveCount(0);
  });

  test("loupe overlay, Develop toolbar and filmstrip stars are clickable", async ({ page }) => {
    await openApp(page, N);
    await page.getByTestId("cell-11").click();
    await page.keyboard.press("Space");
    await expect(page.getByTestId("loupe")).toBeVisible();
    await clearCalls(page);
    await page.getByTestId("loupe-stars-11-5").click();
    await expect.poll(async () => (await calls(page, "set_rating")).length).toBe(1);
    expect((await calls(page, "set_rating"))[0].args).toEqual({ ids: [11], rating: 5 });
    await expect(page.getByTestId("loupe")).toBeVisible(); // still in the loupe
    await shot(page, `${P}stars-loupe`);

    // Filmstrip badge of another photo (id 16 has 3 stars and is near the strip's centre).
    await clearCalls(page);
    await page.getByTestId("film-stars-16-1").click();
    await expect.poll(async () => (await calls(page, "set_rating")).length).toBe(1);
    expect((await calls(page, "set_rating"))[0].args).toEqual({ ids: [16], rating: 1 });
    await expect(page.getByTestId("zoom-a")).toBeVisible();
    await expect(page.getByTestId("info-overlay")).toContainText("00011"); // the loupe did not move to 16

    await page.keyboard.press("d");
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "11");
    await clearCalls(page);
    await page.getByTestId("develop-stars-3").click();
    await expect.poll(async () => (await calls(page, "set_rating")).length).toBe(1);
    expect((await calls(page, "set_rating"))[0].args).toEqual({ ids: [11], rating: 3 });
    await page.getByTestId("develop-stars-3").click(); // current rating: clears
    await expect.poll(async () => (await calls(page, "set_rating")).length).toBe(2);
    expect((await calls(page, "set_rating"))[1].args).toEqual({ ids: [11], rating: 0 });
    await shot(page, `${P}stars-develop`);
  });
});

test.describe("scenes toggle", () => {
  test("toggle button and Shift+S hide the strip and clear the scene filter", async ({ page }) => {
    await openApp(page, 200);
    await expect(page.getByTestId("scenes-toggle")).toHaveCount(0); // nothing to toggle before detection
    await detectScenes(page);
    await expect(page.getByTestId("scene-chip-6")).toBeVisible();
    await page.getByTestId("scene-chip-2").click();
    await expect(page.getByTestId("scene-chip-2")).toHaveAttribute("data-active", "true");
    await expect.poll(async () => ((await calls(page, "list_image_ids")).at(-1)!.args.query as { sceneId: number | null }).sceneId).toBe(2);
    await shot(page, `${P}scenes-on`);

    await page.getByTestId("scenes-toggle").click();
    await expect(page.getByTestId("scene-strip")).toHaveCount(0);
    await expect.poll(async () => ((await calls(page, "list_image_ids")).at(-1)!.args.query as { sceneId: number | null }).sceneId).toBeNull();
    await expect(page.getByTestId("scenes-toggle")).toHaveAttribute("aria-pressed", "false");
    await shot(page, `${P}scenes-off`);

    // Shift+S brings it back, with no filter applied.
    await page.keyboard.press("Shift+S");
    await expect(page.getByTestId("scene-strip")).toBeVisible();
    await expect(page.getByTestId("scene-chip-all")).toHaveClass(/bg-sky-800/);

    // ...and hides it again, clearing a newly chosen filter; the strip's own Hide button does the same.
    await page.getByTestId("scene-chip-3").click();
    await page.keyboard.press("Shift+S");
    await expect(page.getByTestId("scene-strip")).toHaveCount(0);
    await expect.poll(async () => ((await calls(page, "list_image_ids")).at(-1)!.args.query as { sceneId: number | null }).sceneId).toBeNull();
    await page.getByTestId("scenes-toggle").click();
    await page.getByTestId("scene-chip-4").click();
    await page.getByTestId("scene-strip-hide").click();
    await expect(page.getByTestId("scene-strip")).toHaveCount(0);
  });
});

test.describe("size slider", () => {
  test("track and thumb are visible on the dark theme (>= 3:1)", async ({ page }) => {
    await openApp(page, 200);
    // Pseudo-elements of the native range cannot be read with getComputedStyle, so read the stylesheet rules.
    const r = await page.getByTestId("thumb-size").evaluate((el) => {
      const lum = (css: string) => {
        const m = css.match(/[\d.]+/g)!.map(Number);
        const [R, G, B] = m.slice(0, 3).map((v) => {
          const c = v / 255;
          return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
        });
        return 0.2126 * R + 0.7152 * G + 0.0722 * B;
      };
      const rule = (sel: string) => {
        for (const sheet of [...document.styleSheets])
          for (const r of [...sheet.cssRules]) if (r instanceof CSSStyleRule && r.selectorText.includes(sel)) return r.style;
        return null;
      };
      const probe = document.createElement("i");
      const toRgb = (c: string) => {
        probe.style.color = c;
        document.body.append(probe);
        const v = getComputedStyle(probe).color;
        probe.remove();
        return v;
      };
      const track = toRgb(rule("solid-range::-webkit-slider-runnable-track")!.backgroundColor);
      const thumb = toRgb(rule("solid-range::-webkit-slider-thumb")!.backgroundColor);
      const bg = toRgb("#0a0a0a"); // body background (neutral-950)
      const ratio = (a: string, b: string) => {
        const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p);
        return (x + 0.05) / (y + 0.05);
      };
      return { cls: el.className, track, thumb, bg, trackRatio: ratio(track, bg), thumbRatio: ratio(thumb, bg) };
    });
    expect(r.trackRatio, JSON.stringify(r)).toBeGreaterThanOrEqual(3);
    expect(r.thumbRatio, JSON.stringify(r)).toBeGreaterThanOrEqual(3);
    await page.getByTestId("thumb-size").hover();
    await shot(page, `${P}size-slider`);
  });
});

async function openMasks(page: Page, id = 1, height = 900) {
  await page.setViewportSize({ width: 1440, height });
  await openApp(page, 200);
  await page.getByTestId(`cell-${id}`).click();
  await page.keyboard.press("d");
  await expect(page.getByTestId("view-main")).toBeVisible();
  await expect(page.getByTestId("histogram")).toHaveAttribute("data-empty", "false");
  await page.keyboard.press("Shift+W");
  await expect(page.getByTestId("masks-panel")).toBeVisible();
}
async function createSubject(page: Page) {
  await page.getByTestId("mask-create-subject").click();
  await expect(page.locator('[data-testid^="mask-group-"][data-selected="true"]')).toHaveCount(1);
  await expect(page.getByTestId("mask-busy")).toHaveCount(0);
}

test.describe("masks UX", () => {
  test("overlay is dark red, shows while adjusting, fades out, and O pins it", async ({ page }) => {
    await openMasks(page);
    await createSubject(page);
    // Just created: the overlay flashes, then fades away (nothing pinned).
    await expect(page.getByTestId("mask-overlay")).toHaveAttribute("data-overlay-visible", "true");
    const tint = await page.getByTestId("mask-overlay-tint").evaluate((el) => ({ bg: getComputedStyle(el).backgroundColor, op: Number(getComputedStyle(el).opacity) }));
    expect(tint.bg).toBe("rgb(192, 0, 0)"); // #c00000
    expect(tint.op).toBeGreaterThanOrEqual(0.55);
    expect(tint.op).toBeLessThanOrEqual(0.6);
    await shot(page, `${P}overlay-auto-visible`);
    await expect(page.getByTestId("mask-overlay")).toHaveCount(0, { timeout: 4000 });
    await shot(page, `${P}overlay-auto-hidden`);

    // Adjusting a mask slider brings it back; ~600 ms after the release it is gone again.
    await clearCalls(page);
    await setSlider(page, "mask-exposure", 1);
    await expect(page.getByTestId("mask-overlay")).toHaveAttribute("data-overlay-visible", "true");
    const t0 = Date.now();
    await expect(page.getByTestId("mask-overlay")).toHaveAttribute("data-overlay-visible", "false", { timeout: 3000 });
    expect(Date.now() - t0).toBeGreaterThanOrEqual(300);
    await expect(page.getByTestId("mask-overlay")).toHaveCount(0, { timeout: 3000 });

    // Show overlay (O) pins it: stays after editing.
    await page.keyboard.press("o");
    await expect(page.getByTestId("mask-overlay-toggle")).toBeChecked();
    await expect(page.getByTestId("mask-overlay")).toHaveAttribute("data-overlay-pinned", "true");
    await setSlider(page, "mask-exposure", 2);
    await page.waitForTimeout(1200);
    await expect(page.getByTestId("mask-overlay")).toHaveAttribute("data-overlay-visible", "true");
    await shot(page, `${P}overlay-pinned`);
    await page.keyboard.press("o");
    await expect(page.getByTestId("mask-overlay")).toHaveCount(0, { timeout: 3000 });
  });

  test("Add / Subtract / Intersect menus open in a portal, inside the window, and their items are clickable", async ({ page }) => {
    // A short window makes the mask list area tiny, which used to crop the menus.
    await openMasks(page, 1, 640);
    await createSubject(page);
    const gid = (await page.locator('[data-testid^="mask-group-"][data-selected="true"]').getAttribute("data-testid"))!.replace("mask-group-", "");
    await expect(page.getByTestId(`mask-add-row-${gid}`)).toBeVisible();
    for (const mode of ["add", "subtract", "intersect"] as const) {
      await page.getByTestId(`mask-${mode}-${gid}`).click();
      const menu = page.getByRole("menu");
      await expect(menu).toBeVisible();
      await expect(menu).toHaveAttribute("data-placed", "true");
      // Rendered under <body>, not inside the panel.
      expect(await menu.evaluate((el) => el.parentElement === document.body)).toBe(true);
      const b = (await menu.boundingBox())!;
      const vp = page.viewportSize()!;
      expect(b.x).toBeGreaterThanOrEqual(0);
      expect(b.y).toBeGreaterThanOrEqual(0);
      expect(b.x + b.width).toBeLessThanOrEqual(vp.width);
      expect(b.y + b.height).toBeLessThanOrEqual(vp.height);
      // The item is what a click at its centre actually hits (nothing crops or covers it).
      const item = page.getByTestId(`mask-${mode}-item-linear`);
      const ib = (await item.boundingBox())!;
      const hit = await page.evaluate(([x, y]) => document.elementFromPoint(x, y)?.closest('[data-testid^="mask-"]')?.getAttribute("data-testid"), [ib.x + ib.width / 2, ib.y + ib.height / 2]);
      expect(hit).toBe(`mask-${mode}-item-linear`);
      if (mode === "subtract") await shot(page, `${P}mask-menu-subtract`);
      await page.keyboard.press("Escape");
      await expect(menu).toHaveCount(0);
    }
    await page.getByTestId(`mask-intersect-${gid}`).click();
    await page.getByTestId("mask-intersect-item-linear").click();
    await expect(page.getByTestId("mask-tool-badge")).toHaveAttribute("data-tool", "linear");
  });
});

/** Library Compare of photos `a` and `b`: select a, Cmd-click b, C. */
async function openCompare(page: Page, a = 30, b = 33) {
  await openApp(page, N);
  await page.getByTestId(`cell-${a}`).click();
  await page.getByTestId(`cell-${b}`).click({ modifiers: ["Meta"] });
  await page.keyboard.press("c");
  await expect(page.getByTestId("compare-pane-a")).toHaveAttribute("data-image-id", String(a));
  await expect(page.getByTestId("compare-pane-b")).toHaveAttribute("data-image-id", String(b));
}
const paneIds = async (page: Page, prefix = "compare-pane") => [await page.getByTestId(`${prefix}-a`).getAttribute("data-image-id"), await page.getByTestId(`${prefix}-b`).getAttribute("data-image-id")];

test.describe("compare view", () => {
  test("Library: bottom filmstrip picks the Candidate; Swap / Make Select; arrows step the Candidate", async ({ page }) => {
    await openCompare(page);
    await expect(page.getByTestId("filmstrip")).toBeVisible();
    await expect(page.getByTestId("film-30")).toHaveAttribute("data-selected", "true"); // Select
    await expect(page.getByTestId("film-33")).toHaveAttribute("data-marked", "true"); // Candidate
    await expect(page.getByTestId("film-tag-30")).toHaveText("Select");
    await expect(page.getByTestId("film-tag-33")).toHaveText("Candidate");
    await shot(page, `${P}compare-library`);

    // Click a thumbnail: it becomes the Candidate, the Select stays.
    await page.getByTestId("film-35").click();
    expect(await paneIds(page)).toEqual(["30", "35"]);
    await expect(page.getByTestId("film-35")).toHaveAttribute("data-marked", "true");
    // Arrow keys move the Candidate (and skip the Select).
    await page.keyboard.press("ArrowRight");
    expect(await paneIds(page)).toEqual(["30", "36"]);
    await page.keyboard.press("ArrowLeft");
    await page.keyboard.press("ArrowLeft");
    expect(await paneIds(page)).toEqual(["30", "34"]);

    // Swap (Down): the photos trade places and the active photo stays active.
    await page.keyboard.press("ArrowDown");
    expect(await paneIds(page)).toEqual(["34", "30"]);
    await page.getByTestId("compare-swap").click();
    expect(await paneIds(page)).toEqual(["30", "34"]);
    // Make Select (Up): the Candidate becomes the Select and the next photo the new Candidate.
    await page.keyboard.press("ArrowUp");
    expect(await paneIds(page)).toEqual(["34", "35"]);
    await page.getByTestId("compare-make-select").click();
    expect(await paneIds(page)).toEqual(["35", "36"]);

    // Clicking the Select's thumbnail swaps.
    await page.getByTestId("film-35").click();
    expect(await paneIds(page)).toEqual(["36", "35"]);

    // Ratings go to the active pane; filmstrip stars rate any thumbnail without moving the Candidate.
    await clearCalls(page);
    await page.getByTestId("compare-focus-a").click();
    await page.keyboard.press("3");
    await expect.poll(async () => (await calls(page, "set_rating")).length).toBe(1);
    expect((await calls(page, "set_rating"))[0].args).toEqual({ ids: [36], rating: 3 });

    // Shortcuts are in the cheat sheet.
    await page.keyboard.press("?");
    for (const id of ["compare", "compareSwap", "compareMakeSelect", "compareFocus", "scenesToggle"]) await expect(page.getByTestId(`cheat-${id}`).first()).toBeVisible();
    await page.keyboard.press("Escape");
  });

  test("Develop: Compare is a view; sliders edit the active pane; Tab-less pane switching keeps edits per photo", async ({ page }) => {
    await openCompare(page);
    await page.keyboard.press("d"); // Edit in Develop keeps the pair
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await expect(page.getByTestId("dev-compare")).toBeVisible();
    expect(await paneIds(page, "dev-compare-pane")).toEqual(["30", "33"]);
    await expect(page.getByTestId("dev-compare-pane-a")).toHaveAttribute("data-active", "true");
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "30");
    await expect(page.getByTestId("film-33")).toHaveAttribute("data-marked", "true");
    await expect(page.getByTestId("compare-editing")).toContainText("Select");
    await expect(page.getByTestId("dev-compare-pane-a").getByTestId("view-main")).toBeVisible();
    await expect(page.getByTestId("dev-compare-pane-b").getByTestId("view-main")).toBeVisible();
    await shot(page, `${P}compare-develop`);

    // Edit the Select.
    await clearCalls(page);
    await setSlider(page, "exposure", 1.5);
    await expect.poll(async () => (await saves(page)).length).toBe(1);
    expect((await saves(page))[0].args.id).toBe(30);
    expect((await saves(page))[0].args.adjustments.exposure).toBe(1.5);

    // Click the Candidate pane: the same panel now edits it.
    await page.getByTestId("dev-compare-pane-b").click({ position: { x: 40, y: 40 } });
    await expect(page.getByTestId("dev-compare-pane-b")).toHaveAttribute("data-active", "true");
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "33");
    await expect(page.getByTestId("compare-editing")).toContainText("Candidate");
    await expect(page.getByTestId("slider-value-exposure")).toHaveText("0.00");
    await setSlider(page, "exposure", -1);
    await expect.poll(async () => (await saves(page)).length).toBe(2);
    expect((await saves(page))[1].args.id).toBe(33);
    expect((await saves(page))[1].args.adjustments.exposure).toBe(-1);
    await shot(page, `${P}compare-develop-candidate`);

    // The other pane kept its own render/edit: back to the Select shows +1.50.
    await page.getByTestId("compare-focus-a").click();
    await expect(page.getByTestId("slider-value-exposure")).toHaveText("+1.50");

    // Filmstrip click chooses the Candidate; arrows too; Swap flips the panes.
    await page.getByTestId("film-40").click();
    expect(await paneIds(page, "dev-compare-pane")).toEqual(["30", "40"]);
    await page.keyboard.press("ArrowRight");
    expect(await paneIds(page, "dev-compare-pane")).toEqual(["30", "41"]);
    await page.getByTestId("compare-swap").click();
    expect(await paneIds(page, "dev-compare-pane")).toEqual(["41", "30"]);
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "30"); // the active photo stayed active
    await page.keyboard.press("ArrowUp"); // Make Select
    expect(await paneIds(page, "dev-compare-pane")).toEqual(["30", "31"]);

    // Shift+C switches pane from the keyboard; C leaves Compare but stays in Develop.
    await page.keyboard.press("Shift+C");
    await expect(page.getByTestId("dev-compare-pane-a")).toHaveAttribute("data-active", "true");
    await page.keyboard.press("c");
    await expect(page.getByTestId("dev-compare")).toHaveCount(0);
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "30");
    // ...and the toolbar toggle opens it again from Develop.
    await page.getByTestId("develop-compare").click();
    await expect(page.getByTestId("dev-compare")).toBeVisible();
    await shot(page, `${P}compare-develop-reopened`);
  });

  test("top bar Compare inside Develop compares with the next photo", async ({ page }) => {
    await openApp(page, N);
    await page.getByTestId("cell-5").click();
    await page.keyboard.press("d");
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "5");
    await page.getByTestId("mode-compare").click();
    await expect(page.getByTestId("dev-compare")).toBeVisible();
    expect(await paneIds(page, "dev-compare-pane")).toEqual(["5", "6"]);
    await expect(page.getByTestId("mode-compare")).toHaveAttribute("aria-pressed", "true");
    await page.getByTestId("mode-grid").click();
    await expect(page.getByTestId("dev-compare")).toHaveCount(0);
    await expect(page.getByTestId("grid-scroll")).toBeVisible();
  });
});
