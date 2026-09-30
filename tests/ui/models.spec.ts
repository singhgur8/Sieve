// IPC v12 model downloads: contract of the mock backend (simulated progress, cancel, failure).
// The UI for it is frontend-dev's; these tests pin the mock the UI is built against.
import { expect, test, type Page } from "@playwright/test";
import { openApp } from "./helpers";

type Outcome = {
  before: { installed: boolean; subject: boolean };
  progress: { fileIndex: number; bytesDone: number; bytesTotal: number }[];
  finished: { ok: boolean; cancelled: boolean; error: string | null };
  after: { installed: boolean; downloading: string | null; subject: boolean };
  busyError: string | null;
};

/** Runs `downloadModels` through the typed wrappers; cancels after `cancelAfter` progress events. */
async function download(page: Page, cancelAfter?: number): Promise<Outcome> {
  return page.evaluate(async (cancelAfter) => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const ipc: any = await import("/src/ipc/index.ts");
    const { commands, events, unwrap, MODEL_GROUP_SEGMENTATION } = ipc;
    const subject = async () => (await unwrap(commands.getMaskCapabilities())).ai.find((c: { kind: string }) => c.kind === "subject").available;
    const status = async () => unwrap(commands.modelDownloadsStatus());
    const s0 = await status();
    const before = { installed: s0.groups[0].installed, subject: await subject() };
    const progress: Outcome["progress"] = [];
    const done = new Promise<Outcome["finished"]>((resolve) => {
      void events.modelDownloadFinished.listen((e: { payload: Outcome["finished"] }) => resolve(e.payload));
    });
    await events.modelDownloadProgress.listen((e: { payload: Outcome["progress"][number] }) => {
      progress.push(e.payload);
      if (cancelAfter !== undefined && progress.length === cancelAfter) void unwrap(commands.cancelModelDownload());
    });
    await unwrap(commands.downloadModels(MODEL_GROUP_SEGMENTATION));
    let busyError: string | null = null;
    try {
      await unwrap(commands.downloadModels(MODEL_GROUP_SEGMENTATION));
    } catch (e) {
      busyError = (e as { kind: string }).kind;
    }
    const finished = await done;
    const s1 = await status();
    return { before, progress, finished, after: { installed: s1.groups[0].installed, downloading: s1.downloading, subject: await subject() }, busyError };
  }, cancelAfter);
}

test.describe("model downloads (mock, IPC v12)", () => {
  test("download installs the models and AI masks become available", async ({ page }) => {
    await openApp(page, 50, "&models=missing");
    await page.evaluate(() => (window.__mockModelDelay = 2));
    const r = await download(page);
    expect(r.before).toEqual({ installed: false, subject: false });
    expect(r.busyError).toBe("invalid_argument");
    expect(r.finished).toEqual({ group: "segmentation", ok: true, cancelled: false, error: null });
    expect(r.progress.length).toBe(60);
    const last = r.progress[r.progress.length - 1];
    expect(last.fileIndex).toBe(5);
    expect(last.bytesDone).toBe(last.bytesTotal);
    expect(r.after).toEqual({ installed: true, downloading: null, subject: true });
  });

  test("cancel and failure leave the models missing", async ({ page }) => {
    await openApp(page, 50, "&models=missing");
    await page.evaluate(() => (window.__mockModelDelay = 2));
    const c = await download(page, 3);
    expect(c.finished).toEqual({ group: "segmentation", ok: false, cancelled: true, error: "model download cancelled" });
    expect(c.after).toEqual({ installed: false, downloading: null, subject: false });

    await page.evaluate(() => (window.__mockModelFail = "yolox_m.onnx: SHA-256 mismatch (mock)"));
    const f = await download(page);
    expect(f.finished).toEqual({ group: "segmentation", ok: false, cancelled: false, error: "yolox_m.onnx: SHA-256 mismatch (mock)" });
    expect(f.after.installed).toBe(false);
  });

  test("installed by default", async ({ page }) => {
    await openApp(page, 50);
    const s = await page.evaluate(async () => {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const { commands, unwrap }: any = await import("/src/ipc/index.ts");
      return unwrap(commands.modelDownloadsStatus());
    });
    expect(s.downloading).toBeNull();
    expect(s.groups).toHaveLength(1);
    expect(s.groups[0]).toMatchObject({ id: "segmentation", installed: true, bytesTotal: 559_081_960 });
    expect(s.groups[0].files).toHaveLength(6);
  });
});
