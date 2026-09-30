// Profiles: the Profile row in Basic (current profile dropdown + browser button + Amount) and the full-panel Profile Browser
// (camera profiles, Adobe looks and `.cube` LUTs, grouped like the preset library), all from `list_profiles` (v14).
import { useEffect, useMemo, useState } from "react";
import { ChevronDown, ChevronRight, FileUp, LayoutGrid, Loader2, X } from "lucide-react";
import { commands, unwrap, type CameraProfileInfo, type CompleteAdjustments, type LookProfileInfo, type LutProfileInfo, type ParametricAdjustments, type ProfileCatalog } from "../../ipc";
import type { Editor } from "../../hooks/useEditor";
import { Slider } from "./Slider";
import { Menu, menuItem } from "../Menu";

const item = (on: boolean) => `flex h-7 w-full items-center justify-between rounded px-2 text-left text-xs disabled:opacity-40 ${on ? "bg-sky-800 text-sky-100" : "hover:bg-neutral-800"}`;

/** `list_profiles(imageId)`; `version` re-reads it (after a style import). */
export function useProfileCatalog(imageId: number | null, onError: (e: unknown) => void, version = 0): ProfileCatalog | null {
  const [catalog, setCatalog] = useState<ProfileCatalog | null>(null);
  useEffect(() => {
    setCatalog(null);
    if (imageId == null) return;
    let stale = false;
    unwrap(commands.listProfiles(imageId))
      .then((c) => !stale && setCatalog(c))
      .catch((e) => !stale && onError(e));
    return () => {
      stale = true;
    };
  }, [imageId, onError, version]);
  return catalog;
}

// ---- recent profiles (the dropdown's "5 most recent") ----
interface Recent {
  kind: "none" | "camera" | "look" | "lut";
  name: string;
  uuid?: string;
  cameraProfile?: string | null;
  lutId?: string;
}
const RECENT_KEY = "sieve.profiles.recent.v1";
const loadRecent = (): Recent[] => {
  try {
    const v = JSON.parse(localStorage.getItem(RECENT_KEY) ?? "[]");
    return Array.isArray(v) ? v.slice(0, 5) : [];
  } catch {
    return [];
  }
};
const pushRecent = (r: Recent) => {
  const next = [r, ...loadRecent().filter((x) => !(x.kind === r.kind && x.name === r.name && x.uuid === r.uuid))].slice(0, 5);
  try {
    localStorage.setItem(RECENT_KEY, JSON.stringify(next));
  } catch {
    /* not remembered */
  }
};

/** Apply helpers shared by the dropdown and the browser (a profile pick replaces any LUT, like `applyStyleProfile`). */
function useProfilePicks(editor: Editor) {
  const withCamera = (adj: CompleteAdjustments, name: string | null): CompleteAdjustments => ({ ...adj, lut: null, profile: { cameraProfile: name, look: null } });
  const withLook = (adj: CompleteAdjustments, l: { name: string; uuid: string; cameraProfile?: string | null }): CompleteAdjustments => ({
    ...adj,
    lut: null,
    profile: { cameraProfile: l.cameraProfile ?? adj.profile.cameraProfile, look: { name: l.name, uuid: l.uuid, amount: 1 } },
  });
  const withLut = (adj: CompleteAdjustments, l: { lutId: string }): CompleteAdjustments => ({ ...adj, lut: { id: l.lutId, amount: 100 } });
  const pickCamera = (name: string | null) => {
    pushRecent({ kind: name ? "camera" : "none", name: name ?? "None" });
    editor.change((a) => withCamera(a, name), `Profile: ${name ?? "None"}`);
  };
  const pickLook = (l: { name: string; uuid: string; cameraProfile?: string | null }) => {
    pushRecent({ kind: "look", name: l.name, uuid: l.uuid, cameraProfile: l.cameraProfile });
    editor.change((a) => withLook(a, l), `Profile: ${l.name}`);
  };
  const pickLut = (l: { lutId: string; name: string }) => {
    pushRecent({ kind: "lut", name: l.name, lutId: l.lutId });
    editor.change((a) => withLut(a, l), "LUT");
  };
  return { pickCamera, pickLook, pickLut, withCamera, withLook, withLut };
}

