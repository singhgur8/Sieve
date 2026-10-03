// Modal layer stack. While any layer is open the global shortcut handler is inert (`modalCount() > 0`);
// only the topmost layer reacts to Esc (cancel), Enter (confirm when enabled) and Tab (focus trap).
import { useEffect, useRef, useSyncExternalStore, type RefObject } from "react";

interface Layer {
  id: number;
  root: () => HTMLElement | null;
  cancel: () => void;
  hasConfirm: () => boolean;
  confirm: () => void;
  canConfirm: () => boolean;
  /** Only Cmd/Ctrl+Enter confirms (dialogs full of text fields, e.g. Export). */
  requireMod: () => boolean;
  trap: () => boolean;
}

const stack: Layer[] = [];
let nextId = 1;
let installed = false;

/** Number of open modal layers (dialogs and popover menus). */
export const modalCount = () => stack.length;

const listeners = new Set<() => void>();
const notify = () => listeners.forEach((l) => l());
const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => void listeners.delete(l);
};
/** Reactive `modalCount()`: re-renders when a layer is pushed or removed. */
export const useModalCount = () => useSyncExternalStore(subscribe, modalCount, modalCount);

const FOCUSABLE = 'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

function focusables(root: HTMLElement): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((n) => n.offsetParent !== null || n === document.activeElement);
}

function onKeyDown(e: KeyboardEvent) {
  const top = stack[stack.length - 1];
  if (!top) return;
  const root = top.root();
  const t = e.target instanceof HTMLElement ? e.target : null;
  const swallow = () => {
    e.preventDefault();
    e.stopPropagation();
    e.stopImmediatePropagation();
  };
  if (e.key === "Escape") {
    swallow();
    top.cancel();
  } else if (e.key === "Enter" && top.hasConfirm()) {
    const mod = e.metaKey || e.ctrlKey;
    if (top.requireMod() && !mod) return;
    // Native behaviour of buttons / selects / multi-line fields wins for a plain Enter.
    if (!mod && t?.closest("button, select, textarea, a")) return;
    swallow();
    if (top.canConfirm()) top.confirm();
  } else if (e.key === "Tab" && top.trap() && root) {
    const list = focusables(root);
    if (list.length === 0) {
      swallow();
      return;
    }
    const first = list[0];
    const last = list[list.length - 1];
    const inside = t != null && root.contains(t);
    if (!inside) {
      swallow();
      (e.shiftKey ? last : first).focus();
    } else if (e.shiftKey && t === first) {
      swallow();
      last.focus();
    } else if (!e.shiftKey && t === last) {
      swallow();
      first.focus();
    }
  }
}

function install() {
  if (installed) return;
  installed = true;
  window.addEventListener("keydown", onKeyDown, true);
}

export interface ModalLayerOptions {
  ref: RefObject<HTMLElement | null>;
  onCancel: () => void;
  onConfirm?: () => void;
  canConfirm?: () => boolean;
  requireMod?: boolean;
  /** Trap Tab inside the element (dialogs); popover menus do not. */
  trap?: boolean;
  /** Focus the first control on mount (default true). */
  focusFirst?: boolean;
}

/** Registers a modal layer for the lifetime of the calling component. */
export function useModalLayer(o: ModalLayerOptions) {
  const opts = useRef(o);
  opts.current = o;
  useEffect(() => {
    install();
    const layer: Layer = {
      id: nextId++,
      root: () => opts.current.ref.current,
      cancel: () => opts.current.onCancel(),
      hasConfirm: () => !!opts.current.onConfirm,
      confirm: () => opts.current.onConfirm?.(),
      canConfirm: () => opts.current.canConfirm?.() ?? true,
      requireMod: () => !!opts.current.requireMod,
      trap: () => opts.current.trap ?? true,
    };
    stack.push(layer);
    notify();
    const prev = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    if (opts.current.focusFirst !== false) {
      const root = opts.current.ref.current;
      if (root) {
        const auto = root.querySelector<HTMLElement>("[data-autofocus]");
        (auto ?? focusables(root)[0])?.focus();
      }
    }
    return () => {
      const i = stack.findIndex((l) => l.id === layer.id);
      if (i >= 0) stack.splice(i, 1);
      notify();
      if (prev && document.contains(prev) && prev !== document.body && document.activeElement === document.body) prev.focus();
    };
  }, []);
}
