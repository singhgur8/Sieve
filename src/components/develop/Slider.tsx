import { memo } from "react";

interface Props {
  id: string;
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  digits?: number;
  defaultValue?: number;
  /** Map between the slider position (min..max) and the displayed value (e.g. logarithmic Kelvin). */
  display?: (v: number) => string;
  accent?: string;
  disabled?: boolean;
  onInput: (v: number) => void;
  onCommit: () => void;
  /** Double-click on the label or thumb. */
  onReset: () => void;
}

/** Lightroom-style slider: live `onInput`, `onCommit` on release, double-click resets. */
export const Slider = memo(function Slider({ id, label, value, min, max, step, digits = 0, display, accent, disabled, onInput, onCommit, onReset }: Props) {
  const shown = display ? display(value) : `${value > 0 && min < 0 ? "+" : ""}${value.toFixed(digits)}`;
  return (
    <div className="mb-1.5" data-testid={`slider-row-${id}`}>
      <div className="flex items-baseline justify-between text-[11px] leading-4">
        <span className="cursor-default select-none text-neutral-400" onDoubleClick={onReset} title="Double-click to reset" data-testid={`slider-label-${id}`}>
          {label}
        </span>
        <span className="tabular-nums text-neutral-300" data-testid={`slider-value-${id}`}>
          {shown}
        </span>
      </div>
      <input
        type="range"
        data-testid={`slider-${id}`}
        aria-label={label}
        className="block h-3 w-full cursor-pointer"
        style={{ accentColor: accent ?? "#7dd3fc" }}
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        onChange={(e) => onInput(Number(e.target.value))}
        onPointerUp={onCommit}
        onKeyUp={(e) => (e.key.startsWith("Arrow") || ["Home", "End", "PageUp", "PageDown"].includes(e.key)) && onCommit()}
        onBlur={onCommit}
        onDoubleClick={disabled ? undefined : onReset}
      />
    </div>
  );
});
