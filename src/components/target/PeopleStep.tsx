// People of the shoot: "We think this is the couple" (confirm or change who), then "Is this person important?" per recurring
// person with big cards (Y / N, arrows), then "Re-run with these people".
import { forwardRef, useCallback, useEffect, useImperativeHandle, useRef, useState } from "react";
import { Check, Heart, Users } from "lucide-react";
import { commands, convertFileSrc, faceCropStyle, unwrap, type PeopleOverview, type Person, type PersonRole } from "../../ipc";
import { plural } from "../../lib/target";
import { hint } from "../../lib/keymap";
import { ActionButton, Key } from "./bits";
import type { StageRef } from "./SetupStep";
import type { TargetCtx } from "./types";

function Face({ person, size = "size-full", index = 0 }: { person: Person; size?: string; index?: number }) {
  const s = person.samples[index];
  if (!s) return <div className={`${size} flex items-center justify-center bg-neutral-800 text-neutral-500`}><Users className="size-6" /></div>;
  return <div className={`${size} rounded bg-neutral-800`} style={faceCropStyle(s, s.previewPath ? convertFileSrc(s.previewPath) : null)} role="img" aria-label={`Face of person ${person.id}`} />;
}

const roleText = (p: Person) => (p.role === "main" ? "Main" : p.roleConfirmed ? (p.role === "important" ? "Important" : "Not important") : "?");

