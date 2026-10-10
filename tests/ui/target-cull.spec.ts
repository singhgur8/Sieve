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
    await expect(page.getByTestId("target-rerun-note")).toContainText("keeps the picks you have reviewed and every keep, set-aside and swap");
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
    await expect(page.getByTestId("target-cur").getByTestId("target-reasons")).toBeVisible();

    // Walk to the first photo with at least two alternatives.
    const strip = page.locator('[data-testid^="target-alt-"][data-rank]');
    // (Past the first photo, so moving on has marked at least one reviewed.)
    await page.keyboard.press("ArrowRight");
    for (let i = 0; i < 12 && (await strip.count()) < 2; i++) await page.keyboard.press("ArrowRight");
    expect(await strip.count()).toBeGreaterThanOrEqual(2);
    await expect(page.getByTestId("target-strip")).toContainText(/This moment: \d+ picked · \d+ alternatives?/);
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

    // X sets aside: the photo moves to Set aside and the next one slides in; the flag is the user's. Z keeps and advances.
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
    const n = run.counts.notSure; // the default pile (P0-1)
    await expect(page.getByTestId("target-second")).toHaveAttribute("data-total", String(n));
    await expect(page.getByTestId("target-second-count")).toHaveText(`0 of ${n} reviewed`);
    const cur = () => attr(page, "target-second", "data-current");
    const covered = page.getByTestId("target-covered");
    const noCover = page.getByTestId("target-no-cover");

    // Skip (Space) advances on its own and counts as reviewed.
    await expect(covered.or(noCover).or(page.getByTestId("target-cover-thumb"))).toBeVisible();
    const id0 = await cur();
    await page.keyboard.press(" ");
    await expect.poll(cur).not.toBe(id0);
    await expect(page.getByTestId("target-second-count")).toHaveText(`1 of ${n} reviewed`);

    /** Steps with skip until a photo with (or without) a covered-by thumbnail shows. */
    const stepTo = async (withCover: boolean) => {
      for (let i = 0; i < n; i++) {
        await expect(covered.or(noCover).or(page.getByTestId("target-cover-thumb"))).toBeVisible(); // a weak cover (R1-1) shows the small thumb instead
        if ((await covered.count()) > 0 === withCover) return;
        await page.keyboard.press(" ");
      }
      throw new Error("no such photo");
    };

    await stepTo(true);
    await expect(page.getByTestId("target-covered-text")).toHaveText(/^(Almost identical to|Similar to|Same moment as|Looks like) DSC\d+ \((kept|you added it)(, another moment)?\)$/);
    await expect(page.getByTestId("target-similarity")).toContainText(/\d+% similar/);
    await expect(page.getByTestId("target-covered-pic").locator("img").last()).toBeVisible();
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

    // Prefetch: the next photos' selections and covered-by were fetched ahead.
    expect((await calls(page, "get_covered_by")).length).toBeGreaterThan(2);
    // Nothing similar kept (the backend now gives every Not sure photo a covered-by): the plain keep lives in the Weaker pile.
    await page.getByTestId("target-pile-weaker").click();
    await expect(page.locator('[data-testid^="target-cell-"]').first()).toBeVisible();
    curId = Number(await page.getByTestId("target-second").getAttribute("data-current"));
    await clearCalls(page);
    await page.keyboard.press("a");
    await expect.poll(async () => (await calls(page, "set_target_choice")).length).toBe(1);
    expect((await calls(page, "set_target_choice"))[0].args).toMatchObject({ ids: [curId], choice: "deliver" });

    // Undo the keep.
    await page.keyboard.press("Control+z");
    await expect.poll(async () => (await selOf(page, curId)).choice).not.toBe("deliver");
  });

  test("apply: exact counts from the plan, optional rejects, undo, then offers the picks as keepers", async ({ page }) => {
    await openTarget(page, "&target=answered");
    type Plan = { picks: number; rejects: number; rejectable: number; unflags: number; unchanged: number; userFlagged: number };
    const planOn = await inv<Plan>(page, "plan_target_apply", { projectId: 1, opts: { rejects: true } });
    const planOff = await inv<Plan>(page, "plan_target_apply", { projectId: 1, opts: { rejects: false } });
    await page.getByTestId("target-apply").click();
    const text = page.getByTestId("target-apply-text");
    await expect(text).toHaveAttribute("data-loading", "false");
    await expect(page.getByTestId("target-apply-picks")).toHaveAttribute("data-n", String(planOn.picks));
    await expect(page.getByTestId("target-apply-rejects")).toBeChecked();
    await expect(page.getByTestId("target-apply-rejects-label")).toHaveAttribute("data-n", String(planOn.rejectable));
    await expect(page.getByTestId("target-apply-note")).toContainText("XMP sidecars");
    await expect(page.getByTestId("target-apply-note")).toContainText("stay as they are");
    const n = (p: Plan) => p.picks + p.rejects + p.unflags;
    await expect(page.getByTestId("target-apply-confirm")).toHaveText(`Apply to ${n(planOn)} photos`);
    await shot(page, "t-09-apply");
    // Unticking the defects follows the plan without rejects.
    await page.getByTestId("target-apply-rejects").uncheck();
    await expect(page.getByTestId("target-apply-confirm")).toHaveText(`Apply to ${n(planOff)} photos`);
    await page.getByTestId("target-apply-rejects").check();
    await expect(page.getByTestId("target-apply-confirm")).toHaveText(`Apply to ${n(planOn)} photos`);
    await clearCalls(page);
    await page.getByTestId("target-apply-confirm").click();
    const result = page.getByTestId("target-apply-result");
    await expect(result).toContainText(`Picked ${planOn.picks.toLocaleString("en-US")} · Rejected ${planOn.rejects.toLocaleString("en-US")} · Unflagged ${planOn.unflags}`);
    const applied = await calls(page, "apply_target_selection");
    expect(applied).toHaveLength(1);
    expect(applied[0].args).toMatchObject({ projectId: 1, opts: { rejects: true } });
    await expect(page.getByTestId("target-keeper-offer")).toBeVisible();
    await expect(page.getByTestId("target-keeper-explain")).toContainText("just the picks");
    await shot(page, "t-10-keeper-offer");
    await page.getByTestId("target-keeper-set").click();
    await expect(page.getByTestId("target-keeper-done")).toBeVisible();
    const rule = (await calls(page, "set_keeper_rule")).at(-1)!;
    expect(rule.args.rule).toEqual({ mode: "picks_and_ratings", minRating: 1, useSuggestions: false });
    // Undo restores every flag the apply wrote (the plan is back to where it was).
    await page.getByTestId("target-apply-undo").click();
    await expect(result).toContainText("back as they were");
    expect(await calls(page, "restore_cull_snapshot")).toHaveLength(1);
    expect(await inv<Plan>(page, "plan_target_apply", { projectId: 1, opts: { rejects: true } })).toEqual(planOn);
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

// ---------------------------------------------------------------------------------------------------------------------
// UX review 9 (docs/ux-review-9.md): P0-1, P0-3, P1-1 ... P1-11
// ---------------------------------------------------------------------------------------------------------------------
type SelM = Sel & { momentId: number | null; coveredBy: number | null };
const selM = async (page: Page, id: number) => (await inv<SelM[]>(page, "get_image_selections", { ids: [id] }))[0];
const deliverIds = (page: Page, projectId = 1) =>
  inv<number[]>(page, "list_image_ids", { query: { includeTags: [], excludeTags: [], tagMatch: "any", picks: [], minRating: null, maxRating: null, colorLabels: [], burstGroupId: null, sceneId: null, collapseBursts: false, folderId: null, projectId, targetChoices: ["deliver"], sort: "capture_time", sortDescending: false, offset: 0, limit: 100000 } });
const runCounts = async (page: Page) => (await inv<{ counts: { deliver: number; notSure: number; setAside: number } }>(page, "get_target_run", { projectId: 1 })).counts;

async function openSecond(page: Page, query = "&target=answered", count = 201) {
  await openHome(page, count, query);
  await page.getByTestId("project-open-1").click();
  await expect(page.getByTestId("grid-toolbar")).toBeVisible();
  await page.getByTestId("target-open").click();
  await tab(page, "second");
  await expect(page.getByTestId("target-second")).toBeVisible();
}

test.describe("Pick the best N: UX review 9", () => {
  test("P0-1: Second look splits into four piles, Not sure by default, counts add up at scale", async ({ page }) => {
    await openSecond(page, "&target=answered", 5000);
    const c = await runCounts(page);
    await expect(page.getByTestId("target-second")).toHaveAttribute("data-pile", "not_sure");
    await expect(page.getByTestId("target-second")).toHaveAttribute("data-total", String(c.notSure));
    const counts = await Promise.all((["not_sure", "similar", "weaker", "defects"] as const).map((p) => attr(page, `target-pile-${p}`, "data-count")));
    expect(counts.reduce((a, b) => a + b, 0)).toBe(c.notSure + c.setAside);
    expect(counts[0]).toBe(c.notSure);
    for (const n of counts) expect(n).toBeGreaterThan(0);
    await expect(page.getByTestId("target-pile-similar")).toContainText("Similar to a kept photo");
    await shot(page, "t-20-piles");

    // Similar: one row per moment, the moment's kept photo on the left; only the rows near the viewport are mounted.
    await page.getByTestId("target-pile-similar").click();
    await expect(page.getByTestId("target-similar")).toBeVisible();
    const rows = await attr(page, "target-second", "data-rows");
    expect(rows).toBeGreaterThan(20);
    const mounted = page.locator('[data-testid^="target-sim-row-"]');
    await expect(mounted.first()).toBeVisible();
    expect(await mounted.count()).toBeLessThan(25);
    for (let i = 0; i < Math.min(5, await mounted.count()); i++) {
      const row = mounted.nth(i);
      await expect(row.locator('[data-testid="target-sim-kept-photo"]').first()).toBeVisible(); // its moment's delivered photo
      expect(await row.locator('[data-testid^="target-sim-frame-"]').count()).toBeGreaterThan(0);
    }
    await expect(page.locator('[data-testid="target-sim-pct"]').first()).toContainText("%");
    await shot(page, "t-21-similar");

    // A on the focused frame adds it (add_alternative); Shift+Right goes to the next moment.
    const focused = page.locator('[data-testid^="target-sim-frame-"][data-focused="true"]');
    await expect(focused.first()).toBeVisible();
    const fId = Number((await focused.first().getAttribute("data-testid"))!.replace("target-sim-frame-", ""));
    await clearCalls(page);
    await page.keyboard.press("a");
    await expect.poll(async () => (await selM(page, fId)).choice).toBe("deliver");
    expect((await calls(page, "add_alternative"))[0].args).toMatchObject({ imageId: fId });
    const momentOfFocus = async () => Number((await selM(page, Number((await page.getByTestId("target-second").getAttribute("data-current")) ?? 0))).momentId);
    const m0 = await momentOfFocus();
    await page.keyboard.press("Shift+ArrowRight");
    expect(await momentOfFocus()).not.toBe(m0);
    await expect(page.getByTestId("target-second-count")).toContainText("moments reviewed");

    // Weaker frames and Defects are plain grids with the reason on each cell, and say what Apply does with them.
    await page.getByTestId("target-pile-defects").click();
    await expect(page.getByTestId("target-pile-note")).toContainText("Defects are rejected when you apply. Weaker frames stay unflagged.");
    const cells = page.locator('[data-testid^="target-cell-"]');
    await expect(cells.first()).toBeVisible();
    for (const k of await cells.evaluateAll((els) => els.map((e) => e.getAttribute("data-kind")))) expect(["defect", "user_choice"]).toContain(k);
    await page.getByTestId("target-pile-weaker").click();
    await expect(cells.first()).toBeVisible();
    for (const k of await cells.evaluateAll((els) => els.map((e) => e.getAttribute("data-kind")))) expect(["defect", "user_choice", "near_duplicate", "not_best_of_setup"]).not.toContain(k);
  });

  test("P0-1: Not sure is ordered by moment; Shift+Space skips the rest of a moment; summary button shows the count", async ({ page }) => {
    await openHome(page, 201, "&target=answered");
    await page.getByTestId("project-open-1").click();
    await page.getByTestId("target-open").click();
    await tab(page, "setup");
    const c = await runCounts(page);
    await expect(page.getByTestId("target-next-second")).toContainText(`Second look · ${c.notSure} not sure`);
    await page.getByTestId("target-next-second").click();
    await expect(page.getByTestId("target-second")).toHaveAttribute("data-total", String(c.notSure));
    // Walk the pile: moments are contiguous (ordered by moment, then score).
    const seen: (number | null)[] = [];
    const curId = () => attr(page, "target-second", "data-current");
    await page.keyboard.press("Home");
    for (let i = 0; i < c.notSure; i++) {
      seen.push((await selM(page, await curId())).momentId);
      await page.keyboard.press("ArrowRight");
    }
    const closed = new Set<number | null>();
    seen.forEach((m, i) => {
      if (i > 0 && m !== seen[i - 1]) {
        expect(closed.has(m)).toBe(false);
        closed.add(seen[i - 1]);
      }
    });
    await page.keyboard.press("Home");
    const m0 = (await selM(page, await curId())).momentId;
    const inMoment = seen.filter((m) => m === m0).length;
    await page.keyboard.press("Shift+Space");
    expect((await selM(page, await curId())).momentId).not.toBe(m0);
    await expect(page.getByTestId("target-second")).toHaveAttribute("data-reviewed", String(inMoment));
  });

  test("P0-3: kept-from-this-moment strip marks what you added; the add button turns amber", async ({ page }) => {
    await openSecond(page);
    const curId = () => attr(page, "target-second", "data-current");
    await page.keyboard.press("Home");
    let prev = await curId();
    let found = false;
    for (let i = 0; i < 16 && !found; i++) {
      const before = await selM(page, prev);
      await clearCalls(page);
      await page.keyboard.press("a");
      await expect.poll(async () => (await selM(page, prev)).choice).toBe("deliver");
      await expect.poll(curId).not.toBe(prev);
      const now = await curId();
      // Covered-by is refetched after the edit.
      await expect.poll(async () => (await calls(page, "get_covered_by")).some((c) => c.args.imageId === now)).toBe(true);
      if ((await selM(page, now)).momentId === before.momentId && before.momentId != null) {
        found = true;
        const strip = page.getByTestId("target-kept-strip");
        await expect(strip.getByTestId(`target-kept-${prev}`)).toHaveAttribute("data-added", "true");
        await expect(strip.getByTestId(`target-kept-${prev}`)).toContainText("you added");
        const name = `DSC${String(prev).padStart(5, "0")}`;
        await expect(page.getByTestId("target-already-added")).toContainText(`You already added ${name} from this moment`);
        const n = Number(await strip.getAttribute("data-count"));
        const ord = (k: number) => `${k}${["th", "st", "nd", "rd"][k % 10 > 3 || [11, 12, 13].includes(k % 100) ? 0 : k % 10]}`;
        const add = page.getByTestId("target-second-add");
        await expect(add).toHaveAttribute("data-amber", "true");
        await expect(add).toContainText(`Add a ${ord(n + 1)} from this moment`);
        await shot(page, "t-22-added");
      }
      prev = now;
    }
    expect(found).toBe(true);
  });

  test("P1-1 / P1-2 / P1-3: moment strip, accept the moment, resume, unreviewed jumps, undo returns to the photo", async ({ page }) => {
    await openTarget(page, "&target=answered");
    await tab(page, "review");
    const list = await deliverIds(page);
    const cur = () => attr(page, "target-review", "data-current");
    // A moment with several picks: walk to it. The strip shows the other picks and the alternatives; the header says so.
    const moments = await inv<{ id: number; deliveredIds: number[]; imageIds: number[] }[]>(page, "list_moments", { projectId: 1 });
    const mOf = (id: number) => moments.find((m) => m.imageIds.includes(id))!;
    const multi = list.findIndex((id) => mOf(id).deliveredIds.length >= 2);
    expect(multi).toBeGreaterThanOrEqual(0);
    await page.keyboard.press("End");
    await page.keyboard.press("Home");
    for (let i = 0; i < multi; i++) await page.keyboard.press("ArrowRight");
    const here = await cur();
    const m = mOf(here);
    await expect(page.getByTestId("target-strip-title")).toContainText(`This moment: ${m.deliveredIds.length} picked`);
    await expect(page.locator('[data-testid^="target-pick-"]')).toHaveCount(m.deliveredIds.length - 1);
    await expect(page.getByTestId("target-review-moment-no")).toContainText(/Moment \d+ of \d+/);
    await shot(page, "t-23-moment");
    // Clicking another pick of the moment jumps to it.
    const other = m.deliveredIds.find((i) => i !== here)!;
    await page.getByTestId(`target-pick-${other}`).click();
    expect(await cur()).toBe(other);
    // Shift+Right: every pick of the moment is reviewed, and we are on the first pick of the next moment.
    await page.keyboard.press("Shift+ArrowRight");
    const after = await cur();
    expect(mOf(after).id).not.toBe(m.id);
    const lastOfMoment = Math.max(...m.deliveredIds.map((i) => list.indexOf(i)));
    expect(list.indexOf(after)).toBeGreaterThan(lastOfMoment);
    const doneSet = await page.evaluate(() => JSON.parse(localStorage.getItem("sieve.target.reviewed.review.1") ?? "[]") as number[]);
    for (const i of m.deliveredIds) expect(doneSet).toContain(i);
    await expect(page.getByTestId("target-review")).toHaveAttribute("data-reviewed", String(doneSet.length));

    // P1-2: close, reopen: the first pick not looked at yet. ] / [ / Home / End.
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("target-view")).toHaveCount(0);
    await page.getByTestId("target-open").click();
    await expect(page.getByTestId("target-view")).toHaveAttribute("data-stage", "review");
    expect(await cur()).toBe(list.find((i) => !doneSet.includes(i)));
    await page.keyboard.press("End");
    expect(await cur()).toBe(list[list.length - 1]);
    await page.keyboard.press("Home");
    expect(await cur()).toBe(list[0]);
    const done2 = await page.evaluate(() => JSON.parse(localStorage.getItem("sieve.target.reviewed.review.1") ?? "[]") as number[]);
    await page.keyboard.press("]");
    expect(await cur()).toBe(list.find((i) => !done2.includes(i)));

    // P1-3: X on a pick, move on three, Cmd+Z: back on that pick, the count is back.
    await page.keyboard.press("Home");
    for (let i = 0; i < 3; i++) await page.keyboard.press("ArrowRight");
    const victim = await cur();
    const total = await attr(page, "target-review", "data-total");
    await page.keyboard.press("x");
    await expect.poll(async () => (await selOf(page, victim)).choice).toBe("set_aside");
    await expect(page.getByTestId("target-review")).toHaveAttribute("data-total", String(total - 1));
    for (let i = 0; i < 3; i++) await page.keyboard.press("ArrowRight");
    expect(await cur()).not.toBe(victim);
    await page.keyboard.press("Control+z");
    await expect(page.getByTestId("target-review")).toHaveAttribute("data-total", String(total));
    await expect(page.getByTestId("target-review")).toHaveAttribute("data-current", String(victim));

    // Undo from another stage returns to the stage of the action.
    await page.keyboard.press("x");
    await expect.poll(async () => (await selOf(page, victim)).choice).toBe("set_aside");
    await tab(page, "second");
    await page.keyboard.press("Control+z");
    await expect(page.getByTestId("target-view")).toHaveAttribute("data-stage", "review");
    await expect(page.getByTestId("target-review")).toHaveAttribute("data-current", String(victim));
  });

  test("P1-3: undo in the Second look selects the photo again and removes its reviewed mark", async ({ page }) => {
    await openSecond(page);
    const curId = () => attr(page, "target-second", "data-current");
    await page.keyboard.press("Home");
    await page.keyboard.press("ArrowRight");
    const target = await curId();
    const sel = await selM(page, target);
    await page.keyboard.press(sel.choice === "not_sure" ? "x" : "a");
    await expect.poll(curId).not.toBe(target);
    await page.keyboard.press(" ");
    await page.keyboard.press(" ");
    await page.keyboard.press("Control+z");
    await expect.poll(curId).toBe(target);
    const rev = await page.evaluate(() => JSON.parse(localStorage.getItem("sieve.target.reviewed.second.1") ?? "[]") as number[]);
    expect(rev).not.toContain(target);
  });

  test("P1-4: reviewed picks are tracked and survive a re-run; the footer says so", async ({ page }) => {
    await openTarget(page, "&target=answered");
    await tab(page, "review");
    const list = await deliverIds(page);
    for (let i = 0; i < 5; i++) await page.keyboard.press("ArrowRight");
    const stored = async () => page.evaluate(() => JSON.parse(localStorage.getItem("sieve.target.reviewed.review.1") ?? "[]") as number[]);
    expect(await stored()).toEqual(list.slice(0, 5));
    await tab(page, "people");
    await expect(page.getByTestId("target-rerun-note")).toContainText("keeps the picks you have reviewed");
    await clearCalls(page);
    await page.getByTestId("target-rerun").click();
    await expect(page.getByTestId("target-summary")).toBeVisible();
    expect((await calls(page, "run_target_selection")).length).toBe(1);
    const nowDelivered = await deliverIds(page);
    await expect.poll(async () => (await stored()).every((i) => nowDelivered.includes(i))).toBe(true);
  });

  test("P1-5 / P1-6: X is Set aside (not Reject); Enter keeps in Review, skips in Second look, and never swaps", async ({ page }) => {
    await openTarget(page, "&target=answered");
    await tab(page, "review");
    await expect(page.getByTestId("target-reject")).toContainText("Set aside");
    await expect(page.getByTestId("target-reject")).not.toContainText("Reject");
    await expect(page.getByTestId("target-reject")).toHaveAttribute("title", /Nothing is rejected or deleted/);
    const strip = page.locator('[data-testid^="target-alt-"][data-rank]');
    for (let i = 0; i < 12 && (await strip.count()) < 1; i++) await page.keyboard.press("ArrowRight");
    await expect(strip.first()).toHaveAttribute("title", /Click to compare; S swaps it in/);
    const id = await attr(page, "target-review", "data-current");
    await clearCalls(page);
    await page.keyboard.press("Enter");
    await expect.poll(async () => (await calls(page, "set_target_choice")).length).toBe(1);
    expect((await calls(page, "set_target_choice"))[0].args).toMatchObject({ ids: [id], choice: "deliver" });
    expect(await calls(page, "swap_alternative")).toHaveLength(0);
    await tab(page, "second");
    await clearCalls(page);
    const cur = await attr(page, "target-second", "data-current");
    await page.keyboard.press("Enter");
    await expect.poll(() => attr(page, "target-second", "data-current")).not.toBe(cur);
    expect(await calls(page, "swap_alternative")).toHaveLength(0);
    expect(await calls(page, "set_target_choice")).toHaveLength(0);
    // The keymap says so.
    await page.keyboard.press("?");
    await expect(page.getByTestId("cheat-targetReject")).toContainText("Set aside");
    await expect(page.getByTestId("cheat-targetSwap")).not.toContainText("Enter");
  });

  test("P2: X in the Second look sets a Not sure photo aside and moves on; empty alternatives flash", async ({ page }) => {
    await openSecond(page);
    const curId = () => attr(page, "target-second", "data-current");
    await page.keyboard.press("Home");
    const id = await curId();
    expect((await selM(page, id)).choice).toBe("not_sure");
    await page.keyboard.press("x");
    await expect.poll(async () => (await selM(page, id)).choice).toBe("set_aside");
    await expect.poll(curId).not.toBe(id);
    await tab(page, "review");
    // A pick without alternatives flashes the strip's empty text on Tab / S / A.
    for (let i = 0; i < 40 && (await page.locator('[data-testid^="target-alt-"][data-rank]').count()) > 0; i++) await page.keyboard.press("ArrowRight");
    if ((await page.locator('[data-testid^="target-alt-"][data-rank]').count()) === 0) {
      await page.keyboard.press("Tab");
      await expect(page.getByTestId("target-strip-empty")).toHaveAttribute("data-flash", "true");
    }
  });

  test("P1-7: Space zooms both photos to the same point and keeps it across Right; Second look zooms by click", async ({ page }) => {
    await openTarget(page, "&target=answered");
    await tab(page, "review");
    const strip = page.locator('[data-testid^="target-alt-"][data-rank]');
    for (let i = 0; i < 12 && (await strip.count()) < 1; i++) await page.keyboard.press("ArrowRight");
    const a = page.getByTestId("target-cur-pic");
    const b = page.getByTestId("target-alt-pic");
    await expect(a).toHaveAttribute("data-zoom", "fit");
    await page.keyboard.press(" ");
    await expect(a).toHaveAttribute("data-zoom", "100");
    await expect(b).toHaveAttribute("data-zoom", "100");
    expect(await a.getAttribute("data-cx")).toBe(await b.getAttribute("data-cx"));
    expect(await a.getAttribute("data-cy")).toBe(await b.getAttribute("data-cy"));
    await shot(page, "t-24-zoom");
    await page.keyboard.press("ArrowRight");
    await expect(a).toHaveAttribute("data-zoom", "100");
    await page.keyboard.press(" ");
    await expect(a).toHaveAttribute("data-zoom", "fit");

    await tab(page, "second");
    for (let i = 0; i < 16 && (await page.getByTestId("target-covered").count()) === 0; i++) await page.keyboard.press(" ");
    const pic = page.getByTestId("target-second-pic");
    await expect(pic).toHaveAttribute("data-zoom", "fit");
    await pic.click();
    await expect(pic).toHaveAttribute("data-zoom", "100");
    await expect(page.getByTestId("target-covered-pic")).toHaveAttribute("data-zoom", "100");
    const id = await attr(page, "target-second", "data-current");
    await page.keyboard.press(" "); // Space stays Skip
    await expect.poll(() => attr(page, "target-second", "data-current")).not.toBe(id);
    await pic.click();
    await expect(pic).toHaveAttribute("data-zoom", "fit");
  });

  test("P1-8: at 1280x800 the Yes / No buttons of the focused person card stay above the footer", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await openTarget(page, "&target=1");
    await tab(page, "people");
    await page.keyboard.press("y"); // the couple
    const cards = page.locator('[data-testid^="target-person-"][data-state]');
    await expect(cards).toHaveCount(4);
    await shot(page, "t-25-people-1280");
    for (let i = 0; i < 4; i++) {
      const card = cards.nth(i);
      await expect(card).toHaveAttribute("data-focused", "true");
      await page.waitForTimeout(150);
      const foot = (await page.getByTestId("target-people-footer").boundingBox())!;
      for (const kind of ["yes", "no"]) {
        const bb = (await card.locator(`[data-testid^="target-person-${kind}-"]`).boundingBox())!;
        expect(bb.y + bb.height, `card ${i} ${kind}`).toBeLessThanOrEqual(foot.y + 1);
      }
      await page.keyboard.press(i % 2 ? "n" : "y");
    }
    const face = (await cards.first().locator('[data-testid^="target-person-face-"]').boundingBox())!;
    expect(face.width).toBeLessThanOrEqual(145);
  });

  test("P1-9: the Cull step shows the run (Best N row), B opens it, labelled button at 1280", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await openHome(page, 201, "&target=answered");
    await page.getByTestId("project-open-1").click();
    await expect(page.getByTestId("grid-toolbar")).toBeVisible();
    const c = await runCounts(page);
    const row = page.getByTestId("target-best-row");
    await expect(row).toBeVisible();
    await expect(row).toContainText(`${c.deliver} picked`);
    await expect(row).toContainText(`reviewed 0 of ${c.deliver}`);
    await expect(row).toContainText(`second look 0 of ${c.notSure}`);
    await expect(row).toContainText("not applied yet");
    await expect(page.getByTestId("target-offer")).toHaveCount(0);
    await expect(page.getByTestId("target-open")).toContainText(`Best ${c.deliver}`); // labelled at 1280
    await expect(page.getByTestId("target-open")).toHaveAttribute("title", /\(B\)/);
    await shot(page, "t-26-best-row");
    // Live counts after reviewing.
    await page.getByTestId("target-best-continue").click();
    await expect(page.getByTestId("target-view")).toHaveAttribute("data-stage", "review");
    for (let i = 0; i < 3; i++) await page.keyboard.press("ArrowRight");
    await page.keyboard.press("Escape");
    await expect(row).toContainText(`reviewed 3 of ${c.deliver}`);
    // B opens the overlay at the stage with unfinished work.
    await page.keyboard.press("b");
    await expect(page.getByTestId("target-view")).toHaveAttribute("data-stage", "review");
    await page.keyboard.press("Escape");
    // Apply flags… opens the dialog; afterwards the row says when.
    await page.getByTestId("target-best-apply").click();
    await expect(page.getByTestId("target-apply-dialog")).toBeVisible();
    await page.getByTestId("target-apply-confirm").click();
    await expect(page.getByTestId("target-apply-result")).toBeVisible();
    await page.keyboard.press("Escape");
    await page.keyboard.press("Escape");
    await expect(row).toContainText("applied");
    // The Suggestions chip says it is a different set (when it shows).
    const chip = page.getByTestId("cull-sum-suggest");
    if ((await chip.count()) > 0) await expect(chip).toContainText("from Best");
  });

  test("P1-9: a project with a few flags (under 5 %) still gets the offer; B opens Pick", async ({ page }) => {
    await openHome(page, 201);
    const ids = await inv<number[]>(page, "list_image_ids", { query: { includeTags: [], excludeTags: [], tagMatch: "any", picks: [], minRating: null, maxRating: null, colorLabels: [], burstGroupId: null, sceneId: null, collapseBursts: false, folderId: null, projectId: 2, sort: "capture_time", sortDescending: false, offset: 0, limit: 500 } });
    await inv(page, "set_pick", { ids, pick: "unflagged" });
    await inv(page, "set_pick", { ids: ids.slice(0, 2), pick: "pick" });
    await page.getByTestId("project-open-2").click();
    await expect(page.getByTestId("target-offer")).toBeVisible();
    await page.keyboard.press("b");
    await expect(page.getByTestId("target-view")).toHaveAttribute("data-stage", "setup");
  });

  test("P1-10: the summary explains a shortfall and offers the Second look with its count", async ({ page }) => {
    await openHome(page, 201, "&target=answered");
    await page.getByTestId("project-open-1").click();
    await page.getByTestId("target-open").click();
    await tab(page, "setup");
    const c = await runCounts(page);
    const target = (await inv<{ settings: { targetCount: number } }>(page, "get_target_run", { projectId: 1 })).settings.targetCount;
    expect(c.deliver).toBeLessThan(0.9 * target);
    await expect(page.getByTestId("target-shortfall")).toContainText(`${target - c.deliver} short of ${target}`);
    await expect(page.getByTestId("target-shortfall")).toContainText(`The ${c.notSure} Not sure are the closest`);
    await expect(page.getByTestId("target-next-review")).toContainText(`Review the ${c.deliver} picks`);
    await expect(page.getByTestId("target-next-second")).toContainText(`Second look · ${c.notSure} not sure`);
  });

  test("P1-11: toasts sit bottom-right (R1-4) while the overlay is open, one at a time", async ({ page }) => {
    await openTarget(page, "&target=answered");
    await tab(page, "review");
    await page.keyboard.press("x");
    const notice = page.getByTestId("notice");
    await expect(notice).toHaveCount(1);
    const bb = (await notice.boundingBox())!;
    const vp = page.viewportSize()!;
    expect(bb.y + bb.height).toBeGreaterThan(vp.height - 100); // R1-4: bottom right, above the action bar
    expect(bb.x + bb.width).toBeGreaterThan(vp.width - 30);
    await page.keyboard.press("x");
    await expect(notice).toHaveCount(1); // the newest replaces the older one
    await expect(notice).toBeHidden({ timeout: 5000 }); // gone after about 3 s
  });
});

