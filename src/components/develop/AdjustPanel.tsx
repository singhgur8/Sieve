// Develop right panel, Lightroom order (docs/ux-spec-8b.md 5.4): Histogram, tool strip (Crop, Masking), Basic (Treatment,
// Profile, WB, Tone with Auto, Presence), Tone Curve, HSL / Color, Color Grading, Detail, Effects, Calibration, bottom bar.
import { memo, useCallback, useMemo, useRef, useState, type ReactNode } from "react";
import { CircleDashed, Crop as CropIcon, Loader2, Pipette, RotateCcw, RefreshCw, History, Wand2 } from "lucide-react";
import type { AdjustmentField } from "../../ipc";
import { BUSY_WHY, useActivityRunning } from "../../lib/activity";
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
  type SimpleKey,
  type SliderDef,
} from "../../lib/adjust";
import { Slider } from "./Slider";
import { HistogramView } from "./Histogram";
import { Section, seg } from "./fields";
import { ProfileBrowser, ProfileRow, useProfileCatalog, type ProfileHover } from "./ProfilePanel";
import { ToneCurvePanel } from "./ToneCurvePanel";
import { ColorGradingPanel } from "./ColorGradingPanel";
import { CalibrationPanel, DetailPanel, EffectsPanel } from "./DetailPanels";
import { CropPanel, type CropApi } from "./CropPanel";
import { TransformPanel, type GuidedApi } from "./TransformPanel";
import type { UprightApi } from "../../hooks/useUpright";
import { hint } from "../../lib/keymap";

const BASIC_ALL: AdjustmentField[] = [...BASIC_FIELDS, ...PRESENCE_FIELDS, "profile", "black_and_white"];

export interface BottomBar {
  /** Photos the bar acts on (> 1: Sync… replaces Previous and Reset shows the count). */
  count: number;
  hasPrevious: boolean;
  onPrevious: () => void;
  /** Sync… (Alt held: no dialog). */
  onSync: (alt: boolean) => void;
  onReset: () => void;
}

/** Lightroom's Auto buttons (Tone, WB, Shift+double-click on a slider), driven by DevelopView (v14 `auto_tone` / `auto_white_balance`). */
export interface AutoApi {
  busy: boolean;
  /** Generic Auto: tone + white balance from the photo (no learned style needed), one history entry. */
  all: () => void;
  tone: () => void;
  wb: () => void;
  /** Shift+double-click: auto for one slider (temp / tint use the white balance, the rest `auto_tone` with that key). */
  slider: (key: "temp" | "tint" | SimpleKey) => void;
  /** The white balance is the one Auto produced (the WB select reads "Auto" until Temp / Tint are touched). */
  wbIsAuto: boolean;
}

interface Props {
  editor: Editor;
  /** Bumped when the style library changed (imports): the profile lists re-read. */
  styleVersion: number;
  importing: boolean;
  onImportStyles: () => void;
  hover: ProfileHover;
  auto: AutoApi;
  /** Press-and-hold of a section's changed dot: the photo without that section's changes (renderPreviewVariant). */
  hold: { start: (title: string, fields: AdjustmentField[]) => void; stop: () => void };
  imageId: number | null;
  onError: (e: unknown) => void;
  crop: CropApi;
  /** Transform section: Upright buttons / Guided tool. */
  upright: UprightApi;
  guided: GuidedApi;
  /** White balance eyedropper (W). */
  picker: { active: boolean; toggle: () => void };
  /** Masking tool: the strip button toggles the Masks panel, which replaces the section list while open. */
  masks: { open: boolean; count: number; toggle: () => void; panel: ReactNode };
  /** Profile Browser (replaces the section list while open). */
  browser: { open: boolean; setOpen: (v: boolean) => void };
  /** "ISO 800 · 85 mm · f/1.8 · 1/250 s" (null when the file has no EXIF). */
  exif: string | null;
  bar: BottomBar;
}

