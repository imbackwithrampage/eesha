// The product's tooltip (src/ui/tooltip.ts): where a hint goes on its page, what in its text is
// a key, and when it comes up and goes. The clock is the test's: nothing here waits.

import { afterEach, beforeEach, mock, test } from "node:test";
import assert from "node:assert/strict";
import { REST_MS, TipTimer, WARM_MS, placeTip, splitTip } from "../src/ui/tooltip.ts";

/** The island's window, and a hint of one short line. */
const PAGE = { w: 720, h: 360 };
const TIP = { w: 80, h: 24 };
const box = (left, top, w = 26, h = 26) => ({ left, top, right: left + w, bottom: top + h });

test("a hint sits under its control, centred, six pixels away", () => {
  assert.deepEqual(placeTip(box(300, 100), TIP, PAGE), { x: 273, y: 132, side: "below", shadow: true });
});

test("no room under it: it flips over the control", () => {
  const at = placeTip(box(300, 320), TIP, PAGE);
  assert.equal(at.side, "above");
  assert.equal(at.y, 320 - 6 - 24);
});

test("near a side it slides along to stay inside, never past the edge", () => {
  assert.equal(placeTip(box(0, 100), TIP, PAGE).x, 4);
  assert.equal(placeTip(box(700, 100, 20), TIP, PAGE).x, 720 - 4 - 80);
  // And says when the floating shadow would be cut by the page's edge.
  assert.equal(placeTip(box(0, 100), TIP, PAGE).shadow, false);
  assert.equal(placeTip(box(300, 300), TIP, PAGE).shadow, false);
});

test("a control as tall as the page takes its hint beside it, on the side with room", () => {
  const strip = { w: 300, h: 32 };
  assert.equal(placeTip(box(20, 0, 28, 32), TIP, strip).side, "right");
  assert.equal(placeTip(box(250, 0, 28, 32), TIP, strip).side, "left");
});

test("no side holds all of it: it is not shown rather than shown cut", () => {
  // The hidden island's window: 240 x 6.
  assert.equal(placeTip(box(100, 0, 26, 6), TIP, { w: 240, h: 6 }), null);
  // A control that fills a small page, and a hint wider than its page.
  assert.equal(placeTip(box(0, 0, 100, 40), TIP, { w: 100, h: 40 }), null);
  assert.equal(placeTip(box(10, 10), { w: 300, h: 24 }, { w: 288, h: 200 }), null);
});

test("a key at the end of a hint is drawn as key caps", () => {
  assert.deepEqual(splitTip("Fold (Esc)"), { label: "Fold", keys: ["Esc"] });
  assert.deepEqual(splitTip("All sections (Ctrl+K)"), { label: "All sections", keys: ["Ctrl", "K"] });
  assert.deepEqual(splitTip("Paste (Ctrl+Shift+V)"), { label: "Paste", keys: ["Ctrl", "Shift", "V"] });
  // Words in brackets are words.
  assert.deepEqual(splitTip("Claude (5 hours)"), { label: "Claude (5 hours)", keys: [] });
  assert.deepEqual(splitTip("Settings"), { label: "Settings", keys: [] });
});

let clock = 0;
const after = (ms) => {
  clock += ms;
  mock.timers.tick(ms);
};
beforeEach(() => {
  clock = 0;
  mock.timers.enable({ apis: ["setTimeout"] });
});
afterEach(() => mock.timers.reset());

/** The rules with a page that writes down what it is asked. */
function rules(room = true) {
  const log = [];
  const tip = new TipTimer((el) => (log.push(`show ${el}`), room), () => log.push("hide"), () => clock);
  return { tip, log };
}

test("the first hint waits for the pointer to rest", () => {
  const { tip, log } = rules();
  tip.over("fold");
  after(REST_MS - 1);
  assert.deepEqual(log, []);
  assert.equal(tip.waiting, true);
  after(1);
  assert.deepEqual(log, ["show fold"]);
  assert.equal(tip.shown, "fold");
  assert.equal(tip.waiting, false);
});

test("a pointer that only passes brings nothing, and leaves no timer behind", () => {
  const { tip, log } = rules();
  tip.over("fold");
  after(100);
  tip.over(null);
  assert.equal(tip.waiting, false);
  after(1000);
  assert.deepEqual(log, []);
});

test("from one hinted control to the next the hint comes at once", () => {
  const { tip, log } = rules();
  tip.over("fold");
  after(REST_MS);
  tip.over("settings");
  assert.deepEqual(log, ["show fold", "hide", "show settings"]);
  // Also across a gap between two controls, for a moment.
  tip.over(null);
  after(WARM_MS - 1);
  tip.over("sound");
  assert.equal(tip.shown, "sound");
  // After that the pointer has to rest again.
  tip.over(null);
  after(WARM_MS);
  tip.over("fold");
  assert.equal(tip.shown, null);
  assert.equal(tip.waiting, true);
});

test("a press, a key or a scroll puts it away at once, and it stays away on that control", () => {
  const { tip, log } = rules();
  tip.over("fold");
  after(REST_MS);
  tip.away();
  assert.deepEqual(log, ["show fold", "hide"]);
  // The pointer is still on the control it pressed: nothing comes back.
  tip.over("fold");
  after(5000);
  assert.deepEqual(log, ["show fold", "hide"]);
  assert.equal(tip.waiting, false);
  // A press before the hint was up means there is none to wait for.
  tip.over(null);
  after(WARM_MS);
  tip.over("settings");
  tip.away();
  after(5000);
  assert.equal(tip.shown, null);
  assert.equal(tip.waiting, false);
});

test("a hint with no room is not up, and nothing is hidden for it", () => {
  const { tip, log } = rules(false);
  tip.over("fold");
  after(REST_MS);
  assert.equal(tip.shown, null);
  tip.over(null);
  assert.deepEqual(log, ["show fold"]);
});
