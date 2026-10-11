// The wheel's highlight (src/features/radial-menu): the geometry it travels by (ring.ts), and that
// every way of choosing a slice moves the one shape the shorter way round (wheel.ts, on a made-up DOM).

import { test } from "node:test";
import assert from "node:assert/strict";
import { installFakeDom } from "./fakedom.mjs";
import { INNER, MID, RADIUS, shorter, sliceAngle, travel, wedge } from "../src/features/radial-menu/ring.ts";

/** The corner points of a wedge path, in drawing order ("x y" pairs; the arc radii are whole numbers and are skipped). */
const points = (d) => [...d.matchAll(/(-?\d+\.\d\d) (-?\d+\.\d\d)/g)].map((m) => [Number(m[1]), Number(m[2])]);
const turned = ([x, y], degrees) => {
  const a = (degrees * Math.PI) / 180;
  const [dx, dy] = [x - MID, y - MID];
  return [MID + dx * Math.cos(a) - dy * Math.sin(a), MID + dx * Math.sin(a) + dy * Math.cos(a)];
};
const near = (a, b, eps = 0.02) => Math.abs(a - b) <= eps;

test("a slice is centred clockwise from 12 o'clock", () => {
  assert.equal(sliceAngle(0, 8), 0);
  assert.equal(sliceAngle(2, 8), 90);
  assert.equal(sliceAngle(3, 6), 180);
  assert.equal(sliceAngle(5, 6), 300);
  assert.equal(sliceAngle(0, 0), 0);
});

test("the shorter way round", () => {
  assert.equal(shorter(0, 1, 8), 1);
  assert.equal(shorter(1, 0, 8), -1);
  // Across the seam between the last slice and the first: one step, not seven back.
  assert.equal(shorter(7, 0, 8), 1);
  assert.equal(shorter(0, 7, 8), -1);
  assert.equal(shorter(1, 6, 8), -3);
  assert.equal(shorter(6, 1, 8), 3);
  // Straight across is as far either way: clockwise.
  assert.equal(shorter(0, 4, 8), 4);
  assert.equal(shorter(4, 0, 8), 4);
  assert.equal(shorter(2, 2, 5), 0);
  assert.equal(shorter(0, 3, 5), -2);
  assert.equal(shorter(0, 1, 2), 1);
  assert.equal(shorter(0, 0, 1), 0);
  assert.equal(shorter(0, 3, 0), 0);
  // Never more than half the ring, whatever the two slices.
  for (let n = 1; n <= 12; n++) for (let a = 0; a < n; a++) for (let b = 0; b < n; b++) {
    const s = shorter(a, b, n);
    assert.ok(Math.abs(s) <= n / 2, `${a} to ${b} of ${n}`);
    assert.equal((((a + s) % n) + n) % n, b);
  }
});

test("the highlight's angle is never wrapped, and always ends on the slice", () => {
  // Round and round clockwise: the angle keeps growing, so no transition runs backwards over the seam.
  let angle = sliceAngle(6, 8);
  for (const [from, to] of [[6, 7], [7, 0], [0, 1]]) angle = travel(angle, from, to, 8);
  assert.equal(angle, 405);
  assert.equal(travel(0, 0, 7, 8), -45);
  // A long walk over every pair ends where a fresh start would put it, modulo a turn.
  let at = 0;
  let turn = 0;
  for (const to of [3, 7, 1, 6, 0, 4, 2, 2, 5, 0]) {
    const step = Math.abs(travel(turn, at, to, 8) - turn);
    assert.ok(step <= 180, "never the long way");
    turn = travel(turn, at, to, 8);
    at = to;
    const off = (((turn - sliceAngle(to, 8)) % 360) + 360) % 360;
    assert.ok(near(off, 0, 1e-9) || near(off, 360, 1e-9), `on slice ${to}`);
  }
});

test("the wedge of slice 0, turned by a slice's angle, is that slice's wedge", () => {
  for (let n = 2; n <= 12; n++) {
    const first = points(wedge(0, n));
    assert.equal(first.length, 5, `five corners for ${n} slices`);
    for (let i = 0; i < n; i++) {
      const own = points(wedge(i, n));
      first.forEach((p, k) => {
        const [x, y] = turned(p, sliceAngle(i, n));
        assert.ok(near(x, own[k][0]) && near(y, own[k][1]), `slice ${i} of ${n}, corner ${k}`);
      });
    }
  }
});

