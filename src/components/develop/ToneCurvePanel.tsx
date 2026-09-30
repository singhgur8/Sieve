// Tone Curve: parametric region sliders with split handles, and a point-curve editor (master / R / G / B).
import { useRef, useState } from "react";
import { MAX_CURVE_POINTS, IDENTITY_CURVE, type CompleteAdjustments, type CurvePoint, type PointCurves } from "../../ipc";
import type { Editor } from "../../hooks/useEditor";
import { addPoint, movePoint, removePoint, sampleCurve } from "../../lib/curve";
import { NumField, seg } from "./fields";

export type Channel = keyof PointCurves;
const CHANNELS: { id: Channel; label: string; color: string }[] = [
  { id: "master", label: "RGB", color: "#e5e5e5" },
  { id: "red", label: "R", color: "#f87171" },
  { id: "green", label: "G", color: "#4ade80" },
  { id: "blue", label: "B", color: "#60a5fa" },
];

const PAD = 8;
const VIEW = 255 + PAD * 2;

const setPts = (a: CompleteAdjustments, ch: Channel, fn: (pts: CurvePoint[]) => CurvePoint[]): CompleteAdjustments => {
  const cur = a.toneCurve.point[ch];
  const next = fn(cur);
  return next === cur ? a : { ...a, toneCurve: { ...a.toneCurve, point: { ...a.toneCurve.point, [ch]: next } } };
};

/** What a point-curve editor edits: the four curves plus live / committed / release callbacks (global or per-mask). */
export interface CurveHost {
  curves: PointCurves;
  /** Live edit (drag): `fn` maps the channel's points to the new points (return the same array for "no change"). */
  edit: (ch: Channel, fn: (pts: CurvePoint[]) => CurvePoint[]) => void;
  /** Committed edit (own history entry). */
  change: (ch: Channel, fn: (pts: CurvePoint[]) => CurvePoint[]) => void;
  commit: () => void;
}

const isIdentity = (pts: readonly CurvePoint[]) => pts.length === 2 && pts[0][0] === 0 && pts[0][1] === 0 && pts[1][0] === 255 && pts[1][1] === 255;

