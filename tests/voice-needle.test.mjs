// The action model, page side (docs/VOICE.md §4.2; src/features/voice/brain/needle.ts): the registry
// as tools, the sentence's shortlist, grounding, the trigger, the decision.
//
// With the Rust probe it is one chain, without the app:
//   npm test                                   writes target/voice-dev/voice-needle/tools.json (the tools
//                                              the page builds from the registry below, per sentence)
//   cargo run -p notch-feat-voice --example voice-needle-probe
//                                              gives them to the model, writes answers.json beside it
//   npm test                                   puts those answers through the page's rules (the last test)

import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { LIMIT, choose, grounded, needleTools, pick, prepare, pushTools, shortlist } from "../src/features/voice/brain/needle.ts";

const DEV = new URL("../target/voice-dev/voice-needle/", import.meta.url);

// ── A registry: the spike's eight tools as actions, and fifty-one more in the manner of VOICE §5 ──

const entry = (tool, describe, forms, args = [], safety = "run") => ({
  feature: "fixture",
  action: { id: tool, tool, describe, forms, args, safety, run: async () => {} },
});
const said = (...ids) => ids.map((id) => (Array.isArray(id) ? { id: id[0], say: id } : { id, say: [id] }));
const arg = (name, kind, more = {}) => ({ name, kind, ...more });
const pickOne = (name, ...ids) => arg(name, "choice", { choices: () => said(...ids) });

const SPIKE = [
  entry("start_timer", "Start a countdown timer. Use for 'timer', 'time that', 'tell me in N minutes'.",
    ["set a timer for {minutes}", "start a {minutes} timer", "{minutes} timer", "time (that|this|it) [for me]", "tell me in {minutes}"],
    [arg("minutes", "minutes", { ask: "For how long?" }), arg("label", "text", { optional: true })]),
  entry("control_media", "Control the music or video that is playing.",
    ["(pause|play|resume|stop) [the|this] (music|song|video|track)", "(skip|next) [this|the] (song|track)", "previous (song|track)", "{action} the music"],
    [pickOne("action", ["play", "resume"], ["pause", "stop"], ["next", "skip"], ["previous", "go back", "last song"])]),
  entry("set_volume", "Set the system speaker volume to a percent.",
    ["set [the] volume to {percent}", "volume {percent}", "turn [the] volume (up|down) to {percent}"], [arg("percent", "percent")]),
  entry("open_section", "Open or show one panel of the island: music, clipboard, calendar, timer, shelf, notes, agents, mixer, settings.",
    ["open [the] {name}", "show [me] [the|my] {name}"],
    [pickOne("name", "music", "clipboard", "calendar", "timer", "shelf", "notes", "agents", "mixer", "settings")]),
  entry("add_reminder", "Add a reminder to the list.", ["remind me to {text}", "add a reminder to {text}"], [arg("text", "text")]),
  entry("get_agent_status", "Report what the AI coding agents (Claude Code, Codex, Cursor) are doing right now: working, waiting for approval, finished.",
    ["what are [my|the] agents doing", "is (claude|codex|cursor|the agent) (done|finished|working) [yet]"]),
  entry("keep_awake", "Prevent the computer from going to sleep (stay awake, caffeinate) for a number of minutes.",
    ["keep [the] (computer|pc) awake for {minutes}", "keep awake for {minutes}", "stay awake for {minutes}", "don't [let the computer] (sleep|go to sleep) for {minutes}"],
    [arg("minutes", "minutes")]),
  entry("mute_microphones", "Mute all microphones.", ["mute [my|the|all] (microphones|microphone|mics|mic)"]),
];

