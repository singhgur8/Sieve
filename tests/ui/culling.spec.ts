import { expect, test } from "@playwright/test";
import { calls, clearCalls, openApp, shot } from "./helpers";

test.describe("grid", () => {
  test("renders 5000 items with a bounded DOM and scrolls smoothly", async ({ page }) => {
    await openApp(page, 5000);
    await expect(page.getByTestId("selection-count")).toContainText("5000 photos");

    const domCount = () => page.locator('[data-testid^="cell-"]').count();
    const initial = await domCount();
    expect(initial).toBeGreaterThan(5);
    expect(initial).toBeLessThan(250);
    await shot(page, "01-grid-top");

    // Scroll through the whole catalog with rAF-driven steps, recording frame times.
    const stats = await page.evaluate(async () => {
      const el = document.querySelector('[data-testid="grid-scroll"]') as HTMLElement;
      const deltas: number[] = [];
      let maxDom = 0;
      let last = performance.now();
      const total = el.scrollHeight - el.clientHeight;
      const frames = 120;
      await new Promise<void>((resolve) => {
        let f = 0;
        const tick = (now: number) => {
          deltas.push(now - last);
          last = now;
          el.scrollTop = (total * f) / frames;
          maxDom = Math.max(maxDom, document.querySelectorAll('[data-testid^="cell-"]').length);
          if (++f > frames) resolve();
          else requestAnimationFrame(tick);
        };
        requestAnimationFrame(tick);
      });
      deltas.shift();
      const sorted = [...deltas].sort((a, b) => a - b);
      return {
        frames: deltas.length,
        avg: deltas.reduce((a, b) => a + b, 0) / deltas.length,
        p95: sorted[Math.floor(sorted.length * 0.95)],
        max: sorted[sorted.length - 1],
        maxDom,
        scrollHeight: el.scrollHeight,
      };
    });
    console.log("scroll stats", JSON.stringify(stats));
    expect(stats.maxDom).toBeLessThan(300);
    expect(stats.scrollHeight).toBeGreaterThan(10000);
    // Headless CI renderers are slower than a real GPU; this is a regression guard, not the 60 fps claim.
    expect(stats.avg).toBeLessThan(40);

    // Bottom of the list is reachable and loaded lazily.
    await expect(page.getByTestId("cell-5000")).toBeVisible();
    await expect(page.getByTestId("cell-5000").locator("img")).toBeVisible();
    expect(await domCount()).toBeLessThan(300);
    await shot(page, "02-grid-bottom");
  });

  test("only fetches entries for visible rows", async ({ page }) => {
    await openApp(page, 5000);
    await page.waitForTimeout(200);
    const fetched = (await calls(page, "get_images")).reduce((n, c) => n + c.args.ids.length, 0);
    expect(fetched).toBeLessThan(400);
    const idCalls = await calls(page, "list_image_ids");
    expect(idCalls.length).toBeGreaterThan(0);
  });

  test("thumbnail size slider changes column count; sort re-queries", async ({ page }) => {
    await openApp(page, 500);
    const perRow = () => page.locator('[data-testid="grid-row"]').first().locator('[data-testid^="cell-"]').count();
    const before = await perRow();
    await page.getByTestId("thumb-size").fill("400");
    await expect.poll(perRow).toBeLessThan(before);
    await page.getByTestId("thumb-size").fill("100");
    await expect.poll(perRow).toBeGreaterThan(before);

    await clearCalls(page);
    await page.getByTestId("sort-select").selectOption("rating");
    await page.getByTestId("sort-dir").click();
    await expect.poll(async () => (await calls(page, "list_image_ids")).length).toBeGreaterThan(0);
    const last = (await calls(page, "list_image_ids")).at(-1)!;
    expect(last.args.query.sort).toBe("rating");
    expect(last.args.query.sortDescending).toBe(true);
  });

  test("selection: click, shift-range, cmd-toggle, select all", async ({ page }) => {
    await openApp(page, 1000);
    const count = page.getByTestId("selection-count");
    await page.getByTestId("cell-2").click();
    await expect(count).toContainText("1 selected");
    await page.getByTestId("cell-5").click({ modifiers: ["Shift"] });
    await expect(count).toContainText("4 selected");
    await page.getByTestId("cell-9").click({ modifiers: ["Meta"] });
    await expect(count).toContainText("5 selected");
    await page.getByTestId("cell-9").click({ modifiers: ["Meta"] });
    await expect(count).toContainText("4 selected");
    await page.keyboard.press("Meta+a");
    await expect(count).toContainText("1000 selected");
    await shot(page, "03-selection");
  });
});

