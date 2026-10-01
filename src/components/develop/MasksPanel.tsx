// Masks panel (Shift+W): create tools, mask groups with their components, and the local adjustment sliders.
import { useEffect, useRef, useState } from "react";
import { Brush, Copy, Eye, GripVertical, EyeOff, FlipHorizontal2, Loader2, MoreHorizontal, MousePointer2, Mountain, RefreshCw, Sun, Trash2, Users, UserRound, Circle, Blend, Palette, SquareDashed } from "lucide-react";
import { IDENTITY_CURVE, defaultLocalAdjustments, type CurvePoint, type MaskBlendMode, type MaskComponent, type MaskGroup, type PointCurves } from "../../ipc";
import { PointCurveEditor, type CurveHost } from "./ToneCurvePanel";
import type { MasksApi } from "../../hooks/useMasks";
import { hint } from "../../lib/keymap";
import { CREATE_LABEL, CREATE_ORDER, CREATE_SHORT, LOCAL_GROUPS, MODE_GLYPH, OVERLAY_STYLES, componentTitle, type CreateKind } from "../../lib/masks";
import { Menu, menuItem } from "../Menu";
import { Section, seg } from "./fields";
import { ModelsCard } from "../ModelsCard";
import { Slider } from "./Slider";

const ICON: Record<CreateKind, typeof Brush> = {
  subject: UserRound,
  sky: Sun,
  background: Mountain,
  people: Users,
  object: SquareDashed,
  brush: Brush,
  linear: Blend,
  radial: Circle,
  color: Palette,
  luminance: MousePointer2,
};

const HINT: Partial<Record<CreateKind, string>> = {
  brush: hint("maskBrush"),
  linear: hint("maskLinear"),
  radial: hint("maskRadial"),
  color: hint("maskColor"),
  luminance: hint("maskLuminance"),
};

const MODES: { mode: MaskBlendMode; label: string }[] = [
  { mode: "add", label: "Add" },
  { mode: "subtract", label: "Subtract" },
  { mode: "intersect", label: "Intersect" },
];

interface Props {
  masks: MasksApi;
}

const identityCurves = (): PointCurves => {
  const id = () => IDENTITY_CURVE.map((p) => [...p] as CurvePoint);
  return { master: id(), red: id(), green: id(), blue: id() };
};

export function MasksPanel({ masks }: Props) {
  const sel = masks.groups.find((g) => g.id === masks.selGroup) ?? null;
  const selComp = sel?.components.find((c) => c.id === masks.selComp) ?? null;
  const compact = masks.groups.length > 0;
  return (
    <div className="flex h-full flex-col overflow-hidden px-2" data-testid="masks-panel" data-compact={compact}>
      <div className="shrink-0 py-2">
        <ModelsCard />
        <div className="mb-1 flex items-center justify-between">
          <h2 className="text-[11px] font-semibold uppercase tracking-wide text-neutral-300">{compact ? "Create" : "Create new mask"}</h2>
          {masks.needsUpdate > 0 && (
            <button className="flex items-center gap-1 rounded bg-amber-900/70 px-2 py-0.5 text-[11px] text-amber-100 hover:bg-amber-800" onClick={() => void masks.updateAll()} data-testid="mask-update-all" title="Compute the AI selections that are missing for this photo">
              <RefreshCw className="size-3" /> Update {masks.needsUpdate}
            </button>
          )}
        </div>
        {compact && (
          <div className="flex items-center gap-0.5" data-testid="mask-create">
            {CREATE_ORDER.map((k) => (
              <CreateButton key={k} kind={k} masks={masks} icon />
            ))}
          </div>
        )}
        {!compact && (
          <div className="grid grid-cols-2 gap-1" data-testid="mask-create">
            {CREATE_ORDER.map((k) => (
              <CreateButton key={k} kind={k} masks={masks} />
            ))}
          </div>
        )}
        {masks.busy && (
          <p className="mt-2 flex items-center gap-1.5 text-xs text-sky-300" data-testid="mask-busy" role="status">
            <Loader2 className="size-3.5 animate-spin" /> {masks.busy.label}
          </p>
        )}
      </div>

      <section className="flex max-h-[35%] min-h-[96px] shrink-0 flex-col overflow-hidden border-t border-neutral-800 py-2" data-testid="mask-list">
        <div className="mb-1 flex flex-wrap items-center justify-between gap-1">
          <h2 className="whitespace-nowrap text-xs font-semibold uppercase tracking-wide text-neutral-300">Masks ({masks.groups.length})</h2>
          <div className="flex items-center gap-2 text-[11px] text-neutral-300">
            <label className="flex items-center gap-1" title={`Keep the selected mask overlay on. Off: it appears while you paint or adjust and fades out afterwards${hint("maskOverlay")}`}>
              <input type="checkbox" checked={masks.overlayOn} onChange={masks.toggleOverlay} data-testid="mask-overlay-toggle" /> Show overlay
            </label>
            <select
              className="rounded bg-neutral-800 px-1 py-0.5 text-[11px]"
              value={masks.overlayStyle}
              onChange={(e) => masks.setOverlayStyle(Number(e.target.value))}
              aria-label="Overlay style"
              title={`Overlay color and mode${hint("maskOverlayStyle")}`}
              data-testid="mask-overlay-style"
            >
              {OVERLAY_STYLES.map((s, i) => (
                <option key={s.id} value={i}>
                  {s.label}
                </option>
              ))}
            </select>
            <label className="flex items-center gap-1" title={`Show mask pins${hint("maskPins")}`}>
              <input type="checkbox" checked={masks.pins} onChange={masks.togglePins} data-testid="mask-pins-toggle" /> Pins
            </label>
          </div>
        </div>
        {masks.groups.length === 0 && (
          <p className="py-2 text-[11px] text-neutral-400" data-testid="mask-empty">
            No masks yet. Choose a tool above, then click or drag on the photo.
          </p>
        )}
        <ul className="min-h-0 flex-1 space-y-1 overflow-y-auto" data-testid="mask-list-scroll">
          {masks.groups.map((g, i) => (
            <GroupRow key={g.id} g={g} index={i} masks={masks} />
          ))}
        </ul>
      </section>

      {(sel || masks.tool) && (
        <div className="min-h-0 flex-1 overflow-y-auto pb-6" data-testid="mask-settings" data-group={sel?.id ?? ""}>
          <ToolSettings masks={masks} group={sel} comp={selComp} />
          {sel && <LocalSliders masks={masks} g={sel} />}
        </div>
      )}
    </div>
  );
}

