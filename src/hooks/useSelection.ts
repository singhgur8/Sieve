import { useCallback, useRef, useState } from "react";

export interface SelectionApi {
  selected: Set<number>;
  active: number | null;
  click: (id: number, mods: { shift: boolean; meta: boolean }) => void;
  set: (ids: number[], active?: number | null) => void;
  selectAll: () => void;
  moveTo: (id: number, extend: boolean) => void;
  clear: () => void;
}

/** Click / shift-range / cmd-toggle selection over an ordered id list (`ids` = listImageIds result). */
export function useSelection(ids: number[]): SelectionApi {
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [active, setActive] = useState<number | null>(null);
  const anchor = useRef<number | null>(null);
  const idsRef = useRef(ids);
  idsRef.current = ids;

  const range = useCallback((a: number, b: number) => {
    const list = idsRef.current;
    const i = list.indexOf(a);
    const j = list.indexOf(b);
    if (i < 0 || j < 0) return [b];
    return list.slice(Math.min(i, j), Math.max(i, j) + 1);
  }, []);

  const click = useCallback(
    (id: number, mods: { shift: boolean; meta: boolean }) => {
      if (mods.shift && anchor.current != null) {
        setSelected(new Set(range(anchor.current, id)));
      } else if (mods.meta) {
        setSelected((prev) => {
          const n = new Set(prev);
          if (n.has(id)) n.delete(id);
          else n.add(id);
          return n;
        });
        anchor.current = id;
      } else {
        setSelected(new Set([id]));
        anchor.current = id;
      }
      setActive(id);
    },
    [range],
  );

  const moveTo = useCallback(
    (id: number, extend: boolean) => {
      if (extend && anchor.current != null) {
        setSelected(new Set(range(anchor.current, id)));
      } else {
        setSelected(new Set([id]));
        anchor.current = id;
      }
      setActive(id);
    },
    [range],
  );

  const set = useCallback((list: number[], act?: number | null) => {
    setSelected(new Set(list));
    const a = act === undefined ? (list[0] ?? null) : act;
    setActive(a);
    anchor.current = a;
  }, []);

  const selectAll = useCallback(() => setSelected(new Set(idsRef.current)), []);
  const clear = useCallback(() => setSelected(new Set()), []);

  return { selected, active, click, set, selectAll, moveTo, clear };
}