test.describe("filters", () => {
  test("tag include / exclude, picks, rating, labels, bursts and folder change the query", async ({ page }) => {
    await openApp(page, 2000);
    const lastQuery = async () => (await calls(page, "list_image_ids")).at(-1)!.args.query;
    const shown = page.getByTestId("selection-count");

    await page.getByTestId("tag-blink").click();
    await expect.poll(async () => (await lastQuery()).includeTags).toEqual(["blink"]);
    await expect(shown).toContainText(" of 2000");
    const blinkOnly = await page.locator('[data-testid^="cell-"]').count();
    expect(blinkOnly).toBeGreaterThan(0);
    await shot(page, "04-filter-blink");

    await page.getByTestId("tag-blink").click(); // -> exclude
    await expect.poll(async () => (await lastQuery()).excludeTags).toEqual(["blink"]);
    expect((await lastQuery()).includeTags).toEqual([]);

    await page.getByTestId("tag-motion_blur").click();
    await page.locator('select[aria-label="Tag match mode"]').selectOption("all");
    await expect.poll(async () => (await lastQuery()).tagMatch).toBe("all");

    await page.getByTestId("pick-reject").click();
    await expect.poll(async () => (await lastQuery()).picks).toEqual(["reject"]);

    await page.locator('select[aria-label="Minimum rating"]').selectOption("2");
    await expect.poll(async () => (await lastQuery()).minRating).toBe(2);

    await page.getByTestId("label-red").click();
    await expect.poll(async () => (await lastQuery()).colorLabels).toEqual(["red"]);

    await page.getByTestId("collapse-bursts").check();
    await expect.poll(async () => (await lastQuery()).collapseBursts).toBe(true);

    await page.getByTestId("folder-select").selectOption("2");
    await expect.poll(async () => (await lastQuery()).folderId).toBe(2);

    await page.getByTestId("clear-filters").click();
    await expect(shown).toContainText("2000 photos");
    const q = await lastQuery();
    expect(q.includeTags).toEqual([]);
    expect(q.folderId).toBeNull();
  });

  test("collapse bursts hides non-keepers", async ({ page }) => {
    await openApp(page, 1000);
    await page.getByTestId("collapse-bursts").check();
    // 4 burst members per 25 frames: 3 hidden each -> 1000 - 40*3 = 880
    await expect(page.getByTestId("selection-count")).toContainText("880 of 1000");
  });
});