test("a wedge stays inside the ring, is even about its middle and leaves a gap to its neighbours", () => {
  for (let n = 1; n <= 12; n++) {
    const p = points(wedge(0, n));
    for (const [x, y] of p) {
      assert.ok(Number.isFinite(x) && Number.isFinite(y));
      const r = Math.hypot(x - MID, y - MID);
      assert.ok(r >= INNER && r <= RADIUS, `${n} slices: ${r}`);
    }
    // Slice 0 is centred at 12 o'clock: its left and right corners mirror each other.
    assert.ok(near(p[0][0] + p[3][0], 2 * MID) && near(p[0][1], p[3][1]), `inner corners of ${n}`);
    assert.ok(near(p[1][0] + p[2][0], 2 * MID) && near(p[1][1], p[2][1]), `outer corners of ${n}`);
    // The outer corner stops short of the slice's edge.
    const outer = Math.atan2(p[2][0] - MID, MID - p[2][1]);
    if (n > 1) assert.ok(outer > 0 && outer < Math.PI / n, `gap of ${n}`);
  }
});

test("pointer, arrows and scroll move one highlight: in place from rest, the shorter way from a slice", async () => {
  const dom = installFakeDom();
  const { mountWheel } = await import("../src/features/radial-menu/wheel.ts");
  const root = dom.root();
  const wheel = mountWheel(root, { t: (key) => key, icon: async () => null, art: async () => null });
  const items = Array.from({ length: 8 }, (_, i) => ({ kind: "link", name: `Slice ${i}`, target: "https://example.com", icon: "", items: [] }));
  wheel.show({ title: "Wheel", back: false, items, sticky: false, np: null });

  const byClass = (name) => [...root.walk()].filter((e) => (e.getAttribute("class") ?? e.className).split(" ").includes(`f-radial-menu-${name}`));
  const [glow] = byClass("glow");
  const marks = byClass("mark");
  const hubs = byClass("hub-text");
  const shownWords = () => hubs.filter((hub) => hub.classList.contains("on")).map((hub) => hub.textContent);
  assert.equal(byClass("glow").length, 1, "one highlight for the whole ring");
  assert.equal(marks.length, 8);
  assert.equal(glow.firstChild.getAttribute("d"), wedge(0, 8));
  assert.ok(!glow.classList.contains("on"));
  assert.deepEqual(shownWords(), ["Wheel"]);

  // From rest: put on the slice, no travel, and it fades in.
  wheel.hover(1);
  assert.equal(glow.style.transform, "rotate(45deg)");
  assert.ok(glow.classList.contains("on") && !glow.classList.contains("travel"));
  assert.deepEqual(marks.map((m) => m.classList.contains("on")), [false, true, false, false, false, false, false, false]);
  assert.deepEqual(shownWords(), ["Slice 1"]);

  // From a slice: it travels, the shorter way (1 to 7 is two steps back, not six on).
  wheel.hover(7);
  assert.equal(glow.style.transform, "rotate(-45deg)");
  assert.ok(glow.classList.contains("travel"));
  wheel.hover(0);
  assert.equal(glow.style.transform, "rotate(0deg)");
  assert.deepEqual(shownWords(), ["Slice 0"]);

  // Leaving fades it out where it is; a slice chosen right after is still travelled to.
  wheel.hover(null);
  assert.ok(!glow.classList.contains("on"));
  assert.equal(glow.style.transform, "rotate(0deg)");
  assert.deepEqual(shownWords(), ["Wheel"]);
  wheel.hover(3);
  assert.equal(glow.style.transform, "rotate(135deg)");
  assert.ok(glow.classList.contains("on") && glow.classList.contains("travel"));

  // After a rest in the hub it appears in place again.
  wheel.hover(null);
  await new Promise((done) => setTimeout(done, 200));
  wheel.hover(6);
  assert.equal(glow.style.transform, "rotate(270deg)");
  assert.ok(glow.classList.contains("on") && !glow.classList.contains("travel"));

  // A new level starts clean, with the wedge for its own count of slices.
  wheel.show({ title: "Group", back: true, items: items.slice(0, 3), sticky: true, np: null });
  assert.ok(!glow.classList.contains("on"));
  assert.equal(glow.firstChild.getAttribute("d"), wedge(0, 3));
  assert.deepEqual(shownWords(), ["GroupClick to go back"]);
  wheel.hover(2);
  assert.equal(glow.style.transform, "rotate(240deg)");
  assert.ok(!glow.classList.contains("travel"));
});
