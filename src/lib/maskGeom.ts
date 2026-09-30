// Mask geometry: conversions between the displayed frame (what the viewer shows: EXIF-oriented and cropped),
// and the sensor frame the contract stores (un-oriented, uncropped, normalized). See docs/architecture.md "Masks".
//
// Chain: displayed (cropped) --crop rect (+ straighten angle)--> oriented uncropped --unorientPoint--> sensor.
import { orientPoint, unorientPoint, type CropSettings, type NormPoint, type NormRect, type RadialMask } from "../ipc";
import { FULL, fromStored } from "./crop";

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

function cropInfo(f: Frame) {
  const enabled = f.crop.enabled;
  const r = enabled ? fromStored(f.crop, f.orientation) : FULL;
  return { r, cx: (r.l + r.r) / 2, cy: (r.t + r.b) / 2, rw: r.r - r.l, rh: r.b - r.t, ang: enabled ? f.crop.angle : 0 };
}

/** Displayed (cropped) frame point -> oriented uncropped frame. The straighten angle rotates about the crop centre. */
export function dispToOriented(p: NormPoint, f: Frame): NormPoint {
  const { r, cx, cy, rw, rh, ang } = cropInfo(f);
  let x = r.l + p.x * rw;
  let y = r.t + p.y * rh;
  if (ang) {
    const dx = (x - cx) * f.w;
    const dy = (y - cy) * f.h;
    const c = Math.cos(ang * RAD);
    const s = Math.sin(ang * RAD);
    x = cx + (dx * c - dy * s) / f.w;
    y = cy + (dx * s + dy * c) / f.h;
  }
  return { x, y };
}

export function orientedToDisp(p: NormPoint, f: Frame): NormPoint {
  const { r, cx, cy, rw, rh, ang } = cropInfo(f);
  let { x, y } = p;
  if (ang) {
    const dx = (x - cx) * f.w;
    const dy = (y - cy) * f.h;
    const c = Math.cos(-ang * RAD);
    const s = Math.sin(-ang * RAD);
    x = cx + (dx * c - dy * s) / f.w;
    y = cy + (dx * s + dy * c) / f.h;
  }
  return { x: (x - r.l) / rw, y: (y - r.t) / rh };
}

/** Displayed frame -> sensor frame (what masks store). */
export const dispToSensor = (p: NormPoint, f: Frame): NormPoint => unorientPoint(dispToOriented(p, f), f.orientation);
/** Sensor frame -> displayed frame. */
export const sensorToDisp = (p: NormPoint, f: Frame): NormPoint => orientedToDisp(orientPoint(p, f.orientation), f);

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
  const { rw } = cropInfo(f);
  return box.w / Math.max(1, rw * f.w);
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
  const rot = Math.atan2(u.y, u.x) / RAD - cropInfo(f).ang;
  return { cx: c.x, cy: c.y, rx: ((m.right - m.left) / 2) * sz.w * s, ry: ((m.bottom - m.top) / 2) * sz.h * s, rot };
}

/** Inverse of `radialToScreen`; keeps the non-geometric fields of `base`. */
export function radialFromScreen(e: ScreenEllipse, f: Frame, box: Box, base: RadialMask): RadialMask {
  const cs = dispToSensor(screenToDisp(e.cx, e.cy, box), f);
  const s = screenScale(f, box);
  const sz = sensorSize(f);
  const rot = (e.rot + cropInfo(f).ang) * RAD;
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
