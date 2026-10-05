// Which image a view paints for a photo (IPC v19.1): an edited photo shows its cached edited render
// (`RawImageEntry.editedPreview`, content-addressed sieve:// URLs) instead of the camera's embedded preview, so
// edits are visible in the Library grid, Loupe, filmstrip and scenes, and switching photos in Develop never
// flashes the unedited look of an edited photo.
import { convertFileSrc, type RawImageEntry } from "../ipc";

/** The photo has edits but its edited render is not available yet (being rendered in the background). */
export function editedPending(e: RawImageEntry | undefined): boolean {
  return !!e && e.hasEdits && !e.editedPreview;
}

/** The camera's embedded thumbnail (512), whatever the edits. */
export function embeddedThumbSrc(e: RawImageEntry | undefined, version = 0): string | null {
  const t = e?.thumbnail;
  return t?.status === "ready" ? `${convertFileSrc(t.path)}?v=${version}` : null;
}

/** The camera's embedded preview (2048), whatever the edits. */
export function embeddedPreviewSrc(e: RawImageEntry | undefined, version = 0): string | null {
  const t = e?.thumbnail;
  return t?.status === "ready" ? `${convertFileSrc(t.previewPath ?? t.path)}?v=${version}` : null;
}

/** Grid-size image (long edge 512): the edited render when there is one, else the embedded thumbnail. */
export function thumbSrc(e: RawImageEntry | undefined, version = 0): string | null {
  if (!e) return null;
  if (e.hasEdits && e.editedPreview) return e.editedPreview.thumbUrl;
  return embeddedThumbSrc(e, version);
}

/** Loupe-size image (long edge 2048): the edited render when there is one, else the embedded preview. */
export function previewSrc(e: RawImageEntry | undefined, version = 0): string | null {
  if (!e) return null;
  if (e.hasEdits && e.editedPreview) return e.editedPreview.previewUrl;
  return embeddedPreviewSrc(e, version);
}

/**
 * Develop placeholder while a photo's own render is pending: its edited render, or the embedded preview for an
 * unedited photo. `null` for an edited photo without an edited render yet: showing the embedded preview there
 * would flash the photo without its edits.
 */
export function developPlaceholder(e: RawImageEntry | undefined, version = 0): string | null {
  return editedPending(e) ? null : previewSrc(e, version);
}
