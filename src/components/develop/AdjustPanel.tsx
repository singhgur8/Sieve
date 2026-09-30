import { useState } from "react";
import { FileUp, Pipette } from "lucide-react";
import type { AdjustmentField, LutInfo, ParametricAdjustments } from "../../ipc";
import type { Editor } from "../../hooks/useEditor";
import {
  BANDS,
  BAND_COLOR,
  BASIC,
  BASIC_FIELDS,
  copyFields,
  HSL_BW_FIELDS,
  HSL_FIELD,
  posToTemp,
  sameAdjustments,
  TEMP_MAX,
  TEMP_MIN,
  PRESENCE,
  PRESENCE_FIELDS,
  tempToPos,
  type HslKind,
  type SliderDef,
} from "../../lib/adjust";
import { Slider } from "./Slider";
import { HistogramView } from "./Histogram";
import { Section, seg } from "./fields";
import { ProfilePanel } from "./ProfilePanel";
import { ToneCurvePanel } from "./ToneCurvePanel";
import { ColorGradingPanel } from "./ColorGradingPanel";
import { CalibrationPanel, DetailPanel, EffectsPanel } from "./DetailPanels";
import { CropPanel, type CropApi } from "./CropPanel";
import { hint } from "../../lib/keymap";

interface Props {
  editor: Editor;
  luts: LutInfo[];
  onImportLut: () => void;
  imageId: number | null;
  onError: (e: unknown) => void;
  crop: CropApi;
  /** White balance eyedropper (W). */
  picker: { active: boolean; toggle: () => void };
}

