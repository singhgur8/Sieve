import { useEffect, useMemo, useRef, useState } from "react";
import { X } from "lucide-react";
import { keymapGroups, type Mode } from "../lib/keymap";
import { Dialog } from "./Dialog";

/** Shortcut reference generated from the keymap definition. */
const FIRST: Record<"develop" | "library", string[]> = {
  develop: ["Develop", "Masks", "Culling"],
  library: ["Culling", "Navigate", "View"],
};
/** Edit step (Plan or Develop): the workflow chords lead. */
const EDIT_FIRST: Record<"develop" | "library", string[]> = {
  develop: ["Workflow", "Develop", "Masks", "Culling"],
  library: ["Workflow", "Develop", "Culling"],
};

/** Groups ordered so the ones for the current module come first. */
function ordered(mode: Mode, editStep: boolean) {
  const first = (editStep ? EDIT_FIRST : FIRST)[mode === "develop" ? "develop" : "library"];
  const all = keymapGroups();
  return [...first.map((n) => all.find((g) => g.group === n)).filter((g): g is (typeof all)[number] => !!g), ...all.filter((g) => !first.includes(g.group))];
}

export function CheatSheet({ onClose, mode = "grid", editStep = false }: { onClose: () => void; mode?: Mode; editStep?: boolean }) {
  const [filter, setFilter] = useState("");
  const [more, setMore] = useState(false);
  const cols = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const q = filter.trim().toLowerCase();
  const groups = useMemo(() => {
    const all = ordered(mode, editStep);
    if (!q) return all;
    return all
      .map((g) => ({ ...g, items: g.items.filter((d) => `${d.label} ${d.where} ${g.group} ${d.display.join(" ")}`.toLowerCase().includes(q)) }))
      .filter((g) => g.items.length > 0);
  }, [mode, editStep, q]);

  const measure = () => {
    const el = cols.current;
    if (el) setMore(el.scrollTop + el.clientHeight < el.scrollHeight - 4);
  };
  useEffect(measure, [groups]);

  /** Scroll keys work from the filter field and the content alike (Space only while the filter is empty). */
  const scrollKey = (e: React.KeyboardEvent) => {
    const el = cols.current;
    if (!el || e.metaKey || e.ctrlKey || e.altKey) return;
    const page = el.clientHeight * 0.9;
    const inField = e.target === input.current;
    const to = (top: number) => {
      e.preventDefault();
      el.scrollTo({ top });
    };
    if (e.key === "PageDown" || (e.key === " " && !e.shiftKey && (!inField || !filter))) return to(el.scrollTop + page);
    if (e.key === "PageUp" || (e.key === " " && e.shiftKey && (!inField || !filter))) return to(el.scrollTop - page);
    if (e.key === "ArrowDown") return to(el.scrollTop + 40);
    if (e.key === "ArrowUp") return to(el.scrollTop - 40);
    if (!inField && e.key === "Home") return to(0);
    if (!inField && e.key === "End") return to(el.scrollHeight);
    // Type to filter: a printable key on the content moves into the field.
    if (!inField && e.key.length === 1 && e.key !== " ") input.current?.focus();
  };

  return (
    <Dialog
      label="Keyboard shortcuts"
      testid="cheat-sheet"
      className="flex max-h-[86vh] w-[min(1560px,96vw)] flex-col rounded-xl border border-neutral-700 bg-neutral-900 shadow-2xl"
      onCancel={() => (filter ? setFilter("") : onClose())}
      backdropClose
    >
      <header className="flex items-center justify-between gap-3 border-b border-neutral-800 px-4 py-2.5">
        <h2 className="shrink-0 font-semibold">
          Keyboard shortcuts <span className="ml-2 text-xs font-normal text-neutral-400" data-testid="cheat-subtitle">Showing {editStep ? "Edit step" : mode === "develop" ? "Develop" : "Library"} first</span>
        </h2>
        <input
          ref={input}
          data-autofocus
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          onKeyDown={scrollKey}
          placeholder="Type to filter"
          aria-label="Filter shortcuts"
          data-testid="cheat-filter"
          className="h-7 w-56 rounded-md border border-neutral-700 bg-neutral-950 px-2 text-xs text-neutral-100 placeholder:text-neutral-400 focus:border-sky-500 focus:outline-none"
        />
        <button onClick={onClose} aria-label="Close" data-testid="cheat-close" className="shrink-0 text-neutral-400 hover:text-neutral-100">
          <X className="size-4" />
        </button>
      </header>
      <div className="relative flex min-h-0 flex-1 flex-col">
        <div
          ref={cols}
          tabIndex={-1}
          onKeyDown={scrollKey}
          onScroll={measure}
          className="min-h-0 flex-1 columns-2 gap-x-8 overflow-y-auto p-4 text-sm outline-none min-[1400px]:columns-3 min-[1700px]:columns-4"
          data-testid="cheat-columns"
        >
          {groups.length === 0 && <p className="text-neutral-400">No shortcut matches "{filter}".</p>}
          {groups.map((g) => (
            <section key={g.group} className="mb-4 break-inside-avoid" data-testid={`cheat-group-${g.group}`}>
              <h3 className="mb-1 text-xs font-semibold uppercase tracking-wide text-neutral-400">{g.group}</h3>
              <ul className="space-y-0.5">
                {g.items.map((d) => (
                  <li key={d.id} className="flex items-baseline gap-3" data-testid={`cheat-${d.id}`}>
                    <span className="flex w-32 shrink-0 flex-wrap gap-1">
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
        {more && <div className="pointer-events-none absolute inset-x-0 bottom-0 h-6 rounded-b-xl bg-gradient-to-t from-neutral-900 to-transparent" data-testid="cheat-fade" />}
      </div>
    </Dialog>
  );
}