function CreateButton({ kind, masks, target, icon }: { kind: CreateKind; masks: MasksApi; target?: { groupId: string | null; mode: MaskBlendMode }; icon?: boolean }) {
  const Icon = ICON[kind];
  const why = masks.unavailable(kind);
  const active = masks.tool?.kind === kind && !target;
  return (
    <button
      className={`flex items-center ${icon ? "size-6 shrink-0 justify-center rounded" : "gap-1.5 rounded px-2 py-1.5 text-left text-xs"} ${active ? "bg-sky-800 text-sky-100" : "bg-neutral-800 hover:bg-neutral-700"} disabled:opacity-40 disabled:hover:bg-neutral-800`}
      disabled={!!why || !!masks.busy}
      title={why ? `${CREATE_LABEL[kind]} is not available: ${why}` : `${CREATE_LABEL[kind]}${HINT[kind] ?? ""}`}
      aria-label={CREATE_LABEL[kind]}
      data-testid={`mask-create-${kind}`}
      data-unavailable={why ? "true" : "false"}
      aria-pressed={active}
      onClick={() => masks.create(kind, target)}
    >
      <Icon className="size-3.5 shrink-0" />
      {!icon && <span className="truncate">{CREATE_SHORT[kind]}</span>}
    </button>
  );
}

function AddMenu({ g, mode, masks }: { g: MaskGroup; mode: MaskBlendMode; masks: MasksApi }) {
  const label = MODES.find((m) => m.mode === mode)!.label;
  return (
    <Menu
      trigger={
        <>
          {MODE_GLYPH[mode]} {label}
        </>
      }
      triggerClass="rounded bg-neutral-800 px-1.5 py-0.5 text-[11px] hover:bg-neutral-700"
      triggerTestId={`mask-${mode}-${g.id}`}
      title={`${label} a new component to this mask`}
    >
      {(close) => (
        <div data-testid={`mask-${mode}-menu`}>
          {CREATE_ORDER.map((k) => {
            const why = masks.unavailable(k);
            return (
              <button
                key={k}
                className={menuItem}
                disabled={!!why}
                title={why ?? undefined}
                data-testid={`mask-${mode}-item-${k}`}
                onClick={() => {
                  close();
                  masks.create(k, { groupId: g.id, mode });
                }}
              >
                {CREATE_LABEL[k]}
              </button>
            );
          })}
        </div>
      )}
    </Menu>
  );
}

