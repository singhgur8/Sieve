// Profile browser: base camera profile (DCP) and Adobe Look with an Amount slider, from `list_profiles`.
import { useEffect, useMemo, useState } from "react";
import { ChevronDown, ChevronRight, LayoutGrid } from "lucide-react";
import { commands, unwrap, type CameraProfileInfo, type LookProfileInfo, type ProfileCatalog } from "../../ipc";
import type { Editor } from "../../hooks/useEditor";
import { Slider } from "./Slider";

const item = (on: boolean) => `flex w-full items-center justify-between rounded px-2 py-1 text-left text-xs disabled:opacity-40 ${on ? "bg-sky-800 text-sky-100" : "hover:bg-neutral-800"}`;

function Group({ id, title, children }: { id: string; title: string; children: React.ReactNode }) {
  const [open, setOpen] = useState(true);
  return (
    <div data-testid={`profile-group-${id}`}>
      <button className="flex w-full items-center gap-1 px-1 py-0.5 text-[11px] font-semibold uppercase tracking-wide text-neutral-400" onClick={() => setOpen(!open)} aria-expanded={open}>
        {open ? <ChevronDown className="size-3" /> : <ChevronRight className="size-3" />}
        {title}
      </button>
      {open && children}
    </div>
  );
}

export function ProfilePanel({ editor, imageId, onError }: { editor: Editor; imageId: number | null; onError: (e: unknown) => void }) {
  const [catalog, setCatalog] = useState<ProfileCatalog | null>(null);
  const [browsing, setBrowsing] = useState(false);
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
  }, [imageId, onError]);

  const { cameraProfile, look } = editor.adj.profile;
  const lookInfo = catalog?.looks.find((l) => l.uuid === look?.uuid);
  // Camera profiles and looks that share a group name (e.g. "Adobe Raw") are listed together.
  const groups = useMemo(() => {
    const m = new Map<string, { cams: CameraProfileInfo[]; looks: LookProfileInfo[] }>();
    const slot = (g: string) => m.get(g || "Other") ?? (m.set(g || "Other", { cams: [], looks: [] }), m.get(g || "Other")!);
    catalog?.cameraProfiles.forEach((p) => slot(p.group).cams.push(p));
    catalog?.looks.forEach((l) => slot(l.group).looks.push(l));
    return [...m.entries()];
  }, [catalog]);
  const current = look?.name ?? cameraProfile ?? "None";

  const pickCamera = (name: string | null) => editor.change((a) => ({ ...a, profile: { cameraProfile: name, look: null } }), `Profile: ${name ?? "None"}`);
  const pickLook = (l: LookProfileInfo) =>
    editor.change((a) => ({ ...a, profile: { cameraProfile: l.cameraProfile ?? a.profile.cameraProfile, look: { name: l.name, uuid: l.uuid, amount: 1 } } }), `Profile: ${l.name}`);

  return (
    <div data-testid="profile-panel">
      <div className="flex gap-1">
        <div className="min-w-0 flex-1 truncate rounded bg-neutral-800 px-2 py-1 text-xs" data-testid="profile-current" title={cameraProfile ? `${current} (${cameraProfile})` : current} data-camera={cameraProfile ?? ""} data-look={look?.uuid ?? ""}>
          {current}
        </div>
        <button className={`flex items-center gap-1 rounded px-2 py-1 text-xs ${browsing ? "bg-sky-800 text-sky-100" : "bg-neutral-800 hover:bg-neutral-700"}`} onClick={() => setBrowsing(!browsing)} aria-expanded={browsing} title="Browse profiles" data-testid="profile-browse">
          <LayoutGrid className="size-3.5" /> Browse
        </button>
      </div>
      {look && lookInfo?.supportsAmount && (
        <div className="mt-2">
          <Slider
            id="look-amount"
            label="Amount"
            value={Math.round(look.amount * 100)}
            min={0}
            max={200}
            step={1}
            display={(v) => `${v}%`}
            onInput={(v) => editor.edit((a) => ({ ...a, profile: { ...a.profile, look: a.profile.look ? { ...a.profile.look, amount: v / 100 } : null } }), "Profile: Amount")}
            onCommit={editor.commit}
            onReset={() => editor.change((a) => ({ ...a, profile: { ...a.profile, look: a.profile.look ? { ...a.profile.look, amount: 1 } : null } }), "Profile: Amount")}
          />
        </div>
      )}
      {browsing && (
        <div className="mt-2 max-h-72 overflow-y-auto rounded border border-neutral-800 bg-neutral-900 p-1" data-testid="profile-browser">
          {!catalog && <p className="p-2 text-xs text-neutral-400">Loading profiles...</p>}
          {catalog && catalog.cameraProfiles.length === 0 && catalog.looks.length === 0 && (
            <p className="p-2 text-[11px] text-amber-400" data-testid="profile-empty">
              No Adobe profiles found. Install Adobe DNG Converter or Camera Raw ({catalog.searchDirs[0] ?? "CameraProfiles"}).
            </p>
          )}
          {catalog && (
            <>
              <button className={item(cameraProfile === null && !look)} onClick={() => pickCamera(null)} data-testid="profile-item-none">
                <span>None</span>
                <span className="text-[10px] text-neutral-400">Sieve base</span>
              </button>
              {groups.map(([g, { cams, looks }]) => (
                <Group key={g} id={g} title={g}>
                  {cams.map((p) => (
                    <button key={p.name} className={item(cameraProfile === p.name && !look)} onClick={() => pickCamera(p.name)} data-testid={`profile-item-${p.name}`}>
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
                    >
                      <span className="truncate">{l.name}</span>
                      {!l.available ? <span className="text-[10px] text-amber-500">not installed</span> : l.monochrome ? <span className="text-[10px] text-neutral-400">B&amp;W</span> : null}
                    </button>
                  ))}
                </Group>
              ))}
            </>
          )}
        </div>
      )}
    </div>
  );
}
