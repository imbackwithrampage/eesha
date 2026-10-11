// What can be said to the mixer (docs/VOICE.md §5). Every action is `run`, with an undo where the
// old value is known. Unmuting the microphones is not here: voice may not (Ctrl+Alt+U does it).

import { defineVoice, type VoiceAction, type VoiceCx } from "../../core/feature";

interface Level { level: number; mute: boolean }
interface App { id: string; name: string; volume: number; mute: boolean }
interface Device { id: string; name: string; default: boolean; headphones: boolean }
interface State { out: Level | null; apps: App[]; outputs: Device[]; micsMuted: boolean }

const STEP = 10;
const state = (v: VoiceCx) => v.call<State>("state");
const pct = (n: number) => Math.round(n * 100);
const clamp = (n: number) => Math.max(0, Math.min(100, n));

// What undo puts back, per action (an action is undone only right after it ran).
const was = { volume: 0, mute: false, app: { id: "", volume: 0, mute: false }, output: "" };

async function setVolume(v: VoiceCx, percent: number) {
  const s = await state(v);
  if (!s?.out) return { ok: false, say: "I can't reach the volume." };
  was.volume = s.out.level;
  if ((await v.call("volume", { level: clamp(percent) / 100 })) === null) return { ok: false, say: "I couldn't change the volume." };
  return { say: `Volume ${clamp(percent)} percent.`, show: `Volume · ${clamp(percent)}%` };
}
const undoVolume = async (_a: unknown, v: VoiceCx) => void (await v.call("volume", { level: was.volume }));

const volumeSet: VoiceAction = {
  id: "volume",
  tool: "volume_set",
  describe: "Set the speaker volume to a percent.",
  forms: ["set [the] volume to {percent}", "volume {percent}", "turn the volume (down|up) to {percent}", "put the volume at {percent}"],
  args: [{ name: "percent", kind: "percent", ask: "To what percent?" }],
  safety: "run",
  run: (a, v) => setVolume(v, Number(a.percent)),
  undo: undoVolume,
};

const step = (up: boolean): VoiceAction => ({
  id: up ? "up" : "down",
  tool: up ? "volume_up" : "volume_down",
  describe: `Turn the speaker volume ${up ? "up" : "down"} a notch.`,
  forms: up
    ? ["volume up", "turn [the] volume up", "turn it up", "louder", "raise [the] volume"]
    : ["volume down", "turn [the] volume down", "turn it down", "quieter", "lower [the] volume"],
  safety: "run",
  async run(_a, v) {
    const s = await state(v);
    if (!s?.out) return { ok: false, say: "I can't reach the volume." };
    return setVolume(v, pct(s.out.level) + (up ? STEP : -STEP));
  },
  undo: undoVolume,
});

const muteOut = (mute: boolean): VoiceAction => ({
  id: mute ? "mute" : "unmute",
  tool: mute ? "volume_mute" : "volume_unmute",
  describe: `${mute ? "Mute" : "Unmute"} the speakers.`,
  forms: mute ? ["mute [the] (volume|sound|speakers|audio)", "silence"] : ["unmute [the] (volume|sound|speakers|audio)"],
  safety: "run",
  async run(_a, v) {
    if ((await v.call("mute", { mute })) === null) return { ok: false, say: `I couldn't ${mute ? "mute" : "unmute"} it.` };
    return { say: mute ? "Muted." : "Unmuted." };
  },
  undo: async (_a, v) => void (await v.call("mute", { mute: !mute })),
});

/** The apps that play now, each said by its name and its file name. */
async function appChoices(v: VoiceCx) {
  const apps = (await state(v))?.apps ?? [];
  return apps.map((a) => ({ id: a.id, say: [...new Set([a.name, a.id.replace(/\.exe$/, "")])] }));
}
const appArg = { name: "app", kind: "choice", choices: appChoices, ask: "Which app?" } as const;