/** Master / R / G / B point-curve editor. `tid` prefixes the test ids ("curve" for the Tone Curve panel, "mask-curve" for masks). */
export function PointCurveEditor({ host, tid = "curve" }: { host: CurveHost; tid?: string }) {
  const [ch, setCh] = useState<Channel>("master");
  const [sel, setSel] = useState<number | null>(null);
  const svg = useRef<SVGSVGElement>(null);
  const drag = useRef<{ index: number } | null>(null);
  const pts = host.curves[ch];
  const color = CHANNELS.find((c) => c.id === ch)!.color;

  const toCurve = (e: { clientX: number; clientY: number }): [number, number] => {
    const r = svg.current!.getBoundingClientRect();
    const k = VIEW / r.width;
    return [(e.clientX - r.left) * k - PAD, 255 - ((e.clientY - r.top) * k - PAD)];
  };
  const hit = (x: number, y: number): number => {
    let best = -1;
    let bd = 6 * 6;
    pts.forEach((p, i) => {
      const d = (p[0] - x) ** 2 + (p[1] - y) ** 2;
      if (d <= bd) {
        bd = d;
        best = i;
      }
    });
    return best;
  };

  const onDown = (e: React.PointerEvent<SVGSVGElement>) => {
    if (e.button !== 0) return;
    svg.current!.focus();
    const [x, y] = toCurve(e);
    let index = hit(x, y);
    if (index < 0) {
      let added = -1;
      host.edit(ch, (cur) => {
        const r = addPoint(cur, x, y);
        added = r.index;
        return added < 0 ? cur : r.pts;
      });
      if (added < 0) return;
      index = added;
    }
    drag.current = { index };
    setSel(index);
    svg.current!.setPointerCapture(e.pointerId);
  };
  const onMove = (e: React.PointerEvent<SVGSVGElement>) => {
    const d = drag.current;
    if (!d) return;
    const [x, y] = toCurve(e);
    host.edit(ch, (cur) => movePoint(cur, d.index, x, y));
  };
  const onUp = () => {
    if (!drag.current) return;
    drag.current = null;
    host.commit();
  };
  const remove = (i: number) => {
    if (pts.length <= 2) return;
    host.change(ch, (cur) => removePoint(cur, i));
    setSel(null);
  };
  const onDouble = (e: React.MouseEvent<SVGSVGElement>) => {
    const [x, y] = toCurve(e);
    const i = hit(x, y);
    if (i >= 0) remove(i);
  };
  const onKey = (e: React.KeyboardEvent<SVGSVGElement>) => {
    if (sel == null || sel >= pts.length) return;
    if (e.key === "Backspace" || e.key === "Delete") {
      e.preventDefault();
      e.stopPropagation();
      remove(sel);
    } else if (e.key.startsWith("Arrow")) {
      e.preventDefault();
      e.stopPropagation();
      const s = e.shiftKey ? 10 : 1;
      const dx = e.key === "ArrowRight" ? s : e.key === "ArrowLeft" ? -s : 0;
      const dy = e.key === "ArrowUp" ? s : e.key === "ArrowDown" ? -s : 0;
      host.change(ch, (cur) => movePoint(cur, sel, cur[sel][0] + dx, cur[sel][1] + dy));
    } else if (e.key === "Escape" || e.key === "Tab") {
      setSel(null);
    }
  };

  const line = sampleCurve(pts)
    .map((p) => `${p[0] + PAD},${255 - p[1] + PAD}`)
    .join(" ");
  return (
    <div data-testid={`${tid}-editor`} data-channel={ch}>
      <div className="mb-1 flex gap-1" data-testid={`${tid}-channels`}>
        {CHANNELS.map((c) => (
          <button key={c.id} className={seg(ch === c.id)} style={{ color: ch === c.id ? undefined : c.color }} onClick={() => (setCh(c.id), setSel(null))} data-testid={`${tid}-ch-${c.id}`}>
            {c.label}
          </button>
        ))}
      </div>
      <svg
        ref={svg}
        viewBox={`0 0 ${VIEW} ${VIEW}`}
        className="mx-auto block aspect-square w-full max-w-60 touch-none select-none rounded bg-neutral-900 outline-none focus-visible:ring-1 focus-visible:ring-sky-500"
        tabIndex={0}
        role="application"
        aria-label={`${ch} tone curve. Click to add a point, drag to move, double-click or Backspace to delete`}
        data-testid={`${tid}-svg`}
        data-points={JSON.stringify(pts)}
        data-selected={sel ?? ""}
        onPointerDown={onDown}
        onPointerMove={onMove}
        onPointerUp={onUp}
        onPointerCancel={onUp}
        onDoubleClick={onDouble}
        onKeyDown={onKey}
      >
        {[0, 1, 2, 3, 4].map((i) => (
          <g key={i} stroke="#404040" strokeWidth="0.6">
            <line x1={PAD + (i * 255) / 4} y1={PAD} x2={PAD + (i * 255) / 4} y2={PAD + 255} />
            <line x1={PAD} y1={PAD + (i * 255) / 4} x2={PAD + 255} y2={PAD + (i * 255) / 4} />
          </g>
        ))}
        <line x1={PAD} y1={PAD + 255} x2={PAD + 255} y2={PAD} stroke="#525252" strokeDasharray="3 3" strokeWidth="0.8" />
        <polyline points={line} fill="none" stroke={color} strokeWidth="1.6" />
        {pts.map((p, i) => (
          <circle
            key={i}
            cx={p[0] + PAD}
            cy={255 - p[1] + PAD}
            r={sel === i ? 5 : 4}
            fill={sel === i ? color : "#171717"}
            stroke={color}
            strokeWidth="1.5"
            data-testid={`${tid}-pt-${i}`}
          />
        ))}
      </svg>
      <div className="mt-1 flex items-center justify-between text-[11px] text-neutral-400">
        <span data-testid={`${tid}-info`}>
          {sel != null && pts[sel] ? `In ${pts[sel][0]}  Out ${pts[sel][1]}` : `${pts.length} / ${MAX_CURVE_POINTS} points`}
        </span>
        <button
          className="hover:text-neutral-200 disabled:opacity-40"
          disabled={isIdentity(pts)}
          onClick={() => (host.change(ch, () => IDENTITY_CURVE.map((p) => [...p] as CurvePoint)), setSel(null))}
          data-testid={`${tid}-reset`}
        >
          Reset curve
        </button>
      </div>
    </div>
  );
}

const SPLITS = ["shadowSplit", "midtoneSplit", "highlightSplit"] as const;

