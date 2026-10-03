import { HelpCircle } from "lucide-react";
import { openHelp } from "../lib/helpStore";

/** Small "?" (or "Learn more") that opens the Help panel on one entry. Safe inside labels and menus. */
export function HelpLink({ id, label, title = "Learn more", className = "" }: { id: string; label?: string; title?: string; className?: string }) {
  return (
    <button
      type="button"
      title={title}
      aria-label={label ?? `${title} (Help)`}
      data-testid={`help-link-${id}`}
      className={`inline-flex shrink-0 items-center gap-1 text-neutral-400 hover:text-sky-300 ${className}`}
      onMouseDown={(e) => e.stopPropagation()}
      onClick={(e) => {
        e.preventDefault();
        e.stopPropagation();
        openHelp(id);
      }}
    >
      <HelpCircle className="size-3.5" />
      {label && <span className="text-xs">{label}</span>}
    </button>
  );
}