test.describe("keyboard", () => {
  test("P / X / U / 0-5 / labels apply to the selection via IPC", async ({ page }) => {
    await openApp(page, 1000);
    await page.getByTestId("cell-3").click();
    await clearCalls(page);

    await page.keyboard.press("p");
    await expect.poll(async () => (await calls(page, "set_pick")).length).toBe(1);
    expect((await calls(page, "set_pick"))[0].args).toEqual({ ids: [3], pick: "pick" });
    await expect(page.getByTestId("cell-3")).toHaveAttribute("data-pick", "pick");

    await page.keyboard.press("x");
    await expect.poll(async () => (await calls(page, "set_pick")).length).toBe(2);
    expect((await calls(page, "set_pick"))[1].args).toEqual({ ids: [3], pick: "reject" });

    await page.keyboard.press("u");
    await expect.poll(async () => (await calls(page, "set_pick")).length).toBe(3);
    expect((await calls(page, "set_pick"))[2].args).toEqual({ ids: [3], pick: "unflagged" });

    for (const n of [0, 1, 2, 3, 4, 5]) {
      await page.keyboard.press(String(n));
      await expect.poll(async () => (await calls(page, "set_rating")).length).toBe(n + 1);
      expect((await calls(page, "set_rating"))[n].args).toEqual({ ids: [3], rating: n });
    }
    await expect(page.getByTestId("cell-3")).toHaveAttribute("data-rating", "5");

    await page.keyboard.press("7");
    await expect.poll(async () => (await calls(page, "set_color_label")).length).toBe(1);
    expect((await calls(page, "set_color_label"))[0].args).toEqual({ ids: [3], label: "yellow" });
  });

  test("batch: applies to a shift-range and to select all; Cmd+S writes XMP for the selection", async ({ page }) => {
    await openApp(page, 1000);
    await page.getByTestId("cell-10").click();
    await page.getByTestId("cell-13").click({ modifiers: ["Shift"] });
    await clearCalls(page);
    await page.keyboard.press("3");
    await expect.poll(async () => (await calls(page, "set_rating")).length).toBe(1);
    expect((await calls(page, "set_rating"))[0].args).toEqual({ ids: [10, 11, 12, 13], rating: 3 });

    await page.keyboard.press("Meta+s");
    await expect.poll(async () => (await calls(page, "write_xmp")).length).toBe(1);
    expect((await calls(page, "write_xmp"))[0].args.ids).toEqual([10, 11, 12, 13]);

    await page.keyboard.press("Meta+a");
    await clearCalls(page);
    await page.keyboard.press("x");
    await expect.poll(async () => (await calls(page, "set_pick")).length).toBe(1);
    expect((await calls(page, "set_pick"))[0].args.ids).toHaveLength(1000);
  });

  test("arrows navigate; auto-advance moves to the next photo after flagging", async ({ page }) => {
    await openApp(page, 1000);
    await page.getByTestId("cell-1").click();
    await page.keyboard.press("ArrowRight");
    await expect(page.getByTestId("cell-2")).toHaveAttribute("data-active", "true");
    await page.keyboard.press("ArrowDown");
    const cols = await page.locator('[data-testid="grid-row"]').first().locator('[data-testid^="cell-"]').count();
    await expect(page.getByTestId(`cell-${2 + cols}`)).toHaveAttribute("data-active", "true");
    await page.keyboard.press("ArrowLeft");
    await page.keyboard.press("ArrowUp");

    await page.getByTestId("cell-20").click();
    await page.keyboard.press("Shift+p");
    await expect(page.getByTestId("cell-21")).toHaveAttribute("data-active", "true");
    await page.getByTestId("auto-advance").check();
    await page.keyboard.press("4");
    await expect(page.getByTestId("cell-22")).toHaveAttribute("data-active", "true");
  });
});

test.describe("loupe", () => {
  test("Space opens the loupe, Space toggles 100%, F cycles faces, arrows navigate, G returns", async ({ page }) => {
    await openApp(page, 1000);
    await page.getByTestId("cell-4").click();
    await page.keyboard.press("Space");
    await expect(page.getByTestId("loupe")).toBeVisible();
    await expect(page.getByTestId("zoom-label")).toContainText("Fit");
    await expect(page.getByTestId("info-overlay")).toContainText("DSC00004");
    await expect(page.getByTestId("zoom-a").locator("img").nth(1)).toBeVisible();
    await shot(page, "05-loupe-fit");

    await page.keyboard.press("Space");
    await expect(page.getByTestId("zoom-label")).toContainText("100%");
    await shot(page, "06-loupe-100");
    await page.keyboard.press("Space");
    await expect(page.getByTestId("zoom-label")).toContainText("Fit");

    await page.keyboard.press("f");
    await expect(page.getByTestId("zoom-a")).not.toHaveAttribute("data-scale", "1");
    await shot(page, "07-loupe-face-zoom");
    await page.keyboard.press("f");
    const s1 = await page.getByTestId("zoom-a").getAttribute("data-scale");
    await page.keyboard.press("f");
    await expect(page.getByTestId("zoom-a")).toHaveAttribute("data-scale", "1");
    expect(s1).not.toBe("1");

    await page.keyboard.press("ArrowRight");
    await expect(page.getByTestId("info-overlay")).toContainText("DSC00005");
    await clearCalls(page);
    await page.keyboard.press("p");
    await expect.poll(async () => (await calls(page, "set_pick")).length).toBe(1);
    expect((await calls(page, "set_pick"))[0].args).toEqual({ ids: [5], pick: "pick" });

    await page.keyboard.press("g");
    await expect(page.getByTestId("loupe")).toHaveCount(0);
    await expect(page.getByTestId("cell-5")).toHaveAttribute("data-active", "true");
  });

  test("wheel zoom and drag pan", async ({ page }) => {
    await openApp(page, 100);
    await page.getByTestId("cell-1").dblclick();
    await expect(page.getByTestId("loupe")).toBeVisible();
    const pane = page.getByTestId("zoom-a");
    const box = (await pane.boundingBox())!;
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.wheel(0, -600);
    await expect.poll(async () => Number(await pane.getAttribute("data-scale"))).toBeGreaterThan(1);
    const img = pane.locator("img").last();
    const before = await img.evaluate((el) => el.style.transform);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width / 2 + 80, box.y + box.height / 2 + 40, { steps: 4 });
    await page.mouse.up();
    expect(await img.evaluate((el) => el.style.transform)).not.toBe(before);
    await pane.click(); // a click zooms back to Fit (Lightroom)
    await expect(pane).toHaveAttribute("data-scale", "1");
  });
});

