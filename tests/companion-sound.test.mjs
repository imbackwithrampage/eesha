import test from "node:test";
import assert from "node:assert/strict";
import { companionSound, COMPANION_SOUND } from "../src/mascot/reactions.ts";
import { SOUND_NAMES } from "../src/core/sound.ts";

const s = { on: true, master: true, visible: true, now: 10_000, last: 0 };

test("every companion sound is a real sound", () => {
  for (const n of Object.values(COMPANION_SOUND)) assert.ok(SOUND_NAMES.includes(n), n);
});
test("plays when everything allows it", () => {
  assert.equal(companionSound("agent", s), "finish");
  assert.equal(companionSound("land", s), "open");
});
test("held back by the companion switch, the master switch and a hidden window", () => {
  for (const off of [{ on: false }, { master: false }, { visible: false }]) {
    assert.equal(companionSound("agent", { ...s, ...off }), null);
  }
});
test("gestures keep a gap, reactions do not", () => {
  assert.equal(companionSound("pat", { ...s, last: s.now - 100 }), null);
  assert.equal(companionSound("pat", { ...s, last: s.now - 500 }), "blip");
  assert.equal(companionSound("timer", { ...s, last: s.now - 100 }), "approval");
});
