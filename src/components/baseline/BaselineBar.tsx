// Compact bar above Develop while a Baseline edit is being set up (step 3): the preset, how the anchor photo differs from
// its own Auto, and the way on to "Edit the rest".
import { ArrowRight, Layers, Wand2, X } from "lucide-react";
import { describeOffset, useAnchorOffset, type BaselineSession } from "../../hooks/useBaseline";

interface Props {
  projectId: number;
  session: BaselineSession;
  activeId: number | null;
  anchorName: string;
  /** Bumped after every committed edit so the offset is measured again. */
  tick: number;
  onRest: () => void;
  /** The open photo's light := its Auto (`auto_light`), one history entry. */
  onStartFromAuto: () => void;
  onGoAnchor: () => void;
  onClose: () => void;
}

export function BaselineBar({ projectId, session, activeId, anchorName, tick, onRest, onStartFromAuto, onGoAnchor, onClose }: Props) {
  const { anchor, error } = useAnchorOffset(projectId, session.anchorId, session.presetId, tick);
  const text = describeOffset(anchor?.offset);
  const onAnchor = activeId === session.anchorId;
  return (
    <div className="flex h-8 shrink-0 items-center gap-2 border-b border-emerald-900 bg-emerald-950/70 px-3 text-xs" data-testid="baseline-bar" role="region" aria-label="Baseline edit">
      <Layers className="size-3.5 shrink-0 text-emerald-400" aria-hidden />
      <span className="font-semibold text-emerald-200">Baseline</span>
      <span className="min-w-0 truncate text-neutral-200" data-testid="baseline-bar-text" title="Colours come from the preset and this photo. Light is Auto for every photo, shifted the way you shift this one.">
        Preset: <b data-testid="baseline-bar-preset">{session.presetName ?? "none"}</b> · Your light vs Auto: <b data-testid="baseline-bar-offset">{error ? "could not be measured" : anchor ? text : "measuring…"}</b>
        <span className="text-neutral-400"> · carried to every photo</span>
      </span>
      {!onAnchor && (
        <button className="shrink-0 whitespace-nowrap text-sky-300 hover:underline" data-testid="baseline-bar-anchor" title={`Go back to the anchor photo (${anchorName}), the one you are adjusting for the baseline`} onClick={onGoAnchor}>
          Back to {anchorName}
        </button>
      )}
      <span className="ml-auto flex shrink-0 items-center gap-2">
        <button
          className="flex h-6 items-center gap-1 whitespace-nowrap rounded-md bg-neutral-800 px-2.5 text-neutral-100 hover:bg-neutral-700 disabled:opacity-40"
          data-testid="baseline-bar-auto"
          disabled={!anchor || !onAnchor}
          title={onAnchor ? "Set exposure, contrast, highlights, shadows, whites, blacks and white balance to Auto for this photo. The preset's colours stay" : `Open the anchor photo (${anchorName}) first`}
          onClick={onStartFromAuto}
        >
          <Wand2 className="size-3.5" aria-hidden /> Start from Auto
        </button>
        <button
          className="flex h-6 items-center gap-1 whitespace-nowrap rounded-md bg-emerald-700 px-3 font-medium text-white hover:bg-emerald-600"
          data-testid="baseline-bar-rest"
          title="Done adjusting the anchor. Choose which photos to edit and see a before / after preview (Cmd+Alt+B)"
          onClick={onRest}
        >
          Edit the rest <ArrowRight className="size-3.5" />
        </button>
        <button className="rounded p-0.5 text-neutral-400 hover:bg-emerald-900 hover:text-white" data-testid="baseline-bar-close" aria-label="Leave the baseline edit" title="Leave the baseline edit setup. Nothing is lost; the anchor photo keeps its edit" onClick={onClose}>
          <X className="size-3.5" />
        </button>
      </span>
    </div>
  );
}