const MORE = [
  entry("stopwatch_start", "Start the stopwatch.", ["start [the] stopwatch"]),
  entry("stopwatch_stop", "Stop the stopwatch.", ["stop [the] stopwatch"]),
  entry("pomodoro_start", "Start a Pomodoro focus session.", ["start a pomodoro", "focus session"]),
  entry("timer_pause", "Pause the running timer.", ["pause [the] timer"]),
  entry("timer_resume", "Resume the paused timer.", ["resume [the] timer"]),
  entry("timer_cancel", "Cancel the running timer.", ["cancel [the] timer", "stop [the] timer"]),
  entry("timer_add", "Add minutes to the running timer.", ["add {minutes} to the timer", "{minutes} more"], [arg("minutes", "minutes")]),
  entry("timer_left", "Say how much time is left on the timer.", ["how much time is left", "time left"]),
  entry("media_shuffle", "Turn shuffle on or off.", ["shuffle [the] (music|songs)", "turn shuffle (on|off)"]),
  entry("media_repeat", "Turn repeat on or off.", ["repeat [this] (song|track)", "turn repeat (on|off)"]),
  entry("media_now_playing", "Say which song is playing.", ["what is playing", "what song is this"]),
  entry("volume_up", "Make the sound a little louder.", ["volume up", "turn it up", "louder"]),
  entry("volume_down", "Make the sound a little quieter.", ["volume down", "turn it down", "quieter"]),
  entry("mute_sound", "Mute or unmute the speakers.", ["mute [the] (sound|speakers)", "unmute"]),
  entry("app_volume", "Set the volume of one app.", ["set {app} volume to {percent}"], [arg("app", "text"), arg("percent", "percent")]),
  entry("app_mute", "Mute one app.", ["mute {app}"], [arg("app", "text")]),
  entry("next_output", "Switch the sound to the next output device.", ["next output", "switch [the] (output|speakers)"]),
  entry("set_output", "Switch the sound to a named output device.", ["switch [the] sound to {name}", "use {name}"], [arg("name", "text")]),
  entry("set_brightness", "Set the screen brightness to a percent.", ["set [the] brightness to {percent}", "brightness {percent}"], [arg("percent", "percent")]),
  entry("dark_mode", "Switch Windows between dark and light mode.", ["{mode} mode", "switch to {mode} mode"], [pickOne("mode", "dark", "light")]),
  entry("hidden_files", "Show or hide hidden files in Explorer.", ["(show|hide) hidden files"]),
  entry("desktop_icons", "Show or hide the desktop icons.", ["(show|hide) [the] desktop icons"]),
  entry("lock_pc", "Lock the PC.", ["lock [the] (pc|computer|screen)"]),
  entry("display_off", "Turn the display off.", ["turn [the] (display|screen) off", "screen off"]),
  entry("add_note", "Add a line to the quick note.", ["add {text} to my notes", "note {text}", "take a note {text}"], [arg("text", "text")]),
  entry("empty_bin", "Empty the Recycle Bin.", ["empty the recycle bin"], [], "prepare"),
  entry("keep_awake_stop", "Stop keeping the computer awake.", ["stop keeping [the] (computer|pc) awake", "let [the] (computer|pc) sleep"]),
  entry("keep_awake_extend", "Keep the computer awake for more minutes.", ["stay awake {minutes} more", "extend keep awake by {minutes}"], [arg("minutes", "minutes")]),
  entry("tick_reminder", "Mark a reminder as done.", ["tick {text}", "mark {text} [as] done"], [arg("text", "text")]),
  entry("next_event", "Say what the next calendar event is.", ["what is next [on my calendar]", "next (event|meeting)"]),
  entry("today_events", "Say what is on the calendar today.", ["what is on today", "what do i have today"]),
  entry("delete_reminder", "Delete a reminder.", ["delete [the] reminder {text}"], [arg("text", "text")], "prepare"),
  entry("clean_link", "Remove tracking from the copied link.", ["clean [the] [copied] link"]),
  entry("paste_last", "Paste the last thing copied.", ["paste [the] last copy", "paste that"], [], "confirm"),
  entry("paste_plain", "Paste as plain text.", ["paste [as] plain [text]"], [], "confirm"),
  entry("clear_clipboard", "Clear the clipboard history.", ["clear [the] clipboard [history]"], [], "prepare"),
  entry("shelf_add", "Put a named file on the shelf.", ["put {name} on the shelf", "add {name} to the shelf"], [arg("name", "text")], "confirm"),
  entry("shelf_remove", "Take a file off the shelf.", ["take {name} off the shelf"], [arg("name", "text")], "prepare"),
  entry("shelf_share", "Share what is on the shelf.", ["share [the] shelf"], [], "prepare"),
  entry("open_tile", "Open a quick-access tile.", ["open [the] {name} tile"], [arg("name", "text")]),
  entry("battery_status", "Say the battery level.", ["how much battery [is left]", "battery (level|status)"]),
  entry("tokens_today", "Say how many tokens the agents used today.", ["how many tokens [did i use] today", "tokens today"]),
  entry("close_island", "Close the island.", ["close [the] island", "close it", "hide"]),
  entry("app_sounds", "Turn the app's sounds on or off.", ["turn [the] app sounds {state}", "app sounds {state}"], [pickOne("state", "on", "off")]),
  entry("agent_question", "Read what the agent is asking.", ["what is (it|the agent|claude) asking"]),
  entry("jump_terminal", "Bring the agent's terminal to the front.", ["(jump|go) to the terminal", "show [me] the terminal"]),
  entry("plan_usage", "Say how much of the plan is used.", ["how much of my plan is (used|left)", "plan usage"]),
  entry("pill_add", "Add a pill to the strip.", ["add [the] {name} pill"], [arg("name", "text")]),
  entry("pill_remove", "Remove a pill from the strip.", ["remove [the] {name} pill"], [arg("name", "text")]),
  entry("open_app", "Open a program from the Start Menu.", ["open {name}", "launch {name}", "start {name}"], [arg("name", "text")]),
  entry("stop_listening", "Stop listening for the wake phrase.", ["stop listening"]),
];

