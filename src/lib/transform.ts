// Transform panel helpers: Upright modes, the crop angle of a Level solve, guide geometry (sensor frame <-> displayed).
import type { UprightGuide, UprightMode } from "../ipc";
import { fromStoredPoint, toStoredPoint } from "./crop";

/** Lightroom's Upright button order. */
export const UPRIGHT_BUTTONS: { mode: UprightMode; label: string; tip: string }[] = [
  { mode: "off", label: "Off", tip: "No Upright correction" },
  { mode: "auto", label: "Auto", tip: "Balanced level + vertical correction" },
  { mode: "guided", label: "Guided", tip: "Draw 2 to 4 lines along things that should be straight (Shift+T)" },
  { mode: "level", label: "Level", tip: "Level the horizon (rotation only)" },
  { mode: "vertical", label: "Vertical", tip: "Level and straighten converging verticals" },
  { mode: "full", label: "Full", tip: "Level, vertical and horizontal perspective" },
];

export const uprightLabel = (m: UprightMode) => UPRIGHT_BUTTONS.find((b) => b.mode === m)!.label;

/** Max guide lines (Lightroom: 4). */
export const MAX_GUIDES = 4;

/** `ml::upright::crop_angle_for_rotation`: crop angle (degrees) that straightens like a Level solve's `rotationDeg`. */
export function cropAngleForRotation(rotationDeg: number, orientation: number): number {
  const mirrored = orientation === 2 || orientation === 4 || orientation === 5 || orientation === 7;
  const a = mirrored ? -rotationDeg : rotationDeg;
  return Math.max(-45, Math.min(45, Math.round(a * 100) / 100)) || 0;
}

type P = { x: number; y: number };
/** Sensor-frame point -> displayed (oriented) fraction of the frame. */
export const guideToDisplay = (o: number, p: P): P => {
  const [x, y] = fromStoredPoint(o, p.x, p.y);
  return { x, y };
};
/** Displayed fraction -> sensor-frame point. */
export const guideFromDisplay = (o: number, p: P): P => {
  const [x, y] = toStoredPoint(o, p.x, p.y);
  return { x, y };
};

export const sameGuides = (a: UprightGuide[], b: UprightGuide[]) => JSON.stringify(a) === JSON.stringify(b);
