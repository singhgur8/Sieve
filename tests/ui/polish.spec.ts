// Phase 7c/8 polish: crop straighten live preview (sign vs the backend's crop_geometry, rotate-by-drag, Cmd-drag line,
// constrain to image), per-mask tone curve, mask reorder, region-sized overlays, gradient geometry, luminance eyedropper.
import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openApp, shot } from "./helpers";
import { fitInsideRotated, fromStored, insideRotated, previewRotation, toStored, angleFromRotation } from "../../src/lib/crop";
import { dispToSensor, radialFromScreen, radialToScreen, screenToDisp, sensorToDisp, sensorSize, type Frame } from "../../src/lib/maskGeom";

const P = "9x-polish-";

async function openDevelop(page: Page, id = 1) {
  await openApp(page, 200);
  await page.getByTestId(`cell-${id}`).click();
  await page.keyboard.press("d");
  await expect(page.getByTestId("develop-view")).toHaveAttribute("data-image-id", String(id));
  await expect(page.getByTestId("view-main")).toBeVisible();
  await expect(page.getByTestId("histogram")).toHaveAttribute("data-empty", "false");
}

async function openMasks(page: Page, id = 1) {
  await openDevelop(page, id);
  await page.keyboard.press("Shift+W");
  await expect(page.getByTestId("masks-panel")).toBeVisible();
}

const saved = async (page: Page) => (await calls(page, "save_adjustments")).filter((c) => c.args.label !== undefined);
type Adj = { masks: any[]; crop: any };
async function lastAdj(page: Page): Promise<Adj> {
  const r = (await calls(page, "render_preview")).filter((c) => c.args.options.slot === "main");
  return (r.at(-1)?.args.adjustments as Adj | undefined) ?? { masks: [], crop: {} };
}
const rectOf = async (page: Page) => JSON.parse((await page.getByTestId("crop-rect").getAttribute("data-rect"))!) as { l: number; t: number; r: number; b: number };

async function createSubject(page: Page) {
  await page.getByTestId("mask-create-subject").click();
  await expect(page.getByTestId("mask-busy")).toBeVisible();
  await expect(page.locator('[data-testid^="mask-group-"][data-selected="true"]')).toHaveCount(1);
  await expect(page.getByTestId("mask-busy")).toHaveCount(0);
}

async function frameRect(page: Page) {
  const layer = page.getByTestId("mask-layer");
  const l = (await layer.boundingBox())!;
  const b = JSON.parse((await layer.getAttribute("data-box"))!) as { x: number; y: number; w: number; h: number };
  return { x: l.x + b.x, y: l.y + b.y, w: b.w, h: b.h };
}

// ---------------------------------------------------------------------------------------------------------------------
// Crop straighten: geometry (pure)
// ---------------------------------------------------------------------------------------------------------------------

