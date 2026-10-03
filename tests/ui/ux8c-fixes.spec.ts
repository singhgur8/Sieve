import { expect, test, type Page } from "@playwright/test";
import { openApp, openHome, shot } from "./helpers";
import { TAG_MEANING } from "../../src/lib/cull";

// UX review 8c, P1-2..P1-9 (+ cheap P2s). Acceptance checks follow docs/ux-review-8c.md.
const V18 = "&keepers=not_rejected";

const activity = (page: Page, e: Record<string, unknown>) =>
  page.evaluate((ev) => (window as unknown as { __mockActivity: (e: unknown) => void }).__mockActivity(ev), { message: null, done: 0, total: null, state: "running", kind: "other", label: "Working", ...e });

async function openProject(page: Page) {
  await openHome(page, 201, V18);
  await page.getByTestId("project-open-1").click();
  await expect(page.getByTestId("cull-summary")).toBeVisible();
}

test.describe("P1-2 one bottom-right stack", () => {
  test("export card and other activities never overlap; export progress shows once", async ({ page }) => {
    await page.addInitScript(() => (window.__mockExportManual = true));
    await openApp(page, 200);
    await page.getByTestId("cell-5").click();
    for (let i = 0; i < 3; i++) await page.keyboard.press("Shift+ArrowRight");
    await page.keyboard.press("Meta+Shift+E");
    await expect(page.getByTestId("export-dialog")).toBeVisible();
    await page.getByTestId("export-skip-rejected").uncheck();
    await page.getByTestId("export-choose-folder").click();
    await expect(page.getByTestId("export-dest-path")).toBeVisible();
    await page.getByTestId("export-preset-name").fill("");
    await page.getByTestId("export-go").click();
    await expect(page.getByTestId("export-job-1")).toBeVisible();

    await activity(page, { id: 9101, kind: "export", label: "Exporting 4 photos", done: 1, total: 4 });
    await activity(page, { id: 9102, kind: "xmp_save", label: "Saving metadata", done: 3, total: 40 });
    await activity(page, { id: 9103, kind: "paste_sync", label: "Pasting settings", done: 1, total: 9 });
    await expect(page.locator('[data-testid="activity"][data-kind="xmp_save"]')).toBeVisible();
    await expect(page.locator('[data-testid="activity"][data-kind="paste_sync"]')).toBeVisible();
    await expect(page.locator('[data-testid="activity"][data-kind="export"]')).toHaveCount(0);

    const widget = page.getByTestId("activity-widget");
    await expect(widget.getByTestId("export-job-1")).toBeVisible(); // the card lives inside the one stack
    const boxes = await widget.locator("> *").evaluateAll((els) => els.map((e) => e.getBoundingClientRect()).map((r) => ({ l: r.left, t: r.top, r: r.right, b: r.bottom })));
    expect(boxes.length).toBeGreaterThanOrEqual(3);
    for (let i = 0; i < boxes.length; i++)
      for (let j = i + 1; j < boxes.length; j++) {
        const a = boxes[i];
        const b = boxes[j];
        expect(a.l < b.r && b.l < a.r && a.t < b.b && b.t < a.b, `elements ${i} and ${j} intersect`).toBe(false);
      }
    // The export count appears once (the card), not again as an activity row.
    await expect(widget.getByText("Exporting 4 photos")).toHaveCount(0);
    await shot(page, "8c-p1-2-stack");
  });

  test("analysis activity still shows in the corner when no top bar carries it", async ({ page }) => {
    await openApp(page, 200);
    await activity(page, { id: 9201, kind: "analysis", label: "Analyzing", done: 2, total: 9 });
    await expect(page.locator('[data-testid="activity"][data-kind="analysis"]')).toHaveCount(1);
  });
});

test.describe("P1-3 reject strip is not dimmed", () => {
  test("strip and its ancestors are fully opaque; only the picture is dimmed", async ({ page }) => {
    await openProject(page);
    await page.getByTestId("cull-sum-rejected").click();
    const strip = page.locator('[data-testid^="reject-reason-"]').first();
    await expect(strip).toBeVisible();
    const ops = await strip.evaluate((el) => {
      const out: number[] = [];
      for (let n: Element | null = el; n && n !== document.body; n = n.parentElement) out.push(Number(getComputedStyle(n).opacity));
      return out;
    });
    expect(ops.every((o) => o === 1)).toBe(true);
    const img = strip.locator("xpath=ancestor::*[starts-with(@data-testid,'cell-')][1]").locator("img").first();
    expect(Number(await img.evaluate((e) => getComputedStyle(e).opacity))).toBeLessThan(0.5);
    await shot(page, "8c-p1-3-rejected");
  });
});

