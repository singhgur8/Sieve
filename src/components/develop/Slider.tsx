import { memo, useEffect, useRef, useState } from "react";

interface Props {
  id: string;
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  digits?: number;
  /** Neutral value: the accent fill runs from here to the current value, and a different value is emphasised. */
  defaultValue?: number;
  /** Map between the slider position (min..max) and the displayed value (e.g. logarithmic Kelvin). */
  display?: (v: number) => string;
  /** Text shown when the value is clicked for typing (default: the number with `digits` decimals). */
  editText?: (v: number) => string;
  /** Typed text -> slider position (default: the number itself); null = invalid. Temp accepts Kelvin here. */
  parse?: (s: string) => number | null;
  /** Step of the typed value for Up / Down (default `step`; Shift = x10). */
  textStep?: number;
  accent?: string;
  disabled?: boolean;
  onInput: (v: number) => void;
  onCommit: () => void;
  /** Double-click on the label or thumb. */
  onReset: () => void;
  /** Shift+double-click on the label or thumb: Lightroom's per-slider Auto (sliders that have one). */
  onAuto?: () => void;
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/** Lightroom-style slider: live `onInput`, `onCommit` on release, double-click resets, click the value to type it. */
export const Slider = memo(function Slider({
  id,
  label,
  value: valueProp,
  min,
  max,
  step,
  digits = 0,
  defaultValue,
  display,
  editText,
  parse,
  textStep,
  accent,
  disabled,
  onInput,
  onCommit,
  onReset,
  onAuto,
}: Props) {
  // While the user drags / nudges, the thumb and number follow the input on every event from local state, so they never
  // wait for the parent (editor state, renders). Released (`onCommit`) -> back to the prop.
  const [live, setLive] = useState<number | null>(null);
  const value = live ?? valueProp;
  // A gesture (pointer / key) is in progress: only then may `live` run ahead of the prop. Otherwise (programmatic
  // input, or the value was changed elsewhere: reset, undo, preset) the prop wins, so `live` can never get stuck.
  const gesture = useRef(false);
  useEffect(() => {
    if (!gesture.current) setLive(null);
  }, [valueProp]);
  const commit = () => {
    gesture.current = false;
    setLive(null);
    onCommit();
  };
  const shown = display ? display(value) : `${value > 0 && min < 0 ? "+" : ""}${value.toFixed(digits)}`;
  // Bipolar sliders are neutral at their centre / zero; 0-based ones at their minimum.
  const def = clamp(defaultValue ?? (min < 0 && max > 0 ? 0 : min), min, max);
  const changed = Math.abs(value - def) > step / 2;
  const span = max - min || 1;
  const frac = (v: number) => (clamp(v, min, max) - min) / span;
  const lo = Math.min(def, value);
  const hi = Math.max(def, value);
  const color = accent ?? "#7dd3fc";

  // Latest value for nudging (keys repeat faster than the parent re-renders).
  const valRef = useRef(value);
  valRef.current = value;
  const hovered = useRef(false);
  const nudgeTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  /** Step of a key press: Shift x10, Alt x0.1 (only where the display can show the finer step). */
  const stepFor = (e: { shiftKey: boolean; altKey: boolean }) => {
    const base = textStep ?? step;
    // Fine only where the display can show it (Temp in 5 K steps, a 0.1-step slider with 2 digits...).
    // Kelvin shows 10 K steps, so its fine step is 10 K (x0.2 of 50).
    const fine = textStep != null ? 0.2 : base * 0.1 >= 10 ** -digits - 1e-9 ? 0.1 : 1;
    return base * (e.shiftKey ? 10 : e.altKey ? fine : 1);
  };
  /** The slider position `sign` steps away (Kelvin-like sliders step in their typed unit); null = unchanged. */
  const nudgeValue = (sign: 1 | -1, e: { shiftKey: boolean; altKey: boolean }): number | null => {
    const cur = parse && editText ? Number.parseFloat(editText(valRef.current)) : valRef.current;
    if (!Number.isFinite(cur)) return null;
    const next = Math.round((cur + sign * stepFor(e)) * 1e6) / 1e6;
    const v = parse ? parse(String(next)) : next;
    if (v == null || !Number.isFinite(v)) return null;
    const out = clamp(v, min, max);
    return out === valRef.current ? null : out;
  };
  const nudge = (sign: 1 | -1, e: { shiftKey: boolean; altKey: boolean }) => {
    const v = nudgeValue(sign, e);
    if (v == null) return;
    valRef.current = v;
    setLive(v);
    onInput(v);
  };
  const nudgeRef = useRef(nudge);
  nudgeRef.current = nudge;
  const commitRef = useRef(commit);
  commitRef.current = commit;
  // Lightroom: hover a slider and press Up / Down. Only Up / Down (Left / Right keep moving between photos);
  // a focused text field or slider keeps its own keys.
  useEffect(() => {
    if (disabled) return;
    const down = (e: KeyboardEvent) => {
      if (!hovered.current || (e.key !== "ArrowUp" && e.key !== "ArrowDown") || e.metaKey || e.ctrlKey) return;
      const t = e.target;
      if (t instanceof HTMLElement && t.closest("input, textarea, select")) return;
      e.preventDefault();
      e.stopPropagation();
      nudgeRef.current(e.key === "ArrowUp" ? 1 : -1, e);
      clearTimeout(nudgeTimer.current);
      nudgeTimer.current = setTimeout(() => {
        nudgeTimer.current = undefined;
        commitRef.current();
      }, 300);
    };
    window.addEventListener("keydown", down, true);
    return () => {
      window.removeEventListener("keydown", down, true);
      if (nudgeTimer.current) {
        clearTimeout(nudgeTimer.current);
        nudgeTimer.current = undefined;
        commitRef.current();
      }
    };
  }, [disabled]);
  const leave = () => {
    hovered.current = false;
    if (nudgeTimer.current) {
      clearTimeout(nudgeTimer.current);
      nudgeTimer.current = undefined;
      commit();
    }
  };

  const [editing, setEditing] = useState<string | null>(null);
  const editRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (editing !== null) editRef.current?.select();
    // Select once when editing starts.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editing === null]);
  const dbl = (e: React.MouseEvent) => (e.shiftKey && onAuto ? onAuto() : onReset());
  const done = useRef(true);
  const startEdit = () => {
    if (disabled) return;
    done.current = false;
    setEditing(editText ? editText(value) : value.toFixed(digits));
  };
  const toValue = (s: string): number | null => {
    const v = parse ? parse(s) : Number.parseFloat(s);
    return v == null || !Number.isFinite(v) ? null : clamp(v, min, max);
  };
  const finish = (text: string | null) => {
    if (done.current) return; // blur fires again while the input unmounts
    done.current = true;
    setEditing(null);
    if (text === null) return;
    const v = toValue(text);
    if (v == null || v === value) return;
    onInput(v);
    onCommit(); // one history entry
  };

  return (
    <div className="flex h-6 items-center gap-2" data-testid={`slider-row-${id}`} data-changed={changed} onMouseEnter={() => (hovered.current = true)} onMouseLeave={leave}>
      <span
        className={`w-[72px] shrink-0 cursor-default select-none truncate text-xs ${changed ? "text-neutral-100" : "text-neutral-300"}`}
        onDoubleClick={dbl}
        title={`${label} (double-click to reset${onAuto ? ", Shift+double-click for auto" : ""})`}
        data-testid={`slider-label-${id}`}
      >
        {label}
      </span>
      <div className="relative h-6 min-w-0 flex-1">
        <div className="pointer-events-none absolute inset-x-0 top-1/2 h-0.5 -translate-y-1/2 rounded bg-neutral-700" />
        <div
          className="pointer-events-none absolute top-1/2 h-0.5 -translate-y-1/2 rounded"
          style={{ left: `calc(5px + (100% - 10px) * ${frac(lo)})`, width: `calc((100% - 10px) * ${frac(hi) - frac(lo)})`, backgroundColor: color, opacity: disabled ? 0.4 : 1 }}
          data-testid={`slider-fill-${id}`}
        />
        <input
          type="range"
          data-testid={`slider-${id}`}
          aria-label={label}
          className="sieve-range absolute inset-0 block h-6 w-full cursor-pointer"
          min={min}
          max={max}
          step={step}
          value={value}
          disabled={disabled}
          onChange={(e) => {
            const v = Number(e.target.value);
            setLive(v);
            onInput(v);
          }}
          onPointerDown={() => (gesture.current = true)}
          onKeyDown={() => (gesture.current = true)}
          onPointerUp={commit}
          onKeyDown={(e) => {
            // Focused slider: all four arrows step (Shift x10, Alt fine); Home / End / Page keys stay native.
            if (e.metaKey || e.ctrlKey || !e.key.startsWith("Arrow")) return;
            e.preventDefault();
            nudge(e.key === "ArrowUp" || e.key === "ArrowRight" ? 1 : -1, e);
          }}
          onKeyUp={(e) => (e.key.startsWith("Arrow") || ["Home", "End", "PageUp", "PageDown"].includes(e.key)) && commit()}
          onBlur={commit}
          onDoubleClick={disabled ? undefined : dbl}
        />
      </div>
      {editing !== null ? (
        <input
          ref={editRef}
          type="text"
          inputMode="decimal"
          autoFocus
          className="w-12 shrink-0 rounded bg-neutral-800 px-1 text-right text-xs tabular-nums text-neutral-100 outline outline-1 outline-sky-500"
          value={editing}
          aria-label={`${label} value`}
          data-testid={`slider-edit-${id}`}
          onChange={(e) => setEditing(e.target.value)}
          onBlur={() => finish(editing)}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === "Enter") {
              e.preventDefault();
              finish(editing);
            } else if (e.key === "Escape") {
              e.preventDefault();
              finish(null);
            } else if (e.key === "Tab") {
              // Commit and type into the next (previous with Shift) slider's value.
              e.preventDefault();
              // This slider shows its text field right now, so find it by the field's position among the rows.
              const rows = [...document.querySelectorAll<HTMLElement>('[data-testid^="slider-row-"]')].filter((r) => !r.querySelector("input[type=range]:disabled"));
              const at = rows.findIndex((r) => r.dataset.testid === `slider-row-${id}`);
              finish(editing);
              const next = rows[at + (e.shiftKey ? -1 : 1)]?.querySelector<HTMLButtonElement>('[data-testid^="slider-value-"]');
              if (next) setTimeout(() => next.click(), 0);
            } else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
              e.preventDefault();
              const cur = Number.parseFloat(editing);
              if (!Number.isFinite(cur)) return;
              const d = stepFor(e) * (e.key === "ArrowUp" ? 1 : -1);
              const next = Math.round((cur + d) * 1e6) / 1e6;
              const v = toValue(String(next));
              setEditing(v == null ? editing : editText ? editText(v) : v.toFixed(digits));
            }
          }}
        />
      ) : (
        <button
          type="button"
          className={`w-12 shrink-0 cursor-text whitespace-nowrap text-right text-xs tabular-nums ${changed ? "font-medium text-neutral-100" : "text-neutral-400"} disabled:cursor-default`}
          onClick={startEdit}
          disabled={disabled}
          title="Click to type a value (Enter applies, Esc cancels, Tab: next slider). Hover the slider and press Up / Down to nudge (Shift x10, Alt fine)"
          data-testid={`slider-value-${id}`}
        >
          {shown}
        </button>
      )}
    </div>
  );
});
