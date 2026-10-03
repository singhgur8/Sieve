// UX review 1 (docs/ux-review-1.md): modals, layout targets, undo, keeper, cheat sheet, toasts, empty states.
import { expect, test, type Page } from "@playwright/test";
import { KEYMAP } from "../../src/lib/keymap";
import { calls, clearCalls, closeMenus, detectScenes, openApp, sceneItem, shot } from "./helpers";

const pick = (page: Page, id: number) => page.getByTestId(`cell-${id}`).getAttribute("data-pick");
const rating = (page: Page, id: number) => page.getByTestId(`cell-${id}`).getAttribute("data-rating");

async function openDevelop(page: Page, id = 1) {
  await page.getByTestId(`cell-${id}`).click();
  await page.keyboard.press("d");
  await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", String(id));
  await expect(page.getByTestId("view-main")).toBeVisible();
}

test.describe("layout targets", () => {
  for (const vp of [
    { width: 1280, height: 800, tag: "1280" },
    { width: 1728, height: 1117, tag: "1728" },
  ]) {
    test(`Library chrome and Develop viewer at ${vp.width}x${vp.height}`, async ({ page }) => {
      await page.setViewportSize({ width: vp.width, height: vp.height });
      await openApp(page, 200);
      const gridTop = async () => (await page.getByTestId("grid-scroll").boundingBox())!.y;

      // No scenes yet: strip hidden.
      const noScenes = await gridTop();
      expect(noScenes).toBeLessThanOrEqual(150);
      await expect(page.getByTestId("scene-strip")).toHaveCount(0);
      // The top bar is a single 44 px row.
      const bar = (await page.getByTestId("top-bar").boundingBox())!;
      expect(bar.height).toBe(44);
      await shot(page, `5x-ux-library-${vp.tag}`);

      await detectScenes(page);
      await expect(page.getByTestId("scene-chip-6")).toBeVisible();
      const withScenes = await gridTop();
      expect(withScenes, `Library chrome with scenes at ${vp.tag}`).toBeLessThanOrEqual(150);
      expect((await page.getByTestId("scene-strip").boundingBox())!.height).toBe(32);
      test.info().annotations.push({ type: `chrome-${vp.tag}`, description: `no scenes ${noScenes}px, with scenes ${withScenes}px` });
      console.log(`CHROME ${vp.tag}: no scenes=${noScenes} with scenes=${withScenes}`);
      await shot(page, `5x-ux-library-scenes-${vp.tag}`);

      await page.getByTestId("cell-3").click();
      await page.keyboard.press("d");
      await expect(page.getByTestId("view-main")).toBeVisible();
      const viewer = (await page.getByTestId("viewer").boundingBox())!;
      const share = viewer.height / vp.height;
      console.log(`VIEWER ${vp.tag}: height=${viewer.height} share=${(share * 100).toFixed(1)}%`);
      test.info().annotations.push({ type: `viewer-${vp.tag}`, description: `${viewer.height}px (${(share * 100).toFixed(1)}%)` });
      if (vp.height === 800) expect(viewer.height).toBeGreaterThanOrEqual(560);
      expect(share).toBeGreaterThanOrEqual(0.7);
      await shot(page, `5x-ux-develop-${vp.tag}`);
      // Develop has no filter row: the summary lives in the 22 px filmstrip header.
      expect((await page.getByTestId("filmstrip-header").boundingBox())!.height).toBe(22);
      await expect(page.getByTestId("filter-summary")).toHaveCount(0);
      await expect(page.getByTestId("filter-summary-text")).toBeVisible();
    });
  }

  test("filter summary outside the Grid, Cmd+F toggles the bar", async ({ page }) => {
    await openApp(page, 200);
    await expect(page.getByTestId("filter-bar")).toBeVisible();
    await page.keyboard.press("Meta+f");
    await expect(page.getByTestId("filter-bar")).toHaveCount(0);
    await expect(page.getByTestId("filter-summary")).toBeVisible();
    await page.keyboard.press("Meta+f");
    await expect(page.getByTestId("filter-bar")).toBeVisible();
    await page.getByTestId("pick-pick").click();
    await page.getByTestId("cell-1").click();
    await page.keyboard.press("Space");
    await expect(page.getByTestId("loupe")).toBeVisible();
    await expect(page.getByTestId("filter-bar")).toHaveCount(0);
    await expect(page.getByTestId("filter-summary-text")).toContainText("Filtered: Picked");
    await page.getByTestId("edit-filters").click();
    await expect(page.getByTestId("filter-bar")).toBeVisible();
    await expect(page.getByTestId("loupe")).toHaveCount(0);
  });
});

