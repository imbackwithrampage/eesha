// The Visor's two voice faces (src/mascot/engine.ts `voice`), the dot's level, and proof that a
// face costs nothing when none is set: the draw calls for existing states are hashed and must not change.

import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";

globalThis.document ??= { documentElement: {} };
globalThis.getComputedStyle ??= () => ({ getPropertyValue: () => "" });
Math.random = () => 0.5;

const load = async (reduced) => {
  globalThis.matchMedia = () => ({ matches: reduced });
  return import(new URL(`../src/mascot/engine.ts?${reduced ? "r" : "n"}`, import.meta.url).href);
};
const { BotEngine } = await load(false);
const still = await load(true);

/** A canvas stand-in that records every call and property write. */
function recorder() {
  const log = [];
  const rects = [];
  const fmt = (a) => a.map((v) => (typeof v === "number" ? v.toFixed(6) : typeof v === "object" ? "obj" : String(v))).join(",");
  const grad = { addColorStop: (...a) => log.push(`g.stop(${fmt(a)})`) };
  const ctx = new Proxy({ canvas: { width: 100, height: 100 }, globalAlpha: 1 }, {
    get: (t, k) => (k in t ? t[k] : (...a) => { if (k === "roundRect") rects.push([...a.slice(0, 4), t.globalAlpha]); log.push(`${String(k)}(${fmt(a)})`); return grad; }),
    set: (t, k, v) => { t[k] = v; log.push(`${String(k)}=${typeof v === "object" ? "obj" : v}`); return true; },
  });
  return { ctx, log, rects };
}

function frames(E, setup, d, n = 40) {
  const e = new E("idle");
  setup(e);
  const r = recorder();
  for (let i = 0; i < n; i++) {
    e.update(0.016);
    r.ctx.globalAlpha = 1;
    r.rects.length = 0;
    e.draw(r.ctx, 50, 50, d);
  }
  return { e, log: r.log, rects: r.rects };
}
/** The bars of the voice face in the last frame: the rects that are drawn visibly after the band. */
const bars = (rects) => rects.slice(1).filter((r) => r[4] > 0.5);
const hash = (log) => createHash("sha256").update(log.join("\n")).digest("hex").slice(0, 16);

const BASELINE = {};

test("a face costs nothing when none is set: five existing states draw as before", () => {
  for (const s of ["idle", "working", "thinking", "approval", "sleeping"]) {
    for (const d of [58, 22]) {
      const { log } = frames(BotEngine, (e) => e.setState(s), d);
      const k = `${s}@${d}`;
      if (process.env.PRINT_BASELINE) BASELINE[k] = hash(log);
      else assert.equal(hash(log), EXPECT[k], k);
    }
  }
  if (process.env.PRINT_BASELINE) console.log(JSON.stringify(BASELINE));
});
const speak = (face, level) => (e) => { e.voice = { face, level }; };

test("listening follows the microphone at 58, 28 and 22 px", () => {
  for (const [d, n] of [[58, 5], [28, 3], [22, 3]]) {
    const quiet = bars(frames(BotEngine, speak("listening", 0), d, 60).rects);
    const loud = bars(frames(BotEngine, speak("listening", 1), d, 60).rects);
    assert.equal(quiet.length, n, `segments at ${d}`);
    assert.equal(loud.length, n);
    assert.ok(loud.every((r, i) => r[3] > quiet[i][3]), `taller with level at ${d}`);
    const mid = loud[n >> 1][3];
    assert.ok(mid >= loud[0][3], "the middle is the tallest");
  }
});

test("talking follows the output level: taller and narrower as it grows", () => {
  for (const d of [58, 28, 22]) {
    const [q] = bars(frames(BotEngine, speak("talking", 0), d, 60).rects);
    const [l] = bars(frames(BotEngine, speak("talking", 1), d, 60).rects);
    assert.ok(l[3] > q[3] && (d < 30 ? l[2] <= q[2] : l[2] < q[2]), `at ${d}`);
  }
});

test("at 22 px every voice rect sits on whole pixels, whatever the level", () => {
  for (const face of ["listening", "talking"]) {
    const e = new BotEngine("idle");
    e.voice = { face, level: 0 };
    const r = recorder();
    for (let i = 0; i < 90; i++) {
      e.voice.level = (Math.sin(i / 5) + 1) / 2;
      e.update(0.016);
      r.rects.length = 0;
      e.draw(r.ctx, 50, 50, 22);
      if (i > 40) for (const q of bars(r.rects)) assert.ok(q.slice(0, 4).every(Number.isInteger), `${face}: ${q}`);
    }
  }
});

test("a level out of range or not a number is held to 0…1", () => {
  for (const level of [Infinity, NaN, -4, 9]) {
    const { rects } = frames(BotEngine, speak("listening", level), 58, 60);
    assert.ok(bars(rects).every((r) => Number.isFinite(r[3]) && r[3] < 40));
  }
});

test("reduced motion: one still pose per face, whatever the level, and nothing left to animate", () => {
  for (const face of ["listening", "talking"]) {
    const a = frames(still.BotEngine, speak(face, 0), 58, 80);
    const b = frames(still.BotEngine, speak(face, 1), 58, 80);
    assert.deepEqual(bars(a.rects), bars(b.rects));
    assert.ok(bars(a.rects).length > 0);
    assert.equal(a.e.busy, false);
  }
});

test("the other faces take a pose the engine has", () => {
  const same = (face, pose) =>
    assert.equal(hash(frames(BotEngine, (e) => { e.voice = { face, level: 0.5 }; }, 58).log), hash(frames(BotEngine, (e) => { e.face = pose; }, 58).log), face);
  same("thinking", "thinking");
  same("asking", "question");
  same("surprised", "surprised");
  same("wink", "wink");
});

test("clearing the face returns to the state's own drawing", () => {
  const e = new BotEngine("working");
  e.voice = { face: "talking", level: 1 };
  for (let i = 0; i < 40; i++) e.update(0.016);
  e.voice = null;
  for (let i = 0; i < 200; i++) e.update(0.016);
  const r = recorder();
  e.draw(r.ctx, 50, 50, 58);
  const c = frames(BotEngine, (x) => x.setState("working"), 58, 1);
  assert.equal(e.wl + e.wt, 0);
  assert.equal(bars(r.rects).length, bars(c.rects).length);
});

test("the dot carries the face and the level", async () => {
  const { setDotMark } = await import("../src/ui/components.ts");
  const props = {};
  const el = { dataset: {}, style: { setProperty: (k, v) => (props[k] = v), removeProperty: (k) => delete props[k] } };
  props["--level"] = "0.500";
  setDotMark(el, "#fff", "listening");
  assert.equal(el.dataset.state, "listening");
  assert.equal(props["--level"], "0.500", "the island's level is not cleared by a state sync");
  setDotMark(el, "#fff", "talking", 0.25);
  assert.equal(props["--level"], "0.25");
});

const EXPECT = {"idle@58":"87617eb63a1a9e66","idle@22":"6c35a7bbe0a4cee3","working@58":"28788eee9983bbd0","working@22":"f62d002a1deb6fb0","thinking@58":"79f27b3fc02df852","thinking@22":"fb441cee8056112d","approval@58":"ea0d7e380bab7f37","approval@22":"8bfa8b88638ddfac","sleeping@58":"1ce96383374482e7","sleeping@22":"6fd80e65ac645cf6"};
