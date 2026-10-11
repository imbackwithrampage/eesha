// The live registry (docs/VOICE.md §5): every action of the features that are on, read through
// `voiceActions()` of src/core/registry.ts when voice starts and again when a feature is switched.
// (That file cannot be loaded under `node --test`: whatever is tested takes the entries as an argument.)
// Package voice-runner (VOICE §10).

import type { VoiceCx, VoiceEntry } from "../../core/feature";
import { featureCtx, onFeaturesChanged, voiceActions } from "../../core/registry";
import { voiceCx, type Pass } from "./runner";

let list: readonly VoiceEntry[] = [];
const watchers = new Set<(entries: readonly VoiceEntry[]) => void>();
const cxs = new Map<string, VoiceCx>();
/** Counts the reads: one that ends after a newer one began, or after the stop, is thrown away. */
let reads = 0;

/** Follows `features-changed` from the feature's `start` to the dispose it returns. */
export function startRegistry(): () => void {
  const off = onFeaturesChanged(() => void loadRegistry());
  void loadRegistry();
  return () => {
    off();
    reads++;
    list = [];
    cxs.clear();
  };
}

/** Reads the actions again and tells the watchers. */
export async function loadRegistry(): Promise<readonly VoiceEntry[]> {
  const mine = ++reads;
  const read = await voiceActions();
  if (mine !== reads) return list;
  list = read;
  for (const fn of [...watchers]) fn(list);
  return list;
}

/** The actions as last read. */
export function entries(): readonly VoiceEntry[] {
  return list;
}

/** By `VoiceAction.tool`: what the action model and Claude name. */
export function byTool(tool: string): VoiceEntry | undefined {
  return list.find((entry) => entry.action.tool === tool);
}

/** `fn` is called after each read; the returned function ends it. */
export function onRegistry(fn: (entries: readonly VoiceEntry[]) => void): () => void {
  watchers.add(fn);
  return () => void watchers.delete(fn);
}

/**
 * The `VoiceCx` of an action's own feature (`voiceCx` of runner.ts over `featureCtx(entry.feature)`).
 * With `pass` (the runner's, for one run the user said yes to) a new one that carries it.
 */
export function cxOf(entry: VoiceEntry, pass?: Pass): VoiceCx {
  if (pass) return voiceCx(featureCtx(entry.feature), pass);
  let v = cxs.get(entry.feature);
  if (!v) cxs.set(entry.feature, (v = voiceCx(featureCtx(entry.feature))));
  return v;
}
