// voice-actions-a: the voice actions of timers, media, mixer, controls and displays, against a
// stand-in for feature_call. Nothing real runs: no timer, volume, device or brightness is touched.

import { test } from "node:test";
import assert from "node:assert/strict";
import { parse } from "../src/features/voice/parse/index.ts";
import timers from "../src/features/timers-downloads/voice.ts";
import media from "../src/features/media/voice.ts";
import mixer from "../src/features/audio-mixer/voice.ts";
import controls from "../src/features/controls-toggles/voice.ts";
import displays from "../src/features/displays-keepawake/voice.ts";

const FEATURES = { "timers-downloads": timers, media, "audio-mixer": mixer, "controls-toggles": controls, "displays-keepawake": displays };
const ENTRIES = Object.entries(FEATURES).flatMap(([feature, list]) => list.map((action) => ({ feature, action })));
const run = async () => {};
// the base action that shares words with ours ("open the music"), as the voice feature declares it
const BASE = {
  feature: "base",
  action: {
    id: "island_open", tool: "island_open", describe: "", safety: "run", run,
    forms: ["open [the] {section}"],
    args: [{ name: "section", kind: "choice", choices: () => ["music", "mixer", "timer"].map((id) => ({ id, say: [id] })) }],
  },
};

const NOTE = { active: 1, tabs: [{ id: 1, name: "Note 1", text: "milk" }] };
const STATE = {
  clock: { mode: "timer", status: "running", remainingMs: 90000, elapsedMs: 0 },
  track: { title: "Blue", artist: "Ada", playing: true, repeat: "off" },
  out: { level: 0.4, mute: false },
  apps: [{ id: "spotify.exe", name: "Spotify", volume: 0.8, mute: false }],
  outputs: [
    { id: "spk", name: "Speakers (Realtek(R) Audio)", default: true, headphones: false },
    { id: "hp", name: "Headphones (USB)", default: false, headphones: true },
  ],
  themeApp: false, hidden: false, icons: true, active: true,
};
const DISPLAYS = [{ id: "d1", method: "wmi", level: 0.7 }];
const READS = ["state", "displays", "scratch_load", "brightness_get", "ui.open"];

/** A VoiceCx that records every call and answers from fixtures. */
function stand() {
  const log = [];
  const v = {
    t: (s) => s,
    open: (s) => log.push(["ui.open", s]),
    async call(method, args) {
      log.push([method, args]);
      if (method === "state") return STATE;
      if (method === "displays") return DISPLAYS;
      if (method === "scratch_load") return structuredClone(NOTE);
      if (method === "brightness_get") return 0.5;
      if (method === "nextOutput") return { name: "Headphones", ok: true };
      return {};
    },
  };
  return { v, log, writes: () => log.filter(([m]) => !READS.includes(m)) };
}

const find = (tool) => ENTRIES.find((e) => e.action.tool === tool).action;
const P = (text) => parse(text, [...ENTRIES, BASE], () => stand().v);

test("every action is well formed", () => {
  const tools = new Set();
  for (const { action: a } of ENTRIES) {
    assert.ok(a.id && a.tool && a.describe, a.tool);
    assert.ok(!tools.has(a.tool), `duplicate tool ${a.tool}`);
    tools.add(a.tool);
    assert.ok(a.forms.length && a.forms.every((f) => typeof f === "string" && f.trim()), a.tool);
    assert.ok(["run", "confirm", "prepare"].includes(a.safety), a.tool);
    assert.equal(typeof a.run, "function");
    for (const g of a.args ?? []) assert.ok(g.name && g.kind && (g.optional || g.ask), `${a.tool}.${g.name} needs an ask`);
    if (a.safety === "confirm") assert.equal(typeof a.confirm, "function");
  }
  assert.equal(find("recycle_bin_open").safety, "prepare");
  assert.equal(find("wallpaper_open").safety, "prepare");
});

