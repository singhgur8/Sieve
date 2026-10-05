// Edit Capture Time (Lightroom's Metadata > Edit Capture Time): shift the selection by h/m/s, set one photo's time exactly
// (the others move by the same amount) or sync two cameras (pick one frame of each that happened at the same moment).
// The original file is never touched; the corrected time goes to the catalog and the XMP sidecar.
import { useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { commands, unwrap, type CaptureTimeEdit, type RawImageEntry } from "../ipc";
import { Dialog } from "./Dialog";
import { HelpLink } from "./HelpLink";
import { formatTime } from "../lib/format";
import { formatOffset, fromInputValue, offsetFrom, toInputValue } from "../lib/captureTime";
import { cameraLabel } from "../lib/metaFilter";
import { Thumb } from "./edit/bits";

type Tab = "shift" | "set" | "sync" | "revert";

interface Props {
  /** Photos the edit applies to (the selection, or the active photo). */
  targetIds: number[];
  activeId: number | null;
  /** Every photo in the current view (the sync tab shifts every frame of the other camera in it). */
  viewIds: number[];
  onApply: (ids: number[], mode: CaptureTimeEdit, label: string) => Promise<void>;
  onCancel: () => void;
}

const camName = (e: RawImageEntry) => cameraLabel(e.camera);
const field = "w-16 rounded border border-neutral-700 bg-neutral-950 px-1.5 py-1 text-right text-neutral-100";
const sel = "w-full rounded border border-neutral-700 bg-neutral-950 px-1.5 py-1 text-neutral-100";

/** Searchable, virtualized list of one camera's frames (no cap) with the chosen frame's thumbnail and time underneath. */
function FramePicker({ frames, value, onChange, testid, entries }: { frames: RawImageEntry[]; value: number | null; onChange: (id: number) => void; testid: string; entries: Map<number, RawImageEntry> }) {
  const [q, setQ] = useState("");
  const box = useRef<HTMLDivElement>(null);
  const list = useMemo(() => {
    const t = q.trim().toLowerCase();
    return t ? frames.filter((e) => `${e.fileName} ${formatTime(e.capture.capturedAtMs as number)}`.toLowerCase().includes(t)) : frames;
  }, [frames, q]);
  const v = useVirtualizer({ count: list.length, getScrollElement: () => box.current, estimateSize: () => 24, overscan: 6 });
  const chosen = value != null ? entries.get(value) : undefined;
  return (
    <div className="flex flex-col gap-1" data-testid={testid}>
      <input className={sel} placeholder={`Search ${frames.length.toLocaleString()} frames by name or time…`} value={q} onChange={(e) => setQ(e.target.value)} aria-label="Search frames" data-testid={`${testid}-search`} />
      <div ref={box} className="h-28 overflow-y-auto rounded border border-neutral-700 bg-neutral-950" role="listbox" aria-label="Frames" data-testid={`${testid}-list`} data-count={list.length}>
        <div className="relative w-full" style={{ height: v.getTotalSize() }}>
          {v.getVirtualItems().map((r) => {
            const e = list[r.index];
            const on = e.id === value;
            return (
              <button
                key={e.id}
                role="option"
                aria-selected={on}
                className={`absolute left-0 flex h-6 w-full items-center justify-between gap-2 px-1.5 text-left ${on ? "bg-sky-800 text-sky-50" : "text-neutral-300 hover:bg-neutral-800"}`}
                style={{ top: r.start }}
                onClick={() => onChange(e.id)}
                data-testid={`${testid}-opt-${e.id}`}
              >
                <span className="truncate">{e.fileName}</span>
                <span className="shrink-0 tabular-nums text-neutral-400">{formatTime(e.capture.capturedAtMs as number)}</span>
              </button>
            );
          })}
        </div>
        {list.length === 0 && <p className="p-2 text-neutral-500">No frame matches.</p>}
      </div>
      {chosen ? (
        <div className="flex items-center gap-2" data-testid={`${testid}-chosen`} data-id={chosen.id}>
          <Thumb entry={chosen} className="h-20 w-[120px] shrink-0 rounded" />
          <div className="min-w-0 text-neutral-300">
            <div className="truncate font-medium">{chosen.fileName}</div>
            <div className="tabular-nums text-neutral-400" data-testid={`${testid}-time`}>
              {formatTime(chosen.capture.capturedAtMs as number)}
            </div>
          </div>
        </div>
      ) : (
        <p className="text-neutral-500" data-testid={`${testid}-empty`}>
          Choose a frame…
        </p>
      )}
    </div>
  );
}

export function CaptureTimeDialog({ targetIds, activeId, viewIds, onApply, onCancel }: Props) {
  const [tab, setTab] = useState<Tab>("shift");
  const [entries, setEntries] = useState<Map<number, RawImageEntry> | null>(null);
  const [busy, setBusy] = useState(false);
  // shift
  const [sign, setSign] = useState<1 | -1>(-1);
  const [h, setH] = useState(1);
  const [m, setM] = useState(0);
  const [s, setS] = useState(0);
  // set exact
  const [exact, setExact] = useState("");
  // sync
  const [refCam, setRefCam] = useState("");
  const [tgtCam, setTgtCam] = useState("");
  const [refFrame, setRefFrame] = useState<number | null>(null);
  const [tgtFrame, setTgtFrame] = useState<number | null>(null);

  const loadIds = useMemo(() => [...new Set([...targetIds, ...viewIds])], [targetIds, viewIds]);
  useEffect(() => {
    let live = true;
    (async () => {
      const out = new Map<number, RawImageEntry>();
      for (let i = 0; i < loadIds.length; i += 200) {
        try {
          for (const e of await unwrap(commands.getImages(loadIds.slice(i, i + 200)))) out.set(e.id, e);
        } catch {
          break;
        }
        if (!live) return;
      }
      if (live) setEntries(out);
    })();
    return () => {
      live = false;
    };
  }, [loadIds]);

  const timeOf = (id: number | null) => (id != null ? (entries?.get(id)?.capture.capturedAtMs ?? null) : null);
  const reference = activeId != null && targetIds.includes(activeId) ? activeId : targetIds[0] ?? null;
  const refTime = timeOf(reference);
  useEffect(() => {
    if (exact === "" && refTime != null) setExact(toInputValue(refTime));
  }, [refTime, exact]);

  // ---- cameras (sync) ----
  const cameras = useMemo(() => {
    const by = new Map<string, RawImageEntry[]>();
    for (const id of viewIds) {
      const e = entries?.get(id);
      if (e && e.capture.capturedAtMs != null) by.set(camName(e), [...(by.get(camName(e)) ?? []), e]);
    }
    return by;
  }, [entries, viewIds]);
  const camNames = [...cameras.keys()];
  // Exactly one selected frame from each of two cameras: that is the pair ("this is the same moment"). The active photo's camera
  // is the one to fix (it is the last one you clicked); the other is the reference.
  const pair = useMemo(() => {
    if (!entries || targetIds.length !== 2) return null;
    const a = entries.get(targetIds[0]);
    const b = entries.get(targetIds[1]);
    if (!a || !b || a.capture.capturedAtMs == null || b.capture.capturedAtMs == null || camName(a) === camName(b)) return null;
    const fix = activeId === a.id ? a : b;
    return { fix, ref: fix === a ? b : a };
  }, [entries, targetIds, activeId]);
  const openedOnSync = useRef(false);
  useEffect(() => {
    if (!pair || openedOnSync.current) return;
    openedOnSync.current = true;
    setTab("sync");
    setTgtCam(camName(pair.fix));
    setRefCam(camName(pair.ref));
    setTgtFrame(pair.fix.id);
    setRefFrame(pair.ref.id);
  }, [pair]);
  useEffect(() => {
    if (camNames.length < 2 || (refCam && tgtCam) || pair) return;
    // The camera of the selected photos is the one to fix; the other one is the reference. No frames are pre-picked.
    const selCam = targetIds.map((i) => entries?.get(i)).find(Boolean);
    const fix = selCam ? camName(selCam) : camNames[1];
    setTgtCam(fix);
    setRefCam(camNames.find((c) => c !== fix) ?? camNames[0]);
  }, [camNames.join("|"), targetIds, entries, pair]); // eslint-disable-line react-hooks/exhaustive-deps
  const framesOf = (cam: string) =>
    [...(cameras.get(cam) ?? [])].sort((a, b) => (a.capture.capturedAtMs ?? 0) - (b.capture.capturedAtMs ?? 0));
  // An explicit pick only: nothing is guessed (an arbitrary pair would shift a whole camera by a meaningless offset).
  const pickFrame = (cam: string, cur: number | null) => (cur != null && framesOf(cam).some((e) => e.id === cur) ? cur : null);
  const refId = pickFrame(refCam, refFrame);
  const tgtId = pickFrame(tgtCam, tgtFrame);
  const syncOffset = timeOf(refId) != null && timeOf(tgtId) != null ? (timeOf(refId) as number) - (timeOf(tgtId) as number) : null;
  // Scope seam (IPC v19.2 adds project-wide and per-body scopes): today the frames of the camera that are in the current view.
  const syncScope = { kind: "view" as const, label: "in the current view" };
  const syncTargets = viewIds.filter((id) => entries?.get(id) && camName(entries.get(id)!) === tgtCam && entries.get(id)!.capture.capturedAtMs != null);

  // ---- what each tab would do ----
  const shiftOffset = offsetFrom(sign, h, m, s);
  const exactMs = fromInputValue(exact);
  const exactOffset = exactMs != null && refTime != null ? exactMs - refTime : null;
  const plan: { ids: number[]; offset: number | null; mode: CaptureTimeEdit | null; label: string } =
    tab === "shift"
      ? { ids: targetIds, offset: shiftOffset, mode: shiftOffset !== 0 ? { kind: "shift", offsetMs: shiftOffset } : null, label: `Shifted capture time ${formatOffset(shiftOffset)}` }
      : tab === "set"
        ? {
            ids: reference != null && !targetIds.includes(reference) ? [reference, ...targetIds] : targetIds,
            offset: exactOffset,
            mode: reference != null && exactMs != null ? { kind: "set_exact", referenceId: reference, capturedAtMs: exactMs } : null,
            label: "Set capture time",
          }
        : tab === "sync"
          ? {
              ids: syncTargets,
              offset: syncOffset,
              mode: refId != null && tgtId != null && syncOffset != null && syncOffset !== 0 ? { kind: "sync_cameras", referenceId: refId, targetId: tgtId } : null,
              label: `Synced ${tgtCam} to ${refCam} (${syncOffset != null ? formatOffset(syncOffset) : ""})`,
            }
          : { ids: targetIds, offset: null, mode: { kind: "revert" }, label: "Reverted capture time to original" };

  const preview = plan.ids
    .map((id) => entries?.get(id))
    .filter((e): e is RawImageEntry => !!e && e.capture.capturedAtMs != null)
    .slice(0, 4)
    .map((e) => {
      const before = e.capture.capturedAtMs as number;
      const after = tab === "revert" ? (e.capture.originalCapturedAtMs ?? before) : plan.offset != null ? before + plan.offset : before;
      return { id: e.id, name: e.fileName, before, after };
    });
  const changeCount = tab === "revert" ? plan.ids.filter((id) => (entries?.get(id)?.capture.captureTimeSource ?? "exif") !== "exif").length : plan.offset === 0 || plan.offset == null ? 0 : plan.ids.length;

  const apply = async () => {
    if (!plan.mode || busy) return;
    setBusy(true);
    try {
      await onApply(plan.ids, plan.mode, plan.label);
    } finally {
      setBusy(false);
    }
  };

  const tabBtn = (t: Tab, label: string) => (
    <button
      key={t}
      role="tab"
      aria-selected={tab === t}
      data-testid={`capture-tab-${t}`}
      onClick={() => setTab(t)}
      className={`rounded px-2.5 py-1 ${tab === t ? "bg-sky-800 text-sky-100" : "bg-neutral-800 text-neutral-300 hover:bg-neutral-700"}`}
    >
      {label}
    </button>
  );
  const numField = (v: number, set: (n: number) => void, label: string, unit: string, tid: string, max?: number) => (
    <label className="flex items-center gap-1 text-neutral-400">
      <input type="number" min={0} max={max} className={field} value={v} aria-label={label} data-testid={tid} onChange={(e) => set(Math.max(0, Math.floor(Number(e.target.value) || 0)))} />
      {unit}
    </label>
  );
  return (
    <Dialog label="Edit capture time" testid="capture-time-dialog" className="flex max-h-[90vh] w-[560px] flex-col gap-3 overflow-y-auto rounded-lg border border-neutral-700 bg-neutral-900 p-5 text-sm" onCancel={onCancel}>
      <h2 className="text-base font-semibold text-neutral-100">
        Edit capture time <HelpLink id="capture-time" title="How does this work?" />
      </h2>
      <p className="text-xs text-neutral-400">
        Fixes a camera clock that was off (wrong time zone, daylight saving, two cameras out of step). Sorting, bursts and scenes use the corrected time. The original files are never changed; the correction is saved in the XMP sidecars and can be reverted.
      </p>
      <div role="tablist" className="flex flex-wrap gap-1 text-xs">
        {tabBtn("shift", "Shift by hours / minutes")}
        {tabBtn("set", "Set exact time")}
        {tabBtn("sync", "Sync two cameras")}
        {tabBtn("revert", "Revert to original")}
      </div>

      {!entries && <p className="text-xs text-neutral-400">Loading photos…</p>}

      {entries && tab === "shift" && (
        <div className="flex flex-col gap-2" data-testid="capture-pane-shift">
          <p className="text-xs text-neutral-400">Moves {targetIds.length === 1 ? "this photo" : `all ${targetIds.length} selected photos`} by the same amount.</p>
          <div className="flex flex-wrap items-center gap-3 text-xs">
            <select className="rounded border border-neutral-700 bg-neutral-950 px-1.5 py-1 text-neutral-100" value={sign} onChange={(e) => setSign(Number(e.target.value) as 1 | -1)} data-testid="capture-sign" aria-label="Direction">
              <option value={1}>Later (+)</option>
              <option value={-1}>Earlier (-)</option>
            </select>
            {numField(h, setH, "Hours", "h", "capture-h")}
            {numField(m, setM, "Minutes", "min", "capture-m", 59)}
            {numField(s, setS, "Seconds", "s", "capture-s", 59)}
          </div>
        </div>
      )}

      {entries && tab === "set" && (
        <div className="flex flex-col gap-2" data-testid="capture-pane-set">
          <p className="text-xs text-neutral-400">
            Give <b className="text-neutral-200">{reference != null ? (entries.get(reference)?.fileName ?? `#${reference}`) : "the photo"}</b> its true time{targetIds.length > 1 ? "; the other selected photos move by the same amount" : ""}.
          </p>
          <input type="datetime-local" step={1} className="w-fit rounded border border-neutral-700 bg-neutral-950 px-2 py-1 text-neutral-100" value={exact} onChange={(e) => setExact(e.target.value)} data-testid="capture-exact" aria-label="New capture time" />
          {refTime == null && <p className="text-xs text-amber-300">This photo has no capture time in the file; it simply gets this one.</p>}
        </div>
      )}

      {entries && tab === "sync" && (
        <div className="flex flex-col gap-2 text-xs" data-testid="capture-pane-sync">
          {camNames.length < 2 ? (
            <p className="text-amber-300" data-testid="capture-sync-one-camera">
              Only one camera with capture times is in view, so there is nothing to sync. Clear the filters or open a project with photos from both cameras.
            </p>
          ) : (
            <>
              <p className="text-neutral-400">Pick one frame from each camera that was taken at the same moment (a shared clap, a group shot, or a burst fired together). Every photo of the camera to fix moves so its frame lands on the other camera&apos;s time. Check the two thumbnails show the same moment.</p>
              <div className="grid grid-cols-2 gap-3">
                <div className="flex flex-col gap-1">
                  <span className="font-medium text-neutral-300">Clock is right</span>
                  <select className={sel} value={refCam} onChange={(e) => (setRefCam(e.target.value), setRefFrame(null))} data-testid="capture-ref-cam" aria-label="Camera with the right clock">
                    {camNames.map((c) => (
                      <option key={c}>{c}</option>
                    ))}
                  </select>
                  {entries && <FramePicker frames={framesOf(refCam)} value={refId} onChange={setRefFrame} testid="capture-ref-frame" entries={entries} />}
                </div>
                <div className="flex flex-col gap-1">
                  <span className="font-medium text-neutral-300">Clock to fix (these photos move)</span>
                  <select className={sel} value={tgtCam} onChange={(e) => (setTgtCam(e.target.value), setTgtFrame(null))} data-testid="capture-tgt-cam" aria-label="Camera to correct">
                    {camNames.map((c) => (
                      <option key={c}>{c}</option>
                    ))}
                  </select>
                  {entries && <FramePicker frames={framesOf(tgtCam)} value={tgtId} onChange={setTgtFrame} testid="capture-tgt-frame" entries={entries} />}
                </div>
              </div>
              {refCam === tgtCam && <p className="text-amber-300">Choose two different cameras.</p>}
              {(refId == null || tgtId == null) && (
                <p className="text-amber-300" data-testid="capture-sync-hint">
                  {refId == null && tgtId == null ? "Choose a frame from each camera." : "Choose the other frame."} Tip: Cmd-click one photo from each camera in the grid, then press Cmd+Shift+T.
                </p>
              )}
              <p className="text-neutral-400" data-testid="capture-scope" data-scope={syncScope.kind}>
                Scope: every {tgtCam} photo {syncScope.label} ({syncTargets.length.toLocaleString()}) moves. Photos hidden by the current filters are not changed.
              </p>
            </>
          )}
        </div>
      )}

      {entries && tab === "revert" && <p className="text-xs text-neutral-400" data-testid="capture-pane-revert">Puts {targetIds.length === 1 ? "this photo" : `the ${targetIds.length} selected photos`} back to the time stored in the file.</p>}

      {entries && (
        <div className="rounded border border-neutral-800 bg-neutral-950 p-2 text-xs" data-testid="capture-preview" data-count={changeCount} data-offset={plan.offset ?? ""}>
          <div className="mb-1 text-neutral-300" data-testid="capture-summary">
            {tab === "sync" && camNames.length < 2
              ? "Nothing to change"
              : changeCount === 0
                ? "No photos would change"
                : `${changeCount} ${changeCount === 1 ? "photo" : "photos"} will change${plan.offset != null ? ` (${formatOffset(plan.offset)})` : ""}${tab === "sync" ? `: all ${syncTargets.length.toLocaleString()} ${tgtCam} photos ${syncScope.label}` : ""}`}
          </div>
          {changeCount > 0 &&
            preview.map((p) => (
              <div key={p.id} className="flex gap-2 tabular-nums text-neutral-400" data-testid={`capture-preview-${p.id}`}>
                <span className="w-28 truncate text-neutral-300">{p.name}</span>
                <span>{formatTime(p.before)}</span>
                <span>→</span>
                <span className="text-sky-300">{formatTime(p.after)}</span>
              </div>
            ))}
          {changeCount > preview.length && <div className="text-neutral-500">…and {changeCount - preview.length} more</div>}
        </div>
      )}

      <div className="flex justify-end gap-2">
        <button onClick={onCancel} className="rounded bg-neutral-800 px-3 py-1.5 hover:bg-neutral-700" data-testid="capture-cancel">
          Cancel
        </button>
        <button
          onClick={() => void apply()}
          disabled={!entries || busy || !plan.mode || (tab === "sync" && (camNames.length < 2 || refCam === tgtCam)) || (tab !== "revert" && changeCount === 0)}
          className="rounded bg-emerald-700 px-3 py-1.5 font-medium text-white hover:bg-emerald-600 disabled:opacity-40"
          data-testid="capture-apply"
        >
          {tab === "revert" ? "Revert" : "Apply"}
        </button>
      </div>
    </Dialog>
  );
}
