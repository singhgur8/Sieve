// Small shared pieces of the Edit step: scene status icons and copy, the style-aware "Auto edit (my style)" button, a thumbnail.
import { useEffect, useState } from "react";
import { CheckCircle2, Circle, CircleDot, Loader2, RefreshCw, Sparkles, Wand2 } from "lucide-react";
import { convertFileSrc, type RawImageEntry, type StyleModelStatus } from "../../ipc";
import { styleGate, type SceneRow, type SceneUi } from "../../hooks/useWorkflow";

export const STATUS_ICON: Record<SceneUi, { icon: typeof Circle; cls: string }> = {
  todo: { icon: Circle, cls: "text-neutral-400" },
  auto: { icon: Wand2, cls: "text-amber-400" },
  edited: { icon: CircleDot, cls: "text-sky-400" },
  applied: { icon: CheckCircle2, cls: "text-emerald-400" },
  stale: { icon: RefreshCw, cls: "text-amber-400" },
};

export function StatusIcon({ ui, className = "size-5" }: { ui: SceneUi; className?: string }) {
  const m = STATUS_ICON[ui];
  return <m.icon className={`${className} shrink-0 ${m.cls}`} aria-hidden />;
}

/** One status line per scene state, with the colour the spec gives it. */
export function statusLine(r: SceneRow): { text: string; cls: string; extra?: string } {
  const n = r.targets;
  switch (r.ui) {
    case "todo":
      return { text: "To do: edit this photo", cls: "text-neutral-400" };
    case "auto":
      return { text: "Auto edited (my style), review it", cls: "text-amber-300" };
    case "edited":
      return { text: n > 0 ? `Edited, ready to apply to ${n}` : "Edited (no other keepers in this scene)", cls: "text-sky-300" };
    case "stale":
      return { text: "Changed since it was applied", cls: "text-amber-300" };
    case "applied":
      return { text: `Applied to ${r.applied ?? n}`, cls: "text-emerald-300", extra: r.review.length > 0 ? ` · ${r.review.length} need a look` : undefined };
  }
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
  return (
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
  );
}

/** Rendered thumbnail of a photo (cached row), or an empty dark block while it loads. */
export function Thumb({ entry, version = 0, className = "" }: { entry: RawImageEntry | undefined; version?: number; className?: string }) {
  const t = entry?.thumbnail;
  return (
    <div className={`overflow-hidden bg-neutral-800 ${className}`}>
      {t?.status === "ready" && <img src={`${convertFileSrc(t.path)}?v=${version}`} alt="" draggable={false} className="size-full object-cover" />}
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
