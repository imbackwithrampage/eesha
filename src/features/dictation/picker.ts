// The rules of the "Hold to dictate" picker, without the DOM: what each key pressed or let go
// means while it listens (tests/dictation-picker.test.mjs feeds it made-up keys).
//
// A press is read as the Shortcuts page reads it (`recordPress`: Esc, Backspace, the modifier rule,
// what AltGr types). Two things differ: a combination is taken when a key is let go, and only the
// keys crates/dictation/src/hold.rs knows pass (letters, digits, Space, F1 to F24; "Win", not "Super").

import { displayKeys, recordPress, type KeyPress } from "../../core/shortcuts";

export type Picked =
  /** Still listening: what is held so far ("Ctrl+Alt", "Ctrl+Alt+Q"), to show. */
  | { kind: "held"; keys: string }
  /** Still listening: why that press cannot be the combination (English, shown through `t`). */
  | { kind: "refused"; why: string; vars?: Record<string, string> }
  | { kind: "save"; chord: string }
  | { kind: "clear" }
  | { kind: "cancel" };

const OTHER_KEY = "Use a letter, a digit, Space or a function key.";

const mods = (e: KeyPress): string[] =>
  [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Win"].filter((m): m is string => !!m);

/** One listening session: every keydown goes to `down`, every keyup to `up`, until one answers save, clear or cancel. */
export function picker(): { down(e: KeyPress): Picked; up(e: KeyPress): Picked } {
  /** A whole combination is held: letting a key go saves it. */
  let ready = "";
  return {
    down(e) {
      ready = "";
      const r = recordPress(e);
      switch (r.kind) {
        case "cancel":
        case "clear":
          return r;
        case "pending":
          return { kind: "held", keys: mods(e).join("+") };
        case "needsModifier":
          return { kind: "refused", why: "Hold Ctrl, Alt or the Windows key with it." };
        case "typesCharacter":
          return {
            kind: "refused",
            why: "{keys} types “{char}” on your keyboard, so it can't be a shortcut. Pick another key.",
            vars: { keys: displayKeys(r.keys), char: r.typed },
          };
        case "unsupported":
          return { kind: "refused", why: OTHER_KEY };
        case "keys": {
          const key = r.keys.split("+").pop() ?? "";
          if (!/^([A-Z0-9]|Space|F\d+)$/.test(key)) return { kind: "refused", why: OTHER_KEY };
          ready = [...mods(e), key].join("+");
          return { kind: "held", keys: ready };
        }
      }
    },
    up(e) {
      return ready ? { kind: "save", chord: ready } : { kind: "held", keys: mods(e).join("+") };
    },
  };
}
