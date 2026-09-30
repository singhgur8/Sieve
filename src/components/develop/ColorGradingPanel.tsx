// Color Grading: shadows / midtones / highlights / global wheels (hue + saturation puck, luminance slider),
// blending and balance.
import { useRef, useState } from "react";
import type { ColorWheel, CompleteAdjustments } from "../../ipc";
import type { Editor } from "../../hooks/useEditor";
import { NumField, seg } from "./fields";

type Zone = "shadows" | "midtones" | "highlights" | "global";
const ZONES: { id: Zone; label: string }[] = [
  { id: "shadows", label: "Shadows" },
  { id: "midtones", label: "Midtones" },
  { id: "highlights", label: "Highlights" },
  { id: "global", label: "Global" },
];

const SHORT: Record<Zone, string> = { shadows: "Shadows", midtones: "Mids", highlights: "Highs", global: "Global" };

const SIZE = 148;
const R = SIZE / 2;

const setWheel = (a: CompleteAdjustments, z: Zone, w: Partial<ColorWheel>): CompleteAdjustments => ({
  ...a,
  colorGrading: { ...a.colorGrading, [z]: { ...a.colorGrading[z], ...w } },
});

/** Puck offset from the wheel centre (px) for a hue/saturation pair. Hue 0 = top, clockwise. */
export const puckOffset = (hue: number, sat: number): [number, number] => {
  const rad = (hue * Math.PI) / 180;
  const r = (sat / 100) * (R - 6);
  return [Math.sin(rad) * r, -Math.cos(rad) * r];
};

/** Inverse of `puckOffset`. */
export function hueSatAt(dx: number, dy: number): { hue: number; saturation: number } {
  const r = Math.min(R - 6, Math.hypot(dx, dy));
  const hue = ((Math.atan2(dx, -dy) * 180) / Math.PI + 360) % 360;
  return { hue: Math.round(hue), saturation: Math.round((r / (R - 6)) * 100) };
}

function Wheel({ editor, zone }: { editor: Editor; zone: Zone }) {
  const ref = useRef<HTMLDivElement>(null);
  const drag = useRef<{ x: number; y: number; px: number; py: number } | null>(null);
  const w = editor.adj.colorGrading[zone];
  const [px, py] = puckOffset(w.hue, w.saturation);
  const label = ZONES.find((z) => z.id === zone)!.label;

  const apply = (dx: number, dy: number) => {
    const hs = hueSatAt(dx, dy);
    editor.edit((a) => setWheel(a, zone, hs), `Color Grading: ${label}`);
  };
  const centre = (e: { clientX: number; clientY: number }) => {
    const r = ref.current!.getBoundingClientRect();
    return [e.clientX - (r.left + r.width / 2), e.clientY - (r.top + r.height / 2)] as const;
  };
  return (
    <div
      ref={ref}
      className="relative mx-auto my-1 touch-none select-none rounded-full"
      style={{
        width: SIZE,
        height: SIZE,
        background:
          "radial-gradient(circle, rgba(128,128,128,1) 0%, rgba(128,128,128,0) 72%), conic-gradient(hsl(0 85% 55%), hsl(60 85% 55%), hsl(120 85% 55%), hsl(180 85% 55%), hsl(240 85% 55%), hsl(300 85% 55%), hsl(360 85% 55%))",
      }}
      data-testid={`wheel-${zone}`}
      data-hue={w.hue}
      data-sat={w.saturation}
      title="Drag to set hue and saturation (Shift = fine), double-click to reset"
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        e.currentTarget.setPointerCapture(e.pointerId);
        const [dx, dy] = centre(e);
        drag.current = { x: e.clientX, y: e.clientY, px: dx, py: dy };
        // Shift = fine: move relative to the current puck instead of jumping to the pointer.
        if (!e.shiftKey) apply(dx, dy);
        else drag.current = { x: e.clientX, y: e.clientY, px, py };
      }}
      onPointerMove={(e) => {
        const d = drag.current;
        if (!d) return;
        if (e.shiftKey) {
          const nx = d.px + (e.clientX - d.x) * 0.2;
          const ny = d.py + (e.clientY - d.y) * 0.2;
          drag.current = { x: e.clientX, y: e.clientY, px: nx, py: ny };
          apply(nx, ny);
        } else {
          const [dx, dy] = centre(e);
          drag.current = { x: e.clientX, y: e.clientY, px: dx, py: dy };
          apply(dx, dy);
        }
      }}
      onPointerUp={() => {
        if (!drag.current) return;
        drag.current = null;
        editor.commit();
      }}
      onDoubleClick={() => editor.change((a) => setWheel(a, zone, { hue: 0, saturation: 0 }), `Color Grading: ${label}`)}
    >
      <div className="pointer-events-none absolute inset-0 rounded-full ring-1 ring-neutral-600" />
      <div
        className="pointer-events-none absolute size-3.5 rounded-full border-2 border-white shadow"
        style={{ left: R + px - 7, top: R + py - 7, background: w.saturation > 0 ? `hsl(${w.hue} 90% 55%)` : "#888" }}
        data-testid={`wheel-puck-${zone}`}
      />
    </div>
  );
}

export function ColorGradingPanel({ editor }: { editor: Editor }) {
  const [zone, setZone] = useState<Zone>("shadows");
  const cg = editor.adj.colorGrading;
  const label = ZONES.find((z) => z.id === zone)!.label;
  const g = `Color Grading: ${label}`;
  return (
    <>
      <div className="mb-1 flex gap-1" data-testid="grading-zones">
        {ZONES.map((z) => (
          <button key={z.id} className={`${seg(zone === z.id)} flex items-center justify-center gap-1 px-1 text-[11px]`} onClick={() => setZone(z.id)} data-testid={`grading-zone-${z.id}`} title={z.label}>
            <span className="size-2 rounded-full border border-neutral-500" style={{ background: cg[z.id].saturation > 0 ? `hsl(${cg[z.id].hue} 90% 55%)` : "transparent" }} />
            {SHORT[z.id]}
          </button>
        ))}
      </div>
      <Wheel editor={editor} zone={zone} />
      <NumField editor={editor} id="grading-hue" label="Hue" group={g} min={0} max={360} step={1} accent="#a3a3a3" get={(a) => a.colorGrading[zone].hue} set={(a, v) => setWheel(a, zone, { hue: v })} />
      <NumField editor={editor} id="grading-sat" label="Saturation" group={g} min={0} max={100} step={1} accent="#a3a3a3" get={(a) => a.colorGrading[zone].saturation} set={(a, v) => setWheel(a, zone, { saturation: v })} />
      <NumField editor={editor} id="grading-lum" label="Luminance" group={g} min={-100} max={100} step={1} get={(a) => a.colorGrading[zone].luminance} set={(a, v) => setWheel(a, zone, { luminance: v })} />
      <div className="mt-2 border-t border-neutral-800 pt-2">
        <NumField editor={editor} id="grading-blending" label="Blending" group="Color Grading" min={0} max={100} step={1} get={(a) => a.colorGrading.blending} set={(a, v) => ({ ...a, colorGrading: { ...a.colorGrading, blending: v } })} />
        <NumField editor={editor} id="grading-balance" label="Balance" group="Color Grading" min={-100} max={100} step={1} get={(a) => a.colorGrading.balance} set={(a, v) => ({ ...a, colorGrading: { ...a.colorGrading, balance: v } })} />
      </div>
    </>
  );
}
