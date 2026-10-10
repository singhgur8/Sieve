// Helpers of the "Pick the best N" flow (target-count culling, IPC v20): labels, the remembered target per shoot type and the
// per-project "reviewed" sets of the two review passes.
import type { Moment, RawImageEntry, ShootType, ShotType, TargetChoice } from "../ipc";

export const SHOT_LABEL: Record<ShotType, string> = { couple: "Couple", group: "Group", detail: "Detail", candid: "Candid", other: "Other" };
export const SHOT_ORDER: ShotType[] = ["couple", "group", "detail", "candid", "other"];
export const SHOT_STYLE: Record<ShotType, string> = {
  couple: "bg-rose-900 text-rose-100",
  group: "bg-indigo-900 text-indigo-100",
  detail: "bg-amber-900 text-amber-100",
  candid: "bg-teal-900 text-teal-100",
  other: "bg-neutral-700 text-neutral-200",
};
export const SHOT_HINT: Record<ShotType, string> = {
  couple: "The main couple: many variations are kept, only near-identical frames collapse",
  group: "Posed groups: one per setup, the frame where most people look at the camera",
  detail: "Rings, dress, decor: one per detail, sharp on the object",
  candid: "Guests and moments: kept when a face or the action is clearly visible",
  other: "Everything that fits no other kind",
};
export const CHOICE_LABEL: Record<TargetChoice, string> = { deliver: "Picked", alternative: "Alternative", not_sure: "Not sure", set_aside: "Set aside" };

export const SHOOT_TYPES: ShootType[] = ["wedding", "portrait", "sports", "event", "landscape", "general"];
export const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);
export const plural = (n: number, w: string, many = `${w}s`) => `${n} ${n === 1 ? w : many}`;

/** The default target for a project: about a third of its photos (rounded to 10 above 100). */
export function suggestTarget(photoCount: number): number {
  const third = Math.round(photoCount / 3);
  return Math.max(1, photoCount >= 100 ? Math.round(third / 10) * 10 : third);
}

const countKey = (t: ShootType) => `sieve.target.count.${t}`;
/** Last target the user ran for this shoot type, or null. */
export function rememberedTarget(t: ShootType): number | null {
  try {
    const v = Number(localStorage.getItem(countKey(t)));
    return Number.isInteger(v) && v > 0 ? v : null;
  } catch {
    return null;
  }
}
export function rememberTarget(t: ShootType, n: number) {
  try {
    localStorage.setItem(countKey(t), String(n));
  } catch {
    /* private mode: the suggestion is used next time */
  }
}

const reviewedKey = (projectId: number, pass: "review" | "second") => `sieve.target.reviewed.${pass}.${projectId}`;
/** Photos already looked at in a review pass (kept per project, so a second sitting continues where the first stopped). */
export function loadReviewed(projectId: number, pass: "review" | "second"): Set<number> {
  try {
    const raw = localStorage.getItem(reviewedKey(projectId, pass));
    return new Set(raw ? (JSON.parse(raw) as number[]) : []);
  } catch {
    return new Set();
  }
}
export function saveReviewed(projectId: number, pass: "review" | "second", s: Set<number>) {
  try {
    localStorage.setItem(reviewedKey(projectId, pass), JSON.stringify([...s]));
  } catch {
    /* ignore */
  }
}
export function clearReviewed(projectId: number) {
  try {
    localStorage.removeItem(reviewedKey(projectId, "review"));
    localStorage.removeItem(reviewedKey(projectId, "second"));
  } catch {
    /* ignore */
  }
}

const time = (t: number | null) => (t == null ? "" : new Date(t).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }));

/** "Couple · 14:32 · frame 3 of 8" */
export function momentLabel(m: Moment | undefined, imageId: number): string {
  if (!m) return "";
  const at = m.imageIds.indexOf(imageId);
  return [SHOT_LABEL[m.shotType], time(m.startedAtMs), at >= 0 ? `frame ${at + 1} of ${m.imageIds.length}` : `${m.imageIds.length} frames`].filter(Boolean).join(" · ");
}

export const stemOf = (e: RawImageEntry | undefined, id: number) => (e?.fileName ?? `#${id}`).replace(/\.[^.]+$/, "");
