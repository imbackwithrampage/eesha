// What can be said to the clipboard (docs/VOICE.md §5): open it, clean the copied link. A paste goes
// into the window in front, so it asks first; clearing the history only opens the clipboard.

import { defineVoice, type VoiceCx } from "../../core/feature";

interface Entry { id: string; at: number }

/** Newest first, whatever is pinned. */
const newest = async (v: VoiceCx): Promise<Entry[]> =>
  ((await v.call<{ entries: Entry[] }>("list"))?.entries ?? []).slice().sort((a, b) => b.at - a.at);

export default defineVoice([
  {
    id: "open",
    tool: "clipboard_open",
    describe: "Open the clipboard history.",
    forms: ["open [the] clipboard [history]", "show [me] [the] clipboard [history]"],
    safety: "run",
    run: async (_a, v) => {
      v.open("clipboard");
      return { say: "Here is the clipboard." };
    },
  },
  {
    id: "clean",
    tool: "clipboard_clean_link",
    describe: "Remove the tracking parts of the link that was just copied.",
    forms: ["clean [the] (copied|clipboard) link", "clean [the] link [i copied]", "clean up [the] (copied|clipboard) link"],
    safety: "run",
    run: async (_a, v) => {
      const r = await v.call<{ removed: string[] } | null>("clean-clipboard");
      if (!r) return { say: "There is no link on the clipboard.", ok: false };
      return { say: r.removed.length ? "Cleaned the link." : "The link was already clean." };
    },
  },
  {
    id: "paste",
    tool: "clipboard_paste",
    describe: "Paste the last copy, or the nth most recent one (1 is the last), into the window in front.",
    forms: ["paste [the] (last|latest) (copy|copied thing|clipboard)", "paste [the] (copy|clipboard item|clipboard entry) number {n}", "paste [the] {n} copy"],
    args: [{ name: "n", kind: "number", optional: true }],
    safety: "confirm",
    confirm: (a) => (a.n && Number(a.n) > 1 ? `Paste copy number ${a.n} into the window in front?` : "Paste your last copy into the window in front?"),
    run: async (a, v) => {
      const e = (await newest(v))[Math.max(1, Math.floor(Number(a.n) || 1)) - 1];
      if (!e) return { say: "The clipboard has no such copy.", ok: false };
      const r = await v.call<{ pasted: boolean; copied?: boolean }>("paste", { id: e.id });
      if (!r) return { say: "I couldn't paste it.", ok: false };
      return r.pasted ? { say: "Pasted." } : r.copied ? { say: "It is copied. Paste it yourself." } : { say: "I couldn't paste it, but it is copied.", ok: false };
    },
  },
  {
    id: "paste_plain",
    tool: "clipboard_paste_plain",
    describe: "Paste what is copied as plain text, without formatting, into the window in front.",
    forms: ["paste [it] as plain text", "paste [it] plain", "paste without formatting"],
    safety: "confirm",
    confirm: () => "Paste as plain text into the window in front?",
    run: async (_a, v) => {
      await v.call("shortcut", { id: "paste-plain" });
      return { say: "Pasting as plain text." };
    },
  },
  {
    id: "clear",
    tool: "clipboard_clear",
    describe: "Open the clipboard so the history can be cleared by a click.",
    forms: ["clear [the] clipboard [history]", "empty [the] clipboard [history]", "delete [the] clipboard history"],
    safety: "prepare",
    // Clearing is a click on the clipboard's own button; the runner refuses the method.
    run: async (_a, v) => {
      v.open("clipboard");
      return { say: "Here is the clipboard. Clear it with the button." };
    },
  },
]);
