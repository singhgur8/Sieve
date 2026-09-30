// Crop geometry for the crop tool. Rects are fractions (0..1) of the frame *as displayed* (EXIF orientation applied);
// `CropSettings` stores fractions of the un-oriented image, so commit/load convert with `toStored` / `fromStored`.
import type { CropSettings } from "../ipc";

export interface Rect {
  l: number;
  t: number;
  r: number;
  b: number;
}

export const FULL: Rect = { l: 0, t: 0, r: 1, b: 1 };
export const MIN_SIZE = 0.02;

/** "custom" = locked to the ratio the free rectangle had when X / the swap button was used (not offered in the list). */
export type AspectId = "free" | "original" | "custom" | "1:1" | "4:5" | "5:7" | "3:2" | "16:9";
export const ASPECTS: { id: Exclude<AspectId, "custom">; label: string; ratio: number | null }[] = [
  { id: "free", label: "Free", ratio: null },
  { id: "original", label: "Original", ratio: null },
  { id: "1:1", label: "1 x 1", ratio: 1 },
  { id: "4:5", label: "4 x 5", ratio: 4 / 5 },
  { id: "5:7", label: "5 x 7", ratio: 5 / 7 },
  { id: "3:2", label: "3 x 2", ratio: 3 / 2 },
  { id: "16:9", label: "16 x 9", ratio: 16 / 9 },
];

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/** Display -> stored (un-oriented) point mapping for EXIF orientation 1..8. */
function toStoredPoint(o: number, u: number, v: number): [number, number] {
  switch (o) {
    case 2: return [1 - u, v];
    case 3: return [1 - u, 1 - v];
    case 4: return [u, 1 - v];
    case 5: return [v, u];
    case 6: return [v, 1 - u];
    case 7: return [1 - v, 1 - u];
    case 8: return [1 - v, u];
    default: return [u, v];
  }
}
function fromStoredPoint(o: number, x: number, y: number): [number, number] {
  switch (o) {
    case 2: return [1 - x, y];
    case 3: return [1 - x, 1 - y];
    case 4: return [x, 1 - y];
    case 5: return [y, x];
    case 6: return [1 - y, x];
    case 7: return [1 - y, 1 - x];
    case 8: return [y, 1 - x];
    default: return [x, y];
  }
}

function mapRect(r: Rect, f: (u: number, v: number) => [number, number]): Rect {
  const a = f(r.l, r.t);
  const b = f(r.r, r.b);
  return { l: Math.min(a[0], b[0]), r: Math.max(a[0], b[0]), t: Math.min(a[1], b[1]), b: Math.max(a[1], b[1]) };
}

const round = (n: number) => Math.round(n * 10000) / 10000;

const RAD = Math.PI / 180;

/** True for the mirrored EXIF orientations (2, 4, 5, 7), which flip the sense of a rotation on screen. */
const mirrored = (o: number) => o === 2 || o === 4 || o === 5 || o === 7;
/** Orientations 5..8 swap the sensor's width and height on screen. */
const swapsAxes = (o: number) => o >= 5 && o <= 8;

/**
 * Rotation (degrees, CSS `rotate()` sense: positive = clockwise on screen) of the displayed uncropped image while the
 * straighten angle is `angle` (`crs:CropAngle`).
 *
 * Sign convention, read from `src-tauri/src/develop/parity.rs::crop_geometry`: the output frame is sampled from the
 * source at `p = C + R(angle) * local` with `R(a) = [cos a, -sin a; sin a, cos a]` in y-down pixels, i.e. the crop frame
 * is rotated *clockwise* by `angle` in the source. A source point therefore appears in the straightened frame rotated by
 * `-angle` (counter-clockwise for a positive angle): `rotation = -angle`. A mirrored orientation flips it, because the
 * rotation happens in the un-oriented frame and mirroring conjugates a rotation to its inverse.
 */
export function previewRotation(angle: number, orientation: number): number {
  return (mirrored(orientation) ? angle : -angle) || 0;
}