const REGISTRY = [...SPIKE, ...MORE];
const of = () => ({});
const tool = (name) => REGISTRY.find((e) => e.action.tool === name);

// The spike's 23 sentences with the calls it expected (the voice inventory, §13.3).
const SENTENCES = [
  ["set a timer for 12 minutes", [["start_timer", { minutes: 12 }]]],
  ["start a 5 minute timer", [["start_timer", { minutes: 5 }]]],
  ["I have to leave in 25 minutes, can you time that for me", [["start_timer", { minutes: 25 }]]],
  ["pause the music", [["control_media", { action: "pause" }]]],
  ["skip this song", [["control_media", { action: "next" }]]],
  ["it's too quiet in here, put the volume at 70 percent", [["set_volume", { percent: 70 }]]],
  ["volume 30", [["set_volume", { percent: 30 }]]],
  ["open the clipboard", [["open_section", { name: "clipboard" }]]],
  ["show me my calendar", [["open_section", { name: "calendar" }]]],
  ["remind me to call the dentist", [["add_reminder", { text: "call the dentist" }]]],
  ["add a reminder to buy milk", [["add_reminder", { text: "buy milk" }]]],
  ["what are my agents doing right now", [["get_agent_status", {}]]],
  ["is Claude done yet", [["get_agent_status", {}]]],
  ["don't let the computer go to sleep for two hours", [["keep_awake", { minutes: 120 }]]],
  ["keep awake for 45 minutes", [["keep_awake", { minutes: 45 }]]],
  ["mute my microphones", [["mute_microphones", {}]]],
  ["pause the music and set the volume to 20", [["control_media", { action: "pause" }], ["set_volume", { percent: 20 }]]],
  ["mets un minuteur de dix minutes", [["start_timer", { minutes: 10 }]]],
  ["mets la musique en pause", [["control_media", { action: "pause" }]]],
  ["what's the weather in Paris", []],
  ["tell me a joke", []],
  ["who won the football match yesterday", []],
  ["send an email to Tana", []],
];

