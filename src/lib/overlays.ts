// Crop composition overlays as plain geometry in the crop rectangle's unit square (x right, y down). `pa` = the
// rectangle's pixel width / height, used where the guide is defined in real proportions (squares, perpendiculars, ratios).
import type { OverlayId } from "./crop";

export interface OverlayShapes {
  lines: [number, number, number, number][];
  paths: string[];
}

const PHI = (1 + Math.sqrt(5)) / 2;
const f = (n: number) => +n.toFixed(5);
type Pt = [number, number];

const arc = (e: Pt, r: number) => `A${f(r / PHI)} ${f(r)} 0 0 1 ${f(e[0] / PHI)} ${f(e[1])}`;

/** Golden spiral: successive squares of a golden rectangle, one quarter-ellipse arc per square (clockwise, continuous). */
function spiral(): string {
  let [x, y, w, h] = [0, 0, PHI, 1];
  let s: Pt = [0, 1]; // start of the first arc: bottom-left
  const d: string[] = [];
  for (let k = 0; k < 9; k++) {
    const side = Math.min(w, h);
    let e: Pt;
    switch (k % 4) {
      case 0: // square at the left; shared edge vertical at x + side
        e = [x + side, s[1] === y ? y + side : y];
        x += side;
        w -= side;
        break;
      case 1: // square at the top; shared edge horizontal at y + side
        e = [s[0] === x ? x + side : x, y + side];
        y += side;
        h -= side;
        break;
      case 2: // square at the right; shared edge vertical at x + w - side
        e = [x + w - side, s[1] === y ? y + side : y];
        w -= side;
        break;
      default: // square at the bottom; shared edge horizontal at y + h - side
        e = [s[0] === x ? x + side : x, y + h - side];
        h -= side;
    }
    d.push(arc(e, side));
    s = e; // the next arc starts where this one ended
  }
  return `M0 1 ${d.join(" ")}`;
}

let spiralPath: string | null = null;

export function overlayShapes(id: OverlayId, pa: number): OverlayShapes {
  const lines: OverlayShapes["lines"] = [];
  const paths: string[] = [];
  const hv = (a: number) => {
    lines.push([a, 0, a, 1]);
    lines.push([0, a, 1, a]);
  };
  switch (id) {
    case "thirds":
      for (const a of [1 / 3, 2 / 3]) hv(a);
      break;
    case "goldenRatio":
      for (const a of [1 / (PHI * PHI), 1 / PHI]) hv(a);
      break;
    case "grid": {
      const ny = pa >= 1 ? 6 : Math.max(2, Math.round(6 / pa));
      const nx = pa >= 1 ? Math.max(2, Math.round(6 * pa)) : 6;
      for (let i = 1; i < nx; i++) lines.push([i / nx, 0, i / nx, 1]);
      for (let i = 1; i < ny; i++) lines.push([0, i / ny, 1, i / ny]);
      break;
    }
    case "diagonal":
      lines.push([0, 0, 1, 1], [0, 1, 1, 0]);
      break;
    case "triangle": {
      // Diagonal bottom-left -> top-right; perpendiculars from the other two corners (in real proportions).
      lines.push([0, 1, 1, 0]);
      const [dx, dy] = [pa, -1];
      const n = dx * dx + dy * dy;
      for (const [px, py] of [[0, 0], [pa, 1]]) {
        const t = (px * dx + (py - 1) * dy) / n;
        lines.push([px / pa, py, (t * dx) / pa, 1 + t * dy]);
      }
      break;
    }
    case "goldenSpiral":
      spiralPath ??= spiral();
      paths.push(spiralPath);
      break;
    case "aspects":
      for (const r0 of [1, 5 / 4, 7 / 5, 3 / 2, 16 / 9]) {
        const r = pa >= 1 ? r0 : 1 / r0;
        let w = 1;
        let h = pa / r;
        if (h > 1) {
          h = 1;
          w = r / pa;
        }
        if (Math.abs(w - 1) < 1e-3 && Math.abs(h - 1) < 1e-3) continue;
        const [l, t, rr, b] = [(1 - w) / 2, (1 - h) / 2, (1 + w) / 2, (1 + h) / 2];
        paths.push(`M${f(l)} ${f(t)}H${f(rr)}V${f(b)}H${f(l)}Z`);
      }
      break;
  }
  return { lines, paths };
}

/** SVG transform for overlay orientation 0..3 (Shift+O): none, mirror horizontally, rotate 180, mirror vertically. */
export const ORIENT_TRANSFORM = ["", "translate(1 0) scale(-1 1)", "translate(1 1) scale(-1 -1)", "translate(0 1) scale(1 -1)"];
