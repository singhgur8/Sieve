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
    expect(cov.text).toMatch(/^(Almost identical to|Similar to|Same moment as|Looks like) DSC\d+ \(kept(, another moment)?\)$/);

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
    const applied = await inv<{ picks: number; changed: number[] }>(page, "apply_target_selection", { projectId: 1, opts: { rejects: true } });
    expect(applied.picks).toBeGreaterThan(0);
    const [rej] = await inv<Entry[]>(page, "get_images", { ids: [d] });
    expect([rej.pick, rej.pickOrigin]).toEqual(["reject", "user"]);
    expect((await inv<{ appliedAtMs: number | null }>(page, "get_target_run", { projectId: 1 })).appliedAtMs).not.toBeNull();
  });

  test("v20.1: covered-by follows adds, piles, reason / pile filters, moment sort, lock, plan + apply + undo", async ({ page }) => {
    await openHome(page, 201, "&target=1");
    type Sel1 = Sel & { origin: string; pile: string | null; coveredSimilarity: number | null; coveredTier: string | null; reasons: { kind: string }[]; score: number };
    type Cov = { coveredById: number; coveredByName: string; similarity: number; tier: string; sameMoment: boolean; coveredByOrigin: string; text: string };
    const moments = await inv<{ id: number; imageIds: number[]; deliveredIds: number[]; userDeliveredIds: number[] }[]>(page, "list_moments", { projectId: 1 });
    const all = await inv<Sel1[]>(page, "get_image_selections", { ids: moments.flatMap((m) => m.imageIds) });
    const byId = new Map(all.map((s) => [s.imageId, s]));

    // Engine contract mirrored: user-rejected photos are never delivered; backs of heads are set aside without a cover.
    expect(all.filter((s) => s.reasons[0]?.kind === "no_visible_face").every((s) => s.choice === "set_aside" && s.coveredBy == null)).toBe(true);
    const deliveredEntries = await inv<Entry[]>(page, "get_images", { ids: all.filter((s) => s.choice === "deliver").map((s) => s.imageId) });
    expect(deliveredEntries.some((e) => e.pick === "reject" && e.pickOrigin !== "auto")).toBe(false);

    // Piles add up and match the filter.
    const run = await inv<{ counts: { notSure: number; setAside: number; piles: { notSure: number; similar: number; weaker: number; defects: number } } }>(page, "get_target_run", { projectId: 1 });
    const pl = run.counts.piles;
    expect(pl.notSure + pl.similar + pl.weaker + pl.defects).toBe(run.counts.notSure + run.counts.setAside);
    expect(pl.notSure).toBe(run.counts.notSure);
    const q = (extra: Record<string, unknown>) => ({ includeTags: [], excludeTags: [], tagMatch: "any", picks: [], minRating: null, maxRating: null, colorLabels: [], burstGroupId: null, folderId: null, projectId: 1, sort: "capture_time", sortDescending: false, offset: 0, limit: 200, ...extra });
    for (const [pile, n] of [["not_sure", pl.notSure], ["similar", pl.similar], ["weaker", pl.weaker], ["defects", pl.defects]] as const) {
      expect((await inv<number[]>(page, "list_image_ids", { query: q({ targetPiles: [pile] }) })).length, pile).toBe(n);
    }
    const nf = await inv<number[]>(page, "list_image_ids", { query: q({ targetReasonKinds: ["no_visible_face"] }) });
    expect(nf.length).toBe(all.filter((s) => s.reasons[0]?.kind === "no_visible_face").length);

    // Sort by moment, then score: moments in order, best first within each.
    const sorted = await inv<number[]>(page, "list_image_ids", { query: q({ targetPiles: ["not_sure"], sort: "target_moment" }) });
    const momentOrder = new Map(moments.map((m, i) => [m.id, i]));
    for (let i = 1; i < sorted.length; i++) {
      const a = byId.get(sorted[i - 1])!;
      const b = byId.get(sorted[i])!;
      const ma = momentOrder.get(a.momentId!)!;
      const mb = momentOrder.get(b.momentId!)!;
      expect(ma <= mb && (ma < mb || a.score >= b.score), `${a.imageId} before ${b.imageId}`).toBe(true);
    }

    // Covered-by is recomputed after an add: a moment with a delivered photo and two other covered frames.
    const m = moments.find((x) => x.deliveredIds.length > 0 && x.imageIds.filter((id) => byId.get(id)!.choice !== "deliver" && byId.get(id)!.coveredBy != null).length >= 2)!;
    const [a, b] = m.imageIds.filter((id) => byId.get(id)!.choice !== "deliver" && byId.get(id)!.coveredBy != null);
    const added = await inv<{ changed: Sel1[]; previous: unknown[] }>(page, "add_alternative", { imageId: a });
    expect(added.changed.find((s) => s.imageId === a)).toMatchObject({ choice: "deliver", origin: "user" });
    const covB = await inv<Cov>(page, "get_covered_by", { imageId: b });
    const pairSim = (x: number, y: number) => {
      const [lo, hi] = x < y ? [x, y] : [y, x];
      return Math.round((0.97 - ((lo * 7 + hi * 13) % 43) / 100) * 1000) / 1000;
    };
    const delivered = [...m.deliveredIds, a].sort((x, y) => x - y);
    const best = delivered.reduce((acc, d) => (pairSim(b, d) > pairSim(b, acc) ? d : acc), delivered[0]);
    expect(covB.coveredById).toBe(best);
    expect(covB.sameMoment).toBe(true);
    if (best === a) {
      expect(covB.coveredByOrigin).toBe("user");
      expect(covB.text).toContain(`${covB.coveredByName} (you added it)`);
    }
    const tier = covB.similarity >= 0.9 ? "near_identical" : covB.similarity >= 0.7 ? "very_similar" : "same_moment";
    expect(covB.tier).toBe(tier);
    const mAfter = (await inv<{ id: number; userDeliveredIds: number[] }[]>(page, "list_moments", { projectId: 1 })).find((x) => x.id === m.id)!;
    expect(mAfter.userDeliveredIds).toContain(a);
    // Undo puts covers, origin and reasons back.
    await inv(page, "restore_target_snapshot", { snapshots: added.previous });
    const [aBack, bBack] = await inv<Sel1[]>(page, "get_image_selections", { ids: [a, b] });
    expect([aBack.choice, aBack.origin, aBack.reasons]).toEqual([byId.get(a)!.choice, "engine", byId.get(a)!.reasons]);
    expect([bBack.coveredBy, bBack.coveredSimilarity]).toEqual([byId.get(b)!.coveredBy, byId.get(b)!.coveredSimilarity]);

    // Lock without flags; Keep (same choice) flags.
    const d0 = m.deliveredIds[0];
    expect(await inv<number[]>(page, "lock_target_choices", { ids: [d0] })).toEqual([d0]);
    expect(await inv<number[]>(page, "lock_target_choices", { ids: [d0] })).toEqual([]);
    expect((await inv<Sel1[]>(page, "get_image_selections", { ids: [d0] }))[0]).toMatchObject({ locked: true, origin: "engine", choice: "deliver" });
    expect((await inv<Entry[]>(page, "get_images", { ids: [d0] }))[0].pick).toBe("unflagged");

    // Plan = apply; rejects off leaves defects alone; undo through restore_cull_snapshot.
    type Plan = { total: number; picks: number; rejects: number; rejectable: number; unflags: number; unchanged: number; userFlagged: number };
    const planOff = await inv<Plan>(page, "plan_target_apply", { projectId: 1, opts: { rejects: false } });
    const planOn = await inv<Plan>(page, "plan_target_apply", { projectId: 1, opts: { rejects: true } });
    expect(planOff.rejects).toBe(0);
    expect(planOn.rejects).toBe(planOn.rejectable);
    expect(planOn.rejectable).toBeGreaterThan(0);
    for (const p of [planOff, planOn]) expect(p.picks + p.rejects + p.unflags + p.unchanged + p.userFlagged).toBe(p.total);
    const res = await inv<{ picks: number; rejects: number; unflags: number; changed: number[]; previous: { imageId: number; pick: string }[] }>(page, "apply_target_selection", { projectId: 1, opts: { rejects: false } });
    expect([res.picks, res.rejects, res.unflags]).toEqual([planOff.picks, 0, planOff.unflags]);
    expect(res.previous.map((s) => s.imageId)).toEqual(res.changed);
    await inv(page, "restore_cull_snapshot", { snapshots: res.previous });
    const back = await inv<Entry[]>(page, "get_images", { ids: res.changed });
    expect(back.map((e) => e.pick)).toEqual(res.previous.map((s) => s.pick));
    const again = await inv<Plan>(page, "plan_target_apply", { projectId: 1, opts: { rejects: true } });
    expect(again).toEqual(planOn);
    await expect(inv(page, "apply_target_selection", { projectId: 1 })).rejects.toBeTruthy();
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
