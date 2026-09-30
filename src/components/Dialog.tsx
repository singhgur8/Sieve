// Accessible modal dialog: overlay + role=dialog, focus moved in on mount, Tab trapped, Esc / Enter via the modal stack.
import { useRef, type ReactNode } from "react";
import { useModalLayer } from "../lib/modal";

interface Props {
  label: string;
  testid?: string;
  /** Classes for the panel (width, padding...). */
  className?: string;
  /** Classes for the overlay (z-index, backdrop). */
  overlayClass?: string;
  onCancel: () => void;
  onConfirm?: () => void;
  canConfirm?: () => boolean;
  requireMod?: boolean;
  /** Close on a click on the backdrop. */
  backdropClose?: boolean;
  children: ReactNode;
}

export function Dialog({ label, testid, className = "", overlayClass = "z-50 bg-black/60", onCancel, onConfirm, canConfirm, requireMod, backdropClose, children }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  useModalLayer({ ref, onCancel, onConfirm, canConfirm, requireMod });
  return (
    <div
      className={`fixed inset-0 flex items-center justify-center p-4 ${overlayClass}`}
      data-testid={testid}
      onMouseDown={backdropClose ? (e) => e.target === e.currentTarget && onCancel() : undefined}
    >
      <div ref={ref} role="dialog" aria-modal="true" aria-label={label} className={className}>
        {children}
      </div>
    </div>
  );
}
