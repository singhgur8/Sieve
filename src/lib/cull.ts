// Plain-language descriptions of culling state: the keeper formula, why a photo is rejected / tagged, sidecar names.
import type { CullSummary, CullTag, RawImageEntry } from "../ipc";
import { tagName } from "./format";

/** Words for each tag when the engine gave no reason text of its own. */
export const TAG_MEANING: Record<CullTag, string> = {
  blink: "Eyes closed",
  missed_focus: "The face or subject is not sharp",
  motion_blur: "Blurred by camera or subject movement",
  creative_blur: "Intentional blur (panning, bokeh). Never rejected on its own",
  underexposed: "Too dark",
  overexposed: "Highlights are blown out",
  duplicate_burst: "A better frame of the same burst exists",
};

/** The pieces of "Keepers 412 = 120 picked + 292 unflagged"; zero parts are dropped except the first. */
export function keeperParts(s: CullSummary): { label: string; count: number }[] {
  const b = s.keeperBreakdown;
  if (s.keeperRule.mode === "not_rejected")
    return [
      { label: "picked", count: b.picked },
      { label: "unflagged", count: b.unflagged },
    ];
  return [
    { label: "picked", count: b.picked },
    { label: `rated ${s.keeperRule.minRating}★+`, count: b.starred },
    { label: "suggested by Sieve", count: b.suggested },
  ].filter((p, i) => i === 0 || p.count > 0);
}

/** "= 120 picked + 292 unflagged · 273 rejected are left out" */
export function keeperEquation(s: CullSummary): string {
  const parts = keeperParts(s)
    .map((p) => `${p.count} ${p.label}`)
    .join(" + ");
  const out = s.total - s.keepers;
  const what = s.keeperRule.mode === "not_rejected" ? "rejected" : "others";
  return `= ${parts} · ${out} ${what} ${out === 1 ? "is" : "are"} left out`;
}

/** "Keepers 412 = 120 picked + 292 unflagged · 273 rejected are left out" */
export function keeperFormula(s: CullSummary): string {
  return `Keepers ${s.keepers} ${keeperEquation(s)}`;
}

/** Hover text for every place that says how many photos go on to Edit / Export. */
export function keeperTitle(s: CullSummary | null | undefined, lead: string): string {
  return s ? `${lead}. ${keeperFormula(s)}. Change the keeper rule in the Cull summary bar, the Edit plan or the Export dialog.` : lead;
}

/** Texts of the engine's reasons for this photo ("Eyes closed", "Duplicate in burst (best DSC0123)"). */
export function reasonTexts(e: RawImageEntry): string[] {
  return (e.quality?.reasons ?? []).map((r) => r.text).filter(Boolean);
}

/** The engine's reason for one tag, or the generic meaning of the tag. */
export function tagReason(e: RawImageEntry, tag: CullTag): string {
  const r = e.quality?.reasons?.find((x) => x.kind === tag);
  return r?.text ?? TAG_MEANING[tag];
}

export const tagTitle = (e: RawImageEntry, tag: CullTag) => `${tagName(tag)}: ${tagReason(e, tag)}`;

/** Reason kinds that are notes, not defects (never a headline). */
const NOTE_KINDS = new Set(["creative_blur"]);

/** The reason to lead with: the first defect, skipping notes such as intentional blur. */
export function headlineReason(e: RawImageEntry): string | null {
  const rs = e.quality?.reasons ?? [];
  return (rs.find((r) => r.text && !NOTE_KINDS.has(r.kind)) ?? null)?.text ?? null;
}

export interface RejectInfo {
  /** "Rejected by you" / "Auto-rejected". */
  origin: string;
  who: "user" | "auto";
  reasons: string[];
  /** First defect reason (notes skipped), or null. */
  headline: string | null;
  /** Joins the origin and the reasons: "Rejected by you · Sieve noted: …" / "Auto-rejected · …". */
  lead: string;
}

/** Why a rejected photo is rejected: who did it and the engine's reasons. Null when not rejected. */
export function rejectInfo(e: RawImageEntry): RejectInfo | null {
  if (e.pick !== "reject") return null;
  const auto = e.pickOrigin === "auto";
  return { origin: auto ? "Auto-rejected" : "Rejected by you", who: auto ? "auto" : "user", reasons: reasonTexts(e), headline: headlineReason(e), lead: auto ? " · " : " · Sieve noted: " };
}

/** "Sieve suggests reject · Eyes closed" for an unflagged photo the engine would reject (not applied). */
export function suggestedReject(e: RawImageEntry): { lead: string; reason: string } | null {
  if (e.pick !== "unflagged" || e.quality?.suggestedPick !== "reject") return null;
  return { lead: "Sieve suggests reject", reason: headlineReason(e) ?? "Low quality score" };
}

const NON_RAW_EXT = new Set(["jpg", "jpeg", "tif", "tiff", "png", "heic", "heif", "webp"]);

/** DSC0001.ARW -> DSC0001.xmp; JPEG/TIFF/PNG/HEIC keep their extension: DSC0001.JPG -> DSC0001.JPG.xmp (mirrors `xmp::sidecar_path`). */
export function sidecarName(fileName: string): string {
  const ext = /\.([^./]+)$/.exec(fileName)?.[1]?.toLowerCase();
  return ext && NON_RAW_EXT.has(ext) ? fileName + ".xmp" : fileName.replace(/\.[^./]+$/, "") + ".xmp";
}

/** Hover text of the flag icon of a photo. */
export function flagTitle(e: RawImageEntry): string {
  if (e.pick === "pick") return e.pickOrigin === "auto" ? "Picked automatically (Apply suggestions). Z keeps it, U unflags" : "Picked by you. U unflags";
  const r = rejectInfo(e);
  if (!r) return "Not flagged";
  return `${r.origin}${r.reasons.length ? `. ${r.reasons.join("; ")}` : ""}. U unflags`;
}
