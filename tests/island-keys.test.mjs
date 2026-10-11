// The island's keys under the pointer: the table between the names the page
// uses and the numbers Rust swallows (src/island/keys.ts).

import { test } from "node:test";
import assert from "node:assert/strict";
import { ALT, CTRL, SHIFT, comboCode, comboCodes, comboEvent } from "../src/island/keys.ts";

test("a combination and its number say the same key both ways", () => {
  for (const [name, code, key, id] of [
    ["Escape", 0x1b, "Escape", "Escape"],
    ["Ctrl+K", CTRL | 0x4b, "k", "KeyK"],
    ["Alt+m", ALT | 0x4d, "m", "KeyM"],
    ["Shift+Enter", SHIFT | 0x0d, "Enter", "Enter"],
    ["ArrowDown", 0x28, "ArrowDown", "ArrowDown"],
    ["Ctrl+1", CTRL | 0x31, "1", "Digit1"],
  ]) {
    assert.equal(comboCode(name), code, name);
    const e = comboEvent(code);
    assert.equal(e.key, key, name);
    assert.equal(e.code, id, name);
    assert.equal(e.ctrlKey, (code & CTRL) !== 0, name);
    assert.equal(e.altKey, (code & ALT) !== 0, name);
    assert.equal(e.shiftKey, (code & SHIFT) !== 0, name);
  }
});

test("what the user types into their own window is never the island's", () => {
  // A letter or a digit alone or with Shift only, an unknown key, an unknown modifier.
  for (const name of ["k", "K", "7", "Shift+A", "F5", "Win+K", "Ctrl+", ""]) assert.equal(comboCode(name), null, name);
  assert.equal(comboEvent(0x70), null);
  assert.deepEqual(comboCodes(["Escape", "k", "Ctrl+K", "Escape"]), [0x1b, CTRL | 0x4b]);
});
