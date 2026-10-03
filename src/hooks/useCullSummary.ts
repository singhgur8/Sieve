// Cull summary of a project (`get_cull_summary`), refetched (debounced) whenever culling state may have changed.
import { useCallback, useEffect, useState } from "react";
import { commands, unwrap, type CullSummary } from "../ipc";

/** `deps` change whenever the library, the analysis or the keeper rule changed; the fetch trails by `delay` ms. */
export function useCullSummary(projectId: number | null, deps: readonly unknown[], delay = 300) {
  const [summary, setSummary] = useState<{ projectId: number; value: CullSummary } | null>(null);
  const [bump, setBump] = useState(0);
  useEffect(() => {
    if (projectId == null) return;
    let stale = false;
    const t = setTimeout(() => {
      unwrap(commands.getCullSummary(projectId))
        .then((s) => !stale && setSummary({ projectId, value: s }))
        .catch(() => {});
    }, delay);
    return () => {
      stale = true;
      clearTimeout(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectId, delay, bump, ...deps]);
  const refresh = useCallback(() => setBump((n) => n + 1), []);
  return { summary: summary && summary.projectId === projectId ? summary.value : null, refresh };
}
