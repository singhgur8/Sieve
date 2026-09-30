// Top-level router: the Projects home page or one open project (the Library / Develop workspace in App).
// The app always starts on the home page; the open project is never persisted.
import { useCallback, useState } from "react";
import { commands, unwrap, type Project } from "./ipc";
import App from "./App";
import { HomePage } from "./components/home/HomePage";

type View = { kind: "home" } | { kind: "project"; project: Project } | { kind: "all" };

/** Dev-only mock: `?scope=all` opens the workspace over every photo (no project), for suites that predate projects. */
function initialView(): View {
  if (import.meta.env.DEV) {
    const q = new URLSearchParams(window.location.search);
    if (q.has("mock") && q.get("scope") === "all") return { kind: "all" };
  }
  return { kind: "home" };
}

export default function Root() {
  const [view, setView] = useState<View>(initialView);

  const open = useCallback(async (id: number) => {
    const project = await unwrap(commands.openProject(id));
    setView({ kind: "project", project });
  }, []);
  const home = useCallback(() => setView({ kind: "home" }), []);

  if (view.kind === "home") return <HomePage onOpen={open} />;
  return <App key={view.kind === "project" ? view.project.id : "all"} project={view.kind === "project" ? view.project : null} onHome={home} onOpenProject={open} />;
}