/** Inverse of `previewRotation`. */
export const angleFromRotation = (rotation: number, orientation: number): number => (mirrored(orientation) ? rotation : -rotation) || 0;

/**
 * Crop rectangle of the displayed *straightened* frame -> `CropSettings`. While straightening, the tool's rectangle is
 * axis-aligned on the rotated image (Lightroom); the stored fields are the corners of the same frame in the un-rotated
 * source (`crop_geometry`: `left/top` and `right/bottom` are the rotated frame's corner-to-corner points, its size is that
 * vector rotated back by `-angle`). `aspect` = displayed (oriented, uncropped) width / height; used when `angle != 0`.
 */
export function toStored(r: Rect, orientation: number, angle: number, aspect = 1): CropSettings {
  if (!angle) {
    const s = mapRect(r, (u, v) => toStoredPoint(orientation, u, v));
    return { enabled: true, top: round(s.t), left: round(s.l), bottom: round(s.b), right: round(s.r), angle: 0 };
  }
  // Un-oriented pixel space with height 1: the image is (au, 1).
  const au = swapsAxes(orientation) ? 1 / aspect : aspect;
  const [cxf, cyf] = toStoredPoint(orientation, (r.l + r.r) / 2, (r.t + r.b) / 2);
  const dw = (r.r - r.l) * aspect;
  const dh = r.b - r.t;
  const [fw, fh] = swapsAxes(orientation) ? [dh, dw] : [dw, dh];
  const th = angle * RAD;
  const [sn, cs] = [Math.sin(th), Math.cos(th)];
  // Frame centre in the straightened image (q) -> in the source: C = c + R(th) (q - c).
  const qx = cxf * au - au / 2;
  const qy = cyf - 0.5;
  const cx = au / 2 + cs * qx - sn * qy;
  const cy = 0.5 + sn * qx + cs * qy;
  const dx = fw * cs - fh * sn;
  const dy = fw * sn + fh * cs;
  const xs = [(cx - dx / 2) / au, (cx + dx / 2) / au];
  const ys = [cy - dy / 2, cy + dy / 2];
  return { enabled: true, top: round(Math.min(...ys)), left: round(Math.min(...xs)), bottom: round(Math.max(...ys)), right: round(Math.max(...xs)), angle: Math.round(angle * 100) / 100 };
}

/** `CropSettings` -> the displayed straightened rect (inverse of `toStored`; `aspect` = displayed uncropped w / h). */
export function fromStored(c: Pick<CropSettings, "top" | "left" | "bottom" | "right"> & { angle?: number }, orientation: number, aspect = 1): Rect {
  const angle = c.angle ?? 0;
  if (!angle) return mapRect({ l: c.left, t: c.top, r: c.right, b: c.bottom }, (x, y) => fromStoredPoint(orientation, x, y));
  const au = swapsAxes(orientation) ? 1 / aspect : aspect;
  const th = angle * RAD;
  const [sn, cs] = [Math.sin(th), Math.cos(th)];
  const cx = ((c.left + c.right) / 2) * au;
  const cy = (c.top + c.bottom) / 2;
  const dx = (c.right - c.left) * au;
  const dy = c.bottom - c.top;
  const fw = Math.max(dx * cs + dy * sn, 1e-6);
  const fh = Math.max(-dx * sn + dy * cs, 1e-6);
  // Source -> straightened: q = c + R(-th) (C - c).
  const px = cx - au / 2;
  const py = cy - 0.5;
  const qx = (au / 2 + cs * px + sn * py) / au;
  const qy = 0.5 - sn * px + cs * py;
  const [u, v] = fromStoredPoint(orientation, qx, qy);
  const [w, h] = swapsAxes(orientation) ? [fh, fw] : [fw, fh];
  const hw = w / aspect / 2;
  return { l: u - hw, r: u + hw, t: v - h / 2, b: v + h / 2 };
}

