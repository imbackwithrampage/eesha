// What can be said to the music player (docs/VOICE.md §5). Every action is `run`.
// "Pause" calls `pause` (idempotent) instead of `playPause` after `state`: it can never start a song.

import { defineVoice, type VoiceAction, type VoiceCx } from "../../core/feature";

interface Snapshot {
  track: { title: string; artist: string; playing: boolean; shuffle: boolean; repeat: "off" | "track" | "list" } | null;
}

/** A transport action: calls `method`, says `say`, and `undo` takes it back when there is a way. */
const simple = (
  id: string, tool: string, describe: string, forms: string[], method: string, say: string, undo?: string,
): VoiceAction => ({
  id, tool, describe, forms, safety: "run",
  async run(_a, v) {
    if ((await v.call(method)) === null) return { ok: false, say: "I couldn't do that." };
    return { say };
  },
  undo: undo ? async (_a, v: VoiceCx) => void (await v.call(undo)) : undefined,
});

const shuffle = (on: boolean): VoiceAction => ({
  id: on ? "shuffle_on" : "shuffle_off",
  tool: on ? "media_shuffle_on" : "media_shuffle_off",
  describe: `Turn shuffle ${on ? "on" : "off"}.`,
  forms: on ? ["shuffle [the] [music|songs|playlist]", "turn shuffle on", "shuffle on"] : ["turn shuffle off", "shuffle off", "stop shuffling"],
  safety: "run",
  async run(_a, v) {
    if ((await v.call("shuffle", { on })) === null) return { ok: false, say: "I couldn't change shuffle." };
    return { say: on ? "Shuffle on." : "Shuffle off." };
  },
  undo: async (_a, v) => void (await v.call("shuffle", { on: !on })),
});

const MODES = [
  { id: "track", say: ["song", "track", "this song", "this track", "one", "the song", "the track"] },
  { id: "list", say: ["all", "playlist", "the playlist", "everything", "list", "the list", "all songs"] },
  { id: "off", say: ["off", "none", "nothing"] },
];

const repeat: VoiceAction = {
  id: "repeat",
  tool: "media_repeat",
  describe: "Set repeat to the current song, the whole playlist, or off.",
  forms: ["repeat {mode}", "set repeat to {mode}", "turn repeat {mode}", "loop {mode}"],
  args: [{ name: "mode", kind: "choice", choices: () => MODES, ask: "Repeat the song, the playlist, or off?" }],
  safety: "run",
  async run(a, v) {
    const before = (await v.call<Snapshot>("state"))?.track?.repeat ?? "off";
    if ((await v.call("repeat", { mode: a.mode })) === null) return { ok: false, say: "I couldn't change repeat." };
    last = before;
    return { say: a.mode === "off" ? "Repeat off." : a.mode === "track" ? "Repeating this song." : "Repeating the playlist." };
  },
  undo: async (_a, v) => void (await v.call("repeat", { mode: last })),
};
let last = "off";

const now: VoiceAction = {
  id: "now",
  tool: "media_now",
  describe: "Say what song is playing.",
  forms: ["what's playing [right now]", "what song is (this|playing)", "what is this song", "what am I listening to", "who sings this", "what's this song"],
  safety: "run",
  async run(_a, v) {
    const t = (await v.call<Snapshot>("state"))?.track;
    if (!t) return { say: "Nothing is playing." };
    const by = t.artist ? ` by ${t.artist}` : "";
    return { say: `${t.playing ? "" : "Paused: "}${t.title}${by}.`, show: t.title };
  },
};

export default defineVoice([
  simple("play", "media_play", "Play or resume the music.", ["play [the] [music|media|song]", "resume [the] [music|media|song]"], "play", "Playing.", "pause"),
  simple("pause", "media_pause", "Pause the music.", ["pause [the] [music|media|song|video]"], "pause", "Paused.", "play"),
  simple("next", "media_next", "Skip to the next song.", ["skip [this] [song|track]", "next (song|track)", "play [the] next (song|track)"], "next", "Next song."),
  simple("prev", "media_prev", "Go back to the previous song.", ["previous (song|track)", "play [the] previous (song|track)", "go back [a song]", "last (song|track)"], "prev", "Previous song.", "next"),
  shuffle(true),
  shuffle(false),
  repeat,
  simple("open", "media_open", "Open the music player's own window.", ["open [the] (music|media) player", "open [my] player"], "open", "Opening the player."),
  now,
]);