const appVolume: VoiceAction = {
  id: "appVolume",
  tool: "app_volume",
  describe: "Set the volume of one app to a percent.",
  forms: ["set [the] {app} volume to {percent}", "{app} volume {percent}", "turn {app} (down|up) to {percent}", "put {app} at {percent}"],
  args: [appArg, { name: "percent", kind: "percent", ask: "To what percent?" }],
  safety: "run",
  async run(a, v) {
    const app = (await state(v))?.apps.find((x) => x.id === a.app);
    if (!app) return { ok: false, say: "I can't find that app in the mixer." };
    was.app = { id: app.id, volume: app.volume, mute: app.mute };
    if ((await v.call("appVolume", { id: app.id, level: clamp(Number(a.percent)) / 100 })) === null) return { ok: false, say: "I couldn't change it." };
    return { say: `${app.name} at ${clamp(Number(a.percent))} percent.` };
  },
  undo: async (_a, v) => void (await v.call("appVolume", { id: was.app.id, level: was.app.volume })),
};

const appMute = (mute: boolean): VoiceAction => ({
  id: mute ? "appMute" : "appUnmute",
  tool: mute ? "app_mute" : "app_unmute",
  describe: `${mute ? "Mute" : "Unmute"} one app.`,
  forms: mute ? ["mute {app}"] : ["unmute {app}"],
  args: [appArg],
  safety: "run",
  async run(a, v) {
    const app = (await state(v))?.apps.find((x) => x.id === a.app);
    if (!app) return { ok: false, say: "I can't find that app in the mixer." };
    if ((await v.call("appMute", { id: app.id, mute })) === null) return { ok: false, say: "I couldn't change it." };
    was.app.id = app.id;
    return { say: `${app.name} ${mute ? "muted" : "unmuted"}.` };
  },
  undo: async (_a, v) => void (await v.call("appMute", { id: was.app.id, mute: !mute })),
});

const defaultOut = (s: State | null) => s?.outputs.find((d) => d.default)?.id ?? "";
const undoOutput = async (_a: unknown, v: VoiceCx) => {
  if (was.output) await v.call("output", { id: was.output });
};

const nextOutput: VoiceAction = {
  id: "nextOutput",
  tool: "audio_output_next",
  describe: "Switch the sound to the next output device.",
  forms: ["next (output|speakers|audio device)", "switch to the next (output|speakers|audio device)", "change the (output|speakers)"],
  safety: "run",
  async run(_a, v) {
    was.output = defaultOut(await state(v));
    const r = await v.call<{ name: string; ok: boolean }>("nextOutput");
    if (!r?.ok) return { ok: false, say: "I couldn't switch the output." };
    return { say: `Sound is on ${r.name}.`, show: r.name };
  },
  undo: undoOutput,
};

const output: VoiceAction = {
  id: "output",
  tool: "audio_output",
  describe: "Switch the sound to a named output device (speakers, headphones).",
  forms: ["switch [the] (output|sound) to {output}", "switch to {output}", "use [the] {output}", "play (sound|audio) (on|through) [the] {output}"],
  args: [{
    name: "output",
    kind: "choice",
    ask: "Which output?",
    async choices(v) {
      const outs = (await state(v))?.outputs ?? [];
      return outs.map((d) => {
        const plain = d.name.replace(/\s*\(.*\)\s*$/, "").trim();
        return { id: d.id, say: [...new Set([d.name, plain, d.headphones ? "headphones" : ""].filter(Boolean))] };
      });
    },
  }],
  safety: "run",
  async run(a, v) {
    const s = await state(v);
    const d = s?.outputs.find((x) => x.id === a.output);
    if (!d) return { ok: false, say: "I can't find that output." };
    was.output = defaultOut(s);
    if ((await v.call("output", { id: d.id })) === null) return { ok: false, say: "I couldn't switch the output." };
    return { say: `Sound is on ${d.name.replace(/\s*\(.*\)\s*$/, "")}.`, show: d.name };
  },
  undo: undoOutput,
};

const muteMics: VoiceAction = {
  id: "muteMics",
  tool: "mics_mute",
  describe: "Mute every microphone. Voice cannot unmute them again.",
  forms: ["mute (all|my|the) (microphones|mics|mic)", "mute (microphones|mics)"],
  safety: "run",
  async run(_a, v) {
    if ((await v.call("muteMics", { on: true })) === null) return { ok: false, say: "I couldn't mute the microphones." };
    return { say: "Microphones muted. I can no longer hear you. Press Control Alt U to unmute." };
  },
};

export default defineVoice([
  volumeSet, step(true), step(false), muteOut(true), muteOut(false),
  appVolume, appMute(true), appMute(false), nextOutput, output, muteMics,
]);
