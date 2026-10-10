// Helpers of the "Pick the best N" flow (target-count culling, IPC v20): labels, the remembered target per shoot type and the
// per-project "reviewed" sets of the two review passes.
import { commands, unwrap, type Moment, type RawImageEntry, type ShootType, type ShotType, type TargetChoice } from "../ipc";

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

/** The review passes whose "looked at" sets are remembered: Pass 1, Second look Not sure, Second look Similar to a kept photo. */
export type Pass = "review" | "second" | "second_similar";
const reviewedKey = (projectId: number, pass: Pass) => `sieve.target.reviewed.${pass}.${projectId}`;
/** Photos already looked at in a review pass (kept per project, so a second sitting continues where the first stopped). */
export function loadReviewed(projectId: number, pass: Pass): Set<number> {
  try {
    const raw = localStorage.getItem(reviewedKey(projectId, pass));
    return new Set(raw ? (JSON.parse(raw) as number[]) : []);
  } catch {
    return new Set();
  }
}
export function saveReviewed(projectId: number, pass: Pass, s: Set<number>) {
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
export function momentLabel(m: Moment | undefined, imageId: number, withType = true): string {
  if (!m) return "";
  const at = m.imageIds.indexOf(imageId);
  return [withType ? SHOT_LABEL[m.shotType] : "", time(m.startedAtMs), at >= 0 ? `frame ${at + 1} of ${m.imageIds.length}` : `${m.imageIds.length} frames`].filter(Boolean).join(" · ");
}

export const stemOf = (e: RawImageEntry | undefined, id: number) => (e?.fileName ?? `#${id}`).replace(/\.[^.]+$/, "");

export const num = (n: number) => n.toLocaleString("en-US");
/** 1 -> "1st", 2 -> "2nd", 3 -> "3rd", 4 -> "4th" */
export function ordinal(n: number): string {
  const v = n % 100;
  if (v >= 11 && v <= 13) return `${n}th`;
  return `${n}${["th", "st", "nd", "rd"][n % 10 > 3 ? 0 : n % 10]}`;
}

// ---- Second look piles (P0-1): classified client-side from the first reason of each Not sure / Set aside row ----
export type Pile = "not_sure" | "similar" | "weaker" | "defects";
export const PILES: Pile[] = ["not_sure", "similar", "weaker", "defects"];
export const PILE_LABEL: Record<Pile, string> = { not_sure: "Not sure", similar: "Similar to a kept photo", weaker: "Weaker frames", defects: "Defects" };
export const PILE_HINT: Record<Pile, string> = {
  not_sure: "Close calls: one at a time, by moment",
  similar: "Set aside because a kept photo of the same moment looks alike: one row per moment next to the kept photo",
  weaker: "Good, but better frames were chosen. They stay unflagged",
  defects: "Closed eyes, missed focus, blur or your own set-aside. Defects are rejected when you apply",
};
/** Which pile a Not sure / Set aside row belongs to. */
export function pileOf(sel: { choice: string; reasons: { kind: string }[]; coveredSimilarity: number | null }): Pile {
  if (sel.choice === "not_sure") return "not_sure";
  const k = sel.reasons[0]?.kind;
  if (k === "defect" || k === "user_choice") return "defects";
  if (k === "near_duplicate" || k === "not_best_of_setup") return "similar";
  if ((sel.coveredSimilarity ?? 0) >= 0.7) return "similar";
  return "weaker";
}

/** True for a selection the user added / swapped in (reason `user_choice`). */
export const isUserAdded = (sel: { choice: string; reasons: { kind: string }[] } | undefined) => !!sel && sel.choice === "deliver" && sel.reasons.some((r) => r.kind === "user_choice");

// ---- Reviewed picks are locked at a re-run (P1-4) ----
/**
 * Locks picks the user approved by moving past them so a re-run keeps them (`Forced::Locked`). Uses the dedicated lock command
 * once the backend has it (it must not write XMP flags, which `set_target_choice` does); until then it does nothing.
 */
export async function lockReviewedPicks(ids: number[]): Promise<boolean> {
  if (ids.length === 0) return false;
  const fn = (commands as unknown as Record<string, ((ids: number[]) => Promise<unknown>) | undefined>).lockTargetChoices;
  if (!fn) return false;
  await unwrap(fn(ids) as Promise<never>);
  return true;
}
/** Keeps only the reviewed ids that are still in `keep` (after a re-run). */
export function pruneReviewed(projectId: number, pass: Pass, keep: Set<number>) {
  saveReviewed(projectId, pass, new Set([...loadReviewed(projectId, pass)].filter((i) => keep.has(i))));
}

/** Progress of a run for the Cull step row: picks reviewed, Not sure looked at, and the stage with unfinished work. */
export function runProgress(projectId: number, c: { deliver: number; notSure: number }): { picksDone: number; secondDone: number; next: "review" | "second" } {
  const picksDone = Math.min(c.deliver, loadReviewed(projectId, "review").size);
  const secondDone = Math.min(c.notSure, loadReviewed(projectId, "second").size);
  return { picksDone, secondDone, next: picksDone < c.deliver ? "review" : secondDone < c.notSure ? "second" : "review" };
}