// Nothing may run on these.
const SMALL_TALK = [
  "tell me a joke", "I love this song", "uh huh", "Thank you.", "Both the music and turn the volume down to 20%.", "I am going to bed",
  "good morning", "that was a long meeting", "how are you", "never mind", "okay", "I think it is going to rain",
  "the timer went off twenty minutes ago", "my microphone sounds strange",
];

/** The model's answer as Rust hands it over. */
const chosen = (calls, confidence = 0.8, suppressed = false) => ({
  tool: calls[0]?.[0] ?? "",
  args: calls[0]?.[1] ?? {},
  confidence,
  suppressed,
  more: calls.slice(1).map(([name, args]) => ({ tool: name, args })),
});

/** What a pick would run, as the sentences above write it. */
const ran = (p) => (p.kind === "act" ? [p.parsed, ...(p.more ?? [])].map((x) => [x.entry.action.tool, x.args]) : []);

// ── Grounding ─────────────────────────────────────────────────────────────────

test("a number is grounded as digits, in words, in hours or in seconds", () => {
  const timer = tool("start_timer");
  const ok = (text, minutes) => grounded(text, timer, { minutes });
  assert.ok(ok("set a timer for 12 minutes", 12));
  assert.ok(ok("set a timer for twelve minutes", 12));
  assert.ok(ok("a timer for one hundred and twenty five minutes", 125));
  assert.ok(ok("timer for twenty five minutes", 25));
  assert.ok(ok("don't let the computer go to sleep for two hours", 120));
  assert.ok(ok("timer for an hour and a half", 90));
  assert.ok(ok("timer for half an hour", 30));
  assert.ok(ok("timer for 2 hours 15", 135));
  assert.ok(ok("timer for 90 seconds", 1.5));
  assert.ok(ok("volume at 20%", 20));
  assert.ok(!ok("I am going to bed", 600), "a length nobody said");
  assert.ok(!ok("set a timer for five twenty", 25), "two numbers are not their sum");
  assert.ok(!ok("set a timer", 1), "the article is not a one");
  assert.ok(!ok("set a timer for 12 minutes", 120), "no hour was said");
});

test("a choice is grounded by a way of saying it, free text as a part of the sentence, a time by its hour", () => {
  const media = tool("control_media");
  assert.ok(grounded("skip this song", media, { action: "next" }, { action: ["next", "skip"] }));
  assert.ok(!grounded("I love this song", media, { action: "play" }, { action: ["play", "resume"] }));
  assert.ok(grounded("open the clipboard", tool("open_section"), { name: "clipboard" }));
  assert.ok(grounded("Remind me to call the dentist, please.", tool("add_reminder"), { text: "call the dentist" }));
  assert.ok(!grounded("remind me to call the dentist", tool("add_reminder"), { text: "call the doctor" }));
  assert.ok(!grounded("remind me to recall the dentistry", tool("add_reminder"), { text: "call the dentist" }), "whole words only");
  assert.ok(!grounded("pause the music", media, { volume: 3 }), "an argument the action does not have");
  const at = entry("remind_at", "Remind at a time.", ["remind me at {time}"], [arg("time", "time")]);
  assert.ok(grounded("remind me at 5 pm", at, { time: "17:00" }));
  assert.ok(grounded("remind me at half past 5", at, { time: "17:30" }));
  assert.ok(grounded("remind me at noon", at, { time: "12:00" }));
  assert.ok(!grounded("remind me later", at, { time: "17:00" }));
});

// ── The decision ──────────────────────────────────────────────────────────────

