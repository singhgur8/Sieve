import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openApp, openSection } from "./helpers";

// Transform panel (Upright, manual sliders, Guided tool) and the crop tool's Auto straighten, on the mock backend.

async function openTransform(page: Page, query = "") {
  await openApp(page, 200, query);
  await page.getByTestId("cell-1").click();
  await page.keyboard.press("d");
  await expect(page.getByTestId("view-main")).toBeVisible();
  await expect(page.getByTestId("histogram")).toHaveAttribute("data-empty", "false");
  await openSection(page, "transform");
}

const saves = async (page: Page) => (await calls(page, "save_adjustments")).filter((c) => c.args.label !== undefined);
const upright = async (page: Page, mode: string) => {
  await page.getByTestId(`upright-${mode}`).click();
};

test.describe("Transform / Upright", () => {
  for (const [mode, label] of [
    ["level", "Upright: Level"],
    ["vertical", "Upright: Vertical"],
    ["auto", "Upright: Auto"],
    ["full", "Upright: Full"],
  ] as const) {
    test(`${mode} sets the transform and records one history entry`, async ({ page }) => {
      await openTransform(page);
      await clearCalls(page);
      await upright(page, mode);
      await expect(page.getByTestId("transform-panel")).toHaveAttribute("data-upright", mode);
      await expect(page.getByTestId("upright-message")).toHaveCount(0);
      const s = await saves(page);
      expect(s).toHaveLength(1);
      expect(s[0].args.label).toBe(label);
      const t = (s[0].args.adjustments as any).transform;
      expect(t.upright).toBe(mode);
      expect(t.solution.mode).toBe(mode);
      expect((await calls(page, "auto_upright")).at(-1)!.args.mode).toBe(mode);
      // The changed dot of the section appears.
      await expect(page.getByTestId("section-dot-transform")).toBeVisible();
    });
  }

  test("Off clears the mode; Reset Transform resets everything", async ({ page }) => {
    await openTransform(page);
    await upright(page, "level");
    await upright(page, "off");
    await expect(page.getByTestId("transform-panel")).toHaveAttribute("data-upright", "off");
    await upright(page, "full");
    await page.getByTestId("reset-transform").click();
    await expect(page.getByTestId("transform-panel")).toHaveAttribute("data-upright", "off");
    await expect(page.getByTestId("section-dot-transform")).toHaveCount(0);
    expect((await saves(page)).at(-1)!.args.label).toBe("Reset Transform");
  });

  test("no usable lines: inline message, nothing saved", async ({ page }) => {
    await openTransform(page, "&upright=none");
    await clearCalls(page);
    await upright(page, "vertical");
    await expect(page.getByTestId("upright-message")).toContainText("No straight lines found");
    expect(await saves(page)).toHaveLength(0);
    await expect(page.getByTestId("transform-panel")).toHaveAttribute("data-upright", "off");
  });

  test("manual slider: typed value, then reset", async ({ page }) => {
    await openTransform(page);
    await clearCalls(page);
    await page.getByTestId("slider-value-tf-vertical").click();
    await page.getByTestId("slider-edit-tf-vertical").fill("25");
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("slider-value-tf-vertical")).toHaveText("+25");
    const s = await saves(page);
    expect(s).toHaveLength(1);
    expect((s[0].args.adjustments as any).transform.vertical).toBe(25);
    expect(s[0].args.label).toMatch(/^Transform: Vertical/);
    await page.getByTestId("slider-label-tf-vertical").dblclick();
    await expect(page.getByTestId("slider-value-tf-vertical")).toHaveText("0");
    await page.getByTestId("tf-constrain").check();
    expect(((await saves(page)).at(-1)!.args.adjustments as any).transform.constrainCrop).toBe(true);
  });

  test("Guided: Shift+T, two guides trigger a solve, guides can be deleted, Esc exits", async ({ page }) => {
    await openTransform(page);
    await page.keyboard.press("Shift+T");
    await expect(page.getByTestId("guide-overlay")).toBeVisible();
    const f = (await page.getByTestId("guide-frame").boundingBox())!;
    const draw = async (x0: number, y0: number, x1: number, y1: number) => {
      await page.mouse.move(f.x + f.width * x0, f.y + f.height * y0);
      await page.mouse.down();
      await page.mouse.move(f.x + f.width * x1, f.y + f.height * y1, { steps: 5 });
      await page.mouse.up();
    };
    await clearCalls(page);
    await draw(0.2, 0.2, 0.22, 0.8);
    await expect(page.getByTestId("guide-line-0")).toBeVisible();
    await expect(page.getByTestId("upright-message")).toContainText("at least two");
    expect(await calls(page, "auto_upright")).toHaveLength(0);
    await draw(0.7, 0.2, 0.72, 0.8);
    await expect(page.getByTestId("guide-overlay")).toHaveAttribute("data-count", "2");
    await expect(page.getByTestId("transform-panel")).toHaveAttribute("data-upright", "guided");
    const solves = await calls(page, "auto_upright");
    expect(solves).toHaveLength(1);
    expect(solves[0].args.mode).toBe("guided");
    expect(((solves[0].args.adjustments as any).transform.guides as unknown[]).length).toBe(2);
    const last = (await saves(page)).at(-1)!;
    expect(last.args.label).toBe("Upright: Guided");
    expect(((last.args.adjustments as any).transform.solution as any).mode).toBe("guided");
    // Editing: delete a guide -> fewer than two, the solution is dropped.
    await page.getByTestId("guide-delete-1").click();
    await expect(page.getByTestId("guide-overlay")).toHaveAttribute("data-count", "1");
    expect(((await saves(page)).at(-1)!.args.adjustments as any).transform.solution).toBeNull();
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("guide-overlay")).toHaveCount(0);
  });
});