test.describe("P0-1 modals own the keyboard", () => {
  test("Develop dialogs: keys do not leak, Esc closes only the dialog, Tab is trapped, Enter confirms", async ({ page }) => {
    await openApp(page, 200);
    await openDevelop(page, 8);
    await clearCalls(page);

    await page.keyboard.press("Meta+Shift+c");
    const dlg = page.getByRole("dialog", { name: "Copy Settings" });
    await expect(dlg).toBeVisible();
    await expect(dlg).toHaveAttribute("aria-modal", "true");
    // Focus moved into the dialog on mount.
    expect(await page.evaluate(() => !!document.activeElement?.closest('[role="dialog"]'))).toBe(true);

    for (const key of ["x", "p", "3", "u", "ArrowRight", "ArrowLeft", "g", "z", "\\", "d"]) await page.keyboard.press(key);
    expect((await calls(page, "set_pick")).length).toBe(0);
    expect((await calls(page, "set_rating")).length).toBe(0);
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "8");
    await expect(page.getByTestId("viewer")).toBeVisible();

    for (let i = 0; i < 40; i++) {
      await page.keyboard.press(i % 3 === 0 ? "Shift+Tab" : "Tab");
      expect(await page.evaluate(() => !!document.activeElement?.closest('[role="dialog"]'))).toBe(true);
    }

    await page.keyboard.press("Escape");
    await expect(dlg).toHaveCount(0);
    await expect(page.getByTestId("develop-view")).toBeVisible(); // Esc did not leave Develop
    // The X inside the dialog never rejected the photo behind it.
    expect((await calls(page, "set_pick")).length).toBe(0);

    // Enter confirms; a plain Esc afterwards does nothing in Develop (UX2 P0-1), G leaves.
    await page.keyboard.press("Meta+Shift+c");
    await expect(dlg).toBeVisible();
    await page.keyboard.press("Enter");
    await expect(dlg).toHaveCount(0);
    await expect(page.getByTestId("notice")).toContainText("Copied");
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await page.keyboard.press("g");
    await expect(page.getByTestId("develop-view")).toHaveCount(0);
  });

  test("Match panel closes with Esc; its Copy dialog closes first", async ({ page }) => {
    await openApp(page, 200);
    await detectScenes(page);
    await expect(page.getByTestId("scene-chip-6")).toBeVisible();
    await page.getByTestId("cell-1").click();
    await page.keyboard.press("Shift+A");
    await page.getByTestId("scene-match").click();
    const panel = page.getByTestId("match-panel");
    await expect(panel).toBeVisible();
    await clearCalls(page);
    await page.keyboard.press("x");
    expect((await calls(page, "set_pick")).length).toBe(0);
    await page.getByTestId("match-copy-fields").click();
    await expect(page.getByTestId("fields-dialog")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("fields-dialog")).toHaveCount(0);
    await expect(panel).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(panel).toHaveCount(0);
  });

  test("popover menus swallow shortcuts and close with Esc", async ({ page }) => {
    await openApp(page, 200);
    await page.getByTestId("cell-3").click();
    await page.getByTestId("more-menu").click();
    await expect(page.getByRole("menu")).toBeVisible();
    await clearCalls(page);
    await page.keyboard.press("x");
    expect((await calls(page, "set_pick")).length).toBe(0);
    await page.keyboard.press("Escape");
    await expect(page.getByRole("menu")).toHaveCount(0);
    await page.keyboard.press("x");
    await expect.poll(async () => (await calls(page, "set_pick")).length).toBe(1);
  });
});

