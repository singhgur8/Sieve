// Small popover menu, rendered in a portal on <body> with fixed positioning so no scroll container or overflow-hidden
// panel can crop it; it flips above the trigger / clamps to the window when there is no room below.
// Registers as a modal layer so shortcuts stay inert and Esc closes only the menu.
import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { useModalLayer } from "../lib/modal";

interface Props {
  /** Trigger content; rendered inside the trigger button. */
  trigger: ReactNode;
  triggerClass?: string;
  triggerTestId?: string;
  title?: string;
  align?: "left" | "right";
  disabled?: boolean;
  /** Menu body. Call `close()` from item handlers. */
  children: (close: () => void) => ReactNode;
}

export function Menu({ trigger, triggerClass = "", triggerTestId, title, align = "left", disabled, children }: Props) {
  const [open, setOpen] = useState(false);
  const btn = useRef<HTMLButtonElement>(null);
  return (
    <div className="relative">
      <button
        ref={btn}
        type="button"
        className={triggerClass}
        data-testid={triggerTestId}
        title={title}
        disabled={disabled}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        {trigger}
      </button>
      {open && (
        <Popover anchor={btn} align={align} onClose={() => setOpen(false)}>
          {children(() => setOpen(false))}
        </Popover>
      )}
    </div>
  );
}

const MARGIN = 8;

function Popover({ anchor, align, onClose, children }: { anchor: React.RefObject<HTMLButtonElement | null>; align: "left" | "right"; onClose: () => void; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number; maxHeight: number } | null>(null);
  useModalLayer({ ref, onCancel: onClose, trap: false, focusFirst: false });
  useLayoutEffect(() => {
    const place = () => {
      const a = anchor.current?.getBoundingClientRect();
      const m = ref.current;
      if (!a || !m) return;
      const w = m.offsetWidth;
      const h = m.scrollHeight;
      const vw = window.innerWidth;
      const vh = window.innerHeight;
      const below = vh - a.bottom - MARGIN - 4;
      const above = a.top - MARGIN - 4;
      const up = h > below && above > below;
      const maxHeight = Math.max(80, up ? above : below);
      const height = Math.min(h, maxHeight);
      const left = Math.min(Math.max(MARGIN, align === "right" ? a.right - w : a.left), Math.max(MARGIN, vw - w - MARGIN));
      const top = up ? a.top - 4 - height : a.bottom + 4;
      setPos({ left, top, maxHeight });
    };
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [anchor, align]);
  return createPortal(
    <>
      <div className="fixed inset-0 z-[60]" onMouseDown={onClose} data-testid="menu-backdrop" />
      <div
        ref={ref}
        role="menu"
        data-placed={pos ? "true" : "false"}
        className="fixed z-[61] min-w-52 overflow-y-auto rounded-md border border-neutral-700 bg-neutral-900 py-1 text-sm shadow-xl"
        style={{ left: pos?.left ?? 0, top: pos?.top ?? 0, maxHeight: pos?.maxHeight, visibility: pos ? "visible" : "hidden" }}
      >
        {children}
      </div>
    </>,
    document.body,
  );
}

export const menuItem = "flex w-full items-center gap-2 px-3 py-1.5 text-left text-neutral-200 hover:bg-neutral-800 disabled:opacity-40 disabled:hover:bg-transparent";