test("the spike's wrong calls run nothing", async () => {
  // What the model answered in the spike and in the probe (confidence as measured), with the reason each is stopped.
  const wrong = [
    ["tell me a joke", [["control_media", { action: "play" }]], 0.21, "miss"],
    ["I love this song", [["control_media", { action: "play" }]], 0.5, "miss"], // above the floor: grounding alone stops it
    ["uh huh", [["control_media", { action: "play" }]], 0.12, "miss"],
    ["Thank you.", [["control_media", { action: "pause" }]], 0.34, "miss"],
    ["Both the music and turn the volume down to 20%.", [["control_media", { action: "play" }], ["set_volume", { percent: 20 }]], 0.33, "miss"],
    ["what's the weather in Paris", [["open_section", { name: "clipboard" }]], 0.9, "question"],
    ["I am going to bed", [["start_timer", { minutes: 600 }]], 0.9, "miss"],
    ["good night", [["mute_microphones", {}]], 0.9, "miss"], // nothing to ground: the trigger stops it
    ["I like my microphones", [["mute_microphones", {}]], 0.9, "miss"], // a word of the action, not its verb
    ["I will be there in 20 minutes", [["start_timer", { minutes: 20 }]], 0.9, "miss"], // said, but no word of the action
    ["set a timer for 12 minutes", [["made_up", {}]], 0.9, "miss"],
  ];
  for (const [text, calls, confidence, kind] of wrong) {
    assert.deepEqual(await pick(text, chosen(calls, confidence), SPIKE, of), { kind }, text);
  }
});

test("every call the spike expected passes, whatever the model's confidence above the floor", async () => {
  for (const [text, calls] of SENTENCES) {
    const got = await pick(text, chosen(calls, 0.31), SPIKE, of);
    if (text.startsWith("mets un")) assert.deepEqual(got, { kind: "miss" }, "French numbers are not read (English only)");
    else if (calls.length === 0) assert.deepEqual(got, { kind: "question" }, text);
    else assert.deepEqual(ran(got), calls, text);
  }
});

test("held back, unsure or unasked: nothing runs, and where the sentence goes", async () => {
  const timer = [["start_timer", { minutes: 12 }]];
  const text = "set a timer for 12 minutes";
  assert.deepEqual(await pick(text, chosen(timer, 0.29), SPIKE, of), { kind: "miss" }, "under 0.30");
  assert.deepEqual(await pick(text, chosen(timer, 0.9, true), SPIKE, of), { kind: "miss" }, "held back by the engine");
  assert.deepEqual(await pick("can you set a timer for 12 minutes", chosen(timer, 0.1), SPIKE, of), { kind: "question" }, "a failed question is the text model's");
  assert.deepEqual(await pick(text, null, SPIKE, of), { kind: "none" }, "Rust did not answer");
  assert.deepEqual(await pick("thank you", chosen([["no_action", {}]], 0.9), SPIKE, of), { kind: "question" }, "not a command");
  assert.deepEqual(await pick("thank you", chosen([], 0.9), SPIKE, of), { kind: "question" });
});

test("a detail nobody said is asked for, and the class of an action stays the runner's", async () => {
  const noLength = await pick("set a timer", chosen([["start_timer", {}]]), SPIKE, of);
  assert.equal(noLength.kind, "act");
  assert.deepEqual(noLength.parsed.missing.map((a) => a.name), ["minutes"], "the runner asks 'For how long?'");
  assert.deepEqual(noLength.parsed.args, {});
  for (const [text, name, safety] of [["paste the last copy", "paste_last", "confirm"], ["empty the recycle bin", "empty_bin", "prepare"]]) {
    const got = await pick(text, chosen([[name, {}]]), REGISTRY, of);
    assert.equal(got.kind, "act");
    assert.equal(got.parsed.entry.action.safety, safety, "handed on with its class: the runner asks or prepares");
  }
});

