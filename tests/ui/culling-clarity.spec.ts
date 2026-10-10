import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openApp, openHome, shot } from "./helpers";

// Phase 8c "Culling clarity": shown-of-total readout, cull summary + keeper formula, rejected pile with reasons,
// hover text on every icon, Apply suggestions copy, sidecar refresh. Mock rules: project 1 = ids 1..101; every 7th photo
// is pre-flagged by the user; `?keepers=not_rejected` = the v18 default keeper rule.
const V18 = "&keepers=not_rejected";

async function openProject(page: Page, query = V18) {
  await openHome(page, 201, query);
  await page.getByTestId("project-open-1").click();
  await expect(page.getByTestId("grid-toolbar")).toBeVisible();
  await expect(page.getByTestId("cull-summary")).toBeVisible();
}

/** Ids the mock returns for the grid's current query (what the grid shows). */
async function gridIds(page: Page): Promise<number[]> {
  const q = (await calls(page, "list_image_ids")).at(-1)!.args.query;
  return page.evaluate((query) => (window as any).__TAURI_INTERNALS__.invoke("list_image_ids", { query }), q);
}

async function expectReadoutMatchesGrid(page: Page) {
  const readout = page.getByTestId("selection-count");
  await expect(readout).toBeVisible();
  await expect
    .poll(async () => {
      const n = (await gridIds(page)).length;
      return `${await page.getByTestId("readout-shown").textContent()}/${n}`;
    })
    .toMatch(/^(\d+)\/\1$/);
  const n = (await gridIds(page)).length;
  // The scroll height of the grid agrees with it too: rows * pitch.
  if (n > 0) {
    const cols = await page.getByTestId("grid-row").first().evaluate((el) => getComputedStyle(el).gridTemplateColumns.split(" ").length);
    const cell = await page.locator('[data-testid^="cell-"]').first().boundingBox();
    const h = await page.getByTestId("grid-inner").evaluate((el) => el.getBoundingClientRect().height);
    expect(Math.round((h - 16 + 6) / (cell!.height + 6))).toBe(Math.ceil(n / cols));
  }
  return n;
}

test.describe("shown-of-total readout", () => {
  test("Showing N of M photos follows three filter combinations", async ({ page }) => {
    await openApp(page, 400);
    const readout = page.getByTestId("selection-count");
    await expect(readout).toHaveText("Showing 400 of 400 photos · 0 selected");
    // 1: one tag
    await page.getByTestId("tag-blink").click();
    const a = await expectReadoutMatchesGrid(page);
    expect(a).toBeLessThan(400);
    await expect(page.getByTestId("readout-total")).toHaveText("400");
    // 2: tag + rejected flag
    await page.getByTestId("pick-reject").click();
    const b = await expectReadoutMatchesGrid(page);
    expect(b).toBeLessThanOrEqual(a);
    // 3: clear, collapse bursts + exclude a tag
    await page.getByTestId("clear-filters").click();
    await page.getByTestId("collapse-bursts").check();
    await page.getByTestId("tag-underexposed").click();
    await page.getByTestId("tag-underexposed").click(); // include -> exclude
    const c = await expectReadoutMatchesGrid(page);
    expect(c).toBeLessThan(400);
    await page.locator('[data-testid^="cell-"]').first().click();
    await expect(readout).toContainText(`Showing ${c} of 400 photos · 1 selected`);
    await shot(page, "clarity-readout");
  });

  test("Loupe shows the position in the filtered set", async ({ page }) => {
    await openApp(page, 400);
    await page.getByTestId("pick-pick").click();
    const n = await expectReadoutMatchesGrid(page);
    await page.getByTestId("cell-1").click();
    await page.keyboard.press("Space");
    await expect(page.getByTestId("loupe")).toBeVisible();
    await expect(page.getByTestId("loupe-position")).toHaveText(new RegExp(`^\\d+ of ${n}$`));
  });
});

