// IPC v13 (Phase 8 hardening): contract of the mock backend for missing originals, the
// `missing` facet, relocate_folder and catalog health / backup restore. The UI for it is
// frontend-dev's; these tests pin the mock the UI is built against.
import { expect, test } from "@playwright/test";
import { openApp } from "./helpers";

test.describe("hardening contract (mock, IPC v13)", () => {
  test("?missing=5 flags images, facet + filter, file_missing errors, relocate clears", async ({ page }) => {
    await openApp(page, 50, "&missing=5");
    const r = await page.evaluate(async () => {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const ipc: any = await import("/src/ipc/index.ts");
      const { commands } = ipc;
      // The mock hands out live objects: snapshot each result before later calls mutate it.
      const unwrap = async (p: Promise<unknown>) => JSON.parse(JSON.stringify(await ipc.unwrap(p)));
      const base = { includeTags: [], excludeTags: [], tagMatch: "any", picks: [], minRating: null, maxRating: null, colorLabels: [], burstGroupId: null, collapseBursts: false, folderId: null, sort: "file_name", sortDescending: false, offset: 0, limit: 100 };
      const counts = await unwrap(commands.getFilterCounts(null));
      const ids = await unwrap(commands.listImageIds({ ...base, missingOnly: true }));
      const entry = await unwrap(commands.getImage(1));
      const present = await unwrap(commands.getImage(6));
      const info = await commands.getDevelopInfo(1);
      const tooFar = await commands.relocateFolder(1, "/Volumes/empty");
      const partial = await unwrap(commands.relocateFolder(1, "/Volumes/partial/ceremony"));
      const moved = await unwrap(commands.relocateFolder(1, "/Volumes/Shoots/ceremony"));
      const after = await unwrap(commands.getFilterCounts(null));
      const state = await unwrap(commands.getCatalogState());
      const relocated = await unwrap(commands.getImage(2));
      return { counts, ids, entry, present, info, tooFar, partial, moved, after, state, relocated };
    });
    expect(r.counts.missing).toBe(5);
    expect(r.ids).toEqual([1, 2, 3, 4, 5]);
    expect(typeof r.entry.missingSinceMs).toBe("number");
    expect(r.present.missingSinceMs).toBeNull();
    expect(r.info.status).toBe("error");
    expect(r.info.error.kind).toBe("file_missing");
    expect(r.info.error.message).toMatch(/^Original file is missing or was moved/);
    expect(r.tooFar.status).toBe("error");
    expect(r.tooFar.error.kind).toBe("invalid_argument");
    expect(r.partial).toEqual({ matched: 24, stillMissing: 1 });
    expect(r.moved).toEqual({ matched: 25, stillMissing: 0 });
    expect(r.after.missing).toBe(0);
    expect(r.state.folders[0].path).toBe("/Volumes/Shoots/ceremony");
    expect(r.relocated.path).toBe("/Volumes/Shoots/ceremony/DSC00002.ARW");
    expect(r.relocated.missingSinceMs).toBeNull();
  });

  test("healthy catalog lists backups; restore stages one", async ({ page }) => {
    await openApp(page, 50);
    const r = await page.evaluate(async () => {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const ipc: any = await import("/src/ipc/index.ts");
      const { commands } = ipc;
      const unwrap = async (p: Promise<unknown>) => JSON.parse(JSON.stringify(await ipc.unwrap(p)));
      const before = (await unwrap(commands.getCatalogState())).health;
      const missing = await commands.restoreCatalogBackup(9);
      const staged = await unwrap(commands.restoreCatalogBackup(1));
      const after = (await unwrap(commands.getCatalogState())).health;
      return { before, missing, staged, after };
    });
    expect(r.before.status).toBe("ok");
    expect(r.before.message).toBeNull();
    expect(r.before.backups.map((b: { index: number }) => b.index)).toEqual([1, 2, 3]);
    expect(r.before.restorePending).toBe(false);
    expect(r.missing.error.kind).toBe("not_found");
    expect(r.staged.restorePending).toBe(true);
    expect(r.after.restorePending).toBe(true);
  });

  test("?health=read_only refuses writes with catalog_read_only", async ({ page }) => {
    await openApp(page, 50, "&health=read_only");
    const r = await page.evaluate(async () => {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const { commands, unwrap }: any = await import("/src/ipc/index.ts");
      const health = (await unwrap(commands.getCatalogState())).health;
      const write = await commands.setRating([1], 3);
      return { health, write };
    });
    expect(r.health.status).toBe("read_only");
    expect(r.health.message).toContain("read-only");
    expect(r.write.status).toBe("error");
    expect(r.write.error.kind).toBe("catalog_read_only");
  });
});
