// voice-actions-b: the voice actions of calendar, clipboard, shelf, island-extras, huds-power and
// agent-usage. Nothing runs for real: `feature_call` is a stand-in that records what it was asked.

import { test } from "node:test";
import assert from "node:assert/strict";
import { parse, resolveChoice } from "../src/features/voice/parse/index.ts";
import { internals } from "./tauri.mjs";

const IDS = ["calendar", "clipboard", "shelf", "island-extras", "huds-power", "agent-usage"];
const REG = [];
for (const id of IDS) for (const action of (await import(`../src/features/${id}/voice.ts`)).default) REG.push({ feature: id, action });

// The state the stand-in answers with.
const NOW = Date.now();
const world = {
  quick: [{ label: "Notepad", target: "notepad.exe" }, { label: "Downloads", target: "C:\\Users\\me\\Downloads" }],
  reminders: [{ id: 7, text: "buy milk", done: false }, { id: 8, text: "call mum", done: true }],
  events: [{ title: "Standup", start: NOW + 3600e3, end: NOW + 7200e3, allDay: false }],
  clip: [{ id: "a", at: 1 }, { id: "b", at: 3 }, { id: "c", at: 2 }],
  shelf: [{ id: "s1" }, { id: "s2" }],
};
internals.invoke = async (cmd, args) => (cmd === "feature_settings" && args.feature === "island-extras" ? { quick: world.quick } : null);

const calls = [];
const opened = [];
const answers = {
  state: () => ({ reminders: world.reminders }),
  events: () => ({ events: world.events }),
  add_reminder: () => ({}),
  toggle_reminder: () => ({}),
  list: (_a, feature) => (feature === "clipboard" ? { entries: world.clip } : world.shelf),
  paste: () => ({ pasted: true }),
  shortcut: () => null,
  "clean-clipboard": () => ({ removed: ["utm_source"] }),
  share: () => true,
  open: () => ({}),
  battery: () => ({ ac: false, percent: 64, charging: false, minutes: 150 }),
};
const cxOf = (entry) => ({
  call: async (method, args) => {
    calls.push({ feature: entry.feature, method, args });
    return answers[method]?.(args, entry.feature) ?? null;
  },
  t: (s) => s,
  open: (s) => opened.push(s),
});
const find = (tool) => REG.find((e) => e.action.tool === tool);
const reset = () => { calls.length = 0; opened.length = 0; };

async function say(text) {
  const r = await parse(text, REG, cxOf);
  assert.ok(r, `"${text}" should parse`);
  return r;
}
async function check(text, tool, args = {}) {
  const r = await say(text);
  assert.equal(r.entry.action.tool, tool, `"${text}" -> ${r.entry.action.tool}`);
  assert.deepEqual(r.args, args, `"${text}" args`);
  return r;
}
const runIt = (r) => { reset(); return r.entry.action.run(r.args, cxOf(r.entry)); };

test("every action is well formed", () => {
  const tools = new Set();
  for (const { action: a } of REG) {
    assert.ok(a.id && a.tool && a.describe && a.forms.length, a.tool);
    assert.ok(!tools.has(a.tool), `duplicate tool ${a.tool}`);
    tools.add(a.tool);
    assert.ok(["run", "confirm", "prepare"].includes(a.safety), a.tool);
    assert.equal(typeof a.run, "function");
    if (a.safety === "confirm") assert.equal(typeof a.confirm, "function", `${a.tool} needs its question`);
    for (const f of a.forms) for (const slot of f.match(/\{(\w+)\}/g) ?? []) assert.ok((a.args ?? []).some((g) => `{${g.name}}` === slot), `${a.tool}: ${slot}`);
  }
  assert.ok(REG.length === 16);
});

test("the classes of VOICE section 5", () => {
  const safety = (t) => find(t).action.safety;
  for (const t of ["reminder_add", "reminder_done", "calendar_next", "calendar_today", "clipboard_open", "clipboard_clean_link", "shelf_open", "quick_open", "battery_level", "agent_usage"]) assert.equal(safety(t), "run", t);
  for (const t of ["clipboard_paste", "clipboard_paste_plain", "shelf_share"]) assert.equal(safety(t), "confirm", t);
  for (const t of ["reminder_delete", "clipboard_clear", "shelf_remove"]) assert.equal(safety(t), "prepare", t);
});

test("phrases parse to the right action", async () => {
  await check("remind me to buy bread", "reminder_add", { text: "buy bread" });
  await check("add a reminder to call the bank", "reminder_add", { text: "call the bank" });
  await check("tick off buy milk", "reminder_done", { reminder: "7" });
  await check("what is next on my calendar", "calendar_next");
  await check("when is my next meeting", "calendar_next");
  await check("what do I have today", "calendar_today");
  await check("delete a reminder", "reminder_delete");
  await check("open the clipboard", "clipboard_open");
  await check("clean the copied link", "clipboard_clean_link");
  await check("paste the last copy", "clipboard_paste");
  await check("paste as plain text", "clipboard_paste_plain");
  await check("clear the clipboard history", "clipboard_clear");
  await check("open the shelf", "shelf_open");
  await check("share the latest item on the shelf", "shelf_share");
  await check("remove a file from the shelf", "shelf_remove");
  await check("open notepad from quick access", "quick_open", { tile: "0" });
  await check("open the downloads tile", "quick_open", { tile: "1" });
  await check("what is the battery level", "battery_level");
  await check("how much battery do I have", "battery_level");
  await check("how many tokens did I use today", "agent_usage");
});

