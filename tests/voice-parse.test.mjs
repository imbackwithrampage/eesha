// The voice parser (src/features/voice/parse): one phrase table against a fixture registry.

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  isCancel, isEnd, isNo, isYes, normalise, parse, readArg, resolveChoice, splitCommands,
} from "../src/features/voice/parse/index.ts";

const SECTIONS = ["music", "clipboard", "calendar", "timer", "shelf", "notes", "agents", "mixer", "settings"].map((id) => ({ id, say: [id] }));
const OUTPUTS = [
  { id: "spk", say: ["speakers", "realtek speakers"] },
  { id: "hp", say: ["headphones", "bluetooth headphones"] },
];
const section = { name: "section", kind: "choice", choices: () => SECTIONS, ask: "Which one?" };
const run = async () => {};
const act = (id, forms, args, safety = "run") => ({
  feature: id.split("_")[0],
  action: { id, tool: id, describe: id, forms, args, safety, run },
});
const REG = [
  act("timer_start", ["set a timer for {minutes} [called {label}]", "[start|set] [a] {minutes} timer", "[set a] timer for {minutes}", "start a timer"], [
    { name: "minutes", kind: "minutes", ask: "For how long?" }, { name: "label", kind: "text", optional: true },
  ]),
  act("media_pause", ["pause [the] [music|media|song|video]"]),
  act("media_play", ["play [the] [music|media|song]", "resume [the] [music|media]"]),
  act("media_next", ["skip [this] [song|track]", "next (song|track)"]),
  act("volume_set", ["set [the] volume to {percent}", "volume {percent}", "turn the volume (down|up) to {percent}", "put the volume at {percent}"], [
    { name: "percent", kind: "percent" },
  ]),
  act("island_open", ["open [the] {section}", "show [me] [my] [the] {section}"], [section]),
  act("reminder_add", ["remind me to {note}", "add a reminder to {note}"], [{ name: "note", kind: "text" }]),
  act("awake_keep", ["keep [the computer|it|my pc] awake for {minutes}"], [{ name: "minutes", kind: "minutes" }]),
  act("agents_status", ["what are my agents doing [right now]", "is claude done [yet]"]),
  act("mic_mute", ["mute [my] microphones"]),
  act("alarm_set", ["set an alarm for {time}", "wake me up at {time}"], [{ name: "time", kind: "time", ask: "At what time?" }]),
  act("audio_out", ["switch to {output}", "use [the] {output}"], [{ name: "output", kind: "choice", choices: () => OUTPUTS }]),
];
const cx = () => ({});
const P = (t, ctx) => parse(t, REG, cx, ctx);

async function check(text, tool, args = {}) {
  const r = await P(text);
  assert.ok(r, `"${text}" should parse`);
  assert.equal(r.entry.action.tool, tool, `"${text}" -> ${r.entry.action.tool}`);
  assert.deepEqual(r.args, args, `"${text}" args`);
  return r;
}

test("the spike's sentences", async () => {
  await check("set a timer for 12 minutes", "timer_start", { minutes: 12 });
  await check("start a 5 minute timer", "timer_start", { minutes: 5 });
  await check("pause the music", "media_pause");
  await check("skip this song", "media_next");
  await check("volume 30", "volume_set", { percent: 30 });
  await check("open the clipboard", "island_open", { section: "clipboard" });
  await check("show me my calendar", "island_open", { section: "calendar" });
  await check("remind me to call the dentist", "reminder_add", { note: "call the dentist" });
  await check("add a reminder to buy milk", "reminder_add", { note: "buy milk" });
  await check("what are my agents doing right now", "agents_status");
  await check("is Claude done yet", "agents_status");
  await check("keep awake for 45 minutes", "awake_keep", { minutes: 45 });
  await check("mute my microphones", "mic_mute");
  // Needle's job, not the parser's: the parser must stay out of them.
  for (const t of [
    "I have to leave in 25 minutes, can you time that for me", "it's too quiet in here, put the volume at 70 percent",
    "don't let the computer go to sleep for two hours", "tell me a joke", "what's the weather in Paris",
    "who won the football match yesterday", "send an email to Tana", "mets un minuteur de dix minutes", "thank you",
  ]) assert.equal(await P(t), null, t);
});

