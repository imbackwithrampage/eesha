// The fixed order of the live activities (src/island/arbiter.ts, §5.2), and
// that the features which have a name there use the number it stands for.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { ORDER, pickActivities } from "../src/island/arbiter.ts";

const active = (order) => ({ order, isActive: () => true });
const source = (feature) => readFileSync(new URL(`../src/features/${feature}/index.ts`, import.meta.url), "utf8");

test("a recording owns the strip before a timer, the agents and music, and nothing rides beside it", () => {
  const { primary, companions } = pickActivities([active(ORDER.music), active(ORDER.agents), active(ORDER.timer), active(ORDER.recording)]);
  assert.equal(primary.order, ORDER.recording);
  assert.deepEqual(companions, []);
  // A break alone comes before it.
  assert.equal(pickActivities([active(ORDER.recording), active(-1)]).primary.order, -1);
});

test("a microphone in use comes before the agents, whose dot rides beside it, and before the calendar and music", () => {
  assert.ok(ORDER.micInUse < ORDER.agents && ORDER.agents < ORDER.calendar && ORDER.calendar < ORDER.music);
  assert.equal(pickActivities([active(ORDER.music), active(ORDER.micInUse)]).primary.order, ORDER.micInUse);
  const { primary, companions } = pickActivities([active(ORDER.agents), active(ORDER.micInUse)]);
  assert.equal(primary.order, ORDER.micInUse);
  assert.deepEqual(companions.map((c) => c.order), [ORDER.agents]);
});

test("the recorder and the microphone dot are wired to those places", () => {
  // The recorder's package is its own: it writes the number, which must be the arbiter's.
  assert.equal(Number(/id: "recording",[^}]*?order: (-?[\d.]+)/s.exec(source("recorder"))[1]), ORDER.recording);
  assert.match(source("huds-power"), /id: "mic-use",\s*order: ORDER\.micInUse,/);
});
