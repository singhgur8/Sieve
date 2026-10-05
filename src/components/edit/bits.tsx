// Small shared pieces of the Edit step: scene status icons and copy, the style-aware "Auto edit (my style)" button, a thumbnail.
import { useEffect, useState } from "react";
import { CheckCircle2, Circle, CircleDot, Loader2, RefreshCw, Sparkles, Wand2 } from "lucide-react";
import { type RawImageEntry, type StyleModelStatus } from "../../ipc";
import { styleGate, type SceneRow, type SceneUi } from "../../hooks/useWorkflow";
import { thumbSrc } from "../../lib/entryImage";

export const STATUS_ICON: Record<SceneUi, { icon: typeof Circle; cls: string }> = {
  todo: { icon: Circle, cls: "text-neutral-400" },
  auto: { icon: Wand2, cls: "text-amber-400" },
  edited: { icon: CircleDot, cls: "text-sky-400" },
  applied: { icon: CheckCircle2, cls: "text-emerald-400" },
  stale: { icon: RefreshCw, cls: "text-amber-400" },
  reset: { icon: RefreshCw, cls: "text-amber-400" },
};

export function StatusIcon({ ui, className = "size-5" }: { ui: SceneUi; className?: string }) {
  const m = STATUS_ICON[ui];
  return <m.icon className={`${className} shrink-0 ${m.cls}`} aria-hidden />;
}

/** One status line per scene state, with the colour the spec gives it. */
export function statusLine(r: SceneRow): { text: string; cls: string; extra?: string } {
  const n = r.targets;
  if (r.skipped) return { text: "Skipped, no edit copied", cls: "text-neutral-400" };
  switch (r.ui) {
    case "todo":
      return { text: "To do: edit this photo", cls: "text-neutral-400" };
    case "auto":
      return { text: "Auto edited (my style), review it", cls: "text-amber-300" };
    case "edited":
      return { text: n > 0 ? `Edited, ready to apply to ${n}` : "Edited (no other keepers in this scene)", cls: "text-sky-300" };
    case "stale":
      return { text: "Changed since it was applied", cls: "text-amber-300" };
    case "reset":
      return { text: `Representative reset · ${r.applied} photo${r.applied === 1 ? "" : "s"} keep the earlier look`, cls: "text-amber-300" };
    case "applied": {
      const extra = `${r.review.length > 0 ? ` · ${r.review.length} need a look` : ""}${r.unapplied.length > 0 ? ` · ${r.unapplied.length} new keeper${r.unapplied.length === 1 ? "" : "s"} not edited` : ""}`;
      return { text: `Applied to ${r.applied > 0 ? r.applied : n}`, cls: "text-emerald-300", extra: extra || undefined };
    }
  }
}

export type ApplyFix = "open" | "include";

/** Why Apply to scene is unavailable for this scene (null = it can run). `soft`: informational only, the button still works. */
export function applyBlock(r: SceneRow | undefined): { reason: string; fix?: ApplyFix; soft?: boolean } | null {
  if (!r) return { reason: "This photo is not in a scene" };
  if (r.skipped) return { reason: "This scene is skipped", fix: "include" };
  if (r.ui === "todo") return { reason: "Edit the representative first", fix: "open" };
  if (r.ui === "reset") return { reason: "Edit the representative first (it was reset)", fix: "open" };
  if (r.targets === 0) return { reason: "No keepers in this scene" };
  if (r.ui === "applied" && r.unapplied.length === 0) return { reason: "Already applied", soft: true };
  return null;
}

export const FIX_LABEL: Record<ApplyFix, string> = { open: "Open representative", include: "Include scene" };

/** The reason a scene cannot be applied, in place, with its one-click fix. */
export function ApplyWhy({ block, onFix, testid, className = "" }: { block: { reason: string; fix?: ApplyFix } | null; onFix?: (fix: ApplyFix) => void; testid: string; className?: string }) {
  if (!block) return null;
  return (
    <span className={`flex min-w-0 items-center gap-1.5 text-[11px] text-amber-300 ${className}`} data-testid={testid}>
      <span className="truncate">{block.reason}</span>
      {block.fix && onFix && (
        <button className="shrink-0 whitespace-nowrap text-sky-300 hover:underline" data-testid={`${testid}-fix`} onClick={() => onFix(block.fix!)}>
          {FIX_LABEL[block.fix]}
        </button>
      )}
    </span>
  );
}

interface AutoProps {
  style: StyleModelStatus | null;
  onClick: () => void;
  label?: string;
  testid?: string;
  className?: string;
  disabled?: boolean;
}

/** "Auto edit (my style)" in every state of the style model (spec 4.5). */
export function AutoEditButton({ style, onClick, label = "Auto edit (my style)", testid, className = "", disabled }: AutoProps) {
  const g = styleGate(style);
  const training = g.kind === "training";
  const off = g.kind === "insufficient";
  const pct = style?.progress != null ? Math.round(style.progress * 100) : 0;
  const need = style ? Math.max(0, style.minExamples - style.availableExamples) : 0;
  return (
    <>
    <button
      onClick={onClick}
      disabled={disabled || training || off}
      title={g.tip}
      data-testid={testid}
      data-style-state={g.kind}
      className={`flex h-7 items-center gap-1.5 whitespace-nowrap rounded-md bg-neutral-800 px-2.5 text-xs font-medium text-amber-200 hover:bg-neutral-700 disabled:cursor-not-allowed disabled:opacity-50 ${className}`}
    >
      {training ? <Loader2 className="size-3.5 animate-spin" /> : <Sparkles className="size-3.5 text-amber-400" />}
      {training ? `Learning your style… ${pct}%` : label}
    </button>
    {off && style && (
      <span className="whitespace-nowrap text-[11px] text-amber-300" data-testid={testid ? `${testid}-why` : undefined}>
        {need > 0 ? `Edit ${need} more photo${need === 1 ? "" : "s"} first` : "Not trained yet"}
      </span>
    )}
    </>
  );
}

/** Rendered thumbnail of a photo (cached row), or an empty dark block while it loads. */
export function Thumb({ entry, version = 0, className = "" }: { entry: RawImageEntry | undefined; version?: number; className?: string }) {
  const t = entry?.thumbnail;
  return (
    <div className={`overflow-hidden bg-neutral-800 ${className}`}>
      {t?.status === "ready" && <img src={thumbSrc(entry, version)!} alt="" draggable={false} className="size-full object-cover" />}
    </div>
  );
}

export function useWide(query = "(min-width: 1600px)") {
  const [on, setOn] = useState(() => window.matchMedia(query).matches);
  useEffect(() => {
    const m = window.matchMedia(query);
    const f = () => setOn(m.matches);
    m.addEventListener("change", f);
    return () => m.removeEventListener("change", f);
  }, [query]);
  return on;
}
