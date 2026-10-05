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
    <div className="flex h-6 items-center gap-2" data-testid={`slider-row-${id}`} data-changed={changed}>
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
            } else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
              e.preventDefault();
              const cur = Number.parseFloat(editing);
              if (!Number.isFinite(cur)) return;
              const d = (textStep ?? step) * (e.shiftKey ? 10 : 1) * (e.key === "ArrowUp" ? 1 : -1);
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
          title="Click to type a value"
          data-testid={`slider-value-${id}`}
        >
          {shown}
        </button>
      )}
    </div>
  );
});
