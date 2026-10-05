import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openApp, openHome } from "./helpers";

// Phase 8d: per-photo Photo info panel, Edit Capture Time (shift / sync cameras / undo), reject strictness.

/** Ids of the cells on screen in reading order. */
async function order(page: Page, n = 14): Promise<number[]> {
  const cells = await page.$$eval('[data-testid^="cell-"]', (els) =>
    els.map((e) => ({ id: Number((e.getAttribute("data-testid") ?? "").slice(5)), r: e.getBoundingClientRect() })).filter((c) => Number.isFinite(c.id)),
  );
  cells.sort((a, b) => Math.round(a.r.top) - Math.round(b.r.top) || a.r.left - b.r.left);
  return cells.slice(0, n).map((c) => c.id);
}

const captured = (page: Page) => page.getByTestId("info-captured");

test.describe("photo info panel", () => {
  test("shows the selected photo's time and follows the selection", async ({ page }) => {
    await openApp(page, 60);
    await page.getByTestId("cell-2").click();
    await page.getByTestId("info-toggle").click();
    await expect(page.getByTestId("photo-info-body")).toHaveAttribute("data-image-id", "2");
    const t2 = await captured(page).innerText();
    await expect(page.getByTestId("info-file")).toContainText("2");
    await expect(page.getByTestId("info-camera")).not.toBeEmpty();
    await expect(page.getByTestId("info-sidecar")).toContainText(".xmp");
    await page.getByTestId("cell-5").click();
    await expect(page.getByTestId("photo-info-body")).toHaveAttribute("data-image-id", "5");
    expect(await captured(page).innerText()).not.toBe(t2);
    // Cmd+I toggles it; the gallery-level filter keeps its own, clearly named button.
    await page.keyboard.press("Meta+i");
    await expect(page.getByTestId("photo-info")).toHaveCount(0);
    await expect(page.getByTestId("meta-toggle")).toContainText("Metadata filter");
  });

  test("loupe info overlay shows the capture time", async ({ page }) => {
    await openApp(page, 30);
    await page.getByTestId("cell-4").dblclick();
    await expect(page.getByTestId("info-capture-time")).toBeVisible();
  });
});

test.describe("edit capture time", () => {
  test("shift by -1 h changes the time, Undo restores it", async ({ page }) => {
    await openApp(page, 60);
    await page.getByTestId("cell-7").click();
    await page.keyboard.press("Meta+i");
    const before = (await captured(page).innerText()).split("\n")[0];
    await page.keyboard.press("Meta+Shift+t");
    await expect(page.getByTestId("capture-time-dialog")).toBeVisible();
    await expect(page.getByTestId("capture-h")).toHaveValue("1");
    await expect(page.getByTestId("capture-summary")).toContainText("1 photo will change");
    await page.getByTestId("capture-apply").click();
    await expect(page.getByTestId("capture-time-dialog")).toHaveCount(0);
    await expect(page.getByTestId("info-captured-source")).toHaveAttribute("data-source", "user");
    const after = await captured(page).innerText();
    expect(after).not.toBe(before);
    await expect(page.getByTestId("info-original")).toBeVisible();
    const edit = (await calls(page, "edit_capture_time"))[0];
    expect(edit.args.mode).toEqual({ kind: "shift", offsetMs: -3_600_000 });
    await page.getByTestId("capture-undo").click();
    await expect(captured(page)).toContainText(before);
    expect((await calls(page, "restore_capture_times")).length).toBe(1);
  });

  test("Revert to original from the panel", async ({ page }) => {
    await openApp(page, 30);
    await page.getByTestId("cell-3").click();
    await page.getByTestId("more-menu").click();
    await page.getByTestId("edit-capture-time-menu").click();
    await page.getByTestId("capture-m").fill("5");
    await page.getByTestId("capture-apply").click();
    await page.keyboard.press("Meta+i");
    await expect(page.getByTestId("info-revert-time")).toBeVisible();
    await page.getByTestId("info-revert-time").click();
    await expect(page.getByTestId("info-captured-source")).toHaveAttribute("data-source", "exif");
    expect((await calls(page, "edit_capture_time")).at(-1)?.args.mode).toEqual({ kind: "revert" });
  });

  test("sync two cameras reorders the grid", async ({ page }) => {
    await openApp(page, 60, "&twocams=1");
    const before = await order(page);
    await page.getByTestId("cell-2").click();
    await page.keyboard.press("Meta+Shift+t");
    await page.getByTestId("capture-tab-sync").click();
    const cams = await page.getByTestId("capture-ref-cam").locator("option").allTextContents();
    expect(cams.length).toBe(2);
    const canon = cams.find((c) => /r5/i.test(c))!;
    const sony = cams.find((c) => c !== canon)!;
    await page.getByTestId("capture-ref-cam").selectOption(sony);
    await page.getByTestId("capture-tgt-cam").selectOption(canon);
    await expect(page.getByTestId("capture-apply")).toBeDisabled(); // nothing is pre-picked from one selected photo
    await page.getByTestId("capture-ref-frame-opt-2").click();
    await page.getByTestId("capture-tgt-frame-opt-3").click();
    await expect(page.getByTestId("capture-preview")).not.toHaveAttribute("data-count", "0");
    await expect(page.getByTestId("capture-preview-3")).toBeVisible();
    await page.getByTestId("capture-apply").click();
    await expect(page.getByTestId("capture-time-dialog")).toHaveCount(0);
    await expect.poll(() => order(page)).not.toEqual(before);
    const edit = (await calls(page, "edit_capture_time"))[0];
    expect(edit.args.mode).toEqual({ kind: "sync_cameras", referenceId: 2, targetId: 3 });
    expect((edit.args.ids as number[]).every((i) => i % 3 === 0)).toBe(true);
    await page.getByTestId("capture-undo").click();
    await expect.poll(() => order(page)).toEqual(before);
  });
});

test.describe("reject strictness", () => {
  test("changing it calls the command and the suggestion count updates", async ({ page }) => {
    await openHome(page, 201, "&keepers=not_rejected");
    await page.getByTestId("project-open-1").click();
    await expect(page.getByTestId("reject-strictness")).toHaveValue("balanced");
    await expect(page.getByTestId("strictness-explain")).toContainText("closed eyes on the main subject");
    const n0 = Number(await page.getByTestId("strictness-count").getAttribute("data-rejects"));
    await clearCalls(page);
    await page.getByTestId("reject-strictness").selectOption("aggressive");
    await expect.poll(async () => (await calls(page, "set_project_reject_strictness")).length).toBe(1);
    expect((await calls(page, "set_project_reject_strictness"))[0].args).toMatchObject({ strictness: "aggressive" });
    await expect(page.getByTestId("strictness-explain")).toContainText("Expect some keepers among the suggestions");
    await expect.poll(async () => Number(await page.getByTestId("strictness-count").getAttribute("data-rejects"))).toBeGreaterThan(n0);
  });
});