test("every action runs against the stand-in and replies", async () => {
  const sample = { minutes: 5, percent: 30, mode: "list", app: "spotify.exe", output: "hp", text: "eggs" };
  for (const { action: a } of ENTRIES) {
    const { v } = stand();
    const r = await a.run(sample, v);
    assert.equal(typeof r?.say, "string", `${a.tool} should answer`);
    assert.notEqual(r.ok, false, `${a.tool}: ${r.say}`);
    if (a.undo) await a.undo(sample, v);
  }
});

async function parsed(text, tool, args = {}) {
  const r = await P(text);
  assert.ok(r, `"${text}" should parse`);
  assert.equal(r.entry.action.tool, tool, `"${text}" -> ${r.entry.action.tool}`);
  assert.deepEqual(r.args, args, `"${text}" args`);
  assert.deepEqual(r.missing, [], `"${text}" missing`);
}

test("phrases parse to the right action", async () => {
  await parsed("set a timer for 12 minutes", "timer_start", { minutes: 12 });
  await parsed("start a 5 minute timer", "timer_start", { minutes: 5 });
  await parsed("start the stopwatch", "stopwatch_start");
  await parsed("start a pomodoro", "pomodoro_start");
  await parsed("pause the timer", "timer_pause");
  await parsed("resume the timer", "timer_resume");
  await parsed("cancel the timer", "timer_cancel");
  await parsed("how much time is left", "timer_left");

  await parsed("play the music", "media_play");
  await parsed("pause the music", "media_pause");
  await parsed("skip this song", "media_next");
  await parsed("previous song", "media_prev");
  await parsed("shuffle the music", "media_shuffle_on");
  await parsed("turn shuffle off", "media_shuffle_off");
  await parsed("repeat this song", "media_repeat", { mode: "track" });
  await parsed("open the music player", "media_open");
  await parsed("what's playing", "media_now");

  await parsed("set the volume to 30", "volume_set", { percent: 30 });
  await parsed("volume 55", "volume_set", { percent: 55 });
  await parsed("turn it up", "volume_up");
  await parsed("volume down", "volume_down");
  await parsed("mute the sound", "volume_mute");
  await parsed("unmute the speakers", "volume_unmute");
  await parsed("set spotify volume to 20", "app_volume", { app: "spotify.exe", percent: 20 });
  await parsed("mute spotify", "app_mute", { app: "spotify.exe" });
  await parsed("switch to the headphones", "audio_output", { output: "hp" });
  await parsed("switch to the next output", "audio_output_next");
  await parsed("mute my microphones", "mics_mute");

  await parsed("set the screen brightness to 40", "brightness_set", { percent: 40 });
  await parsed("turn on dark mode", "theme_dark");
  await parsed("switch to light mode", "theme_light");
  await parsed("show hidden files", "hidden_show");
  await parsed("hide the desktop icons", "icons_hide");
  await parsed("lock the computer", "pc_lock");
  await parsed("turn off the screen", "display_off");
  await parsed("take a note buy eggs", "note_add", { text: "buy eggs" });
  await parsed("empty the recycle bin", "recycle_bin_open");

  await parsed("keep the computer awake for 30 minutes", "awake_keep", { minutes: 30 });
  await parsed("keep the computer awake", "awake_keep_open");
  await parsed("extend keep awake by 15 minutes", "awake_extend", { minutes: 15 });
  await parsed("stop keeping the computer awake", "awake_stop");
  await parsed("set the monitor brightness to 60", "display_brightness_set", { percent: 60 });
  await parsed("change the wallpaper", "wallpaper_open");
});

