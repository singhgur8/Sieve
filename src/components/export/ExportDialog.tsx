// Export dialog: preset list on the left, full ExportSettings editor on the right.
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { X } from "lucide-react";
import {
  commands,
  EXPORT_FILENAME_TOKENS,
  unwrap,
  type ExportCapabilities,
  type ExportFormatKind,
  type ExportJob,
  type ExportPlan,
  type ExportPreset,
  type ExportSettings,
  type RawImageEntry,
  type ResizeMode,
} from "../../ipc";
import { formatError } from "../../lib/format";
import { defaultFormat, defaultResize, DEFAULT_SETTINGS, normalizeSettings, previewTemplate, validateSubfolder } from "../../lib/exportSettings";

interface Props {
  /** Selected photos (or the active one). */
  selectionIds: number[];
  /** Everything in the current filter. */
  filteredIds: number[];
  sampleEntry: (id: number) => RawImageEntry | undefined;
  onClose: () => void;
  onStarted: (job: ExportJob) => void;
}

const FORMAT_LABEL: Record<ExportFormatKind, string> = { jpeg: "JPEG", tiff: "TIFF", png: "PNG", webp: "WebP", heic: "HEIC" };
const RESIZE_LABEL: Record<ResizeMode["kind"], string> = {
  none: "Full resolution",
  long_edge: "Long edge",
  short_edge: "Short edge",
  megapixels: "Megapixels",
  width_height: "Width x Height",
};
const field = "rounded bg-neutral-800 px-2 py-1 text-sm text-neutral-100 outline-none focus:ring-1 focus:ring-amber-400";
const btn = "rounded-md bg-neutral-800 px-2.5 py-1 text-sm hover:bg-neutral-700 disabled:opacity-40";

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="border-b border-neutral-800 py-3">
      <h3 className="mb-2 text-xs font-semibold uppercase tracking-wide text-neutral-500">{title}</h3>
      <div className="space-y-2">{children}</div>
    </section>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="flex items-center gap-3 text-sm">
      <span className="w-28 shrink-0 text-neutral-400">{label}</span>
      <span className="flex flex-wrap items-center gap-2">{children}</span>
    </label>
  );
}

function NumInput({ id, value, min, max, step, onChange, width = "w-24" }: { id: string; value: number; min?: number; max?: number; step?: number; onChange: (n: number) => void; width?: string }) {
  return (
    <input
      type="number"
      data-testid={id}
      value={Number.isFinite(value) ? value : ""}
      min={min}
      max={max}
      step={step}
      onChange={(e) => onChange(e.target.value === "" ? NaN : Number(e.target.value))}
      className={`${field} ${width}`}
    />
  );
}