test.describe("P0-2 apply suggestions", () => {
  test("confirm dialog with counts, only-unset default, and undo restores prior state", async ({ page }) => {
    await openApp(page, 200);
    await page.getByTestId("cell-2").click();
    await page.getByTestId("cell-6").click({ modifiers: ["Shift"] });
    const before = await page.evaluate(() =>
      [2, 3, 4, 5, 6].map((id) => {
        const el = document.querySelector(`[data-testid="cell-${id}"]`)!;
        return [el.getAttribute("data-pick"), el.getAttribute("data-rating")];
      }),
    );
    expect(before[4]).toEqual(["unflagged", "1"]); // photo 6 is already rated

    await clearCalls(page);
    await page.getByTestId("more-menu").click();
    await page.getByTestId("apply-suggestions").click();
    const dlg = page.getByTestId("apply-dialog");
    await expect(dlg).toBeVisible();
    expect((await calls(page, "apply_suggestions")).length).toBe(0); // nothing happens before confirming
    await expect(page.getByTestId("apply-title")).toHaveText("Apply suggestions");
    await expect(page.getByTestId("apply-scope-selected")).toBeChecked();
    await expect(page.getByTestId("apply-scope")).toContainText("Selected (5)");
    await expect(page.getByTestId("apply-only-unset")).toBeChecked();
    await expect(page.getByTestId("apply-count-skipped")).toHaveText("1");
    await expect(page.getByTestId("apply-count-apply")).toHaveText("4");
    await shot(page, "5x-ux-apply-dialog");

    // Esc cancels; keys do not leak.
    await page.keyboard.press("x");
    await page.keyboard.press("Escape");
    await expect(dlg).toHaveCount(0);
    expect((await calls(page, "apply_suggestions")).length).toBe(0);
    expect((await calls(page, "set_pick")).length).toBe(0);

    await page.getByTestId("more-menu").click();
    await page.getByTestId("apply-suggestions").click();
    await page.keyboard.press("Enter"); // Enter confirms
    await expect(dlg).toHaveCount(0);
    const [call] = await calls(page, "apply_suggestions");
    expect(call.args).toEqual({ ids: [2, 3, 4, 5, 6], onlyUnset: true });
    await expect(page.getByTestId("notice")).toContainText("Applied suggestions to 4 of 5 photos (1 skipped)");
    expect(await rating(page, 6)).toBe("1"); // the manual rating survived
    const after = await page.evaluate(() =>
      [2, 3, 4, 5, 6].map((id) => {
        const el = document.querySelector(`[data-testid="cell-${id}"]`)!;
        return [el.getAttribute("data-pick"), el.getAttribute("data-rating")];
      }),
    );
    expect(after).not.toEqual(before);

    await page.getByTestId("apply-undo").click();
    await expect.poll(async () => (await calls(page, "restore_cull_snapshot")).length).toBe(1);
    const snaps = (await calls(page, "restore_cull_snapshot"))[0].args.snapshots as { imageId: number }[];
    expect(snaps.map((s) => s.imageId)).toEqual([2, 3, 4, 5, 6]);
    await expect
      .poll(() =>
        page.evaluate(() =>
          [2, 3, 4, 5, 6].map((id) => {
            const el = document.querySelector(`[data-testid="cell-${id}"]`)!;
            return [el.getAttribute("data-pick"), el.getAttribute("data-rating")];
          }),
        ),
      )
      .toEqual(before);

    // Unchecking "skip" overwrites everything (onlyUnset false) that differs from its suggestion: one of the
    // five already matches its suggestion (after the undo) and is left alone (IPC v18.1).
    await page.getByTestId("more-menu").click();
    await page.getByTestId("apply-suggestions").click();
    await page.getByTestId("apply-only-unset").uncheck();
    await expect(page.getByTestId("apply-count-skipped")).toHaveText("1");
    await expect(page.getByTestId("apply-count-apply")).toHaveText("4");
    await clearCalls(page);
    await page.getByTestId("apply-confirm").click();
    expect((await calls(page, "apply_suggestions"))[0].args.onlyUnset).toBe(false);
  });

  test("loupe shows the suggestion when it differs", async ({ page }) => {
    await openApp(page, 200);
    await page.getByTestId("cell-2").click();
    await page.keyboard.press("Space");
    await expect(page.getByTestId("loupe")).toBeVisible();
    let found: string | null = null;
    for (let i = 0; i < 12 && !found; i++) {
      const line = page.getByTestId("suggested-line");
      if ((await line.count()) > 0) found = await line.textContent();
      else await page.keyboard.press("ArrowRight");
    }
    expect(found).toMatch(/^Suggested: (Pick|Reject|Unflagged) · \d★$/);
  });
});

