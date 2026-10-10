// Baseline edit, step 5: finish in Lightroom. The sidecars carry everything as standard Lightroom settings; this panel
// makes sure they are written and spells out how Lightroom picks them up.
import { useCallback, useEffect, useState } from "react";
import { AlertTriangle, CheckCircle2, CloudUpload, Loader2 } from "lucide-react";
import { commands, unwrap, type ParametricAdjustments, type XmpStatus } from "../../ipc";
import { HelpLink } from "../HelpLink";

interface Props {
  /** The project's folders (where the .xmp sidecars are). */
  folders: string[];
  /** The anchor: its LUT / imported profile do not travel to Lightroom. */
  anchorId: number | null;
  onExport: () => void;
  onPlan: () => void;
  onError: (e: unknown) => void;
}

export function FinishStep({ folders, anchorId, onExport, onPlan, onError }: Props) {
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
  const [anchorAdj, setAnchorAdj] = useState<ParametricAdjustments | null>(null);
  useEffect(() => {
    if (anchorId == null) return;
    let dead = false;
    unwrap(commands.getAdjustments(anchorId))
      .then((a) => !dead && setAnchorAdj(a))
      .catch(() => {});
    return () => {
      dead = true;
    };
  }, [anchorId]);
  const lutName = anchorAdj?.lut?.id ?? null;
  const camProfile = anchorAdj?.profile?.cameraProfile ?? null;
  const lookName = anchorAdj?.profile?.look?.name ?? null;
  const importedProfile = camProfile && !/^(Adobe|Camera)\b/i.test(camProfile) ? camProfile : lookName && !/^Adobe\b/i.test(lookName) ? lookName : null;
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
          {!saved && failed === 0 && <span className="ml-2 text-xs text-neutral-400">(Save, Cmd+S)</span>}
          {xmp && !xmp.autoSync && <p className="mt-2 text-xs text-amber-300">Auto-save to sidecars is off, so use Save now before you switch.</p>}
        </section>
        <section className="rounded-lg bg-neutral-900 p-4" data-testid="baseline-lr-steps">
          <h3 className="mb-2 flex items-center gap-2 text-sm font-semibold">
            <span className="flex size-5 items-center justify-center rounded-full bg-neutral-800 text-xs">2</span>
            Open the edit in Lightroom
          </h3>
          {folders.length > 0 && (
            <div className="mb-2 space-y-1 text-xs text-neutral-300" data-testid="baseline-folder">
              <div>{folders.length === 1 ? "The sidecars are in:" : "The sidecars are in these folders:"}</div>
              {folders.map((folder, i) => {
                const sfx = i === 0 ? "" : `-${i}`;
                return (
                  <p key={folder} className="flex flex-wrap items-center gap-2" data-testid={`baseline-folder-row${sfx}`}>
                    <code className="rounded bg-neutral-950 px-1.5 py-0.5 font-mono text-neutral-100" data-testid={`baseline-folder-path${sfx}`}>
                      {folder}
                    </code>
                    <button type="button" className="rounded bg-neutral-800 px-2 py-0.5 hover:bg-neutral-700" data-testid={`baseline-folder-reveal${sfx}`} title="Show the folder in Finder" onClick={() => void unwrap(commands.revealInFinder(folder)).catch(onError)}>
                      Reveal in Finder
                    </button>
                    <button type="button" className="rounded bg-neutral-800 px-2 py-0.5 hover:bg-neutral-700" data-testid={`baseline-folder-copy${sfx}`} title="Copy the folder path" onClick={() => void navigator.clipboard?.writeText(folder).catch(() => undefined)}>
                      Copy path
                    </button>
                  </p>
                );
              })}
            </div>
          )}
          <ul className="space-y-2 text-sm text-neutral-300">
            <li data-testid="baseline-lr-existing">
              <b className="text-neutral-100">Photos already in a Lightroom catalog:</b> in the Library module select them (Cmd+A selects the folder), then choose <b>Metadata &gt; Read Metadata from Files</b> and confirm. This replaces any changes made to them in Lightroom since.
            </li>
            <li data-testid="baseline-lr-new">
              <b className="text-neutral-100">A new import:</b> import the folder as usual, with <b>Apply During Import &gt; Develop Settings: None</b>. Otherwise Lightroom puts its preset over the baseline. Lightroom reads the sidecars by itself.
            </li>
          </ul>
        </section>
        <section className="rounded-lg bg-neutral-900 p-4" data-testid="baseline-lr-note">
          <h3 className="mb-1 flex items-center gap-2 text-sm font-semibold">
            <span className="flex size-5 items-center justify-center rounded-full bg-neutral-800 text-xs">3</span>
            Finish the edit there
          </h3>
          <p className="text-sm text-neutral-300">
            Every edited photo now has the anchor&apos;s look and its own Auto-based light and white balance, as standard Lightroom settings in its .xmp sidecar. Crop, straighten, masks, spot removal and lens corrections are as they were.
          </p>
          {lutName && (
            <p className="mt-2 flex items-start gap-2 text-sm text-amber-300" data-testid="baseline-lut-warning">
              <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden />
              <span>
                Your look uses the LUT <b>{lutName}</b>. Lightroom cannot read Sieve LUTs, so it will look different there. Use a Lightroom profile instead, or export from Sieve.
              </span>
            </p>
          )}
          {importedProfile && (
            <p className="mt-2 flex items-start gap-2 text-sm text-amber-300" data-testid="baseline-profile-warning">
              <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden />
              <span>
                Lightroom needs the profile <b>{importedProfile}</b> installed.
              </span>
            </p>
          )}
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