test.describe("cull summary and keepers", () => {
  test("formula adds up, every number filters the grid, and it follows culling changes", async ({ page }) => {
    await openProject(page);
    const bar = page.getByTestId("cull-summary");
    const read = async () => ({
      picked: Number(await page.getByTestId("cull-sum-picked").locator("b").textContent()),
      unflagged: Number(await page.getByTestId("cull-sum-unflagged").locator("b").textContent()),
      rejected: Number(await page.getByTestId("cull-sum-rejected").locator("b").textContent()),
      keepers: Number(await page.getByTestId("cull-sum-keepers").locator("b").textContent()),
      total: Number(await bar.getAttribute("data-total")),
    });
    const s = await read();
    expect(s.total).toBe(101);
    expect(s.picked + s.unflagged + s.rejected).toBe(s.total);
    expect(s.keepers).toBe(s.picked + s.unflagged); // v18 default: everything not rejected
    expect(s.rejected).toBeGreaterThan(0);
    await expect(page.getByTestId("cull-sum-formula")).toHaveText(`= ${s.picked} picked + ${s.unflagged} unflagged · ${s.rejected} rejected are left out`);
    await expect(page.getByTestId("step-cull-sub")).toHaveText(`· ${s.keepers} keepers`);
    await shot(page, "clarity-cull-summary");

    for (const [id, n] of [
      ["cull-sum-picked", s.picked],
      ["cull-sum-unflagged", s.unflagged],
      ["cull-sum-rejected", s.rejected],
      ["cull-sum-keepers", s.keepers],
    ] as const) {
      await page.getByTestId(id).click();
      expect(await expectReadoutMatchesGrid(page), id).toBe(n);
      await expect(page.getByTestId(id)).toHaveClass(/ring-1/);
      await page.getByTestId(id).click(); // toggles off
      expect(await expectReadoutMatchesGrid(page), `${id} off`).toBe(s.total);
    }

    // Rejecting one more photo updates the summary (debounced) without any reload.
    await page.getByTestId("cell-3").click();
    await page.keyboard.press("x");
    await expect(page.getByTestId("cull-sum-rejected").locator("b")).toHaveText(String(s.rejected + 1));
    await expect(page.getByTestId("cull-sum-keepers").locator("b")).toHaveText(String(s.keepers - 1));
    await expect(page.getByTestId("step-cull-sub")).toHaveText(`· ${s.keepers - 1} keepers`);
  });

  test("keeper rule menu lists 'Everything not rejected' first and the formula follows the rule", async ({ page }) => {
    await openProject(page);
    await page.getByTestId("cull-sum-rule").click();
    const items = page.getByTestId("keeper-rule-menu").locator("button[role=menuitemradio]");
    await expect(items.first()).toHaveAttribute("data-testid", "keeper-rule-4");
    await expect(items.first()).toHaveAttribute("aria-checked", "true");
    await expect(items.nth(1)).toHaveAttribute("data-testid", "keeper-rule-0");
    await page.getByTestId("keeper-rule-1").click(); // Picks, 1★+
    await expect(page.getByTestId("cull-sum-formula")).toContainText("picked");
    await expect(page.getByTestId("cull-sum-formula")).toContainText("others are left out");
    // The Keepers number still equals what the filter shows.
    const k = Number(await page.getByTestId("cull-sum-keepers").locator("b").textContent());
    await page.getByTestId("cull-sum-keepers").click();
    expect(await expectReadoutMatchesGrid(page)).toBe(k);
  });

  test("Edit plan and Export dialog show the same formula with a link to change the rule", async ({ page }) => {
    await openProject(page);
    const k = Number(await page.getByTestId("cull-sum-keepers").locator("b").textContent());
    await expect(page.getByTestId("step-edit")).toHaveAttribute("title", new RegExp(`Keepers ${k} = `));
    await expect(page.getByTestId("continue-edit")).toHaveAttribute("title", new RegExp(`Keepers ${k} = `));
    await page.getByTestId("continue-edit").click();
    await expect(page.getByTestId("plan-view")).toBeVisible();
    await expect(page.getByTestId("plan-keeper-formula")).toContainText(`Keepers ${k} = `);
    await expect(page.getByTestId("plan-keeper-formula")).toContainText("rejected are left out");
    await page.getByTestId("plan-keepers-rule").click();
    await expect(page.getByTestId("keeper-rule-menu").locator("button[role=menuitemradio]").first()).toHaveAttribute("data-testid", "keeper-rule-4");
    await page.keyboard.press("Escape");
    await shot(page, "clarity-plan-formula");
    await page.getByTestId("step-export").click();
    await expect(page.getByTestId("export-keeper-formula")).toContainText(`Keepers ${k} = `);
    await page.getByTestId("export-keeper-rule").click();
    await page.getByTestId("keeper-rule-1").click();
    await expect(page.getByTestId("export-keeper-formula")).toContainText("others are left out");
  });
});

