// Mask geometry: conversions between the displayed frame (what the viewer shows: EXIF-oriented and cropped),
// and the sensor frame the contract stores (un-oriented, uncropped, normalized). See docs/architecture.md "Masks".
//
// Chain: displayed (cropped) --unorient--> un-oriented frame --crop frame (corners + straighten angle)--> sensor.
import { orientPoint, unorientPoint, type CropSettings, type NormPoint, type NormRect, type RadialMask } from "../ipc";

export interface Frame {
  /** EXIF orientation 1..8. */
  orientation: number;
  crop: CropSettings;
  /** Oriented, uncropped size in pixels (`DevelopInfo.fullWidth/fullHeight`). */
  w: number;
  h: number;
}

/** Screen box of the displayed (cropped) frame inside the viewer, in CSS px. */
export interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

const RAD = Math.PI / 180;
export const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/**
 * The crop frame exactly as the backend evaluates it (`develop/masks/eval.rs::crop_frame`, same as
 * `parity.rs::crop_geometry`): `left/top` and `right/bottom` are the corners of the (rotated) frame in the un-oriented
 * source, whose size is that corner-to-corner vector rotated back by `-angle`; the frame is sampled at
 * `p = C + R(angle) * ((u - 0.5) fw, (v - 0.5) fh)` (pixels, y down). Everything here is a similarity (uniform scale,
 * rotation, orientation), so angles and circles survive the mapping.
 */
function cropFrame(f: Frame) {
  const { w: sw, h: sh } = sensorSize(f);
  const c = f.crop;
  const l = clamp(c.left, 0, 1);
  const r = clamp(c.right, 0, 1);
  const t = clamp(c.top, 0, 1);
  const b = clamp(c.bottom, 0, 1);
  const on = c.enabled && r - l > 1e-6 && b - t > 1e-6;
  const ang = on ? clamp(c.angle, -45, 45) : 0;
  const [cx, cy] = on ? [((l + r) / 2) * sw, ((t + b) / 2) * sh] : [sw / 2, sh / 2];
  const [dx, dy] = on ? [(r - l) * sw, (b - t) * sh] : [sw, sh];
  const cs = Math.cos(ang * RAD);
  const sn = Math.sin(ang * RAD);
  const fw = Math.max(dx * cs + dy * sn, 1);
  const fh = Math.max(-dx * sn + dy * cs, 1);
  return { sw, sh, cx, cy, cs, sn, fw, fh, ang };
}

/** Displayed (cropped) frame point -> sensor frame (what masks store). */
export function dispToSensor(p: NormPoint, f: Frame): NormPoint {
  const { sw, sh, cx, cy, cs, sn, fw, fh } = cropFrame(f);
  const q = unorientPoint(p, f.orientation); // un-oriented frame coordinates (u, v)
  const lx = (q.x - 0.5) * fw;
  const ly = (q.y - 0.5) * fh;
  return { x: (cx + cs * lx - sn * ly) / sw, y: (cy + sn * lx + cs * ly) / sh };
}

/** Sensor frame -> displayed (cropped) frame. */
export function sensorToDisp(p: NormPoint, f: Frame): NormPoint {
  const { sw, sh, cx, cy, cs, sn, fw, fh } = cropFrame(f);
  const px = p.x * sw - cx;
  const py = p.y * sh - cy;
  return orientPoint({ x: (cs * px + sn * py) / fw + 0.5, y: (-sn * px + cs * py) / fh + 0.5 }, f.orientation);
}

/** True for the mirrored EXIF orientations, which flip the sense of a rotation on screen. */
const mirroredOrientation = (o: number) => o === 2 || o === 4 || o === 5 || o === 7;
/** Straighten angle as seen on screen: the sense of the displayed rotation (a mirrored orientation flips it). */
const screenAngle = (f: Frame) => (mirroredOrientation(f.orientation) ? -1 : 1) * cropFrame(f).ang;

/** Axis-aligned rectangle spanned by two displayed points, as a sensor-frame `NormRect`. */
export function dispRectToSensor(a: NormPoint, b: NormPoint, f: Frame): NormRect {
  const p = dispToSensor(a, f);
  const q = dispToSensor(b, f);
  return { x: Math.min(p.x, q.x), y: Math.min(p.y, q.y), width: Math.abs(p.x - q.x), height: Math.abs(p.y - q.y) };
}

/** True for orientations 5..8, which swap the sensor's width and height on screen. */
export const swapsAxes = (o: number) => o >= 5 && o <= 8;

