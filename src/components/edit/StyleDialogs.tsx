// Dialogs of "Auto edit (my style)": learn the style first, and confirm replacing a hand edit.
import type { Workflow } from "../../hooks/useWorkflow";
import { Dialog } from "../Dialog";

const panel = "w-[440px] max-w-full rounded-lg border border-neutral-700 bg-neutral-900 p-4 shadow-xl";
const ghost = "rounded-md bg-neutral-800 px-3 py-1 text-sm hover:bg-neutral-700";

export function StyleDialogs({ wf, fileName }: { wf: Workflow; fileName: (id: number) => string }) {
  const learn = wf.learnFor;
  const replace = wf.replaceFor;
  const s = wf.style;
  const edited = replace && wf.plan ? replace.filter((id) => wf.plan!.scenes.find((x) => x.sceneId === id)?.edited) : [];
  const firstRep = edited.length > 0 ? wf.plan?.scenes.find((x) => x.sceneId === edited[0])?.representativeId : undefined;
  return (
    <>
      {learn && (
        <Dialog
          label="Learn your style"
          testid="learn-style-dialog"
          className={panel}
          onCancel={() => wf.setLearnFor(null)}
          onConfirm={() => {
            wf.setLearnFor(null);
            void wf.train(learn);
          }}
        >
          <h2 className="mb-1 text-sm font-semibold">Learn your style first?</h2>
          <p className="mb-2 text-xs text-neutral-300">
            Sieve learns from the {s?.availableExamples ?? 0} photos you have edited in this catalog, including Lightroom edits read from XMP. It takes about a minute and stays on this Mac.
          </p>
          {s?.state === "failed" && s.error && <p className="mb-2 text-xs text-red-300">The last attempt failed: {s.error}</p>}
          <div className="flex justify-end gap-2">
            <button className={ghost} onClick={() => wf.setLearnFor(null)} data-testid="learn-cancel">
              Cancel
            </button>
            <button
              className="rounded-md bg-sky-700 px-3 py-1 text-sm hover:bg-sky-600"
              data-testid="learn-confirm"
              onClick={() => {
                wf.setLearnFor(null);
                void wf.train(learn);
              }}
            >
              Learn and auto edit
            </button>
          </div>
        </Dialog>
      )}
      {replace && edited.length > 0 && (
        <Dialog
          label="Replace your edit"
          testid="replace-edit-dialog"
          className={panel}
          onCancel={() => wf.setReplaceFor(null)}
          onConfirm={() => {
            wf.setReplaceFor(null);
            void wf.runAuto(replace);
          }}
        >
          <h2 className="mb-1 text-sm font-semibold">Replace your edit?</h2>
          <p className="mb-4 text-xs text-neutral-300">
            {edited.length === 1 && firstRep != null ? `Replace your edit on ${fileName(firstRep)} with an auto edit?` : `Replace your edit on ${edited.length} representatives with an auto edit?`} You can undo it.
          </p>
          <div className="flex justify-end gap-2">
            <button className={ghost} onClick={() => wf.setReplaceFor(null)} data-testid="replace-cancel">
              Cancel
            </button>
            <button
              className="rounded-md bg-sky-700 px-3 py-1 text-sm hover:bg-sky-600"
              data-testid="replace-confirm"
              onClick={() => {
                wf.setReplaceFor(null);
                void wf.runAuto(replace);
              }}
            >
              Replace
            </button>
          </div>
        </Dialog>
      )}
    </>
  );
}
