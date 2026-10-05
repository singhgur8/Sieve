// Develop left panel: Navigator, Presets, Snapshots, History and the sticky Copy… / Paste bar (docs/ux-spec-8b.md 5.3).
import { useRef, useState, type ReactNode } from "react";
import { ChevronDown, ChevronRight, ClipboardCopy, ClipboardPaste, Folder, Loader2, Plus, Redo2, Trash2, Undo2, User } from "lucide-react";
import { hint } from "../../lib/keymap";
import { BUSY_WHY, useActivityRunning } from "../../lib/activity";
import type { AdjustmentHistory, NormRect, StyleGroup, StylePreset } from "../../ipc";
import type { Copied } from "../../lib/clipboard";
import { useSnapshots } from "../../hooks/useDevelopV14";
import { Menu, menuItem } from "../Menu";
import type { Zoom } from "./Viewer";
import { ZOOM_PRESETS, type ZoomPreset } from "../../lib/zoom";

const GROUPS_KEY = "sieve.presetGroups.v1";

function loadGroups(): Record<string, boolean> {
  try {
    const v = JSON.parse(localStorage.getItem(GROUPS_KEY) ?? "{}");
    return v && typeof v === "object" ? v : {};
  } catch {
    return {};
  }
}

interface Props {
  /** Style library groups (User Presets first, then one per imported folder). */
  groups: StyleGroup[];
  importing: boolean;
  onImport: () => void;
  onRemoveGroup: (g: StyleGroup) => void;
  /** Hover on a preset row (null = left); the parent renders the preview after a 150 ms dwell. */
  onHoverPreset: (p: StylePreset | null) => void;
  /** Hover preview shown in the Navigator instead of the photo (label `Preview: <name>`). */
  navPreview: { url: string; label: string } | null;
  history: AdjustmentHistory | null;
  imageId: number | null;
  onApplyPreset: (p: StylePreset) => void;
  onSavePreset: () => void;
  onDeletePreset: (p: StylePreset) => void;
  onUndo: () => void;
  onRedo: () => void;
  onGoto: (entryId: number) => void;
  /** Photos a preset / reset / paste would hit (> 1 shows a "-> n" hint). */
  targetCount?: number;
  /** Navigator: the photo as currently rendered, the zoom state and the region visible at 100%. */
  navUrl: string | null;
  zoom: Zoom;
  region: NormRect | null;
  onZoom: (z: Zoom) => void;
  /** Navigator zoom presets. */
  onPreset: (p: ZoomPreset) => void;
  activePreset: ZoomPreset | null;
  /** Copy… (Alt held: copy with the remembered fields, no dialog). */
  onCopy: (alt: boolean) => void;
  onPaste: () => void;
  copied: Copied | null;
}

function Section({ id, title, defaultOpen = true, action, children }: { id: string; title: string; defaultOpen?: boolean; action?: ReactNode; children: ReactNode }) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <section className="border-b border-neutral-800 px-3 py-2" data-testid={`section-${id}`} data-open={open}>
      <div className="flex items-center justify-between">
        <button className="flex items-center gap-1 font-semibold uppercase tracking-wide text-neutral-300" aria-expanded={open} onClick={() => setOpen(!open)} data-testid={`section-toggle-${id}`}>
          {open ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
          {title}
        </button>
        {action}
      </div>
      {open && <div className="mt-1">{children}</div>}
    </section>
  );
}