export const PeopleStep = forwardRef<StageRef, { ctx: TargetCtx }>(function PeopleStep({ ctx }, ref) {
  const [ov, setOv] = useState<PeopleOverview | null>(null);
  const [focus, setFocus] = useState(0);
  const [changing, setChanging] = useState<number[] | null>(null);
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState(false);
  const stamp = ctx.run?.finishedAtMs ?? 0;
  const lastStamp = useRef(stamp);
  useEffect(() => {
    if (stamp !== lastStamp.current) {
      lastStamp.current = stamp;
      setDirty(false); // a run happened: its selection already used the answers
    }
  }, [stamp]);

  const load = useCallback(async () => {
    try {
      setOv(await unwrap(commands.listPeople(ctx.projectId)));
    } catch (e) {
      ctx.onError(e);
    }
  }, [ctx.projectId, ctx.onError]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    void load();
  }, [load, stamp]);

  const people = ov?.people ?? [];
  const mains = people.filter((p) => p.role === "main");
  // A stable order (most photos first, the asking order): answering must not shuffle the cards.
  const asked = people.filter((p) => p.ask && p.role !== "main").sort((a, b) => b.photoCount - a.photoCount || a.id - b.id);
  const coupleConfirmed = mains.length > 0 && mains.every((p) => p.roleConfirmed);
  const open = ov?.questions.length ?? 0;
  const cur = asked[Math.min(focus, Math.max(0, asked.length - 1))];

  const setRole = async (ids: number[], role: PersonRole) => {
    setBusy(true);
    try {
      for (const id of ids) await unwrap(commands.setPersonRole(id, role));
      setDirty(true);
      await load();
      await ctx.refreshRun();
    } catch (e) {
      ctx.onError(e);
    } finally {
      setBusy(false);
    }
  };
  const answer = async (role: "important" | "other") => {
    if (!cur || busy) return;
    const at = asked.findIndex((p) => p.id === cur.id);
    await setRole([cur.id], role);
    // On to the next card still waiting for an answer, else the next card.
    const next = asked.findIndex((p, i) => i > at && !p.roleConfirmed);
    setFocus(next >= 0 ? next : Math.min(at + 1, asked.length - 1));
  };
  const confirmCouple = () => void setRole(mains.map((p) => p.id), "main");
  const saveChange = async () => {
    if (!changing || changing.length === 0) return;
    const drop = mains.filter((p) => !changing.includes(p.id)).map((p) => p.id);
    await setRole(drop, "other");
    await setRole(changing, "main");
    setChanging(null);
  };
  const rerun = async () => {
    if (!ctx.run) return;
    setBusy(true);
    try {
      await unwrap(commands.runTargetSelection(ctx.projectId, ctx.run.settings));
      await ctx.refreshRun();
      ctx.go("setup");
    } catch (e) {
      ctx.onError(e);
    } finally {
      setBusy(false);
    }
  };

  useImperativeHandle(
    ref,
    () => ({
      key: (id) => {
        if (changing) {
          if (id === "targetClose") {
            setChanging(null);
            return true;
          }
          return false;
        }
        if (id === "targetPrev") return setFocus((f) => Math.max(0, f - 1)), true;
        if (id === "targetNext") return setFocus((f) => Math.min(asked.length - 1, f + 1)), true;
        if (id === "targetYes") {
          if (!coupleConfirmed && mains.length > 0) confirmCouple();
          else void answer("important");
          return true;
        }
        if (id === "targetNo") {
          if (coupleConfirmed || mains.length === 0) void answer("other");
          return true;
        }
        return false;
      },
    }),
    [changing, asked, coupleConfirmed, mains, cur, busy], // eslint-disable-line react-hooks/exhaustive-deps
  );

  if (!ov) return <div className="p-6 text-sm text-neutral-400" data-testid="target-people-loading">Loading people…</div>;
  if (ov.modelVersion == null || people.length === 0)
    return (
      <div className="mx-auto max-w-2xl p-6 text-sm text-neutral-300" data-testid="target-people-empty">
        <h2 className="mb-2 text-lg font-semibold text-neutral-50">People</h2>
        {ov.message ?? "No recurring people were found."} The selection works without them; photos are ranked by quality and moment only.
      </div>
    );

  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="target-people" data-open={open}>
      <div className="flex-1 overflow-y-auto p-6">
        <div className="mx-auto flex max-w-4xl flex-col gap-6">
          {mains.length > 0 && (
            <section className="rounded-lg border border-neutral-800 bg-neutral-900 p-4" data-testid="target-couple" data-confirmed={coupleConfirmed}>
              <h2 className="flex items-center gap-2 text-base font-semibold text-neutral-50">
                <Heart className="size-4 text-rose-400" /> {coupleConfirmed ? "The main people" : "We think this is the couple"}
              </h2>
              <p className="mt-1 text-xs text-neutral-400">
                {coupleConfirmed ? "Confirmed. Photos with them are favoured most." : "They appear most often, and together. Photos with them are favoured most."}
              </p>
              {changing ? (
                <div className="mt-3" data-testid="target-couple-change">
                  <p className="mb-2 text-sm text-neutral-200">Click who the main people are (up to two).</p>
                  <div className="grid grid-cols-6 gap-2">
                    {people.map((p) => {
                      const on = changing.includes(p.id);
                      return (
                        <button
                          key={p.id}
                          className={`relative overflow-hidden rounded ${on ? "ring-2 ring-rose-400" : "ring-1 ring-neutral-700 hover:ring-neutral-500"}`}
                          data-testid={`target-couple-pick-${p.id}`}
                          aria-pressed={on}
                          title={`${on ? "Remove" : "Add"} this person as one of the main people (${plural(p.photoCount, "photo")})`}
                          onClick={() => setChanging((c) => (c!.includes(p.id) ? c!.filter((x) => x !== p.id) : c!.length >= 2 ? [c![1], p.id] : [...c!, p.id]))}
                        >
                          <Face person={p} />
                          {on && <Check className="absolute right-1 top-1 size-4 rounded bg-rose-500 p-0.5 text-white" />}
                        </button>
                      );
                    })}
                  </div>
                  <div className="mt-3 flex gap-2">
                    <ActionButton testid="target-couple-save" label="Use these" keys={[]} tone="primary" disabled={changing.length === 0 || busy} title="Make the chosen people the main subject" onClick={() => void saveChange()} />
                    <ActionButton testid="target-couple-cancel" label="Cancel" keys={["Esc"]} title="Keep the people we found" onClick={() => setChanging(null)} />
                  </div>
                </div>
              ) : (
                <div className="mt-3 flex items-center gap-4">
                  <div className="flex gap-3">
                    {mains.map((p) => (
                      <figure key={p.id} className="w-28" data-testid={`target-main-${p.id}`}>
                        <Face person={p} />
                        <figcaption className="mt-1 text-center text-[11px] text-neutral-400">{plural(p.photoCount, "photo")}</figcaption>
                      </figure>
                    ))}
                  </div>
                  <div className="flex flex-col gap-2">
                    {!coupleConfirmed && (
                      <ActionButton testid="target-couple-confirm" label="Yes, that is them" keys={["Y"]} tone="good" disabled={busy} title={`Confirm the main people${hint("targetYes")}`} onClick={confirmCouple} />
                    )}
                    <ActionButton testid="target-couple-change-btn" label="Change who" keys={[]} title="Pick other people as the main subject" onClick={() => setChanging(mains.map((p) => p.id))} />
                  </div>
                </div>
              )}
            </section>
          )}

          {(coupleConfirmed || mains.length === 0) && asked.length > 0 && (
            <section data-testid="target-questions" data-open={open}>
              <h2 className="flex items-center gap-2 text-base font-semibold text-neutral-50">
                <Users className="size-4 text-sky-400" /> Is this person important?
                <span className="text-xs font-normal text-neutral-400" data-testid="target-questions-count">
                  {open > 0 ? `${open} to answer` : "All answered"}
                </span>
              </h2>
              <p className="mt-1 text-xs text-neutral-400">Parents, siblings, friends: say who matters. Photos with important people are favoured. Answer with <Key>Y</Key> or <Key>N</Key>, move with the arrow keys.</p>
              <div className="mt-3 grid grid-cols-2 gap-4 md:grid-cols-4" data-testid="target-person-cards">
                {asked.map((p, i) => {
                  const state = !p.roleConfirmed ? "open" : p.role === "important" ? "yes" : "no";
                  return (
                    <div
                      key={p.id}
                      className={`flex flex-col gap-2 rounded-lg bg-neutral-900 p-3 ${i === Math.min(focus, asked.length - 1) ? "ring-2 ring-sky-500" : "ring-1 ring-neutral-800"}`}
                      data-testid={`target-person-${p.id}`}
                      data-state={state}
                      data-focused={i === Math.min(focus, asked.length - 1)}
                      onClick={() => setFocus(i)}
                    >
                      <Face person={p} />
                      <div className="flex gap-1">
                        {p.samples.slice(1, 4).map((_, k) => (
                          <div key={k} className="w-1/3">
                            <Face person={p} index={k + 1} />
                          </div>
                        ))}
                      </div>
                      <div className="text-xs text-neutral-300">
                        {plural(p.photoCount, "photo")} · <span data-testid={`target-person-state-${p.id}`}>{roleText(p)}</span>
                      </div>
                      <div className="flex gap-2">
                        <button
                          className={`flex h-9 flex-1 items-center justify-center gap-1.5 rounded-md text-sm ${state === "yes" ? "bg-emerald-600 text-white" : "bg-neutral-800 text-neutral-100 hover:bg-emerald-800"}`}
                          data-testid={`target-person-yes-${p.id}`}
                          title={`Yes, important: photos with this person are favoured${hint("targetYes")}`}
                          disabled={busy}
                          onClick={(e) => {
                            e.stopPropagation();
                            setFocus(i);
                            void setRole([p.id], "important");
                          }}
                        >
                          Yes <Key>Y</Key>
                        </button>
                        <button
                          className={`flex h-9 flex-1 items-center justify-center gap-1.5 rounded-md text-sm ${state === "no" ? "bg-red-700 text-white" : "bg-neutral-800 text-neutral-100 hover:bg-red-900"}`}
                          data-testid={`target-person-no-${p.id}`}
                          title={`No, not important: treated like any guest${hint("targetNo")}`}
                          disabled={busy}
                          onClick={(e) => {
                            e.stopPropagation();
                            setFocus(i);
                            void setRole([p.id], "other");
                          }}
                        >
                          No <Key>N</Key>
                        </button>
                      </div>
                    </div>
                  );
                })}
              </div>
            </section>
          )}
        </div>
      </div>
      <footer className="flex shrink-0 items-center gap-3 border-t border-neutral-800 bg-neutral-950 px-6 py-3" data-testid="target-people-footer">
        <p className="min-w-0 flex-1 text-xs text-neutral-300" data-testid="target-rerun-note">
          {dirty ? "Your answers apply at the next run. A re-run keeps every keep, reject and swap you made." : "Answers apply at the next run. A re-run keeps every keep, reject and swap you made."}
        </p>
        <ActionButton
          testid="target-rerun"
          label="Re-run with these people"
          keys={[]}
          tone={dirty ? "primary" : "neutral"}
          disabled={busy || !ctx.run}
          title="Choose the delivery set again with the people you confirmed. Your own keep, reject and swap decisions are kept"
          onClick={() => void rerun()}
        />
      </footer>
    </div>
  );
});