test.describe("Crop: Auto straighten", () => {
  async function openCrop(page: Page, query = "") {
    await openApp(page, 200, query);
    await page.getByTestId("cell-1").click();
    await page.keyboard.press("d");
    await expect(page.getByTestId("view-main")).toBeVisible();
    await expect(page.getByTestId("histogram")).toHaveAttribute("data-empty", "false");
    await page.keyboard.press("r");
    await expect(page.getByTestId("crop-frame")).toBeVisible();
  }
  const area = async (page: Page) => {
    const r = JSON.parse((await page.getByTestId("crop-rect").getAttribute("data-rect"))!);
    return (r.r - r.l) * (r.b - r.t);
  };

  test("Auto sets the crop angle from a Level solve and fits the crop", async ({ page }) => {
    await openCrop(page);
    const before = await area(page);
    await clearCalls(page);
    await page.getByTestId("crop-auto-straighten").click();
    await expect(page.getByTestId("slider-value-crop-angle")).toHaveText("+1.2°");
    const c = await calls(page, "auto_upright");
    expect(c).toHaveLength(1);
    expect(c[0].args.mode).toBe("level");
    expect(await area(page)).toBeLessThan(before);
    await expect(page.getByTestId("crop-auto-message")).toHaveCount(0);
  });

  test("Shift+double-click on the Angle slider triggers it", async ({ page }) => {
    await openCrop(page);
    await page.getByTestId("slider-label-crop-angle").dblclick({ modifiers: ["Shift"] });
    await expect(page.getByTestId("slider-value-crop-angle")).toHaveText("+1.2°");
    // Plain double-click still resets.
    await page.getByTestId("slider-label-crop-angle").dblclick();
    await expect(page.getByTestId("slider-value-crop-angle")).toHaveText("0.0°");
  });

  test("no lines: inline message, the angle stays", async ({ page }) => {
    await openCrop(page, "&upright=none");
    await page.getByTestId("crop-auto-straighten").click();
    await expect(page.getByTestId("crop-auto-message")).toContainText("No straight lines");
    await expect(page.getByTestId("slider-value-crop-angle")).toHaveText("0.0°");
  });

  test("Constrain to image off shows the white paper in the rotated corners", async ({ page }) => {
    await openCrop(page);
    await page.getByTestId("slider-crop-angle").fill("8");
    await expect(page.getByTestId("crop-paper")).toHaveCount(0);
    await page.getByTestId("crop-constrain").uncheck();
    await expect(page.getByTestId("crop-paper")).toBeVisible();
  });
});
