// The voice assistant's three hooks on the island (docs/VOICE.md §7.5, §7.6):
// `IslandApi.ear`, `shape` and `face`. The rules only, no DOM and no Tauri:
// island.ts draws what they decide. Cases in tests/voice-seams.test.mjs.

import type { IslandShape, VoiceFace } from "../core/feature";
import { ACTIVITY_W, NOTCH_W, NOTICE_W, type BotEmoteName, type BotStateName } from "../core/layout";

/** The one feature the three hooks belong to. */
export const VOICE = "voice";
/** How long the ear bar stays bright after a voice was heard. */
export const EAR_HEARD_MS = 300;

/** Only the voice feature, and only while it runs: for anyone else the three calls do nothing. */
export function voiceMay(feature: string, on: (id: string) => boolean): boolean {
  return feature === VOICE && on(VOICE);
}

/** The ear bar: out, lit (the microphone is open) or bright (a voice is heard). */
export type EarLook = "off" | "on" | "heard";

export function earLook(on: boolean, heard = false): EarLook {
  return !on ? "off" : heard ? "heard" : "on";
}

/**
 * The strip's width for an activity of `feature`: 340 as for every activity, unless it is the voice
 * row and it asked for its own, kept between the wake strip's width and a notice's.
 */
export function stripWidth(feature: string, asked?: number): number {
  if (feature !== VOICE || asked == null || !Number.isFinite(asked)) return ACTIVITY_W;
  return Math.max(NOTCH_W, Math.min(NOTICE_W, asked));
}

export function sameShape(a: IslandShape, b: IslandShape): boolean {
  return a.mode === b.mode && a.width === b.width && a.height === b.height && a.top === b.top;
}

/** What the island's mascot is handed for a voice face. */
export interface MascotFace {
  /** A face the engine already draws, held over its state. */
  held: BotStateName | null;
  /** One of the two new faces with its level 0…1, for the engine that draws them (`BotEngine.voice`). */
  voice: { face: "listening" | "talking"; level: number } | null;
  /** Where the bar looks instead of at the pointer (§7.6: thinking looks up and right). */
  look: { x: number; y: number } | null;
}

const OWN: MascotFace = { held: null, voice: null, look: null };

/** `wink` and `surprised` are played once: they are emotes, not a face that is kept. */
export function faceEmote(face: VoiceFace | null): BotEmoteName | null {
  return face === "wink" || face === "surprised" ? face : null;
}

/**
 * The face the mascot wears for what voice asked. A card that waits for an answer keeps its own
 * (approval, question), whatever voice is doing.
 */
export function mascotFace(face: VoiceFace | null, level: number, cardWaits: boolean): MascotFace {
  if (cardWaits) return OWN;
  switch (face) {
    case "listening":
    case "talking":
      return { held: null, voice: { face, level: Number.isFinite(level) ? Math.max(0, Math.min(1, level)) : 0 }, look: null };
    case "thinking":
      return { held: "thinking", voice: null, look: { x: 0.4, y: 0.5 } };
    case "asking":
      return { held: "question", voice: null, look: null };
    default:
      return OWN;
  }
}