/** Sensor (un-oriented) size in pixels. */
export function sensorSize(f: Frame): { w: number; h: number } {
  return swapsAxes(f.orientation) ? { w: f.h, h: f.w } : { w: f.w, h: f.h };
}

/** Screen pixels per (oriented, uncropped) image pixel for a displayed-frame box. */
export function screenScale(f: Frame, box: Box): number {
  const { fw, fh } = cropFrame(f);
  return box.w / Math.max(1, swapsAxes(f.orientation) ? fh : fw);
}

/** On-screen brush radius (px) of a stroke radius given as a fraction of the sensor width. */
export function brushRadiusPx(radius: number, f: Frame, box: Box): number {
  return radius * sensorSize(f).w * screenScale(f, box);
}

/** Displayed-frame point (0..1) -> screen px inside the viewer. */
export const dispToScreen = (p: NormPoint, box: Box) => ({ x: box.x + p.x * box.w, y: box.y + p.y * box.h });
export const screenToDisp = (x: number, y: number, box: Box): NormPoint => ({ x: (x - box.x) / box.w, y: (y - box.y) / box.h });

// ---------------------------------------------------------------------------
// Radial gradients (ellipse + rotation)
// ---------------------------------------------------------------------------

/** Linear part of the orientation map applied to a direction vector. */
function orientVec(v: NormPoint, o: number): NormPoint {
  const a = orientPoint(v, o);
  const z = orientPoint({ x: 0, y: 0 }, o);
  return { x: a.x - z.x, y: a.y - z.y };
}
function unorientVec(v: NormPoint, o: number): NormPoint {
  const a = unorientPoint(v, o);
  const z = unorientPoint({ x: 0, y: 0 }, o);
  return { x: a.x - z.x, y: a.y - z.y };
}

/** A radial mask as drawn on screen: centre (px), semi-axes (px) along the rotated axes, rotation (deg, screen). */
export interface ScreenEllipse {
  cx: number;
  cy: number;
  rx: number;
  ry: number;
  rot: number;
}

export function radialToScreen(m: RadialMask, f: Frame, box: Box): ScreenEllipse {
  const cs = { x: (m.left + m.right) / 2, y: (m.top + m.bottom) / 2 };
  const c = dispToScreen(sensorToDisp(cs, f), box);
  const s = screenScale(f, box);
  const sz = sensorSize(f);
  const u = orientVec({ x: Math.cos(m.angle * RAD), y: Math.sin(m.angle * RAD) }, f.orientation);
  const rot = Math.atan2(u.y, u.x) / RAD - screenAngle(f);
  return { cx: c.x, cy: c.y, rx: ((m.right - m.left) / 2) * sz.w * s, ry: ((m.bottom - m.top) / 2) * sz.h * s, rot };
}

/** Inverse of `radialToScreen`; keeps the non-geometric fields of `base`. */
export function radialFromScreen(e: ScreenEllipse, f: Frame, box: Box, base: RadialMask): RadialMask {
  const cs = dispToSensor(screenToDisp(e.cx, e.cy, box), f);
  const s = screenScale(f, box);
  const sz = sensorSize(f);
  const rot = (e.rot + screenAngle(f)) * RAD;
  const u = unorientVec({ x: Math.cos(rot), y: Math.sin(rot) }, f.orientation);
  let hx = e.rx / s / sz.w;
  let hy = e.ry / s / sz.h;
  let ang = (Math.atan2(u.y, u.x) / RAD) % 180;
  // Prefer angle 0 with swapped extents over +-90 (the same ellipse; what Lightroom would write for an unrotated one).
  if (Math.abs(Math.abs(ang) - 90) < 0.01) {
    [hx, hy] = [hy * (sz.h / sz.w), hx * (sz.w / sz.h)];
    ang = 0;
  } else if (Math.abs(ang) < 0.01 || Math.abs(Math.abs(ang) - 180) < 0.01) ang = 0;
  return { ...base, left: cs.x - hx, right: cs.x + hx, top: cs.y - hy, bottom: cs.y + hy, angle: Math.round(ang * 100) / 100 };
}

/** Ellipse spanned by a drag from the centre (`a`) to a corner (`b`), in screen px. */
export function ellipseFromDrag(a: { x: number; y: number }, b: { x: number; y: number }): ScreenEllipse {
  return { cx: a.x, cy: a.y, rx: Math.max(4, Math.abs(b.x - a.x)), ry: Math.max(4, Math.abs(b.y - a.y)), rot: 0 };
}
