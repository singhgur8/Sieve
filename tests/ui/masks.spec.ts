import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openApp, shot } from "./helpers";

const P = "7x-masks-";

async function openDevelop(page: Page, id = 1, query = "") {
  await openApp(page, 200, query);
  await page.getByTestId(`cell-${id}`).click();
  await page.keyboard.press("d");
  await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", String(id));
  await expect(page.getByTestId("view-main")).toBeVisible();
  await expect(page.getByTestId("histogram")).toHaveAttribute("data-empty", "false");
}

async function openMasks(page: Page, id = 1, query = "") {
  await openDevelop(page, id, query);
  await page.keyboard.press("Shift+W");
  await expect(page.getByTestId("masks-panel")).toBeVisible();
}

/** Sets a range input the way a completed drag would: input events, then release (blur). */
async function setSlider(page: Page, id: string, value: number) {
  const s = page.getByTestId(`slider-${id}`);
  await s.fill(String(value));
  await s.evaluate((el) => (el as HTMLElement).blur());
}

/** Screen rectangle of the displayed (cropped) frame, from the mask layer's own box. */
async function frameRect(page: Page) {
  const layer = page.getByTestId("mask-layer");
  const l = (await layer.boundingBox())!;
  const b = JSON.parse((await layer.getAttribute("data-box"))!) as { x: number; y: number; w: number; h: number };
  return { x: l.x + b.x, y: l.y + b.y, w: b.w, h: b.h };
}

/** Displayed-frame fraction -> page coordinates. */
async function at(page: Page, fx: number, fy: number) {
  const r = await frameRect(page);
  return { x: r.x + fx * r.w, y: r.y + fy * r.h };
}

async function dragFrac(page: Page, a: [number, number], b: [number, number], steps = 8) {
  const p = await at(page, a[0], a[1]);
  const q = await at(page, b[0], b[1]);
  await page.mouse.move(p.x, p.y);
  await page.mouse.down();
  await page.mouse.move(q.x, q.y, { steps });
  await page.mouse.up();
}

async function centerOf(page: Page, testid: string) {
  const b = (await page.getByTestId(testid).boundingBox())!;
  return { x: b.x + b.width / 2, y: b.y + b.height / 2 };
}

async function dragTo(page: Page, testid: string, dx: number, dy: number) {
  const c = await centerOf(page, testid);
  await page.mouse.move(c.x, c.y);
  await page.mouse.down();
  await page.mouse.move(c.x + dx, c.y + dy, { steps: 6 });
  await page.mouse.up();
}

type Adj = { masks: any[] };
async function lastAdj(page: Page): Promise<Adj> {
  const r = (await calls(page, "render_preview")).filter((c) => c.args.options.slot === "main");
  return (r.at(-1)?.args.adjustments as Adj | undefined) ?? { masks: [] };
}
const saved = async (page: Page) => (await calls(page, "save_adjustments")).filter((c) => c.args.label !== undefined);

async function createSubject(page: Page) {
  await page.getByTestId("mask-create-subject").click();
  await expect(page.getByTestId("mask-busy")).toBeVisible();
  await expect(page.locator('[data-testid^="mask-group-"][data-selected="true"]')).toHaveCount(1);
  await expect(page.getByTestId("mask-busy")).toHaveCount(0);
}

