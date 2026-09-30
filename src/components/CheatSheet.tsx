import { X } from "lucide-react";
import { keymapGroups, type Mode } from "../lib/keymap";
import { Dialog } from "./Dialog";

/** Shortcut reference generated from the keymap definition. */
const FIRST: Record<"develop" | "library", string[]> = {
  develop: ["Develop", "Masks", "Culling"],
  library: ["Culling", "Navigate", "View"],
};

/** Groups ordered so the ones for the current module come first. */
function ordered(mode: Mode) {
  const first = FIRST[mode === "develop" ? "develop" : "library"];
  const all = keymapGroups();
  return [...first.map((n) => all.find((g) => g.group === n)).filter((g): g is (typeof all)[number] => !!g), ...all.filter((g) => !first.includes(g.group))];
}

export function CheatSheet({ onClose, mode = "grid" }: { onClose: () => void; mode?: Mode }) {
  return (
    <Dialog
      label="Keyboard shortcuts"
      testid="cheat-sheet"
      className="flex max-h-[86vh] w-[min(1560px,96vw)] flex-col rounded-xl border border-neutral-700 bg-neutral-900 shadow-2xl"
      onCancel={onClose}
      onConfirm={onClose}
      backdropClose
    >
      <header className="flex items-center justify-between border-b border-neutral-800 px-4 py-2.5">
        <h2 className="font-semibold">
          Keyboard shortcuts <span className="ml-2 text-xs font-normal text-neutral-400" data-testid="cheat-subtitle">Showing {mode === "develop" ? "Develop" : "Library"} first</span>
        </h2>
        <button onClick={onClose} aria-label="Close" data-testid="cheat-close" className="text-neutral-400 hover:text-neutral-100">
          <X className="size-4" />
        </button>
      </header>
      <div className="min-h-0 flex-1 columns-2 gap-x-8 overflow-y-auto p-4 text-sm min-[1400px]:columns-3 min-[1700px]:columns-4" data-testid="cheat-columns">
        {ordered(mode).map((g) => (
          <section key={g.group} className="mb-4 break-inside-avoid" data-testid={`cheat-group-${g.group}`}>
            <h3 className="mb-1 text-xs font-semibold uppercase tracking-wide text-neutral-400">{g.group}</h3>
            <ul className="space-y-0.5">
              {g.items.map((d) => (
                <li key={d.id} className="flex items-baseline gap-3" data-testid={`cheat-${d.id}`}>
                  <span className="flex w-28 shrink-0 flex-wrap gap-1">
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
