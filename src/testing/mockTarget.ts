// Mock of the target-count culling commands (IPC v20) for `src/testing/mockBackend.ts`.
// Mirrors `db::target` (storage + user edits) with synthetic engine output:
// - people: a couple (main, suggested) + 4 recurring people to ask about ("Is this person important?");
// - moments of every shot type (couple / group / detail / candid / other) over project 1's photos;
// - a selection with delivered photos, ranked alternatives, not sure / set aside and "covered by".
// Switches (URL of the mock page): `?target=1` starts with a finished run on project 1 (open people
// questions); without it there is no run until `run_target_selection` (which builds the same data after
// `window.__mockTargetDelay` ms, default 300, reporting `activity-event` kind `target_selection` and one
// `target-run-finished`). `?target=answered` = like 1 with the questions answered.
import { emit } from "@tauri-apps/api/event";
import type {
  ActivityEvent,
  Alternatives,
  ApplySuggestionsResult,
  CoveredBy,
  CullSnapshot,
  FaceInfo,
  FaceSample,
  ImageSelection,
  Moment,
  PeopleOverview,
  Person,
  PersonRole,
  PickFlag,
  RawImageEntry,
  ShotType,
  TargetChoice,
  TargetCounts,
  TargetEditResult,
  TargetReason,
  TargetRun,
  TargetRunFinished,
  TargetRunSettings,
  TargetSnapshot,
} from "../ipc";

declare global {
  interface Window {
    /** Test hook: ms before a mock `run_target_selection` finishes (default 300). */
    __mockTargetDelay?: number;
  }
}

/** What the mock backend gives the target mock. */
export interface MockTargetContext {
  rows: RawImageEntry[];
  byId: Map<number, RawImageEntry>;
  projectOf: (r: RawImageEntry) => number | null;
  requireProject: (id: number) => { shootType: string };
  faces: (id: number) => FaceInfo[];
  guardWrite: () => void;
}

/** Returned by `handle` for commands it does not know. */
export const NOT_TARGET = Symbol("not-target");

type Row = Omit<ImageSelection, "personIds"> & { projectId: number };

const SHOT_PATTERN: ShotType[] = ["couple", "couple", "group", "detail", "candid", "couple", "other", "group", "candid", "detail"];
const MOMENT_SIZE = 8;