test("a choice comes back as its id", async () => {
  const outputs = entry("set_output", "Switch the sound.", ["switch [the] sound to {name}"], [
    arg("name", "choice", { choices: async () => [{ id: "{0.0.0}.{a1}", say: ["Speakers", "the speakers"] }, { id: "{0.0.0}.{b2}", say: ["Headphones"] }] }),
  ]);
  const [made] = await needleTools([outputs], of);
  assert.deepEqual(made.parameters.properties.name, { type: "string", enum: ["speakers", "headphones"] });
  const got = await pick("switch the sound to the speakers", chosen([["set_output", { name: "speakers" }]]), [outputs], of);
  assert.deepEqual(ran(got), [["set_output", { name: "{0.0.0}.{a1}" }]]);
  assert.deepEqual(await pick("switch the sound", chosen([["set_output", { name: "headphones" }]]), [outputs], of), { kind: "miss" }, "not said");
});

// ── Tools and the shortlist ───────────────────────────────────────────────────

test("an action becomes a tool: its line, two ways of saying it, its arguments", async () => {
  const tools = await needleTools(SPIKE, of);
  assert.deepEqual(tools.map((t) => t.name), [...SPIKE.map((e) => e.action.tool), "no_action"]);
  assert.deepEqual(tools[0], {
    name: "start_timer",
    description: "Start a countdown timer. Use for 'timer', 'time that', 'tell me in N minutes'. Said as 'set a timer for 10 minutes' or 'start a 10 minutes timer'.",
    parameters: { type: "object", properties: { minutes: { type: "integer", description: "length in minutes" }, label: { type: "string" } }, required: ["minutes"] },
  });
  assert.deepEqual(tools[1].parameters, { type: "object", properties: { action: { type: "string", enum: ["play", "pause", "next", "previous"] } }, required: ["action"] });
  // The model follows these two to the letter: without "skip this song" among them it answers that sentence with "pause".
  assert.match(tools[1].description, /Said as 'pause the music' or 'skip this song'\.$/);
});

test("a long registry is cut to the actions that fit the sentence best", async () => {
  assert.equal(REGISTRY.length, 59);
  assert.deepEqual(await shortlist("anything at all", SPIKE, of), SPIKE, "a short one goes whole: one list, one server");
  const names = async (text) => (await shortlist(text, REGISTRY, of)).map((e) => e.action.tool);
  for (const [text, calls] of SENTENCES) {
    const short = await names(text);
    assert.ok(short.length <= LIMIT, text);
    // English only: the French timer has no word of any action.
    for (const [name] of text.startsWith("mets un") ? [] : calls) assert.ok(short.includes(name), `${text}: ${name} is among ${short.join(", ")}`);
  }
  assert.deepEqual(await names("skip this song"), ["control_media"], "its verb and a choice of its, against a neighbour's one word");
  assert.deepEqual(await names("open the clipboard"), ["open_section"], "the choice that is named counts");
  assert.deepEqual(await names("keep awake for 45 minutes"), ["keep_awake"]);
  assert.deepEqual(await names("pause the music and set the volume to 20"), ["control_media", "set_volume"], "each command's best");
  assert.deepEqual(await names("I am 30 years old"), [], "a number alone is nobody's");
  assert.deepEqual(await names("uh huh"), []);
});

