// Adapters for Develop features whose backend arrives with IPC v14. The UI is built against these shapes now;
// each hook reports `supported: false` until it is wired to the real commands (see docs/ux-spec-8b.md 5.3, 7, 8).
// TO WIRE (v14):
//   useAutoAdjust        -> commands.autoTone(imageId, adjustments) / commands.autoWhiteBalance(imageId, adjustments)
//   usePresetGroups      -> commands.listPresetLibrary(), importPresetFolder(path), removePresetGroup(id), resolvePreset(imageId, presetId)
//   useSnapshots         -> list/create/update/delete snapshot commands (not in the v14 list yet)
import { useMemo } from "react";
import type { CompleteAdjustments, Preset } from "../ipc";

export interface AutoAdjust {
  supported: boolean;
  /** Returns the auto tone values to commit as ONE history entry "Auto Tone" (v14: exposure, contrast, highlights, shadows, whites, blacks, vibrance, saturation). */
  autoTone?: (imageId: number, adj: CompleteAdjustments) => Promise<Partial<CompleteAdjustments>>;
  /** Returns the auto white balance (v14). */
  autoWhiteBalance?: (imageId: number, adj: CompleteAdjustments) => Promise<{ temperatureK: number; tint: number }>;
}

export function useAutoAdjust(): AutoAdjust {
  return useMemo(() => ({ supported: false }), []);
}

export interface PresetGroupView {
  id: string;
  name: string;
  kind: "user" | "imported";
  presets: Preset[];
}

export interface PresetGroups {
  groups: PresetGroupView[];
  /** `importPresetFolder` available. */
  canImport: boolean;
  importFolder?: () => void;
}

/** v13: one "User Presets" group built from `list_presets`. v14: `listPresetLibrary()` groups + imports. */
export function usePresetGroups(userPresets: Preset[]): PresetGroups {
  return useMemo(() => ({ groups: [{ id: "user", name: "User Presets", kind: "user" as const, presets: userPresets }], canImport: false }), [userPresets]);
}

export interface SnapshotView {
  id: string;
  name: string;
}

export interface Snapshots {
  supported: boolean;
  items: SnapshotView[];
}

export function useSnapshots(_imageId: number | null): Snapshots {
  return useMemo(() => ({ supported: false, items: [] }), []);
}