export function ExportDialog({ selectionIds, filteredIds, sampleEntry, onClose, onStarted }: Props) {
  const [presets, setPresets] = useState<ExportPreset[]>([]);
  const [caps, setCaps] = useState<ExportCapabilities | null>(null);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [draft, setDraft] = useState<ExportSettings>(DEFAULT_SETTINGS);
  const [scope, setScope] = useState<"selection" | "filtered">("selection");
  const [saveName, setSaveName] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [plan, setPlan] = useState<ExportPlan | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const templateRef = useRef<HTMLInputElement>(null);

  const ids = scope === "selection" ? selectionIds : filteredIds;
  const preset = presets.find((p) => p.id === selectedId) ?? null;
  const fail = useCallback((e: unknown) => setError(formatError(e)), []);

  // Load presets + capabilities once.
  useEffect(() => {
    void (async () => {
      try {
        const [list, c] = await Promise.all([unwrap(commands.listExportPresets()), unwrap(commands.getExportCapabilities())]);
        setCaps(c);
        setPresets(list);
        if (list[0]) {
          setSelectedId(list[0].id);
          setDraft(structuredClone(list[0].settings));
        }
      } catch (e) {
        fail(e);
      }
    })();
  }, [fail]);

  const selectPreset = (p: ExportPreset) => {
    setSelectedId(p.id);
    setConfirmDelete(false);
    setError(null);
    setDraft((cur) => {
      const s = structuredClone(p.settings);
      // Keep an already picked folder when the preset asks to choose at export time.
      if (s.destination.kind === "choose" && cur.destination.kind === "folder") s.destination = cur.destination;
      return s;
    });
  };

  const patch = (f: Partial<ExportSettings>) => setDraft((d) => ({ ...d, ...f }));

  const dirty = useMemo(() => {
    if (!preset) return true;
    const a = normalizeSettings(draft);
    const b = normalizeSettings(preset.settings);
    if (b.destination.kind === "choose") a.destination = b.destination;
    return JSON.stringify(a) !== JSON.stringify(b);
  }, [draft, preset]);

  const info = (k: ExportFormatKind) => caps?.formats.find((f) => f.kind === k);
  const fmt = draft.format;

  const tpl = useMemo(
    () => previewTemplate(draft.naming.template, draft.naming.startNumber, fmt.kind, ids[0] != null ? sampleEntry(ids[0]) ?? null : null),
    [draft.naming.template, draft.naming.startNumber, fmt.kind, ids, sampleEntry],
  );
  const subErr = validateSubfolder(draft.subfolder);
  const destOk = draft.destination.kind !== "choose" && !(draft.destination.kind === "folder" && draft.destination.path === "");
  const numsOk = (() => {
    const r = draft.resize;
    const m = r.mode;
    const okRes =
      m.kind === "none" ||
      (m.kind === "long_edge" || m.kind === "short_edge" ? m.px >= 1 : m.kind === "megapixels" ? m.mp >= 0.1 && m.mp <= 200 : m.width >= 1 && m.height >= 1);
    return okRes && r.resolutionPpi >= 1 && r.resolutionPpi <= 4800 && draft.naming.startNumber >= 0 && draft.naming.startNumber <= 999999999;
  })();
  const canExport = ids.length > 0 && destOk && !tpl.error && !subErr && numsOk && !busy;

  // Debounced dry run: warn about existing files.
  const planKey = JSON.stringify([ids, normalizeSettings(draft)]);
  useEffect(() => {
    setPlan(null);
    if (!canExport && !(ids.length > 0 && destOk && !tpl.error && !subErr && numsOk)) return;
    const t = setTimeout(() => {
      unwrap(commands.planExport(ids, normalizeSettings(draft)))
        .then(setPlan)
        .catch(() => setPlan(null));
    }, 300);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [planKey]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const chooseFolder = async () => {
    try {
      const p = await open({ directory: true, title: "Export destination" });
      if (typeof p === "string") patch({ destination: { kind: "folder", path: p } });
    } catch (e) {
      fail(e);
    }
  };

  const saveAs = async () => {
    try {
      setError(null);
      const p = await unwrap(commands.saveExportPreset(null, saveName.trim(), normalizeSettings(draft)));
      setPresets(await unwrap(commands.listExportPresets()));
      setSelectedId(p.id);
      setSaveName("");
    } catch (e) {
      fail(e);
    }
  };

  const update = async () => {
    if (!preset || preset.builtIn) return;
    try {
      setError(null);
      await unwrap(commands.saveExportPreset(preset.id, preset.name, normalizeSettings(draft)));
      setPresets(await unwrap(commands.listExportPresets()));
    } catch (e) {
      fail(e);
    }
  };

  const remove = async () => {
    if (!preset || preset.builtIn) return;
    if (!confirmDelete) {
      setConfirmDelete(true);
      return;
    }
    try {
      await unwrap(commands.deleteExportPreset(preset.id));
      const list = await unwrap(commands.listExportPresets());
      setPresets(list);
      setConfirmDelete(false);
      if (list[0]) {
        setSelectedId(list[0].id);
        setDraft((cur) => {
          const s = structuredClone(list[0].settings);
          if (s.destination.kind === "choose" && cur.destination.kind === "folder") s.destination = cur.destination;
          return s;
        });
      }
    } catch (e) {
      fail(e);
    }
  };

  const doExport = async () => {
    setBusy(true);
    setError(null);
    try {
      const job = await unwrap(commands.exportImages(ids, normalizeSettings(draft), preset?.name ?? null));
      onStarted(job);
      onClose();
    } catch (e) {
      fail(e);
      setBusy(false);
    }
  };

  const insertToken = (token: string) => {
    const el = templateRef.current;
    const t = draft.naming.template;
    const a = el?.selectionStart ?? t.length;
    const b = el?.selectionEnd ?? t.length;
    const next = t.slice(0, a) + token + t.slice(b);
    patch({ naming: { ...draft.naming, template: next } });
    requestAnimationFrame(() => {
      el?.focus();
      el?.setSelectionRange(a + token.length, a + token.length);
    });
  };

  const setKind = (k: ExportFormatKind) => patch({ format: defaultFormat(k) });
  const hasQuality = fmt.kind === "jpeg" || fmt.kind === "heic" || (fmt.kind === "webp" && !fmt.lossless);
  const sharpMedia = draft.sharpening?.media ?? "none";
  const meta = draft.metadata;
  const fmtInfo = info(fmt.kind);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-6" data-testid="export-dialog" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="flex max-h-full w-full max-w-5xl flex-col overflow-hidden rounded-xl border border-neutral-700 bg-neutral-900 shadow-2xl" role="dialog" aria-label="Export">
        <header className="flex items-center justify-between border-b border-neutral-800 px-4 py-2.5">
          <h2 className="font-semibold">Export</h2>
          <button onClick={onClose} aria-label="Close" data-testid="export-close" className="text-neutral-400 hover:text-neutral-100">
            <X className="size-4" />
          </button>
        </header>

        <div className="flex min-h-0 flex-1">
          {/* presets */}
          <aside className="flex w-64 shrink-0 flex-col border-r border-neutral-800 p-3" data-testid="export-presets">
            <h3 className="mb-2 text-xs font-semibold uppercase tracking-wide text-neutral-500">Presets</h3>
            <ul className="min-h-0 flex-1 space-y-0.5 overflow-auto">
              {presets.map((p) => (
                <li key={p.id}>
                  <button
                    onClick={() => selectPreset(p)}
                    data-testid={`export-preset-${p.id}`}
                    aria-pressed={p.id === selectedId}
                    className={`w-full truncate rounded px-2 py-1 text-left text-sm ${p.id === selectedId ? "bg-amber-400/20 text-amber-200" : "hover:bg-neutral-800"}`}
                  >
                    {p.name}
                    {p.builtIn && <span className="ml-1 text-[10px] text-neutral-500">built-in</span>}
                    {p.id === selectedId && dirty && <span className="ml-1 text-amber-400" title="Modified">*</span>}
                  </button>
                </li>
              ))}
            </ul>
            <div className="mt-3 space-y-2 border-t border-neutral-800 pt-3">
              <div className="flex gap-1.5">
                <input
                  type="text"
                  value={saveName}
                  onChange={(e) => setSaveName(e.target.value)}
                  placeholder="New preset name"
                  data-testid="export-preset-name"
                  className={`${field} min-w-0 flex-1`}
                />
                <button onClick={() => void saveAs()} disabled={saveName.trim() === ""} data-testid="export-preset-save-as" className={btn}>
                  Save as
                </button>
              </div>
              <div className="flex gap-1.5">
                <button onClick={() => void update()} disabled={!preset || preset.builtIn || !dirty} data-testid="export-preset-update" className={btn}>
                  Update
                </button>
                <button
                  onClick={() => void remove()}
                  disabled={!preset || preset.builtIn}
                  data-testid="export-preset-delete"
                  className={`${btn} ${confirmDelete ? "!bg-red-900 hover:!bg-red-800" : ""}`}
                >
                  {confirmDelete ? "Confirm delete" : "Delete"}
                </button>
              </div>
            </div>
          </aside>

          {/* editor */}
          <div className="min-w-0 flex-1 overflow-auto px-4 pb-3" data-testid="export-editor">
            <Section title="File format">
              <Row label="Format">
                <select data-testid="export-format" value={fmt.kind} onChange={(e) => setKind(e.target.value as ExportFormatKind)} className={field}>
                  {(["jpeg", "tiff", "png", "webp", "heic"] as ExportFormatKind[]).map((k) => {
                    const i = info(k);
                    const off = !!caps && i != null && !i.available;
                    return (
                      <option key={k} value={k} disabled={off && k !== fmt.kind}>
                        {FORMAT_LABEL[k]}
                        {off ? " (unavailable)" : ""}
                      </option>
                    );
                  })}
                </select>
                {fmtInfo && !fmtInfo.available && <span className="text-xs text-red-400">{fmtInfo.reason ?? "Encoder not available"}</span>}
                {fmtInfo && !fmtInfo.supportsMetadata && <span className="text-xs text-neutral-500">No metadata embedding</span>}
              </Row>
              {hasQuality && "quality" in fmt && (
                <Row label="Quality">
                  <input
                    type="range"
                    min={0}
                    max={100}
                    value={fmt.quality}
                    data-testid="export-quality"
                    onChange={(e) => patch({ format: { ...fmt, quality: Number(e.target.value) } })}
                    className="w-56 accent-amber-400"
                  />
                  <span className="w-8 text-right tabular-nums" data-testid="export-quality-value">
                    {fmt.quality}
                  </span>
                </Row>
              )}
              {fmt.kind === "jpeg" && (
                <Row label="Chroma">
                  <select data-testid="export-chroma" value={fmt.chromaSubsampling} onChange={(e) => patch({ format: { ...fmt, chromaSubsampling: e.target.value as typeof fmt.chromaSubsampling } })} className={field}>
                    <option value="444">4:4:4 (best)</option>
                    <option value="422">4:2:2</option>
                    <option value="420">4:2:0 (smallest)</option>
                  </select>
                </Row>
              )}
              {fmt.kind === "webp" && (
                <Row label="Lossless">
                  <input type="checkbox" data-testid="export-webp-lossless" checked={fmt.lossless} onChange={(e) => patch({ format: { ...fmt, lossless: e.target.checked } })} />
                </Row>
              )}
              {(fmt.kind === "tiff" || fmt.kind === "png") && (
                <Row label="Bit depth">
                  <select data-testid="export-bitdepth" value={fmt.bitDepth} onChange={(e) => patch({ format: { ...fmt, bitDepth: e.target.value as "8" | "16" } })} className={field}>
                    {(fmtInfo?.bitDepths ?? ["8", "16"]).map((d) => (
                      <option key={d} value={d}>
                        {d}-bit
                      </option>
                    ))}
                  </select>
                </Row>
              )}
              {fmt.kind === "tiff" && (
                <Row label="Compression">
                  <select data-testid="export-tiff-compression" value={fmt.compression} onChange={(e) => patch({ format: { ...fmt, compression: e.target.value as typeof fmt.compression } })} className={field}>
                    <option value="none">None</option>
                    <option value="lzw">LZW</option>
                    <option value="zip">ZIP</option>
                  </select>
                </Row>
              )}
              <Row label="Color space">
                <select data-testid="export-colorspace" value={draft.colorSpace} onChange={(e) => patch({ colorSpace: e.target.value as ExportSettings["colorSpace"] })} className={field}>
                  <option value="srgb">sRGB</option>
                  <option value="display_p3">Display P3</option>
                  <option value="adobe_rgb">Adobe RGB (1998)</option>
                </select>
                <span className="text-xs text-neutral-500">ICC profile embedded</span>
              </Row>
            </Section>

            <Section title="Image sizing">
              <Row label="Resize to">
                <select
                  data-testid="export-resize-mode"
                  value={draft.resize.mode.kind}
                  onChange={(e) => patch({ resize: { ...draft.resize, mode: defaultResize(e.target.value as ResizeMode["kind"]) } })}
                  className={field}
                >
                  {(Object.keys(RESIZE_LABEL) as ResizeMode["kind"][]).map((k) => (
                    <option key={k} value={k}>
                      {RESIZE_LABEL[k]}
                    </option>
                  ))}
                </select>
                {(() => {
                  const m = draft.resize.mode;
                  const set = (mode: ResizeMode) => patch({ resize: { ...draft.resize, mode } });
                  if (m.kind === "long_edge" || m.kind === "short_edge")
                    return (
                      <>
                        <NumInput id="export-resize-px" value={m.px} min={1} onChange={(px) => set({ ...m, px })} />
                        <span className="text-xs text-neutral-500">px</span>
                      </>
                    );
                  if (m.kind === "megapixels")
                    return (
                      <>
                        <NumInput id="export-resize-mp" value={m.mp} min={0.1} max={200} step={0.1} onChange={(mp) => set({ ...m, mp })} />
                        <span className="text-xs text-neutral-500">MP</span>
                      </>
                    );
                  if (m.kind === "width_height")
                    return (
                      <>
                        <NumInput id="export-resize-width" value={m.width} min={1} onChange={(width) => set({ ...m, width })} />
                        <span className="text-xs text-neutral-500">x</span>
                        <NumInput id="export-resize-height" value={m.height} min={1} onChange={(height) => set({ ...m, height })} />
                        <span className="text-xs text-neutral-500">px</span>
                      </>
                    );
                  return null;
                })()}
              </Row>
              <Row label="Don't enlarge">
                <input type="checkbox" data-testid="export-dont-enlarge" checked={draft.resize.dontEnlarge} onChange={(e) => patch({ resize: { ...draft.resize, dontEnlarge: e.target.checked } })} />
              </Row>
              <Row label="Resolution">
                <NumInput id="export-ppi" value={draft.resize.resolutionPpi} min={1} max={4800} onChange={(resolutionPpi) => patch({ resize: { ...draft.resize, resolutionPpi } })} />
                <span className="text-xs text-neutral-500">pixels per inch</span>
              </Row>
            </Section>

            <Section title="Output sharpening">
              <Row label="Sharpen for">
                <select
                  data-testid="export-sharpen-media"
                  value={sharpMedia}
                  onChange={(e) => {
                    const v = e.target.value;
                    patch({ sharpening: v === "none" ? null : { media: v as "screen", amount: draft.sharpening?.amount ?? "standard" } });
                  }}
                  className={field}
                >
                  <option value="none">None</option>
                  <option value="screen">Screen</option>
                  <option value="matte">Matte paper</option>
                  <option value="glossy">Glossy paper</option>
                </select>
                {draft.sharpening && (
                  <select
                    data-testid="export-sharpen-amount"
                    value={draft.sharpening.amount}
                    onChange={(e) => patch({ sharpening: { ...draft.sharpening!, amount: e.target.value as "low" } })}
                    className={field}
                  >
                    <option value="low">Low</option>
                    <option value="standard">Standard</option>
                    <option value="high">High</option>
                  </select>
                )}
              </Row>
            </Section>

            <Section title="File naming">
              <Row label="Template">
                <input
                  ref={templateRef}
                  type="text"
                  data-testid="export-template"
                  value={draft.naming.template}
                  onChange={(e) => patch({ naming: { ...draft.naming, template: e.target.value } })}
                  className={`${field} w-72 font-mono`}
                />
              </Row>
              <div className="ml-[7.75rem] flex flex-wrap gap-1" data-testid="export-token-chips">
                {EXPORT_FILENAME_TOKENS.map((t) => (
                  <button
                    key={t.token}
                    type="button"
                    title={t.description}
                    data-testid={`export-token-${t.token.slice(1, -1)}`}
                    onClick={() => insertToken(t.token)}
                    className="rounded-full bg-neutral-800 px-2 py-0.5 font-mono text-xs text-neutral-300 hover:bg-neutral-700"
                  >
                    {t.token}
                  </button>
                ))}
              </div>
              <p className="ml-[7.75rem] text-xs" data-testid="export-template-example">
                {tpl.error ? <span className="text-red-400">{tpl.error}</span> : <span className="text-neutral-400">Example: <span className="font-mono text-neutral-200">{tpl.example}</span></span>}
              </p>
              <Row label="Start number">
                <NumInput id="export-start-number" value={draft.naming.startNumber} min={0} max={999999999} onChange={(startNumber) => patch({ naming: { ...draft.naming, startNumber } })} />
              </Row>
              <Row label="If file exists">
                <select data-testid="export-collision" value={draft.naming.collision} onChange={(e) => patch({ naming: { ...draft.naming, collision: e.target.value as typeof draft.naming.collision } })} className={field}>
                  <option value="unique_suffix">Add suffix (-2, -3, ...)</option>
                  <option value="overwrite">Overwrite</option>
                  <option value="skip">Skip</option>
                </select>
              </Row>
            </Section>

            <Section title="Destination">
              <Row label="Export to">
                <select
                  data-testid="export-dest-kind"
                  value={draft.destination.kind === "source_folder" ? "source_folder" : "folder"}
                  onChange={(e) => patch({ destination: e.target.value === "source_folder" ? { kind: "source_folder" } : draft.destination.kind === "folder" ? draft.destination : { kind: "choose" } })}
                  className={field}
                >
                  <option value="folder">Specific folder</option>
                  <option value="source_folder">Same folder as original</option>
                </select>
              </Row>
              {draft.destination.kind !== "source_folder" && (
                <Row label="Folder">
                  <button type="button" onClick={() => void chooseFolder()} data-testid="export-choose-folder" className={btn}>
                    Choose…
                  </button>
                  <span className="max-w-md break-all font-mono text-xs text-neutral-300" data-testid="export-dest-path">
                    {draft.destination.kind === "folder" ? draft.destination.path : <span className="text-amber-400">No folder chosen</span>}
                  </span>
                </Row>
              )}
              <Row label="Subfolder">
                <input
                  type="text"
                  data-testid="export-subfolder"
                  value={draft.subfolder ?? ""}
                  onChange={(e) => patch({ subfolder: e.target.value })}
                  placeholder="optional, e.g. Smith Wedding/Web"
                  className={`${field} w-72`}
                />
                {subErr && <span className="text-xs text-red-400">{subErr}</span>}
              </Row>
            </Section>

            <Section title="Metadata">
              <Row label="Include">
                <select data-testid="export-metadata" value={meta.include} onChange={(e) => patch({ metadata: { ...meta, include: e.target.value as typeof meta.include } })} className={field}>
                  <option value="all">All metadata</option>
                  <option value="copyright_and_contact">Copyright and contact</option>
                  <option value="copyright_only">Copyright only</option>
                  <option value="none">None</option>
                </select>
              </Row>
              {meta.include === "all" && (
                <>
                  <Row label="Remove location">
                    <input type="checkbox" data-testid="export-remove-location" checked={meta.removeLocation} onChange={(e) => patch({ metadata: { ...meta, removeLocation: e.target.checked } })} />
                    <span className="text-xs text-neutral-500">Strip GPS and location fields</span>
                  </Row>
                  <Row label="Keywords">
                    <input type="checkbox" data-testid="export-keywords" checked={meta.includeKeywords} onChange={(e) => patch({ metadata: { ...meta, includeKeywords: e.target.checked } })} />
                  </Row>
                </>
              )}
              {meta.include !== "none" && (
                <>
                  <Row label="Copyright">
                    <input type="text" data-testid="export-copyright" value={meta.copyright ?? ""} onChange={(e) => patch({ metadata: { ...meta, copyright: e.target.value } })} placeholder="override (optional)" className={`${field} w-72`} />
                  </Row>
                  {meta.include !== "copyright_only" && (
                    <Row label="Creator">
                      <input type="text" data-testid="export-creator" value={meta.creator ?? ""} onChange={(e) => patch({ metadata: { ...meta, creator: e.target.value } })} placeholder="override (optional)" className={`${field} w-72`} />
                    </Row>
                  )}
                </>
              )}
            </Section>
          </div>
        </div>

        <footer className="flex flex-wrap items-center gap-3 border-t border-neutral-800 px-4 py-2.5">
          <div className="flex items-center gap-3 text-sm" data-testid="export-scope">
            {selectionIds.length !== filteredIds.length && (
              <>
                <label className="flex items-center gap-1.5">
                  <input type="radio" name="export-scope" checked={scope === "selection"} onChange={() => setScope("selection")} data-testid="export-scope-selection" />
                  Selection ({selectionIds.length})
                </label>
                <label className="flex items-center gap-1.5">
                  <input type="radio" name="export-scope" checked={scope === "filtered"} onChange={() => setScope("filtered")} data-testid="export-scope-filtered" />
                  All filtered ({filteredIds.length})
                </label>
              </>
            )}
            <span className="text-neutral-400" data-testid="export-count">
              {ids.length} photo{ids.length === 1 ? "" : "s"}
            </span>
          </div>
          {plan && plan.existing > 0 && (
            <span className="text-xs text-amber-300" data-testid="export-plan-warning">
              {plan.existing} file{plan.existing === 1 ? "" : "s"} already exist
              {draft.naming.collision === "overwrite" ? " and will be overwritten" : draft.naming.collision === "skip" ? " and will be skipped" : " (a suffix will be added)"}
            </span>
          )}
          {error && (
            <span className="text-xs text-red-400" data-testid="export-error">
              {error}
            </span>
          )}
          <div className="ml-auto flex gap-2">
            <button onClick={onClose} className={btn} data-testid="export-cancel-dialog">
              Cancel
            </button>
            <button
              onClick={() => void doExport()}
              disabled={!canExport}
              data-testid="export-go"
              title={!destOk ? "Choose a destination folder" : undefined}
              className="rounded-md bg-amber-500 px-4 py-1 text-sm font-medium text-neutral-950 hover:bg-amber-400 disabled:opacity-40"
            >
              Export {ids.length}
            </button>
          </div>
        </footer>
      </div>
    </div>
  );
}
