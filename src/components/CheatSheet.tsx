import { X } from "lucide-react";
import { keymapGroups } from "../lib/keymap";
import { Dialog } from "./Dialog";

/** Shortcut reference generated from the keymap definition. */
export function CheatSheet({ onClose }: { onClose: () => void }) {
  return (
    <Dialog
      label="Keyboard shortcuts"
      testid="cheat-sheet"
      className="flex max-h-[86vh] w-[min(980px,94vw)] flex-col rounded-xl border border-neutral-700 bg-neutral-900 shadow-2xl"
      onCancel={onClose}
      onConfirm={onClose}
      backdropClose
    >
      <header className="flex items-center justify-between border-b border-neutral-800 px-4 py-2.5">
        <h2 className="font-semibold">Keyboard shortcuts</h2>
        <button onClick={onClose} aria-label="Close" data-testid="cheat-close" className="text-neutral-400 hover:text-neutral-100">
          <X className="size-4" />
        </button>
      </header>
      <div className="grid min-h-0 flex-1 grid-cols-1 gap-x-8 gap-y-4 overflow-y-auto p-4 text-sm md:grid-cols-2">
        {keymapGroups().map((g) => (
          <section key={g.group} data-testid={`cheat-group-${g.group}`}>
            <h3 className="mb-1 text-xs font-semibold uppercase tracking-wide text-neutral-400">{g.group}</h3>
            <ul className="space-y-1">
              {g.items.map((d) => (
                <li key={d.id} className="flex items-baseline gap-3" data-testid={`cheat-${d.id}`}>
                  <span className="flex w-44 shrink-0 flex-wrap gap-1">
                    {d.display.map((k) => (
                      <kbd key={k} className="rounded border border-neutral-600 bg-neutral-800 px-1.5 py-0.5 font-mono text-[11px] text-neutral-100">
                        {k}
                      </kbd>
                    ))}
                  </span>
                  <span className="text-neutral-300">
                    {d.label}
                    <span className="ml-1.5 text-xs text-neutral-400">{d.where}</span>
                  </span>
                </li>
              ))}
            </ul>
          </section>
        ))}
      </div>
    </Dialog>
  );
}