test.describe("rejected pile with reasons", () => {
  test("each rejected photo shows who rejected it and why; auto rejects and suggestions are told apart", async ({ page }) => {
    await openProject(page);
    // Suggested but not applied: unflagged photos the engine would reject carry a "Suggested:" line.
    await expect.poll(async () => page.locator('[data-testid^="suggested-reason-"]').count()).toBeGreaterThan(0);
    const sug = await page.locator('[data-testid^="suggested-reason-"]').first().textContent();
    expect(sug).toMatch(/^Sieve suggests reject · \S/);

    await page.getByTestId("cull-sum-rejected").click();
    await expectReadoutMatchesGrid(page);
    const cells = page.locator('[data-testid^="reject-reason-"]');
    await expect(cells.first()).toBeVisible();
    for (const el of await cells.all()) {
      await expect(el).toContainText("Rejected by you");
      await expect(el).toHaveAttribute("data-origin", "user");
      expect(await el.getAttribute("title")).toContain("Rejected by you");
    }
    // Apply suggestions (to everything in view) turns the suggested rejects into auto-rejects.
    await page.getByTestId("cull-sum-rejected").click(); // filter off
    await expectReadoutMatchesGrid(page);
    await page.getByTestId("more-menu").click();
    await expect(page.getByTestId("apply-info")).toContainText("only on photos you have not flagged or rated");
    await page.getByTestId("apply-suggestions").click();
    await expect(page.getByTestId("apply-explain")).toContainText("Rejected view");
    await page.getByTestId("apply-confirm").click();
    await expect(page.getByTestId("notice").last()).toContainText("Review the result in the Rejected view");
    await page.getByTestId("cull-sum-rejected").click();
    await expect.poll(async () => page.locator('[data-origin="auto"]').count()).toBeGreaterThan(0);
    const auto = page.locator('[data-testid^="reject-reason-"][data-origin="auto"]').first();
    await expect(auto).toContainText("Auto-rejected");
    expect(((await auto.getAttribute("title")) ?? "").length).toBeGreaterThan("Auto-rejected".length); // a reason follows
    await expect(page.getByTestId("cull-sum-auto")).not.toHaveText(/^Auto 0/);
    await shot(page, "clarity-rejected-pile");
    // Loupe info shows the reason too.
    const id = Number((await auto.getAttribute("data-testid"))!.replace("reject-reason-", ""));
    await page.getByTestId(`cell-${id}`).dblclick();
    await expect(page.getByTestId("loupe-reject-reason")).toContainText("Auto-rejected");
    await expect(page.getByTestId("loupe-reject-reason")).toHaveAttribute("data-origin", "auto");
    await shot(page, "clarity-loupe-reason");
    // A flag you set yourself is yours again (undo of the apply restores the origin exactly: covered by the mock snapshot).
    await page.keyboard.press("Escape");
    await page.getByTestId(`cell-${id}`).click();
    await page.keyboard.press("x");
    await expect(page.getByTestId(`reject-reason-${id}`)).toHaveAttribute("data-origin", "user");
  });
});