export function AdjustPanel({ editor, luts, onImportLut, imageId, onError, crop, picker }: Props) {
  const { adj, info, edit, commit, change } = editor;
  const [hslTab, setHslTab] = useState<HslKind>("hue");
  const resetFields = (fields: AdjustmentField[], label: string) => change((a) => copyFields(a, editor.defaults, fields), label);
  /** Some field of the section differs from its default (section dot). */
  const dirty = (fields: AdjustmentField[]) => !sameAdjustments(copyFields(adj, editor.defaults, fields), adj);

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
      defaultValue={editor.defaults[d.key]}
      onInput={(v) => edit((a) => ({ ...a, [d.key]: v }), d.label)}
      onCommit={commit}
      onReset={() => change((a) => ({ ...a, [d.key]: editor.defaults[d.key] }), d.label)}
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

      <Section id="crop" dirty={adj.crop.enabled} title="Crop" onReset={() => resetFields(["crop"], "Reset Crop")}>
        <CropPanel editor={editor} crop={crop} />
      </Section>

      <Section id="profile" dirty={dirty(["profile"])} title="Profile" onReset={() => resetFields(["profile"], "Reset Profile")}>
        <ProfilePanel editor={editor} imageId={imageId} onError={onError} />
      </Section>

      <Section id="basic" dirty={dirty(BASIC_FIELDS)} title="Basic" onReset={() => resetFields(BASIC_FIELDS, "Reset Basic")}>
        <div className="mb-2 flex gap-1" data-testid="wb-mode">
          <button
            className={`flex items-center justify-center rounded px-2 py-0.5 ${picker.active ? "bg-sky-800 text-sky-100" : "bg-neutral-800 hover:bg-neutral-700"}`}
            data-testid="wb-picker"
            aria-pressed={picker.active}
            title={`White balance picker${hint("wbPicker")}`}
            onClick={picker.toggle}
          >
            <Pipette className="size-3.5" />
          </button>
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
          defaultValue={tempToPos(asShot.temperatureK)}
          editText={(p) => String(posToTemp(p))}
          parse={(t) => {
            const k = Number.parseFloat(t);
            return Number.isFinite(k) ? tempToPos(Math.min(TEMP_MAX, Math.max(TEMP_MIN, k))) : null;
          }}
          textStep={50}
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
          defaultValue={asShot.tint}
          accent="#e879f9"
          onInput={(v) => setWb(temp, v, "Tint")}
          onCommit={commit}
          onReset={() => resetWb(temp, asShot.tint, "Tint")}
          disabled={!wbReady}
        />
        {BASIC.map(simple)}
      </Section>

      <Section id="presence" dirty={dirty(PRESENCE_FIELDS)} title="Presence" onReset={() => resetFields(PRESENCE_FIELDS, "Reset Presence")}>
        {PRESENCE.map(simple)}
      </Section>

      <Section id="tone-curve" dirty={dirty(["tone_curve"])} title="Tone Curve" onReset={() => resetFields(["tone_curve"], "Reset Tone Curve")}>
        <ToneCurvePanel editor={editor} />
      </Section>

      <Section id="hsl" dirty={dirty(HSL_BW_FIELDS)} title={adj.blackAndWhite.enabled ? "Black & White" : "Color Mixer"} onReset={() => resetFields(HSL_BW_FIELDS, "Reset Color Mixer")}>
        <div className="mb-2 flex gap-1" data-testid="bw-mode">
          <button className={seg(!adj.blackAndWhite.enabled)} onClick={() => change((a) => ({ ...a, blackAndWhite: { ...a.blackAndWhite, enabled: false } }), "Color")} data-testid="bw-off">
            Color
          </button>
          <button className={seg(adj.blackAndWhite.enabled)} onClick={() => change((a) => ({ ...a, blackAndWhite: { ...a.blackAndWhite, enabled: true } }), "Black & White")} data-testid="bw-on">
            Black &amp; White
          </button>
        </div>
        {adj.blackAndWhite.enabled ? (
          <div data-testid="bw-mixer">
            {BANDS.map((b) => (
              <Slider
                key={b}
                id={`bw-${b}`}
                label={b[0].toUpperCase() + b.slice(1)}
                value={adj.blackAndWhite.mixer[b]}
                min={-100}
                max={100}
                step={1}
                accent={BAND_COLOR[b]}
                onInput={(v) => edit((a) => ({ ...a, blackAndWhite: { ...a.blackAndWhite, mixer: { ...a.blackAndWhite.mixer, [b]: v } } }), `B&W: ${b}`)}
                onCommit={commit}
                onReset={() => change((a) => ({ ...a, blackAndWhite: { ...a.blackAndWhite, mixer: { ...a.blackAndWhite.mixer, [b]: editor.defaults.blackAndWhite.mixer[b] } } }), `B&W: ${b}`)}
              />
            ))}
          </div>
        ) : (
          <>
            <div className="mb-2 flex gap-1" data-testid="hsl-tabs">
              {(["hue", "saturation", "luminance"] as const).map((k) => (
                <button key={k} className={seg(hslTab === k)} onClick={() => setHslTab(k)} data-testid={`hsl-tab-${k}`}>
                  {k === "hue" ? "Hue" : k === "saturation" ? "Sat" : "Lum"}
                </button>
              ))}
            </div>
            <button
              className="mb-1 text-[11px] text-neutral-400 hover:text-neutral-200"
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
          </>
        )}
      </Section>

      <Section id="color-grading" dirty={dirty(["color_grading"])} title="Color Grading" onReset={() => resetFields(["color_grading"], "Reset Color Grading")}>
        <ColorGradingPanel editor={editor} />
      </Section>

      <Section id="detail" dirty={dirty(["sharpening", "noise_reduction"])} title="Detail" onReset={() => resetFields(["sharpening", "noise_reduction"], "Reset Detail")}>
        <DetailPanel editor={editor} />
      </Section>

      <Section id="effects" dirty={dirty(["vignette", "grain"])} title="Effects" onReset={() => resetFields(["vignette", "grain"], "Reset Effects")}>
        <EffectsPanel editor={editor} />
      </Section>

      <Section id="lut" dirty={dirty(["lut"])} title="LUT" onReset={() => resetFields(["lut"], "Reset LUT")}>
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
            defaultValue={100}
            onInput={(v) => setLut({ ...lut, amount: v }, false)}
            onCommit={commit}
            onReset={() => change((a) => ({ ...a, lut: a.lut ? { ...a.lut, amount: 100 } : null }), "LUT")}
          />
        )}
      </Section>

      <Section id="calibration" dirty={dirty(["calibration"])} title="Calibration" onReset={() => resetFields(["calibration"], "Reset Calibration")}>
        <CalibrationPanel editor={editor} />
      </Section>
    </div>
  );
}