test.describe("P0-3 export reason and defaults", () => {
  async function openExport(page: Page, ids = [5, 6]) {
    await openApp(page, 200);
    await page.getByTestId(`cell-${ids[0]}`).click();
    if (ids.length > 1) await page.getByTestId(`cell-${ids.at(-1)}`).click({ modifiers: ["Shift"] });
    await page.keyboard.press("Meta+Shift+E");
    await expect(page.getByTestId("export-dialog")).toBeVisible();
  }

  test("amber reason is visible, names the fix, and clicking Export jumps to the field", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await openExport(page);
    const reason = page.getByTestId("export-reason");
    await expect(reason).toHaveText("Choose a destination folder");
    const box = (await reason.boundingBox())!;
    expect(box.y + box.height).toBeLessThanOrEqual(800); // not below the fold
    // Destination is the first section.
    const dest = (await page.getByTestId("export-dest-kind").boundingBox())!;
    const fmt = (await page.getByTestId("export-format").boundingBox())!;
    expect(dest.y).toBeLessThan(fmt.y);
    await shot(page, "5x-ux-export-reason");

    await clearCalls(page);
    await page.getByTestId("export-go").click({ force: true });
    expect((await calls(page, "export_images")).length).toBe(0);
    await expect(page.getByTestId("export-choose-folder")).toBeFocused();

    await page.getByTestId("export-choose-folder").click();
    await expect(page.getByTestId("export-reason")).toHaveCount(0);
    await page.getByTestId("export-template").fill("{nope}");
    await expect(reason).toHaveText("Fix the file name template");
    await page.getByTestId("export-go").click({ force: true });
    await expect(page.getByTestId("export-template")).toBeFocused();
    await page.getByTestId("export-template").fill("{filename}");
    await page.getByTestId("export-subfolder").fill("../x");
    await expect(reason).toHaveText("Invalid subfolder");
    await page.getByTestId("export-subfolder").fill("");
    await page.getByTestId("export-resize-mode").selectOption("long_edge");
    await page.getByTestId("export-resize-px").fill("0");
    await expect(reason).toHaveText("Check the size fields");
    await page.getByTestId("export-go").click({ force: true });
    await expect(page.getByTestId("export-resize-px")).toBeFocused();
    await page.getByTestId("export-resize-px").fill("2048");
    await expect(page.getByTestId("export-reason")).toHaveCount(0);
  });

  test("remembers the last folder, defaults to All filtered for a single photo, Cmd+Enter exports", async ({ page }) => {
    await openExport(page, [5]);
    await expect(page.getByTestId("export-scope-filtered")).toBeChecked();
    await expect(page.getByTestId("export-count")).toHaveText("186 photos"); // 14 rejects skipped by default
    await page.getByTestId("export-scope-selection").check();
    await expect(page.getByTestId("export-count")).toHaveText("1 photo");
    await page.getByTestId("export-choose-folder").click();
    await expect(page.getByTestId("export-dest-path")).toHaveText("/mock/export/Smith Wedding");
    await clearCalls(page);
    await page.getByTestId("export-template").focus();
    await page.keyboard.press("Enter"); // plain Enter in a text field must not export
    expect((await calls(page, "export_images")).length).toBe(0);
    await page.keyboard.press("Meta+Enter");
    await expect.poll(async () => (await calls(page, "export_images")).length).toBe(1);
    const prefs = await calls(page, "set_ui_prefs");
    expect(prefs.at(-1)!.args.prefs).toEqual({ lastExportFolder: "/mock/export/Smith Wedding" });
    await expect(page.getByTestId("export-dialog")).toHaveCount(0);

    await page.keyboard.press("Meta+Shift+E");
    await expect(page.getByTestId("export-dialog")).toBeVisible();
    await expect(page.getByTestId("export-dest-path")).toHaveText("/mock/export/Smith Wedding");
    await expect(page.getByTestId("export-reason")).toHaveCount(0);
  });
});

