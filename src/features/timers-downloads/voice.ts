// What can be said to the timer (docs/VOICE.md §5). Every action is `run`.
// Left out: "add minutes" (the clock has no such method).

import { defineVoice, type VoiceAction, type VoiceCx } from "../../core/feature";

interface Clock {
  mode: string;
  status: "idle" | "running" | "paused" | "waiting" | "finished";
  remainingMs: number;
  elapsedMs: number;
}

/** "1 minute 30 seconds", "45 seconds", "2 hours 5 minutes". */
function spoken(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor(s / 60) % 60;
  const sec = s % 60;
  const part = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;
  const out = [h && part(h, "hour"), m && part(m, "minute"), !h && sec && part(sec, "second")].filter(Boolean);
  return out.length ? out.join(" ") : "0 seconds";
}

const cancel = async (_a: unknown, v: VoiceCx) => void (await v.call("cancel"));

const timerStart: VoiceAction = {
  id: "start",
  tool: "timer_start",
  describe: "Start a countdown timer for a number of minutes.",
  forms: ["set a timer for {minutes} [called {label}]", "[start|set] [a] {minutes} timer", "[set a] timer for {minutes}", "start a timer"],
  args: [
    { name: "minutes", kind: "minutes", ask: "For how long?" },
    { name: "label", kind: "text", optional: true },
  ],
  safety: "run",
  async run(a, v) {
    const r = await v.call("start", { mode: "timer", minutes: a.minutes });
    if (r === null) return { ok: false, say: "I couldn't start the timer." };
    return { say: `Timer set for ${spoken(Number(a.minutes) * 60000)}.`, show: `Timer · ${spoken(Number(a.minutes) * 60000)}` };
  },
  undo: cancel,
};

const stopwatchStart: VoiceAction = {
  id: "stopwatch",
  tool: "stopwatch_start",
  describe: "Start the stopwatch.",
  forms: ["start [a|the] stopwatch", "stopwatch"],
  safety: "run",
  async run(_a, v) {
    if ((await v.call("start", { mode: "stopwatch" })) === null) return { ok: false, say: "I couldn't start the stopwatch." };
    return { say: "Stopwatch started." };
  },
  undo: cancel,
};

const pomodoroStart: VoiceAction = {
  id: "pomodoro",
  tool: "pomodoro_start",
  describe: "Start a Pomodoro focus session with the saved lengths.",
  forms: ["start [a] pomodoro [timer|session]", "pomodoro", "start [a] focus (timer|session)"],
  safety: "run",
  async run(_a, v) {
    if ((await v.call("start", { mode: "pomodoro" })) === null) return { ok: false, say: "I couldn't start the Pomodoro." };
    return { say: "Pomodoro started. Focus time." };
  },
  undo: cancel,
};

const pause: VoiceAction = {
  id: "pause",
  tool: "timer_pause",
  describe: "Pause the running timer, stopwatch or Pomodoro.",
  forms: ["pause [the] (timer|stopwatch|pomodoro)", "hold [the] timer"],
  safety: "run",
  async run(_a, v) {
    if ((await v.call("pause")) === null) return { ok: false, say: "I couldn't pause it." };
    return { say: "Paused." };
  },
  undo: async (_a, v) => void (await v.call("resume")),
};

const resume: VoiceAction = {
  id: "resume",
  tool: "timer_resume",
  describe: "Continue a paused timer, stopwatch or Pomodoro.",
  forms: ["resume [the] (timer|stopwatch|pomodoro)", "continue [the] (timer|stopwatch|pomodoro)", "unpause [the] timer"],
  safety: "run",
  async run(_a, v) {
    if ((await v.call("resume")) === null) return { ok: false, say: "I couldn't resume it." };
    return { say: "Resumed." };
  },
  undo: async (_a, v) => void (await v.call("pause")),
};

const stop: VoiceAction = {
  id: "cancel",
  tool: "timer_cancel",
  describe: "Cancel the running timer, stopwatch or Pomodoro.",
  forms: ["cancel [the] (timer|stopwatch|pomodoro)", "stop [the] (timer|stopwatch|pomodoro)", "reset [the] (timer|stopwatch)"],
  safety: "run",
  async run(_a, v) {
    if ((await v.call("cancel")) === null) return { ok: false, say: "I couldn't cancel it." };
    return { say: "Cancelled." };
  },
};

const left: VoiceAction = {
  id: "left",
  tool: "timer_left",
  describe: "Say how much time is left on the timer, or how long the stopwatch has run.",
  forms: ["how much time is left [on the timer]", "how long is left [on the timer]", "how long (is|has) [the] (timer|stopwatch) [been] (left|running)", "time left [on the timer]", "how long until the timer (ends|is done|goes off)"],
  safety: "run",
  async run(_a, v) {
    const s = (await v.call<{ clock: Clock }>("state"))?.clock;
    if (!s || s.status === "idle") return { say: "No timer is running." };
    if (s.status === "finished") return { say: "The timer has finished." };
    if (s.mode === "stopwatch") return { say: `The stopwatch is at ${spoken(s.elapsedMs)}.` };
    return { say: `${spoken(s.remainingMs)} left${s.status === "paused" ? ", paused" : ""}.` };
  },
};

export default defineVoice([timerStart, stopwatchStart, pomodoroStart, pause, resume, stop, left]);