function compKind(c: MaskComponent): CreateKind {
  const sh = c.shape;
  if (sh.kind === "ai") return sh.target.kind === "sky" ? "sky" : sh.target.kind === "background" ? "background" : sh.target.kind === "people" ? "people" : sh.target.kind === "object" ? "object" : "subject";
  if (sh.kind === "brush" || sh.kind === "linear" || sh.kind === "radial" || sh.kind === "color" || sh.kind === "luminance") return sh.kind;
  return "object";
}

/**
 * Drag handle that reorders its row among the siblings of the same list (`ul`). Pointer-based: while dragging, a line
 * marks the drop position; on release `onMove(finalIndex)` runs once. Alt+Up / Alt+Down move the selection (keymap).
 */
function ReorderHandle({ index, onMove, onPick, testid, label }: { index: number; onMove: (to: number) => void; onPick: () => void; testid: string; label: string }) {
  const st = useRef<{ items: HTMLElement[]; to: number } | null>(null);
  const clear = () => st.current?.items.forEach((el) => (el.style.boxShadow = ""));
  const target = (y: number, items: HTMLElement[]) => items.filter((el, i) => i !== index && (() => { const r = el.getBoundingClientRect(); return r.top + r.height / 2 < y; })()).length;
  const mark = (to: number, items: HTMLElement[]) => {
    items.forEach((el) => (el.style.boxShadow = ""));
    const others = items.filter((_, i) => i !== index);
    if (to < others.length) others[to].style.boxShadow = "0 -2px 0 0 #38bdf8";
    else others[others.length - 1] && (others[others.length - 1].style.boxShadow = "0 2px 0 0 #38bdf8");
  };
  return (
    <button
      className="cursor-grab touch-none rounded p-0.5 text-neutral-400 hover:text-neutral-200 active:cursor-grabbing"
      aria-label={label}
      title={`${label} (drag, or Alt+Up / Alt+Down)`}
      data-testid={testid}
      onClick={onPick}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        const ul = e.currentTarget.closest("li")?.parentElement;
        if (!ul) return;
        e.currentTarget.setPointerCapture(e.pointerId);
        st.current = { items: Array.from(ul.children).filter((c): c is HTMLElement => c instanceof HTMLElement && c.tagName === "LI"), to: index };
      }}
      onPointerMove={(e) => {
        const s = st.current;
        if (!s) return;
        s.to = target(e.clientY, s.items);
        mark(s.to, s.items);
      }}
      onPointerUp={() => {
        const s = st.current;
        st.current = null;
        if (!s) return;
        s.items.forEach((el) => (el.style.boxShadow = ""));
        if (s.to !== index) onMove(s.to);
      }}
      onPointerCancel={() => {
        clear();
        st.current = null;
      }}
    >
      <GripVertical className="size-3.5" />
    </button>
  );
}