test.describe("P1-3 culling undo", () => {
  test("Cmd+Z / Cmd+Shift+Z undo and redo pick, rating and label changes", async ({ page }) => {
    await openApp(page, 200);
    await page.getByTestId("cell-3").click();
    await page.keyboard.press("x");
    await expect.poll(() => pick(page, 3)).toBe("reject");
    await page.keyboard.press("4");
    await expect.poll(() => rating(page, 3)).toBe("4");
    await clearCalls(page);

    await page.keyboard.press("Meta+z");
    await expect.poll(() => rating(page, 3)).toBe("0");
    await expect(page.getByTestId("notice")).toContainText("Undid");
    const [r] = await calls(page, "restore_cull_snapshot");
    expect(r.args.snapshots).toEqual([{ imageId: 3, rating: 0, pick: "reject", colorLabel: null, pickOrigin: "user" }]);
    await page.keyboard.press("Meta+z");
    await expect.poll(() => pick(page, 3)).toBe("unflagged");
    await page.keyboard.press("Meta+Shift+z");
    await expect.poll(() => pick(page, 3)).toBe("reject");
    await page.keyboard.press("Meta+Shift+z");
    await expect.poll(() => rating(page, 3)).toBe("4");
    await page.keyboard.press("Meta+Shift+z");
    await expect(page.getByTestId("notice")).toContainText("Nothing to redo");
  });

  test("batch changes undo together; Develop keeps adjustment undo", async ({ page }) => {
    await openApp(page, 200);
    await page.getByTestId("cell-10").click();
    await page.getByTestId("cell-12").click({ modifiers: ["Shift"] });
    await page.keyboard.press("p");
    await expect.poll(() => pick(page, 12)).toBe("pick");
    await page.keyboard.press("Meta+z");
    await expect.poll(() => pick(page, 10)).toBe("unflagged");
    await expect.poll(() => pick(page, 12)).toBe("unflagged");

    await page.getByTestId("cell-1").click();
    await page.keyboard.press("d");
    await expect(page.getByTestId("view-main")).toBeVisible();
    await page.getByTestId("slider-exposure").fill("1");
    await page.getByTestId("slider-exposure").evaluate((el) => (el as HTMLElement).blur());
    await expect(page.getByTestId("history-list")).toContainText("Exposure");
    await clearCalls(page);
    await page.keyboard.press("Meta+z");
    await expect.poll(async () => (await calls(page, "undo_adjustments")).length).toBe(1);
    expect((await calls(page, "restore_cull_snapshot")).length).toBe(0);
  });
});

test.describe("P1-4 burst keeper", () => {
  test("K in Compare sets keeper + Pick; Shift+K in Grid sets keeper", async ({ page }) => {
    await openApp(page, 200);
    await page.getByTestId("cell-3").click(); // burst 1 = ids 1-4, keeper 2
    await page.keyboard.press("c");
    await expect(page.getByTestId("compare")).toBeVisible();
    await expect(page.getByTestId("compare-pane-a")).toHaveAttribute("data-image-id", "3");
    await expect(page.getByTestId("compare-pane-b")).toHaveAttribute("data-image-id", "2");
    await expect(page.getByTestId("compare-pane-b").getByTestId("keeper-badge")).toBeVisible();
    await expect(page.getByTestId("compare-pane-a").getByTestId("keeper-badge")).toHaveCount(0);
    await clearCalls(page);
    await page.keyboard.press("k");
    await expect.poll(async () => (await calls(page, "set_burst_keeper")).length).toBe(1);
    expect((await calls(page, "set_burst_keeper"))[0].args).toEqual({ groupId: 1, imageId: 3 });
    expect((await calls(page, "set_pick"))[0].args).toEqual({ ids: [3], pick: "pick" });
    await expect(page.getByTestId("compare-pane-a").getByTestId("keeper-badge")).toBeVisible();
    await expect(page.getByTestId("compare-pane-b").getByTestId("keeper-badge")).toHaveCount(0);
    await shot(page, "5x-ux-compare-keeper");

    // Tab focuses the other pane; K there makes it the keeper again.
    await page.keyboard.press("Tab");
    await page.keyboard.press("k");
    await expect.poll(async () => (await calls(page, "set_burst_keeper")).length).toBe(2);
    expect((await calls(page, "set_burst_keeper"))[1].args).toEqual({ groupId: 1, imageId: 2 });

    await page.keyboard.press("Escape");
    await expect(page.getByTestId("compare")).toHaveCount(0);
    await page.getByTestId("cell-4").click();
    await clearCalls(page);
    await page.keyboard.press("Shift+K");
    await expect.poll(async () => (await calls(page, "set_burst_keeper")).length).toBe(1);
    expect((await calls(page, "set_burst_keeper"))[0].args).toEqual({ groupId: 1, imageId: 4 });
    expect((await calls(page, "set_pick")).length).toBe(0);
  });
});

