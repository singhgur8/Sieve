// Baseline edit (IPC v21) helpers: the session (preset + anchor) the user is building, the project's latest run, the
// anchor's offset from its own Auto (for the Develop bar) and the plain-language text for it.
import { useCallback, useEffect, useRef, useState } from "react";
import { commands, events, unwrap, type BaselineAnchor, type BaselineRun, type BaselineScope, type BaselineSettings, type HistoryEntry, type LightOffset, type LightValues, type ParametricAdjustments } from "../ipc";

/** What the user chose so far. `stage: "rest"` = they adjusted the anchor and pressed "Edit the rest". */
export interface BaselineSession {
  presetId: number | null;
  presetName: string | null;
  anchorId: number | null;
  stage: "setup" | "rest";
  /** `anchorId:presetId` the preset was applied to in Develop already (never applied twice over the user's edits). */
  appliedKey?: string;
  /** The preset applied to the anchor in this session (null = none), for replacing it when the choice changes. */
  appliedPresetId?: number | null;
  /** The preset's own light values (exposure .. blacks), added to the anchor's Auto when it starts from Auto. */
  presetLight?: Partial<Pick<LightValues, "exposure" | "contrast" | "highlights" | "shadows" | "whites" | "blacks">>;
}

export const EMPTY_SESSION: BaselineSession = { presetId: null, presetName: null, anchorId: null, stage: "setup" };

export function baselineSettings(anchorId: number, presetId: number | null, scope: BaselineScope = { kind: "keepers" }, replaceEdited = false): BaselineSettings {
  return { anchorId, presetId, scope, replaceEdited };
}

const sign = (n: number, digits = 0) => `${n > 0 ? "+" : n < 0 ? "-" : ""}${Math.abs(n).toFixed(digits)}`;

/**
 * "+0.3 EV, warmer than Auto" for the anchor's offset from its own Auto (what carries to every other photo).
 * Negative mireds = warmer. Small differences are left out; nothing left = "same as Auto".
 */
export function describeOffset(o: LightOffset | null | undefined): string {
  if (!o) return "";
  const parts: string[] = [];
  const ev = Math.round(o.exposure * 10) / 10;
  if (ev !== 0) parts.push(`${sign(ev, 1)} EV`);
  if (Math.abs(o.temperatureMired) >= 4) parts.push(`${o.temperatureMired < 0 ? "warmer" : "cooler"} than Auto`);
  if (Math.abs(o.tint) >= 3) parts.push(`${o.tint > 0 ? "more magenta" : "more green"}`);
  const others: [string, number][] = [
    ["contrast", o.contrast],
    ["highlights", o.highlights],
    ["shadows", o.shadows],
    ["whites", o.whites],
    ["blacks", o.blacks],
  ];
  others
    .filter(([, v]) => Math.abs(v) >= 5)
    .sort((a, b) => Math.abs(b[1]) - Math.abs(a[1]))
    .slice(0, 2)
    .forEach(([n, v]) => parts.push(`${n} ${sign(Math.round(v))}`));
  return parts.length > 0 ? parts.join(", ") : "same as Auto";
}

/** "Undone for 41 photos. DSC00011.ARW keeps your change." when some photos kept a later edit. */
export function undoneText(kept: string[], total: number): string {
  if (kept.length === 0) return "Undone. The photos are back to how they were.";
  const n = Math.max(0, total - kept.length);
  const names = kept.slice(0, 2).join(", ") + (kept.length > 2 ? ` and ${kept.length - 2} more` : "");
  return `Undone${n > 0 ? ` for ${n} photo${n === 1 ? "" : "s"}` : ""}. ${names} ${kept.length === 1 ? "keeps" : "keep"} your change.`;
}

/** After "Undo the rest": the photos that kept the user's later change (the run's batch reads undone, these stay user-edited). */
export function useKeptAfterUndo(projectId: number | null, run: BaselineRun | null): number[] {
  const [ids, setIds] = useState<number[]>([]);
  const undone = run?.state === "finished" && run.batch?.undoneAtMs != null;
  const runId = run?.id;
  useEffect(() => {
    if (projectId == null || !undone) return void setIds([]);
    let dead = false;
    void (async () => {
      try {
        const all = await unwrap(commands.getBaselineResults(projectId, null));
        const prov = await unwrap(commands.getBaselineProvenance(all.map((x) => x.imageId)));
        if (!dead) setIds(prov.filter((x) => x.state === "user_edited").map((x) => x.imageId));
      } catch {
        if (!dead) setIds([]);
      }
    })();
    return () => {
      dead = true;
    };
  }, [projectId, undone, runId]);
  return ids;
}

/** The project's latest baseline run, kept fresh by `baseline-run-finished`. */
export function useBaselineRun(projectId: number | null) {
  const [run, setRun] = useState<BaselineRun | null>(null);
  const [loaded, setLoaded] = useState(false);
  const pid = useRef(projectId);
  pid.current = projectId;
  const refresh = useCallback(async () => {
    if (projectId == null) return setRun(null);
    try {
      const r = await unwrap(commands.getBaselineRun(projectId));
      if (pid.current === projectId) setRun(r);
    } catch {
      /* no run info is not an error for the Plan */
    } finally {
      setLoaded(true);
    }
  }, [projectId]);
  useEffect(() => {
    setRun(null);
    setLoaded(false);
    void refresh();
  }, [refresh]);
  // The run's batch can be undone from elsewhere (Cmd+Z in the Plan, a toast, the scene menu): read it again on focus.
  useEffect(() => {
    const f = () => void refresh();
    window.addEventListener("focus", f);
    return () => window.removeEventListener("focus", f);
  }, [refresh]);
  useEffect(() => {
    let off: (() => void) | undefined;
    let dead = false;
    void events.baselineRunFinished.listen((e) => {
      if (e.payload.run.projectId === pid.current) setRun(e.payload.run);
    }).then((u) => (dead ? u() : (off = u)));
    return () => {
      dead = true;
      off?.();
    };
  }, []);
  return { run, setRun, refresh, loaded };
}

