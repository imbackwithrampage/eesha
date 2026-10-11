// What can be said to the shelf (docs/VOICE.md §5): open it, share its newest item (asks first), and
// removing, which only opens it. Nothing here opens a file of the shelf or changes one.

import { defineVoice } from "../../core/feature";

interface Item { id: string }

export default defineVoice([
  {
    id: "open",
    tool: "shelf_open",
    describe: "Open the shelf.",
    forms: ["open [the] shelf", "show [me] [the] shelf"],
    safety: "run",
    run: async (_a, v) => {
      v.open("shelf");
      return { say: "Here is the shelf." };
    },
  },
  {
    id: "share",
    tool: "shelf_share",
    describe: "Share the newest item on the shelf with the Windows share sheet.",
    forms: ["share [the] (newest|latest|last) [item|file] [on the shelf]", "share (it|that) from [the] shelf"],
    safety: "confirm",
    confirm: () => "Share the newest item on the shelf?",
    run: async (_a, v) => {
      const items = (await v.call<Item[]>("list")) ?? [];
      const last = items[items.length - 1];
      if (!last) return { say: "The shelf is empty.", ok: false };
      return (await v.call("share", { id: last.id })) ? { say: "Choose where to share it." } : { say: "I couldn't share it.", ok: false };
    },
  },
  {
    id: "remove",
    tool: "shelf_remove",
    describe: "Open the shelf so an item can be removed by a click.",
    forms: ["remove (something|an item|a file) from [the] shelf", "clear [the] shelf", "empty [the] shelf", "take (it|that) off [the] shelf"],
    safety: "prepare",
    // Removing is a click on the item; the runner refuses the method.
    run: async (_a, v) => {
      v.open("shelf");
      return { say: "Here is the shelf. Remove it with the button." };
    },
  },
]);