test("two commands in one sentence are split before any text slot", async () => {
  assert.deepEqual(splitCommands("pause the music and set the volume to 20"), ["pause the music", "set the volume to 20"]);
  assert.deepEqual(splitCommands("Pause the music. Set a timer for 5 minutes"), ["Pause the music", "Set a timer for 5 minutes"]);
  assert.deepEqual(splitCommands("remind me to buy milk and open the clipboard"), ["remind me to buy milk", "open the clipboard"]);
  assert.deepEqual(splitCommands("remind me to buy bread and butter"), ["remind me to buy bread and butter"]);
  assert.deepEqual(splitCommands("set a timer for 5 minutes"), ["set a timer for 5 minutes"]);
  assert.deepEqual(splitCommands("and 5 more minutes"), ["and 5 more minutes"]);
  const parts = splitCommands("Remind me to call Mom and open the clipboard");
  const first = await P(parts[0]);
  assert.deepEqual(first.args, { note: "call Mom" }, "original case, nothing of the second command");
  assert.equal((await P(parts[1])).entry.action.tool, "island_open");
});

test("homophones and the speech model's habits", async () => {
  await check("Said a timer for twelve minutes.", "timer_start", { minutes: 12 });
  await check("Sent a timer for 12 minutes", "timer_start", { minutes: 12 });
  await check("Senator a timer for twelve minutes", "timer_start", { minutes: 12 });
  await check("Both the music", "media_pause");
  await check("Both", "media_pause");
  await check("Turn the volume down to twenty per cent", "volume_set", { percent: 20 });
  await check("Turn the volume down to 20%.", "volume_set", { percent: 20 });
  const r = await check("Remind me to call the dentist to morrow morning", "reminder_add", { note: "call the dentist tomorrow morning" });
  assert.equal(r.missing.length, 0);
  assert.equal(normalise("Hey, could you please set a timer?"), "set a timer");
  assert.equal(normalise("Open the Clipboard, thanks"), "open the clipboard");
  assert.equal(normalise("Set an alarm for 5 P.M."), "set an alarm for 5 pm");
});

test("number words, minutes and times", async () => {
  const mins = (t, v) => check(`set a timer for ${t}`, "timer_start", { minutes: v });
  await mins("twenty five minutes", 25);
  await mins("one hundred and five minutes", 105);
  await mins("a couple minutes", 2);
  await mins("a minute", 1);
  await mins("90 seconds", 1.5);
  await mins("an hour", 60);
  await mins("an hour and a half", 90);
  await mins("one and a half hours", 90);
  await mins("half an hour", 30);
  await mins("a quarter of an hour", 15);
  await mins("2 h 15", 135);
  await mins("2 hours 15 minutes", 135);
  await mins("two hours and ten minutes", 130);
  await mins("12", 12);
  await check("volume fifty", "volume_set", { percent: 50 });
  await check("volume 100", "volume_set", { percent: 100 });
  assert.equal(await P("volume 150"), null);
  const t = async (s, v) => await check(`set an alarm for ${s}`, "alarm_set", { time: v });
  await t("5 pm", "17:00");
  await t("at 5:30 pm", "17:30");
  await t("17:30", "17:30");
  await t("seven am", "07:00");
  await t("noon", "12:00");
  await t("midnight", "00:00");
  await t("12 am", "00:00");
  await t("five thirty in the evening", "17:30");
  await t("half past six pm", "18:30");
  await t("quarter to 7 am", "06:45");
  await t("ten past nine pm", "21:10");
  await t("eight o clock in the morning", "08:00");
  await t("6 oh 5 am", "06:05");
  await check("wake me up at 6:45 am", "alarm_set", { time: "06:45" });
});