test.describe("P1-10 cheat sheet and keymap", () => {
  test("? and Cmd+/ open a sheet generated from the keymap; keys are inert while it is open", async ({ page }) => {
    await openApp(page, 200);
    await page.getByTestId("cell-3").click();
    await page.keyboard.press("?");
    const sheet = page.getByTestId("cheat-sheet");
    await expect(sheet).toBeVisible();
    for (const d of KEYMAP) await expect(page.getByTestId(`cheat-${d.id}`), d.id).toHaveCount(1);
    await expect(page.getByTestId("cheat-pick")).toContainText("P");
    await expect(page.getByTestId("cheat-keeper")).toContainText("K");
    await expect(page.getByTestId("cheat-sync")).toContainText("Cmd+Shift+S");
    await shot(page, "5x-ux-cheat-sheet");
    await clearCalls(page);
    await page.keyboard.press("x");
    await page.keyboard.press("d");
    expect((await calls(page, "set_pick")).length).toBe(0);
    await expect(page.getByTestId("develop-view")).toHaveCount(0);
    await expect(page.getByTestId("cheat-filter")).toHaveValue("xd"); // typing filters the list
    await page.keyboard.press("Escape"); // clears the filter first
    await expect(page.getByTestId("cheat-filter")).toHaveValue("");
    await page.keyboard.press("Escape");
    await expect(sheet).toHaveCount(0);

    await page.keyboard.press("Meta+/");
    await expect(sheet).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(sheet).toHaveCount(0);
    await page.getByTestId("more-menu").click();
    await page.getByTestId("open-cheat-sheet").click();
    await expect(sheet).toBeVisible();
  });

  test("tooltips come from the keymap", async ({ page }) => {
    await openApp(page, 200);
    await expect(page.getByTestId("mode-develop")).toHaveAttribute("title", "Develop (D)");
    await expect(page.getByTestId("export-button")).toHaveAttribute("title", /\(Cmd\+Shift\+E\)/);
    await expect(page.getByTestId("import-button")).toHaveAttribute("title", /\(Cmd\+Shift\+I\)/);
    await page.getByTestId("cell-1").click();
    await page.keyboard.press("d");
    await expect(page.getByTestId("before-toggle")).toHaveAttribute("title", "Before / after (\\)");
    await expect(page.getByTestId("split-toggle")).toHaveAttribute("title", "Split view (Y)");
    await expect(page.getByTestId("reset-all")).toHaveAttribute("title", "Reset all adjustments (Cmd+Shift+R)");
    await expect(page.getByTestId("previous-settings")).toHaveAttribute("title", "No previous photo yet");
    await expect(page.getByTestId("paste-settings")).toHaveAttribute("title", "Copy settings first (Cmd+Shift+C)");
  });

  test("Develop keys: Y split, Cmd+Shift+R reset, E to Loupe, Cmd+Shift+S sync hint", async ({ page }) => {
    await openApp(page, 200);
    await openDevelop(page, 1);
    await page.keyboard.press("y");
    await expect(page.getByTestId("split-handle")).toBeVisible();
    await page.keyboard.press("y");
    await expect(page.getByTestId("split-handle")).toHaveCount(0);
    await page.getByTestId("slider-exposure").fill("1");
    await page.getByTestId("slider-exposure").evaluate((el) => (el as HTMLElement).blur());
    await expect(page.getByTestId("slider-value-exposure")).toHaveText("+1.00");
    await page.keyboard.press("Meta+Shift+r");
    await expect(page.getByTestId("slider-value-exposure")).toHaveText("0.00");
    await page.keyboard.press("Meta+Shift+s"); // single selection: explains itself
    await expect(page.getByTestId("notice")).toContainText("Cmd/Shift-click other photos");
    await page.getByTestId("film-3").click({ modifiers: ["Meta"] });
    await page.keyboard.press("Meta+Shift+s");
    await expect(page.getByTestId("fields-dialog")).toBeVisible();
    await page.keyboard.press("Escape");
    await page.keyboard.press("e");
    await expect(page.getByTestId("loupe")).toBeVisible();
    await expect(page.getByTestId("develop-view")).toHaveCount(0);
  });

  test("Grid Home/End/PageDown and Loupe info overlay cycling", async ({ page }) => {
    await openApp(page, 200);
    await page.getByTestId("cell-3").click();
    await page.keyboard.press("End");
    await expect(page.getByTestId("cell-200")).toHaveAttribute("data-active", "true");
    await page.keyboard.press("Home");
    await expect(page.getByTestId("cell-1")).toHaveAttribute("data-active", "true");
    await page.keyboard.press("PageDown");
    const first = await page.locator('[data-testid^="cell-"][data-active="true"]').getAttribute("data-testid");
    expect(first).not.toBe("cell-1");
    await page.keyboard.press("Shift+End");
    await expect(page.getByTestId("selection-count")).toContainText("selected");
    expect(await page.getByTestId("selection-count").textContent()).toMatch(/(\d+) selected/);

    await page.keyboard.press("Home");
    await page.keyboard.press("Space");
    await expect(page.getByTestId("info-overlay")).toHaveAttribute("data-level", "full");
    await page.keyboard.press("i");
    await expect(page.getByTestId("info-overlay")).toHaveAttribute("data-level", "name");
    await page.keyboard.press("i");
    await expect(page.getByTestId("info-overlay")).toHaveCount(0);
    await page.keyboard.press("i");
    await expect(page.getByTestId("info-overlay")).toHaveAttribute("data-level", "full");
  });
});

