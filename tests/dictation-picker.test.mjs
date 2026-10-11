// The "Hold to dictate" picker (src/features/dictation/picker.ts), fed with made-up keys:
// what is taken and saved, what is refused and why, Esc and Backspace.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { picker } from "../src/features/dictation/picker.ts";

/** A key event as the page would see it. `held`: c Ctrl, a Alt, s Shift, w the Windows key. */
const key = (code, name, held = "") => ({
  code, key: name, ctrlKey: held.includes("c"), altKey: held.includes("a"), shiftKey: held.includes("s"), metaKey: held.includes("w"),
});
const CTRL = key("ControlLeft", "Control", "c");
const ALT = key("AltLeft", "Alt", "ca");
const NEEDS = "Hold Ctrl, Alt or the Windows key with it.";
const OTHER = "Use a letter, a digit, Space or a function key.";

test("Ctrl+Alt+Q shows as it is held and is saved when a key is let go", () => {
  const p = picker();
  assert.deepEqual(p.down(CTRL), { kind: "held", keys: "Ctrl" });
  assert.deepEqual(p.down(ALT), { kind: "held", keys: "Ctrl+Alt" });
  assert.deepEqual(p.down(key("KeyQ", "q", "ca")), { kind: "held", keys: "Ctrl+Alt+Q" });
  assert.deepEqual(p.up(key("KeyQ", "q", "ca")), { kind: "save", chord: "Ctrl+Alt+Q" });
});

test("a lone letter is refused with the reason, and nothing is saved", () => {
  const p = picker();
  assert.deepEqual(p.down(key("KeyQ", "q")), { kind: "refused", why: NEEDS });
  assert.deepEqual(p.up(key("KeyQ", "q")), { kind: "held", keys: "" });
  // Shift alone is how a capital is typed: not enough either.
  assert.deepEqual(p.down(key("KeyQ", "Q", "s")), { kind: "refused", why: NEEDS });
  // It still listens: the next combination is taken.
  p.down(key("KeyD", "d", "cs"));
  assert.deepEqual(p.up(key("ControlLeft", "Control", "s")), { kind: "save", chord: "Ctrl+Shift+D" });
});

test("Esc cancels and Backspace clears", () => {
  assert.deepEqual(picker().down(key("Escape", "Escape")), { kind: "cancel" });
  assert.deepEqual(picker().down(key("Backspace", "Backspace")), { kind: "clear" });
  assert.deepEqual(picker().down(key("Delete", "Delete")), { kind: "clear" });
});

test("modifiers let go with no key save nothing", () => {
  const p = picker();
  p.down(CTRL);
  p.down(ALT);
  assert.deepEqual(p.up(key("AltLeft", "Alt", "c")), { kind: "held", keys: "Ctrl" });
  assert.deepEqual(p.up(key("ControlLeft", "Control")), { kind: "held", keys: "" });
});

test("a key the hold shortcut does not know, and one AltGr types, are refused with their reason", () => {
  const p = picker();
  assert.deepEqual(p.down(key("ArrowUp", "ArrowUp", "ca")), { kind: "refused", why: OTHER });
  assert.deepEqual(p.down(key("Pause", "Pause", "c")), { kind: "refused", why: OTHER });
  // A German keyboard: Ctrl+Alt+Q is AltGr+Q, which types @.
  const typed = p.down(key("KeyQ", "@", "ca"));
  assert.equal(typed.kind, "refused");
  assert.deepEqual(typed.vars, { keys: "Ctrl+Alt+Q", char: "@" });
  // A refused press is not saved by its release.
  assert.equal(p.up(key("KeyQ", "@", "ca")).kind, "held");
});

test("what is saved is spelled as crates/dictation/src/hold.rs reads it", () => {
  const taken = [
    [key("Space", " ", "w"), "Win+Space"],
    [key("F9", "F9", "c"), "Ctrl+F9"],
    [key("Digit1", "1", "cas"), "Ctrl+Alt+Shift+1"],
    [key("KeyQ", "q", "ca"), "Ctrl+Alt+Q"],
  ];
  // The names `vk_of` knows: the four modifiers, Space, one letter or digit, F1 to F24.
  const rust = readFileSync(new URL("../crates/dictation/src/hold.rs", import.meta.url), "utf8");
  for (const name of ["Ctrl", "Alt", "Shift", "Win", "Space"]) assert.ok(rust.includes(`"${name}" =>`), name);
  for (const [press, chord] of taken) {
    const p = picker();
    p.down(press);
    assert.deepEqual(p.up(press), { kind: "save", chord });
    for (const part of chord.split("+")) assert.match(part, /^(Ctrl|Alt|Shift|Win|Space|[A-Z0-9]|F([1-9]|1\d|2[0-4]))$/);
  }
});