/** Three draggable region boundaries (0..100), Lightroom's triangles under the curve. */
function SplitBar({ editor }: { editor: Editor }) {
  const track = useRef<HTMLDivElement>(null);
  const p = editor.adj.toneCurve.parametric;
  const vals = [p.shadowSplit, p.midtoneSplit, p.highlightSplit];
  const drag = useRef<number | null>(null);
  const setSplit = (k: number, v: number, commit: boolean) => {
    (commit ? editor.change : editor.edit)((a) => {
      const cur = [a.toneCurve.parametric.shadowSplit, a.toneCurve.parametric.midtoneSplit, a.toneCurve.parametric.highlightSplit];
      const lo = k === 0 ? 0 : cur[k - 1] + 1;
      const hi = k === 2 ? 100 : cur[k + 1] - 1;
      const nv = Math.round(Math.min(hi, Math.max(lo, v)));
      return { ...a, toneCurve: { ...a.toneCurve, parametric: { ...a.toneCurve.parametric, [SPLITS[k]]: nv } } };
    }, "Tone Curve: Split");
  };
  const at = (e: React.PointerEvent) => {
    const r = track.current!.getBoundingClientRect();
    return ((e.clientX - r.left) / r.width) * 100;
  };
  return (
    <div className="mb-2 mt-1" data-testid="curve-splits">
      <div ref={track} className="relative mx-2 h-2 rounded bg-gradient-to-r from-neutral-900 to-neutral-200">
        {vals.map((v, k) => (
          <div
            key={k}
            role="slider"
            tabIndex={0}
            aria-label={["Shadows / Darks split", "Darks / Lights split", "Lights / Highlights split"][k]}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={v}
            data-testid={`curve-split-${k}`}
            className="absolute top-1 -translate-x-1/2 cursor-ew-resize touch-none border-x-[6px] border-b-[10px] border-x-transparent border-b-sky-400 outline-none focus-visible:border-b-white"
            style={{ left: `${v}%`, width: 0, height: 0 }}
            onPointerDown={(e) => {
              e.stopPropagation();
              drag.current = k;
              e.currentTarget.setPointerCapture(e.pointerId);
            }}
            onPointerMove={(e) => drag.current === k && setSplit(k, at(e), false)}
            onPointerUp={() => {
              drag.current = null;
              editor.commit();
            }}
            onDoubleClick={() => editor.change((a) => ({ ...a, toneCurve: { ...a.toneCurve, parametric: { ...a.toneCurve.parametric, [SPLITS[k]]: editor.defaults.toneCurve.parametric[SPLITS[k]] } } }), "Tone Curve: Split")}
            onKeyDown={(e) => {
              if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
                e.preventDefault();
                e.stopPropagation();
                setSplit(k, v + (e.key === "ArrowRight" ? 1 : -1) * (e.shiftKey ? 5 : 1), true);
              }
            }}
          />
        ))}
      </div>
      <div className="mt-3 flex justify-between px-2 text-[10px] text-neutral-400">
        <span>Shadows</span>
        <span>Darks</span>
        <span>Lights</span>
        <span>Highlights</span>
      </div>
    </div>
  );
}

const PARAMETRIC = [
  ["highlights", "Highlights"],
  ["lights", "Lights"],
  ["darks", "Darks"],
  ["shadows", "Shadows"],
] as const;

export function ToneCurvePanel({ editor }: { editor: Editor }) {
  const host: CurveHost = {
    curves: editor.adj.toneCurve.point,
    edit: (ch, fn) => editor.edit((a) => setPts(a, ch, fn), "Tone Curve"),
    change: (ch, fn) => editor.change((a) => setPts(a, ch, fn), "Tone Curve"),
    commit: editor.commit,
  };
  return (
    <>
      <PointCurveEditor host={host} />
      <div className="mt-2 text-[11px] font-semibold uppercase tracking-wide text-neutral-400">Region</div>
      <SplitBar editor={editor} />
      {PARAMETRIC.map(([k, label]) => (
        <NumField
          key={k}
          editor={editor}
          id={`curve-${k}`}
          label={label}
          group="Tone Curve"
          min={-100}
          max={100}
          step={1}
          get={(a) => a.toneCurve.parametric[k]}
          set={(a, v) => ({ ...a, toneCurve: { ...a.toneCurve, parametric: { ...a.toneCurve.parametric, [k]: v } } })}
        />
      ))}
    </>
  );
}