export function LeftPanel({ groups, importing, onImport, onRemoveGroup, onHoverPreset, navPreview, history, imageId, onApplyPreset, onSavePreset, onDeletePreset, onUndo, onRedo, onGoto, targetCount = 1, navUrl, zoom, region, onZoom, onPreset, activePreset, onCopy, onPaste, copied }: Props) {
  const [confirming, setConfirming] = useState<number | null>(null);
  const [groupOpen, setGroupOpen] = useState<Record<string, boolean>>(loadGroups);
  const [removingGroup, setRemovingGroup] = useState<number | null>(null);
  const shown = groups.filter((g) => g.presets.length > 0);
  const total = groups.reduce((n, g) => n + g.presets.length, 0);
  const snaps = useSnapshots(imageId);
  const entries = history ? [...history.entries].reverse() : [];
  const toggleGroup = (id: number, now: boolean) => {
    const next = { ...groupOpen, [id]: !now };
    setGroupOpen(next);
    try {
      localStorage.setItem(GROUPS_KEY, JSON.stringify(next));
    } catch {
      /* not remembered */
    }
  };
  const pasting = useActivityRunning("paste_sync");
  const pasteTitle = pasting ? BUSY_WHY.paste_sync : copied
    ? `Paste ${copied.fields.length} settings${copied.fromName ? ` from ${copied.fromName.replace(/\.[^.]+$/, "")}` : ""} to ${targetCount} photo${targetCount === 1 ? "" : "s"}${hint("paste")}`
    : `Copy settings first${hint("copy")}`;

  return (
    <div className="flex h-full flex-col text-xs" data-testid="left-panel">
      <div className="min-h-0 flex-1 overflow-y-auto">
        <Section
          id="navigator"
          title="Navigator"
          action={
            <span className="flex gap-1.5 text-[11px]" role="group" aria-label="Zoom presets">
              {ZOOM_PRESETS.map((z) => (
                <button
                  key={String(z.id)}
                  className={activePreset === z.id ? "text-neutral-100" : "text-neutral-400 hover:text-neutral-300"}
                  onMouseDown={(e) => e.preventDefault()}
                  tabIndex={-1}
                  onClick={() => onPreset(z.id)}
                  title={z.title}
                  data-testid={z.id === "fit" ? "nav-fit" : `nav-${z.id}`}
                  aria-pressed={activePreset === z.id}
                >
                  {z.id === "fit" ? "FIT" : z.label}
                </button>
              ))}
            </span>
          }
        >
          <Navigator url={navPreview?.url ?? navUrl} label={navPreview?.label ?? null} zoom={zoom} region={region} onZoom={onZoom} />
        </Section>

        <Section
          id="presets"
          title="Presets"
          action={
            <span className="flex items-center gap-1.5">
              {importing && (
                <span className="flex items-center gap-1 text-[11px] text-neutral-400" data-testid="preset-importing">
                  <Loader2 className="size-3 animate-spin" /> Importing…
                </span>
              )}
              <Menu trigger={<Plus className="size-3.5" />} triggerClass="text-neutral-400 hover:text-white" triggerTestId="preset-add" title="Add presets" align="right">
                {(close) => (
                  <>
                    <button
                      className={menuItem}
                      role="menuitem"
                      data-testid="preset-save"
                      onClick={() => {
                        close();
                        onSavePreset();
                      }}
                    >
                      Create Preset… <span className="ml-auto text-neutral-400">{(hint("savePreset").match(/\((.*)\)/) ?? [])[1]}</span>
                    </button>
                    <button
                      className={menuItem}
                      role="menuitem"
                      data-testid="preset-import"
                      disabled={importing}
                      onClick={() => {
                        close();
                        onImport();
                      }}
                    >
                      Import Presets &amp; Profiles…
                    </button>
                  </>
                )}
              </Menu>
            </span>
          }
        >
          {total === 0 && (
            <div className="text-neutral-400" data-testid="preset-empty">
              <p>No presets yet. Save one with + or import a Lightroom presets folder.</p>
              <button className="mt-1 text-sky-400 hover:text-sky-300" onClick={onImport} data-testid="preset-import-link">
                Import presets &amp; profiles…
              </button>
            </div>
          )}
          <div data-testid="preset-list">
            {shown.map((g) => {
              const open = groupOpen[g.id] ?? g.kind === "user";
              return (
                <div key={g.id} data-testid={`preset-group-${g.id}`} data-open={open} data-kind={g.kind}>
                  <div className="group/g flex h-6 items-center">
                    <button className="flex h-6 min-w-0 flex-1 items-center gap-1 text-left text-neutral-300 hover:text-white" aria-expanded={open} onClick={() => toggleGroup(g.id, open)} data-testid={`preset-group-toggle-${g.id}`} title={g.sourcePath ?? g.name}>
                      {open ? <ChevronDown className="size-3 shrink-0" /> : <ChevronRight className="size-3 shrink-0" />}
                      {g.kind === "user" ? <User className="size-3 shrink-0 text-neutral-400" /> : <Folder className="size-3 shrink-0 text-neutral-400" />}
                      <span className="min-w-0 flex-1 truncate">{g.name}</span>
                      <span className="text-neutral-400">{g.presets.length}</span>
                    </button>
                    {g.kind === "imported" && removingGroup !== g.id && (
                      <button className="invisible ml-1 text-neutral-400 hover:text-red-400 group-hover/g:visible" onClick={() => setRemovingGroup(g.id)} title="Remove group from Sieve" data-testid={`preset-group-remove-${g.id}`}>
                        <Trash2 className="size-3.5" />
                      </button>
                    )}
                  </div>
                  {removingGroup === g.id && (
                    <div className="mb-1 rounded bg-neutral-900 p-2 text-[11px] text-neutral-300" data-testid={`preset-group-confirm-${g.id}`}>
                      Presets are removed from Sieve only; the files are not touched.
                      <div className="mt-1 flex gap-2">
                        <button
                          className="rounded bg-red-900 px-1.5 py-0.5 text-red-100 hover:bg-red-800"
                          data-testid={`preset-group-remove-confirm-${g.id}`}
                          onClick={() => {
                            setRemovingGroup(null);
                            onRemoveGroup(g);
                          }}
                        >
                          Remove group
                        </button>
                        <button className="text-neutral-400 hover:text-white" onClick={() => setRemovingGroup(null)}>
                          Keep
                        </button>
                      </div>
                    </div>
                  )}
                  {open && (
                    <ul>
                      {g.presets.map((p) => (
                        <li key={p.id} className="group flex h-6 items-center justify-between rounded pl-5 pr-1 hover:bg-neutral-800" onMouseEnter={() => onHoverPreset(p)} onMouseLeave={() => onHoverPreset(null)}>
                          <button className="min-w-0 flex-1 truncate text-left" onClick={() => onApplyPreset(p)} data-testid={`preset-${p.id}`} title={`Apply ${p.name}`}>
                            {p.name}
                          </button>
                          {p.warnings.length > 0 && (
                            <span className="mr-1 shrink-0 rounded bg-neutral-800 px-1 text-[10px] text-neutral-400" title={p.warnings.join("\n")} data-testid={`preset-partial-${p.id}`}>
                              partial
                            </span>
                          )}
                          {targetCount > 1 && (
                            <span className="invisible mr-1 shrink-0 text-sky-300 group-hover:visible" data-testid={`preset-hint-${p.id}`}>
                              → {targetCount}
                            </span>
                          )}
                          {g.kind !== "user" ? null : confirming === p.id ? (
                            <span className="flex shrink-0 items-center gap-1">
                              <button
                                className="rounded bg-red-900 px-1.5 py-0.5 text-red-100 hover:bg-red-800"
                                data-testid={`preset-delete-confirm-${p.id}`}
                                onClick={() => {
                                  setConfirming(null);
                                  onDeletePreset(p);
                                }}
                              >
                                Delete
                              </button>
                              <button className="text-neutral-400 hover:text-white" data-testid={`preset-delete-cancel-${p.id}`} onClick={() => setConfirming(null)}>
                                Keep
                              </button>
                            </span>
                          ) : (
                            <button className="invisible text-neutral-400 hover:text-red-400 group-hover:visible" onClick={() => setConfirming(p.id)} data-testid={`preset-delete-${p.id}`} title="Delete preset">
                              <Trash2 className="size-3.5" />
                            </button>
                          )}
                        </li>
                      ))}
                    </ul>
                  )}
                </div>
              );
            })}
          </div>
        </Section>

        {/* Hidden until list/create/update/delete_snapshot exist (docs/ux-review-8b.md P2-2). */}
        {snaps.supported && (
          <Section
            id="snapshots"
            title="Snapshots"
            defaultOpen={false}
            action={
              <button className="text-neutral-400 hover:text-white disabled:opacity-40" disabled={!snaps.supported} title={snaps.supported ? "New snapshot (Cmd+N)" : "Snapshots need backend support (not wired yet)"} data-testid="snapshot-add">
                <Plus className="size-3.5" />
              </button>
            }
          >
            {snaps.items.length === 0 && <p className="text-neutral-400">No snapshots. Save the current look with + (Cmd+N).</p>}
            <ul data-testid="snapshot-list">
              {snaps.items.map((s) => (
                <li key={s.id} className="h-6 truncate px-1 leading-6">
                  {s.name}
                </li>
              ))}
            </ul>
          </Section>
        )}

        <Section
          id="history"
          title="History"
          action={
            <div className="flex gap-2">
              <button disabled={!history?.canUndo} onClick={onUndo} title={`Undo${hint("undoAdj")}`} data-testid="undo" className="text-neutral-400 hover:text-white disabled:opacity-30">
                <Undo2 className="size-4" />
              </button>
              <button disabled={!history?.canRedo} onClick={onRedo} title={`Redo${hint("redoAdj")}`} data-testid="redo" className="text-neutral-400 hover:text-white disabled:opacity-30">
                <Redo2 className="size-4" />
              </button>
            </div>
          }
        >
          {entries.length === 0 && <p className="text-neutral-400">No edits yet. Each change you make is listed here.</p>}
          <ul data-testid="history-list">
            {entries.map((e) => (
              <li key={e.id}>
                <button
                  className={`w-full truncate rounded px-1.5 py-1 text-left ${history?.currentEntryId === e.id ? "bg-sky-900/60 text-sky-100" : "text-neutral-400 hover:bg-neutral-800"}`}
                  data-testid={`history-${e.id}`}
                  data-current={history?.currentEntryId === e.id}
                  onClick={() => onGoto(e.id)}
                >
                  {e.label}
                </button>
              </li>
            ))}
          </ul>
        </Section>
      </div>

      <div className="flex h-9 shrink-0 gap-2 border-t border-neutral-800 px-3 py-1" data-testid="left-bar">
        <button
          className="flex flex-1 items-center justify-center gap-1 rounded bg-neutral-800 text-xs hover:bg-neutral-700"
          onClick={(e) => onCopy(e.altKey)}
          title={`Copy settings… (Alt: copy with the remembered fields, no dialog)${hint("copy")}`}
          data-testid="copy-settings"
        >
          <ClipboardCopy className="size-3.5" /> Copy…
        </button>
        <button className="flex flex-1 items-center justify-center gap-1 rounded bg-neutral-800 text-xs hover:bg-neutral-700 disabled:opacity-40" disabled={!copied || pasting} onClick={onPaste} title={pasteTitle} data-testid="paste-settings">
          <ClipboardPaste className="size-3.5" /> Paste
        </button>
      </div>
    </div>
  );
}