/** The anchor's measured light and its offset from Auto (`preview_baseline` with one sample), re-read after edits. */
export function useAnchorOffset(projectId: number | null, anchorId: number | null, presetId: number | null, tick: number) {
  const [anchor, setAnchor] = useState<BaselineAnchor | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (projectId == null || anchorId == null) {
      setAnchor(null);
      return;
    }
    let stale = false;
    const t = setTimeout(() => {
      unwrap(commands.previewBaseline(projectId, baselineSettings(anchorId, presetId), { sampleCount: 1, imageIds: null }))
        .then((p) => {
          if (stale) return;
          setAnchor(p.anchor);
          setError(null);
        })
        .catch((e) => !stale && setError(String((e as { message?: string })?.message ?? e)));
    }, 400);
    return () => {
      stale = true;
      clearTimeout(t);
    };
  }, [projectId, anchorId, presetId, tick]);
  return { anchor, error };
}

const LIGHT_SIMPLE = ["exposure", "contrast", "highlights", "shadows", "whites", "blacks"] as const;
const clampTo = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

export const START_LABEL = "Baseline: start from Auto";
const OWN_ENTRY = /^(Original|Preset:|Baseline)/;
const LIGHT_LABEL = /^(Exposure|Contrast|Highlights|Shadows|Whites|Blacks|Temp|Tint|White Balance|Auto|Reset Basic)/i;

/** What the user changed by hand (every entry that is not the original, a preset or a baseline step). */
export const userEntries = (entries: HistoryEntry[]) => entries.filter((e) => !OWN_ENTRY.test(e.label));
/** The user set light by hand: it must never be replaced by Auto. */
export const lightEditedByUser = (entries: HistoryEntry[]) => userEntries(entries).length > 0;
/** The user changed colour / look settings by hand (light edits do not count). */
export const lookEditedAfterPreset = (entries: HistoryEntry[]) => userEntries(entries).some((e) => !LIGHT_LABEL.test(e.label));

/**
 * True when the anchor's light is still untouched and has not started from Auto since its last preset: it may start
 * from Auto now. Never overwrites light the user set by hand.
 */
export function anchorLightUntouched(entries: HistoryEntry[]): boolean {
  if (lightEditedByUser(entries)) return false;
  let last = -1;
  entries.forEach((e, i) => e.label.startsWith("Preset:") && (last = i));
  return !entries.slice(last + 1).some((e) => e.label === START_LABEL);
}

/** The light the anchor starts from: its own Auto plus the preset's light values (a preset white balance stays absolute). */
export function startLight(auto: LightValues, presetLight: BaselineSession["presetLight"], keepWb: ParametricAdjustments["whiteBalance"] | null): LightValues {
  const out: LightValues = { ...auto };
  for (const k of LIGHT_SIMPLE) out[k] = clampTo(Math.round((auto[k] + (presetLight?.[k] ?? 0)) * 100) / 100, k === "exposure" ? -5 : -100, k === "exposure" ? 5 : 100);
  if (keepWb && keepWb.mode === "custom") {
    out.temperatureK = keepWb.temperatureK;
    out.tint = keepWb.tint;
  }
  return out;
}

/** The preset's own light values: what its apply left on the (untouched) anchor in the six simple light sliders. */
export function lightOfAdjustments(a: ParametricAdjustments): NonNullable<BaselineSession["presetLight"]> {
  const o: NonNullable<BaselineSession["presetLight"]> = {};
  for (const k of LIGHT_SIMPLE) o[k] = a[k];
  return o;
}

/**
 * Step 3 opens: the anchor's light starts at its own Auto (+ the preset's light values), so "same as Auto" is the
 * zero point. Returns the preset's light values for the "Start from Auto" button. One history entry.
 */
export async function startAnchorFromAuto(_projectId: number, anchorId: number, _presetId: number | null, presetHasWb: boolean): Promise<NonNullable<BaselineSession["presetLight"]> | null> {
  const h = await unwrap(commands.getHistory(anchorId));
  const cur = await unwrap(commands.getAdjustments(anchorId));
  const presetLight = lightOfAdjustments(cur);
  if (!anchorLightUntouched(h.entries)) return null;
  // v21.1 `auto_light`: the same numbers the engine's meter gives (`BaselineAnchor.auto`).
  const { light } = await unwrap(commands.autoLight(anchorId, cur));
  const l = startLight(light, presetLight, presetHasWb ? cur.whiteBalance : null);
  await unwrap(
    commands.saveAdjustments(
      anchorId,
      { ...cur, exposure: l.exposure, contrast: l.contrast, highlights: l.highlights, shadows: l.shadows, whites: l.whites, blacks: l.blacks, whiteBalance: { mode: "custom", temperatureK: l.temperatureK, tint: l.tint } },
      START_LABEL,
    ),
  );
  return presetLight;
}

/**
 * `undo_edit_batch(batchId, { keepLaterEdits })` (architect, pending) restores the photos still on the batch's entry and
 * leaves the ones edited since. The generated wrapper takes a second argument once the contract has it; until then the
 * "Undo the rest" button is not offered.
 */
export const KEEP_LATER_SUPPORTED = true;
