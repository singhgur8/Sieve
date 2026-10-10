// IPC v20 (docs/ipc-changelog.md v20): target-count culling in the mock backend (`src/testing/mockTarget.ts`), which
// mirrors `db::target` (overlay, user edits, apply) with synthetic people / moments / selection. Invoke-level only.
import { expect, test, type Page } from "@playwright/test";
import { openHome } from "./helpers";

const inv = <T = unknown>(page: Page, cmd: string, args: Record<string, unknown> = {}) =>
  page.evaluate(
    ([c, a]) =>
      (window as unknown as { __TAURI_INTERNALS__: { invoke: (c: string, a: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke(c as string, a),
    [cmd, args] as const,
  ) as Promise<T>;

type Sel = { imageId: number; choice: string; alternativeOf: number | null; rank: number | null; coveredBy: number | null; locked: boolean; momentId: number | null };
type Run = { state: string; counts: { total: number; deliver: number; alternative: number; notSure: number; setAside: number }; peopleQuestions: number };
type Entry = { id: number; pick: string; pickOrigin: string | null; quality: { suggestedPick: string } };

test.describe("IPC v20 mock contract", () => {
  test("?target=1: people, moments of every shot type, selection with alternatives and covered-by", async ({ page }) => {
    await openHome(page, 201, "&target=1");
    const run = await inv<Run>(page, "get_target_run", { projectId: 1 });
    expect(run.state).toBe("finished");
    const c = run.counts;
    expect(c.total).toBe(c.deliver + c.alternative + c.notSure + c.setAside);
    expect(c.deliver).toBeGreaterThan(0);
    expect(c.alternative).toBeGreaterThan(0);

    const people = await inv<{ people: { id: number; role: string; samples: { crop: { x: number; width: number } }[] }[]; questions: number[] }>(page, "list_people", { projectId: 1 });
    expect(people.people.filter((p) => p.role === "main")).toHaveLength(2);
    expect(people.questions).toHaveLength(4);
    expect(run.peopleQuestions).toBe(4);
    expect(people.people[0].samples.length).toBeGreaterThan(0);
    const crop = people.people[0].samples[0].crop;
    expect(crop.x).toBeGreaterThanOrEqual(0);
    expect(crop.x + crop.width).toBeLessThanOrEqual(1.0001);

    const moments = await inv<{ shotType: string; imageIds: number[] }[]>(page, "list_moments", { projectId: 1 });
    expect(new Set(moments.map((m) => m.shotType))).toEqual(new Set(["couple", "group", "detail", "candid", "other"]));

    // An alternative's strip and a covered photo.
    const ids = moments.flatMap((m) => m.imageIds);
    const sels = await inv<Sel[]>(page, "get_image_selections", { ids });
    const alt = sels.find((s) => s.choice === "alternative")!;
    const strip = await inv<{ delivered: Sel; alternatives: Sel[] }>(page, "get_alternatives", { imageId: alt.imageId });
    expect(strip.delivered.imageId).toBe(alt.alternativeOf);
    expect(strip.alternatives.map((a) => a.rank)).toEqual(strip.alternatives.map((_, i) => i + 1));
    const covered = sels.find((s) => s.choice !== "deliver" && s.coveredBy != null)!;
    const cov = await inv<{ coveredById: number; text: string }>(page, "get_covered_by", { imageId: covered.imageId });
    expect(cov.text).toMatch(/^Already kept a similar one: DSC\d+$/);

    // Overlay: delivered photos are suggested picks.
    const delivered = sels.filter((s) => s.choice === "deliver").map((s) => s.imageId);
    const entries = await inv<Entry[]>(page, "get_images", { ids: delivered });
    expect(entries.every((e) => e.quality.suggestedPick === "pick")).toBe(true);

    // Grid filter for pass 2.
    const pass2 = await inv<number[]>(page, "list_image_ids", {
      query: { includeTags: [], excludeTags: [], tagMatch: "any", picks: [], minRating: null, maxRating: null, colorLabels: [], burstGroupId: null, folderId: null, projectId: 1, sort: "capture_time", sortDescending: false, offset: 0, limit: 200, targetChoices: ["not_sure", "set_aside"] },
    });
    expect(pass2.length).toBe(c.notSure + c.setAside);
  });

  test("swap / add / undo / apply keep the user's flags", async ({ page }) => {
    await openHome(page, 201, "&target=1");
    const moments = await inv<{ imageIds: number[] }[]>(page, "list_moments", { projectId: 1 });
    const sels = await inv<Sel[]>(page, "get_image_selections", { ids: moments.flatMap((m) => m.imageIds) });
    const alt = sels.find((s) => s.choice === "alternative")!;
    const d = alt.alternativeOf!;
    const r = await inv<{ changed: Sel[]; flagsChanged: number[]; previous: unknown[] }>(page, "swap_alternative", { deliveredId: d, alternativeId: alt.imageId });
    const now = new Map(r.changed.map((s) => [s.imageId, s]));
    expect(now.get(alt.imageId)!.choice).toBe("deliver");
    expect(now.get(d)!).toMatchObject({ choice: "alternative", alternativeOf: alt.imageId, rank: 1, locked: true });
    expect(r.flagsChanged).toContain(alt.imageId);
    // Undo.
    await inv(page, "restore_target_snapshot", { snapshots: r.previous });
    const back = await inv<Sel[]>(page, "get_image_selections", { ids: [d, alt.imageId] });
    expect(back[0].choice).toBe("deliver");
    expect(back[1].alternativeOf).toBe(d);

    // Add both: a not-sure photo joins the delivery set, picked as the user's.
    const unsure = sels.find((s) => s.choice === "not_sure")!;
    const added = await inv<{ changed: Sel[] }>(page, "add_alternative", { imageId: unsure.imageId });
    expect(added.changed.find((s) => s.imageId === unsure.imageId)).toMatchObject({ choice: "deliver", locked: true, coveredBy: null });
    expect((await inv<Entry[]>(page, "get_images", { ids: [unsure.imageId] }))[0].pick).toBe("pick");

    // A user reject on a delivered photo locks it as set aside; apply leaves it alone.
    await inv(page, "set_pick", { ids: [d], pick: "reject" });
    const locked = (await inv<Sel[]>(page, "get_image_selections", { ids: [d] }))[0];
    expect(locked).toMatchObject({ choice: "set_aside", locked: true });
    const applied = await inv<{ applied: number }>(page, "apply_target_selection", { projectId: 1 });
    expect(applied.applied).toBeGreaterThan(0);
    const [rej] = await inv<Entry[]>(page, "get_images", { ids: [d] });
    expect([rej.pick, rej.pickOrigin]).toEqual(["reject", "user"]);
    expect((await inv<{ appliedAtMs: number | null }>(page, "get_target_run", { projectId: 1 })).appliedAtMs).not.toBeNull();
  });

  test("run_target_selection without a run: running, then finished with a selection", async ({ page }) => {
    await openHome(page, 201);
    expect(await inv(page, "get_target_run", { projectId: 1 })).toBeNull();
    expect((await inv<{ message: string | null }>(page, "list_people", { projectId: 1 })).message).toBe("Face recognition is not available yet");
    const started = await inv<Run>(page, "run_target_selection", { projectId: 1, settings: { targetCount: 40, shootType: null } });
    expect(started.state).toBe("running");
    await expect(inv(page, "run_target_selection", { projectId: 1, settings: { targetCount: 40, shootType: null } })).rejects.toBeTruthy();
    await expect.poll(async () => (await inv<Run>(page, "get_target_run", { projectId: 1 })).state).toBe("finished");
    const run = await inv<Run>(page, "get_target_run", { projectId: 1 });
    expect(run.counts.deliver).toBeGreaterThan(0);
    await expect(inv(page, "run_target_selection", { projectId: 1, settings: { targetCount: 0, shootType: null } })).rejects.toBeTruthy();
  });
});
