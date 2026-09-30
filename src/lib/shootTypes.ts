import type { ShootType, WorkflowStep } from "../ipc";

export const SHOOT_TYPES: ShootType[] = ["wedding", "portrait", "sports", "event", "landscape", "general"];
export const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

export const STEP_LABEL: Record<WorkflowStep, string> = { cull: "Cull", edit: "Edit", export: "Export" };
export const STEP_STYLE: Record<WorkflowStep, string> = {
  cull: "bg-amber-950 text-amber-200 ring-amber-800",
  edit: "bg-sky-950 text-sky-200 ring-sky-800",
  export: "bg-emerald-950 text-emerald-200 ring-emerald-800",
};
