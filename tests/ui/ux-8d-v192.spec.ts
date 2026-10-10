import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openApp, openHome, shot } from "./helpers";

// UX review 8d on contract v19.2: P1-3 / P1-4 (bodies, scopes), P1-5 (suggested rejects, apply per kind).
const V18 = "&keepers=not_rejected";

async function openProject(page: Page, extra = "") {
  await openHome(page, 201, V18 + extra);
  await page.getByTestId("project-open-1").click();
  await expect(page.getByTestId("cull-summary")).toBeVisible();
}

test.describe("P1-4 two bodies of the same model", () => {
  test("Camera facet lists both bodies (serial suffix) and filters by body", async ({ page }) => {
    await openApp(page, 60, "&twobodies=1&meta=1");
    await page.getByTestId("meta-toggle").click();
    const col = page.getByTestId("meta-col-cameras");
    await expect(col.getByRole("button").filter({ hasText: "ILCE-7M4" })).toHaveCount(2);
    await expect(col).toContainText("(…8214)");
    await expect(col).toContainText("(…9876)");
    await page.getByTestId('meta-opt-cameras-{"make":"sony","model":"ILCE-7M4","serial":"05119876"}').click();
    await expect.poll(async () => ((await calls(page, "list_image_ids")).at(-1)!.args.query as any).metadata?.bodies?.[0]?.serial).toBe("05119876");
  });

  test("Sync tab: the pair opens it, two bodies are told apart, scope defaults to the body and goes to the backend with ids []", async ({ page }) => {
    await openApp(page, 60, "&twobodies=1");
    await page.getByTestId("cell-2").click();
    await page.getByTestId("cell-3").click({ modifiers: ["Meta"] });
    await page.keyboard.press("Meta+Shift+t");
    await expect(page.getByTestId("capture-pane-sync")).toBeVisible();
    const opts = await page.getByTestId("capture-ref-cam").locator("option").allTextContents();
    expect(opts.sort()).toEqual(["Sony ILCE-7M4 (…8214)", "Sony ILCE-7M4 (…9876)"]);
    await expect(page.getByTestId("capture-scope-body")).toBeChecked();
    await expect(page.getByTestId("capture-scope")).toContainText("of this project moves");
    await shot(page, "v192-sync-bodies");
    await clearCalls(page);
    await page.getByTestId("capture-apply").click();
    await expect.poll(async () => (await calls(page, "edit_capture_time")).length).toBe(1);
    const e = (await calls(page, "edit_capture_time"))[0].args as any;
    expect(e.mode).toEqual({ kind: "sync_cameras", referenceId: 2, targetId: 3, scope: "body" });
    expect(e.ids).toEqual([]);
  });

  test("'The selected photos' fallback moves exactly the selection (scope selected)", async ({ page }) => {
    await openApp(page, 60, "&twobodies=1");
    await page.getByTestId("cell-2").click();
    await page.getByTestId("cell-3").click({ modifiers: ["Meta"] });
    await page.keyboard.press("Meta+Shift+t");
    await page.getByTestId("capture-scope-selected").check();
    await clearCalls(page);
    await page.getByTestId("capture-apply").click();
    await expect.poll(async () => (await calls(page, "edit_capture_time")).length).toBe(1);
    const e = (await calls(page, "edit_capture_time"))[0].args as any;
    expect(e.mode.scope).toBe("selected");
    expect([...e.ids].sort()).toEqual([2, 3]);
  });

  test("a filter hides frames of the camera: the summary counts the whole body and says how many are hidden", async ({ page }) => {
    await openApp(page, 60, "&twobodies=1");
    await page.getByTestId("cell-6").click();
    await page.keyboard.press("x"); // a frame of the second body is rejected ...
    await page.getByTestId("pick-unflagged").click(); // ... and the Unflagged view hides it
    await expect(page.getByTestId("cell-6")).toHaveCount(0);
    await page.getByTestId("cell-2").click();
    await page.getByTestId("cell-3").click({ modifiers: ["Meta"] });
    await page.keyboard.press("Meta+Shift+t");
    await expect(page.getByTestId("capture-pane-sync")).toBeVisible();
    await expect(page.getByTestId("capture-scope")).toContainText(/including [1-9]\d* hidden by the current filters/);
    await expect(page.getByTestId("capture-summary")).toContainText(/including [1-9]\d* hidden by the current filters/);
    expect(Number(await page.getByTestId("capture-preview").getAttribute("data-count"))).toBe(20); // the whole body (ids 3, 6, ..., 60)
  });
});

test.describe("P1-5 suggested rejects", () => {
  test("the chip shows exactly the suggested rejects; Apply opens with only Rejects and sends kinds", async ({ page }) => {
    await openProject(page);
    const n = Number(await page.getByTestId("strictness-count").getAttribute("data-rejects"));
    expect(n).toBeGreaterThan(0);
    await expect(page.getByTestId("strictness-count")).toHaveText(`Review ${n} suggested ${n === 1 ? "reject" : "rejects"}`);
    await page.getByTestId("strictness-count").click();
    await expect(page.getByTestId("filter-suggested")).toBeVisible();
    await expect.poll(async () => ((await calls(page, "list_image_ids")).at(-1)!.args.query as any).suggested).toBe("reject");
    await expect(page.getByTestId("readout-shown")).toHaveText(String(n));
    await page.getByTestId("cull-sum-suggest").click();
    await expect(page.getByTestId("apply-kind-rejects")).toBeChecked();
    await expect(page.getByTestId("apply-kind-picks")).not.toBeChecked();
    await expect(page.getByTestId("apply-kind-stars")).not.toBeChecked();
    await expect(page.getByTestId("apply-count-apply")).toHaveText(String(n));
    await expect(page.getByTestId("apply-confirm")).toHaveText(`Apply to ${n}`);
    await clearCalls(page);
    await page.getByTestId("apply-confirm").click();
    await expect.poll(async () => (await calls(page, "apply_suggestions")).length).toBe(1);
    const a = (await calls(page, "apply_suggestions"))[0].args as any;
    expect(a.kinds).toEqual({ picks: false, rejects: true, stars: false });
    expect(a.ids).toHaveLength(n);
    await shot(page, "v192-review-rejects");
  });

  test("the three checkboxes follow the counts and the choice is remembered per project", async ({ page }) => {
    await openProject(page);
    await page.getByTestId("cull-sum-suggest").click();
    const all = Number(await page.getByTestId("apply-count-apply").textContent());
    await page.getByTestId("apply-kind-picks").uncheck();
    await page.getByTestId("apply-kind-stars").uncheck();
    const rejOnly = Number(await page.getByTestId("apply-count-apply").textContent());
    expect(rejOnly).toBeLessThan(all);
    expect(rejOnly).toBe(Number(await page.getByTestId("apply-kind-rejects-count").textContent()));
    await page.getByTestId("apply-cancel").click();
    await page.getByTestId("cull-sum-suggest").click();
    await expect(page.getByTestId("apply-kind-picks")).not.toBeChecked();
    await expect(page.getByTestId("apply-count-apply")).toHaveText(String(rejOnly));
  });

  test("Auto 0 explains itself and Review them turns the suggested-rejects filter on", async ({ page }) => {
    await openProject(page);
    await page.getByTestId("cull-sum-auto").click();
    await page.getByTestId("auto-zero-apply").click();
    await expect(page.getByTestId("filter-suggested")).toBeVisible();
    await page.getByTestId("filter-suggested").click();
    await expect(page.getByTestId("filter-suggested")).toHaveCount(0);
  });
});