test.describe("compare", () => {
  test("C on a burst member compares against the keeper with synchronized zoom", async ({ page }) => {
    await openApp(page, 1000);
    await page.getByTestId("cell-3").click(); // burst 1, non-keeper; keeper is id 2
    await page.keyboard.press("c");
    await expect(page.getByTestId("compare")).toBeVisible();
    await expect(page.getByTestId("compare-pane-a")).toHaveAttribute("data-image-id", "3");
    await expect(page.getByTestId("compare-pane-b")).toHaveAttribute("data-image-id", "2");
    await expect(page.getByTestId("zoom-a").locator("img").last()).toBeVisible();
    await shot(page, "08-compare");

    await page.keyboard.press("Space");
    await expect(page.getByTestId("zoom-label")).toContainText("100%");
    const a = await page.getByTestId("zoom-a").getAttribute("data-scale");
    const b = await page.getByTestId("zoom-b").getAttribute("data-scale");
    expect(a).toBe(b);
    expect(Number(a)).toBeGreaterThan(1);
    await shot(page, "09-compare-zoomed");

    // Rating applies to the focused pane; Tab switches focus.
    await clearCalls(page);
    await page.keyboard.press("5");
    await expect.poll(async () => (await calls(page, "set_rating")).length).toBe(1);
    expect((await calls(page, "set_rating"))[0].args).toEqual({ ids: [3], rating: 5 });
    await page.keyboard.press("Tab");
    await page.keyboard.press("x");
    await expect.poll(async () => (await calls(page, "set_pick")).length).toBe(1);
    expect((await calls(page, "set_pick"))[0].args).toEqual({ ids: [2], pick: "reject" });

    // Arrows step the Candidate through the filtered gallery, skipping the Select.
    await page.keyboard.press("ArrowRight");
    await expect(page.getByTestId("compare-pane-b")).toHaveAttribute("data-image-id", "4");
    await page.keyboard.press("ArrowRight");
    await expect(page.getByTestId("compare-pane-b")).toHaveAttribute("data-image-id", "5");

    await page.keyboard.press("Escape");
    await expect(page.getByTestId("compare")).toHaveCount(0);
  });

  test("two selected photos compare 2-up", async ({ page }) => {
    await openApp(page, 1000);
    await page.getByTestId("cell-30").click();
    await page.getByTestId("cell-33").click({ modifiers: ["Meta"] });
    await page.keyboard.press("c");
    await expect(page.getByTestId("compare-pane-a")).toHaveAttribute("data-image-id", "30");
    await expect(page.getByTestId("compare-pane-b")).toHaveAttribute("data-image-id", "33");
  });
});

test("top bar: XMP auto-sync toggle and save metadata call the backend", async ({ page }) => {
  await openApp(page, 200);
  await page.getByTestId("more-menu").click();
  await page.getByTestId("xmp-auto").check();
  await page.keyboard.press("Escape");
  await expect.poll(async () => (await calls(page, "set_xmp_auto_sync")).length).toBe(1);
  expect((await calls(page, "set_xmp_auto_sync"))[0].args).toEqual({ enabled: true });
  await page.getByTestId("cell-8").click();
  await page.getByTestId("save-metadata").click();
  await expect.poll(async () => (await calls(page, "write_xmp")).length).toBe(1);
  await expect(page.getByTestId("notice")).toContainText("Saved metadata");
  await shot(page, "10-top-bar");
});
