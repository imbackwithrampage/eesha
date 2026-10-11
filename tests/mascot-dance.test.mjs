// The mascot's dance while music plays (stepDanceLevel and danceTransform in
// src/mascot/engine.ts). The island says when: on the strip, while the
// `audio-extras:beat` event reports a song (Island.dances).

import { test } from "node:test";
import assert from "node:assert/strict";
import { danceTransform, stepDanceLevel } from "../src/mascot/engine.ts";

test("the dance fades in over 0.3 s and out over 0.5 s", () => {
  assert.equal(stepDanceLevel(0, true, 0.15), 0.5);
  assert.equal(stepDanceLevel(0.9, true, 0.15), 1);
  assert.equal(stepDanceLevel(1, false, 0.25), 0.5);
  assert.equal(stepDanceLevel(0.1, false, 0.25), 0);
  assert.equal(stepDanceLevel(1, true, 0.05), 1);
});

test("112 BPM: still on the beat, highest half a beat later, nothing at level 0", () => {
  const R = 10;
  const onBeat = danceTransform(0, 1, R);
  assert.equal(onBeat.dx, 0);
  assert.equal(onBeat.dy, -0);
  assert.equal(onBeat.rotate, 0);
  assert.ok(Math.abs(onBeat.sx - 1.045) < 1e-9 && Math.abs(onBeat.sy - 0.94) < 1e-9, "squashed on landing");
  const top = danceTransform(0.5 * 60 / 112, 1, R);
  assert.ok(Math.abs(top.dy + 2) < 1e-9, "hops 0.2 R");
  assert.ok(Math.abs(top.dx - 0.8) < 1e-9 && Math.abs(top.rotate - 0.1) < 1e-9);
  assert.ok(Math.abs(top.sx - 1) < 1e-9 && Math.abs(top.sy - 1) < 1e-9);
  const off = danceTransform(0.3, 0, R);
  assert.deepEqual([off.dx, off.dy, off.rotate, off.sx, off.sy].map((v) => Math.abs(v)), [0, 0, 0, 1, 1]);
});
