// The island's keys under the pointer (docs/ARCHITECTURE.md §4, "Keyboard").
//
// While the island is open with the pointer over it and does not hold the
// keyboard, Rust reads its keys with a keyboard hook (src-tauri/src/platform/
// keyhook.rs): the page says which combinations are the island's at that
// moment, Rust swallows exactly those and sends each one back, and the page
// hands it to its own key listeners as if it had been typed into it.
// This file is the table between the two: a combination by name ("Escape",
// "Ctrl+K", "Alt+M", "Shift+Enter") and the number Rust goes by (modifiers
// above, the Windows virtual key below). No DOM, no Tauri.

export const CTRL = 1 << 16;
export const ALT = 2 << 16;
export const SHIFT = 4 << 16;

/** Key name (KeyboardEvent.key) to Windows virtual key, for the keys that have a name. */
const NAMED: Record<string, number> = {
  Escape: 0x1b, Enter: 0x0d, Tab: 0x09, Backspace: 0x08, Delete: 0x2e, Space: 0x20,
  ArrowLeft: 0x25, ArrowUp: 0x26, ArrowRight: 0x27, ArrowDown: 0x28,
  Home: 0x24, End: 0x23, PageUp: 0x21, PageDown: 0x22,
};

/**
 * "Ctrl+K" → the number Rust swallows. Letters and digits only with Ctrl or
 * Alt: alone they are what the user types into their own window, and the
 * island never takes those. null for anything this table does not know.
 * `bare` lets a digit through alone: only the question card asks for that,
 * for its numbered options, while the pointer rests on it.
 */
export function comboCode(text: string, bare = false): number | null {
  const parts = text.split("+").map((p) => p.trim());
  const key = parts.pop() ?? "";
  let mods = 0;
  for (const m of parts) {
    if (m === "Ctrl") mods |= CTRL;
    else if (m === "Alt") mods |= ALT;
    else if (m === "Shift") mods |= SHIFT;
    else return null;
  }
  if (key in NAMED) return mods | NAMED[key];
  const typed = /^[a-z0-9]$/i.test(key);
  if (typed && (mods & (CTRL | ALT) || (bare && mods === 0 && /^[0-9]$/.test(key)))) return mods | key.toUpperCase().charCodeAt(0);
  return null;
}

/** The number Rust sent back → the key event the page's listeners are given. null for a number that is no key of ours. */
export function comboEvent(code: number): KeyboardEventInit | null {
  const vk = code & 0xffff;
  const mods = { ctrlKey: (code & CTRL) !== 0, altKey: (code & ALT) !== 0, shiftKey: (code & SHIFT) !== 0 };
  const name = Object.keys(NAMED).find((k) => NAMED[k] === vk);
  if (name) return { ...mods, key: name === "Space" ? " " : name, code: name };
  const ch = String.fromCharCode(vk);
  if (/[A-Z]/.test(ch)) return { ...mods, key: mods.shiftKey ? ch : ch.toLowerCase(), code: `Key${ch}` };
  if (/[0-9]/.test(ch)) return { ...mods, key: ch, code: `Digit${ch}` };
  return null;
}

/** The numbers of the combinations named, without the ones the table does not know and without repeats. */
export function comboCodes(names: readonly string[], bare = false): number[] {
  const codes = names.map((n) => comboCode(n, bare)).filter((c): c is number => c != null);
  return [...new Set(codes)].sort((a, b) => a - b);
}