test("P1-3: swap on photo 1, Cmd+Z returns to photo 1 (the swapped-in id is no longer delivered)", async ({ page }) => {
  await openTarget(page, "&target=answered");
  await tab(page, "review");
  await page.keyboard.press("Home");
  const first = await attr(page, "target-review", "data-current");
  const total = await attr(page, "target-review", "data-total");
  const strip = page.locator('[data-testid^="target-alt-"][data-rank]');
  await expect(strip.first()).toBeVisible();
  const alt = Number((await strip.first().getAttribute("data-testid"))!.replace("target-alt-", ""));
  await page.keyboard.press("s");
  await expect(page.getByTestId("target-review")).toHaveAttribute("data-current", String(alt));
  await page.keyboard.press("Control+z");
  await expect.poll(async () => (await selOf(page, alt)).choice).toBe("alternative");
  await expect(page.getByTestId("target-review")).toHaveAttribute("data-current", String(first));
  await expect(page.getByTestId("target-review")).toHaveAttribute("data-total", String(total));
});

// ---------------------------------------------------------------------------------------------------------------------
// UX re-check 1 (docs/ux-review-9.md "Re-check 1"): R1-1 ... R1-9, on the 5,000-photo mock
// ---------------------------------------------------------------------------------------------------------------------
type Cov = { tier: string; sameMoment: boolean; similarity: number; coveredById: number; coveredByName: string } | null;
const notSureIds = (page: Page) =>
  inv<number[]>(page, "list_image_ids", { query: { includeTags: [], excludeTags: [], tagMatch: "any", picks: [], minRating: null, maxRating: null, colorLabels: [], burstGroupId: null, sceneId: null, collapseBursts: false, folderId: null, projectId: 1, targetChoices: ["not_sure"], sort: "target_moment", sortDescending: false, offset: 0, limit: 100000 } });

