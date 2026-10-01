// TopBar project switcher: current project name; menu jumps to another project or back to the home page.
import { useEffect, useState } from "react";
import { Check, ChevronDown, Home, Image as ImageIcon } from "lucide-react";
import { commands, unwrap, type Project } from "../ipc";
import { Menu, menuItem } from "./Menu";

interface Props {
  project: Project;
  onHome: () => void;
  onOpenProject: (id: number) => Promise<void>;
  /** Makes the active photo the project's cover (null = no active photo). */
  onSetCover: (() => void) | null;
}

function Body({ project, close, onHome, onOpenProject, onSetCover }: Props & { close: () => void }) {
  const [others, setOthers] = useState<Project[] | null>(null);
  useEffect(() => {
    let stale = false;
    unwrap(commands.listProjects())
      .then((l) => !stale && setOthers(l))
      .catch(() => !stale && setOthers([]));
    return () => {
      stale = true;
    };
  }, []);
  return (
    <div className="w-72 py-1" data-testid="project-switcher-menu">
      <button
        className={menuItem}
        data-testid="switcher-home"
        onClick={() => {
          close();
          onHome();
        }}
      >
        <Home className="size-4" /> All projects
      </button>
      <div className="mt-1 border-t border-neutral-800 px-3 pb-1 pt-2 text-[11px] font-medium uppercase tracking-wide text-neutral-400">Switch to</div>
      <div className="max-h-72 overflow-y-auto">
        {others === null && <p className="px-3 py-1.5 text-xs text-neutral-400">Loading…</p>}
        {others?.map((p) => (
          <button
            key={p.id}
            className={menuItem}
            data-testid={`switcher-project-${p.id}`}
            onClick={() => {
              close();
              if (p.id !== project.id) void onOpenProject(p.id);
            }}
          >
            <span className="flex size-4 shrink-0 items-center justify-center">{p.id === project.id && <Check className="size-4 text-sky-400" />}</span>
            <span className="min-w-0 flex-1 truncate">{p.name}</span>
            <span className="shrink-0 text-xs text-neutral-400">{p.photoCount}</span>
          </button>
        ))}
      </div>
      {onSetCover && (
        <button
          className={`${menuItem} mt-1 border-t border-neutral-800`}
          data-testid="switcher-set-cover"
          onClick={() => {
            close();
            onSetCover();
          }}
        >
          <ImageIcon className="size-4" /> Use current photo as cover
        </button>
      )}
    </div>
  );
}

export function ProjectSwitcher(p: Props) {
  return (
    <Menu
      trigger={
        <>
          <span className="max-w-44 truncate" data-testid="project-name">
            {p.project.name}
          </span>
          <ChevronDown className="size-4 shrink-0" />
        </>
      }
      triggerClass="flex h-7 items-center gap-1.5 rounded-md bg-neutral-800 px-2.5 text-sm font-medium hover:bg-neutral-700"
      triggerTestId="project-switcher"
      title="Switch project or go back to all projects"
    >
      {(close) => <Body {...p} close={close} />}
    </Menu>
  );
}
