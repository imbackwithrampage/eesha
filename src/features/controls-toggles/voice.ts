// What can be said to the Controls (docs/VOICE.md §5). Everything is `run` but the Recycle Bin: that
// opens Controls and the click on its tile (with the count and size shown) is the user's.
// Brightness here is the built-in panel; a monitor is "monitor brightness" (displays-keepawake).

import { defineVoice, type VoiceAction } from "../../core/feature";

interface State { themeApp: boolean; hidden: boolean; icons: boolean | null }
interface Pad { active: number; tabs: { id: number; name: string; text: string }[] }

const clamp = (n: number) => Math.max(0, Math.min(100, n));

const brightness: VoiceAction = {
  id: "brightness",
  tool: "brightness_set",
  describe: "Set the screen brightness (the laptop panel) to a percent.",
  forms: ["set [the] [screen] brightness to {percent}", "[screen] brightness {percent}", "turn the brightness (down|up) to {percent}"],
  args: [{ name: "percent", kind: "percent", ask: "To what percent?" }],
  safety: "run",
  async run(a, v) {
    before = (await v.call<number | null>("brightness_get")) ?? null;
    if ((await v.call("brightness", { level: clamp(Number(a.percent)) / 100 })) === null) return { ok: false, say: "I couldn't change the brightness." };
    return { say: `Brightness ${clamp(Number(a.percent))} percent.` };
  },
  undo: async (_a, v) => {
    if (before !== null) await v.call("brightness", { level: before });
  },
};
let before: number | null = null;

/** Flips `what` when Windows is not already as wanted (`want` of the key `key`). */
const toggle = (
  id: string, tool: string, describe: string, forms: string[], what: string, key: keyof State, want: boolean, say: string, already: string,
): VoiceAction => ({
  id, tool, describe, forms, safety: "run",
  async run(_a, v) {
    const s = await v.call<State>("state");
    if (!s) return { ok: false, say: "I can't read that setting." };
    if (s[key] === null) return { ok: false, say: "Windows doesn't allow that here." };
    if (s[key] === want) return { say: already };
    if ((await v.call("toggle", { what })) === null) return { ok: false, say: "Windows didn't apply it." };
    return { say };
  },
  undo: async (_a, v) => void (await v.call("toggle", { what })),
});

const lock: VoiceAction = {
  id: "lock",
  tool: "pc_lock",
  describe: "Lock the computer.",
  forms: ["lock [the|my] (computer|pc|screen|laptop)", "lock up"],
  safety: "run",
  async run(_a, v) {
    if ((await v.call("lock")) === null) return { ok: false, say: "I couldn't lock it." };
    return { say: "Locking." };
  },
};

const displayOff: VoiceAction = {
  id: "display_off",
  tool: "display_off",
  describe: "Turn the screen off (the computer keeps running).",
  forms: ["turn [the] (screen|display|monitor) off", "turn off [the] (screen|display|monitor)", "(screen|display|monitor) off"],
  safety: "run",
  async run(_a, v) {
    if ((await v.call("display_off")) === null) return { ok: false, say: "I couldn't turn the screen off." };
    return { say: "Screen off." };
  },
};

let added: { text: string } | null = null;
const note: VoiceAction = {
  id: "note",
  tool: "note_add",
  describe: "Add a line to the current note of the scratchpad.",
  forms: ["add {text} to [my|the] [notes|note|scratchpad]", "take a note [that] {text}", "note down {text}", "jot down {text}"],
  args: [{ name: "text", kind: "text", ask: "What should I write?" }],
  safety: "run",
  async run(a, v) {
    // debt: if the Notes section is open its page keeps the old text and writes it back on the next edit.
    const pad = await v.call<Pad | null>("scratch_load");
    const text = String(a.text).trim();
    const cur = pad?.tabs.find((x) => x.id === pad.active) ?? pad?.tabs[0];
    if (!pad || !cur) return { ok: false, say: "I can't find your note." };
    const line = (cur.text && !cur.text.endsWith("\n") ? "\n" : "") + text + "\n";
    cur.text += line;
    if ((await v.call("scratch_save", { data: pad })) === null) return { ok: false, say: "I couldn't save the note." };
    added = { text: line };
    return { say: "Added to your note.", show: text };
  },
  undo: async (_a, v) => {
    const pad = await v.call<Pad | null>("scratch_load");
    const cur = pad?.tabs.find((x) => x.id === pad.active) ?? pad?.tabs[0];
    if (!pad || !cur || !added || !cur.text.endsWith(added.text)) return;
    cur.text = cur.text.slice(0, cur.text.length - added.text.length);
    await v.call("scratch_save", { data: pad });
  },
};

const bin: VoiceAction = {
  id: "bin",
  tool: "recycle_bin_open",
  describe: "Open Controls at the Recycle Bin so the user can empty it with a click.",
  forms: ["empty [the] (recycle bin|trash|bin)", "clean out [the] (recycle bin|trash)"],
  safety: "prepare",
  async run(_a, v) {
    v.open("controls");
    return { say: "Controls is open. Click Recycle Bin to empty it." };
  },
};

export default defineVoice([
  brightness,
  toggle("dark", "theme_dark", "Switch Windows to dark mode.", ["(turn on|switch to|enable) dark mode", "dark mode [on]"], "theme", "themeApp", true, "Dark mode.", "It is already dark."),
  toggle("light", "theme_light", "Switch Windows to light mode.", ["(turn on|switch to|enable) light mode", "dark mode off", "turn off dark mode"], "theme", "themeApp", false, "Light mode.", "It is already light."),
  toggle("hidden_show", "hidden_show", "Show hidden files in File Explorer.", ["show hidden files", "turn on hidden files"], "hidden", "hidden", true, "Hidden files are shown.", "They are already shown."),
  toggle("hidden_hide", "hidden_hide", "Hide hidden files in File Explorer.", ["hide hidden files", "turn off hidden files"], "hidden", "hidden", false, "Hidden files are hidden.", "They are already hidden."),
  toggle("icons_show", "icons_show", "Show the desktop icons.", ["show [the] desktop icons", "bring back [the] desktop icons"], "icons", "icons", true, "Desktop icons shown.", "They are already shown."),
  toggle("icons_hide", "icons_hide", "Hide the desktop icons.", ["hide [the] desktop icons"], "icons", "icons", false, "Desktop icons hidden.", "They are already hidden."),
  lock, displayOff, note, bin,
]);