/**
 * Lightroom "constrain to image": true when the axis-aligned displayed rect lies inside the uncropped image rotated by
 * `rotation` degrees (CSS sense) about its centre, and inside the frame. `aspect` = image w / h.
 */
export function insideRotated(rect: Rect, aspect: number, rotation: number, eps = 1e-4): boolean {
  const th = rotation * RAD;
  const [sn, cs] = [Math.sin(th), Math.cos(th)];
  for (const [u, v] of [[rect.l, rect.t], [rect.r, rect.t], [rect.l, rect.b], [rect.r, rect.b]]) {
    const dx = (u - 0.5) * aspect;
    const dy = v - 0.5;
    // Rotate by -rotation into the image's own frame.
    const lx = cs * dx + sn * dy;
    const ly = -sn * dx + cs * dy;
    if (Math.abs(lx) > aspect / 2 + eps || Math.abs(ly) > 0.5 + eps) return false;
  }
  return rect.l >= -eps && rect.t >= -eps && rect.r <= 1 + eps && rect.b <= 1 + eps;
}

/** Shrinks `rect` about its centre (same aspect ratio) until it lies inside the rotated image and the frame. */
export function fitInsideRotated(rect: Rect, aspect: number, rotation: number): Rect {
  if (insideRotated(rect, aspect, rotation)) return rect;
  const th = rotation * RAD;
  const [sn, cs] = [Math.sin(th), Math.cos(th)];
  const hw = ((rect.r - rect.l) * aspect) / 2; // half sizes in height units
  const hh = (rect.b - rect.t) / 2;
  const toLocal = (x: number, y: number): [number, number] => [cs * x + sn * y, -sn * x + cs * y];
  // Centre in the image's frame, pulled inside first.
  let [qx, qy] = toLocal(((rect.l + rect.r) / 2 - 0.5) * aspect, (rect.t + rect.b) / 2 - 0.5);
  qx = clamp(qx, -aspect / 2, aspect / 2);
  qy = clamp(qy, -0.5, 0.5);
  // Corner extents in the image's frame (worst case over the four corners).
  const ex = Math.max(Math.abs(toLocal(hw, hh)[0]), Math.abs(toLocal(-hw, hh)[0]));
  const ey = Math.max(Math.abs(toLocal(hw, hh)[1]), Math.abs(toLocal(-hw, hh)[1]));
  let s = Math.min(1, ex > 1e-9 ? (aspect / 2 - Math.abs(qx)) / ex : 1, ey > 1e-9 ? (0.5 - Math.abs(qy)) / ey : 1);
  // Back to displayed coordinates (rotate by +rotation).
  const cu = (cs * qx - sn * qy) / aspect + 0.5;
  const cv = sn * qx + cs * qy + 0.5;
  // The frame itself bounds the rect too.
  const fu = hw / aspect;
  s = Math.min(s, fu > 1e-9 ? Math.min(cu, 1 - cu) / fu : 1, hh > 1e-9 ? Math.min(cv, 1 - cv) / hh : 1);
  s = Math.max(0, s - 1e-6);
  return { l: cu - fu * s, r: cu + fu * s, t: cv - hh * s, b: cv + hh * s };
}

export const isFull = (r: Rect) => r.l <= 0.0005 && r.t <= 0.0005 && r.r >= 0.9995 && r.b >= 0.9995;

export type Handle = "nw" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w";
export const HANDLES: Handle[] = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];

/**
 * Resizes `rect` by dragging `handle` to the fractional point (px, py). `fr` = width / height in fraction units
 * (aspect lock) or null for a free crop.
 */
