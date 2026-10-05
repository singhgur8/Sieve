// IPC v19.3 (docs/ipc-changelog.md v19.3, docs/ux-review-8d.md R1-3): `get_transform_bounds` in the mock backend, which
// mirrors the Rust contract (develop::transform::tests bounds_*) with a simplified warp: no warp -> both null; Upright on
// -> a clockwise trapezoid with the bottom corners pulled in, and a constrained crop inside it. Invoke-level only.
import { expect, test, type Page } from "@playwright/test";
import { openHome } from "./helpers";

type Crop = { enabled: boolean; top: number; left: number; bottom: number; right: number; angle: number };
type Bounds = { validQuad: [number, number][] | null; constrainedCrop: Crop | null };

const inv = (page: Page, cmd: string, args: Record<string, unknown>) =>
  page.evaluate(
    ([c, a]) =>
      (window as unknown as { __TAURI_INTERNALS__: { invoke: (c: string, a: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke(c as string, a),
    [cmd, args] as const,
  );

test.describe("IPC v19.3 mock contract", () => {
  test("get_transform_bounds: null without a warp, trapezoid + constrained crop with Upright", async ({ page }) => {
    await openHome(page, 201);
    const adj = (await inv(page, "get_adjustments", { id: 1 })) as Record<string, unknown> & { transform: Record<string, unknown> };
    const none = (await inv(page, "get_transform_bounds", { id: 1, adjustments: adj })) as Bounds;
    expect(none).toEqual({ validQuad: null, constrainedCrop: null });

    const upright = { ...adj, transform: { ...adj.transform, upright: "vertical" } };
    const b = (await inv(page, "get_transform_bounds", { id: 1, adjustments: upright })) as Bounds;
    const q = b.validQuad!;
    expect(q).toHaveLength(4);
    expect(q[3][0]).toBeGreaterThan(0);
    expect(q[2][0]).toBeLessThan(1);
    // Clockwise on screen (y down): positive shoelace sum.
    const area = q.reduce((s, p, i) => s + p[0] * q[(i + 1) % 4][1] - q[(i + 1) % 4][0] * p[1], 0);
    expect(area).toBeGreaterThan(0);
    const c = b.constrainedCrop!;
    expect(c.enabled).toBe(true);
    expect(c.right - c.left).toBeLessThan(1);
    expect(c.bottom - c.top).toBeLessThan(1);
  });
});
