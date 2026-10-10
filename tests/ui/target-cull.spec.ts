// Phase 9 UI: "Pick the best N" (target-count culling) against the mock backend (src/testing/mockTarget.ts).
// `?target=1` = a finished run on project 1 with open people questions, `?target=answered` = questions answered,
// no switch = no run until "Pick the best N" runs one (`window.__mockTargetDelay` ms).
import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openHome, shot } from "./helpers";

const T = "target-";

async function openProject(page: Page, query = "", id = 1) {
  await openHome(page, 201, query);
  await page.getByTestId(`project-open-${id}`).click();
  await expect(page.getByTestId("grid-toolbar")).toBeVisible();
}

/** Opens the Pick the best N overlay (the toolbar button) on a project with a finished run. */
async function openTarget(page: Page, query = "&target=1") {
  await openProject(page, query);
  await page.getByTestId("target-open").click();
  await expect(page.getByTestId("target-view")).toBeVisible();
}

async function tab(page: Page, id: "setup" | "people" | "review" | "second") {
  await page.getByTestId(`target-tab-${id}`).click();
  await expect(page.getByTestId("target-view")).toHaveAttribute("data-stage", id);
}

const inv = <T = unknown>(page: Page, cmd: string, args: Record<string, unknown> = {}) =>
  page.evaluate(([c, a]) => (window as any).__TAURI_INTERNALS__.invoke(c, a), [cmd, args] as const) as Promise<T>;

const attr = async (page: Page, testid: string, name: string) => Number((await page.getByTestId(testid).getAttribute(name)) ?? NaN);
type Sel = { imageId: number; choice: string; alternativeOf: number | null; locked: boolean };
const selOf = async (page: Page, id: number) => (await inv<Sel[]>(page, "get_image_selections", { ids: [id] }))[0];