export function resizeRect(rect: Rect, handle: Handle, px: number, py: number, fr: number | null): Rect {
  const hx = handle.includes("w") ? -1 : handle.includes("e") ? 1 : 0;
  const hy = handle.includes("n") ? -1 : handle.includes("s") ? 1 : 0;
  px = clamp(px, 0, 1);
  py = clamp(py, 0, 1);
  if (fr == null) {
    const o = { ...rect };
    if (hx < 0) o.l = Math.min(px, rect.r - MIN_SIZE);
    if (hx > 0) o.r = Math.max(px, rect.l + MIN_SIZE);
    if (hy < 0) o.t = Math.min(py, rect.b - MIN_SIZE);
    if (hy > 0) o.b = Math.max(py, rect.t + MIN_SIZE);
    return o;
  }
  const ax = hx < 0 ? rect.r : hx > 0 ? rect.l : (rect.l + rect.r) / 2;
  const ay = hy < 0 ? rect.b : hy > 0 ? rect.t : (rect.t + rect.b) / 2;
  let w: number;
  let h: number;
  if (hx !== 0 && hy !== 0) {
    w = Math.abs(px - ax);
    h = Math.abs(py - ay);
    w = Math.max(w, h * fr, MIN_SIZE);
    h = w / fr;
  } else if (hx !== 0) {
    w = Math.max(Math.abs(px - ax), MIN_SIZE);
    h = w / fr;
  } else {
    h = Math.max(Math.abs(py - ay), MIN_SIZE);
    w = h * fr;
  }
  // Room available from the anchor.
  const maxW = hx < 0 ? ax : hx > 0 ? 1 - ax : 2 * Math.min(ax, 1 - ax);
  const maxH = hy < 0 ? ay : hy > 0 ? 1 - ay : 2 * Math.min(ay, 1 - ay);
  const s = Math.min(1, maxW / w, maxH / h);
  w *= s;
  h *= s;
  const l = hx < 0 ? ax - w : hx > 0 ? ax : ax - w / 2;
  const t = hy < 0 ? ay - h : hy > 0 ? ay : ay - h / 2;
  return { l, t, r: l + w, b: t + h };
}

/** Moves `rect` by (dx, dy), keeping it inside the frame. */
export function moveRect(rect: Rect, dx: number, dy: number): Rect {
  const w = rect.r - rect.l;
  const h = rect.b - rect.t;
  const l = clamp(rect.l + dx, 0, 1 - w);
  const t = clamp(rect.t + dy, 0, 1 - h);
  return { l, t, r: l + w, b: t + h };
}

/** Largest rect of fraction ratio `fr` centred in `rect`. */
export function fitRatio(rect: Rect, fr: number): Rect {
  const w0 = rect.r - rect.l;
  const h0 = rect.b - rect.t;
  let w = w0;
  let h = w / fr;
  if (h > h0) {
    h = h0;
    w = h * fr;
  }
  const cx = (rect.l + rect.r) / 2;
  const cy = (rect.t + rect.b) / 2;
  return { l: cx - w / 2, r: cx + w / 2, t: cy - h / 2, b: cy + h / 2 };
}

/** Fraction-unit ratio (w / h of the rect) for a pixel aspect ratio on an image of aspect `imageAspect` (w / h). */
export const fractionRatio = (pixelRatio: number, imageAspect: number) => pixelRatio / imageAspect;

const ASPECT_KEY = "sieve.crop.aspect";
const VALID: AspectId[] = ["free", "original", "1:1", "4:5", "5:7", "3:2", "16:9"];
let lastLocked: AspectId = "original";

/** Aspect the crop tool starts with: the last one used (Lightroom starts locked to Original). */
export function loadCropAspect(): AspectId {
  try {
    const v = localStorage.getItem(ASPECT_KEY) as AspectId | null;
    if (v && VALID.includes(v)) {
      if (v !== "free") lastLocked = v;
      return v;
    }
  } catch {
    /* private mode */
  }
  return "original";
}

export function saveCropAspect(a: AspectId) {
  if (a === "custom") return;
  if (a !== "free") lastLocked = a;
  try {
    localStorage.setItem(ASPECT_KEY, a);
  } catch {
    /* private mode */
  }
}

/** Aspect the A key restores when unlocking -> locking (the last locked one, Original by default). */
export const lastLockedAspect = (): AspectId => lastLocked;