test.describe("masks", () => {
  test("Subject mask: group appears, local exposure reaches renderPreview and history", async ({ page }) => {
    await openMasks(page);
    await shot(page, `${P}01-panel-empty`);
    await clearCalls(page);
    await createSubject(page);
    const ai = await calls(page, "compute_ai_mask");
    expect(ai).toHaveLength(1);
    expect(ai[0].args.request).toMatchObject({ target: { kind: "subject" }, referencePoint: null, force: false });
    await expect(page.getByTestId("panel-tab-masks")).toContainText("(1)");
    const rows = page.locator('[data-testid^="mask-comp-"][data-kind="ai"]');
    await expect(rows).toHaveCount(1);
    await expect(rows.first()).toContainText("Subject 1");
    // Saved with the group and the digest from computeAiMask.
    await expect.poll(async () => (await saved(page)).length).toBeGreaterThan(0);
    const a1 = await lastAdj(page);
    expect(a1.masks).toHaveLength(1);
    expect(a1.masks[0].components[0].shape).toMatchObject({ kind: "ai", target: { kind: "subject" } });
    expect(a1.masks[0].components[0].shape.digest).toMatch(/^[0-9A-F]{32}$/);

    await page.getByTestId("mask-group-name-" + a1.masks[0].id).click();
    await setSlider(page, "mask-exposure", 1.5);
    await expect(page.getByTestId("slider-value-mask-exposure")).toHaveText("+1.50");
    await expect.poll(async () => (await lastAdj(page)).masks[0]?.adjustments.exposure).toBe(1.5);
    await expect.poll(async () => (await saved(page)).at(-1)?.args.label).toMatch(/^Mask: Subject 1 Exposure [+-]\d/);
    expect(((await saved(page)).at(-1)!.args.adjustments as Adj).masks[0].adjustments.exposure).toBe(1.5);

    // Double-click resets to the default (0).
    await page.getByTestId("slider-label-mask-exposure").dblclick();
    await expect.poll(async () => (await lastAdj(page)).masks[0]?.adjustments.exposure).toBe(0);
    await shot(page, `${P}02-subject-sliders`);
  });

  test("local sliders use Lightroom ranges", async ({ page }) => {
    await openMasks(page);
    await createSubject(page);
    const ranges: Record<string, [number, number]> = {
      temperature: [-100, 100],
      tint: [-100, 100],
      exposure: [-4, 4],
      contrast: [-100, 100],
      highlights: [-100, 100],
      shadows: [-100, 100],
      whites: [-100, 100],
      blacks: [-100, 100],
      texture: [-100, 100],
      clarity: [-100, 100],
      dehaze: [-100, 100],
      hue: [-180, 180],
      saturation: [-100, 100],
      sharpness: [-100, 100],
      noise: [-100, 100],
      moire: [-100, 100],
      defringe: [-100, 100],
    };
    for (const [k, [lo, hi]] of Object.entries(ranges)) {
      const s = page.getByTestId(`slider-mask-${k}`);
      await expect(s, k).toHaveAttribute("min", String(lo));
      await expect(s, k).toHaveAttribute("max", String(hi));
    }
    await expect(page.getByTestId("slider-mask-amount")).toHaveAttribute("max", "200");
    await setSlider(page, "mask-color-sat", 60);
    await setSlider(page, "mask-color-hue", 200);
    await expect.poll(async () => (await lastAdj(page)).masks[0]?.adjustments.color).toEqual({ hue: 200, saturation: 60 });
    await setSlider(page, "mask-amount", 150);
    await expect.poll(async () => (await lastAdj(page)).masks[0]?.amount).toBe(1.5);
  });

  test("brush records dabs in un-oriented sensor coordinates (EXIF orientation 8)", async ({ page }) => {
    await openMasks(page, 21);
    await clearCalls(page);
    await page.keyboard.press("k");
    await expect(page.getByTestId("mask-tool-badge")).toHaveAttribute("data-tool", "brush");
    await expect(page.getByTestId("mask-brush-settings")).toBeVisible();
    // Displayed (0.25, 0.5) -> sensor: unorient(8) = (1 - y, x) = (0.5, 0.25).
    const start = await at(page, 0.25, 0.5);
    await page.mouse.move(start.x, start.y);
    await page.mouse.down();
    const end = await at(page, 0.25, 0.7);
    await page.mouse.move(end.x, end.y, { steps: 15 });
    await page.mouse.up();
    await expect.poll(async () => (await lastAdj(page)).masks[0]?.components[0]?.shape.strokes?.[0]?.dabs.length ?? 0).toBeGreaterThan(3);
    const shape = (await lastAdj(page)).masks[0].components[0].shape;
    const dabs = shape.strokes[0].dabs;
    expect(dabs[0].x).toBeCloseTo(0.5, 2);
    expect(dabs[0].y).toBeCloseTo(0.25, 2);
    // Moving down in the displayed frame moves along sensor x (orientation 8): x decreases... y (display) 0.5->0.7 = sensor x 0.5->0.3.
    expect(dabs.at(-1).x).toBeCloseTo(0.3, 1);
    expect(dabs.at(-1).y).toBeCloseTo(0.25, 2);
    expect(shape.strokes[0]).toMatchObject({ erase: false, autoMask: false, feather: 0.5, flow: 0.5, density: 1 });
    expect(shape.strokes[0].radius).toBeCloseTo(0.02, 3);
    await expect.poll(async () => (await saved(page)).at(-1)?.args.label).toBe("Mask: Brush");
    await expect(page.locator('[data-testid^="mask-comp-"][data-kind="brush"]')).toHaveCount(1);
    await shot(page, `${P}03-brush-stroke`);

    // Second stroke goes into the same component (tool stays active).
    await dragFrac(page, [0.5, 0.3], [0.6, 0.3]);
    await expect.poll(async () => (await lastAdj(page)).masks[0].components[0].shape.strokes.length).toBe(2);
    expect((await lastAdj(page)).masks).toHaveLength(1);
  });

  test("brush settings: [ ] size, Shift+[ ] feather, A auto mask, Alt erases, erase toggle", async ({ page }) => {
    await openMasks(page);
    await page.keyboard.press("k");
    await expect(page.getByTestId("slider-value-mask-brush-size")).toHaveText("20");
    await page.keyboard.press("]");
    await expect(page.getByTestId("slider-value-mask-brush-size")).toHaveText("25");
    await page.keyboard.press("[");
    await page.keyboard.press("[");
    await expect(page.getByTestId("slider-value-mask-brush-size")).toHaveText("15");
    await page.keyboard.press("Shift+]");
    await expect(page.getByTestId("slider-value-mask-brush-feather")).toHaveText("60");
    await page.keyboard.press("Shift+[");
    await page.keyboard.press("Shift+[");
    await expect(page.getByTestId("slider-value-mask-brush-feather")).toHaveText("40");
    await page.keyboard.press("a");
    await expect(page.getByTestId("mask-brush-auto")).toBeChecked();
    await setSlider(page, "mask-brush-flow", 30);
    await setSlider(page, "mask-brush-density", 80);

    // Cursor circle follows the size (radius fraction x sensor width in displayed px).
    const p = await at(page, 0.5, 0.5);
    await page.mouse.move(p.x, p.y);
    const r1 = Number(await page.getByTestId("brush-cursor").getAttribute("data-radius"));
    expect(r1).toBeGreaterThan(4);
    await page.keyboard.press("]");
    await page.mouse.move(p.x + 1, p.y);
    expect(Number(await page.getByTestId("brush-cursor").getAttribute("data-radius"))).toBeGreaterThan(r1);
    await page.keyboard.press("[");

    await clearCalls(page);
    await dragFrac(page, [0.3, 0.3], [0.4, 0.3]);
    await expect.poll(async () => (await lastAdj(page)).masks[0]?.components[0]?.shape.strokes?.length).toBe(1);
    expect((await lastAdj(page)).masks[0].components[0].shape.strokes[0]).toMatchObject({ autoMask: true, flow: 0.3, density: 0.8, feather: 0.4, erase: false });

    // Alt = eraser for that stroke.
    await page.keyboard.down("Alt");
    await dragFrac(page, [0.3, 0.5], [0.4, 0.5]);
    await page.keyboard.up("Alt");
    await expect.poll(async () => (await lastAdj(page)).masks[0].components[0].shape.strokes.length).toBe(2);
    expect((await lastAdj(page)).masks[0].components[0].shape.strokes[1].erase).toBe(true);
    // Erase toggle in the panel.
    await page.getByTestId("mask-brush-erase").click();
    await dragFrac(page, [0.3, 0.6], [0.4, 0.6]);
    await expect.poll(async () => (await lastAdj(page)).masks[0].components[0].shape.strokes.length).toBe(3);
    expect((await lastAdj(page)).masks[0].components[0].shape.strokes[2].erase).toBe(true);
    await page.getByTestId("mask-tool-done").click();
    await expect(page.getByTestId("mask-capture")).toHaveCount(0);
  });

  test("linear gradient: drag creates it, handles move, rotate and resize the shape", async ({ page }) => {
    await openMasks(page);
    await clearCalls(page);
    await page.keyboard.press("m");
    await expect(page.getByTestId("mask-tool-badge")).toHaveAttribute("data-tool", "linear");
    await dragFrac(page, [0.5, 0.2], [0.5, 0.6]);
    await expect(page.getByTestId("linear-handles")).toBeVisible();
    await expect(page.getByTestId("mask-capture")).toHaveCount(0);
    await expect.poll(async () => (await lastAdj(page)).masks[0]?.components[0]?.shape.kind).toBe("linear");
    let sh = (await lastAdj(page)).masks[0].components[0].shape;
    // Full (100 %) at the drag start, zero at the end; orientation 1 so display == sensor.
    expect(sh.full.x).toBeCloseTo(0.5, 2);
    expect(sh.full.y).toBeCloseTo(0.2, 2);
    expect(sh.zero.y).toBeCloseTo(0.6, 2);
    await expect.poll(async () => (await saved(page)).at(-1)?.args.label).toBe("Mask: Linear Gradient");
    await shot(page, `${P}04-linear-handles`);

    // Move the pin: both points shift.
    await dragTo(page, "linear-handle-pin", 40, 30);
    await expect.poll(async () => (await lastAdj(page)).masks[0].components[0].shape.full.x).toBeGreaterThan(sh.full.x + 0.02);
    let sh2 = (await lastAdj(page)).masks[0].components[0].shape;
    expect(sh2.zero.x - sh2.full.x).toBeCloseTo(sh.zero.x - sh.full.x, 2);
    expect(sh2.zero.y).toBeGreaterThan(sh.zero.y + 0.02);
    // Zero line handle: lengthens the ramp along its axis.
    await dragTo(page, "linear-handle-zero", 0, 40);
    await expect.poll(async () => (await lastAdj(page)).masks[0].components[0].shape.zero.y).toBeGreaterThan(sh2.zero.y + 0.02);
    sh = (await lastAdj(page)).masks[0].components[0].shape;
    // Rotate handle: swings the axis.
    const angleBefore = Math.atan2(sh.zero.y - sh.full.y, sh.zero.x - sh.full.x);
    await dragTo(page, "linear-handle-rotate", 60, 0);
    await expect
      .poll(async () => {
        const s = (await lastAdj(page)).masks[0].components[0].shape;
        return Math.abs(Math.atan2(s.zero.y - s.full.y, s.zero.x - s.full.x) - angleBefore);
      })
      .toBeGreaterThan(0.1);
    // History carries the edits.
    expect((await saved(page)).length).toBeGreaterThanOrEqual(3);
  });

  test("radial gradient: drag creates an ellipse; handles resize, move, feather and rotate", async ({ page }) => {
    await openMasks(page);
    await clearCalls(page);
    await page.keyboard.press("Shift+M");
    await expect(page.getByTestId("mask-tool-badge")).toHaveAttribute("data-tool", "radial");
    await dragFrac(page, [0.5, 0.5], [0.7, 0.65]);
    await expect(page.getByTestId("radial-handles")).toBeVisible();
    await expect.poll(async () => (await lastAdj(page)).masks[0]?.components[0]?.shape.kind).toBe("radial");
    let sh = (await lastAdj(page)).masks[0].components[0].shape;
    expect((sh.left + sh.right) / 2).toBeCloseTo(0.5, 2);
    expect((sh.top + sh.bottom) / 2).toBeCloseTo(0.5, 2);
    expect((sh.right - sh.left) / 2).toBeCloseTo(0.2, 1);
    expect(sh).toMatchObject({ angle: 0, feather: 50, midpoint: 50, roundness: 0 });
    await shot(page, `${P}05-radial-handles`);

    await dragTo(page, "radial-handle-e", 40, 0);
    await expect.poll(async () => (await lastAdj(page)).masks[0].components[0].shape.right).toBeGreaterThan(sh.right + 0.02);
    let s2 = (await lastAdj(page)).masks[0].components[0].shape;
    // Resizing is symmetric about the centre.
    expect((s2.left + s2.right) / 2).toBeCloseTo((sh.left + sh.right) / 2, 3);
    expect(s2.left).toBeLessThan(sh.left - 0.02);
    await dragTo(page, "radial-handle-center", 30, 20);
    await expect.poll(async () => {
      const s = (await lastAdj(page)).masks[0].components[0].shape;
      return (s.left + s.right) / 2;
    }).toBeGreaterThan(0.5);
    sh = (await lastAdj(page)).masks[0].components[0].shape;
    await dragTo(page, "radial-handle-feather", 0, -30);
    await expect.poll(async () => (await lastAdj(page)).masks[0].components[0].shape.feather).not.toBe(50);
    await dragTo(page, "radial-handle-rotate", 0, 50);
    await expect.poll(async () => Math.abs((await lastAdj(page)).masks[0].components[0].shape.angle)).toBeGreaterThan(5);
    // Panel sliders: feather + roundness.
    await setSlider(page, "mask-radial-roundness", 40);
    await expect.poll(async () => (await lastAdj(page)).masks[0].components[0].shape.roundness).toBe(40);
    expect((await saved(page)).length).toBeGreaterThanOrEqual(5);
  });

  test("radial gradient on an orientation-8 photo stores sensor-frame bounds", async ({ page }) => {
    await openMasks(page, 21);
    await clearCalls(page);
    await page.keyboard.press("Shift+M");
    // Centre (0.5, 0.5); drag right by 0.2 of the displayed width and down by 0.1 of its height.
    await dragFrac(page, [0.5, 0.5], [0.7, 0.6]);
    await expect.poll(async () => (await lastAdj(page)).masks[0]?.components[0]?.shape.kind).toBe("radial");
    const sh = (await lastAdj(page)).masks[0].components[0].shape;
    expect((sh.left + sh.right) / 2).toBeCloseTo(0.5, 2);
    // Displayed width (0.2 of 4000 sensor-height px) is the sensor's vertical extent: hy = 0.2, hx = 0.1 * 6000/4000 (aspect).
    expect((sh.bottom - sh.top) / 2).toBeCloseTo(0.2, 1);
    expect((sh.right - sh.left) / 2).toBeCloseTo(0.15, 1);
    expect(Math.abs(sh.angle)).toBeLessThan(0.1);
  });

  test("Add / Subtract / Intersect components; invert; delete", async ({ page }) => {
    await openMasks(page);
    await createSubject(page);
    const gid = (await lastAdj(page)).masks[0].id as string;
    await expect(page.getByTestId(`mask-add-row-${gid}`)).toBeVisible();

    // Subtract a linear gradient from the group.
    await page.getByTestId(`mask-subtract-${gid}`).click();
    await page.getByTestId("mask-subtract-item-linear").click();
    await dragFrac(page, [0.5, 0.2], [0.5, 0.5]);
    await expect.poll(async () => (await lastAdj(page)).masks[0].components.length).toBe(2);
    let comps = (await lastAdj(page)).masks[0].components;
    expect(comps[1]).toMatchObject({ mode: "subtract", inverted: false });
    expect(comps[1].shape.kind).toBe("linear");
    expect((await lastAdj(page)).masks).toHaveLength(1);

    // Intersect with sky.
    await page.getByTestId(`mask-intersect-${gid}`).click();
    await page.getByTestId("mask-intersect-item-sky").click();
    await expect.poll(async () => (await lastAdj(page)).masks[0].components.length).toBe(3);
    comps = (await lastAdj(page)).masks[0].components;
    expect(comps[2]).toMatchObject({ mode: "intersect" });
    expect(comps[2].shape).toMatchObject({ kind: "ai", target: { kind: "sky" } });

    // Add with a brush.
    await page.getByTestId(`mask-add-${gid}`).click();
    await page.getByTestId("mask-add-item-brush").click();
    await dragFrac(page, [0.3, 0.3], [0.4, 0.4]);
    await expect.poll(async () => (await lastAdj(page)).masks[0].components.length).toBe(4);
    comps = (await lastAdj(page)).masks[0].components;
    expect(comps[3]).toMatchObject({ mode: "add" });
    expect(comps[3].shape.kind).toBe("brush");
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("mask-capture")).toHaveCount(0);
    await expect(page.getByTestId("develop-view")).toBeVisible(); // Esc finished the tool, did not leave Develop
    await shot(page, `${P}06-components`);

    // Invert a component.
    await page.getByTestId(`mask-comp-invert-${comps[1].id}`).click();
    await expect.poll(async () => (await lastAdj(page)).masks[0].components[1].inverted).toBe(true);
    await expect(page.getByTestId(`mask-comp-${comps[1].id}`)).toHaveAttribute("data-inverted", "true");
    // Toggle component visibility.
    await page.getByTestId(`mask-comp-eye-${comps[2].id}`).click();
    await expect.poll(async () => (await lastAdj(page)).masks[0].components[2].active).toBe(false);
    // Delete a component with its button; the group stays.
    await page.getByTestId(`mask-comp-delete-${comps[3].id}`).click();
    await expect.poll(async () => (await lastAdj(page)).masks[0].components.length).toBe(3);
    // Delete key removes the selected component.
    await page.getByTestId(`mask-comp-name-${comps[2].id}`).click();
    await page.keyboard.press("Delete");
    await expect.poll(async () => (await lastAdj(page)).masks[0].components.length).toBe(2);
    // Remaining component deletion removes the empty group... via the group menu.
    await page.getByTestId(`mask-group-menu-${gid}`).click();
    await page.getByTestId("mask-action-delete").click();
    await expect.poll(async () => (await lastAdj(page)).masks.length).toBe(0);
    await expect(page.getByTestId("mask-empty")).toBeVisible();
  });

  test("group menu: rename, duplicate, invert; eye toggle; amount", async ({ page }) => {
    await openMasks(page);
    await createSubject(page);
    const gid = (await lastAdj(page)).masks[0].id as string;
    await page.getByTestId(`mask-group-menu-${gid}`).click();
    await page.getByTestId("mask-action-rename").click();
    const input = page.getByTestId(`mask-group-rename-input-${gid}`);
    await input.fill("Face light");
    await input.press("Enter");
    await expect(page.getByTestId(`mask-group-name-${gid}`)).toHaveText("Face light");
    await expect.poll(async () => (await lastAdj(page)).masks[0].name).toBe("Face light");
    // Double-click renames too.
    await page.getByTestId(`mask-group-name-${gid}`).dblclick();
    await expect(page.getByTestId(`mask-group-rename-input-${gid}`)).toBeVisible();
    await page.getByTestId(`mask-group-rename-input-${gid}`).press("Escape");
    await expect(page.getByTestId(`mask-group-name-${gid}`)).toHaveText("Face light");

    await page.getByTestId(`mask-group-menu-${gid}`).click();
    await page.getByTestId("mask-action-invert").click();
    await expect.poll(async () => (await lastAdj(page)).masks[0].components[0].inverted).toBe(true);

    await page.getByTestId(`mask-group-menu-${gid}`).click();
    await page.getByTestId("mask-action-duplicate").click();
    await expect.poll(async () => (await lastAdj(page)).masks.length).toBe(2);
    const m = (await lastAdj(page)).masks;
    expect(m[1].id).not.toBe(m[0].id);
    expect(m[1].components[0].id).not.toBe(m[0].components[0].id);
    expect(m[1].name).toBe("Face light copy");

    await page.getByTestId(`mask-group-eye-${gid}`).click();
    await expect.poll(async () => (await lastAdj(page)).masks[0].active).toBe(false);
    await expect(page.getByTestId(`mask-group-${gid}`)).toHaveAttribute("data-active", "false");
  });

  test("overlay: O toggles, Shift+O cycles styles, render_mask_overlay latest-wins on the selected mask", async ({ page }) => {
    await openMasks(page);
    await createSubject(page);
    const gid = (await lastAdj(page)).masks[0].id as string;
    await page.getByTestId(`mask-group-name-${gid}`).click();
    expect((await calls(page, "render_mask_overlay")).length).toBe(0);
    await expect(page.getByTestId("mask-overlay")).toHaveCount(0);
    await page.keyboard.press("o");
    await expect(page.getByTestId("mask-overlay")).toBeVisible();
    const oc = await calls(page, "render_mask_overlay");
    expect(oc.length).toBeGreaterThan(0);
    expect(oc.at(-1)!.args.target).toEqual({ groupId: gid, componentId: null });
    expect((oc.at(-1)!.args.adjustments as Adj).masks[0].id).toBe(gid);
    expect(oc.at(-1)!.args.options).toMatchObject({ region: null });
    await expect(page.getByTestId("mask-overlay-toggle")).toBeChecked();
    await expect(page.getByTestId("mask-overlay")).toHaveAttribute("data-overlay-style", "red");
    await shot(page, `${P}07-overlay-red`);
    await page.keyboard.press("Shift+O");
    await expect(page.getByTestId("mask-overlay")).toHaveAttribute("data-overlay-style", "green");
    for (let i = 0; i < 3; i++) await page.keyboard.press("Shift+O");
    await expect(page.getByTestId("mask-overlay")).toHaveAttribute("data-overlay-style", "gray");
    await shot(page, `${P}08-overlay-gray`);
    await page.keyboard.press("Shift+O");
    await expect(page.getByTestId("mask-overlay")).toHaveAttribute("data-overlay-style", "bw");
    // Editing a local slider re-renders the overlay with the live masks.
    const before = (await calls(page, "render_mask_overlay")).length;
    await setSlider(page, "mask-exposure", 1);
    await expect.poll(async () => (await calls(page, "render_mask_overlay")).length).toBeGreaterThan(before);
    expect(((await calls(page, "render_mask_overlay")).at(-1)!.args.adjustments as Adj).masks[0].adjustments.exposure).toBe(1);
    // Hover a component: its own overlay.
    const cid = (await lastAdj(page)).masks[0].components[0].id as string;
    await page.getByTestId(`mask-comp-${cid}`).hover();
    await expect.poll(async () => (await calls(page, "render_mask_overlay")).at(-1)!.args.target).toEqual({ groupId: gid, componentId: cid });
    await page.keyboard.press("o");
    await expect(page.getByTestId("mask-overlay")).toHaveCount(0);
  });

  test("H hides the pins; pins select their mask", async ({ page }) => {
    await openMasks(page);
    await createSubject(page);
    const gid = (await lastAdj(page)).masks[0].id as string;
    await page.keyboard.press("m");
    await dragFrac(page, [0.3, 0.2], [0.3, 0.5]);
    await expect.poll(async () => (await lastAdj(page)).masks.length).toBe(2);
    const g2 = (await lastAdj(page)).masks[1].id as string;
    await expect(page.getByTestId(`mask-pin-${gid}`)).toBeVisible();
    // The selected gradient's centre handle is its pin.
    await expect(page.getByTestId(`mask-pin-${g2}`)).toHaveCount(0);
    await expect(page.getByTestId("linear-handles")).toBeVisible();
    await page.getByTestId(`mask-pin-${gid}`).click();
    await expect(page.getByTestId(`mask-group-${gid}`)).toHaveAttribute("data-selected", "true");
    await expect(page.getByTestId("linear-handles")).toHaveCount(0);
    await shot(page, `${P}09-pins`);
    await page.keyboard.press("h");
    await expect(page.getByTestId(`mask-pin-${gid}`)).toHaveCount(0);
    await expect(page.getByTestId("mask-pins-toggle")).not.toBeChecked();
    await page.keyboard.press("h");
    await expect(page.getByTestId(`mask-pin-${gid}`)).toBeVisible();
  });

  test("capabilities disable AI kinds without models, with the reason as tooltip", async ({ page }) => {
    await openMasks(page, 1, "&noai=sky,people");
    for (const k of ["sky", "people"]) {
      const b = page.getByTestId(`mask-create-${k}`);
      await expect(b).toBeDisabled();
      await expect(b).toHaveAttribute("title", new RegExp(`is not available: mock: ${k} model not installed`));
    }
    // Mock default: no object / landscape model either.
    await expect(page.getByTestId("mask-create-object")).toBeDisabled();
    await expect(page.getByTestId("mask-create-subject")).toBeEnabled();
    await expect(page.getByTestId("mask-create-brush")).toBeEnabled();
    await clearCalls(page);
    await page.getByTestId("mask-create-sky").click({ force: true }).catch(() => {});
    expect(await calls(page, "compute_ai_mask")).toHaveLength(0);
    await shot(page, `${P}10-capabilities`);
    // The Add menu honours it too.
    await createSubject(page);
    const gid = (await lastAdj(page)).masks[0].id as string;
    await page.getByTestId(`mask-add-${gid}`).click();
    await expect(page.getByTestId("mask-add-item-sky")).toBeDisabled();
    await expect(page.getByTestId("mask-add-item-brush")).toBeEnabled();
  });

  test("People picker: person and parts checkboxes drive the AI request", async ({ page }) => {
    await openMasks(page);
    await clearCalls(page);
    await page.getByTestId("mask-create-people").click();
    const dlg = page.getByTestId("people-picker");
    await expect(dlg).toBeVisible();
    await expect(page.getByTestId("people-person-0")).toBeVisible();
    await expect(page.getByTestId("people-person-1")).toBeVisible();
    for (const part of ["face_skin", "body_skin", "eyebrows", "eye_sclera", "iris_pupil", "lips", "teeth", "hair", "clothes"]) {
      await expect(page.getByTestId(`people-part-${part}`)).toBeVisible();
    }
    await expect(page.getByTestId("people-part-all")).toBeChecked();
    await page.getByTestId("people-person-1").click();
    await page.getByTestId("people-part-hair").check();
    await expect(page.getByTestId("people-part-all")).not.toBeChecked();
    await page.getByTestId("people-part-lips").check();
    await shot(page, `${P}11-people-picker`);
    await page.getByTestId("people-create").click();
    await expect(dlg).toHaveCount(0);
    await expect(page.locator('[data-testid^="mask-comp-"][data-kind="ai"]')).toHaveCount(1);
    const req = (await calls(page, "compute_ai_mask")).at(-1)!.args.request as any;
    expect(req.target).toEqual({ kind: "people", parts: ["lips", "hair"] });
    expect(req.referencePoint).toEqual({ x: 0.65, y: 0.35 });
    const comp = (await lastAdj(page)).masks[0].components[0];
    expect(comp.shape).toMatchObject({ kind: "ai", referencePoint: { x: 0.65, y: 0.35 } });
    expect(comp.name).toBe("Person 1");

    // Cancel with Esc; entire person.
    await page.getByTestId("mask-create-people").click();
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("people-picker")).toHaveCount(0);
    await page.getByTestId("mask-create-people").click();
    await page.getByTestId("people-everyone").click();
    await page.getByTestId("people-create").click();
    await expect.poll(async () => (await lastAdj(page)).masks.length).toBe(2);
    const req2 = (await calls(page, "compute_ai_mask")).at(-1)!.args.request as any;
    expect(req2).toMatchObject({ target: { kind: "people", parts: [] }, referencePoint: null });
  });

  test("Background and Sky create AI groups; color range and luminance range tools", async ({ page }) => {
    await openMasks(page, 21);
    await clearCalls(page);
    await page.getByTestId("mask-create-background").click();
    await expect.poll(async () => (await lastAdj(page)).masks.length).toBe(1);
    expect((await lastAdj(page)).masks[0].components[0].shape).toMatchObject({ kind: "ai", target: { kind: "background" } });
    await page.getByTestId("mask-create-sky").click();
    await expect.poll(async () => (await lastAdj(page)).masks.length).toBe(2);

    // Colour range on the orientation-8 photo: sample stored un-oriented.
    await page.keyboard.press("Shift+J");
    const p = await at(page, 0.25, 0.5);
    await page.mouse.click(p.x, p.y);
    await expect.poll(async () => (await lastAdj(page)).masks.length).toBe(3);
    let sh = (await lastAdj(page)).masks[2].components[0].shape;
    expect(sh.kind).toBe("color");
    expect(sh.samples).toHaveLength(1);
    expect(sh.samples[0].point.x).toBeCloseTo(0.5, 2);
    expect(sh.samples[0].point.y).toBeCloseTo(0.25, 2);
    expect(sh.samples[0].area).toBeNull();
    // A dragged sample records an area.
    await dragFrac(page, [0.4, 0.6], [0.5, 0.7]);
    await expect.poll(async () => (await lastAdj(page)).masks[2].components[0].shape.samples.length).toBe(2);
    sh = (await lastAdj(page)).masks[2].components[0].shape;
    expect(sh.samples[1].area).not.toBeNull();
    expect(sh.samples[1].area.width).toBeGreaterThan(0.05);
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("mask-color-settings")).toBeVisible();
    await expect(page.getByTestId("mask-color-samples")).toHaveAttribute("data-count", "2");
    await setSlider(page, "mask-color-amount", 80);
    await expect.poll(async () => (await lastAdj(page)).masks[2].components[0].shape.amount).toBe(80);
    await page.getByTestId("mask-color-remove-sample").click();
    await expect.poll(async () => (await lastAdj(page)).masks[2].components[0].shape.samples.length).toBe(1);
    await shot(page, `${P}12-color-range`);

    // Luminance range.
    await page.keyboard.press("Shift+Q");
    const q = await at(page, 0.5, 0.5);
    await page.mouse.click(q.x, q.y);
    await expect.poll(async () => (await lastAdj(page)).masks.length).toBe(4);
    const lum = (await lastAdj(page)).masks[3].components[0].shape;
    expect(lum.kind).toBe("luminance");
    expect(lum.featherLow).toBeLessThanOrEqual(lum.low);
    expect(lum.low).toBeLessThanOrEqual(lum.high);
    expect(lum.high).toBeLessThanOrEqual(lum.featherHigh);
    await expect(page.getByTestId("mask-luminance-settings")).toBeVisible();
    await setSlider(page, "mask-lum-low", 20);
    await expect.poll(async () => (await lastAdj(page)).masks[3].components[0].shape.low).toBe(0.2);
    // Ordering is enforced: high cannot go below low.
    await setSlider(page, "mask-lum-high", 5);
    await expect.poll(async () => (await lastAdj(page)).masks[3].components[0].shape.high).toBe(0.2);
    await shot(page, `${P}13-luminance-range`);
  });

  test("undo / redo cover mask edits", async ({ page }) => {
    await openMasks(page);
    await createSubject(page);
    await setSlider(page, "mask-exposure", 2);
    await expect.poll(async () => (await saved(page)).at(-1)?.args.label).toMatch(/^Mask: Subject 1 Exposure [+-]\d/);
    await page.keyboard.press("Meta+z");
    await expect.poll(async () => (await lastAdj(page)).masks[0]?.adjustments.exposure).toBe(0);
    await expect(page.getByTestId("slider-value-mask-exposure")).toHaveText("0.00");
    await page.keyboard.press("Meta+Shift+z");
    await expect(page.getByTestId("slider-value-mask-exposure")).toHaveText("+2.00");
    await page.keyboard.press("Meta+z");
    await page.keyboard.press("Meta+z");
    await expect(page.getByTestId("mask-empty")).toBeVisible();
    await expect.poll(async () => (await lastAdj(page)).masks.length).toBe(0);
    const labels = await page.getByTestId("left-panel").innerText();
    expect(labels).toContain("Mask: Subject");
  });

  test("copy / paste leaves masks out by default; pasted AI masks need an update", async ({ page }) => {
    await openMasks(page, 1);
    await createSubject(page);
    await setSlider(page, "mask-exposure", 1);
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("develop-view")).toBeVisible(); // UX2 P0-1: Esc never leaves Develop
    await page.getByTestId("copy-settings").click();
    const dlg = page.getByTestId("fields-dialog");
    await expect(dlg).toBeVisible();
    const maskBox = dlg.getByTestId("field-masks");
    await expect(maskBox).not.toBeChecked();
    await expect(dlg.getByTestId("field-crop")).not.toBeChecked();
    await expect(dlg.getByTestId("field-exposure")).toBeChecked();
    await shot(page, `${P}14-copy-dialog`);
    await dlg.getByRole("button", { name: "Copy", exact: true }).click();
    await page.getByTestId("film-2").click();
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "2");
    await clearCalls(page);
    await page.getByTestId("paste-settings").click();
    await expect.poll(async () => (await calls(page, "paste_settings")).length).toBe(1);
    expect(((await calls(page, "paste_settings"))[0].args.fields as string[]).includes("masks")).toBe(false);
    await page.getByTestId("panel-tab-masks").click();
    await expect(page.getByTestId("mask-empty")).toBeVisible();

    // Now explicitly include Masking: the group arrives without digests and needs an update.
    await page.getByTestId("film-1").click();
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "1");
    await page.getByTestId("copy-settings").click();
    await page.getByTestId("field-masks").check();
    await page.getByTestId("fields-dialog").getByRole("button", { name: "Copy", exact: true }).click();
    await page.getByTestId("film-2").click();
    await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", "2");
    await page.getByTestId("paste-settings").click();
    await expect.poll(async () => (await calls(page, "paste_settings")).length).toBe(2);
    expect(((await calls(page, "paste_settings"))[1].args.fields as string[]).includes("masks")).toBe(true);
    await expect(page.getByTestId("mask-update-all")).toBeVisible();
    await expect(page.locator('[data-testid^="mask-comp-update-"]')).toHaveCount(1);
    await shot(page, `${P}15-needs-update`);
    // The warnings chip offers the same action.
    await page.getByTestId("warnings-chip").click();
    await expect(page.getByTestId("warning-ai_mask_needs_update")).toBeVisible();
    await clearCalls(page);
    await page.getByTestId("warning-action-ai_mask_needs_update").click();
    await expect(page.getByTestId("mask-busy")).toBeVisible();
    await expect.poll(async () => (await calls(page, "compute_ai_mask")).length).toBe(1);
    await expect(page.getByTestId("mask-update-all")).toHaveCount(0);
    expect((await lastAdj(page)).masks[0].components[0].shape.digest).toMatch(/^[0-9A-F]{32}$/);
    await expect(page.getByTestId("warnings-chip")).toHaveCount(0);
  });

  test("keys: K stays the burst keeper in Compare; mask keys need the Masks panel; cheat sheet lists them", async ({ page }) => {
    await openDevelop(page, 1);
    // UX2 P1-1: the tool key opens the Masks panel and starts the tool in one go.
    await expect(page.getByTestId("mask-layer")).toHaveCount(0);
    await page.keyboard.press("k");
    await expect(page.getByTestId("masks-panel")).toBeVisible();
    await expect(page.getByTestId("mask-capture")).toBeVisible();
    await page.keyboard.press("Enter"); // Enter finishes the tool
    await expect(page.getByTestId("mask-capture")).toHaveCount(0);
    await page.keyboard.press("Shift+W");
    await expect(page.getByTestId("adjust-panel")).toBeVisible();
    // R (crop) and the masks tools are exclusive.
    await page.keyboard.press("Shift+W");
    await page.keyboard.press("k");
    await page.keyboard.press("r");
    await expect(page.getByTestId("crop-bar")).toBeVisible();
    await expect(page.getByTestId("mask-layer")).toHaveCount(0);
    await page.keyboard.press("Escape");
    await page.keyboard.press("?");
    for (const id of ["maskPanel", "maskBrush", "maskLinear", "maskRadial", "maskColor", "maskLuminance", "maskOverlay", "maskOverlayStyle", "maskPins", "maskSize", "maskFeather"]) {
      await expect(page.getByTestId(`cheat-${id}`)).toBeVisible();
    }
    await shot(page, `${P}16-cheatsheet`);
    await page.keyboard.press("Escape");
    // Compare keeps K = keeper.
    await page.keyboard.press("g");
    await page.getByTestId("cell-3").click(); // burst member; the keeper is id 2
    await page.keyboard.press("c");
    await expect(page.getByTestId("compare")).toBeVisible();
    await clearCalls(page);
    await page.keyboard.press("k");
    await expect.poll(async () => (await calls(page, "set_burst_keeper")).length).toBe(1);
  });

  test("creating a mask keeps one render in flight and shows the panel screenshot", async ({ page }) => {
    await openMasks(page, 1);
    await page.keyboard.press("m");
    await dragFrac(page, [0.5, 0.15], [0.5, 0.55]);
    await page.keyboard.press("o");
    await page.keyboard.press("Shift+O");
    await setSlider(page, "mask-exposure", -1.2);
    await setSlider(page, "mask-temperature", 30);
    await expect.poll(async () => (await lastAdj(page)).masks[0]?.adjustments.temperature).toBe(30);
    const stats = await page.evaluate(() => (window as any).__mockRenderStats);
    expect(stats.maxInflight).toBeLessThanOrEqual(2);
    await shot(page, `${P}17-linear-with-overlay`);
  });
});
