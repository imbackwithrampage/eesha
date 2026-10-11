// The wake phrase, page side (src/features/voice/wake.ts; docs/VOICE.md §6, §7.5, §8): when the
// microphone indicator shows, and that it is lit before the microphone opens and put out only after it
// has closed. Rust is made up here (`world`): its events as crates/voice sends them, and `ears.status`.
// What Rust decides (the guards, "only while plugged in" as a rule over the power state, the phrase
// itself) is proved by the example voice-wake-probe; here is what the page makes of Rust's word.

import { test } from "node:test";
import assert from "node:assert/strict";
import { CLOSE_CHECK_MS, earFor, startWake } from "../src/features/voice/wake.ts";
import { emit } from "./tauri.mjs";

// The bridge's unlisten reaches for this.
globalThis.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };

/** Lets every promise that is ready run. */
async function tick(rounds = 6) {
  for (let i = 0; i < rounds; i++) await new Promise((done) => setImmediate(done));
}

/** Rust and the island, as the page sees them. `mic`: the device is open (`ears.status`). */
function world() {
  const handlers = new Map();
  const w = {
    mic: false,
    calls: [],
    /** What the bar was told, in order. */
    ear: [],
    send: (event, payload) => handlers.get(event)?.(payload),
    state: (state) => w.send("state", { state, gen: 1 }),
    called: (method) => w.calls.filter(([name]) => name === method).map(([, args]) => args),
    cx: {
      id: "voice",
      call: async (method, args) => {
        w.calls.push([method, args]);
        return method === "ears.status" ? { mic: w.mic, model: true, stalled: false } : {};
      },
      on: (event, fn) => (handlers.set(event, fn), () => handlers.delete(event)),
      island: { ear: (on, heard) => w.ear.push(!on ? "off" : heard ? "heard" : "on") },
    },
  };
  return w;
}

const STATES = ["off", "idle", "waiting", "hearing", "listening", "thinking", "speaking", "asking", "closing", "paused"];
const on = (state, wanted) => earFor({ state, gen: 1 }, wanted).on;

test("the ear shows for a turn and while it waits, whatever the wake's switch says", () => {
  assert.deepEqual(STATES.filter((s) => on(s, false)), ["waiting", "hearing", "listening", "speaking", "asking"]);
  assert.equal(earFor({ state: "listening", gen: 1 }).on, true, "a turn by the key, the wake phrase off");
});

test("with the wake wanted the microphone stays open between turns, and so does the ear", () => {
  assert.deepEqual(STATES.filter((s) => on(s, true)), ["idle", "waiting", "hearing", "listening", "thinking", "speaking", "asking", "closing"]);
});

test("off and paused show nothing, whatever was wanted before", () => {
  for (const state of ["off", "paused"]) assert.deepEqual(earFor({ state, reason: "lock", gen: 1 }, true), { on: false, heard: false });
});

test("the ear is bright only when a voice has just been heard while it waits", () => {
  assert.deepEqual(STATES.filter((s) => earFor({ state: s, gen: 1 }, true).heard), ["hearing"]);
  assert.deepEqual(earFor({ state: "hearing", gen: 1 }), { on: true, heard: true });
});

test("the page says it can show the indicator, and Rust opens nothing for the wake before that", async () => {
  const w = world();
  const stop = startWake(w.cx);
  assert.deepEqual(w.called("wake.sync"), [{ page: true }]);
  assert.deepEqual(w.ear, [], "nothing is lit by starting");
  stop();
  await tick();
});

test("the ear is lit before the microphone opens: Rust says what it wants before it opens the device", async () => {
  const w = world();
  const stop = startWake(w.cx);
  w.send("wake", { wanted: true, woke: false });
  assert.deepEqual(w.ear, ["on"], "lit while the state is still idle and the device still closed");
  w.mic = true;
  w.state("waiting");
  w.state("hearing");
  w.state("waiting");
  assert.deepEqual(w.ear, ["on", "heard", "on"], "bright for the voice, and no call that changes nothing");
  stop();
  await tick();
});

test("the ear goes out only once the microphone is closed", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const w = world();
  const stop = startWake(w.cx);
  // A turn by the key, the wake phrase off: the device is still open when the turn's text is final.
  w.mic = true;
  w.state("listening");
  w.state("thinking");
  await tick();
  assert.deepEqual(w.ear, ["on"], "thinking, and the device not closed yet: still lit");
  t.mock.timers.tick(CLOSE_CHECK_MS);
  await tick();
  assert.deepEqual(w.ear, ["on"], "looked again, still open");
  w.mic = false;
  t.mock.timers.tick(CLOSE_CHECK_MS);
  await tick();
  assert.deepEqual(w.ear, ["on", "off"]);
  assert.equal(w.called("ears.status").length, 3);
  t.mock.timers.tick(CLOSE_CHECK_MS * 10);
  await tick();
  assert.equal(w.called("ears.status").length, 3, "nothing is asked once it is out");
  stop();
  await tick();
});

test("a microphone that opens again while it was closing keeps the ear lit", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const w = world();
  const stop = startWake(w.cx);
  w.mic = true;
  w.state("listening");
  w.state("thinking");
  w.state("asking");
  w.mic = false;
  await tick();
  t.mock.timers.tick(CLOSE_CHECK_MS * 3);
  await tick();
  assert.deepEqual(w.ear, ["on", "on"], "the check of the turn before does not put out the reply's");
  stop();
  await tick();
});

