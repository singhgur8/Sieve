// One project on the home page: cover, name, path, counts, workflow step, last opened, and the per-project menu.
import { AlertTriangle, Aperture, FolderSearch, Image as ImageIcon, MoreHorizontal, Pencil, Star, Trash2, Wand2, Sliders } from "lucide-react";
import { convertFileSrc, type Project } from "../../ipc";
import { formatDate, relativeTime } from "../../lib/relativeTime";
import { cap, STEP_LABEL, STEP_STYLE } from "../../lib/shootTypes";
import { Menu, menuItem } from "../Menu";

interface Props {
  project: Project;
  onOpen: () => void;
  onRename: () => void;
  onCover: () => void;
  onAutoCover: () => void;
  onReveal: () => void;
  onLocate: (folderId: number) => void;
  onRemove: () => void;
}

export function ProjectCard({ project: p, onOpen, onRename, onCover, onAutoCover, onReveal, onLocate, onRemove }: Props) {
  const missing = p.folders.filter((f) => !f.exists);
  const folderLabel = p.folders.length > 1 ? `${p.folders[0].path} +${p.folders.length - 1} more` : (p.folders[0]?.path ?? "");
  const id = p.id;
  return (
    <article
      className="group relative flex flex-col overflow-hidden rounded-xl border border-neutral-800 bg-neutral-900 transition hover:border-neutral-600 hover:bg-neutral-900/80 focus-within:border-sky-600"
      data-testid={`project-card-${id}`}
      data-project-name={p.name}
    >
      <button onClick={onOpen} className="flex flex-1 flex-col text-left focus:outline-none" data-testid={`project-open-${id}`} aria-label={`Open ${p.name}`}>
        <div className="relative aspect-[3/2] w-full overflow-hidden bg-neutral-950">
          {p.coverThumbnailPath ? (
            <img src={convertFileSrc(p.coverThumbnailPath)} alt="" draggable={false} className="size-full object-cover transition group-hover:scale-[1.02]" data-testid={`project-cover-${id}`} />
          ) : (
            <div className="flex size-full flex-col items-center justify-center gap-1 text-neutral-600" data-testid={`project-nocover-${id}`}>
              <ImageIcon className="size-8" />
              <span className="text-xs">{p.photoCount === 0 ? "No photos yet" : "Preparing cover…"}</span>
            </div>
          )}
          <span className={`absolute left-2 top-2 rounded-full px-2 py-0.5 text-[11px] font-medium ring-1 ${STEP_STYLE[p.workflowStep]}`} data-testid={`project-step-${id}`} title="Workflow step">
            {STEP_LABEL[p.workflowStep]}
          </span>
          <span className="absolute bottom-2 left-2 rounded-full bg-black/60 px-2 py-0.5 text-[11px] text-neutral-200 backdrop-blur" data-testid={`project-shoot-${id}`} title="Shoot type">
            {cap(p.shootType)}
          </span>
        </div>
        <div className="flex flex-1 flex-col gap-1.5 p-3">
          <h2 className="truncate pr-7 text-base font-semibold text-neutral-100" data-testid={`project-name-${id}`} title={p.name}>
            {p.name}
          </h2>
          <p className="truncate text-xs text-neutral-400" title={p.folders.map((f) => f.path).join("\n")} data-testid={`project-path-${id}`}>
            {folderLabel}
          </p>
          <div className="mt-1 flex items-center gap-3 text-xs text-neutral-300" data-testid={`project-counts-${id}`}>
            <span className="flex items-center gap-1" title="Photos">
              <Aperture className="size-3.5 text-neutral-400" />
              <b className="font-semibold" data-testid={`project-photos-${id}`}>{p.photoCount}</b> photos
            </span>
            <span className="flex items-center gap-1" title="Keepers by the current keeper rule">
              <Star className="size-3.5 text-amber-500" />
              <b className="font-semibold" data-testid={`project-keepers-${id}`}>{p.keeperCount}</b> keepers
            </span>
            <span className="flex items-center gap-1" title="Photos with edits">
              <Sliders className="size-3.5 text-sky-500" />
              <b className="font-semibold" data-testid={`project-edited-${id}`}>{p.editedCount}</b> edited
            </span>
          </div>
          <p className="mt-auto pt-1 text-[11px] text-neutral-400" data-testid={`project-opened-${id}`} title={p.lastOpenedAtMs != null ? new Date(p.lastOpenedAtMs).toLocaleString() : undefined}>
            {p.lastOpenedAtMs != null ? `Opened ${relativeTime(p.lastOpenedAtMs)}` : "Never opened"} · Created {formatDate(p.createdAtMs)}
          </p>
        </div>
      </button>
      {missing.length > 0 && (
        <div className="flex items-center gap-2 border-t border-amber-900 bg-amber-950/70 px-3 py-1.5 text-xs text-amber-200" data-testid={`project-missing-${id}`}>
          <AlertTriangle className="size-3.5 shrink-0 text-amber-400" />
          <span className="min-w-0 flex-1 truncate">{missing.length === 1 ? "Folder not found on disk" : `${missing.length} folders not found`}</span>
          <button onClick={() => onLocate(missing[0].id)} className="shrink-0 rounded bg-amber-800 px-2 py-0.5 font-medium text-amber-50 hover:bg-amber-700" data-testid={`project-locate-${id}`}>
            Locate folder…
          </button>
        </div>
      )}
      <div className="absolute right-1.5 top-1.5">
        <Menu
          trigger={<MoreHorizontal className="size-4" />}
          triggerClass="flex size-7 items-center justify-center rounded-md bg-black/55 text-neutral-100 opacity-0 backdrop-blur hover:bg-black/80 focus:opacity-100 group-hover:opacity-100 aria-expanded:opacity-100"
          triggerTestId={`project-menu-${id}`}
          title="Project actions"
          align="right"
        >
          {(close) => {
            const item = (testid: string, label: React.ReactNode, fn: () => void, disabled = false) => (
              <button
                className={menuItem}
                disabled={disabled}
                data-testid={testid}
                onClick={() => {
                  close();
                  fn();
                }}
              >
                {label}
              </button>
            );
            return (
              <div className="w-56 py-1">
                {item(`project-menu-open-${id}`, <>Open</>, onOpen)}
                {item(`project-menu-rename-${id}`, <><Pencil className="size-4" /> Rename…</>, onRename)}
                {item(`project-menu-cover-${id}`, <><ImageIcon className="size-4" /> Choose cover…</>, onCover, p.photoCount === 0)}
                {p.coverChosen && item(`project-menu-autocover-${id}`, <><Wand2 className="size-4" /> Use automatic cover</>, onAutoCover)}
                {item(`project-menu-reveal-${id}`, <>Show in Finder</>, onReveal, p.folders.length === 0 || missing.length === p.folders.length)}
                {missing.length > 0 && item(`project-menu-locate-${id}`, <><FolderSearch className="size-4" /> Locate folder…</>, () => onLocate(missing[0].id))}
                <div className="my-1 border-t border-neutral-800" />
                <button
                  className={`${menuItem} text-red-300`}
                  data-testid={`project-menu-remove-${id}`}
                  onClick={() => {
                    close();
                    onRemove();
                  }}
                >
                  <Trash2 className="size-4" /> Remove from catalog…
                </button>
              </div>
            );
          }}
        </Menu>
      </div>
    </article>
  );
}