/** Moves the Not sure pile to the index-th photo. */
async function gotoNotSure(page: Page, index: number) {
  await page.keyboard.press("Home");
  for (let i = 0; i < index; i++) await page.keyboard.press("ArrowRight");
}

test.describe("Pick the best N: UX re-check 1", () => {
  test("R1-1: a weak cover from another moment gives the photo the full width, disables S and labels A Keep", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await openSecond(page, "&target=answered", 5000);
    const ids = await notSureIds(page);
    let weak = -1;
    let strong = -1;
    for (let i = 0; i < ids.length && (weak < 0 || strong < 0); i++) {
      const c = await inv<Cov>(page, "get_covered_by", { imageId: ids[i] });
      if (c && c.tier === "another_moment" && c.similarity < 0.7 && weak < 0) weak = i;
      if (c && c.sameMoment && strong < 0) strong = i;
    }
    expect(weak).toBeGreaterThanOrEqual(0);
    await gotoNotSure(page, weak);
    const id = ids[weak];
    await expect(page.getByTestId("target-second")).toHaveAttribute("data-current", String(id));
    const cov = (await inv<Cov>(page, "get_covered_by", { imageId: id }))!;
    await expect(page.getByTestId("target-covered")).toHaveCount(0);
    const pic = (await page.getByTestId("target-second-pic").boundingBox())!;
    expect(pic.width).toBeGreaterThan(1280 - 60);
    const thumb = page.getByTestId("target-cover-thumb");
    await expect(thumb).toBeVisible();
    await expect(page.getByTestId("target-cover-note")).toContainText(cov.coveredByName);
    await expect(page.getByTestId("target-cover-note")).toContainText("another moment");
    await expect(page.getByTestId("target-cover-note")).toContainText(`${Math.round(cov.similarity * 100)}%`);
    const tb = (await thumb.locator("> div").boundingBox())!;
    expect(Math.round(tb.width)).toBe(48);
    expect(Math.round(tb.height)).toBe(32);
    await shot(page, "t-30-weak-cover");

    // A is "Keep" (not "Add both"); S is disabled and its key only flashes the hint.
    await expect(page.getByTestId("target-second-keep")).toContainText("Keep");
    await expect(page.getByTestId("target-second-add")).toHaveCount(0);
    await expect(page.getByTestId("target-second-swap")).toBeDisabled();
    const before = await selM(page, id);
    await clearCalls(page);
    await page.keyboard.press("s");
    await expect(page.getByTestId("target-second-hint")).toContainText(`${cov.coveredByName} is the pick of another moment. A keeps this one too`);
    expect(await calls(page, "swap_alternative")).toHaveLength(0);
    expect((await selM(page, id)).choice).toBe(before.choice);
    expect((await selM(page, cov.coveredById)).choice).toBe("deliver");
    await expect(page.getByTestId("target-second")).toHaveAttribute("data-current", String(id));

    // Clicking the thumb shows the two panes; clicking it again hides the kept photo.
    await thumb.click();
    await expect(page.getByTestId("target-covered")).toBeVisible();
    await thumb.click();
    await expect(page.getByTestId("target-covered")).toHaveCount(0);

    // A same-moment cover still shows both panes, with the swap on.
    if (strong >= 0) {
      await gotoNotSure(page, strong);
      await expect(page.getByTestId("target-covered")).toBeVisible();
      await expect(page.getByTestId("target-cover-thumb")).toHaveCount(0);
      await expect(page.getByTestId("target-second-swap")).toBeEnabled();
    }
  });

  test("R1-2: the Review moment grid (G): rows per moment, Shift+Right accepts a row, Enter opens a cell in two-up", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await openHome(page, 5000, "&target=answered");
    await page.getByTestId("project-open-1").click();
    await page.getByTestId("target-open").click();
    await tab(page, "review");
    const root = page.getByTestId("target-review");
    await expect(root).toHaveAttribute("data-view", "two");
    await expect(page.getByTestId("target-showing")).toContainText("Showing");
    await expect(page.getByTestId("target-grid-toggle")).toContainText("Moment grid");
    const list = await deliverIds(page);
    await page.keyboard.press("g");
    await expect(root).toHaveAttribute("data-view", "grid");
    await expect(page.getByTestId("target-grid-toggle")).toContainText("Two-up");
    await expect(page.getByTestId("target-grid-row-0")).toBeVisible();
    const focused = page.locator('[data-testid^="target-grid-cell-"][data-focused="true"]');
    await expect(focused).toHaveCount(1);
    await shot(page, "t-31-moment-grid");

    // At 1280x800 at least 3 full rows are visible; the row header names the moment; cell sizes follow the spec.
    const sc = (await page.getByTestId("target-grid").boundingBox())!;
    const rowBoxes = await page.locator('[data-testid^="target-grid-row-"]').evaluateAll((els) => els.map((e) => e.getBoundingClientRect()).map((r) => ({ top: r.top, bottom: r.bottom })));
    expect(rowBoxes.filter((r) => r.top >= sc.y - 1 && r.bottom <= sc.y + sc.height + 1).length).toBeGreaterThanOrEqual(3);
    await expect(page.getByTestId("target-grid-head-0")).toContainText(/Moment 1 of \d+ · .*\d+ picked · \d+ alternatives?/);
    const cell = (await page.locator('[data-testid^="target-grid-cell-"][data-kind="pick"]').first().boundingBox())!;
    expect(Math.round(cell.width)).toBe(240);
    expect(Math.round(cell.height)).toBe(160);
    const altBox = await page.locator('[data-testid^="target-grid-cell-"][data-kind="alt"]').first().locator("> div").first().boundingBox();
    if (altBox) {
      expect(Math.round(altBox.width)).toBe(160);
      expect(Math.round(altBox.height)).toBe(112);
    }
    // The focus is an outline, not a box-shadow ring.
    expect(await focused.evaluate((e) => getComputedStyle(e).outlineStyle)).toBe("solid");

    // Walk down to a row with 3 picks. Shift+Right marks all three reviewed and focuses the next row.
    const rowOf = () => focused.evaluate((e) => Number(e.closest('[data-testid^="target-grid-row-"]')!.getAttribute("data-testid")!.replace("target-grid-row-", "")));
    let guard = 0;
    while (guard++ < 60 && !/ 3 picked/.test((await page.getByTestId(`target-grid-head-${await rowOf()}`).textContent()) ?? "")) await page.keyboard.press("ArrowDown");
    const n = await rowOf();
    await expect(page.getByTestId(`target-grid-head-${n}`)).toContainText("3 picked");
    const done0 = await attr(page, "target-review", "data-reviewed");
    const rowPickIds = await page.getByTestId(`target-grid-row-${n}`).locator('[data-kind="pick"]').evaluateAll((els) => els.map((e) => Number(e.getAttribute("data-testid")!.replace("target-grid-cell-", ""))));
    expect(rowPickIds).toHaveLength(3);
    const had = await page.evaluate(() => JSON.parse(localStorage.getItem("sieve.target.reviewed.review.1") ?? "[]") as number[]);
    const added = rowPickIds.filter((i) => !had.includes(i)).length;
    await page.keyboard.press("Shift+ArrowRight");
    await expect(root).toHaveAttribute("data-reviewed", String(done0 + added));
    expect(await rowOf()).toBe(n + 1);
    await expect(page.getByTestId(`target-grid-row-${n}`)).toHaveAttribute("data-reviewed", "true");

    // Enter on a pick: back in two-up on that pick. G again; Right moves to the next cell; G opens it.
    const pickId = Number((await focused.getAttribute("data-testid"))!.replace("target-grid-cell-", ""));
    expect(list).toContain(pickId);
    await page.keyboard.press("Enter");
    await expect(root).toHaveAttribute("data-view", "two");
    await expect(root).toHaveAttribute("data-current", String(pickId));
    await page.keyboard.press("g");
    await expect(root).toHaveAttribute("data-view", "grid");
    await page.keyboard.press("ArrowRight");
    const next = Number((await focused.getAttribute("data-testid"))!.replace("target-grid-cell-", ""));
    const kind = await focused.getAttribute("data-kind");
    await page.keyboard.press("g");
    await expect(root).toHaveAttribute("data-view", "two");
    if (kind === "pick") await expect(root).toHaveAttribute("data-current", String(next));
    else await expect(page.getByTestId("target-alt-view")).toHaveAttribute("data-id", String(next));

    // X sets the focused pick aside, A adds the focused alternative (grid keys).
    await page.keyboard.press("g");
    await expect(root).toHaveAttribute("data-view", "grid");
    const total = await attr(page, "target-review", "data-total");
    const victim = Number((await focused.getAttribute("data-testid"))!.replace("target-grid-cell-", ""));
    if ((await focused.getAttribute("data-kind")) === "pick") {
      await page.keyboard.press("x");
      await expect.poll(async () => (await selOf(page, victim)).choice).toBe("set_aside");
      await expect(root).toHaveAttribute("data-total", String(total - 1));
    }
    const altCell = page.locator('[data-testid^="target-grid-cell-"][data-kind="alt"]').first();
    await expect(altCell).toBeVisible();
    const altId = Number((await altCell.getAttribute("data-testid"))!.replace("target-grid-cell-", ""));
    await altCell.click();
    await page.keyboard.press("a");
    await expect.poll(async () => (await selOf(page, altId)).choice).toBe("deliver");

    // G toggles back to the two-up view.
    await page.keyboard.press("g");
    await expect(root).toHaveAttribute("data-view", "two");
  });

  test("R1-3: Similar pile focus outline, up to 3 kept tiles plus +N, you added, the amber line, nearest-kept label", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await openSecond(page, "&target=answered", 5000);
    await page.getByTestId("target-pile-similar").click();
    await expect(page.getByTestId("target-similar")).toBeVisible();
    // The focus outline of the first frame shows on all four sides (the frames row has padding for it).
    const first = page.locator('[data-testid^="target-sim-frame-"][data-focused="true"]').first();
    await expect(first).toBeVisible();
    const style = await first.evaluate((e) => {
      const s = getComputedStyle(e);
      return { style: s.outlineStyle, width: s.outlineWidth, offset: s.outlineOffset, shadow: s.boxShadow };
    });
    expect(style).toMatchObject({ style: "solid", width: "2px", offset: "2px" });
    expect(style.shadow).toBe("none");
    const fb = (await first.boundingBox())!;
    const box = (await first.locator("xpath=..").boundingBox())!;
    expect(fb.x - 4).toBeGreaterThanOrEqual(box.x - 0.5);
    expect(fb.y - 4).toBeGreaterThanOrEqual(box.y - 0.5);
    expect(fb.y + fb.height + 4).toBeLessThanOrEqual(box.y + box.height + 0.5);
    await shot(page, "t-32-similar-focus");

    // A mounted row with 3 kept photos shows 3 tiles; one with at least two frames lets us add one.
    const rowInfo = await page.locator('[data-testid^="target-sim-row-"]').evaluateAll((els) =>
      els.map((e) => ({
        n: Number(e.getAttribute("data-testid")!.replace("target-sim-row-", "")),
        kept: Number(e.querySelector('[data-testid^="target-sim-kept-"]')?.getAttribute("data-count") ?? 0),
        frames: e.querySelectorAll('[data-testid^="target-sim-frame-"]').length,
        tiles: e.querySelectorAll('[data-testid="target-sim-kept-photo"]').length,
      })),
    );
    const three = rowInfo.find((r) => r.kept === 3 && r.frames >= 2);
    expect(three).toBeTruthy();
    expect(three!.tiles).toBe(3);
    const row = page.getByTestId(`target-sim-row-${three!.n}`);
    // Nearest-kept label on rows with 2 or more kept photos.
    await expect(row.locator('[data-testid="target-sim-pct"]').first()).toContainText(/\d+% · like DSC\d+/);

    // A on the first frame of the row: the next frame of that row shows the amber add and the "you added" tile.
    const frames = row.locator('[data-testid^="target-sim-frame-"]');
    const f0 = Number((await frames.nth(0).getAttribute("data-testid"))!.replace("target-sim-frame-", ""));
    const f1 = Number((await frames.nth(1).getAttribute("data-testid"))!.replace("target-sim-frame-", ""));
    await frames.nth(0).click();
    await expect(page.getByTestId("target-second-add")).toHaveCount(0); // nothing added yet: the plain button
    await page.keyboard.press("a");
    await expect.poll(async () => (await selM(page, f0)).choice).toBe("deliver");
    await expect(page.getByTestId("target-second")).toHaveAttribute("data-current", String(f1));
    const add = page.getByTestId("target-second-add");
    await expect(add).toHaveAttribute("data-amber", "true");
    await expect(add).toContainText("Add a 5th from this moment");
    await expect(page.getByTestId("target-already-added")).toContainText(`You already added DSC${String(f0).padStart(5, "0")} from this moment`);
    // 4 kept now: 3 tiles (the one you added first) and a "+1 more" tile that opens all of them.
    const tiles = row.locator('[data-testid="target-sim-kept-photo"]');
    await expect(tiles).toHaveCount(3);
    await expect(tiles.first()).toHaveAttribute("data-added", "true");
    await expect(tiles.first()).toContainText("you added");
    const more = page.getByTestId(`target-sim-kept-more-${three!.n}`);
    await expect(more).toContainText("+1 more");
    await more.click();
    await expect(page.getByTestId(`target-sim-kept-all-${three!.n}`).locator("> [data-id]")).toHaveCount(4);
    await shot(page, "t-33-similar-added");
  });

  test("R1-4: toasts sit bottom right above the action bar and leave the sub-header buttons free", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await openTarget(page, "&target=answered");
    await tab(page, "review");
    await page.keyboard.press("x");
    const notice = page.getByTestId("notice");
    await expect(notice).toHaveCount(1);
    const bb = (await notice.boundingBox())!;
    const vp = page.viewportSize()!;
    expect(Math.round(vp.height - (bb.y + bb.height))).toBeGreaterThanOrEqual(60);
    expect(Math.round(vp.height - (bb.y + bb.height))).toBeLessThanOrEqual(80);
    expect(Math.round(vp.width - (bb.x + bb.width))).toBeLessThanOrEqual(16);
    expect(bb.width).toBeLessThanOrEqual(361);
    const sec = (await page.getByTestId("target-to-second").boundingBox())!;
    expect(bb.y).toBeGreaterThan(sec.y + sec.height);
    await shot(page, "t-34-toast");
  });

  test("R1-5: plural counts are formatted at scale (2,500 photos)", async ({ page }) => {
    await openHome(page, 5000, "&target=answered");
    await page.getByTestId("project-open-1").click();
    await page.getByTestId("target-open").click();
    await page.getByTestId("target-apply").click();
    await expect(page.getByTestId("target-apply-text")).toHaveAttribute("data-loading", "false");
    const plan = await inv<{ picks: number; rejects: number; unchanged: number; userFlagged: number }>(page, "plan_target_apply", { projectId: 1, opts: { rejects: true } });
    await page.getByTestId("target-apply-confirm").click();
    const kept = plan.unchanged + plan.userFlagged;
    const text = (await page.getByTestId("target-apply-result").textContent())!;
    expect(text).toContain(`${kept.toLocaleString("en-US")} ${kept === 1 ? "photo" : "photos"} left as they were`);
    expect(text).not.toMatch(/\d{4,} photos/);
    expect(kept).toBeGreaterThanOrEqual(1000);
  });

  test("R1-5: the Already kept caption reads 92%, not 92 %", async ({ page }) => {
    await openSecond(page);
    const curId = () => attr(page, "target-second", "data-current");
    await page.keyboard.press("Home");
    let prev = await curId();
    for (let i = 0; i < 16; i++) {
      const before = await selM(page, prev);
      await page.keyboard.press("a");
      await expect.poll(curId).not.toBe(prev);
      const now = await curId();
      if ((await selM(page, now)).momentId === before.momentId && before.momentId != null) {
        const caption = page.getByTestId("target-already-added");
        await expect(caption).toBeVisible();
        const txt = (await caption.textContent())!;
        expect(txt).not.toMatch(/\d %/);
        if (txt.includes("Already kept")) expect(txt).toMatch(/Already kept: DSC\d+ \(\d+%\)/);
        return;
      }
      prev = now;
    }
    expect(false).toBe(true);
  });

  test("R1-6: the Apply dialog says how many picks already have the flag", async ({ page }) => {
    await openTarget(page, "&target=answered");
    const ids = await deliverIds(page);
    await inv(page, "set_pick", { ids: ids.slice(0, 3), pick: "picked" });
    await page.getByTestId("target-apply").click();
    await expect(page.getByTestId("target-apply-text")).toHaveAttribute("data-loading", "false");
    const picks = await attr(page, "target-apply-picks", "data-n");
    const other = (await attr(page, "target-header-count", "data-deliver")) - picks; // the count the dialog gets: the header one
    expect(other).toBeGreaterThan(0);
    await expect(page.getByTestId("target-apply-already")).toContainText(`(the other ${other} ${other === 1 ? "pick already has" : "picks already have"} it)`);
  });

  test("R1-7: Shift+Z accepts the rest of a moment in Review but does nothing in the Second look", async ({ page }) => {
    await openTarget(page, "&target=answered");
    await tab(page, "review");
    await page.keyboard.press("Home");
    const cur = await attr(page, "target-review", "data-current");
    await page.keyboard.press("Shift+Z");
    await expect.poll(() => attr(page, "target-review", "data-current")).not.toBe(cur);
    await tab(page, "second");
    await page.keyboard.press("Home");
    const id = await attr(page, "target-second", "data-current");
    const rev = await attr(page, "target-second", "data-reviewed");
    await clearCalls(page);
    await page.keyboard.press("Shift+Z");
    expect(await attr(page, "target-second", "data-current")).toBe(id);
    expect(await attr(page, "target-second", "data-reviewed")).toBe(rev);
    expect(await calls(page, "set_target_choice")).toHaveLength(0);
    expect(await calls(page, "add_alternative")).toHaveLength(0);
    // Shift+Space still skips the moment.
    await page.keyboard.press("Shift+Space");
    expect(await attr(page, "target-second", "data-reviewed")).toBeGreaterThan(rev);
  });

  test("R1-8: opening Pick the best N with B puts the cursor in the count; B, type, Enter runs it", async ({ page }) => {
    await openProject(page);
    await page.keyboard.press("b");
    await expect(page.getByTestId("target-view")).toHaveAttribute("data-stage", "setup");
    const count = page.getByTestId("target-count");
    await expect(count).toBeFocused();
    await page.keyboard.type("40");
    await expect(count).toHaveValue("40"); // the suggested value was selected, so typing replaces it
    await clearCalls(page);
    await page.keyboard.press("Enter");
    await expect.poll(async () => (await calls(page, "run_target_selection")).length).toBe(1);
  });

  test("R1-9: E shows the focused Similar frame large next to its nearest kept photo; E or Esc returns", async ({ page }) => {
    await openSecond(page, "&target=answered", 5000);
    await page.getByTestId("target-pile-similar").click();
    await expect(page.getByTestId("target-similar")).toBeVisible();
    await expect(page.getByTestId("target-sim-large")).toHaveCount(0);
    const fid = await attr(page, "target-second", "data-current");
    await page.keyboard.press("e");
    const large = page.getByTestId("target-sim-large");
    await expect(large).toBeVisible();
    await expect(large).toHaveAttribute("data-id", String(fid));
    await expect(page.getByTestId("target-sim-large-pic")).toBeVisible();
    await expect(page.getByTestId("target-sim-large-kept")).toBeVisible();
    await shot(page, "t-35-similar-large");
    // Click zooms both.
    await page.getByTestId("target-sim-large-pic").click();
    await expect(page.getByTestId("target-sim-large-pic")).toHaveAttribute("data-zoom", "100");
    await expect(page.getByTestId("target-sim-large-kept")).toHaveAttribute("data-zoom", "100");
    await page.keyboard.press("e");
    await expect(large).toHaveCount(0);
    await expect(page.getByTestId("target-similar")).toBeVisible();
    // Esc leaves the large view first, and only the second Esc closes the overlay.
    await page.keyboard.press("e");
    await expect(large).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(large).toHaveCount(0);
    await expect(page.getByTestId("target-view")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("target-view")).toHaveCount(0);
  });

  test("keys: G does nothing in the Second look and E does nothing in Not sure", async ({ page }) => {
    await openTarget(page, "&target=answered");
    await tab(page, "second");
    await page.keyboard.press("g");
    await page.keyboard.press("e");
    await expect(page.getByTestId("target-sim-large")).toHaveCount(0);
    await expect(page.getByTestId("target-second")).toHaveAttribute("data-pile", "not_sure");
  });

  // ---- UX re-check 2 ----
  async function wheelAndKeys(page: Page, listId: string, key: string, n = 10) {
    const list = page.getByTestId(listId).first();
    await expect(list).toBeVisible();
    const box = (await list.boundingBox())!;
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    for (let k = 0; k < 5; k++) {
      await page.mouse.wheel(0, 400);
      await page.waitForTimeout(60);
    }
    for (let k = 0; k < n; k++) await page.keyboard.press(key);
    await page.waitForTimeout(150);
    await expect(page.getByTestId("target-view")).toBeVisible();
    await expect(page.getByText("Something went wrong")).toHaveCount(0);
  }

  test("N2-1: wheel scroll and key moves in the moment grid, the Similar pile and the Weaker pile do not crash (1280x800)", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await openHome(page, 5000, "&target=answered");
    await page.getByTestId("project-open-1").click();
    await page.getByTestId("target-open").click();
    await tab(page, "review");
    await page.keyboard.press("g");
    await expect(page.getByTestId("target-review")).toHaveAttribute("data-view", "grid");
    await wheelAndKeys(page, "target-grid", "Shift+ArrowRight");
    const f = page.locator('[data-testid^="target-grid-cell-"][data-focused="true"]');
    await expect(f).toHaveCount(1);
    const b = (await f.boundingBox())!;
    const scr = (await page.getByTestId("target-grid").boundingBox())!;
    expect(b.y).toBeGreaterThanOrEqual(scr.y - 1);
    expect(b.y + b.height).toBeLessThanOrEqual(scr.y + scr.height + 1);
    await wheelAndKeys(page, "target-grid", "ArrowDown");

    await tab(page, "second");
    await page.getByTestId("target-pile-similar").click();
    await wheelAndKeys(page, "target-similar", "ArrowDown");
    await wheelAndKeys(page, "target-similar", "Shift+ArrowDown", 6);
    await page.getByTestId("target-pile-weaker").click();
    await wheelAndKeys(page, "target-grid", "ArrowDown");
    await page.getByTestId("target-pile-defects").click();
    await wheelAndKeys(page, "target-grid", "ArrowDown");
  });

  test("N2-2: the focused moment-grid cell scrolls into view; overflowing alternatives show +N; captions name their pick", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await openHome(page, 5000, "&target=answered");
    await page.getByTestId("project-open-1").click();
    await page.getByTestId("target-open").click();
    await tab(page, "review");
    await page.keyboard.press("g");
    await expect(page.getByTestId("target-review")).toHaveAttribute("data-view", "grid");
    await page.keyboard.press("ArrowDown"); // moment 2
    for (let k = 0; k < 6; k++) await page.keyboard.press("ArrowRight");
    await page.waitForTimeout(200);
    const f = page.locator('[data-testid^="target-grid-cell-"][data-focused="true"]');
    await expect(f).toHaveCount(1);
    const b = (await f.boundingBox())!;
    const vp = page.viewportSize()!;
    expect(b.x).toBeGreaterThanOrEqual(0);
    expect(b.x + b.width).toBeLessThanOrEqual(vp.width + 1);
    const caps = page.locator('[data-testid="target-alt-caption"]');
    if ((await caps.count()) > 0) await expect(caps.first()).toContainText(/≈ \S+/);
    // The Similar pile keeps its focused frame in view too.
    await tab(page, "second");
    await page.getByTestId("target-pile-similar").click();
    for (let k = 0; k < 8; k++) await page.keyboard.press("ArrowRight");
    await page.waitForTimeout(200);
    const sf = page.locator('[data-testid^="target-sim-frame-"][data-focused="true"]').first();
    const sb = (await sf.boundingBox())!;
    expect(sb.x + sb.width).toBeLessThanOrEqual(vp.width + 1);
  });

  test("N2-3: Shift+arrows do nothing in Weaker and Defects", async ({ page }) => {
    await openSecond(page, "&target=answered", 5000);
    for (const p of ["weaker", "defects"]) {
      await page.getByTestId(`target-pile-${p}`).click();
      await expect(page.locator('[data-testid^="target-cell-"]').first()).toBeVisible();
      const before = await attr(page, "target-second", "data-current");
      for (const k of ["Shift+ArrowDown", "Shift+ArrowRight", "Shift+ArrowUp", "Shift+ArrowLeft"]) await page.keyboard.press(k);
      expect(await attr(page, "target-second", "data-current")).toBe(before);
    }
  });
});
