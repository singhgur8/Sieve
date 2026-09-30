// Develop toolbar chip: features of this photo that Sieve cannot render yet (masks, lens corrections, missing profiles...).
import { TriangleAlert } from "lucide-react";
import type { DevelopWarning, DevelopWarningCode } from "../../ipc";
import { Menu } from "../Menu";

const TEXT: Record<DevelopWarningCode, { short: string; long: string }> = {
  profile_unavailable: { short: "Profile not installed", long: "The camera profile (DCP) is not installed; Sieve's built-in colour matrix is used instead." },
  look_unavailable: { short: "Adobe Look not installed", long: "The Adobe Look is not installed on this Mac; the photo renders without it." },
  masks_unsupported: { short: "Masks not supported yet", long: "This photo has local adjustments (masks) from Lightroom. They are kept in the sidecar but not rendered or exported yet." },
  ai_mask_needs_update: { short: "AI masks need update", long: "Some AI masks have not been computed for this photo yet (pasted or synced masks, or the model is not installed); they render as empty until updated." },
  retouch_unsupported: { short: "Retouching not supported yet", long: "Spot removal / healing from Lightroom is kept in the sidecar but not rendered yet." },
  lens_corrections_unsupported: { short: "Lens corrections not supported yet", long: "Lens profile corrections and chromatic aberration removal are not applied yet." },
  transform_unsupported: { short: "Transform not supported yet", long: "Upright / perspective transforms are not applied yet." },
  legacy_process_version: { short: "Older process version", long: "This photo was edited with an older Lightroom process version; the rendering may differ slightly." },
  source_color_assumed: { short: "Colour space assumed", long: "The source has no embedded colour profile; sRGB is assumed." },
};

export const warningText = (w: DevelopWarning) => TEXT[w.code] ?? { short: w.code, long: w.detail ?? w.code };

export function WarningsChip({ warnings, actions }: { warnings: DevelopWarning[]; actions?: Partial<Record<DevelopWarningCode, { label: string; run: () => void }>> }) {
  if (warnings.length === 0) return null;
  const label = warnings.length === 1 ? warningText(warnings[0]).short : `${warnings.length} develop warnings`;
  return (
    <Menu
      trigger={
        <>
          <TriangleAlert className="size-3.5" /> {label}
        </>
      }
      triggerClass="flex items-center gap-1 rounded-full bg-amber-900/70 px-2.5 py-0.5 text-xs text-amber-100 hover:bg-amber-800"
      triggerTestId="warnings-chip"
      title="This photo uses features Sieve cannot render yet"
    >
      {(close) => (
        <div className="w-80 space-y-2 p-3 text-xs" data-testid="warnings-popover">
          {warnings.map((w, i) => (
            <div key={`${w.code}-${i}`} data-testid={`warning-${w.code}`}>
              <div className="font-semibold text-amber-200">{warningText(w).short}</div>
              <div className="text-neutral-300">{warningText(w).long}</div>
              {w.detail && <div className="mt-0.5 text-neutral-400">{w.detail}</div>}
              {actions?.[w.code] && (
                <button
                  className="mt-1 rounded bg-amber-800 px-2 py-0.5 text-amber-50 hover:bg-amber-700"
                  data-testid={`warning-action-${w.code}`}
                  onClick={() => {
                    close();
                    actions[w.code]!.run();
                  }}
                >
                  {actions[w.code]!.label}
                </button>
              )}
            </div>
          ))}
        </div>
      )}
    </Menu>
  );
}