test("only while plugged in: on battery the ear is out while it waits and lit for a turn by the key", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const w = world();
  const stop = startWake(w.cx);
  // On the charger: it waits for the phrase.
  w.send("wake", { wanted: true, woke: false });
  w.mic = true;
  w.state("waiting");
  // The charger is pulled: Rust no longer wants the microphone and closes it.
  w.send("wake", { wanted: false, woke: false });
  w.state("idle");
  await tick();
  assert.deepEqual(w.ear, ["on"], "not out before the device is closed");
  w.mic = false;
  t.mock.timers.tick(CLOSE_CHECK_MS);
  await tick();
  assert.deepEqual(w.ear, ["on", "off"]);
  // The talk key still works on battery: the ear is on for the turn, and for the turn only.
  w.mic = true;
  w.state("listening");
  assert.deepEqual(w.ear, ["on", "off", "on"]);
  w.mic = false;
  w.state("thinking");
  await tick();
  assert.deepEqual(w.ear, ["on", "off", "on", "off"]);
  w.state("speaking");
  w.state("closing");
  w.state("idle");
  await tick();
  assert.deepEqual(w.ear, ["on", "off", "on", "off", "on", "off"], "on battery it is lit only while it talks: the ears are open for the word stop");
  // The charger is back: lit before the device opens again.
  w.send("wake", { wanted: true, woke: false });
  assert.deepEqual(w.ear, ["on", "off", "on", "off", "on", "off", "on"]);
  stop();
  await tick();
});

test("its own speech: Rust is asked each time it starts or stops talking, and the ear follows its answer", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const w = world();
  const stop = startWake(w.cx);
  w.send("wake", { wanted: true, woke: false });
  w.state("waiting");
  w.state("thinking");
  assert.equal(w.called("wake.sync").length, 1, "only the start so far");
  w.mic = true;
  w.state("speaking");
  assert.deepEqual(w.called("wake.sync"), [{ page: true }, undefined]);
  // Interruption by voice is off: the phrase is not wanted while it talks, but the ears stay open for "stop".
  w.send("wake", { wanted: false, woke: false });
  await tick();
  assert.deepEqual(w.ear, ["on"], "lit while it talks");
  w.state("closing");
  assert.equal(w.called("wake.sync").length, 3, "asked again when it has finished");
  await tick();
  assert.deepEqual(w.ear, ["on"], "still lit while the device closes");
  w.mic = false;
  t.mock.timers.tick(CLOSE_CHECK_MS);
  await tick();
  assert.deepEqual(w.ear, ["on", "off"]);
  w.send("wake", { wanted: true, woke: false });
  assert.deepEqual(w.ear, ["on", "off", "on"]);
  stop();
  await tick();
});

test("paused: the ear is out once the microphone is closed, and lit again with the state that follows", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const w = world();
  const stop = startWake(w.cx);
  w.send("wake", { wanted: true, woke: false });
  w.state("waiting");
  w.send("state", { state: "paused", reason: "lock", gen: 1 });
  await tick();
  assert.deepEqual(w.ear, ["on", "off"]);
  // Unlocked: Rust still wants it and says only that it is idle again.
  w.state("idle");
  assert.deepEqual(w.ear, ["on", "off", "on"]);
  stop();
  await tick();
});

test("the tray's Pause and a shortcut that lifts it: Rust is asked once the island has set the flag", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const w = world();
  const stop = startWake(w.cx);
  await tick();
  emit("tray", "pause");
  assert.equal(w.called("wake.sync").length, 1, "not within the event itself: the island's own handler goes first");
  t.mock.timers.tick(0);
  assert.equal(w.called("wake.sync").length, 2);
  emit("shortcut", "toggle");
  t.mock.timers.tick(0);
  assert.equal(w.called("wake.sync").length, 3);
  stop();
  await tick();
});

test("a wake that no command followed is counted, a wake with a command and a turn by the key are not", async () => {
  const w = world();
  const stop = startWake(w.cx);
  // The phrase, then nothing.
  w.send("wake", { wanted: true, woke: true });
  w.state("listening");
  w.send("heard", { text: "", final: true });
  assert.equal(w.called("wake.empty").length, 1);
  // The phrase and a command.
  w.send("wake", { wanted: true, woke: true });
  w.send("heard", { text: "set a timer", final: false });
  w.send("heard", { text: "set a timer for twelve minutes", final: true });
  // The key, and nothing said.
  w.state("listening");
  w.send("heard", { text: "", final: true });
  // The phrase, then Esc: nobody knows whether a command was coming.
  w.send("wake", { wanted: true, woke: true });
  w.state("closing");
  w.state("listening");
  w.send("heard", { text: " ", final: true });
  assert.equal(w.called("wake.empty").length, 1);
  stop();
  await tick();
});

test("the speech model arrived: Rust is asked again", async () => {
  const w = world();
  const stop = startWake(w.cx);
  w.send("download", { what: "whistle", received: 10, total: 100, done: false });
  assert.equal(w.called("wake.sync").length, 1);
  w.send("download", { what: "whistle", received: 100, total: 100, done: true });
  assert.equal(w.called("wake.sync").length, 2);
  stop();
  await tick();
});

test("the dispose puts the ear out and listens to nothing more", async () => {
  const w = world();
  const stop = startWake(w.cx);
  w.send("wake", { wanted: true, woke: false });
  stop();
  await tick();
  assert.deepEqual(w.ear, ["on", "off"]);
  w.state("listening");
  assert.deepEqual(w.ear, ["on", "off"]);
});