test.describe("P1-4 suggestion copy", () => {
  test("cell strip says Sieve suggests reject; loupe line too", async ({ page }) => {
    await openProject(page);
    const sug = page.locator('[data-testid^="suggested-reason-"]').first();
    await expect(sug).toBeVisible();
    await expect(sug).toHaveText(/^Sieve suggests reject · \S/);
    await expect(sug).toHaveClass(/border-dashed/);
    await expect(sug.locator("b")).toHaveText("Sieve suggests reject");
    const id = (await sug.getAttribute("data-testid"))!.replace("suggested-reason-", "");
    await page.getByTestId(`cell-${id}`).click();
    await page.keyboard.press("Space");
    await expect(page.getByTestId("loupe")).toBeVisible();
    await expect(page.getByTestId("suggested-line")).toHaveText(/^Sieve suggests reject(: \S.*)?$/);
  });
});

test.describe("P1-5 best of burst wording", () => {
  test("badge titles use best-of-burst, never keeper", async ({ page }) => {
    await openApp(page, 400);
    const badges = page.locator('[data-testid^="burst-badge-"]');
    await expect(badges.first()).toBeVisible();
    const titles = await badges.evaluateAll((els) => els.map((e) => e.getAttribute("title") ?? ""));
    expect(titles.length).toBeGreaterThan(1);
    for (const t of titles) {
      expect(t).toMatch(/^Burst of \d+ — (best frame \(Sieve's choice\)|not the best frame)/);
      expect(t.toLowerCase()).not.toContain("keeper");
    }
  });
});

test.describe("P1-7 metadata row fits", () => {
  for (const w of [1280, 1440]) {
    test(`every column (incl. Status) is inside the ${w}px viewport`, async ({ page }) => {
      await page.setViewportSize({ width: w, height: 800 });
      await openApp(page, 400, "&meta=1");
      await page.getByTestId("meta-toggle").click();
      await expect(page.getByTestId("meta-col-status")).toBeVisible();
      const rights = await page.locator('[data-testid="meta-row"] > [data-testid^="meta-col-"]').evaluateAll((els) => els.map((e) => e.getBoundingClientRect().right));
      expect(rights.length).toBe(9);
      for (const r of rights) expect(r).toBeLessThanOrEqual(w);
      for (const t of ["Edited", "Unedited", "Has sidecar", "No sidecar"]) await expect(page.getByTestId("meta-col-status")).toContainText(t);
      expect(await page.getByTestId("meta-row").evaluate((e) => e.getBoundingClientRect().height)).toBe(112);
      await page.getByTestId("meta-opt-edited-no").click();
      await expect(page.getByTestId("meta-chip-edited")).toBeVisible();
      await page.getByTestId("meta-clear-status").click();
      await expect(page.getByTestId("meta-chip-edited")).toHaveCount(0);
    });
  }
});

test.describe("P1-8 / P1-9 Help copy", () => {
  test("keeper and apply-suggestions copy", async ({ page }) => {
    await openApp(page, 200);
    await page.keyboard.press("F1");
    await page.getByTestId("help-nav-keepers").click();
    const k = page.getByTestId("help-entry-keepers");
    await expect(k).toContainText("Change keeper rule");
    await expect(k).toContainText("Cull summary bar");
    const items = await k.locator("li").allInnerTexts();
    expect(items[0]).toMatch(/^Everything not rejected/); // menu order: default first
    await page.getByTestId("help-nav-apply-suggestions").click();
    const a = page.getByTestId("help-entry-apply-suggestions");
    await expect(a).toContainText("More > Apply suggestions");
    await expect(a).toContainText("written to the XMP sidecars");
    await expect(a).not.toContainText("does not touch labels, edits or files");
  });

  test("legend tag texts are the TAG_MEANING strings; strips and badges are listed", async ({ page }) => {
    await openApp(page, 200);
    await page.keyboard.press("F1");
    await page.getByTestId("help-nav-icons").click();
    for (const [tag, text] of Object.entries(TAG_MEANING)) {
      const row = page.getByTestId(`help-legend-${tag.replace("_", " ").replace(/^./, (c) => c.toUpperCase())}`);
      await expect(row.getByTestId("help-legend-text")).toHaveText(text);
    }
    await expect(page.getByTestId("help-legend-Rejected strip")).toBeVisible();
    await expect(page.getByTestId("help-legend-Suggestion strip")).toBeVisible();
    await expect(page.getByTestId("help-legend-Unreadable")).toBeVisible();
    await expect(page.getByTestId("help-legend-Saved check")).toBeVisible();
    await expect(page.getByTestId("help-legend-Best of burst")).toBeVisible();
    expect(await page.getByTestId("help-legend-Blink").locator("span.font-medium").evaluate((e) => getComputedStyle(e).textTransform)).toBe("none");
  });
});

test.describe("cheap P2s", () => {
  test("Cmd+? opens Help; Help button and auto-advance titles", async ({ page }) => {
    await openApp(page, 200);
    await expect(page.getByTestId("help-button")).toHaveAttribute("title", /F1 \/ Cmd\+\?/);
    await expect(page.getByTestId("auto-advance").locator("xpath=ancestor::label")).toHaveAttribute("title", /Shift\+Z \/ Shift\+X always advance/);
    await page.keyboard.press("Meta+?");
    await expect(page.getByTestId("help-panel")).toBeVisible();
  });
});
