import { expect, test } from "@playwright/test";
import { FULL, fitInsideRotated, fitRatio, insideRotated, rectArea, type Rect } from "../../src/lib/crop";
import { constrainTool, newTool, setAspectTool, swapTool, withBase } from "../../src/components/develop/CropOverlay";
import { overlayShapes } from "../../src/lib/overlays";

// Pure geometry checks (no browser): the crop rectangle under rotation, as Lightroom's "constrain to image".

const A = 3 / 2;
const RAD = Math.PI / 180;

/** Analytic area (fraction of the frame) of the largest rect with the frame's own aspect inside the frame rotated by `deg`. */
function analyticArea(deg: number, aspect = A): number {
  const th = Math.abs(deg) * RAD;
  const [s, c] = [Math.sin(th), Math.cos(th)];
  // Frame W = aspect, H = 1; rect w x (w / aspect): w c + h s <= W and w s + h c <= H.
  const w = Math.min(aspect / (c + s / aspect), 1 / (s + c / aspect));
  return (w * (w / aspect)) / aspect;
}

const close = (a: Rect, b: Rect) => (["l", "t", "r", "b"] as const).every((k) => Math.abs(a[k] - b[k]) < 1e-9);

test.describe("crop geometry", () => {
  for (const deg of [5, 10, 30]) {
    test(`3:2 at ${deg} degrees: area within 0.5% of the analytic largest inscribed rect`, () => {
      for (const sign of [1, -1]) {
        const rot = sign * deg;
        const r = fitInsideRotated(FULL, A, rot);
        expect(insideRotated(r, A, rot)).toBe(true);
        const want = analyticArea(deg);
        expect(Math.abs(rectArea(r) - want) / want).toBeLessThan(0.005);
        // Same ratio as the frame, centred.
        expect(((r.r - r.l) * A) / (r.b - r.t)).toBeCloseTo(A, 3);
        expect((r.l + r.r) / 2).toBeCloseTo(0.5, 3);
      }
    });
  }

  test("rotating to theta and back to 0 restores the original rect (no shrink-only drift)", () => {
    const base: Rect = { l: 0.05, t: 0.1, r: 0.95, b: 0.9 };
    let tool = newTool(base, 0, "free", "thirds");
    for (const angle of [2, 7, 15, 30, 12, 4, 0]) tool = constrainTool({ ...tool, angle }, A, 1);
    expect(close(tool.rect, base)).toBe(true);
    // And through the full-frame crop with a locked ratio.
    let t2 = newTool(FULL, 0, "original", "thirds");
    t2 = constrainTool({ ...t2, angle: 8 }, A, 1);
    expect(rectArea(t2.rect)).toBeLessThan(0.8);
    t2 = constrainTool({ ...t2, angle: 0 }, A, 1);
    expect(close(t2.rect, FULL)).toBe(true);
  });

  test("an off-centre rect slides inward before it shrinks", () => {
    const base: Rect = { l: 0.5, t: 0.05, r: 0.95, b: 0.65 }; // small, near the right edge
    const rot = -6; // preview rotation
    const r = fitInsideRotated(base, A, rot);
    expect(insideRotated(r, A, rot)).toBe(true);
    // Size is kept when sliding is enough.
    const keep = (r.r - r.l) / (base.r - base.l);
    expect(keep).toBeGreaterThan(0.97);
    expect(Math.abs((r.r - r.l) * A - (r.b - r.t) * (((base.r - base.l) * A) / (base.b - base.t)))).toBeLessThan(1e-6);
  });

  test("constrain off keeps the intended rect at any angle", () => {
    const tool = { ...newTool(FULL, 20, "original", "thirds"), constrain: false };
    expect(close(constrainTool(tool, A, 1).rect, FULL)).toBe(true);
  });

  test("X swap twice returns to the same rect; aspect change uses the largest rect of the new ratio", () => {
    let t = newTool(FULL, 0, "original", "thirds");
    t = setAspectTool(t, "1:1", A);
    expect(((t.rect.r - t.rect.l) * A) / (t.rect.b - t.rect.t)).toBeCloseTo(1, 6);
    t = setAspectTool(t, "original", A);
    expect(close(t.rect, FULL)).toBe(true);
    const swapped = swapTool(t, A);
    expect(((swapped.rect.r - swapped.rect.l) * A) / (swapped.rect.b - swapped.rect.t)).toBeCloseTo(1 / A, 6);
    expect(close(swapTool(swapped, A).rect, FULL)).toBe(true);
    const moved = withBase(t, { l: 0.1, t: 0.1, r: 0.5, b: 0.5 });
    expect(moved.base).toEqual(moved.rect);
  });

  test("fitRatio stays inside; overlays produce geometry", () => {
    const r = fitRatio(FULL, 1 / A);
    expect(r.l).toBeGreaterThanOrEqual(0);
    for (const id of ["thirds", "grid", "goldenRatio", "goldenSpiral", "diagonal", "triangle", "aspects"] as const) {
      const o = overlayShapes(id, 1.5);
      expect(o.lines.length + o.paths.length).toBeGreaterThan(0);
    }
  });
});
