import { useState, type ReactNode } from "react";
import { ChevronDown, ChevronRight, FileUp, RotateCcw } from "lucide-react";
import type { AdjustmentField, LutInfo, ParametricAdjustments } from "../../ipc";
import type { Editor } from "../../hooks/useEditor";
import {
  BANDS,
  BAND_COLOR,
  BASIC,
  BASIC_FIELDS,
  copyFields,
  HSL_FIELD,
  HSL_FIELDS,
  neutralAdjustments,
  posToTemp,
  PRESENCE,
  PRESENCE_FIELDS,
  tempToPos,
  type HslKind,
  type SliderDef,
} from "../../lib/adjust";
import { Slider } from "./Slider";
import { HistogramView } from "./Histogram";

function Section({ id, title, onReset, children, defaultOpen = true }: { id: string; title: string; onReset?: () => void; children: ReactNode; defaultOpen?: boolean }) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <section className="border-b border-neutral-800 py-2" data-testid={`section-${id}`}>
      <div className="mb-1 flex items-center justify-between">
        <button className="flex items-center gap-1 text-xs font-semibold uppercase tracking-wide text-neutral-300" onClick={() => setOpen(!open)}>
          {open ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
          {title}
        </button>
        {onReset && (
          <button className="text-neutral-500 hover:text-neutral-200" title={`Reset ${title}`} onClick={onReset} data-testid={`reset-${id}`}>
            <RotateCcw className="size-3.5" />
          </button>
        )}
      </div>
      {open && children}
    </section>
  );
}

const seg = (on: boolean) => `flex-1 rounded px-2 py-0.5 text-xs ${on ? "bg-sky-800 text-sky-100" : "bg-neutral-800 hover:bg-neutral-700"}`;

interface Props {
  editor: Editor;
  luts: LutInfo[];
  onImportLut: () => void;
}

