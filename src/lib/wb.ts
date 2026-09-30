// White balance picker backend call. Contract (architect): sampleWhiteBalance(id, point /* sensor frame */, adjustments)
// -> { temperatureK, tint }, averaging 5x5 source pixels. Until the generated bindings carry it, this calls the same
// command by name (guarded by `in commands`) so the build passes either way; the wrapper goes away once merged.
import { invoke } from "@tauri-apps/api/core";
import { commands, unwrap, type NormPoint, type ParametricAdjustments } from "../ipc";

export interface SampledWb {
  temperatureK: number;
  tint: number;
}

type SampleFn = (id: number, point: NormPoint, adjustments: ParametricAdjustments) => Promise<never>;

export async function sampleWhiteBalance(id: number, point: NormPoint, adjustments: ParametricAdjustments): Promise<SampledWb> {
  if ("sampleWhiteBalance" in commands) {
    const fn = (commands as unknown as Record<string, SampleFn>).sampleWhiteBalance;
    return (await unwrap(fn(id, point, adjustments))) as SampledWb;
  }
  return invoke<SampledWb>("sample_white_balance", { id, point, adjustments });
}