test.describe("Pick the best N", () => {
  test("step 1: suggested count, run with progress, cancel, summary by shot type", async ({ page }) => {
    await openProject(page);
    await page.evaluate(() => ((window as any).__mockTargetDelay = 1500));
    await page.getByTestId("target-open").click();
    const count = page.getByTestId("target-count");
    // 101 photos: about a third, rounded.
    await expect(count).toHaveValue("30");
    await expect(page.getByTestId("target-suggest")).toContainText("101 photos");
    await shot(page, "t-01-setup");

    // Cancel keeps nothing; the form comes back.
    await page.getByTestId("target-run").click();
    await expect(page.getByTestId("target-progress")).toBeVisible();
    await expect(page.getByTestId("target-count")).toBeDisabled();
    await page.getByTestId("target-cancel").click();
    await expect(page.getByTestId("target-run-message")).toContainText("Stopped");
    await expect(page.getByTestId("target-summary")).toHaveCount(0);

    // A real run: the target is remembered per shoot type; progress shows, then the summary.
    await count.fill("24");
    await page.getByTestId("target-run").click();
    await expect(page.getByTestId("target-progress")).toBeVisible();
    await expect(page.getByTestId("target-progress-label")).toContainText("Choosing the best 24");
    await shot(page, "t-02-progress");
    const summary = page.getByTestId("target-summary");
    await expect(summary).toBeVisible();
    await expect(page.getByTestId("target-progress")).toHaveCount(0);
    const run = await inv<{ counts: { deliver: number; notSure: number; setAside: number; perShotType: { shotType: string; deliver: number }[] }; settings: { targetCount: number } }>(page, "get_target_run", { projectId: 1 });
    expect(run.settings.targetCount).toBe(24);
    await expect(page.getByTestId("target-sum-deliver")).toHaveText(String(run.counts.deliver));
    for (const s of run.counts.perShotType) await expect(page.getByTestId(`target-sum-shot-${s.shotType}`)).toHaveAttribute("data-deliver", String(s.deliver));
    await expect(page.getByTestId("target-sum-notsure")).toContainText(String(run.counts.notSure));
    await expect(page.getByTestId("target-sum-setaside")).toContainText(String(run.counts.setAside));
    await expect(page.getByTestId("target-next-people")).toBeVisible();
    await shot(page, "t-03-summary");

    // Closing and reopening: the last value is remembered for the shoot type (and the project's run is shown).
    await page.getByTestId("target-close").click();
    await expect(page.getByTestId("target-view")).toHaveCount(0);
    await expect(page.getByTestId("target-open")).toContainText(`Best ${run.counts.deliver}`);
    await page.evaluate(() => localStorage.removeItem("sieve.target.count.portrait"));
    expect(await page.evaluate(() => localStorage.getItem("sieve.target.count.wedding"))).toBe("24");
  });

  test("the offer shows first for a project that has not been culled", async ({ page }) => {
    await openHome(page, 201);
    // Project 2 starts culled in the mock: clear every flag to get an untouched project.
    const ids = await inv<number[]>(page, "list_image_ids", { query: { includeTags: [], excludeTags: [], tagMatch: "any", picks: [], minRating: null, maxRating: null, colorLabels: [], burstGroupId: null, sceneId: null, collapseBursts: false, folderId: null, projectId: 2, sort: "capture_time", sortDescending: false, offset: 0, limit: 500 } });
    await inv(page, "set_pick", { ids, pick: "unflagged" });
    await page.getByTestId("project-open-2").click();
    await expect(page.getByTestId("target-offer")).toBeVisible();
    await shot(page, "t-00-offer");
    await page.getByTestId("target-offer-open").click();
    await expect(page.getByTestId("target-view")).toHaveAttribute("data-stage", "setup");
    await page.getByTestId("target-close").click();
    await page.getByTestId("target-offer-dismiss").click();
    await expect(page.getByTestId("target-offer")).toHaveCount(0);
    await expect(page.getByTestId("target-open")).toBeVisible(); // the toolbar entry stays
  });

  test("people: confirm the couple, answer Y / N with the keyboard, re-run keeps decisions", async ({ page }) => {
    await openTarget(page);
    await tab(page, "people");
    await expect(page.getByTestId("target-people-badge")).toHaveText("4");
    const couple = page.getByTestId("target-couple");
    await expect(couple).toContainText("We think this is the couple");
    await expect(couple).toHaveAttribute("data-confirmed", "false");
    await expect(page.getByTestId("target-questions")).toHaveCount(0); // the couple comes first
    await shot(page, "t-04-couple");

    // "Change who" lets the user pick other people; Esc cancels it.
    await page.getByTestId("target-couple-change-btn").click();
    await expect(page.getByTestId("target-couple-change")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("target-couple-change")).toHaveCount(0);
    await expect(page.getByTestId("target-view")).toBeVisible();

    await page.keyboard.press("y");
    await expect(couple).toHaveAttribute("data-confirmed", "true");
    const cards = page.locator('[data-testid^="target-person-"][data-state]');
    await expect(cards).toHaveCount(4);
    await expect(cards.first()).toHaveAttribute("data-focused", "true");
    await shot(page, "t-05-questions");

    // Y, N, arrow back + Y (changes the answer), then N on the next card.
    await clearCalls(page);
    await page.keyboard.press("y");
    await expect(cards.nth(0)).toHaveAttribute("data-state", "yes");
    await expect(cards.nth(1)).toHaveAttribute("data-focused", "true");
    await page.keyboard.press("n");
    await expect(cards.nth(1)).toHaveAttribute("data-state", "no");
    await expect(cards.nth(2)).toHaveAttribute("data-focused", "true"); // on to the next card still waiting
    await page.keyboard.press("ArrowLeft");
    await page.keyboard.press("ArrowLeft");
    await expect(cards.nth(0)).toHaveAttribute("data-focused", "true");
    await page.keyboard.press("n");
    await expect(cards.nth(0)).toHaveAttribute("data-state", "no");
    await expect(cards.nth(2)).toHaveAttribute("data-focused", "true");
    await page.keyboard.press("y");
    await expect(cards.nth(2)).toHaveAttribute("data-state", "yes");
    await page.keyboard.press("n");
    await expect(cards.nth(3)).toHaveAttribute("data-state", "no");
    await expect(page.getByTestId("target-questions-count")).toHaveText("All answered");
    await expect(page.getByTestId("target-people-badge")).toHaveCount(0);
    const roles = (await calls(page, "set_person_role")).map((c) => c.args.role);
    expect(roles).toEqual(["important", "other", "other", "important", "other"]);
    await expect(page.getByTestId("target-rerun-note")).toContainText("keeps every keep, reject and swap");
    await shot(page, "t-06-answered");

    // Re-run with these people: back to step 1 with progress, the answers survive, the user's decisions are kept.
    await page.getByTestId("target-tab-review").click();
    const lockedId = await attr(page, "target-cur", "data-id");
    await page.keyboard.press("z"); // a user decision (locked)
    await expect.poll(async () => (await selOf(page, lockedId)).locked).toBe(true);
    await tab(page, "people");
    await clearCalls(page);
    await page.getByTestId("target-rerun").click();
    await expect(page.getByTestId("target-view")).toHaveAttribute("data-stage", "setup");
    await expect(page.getByTestId("target-summary")).toBeVisible();
    const rerun = await calls(page, "run_target_selection");
    expect(rerun).toHaveLength(1);
    expect(rerun[0].args.settings).toMatchObject({ targetCount: 800 });
    expect((await selOf(page, lockedId)).locked).toBe(true);
    const ov = await inv<{ people: { roleConfirmed: boolean; ask: boolean }[]; questions: number[] }>(page, "list_people", { projectId: 1 });
    expect(ov.questions).toHaveLength(0);
    expect(ov.people.filter((p) => p.ask && p.roleConfirmed)).toHaveLength(4);
    await expect(page.getByTestId("target-next-people")).toHaveCount(0); // nothing left to ask
  });

  test("people: change who the couple is", async ({ page }) => {
    await openTarget(page);
    await tab(page, "people");
    await page.getByTestId("target-couple-change-btn").click();
    const picks = page.locator('[data-testid^="target-couple-pick-"]');
    expect(await picks.count()).toBe(6);
    await picks.nth(0).click(); // deselect the first main
    await picks.nth(2).click(); // choose another person
    await clearCalls(page);
    await page.getByTestId("target-couple-save").click();
    await expect(page.getByTestId("target-couple-change")).toHaveCount(0);
    const set = await calls(page, "set_person_role");
    expect(set.map((c) => c.args.role)).toEqual(["other", "main", "main"]);
  });

  test("pass 1: alternatives strip, keys for cycle / swap / add / reject / keep, counts and undo", async ({ page }) => {
    await openTarget(page, "&target=answered");
    await tab(page, "review");
    const total0 = attr(page, "target-review", "data-total");
    const run = await inv<{ counts: { deliver: number } }>(page, "get_target_run", { projectId: 1 });
    expect(await total0).toBe(run.counts.deliver);
    await expect(page.getByTestId("target-review-count")).toHaveText(`0 of ${run.counts.deliver} reviewed`);
    // Each photo: shot type, moment label and a reason.
    await expect(page.getByTestId("target-cur-shot")).toHaveAttribute("data-shot", /couple|group|detail|candid/);
    await expect(page.getByTestId("target-cur-moment")).toContainText(/frame \d+ of \d+/);
    await expect(page.getByTestId("target-reasons")).toBeVisible();

    // Walk to the first photo with at least two alternatives.
    const strip = page.locator('[data-testid^="target-alt-"][data-rank]');
    for (let i = 0; i < 12 && (await strip.count()) < 2; i++) await page.keyboard.press("ArrowRight");
    expect(await strip.count()).toBeGreaterThanOrEqual(2);
    await expect(page.getByTestId("target-strip")).toContainText("Alternatives from this moment");
    await shot(page, "t-07-pass1");
    const reviewedAfterWalk = await page.getByTestId("target-review-count").innerText();
    expect(reviewedAfterWalk).not.toMatch(/^0 of/);

    // Tab cycles the alternatives: the selected one changes and the right pane follows.
    const first = strip.nth(0);
    const second = strip.nth(1);
    await expect(first).toHaveAttribute("data-selected", "true");
    await page.keyboard.press("Tab");
    await expect(second).toHaveAttribute("data-selected", "true");
    await expect(page.getByTestId("target-alt-view")).toHaveAttribute("data-id", String(await second.getAttribute("data-testid").then((t) => Number(t!.replace("target-alt-", "")))));
    await page.keyboard.press("Shift+Tab");
    await expect(first).toHaveAttribute("data-selected", "true");
    await page.keyboard.press("ArrowDown");
    await expect(second).toHaveAttribute("data-selected", "true");
    await page.keyboard.press("ArrowUp");

    // S swaps the shown alternative in: it becomes the picked photo, the old one its alternative #1; the count stays.
    const picked = await attr(page, "target-cur", "data-id");
    const altId = Number((await first.getAttribute("data-testid"))!.replace("target-alt-", ""));
    await page.keyboard.press("s");
    await expect(page.getByTestId("target-cur")).toHaveAttribute("data-id", String(altId));
    expect((await selOf(page, altId)).choice).toBe("deliver");
    const old = await selOf(page, picked);
    expect(old).toMatchObject({ choice: "alternative", alternativeOf: altId, locked: true });
    expect(await attr(page, "target-review", "data-total")).toBe(run.counts.deliver);
    await expect(page.getByTestId(`target-alt-${picked}`)).toBeVisible();

    // Undo (Cmd+Z): the old photo is picked again.
    await page.keyboard.press("Control+z");
    await expect(page.getByTestId("target-view")).toBeVisible();
    await expect.poll(async () => (await selOf(page, picked)).choice).toBe("deliver");
    expect((await selOf(page, altId)).choice).toBe("alternative");
    await expect(page.getByTestId("target-cur")).toHaveAttribute("data-id", String(picked)); // the pick is back where it was
    await page.getByTestId("target-tab-review").click();

    // A adds the alternative too: one more picked photo, counts update in the header and the reviewed counter.
    await expect.poll(async () => (await inv<{ counts: { deliver: number } }>(page, "get_target_run", { projectId: 1 })).counts.deliver).toBe(run.counts.deliver);
    const here = await attr(page, "target-cur", "data-id");
    const strip2 = page.locator('[data-testid^="target-alt-"][data-rank]');
    await expect(strip2.first()).toBeVisible();
    const addId = Number((await strip2.first().getAttribute("data-testid"))!.replace("target-alt-", ""));
    await page.keyboard.press("a");
    await expect.poll(async () => (await selOf(page, addId)).choice).toBe("deliver");
    await expect(page.getByTestId("target-review")).toHaveAttribute("data-total", String(run.counts.deliver + 1));
    await expect(page.getByTestId("target-header-count")).toHaveAttribute("data-deliver", String(run.counts.deliver + 1));
    expect(await attr(page, "target-cur", "data-id")).toBe(here);
    await page.keyboard.press("Control+z");
    await expect(page.getByTestId("target-review")).toHaveAttribute("data-total", String(run.counts.deliver));

    // X rejects: the photo moves to Set aside and the next one slides in; the flag is the user's. Z keeps and advances.
    const rejectId = await attr(page, "target-cur", "data-id");
    await page.keyboard.press("x");
    await expect.poll(async () => (await selOf(page, rejectId)).choice).toBe("set_aside");
    await expect(page.getByTestId("target-review")).toHaveAttribute("data-total", String(run.counts.deliver - 1));
    const nextId = await attr(page, "target-cur", "data-id");
    expect(nextId).not.toBe(rejectId);
    await page.keyboard.press("z");
    await expect.poll(async () => (await selOf(page, nextId)).locked).toBe(true);
    expect(await attr(page, "target-cur", "data-id")).not.toBe(nextId);
    const doneNow = Number((await page.getByTestId("target-review-count").innerText()).split(" ")[0]);
    expect(doneNow).toBeGreaterThan(1);
    // Undo twice: keep, then the reject.
    await page.keyboard.press("Control+z");
    await page.keyboard.press("Control+z");
    await expect.poll(async () => (await selOf(page, rejectId)).choice).toBe("deliver");
    await expect(page.getByTestId("target-review")).toHaveAttribute("data-total", String(run.counts.deliver));
  });

  test("pass 1 to the grid: Show in grid filters to the picks (chip, summary, helpers)", async ({ page }) => {
    await openTarget(page, "&target=answered");
    await tab(page, "review");
    const total = await attr(page, "target-review", "data-total");
    await page.getByTestId("target-show-picks").click();
    await expect(page.getByTestId("target-view")).toHaveCount(0);
    const chip = page.getByTestId("filter-target");
    await expect(chip).toBeVisible();
    await expect(chip).toContainText("Picks");
    await expect(page.getByTestId("readout-shown")).toHaveText(String(total));
    await expect(page.getByTestId("clear-filters")).toBeVisible(); // isFiltered knows targetChoices
    const lastList = (await calls(page, "list_image_ids")).at(-1)!;
    expect((lastList.args.query as { targetChoices: string[] }).targetChoices).toEqual(["deliver"]);
    await chip.click();
    await expect(chip).toHaveCount(0);
    await expect(page.getByTestId("readout-shown")).toHaveText("101");
  });

  test("pass 2: covered-by, skip / swap / add both / keep on single keys, auto-advance and prefetch", async ({ page }) => {
    await openTarget(page, "&target=answered");
    await tab(page, "second");
    const run = await inv<{ counts: { notSure: number; setAside: number } }>(page, "get_target_run", { projectId: 1 });
    const n = run.counts.notSure + run.counts.setAside;
    await expect(page.getByTestId("target-second")).toHaveAttribute("data-total", String(n));
    await expect(page.getByTestId("target-second-count")).toHaveText(`0 of ${n} reviewed`);
    const cur = () => attr(page, "target-second", "data-current");
    const covered = page.getByTestId("target-covered");
    const noCover = page.getByTestId("target-no-cover");

    // Skip (Space) advances on its own and counts as reviewed.
    await expect(covered.or(noCover)).toBeVisible();
    const id0 = await cur();
    await page.keyboard.press(" ");
    await expect.poll(cur).not.toBe(id0);
    await expect(page.getByTestId("target-second-count")).toHaveText(`1 of ${n} reviewed`);

    /** Steps with skip until a photo with (or without) a covered-by thumbnail shows. */
    const stepTo = async (withCover: boolean) => {
      for (let i = 0; i < n; i++) {
        await expect(covered.or(noCover)).toBeVisible();
        if ((await covered.count()) > 0 === withCover) return;
        await page.keyboard.press(" ");
      }
      throw new Error("no such photo");
    };

    await stepTo(true);
    await expect(page.getByTestId("target-covered-text")).toHaveText(/^Already kept a similar one: DSC\d+$/);
    await expect(page.getByTestId("target-similarity")).toContainText(/\d+% similar/);
    await expect(page.getByTestId("target-covered-pic").locator("img")).toBeVisible();
    await shot(page, "t-08-pass2");
    // Swap (S): keep this instead of the kept one.
    let coverId = attr(page, "target-covered", "data-id");
    let curId = await cur();
    let cid = await coverId;
    await clearCalls(page);
    await page.keyboard.press("s");
    await expect.poll(async () => (await calls(page, "swap_alternative")).length).toBe(1);
    expect((await calls(page, "swap_alternative"))[0].args).toMatchObject({ deliveredId: cid, alternativeId: curId });
    expect((await selOf(page, curId)).choice).toBe("deliver");
    expect(await selOf(page, cid)).toMatchObject({ choice: "alternative", alternativeOf: curId });
    await expect.poll(cur).not.toBe(curId); // auto-advanced

    // Add both (A).
    await stepTo(true);
    curId = await cur();
    await clearCalls(page);
    await page.keyboard.press("a");
    await expect.poll(async () => (await calls(page, "add_alternative")).length).toBe(1);
    expect((await calls(page, "add_alternative"))[0].args).toMatchObject({ imageId: curId });
    expect((await selOf(page, curId)).choice).toBe("deliver");
    await expect.poll(cur).not.toBe(curId);

    // Nothing similar kept: plain keep / skip.
    await stepTo(false);
    await expect(noCover).toContainText("No similar photo is kept");
    await expect(page.getByTestId("target-second-keep")).toBeVisible();
    await expect(page.getByTestId("target-second-swap")).toHaveCount(0);
    curId = await cur();
    await clearCalls(page);
    await page.keyboard.press("a");
    await expect.poll(async () => (await calls(page, "set_target_choice")).length).toBe(1);
    expect((await calls(page, "set_target_choice"))[0].args).toMatchObject({ ids: [curId], choice: "deliver" });
    await expect.poll(cur).not.toBe(curId);

    // Undo the keep.
    await page.keyboard.press("Control+z");
    await expect.poll(async () => (await selOf(page, curId)).choice).not.toBe("deliver");
    // Prefetch: the next photos' selections and covered-by were fetched ahead.
    expect((await calls(page, "get_covered_by")).length).toBeGreaterThan(2);
  });

  test("apply writes flags, then offers the picks as keepers", async ({ page }) => {
    await openTarget(page, "&target=answered");
    const run = await inv<{ counts: { deliver: number } }>(page, "get_target_run", { projectId: 1 });
    await page.getByTestId("target-apply").click();
    await expect(page.getByTestId("target-apply-text")).toContainText(`${run.counts.deliver} picked photos`);
    await expect(page.getByTestId("target-apply-text")).toContainText("never changed");
    await shot(page, "t-09-apply");
    await clearCalls(page);
    await page.getByTestId("target-apply-confirm").click();
    await expect(page.getByTestId("target-apply-result")).toContainText("as Picked");
    expect(await calls(page, "apply_target_selection")).toHaveLength(1);
    await expect(page.getByTestId("target-keeper-offer")).toBeVisible();
    await expect(page.getByTestId("target-keeper-explain")).toContainText("just the picks");
    await shot(page, "t-10-keeper-offer");
    await page.getByTestId("target-keeper-set").click();
    await expect(page.getByTestId("target-keeper-done")).toBeVisible();
    const rule = (await calls(page, "set_keeper_rule")).at(-1)!;
    expect(rule.args.rule).toEqual({ mode: "picks_and_ratings", minRating: 1, useSuggestions: false });
    await page.getByTestId("target-continue-edit").click();
    await expect(page.getByTestId("plan-view")).toBeVisible();
  });

  test("every control has a title; Help entry and cheat sheet list the keys", async ({ page }) => {
    await openTarget(page, "&target=1");
    const untitled = () =>
      page.evaluate(() =>
        [...document.querySelectorAll('[data-testid="target-view"] :is(button, input, select, [role=button]), [data-testid="target-apply-dialog"] :is(button, input, select)')]
          .filter((el) => !(el.getAttribute("title") ?? "").trim())
          .map((el) => el.getAttribute("data-testid") ?? el.outerHTML.slice(0, 80)),
      );
    for (const id of ["setup", "people", "review", "second"] as const) {
      await tab(page, id);
      await page.waitForTimeout(250);
      if (id === "people") await page.keyboard.press("y");
      expect(await untitled(), `stage ${id}`).toEqual([]);
    }
    await page.getByTestId("target-apply").click();
    await page.getByTestId("target-apply-confirm").click();
    await expect(page.getByTestId("target-keeper-offer")).toBeVisible();
    expect(await untitled(), "apply dialog").toEqual([]);
    await page.keyboard.press("Escape");

    // Cheat sheet (?) leads with the Pick the best N keys.
    await page.keyboard.press("?");
    const group = page.getByTestId("cheat-group-Pick the best N");
    await expect(group).toBeVisible();
    for (const id of ["targetKeep", "targetReject", "targetCycle", "targetSwap", "targetAdd", "targetSkip", "targetYes", "targetNo", "targetUndo"]) await expect(page.getByTestId(`cheat-${id}`)).toBeVisible();
    await shot(page, "t-11-cheat");
    await page.keyboard.press("Escape");

    // Help entry.
    await page.getByTestId("help-link-pick-best-n").click();
    await expect(page.getByTestId("help-panel")).toBeVisible();
    await expect(page.getByTestId("help-panel")).toContainText("Pick the best N");
    await expect(page.getByTestId("help-panel")).toContainText("Re-run with these people");
  });

  test("Esc leaves the overlay; the grid and Cull summary are back", async ({ page }) => {
    await openTarget(page);
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("target-view")).toHaveCount(0);
    await expect(page.getByTestId("grid-toolbar")).toBeVisible();
  });
});