const SubHead = ({ children, action }: { children: string; action?: ReactNode }) => (
  <div className="mt-2 flex h-5 items-center justify-between">
    <span className="text-[11px] font-semibold uppercase tracking-wide text-neutral-400">{children}</span>
    {action}
  </div>
);

const stripBtn = (on: boolean) => `relative flex size-7 items-center justify-center rounded ${on ? "bg-sky-800 text-sky-100" : "text-neutral-300 hover:bg-neutral-800"}`;

const AUTO_KEYS: SimpleKey[] = ["exposure", "contrast", "highlights", "shadows", "whites", "blacks", "vibrance", "saturation"];

type EditFn = Editor["edit"];
type ChangeFn = Editor["change"];
interface RowCbs {
  edit: EditFn;
  commit: () => void;
  change: ChangeFn;
}

// Slider rows take only primitives and the editor's stable callbacks, so a frame of a drag re-renders just the row
// being dragged (Phase 8d: the rest of the panel is skipped by memo).
const SimpleRow = memo(function SimpleRow({ d, value, def, auto, edit, commit, change }: RowCbs & { d: SliderDef; value: number; def: number; auto?: (key: SimpleKey) => void }) {
  return (
    <Slider
      id={d.key}
      label={d.label}
      value={value}
      min={d.min}
      max={d.max}
      step={d.step}
      digits={d.digits}
      defaultValue={def}
      onInput={(v) => edit((a) => ({ ...a, [d.key]: v }), d.label)}
      onCommit={commit}
      onReset={() => change((a) => ({ ...a, [d.key]: def }), d.label)}
      onAuto={auto ? () => auto(d.key) : undefined}
    />
  );
});

const BwRow = memo(function BwRow({ band, value, def, edit, commit, change }: RowCbs & { band: (typeof BANDS)[number]; value: number; def: number }) {
  return (
    <Slider
      id={`bw-${band}`}
      label={band[0].toUpperCase() + band.slice(1)}
      value={value}
      min={-100}
      max={100}
      step={1}
      accent={BAND_COLOR[band]}
      onInput={(v) => edit((a) => ({ ...a, blackAndWhite: { ...a.blackAndWhite, mixer: { ...a.blackAndWhite.mixer, [band]: v } } }), `B&W: ${band}`)}
      onCommit={commit}
      onReset={() => change((a) => ({ ...a, blackAndWhite: { ...a.blackAndWhite, mixer: { ...a.blackAndWhite.mixer, [band]: def } } }), `B&W: ${band}`)}
    />
  );
});

const HslRow = memo(function HslRow({ kind, band, value, edit, commit, change }: RowCbs & { kind: HslKind; band: (typeof BANDS)[number]; value: number }) {
  const name = `${kind[0].toUpperCase()}${kind.slice(1)}: ${band}`;
  return (
    <Slider
      id={`hsl-${kind}-${band}`}
      label={band[0].toUpperCase() + band.slice(1)}
      value={value}
      min={-100}
      max={100}
      step={1}
      accent={BAND_COLOR[band]}
      onInput={(v) => edit((a) => ({ ...a, hsl: { ...a.hsl, [kind]: { ...a.hsl[kind], [band]: v } } }), name)}
      onCommit={commit}
      onReset={() => change((a) => ({ ...a, hsl: { ...a.hsl, [kind]: { ...a.hsl[kind], [band]: 0 } } }), name)}
    />
  );
});

const tempDisplay = (p: number) => `${posToTemp(p)} K`;
const tempEditText = (p: number) => String(posToTemp(p));
const tempParse = (t: string) => {
  const k = Number.parseFloat(t);
  return Number.isFinite(k) ? tempToPos(Math.min(TEMP_MAX, Math.max(TEMP_MIN, k))) : null;
};

