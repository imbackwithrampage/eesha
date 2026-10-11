// The mascot between hook events (src/mascot/engine.ts). Upstream stopped its
// frame loop whenever the engine said it was not busy, so during a long tool
// call the mascot stood still: no dots, no blink (upstream pull request 403).
// Here the engine is busy for as long as it is on screen, which is what keeps
// the island's loop going (`Island.step`): checked on the engine, since the
// loop itself needs a page.

import { test } from "node:test";
import assert from "node:assert/strict";
import { BotEngine } from "../src/mascot/engine.ts";

for (const state of ["working", "thinking", "searching", "idle", "approval", "question"]) {
  test(`the mascot keeps moving and blinking for thirty seconds without an event (${state})`, () => {
    const bot = new BotEngine(state);
    let blinks = 0;
    const blink = bot.blink.bind(bot);
    bot.blink = () => {
      blinks += 1;
      blink();
    };
    const started = bot.t;
    for (let frame = 0; frame < 30 * 60; frame++) {
      // Asked every frame, as the island does: false would end the loop.
      assert.equal(bot.busy, true, `frame ${frame}`);
      bot.update(1 / 60);
    }
    // Its own clock ran (the dots and the scan are drawn from it), and it blinked.
    assert.ok(bot.t - started > 29.9, String(bot.t - started));
    assert.ok(blinks >= 4, String(blinks));
  });
}
