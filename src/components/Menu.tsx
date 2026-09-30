// Small popover menu. Registers as a modal layer so shortcuts stay inert and Esc closes only the menu.
import { useRef, useState, type ReactNode } from "react";
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
  return (
    <div className="relative">
      <button
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
        <Popover align={align} onClose={() => setOpen(false)}>
          {children(() => setOpen(false))}
        </Popover>
      )}
    </div>
  );
}

function Popover({ align, onClose, children }: { align: "left" | "right"; onClose: () => void; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  useModalLayer({ ref, onCancel: onClose, trap: false, focusFirst: false });
  return (
    <>
      <div className="fixed inset-0 z-30" onMouseDown={onClose} data-testid="menu-backdrop" />
      <div
        ref={ref}
        role="menu"
        className={`absolute top-full z-40 mt-1 min-w-52 rounded-md border border-neutral-700 bg-neutral-900 py-1 text-sm shadow-xl ${align === "right" ? "right-0" : "left-0"}`}
      >
        {children}
      </div>
    </>
  );
}

export const menuItem = "flex w-full items-center gap-2 px-3 py-1.5 text-left text-neutral-200 hover:bg-neutral-800 disabled:opacity-40 disabled:hover:bg-transparent";
