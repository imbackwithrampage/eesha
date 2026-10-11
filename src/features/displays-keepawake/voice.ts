// What can be said to keep-awake and the displays (docs/VOICE.md §5). Everything is `run` but the
// wallpaper, which opens its section (picking and applying a picture is a click).

import { defineVoice, type VoiceAction, type VoiceCx } from "../../core/feature";

interface Session { active: boolean }
interface Display { id: string; method: string; level: number }

const minutesOf = (n: number) => Math.max(1, Math.round(n));
const said = (m: number) => (m % 60 === 0 && m >= 60 ? `${m / 60} hour${m === 60 ? "" : "s"}` : `${m} minute${m === 1 ? "" : "s"}`);
const stopIt = async (_a: unknown, v: VoiceCx) => void (await v.call("stop"));

const start: VoiceAction = {
  id: "start",
  tool: "awake_keep",
  describe: "Keep the computer awake for a number of minutes.",
  forms: ["keep [the computer|it|my pc] awake for {minutes}", "stay awake for {minutes}", "keep awake for {minutes}"],
  args: [{ name: "minutes", kind: "minutes", ask: "For how long?" }],
  safety: "run",
  async run(a, v) {
    const m = minutesOf(Number(a.minutes));
    if ((await v.call("start", { minutes: m })) === null) return { ok: false, say: "I couldn't keep it awake." };
    return { say: `Keeping the computer awake for ${said(m)}.`, show: `Awake · ${said(m)}` };
  },
  undo: stopIt,
};

const forever: VoiceAction = {
  id: "start_open",
  tool: "awake_keep_open",
  describe: "Keep the computer awake until the user says stop.",
  forms: ["keep [the computer|it|my pc] awake [until I (say stop|stop it)|until stopped|indefinitely]", "don't let [the computer|it|my pc] sleep"],
  safety: "run",
  async run(_a, v) {
    if ((await v.call("start", { minutes: 0 })) === null) return { ok: false, say: "I couldn't keep it awake." };
    return { say: "Keeping the computer awake until you say stop.", show: "Awake · until stopped" };
  },
  undo: stopIt,
};

const extend: VoiceAction = {
  id: "extend",
  tool: "awake_extend",
  describe: "Add minutes to the running keep-awake.",
  forms: ["extend [the] keep awake [by|for] {minutes}", "add {minutes} to [the] keep awake", "keep it awake {minutes} longer"],
  args: [{ name: "minutes", kind: "minutes", ask: "By how long?" }],
  safety: "run",
  async run(a, v) {
    const s = await v.call<Session>("state");
    if (!s?.active) return { ok: false, say: "Keep awake isn't on." };
    const m = minutesOf(Number(a.minutes));
    if ((await v.call("extend", { minutes: m })) === null) return { ok: false, say: "I couldn't extend it." };
    return { say: `Added ${said(m)}.` };
  },
};

const stop: VoiceAction = {
  id: "stop",
  tool: "awake_stop",
  describe: "Stop keeping the computer awake.",
  forms: ["stop keeping [the computer|it|my pc] awake", "let [the computer|it|my pc] sleep", "stop keep awake", "turn off keep awake"],
  safety: "run",
  async run(_a, v) {
    if ((await v.call("stop")) === null) return { ok: false, say: "I couldn't stop it." };
    return { say: "The computer can sleep again." };
  },
};

let before: { id: string; level: number } | null = null;
const brightness: VoiceAction = {
  id: "set",
  tool: "display_brightness_set",
  describe: "Set the brightness of the monitor (the first display that can be dimmed) to a percent.",
  forms: ["set [the] (monitor|display) brightness to {percent}", "(monitor|display) brightness {percent}", "dim [the] (monitor|display) to {percent}"],
  args: [{ name: "percent", kind: "percent", ask: "To what percent?" }],
  safety: "run",
  async run(a, v) {
    const d = (await v.call<Display[]>("displays"))?.find((x) => x.method !== "none");
    if (!d) return { ok: false, say: "I can't change the brightness of this display." };
    const level = Math.max(0, Math.min(100, Number(a.percent))) / 100;
    before = { id: d.id, level: d.level };
    if ((await v.call("set", { id: d.id, level })) === null) return { ok: false, say: "I couldn't change the brightness." };
    return { say: `Display brightness ${Math.round(level * 100)} percent.` };
  },
  undo: async (_a, v) => {
    if (before) await v.call("set", before);
  },
};

const wallpaper: VoiceAction = {
  id: "wallpaper",
  tool: "wallpaper_open",
  describe: "Open the wallpaper picker.",
  forms: ["(change|set) [the|my] wallpaper", "open [the] wallpaper", "show [me] [the] wallpapers"],
  safety: "prepare",
  async run(_a, v) {
    v.open("wallpaper");
    return { say: "Wallpaper is open. Pick a picture." };
  },
};

export default defineVoice([start, forever, extend, stop, brightness, wallpaper]);
