// Three pats on the companion give the dizzy look and the dizzy sound, together and once
// (src/features/companion/pats.ts, played by the real engine).

import test from "node:test";
import assert from "node:assert/strict";
import { DIZZY_MS, patCounter, showPat } from "../src/features/companion/pats.ts";
import { COMPANION_SOUND } from "../src/mascot/reactions.ts";

const engine = async (reduced) => {
  globalThis.matchMedia = () => ({ matches: reduced });
  return new (await import(new URL(`../src/mascot/engine.ts?${reduced}`, import.meta.url).href)).BotEngine("idle");
};
/** Three pats 200 ms apart, with the frames between them; what each one was. */
const pats = (e, pat = patCounter()) =>
  [0, 200, 400].map((t) => {
    const what = pat(t);
    showPat(e, what);
    for (let i = 0; i < 12; i++) e.update(1 / 60);
    return what;
  });

test("the third pat inside 1.2 s is the dizzy one, and it has the dizzy sound", () => {
  const pat = patCounter();
  const third = [0, 200, 400].map(pat);
  assert.deepEqual(third, ["pat", "pat", "dizzy"]);
  assert.deepEqual(third.map((w) => COMPANION_SOUND[w]), ["blip", "blip", "dizzy"]);
});

test("once: a click while it spins is nothing, and the count starts again after", () => {
  const pat = patCounter();
  [0, 200, 400].map(pat);
  assert.deepEqual([500, 700, 900, 400 + DIZZY_MS - 1].map(pat), [null, null, null, null]);
  assert.deepEqual([0, 200, 400].map((t) => pat(400 + DIZZY_MS + t)), ["pat", "pat", "dizzy"]);
});

test("slow pats stay pats", () => {
  assert.deepEqual([0, 700, 1400, 2100].map(patCounter()), ["pat", "pat", "pat", "pat"]);
});

// `w` is the engine's own weight of each face (private to it in TypeScript): 1 is fully shown.
test("the dizzy look shows with the third pat, over the happy emote of the pat before", async () => {
  const e = await engine(false);
  assert.equal(pats(e)[2], "dizzy");
  assert.ok(e.w.dizzy > 0.85 && e.w.happy < 0.15, `dizzy ${e.w.dizzy}, happy ${e.w.happy}`);
  // What it did before: the state alone, which the emote of the second pat outranks for 1.4 s.
  const before = await engine(false);
  before.triggerEmote("happy");
  before.setState("dizzy");
  for (let i = 0; i < 12; i++) before.update(1 / 60);
  assert.ok(before.w.dizzy < 0.15, "the old way hid it");
});

test("it goes back to rest when the face is let go", async () => {
  const e = await engine(false);
  pats(e);
  e.face = null;
  for (let i = 0; i < 120; i++) e.update(1 / 60); // the last pat's emote runs out too
  assert.ok(e.w.dizzy < 0.05 && e.w.idle > 0.95);
});

test("motion reduced: the pose is there in the one frame the page draws", async () => {
  const e = await engine(true);
  const pat = patCounter();
  for (const t of [0, 200, 400]) showPat(e, pat(t));
  e.update(0.35); // what the page's `kick` does when nothing may move
  assert.ok(e.w.dizzy > 0.9);
});