interface WbRowProps extends RowCbs {
  kind: "temp" | "tint";
  /** Slider position (temp) or tint value. */
  value: number;
  /** As-shot default in slider units. */
  def: number;
  /** The other WB component (Kelvin for tint rows, tint for temp rows). */
  other: number;
  asShot: { temperatureK: number; tint: number };
  disabled: boolean;
  onAuto: (key: "temp" | "tint") => void;
}
const WbRow = memo(function WbRow({ kind, value, def, other, asShot, disabled, onAuto, edit, commit, change }: WbRowProps) {
  const isTemp = kind === "temp";
  const label = isTemp ? "Temp" : "Tint";
  const setWb = (t: number, ti: number) => edit((a) => ({ ...a, whiteBalance: { mode: "custom", temperatureK: t, tint: ti } }), label);
  const resetWb = (t: number, ti: number) =>
    change((a) => ({ ...a, whiteBalance: t === asShot.temperatureK && ti === asShot.tint ? { mode: "as_shot" } : { mode: "custom", temperatureK: t, tint: ti } }), label);
  return isTemp ? (
    <Slider
      id="temp"
      label={label}
      value={value}
      min={0}
      max={1}
      step={0.001}
      display={tempDisplay}
      defaultValue={def}
      editText={tempEditText}
      parse={tempParse}
      textStep={50}
      accent="#fbbf24"
      onInput={(p) => setWb(posToTemp(p), other)}
      onCommit={commit}
      onReset={() => resetWb(asShot.temperatureK, other)}
      onAuto={() => onAuto("temp")}
      disabled={disabled}
    />
  ) : (
    <Slider
      id="tint"
      label={label}
      value={value}
      min={-150}
      max={150}
      step={1}
      defaultValue={def}
      accent="#e879f9"
      onInput={(v) => setWb(other, v)}
      onCommit={commit}
      onReset={() => resetWb(other, asShot.tint)}
      onAuto={() => onAuto("tint")}
      disabled={disabled}
    />
  );
});

