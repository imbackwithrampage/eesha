// Pats: a click on the companion. Three inside 1.2 s make it dizzy, and while it spins a click
// is nothing. Apart from index.ts so that it runs without a page (tests/companion-pats.test.mjs).

import type { BotEngine } from "../../mascot/engine";

/** How long it stays dizzy: nearly two turns of the bar. */
export const DIZZY_MS = 1500;
const WITHIN = 1200;

/** Also the name of the sound that goes with it. */
export type Pat = "pat" | "dizzy";

/** Says what the click at `now` (ms) is: a pat, the third one that makes it dizzy, or null while it is dizzy. */
export function patCounter() {
  let clicks: number[] = [];
  let until = -Infinity;
  return (now: number): Pat | null => {
    if (now < until) return null;
    clicks = [...clicks.filter((t) => now - t < WITHIN), now];
    if (clicks.length < 3) return "pat";
    clicks = [];
    until = now + DIZZY_MS;
    return "dizzy";
  };
}

/**
 * The look that goes with it. Dizzy is held as the engine's face, not set as its state: an emote
 * outranks the state, so the happy emote of the pat before hid the spinning bar until the spell
 * was nearly over. Whoever holds it clears it (`face = null`) after DIZZY_MS.
 */
export function showPat(e: BotEngine, what: Pat) {
  if (what === "dizzy") e.face = "dizzy";
  else e.triggerEmote("happy");
}
