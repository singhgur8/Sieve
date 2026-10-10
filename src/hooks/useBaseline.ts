// Baseline edit (IPC v21) helpers: the session (preset + anchor) the user is building, the project's latest run, the
// anchor's offset from its own Auto (for the Develop bar) and the plain-language text for it.
import { useCallback, useEffect, useRef, useState } from "react";
import { commands, events, unwrap, type BaselineAnchor, type BaselineRun, type BaselineScope, type BaselineSettings, type LightOffset } from "../ipc";

/** What the user chose so far. `stage: "rest"` = they adjusted the anchor and pressed "Edit the rest". */
export interface BaselineSession {
  presetId: number | null;
  presetName: string | null;
  anchorId: number | null;
  stage: "setup" | "rest";
  /** `anchorId:presetId` the preset was applied to in Develop already (never applied twice over the user's edits). */
  appliedKey?: string;
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
  if (Math.abs(o.exposure) >= 0.05) parts.push(`${sign(Math.round(o.exposure * 10) / 10, 1)} EV`);
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