export function AdjustPanel({ editor, luts, onImportLut }: Props) {
  const { adj, info, edit, commit, change } = editor;
  const [hslTab, setHslTab] = useState<HslKind>("hue");
  const resetFields = (fields: AdjustmentField[], label: string) => change((a) => copyFields(a, neutralAdjustments(), fields), label);

  const simple = (d: SliderDef) => (
    <Slider
      key={d.key}
      id={d.key}
      label={d.label}
      value={adj[d.key]}
      min={d.min}
      max={d.max}
      step={d.step}
      digits={d.digits}
      onInput={(v) => edit((a) => ({ ...a, [d.key]: v }), d.label)}
      onCommit={commit}
      onReset={() => change((a) => ({ ...a, [d.key]: 0 }), d.label)}
    />
  );

  const wb = adj.whiteBalance;
  const asShot = info?.asShot ?? { temperatureK: 5500, tint: 0 };
  const temp = wb.mode === "custom" ? wb.temperatureK : asShot.temperatureK;
  const tint = wb.mode === "custom" ? wb.tint : asShot.tint;
  const setWb = (t: number, ti: number, label: string) => edit((a) => ({ ...a, whiteBalance: { mode: "custom", temperatureK: t, tint: ti } }), label);
  const wbReady = info !== null;
  const resetWb = (t: number, ti: number, label: string) =>
    change(
      (a) => ({ ...a, whiteBalance: t === asShot.temperatureK && ti === asShot.tint ? { mode: "as_shot" } : { mode: "custom", temperatureK: t, tint: ti } }),
      label,
    );
  const lut = adj.lut;
  const setLut = (l: ParametricAdjustments["lut"], commitNow: boolean) => (commitNow ? change : edit)((a) => ({ ...a, lut: l }), "LUT");

  return (
    <div className="flex h-full flex-col overflow-y-auto px-3 pb-6" data-testid="adjust-panel">
      <div className="py-2">
        <HistogramView h={editor.histogram} />
      </div>

      <Section id="basic" title="Basic" onReset={() => resetFields(BASIC_FIELDS, "Reset Basic")}>
        <div className="mb-2 flex gap-1" data-testid="wb-mode">
          <button
            className={seg(wb.mode === "as_shot")}
            data-testid="wb-as-shot"
            onClick={() => change((a) => ({ ...a, whiteBalance: { mode: "as_shot" } }), "White Balance")}
          >
            As Shot
          </button>
          <button
            className={seg(wb.mode === "custom")}
            data-testid="wb-custom"
            onClick={() => change((a) => ({ ...a, whiteBalance: { mode: "custom", temperatureK: asShot.temperatureK, tint: asShot.tint } }), "White Balance")}
          >
            Custom
          </button>
        </div>
        <Slider
          id="temp"
          label="Temp"
          value={tempToPos(temp)}
          min={0}
          max={1}
          step={0.001}
          display={(p) => `${posToTemp(p)} K`}
          accent="#fbbf24"
          onInput={(p) => setWb(posToTemp(p), tint, "Temp")}
          onCommit={commit}
          onReset={() => resetWb(asShot.temperatureK, tint, "Temp")}
          disabled={!wbReady}
        />
        <Slider
          id="tint"
          label="Tint"
          value={tint}
          min={-150}
          max={150}
          step={1}
          accent="#e879f9"
          onInput={(v) => setWb(temp, v, "Tint")}
          onCommit={commit}
          onReset={() => resetWb(temp, asShot.tint, "Tint")}
          disabled={!wbReady}
        />
        {BASIC.map(simple)}
      </Section>

      <Section id="presence" title="Presence" onReset={() => resetFields(PRESENCE_FIELDS, "Reset Presence")}>
        {PRESENCE.map(simple)}
      </Section>

      <Section id="hsl" title="Color Mixer" onReset={() => resetFields(HSL_FIELDS, "Reset Color Mixer")}>
        <div className="mb-2 flex gap-1" data-testid="hsl-tabs">
          {(["hue", "saturation", "luminance"] as const).map((k) => (
            <button key={k} className={seg(hslTab === k)} onClick={() => setHslTab(k)} data-testid={`hsl-tab-${k}`}>
              {k === "hue" ? "Hue" : k === "saturation" ? "Sat" : "Lum"}
            </button>
          ))}
        </div>
        <button
          className="mb-1 text-[11px] text-neutral-500 hover:text-neutral-200"
          onClick={() => resetFields([HSL_FIELD[hslTab]], `Reset ${hslTab}`)}
          data-testid="hsl-reset-tab"
        >
          Reset {hslTab}
        </button>
        {BANDS.map((b) => (
          <Slider
            key={b}
            id={`hsl-${hslTab}-${b}`}
            label={b[0].toUpperCase() + b.slice(1)}
            value={adj.hsl[hslTab][b]}
            min={-100}
            max={100}
            step={1}
            accent={BAND_COLOR[b]}
            onInput={(v) => edit((a) => ({ ...a, hsl: { ...a.hsl, [hslTab]: { ...a.hsl[hslTab], [b]: v } } }), `${hslTab[0].toUpperCase()}${hslTab.slice(1)}: ${b}`)}
            onCommit={commit}
            onReset={() => change((a) => ({ ...a, hsl: { ...a.hsl, [hslTab]: { ...a.hsl[hslTab], [b]: 0 } } }), `${hslTab[0].toUpperCase()}${hslTab.slice(1)}: ${b}`)}
          />
        ))}
      </Section>

      <Section id="lut" title="LUT" onReset={() => resetFields(["lut"], "Reset LUT")}>
        <div className="mb-2 flex gap-1">
          <select
            data-testid="lut-select"
            value={lut?.id ?? ""}
            onChange={(e) => setLut(e.target.value ? { id: e.target.value, amount: lut?.amount ?? 100 } : null, true)}
            className="min-w-0 flex-1 rounded bg-neutral-800 px-1.5 py-1 text-xs"
          >
            <option value="">None</option>
            {lut && !luts.some((l) => l.id === lut.id) && <option value={lut.id}>{lut.id} (missing)</option>}
            {luts.map((l) => (
              <option key={l.id} value={l.id}>
                {l.name}
              </option>
            ))}
          </select>
          <button className="flex items-center gap-1 rounded bg-neutral-800 px-2 py-1 text-xs hover:bg-neutral-700" onClick={onImportLut} data-testid="lut-import" title="Import .cube LUT">
            <FileUp className="size-3.5" /> Import
          </button>
        </div>
        {editor.main?.lutMissing && (
          <p className="mb-1 text-[11px] text-amber-400" data-testid="lut-missing">
            LUT not found in the library; rendered without it.
          </p>
        )}
        {lut && (
          <Slider
            id="lut-amount"
            label="Amount"
            value={lut.amount}
            min={0}
            max={100}
            step={1}
            onInput={(v) => setLut({ ...lut, amount: v }, false)}
            onCommit={commit}
            onReset={() => change((a) => ({ ...a, lut: a.lut ? { ...a.lut, amount: 100 } : null }), "LUT")}
          />
        )}
      </Section>
    </div>
  );
}