test("a numbered paste carries its number", async () => {
  const r = await say("paste the copy number 2");
  assert.equal(r.entry.action.tool, "clipboard_paste");
  assert.equal(r.args.n, 2);
});

test("reading runs the method it should", async () => {
  let r = await runIt(await say("what is next on my calendar"));
  assert.equal(calls[0].method, "events");
  // The stand-in event is an hour from now: after 23:00 that is tomorrow.
  assert.match(r.say, /^Standup, (today|tomorrow) at /);
  r = await runIt(await say("what is the battery level"));
  assert.deepEqual(calls.map((c) => c.method), ["battery"]);
  assert.match(r.say, /64 percent.*2 hours 30 minutes left/);
  r = await runIt(await say("clean the copied link"));
  assert.deepEqual(calls.map((c) => c.method), ["clean-clipboard"]);
  assert.match(r.say, /Cleaned/);
  answers.state = () => ({ providers: [{ name: "Claude", today: { total: 1500000 }, week: { total: 20000000 } }] });
  r = await runIt(await say("how many tokens today"));
  assert.match(r.say, /1\.5 million tokens today, 20 million/);
  answers.state = () => ({ reminders: world.reminders });
});

test("reminders: add, tick (only an open one) and delete only prepares", async () => {
  await runIt(await say("remind me to buy bread"));
  assert.deepEqual(calls, [{ feature: "calendar", method: "add_reminder", args: { text: "buy bread" } }]);
  await runIt(await say("tick off buy milk"));
  assert.deepEqual(calls.map((c) => [c.method, c.args]), [["state", undefined], ["toggle_reminder", { id: 7 }]]);
  // a reminder already done is not one to tick again
  const r = await find("reminder_done").action.run({ reminder: "8" }, cxOf(find("reminder_done")));
  assert.equal(r.ok, false);
  await runIt(await say("delete a reminder"));
  assert.deepEqual(opened, ["reminders"]);
  assert.equal(calls.length, 0, "deleting calls nothing");
});

test("a paste picks the right copy and is a confirm", async () => {
  const r = await say("paste the last copy");
  assert.equal(r.entry.action.safety, "confirm");
  assert.match(r.entry.action.confirm(r.args), /window in front/);
  const reply = await runIt(r);
  assert.deepEqual(calls.map((c) => [c.method, c.args]), [["list", undefined], ["paste", { id: "b" }]]);
  assert.equal(reply.say, "Pasted.");
  await find("clipboard_paste").action.run({ n: 3 }, cxOf(find("clipboard_paste")));
  assert.deepEqual(calls.at(-1).args, { id: "a" }, "the third most recent");
  reset();
  const none = await find("clipboard_paste").action.run({ n: 9 }, cxOf(find("clipboard_paste")));
  assert.equal(none.ok, false);
  assert.ok(!calls.some((c) => c.method === "paste"), "no copy, nothing pasted");
  reset();
  await runIt(await say("paste as plain text"));
  assert.deepEqual(calls, [{ feature: "clipboard", method: "shortcut", args: { id: "paste-plain" } }]);
});

test("clear, remove and delete never call a destructive method", async () => {
  for (const text of ["clear the clipboard history", "remove a file from the shelf", "clear the shelf", "delete a reminder"]) {
    await runIt(await say(text));
    assert.equal(calls.length, 0, text);
    assert.equal(opened.length, 1, text);
  }
  for (const { action } of REG) {
    const src = action.run.toString();
    assert.doesNotMatch(src, /"(remove|clear|delete_reminder|recycle|rename|remove_calendar)"/, action.tool);
  }
});

test("the shelf shares the newest item, after a yes", async () => {
  const r = await runIt(await say("share the latest item on the shelf"));
  assert.deepEqual(calls.map((c) => [c.method, c.args]), [["list", undefined], ["share", { id: "s2" }]]);
  assert.ok(r.say);
});

test("a quick-access tile opens the user's own target, found by its place in the list", async () => {
  let r = await runIt(await say("open notepad from quick access"));
  assert.deepEqual(calls, [{ feature: "island-extras", method: "open", args: { target: "notepad.exe" } }]);
  assert.ok(r.say);
  // anything that is not a place in the list is refused, whatever was heard
  for (const tile of ["cmd.exe /c calc", "C:\\Windows\\System32\\cmd.exe", "-1", "5", "", "0 && calc"]) {
    r = await find("quick_open").action.run({ tile }, cxOf(find("quick_open")));
    assert.equal(r.ok, false, tile);
  }
  assert.equal(calls.length, 1);
  // saying a program that is not a tile finds no tile
  const none = await parse("open calc.exe from quick access", REG, cxOf);
  assert.ok(!none || none.entry.action.tool !== "quick_open" || none.missing.length > 0);
});

test("open <app> only ever names a program found in the Start Menu list", () => {
  // The base action's choice list is the Start Menu's `files.apps`; what is heard resolves to an entry of it or to nothing.
  const apps = [{ id: "C:\\SM\\Notepad.lnk", say: ["Notepad"] }, { id: "C:\\SM\\Paint.lnk", say: ["Paint"] }];
  assert.equal(resolveChoice("notepad", apps)?.id, "C:\\SM\\Notepad.lnk");
  for (const heard of ["cmd.exe /c calc", "c colon backslash windows backslash system32 cmd exe", "powershell -command rm -rf", "https evil dot com", "notepad and then format"]) {
    const hit = resolveChoice(heard, apps);
    assert.ok(!hit || apps.includes(hit), heard);
    assert.notEqual(hit?.id, heard);
  }
  // and Rust's `files.open` takes only a path `files.apps` listed (crates/voice/examples/voice-files-probe.rs)
});