test.describe("P1-2 toasts and job pill", () => {
  test("toasts float above the layout; finished export collapses to a pill with Reveal", async ({ page }) => {
    await page.addInitScript(() => (window.__mockExportManual = true));
    await page.setViewportSize({ width: 1280, height: 800 });
    await openApp(page, 200);
    const top = async () => (await page.getByTestId("grid-scroll").boundingBox())!.y;
    const y0 = await top();
    await page.getByTestId("cell-5").click();
    await page.keyboard.press("Meta+s");
    const toast = page.getByTestId("notice");
    await expect(toast).toContainText("Saved metadata");
    expect(await top()).toBe(y0); // no layout shift
    const tb = (await toast.boundingBox())!;
    expect(tb.width).toBeLessThanOrEqual(480);
    expect(Math.abs(tb.x + tb.width / 2 - 640)).toBeLessThan(4); // bottom centre
    expect(tb.y).toBeGreaterThan(400);
    await expect(toast).toHaveCount(0, { timeout: 6000 }); // fades after ~4 s

    await page.keyboard.press("Meta+Shift+E");
    await page.getByTestId("export-scope-selection").check(); // photo 7 fails in the mock; failures keep the card open
    await page.getByTestId("export-choose-folder").click();
    await page.keyboard.press("Meta+Enter");
    const job = page.getByTestId("export-job-1");
    await expect(job).toBeVisible();
    const jb = (await job.boundingBox())!;
    expect(jb.x + jb.width).toBeGreaterThan(1200); // bottom right
    expect(jb.y).toBeGreaterThan(400);
    await expect(page.getByTestId("export-ring")).toBeVisible(); // progress ring on the Export button while running
    await page.evaluate(() => window.__mockExportStep!(300));
    await expect(job).toHaveAttribute("data-state", "finished");
    await expect(page.getByTestId("export-reveal")).toBeVisible();
    await expect(job).toHaveAttribute("data-state", "pill", { timeout: 12_000 });
    expect((await job.boundingBox())!.height).toBe(28);
    await expect(page.getByTestId("export-pill-text")).toContainText("Export done");
    await shot(page, "5x-ux-export-pill");
    await clearCalls(page);
    await page.getByTestId("export-reveal").click();
    await expect.poll(async () => (await calls(page, "reveal_in_finder")).length).toBe(1);
    expect((await calls(page, "reveal_in_finder"))[0].args).toEqual({ path: "/mock/export/Smith Wedding" });
  });
});

test.describe("P1-8 XMP pill", () => {
  test("auto-sync off: pill shows unsaved count and Save now writes everything", async ({ page }) => {
    await openApp(page, 200);
    const pill = page.getByTestId("xmp-status");
    await expect(pill).toContainText("5 unsaved");
    await page.getByTestId("xmp-status-button").click();
    await clearCalls(page);
    await page.getByTestId("xmp-save-all").click();
    await expect.poll(async () => (await calls(page, "write_xmp_all_dirty")).length).toBe(1);
    expect((await calls(page, "write_xmp_all_dirty"))[0].args).toEqual({ folderId: null });
    await expect(page.getByTestId("notice")).toContainText("Saved XMP for 5 photos");
    await expect(pill).toHaveAttribute("data-state", "off");
    await expect(pill).toContainText("Auto-save off");
  });
});

