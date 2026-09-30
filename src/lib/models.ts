// AI model download state (IPC v12), shared by the Masks panel card and the "Manage AI models" dialog.
// A module-level store so progress survives the panels unmounting; events are subscribed once.
import { useEffect, useSyncExternalStore } from "react";
import { commands, events, MODEL_GROUP_SEGMENTATION, unwrap, type ModelDownloadProgress, type ModelDownloadStatus } from "../ipc";
import { invalidateMaskCapabilities } from "../hooks/useMasks";
import { formatError } from "./format";

export interface ModelsState {
  /** null until the first status call resolves. */
  status: ModelDownloadStatus | null;
  downloading: boolean;
  cancelling: boolean;
  progress: ModelDownloadProgress | null;
  /** Last failure (not set for a user cancel). */
  error: string | null;
  /** Bumped on every successful install; mask capabilities are refetched by listeners. */
  installs: number;
}

let state: ModelsState = { status: null, downloading: false, cancelling: false, progress: null, error: null, installs: 0 };
const listeners = new Set<() => void>();
let started = false;

function set(patch: Partial<ModelsState>) {
  state = { ...state, ...patch };
  listeners.forEach((l) => l());
}

export async function refreshModelStatus() {
  try {
    const status = await unwrap(commands.modelDownloadsStatus());
    set({ status, downloading: status.downloading != null || state.downloading });
    if (status.downloading == null && state.downloading && !state.progress) set({ downloading: false });
  } catch (e) {
    set({ error: formatError(e) });
  }
}

function start() {
  if (started) return;
  started = true;
  void refreshModelStatus();
  void events.modelDownloadProgress.listen((ev) => set({ progress: ev.payload, downloading: true }));
  void events.modelDownloadFinished.listen((ev) => {
    const f = ev.payload;
    set({ downloading: false, cancelling: false, progress: f.ok ? null : state.progress, error: f.ok || f.cancelled ? null : (f.error ?? "The download failed"), installs: state.installs + (f.ok ? 1 : 0) });
    void refreshModelStatus();
    if (f.ok) invalidateMaskCapabilities();
  });
}

export async function downloadModels() {
  set({ error: null, downloading: true, cancelling: false, progress: null });
  try {
    await unwrap(commands.downloadModels(MODEL_GROUP_SEGMENTATION));
  } catch (e) {
    set({ downloading: false, error: formatError(e) });
  }
}

export async function cancelModelDownload() {
  set({ cancelling: true });
  try {
    await unwrap(commands.cancelModelDownload());
  } catch (e) {
    set({ cancelling: false, error: formatError(e) });
  }
}

export function useModels(): ModelsState {
  useEffect(start, []);
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => state,
  );
}

export const segmentation = (s: ModelDownloadStatus | null) => s?.groups.find((g) => g.id === MODEL_GROUP_SEGMENTATION) ?? null;

export const mb = (bytes: number) => Math.round(bytes / 1_000_000);