/** Photo preview fitted in width x 2/3; at 100% a rectangle marks the visible region and click / drag pans. */
function Navigator({ url, label, zoom, region, onZoom }: { url: string | null; label: string | null; zoom: Zoom; region: NormRect | null; onZoom: (z: Zoom) => void }) {
  const box = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);
  const [aspect, setAspect] = useState(1.5);
  const pan = (e: React.PointerEvent) => {
    const r = box.current?.getBoundingClientRect();
    if (!r || !zoom.on) return;
    onZoom({ ...zoom, on: true, cx: Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)), cy: Math.min(1, Math.max(0, (e.clientY - r.top) / r.height)) });
  };
  // The frame (3:2) is filled by the image at its own aspect: percentages keep the region overlay exact.
  const wPct = aspect >= 1.5 ? 100 : (aspect / 1.5) * 100;
  const hPct = aspect >= 1.5 ? (1.5 / aspect) * 100 : 100;
  return (
    <div className="relative aspect-[3/2] w-full overflow-hidden bg-neutral-950" data-testid="navigator">
      {url ? (
        <div
          ref={box}
          className={`absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 ${zoom.on ? "cursor-crosshair" : ""}`}
          style={{ width: `${wPct}%`, height: `${hPct}%` }}
          onPointerDown={(e) => {
            dragging.current = true;
            e.currentTarget.setPointerCapture(e.pointerId);
            pan(e);
          }}
          onPointerMove={(e) => dragging.current && pan(e)}
          onPointerUp={() => (dragging.current = false)}
        >
          <img
            src={url}
            alt=""
            draggable={false}
            className="block size-full select-none"
            data-testid="navigator-img"
            onLoad={(e) => e.currentTarget.naturalHeight > 0 && setAspect(e.currentTarget.naturalWidth / e.currentTarget.naturalHeight)}
          />
          {label && (
            <span className="pointer-events-none absolute bottom-0.5 left-0.5 max-w-full truncate rounded bg-black/70 px-1 text-[10px] text-neutral-100" data-testid="navigator-preview-label">
              Preview: {label}
            </span>
          )}
          {zoom.on && region && (
            <div
              className="pointer-events-none absolute border border-white"
              style={{ left: `${region.x * 100}%`, top: `${region.y * 100}%`, width: `${region.width * 100}%`, height: `${region.height * 100}%`, boxShadow: "0 0 0 999px rgb(0 0 0 / 0.4)" }}
              data-testid="navigator-region"
            />
          )}
        </div>
      ) : (
        <span className="absolute inset-0 flex items-center justify-center text-neutral-400">No preview</span>
      )}
    </div>
  );
}