test.describe("crop straighten geometry", () => {
  const RAD = Math.PI / 180;
  // crop_geometry (src-tauri/src/develop/parity.rs): output frame (u, v in 0..1) -> source px, y down:
  //   p = C + R(angle) * ((u - .5) fw, (v - .5) fh),  R(a) = [cos -sin; sin cos].
  const backendSample = (angle: number, C: [number, number], fw: number, fh: number, u: number, v: number): [number, number] => {
    const [sn, cs] = [Math.sin(angle * RAD), Math.cos(angle * RAD)];
    const [lx, ly] = [(u - 0.5) * fw, (v - 0.5) * fh];
    return [C[0] + cs * lx - sn * ly, C[1] + sn * lx + cs * ly];
  };
  // CSS rotate(deg) about `c`: clockwise on screen (y down) = the same matrix.
  const cssRotate = (deg: number, c: [number, number], p: [number, number]): [number, number] => {
    const [sn, cs] = [Math.sin(deg * RAD), Math.cos(deg * RAD)];
    return [c[0] + cs * (p[0] - c[0]) - sn * (p[1] - c[1]), c[1] + sn * (p[0] - c[0]) + cs * (p[1] - c[1])];
  };

  test("sign: the preview rotation of the source equals the backend's sampling (rotation = -angle)", () => {
    // The straightened output shows source point P at the output location that the backend samples it from. Rotating the
    // source image by previewRotation(angle) about the frame centre must put P where the (axis-aligned) output frame has it.
    for (const angle of [5, -5.74, 23, -40]) {
      const C: [number, number] = [3000, 2000];
      const [fw, fh] = [4000, 2500];
      for (const [u, v] of [[0, 0], [1, 0], [1, 1], [0.3, 0.8]] as const) {
        const src = backendSample(angle, C, fw, fh, u, v);
        const shown = cssRotate(previewRotation(angle, 1), C, src);
        expect(shown[0]).toBeCloseTo(C[0] + (u - 0.5) * fw, 6);
        expect(shown[1]).toBeCloseTo(C[1] + (v - 0.5) * fh, 6);
      }
    }
    // Concretely: a positive angle samples the frame clockwise in the source, so the preview turns counter-clockwise.
    expect(previewRotation(5, 1)).toBe(-5);
    expect(previewRotation(-5.74, 1)).toBe(5.74);
    expect(previewRotation(5, 6)).toBe(-5); // rotations commute with 90 degree orientations
    expect(previewRotation(5, 2)).toBe(5); // a mirrored orientation flips the sense
    for (const o of [1, 2, 6, 7]) expect(angleFromRotation(previewRotation(12.5, o), o)).toBeCloseTo(12.5, 9);
  });

  test("toStored / fromStored reproduce crop_geometry: corners, size and the Camera Raw reference", () => {
    // parity.rs crop_geometry_sizes_and_mapping: left .1 top .2 right .6 bottom .7, 5 deg on 6000 x 4000 is the frame
    // whose size is the corner vector rotated back; rect -> stored -> rect is the identity.
    const aspect = 6000 / 4000;
    for (const o of [1, 2, 3, 4, 5, 6, 7, 8]) {
      const a = o >= 5 ? 1 / aspect : aspect; // displayed aspect
      const rect = { l: 0.22, t: 0.27, r: 0.71, b: 0.83 };
      for (const angle of [0, 5, -5.74, 17.3]) {
        const st = toStored(rect, o, angle, a);
        const back = fromStored({ ...st, angle }, o, a);
        for (const k of ["l", "t", "r", "b"] as const) expect(back[k]).toBeCloseTo(rect[k], 3);
      }
    }
    // Camera Raw reference (parity.rs): 7008 x 4672, -5.74 deg, corners (0.036395, 0.131022) / (0.963605, 0.868978) renders
    // 6120 x 4080: a frame 0.8733 of the image in both directions, centred; it lies inside the straightened image.
    const lr = { top: 0.131022, left: 0.036395, bottom: 0.868978, right: 0.963605, angle: -5.74 };
    const r = fromStored(lr, 1, 7008 / 4672);
    expect(r.r - r.l).toBeCloseTo(6120 / 7008, 3);
    expect(r.b - r.t).toBeCloseTo(4080 / 4672, 3);
    expect((r.l + r.r) / 2).toBeCloseTo(0.5, 2);
    expect(insideRotated(r, 7008 / 4672, previewRotation(-5.74, 1), 2e-3)).toBe(true);
    // and it is the largest 3:2 frame that fits (constrain-to-image would not shrink it further)
    const bigger = { l: r.l - 0.01, r: r.r + 0.01, t: r.t - 0.0067, b: r.b + 0.0067 };
    expect(insideRotated(bigger, 7008 / 4672, previewRotation(-5.74, 1))).toBe(false);
  });

  test("constrain to image: shrinks about the centre, keeps the ratio, stays inside", () => {
    const A = 1.5;
    for (const rot of [3, -3, 10, -20, 40]) {
      const full = { l: 0, t: 0, r: 1, b: 1 };
      const fit = fitInsideRotated(full, A, rot);
      expect(insideRotated(fit, A, rot)).toBe(true);
      expect((fit.r - fit.l) / (fit.b - fit.t)).toBeCloseTo(1, 6);
      expect((fit.l + fit.r) / 2).toBeCloseTo(0.5, 6);
      // an inside rectangle is left alone
      const small = { l: 0.45, t: 0.45, r: 0.55, b: 0.55 };
      expect(fitInsideRotated(small, A, rot)).toBe(small);
      // off-centre rectangles move in as well
      const off = fitInsideRotated({ l: 0.6, t: 0.5, r: 1, b: 1 }, A, rot);
      expect(insideRotated(off, A, rot)).toBe(true);
    }
    // The bigger the angle the smaller the frame.
    const w = (rot: number) => fitInsideRotated({ l: 0, t: 0, r: 1, b: 1 }, A, rot);
    expect(w(10).r - w(10).l).toBeLessThan(w(3).r - w(3).l);
  });
});

// ---------------------------------------------------------------------------------------------------------------------
// Crop straighten: UI
// ---------------------------------------------------------------------------------------------------------------------

