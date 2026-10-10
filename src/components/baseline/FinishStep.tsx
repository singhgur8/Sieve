// Baseline edit, step 5: finish in Lightroom. The sidecars carry everything as standard Lightroom settings; this panel
// makes sure they are written and spells out how Lightroom picks them up.
import { useCallback, useEffect, useState } from "react";
import { CheckCircle2, CloudUpload, Loader2 } from "lucide-react";
import { commands, unwrap, type XmpStatus } from "../../ipc";
import { HelpLink } from "../HelpLink";

interface Props {
  onExport: () => void;
  onPlan: () => void;
  onError: (e: unknown) => void;
}

export function FinishStep({ onExport, onPlan, onError }: Props) {
  const [xmp, setXmp] = useState<XmpStatus | null>(null);
  const [saving, setSaving] = useState(false);
  const poll = useCallback(async () => {
    try {
      setXmp(await unwrap(commands.getXmpStatus()));
    } catch {
      /* the next poll tries again */
    }
  }, []);
  useEffect(() => {
    void poll();
    const t = setInterval(() => void poll(), 1500);
    return () => clearInterval(t);
  }, [poll]);
  const save = async () => {
    setSaving(true);
    try {
      await unwrap(commands.writeXmpAllDirty(null));
      await poll();
    } catch (e) {
      onError(e);
    } finally {
      setSaving(false);
    }
  };
  const dirty = xmp?.dirty ?? 0;
  const failed = xmp?.failed ?? 0;
  const saved = xmp != null && dirty === 0 && failed === 0 && !xmp.running;
  return (
    <div className="min-h-0 flex-1 overflow-y-auto" data-testid="baseline-step-finish">
      <div className="mx-auto max-w-[760px] space-y-4 p-6">
        <h2 className="text-lg font-semibold">Finish in Lightroom</h2>
        <section className="rounded-lg bg-neutral-900 p-4" data-testid="baseline-xmp" data-state={saved ? "saved" : failed > 0 ? "error" : "pending"}>
          <h3 className="mb-1 flex items-center gap-2 text-sm font-semibold">
            <span className="flex size-5 items-center justify-center rounded-full bg-neutral-800 text-xs">1</span>
            Make sure the sidecars are saved
          </h3>
          <p className="flex items-center gap-2 text-sm text-neutral-300" data-testid="baseline-xmp-text">
            {saved ? <CheckCircle2 className="size-4 text-emerald-400" aria-hidden /> : xmp?.running ? <Loader2 className="size-4 animate-spin text-amber-300" aria-hidden /> : <CloudUpload className="size-4 text-amber-400" aria-hidden />}
            {xmp == null ? "Checking…" : failed > 0 ? `${failed} sidecar${failed === 1 ? "" : "s"} could not be written. Check the Saved pill in the top bar.` : saved ? "All edits are saved to the .xmp files next to your RAWs." : xmp.running ? `Saving… ${dirty} left` : `${dirty} photo${dirty === 1 ? "" : "s"} not saved to their sidecars yet.`}
          </p>
          {!saved && failed === 0 && (
            <button type="button" className="mt-2 h-7 rounded-md bg-sky-700 px-3 text-xs font-medium hover:bg-sky-600 disabled:opacity-40" data-testid="baseline-xmp-save" disabled={saving || xmp?.running === true} title="Write every unsaved edit to its XMP sidecar now (same as Save, Cmd+S)" onClick={() => void save()}>
              {saving ? "Saving…" : "Save now"}
            </button>
          )}
          {xmp && !xmp.autoSync && <p className="mt-2 text-xs text-amber-300">Auto-save to sidecars is off, so use Save now before you switch.</p>}
        </section>
        <section className="rounded-lg bg-neutral-900 p-4" data-testid="baseline-lr-steps">
          <h3 className="mb-2 flex items-center gap-2 text-sm font-semibold">
            <span className="flex size-5 items-center justify-center rounded-full bg-neutral-800 text-xs">2</span>
            Open the edit in Lightroom
          </h3>
          <ul className="space-y-2 text-sm text-neutral-300">
            <li data-testid="baseline-lr-existing">
              <b className="text-neutral-100">Photos already in a Lightroom catalog:</b> select them in the Library, then choose <b>Metadata &gt; Read Metadata from Files</b>.
            </li>
            <li data-testid="baseline-lr-new">
              <b className="text-neutral-100">A new import:</b> import the folder as usual. Lightroom reads the sidecars by itself, so the edit is already there.
            </li>
          </ul>
        </section>
        <section className="rounded-lg bg-neutral-900 p-4" data-testid="baseline-lr-note">
          <h3 className="mb-1 flex items-center gap-2 text-sm font-semibold">
            <span className="flex size-5 items-center justify-center rounded-full bg-neutral-800 text-xs">3</span>
            Finish the edit there
          </h3>
          <p className="text-sm text-neutral-300">
            Every photo now has the preset&apos;s look and its own corrected light and white balance, stored as standard Lightroom settings. Crop, straighten and masks are not touched: Sieve leaves what each photo already has.
          </p>
          <HelpLink id="baseline-edit" label="How the baseline edit works" title="Open the Help entry for the baseline edit" className="mt-2" />
        </section>
        <div className="flex items-center gap-2">
          <button type="button" className="h-8 rounded-md bg-neutral-800 px-3 text-xs hover:bg-neutral-700" data-testid="baseline-finish-plan" title="Back to the Edit plan" onClick={onPlan}>
            Back to the plan
          </button>
          <button type="button" className="ml-auto h-8 rounded-md bg-emerald-700 px-4 text-xs font-medium text-white hover:bg-emerald-600" data-testid="baseline-finish-export" title="Go to the Export step to render client deliverables from Sieve instead" onClick={onExport}>
            Or export from Sieve
          </button>
        </div>
      </div>
    </div>
  );
}
