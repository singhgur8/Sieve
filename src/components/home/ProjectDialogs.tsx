// Modal dialogs of the home page: new project, rename, remove (with the "files are never deleted" promise), choose cover.
import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpen } from "lucide-react";
import { commands, convertFileSrc, unwrap, type Project, type RawImageEntry, type ShootType } from "../../ipc";
import { cap, SHOOT_TYPES } from "../../lib/shootTypes";
import { describeError } from "../../lib/errors";
import { Dialog } from "../Dialog";
import { useImportOptions } from "../../lib/importOptions";

const panel = "w-[460px] max-w-full rounded-xl border border-neutral-700 bg-neutral-900 p-5 shadow-2xl";
const field = "h-9 w-full rounded-md border border-neutral-700 bg-neutral-950 px-2.5 text-sm text-neutral-100 focus:border-sky-600 focus:outline-none";
const primary = "h-8 rounded-md bg-sky-700 px-4 text-sm font-medium text-white hover:bg-sky-600 disabled:cursor-not-allowed disabled:opacity-50";
const secondary = "h-8 rounded-md bg-neutral-800 px-3 text-sm hover:bg-neutral-700";

const baseName = (p: string) => p.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || p;

export function NewProjectDialog({ defaultShoot, onCancel, onCreated }: { defaultShoot: ShootType; onCancel: () => void; onCreated: (p: Project, existing: boolean) => void }) {
  const [path, setPath] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [nameTouched, setNameTouched] = useState(false);
  const [shoot, setShoot] = useState<ShootType>(defaultShoot);
  const [importOptions, setImportOptions] = useImportOptions();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const pick = async () => {
    try {
      const p = await open({ directory: true, title: "Choose the shoot folder" });
      if (typeof p !== "string") return;
      setPath(p);
      if (!nameTouched) setName(baseName(p));
    } catch (e) {
      setError(describeError(e).message);
    }
  };
  const canCreate = path != null && !busy;
  const create = async () => {
    if (!path || busy) return;
    setBusy(true);
    setError(null);
    try {
      const r = await unwrap(commands.createProject(path, name.trim() === "" || name.trim() === baseName(path) ? null : name.trim(), shoot, importOptions));
      onCreated(r.project, r.existing);
    } catch (e) {
      setError(describeError(e).message);
      setBusy(false);
    }
  };
  return (
    <Dialog label="New project" testid="new-project-dialog" className={panel} onCancel={onCancel} onConfirm={() => void create()} canConfirm={() => canCreate}>
      <h2 className="text-lg font-semibold text-neutral-100">New project</h2>
      <p className="mt-1 text-sm text-neutral-400">A project is one shoot. Sieve reads the photos in place and never moves or deletes them.</p>
      <div className="mt-4 space-y-3">
        <div>
          <span className="mb-1 block text-xs font-medium text-neutral-400">Folder</span>
          <div className="flex gap-2">
            <div className="flex h-9 min-w-0 flex-1 items-center rounded-md border border-neutral-800 bg-neutral-950 px-2.5 text-sm" data-testid="new-project-path" title={path ?? undefined}>
              <span className={`truncate ${path ? "text-neutral-200" : "text-neutral-400"}`}>{path ?? "No folder chosen"}</span>
            </div>
            <button onClick={() => void pick()} className={`${secondary} flex h-9 shrink-0 items-center gap-1.5`} data-testid="new-project-pick" data-autofocus>
              <FolderOpen className="size-4" /> Choose…
            </button>
          </div>
        </div>
        <label className="block">
          <span className="mb-1 block text-xs font-medium text-neutral-400">Name</span>
          <input
            className={field}
            value={name}
            placeholder="Defaults to the folder name"
            onChange={(e) => {
              setName(e.target.value);
              setNameTouched(true);
            }}
            maxLength={200}
            data-testid="new-project-name"
          />
        </label>
        <label className="block">
          <span className="mb-1 block text-xs font-medium text-neutral-400">Shoot type (decides what culling looks for)</span>
          <select className={field} value={shoot} onChange={(e) => setShoot(e.target.value as ShootType)} data-testid="new-project-shoot">
            {SHOOT_TYPES.map((t) => (
              <option key={t} value={t}>
                {cap(t)}
              </option>
            ))}
          </select>
        </label>
        <div className="space-y-1.5">
          <label className="flex items-center gap-2 text-sm text-neutral-200">
            <input type="checkbox" checked={importOptions.includeNonRaw} onChange={(e) => setImportOptions({ includeNonRaw: e.target.checked })} data-testid="new-project-nonraw" />
            Include JPEG, HEIC, TIFF, PNG
          </label>
          <label className={`flex items-center gap-2 text-sm ${importOptions.includeNonRaw ? "text-neutral-200" : "text-neutral-400"}`}>
            <input type="checkbox" checked={importOptions.includeNonRaw && importOptions.pairJpegWithRaw} disabled={!importOptions.includeNonRaw} onChange={(e) => setImportOptions({ pairJpegWithRaw: e.target.checked })} data-testid="new-project-pair" />
            Pair a camera JPEG with its RAW
          </label>
        </div>
      </div>
      {error && (
        <p role="alert" className="mt-3 rounded-md border border-red-900 bg-red-950 px-3 py-2 text-xs text-red-200" data-testid="new-project-error">
          {error}
        </p>
      )}
      <div className="mt-5 flex justify-end gap-2">
        <button onClick={onCancel} className={secondary} data-testid="new-project-cancel">
          Cancel
        </button>
        <button onClick={() => void create()} disabled={!canCreate} className={primary} data-testid="new-project-create">
          {busy ? "Creating…" : "Create and open"}
        </button>
      </div>
    </Dialog>
  );
}

