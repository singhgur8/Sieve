// Upright: runs `auto_upright` for the Transform panel (buttons, Guided guides) and commits the result as ONE history entry.
import { useCallback, useEffect, useRef, useState } from "react";
import { commands, unwrap, type UprightGuide, type UprightMode } from "../ipc";
import type { Editor } from "./useEditor";
import { uprightLabel } from "../lib/transform";

export interface UprightApi {
  /** Inline note when the last solve found no usable lines (shown in the Transform panel). */
  message: string | null;
  busy: boolean;
  /** Upright button: Off clears; others solve (or keep Lightroom's own solve of that mode). */
  run: (mode: UprightMode) => void;
  /** Guided: the guide lines changed. 2+ guides re-solve live (one history entry), fewer only store the guides. */
  setGuides: (guides: UprightGuide[]) => void;
}

export function useUpright(editor: Editor, id: number | null, onError: (e: unknown) => void): UprightApi {
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const seq = useRef(0);
  const adjRef = useRef(editor.adj);
  adjRef.current = editor.adj;
  const idRef = useRef(id);
  idRef.current = id;
  const { change } = editor;
  useEffect(() => {
    seq.current++;
    setMessage(null);
    setBusy(false);
  }, [id]);

  const solve = useCallback(
    async (mode: UprightMode, guides?: UprightGuide[]) => {
      const forId = idRef.current;
      if (forId == null) return;
      const cur = adjRef.current;
      const t = cur.transform;
      const my = ++seq.current;
      const label = `Upright: ${uprightLabel(mode)}`;
      if (mode === "off") {
        setMessage(null);
        // Lightroom's own solve stays stored (its mode no longer applies), so a later click on its mode keeps it.
        change((a) => ({ ...a, transform: { ...a.transform, upright: "off", solution: a.transform.solution && a.transform.solution.crs.length > 0 ? a.transform.solution : null } }), label);
        return;
      }
      // A photo Upright-corrected in Lightroom keeps that exact solve for its mode (the backend cannot reproduce its digests).
      if (!guides && t.solution && t.solution.crs.length > 0 && t.solution.mode === mode) {
        setMessage(null);
        change((a) => ({ ...a, transform: { ...a.transform, upright: mode } }), label);
        return;
      }
      const live = guides ? { ...cur, transform: { ...t, upright: mode, guides } } : cur;
      setBusy(true);
      try {
        const r = await unwrap(commands.autoUpright(forId, mode, live));
        if (my !== seq.current || idRef.current !== forId) return;
        if (r.solution) {
          setMessage(null);
          change((a) => ({ ...a, transform: { ...a.transform, upright: mode, guides: guides ?? a.transform.guides, solution: r.solution } }), label);
        } else {
          setMessage(r.message ?? `No straight lines found for ${uprightLabel(mode)}`);
          if (guides) change((a) => ({ ...a, transform: { ...a.transform, guides, solution: null } }), "Upright: Guides");
        }
      } catch (e) {
        if (my === seq.current) onError(e);
      } finally {
        if (my === seq.current) setBusy(false);
      }
    },
    [change, onError],
  );

  const run = useCallback((mode: UprightMode) => void solve(mode), [solve]);
  const setGuides = useCallback(
    (guides: UprightGuide[]) => {
      if (guides.length >= 2) return void solve("guided", guides);
      seq.current++;
      setBusy(false);
      setMessage(guides.length === 0 ? null : "Draw at least two guides");
      change((a) => ({ ...a, transform: { ...a.transform, guides, solution: a.transform.upright === "guided" ? null : a.transform.solution } }), "Upright: Guides");
    },
    [solve, change],
  );
  return { message, busy, run, setGuides };
}
