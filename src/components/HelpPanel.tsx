// Help & FAQ: searchable sections (F1, the Help button, "?" links next to controls).
import { useEffect, useMemo, useRef, useState } from "react";
import { Keyboard, X } from "lucide-react";
import { Dialog } from "./Dialog";
import { closeHelp, useHelpState } from "../lib/helpStore";
import { HELP, searchHelp, type Block, type HelpEntry } from "../lib/helpContent";

function Blocks({ blocks, onShortcuts }: { blocks: Block[]; onShortcuts: () => void }) {
  return (
    <>
      {blocks.map((b, i) => {
        if ("p" in b) return <p key={i} className="mb-3 text-neutral-300">{b.p}</p>;
        if ("ul" in b)
          return (
            <ul key={i} className="mb-3 list-disc space-y-1 pl-5 text-neutral-300">
              {b.ul.map((t, j) => (
                <li key={j}>{t}</li>
              ))}
            </ul>
          );
        if ("legend" in b)
          return (
            <section key={i} className="mb-4" data-testid="help-legend">
              <h4 className="mb-1.5 text-xs font-semibold uppercase tracking-wide text-neutral-400">{b.title}</h4>
              <ul className="space-y-1.5">
                {b.legend.map((l) => (
                  <li key={l.label} className="flex items-center gap-3" data-testid={`help-legend-${l.label}`}>
                    <span className="flex w-20 shrink-0 items-center justify-center rounded bg-neutral-800 py-1">{l.icon}</span>
                    <span className="text-neutral-300">
                      <span className="font-medium text-neutral-100">{l.label}</span>. <span data-testid="help-legend-text">{l.text}</span>
                    </span>
                  </li>
                ))}
              </ul>
            </section>
          );
        return (
          <button key={i} onClick={onShortcuts} data-testid="help-open-shortcuts" className="mb-3 flex items-center gap-1.5 rounded-md bg-neutral-800 px-3 py-1.5 text-neutral-100 hover:bg-neutral-700">
            <Keyboard className="size-4" /> Open keyboard shortcuts
          </button>
        );
      })}
    </>
  );
}

export function HelpPanel({ onShortcuts }: { onShortcuts: () => void }) {
  const { open, entry } = useHelpState();
  if (!open) return null;
  return <Panel key={entry ?? ""} initial={entry} onShortcuts={onShortcuts} />;
}

function Panel({ initial, onShortcuts }: { initial: string | null; onShortcuts: () => void }) {
  const [query, setQuery] = useState("");
  const [picked, setPicked] = useState<string | null>(initial);
  const results = useMemo(() => searchHelp(query), [query]);
  const current: HelpEntry | undefined = results.find((e) => e.id === picked) ?? results[0];
  const body = useRef<HTMLDivElement>(null);
  useEffect(() => {
    body.current?.scrollTo({ top: 0 });
  }, [current?.id]);

  return (
    <Dialog
      label="Help"
      testid="help-panel"
      className="flex h-[min(720px,88vh)] w-[min(900px,96vw)] flex-col rounded-xl border border-neutral-700 bg-neutral-900 shadow-2xl"
      onCancel={() => (query ? setQuery("") : closeHelp())}
      backdropClose
    >
      <header className="flex items-center justify-between gap-3 border-b border-neutral-800 px-4 py-2.5">
        <h2 className="shrink-0 font-semibold">Help &amp; FAQ</h2>
        <input
          data-autofocus
          type="search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search help"
          aria-label="Search help"
          data-testid="help-search"
          className="h-7 w-64 rounded-md border border-neutral-700 bg-neutral-950 px-2 text-xs text-neutral-100 placeholder:text-neutral-400 focus:border-sky-500 focus:outline-none"
        />
        <button onClick={closeHelp} aria-label="Close" data-testid="help-close" className="shrink-0 text-neutral-400 hover:text-neutral-100">
          <X className="size-4" />
        </button>
      </header>
      <div className="flex min-h-0 flex-1">
        <nav className="w-56 shrink-0 overflow-y-auto border-r border-neutral-800 p-2 text-sm" aria-label="Help topics" data-testid="help-nav">
          {results.length === 0 && <p className="px-2 py-1 text-neutral-400" data-testid="help-empty">No help matches "{query}".</p>}
          {results.map((e) => (
            <button
              key={e.id}
              data-testid={`help-nav-${e.id}`}
              aria-current={current?.id === e.id}
              onClick={() => setPicked(e.id)}
              className={`block w-full rounded px-2 py-1.5 text-left ${current?.id === e.id ? "bg-sky-800 text-sky-50" : "text-neutral-300 hover:bg-neutral-800"}`}
            >
              {e.title}
            </button>
          ))}
        </nav>
        <div ref={body} className="min-w-0 flex-1 overflow-y-auto p-5 text-sm" data-testid="help-body">
          {current ? (
            <article data-testid={`help-entry-${current.id}`} data-entry={current.id}>
              <h3 className="mb-3 text-base font-semibold text-neutral-100">{current.title}</h3>
              <Blocks
                blocks={current.blocks}
                onShortcuts={() => {
                  closeHelp();
                  onShortcuts();
                }}
              />
            </article>
          ) : (
            <p className="text-neutral-400">Try another word, such as "save", "flag" or "keepers".</p>
          )}
        </div>
      </div>
    </Dialog>
  );
}

export const HELP_IDS = HELP.map((e) => e.id);