export function createMockTarget(ctx: MockTargetContext) {
  const params = new URLSearchParams(typeof location === "undefined" ? "" : location.search);
  const sel = new Map<number, Row>();
  const people = new Map<number, Person & { samplesRef: [number, number][] }>();
  const moments = new Map<number, Omit<Moment, "imageIds" | "deliveredIds">>();
  const runs = new Map<number, TargetRun>();
  /** The scorer's own suggestion (`quality_scores.scored_pick`), kept before the overlay. */
  const scored = new Map<number, PickFlag>();
  let running: { projectId: number; cancelled: boolean } | null = null;
  let activitySeq = 100_000;

  const stem = (id: number) => (ctx.byId.get(id)?.fileName ?? `photo ${id}`).replace(/\.[^.]+$/, "");
  const projectRows = (projectId: number) =>
    ctx.rows.filter((r) => ctx.projectOf(r) === projectId).sort((a, b) => (a.capture.capturedAtMs ?? 0) - (b.capture.capturedAtMs ?? 0) || a.id - b.id);
  const reason = (kind: TargetReason["kind"], text: string, relatedImageId: number | null = null): TargetReason => ({ kind, text, relatedImageId });
  const personIdsOf = (r: Row): number[] => {
    const m = r.momentId != null ? moments.get(r.momentId) : undefined;
    return ctx.faces(r.imageId).length ? [...(m?.personIds ?? [])] : [];
  };
  const dto = (r: Row): ImageSelection => {
    const { projectId: _p, ...rest } = r;
    void _p;
    return { ...rest, reasons: [...r.reasons], personIds: personIdsOf(r) };
  };

  // ---- overlay (Rust `db::target::OVERLAY_SQL`) ----
  function overlay(ids: Iterable<number>) {
    for (const id of ids) {
      const e = ctx.byId.get(id);
      if (!e?.quality) continue;
      if (!scored.has(id)) scored.set(id, e.quality.suggestedPick);
      const base = scored.get(id)!;
      const choice = sel.get(id)?.choice;
      const next: PickFlag = choice == null ? base : choice === "deliver" ? "pick" : base === "pick" ? "unflagged" : base;
      e.quality = { ...e.quality, suggestedPick: next };
    }
  }

  function counts(projectId: number): TargetCounts {
    const c: TargetCounts = { total: 0, deliver: 0, alternative: 0, notSure: 0, setAside: 0, locked: 0, perShotType: [] };
    const per = new Map<ShotType, { total: number; deliver: number }>();
    for (const r of sel.values()) {
      if (r.projectId !== projectId) continue;
      c.total++;
      if (r.locked) c.locked++;
      if (r.choice === "deliver") c.deliver++;
      else if (r.choice === "alternative") c.alternative++;
      else if (r.choice === "not_sure") c.notSure++;
      else c.setAside++;
      if (r.shotType) {
        const p = per.get(r.shotType) ?? { total: 0, deliver: 0 };
        p.total++;
        if (r.choice === "deliver") p.deliver++;
        per.set(r.shotType, p);
      }
    }
    const order: ShotType[] = ["couple", "group", "detail", "candid", "other"];
    c.perShotType = order.filter((s) => per.has(s)).map((s) => ({ shotType: s, ...per.get(s)! }));
    return c;
  }

  const openQuestions = (projectId: number) =>
    [...people.values()].filter((p) => p.projectId === projectId && p.ask && !p.roleConfirmed).sort((a, b) => b.photoCount - a.photoCount || a.id - b.id).map((p) => p.id);

  const runDto = (projectId: number): TargetRun | null => {
    const r = runs.get(projectId);
    return r ? { ...r, counts: counts(projectId), peopleQuestions: openQuestions(projectId).length } : null;
  };

  // ---- synthetic engine output ----
  function sample(imageId: number, faceIndex: number): FaceSample | null {
    const e = ctx.byId.get(imageId);
    const f = ctx.faces(imageId)[faceIndex];
    if (!e || !f) return null;
    const aspect = e.thumbnail.status === "ready" && e.thumbnail.height > 0 ? e.thumbnail.width / e.thumbnail.height : 1.5;
    const b = f.bbox;
    const side = Math.min(Math.max(b.width * aspect, b.height) * 1.8, Math.min(aspect, 1));
    const left = Math.min(Math.max((b.x + b.width / 2) * aspect - side / 2, 0), aspect - side);
    const top = Math.min(Math.max(b.y + b.height / 2 - side / 2, 0), 1 - side);
    return {
      imageId,
      faceIndex,
      bbox: b,
      crop: { x: left / aspect, y: top, width: side / aspect, height: side },
      imageAspect: aspect,
      previewPath: e.thumbnail.status === "ready" ? e.thumbnail.previewPath : null,
    };
  }

  function buildPeople(projectId: number, photos: RawImageEntry[]) {
    const withFaces = photos.filter((r) => ctx.faces(r.id).length > 0);
    const keep = new Map([...people.values()].filter((p) => p.projectId === projectId).map((p) => [p.id, p]));
    for (const p of [...people.values()]) if (p.projectId === projectId) people.delete(p.id);
    const defs: { role: PersonRole; ask: boolean; share: number }[] = [
      { role: "main", ask: false, share: 0.7 },
      { role: "main", ask: false, share: 0.68 },
      { role: "unknown", ask: true, share: 0.22 },
      { role: "unknown", ask: true, share: 0.15 },
      { role: "unknown", ask: true, share: 0.1 },
      { role: "unknown", ask: true, share: 0.06 },
    ];
    defs.forEach((d, i) => {
      const id = projectId * 100 + i + 1;
      const prev = keep.get(id);
      const step = Math.max(1, Math.round(1 / d.share));
      const mine = withFaces.filter((_, k) => (k + i) % step === 0);
      const refs: [number, number][] = mine.slice(0, 6).map((r) => [r.id, i < 2 ? i : (i + r.id) % 2]);
      const confirmed = prev?.roleConfirmed ?? (params.get("target") === "answered" && d.ask);
      const userRole: PersonRole = prev?.roleConfirmed ? prev.role : i === 2 || i === 3 ? "important" : "other";
      people.set(id, {
        id,
        projectId,
        role: confirmed ? userRole : d.role,
        roleConfirmed: confirmed,
        suggestedRole: d.role,
        ask: d.ask,
        photoCount: mine.length,
        faceCount: mine.length,
        samples: [],
        samplesRef: refs,
      });
    });
  }

  function buildSelection(projectId: number, settings: TargetRunSettings) {
    const photos = projectRows(projectId);
    buildPeople(projectId, photos);
    const ids = (rows: RawImageEntry[]) => rows.map((r) => r.id);
    const important = new Set([...people.values()].filter((p) => p.projectId === projectId && (p.role === "main" || p.role === "important")).map((p) => p.id));
    for (const m of [...moments.values()]) if (m.projectId === projectId) moments.delete(m.id);
    const locked = new Map([...sel.values()].filter((r) => r.projectId === projectId && r.locked).map((r) => [r.imageId, r]));
    for (const r of [...sel.values()]) if (r.projectId === projectId && !r.locked) sel.delete(r.imageId);
    let lastDelivered: number | null = null;
    for (let start = 0, k = 0; start < photos.length; start += MOMENT_SIZE, k++) {
      const chunk = photos.slice(start, start + MOMENT_SIZE);
      const shot = SHOT_PATTERN[k % SHOT_PATTERN.length];
      const momentId = projectId * 10_000 + k + 1;
      const base = projectId * 100;
      const personIds = shot === "couple" ? [base + 1, base + 2] : shot === "group" ? [base + 1, base + 2, base + 3, base + 4, base + 5] : shot === "candid" ? [base + 3 + (k % 4)] : [];
      const ranked = [...chunk].sort((a, b) => (b.quality?.overall ?? 0) - (a.quality?.overall ?? 0) || a.id - b.id);
      const deliverN = shot === "couple" ? 3 : shot === "candid" ? 2 : shot === "other" ? 0 : 1;
      const scorer = (r: RawImageEntry) => scored.get(r.id) ?? r.quality?.suggestedPick;
      const delivered = new Set(ids(ranked.filter((r) => scorer(r) !== "reject" && !locked.has(r.id)).slice(0, deliverN)));
      for (const r of chunk) if (locked.get(r.id)?.choice === "deliver") delivered.add(r.id);
      moments.set(momentId, { id: momentId, projectId, shotType: shot, startedAtMs: chunk[0].capture.capturedAtMs ?? null, endedAtMs: chunk[chunk.length - 1].capture.capturedAtMs ?? null, personIds, representativeId: ranked[0]?.id ?? null });
      const boost = personIds.some((p) => important.has(p));
      const nearestDelivered = (id: number) => {
        const d = [...delivered];
        if (!d.length) return lastDelivered;
        return d.reduce((best, x) => (Math.abs(x - id) < Math.abs(best - id) ? x : best), d[0]);
      };
      const altCount = new Map<number, number>();
      ranked.forEach((r, idx) => {
        const old = locked.get(r.id);
        const score = Math.min(1, (r.quality?.overall ?? 0) * (boost ? 1.15 : 1));
        const cover = delivered.has(r.id) ? null : nearestDelivered(r.id);
        const similarity = cover == null ? null : Math.round((0.95 - ((idx * 7) % 30) / 100) * 100) / 100;
        if (old) {
          sel.set(r.id, { ...old, momentId, shotType: shot, score });
          return;
        }
        let choice: TargetChoice;
        let alternativeOf: number | null = null;
        let rank: number | null = null;
        const reasons: TargetReason[] = [];
        if (delivered.has(r.id)) {
          choice = "deliver";
          const kind = shot === "couple" ? "couple_variation" : shot === "group" ? "group_best" : shot === "detail" ? "detail_best" : "candid_visible";
          const text = shot === "couple" ? "A different pose of the couple" : shot === "group" ? "Best of this group setup: everyone is looking" : shot === "detail" ? "Best of this detail: sharp on the object" : "Faces clearly visible";
          reasons.push(reason(kind, text));
          if (boost) reasons.push(reason("important_person", shot === "couple" ? "Shows the couple" : "Shows people you marked important"));
        } else if (scorer(r) === "reject") {
          choice = "set_aside";
          reasons.push(reason("defect", r.quality?.reasons[0]?.text ?? "Eyes closed"));
        } else if ((shot === "couple" || shot === "group") && cover != null && (altCount.get(cover) ?? 0) < (shot === "couple" ? 3 : 2)) {
          choice = "alternative";
          alternativeOf = cover;
          rank = (altCount.get(cover) ?? 0) + 1;
          altCount.set(cover, rank);
          reasons.push(reason(shot === "group" ? "not_best_of_setup" : "near_duplicate", shot === "group" ? `Same group setup as ${stem(cover)}; fewer people looking` : `Almost the same as ${stem(cover)}`, cover));
        } else if (shot === "candid" || shot === "other") {
          choice = "not_sure";
          reasons.push(shot === "other" ? reason("below_target", "Good, but the target was reached by better frames") : reason("no_visible_face", "Back of the head: no visible face"));
        } else {
          choice = "set_aside";
          reasons.push(shot === "detail" ? reason("detail_out_of_focus", "Focus is not on the object") : reason("near_duplicate", `Almost the same as ${stem(cover ?? r.id)}`, cover));
        }
        sel.set(r.id, { imageId: r.id, projectId, choice, momentId, shotType: shot, alternativeOf, rank, coveredBy: choice === "deliver" ? null : cover, coveredSimilarity: choice === "deliver" ? null : similarity, score, reasons, locked: false });
      });
      const d = [...delivered];
      if (d.length) lastDelivered = d[d.length - 1];
    }
    overlay(ids(photos));
    const c = counts(projectId);
    const shootType = settings.shootType ?? (ctx.requireProject(projectId).shootType as TargetRunSettings["shootType"]);
    runs.set(projectId, {
      projectId,
      settings: { targetCount: settings.targetCount, shootType },
      state: "finished",
      startedAtMs: Date.now() - 1000,
      finishedAtMs: Date.now(),
      appliedAtMs: runs.get(projectId)?.appliedAtMs ?? null,
      message: `Picked ${c.deliver} of ${c.total} photos`,
      modelVersion: "mock-identity@1+mock-selection@1",
      counts: c,
      peopleQuestions: 0,
    });
  }

  if (params.get("target") === "1" || params.get("target") === "answered") buildSelection(1, { targetCount: 800, shootType: null });

  // ---- user edits (Rust `db::target`) ----
  const requireRow = (id: number) => {
    if (!ctx.byId.get(id)) throw { kind: "not_found", message: `image ${id}` };
    const r = sel.get(id);
    if (!r) throw { kind: "invalid_argument", message: `image ${id} is not part of the target selection` };
    return r;
  };
  const cullOf = (id: number): CullSnapshot => {
    const e = ctx.byId.get(id)!;
    return { imageId: id, rating: e.rating, pick: e.pick, colorLabel: e.colorLabel, pickOrigin: e.pick === "unflagged" ? null : (e.pickOrigin ?? "user") };
  };
  const snap = (id: number): TargetSnapshot => {
    const r = sel.get(id)!;
    return { imageId: id, choice: r.choice, alternativeOf: r.alternativeOf, rank: r.rank, coveredBy: r.coveredBy, locked: r.locked, cull: cullOf(id) };
  };
  const compact = (parent: number) => {
    [...sel.values()]
      .filter((r) => r.choice === "alternative" && r.alternativeOf === parent)
      .sort((a, b) => (a.rank ?? 1e9) - (b.rank ?? 1e9) || a.imageId - b.imageId)
      .forEach((r, i) => (r.rank = i + 1));
  };
  const userReason = (r: Row, text: string, related: number | null = null) => {
    r.reasons = [reason("user_choice", text, related), ...r.reasons.filter((x) => x.kind !== "user_choice")];
  };
  const moveTo = (id: number, choice: TargetChoice) => {
    const r = requireRow(id);
    if (r.choice === "deliver" && choice !== "deliver") {
      for (const a of sel.values()) {
        if (a.alternativeOf === id) Object.assign(a, { choice: "not_sure", alternativeOf: null, rank: null });
        if (a.coveredBy === id) Object.assign(a, { coveredBy: null, coveredSimilarity: null });
      }
    }
    const parent = r.choice === "alternative" ? r.alternativeOf : null;
    Object.assign(r, { choice, alternativeOf: null, rank: null, locked: true });
    if (choice === "deliver") Object.assign(r, { coveredBy: null, coveredSimilarity: null });
    if (parent != null) compact(parent);
  };
  const writeUserFlag = (id: number, pick: PickFlag) => {
    const e = ctx.byId.get(id)!;
    if (e.pick === pick) return false;
    e.pick = pick;
    e.pickOrigin = pick === "unflagged" ? null : "user";
    return true;
  };
  function edit(ids: number[], f: (flags: number[]) => void): TargetEditResult {
    ctx.guardWrite();
    if (!ids.length) throw { kind: "invalid_argument", message: "no photos given" };
    const projects = new Set(ids.map((id) => requireRow(id).projectId));
    if (projects.size > 1) throw { kind: "invalid_argument", message: "photos of different projects" };
    const touched = new Set(ids);
    for (const id of ids) {
      const parent = sel.get(id)!.alternativeOf;
      for (const r of sel.values()) if (r.alternativeOf === id || r.coveredBy === id || (parent != null && r.alternativeOf === parent)) touched.add(r.imageId);
    }
    const before = [...touched].sort((a, b) => a - b).map(snap);
    const flags: number[] = [];
    f(flags);
    overlay(touched);
    const changed: ImageSelection[] = [];
    const previous: TargetSnapshot[] = [];
    for (const b of before) {
      const now = snap(b.imageId);
      if (JSON.stringify(now) !== JSON.stringify(b) || ids.includes(b.imageId)) {
        changed.push(dto(sel.get(b.imageId)!));
        previous.push(b);
      }
    }
    return { changed, flagsChanged: [...new Set(flags)].sort((a, b) => a - b), previous, counts: counts([...projects][0]) };
  }

  function noteUserFlags(ids: number[], pick: PickFlag) {
    for (const id of ids) {
      const r = sel.get(id);
      if (!r) continue;
      const target: TargetChoice = pick === "pick" ? "deliver" : pick === "reject" ? "set_aside" : r.choice === "deliver" ? "not_sure" : r.choice;
      if (target === r.choice) {
        r.locked = true;
        continue;
      }
      moveTo(id, target);
      userReason(r, pick === "pick" ? "You picked this" : pick === "reject" ? "You rejected this" : "You removed the flag");
    }
    overlay(ids);
  }

  function startRun(projectId: number, settings: TargetRunSettings): TargetRun {
    ctx.guardWrite();
    ctx.requireProject(projectId);
    if (!Number.isInteger(settings.targetCount) || settings.targetCount < 1 || settings.targetCount > 100_000)
      throw { kind: "invalid_argument", message: `targetCount ${settings.targetCount} is outside 1..=100000` };
    if (running) throw { kind: "invalid_argument", message: "Target selection is already running" };
    const shootType = settings.shootType ?? (ctx.requireProject(projectId).shootType as TargetRunSettings["shootType"]);
    const prev = runs.get(projectId);
    runs.set(projectId, {
      projectId,
      settings: { targetCount: settings.targetCount, shootType },
      state: "running",
      startedAtMs: Date.now(),
      finishedAtMs: null,
      appliedAtMs: prev?.appliedAtMs ?? null,
      message: null,
      modelVersion: prev?.modelVersion ?? "",
      counts: counts(projectId),
      peopleQuestions: 0,
    });
    const job = { projectId, cancelled: false };
    running = job;
    const id = ++activitySeq;
    const label = `Choosing the best ${settings.targetCount}`;
    const send = (done: number, state: ActivityEvent["state"], message: string | null = null) =>
      void emit("activity-event", { id, kind: "target_selection", label, done, total: 3, state, message } satisfies ActivityEvent);
    send(0, "running");
    const delay = window.__mockTargetDelay ?? 300;
    setTimeout(() => send(1, "running"), delay / 3);
    setTimeout(() => send(2, "running"), (2 * delay) / 3);
    setTimeout(() => {
      running = null;
      const run = runs.get(projectId)!;
      if (job.cancelled) {
        Object.assign(run, { state: "cancelled", finishedAtMs: Date.now(), message: "Stopped; the previous selection is kept" });
        send(2, "cancelled", run.message);
      } else {
        buildSelection(projectId, settings);
        send(3, "finished", runs.get(projectId)!.message);
      }
      void emit("target-run-finished", { run: runDto(projectId)! } satisfies TargetRunFinished);
    }, delay);
    return runDto(projectId)!;
  }

  function personDto(p: Person & { samplesRef: [number, number][] }): Person {
    const { samplesRef, ...rest } = p;
    return { ...rest, samples: samplesRef.map(([i, f]) => sample(i, f)).filter((s): s is FaceSample => s != null) };
  }

  /** Handles a target command (`NOT_TARGET` for anything else). */
  function handle(cmd: string, args: Record<string, unknown>): unknown {
    switch (cmd) {
      case "run_target_selection":
        return startRun(args.projectId as number, args.settings as TargetRunSettings);
      case "cancel_target_selection":
        if (running) running.cancelled = true;
        return null;
      case "get_target_run":
        ctx.requireProject(args.projectId as number);
        return runDto(args.projectId as number);
      case "list_people": {
        const projectId = args.projectId as number;
        ctx.requireProject(projectId);
        const order = (r: PersonRole) => (r === "main" ? 0 : r === "important" ? 1 : 2);
        const list = [...people.values()].filter((p) => p.projectId === projectId).sort((a, b) => order(a.role) - order(b.role) || b.photoCount - a.photoCount || a.id - b.id);
        const has = runs.get(projectId)?.modelVersion?.startsWith("mock-identity") ?? false;
        return {
          projectId,
          people: list.map(personDto),
          questions: openQuestions(projectId),
          modelVersion: has ? "mock-identity@1" : null,
          message: has ? (list.length ? null : "No recurring people found") : "Face recognition is not available yet",
        } satisfies PeopleOverview;
      }
      case "set_person_role": {
        ctx.guardWrite();
        const p = people.get(args.personId as number);
        if (!p) throw { kind: "not_found", message: `person ${args.personId}` };
        const role = (args.role as PersonRole | null) ?? null;
        Object.assign(p, role ? { role, roleConfirmed: true } : { role: p.suggestedRole, roleConfirmed: false });
        return personDto(p);
      }
      case "list_moments": {
        const projectId = args.projectId as number;
        ctx.requireProject(projectId);
        const time = (id: number) => ctx.byId.get(id)?.capture.capturedAtMs ?? 0;
        return [...moments.values()]
          .filter((m) => m.projectId === projectId)
          .sort((a, b) => (a.startedAtMs ?? 0) - (b.startedAtMs ?? 0) || a.id - b.id)
          .map((m): Moment => {
            const members = [...sel.values()].filter((r) => r.momentId === m.id).sort((a, b) => time(a.imageId) - time(b.imageId) || a.imageId - b.imageId);
            return { ...m, imageIds: members.map((r) => r.imageId), deliveredIds: members.filter((r) => r.choice === "deliver").map((r) => r.imageId) };
          })
          .filter((m) => m.imageIds.length > 0);
      }
      case "get_image_selections": {
        const ids = (args.ids as number[]) ?? [];
        for (const i of ids) if (!ctx.byId.get(i)) throw { kind: "not_found", message: `image ${i}` };
        return ids.filter((i) => sel.has(i)).map((i) => dto(sel.get(i)!));
      }
      case "get_alternatives": {
        const id = args.imageId as number;
        if (!ctx.byId.get(id)) throw { kind: "not_found", message: `image ${id}` };
        const r = sel.get(id);
        const d = r?.choice === "deliver" ? r : r?.choice === "alternative" && r.alternativeOf != null ? sel.get(r.alternativeOf) : undefined;
        const delivered = d && d.choice === "deliver" ? d : undefined;
        const alternatives = delivered
          ? [...sel.values()].filter((a) => a.choice === "alternative" && a.alternativeOf === delivered.imageId).sort((a, b) => (a.rank ?? 1e9) - (b.rank ?? 1e9) || a.imageId - b.imageId).map(dto)
          : [];
        return { imageId: id, delivered: delivered ? dto(delivered) : null, alternatives } satisfies Alternatives;
      }
      case "get_covered_by": {
        const id = args.imageId as number;
        if (!ctx.byId.get(id)) throw { kind: "not_found", message: `image ${id}` };
        const r = sel.get(id);
        if (!r || r.choice === "deliver" || r.coveredBy == null) return null;
        const c = sel.get(r.coveredBy);
        if (!c || c.choice !== "deliver") return null;
        return { imageId: id, coveredById: c.imageId, similarity: r.coveredSimilarity ?? 0, sameMoment: r.momentId != null && r.momentId === c.momentId, text: `Already kept a similar one: ${stem(c.imageId)}` } satisfies CoveredBy;
      }
      case "swap_alternative": {
        const d = args.deliveredId as number;
        const a = args.alternativeId as number;
        if (d === a) throw { kind: "invalid_argument", message: "cannot swap a photo with itself" };
        return edit([d, a], (flags) => {
          if (requireRow(d).choice !== "deliver") throw { kind: "invalid_argument", message: `image ${d} is not in the delivery set` };
          if (requireRow(a).choice === "deliver") throw { kind: "invalid_argument", message: `image ${a} is already in the delivery set` };
          moveTo(a, "deliver");
          for (const r of sel.values()) {
            if (r.choice === "alternative" && r.alternativeOf === d) Object.assign(r, { alternativeOf: a, rank: (r.rank ?? 0) + 1 });
            if (r.coveredBy === d && r.imageId !== a) r.coveredBy = a;
          }
          Object.assign(sel.get(d)!, { choice: "alternative", alternativeOf: a, rank: 1, locked: true, coveredBy: a, coveredSimilarity: null });
          compact(a);
          userReason(sel.get(a)!, `You swapped this in for ${stem(d)}`, d);
          userReason(sel.get(d)!, `You swapped this out for ${stem(a)}`, a);
          if (writeUserFlag(a, "pick")) flags.push(a);
          if (ctx.byId.get(d)!.pick === "pick" && writeUserFlag(d, "unflagged")) flags.push(d);
        });
      }
      case "add_alternative": {
        const id = args.imageId as number;
        return edit([id], (flags) => {
          if (requireRow(id).choice === "deliver") throw { kind: "invalid_argument", message: `image ${id} is already in the delivery set` };
          moveTo(id, "deliver");
          userReason(sel.get(id)!, "You added this");
          if (writeUserFlag(id, "pick")) flags.push(id);
        });
      }
      case "set_target_choice": {
        const choice = args.choice as TargetChoice;
        if (choice === "alternative") throw { kind: "invalid_argument", message: "use swap_alternative to make a photo an alternative" };
        const ids = [...new Set((args.ids as number[]) ?? [])].sort((a, b) => a - b);
        return edit(ids, (flags) => {
          for (const id of ids) {
            const r = requireRow(id);
            if (r.choice === choice) {
              r.locked = true;
              continue;
            }
            moveTo(id, choice);
            userReason(r, choice === "deliver" ? "You added this" : choice === "not_sure" ? "You marked this not sure" : "You set this aside");
            const pick = ctx.byId.get(id)!.pick;
            const next: PickFlag | null = choice === "deliver" ? "pick" : pick === "pick" ? "unflagged" : null;
            if (next && writeUserFlag(id, next)) flags.push(id);
          }
        });
      }
      case "restore_target_snapshot": {
        ctx.guardWrite();
        const snaps = (args.snapshots as TargetSnapshot[]) ?? [];
        for (const s of snaps) requireRow(s.imageId);
        const changed = new Set<number>();
        for (const s of snaps) {
          const r = sel.get(s.imageId)!;
          const before = JSON.stringify([r.choice, r.alternativeOf, r.rank, r.coveredBy, r.locked]);
          Object.assign(r, { choice: s.choice, alternativeOf: s.alternativeOf, rank: s.rank, coveredBy: s.coveredBy, locked: s.locked });
          if (JSON.stringify([r.choice, r.alternativeOf, r.rank, r.coveredBy, r.locked]) !== before) changed.add(s.imageId);
          const e = ctx.byId.get(s.imageId)!;
          const c = s.cull;
          if (e.pick !== c.pick || e.rating !== c.rating || e.colorLabel !== c.colorLabel || (c.pick !== "unflagged" && e.pickOrigin !== (c.pickOrigin ?? "user"))) changed.add(s.imageId);
          Object.assign(e, { pick: c.pick, rating: c.rating, colorLabel: c.colorLabel, pickOrigin: c.pick === "unflagged" ? null : (c.pickOrigin ?? "user") });
        }
        overlay(snaps.map((s) => s.imageId));
        return [...changed].sort((a, b) => a - b);
      }
      case "apply_target_selection": {
        ctx.guardWrite();
        const projectId = args.projectId as number;
        ctx.requireProject(projectId);
        let applied = 0;
        let total = 0;
        for (const r of sel.values()) {
          if (r.projectId !== projectId) continue;
          total++;
          const e = ctx.byId.get(r.imageId)!;
          if (!e.quality || !(e.pick === "unflagged" || e.pickOrigin === "auto")) continue;
          if (e.pick === e.quality.suggestedPick) continue;
          e.pick = e.quality.suggestedPick;
          e.pickOrigin = e.pick === "unflagged" ? null : "auto";
          applied++;
        }
        const run = runs.get(projectId);
        if (run) run.appliedAtMs = Date.now();
        return { applied, skipped: total - applied } satisfies ApplySuggestionsResult;
      }
      default:
        return NOT_TARGET;
    }
  }

  /** `ImageQuery.targetChoices` (Rust `db::target::choices_clause`). */
  const choiceOk = (id: number, choices: TargetChoice[] | null | undefined) => !choices?.length || choices.includes(sel.get(id)?.choice as TargetChoice);

  return { handle, noteUserFlags, choiceOk };
}
