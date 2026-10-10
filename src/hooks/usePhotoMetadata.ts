// Per-photo metadata (`get_image_metadata`) for the info panel; refetched when the photo, the library or its entry changes.
import { useEffect, useState } from "react";
import { commands, unwrap, type ImageMetadata } from "../ipc";

export function usePhotoMetadata(id: number | null, refetchKey: unknown): { meta: ImageMetadata | null; error: string | null } {
  const [state, setState] = useState<{ id: number; meta: ImageMetadata } | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (id == null) return;
    let stale = false;
    unwrap(commands.getImageMetadata(id))
      .then((m) => {
        if (stale) return;
        setState({ id, meta: m });
        setError(null);
      })
      .catch((e: unknown) => !stale && setError(e instanceof Error ? e.message : String((e as { message?: string })?.message ?? e)));
    return () => {
      stale = true;
    };
  }, [id, refetchKey]);
  return { meta: state && state.id === id ? state.meta : null, error: id == null ? null : error };
}
