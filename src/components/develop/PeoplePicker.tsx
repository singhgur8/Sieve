// People mask picker: choose a detected person (or everyone) and the parts to select, then compute the AI mask.
import { useEffect, useState } from "react";
import { Loader2 } from "lucide-react";
import { commands, unwrap, type DetectedPerson, type MaskCapabilities, type NormPoint, type PersonPart } from "../../ipc";
import { PERSON_PART_LABEL } from "../../lib/masks";
import { Dialog } from "../Dialog";

interface Props {
  imageId: number;
  thumbUrl: string | null;
  caps: MaskCapabilities | null;
  onCreate: (referencePoint: NormPoint | null, parts: PersonPart[], name: string) => void;
  onCancel: () => void;
  onError: (e: unknown) => void;
}

export function PeoplePicker({ imageId, thumbUrl, caps, onCreate, onCancel, onError }: Props) {
  const [people, setPeople] = useState<DetectedPerson[] | null>(null);
  const [pick, setPick] = useState<number | null>(null); // index, null = everyone
  const [parts, setParts] = useState<Set<PersonPart>>(new Set());
  useEffect(() => {
    let stale = false;
    unwrap(commands.detectPeople(imageId))
      .then((p) => {
        if (stale) return;
        setPeople(p);
        setPick(p.length ? 0 : null);
      })
      .catch((e) => {
        if (!stale) {
          setPeople([]);
          onError(e);
        }
      });
    return () => {
      stale = true;
    };
  }, [imageId, onError]);

  const available = caps?.personParts ?? [];
  const toggle = (p: PersonPart) =>
    setParts((s) => {
      const n = new Set(s);
      if (n.has(p)) n.delete(p);
      else n.add(p);
      return n;
    });
  const chosen = available.filter((p) => parts.has(p));
  const person = pick != null && people ? people[pick] : null;
  const submit = () => onCreate(person ? person.referencePoint : null, chosen, person ? "Person" : "People");

  return (
    <Dialog
      label="Select people"
      testid="people-picker"
      overlayClass="z-[60] bg-black/60"
      className="w-[26rem] rounded-lg border border-neutral-700 bg-neutral-900 p-4 shadow-xl"
      onCancel={onCancel}
      onConfirm={submit}
      canConfirm={() => people !== null}
    >
      <h2 className="mb-2 text-sm font-semibold">Select people</h2>
      {people === null ? (
        <p className="flex items-center gap-2 text-xs text-neutral-300" data-testid="people-loading">
          <Loader2 className="size-4 animate-spin" /> Looking for people...
        </p>
      ) : (
        <>
          <div className="mb-3 flex flex-wrap gap-2" role="radiogroup" aria-label="Person" data-testid="people-list">
            {people.length === 0 && <p className="text-xs text-neutral-400">No people found. Continue to select everyone the model finds.</p>}
            {people.map((p, i) => (
              <button
                key={i}
                role="radio"
                aria-checked={pick === i}
                className={`h-20 w-20 overflow-hidden rounded border-2 bg-neutral-800 ${pick === i ? "border-sky-400" : "border-transparent"}`}
                style={
                  thumbUrl
                    ? {
                        backgroundImage: `url(${thumbUrl})`,
                        backgroundSize: `${100 / p.bbox.width}% ${100 / p.bbox.height}%`,
                        backgroundPosition: `${(p.bbox.x / Math.max(0.001, 1 - p.bbox.width)) * 100}% ${(p.bbox.y / Math.max(0.001, 1 - p.bbox.height)) * 100}%`,
                      }
                    : undefined
                }
                onClick={() => setPick(i)}
                title={`Person ${i + 1}`}
                data-testid={`people-person-${i}`}
              >
                <span className="sr-only">Person {i + 1}</span>
              </button>
            ))}
            {people.length > 1 && (
              <button role="radio" aria-checked={pick === null} className={`h-20 w-20 rounded border-2 bg-neutral-800 text-xs ${pick === null ? "border-sky-400" : "border-transparent"}`} onClick={() => setPick(null)} data-testid="people-everyone">
                Everyone
              </button>
            )}
          </div>
          <fieldset className="mb-3" data-testid="people-parts">
            <legend className="mb-1 text-xs font-semibold uppercase tracking-wide text-neutral-400">Parts</legend>
            <label className="flex items-center gap-2 py-0.5 text-xs">
              <input type="checkbox" checked={chosen.length === 0} onChange={() => setParts(new Set())} data-testid="people-part-all" /> Entire person
            </label>
            <div className="grid grid-cols-2">
              {available.map((p) => (
                <label key={p} className="flex items-center gap-2 py-0.5 text-xs">
                  <input type="checkbox" checked={parts.has(p)} onChange={() => toggle(p)} data-testid={`people-part-${p}`} /> {PERSON_PART_LABEL[p]}
                </label>
              ))}
            </div>
            {available.length === 0 && <p className="text-[11px] text-neutral-400">The installed people model can only select the entire person.</p>}
          </fieldset>
        </>
      )}
      <div className="flex justify-end gap-2">
        <button className="rounded bg-neutral-800 px-3 py-1 text-xs hover:bg-neutral-700" onClick={onCancel} data-testid="people-cancel">
          Cancel
        </button>
        <button className="rounded bg-sky-700 px-3 py-1 text-xs text-white hover:bg-sky-600 disabled:opacity-40" disabled={people === null} onClick={submit} data-testid="people-create">
          Create mask
        </button>
      </div>
    </Dialog>
  );
}