test("each run calls the feature method it should", async () => {
  const check = async (tool, args, expected) => {
    const s = stand();
    await find(tool).run(args, s.v);
    assert.deepEqual(s.writes(), expected, tool);
  };
  await check("timer_start", { minutes: 12 }, [["start", { mode: "timer", minutes: 12 }]]);
  await check("stopwatch_start", {}, [["start", { mode: "stopwatch" }]]);
  await check("pomodoro_start", {}, [["start", { mode: "pomodoro" }]]);
  await check("timer_pause", {}, [["pause", undefined]]);
  await check("timer_resume", {}, [["resume", undefined]]);
  await check("timer_cancel", {}, [["cancel", undefined]]);
  assert.match((await find("timer_left").run({}, stand().v)).say, /1 minute 30 seconds left/);

  await check("media_play", {}, [["play", undefined]]);
  await check("media_pause", {}, [["pause", undefined]]);
  await check("media_next", {}, [["next", undefined]]);
  await check("media_prev", {}, [["prev", undefined]]);
  await check("media_shuffle_on", {}, [["shuffle", { on: true }]]);
  await check("media_shuffle_off", {}, [["shuffle", { on: false }]]);
  await check("media_repeat", { mode: "list" }, [["repeat", { mode: "list" }]]);
  await check("media_open", {}, [["open", undefined]]);
  assert.equal((await find("media_now").run({}, stand().v)).say, "Blue by Ada.");

  await check("volume_set", { percent: 30 }, [["volume", { level: 0.3 }]]);
  await check("volume_up", {}, [["volume", { level: 0.5 }]]);
  await check("volume_down", {}, [["volume", { level: 0.3 }]]);
  await check("volume_mute", {}, [["mute", { mute: true }]]);
  await check("volume_unmute", {}, [["mute", { mute: false }]]);
  await check("app_volume", { app: "spotify.exe", percent: 20 }, [["appVolume", { id: "spotify.exe", level: 0.2 }]]);
  await check("app_mute", { app: "spotify.exe" }, [["appMute", { id: "spotify.exe", mute: true }]]);
  await check("audio_output_next", {}, [["nextOutput", undefined]]);
  await check("audio_output", { output: "hp" }, [["output", { id: "hp" }]]);
  await check("mics_mute", {}, [["muteMics", { on: true }]]);

  await check("brightness_set", { percent: 40 }, [["brightness", { level: 0.4 }]]);
  await check("theme_dark", {}, [["toggle", { what: "theme" }]]);
  await check("hidden_show", {}, [["toggle", { what: "hidden" }]]);
  await check("icons_hide", {}, [["toggle", { what: "icons" }]]);
  await check("theme_light", {}, []); // already light: nothing is flipped
  await check("pc_lock", {}, [["lock", undefined]]);
  await check("display_off", {}, [["display_off", undefined]]);
  await check("note_add", { text: "eggs" }, [["scratch_save", { data: { active: 1, tabs: [{ id: 1, name: "Note 1", text: "milk\neggs\n" }] } }]]);

  await check("awake_keep", { minutes: 30 }, [["start", { minutes: 30 }]]);
  await check("awake_keep_open", {}, [["start", { minutes: 0 }]]);
  await check("awake_extend", { minutes: 15 }, [["extend", { minutes: 15 }]]);
  await check("awake_stop", {}, [["stop", undefined]]);
  await check("display_brightness_set", { percent: 60 }, [["set", { id: "d1", level: 0.6 }]]);
});

test("prepare actions only open the card; undo puts things back", async () => {
  const bin = stand();
  await find("recycle_bin_open").run({}, bin.v);
  assert.deepEqual(bin.log, [["ui.open", "controls"]]);
  const wall = stand();
  await find("wallpaper_open").run({}, wall.v);
  assert.deepEqual(wall.log, [["ui.open", "wallpaper"]]);

  const vol = stand();
  await find("volume_set").run({ percent: 90 }, vol.v);
  await find("volume_set").undo({}, vol.v);
  assert.deepEqual(vol.writes(), [["volume", { level: 0.9 }], ["volume", { level: 0.4 }]]);
  const out = stand();
  await find("audio_output").run({ output: "hp" }, out.v);
  await find("audio_output").undo({}, out.v);
  assert.deepEqual(out.writes().at(-1), ["output", { id: "spk" }]);
});
