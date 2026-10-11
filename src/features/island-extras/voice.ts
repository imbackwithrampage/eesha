// What can be said to island-extras (docs/VOICE.md §5): open a quick-access tile by its name. The tile's
// target is read from the user's own settings at the moment of running, never from the heard words:
// the choice is the tile's place in the list and nothing else. Mirror ("open it with a click") is never.

import { Bridge } from "../../core/bridge";
import { defineVoice, type VoiceArg } from "../../core/feature";

interface Quick { label: string; target: string }

const tiles = async (): Promise<Quick[]> =>
  (((await Bridge.featureSettings("island-extras")) as { quick?: Quick[] } | null)?.quick ?? []).filter((q) => q.target);

const tile: VoiceArg = {
  name: "tile",
  kind: "choice",
  choices: async () => (await tiles()).map((q, i) => ({ id: String(i), say: [q.label || q.target] })),
  ask: "Which one?",
};

export default defineVoice([
  {
    id: "quick",
    tool: "quick_open",
    describe: "Open one of the user's quick-access tiles (a program, folder or link they set up) by its name.",
    forms: ["open {tile} from quick access", "quick access {tile}", "open [the] {tile} tile"],
    args: [tile],
    safety: "run",
    run: async (a, v) => {
      const q = /^\d+$/.test(String(a.tile)) ? (await tiles())[Number(a.tile)] : undefined;
      if (!q) return { say: "I don't have that tile.", ok: false };
      return (await v.call("open", { target: q.target })) ? { say: "Opening it." } : { say: "I couldn't open it.", ok: false };
    },
  },
]);