export function ProfileRow({ editor, catalog, onBrowse }: { editor: Editor; catalog: ProfileCatalog | null; onBrowse: () => void }) {
  const { cameraProfile, look } = editor.adj.profile;
  const lut = editor.adj.lut;
  const lookInfo = catalog?.looks.find((l) => l.uuid === look?.uuid);
  const { pickCamera, pickLook, pickLut } = useProfilePicks(editor);
  const current = lut ? `LUT: ${catalog?.luts.find((l) => l.lutId === lut.id)?.name ?? lut.id}` : (look?.name ?? cameraProfile ?? "None");
  const recent = loadRecent();
  return (
    <div data-testid="profile-panel">
      <div className="flex h-7 items-center gap-2">
        <span className="w-[72px] shrink-0 text-xs text-neutral-300">Profile</span>
        <div className="min-w-0 flex-1">
          <Menu
            trigger={
              <>
                <span className="min-w-0 flex-1 truncate text-left" data-testid="profile-current" title={cameraProfile ? `${current} (${cameraProfile})` : current} data-camera={cameraProfile ?? ""} data-look={look?.uuid ?? ""}>
                  {current}
                </span>
                <ChevronDown className="size-3 shrink-0 text-neutral-400" />
              </>
            }
            triggerClass="flex h-6 w-full items-center gap-1 rounded bg-neutral-800 px-2 text-xs hover:bg-neutral-700"
            triggerTestId="profile-select"
            title="Profile"
          >
            {(close) => (
              <>
                {recent.length === 0 && <p className="px-3 py-1.5 text-xs text-neutral-400">No recent profiles</p>}
                {recent.map((r) => (
                  <button
                    key={`${r.kind}-${r.name}-${r.uuid ?? ""}`}
                    role="menuitem"
                    className={menuItem}
                    data-testid={`profile-recent-${r.name}`}
                    onClick={() => {
                      close();
                      if (r.kind === "look" && r.uuid) pickLook({ name: r.name, uuid: r.uuid, cameraProfile: r.cameraProfile });
                      else if (r.kind === "lut" && r.lutId) pickLut({ lutId: r.lutId, name: r.name });
                      else pickCamera(r.kind === "none" ? null : r.name);
                    }}
                  >
                    {r.name}
                  </button>
                ))}
                <div className="my-1 border-t border-neutral-700" />
                <button
                  role="menuitem"
                  className={menuItem}
                  data-testid="profile-browse-menu"
                  onClick={() => {
                    close();
                    onBrowse();
                  }}
                >
                  Browse…
                </button>
              </>
            )}
          </Menu>
        </div>
        <button className="flex size-6 shrink-0 items-center justify-center rounded bg-neutral-800 hover:bg-neutral-700" onClick={onBrowse} title="Browse profiles" aria-label="Browse profiles" data-testid="profile-browse">
          <LayoutGrid className="size-3.5" />
        </button>
      </div>
      {look && lookInfo?.supportsAmount && (
        <Slider
          id="look-amount"
          label="Amount"
          value={Math.round(look.amount * 100)}
          min={0}
          max={200}
          step={1}
          defaultValue={100}
          display={(v) => `${v}%`}
          onInput={(v) => editor.edit((a) => ({ ...a, profile: { ...a.profile, look: a.profile.look ? { ...a.profile.look, amount: v / 100 } : null } }), "Profile: Amount")}
          onCommit={editor.commit}
          onReset={() => editor.change((a) => ({ ...a, profile: { ...a.profile, look: a.profile.look ? { ...a.profile.look, amount: 1 } : null } }), "Profile: Amount")}
        />
      )}
    </div>
  );
}

function Group({ id, title, children }: { id: string; title: string; children: React.ReactNode }) {
  const [open, setOpen] = useState(true);
  return (
    <div data-testid={`profile-group-${id}`}>
      <button className="flex h-6 w-full items-center gap-1 px-1 text-[11px] font-semibold uppercase tracking-wide text-neutral-400" onClick={() => setOpen(!open)} aria-expanded={open}>
        {open ? <ChevronDown className="size-3" /> : <ChevronRight className="size-3" />}
        {title}
      </button>
      {open && children}
    </div>
  );
}

