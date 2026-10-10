import { useEffect, useRef, type ReactNode } from "react";
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
  onHold,
}: {
  id: string;
  title: string;
  onReset?: () => void;
  children: ReactNode;
  badge?: ReactNode;
  /** Some field of the section differs from its default: shows a small dot after the title. */
  dirty?: boolean;
  /** Makes the dot press-and-hold: true while held (the photo is shown without this section's changes), false on release. */
  onHold?: (down: boolean) => void;
}) {
  const open = useSectionOpen(id);
  return (
    <section className="border-b border-neutral-800 py-2" data-testid={`section-${id}`} data-open={open}>
      <div className="mb-1 flex items-center">
        <button
          className="flex items-center gap-1 text-xs font-semibold uppercase tracking-wide text-neutral-300"
          aria-expanded={open}
          title="Click to collapse or expand, Alt-click to show only this section"
          data-testid={`section-toggle-${id}`}
          onClick={(e) => (e.altKey ? soloSection(id) : toggleSection(id))}
        >
          {open ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
          {title}
          {dirty && !onHold && <span className="ml-0.5 size-1.5 rounded-full bg-sky-400" title="Changed from the default" data-testid={`section-dot-${id}`} />}
          {badge}
        </button>
        {dirty && onHold && <HoldDot id={id} title={title} onHold={onHold} />}
        <span className="flex-1" />
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

/** The blue "changed" dot: hold it (pointer, or Space / Enter) to see the photo without this section's changes. */
function HoldDot({ id, title, onHold }: { id: string; title: string; onHold: (down: boolean) => void }) {
  const down = useRef(false);
  const set = (v: boolean) => {
    if (down.current === v) return;
    down.current = v;
    onHold(v);
  };
  // Releasing outside the window, or anything that unmounts the dot, ends the hold.
  useEffect(
    () => () => {
      if (down.current) {
        down.current = false;
        onHold(false);
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );
  return (
    <button
      type="button"
      className="ml-1.5 flex size-4 shrink-0 cursor-pointer items-center justify-center rounded-full hover:bg-neutral-800"
      title={`${title} changed from the default. Press and hold to see the photo without these changes (keyboard: hold Space)`}
      aria-label={`Show without ${title} changes (hold)`}
      data-testid={`section-dot-${id}`}
      onPointerDown={(e) => {
        e.preventDefault();
        e.currentTarget.setPointerCapture(e.pointerId);
        set(true);
      }}
      onPointerUp={() => set(false)}
      onPointerCancel={() => set(false)}
      onLostPointerCapture={() => set(false)}
      onKeyDown={(e) => {
        if (e.key === " " || e.key === "Enter") {
          e.preventDefault();
          e.stopPropagation();
          set(true);
        }
      }}
      onKeyUp={(e) => (e.key === " " || e.key === "Enter") && set(false)}
      onBlur={() => set(false)}
    >
      <span className="size-1.5 rounded-full bg-sky-400" />
    </button>
  );
}

export const seg =(on: boolean) => `flex-1 whitespace-nowrap rounded px-1 py-0.5 text-xs ${on ? "bg-sky-800 text-sky-100" : "bg-neutral-800 hover:bg-neutral-700"}`;

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