export function RenameDialog({ project, onCancel, onRename }: { project: Project; onCancel: () => void; onRename: (name: string) => Promise<void> }) {
  const [name, setName] = useState(project.name);
  const [busy, setBusy] = useState(false);
  const ok = name.trim().length > 0 && !busy;
  const save = async () => {
    if (!ok) return;
    setBusy(true);
    await onRename(name.trim());
  };
  return (
    <Dialog label="Rename project" testid="rename-dialog" className={panel} onCancel={onCancel} onConfirm={() => void save()} canConfirm={() => ok}>
      <h2 className="text-lg font-semibold text-neutral-100">Rename project</h2>
      <input
        className={`${field} mt-3`}
        value={name}
        maxLength={200}
        onChange={(e) => setName(e.target.value)}
        onFocus={(e) => e.currentTarget.select()}
        data-autofocus
        data-testid="rename-input"
        aria-label="Project name"
      />
      <p className="mt-2 text-xs text-neutral-400">Only the name in Sieve changes; the folder on disk keeps its name.</p>
      <div className="mt-4 flex justify-end gap-2">
        <button onClick={onCancel} className={secondary}>
          Cancel
        </button>
        <button onClick={() => void save()} disabled={!ok} className={primary} data-testid="rename-save">
          Rename
        </button>
      </div>
    </Dialog>
  );
}

export function RemoveDialog({ project, onCancel, onRemove }: { project: Project; onCancel: () => void; onRemove: () => Promise<void> }) {
  const [busy, setBusy] = useState(false);
  return (
    <Dialog label="Remove project from catalog" testid="remove-dialog" className={panel} onCancel={onCancel}>
      <h2 className="text-lg font-semibold text-neutral-100">Remove “{project.name}” from the catalog?</h2>
      <p className="mt-3 text-sm text-neutral-300" data-testid="remove-copy">
        Your photos, XMP sidecars and exports on disk are <b className="text-neutral-100">never deleted</b>. Sieve only forgets this project: its {project.photoCount} photo{project.photoCount === 1 ? "" : "s"}, cached previews and
        any edits not yet saved to XMP are removed from the catalog. You can import the folder again at any time.
      </p>
      <div className="mt-5 flex justify-end gap-2">
        <button onClick={onCancel} className={secondary} data-autofocus data-testid="remove-cancel">
          Cancel
        </button>
        <button
          onClick={() => {
            setBusy(true);
            void onRemove();
          }}
          disabled={busy}
          className="h-8 rounded-md bg-red-800 px-4 text-sm font-medium text-white hover:bg-red-700 disabled:opacity-50"
          data-testid="remove-confirm"
        >
          {busy ? "Removing…" : "Remove from catalog"}
        </button>
      </div>
    </Dialog>
  );
}

/** Pick the cover from the project's best-rated photos. */
export function CoverDialog({ project, onCancel, onPick }: { project: Project; onCancel: () => void; onPick: (imageId: number) => Promise<void> }) {
  const [rows, setRows] = useState<RawImageEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let stale = false;
    (async () => {
      const ids = await unwrap(
        commands.listImageIds({
          includeTags: [],
          excludeTags: [],
          tagMatch: "any",
          picks: [],
          minRating: null,
          maxRating: null,
          colorLabels: [],
          burstGroupId: null,
          sceneId: null,
          collapseBursts: false,
          folderId: null,
          projectId: project.id,
          sort: "rating",
          sortDescending: true,
          offset: 0,
          limit: 96,
        }),
      );
      const list = await unwrap(commands.getImages(ids));
      if (!stale) setRows(list.filter((r) => r.thumbnail.status === "ready"));
    })().catch((e) => !stale && setError(describeError(e).message));
    return () => {
      stale = true;
    };
  }, [project.id]);
  return (
    <Dialog label="Choose cover photo" testid="cover-dialog" className="flex max-h-[80vh] w-[720px] max-w-full flex-col rounded-xl border border-neutral-700 bg-neutral-900 p-5 shadow-2xl" onCancel={onCancel}>
      <h2 className="text-lg font-semibold text-neutral-100">Choose a cover for “{project.name}”</h2>
      <p className="mt-1 text-sm text-neutral-400">Best-rated photos first.</p>
      <div className="mt-3 min-h-40 flex-1 overflow-y-auto" data-testid="cover-grid">
        {error && <p className="text-sm text-red-300">{error}</p>}
        {!rows && !error && <p className="text-sm text-neutral-400">Loading…</p>}
        {rows && rows.length === 0 && <p className="text-sm text-neutral-400">No photos with a ready thumbnail yet.</p>}
        <div className="grid grid-cols-[repeat(auto-fill,minmax(110px,1fr))] gap-2">
          {rows?.map((r) =>
            r.thumbnail.status === "ready" ? (
              <button
                key={r.id}
                onClick={() => void onPick(r.id)}
                className={`aspect-[3/2] overflow-hidden rounded-md bg-neutral-950 ring-2 hover:ring-sky-500 focus:outline-none focus:ring-sky-500 ${project.coverImageId === r.id ? "ring-amber-500" : "ring-transparent"}`}
                title={r.fileName}
                data-testid={`cover-option-${r.id}`}
              >
                <img src={convertFileSrc(r.thumbnail.path)} alt={r.fileName} draggable={false} className="size-full object-cover" />
              </button>
            ) : null,
          )}
        </div>
      </div>
      <div className="mt-4 flex justify-end">
        <button onClick={onCancel} className={secondary}>
          Cancel
        </button>
      </div>
    </Dialog>
  );
}