function GroupRow({ g, index, masks }: { g: MaskGroup; index: number; masks: MasksApi }) {
  const on = masks.selGroup === g.id;
  const [renaming, setRenaming] = useState(false);
  const rowRef = useRef<HTMLLIElement>(null);
  useEffect(() => {
    if (on) rowRef.current?.scrollIntoView({ block: "nearest" });
  }, [on]);
  const single = g.components.length === 1;
  const KindIcon = ICON[compKind(g.components[0])];
  const st = single && g.components[0].shape.kind === "ai" ? masks.aiState[g.components[0].id] : undefined;
  const unsupported = g.components.some((c) => c.shape.kind === "unsupported");
  return (
    <li
      ref={rowRef}
      className={`rounded border ${on ? "border-sky-600 bg-neutral-900" : "border-neutral-800 bg-neutral-900/50"}`}
      data-testid={`mask-group-${g.id}`}
      data-selected={on}
      data-active={g.active}
      onMouseEnter={() => masks.setHover({ groupId: g.id, componentId: null })}
      onMouseLeave={() => masks.setHover(null)}
    >
      <div className="flex items-center gap-1 px-1.5 py-1">
        <ReorderHandle index={index} onMove={(to) => masks.reorderGroup(g.id, to)} onPick={() => masks.select(g.id, null)} testid={`mask-group-handle-${g.id}`} label="Reorder mask" />
        {single && <KindIcon className="size-3.5 shrink-0 text-neutral-400" aria-hidden />}
        <button className="text-neutral-300 hover:text-white" onClick={() => masks.toggleGroup(g.id)} title={g.active ? "Hide this mask's adjustments" : "Show this mask's adjustments"} aria-label="Toggle mask" aria-pressed={g.active} data-testid={`mask-group-eye-${g.id}`}>
          {g.active ? <Eye className="size-3.5" /> : <EyeOff className="size-3.5 opacity-60" />}
        </button>
        {renaming ? (
          <RenameInput
            initial={g.name}
            testid={`mask-group-rename-input-${g.id}`}
            onDone={(v) => {
              setRenaming(false);
              if (v != null && v !== g.name) masks.renameGroup(g.id, v);
            }}
          />
        ) : (
          <button className="min-w-0 flex-1 truncate text-left text-xs" onClick={() => masks.select(g.id, null)} onDoubleClick={() => setRenaming(true)} title="Click to select, double-click to rename" data-testid={`mask-group-name-${g.id}`}>
            {g.name || "Mask"}
          </button>
        )}
        {single && !on && st === "needs_update" && <span className="rounded bg-amber-900/70 px-1 text-[10px] text-amber-100" data-testid={`mask-comp-update-${g.components[0].id}`}>update</span>}
        {single && !on && unsupported && <span className="rounded bg-neutral-700 px-1 text-[10px]">not rendered</span>}
        <Menu
          trigger={<MoreHorizontal className="size-4" />}
          triggerClass="rounded p-0.5 text-neutral-300 hover:bg-neutral-800"
          triggerTestId={`mask-group-menu-${g.id}`}
          title="Mask actions"
          align="right"
        >
          {(close) => (
            <div>
              <button className={menuItem} data-testid="mask-action-rename" onClick={() => { close(); setRenaming(true); }}>
                Rename
              </button>
              <button className={menuItem} data-testid="mask-action-duplicate" onClick={() => { close(); masks.duplicateGroup(g.id); }}>
                <Copy className="size-3.5" /> Duplicate
              </button>
              <button className={menuItem} data-testid="mask-action-invert" onClick={() => { close(); masks.invertGroup(g.id); }}>
                <FlipHorizontal2 className="size-3.5" /> Invert
              </button>
              <button className={menuItem} data-testid="mask-action-delete" onClick={() => { close(); masks.deleteGroup(g.id); }}>
                <Trash2 className="size-3.5" /> Delete
              </button>
            </div>
          )}
        </Menu>
      </div>
      {(!single || on) && (
        <ul className="space-y-0.5 px-1.5 pb-1">
          {g.components.map((c, i) => (
            <ComponentRow key={c.id} g={g} c={c} index={i} masks={masks} />
          ))}
        </ul>
      )}
      {on && (
        <div className="flex gap-1 px-1.5 pb-1.5" data-testid={`mask-add-row-${g.id}`}>
          {MODES.map((m) => (
            <AddMenu key={m.mode} g={g} mode={m.mode} masks={masks} />
          ))}
        </div>
      )}
    </li>
  );
}

function RenameInput({ initial, onDone, testid }: { initial: string; onDone: (v: string | null) => void; testid: string }) {
  const ref = useRef<HTMLInputElement>(null);
  const [v, setV] = useState(initial);
  useEffect(() => ref.current?.select(), []);
  return (
    <input
      ref={ref}
      className="min-w-0 flex-1 rounded bg-neutral-800 px-1 py-0.5 text-xs"
      value={v}
      maxLength={64}
      data-testid={testid}
      aria-label="Mask name"
      autoFocus
      onChange={(e) => setV(e.target.value)}
      onBlur={() => onDone(v.trim() || null)}
      onKeyDown={(e) => {
        if (e.key === "Enter") onDone(v.trim() || null);
        else if (e.key === "Escape") {
          e.stopPropagation();
          onDone(null);
        }
      }}
    />
  );
}

