// Projects home page: the app always starts here. Cards for every project in the catalog, search, sort, New project.
import { useCallback, useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Aperture, FolderPlus, Search, X } from "lucide-react";
import { commands, unwrap, type Project, type ShootType } from "../../ipc";
import { useImportOptions } from "../../lib/importOptions";
import { describeError, noteFailure, clearFileHealth } from "../../lib/errors";
import { Toasts, useToasts } from "../Toasts";
import { CoverDialog, NewProjectDialog, RemoveDialog, RenameDialog } from "./ProjectDialogs";
import { ProjectCard } from "./ProjectCard";

type SortKey = "opened" | "name" | "created";
const SORT_KEY = "sieve.home.sort";
const SORTS: { key: SortKey; label: string }[] = [
  { key: "opened", label: "Last opened" },
  { key: "name", label: "Name" },
  { key: "created", label: "Date created" },
];

function loadSort(): SortKey {
  const v = localStorage.getItem(SORT_KEY);
  return v === "name" || v === "created" || v === "opened" ? v : "opened";
}

export function sortProjects(list: Project[], key: SortKey): Project[] {
  const out = [...list];
  if (key === "name") out.sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base", numeric: true }) || a.id - b.id);
  else if (key === "created") out.sort((a, b) => b.createdAtMs - a.createdAtMs || b.id - a.id);
  else out.sort((a, b) => (b.lastOpenedAtMs ?? -1) - (a.lastOpenedAtMs ?? -1) || b.createdAtMs - a.createdAtMs || b.id - a.id);
  return out;
}

type Dialog = { kind: "new" } | { kind: "rename" | "remove" | "cover"; project: Project };