test.describe("crop straighten preview", () => {
  test("the angle slider rotates the preview live (no backend render), shows the grid, shrinks the rect; Enter commits", async ({ page }) => {
    await openDevelop(page);
    await page.keyboard.press("r");
    await expect(page.getByTestId("crop-overlay")).toBeVisible();
    await expect(page.getByTestId("view-main")).toHaveAttribute("data-rotation", "0");
    await expect.poll(async () => (await calls(page, "render_preview")).some((c) => c.args.adjustments.crop.enabled === false)).toBe(true);
    await clearCalls(page);

    await page.getByTestId("slider-crop-angle").fill("5");
    // Positive angle = the preview turns counter-clockwise.
    await expect(page.getByTestId("view-main")).toHaveAttribute("data-rotation", "-5.00");
    await expect(page.getByTestId("view-main")).toHaveCSS("transform", /matrix\(0\.99\d*, -0\.08\d*, 0\.08\d*, 0\.99\d*, 0, 0\)/);
    await expect(page.getByTestId("crop-grid-line")).toHaveCount(9); // the fine grid while dragging
    const r1 = await rectOf(page);
    expect(r1.r - r1.l).toBeLessThan(0.97);
    expect((r1.l + r1.r) / 2).toBeCloseTo(0.5, 2);
    expect(r1.r - r1.l).toBeCloseTo(r1.b - r1.t, 2); // Original aspect stays locked
    await shot(page, `${P}01-straighten-slider`);
    expect(await calls(page, "render_preview")).toHaveLength(0); // no backend render while straightening

    await page.getByTestId("slider-crop-angle").evaluate((el) => (el as HTMLElement).blur());
    await expect(page.getByTestId("crop-grid-line")).toHaveCount(0);
    // The rectangle is inside the rotated image at every angle.
    await page.getByTestId("slider-crop-angle").fill("-12");
    await expect(page.getByTestId("view-main")).toHaveAttribute("data-rotation", "12.00");
    const r2 = await rectOf(page);
    expect(insideRotated(r2, 1.5, 12)).toBe(true);
    expect(r2.r - r2.l).toBeLessThan(r1.r - r1.l);
    await page.getByTestId("slider-crop-angle").evaluate((el) => (el as HTMLElement).blur());
    await page.getByTestId("slider-crop-angle").fill("5");
    await page.getByTestId("slider-crop-angle").evaluate((el) => (el as HTMLElement).blur());
    const before = await rectOf(page);

    await page.keyboard.press("Enter");
    await expect(page.getByTestId("crop-overlay")).toHaveCount(0);
    await expect(page.getByTestId("view-main")).toHaveAttribute("data-rotation", "0");
    await expect.poll(async () => (await saved(page)).length).toBe(1);
    const c = (await saved(page))[0].args.adjustments.crop;
    expect(c).toMatchObject({ enabled: true, angle: 5 });
    // The backend renders the rotated crop.
    await expect.poll(async () => (await lastAdj(page)).crop.angle).toBe(5);

    // Re-opening shows the same rectangle on the straightened image (stored corners <-> displayed rect round trip).
    await page.keyboard.press("r");
    await expect(page.getByTestId("view-main")).toHaveAttribute("data-rotation", "-5.00");
    const again = await rectOf(page);
    for (const k of ["l", "t", "r", "b"] as const) expect(again[k]).toBeCloseTo(before[k], 2);
    await shot(page, `${P}02-straighten-reopen`);
  });

  test("dragging outside the rectangle rotates about the crop centre (rotate cursor)", async ({ page }) => {
    await openDevelop(page);
    await page.keyboard.press("r");
    await expect(page.getByTestId("crop-overlay")).toBeVisible();
    await expect(page.getByTestId("crop-rotate-layer")).toHaveCSS("cursor", /url\(/);
    await page.keyboard.press("a"); // free: the rectangle only shrinks as needed
    const fr = (await page.getByTestId("crop-frame").boundingBox())!;
    // Make room outside the rectangle: pull the south-east handle in.
    const se = (await page.getByTestId("crop-handle-se").boundingBox())!;
    await page.mouse.move(se.x + se.width / 2, se.y + se.height / 2);
    await page.mouse.down();
    await page.mouse.move(fr.x + fr.width * 0.85, fr.y + fr.height * 0.85, { steps: 4 });
    await page.mouse.up();
    const rect = await rectOf(page);
    const cx = fr.x + ((rect.l + rect.r) / 2) * fr.width;
    const cy = fr.y + ((rect.t + rect.b) / 2) * fr.height;
    // Start below the rectangle and sweep 8 degrees clockwise around its centre.
    const R = ((rect.b - rect.t) / 2) * fr.height + 30;
    const at = (deg: number) => [cx + R * Math.sin((deg * Math.PI) / 180), cy + R * Math.cos((deg * Math.PI) / 180)] as const;
    const [sx, sy] = at(0);
    await page.mouse.move(sx, sy);
    await page.mouse.down();
    for (let d = 0; d >= -8; d -= 2) await page.mouse.move(...at(d)); // bottom -> towards the left = clockwise on screen
    await expect(page.getByTestId("crop-grid-line")).toHaveCount(9);
    await expect(page.getByTestId("view-main")).toHaveAttribute("data-rotation", "8.00");
    await shot(page, `${P}03-rotate-by-drag`);
    await page.mouse.up();
    await expect(page.getByTestId("crop-grid-line")).toHaveCount(0);
    // Clockwise photo rotation = a negative crs:CropAngle.
    await expect(page.getByTestId("slider-value-crop-angle")).toHaveText("-8.0°");
    expect(insideRotated(await rectOf(page), 1.5, 8)).toBe(true);
    await page.keyboard.press("Enter");
    await expect.poll(async () => (await saved(page)).length).toBe(1);
    expect((await saved(page))[0].args.adjustments.crop.angle).toBeCloseTo(-8, 1);
  });

  test("Cmd-drag across the photo draws a straighten line that levels it", async ({ page }) => {
    await openDevelop(page);
    await page.keyboard.press("r");
    await expect(page.getByTestId("crop-overlay")).toBeVisible();
    const fr = (await page.getByTestId("crop-frame").boundingBox())!;
    // A line sloping down to the right by 4 degrees over the middle of the photo (on top of the crop rectangle).
    const x0 = fr.x + fr.width * 0.2;
    const x1 = fr.x + fr.width * 0.8;
    const y0 = fr.y + fr.height * 0.5;
    const y1 = y0 + (x1 - x0) * Math.tan((4 * Math.PI) / 180);
    await page.keyboard.down("Meta");
    await expect(page.getByTestId("crop-rotate-layer")).toHaveAttribute("data-cmd", "true");
    await page.mouse.move(x0, y0);
    await page.mouse.down();
    await page.mouse.move(x1, y1, { steps: 6 });
    await expect(page.getByTestId("crop-straighten-line")).toBeVisible();
    await shot(page, `${P}04-straighten-line`);
    await page.mouse.up();
    await page.keyboard.up("Meta");
    await expect(page.getByTestId("crop-straighten-line")).toHaveCount(0);
    // Levelling a line that slopes clockwise turns the photo counter-clockwise (rotation -4, crs:CropAngle +4).
    await expect(page.getByTestId("view-main")).toHaveAttribute("data-rotation", "-4.00");
    await expect(page.getByTestId("slider-value-crop-angle")).toHaveText("+4.0°");
    await shot(page, `${P}05-straightened`);
  });

  test("portrait (orientation 8) photo: same sign, round trip", async ({ page }) => {
    await openDevelop(page, 21);
    await page.keyboard.press("r");
    await expect(page.getByTestId("crop-overlay")).toBeVisible();
    await page.getByTestId("slider-crop-angle").fill("7");
    await page.getByTestId("slider-crop-angle").evaluate((el) => (el as HTMLElement).blur());
    await expect(page.getByTestId("view-main")).toHaveAttribute("data-rotation", "-7.00");
    const before = await rectOf(page);
    await page.keyboard.press("Enter");
    await expect.poll(async () => (await saved(page)).length).toBe(1);
    await page.keyboard.press("r");
    const again = await rectOf(page);
    for (const k of ["l", "t", "r", "b"] as const) expect(again[k]).toBeCloseTo(before[k], 2);
  });
});

// ---------------------------------------------------------------------------------------------------------------------
// Masks: geometry against the backend evaluation (pure)
// ---------------------------------------------------------------------------------------------------------------------

test.describe("mask geometry", () => {
  const crop = (o: Partial<Frame["crop"]> = {}) => ({ enabled: false, left: 0, top: 0, right: 1, bottom: 1, angle: 0, ...o });
  const frame = (orientation: number, c = crop()): Frame => ({ orientation, crop: c, w: orientation >= 5 ? 4000 : 6000, h: orientation >= 5 ? 6000 : 4000 });

  test("dispToSensor matches the Rust crop_geometry tests", () => {
    // parity.rs: orientation 6, no crop: oriented top-left = un-oriented bottom-left.
    const a = dispToSensor({ x: 0, y: 0 }, frame(6));
    expect(a.x).toBeCloseTo(0, 9);
    expect(a.y).toBeCloseTo(1, 9);
    // parity.rs: left .1 top .2 right .6 bottom .7 at 5 degrees on 6000 x 4000: the frame corners land on the given points.
    const f = frame(1, crop({ enabled: true, left: 0.1, top: 0.2, right: 0.6, bottom: 0.7, angle: 5 }));
    const c0 = dispToSensor({ x: 0, y: 0 }, f);
    const c1 = dispToSensor({ x: 1, y: 1 }, f);
    expect(c0.x).toBeCloseTo(0.1, 3);
    expect(c0.y).toBeCloseTo(0.2, 3);
    expect(c1.x).toBeCloseTo(0.6, 3);
    expect(c1.y).toBeCloseTo(0.7, 3);
    // and back
    for (const o of [1, 3, 6, 8]) {
      const g = frame(o, crop({ enabled: true, left: 0.1, top: 0.2, right: 0.6, bottom: 0.7, angle: -7.5 }));
      for (const p of [{ x: 0.2, y: 0.9 }, { x: 0.77, y: 0.33 }]) {
        const q = sensorToDisp(dispToSensor(p, g), g);
        expect(q.x).toBeCloseTo(p.x, 6);
        expect(q.y).toBeCloseTo(p.y, 6);
      }
    }
  });

  test("the display <-> sensor map is a similarity: linear gradients are the same in pixel and screen space", () => {
    // Backend linear(): value = ((p - zero) . d) / |d|^2 with p, zero, full in SENSOR PIXELS (eval.rs). The handles are drawn
    // in screen space; that is only right when the mapping is a similarity. Check equal values for random points.
    const cases = [frame(1), frame(6, crop({ enabled: true, left: 0.1, top: 0.2, right: 0.7, bottom: 0.8, angle: 9 })), frame(8, crop({ enabled: true, left: 0.05, top: 0.1, right: 0.9, bottom: 0.9, angle: -14 }))];
    for (const f of cases) {
      const s = sensorSize(f);
      const box = { x: 30, y: 12, w: 900, h: 0 };
      // Displayed frame aspect from the map itself: one unit of u / v in sensor pixels.
      const o = dispToSensor({ x: 0, y: 0 }, f);
      const ux = dispToSensor({ x: 1, y: 0 }, f);
      const uy = dispToSensor({ x: 0, y: 1 }, f);
      const wPx = Math.hypot((ux.x - o.x) * s.w, (ux.y - o.y) * s.h);
      const hPx = Math.hypot((uy.x - o.x) * s.w, (uy.y - o.y) * s.h);
      box.h = (box.w * hPx) / wPx;
      // orthogonal and equally scaled axes
      expect((ux.x - o.x) * s.w * (uy.x - o.x) * s.w + (ux.y - o.y) * s.h * (uy.y - o.y) * s.h).toBeCloseTo(0, 3);
      const zero = { x: 0.31, y: 0.42 };
      const full = { x: 0.66, y: 0.57 };
      const toScreen = (p: { x: number; y: number }) => {
        const d = sensorToDisp(p, f);
        return { x: box.x + d.x * box.w, y: box.y + d.y * box.h };
      };
      const backend = (p: { x: number; y: number }) => {
        const [dx, dy] = [(full.x - zero.x) * s.w, (full.y - zero.y) * s.h];
        return (((p.x - zero.x) * s.w * dx + (p.y - zero.y) * s.h * dy) / (dx * dx + dy * dy));
      };
      const zs = toScreen(zero);
      const fs = toScreen(full);
      const screen = (p: { x: number; y: number }) => {
        const q = toScreen(p);
        const [dx, dy] = [fs.x - zs.x, fs.y - zs.y];
        return ((q.x - zs.x) * dx + (q.y - zs.y) * dy) / (dx * dx + dy * dy);
      };
      for (const p of [{ x: 0.5, y: 0.5 }, { x: 0.2, y: 0.9 }, { x: 0.8, y: 0.1 }, { x: 0.45, y: 0.61 }]) expect(screen(p)).toBeCloseTo(backend(p), 6);
    }
  });

  test("radial handles round-trip with straightened crops and orientations", () => {
    const base = { top: 0, left: 0, bottom: 0, right: 0, angle: 0, midpoint: 50, roundness: 0, feather: 50, flipped: false };
    for (const f of [frame(1, crop({ enabled: true, left: 0.05, top: 0.1, right: 0.9, bottom: 0.92, angle: 6 })), frame(6, crop({ enabled: true, left: 0.1, top: 0.1, right: 0.8, bottom: 0.9, angle: -11 })), frame(2, crop({ enabled: true, left: 0.1, top: 0.1, right: 0.8, bottom: 0.9, angle: 11 }))]) {
      const box = { x: 10, y: 20, w: 800, h: 0 };
      const o = dispToSensor({ x: 0, y: 0 }, f);
      const ux = dispToSensor({ x: 1, y: 0 }, f);
      const uy = dispToSensor({ x: 0, y: 1 }, f);
      const s = sensorSize(f);
      box.h = (box.w * Math.hypot((uy.x - o.x) * s.w, (uy.y - o.y) * s.h)) / Math.hypot((ux.x - o.x) * s.w, (ux.y - o.y) * s.h);
      const m = { ...base, left: 0.3, right: 0.5, top: 0.35, bottom: 0.6, angle: 20 };
      const e = radialToScreen(m, f, box);
      const back = radialFromScreen(e, f, box, m);
      for (const k of ["left", "right", "top", "bottom"] as const) expect(back[k]).toBeCloseTo(m[k], 4);
      expect(back.angle).toBeCloseTo(20, 1);
      // the on-screen centre is where the sensor centre maps
      const c = sensorToDisp({ x: 0.4, y: 0.475 }, f);
      expect(e.cx).toBeCloseTo(box.x + c.x * box.w, 4);
      expect(screenToDisp(e.cx, e.cy, box).x).toBeCloseTo(c.x, 6);
    }
  });
});

// ---------------------------------------------------------------------------------------------------------------------
// Masks: tone curve, reorder, overlay region, eyedropper
// ---------------------------------------------------------------------------------------------------------------------

test.describe("masking polish", () => {
  test("per-mask tone curve: points edit the group's curve fields, per channel, with undo", async ({ page }) => {
    await openMasks(page);
    await createSubject(page);
    await expect(page.getByTestId("mask-curve-editor")).toBeVisible();
    const gid = (await lastAdj(page)).masks[0]?.id ?? (await page.locator('[data-testid^="mask-group-"][data-selected="true"]').getAttribute("data-testid"))!.replace("mask-group-", "");
    const svg = page.getByTestId("mask-curve-svg");
    await svg.scrollIntoViewIfNeeded();
    const b = (await svg.boundingBox())!;
    const px = (x: number, y: number) => ({ x: b.x + ((8 + x) / 271) * b.width, y: b.y + ((8 + 255 - y) / 271) * b.height });
    const p = px(128, 190);
    await clearCalls(page);
    await page.mouse.click(p.x, p.y);
    await expect(svg).toHaveAttribute("data-points", /\[\[0,0\],\[12[0-9],1[89][0-9]\],\[255,255\]\]/);
    await expect.poll(async () => (await lastAdj(page)).masks.find((m) => m.id === gid)?.adjustments.toneCurve?.master?.length).toBe(3);
    const adj = (await lastAdj(page)).masks.find((m) => m.id === gid).adjustments;
    expect(adj.toneCurve.red).toEqual([[0, 0], [255, 255]]);
    expect(adj.toneCurve.master[1][1]).toBeGreaterThan(180);
    await expect.poll(async () => (await saved(page)).at(-1)?.args.label).toMatch(/^Mask: .+ Tone Curve$/);
    // The global tone curve is untouched.
    expect((await lastAdj(page)) as any).toMatchObject({ toneCurve: { point: { master: [[0, 0], [255, 255]] } } });

    // A different channel, dragged.
    await page.getByTestId("mask-curve-ch-blue").click();
    const q = px(60, 60);
    await page.mouse.move(q.x, q.y);
    await page.mouse.down();
    await page.mouse.move(px(60, 90).x, px(60, 90).y, { steps: 5 });
    await page.mouse.up();
    await expect.poll(async () => (await lastAdj(page)).masks.find((m) => m.id === gid).adjustments.toneCurve.blue.length).toBe(3);
    await shot(page, `${P}06-mask-curve`);

    // Refine saturation slider is there and writes its field.
    await page.getByTestId("slider-mask-curveRefineSaturation").fill("60");
    await page.getByTestId("slider-mask-curveRefineSaturation").evaluate((el) => (el as HTMLElement).blur());
    await expect.poll(async () => (await lastAdj(page)).masks.find((m) => m.id === gid).adjustments.curveRefineSaturation).toBe(60);

    // Reset curve returns the channel to identity; undo brings the point back.
    await page.getByTestId("mask-curve-ch-master").click();
    await page.getByTestId("mask-curve-reset").click();
    await expect.poll(async () => (await lastAdj(page)).masks.find((m) => m.id === gid).adjustments.toneCurve.master).toEqual([[0, 0], [255, 255]]);
    await expect(svg).toHaveAttribute("data-points", "[[0,0],[255,255]]");
    await page.keyboard.press("Meta+z");
    await expect.poll(async () => (await lastAdj(page)).masks.find((m) => m.id === gid).adjustments.toneCurve.master.length).toBe(3);
    // One more undo takes back the refine-saturation edit (its own history entry).
    await page.keyboard.press("Meta+z");
    await expect.poll(async () => (await lastAdj(page)).masks.find((m) => m.id === gid).adjustments.curveRefineSaturation).toBe(100);
  });

  test("groups reorder by drag handle and Alt+Up / Alt+Down; components too; order is array order", async ({ page }) => {
    await openMasks(page);
    await createSubject(page);
    await page.getByTestId("mask-create-sky").click();
    await expect(page.locator('[data-testid^="mask-group-"][data-selected="true"]')).toHaveCount(1);
    await expect.poll(async () => (await lastAdj(page)).masks.length).toBe(2);
    await page.getByTestId("mask-create-background").click();
    await expect.poll(async () => (await lastAdj(page)).masks.length).toBe(3);
    const ids = async () => (await lastAdj(page)).masks.map((m) => m.id as string);
    const [a, b, c] = await ids();
    await expect(page.getByTestId(`mask-group-${c}`)).toHaveAttribute("data-selected", "true");
    await shot(page, `${P}07-mask-list`);

    // Alt+Up moves the selected group (c) up.
    await clearCalls(page);
    await page.keyboard.press("Alt+ArrowUp");
    await expect.poll(ids).toEqual([a, c, b]);
    await expect.poll(async () => (await saved(page)).at(-1)?.args.label).toMatch(/^Mask: Reorder /);
    await page.keyboard.press("Alt+ArrowDown");
    await expect.poll(ids).toEqual([a, b, c]);
    await page.keyboard.press("Alt+ArrowDown"); // at the end: nothing
    await expect.poll(async () => (await saved(page)).length).toBeGreaterThan(0);
    expect(await ids()).toEqual([a, b, c]);

    // Drag the first group's handle below the last one.
    const h = (await page.getByTestId(`mask-group-handle-${a}`).boundingBox())!;
    const last = (await page.getByTestId(`mask-group-${c}`).boundingBox())!;
    await page.mouse.move(h.x + h.width / 2, h.y + h.height / 2);
    await page.mouse.down();
    await page.mouse.move(h.x + h.width / 2, last.y + last.height - 2, { steps: 8 });
    await page.mouse.up();
    await expect.poll(ids).toEqual([b, c, a]);
    // Drag it back to the top (after the 1.5 s window in which same-label edits share one history entry).
    await page.waitForTimeout(1600);
    const h2 = (await page.getByTestId(`mask-group-handle-${a}`).boundingBox())!;
    const first = (await page.getByTestId(`mask-group-${b}`).boundingBox())!;
    await page.mouse.move(h2.x + h2.width / 2, h2.y + h2.height / 2);
    await page.mouse.down();
    await page.mouse.move(h2.x + h2.width / 2, first.y + 2, { steps: 8 });
    await page.mouse.up();
    await expect.poll(ids).toEqual([a, b, c]);
    // Undo restores the previous order in one step.
    await page.keyboard.press("Meta+z");
    await expect.poll(ids).toEqual([b, c, a]);
    await page.keyboard.press("Meta+z");
    await expect.poll(ids).toEqual([a, b, c]);

    // Components: add a second component to group a, then reorder inside the group.
    await page.getByTestId(`mask-group-name-${a}`).click();
    await page.getByTestId(`mask-add-${a}`).click();
    await page.getByTestId("mask-add-item-sky").click();
    await expect.poll(async () => (await lastAdj(page)).masks[0].components.length).toBe(2);
    const comps = async () => ((await lastAdj(page)).masks[0].components as { id: string }[]).map((x) => x.id);
    const [c0, c1] = await comps();
    await expect(page.getByTestId(`mask-comp-${c1}`)).toHaveAttribute("data-selected", "true");
    await page.keyboard.press("Alt+ArrowUp"); // the selected component moves within its group
    await expect.poll(comps).toEqual([c1, c0]);
    expect(await ids()).toEqual([a, b, c]); // groups unchanged
    const ch = (await page.getByTestId(`mask-comp-handle-${c1}`).boundingBox())!;
    const other = (await page.getByTestId(`mask-comp-${c0}`).boundingBox())!;
    await page.mouse.move(ch.x + ch.width / 2, ch.y + ch.height / 2);
    await page.mouse.down();
    await page.mouse.move(ch.x + ch.width / 2, other.y + other.height - 1, { steps: 6 });
    await page.mouse.up();
    await expect.poll(comps).toEqual([c0, c1]);
    await shot(page, `${P}08-components-reordered`);
  });

  test("zoomed to 100%, the overlay is rendered for the visible region", async ({ page }) => {
    await openMasks(page);
    await createSubject(page);
    await page.getByTestId("mask-overlay-toggle").check();
    await expect(page.getByTestId("mask-overlay")).toBeVisible();
    const first = (await calls(page, "render_mask_overlay")).at(-1)!;
    expect((first.args.options as any).region).toBeNull();
    await clearCalls(page);
    await page.getByTestId("zoom-100").click();
    await expect(page.getByTestId("viewer")).toHaveAttribute("data-zoomed", "true");
    await expect.poll(async () => ((await calls(page, "render_mask_overlay")).at(-1)?.args.options as any)?.region).not.toBeNull();
    const o = (await calls(page, "render_mask_overlay")).at(-1)!.args.options as { maxEdge: number; region: { x: number; y: number; width: number; height: number } };
    // The visible part of a 6000 x 4000 frame in the viewport: a sub-rectangle, requested at its screen size.
    expect(o.region.width).toBeLessThan(1);
    expect(o.region.width).toBeGreaterThan(0);
    expect(o.region.x + o.region.width).toBeLessThanOrEqual(1.0001);
    expect(o.maxEdge).toBeGreaterThan(300);
    await expect(page.getByTestId("mask-overlay")).toHaveAttribute("data-overlay-region", /"width"/);
    // The overlay sits over exactly that region of the frame.
    const fr = await frameRect(page);
    const ob = (await page.getByTestId("mask-overlay").boundingBox())!;
    expect(ob.width).toBeCloseTo(o.region.width * fr.w, 0);
    expect(ob.x).toBeCloseTo(fr.x + o.region.x * fr.w, 0);
    await shot(page, `${P}09-overlay-region`);
    // Back to fit: the whole frame again.
    await clearCalls(page);
    await page.getByTestId("zoom-fit").click();
    await expect.poll(async () => ((await calls(page, "render_mask_overlay")).at(-1)?.args.options as any)?.region).toBeNull();
  });

  test("luminance eyedropper samples the rendered image, also when the canvas is tainted (blob fallback)", async ({ page }) => {
    // Simulate a tainted canvas (sieve:// images): reading pixels from a drawn <img> that is not a blob URL throws.
    await page.addInitScript(() => {
      if (!location.search.includes("taint")) return;
      const draw = CanvasRenderingContext2D.prototype.drawImage;
      const read = CanvasRenderingContext2D.prototype.getImageData;
      const tainted = new WeakSet<CanvasRenderingContext2D>();
      (CanvasRenderingContext2D.prototype as any).drawImage = function (this: CanvasRenderingContext2D, src: any, ...rest: any[]) {
        if (src instanceof HTMLImageElement && !src.src.startsWith("blob:")) tainted.add(this);
        return (draw as any).call(this, src, ...rest);
      };
      (CanvasRenderingContext2D.prototype as any).getImageData = function (this: CanvasRenderingContext2D, ...a: any[]) {
        if (tainted.has(this)) throw new DOMException("The canvas has been tainted by cross-origin data.", "SecurityError");
        return (read as any).apply(this, a);
      };
      (window as any).__taint = true;
    });
    const sample = async (query: string) => {
      await openApp(page, 200, query);
      await page.getByTestId("cell-1").click();
      await page.keyboard.press("d");
      await expect(page.getByTestId("view-main")).toBeVisible();
      await expect(page.getByTestId("histogram")).toHaveAttribute("data-empty", "false");
      await page.keyboard.press("Shift+Q");
      await expect(page.getByTestId("masks-panel")).toBeVisible();
      await expect(page.getByTestId("mask-capture")).toBeVisible();
      const r = await frameRect(page);
      await page.mouse.click(r.x + r.w * 0.92, r.y + r.h * 0.92); // the dark corner of the mock gradient
      await expect.poll(async () => (await lastAdj(page)).masks.length).toBe(1);
      const sh = (await lastAdj(page)).masks[0].components[0].shape;
      expect(sh.kind).toBe("luminance");
      return sh as { low: number; high: number; featherLow: number; featherHigh: number };
    };
    const direct = await sample("");
    // Not the 0.5 fallback (low .35 / high .65): the dark corner gives a clearly darker range.
    expect(direct.low).toBeLessThan(0.3);
    expect(direct.high - direct.low).toBeCloseTo(0.3, 2);
    const viaBlob = await sample("&taint=1");
    expect(await page.evaluate(() => (window as any).__taint)).toBe(true);
    expect(viaBlob.low).toBeCloseTo(direct.low, 1);
    expect(viaBlob.low).toBeLessThan(0.3);
  });
});
