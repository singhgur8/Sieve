// Per-photo info (Lightroom's Metadata panel): file, capture time (corrected, with the original when different), camera,
// exposure, size, GPS and sidecar of the ONE photo that is active. Not to be confused with the gallery "Metadata filter".
import { Clock, X } from "lucide-react";
import type { ReactNode } from "react";
import { formatTime, formatShutter, trimNum } from "../lib/format";
import { sourceLabel } from "../lib/captureTime";
import { usePhotoMetadata } from "../hooks/usePhotoMetadata";
import { hint } from "../lib/keymap";
import { cameraLabel } from "../lib/metaFilter";

function Row({ label, children, testid }: { label: string; children: ReactNode; testid?: string }) {
  return (
    <div className="flex gap-2 py-0.5">
      <dt className="w-20 shrink-0 text-neutral-500">{label}</dt>
      <dd className="min-w-0 flex-1 break-words text-neutral-200" data-testid={testid}>
        {children}
      </dd>
    </div>
  );
}

const bytes = (n: number) => (n >= 1e6 ? `${(n / 1e6).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1e3))} KB`);
const dash = <span className="text-neutral-600">-</span>;

interface Props {
  imageId: number | null;
  /** Changes whenever the library may have changed this photo (re-fetches). */
  refetchKey: unknown;
  onClose: () => void;
  onEditTime: () => void;
  onRevert: (id: number) => void;
}

export function PhotoInfoPanel({ imageId, refetchKey, onClose, onEditTime, onRevert }: Props) {
  const { meta: m, error } = usePhotoMetadata(imageId, refetchKey);
  const edited = m != null && m.captureTimeSource !== "exif" && m.originalCapturedAtMs !== m.capturedAtMs;
  const exposure = m ? [m.shutterSeconds != null ? formatShutter(m.shutterSeconds) : null, m.aperture != null ? `f/${trimNum(m.aperture)}` : null, m.iso != null ? `ISO ${m.iso}` : null].filter(Boolean) : [];
  return (
    <aside className="fixed right-3 top-14 z-40 flex max-h-[calc(100vh-7rem)] w-80 flex-col overflow-hidden rounded-lg border border-neutral-700 bg-neutral-900/95 text-xs shadow-xl" data-testid="photo-info" aria-label="Photo info">
      <div className="flex items-center gap-2 border-b border-neutral-800 px-3 py-1.5">
        <h2 className="flex-1 text-xs font-semibold text-neutral-100">Photo info</h2>
        <span className="text-[10px] text-neutral-500">this photo only</span>
        <button onClick={onClose} className="rounded p-0.5 text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100" title={`Close the photo info${hint("photoInfo")}`} aria-label="Close photo info" data-testid="photo-info-close">
          <X className="size-4" />
        </button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-3 py-2">
        {imageId == null && <p className="text-neutral-400">Select a photo to see its details.</p>}
        {imageId != null && !m && !error && <p className="text-neutral-400">Loading…</p>}
        {error && (
          <p className="text-red-300" data-testid="photo-info-error">
            {error}
          </p>
        )}
        {m && (
          <dl data-testid="photo-info-body" data-image-id={m.imageId}>
            <Row label="File" testid="info-file">
              <b>{m.fileName}</b>
              {m.missing && <span className="ml-1 rounded bg-red-900 px-1 text-red-200">original missing</span>}
            </Row>
            <Row label="Folder" testid="info-folder">
              <span title={m.path}>{m.folderPath}</span>
            </Row>
            <Row label="Captured" testid="info-captured">
              {m.capturedAtMs != null ? formatTime(m.capturedAtMs) : <span className="text-neutral-400">no capture time</span>}
              <div className="text-[11px] text-neutral-500" data-testid="info-captured-source" data-source={m.captureTimeSource}>
                {sourceLabel(m.captureTimeSource)}
              </div>
            </Row>
            {edited && (
              <Row label="Original" testid="info-original">
                {m.originalCapturedAtMs != null ? formatTime(m.originalCapturedAtMs) : dash}
              </Row>
            )}
            <div className="flex flex-wrap gap-1.5 pb-1.5 pt-1">
              <button onClick={onEditTime} className="flex items-center gap-1 rounded bg-neutral-800 px-2 py-1 text-neutral-100 hover:bg-neutral-700" data-testid="info-edit-time" title={`Edit capture time: shift, set exactly or sync two cameras${hint("captureTime")}`}>
                <Clock className="size-3.5" /> Edit capture time…
              </button>
              {edited && (
                <button onClick={() => onRevert(m.imageId)} className="rounded bg-neutral-800 px-2 py-1 text-neutral-100 hover:bg-neutral-700" data-testid="info-revert-time" title="Put this photo back to the time stored in the file">
                  Revert to original
                </button>
              )}
            </div>
            <div className="my-1 border-t border-neutral-800" />
            <Row label="Camera" testid="info-camera">
              {m.camera.make === "other" && !m.camera.model ? dash : cameraLabel(m.camera)}
              {m.cameraSerial && <span className="text-neutral-500"> · #{m.cameraSerial}</span>}
            </Row>
            <Row label="Lens" testid="info-lens">
              {m.lens ?? dash}
            </Row>
            <Row label="Exposure" testid="info-exposure">
              {exposure.length > 0 ? exposure.join(" · ") : dash}
            </Row>
            <Row label="Focal length">
              {m.focalLengthMm != null ? `${trimNum(m.focalLengthMm)} mm` : dash}
              {m.focalLength35mm != null && m.focalLength35mm !== m.focalLengthMm && <span className="text-neutral-500"> ({trimNum(m.focalLength35mm)} mm equiv.)</span>}
            </Row>
            {m.exposureCompensationEv != null && <Row label="Exp. bias">{`${m.exposureCompensationEv > 0 ? "+" : ""}${trimNum(m.exposureCompensationEv)} EV`}</Row>}
            {m.flashFired != null && <Row label="Flash">{m.flashFired ? "Fired" : "Did not fire"}</Row>}
            <div className="my-1 border-t border-neutral-800" />
            <Row label="Size" testid="info-size">
              {m.width != null && m.height != null ? `${m.width} × ${m.height} px` : "size unknown"} · {bytes(m.fileSize)}
            </Row>
            <Row label="Format">{m.extension.toUpperCase()}</Row>
            <Row label="GPS" testid="info-gps">
              {m.gps ? `${m.gps.latitude.toFixed(5)}, ${m.gps.longitude.toFixed(5)}${m.gps.altitudeM != null ? ` · ${Math.round(m.gps.altitudeM)} m` : ""}` : <span className="text-neutral-500">none</span>}
            </Row>
            <Row label="Sidecar" testid="info-sidecar">
              <span title={m.sidecarPath}>{m.sidecarPath.split("/").pop()}</span> <span className="text-neutral-500">{m.sidecarExists ? "(exists)" : "(not written yet)"}</span>
            </Row>
            {m.companionPath && <Row label="Paired">{m.companionPath.split("/").pop()}</Row>}
          </dl>
        )}
      </div>
    </aside>
  );
}
