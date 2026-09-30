import type { ReactNode } from "react";
import { ChevronDown, ChevronRight, RotateCcw } from "lucide-react";
import type { CompleteAdjustments } from "../../ipc";
import type { Editor } from "../../hooks/useEditor";
import { soloSection, toggleSection, useSectionOpen } from "../../lib/sections";
import { Slider } from "./Slider";

/** Collapsible panel section. State is remembered (localStorage); Alt-click solos the section. */
export function Section({
  id,
  title,
  onReset,
  children,
  badge,
  dirty,
}: {
  id: string;
  title: string;
  onReset?: () => void;
  children: ReactNode;
  badge?: ReactNode;
  /** Some field of the section differs from its default: shows a small dot after the title. */
  dirty?: boolean;
}) {
  const open = useSectionOpen(id);
  return (
    <section className="border-b border-neutral-800 py-2" data-testid={`section-${id}`} data-open={open}>
      <div className="mb-1 flex items-center justify-between">
        <button
          className="flex items-center gap-1 text-xs font-semibold uppercase tracking-wide text-neutral-300"
          aria-expanded={open}
          title="Click to collapse or expand, Alt-click to show only this section"
          data-testid={`section-toggle-${id}`}
          onClick={(e) => (e.altKey ? soloSection(id) : toggleSection(id))}
        >
          {open ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
          {title}
          {dirty && <span className="ml-0.5 size-1.5 rounded-full bg-sky-400" title="Changed from the default" data-testid={`section-dot-${id}`} />}
          {badge}
        </button>
        {onReset && (
          <button className="text-neutral-400 hover:text-neutral-200" title={`Reset ${title}`} onClick={onReset} data-testid={`reset-${id}`}>
            <RotateCcw className="size-3.5" />
          </button>
        )}
      </div>
      {open && children}
    </section>
  );
}

export const seg = (on: boolean) => `flex-1 whitespace-nowrap rounded px-1 py-0.5 text-xs ${on ? "bg-sky-800 text-sky-100" : "bg-neutral-800 hover:bg-neutral-700"}`;

interface NumFieldProps {
  editor: Editor;
  id: string;
  label: string;
  /** History label prefix ("Tone Curve" -> "Tone Curve: Shadows"). */
  group?: string;
  min: number;
  max: number;
  step: number;
  digits?: number;
  accent?: string;
  disabled?: boolean;
  display?: (v: number) => string;
  get: (a: CompleteAdjustments) => number;
  set: (a: CompleteAdjustments, v: number) => CompleteAdjustments;
}

/** A slider bound to one numeric adjustment; double-click resets to the format's default. */
export function NumField({ editor, id, label, group, min, max, step, digits = 0, accent, disabled, display, get, set }: NumFieldProps) {
  const name = group ? `${group}: ${label}` : label;
  return (
    <Slider
      id={id}
      label={label}
      value={get(editor.adj)}
      min={min}
      max={max}
      step={step}
      digits={digits}
      defaultValue={get(editor.defaults)}
      accent={accent}
      disabled={disabled}
      display={display}
      onInput={(v) => editor.edit((a) => set(a, v), name)}
      onCommit={editor.commit}
      onReset={() => editor.change((a) => set(a, get(editor.defaults)), name)}
    />
  );
}
