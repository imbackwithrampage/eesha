// The microphone-in-use row of src/features/huds-power: what the strip shows, for which app,
// when it clears, and what happens when the row cannot be drawn because something that comes
// first in the order (the agents at work, a timer) owns the strip.
//
// The detection itself is Rust's: `cargo run -p notch-feat-huds-power --example huds-power-mic-probe`.

import { test } from "node:test";
import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { installFakeDom } from "./fakedom.mjs";

// A feature's index.ts imports its stylesheet, which only the bundler understands.
registerHooks({
  resolve(specifier, context, nextResolve) {
    if (specifier.endsWith(".css")) return { url: "data:text/javascript,", shortCircuit: true };
    return nextResolve(specifier, context);
  },
});

installFakeDom();
const { default: feature } = await import("../src/features/huds-power/index.ts");
const mic = feature.activities.find((a) => a.id === "mic-use");

/** Starts the feature against a made-up island. `told` is what the island heard, in order. */
function start() {
  const told = [];
  const events = new Map();
  let settingsChanged = () => {};
  const stop = feature.start({
    id: "huds-power",
    t: (key, vars) => key.replace(/\{(\w+)\}/g, (_, name) => String(vars?.[name])),
    call: async () => null,
    on: (event, fn) => (events.set(event, fn), () => events.delete(event)),
    settings: { get: async () => ({}), patch: async () => {}, onChange: (fn) => ((settingsChanged = fn), () => {}) },
    island: {
      notice: (n) => told.push(["notice", n.kind, n.title, n.detail]),
      activityChanged: (id) => told.push(["changed", id]),
      redraw: (id) => told.push(["redraw", id]),
      fullscreen() {},
    },
  });
  return { told, stop, use: (apps) => events.get("micUse")({ apps }), settings: (s) => settingsChanged(s) };
}

test("an app takes the microphone: the row names it with the dot, and clears when it lets go", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const { told, stop, use } = start();
  assert.equal(mic.isActive(), false);

  use(["brave"]);
  assert.equal(mic.isActive(), true);
  assert.deepEqual(told, [["changed", "mic-use"]]);
  // The strip draws the row: it owns the strip, so no notice follows.
  const row = mic.render();
  assert.ok(row.querySelector(".f-huds-power-mic"), "the pulsing dot");
  assert.match(row.textContent, /^brave\s*Microphone in use$/);
  t.mock.timers.tick(1000);
  assert.deepEqual(told, [["changed", "mic-use"]]);

  // A second app: the row is drawn again where it stands, with a count.
  use(["Teams", "brave"]);
  assert.deepEqual(told.at(-1), ["redraw", "mic-use"]);
  assert.match(mic.render().textContent, /^Teams\s*Microphone in use\s*\+1$/);

  use([]);
  assert.equal(mic.isActive(), false);
  assert.deepEqual(told.at(-1), ["changed", "mic-use"]);
  assert.equal(told.filter(([what]) => what === "notice").length, 0);
  stop();
});

test("behind the agents at work the row is never drawn: the app is announced by a notice", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const { told, stop, use } = start();
  use(["WindowsSoundRecorder"]);
  t.mock.timers.tick(399);
  assert.equal(told.filter(([what]) => what === "notice").length, 0);
  t.mock.timers.tick(1);
  assert.deepEqual(told.at(-1), ["notice", "mic", "WindowsSoundRecorder", "Microphone in use"]);

  // An app that let go before the wait was over is not announced after the fact.
  use([]);
  use(["brave"]);
  use([]);
  t.mock.timers.tick(1000);
  assert.equal(told.filter(([what]) => what === "notice").length, 1);
  stop();
});

test("with the setting off nothing is shown or announced", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const { told, stop, use, settings } = start();
  settings({ micUse: false });
  assert.deepEqual(told, [["changed", "mic-use"]]);
  use(["brave"]);
  assert.equal(mic.isActive(), false);
  t.mock.timers.tick(1000);
  assert.equal(told.filter(([what]) => what === "notice").length, 0);
  stop();
  // A feature that stopped keeps nothing: the next start begins with no app.
  assert.equal(mic.isActive(), false);
});