function ComponentRow({ g, c, index, masks }: { g: MaskGroup; c: MaskComponent; index: number; masks: MasksApi }) {
  const on = masks.selComp === c.id && masks.selGroup === g.id;
  const st = c.shape.kind === "ai" ? masks.aiState[c.id] : undefined;
  const first = g.components[0]?.id === c.id;
  return (
    <li
      className={`flex items-center gap-1 rounded px-1 py-0.5 text-xs ${on ? "bg-sky-900/60" : "hover:bg-neutral-800"}`}
      data-testid={`mask-comp-${c.id}`}
      data-selected={on}
      data-kind={c.shape.kind}
      data-mode={c.mode}
      data-inverted={c.inverted}
      data-active={c.active}
      onMouseEnter={(e) => {
        e.stopPropagation();
        masks.setHover({ groupId: g.id, componentId: c.id });
      }}
      onMouseLeave={() => masks.setHover({ groupId: g.id, componentId: null })}
    >
      <ReorderHandle index={index} onMove={(to) => masks.reorderComponent(g.id, c.id, to)} onPick={() => masks.select(g.id, c.id)} testid={`mask-comp-handle-${c.id}`} label="Reorder component" />
      <button className="text-neutral-300 hover:text-white" onClick={() => masks.patchComponent(g.id, c.id, { active: !c.active }, "Mask: Component visibility")} aria-label="Toggle component" aria-pressed={c.active} title="Show or hide this component" data-testid={`mask-comp-eye-${c.id}`}>
        {c.active ? <Eye className="size-3" /> : <EyeOff className="size-3 opacity-60" />}
      </button>
      <span className="w-3 text-center text-neutral-400" title={first ? "First component" : MODES.find((m) => m.mode === c.mode)?.label}>
        {first ? "" : MODE_GLYPH[c.mode]}
      </span>
      <button className="min-w-0 flex-1 truncate text-left" onClick={() => masks.select(g.id, c.id)} onDoubleClick={() => undefined} data-testid={`mask-comp-name-${c.id}`}>
        {componentTitle(c)}
        {c.inverted && <span className="ml-1 text-neutral-400">(inverted)</span>}
      </button>
      {st === "needs_update" && (
        <span className="rounded bg-amber-900/70 px-1 text-[10px] text-amber-100" data-testid={`mask-comp-update-${c.id}`} title="This AI selection has not been computed for this photo">
          update
        </span>
      )}
      {c.shape.kind === "unsupported" && (
        <span className="rounded bg-neutral-700 px-1 text-[10px]" title="Kept in the sidecar but not rendered">
          not rendered
        </span>
      )}
      {c.shape.kind === "brush" && (
        <button
          className="rounded px-1 text-[10px] text-neutral-300 hover:bg-neutral-700"
          title="Paint more strokes on this component"
          data-testid={`mask-comp-paint-${c.id}`}
          onClick={() => {
            masks.select(g.id, c.id);
            masks.beginTool("brush", { groupId: g.id, mode: c.mode }, c.id);
          }}
        >
          Paint
        </button>
      )}
      <button className={`rounded p-0.5 ${c.inverted ? "bg-sky-800 text-sky-100" : "text-neutral-300 hover:bg-neutral-700"}`} onClick={() => masks.patchComponent(g.id, c.id, { inverted: !c.inverted }, "Mask: Invert")} title="Invert this component" aria-label="Invert component" aria-pressed={c.inverted} data-testid={`mask-comp-invert-${c.id}`}>
        <FlipHorizontal2 className="size-3" />
      </button>
      <button className="rounded p-0.5 text-neutral-300 hover:bg-neutral-700" onClick={() => masks.deleteComponent(g.id, c.id)} title="Delete this component" aria-label="Delete component" data-testid={`mask-comp-delete-${c.id}`}>
        <Trash2 className="size-3" />
      </button>
    </li>
  );
}

// ---- tool / component settings ----