export function HomePage({ onOpen }: { onOpen: (id: number) => Promise<void> }) {
  const [projects, setProjects] = useState<Project[] | null>(null);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SortKey>(loadSort);
  const [dialog, setDialog] = useState<Dialog | null>(null);
  const [shoot, setShoot] = useState<ShootType>("wedding");
  const [importOpts] = useImportOptions();
  const toasts = useToasts();
  const { push } = toasts;
  const [error, setError] = useState<ReturnType<typeof describeError> | null>(null);

  const fail = useCallback((e: unknown) => {
    const info = describeError(e);
    noteFailure(info.message);
    setError(info);
  }, []);

  const refresh = useCallback(async () => {
    try {
      setProjects(await unwrap(commands.listProjects()));
    } catch (e) {
      setProjects((p) => p ?? []);
      fail(e);
    }
  }, [fail]);

  useEffect(() => {
    void refresh();
    unwrap(commands.getCatalogState())
      .then((c) => setShoot(c.shootType))
      .catch(() => {});
  }, [refresh]);

  const openProject = useCallback(
    async (id: number) => {
      try {
        await onOpen(id);
      } catch (e) {
        fail(e);
        void refresh();
      }
    },
    [onOpen, fail, refresh],
  );

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    const list = (projects ?? []).filter((p) => q === "" || p.name.toLowerCase().includes(q) || p.folders.some((f) => f.path.toLowerCase().includes(q)));
    return sortProjects(list, sort);
  }, [projects, query, sort]);

  const act = async (fn: () => Promise<unknown>) => {
    try {
      await fn();
      await refresh();
    } catch (e) {
      fail(e);
    }
  };

  const locate = (p: Project, folderId: number) =>
    void act(async () => {
      const path = await open({ directory: true, title: "Locate folder" });
      if (typeof path !== "string") return;
      const r = await unwrap(commands.relocateFolder(folderId, path));
      clearFileHealth();
      push(`Relinked ${r.matched} photo${r.matched === 1 ? "" : "s"}${r.stillMissing > 0 ? ` · ${r.stillMissing} still missing` : ""}`);
      void p;
    });

  const total = projects?.length ?? 0;
  return (
    <main className="flex h-screen flex-col" data-testid="home-page">
      <header className="flex h-14 shrink-0 items-center gap-3 border-b border-neutral-800 px-6">
        <Aperture className="size-5 shrink-0 text-amber-400" />
        <h1 className="mr-2 font-semibold tracking-tight">Sieve</h1>
        <span className="text-neutral-600">/</span>
        <h2 className="text-sm font-medium text-neutral-200">Projects</h2>
        {total > 0 && (
          <span className="rounded-full bg-neutral-800 px-2 py-0.5 text-xs text-neutral-400" data-testid="home-count">
            {total}
          </span>
        )}
        <div className="mx-auto" />
        {total > 0 && (
          <>
            <label className="relative">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-neutral-500" />
              <input
                type="search"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Search projects"
                aria-label="Search projects"
                className="h-8 w-64 rounded-md border border-neutral-700 bg-neutral-900 pl-8 pr-7 text-sm placeholder:text-neutral-500 focus:border-sky-600 focus:outline-none [&::-webkit-search-cancel-button]:hidden"
                data-testid="home-search"
              />
              {query && (
                <button onClick={() => setQuery("")} aria-label="Clear search" className="absolute right-1.5 top-1/2 -translate-y-1/2 rounded p-0.5 text-neutral-400 hover:text-neutral-100" data-testid="home-search-clear">
                  <X className="size-3.5" />
                </button>
              )}
            </label>
            <label className="flex items-center gap-1.5 text-xs text-neutral-400">
              Sort
              <select
                value={sort}
                onChange={(e) => {
                  const v = e.target.value as SortKey;
                  setSort(v);
                  localStorage.setItem(SORT_KEY, v);
                }}
                className="h-8 rounded-md border border-neutral-700 bg-neutral-900 px-2 text-sm text-neutral-200"
                data-testid="home-sort"
              >
                {SORTS.map((s) => (
                  <option key={s.key} value={s.key}>
                    {s.label}
                  </option>
                ))}
              </select>
            </label>
          </>
        )}
        <button
          onClick={() => setDialog({ kind: "new" })}
          className="flex h-8 items-center gap-1.5 rounded-md bg-sky-700 px-3 text-sm font-medium text-white hover:bg-sky-600"
          data-testid="new-project"
        >
          <FolderPlus className="size-4" /> New project
        </button>
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto" data-testid="home-scroll">
        {projects === null ? (
          <p className="p-10 text-center text-sm text-neutral-500" data-testid="home-loading">
            Loading projects…
          </p>
        ) : total === 0 ? (
          <div className="flex h-full flex-col items-center justify-center gap-3 px-6 text-center" data-testid="home-empty">
            <div className="flex size-16 items-center justify-center rounded-2xl bg-neutral-900 ring-1 ring-neutral-800">
              <FolderPlus className="size-8 text-neutral-400" />
            </div>
            <h2 className="text-lg font-semibold text-neutral-100">Create your first project</h2>
            <p className="max-w-md text-sm text-neutral-400">
              One project per shoot. Pick the folder with your photos; Sieve reads them in place, culls them automatically and writes ratings to XMP sidecars next to the originals.
            </p>
            <button onClick={() => setDialog({ kind: "new" })} className="mt-1 flex items-center gap-2 rounded-md bg-sky-700 px-4 py-2 text-sm font-medium text-white hover:bg-sky-600" data-testid="empty-new-project">
              <FolderPlus className="size-4" /> New project
            </button>
            <p className="text-xs text-neutral-500">Sony ARW, Fujifilm RAF, Canon CR3 · JPEG, HEIC, TIFF, PNG when enabled in Import options</p>
          </div>
        ) : visible.length === 0 ? (
          <div className="flex flex-col items-center gap-2 p-16 text-center text-sm text-neutral-400" data-testid="home-no-match">
            <p>No projects match “{query}”.</p>
            <button onClick={() => setQuery("")} className="rounded bg-neutral-800 px-3 py-1.5 text-neutral-100 hover:bg-neutral-700">
              Clear search
            </button>
          </div>
        ) : (
          <div className="mx-auto grid max-w-[1800px] grid-cols-[repeat(auto-fill,minmax(290px,1fr))] gap-5 p-6" data-testid="home-grid">
            {visible.map((p) => (
              <ProjectCard
                key={p.id}
                project={p}
                onOpen={() => void openProject(p.id)}
                onRename={() => setDialog({ kind: "rename", project: p })}
                onCover={() => setDialog({ kind: "cover", project: p })}
                onAutoCover={() => void act(() => unwrap(commands.setProjectCover(p.id, null)))}
                onReveal={() => {
                  const f = p.folders.find((x) => x.exists) ?? p.folders[0];
                  if (f) void act(() => unwrap(commands.revealInFinder(f.path)));
                }}
                onLocate={(folderId) => locate(p, folderId)}
                onRemove={() => setDialog({ kind: "remove", project: p })}
              />
            ))}
          </div>
        )}
      </div>

      {dialog?.kind === "new" && (
        <NewProjectDialog
          defaultShoot={shoot}
          importOptions={importOpts}
          onCancel={() => setDialog(null)}
          onCreated={(p, existing) => {
            setDialog(null);
            if (existing) push(`That folder is already in “${p.name}”. Opening it.`);
            void openProject(p.id);
          }}
        />
      )}
      {dialog?.kind === "rename" && (
        <RenameDialog
          project={dialog.project}
          onCancel={() => setDialog(null)}
          onRename={async (name) => {
            setDialog(null);
            await act(() => unwrap(commands.renameProject(dialog.project.id, name)));
          }}
        />
      )}
      {dialog?.kind === "cover" && (
        <CoverDialog
          project={dialog.project}
          onCancel={() => setDialog(null)}
          onPick={async (imageId) => {
            setDialog(null);
            await act(() => unwrap(commands.setProjectCover(dialog.project.id, imageId)));
          }}
        />
      )}
      {dialog?.kind === "remove" && (
        <RemoveDialog
          project={dialog.project}
          onCancel={() => setDialog(null)}
          onRemove={async () => {
            const p = dialog.project;
            setDialog(null);
            await act(async () => {
              const r = await unwrap(commands.removeProject(p.id));
              push(`Removed “${p.name}” from the catalog (${r.removedImages} photo${r.removedImages === 1 ? "" : "s"}). Files on disk were not touched.`);
            });
          }}
        />
      )}
      <Toasts api={toasts} error={error} onDismissError={() => setError(null)} />
    </main>
  );
}
