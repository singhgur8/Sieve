// Point-curve helpers for the Tone Curve editor (0..255 in and out).
import { MAX_CURVE_POINTS, type CurvePoint } from "../ipc";

export const CURVE_MAX = 255;
const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/** Monotone cubic (Fritsch-Carlson) samples of the curve for drawing, `n` points over 0..255. */
export function sampleCurve(pts: readonly CurvePoint[], n = 64): CurvePoint[] {
  const m = pts.length;
  if (m < 2) return [];
  const dx = pts.slice(1).map((p, i) => p[0] - pts[i][0]);
  const sl = pts.slice(1).map((p, i) => (p[1] - pts[i][1]) / (dx[i] || 1));
  const t = new Array<number>(m);
  t[0] = sl[0];
  t[m - 1] = sl[m - 2];
  for (let i = 1; i < m - 1; i++) t[i] = sl[i - 1] * sl[i] <= 0 ? 0 : (sl[i - 1] + sl[i]) / 2;
  for (let i = 0; i < m - 1; i++) {
    if (sl[i] === 0) {
      t[i] = 0;
      t[i + 1] = 0;
      continue;
    }
    const a = t[i] / sl[i];
    const b = t[i + 1] / sl[i];
    const h = Math.hypot(a, b);
    if (h > 3) {
      t[i] = (3 * a * sl[i]) / h;
      t[i + 1] = (3 * b * sl[i]) / h;
    }
  }
  const out: CurvePoint[] = [];
  for (let k = 0; k < n; k++) {
    const x = pts[0][0] + ((pts[m - 1][0] - pts[0][0]) * k) / (n - 1);
    let i = 0;
    while (i < m - 2 && x > pts[i + 1][0]) i++;
    const h = dx[i] || 1;
    const u = (x - pts[i][0]) / h;
    const u2 = u * u;
    const u3 = u2 * u;
    const y = (2 * u3 - 3 * u2 + 1) * pts[i][1] + (u3 - 2 * u2 + u) * h * t[i] + (-2 * u3 + 3 * u2) * pts[i + 1][1] + (u3 - u2) * h * t[i + 1];
    out.push([x, clamp(y, 0, CURVE_MAX)]);
  }
  return out;
}

/** Moves point `i` to (x, y): x stays strictly between its neighbours, y within 0..255. */
export function movePoint(pts: readonly CurvePoint[], i: number, x: number, y: number): CurvePoint[] {
  const lo = i === 0 ? 0 : pts[i - 1][0] + 1;
  const hi = i === pts.length - 1 ? CURVE_MAX : pts[i + 1][0] - 1;
  const out = pts.map((p) => [...p] as CurvePoint);
  out[i] = [Math.round(clamp(x, lo, hi)), Math.round(clamp(y, 0, CURVE_MAX))];
  return out;
}

/** Inserts a point at x (keeps input strictly increasing). Returns the new curve and the point's index; -1 when full. */
export function addPoint(pts: readonly CurvePoint[], x: number, y: number): { pts: CurvePoint[]; index: number } {
  const xi = Math.round(clamp(x, 0, CURVE_MAX));
  const existing = pts.findIndex((p) => p[0] === xi);
  if (existing >= 0) return { pts: pts.map((p) => [...p] as CurvePoint), index: existing };
  if (pts.length >= MAX_CURVE_POINTS) return { pts: pts.map((p) => [...p] as CurvePoint), index: -1 };
  const out = pts.map((p) => [...p] as CurvePoint);
  let at = out.findIndex((p) => p[0] > xi);
  if (at < 0) at = out.length;
  out.splice(at, 0, [xi, Math.round(clamp(y, 0, CURVE_MAX))]);
  return { pts: out, index: at };
}

/** Removes point `i` (a curve keeps at least 2 points). */
export function removePoint(pts: readonly CurvePoint[], i: number): CurvePoint[] {
  return pts.length <= 2 ? pts.map((p) => [...p] as CurvePoint) : pts.filter((_, k) => k !== i).map((p) => [...p] as CurvePoint);
}
