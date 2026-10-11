// Drag the island anywhere (src/island/place.ts): when a press is a drag, where
// a drop docks or floats, and where a saved spot is on the displays of today.

import { test } from "node:test";
import assert from "node:assert/strict";
import { DRAG_MIN, displayAt, dragStarted, findDisplay, resolve, settle } from "../src/island/place.ts";

/** A display in screen pixels with a 48 px (logical) taskbar at the bottom. */
function display(name, x, y, w, h, scale = 1) {
  return {
    key: `at:${Math.round(x / scale)},${Math.round(y / scale)}|${name}|${Math.round(w / scale)}x${Math.round(h / scale)}`,
    x, y, w, h, wx: x, wy: y, ww: w, wh: h - 48 * scale, scale,
  };
}

const main = display("ONE", 0, 0, 1920, 1080);
const right = display("TWO", 1920, 0, 2560, 1440);
const leftHiDpi = display("THREE", -3000, 0, 3000, 2000, 2);

test("a press is a drag only once the pointer has travelled", () => {
  assert.equal(dragStarted(0, 0), false);
  assert.equal(dragStarted(3, 4), false);
  assert.equal(dragStarted(DRAG_MIN, 0), false);
  assert.equal(dragStarted(DRAG_MIN + 1, 0), true);
  assert.equal(dragStarted(-5, -5), true);
});

test("let go in the middle of a display, it floats there", () => {
  assert.deepEqual(settle(700, 500, [main]), { display: main.key, x: 700, y: 500, docked: false });
});

test("let go near a top edge, it docks at that horizontal place", () => {
  assert.deepEqual(settle(600, 24, [main]), { display: main.key, x: 600, y: 0, docked: true });
  assert.equal(settle(600, 25, [main]).docked, false);
  // Above the screen counts as its top edge.
  assert.deepEqual(settle(600, -40, [main]), { display: main.key, x: 600, y: 0, docked: true });
});

test("docked close to the middle, it is the middle", () => {
  assert.equal(settle(960 + 24, 10, [main]).x, 960);
  assert.equal(settle(960 - 24, 0, [main]).x, 960);
  assert.equal(settle(960 + 25, 10, [main]).x, 985);
  // Floating, nothing snaps.
  assert.equal(settle(970, 400, [main]).x, 970);
});

test("the whole open island stays on its display", () => {
  // A corner: 320 from the side (half the open island), and above the taskbar by the tallest island.
  assert.deepEqual(settle(1910, 1070, [main]), { display: main.key, x: 1600, y: 1080 - 48 - 320, docked: false });
  assert.deepEqual(settle(5, 300, [main]), { display: main.key, x: 320, y: 300, docked: false });
  assert.equal(settle(3, 3, [main]).x, 320);
  // A display narrower than the island centres it.
  const narrow = display("SLIM", 0, 0, 500, 900);
  assert.equal(settle(40, 300, [narrow]).x, 250);
  assert.equal(settle(40, 5, [narrow]).x, 250);
});

test("another display, another scale: the spot is in that display's own logical pixels", () => {
  const all = [main, right, leftHiDpi];
  assert.deepEqual(settle(1920 + 1000, 700, all), { display: right.key, x: 1000, y: 700, docked: false });
  assert.deepEqual(settle(-2000, 600, all), { display: leftHiDpi.key, x: 500, y: 300, docked: false });
  assert.deepEqual(settle(-1500, 30, all), { display: leftHiDpi.key, x: 750, y: 0, docked: true });
  // Over nothing (below the smaller display): the nearest one.
  assert.equal(displayAt(500, 1300, all), main);
  assert.equal(settle(500, 1300, all).display, main.key);
});

test("a saved spot is found where it was", () => {
  const all = [main, right, leftHiDpi];
  for (const [x, y] of [[700, 500], [600, 3], [2920, 700], [-2000, 600], [-1500, 30]]) {
    const spot = settle(x, y, all);
    const place = resolve(spot, all);
    assert.deepEqual(settle(place.x, place.y, all), spot);
    assert.equal(place.floating, !spot.docked);
  }
  assert.deepEqual(resolve({ display: leftHiDpi.key, x: 500, y: 300, docked: false }, all), { x: -2000, y: 600, scale: 2, floating: true });
});

test("it follows its display through a new layout, a new resolution and a new scale", () => {
  const spot = { display: right.key, x: 1000, y: 700, docked: false };
  // Moved to the left of the main display.
  const moved = display("TWO", -2560, 0, 2560, 1440);
  assert.deepEqual(resolve(spot, [main, moved]), { x: -1560, y: 700, scale: 1, floating: true });
  assert.equal(findDisplay(right.key, [main, moved]), moved);
  // A smaller resolution: still on screen, the whole island.
  const small = display("TWO", 1920, 0, 1280, 720);
  assert.deepEqual(resolve(spot, [main, small]), { x: 1920 + 960, y: 720 - 48 - 320, scale: 1, floating: true });
  // Scaled to 150 %: the same logical place, which is other pixels.
  const scaled = display("TWO", 1920, 0, 2560, 1440, 1.5);
  assert.deepEqual(resolve(spot, [main, scaled]), { x: 1920 + 1500, y: 1440 - 72 - 480, scale: 1.5, floating: true });
});

test("a display that is gone: nothing, which is the top centre of the main display", () => {
  const spot = { display: right.key, x: 1000, y: 700, docked: false };
  assert.equal(resolve(spot, [main]), null);
  assert.equal(resolve(spot, []), null);
  assert.equal(resolve(null, [main]), null);
  assert.equal(resolve(undefined, [main]), null);
  // Half-written settings lose only the spot.
  assert.equal(resolve({ display: main.key, x: "a", y: 3, docked: false }, [main]), null);
  assert.equal(resolve({ x: 1, y: 3 }, [main]), null);
});

test("it is never restored off screen", () => {
  for (const spot of [
    { display: main.key, x: 99999, y: 99999, docked: false },
    { display: main.key, x: -500, y: -500, docked: false },
    { display: main.key, x: 99999, y: 0, docked: true },
  ]) {
    const place = resolve(spot, [main]);
    assert.ok(place.x - 320 >= 0 && place.x + 320 <= 1920, `x ${place.x}`);
    assert.ok(place.y >= 0 && place.y + 320 <= 1080 - 48, `y ${place.y}`);
  }
});