function ToolSettings({ masks, group, comp }: { masks: MasksApi; group: MaskGroup | null; comp: MaskComponent | null }) {
  const brushActive = masks.tool?.kind === "brush" || comp?.shape.kind === "brush";
  const sh = group ? comp?.shape : undefined;
  const patchShape = (fn: (s: NonNullable<typeof sh>) => NonNullable<typeof sh>, label: string, live: boolean) => {
    if (!comp || !group) return;
    masks.editShape(group.id, comp.id, fn, label);
    if (!live) masks.commit();
  };
  const pctSlider = (id: string, label: string, value: number, min: number, max: number, apply: (v: number) => void, reset: number, digits = 0) => (
    <Slider id={id} label={label} value={value} min={min} max={max} step={1} digits={digits} defaultValue={reset} onInput={apply} onCommit={masks.commit} onReset={() => { apply(reset); masks.commit(); }} />
  );
  return (
    <section className="border-t border-neutral-800 py-2" data-testid="mask-tool-settings">
      {brushActive && (
        <div data-testid="mask-brush-settings">
          <div className="mb-1 text-[11px] font-semibold uppercase tracking-wide text-neutral-400">Brush</div>
          <div className="mb-2 flex gap-1">
            <button className={seg(!masks.brush.erase)} onClick={() => masks.patchBrush({ erase: false })} data-testid="mask-brush-paint" aria-pressed={!masks.brush.erase}>
              Paint
            </button>
            <button className={seg(masks.brush.erase)} onClick={() => masks.patchBrush({ erase: true })} data-testid="mask-brush-erase" aria-pressed={masks.brush.erase} title="Erase (hold Alt while painting)">
              Erase
            </button>
          </div>
          {pctSlider("mask-brush-size", "Size", masks.brush.size, 1, 100, (v) => masks.patchBrush({ size: v }), 20)}
          {pctSlider("mask-brush-feather", "Feather", masks.brush.feather, 0, 100, (v) => masks.patchBrush({ feather: v }), 50)}
          {pctSlider("mask-brush-flow", "Flow", masks.brush.flow, 0, 100, (v) => masks.patchBrush({ flow: v }), 50)}
          {pctSlider("mask-brush-density", "Density", masks.brush.density, 0, 100, (v) => masks.patchBrush({ density: v }), 100)}
          <label className="mb-1 flex items-center gap-1.5 text-[11px] text-neutral-300" title="Confine strokes to edges similar to the colour under the brush (A)">
            <input type="checkbox" checked={masks.brush.autoMask} onChange={(e) => masks.patchBrush({ autoMask: e.target.checked })} data-testid="mask-brush-auto" /> Auto Mask
          </label>
          {masks.tool?.kind === "brush" && (
            <button className="rounded bg-sky-700 px-2 py-0.5 text-xs text-white hover:bg-sky-600" onClick={masks.endTool} data-testid="mask-tool-done" title="Finish painting (Esc)">
              Done
            </button>
          )}
        </div>
      )}

      {group && comp && (
        <div className="mt-2" data-testid="mask-comp-settings">
          {pctSlider("mask-comp-opacity", "Component opacity", Math.round(comp.opacity * 100), 0, 100, (v) => masks.patchComponent(group.id, comp.id, { opacity: v / 100 }, "Mask: Opacity"), 100)}
        </div>
      )}

      {group && comp && sh?.kind === "radial" && (
        <div data-testid="mask-radial-settings">
          {pctSlider("mask-radial-feather", "Feather", sh.feather, 0, 100, (v) => patchShape((s) => (s.kind === "radial" ? { ...s, feather: v } : s), "Mask: Radial Gradient", true), 50)}
          {pctSlider("mask-radial-roundness", "Roundness", sh.roundness, -100, 100, (v) => patchShape((s) => (s.kind === "radial" ? { ...s, roundness: v } : s), "Mask: Radial Gradient", true), 0)}
        </div>
      )}

      {group && comp && sh?.kind === "color" && (
        <div data-testid="mask-color-settings">
          <div className="mb-1 flex flex-wrap items-center gap-1" data-testid="mask-color-samples" data-count={sh.samples.length}>
            {sh.samples.map((_, i) => (
              <span key={i} className="flex items-center gap-0.5 rounded bg-neutral-800 py-0.5 pl-1.5 pr-0.5 text-[11px] text-neutral-200" data-testid={`mask-color-sample-${i}`} title={`Sample ${i + 1}`}>
                <span className="size-3 rounded-sm border border-neutral-500 bg-sky-400/60" />
                {i + 1}
                <button
                  className="rounded px-0.5 text-neutral-400 hover:bg-neutral-700 hover:text-white disabled:opacity-30"
                  disabled={sh.samples.length <= 1}
                  aria-label={`Remove sample ${i + 1}`}
                  title="Remove this sample"
                  data-testid={i === sh.samples.length - 1 ? "mask-color-remove-sample" : `mask-color-remove-sample-${i}`}
                  onClick={() => patchShape((x) => (x.kind === "color" ? { ...x, samples: x.samples.filter((_, j) => j !== i) } : x), "Mask: Color Range", false)}
                >
                  ×
                </button>
              </span>
            ))}
          </div>
          {pctSlider("mask-color-amount", "Refine", sh.amount, 0, 100, (v) => patchShape((s) => (s.kind === "color" ? { ...s, amount: v } : s), "Mask: Color Range", true), 50)}
          <div className="flex gap-1">
            <button className="rounded bg-neutral-800 px-2 py-0.5 text-xs hover:bg-neutral-700 disabled:opacity-40" disabled={sh.samples.length >= 5} onClick={() => masks.beginTool("color", { groupId: group.id, mode: comp.mode }, comp.id)} data-testid="mask-color-add-sample" title="Click or drag on the photo to add a colour sample">
              Add sample
            </button>
          </div>
        </div>
      )}

      {group && comp && sh?.kind === "luminance" && (
        <div data-testid="mask-luminance-settings">
          {(
            [
              ["featherLow", "Feather low"],
              ["low", "Low"],
              ["high", "High"],
              ["featherHigh", "Feather high"],
            ] as const
          ).map(([k, label]) => (
            <Slider
              key={k}
              id={`mask-lum-${k}`}
              label={label}
              value={Math.round(sh[k] * 100)}
              min={0}
              max={100}
              step={1}
              onInput={(v) => patchShape((s) => (s.kind === "luminance" ? orderLum({ ...s, [k]: v / 100 }, k) : s), "Mask: Luminance Range", true)}
              onCommit={masks.commit}
              onReset={() => {
                const d = { featherLow: 0, low: 0, high: 1, featherHigh: 1 }[k];
                patchShape((s) => (s.kind === "luminance" ? orderLum({ ...s, [k]: d }, k) : s), "Mask: Luminance Range", true);
                masks.commit();
              }}
            />
          ))}
          {pctSlider("mask-lum-smoothness", "Smoothness", sh.smoothness, 0, 100, (v) => patchShape((s) => (s.kind === "luminance" ? { ...s, smoothness: v } : s), "Mask: Luminance Range", true), 50)}
          <button className="rounded bg-neutral-800 px-2 py-0.5 text-xs hover:bg-neutral-700" onClick={() => masks.beginTool("luminance", { groupId: group.id, mode: comp.mode }, comp.id)} data-testid="mask-luminance-sample" title="Click the photo to pick a brightness">
            Sample from photo
          </button>
        </div>
      )}
    </section>
  );
}

