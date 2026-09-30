import { useEffect, useRef } from "react";

/** True when the event target is a control that needs its own key handling (text entry, dropdowns). */
export function isTypingTarget(t: EventTarget | null, key: string): boolean {
  if (!(t instanceof HTMLElement)) return false;
  if (t.closest("textarea, select")) return true;
  const input = t.closest("input");
  if (!input) return false;
  if (input.type === "range") return key.startsWith("Arrow");
  return input.type === "text" || input.type === "number" || input.type === "search";
}

/** Global keydown handler that always sees the latest closure. */
export function useKeyboard(handler: (e: KeyboardEvent) => void) {
  const ref = useRef(handler);
  ref.current = handler;
  useEffect(() => {
    const down = (e: KeyboardEvent) => {
      if (isTypingTarget(e.target, e.key)) return;
      ref.current(e);
    };
    // Buttons/checkboxes keep focus after a click, which would make Space re-trigger them.
    const release = (e: Event) => {
      const t = e.target;
      if (!(t instanceof HTMLElement)) return;
      if (t.closest("button, input[type=checkbox], input[type=range]") || (e.type === "change" && t.closest("select"))) t.blur();
    };
    window.addEventListener("keydown", down);
    window.addEventListener("click", release);
    window.addEventListener("change", release);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("click", release);
      window.removeEventListener("change", release);
    };
  }, []);
}