type Filter = "all" | "color" | "bw";

export interface ProfileHover {
  start: (label: string, adj: ParametricAdjustments) => void;
  stop: () => void;
}

export function ProfileBrowser({ editor, catalog, importing, onImport, hover, onClose }: { editor: Editor; catalog: ProfileCatalog | null; importing: boolean; onImport: () => void; hover: ProfileHover; onClose: () => void }) {
  const { cameraProfile, look } = editor.adj.profile;
  const lut = editor.adj.lut;
  const [filter, setFilter] = useState<Filter>("all");
  const lookInfo = catalog?.looks.find((l) => l.uuid === look?.uuid);
  const { pickCamera, pickLook, pickLut, withCamera, withLook, withLut } = useProfilePicks(editor);
  // Camera profiles, looks and LUTs that share a group name (e.g. "Adobe Raw", an imported folder) are listed together.
  const groups = useMemo(() => {
    const m = new Map<string, { cams: CameraProfileInfo[]; looks: LookProfileInfo[]; luts: LutProfileInfo[] }>();
    const slot = (g: string) => m.get(g || "Other") ?? (m.set(g || "Other", { cams: [], looks: [], luts: [] }), m.get(g || "Other")!);
    if (filter !== "bw") catalog?.cameraProfiles.forEach((p) => slot(p.group).cams.push(p));
    catalog?.looks.filter((l) => (filter === "all" ? true : filter === "bw" ? l.monochrome : !l.monochrome)).forEach((l) => slot(l.group).looks.push(l));
    if (filter !== "bw") catalog?.luts.forEach((l) => slot(l.group).luts.push(l));
    return [...m.entries()].filter(([, v]) => v.cams.length + v.looks.length + v.luts.length > 0);
  }, [catalog, filter]);
  const hov = (label: string, adj: ParametricAdjustments) => ({ onMouseEnter: () => hover.start(label, adj), onMouseLeave: hover.stop });
  const amount =
    look && lookInfo?.supportsAmount ? (
      <Slider
        id="look-amount"
        label="Amount"
        value={Math.round(look.amount * 100)}
        min={0}
        max={200}
        step={1}
        defaultValue={100}
        display={(v) => `${v}%`}
        onInput={(v) => editor.edit((a) => ({ ...a, profile: { ...a.profile, look: a.profile.look ? { ...a.profile.look, amount: v / 100 } : null } }), "Profile: Amount")}
        onCommit={editor.commit}
        onReset={() => editor.change((a) => ({ ...a, profile: { ...a.profile, look: a.profile.look ? { ...a.profile.look, amount: 1 } : null } }), "Profile: Amount")}
      />
    ) : null;

  return (
    <div data-testid="profile-browser" className="pb-2">
      <div className="sticky top-0 z-10 bg-neutral-950 pb-1">
        <div className="flex h-8 items-center justify-between border-b border-neutral-800">
          <h3 className="text-xs font-semibold uppercase tracking-wide text-neutral-300">Profile Browser</h3>
          <button className="flex items-center gap-1 rounded px-1.5 py-0.5 text-xs text-neutral-300 hover:bg-neutral-800" onClick={onClose} title="Close (Esc)" data-testid="profile-browser-close">
            <X className="size-3.5" /> Close
          </button>
        </div>
        <div className="flex h-7 items-center gap-2">
          <span className="w-[72px] shrink-0 text-xs text-neutral-300">Show</span>
          <select className="h-6 min-w-0 flex-1 rounded bg-neutral-800 px-1.5 text-xs" value={filter} onChange={(e) => setFilter(e.target.value as Filter)} aria-label="Profile filter" data-testid="profile-filter">
            <option value="all">All</option>
            <option value="color">Color</option>
            <option value="bw">B&amp;W</option>
          </select>
        </div>
        {amount}
        {lut && (
          <Slider
            id="lut-amount"
            label="Amount"
            value={lut.amount}
            min={0}
            max={200}
            step={1}
            defaultValue={100}
            display={(v) => `${v}%`}
            onInput={(v) => editor.edit((a) => ({ ...a, lut: a.lut ? { ...a.lut, amount: v } : a.lut }), "LUT")}
            onCommit={editor.commit}
            onReset={() => editor.change((a) => ({ ...a, lut: a.lut ? { ...a.lut, amount: 100 } : null }), "LUT")}
          />
        )}
      </div>
      {!catalog && <p className="p-2 text-xs text-neutral-400">Loading profiles...</p>}
      {catalog && catalog.cameraProfiles.length === 0 && catalog.looks.length === 0 && (
        <p className="p-2 text-[11px] text-amber-400" data-testid="profile-empty">
          No Adobe profiles found. Install Adobe DNG Converter or Camera Raw ({catalog.searchDirs[0] ?? "CameraProfiles"}).
        </p>
      )}
      {catalog && (
        <>
          <button className={item(cameraProfile === null && !look && !lut)} onClick={() => pickCamera(null)} data-testid="profile-item-none" {...hov("None", withCamera(editor.adj, null))}>
            <span>None</span>
            <span className="text-[10px] text-neutral-400">Sieve base</span>
          </button>
          {groups.map(([g, { cams, looks, luts }]) => (
            <Group key={g} id={g} title={g}>
              {cams.map((p) => (
                <button key={p.name} className={item(cameraProfile === p.name && !look && !lut)} onClick={() => pickCamera(p.name)} data-testid={`profile-item-${p.name}`} {...hov(p.name, withCamera(editor.adj, p.name))}>
                  {p.name}
                </button>
              ))}
              {looks.map((l) => (
                <button
                  key={l.uuid}
                  className={item(look?.uuid === l.uuid)}
                  disabled={!l.available}
                  title={l.available ? l.name : `${l.name} is not installed on this Mac`}
                  onClick={() => pickLook(l)}
                  data-testid={`look-item-${l.uuid}`}
                  {...hov(l.name, withLook(editor.adj, l))}
                >
                  <span className="truncate">{l.name}</span>
                  {!l.available ? <span className="text-[10px] text-amber-500">not installed</span> : l.monochrome ? <span className="text-[10px] text-neutral-400">B&amp;W</span> : null}
                </button>
              ))}
              {luts.map((l) => (
                <button
                  key={l.lutId}
                  className={item(lut?.id === l.lutId)}
                  disabled={!l.available}
                  title={l.available ? l.name : `${l.name}: the LUT file is missing`}
                  onClick={() => pickLut(l)}
                  data-testid={`lut-item-${l.lutId}`}
                  {...hov(l.name, withLut(editor.adj, l))}
                >
                  <span className="truncate">{l.name}</span>
                  <span className="text-[10px] text-neutral-400">LUT</span>
                </button>
              ))}
            </Group>
          ))}
          {lut && !catalog.luts.some((l) => l.lutId === lut.id) && (
            <button className={item(true)} onClick={() => pickCamera(cameraProfile)} data-testid="lut-item-missing" title="Click to remove the missing LUT">
              <span className="truncate">{lut.id}</span>
              <span className="text-[10px] text-amber-400">missing</span>
            </button>
          )}
        </>
      )}
      {editor.main?.lutMissing && (
        <p className="px-1 py-1 text-[11px] text-amber-400" data-testid="lut-missing">
          LUT not found in the library; rendered without it.
        </p>
      )}
      <div className="mt-2 border-t border-neutral-800 pt-2">
        <button className="flex h-7 w-full items-center justify-center gap-1 rounded bg-neutral-800 text-xs hover:bg-neutral-700 disabled:opacity-40" disabled={importing} onClick={onImport} data-testid="profile-import" title="Import a folder of Lightroom presets, profiles, DCPs and .cube LUTs">
          {importing ? <Loader2 className="size-3.5 animate-spin" /> : <FileUp className="size-3.5" />} Import Presets &amp; Profiles…
        </button>
      </div>
    </div>
  );
}
