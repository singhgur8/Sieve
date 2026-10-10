// IPC v19.2 (docs/ipc-changelog.md v19.2, docs/ux-review-8d.md P1-1 / P1-3 / P1-4 / P1-5): the mock backend emulates the
// contract like the Rust tests (db::repo camera_body_filter_and_facet / suggested_filter_counts_and_apply_kinds,
// db::capture_time sync_cameras_by_body_or_model_moves_the_whole_project, develop::sync_delta). Invoke-level only.
// Mock rules: project 1 = ids 1..101; `?twobodies=1` = every 3rd frame from a second ILCE-7M4 (serial 05119876).
import { expect, test, type Page } from "@playwright/test";
import { openHome } from "./helpers";

type Err = { kind: string; message: string };
type Body = { make: string; model: string | null; serial: string | null };
type Adj = { exposure: number; contrast: number; whiteBalance: { mode: string; temperatureK?: number; tint?: number } } & Record<string, unknown>;

const invoke = (page: Page, cmd: string, args: Record<string, unknown>) =>
  page.evaluate(
    ([c, a]) =>
      (window as unknown as { __TAURI_INTERNALS__: { invoke: (c: string, a: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke(c as string, a).then(
        (v: unknown) => ({ ok: true, v }),
        (e: unknown) => ({ ok: false, v: e }),
      ),
    [cmd, args] as const,
  ) as Promise<{ ok: boolean; v: unknown }>;
const inv = async <T>(page: Page, cmd: string, args: Record<string, unknown>) => {
  const r = await invoke(page, cmd, args);
  if (!r.ok) throw new Error(`${cmd}: ${JSON.stringify(r.v)}`);
  return r.v as T;
};
const fail = async (page: Page, cmd: string, args: Record<string, unknown>) => {
  const r = await invoke(page, cmd, args);
  return r.ok ? null : (r.v as Err);
};
const query = (extra: Record<string, unknown>) => ({
  includeTags: [],
  excludeTags: [],
  tagMatch: "any",
  picks: [],
  pickOrigin: null,
  minRating: null,
  maxRating: null,
  colorLabels: [],
  burstGroupId: null,
  sceneId: null,
  collapseBursts: false,
  missingOnly: false,
  folderId: null,
  projectId: 1,
  keepersOnly: false,
  metadata: {},
  sort: "capture_time",
  sortDescending: false,
  offset: 0,
  limit: 1000,
  ...extra,
});

test.describe("IPC v19.2 mock contract", () => {
  test("camera bodies: facet, filter, and sync by body / model over the whole project", async ({ page }) => {
    await openHome(page, 201, "&twobodies=1");
    const opts = await inv<{ bodies: { body: Body; count: number }[] }>(page, "get_metadata_filter_options", { query: query({}) });
    expect(opts.bodies.map((b) => [b.body.serial, b.count])).toEqual([
      ["05119876", 33],
      ["06258214", 68],
    ]);
    const second: Body = { make: "sony", model: "ILCE-7M4", serial: "05119876" };
    const ids = await inv<number[]>(page, "list_image_ids", { query: query({ metadata: { bodies: [second] } }) });
    expect(ids).toHaveLength(33);
    expect(ids.every((id) => id % 3 === 0)).toBe(true);
    const [e3] = await inv<{ camera: Body }[]>(page, "get_images", { ids: [3] });
    expect(e3.camera.serial).toBe("05119876");
    expect((await inv<{ cameraSerial: string | null }>(page, "get_image_metadata", { id: 3 })).cameraSerial).toBe("05119876");

    // Model scope would move the reference too.
    expect((await fail(page, "edit_capture_time", { ids: [], mode: { kind: "sync_cameras", referenceId: 1, targetId: 3, scope: "model" } }))?.kind).toBe(
      "invalid_argument",
    );
    // Body scope: every frame of the second body in project 1, `ids` ignored, the first body untouched.
    const r = await inv<{ changedIds: number[]; offsetMs: number; previous: unknown[] }>(page, "edit_capture_time", {
      ids: [1, 2],
      mode: { kind: "sync_cameras", referenceId: 1, targetId: 3, scope: "body" },
    });
    expect([...r.changedIds].sort((a, b) => a - b)).toEqual(ids);
    expect(r.offsetMs).toBe(-3_600_600);
    // Omitted scope = the selected photos (v19 behaviour).
    const sel = await inv<{ changedIds: number[] }>(page, "edit_capture_time", { ids: [6], mode: { kind: "sync_cameras", referenceId: 2, targetId: 3 } });
    expect(sel.changedIds).toEqual([6]);
  });

  test("suggested filter, counts and apply per kind", async ({ page }) => {
    await openHome(page, 201);
    const summary = () => inv<{ suggestedRejectPending: number; suggestedPickPending: number; suggestedRatingPending: number }>(page, "get_cull_summary", { projectId: 1 });
    const s0 = await summary();
    const rejects = await inv<number[]>(page, "list_image_ids", { query: query({ suggested: "reject" }) });
    const picks = await inv<number[]>(page, "list_image_ids", { query: query({ suggested: "pick" }) });
    expect(rejects.length).toBe(s0.suggestedRejectPending);
    expect(rejects.length).toBeGreaterThan(0);
    expect(picks.length).toBe(s0.suggestedPickPending);
    const c = await inv<{ suggestedReject: number; suggestedPick: number; suggestedRating: number }>(page, "get_filter_counts", {
      folderId: null,
      projectId: 1,
      keepersOnly: null,
      metadata: null,
      pickOrigin: null,
    });
    expect([c.suggestedReject, c.suggestedPick, c.suggestedRating]).toEqual([s0.suggestedRejectPending, s0.suggestedPickPending, s0.suggestedRatingPending]);

    const all = await inv<number[]>(page, "list_image_ids", { query: query({}) });
    const before = await inv<{ id: number; rating: number }[]>(page, "get_images", { ids: rejects });
    const r = await inv<{ applied: number }>(page, "apply_suggestions", { ids: all, onlyUnset: true, kinds: { picks: false, rejects: true, stars: false } });
    expect(r.applied).toBe(rejects.length);
    const after = await inv<{ id: number; pick: string; rating: number; pickOrigin: string | null }[]>(page, "get_images", { ids: rejects });
    expect(after.every((e) => e.pick === "reject" && e.pickOrigin === "auto")).toBe(true);
    expect(after.map((e) => e.rating)).toEqual(before.map((e) => e.rating));
    const s1 = await summary();
    expect([s1.suggestedRejectPending, s1.suggestedPickPending, s1.suggestedRatingPending]).toEqual([0, s0.suggestedPickPending, s0.suggestedRatingPending]);
    expect(await inv<number[]>(page, "list_image_ids", { query: query({ suggested: "reject" }) })).toEqual([]);
  });

  test("sync_delta: relative exposure, one sync batch, one undo", async ({ page }) => {
    await openHome(page, 201);
    const adj = (id: number) => inv<Adj>(page, "get_adjustments", { id });
    const a2 = await adj(2);
    await inv(page, "save_adjustments", { id: 2, adjustments: { ...a2, exposure: 1, contrast: 20 }, label: "Exposure" });
    const before = await adj(1);
    const after = { ...before, exposure: 0.5 };
    const r = await inv<{ batch: { batchId: number; changedIds: number[] }; fields: string[]; relativeFields: string[]; history: { entries: { label: string }[] } }>(
      page,
      "sync_delta",
      { sourceId: 1, before, after, targetIds: [2, 3, 4, 5, 1, 2], options: { label: "Exposure" } },
    );
    expect(r.fields).toEqual(["exposure"]);
    expect(r.relativeFields).toEqual(["exposure"]);
    expect(r.batch.changedIds).toEqual([1, 2, 3, 4, 5]);
    expect(r.history.entries.at(-1)?.label).toBe("Exposure");
    expect((await adj(1)).exposure).toBe(0.5);
    expect((await adj(2)).exposure).toBe(1.5);
    expect((await adj(2)).contrast).toBe(20);
    expect((await adj(3)).exposure).toBe(0.5);
    const [info] = await inv<{ kind: string; imageCount: number; undoable: boolean }[]>(page, "get_edit_batches", { batchIds: [r.batch.batchId] });
    expect(info).toMatchObject({ kind: "sync", imageCount: 5, undoable: true });
    await inv(page, "undo_edit_batch", { batchId: r.batch.batchId });
    expect([(await adj(1)).exposure, (await adj(2)).exposure, (await adj(3)).exposure]).toEqual([0, 1, 0]);
    expect((await fail(page, "sync_delta", { sourceId: 1, before, after, targetIds: [2], options: { relative: ["contrast"] } }))?.kind).toBe("invalid_argument");
  });
});
