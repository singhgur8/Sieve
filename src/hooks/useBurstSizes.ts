// Burst sizes by group id ("Burst of 5" on the badges), re-read when the folder / project or the analysis changes.
import { useEffect, useState } from "react";
import { commands, unwrap } from "../ipc";

export function useBurstSizes(folderId: number | null, projectId: number | null, key: unknown): Map<number, number> {
  const [sizes, setSizes] = useState<Map<number, number>>(new Map());
  useEffect(() => {
    let stale = false;
    unwrap(commands.listBurstGroups(folderId, projectId))
      .then((groups) => !stale && setSizes(new Map(groups.map((g) => [g.id, g.imageIds.length]))))
      .catch(() => {});
    return () => {
      stale = true;
    };
  }, [folderId, projectId, key]);
  return sizes;
}