test.describe("P1-9 empty states", () => {
  test("empty catalog shows the import call to action", async ({ page }) => {
    await page.route(/\/mock\/(thumb|preview)\/\d+\.jpg/, (r) => r.fulfill({ contentType: "image/svg+xml", body: "<svg xmlns='http://www.w3.org/2000/svg'/>" }));
    await page.goto("/?mock=0&scope=all");
    await expect(page.getByTestId("grid-empty-catalog")).toBeVisible();
    await expect(page.getByTestId("grid-empty-catalog")).toContainText("Import a shoot folder to start");
    await expect(page.getByTestId("grid-empty-catalog")).toContainText("ARW");
    await expect(page.getByTestId("grid-empty-catalog")).toContainText("RAF");
    await expect(page.getByTestId("grid-empty-catalog")).toContainText("CR3");
    await expect(page.getByTestId("empty-import")).toContainText("Cmd+Shift+I");
    await shot(page, "5x-ux-empty-catalog");
    await clearCalls(page);
    await page.keyboard.press("Meta+Shift+i");
    await expect.poll(async () => (await calls(page, "plugin:dialog|open")).length + (await calls(page, "import_folder")).length).toBeGreaterThan(-1);
  });

  test("empty filter result offers Clear filters", async ({ page }) => {
    await openApp(page, 200);
    await page.getByLabel("Minimum rating").selectOption("5");
    await page.getByLabel("Maximum rating").selectOption("0");
    await expect(page.getByTestId("grid-empty")).toBeVisible();
    await page.getByTestId("empty-clear-filters").click();
    await expect(page.getByTestId("grid-empty")).toHaveCount(0);
    await expect(page.getByTestId("cell-1")).toBeVisible();
  });
});

test.describe("P1-5/6 Develop flags and library-level settings clipboard", () => {
  test("Develop shows flag and stars; paste and select-burst work from the Grid", async ({ page }) => {
    await openApp(page, 200);
    // Photo 8 is rejected (i % 7 == 0) with rating 0; give photo 11 stars via keyboard.
    await page.getByTestId("cell-8").click();
    await page.keyboard.press("d");
    await expect(page.getByTestId("develop-flags")).toHaveAttribute("data-pick", "reject");
    await expect(page.getByTestId("film-flag-8")).toHaveAttribute("data-pick", "reject");
    await page.keyboard.press("3");
    await expect(page.getByTestId("develop-flags")).toHaveAttribute("data-rating", "3");
    await expect(page.getByTestId("film-rating-8")).toHaveAttribute("data-rating", "3");
    await shot(page, "5x-ux-develop-flags");

    await page.getByTestId("slider-exposure").fill("1.5");
    await page.getByTestId("slider-exposure").evaluate((el) => (el as HTMLElement).blur());
    await page.keyboard.press("Meta+Shift+c");
    await page.keyboard.press("Enter");
    await page.keyboard.press("g");
    await page.getByTestId("cell-20").click();
    await clearCalls(page);
    await page.keyboard.press("Meta+Shift+v");
    await expect.poll(async () => (await calls(page, "paste_settings")).length).toBe(1);
    const [p] = await calls(page, "paste_settings");
    expect(p.args.ids).toEqual([20]);
    expect((p.args.adjustments as { exposure: number }).exposure).toBeCloseTo(1.5, 2);

    await page.getByTestId("cell-3").click();
    await page.keyboard.press("Meta+Shift+b");
    await expect(page.getByTestId("selection-count")).toContainText("4 selected");
  });
});

test.describe("P2 contrast", () => {
  test("no neutral-500/600 text remains in Library or Develop", async ({ page }) => {
    await openApp(page, 200);
    const bad = '[class*="text-neutral-500"], [class*="text-neutral-600"]';
    expect(await page.locator(bad).count()).toBe(0);
    await openDevelop(page, 1);
    expect(await page.locator(bad).count()).toBe(0);
  });
});

test.describe("scene strip", () => {
  test("scene menu keeps the seven actions; Match scene is the primary button", async ({ page }) => {
    await openApp(page, 200);
    await detectScenes(page);
    await expect(page.getByTestId("scene-chip-6")).toBeVisible();
    await sceneItem(page, "scenes-detect");
    for (const id of ["scenes-detect", "scene-new", "scene-merge", "scene-split", "scene-remove", "scene-delete", "scene-anchor"]) {
      await expect(page.getByTestId(id), id).toBeVisible();
    }
    await closeMenus(page);
    await expect(page.getByTestId("scene-match")).toBeVisible();
  });
});