test("choose hands the sentence's tools to Rust, asks, and decides", async () => {
  const calls = [];
  const cx = (answer) => ({ call: async (method, args) => (calls.push([method, args]), method === "needle.tools" ? {} : answer) });
  const got = await choose(cx(chosen([["set_volume", { percent: 30 }]], 0.9)), "volume 30", REGISTRY, of);
  assert.deepEqual(ran(got), [["set_volume", { percent: 30 }]]);
  assert.deepEqual(calls.map(([method]) => method), ["needle.tools", "needle.choose"]);
  assert.deepEqual(calls[0][1].tools.map((t) => t.name), ["set_volume", "no_action"]);
  assert.deepEqual([calls[0][1].sentence, calls[0][1].warm], [true, false]);
  assert.deepEqual(calls[1][1], { text: "volume 30" });

  assert.deepEqual(await choose(cx(null), "volume 30", REGISTRY, of), { kind: "none" }, "no answer in time: the turn goes on");
  assert.deepEqual(await choose({ call: async () => null }, "volume 30", REGISTRY, of), { kind: "none" }, "switched off");
  calls.length = 0;
  assert.deepEqual(await choose(cx(null), "uh huh", REGISTRY, of), { kind: "question" });
  assert.deepEqual(calls, [], "no action has a word of it: the model is not asked");

  await pushTools(cx(null), REGISTRY, of);
  assert.deepEqual(calls, [], "a long registry is not handed over whole");
  await pushTools(cx(null), SPIKE, of);
  await prepare(cx(null), "volume 30", REGISTRY, of);
  const pushed = calls.map(([method, args]) => [method, args.tools.length, args.sentence, args.warm]);
  assert.deepEqual(pushed, [["needle.tools", 9, false, false], ["needle.tools", 2, true, true]], "the registry once; a sentence's tools with a start");
});

// ── With the probe ────────────────────────────────────────────────────────────

test("the tools the page builds, sentence by sentence, for the probe", async () => {
  const rows = [];
  for (const [text, expected] of [...SENTENCES, ...SMALL_TALK.map((text) => [text, []])]) {
    const short = await shortlist(text, REGISTRY, of);
    rows.push({ text, expected, tools: short.length ? await needleTools(short, of) : null });
  }
  mkdirSync(DEV, { recursive: true });
  writeFileSync(new URL("tools.json", DEV), JSON.stringify(rows, null, 1));
});

test("what the model answered in the probe, through the page's rules", async (t) => {
  const file = new URL("answers.json", DEV);
  if (!existsSync(file)) return t.skip("no answers.json: run the example voice-needle-probe");
  const answers = JSON.parse(readFileSync(file, "utf8"));
  // `spike`: the spike's nine tools, so its eight actions whole. `page`: the 59 actions, a list per sentence.
  // `choice` null: the page had no tool for the sentence and the model was not asked.
  const judge = async (rows, entries) => {
    const out = [];
    for (const { text, expected, choice } of rows) {
      const got = choice ? ran(await pick(text, choice, await shortlist(text, entries, of), of)) : [];
      const called = !!choice && !choice.suppressed && choice.tool !== "" && choice.tool !== "no_action";
      out.push({ text, got, called, ok: JSON.stringify(got) === JSON.stringify(expected) });
    }
    return out;
  };
  const sets = [
    ["spike", (answers.spike ?? []).concat(answers["small talk"] ?? []), SPIKE],
    ["page", answers.page ?? [], REGISTRY],
  ];
  for (const [name, rows, entries] of sets) {
    if (!rows.length) continue;
    const judged = await judge(rows, entries);
    const [sentences, talk] = [judged.slice(0, SENTENCES.length), judged.slice(SENTENCES.length)];
    const right = sentences.filter((row) => row.ok).length;
    for (const row of sentences.filter((row) => !row.ok)) t.diagnostic(`[${name}] wrong: ${row.text} -> ${JSON.stringify(row.got)}`);
    for (const row of talk.filter((row) => row.got.length)) t.diagnostic(`[${name}] small talk ran: ${row.text} -> ${JSON.stringify(row.got)}`);
    const wrong = judged.filter((row) => row.called && !row.ok);
    t.diagnostic(
      `[${name}] ${right} of ${sentences.length} sentences right after the page's rules; small talk: the model called a tool on ` +
        `${talk.filter((row) => row.called).length} of ${talk.length}, ${talk.filter((row) => row.got.length).length} got through; ` +
        `calls stopped by the rules: ${judged.filter((row) => row.called && !row.got.length).length}; wrong calls that ran: ${wrong.filter((row) => row.got.length).length}`,
    );
    assert.ok(right >= 21, `[${name}] ${right} of 23`);
  }
});