export function AdjustPanel({ editor, styleVersion, importing, onImportStyles, hover, hold, auto, imageId, onError, crop, upright, guided, picker, masks, browser, exif, bar }: Props) {
  const syncing = useActivityRunning("paste_sync");
  const { adj, info, edit, commit, change } = editor;
  const [hslTab, setHslTab] = useState<HslKind>("hue");
  const catalog = useProfileCatalog(imageId, onError, styleVersion);
  const resetFields = (fields: AdjustmentField[], label: string) => change((a) => copyFields(a, editor.defaults, fields), label);
  /** Some field of the section differs from its default (section dot). */
  const dirtyCache = useMemo(() => new Map<string, boolean>(), [adj, editor.defaults]);
  const dirty = (fields: AdjustmentField[]) => {
    const k = fields.join(",");
    let v = dirtyCache.get(k);
    if (v === undefined) dirtyCache.set(k, (v = !sameAdjustments(copyFields(adj, editor.defaults, fields), adj)));
    return v;
  };

  const holdOf = (title: string, fields: AdjustmentField[]) => (down: boolean) => (down ? hold.start(title, fields) : hold.stop());

  const autoRef = useRef(auto);
  autoRef.current = auto;
  const autoSlider = useCallback((key: "temp" | "tint" | SimpleKey) => autoRef.current.slider(key), []);
  const simple = (d: SliderDef) => (
    <SimpleRow key={d.key} d={d} value={adj[d.key]} def={editor.defaults[d.key]} auto={AUTO_KEYS.includes(d.key) ? autoSlider : undefined} edit={edit} commit={commit} change={change} />
  );

  const wb = adj.whiteBalance;
  const asShot = info?.asShot ?? { temperatureK: 5500, tint: 0 };
  const temp = wb.mode === "custom" ? wb.temperatureK : asShot.temperatureK;
  const tint = wb.mode === "custom" ? wb.tint : asShot.tint;
  const wbReady = info !== null;

  return (
    <div className="flex h-full flex-col" data-testid="adjust-panel">
      {/* The Masks panel needs the height: the histogram steps aside while it is open. */}
      {!masks.open && (
        <div className="shrink-0 px-3 pt-2">
          <HistogramView h={editor.histogram} />
          <p className="h-5 truncate text-[11px] leading-5 text-neutral-400" data-testid="histogram-info">
            {exif}
          </p>
        </div>
      )}

      <div className={`flex h-8 shrink-0 items-center gap-1 border-b border-neutral-800 px-3 ${masks.open ? "border-t" : ""}`} data-testid="tool-strip">
        <button className={stripBtn(crop.tool !== null)} onClick={() => (crop.tool ? crop.commit() : crop.start())} title={`Crop and straighten${hint("crop")}`} aria-pressed={crop.tool !== null} data-testid="tool-crop">
          <CropIcon className="size-4" />
          {adj.crop.enabled && crop.tool === null && <span className="absolute right-0.5 top-0.5 size-1.5 rounded-full bg-sky-400" title="Cropped" data-testid="tool-crop-dot" />}
        </button>
        <button className={stripBtn(masks.open)} onClick={masks.toggle} title={`Masking: local adjustments${hint("maskPanel")}`} aria-pressed={masks.open} data-testid="tool-masking" data-count={masks.count}>
          <CircleDashed className="size-4" />
          {masks.count > 0 && <span className="absolute -right-1 -top-1 min-w-3.5 rounded-full bg-neutral-600 px-1 text-[9px] leading-3.5 text-white">{masks.count}</span>}
        </button>
      </div>

      {masks.open ? (
        <div className="min-h-0 flex-1">{masks.panel}</div>
      ) : (
        <div className="min-h-0 flex-1 overflow-y-auto px-3 pb-3" data-testid="adjust-scroll">
          <CropPanel crop={crop} />
          {browser.open ? (
            <ProfileBrowser editor={editor} catalog={catalog} importing={importing} onImport={onImportStyles} hover={hover} onClose={() => browser.setOpen(false)} />
          ) : (
            <>
              <Section id="basic" dirty={dirty(BASIC_ALL)} onHold={holdOf("Basic", BASIC_ALL)} title="Basic" onReset={() => resetFields(BASIC_ALL, "Reset Basic")}>
                <div className="flex h-7 items-center gap-2" data-testid="auto-row">
                  <button
                    className="flex h-6 items-center gap-1 rounded bg-sky-800 px-3 text-xs font-medium text-sky-50 hover:bg-sky-700 disabled:opacity-60"
                    disabled={auto.busy}
                    onClick={auto.all}
                    title={`Auto: set exposure, contrast, highlights, shadows and white balance from this photo's analysis. Works on any photo, no learned style needed. Not the same as "Auto edit (my style)", which applies your own editing style to a whole scene.`}
                    data-testid="auto-all"
                  >
                    {auto.busy ? <Loader2 className="size-3 animate-spin" /> : <Wand2 className="size-3" />} Auto
                  </button>
                  <span className="min-w-0 truncate text-[11px] text-neutral-400">tone and white balance from this photo</span>
                </div>
                <div className="flex h-7 items-center gap-2" data-testid="bw-mode">
                  <span className="w-[72px] shrink-0 text-xs text-neutral-300">Treatment</span>
                  <div className="flex flex-1 gap-1">
                    <button className={seg(!adj.blackAndWhite.enabled)} onClick={() => change((a) => ({ ...a, blackAndWhite: { ...a.blackAndWhite, enabled: false } }), "Color")} data-testid="bw-off">
                      Color
                    </button>
                    <button className={seg(adj.blackAndWhite.enabled)} onClick={() => change((a) => ({ ...a, blackAndWhite: { ...a.blackAndWhite, enabled: true } }), "Black & White")} data-testid="bw-on">
                      Black &amp; White
                    </button>
                  </div>
                </div>
                <ProfileRow editor={editor} catalog={catalog} onBrowse={() => browser.setOpen(true)} />
                <div className="flex h-7 items-center gap-2" data-testid="wb-mode">
                  <span className="w-[72px] shrink-0 text-xs text-neutral-300">WB</span>
                  <button
                    className={`flex size-6 shrink-0 items-center justify-center rounded ${picker.active ? "bg-sky-800 text-sky-100" : "bg-neutral-800 hover:bg-neutral-700"}`}
                    data-testid="wb-picker"
                    aria-pressed={picker.active}
                    title={`White balance picker${hint("wbPicker")}; Auto white balance${hint("autoWb")}`}
                    onClick={picker.toggle}
                  >
                    <Pipette className="size-3.5" />
                  </button>
                  <select
                    className="h-6 min-w-0 flex-1 rounded bg-neutral-800 px-1.5 text-xs"
                    value={wb.mode === "as_shot" ? "as_shot" : auto.wbIsAuto ? "auto" : "custom"}
                    aria-label="White balance"
                    data-testid="wb-select"
                    onChange={(e) => {
                      const v = e.target.value;
                      if (v === "as_shot") change((a) => ({ ...a, whiteBalance: { mode: "as_shot" } }), "White Balance");
                      else if (v === "custom") change((a) => ({ ...a, whiteBalance: { mode: "custom", temperatureK: asShot.temperatureK, tint: asShot.tint } }), "White Balance");
                      else auto.wb();
                    }}
                  >
                    <option value="as_shot">As Shot</option>
                    <option value="auto">Auto</option>
                    <option value="custom">Custom</option>
                  </select>
                </div>
                <WbRow kind="temp" value={tempToPos(temp)} def={tempToPos(asShot.temperatureK)} other={tint} asShot={asShot} disabled={!wbReady} onAuto={autoSlider} edit={edit} commit={commit} change={change} />
                <WbRow kind="tint" value={tint} def={asShot.tint} other={temp} asShot={asShot} disabled={!wbReady} onAuto={autoSlider} edit={edit} commit={commit} change={change} />
                <SubHead
                  action={
                    <button
                      className="flex h-5 items-center gap-1 rounded bg-neutral-800 px-2 text-[11px] hover:bg-neutral-700 disabled:opacity-40"
                      disabled={auto.busy}
                      onClick={auto.tone}
                      title={`Auto tone only: exposure, contrast, highlights, shadows, whites, blacks (white balance is untouched)${hint("autoTone")}`}
                      data-testid="auto-tone"
                    >
                      {auto.busy && <Loader2 className="size-3 animate-spin" />} Auto
                    </button>
                  }
                >
                  Tone
                </SubHead>
                {BASIC.map(simple)}
                <SubHead>Presence</SubHead>
                {PRESENCE.map(simple)}
              </Section>

              <Section id="tone-curve" dirty={dirty(["tone_curve"])} onHold={holdOf("Tone Curve", ["tone_curve"])} title="Tone Curve" onReset={() => resetFields(["tone_curve"], "Reset Tone Curve")}>
                <ToneCurvePanel editor={editor} />
              </Section>

              <Section id="hsl" dirty={dirty(HSL_BW_FIELDS)} onHold={holdOf("Color", HSL_BW_FIELDS)} title={adj.blackAndWhite.enabled ? "B&W" : "HSL / Color"} onReset={() => resetFields(HSL_BW_FIELDS, "Reset Color Mixer")}>
                {adj.blackAndWhite.enabled ? (
                  <div data-testid="bw-mixer">
                    {BANDS.map((b) => (
                      <BwRow key={b} band={b} value={adj.blackAndWhite.mixer[b]} def={editor.defaults.blackAndWhite.mixer[b]} edit={edit} commit={commit} change={change} />
                    ))}
                  </div>
                ) : (
                  <>
                    <div className="mb-2 flex gap-1" data-testid="hsl-tabs">
                      {(["hue", "saturation", "luminance"] as const).map((k) => (
                        <button key={k} className={seg(hslTab === k)} onClick={() => setHslTab(k)} data-testid={`hsl-tab-${k}`}>
                          {k === "hue" ? "Hue" : k === "saturation" ? "Saturation" : "Luminance"}
                        </button>
                      ))}
                    </div>
                    <button className="mb-1 text-[11px] text-neutral-400 hover:text-neutral-200" onClick={() => resetFields([HSL_FIELD[hslTab]], `Reset ${hslTab}`)} data-testid="hsl-reset-tab">
                      Reset {hslTab}
                    </button>
                    {BANDS.map((b) => (
                      <HslRow key={`${hslTab}-${b}`} kind={hslTab} band={b} value={adj.hsl[hslTab][b]} edit={edit} commit={commit} change={change} />
                    ))}
                  </>
                )}
              </Section>

              <Section id="color-grading" dirty={dirty(["color_grading"])} onHold={holdOf("Color Grading", ["color_grading"])} title="Color Grading" onReset={() => resetFields(["color_grading"], "Reset Color Grading")}>
                <ColorGradingPanel editor={editor} />
              </Section>

              <Section id="detail" dirty={dirty(["sharpening", "noise_reduction"])} onHold={holdOf("Detail", ["sharpening", "noise_reduction"])} title="Detail" onReset={() => resetFields(["sharpening", "noise_reduction"], "Reset Detail")}>
                <DetailPanel editor={editor} />
              </Section>

              <Section id="transform" dirty={dirty(["transform"])} onHold={holdOf("Transform", ["transform"])} title="Transform" onReset={() => resetFields(["transform"], "Reset Transform")}>
                <TransformPanel editor={editor} upright={upright} guided={guided} />
              </Section>

              <Section id="effects" dirty={dirty(["vignette", "grain"])} onHold={holdOf("Effects", ["vignette", "grain"])} title="Effects" onReset={() => resetFields(["vignette", "grain"], "Reset Effects")}>
                <EffectsPanel editor={editor} />
              </Section>

              <Section id="calibration" dirty={dirty(["calibration"])} onHold={holdOf("Calibration", ["calibration"])} title="Calibration" onReset={() => resetFields(["calibration"], "Reset Calibration")}>
                <CalibrationPanel editor={editor} />
              </Section>
            </>
          )}
        </div>
      )}

      <div className="flex h-9 shrink-0 gap-2 border-t border-neutral-800 px-3 py-1" data-testid="right-bar">
        {bar.count > 1 ? (
          <button
            className="flex flex-1 items-center justify-center gap-1 rounded bg-neutral-800 text-xs hover:bg-neutral-700 disabled:opacity-40"
            disabled={syncing}
            onClick={(e) => bar.onSync(e.altKey)}
            title={syncing ? BUSY_WHY.paste_sync : `Synchronize settings with the ${bar.count - 1} other selected photos (Alt: no dialog, Cmd+Alt+S)${hint("sync")}`}
            data-testid="sync-settings"
          >
            <RefreshCw className="size-3.5" /> Sync…
          </button>
        ) : (
          <button
            className="flex flex-1 items-center justify-center gap-1 rounded bg-neutral-800 text-xs hover:bg-neutral-700 disabled:opacity-40"
            disabled={!bar.hasPrevious}
            onClick={bar.onPrevious}
            title={bar.hasPrevious ? `Paste the previous photo's settings (not crop or masks)${hint("pastePrev")}` : "No previous photo yet"}
            data-testid="previous-settings"
          >
            <History className="size-3.5" /> Previous
          </button>
        )}
        <button className="flex flex-1 items-center justify-center gap-1 rounded bg-neutral-800 text-xs hover:bg-neutral-700" onClick={bar.onReset} title={`Reset all adjustments${hint("reset")}`} data-testid="reset-all">
          <RotateCcw className="size-3.5" /> {bar.count > 1 ? `Reset (${bar.count})` : "Reset"}
        </button>
      </div>
    </div>
  );
}