test("choices: alias, exact, whole word, near miss, ties", async () => {
  await check("switch to realtek speakers", "audio_out", { output: "spk" });
  await check("switch to the speakers", "audio_out", { output: "spk" });
  await check("switch to bluetooth headphones", "audio_out", { output: "hp" });
  await check("switch to headphone", "audio_out", { output: "hp" });
  await check("switch to bluetooth", "audio_out", { output: "hp" });
  await check("open the calender", "island_open", { section: "calendar" });
  assert.equal(await P("switch to the kitchen"), null);
  assert.equal(resolveChoice("set", SECTIONS), null);
  const near = [{ id: "a", say: ["mixer"] }, { id: "b", say: ["mixed"] }];
  assert.equal(resolveChoice("mixel", near), null, "two as near as each other");
  assert.equal(resolveChoice("mixer", near)?.id, "a");
  assert.equal(resolveChoice("the music", SECTIONS)?.id, "music");
});

test("missing arguments, ties and one-word misses", async () => {
  const r = await P("start a timer");
  assert.equal(r.entry.action.tool, "timer_start");
  assert.deepEqual(r.missing.map((a) => a.name), ["minutes"]);
  assert.equal(await P("set"), null);
  assert.equal(await P(""), null);
  assert.equal(await P("note something"), null);
  const dup = [REG[1], { ...REG[1], action: { ...REG[1].action, id: "other", tool: "other" } }];
  assert.equal(await parse("pause the music", dup, cx), null, "a tie between two actions");
});

test("follow-ups within 90 seconds", async () => {
  const last = await P("set a timer for 10 minutes");
  const now = Date.now();
  const f = await P("and 5 more minutes", { last, at: now });
  assert.equal(f.entry.action.tool, "timer_start");
  assert.equal(f.args.minutes, 15);
  assert.equal((await P("make it 40 minutes", { last, at: now })).args.minutes, 40);
  assert.equal((await P("20 instead", { last, at: now })).args.minutes, 20);
  assert.equal(await P("and 5 more minutes", { last, at: now - 91_000 }), null, "stale");
  assert.equal(await P("and 5 more minutes"), null, "no context");
  const vol = await P("volume 30");
  assert.equal((await P("and 10 more percent", { last: vol, at: now })).args.percent, 40);
  const out = await P("switch to the speakers");
  assert.equal((await P("and the headphones", { last: out, at: now })).args.output, "hp");
  // A command of its own wins over a follow-up.
  assert.equal((await P("pause the music", { last, at: now })).entry.action.tool, "media_pause");
});

test("reading the reply to a question", async () => {
  const [minutes, , , , , , , , , , alarm] = [REG[0].action.args[0], 0, 0, 0, 0, 0, 0, 0, 0, 0, REG[10].action.args[0]];
  assert.equal(await readArg(minutes, "for ten minutes", {}), 10);
  assert.equal(await readArg(minutes, "an hour and a half", {}), 90);
  assert.equal(await readArg(minutes, "banana", {}), null);
  assert.equal(await readArg(alarm, "at half past six pm", {}), "18:30");
  assert.equal(await readArg(section, "the clipboard", {}), "clipboard");
  assert.equal(await readArg(section, "the moon", {}), null);
  assert.equal(await readArg({ name: "t", kind: "text" }, " Buy Milk ", {}), "Buy Milk");
  assert.equal(await readArg({ name: "p", kind: "percent" }, "seventy five percent", {}), 75);
});

test("yes, no, cancel, end", () => {
  for (const t of ["yes", "Yes, please", "yeah", "yep sure", "go ahead", "Do it", "okay", "sounds good"]) assert.ok(isYes(t), t);
  for (const t of ["no", "Nope", "no thanks", "don't do it", "not now", "never"]) assert.ok(isNo(t), t);
  for (const t of ["cancel", "Cancel that", "never mind", "Nevermind.", "forget it"]) assert.ok(isCancel(t), t);
  for (const t of ["that's all", "That's it", "goodbye", "Bye", "thank you", "no, that's all", "I'm done", "nothing else"]) assert.ok(isEnd(t), t);
  for (const t of ["set a timer", "no", "ok", "stop", "yes and no please", "volume 30"]) assert.ok(!isEnd(t), `${t} is not an end`);
  assert.ok(!isYes("no") && !isNo("yes") && !isYes("set a timer") && !isNo("set a timer") && !isCancel("yes"));
  assert.ok(!isYes("do not do it"), "do not do it is a no");
  assert.ok(isNo("do not do it"));
});