test.describe("hover text on icons", () => {
  async function untitled(page: Page, scope: string): Promise<string[]> {
    return page.evaluate((sel) => {
      const bad: string[] = [];
      document.querySelectorAll(`${sel} svg`).forEach((svg) => {
        if (svg.closest("[aria-hidden=true], [aria-hidden]")) return; // decorative
        const t = svg.closest("[title]") as HTMLElement | null;
        const a = svg.closest("[aria-label]") as HTMLElement | null;
        if (!(t?.getAttribute("title") ?? "").trim() || !(a?.getAttribute("aria-label") ?? t?.getAttribute("title") ?? "").trim()) bad.push(`${(svg.parentElement as HTMLElement)?.outerHTML.slice(0, 120)}`);
      });
      document.querySelectorAll(`${sel} [data-testid^="burst-badge"], ${sel} [data-testid^="scene-badge"], ${sel} [data-testid^="companion"], ${sel} [data-testid^="loupe-burst"]`).forEach((el) => {
        if (!(el.getAttribute("title") ?? "").trim()) bad.push(el.getAttribute("data-testid") ?? "?");
      });
      return bad;
    }, scope);
  }

  test("every icon and badge on cells, the loupe and the filmstrip has a title", async ({ page }) => {
    await openApp(page, 400, "&errors=1&autosync=0");
    await page.getByTestId("tag-blink").click(); // show tags, bursts, flags
    await expectReadoutMatchesGrid(page);
    await page.getByTestId("clear-filters").click();
    await expect(page.locator('[data-testid^="cell-"] svg').first()).toBeVisible();
    expect(await untitled(page, '[data-testid^="cell-"]')).toEqual([]);
    // Burst badge says how big the burst is and whether this frame is the keeper.
    const burst = page.locator('[data-testid^="burst-badge-"]').first();
    await expect(burst).toHaveAttribute("title", /Burst of \d+ — (best frame \(Sieve.s choice\)|not the best frame)/);
    // Tag chips carry the reason text.
    const tag = page.locator('[data-testid^="cell-"] span[title^="blink:"], [data-testid^="cell-"] span[title^="missed focus:"], [data-testid^="cell-"] span[title^="duplicate burst:"]').first();
    await expect(tag).toHaveAttribute("title", /: \S/);
    // Loupe info overlay and the Filmstrip.
    await page.getByTestId("cell-8").click();
    await page.keyboard.press("Space");
    await expect(page.getByTestId("info-overlay")).toBeVisible();
    expect(await untitled(page, '[data-testid="info-overlay"]')).toEqual([]);
    await expect(page.getByTestId("loupe-flag")).toHaveAttribute("title", /\S/);
    await page.keyboard.press("d");
    await expect(page.getByTestId("develop-view")).toBeVisible();
    await expect(page.getByTestId("filmstrip")).toBeVisible();
    expect(await untitled(page, '[data-testid="filmstrip"]')).toEqual([]);
    // Top bar, step bar and toolbars.
    await page.keyboard.press("g");
    expect(await untitled(page, '[data-testid="top-bar"]')).toEqual([]);
    expect(await untitled(page, '[data-testid="grid-toolbar"]')).toEqual([]);
    expect(await untitled(page, '[data-testid="filter-bar"]')).toEqual([]);
  });

  test("XMP status icons name the sidecar", async ({ page }) => {
    await openApp(page, 400, "&errors=1");
    // id 5 = sidecar not writable in the mock.
    await expect(page.getByTestId("xmp-error-5")).toHaveAttribute("title", /DSC00005\.xmp could not be written/);
  });

  test("project step bar and Cull summary icons have titles", async ({ page }) => {
    await openProject(page);
    expect(await untitled(page, '[data-testid="step-bar"]')).toEqual([]);
    expect(await untitled(page, '[data-testid="cull-summary"]')).toEqual([]);
    for (const s of ["cull", "edit", "export"]) expect(((await page.getByTestId(`step-${s}`).getAttribute("title")) ?? "").length).toBeGreaterThan(5);
  });
});

test.describe("sidecar refresh", () => {
  test("refresh_sidecars runs when the project opens and when the window regains focus (debounced)", async ({ page }) => {
    await openHome(page, 201, V18);
    await clearCalls(page);
    await page.getByTestId("project-open-1").click();
    await expect(page.getByTestId("grid-toolbar")).toBeVisible();
    await expect.poll(async () => (await calls(page, "refresh_sidecars")).length).toBe(1);
    expect((await calls(page, "refresh_sidecars"))[0].args).toEqual({ projectId: 1 });
    // A burst of focus events within 2 s collapses into nothing new...
    await page.evaluate(() => window.dispatchEvent(new Event("focus")));
    await page.waitForTimeout(200);
    expect((await calls(page, "refresh_sidecars")).length).toBe(1);
    // ...and one after the window has passed runs again.
    await page.waitForTimeout(2000);
    await page.evaluate(() => window.dispatchEvent(new Event("focus")));
    await expect.poll(async () => (await calls(page, "refresh_sidecars")).length).toBe(2);
  });
});