/** Keeps featherLow <= low <= high <= featherHigh, moving the neighbours the user did not drag. */
function orderLum<T extends { featherLow: number; low: number; high: number; featherHigh: number }>(s: T, moved: "featherLow" | "low" | "high" | "featherHigh"): T {
  const o = { ...s };
  if (moved === "featherLow") o.featherLow = Math.min(o.featherLow, o.low);
  if (moved === "low") {
    o.low = Math.min(o.low, o.high);
    o.featherLow = Math.min(o.featherLow, o.low);
  }
  if (moved === "high") {
    o.high = Math.max(o.high, o.low);
    o.featherHigh = Math.max(o.featherHigh, o.high);
  }
  if (moved === "featherHigh") o.featherHigh = Math.max(o.featherHigh, o.high);
  return o;
}

function LocalSliders({ masks, g }: { masks: MasksApi; g: MaskGroup }) {
  const defaults = defaultLocalAdjustments();
  const setLocal = (key: string, v: number, label: string, commit: boolean) => {
    const fn = (x: MaskGroup): MaskGroup => ({ ...x, adjustments: { ...x.adjustments, [key]: v } });
    if (commit) masks.changeGroup(g.id, fn, label);
    else masks.liveGroup(g.id, fn, label);
  };
  const color = g.adjustments.color;
  // Per-mask point curves (`LocalAdjustments.toneCurve`, absent = identity), edited with the Tone Curve panel's editor.
  const curves = g.adjustments.toneCurve ?? identityCurves();
  const curvesDirty = (Object.keys(curves) as (keyof PointCurves)[]).some((k) => JSON.stringify(curves[k]) !== JSON.stringify(IDENTITY_CURVE));
  const curveFn = (ch: keyof PointCurves, fn: (pts: CurvePoint[]) => CurvePoint[]) => (x: MaskGroup): MaskGroup => {
    const cur = (x.adjustments.toneCurve ?? identityCurves())[ch];
    const next = fn(cur);
    return next === cur ? x : { ...x, adjustments: { ...x.adjustments, toneCurve: { ...(x.adjustments.toneCurve ?? identityCurves()), [ch]: next } } };
  };
  const curveHost: CurveHost = {
    curves,
    edit: (ch, fn) => masks.liveGroup(g.id, curveFn(ch, fn), `Mask: ${g.name} Tone Curve`),
    change: (ch, fn) => masks.changeGroup(g.id, curveFn(ch, fn), `Mask: ${g.name} Tone Curve`),
    commit: masks.commit,
  };
  const setColor = (patch: Partial<typeof color>, label: string, commit: boolean) => {
    const fn = (x: MaskGroup): MaskGroup => ({ ...x, adjustments: { ...x.adjustments, color: { ...x.adjustments.color, ...patch } } });
    if (commit) masks.changeGroup(g.id, fn, label);
    else masks.liveGroup(g.id, fn, label);
  };
  return (
    <div data-testid="mask-locals">
      <section className="border-t border-neutral-800 py-2">
        <Slider
          id="mask-amount"
          label="Amount"
          value={Math.round(g.amount * 100)}
          min={0}
          max={200}
          step={1}
          defaultValue={100}
          display={(v) => `${v}%`}
          onInput={(v) => masks.liveGroup(g.id, (x) => ({ ...x, amount: v / 100 }), `Mask: ${g.name} Amount`)}
          onCommit={masks.commit}
          onReset={() => masks.changeGroup(g.id, (x) => ({ ...x, amount: 1 }), `Mask: ${g.name} Amount`)}
        />
      </section>
      {LOCAL_GROUPS.map((grp) => (
        <Section
          key={grp.id}
          id={`mask-${grp.id}`}
          title={grp.title}
          dirty={grp.defs.some((d) => g.adjustments[d.key] !== defaults[d.key])}
          onReset={() =>
            masks.changeGroup(g.id, (x) => ({ ...x, adjustments: { ...x.adjustments, ...Object.fromEntries(grp.defs.map((d) => [d.key, defaults[d.key]])) } }), `Mask: ${g.name} Reset ${grp.title}`)
          }
        >
          {grp.defs.map((d) => (
            <Slider
              key={d.key}
              id={`mask-${d.key}`}
              label={d.label}
              value={g.adjustments[d.key]}
              min={d.min}
              max={d.max}
              step={d.step}
              digits={d.digits}
              defaultValue={defaults[d.key]}
              accent={d.accent}
              onInput={(v) => setLocal(d.key, v, `Mask: ${g.name} ${d.label}`, false)}
              onCommit={masks.commit}
              onReset={() => setLocal(d.key, defaults[d.key], `Mask: ${g.name} ${d.label}`, true)}
            />
          ))}
        </Section>
      ))}
      <Section
        id="mask-tonecurve"
        title="Tone Curve"
        dirty={curvesDirty || g.adjustments.curveRefineSaturation !== defaults.curveRefineSaturation}
        onReset={() => masks.changeGroup(g.id, (x) => ({ ...x, adjustments: { ...x.adjustments, toneCurve: identityCurves(), curveRefineSaturation: defaults.curveRefineSaturation } }), `Mask: ${g.name} Reset Tone Curve`)}
      >
        <PointCurveEditor host={curveHost} tid="mask-curve" />
        <Slider
          id="mask-curveRefineSaturation"
          label="Refine Saturation"
          value={g.adjustments.curveRefineSaturation}
          min={0}
          max={100}
          step={1}
          defaultValue={100}
          onInput={(v) => setLocal("curveRefineSaturation", v, `Mask: ${g.name} Refine Saturation`, false)}
          onCommit={masks.commit}
          onReset={() => setLocal("curveRefineSaturation", 100, `Mask: ${g.name} Refine Saturation`, true)}
        />
      </Section>
      <Section id="mask-color" dirty={color.saturation !== defaults.color.saturation || color.hue !== defaults.color.hue} title="Color" onReset={() => masks.changeGroup(g.id, (x) => ({ ...x, adjustments: { ...x.adjustments, color: defaults.color } }), `Mask: ${g.name} Reset Color`)}>
        <div className="mb-1 flex items-center gap-2 text-[11px] text-neutral-300">
          <span className="size-5 rounded border border-neutral-600" style={{ backgroundColor: color.saturation > 0 ? `hsl(${color.hue} ${color.saturation}% 50%)` : "transparent" }} data-testid="mask-color-swatch" title="Color tint applied inside the mask" />
          Tint
        </div>
        <Slider id="mask-color-hue" label="Hue" value={color.hue} min={0} max={360} step={1} accent={`hsl(${color.hue} 80% 55%)`} onInput={(v) => setColor({ hue: v }, `Mask: ${g.name} Color`, false)} onCommit={masks.commit} onReset={() => setColor({ hue: 0 }, `Mask: ${g.name} Color`, true)} />
        <Slider id="mask-color-sat" label="Saturation" value={color.saturation} min={0} max={100} step={1} onInput={(v) => setColor({ saturation: v }, `Mask: ${g.name} Color`, false)} onCommit={masks.commit} onReset={() => setColor({ saturation: 0 }, `Mask: ${g.name} Color`, true)} />
      </Section>
    </div>
  );
}
