// A card that grows under the pointer (Settings, Island, "Enlarge cards on hover") takes the
// mascot in its header with it (src/island/island.ts): the size and the place it goes to, and
// the card's own time and curve on the way (src/core/anim.ts).

import { test } from "node:test";
import assert from "node:assert/strict";
import { Tracked, easeOut, scaleFrom } from "../src/core/anim.ts";

/** The question card: 608 x 172 under the 40 px header, the mascot 22 px across in its top left corner. */
const CARD = { x: 0, y: 40 + 86 };
const MARK = { x: 16 + 12 + 11 - 320, y: 40 + 12 + 11, d: 22 };
const GROW = 1.02;

test("grown by 2 % about the card's centre, the mascot is 2 % larger and still on its slot", () => {
  const x = scaleFrom(MARK.x, CARD.x, GROW);
  const y = scaleFrom(MARK.y, CARD.y, GROW);
  // The slot's centre moves away from the card's centre by 2 % of its distance: about 6 px left, 1 px up.
  assert.ok(Math.abs(x - (MARK.x - 5.62)) < 0.01);
  assert.ok(Math.abs(y - (MARK.y - 1.26)) < 0.01);
  assert.ok(Math.abs(MARK.d * GROW - 22.44) < 1e-9);
  // The card's centre is where it was, and going back is exact.
  assert.equal(scaleFrom(CARD.x, CARD.x, GROW), CARD.x);
  assert.equal(scaleFrom(MARK.x, CARD.x, 1), MARK.x);
});

test("it goes there in the card's own 120 ms, on the card's own curve, and back the same way", () => {
  const size = new Tracked(22);
  size.curveTowards(22.44, 120, 1000, easeOut);
  size.step(0.06, 1060);
  assert.ok(Math.abs(size.value - (22 + 0.44 * easeOut(0.5))) < 1e-9);
  // An ease-out: most of the way by half the time.
  assert.ok(size.value > 22.3);
  size.step(0.06, 1120);
  assert.equal(size.value, 22.44);
  assert.equal(size.animating, false);
  size.curveTowards(22, 120, 2000, easeOut);
  size.step(0.12, 2120);
  assert.equal(size.value, 22);
});

test("a curve that names no easing is still the close curve", () => {
  const h = new Tracked(100);
  h.curveTowards(0, 340, 0);
  h.step(0.17, 170);
  // cubic-bezier(.45, 0, .2, 1) is past the middle at half time.
  assert.ok(h.value < 50 && h.value > 5);
});
