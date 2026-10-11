// voice-mail: email by voice. Voice prepares an email and never sends one. Nothing runs for real:
// `feature_call` is a stand-in that FAILS THE TEST when `mail_send` is asked for, on every branch.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { internals } from "./tauri.mjs";
import { DENIED, voiceCx } from "../src/features/voice/runner.ts";
import { parse } from "../src/features/voice/parse/index.ts";
import { FILE_MS, literal, mail, openMail, prepareMail, spelled, template, toAddress, toSubject, waitForDrop } from "../src/features/voice/mail/index.ts";

// ── The stand-in for Rust ─────────────────────────────────────────────────────
const featureCalls = [];
internals.invoke = async (cmd, args) => {
  if (cmd !== "feature_call") return null;
  featureCalls.push(args);
  assert.notEqual(args.method, "mail_send", "voice must never reach mail_send");
  return args.method === "mail_open" ? {} : null;
};

// ── Stand-ins for the app ─────────────────────────────────────────────────────
/** replies: what the user says to each question, in order (null: silence). */
function rig(replies, { drop = null, written = null, opens = true } = {}) {
  const r = { asked: [], said: [], opened: [], dropMs: [] };
  r.deps = {
    ask: async (q, kind) => {
      if (q === "" && kind === "file") return new Promise(() => {}); // listening, nobody speaks
      r.asked.push([q, kind]);
      return replies.length ? replies.shift() : null;
    },
    say: async (t) => void r.said.push(t),
    drop: async (ms) => (r.dropMs.push(ms), drop),
    write: async () => written,
    open: async (d) => (r.opened.push(d), opens),
    t: (s) => s,
  };
  return r;
}
const questions = (r) => r.asked.map(([q]) => q);

test("it asks only for what is missing, in order: to whom, about what, what to say", async () => {
  const r = rig(["jane dot doe at example dot com", "yes", "the budget", "word for word, see you at noon"]);
  const out = await prepareMail({}, r.deps);
  assert.deepEqual(questions(r), [
    "Who is it for? Say the email address.",
    "I heard j a n e dot d o e at e x a m p l e dot com. Is that right?",
    "What is it about?",
    "What should it say?",
  ]);
  assert.deepEqual(r.asked.map(([, k]) => k), ["reply", "yesNo", "reply", "freeText"]);
  assert.deepEqual(r.opened, [{ to: "jane.doe@example.com", subject: "The budget", message: "See you at noon", path: "" }]);
  assert.match(out.say, /Press Send when you are ready/);
});

test("what was already said is not asked again", async () => {
  const r = rig(["yes"]);
  await prepareMail({ to: "bob at gmail dot com", subject: "Lunch", message: "word for word: Friday?" }, r.deps);
  assert.equal(r.asked.length, 1);
  assert.match(r.asked[0][0], /^I heard b o b at gmail dot com/);
  assert.deepEqual(r.opened, [{ to: "bob@gmail.com", subject: "Lunch", message: "Friday?", path: "" }]);
});

test("an address that was heard is spelled back, and a no is not taken; never guessed", async () => {
  const r = rig(["jon at example dot com", "no", "john at example dot com", "yes", "hi", "hello there"]);
  await prepareMail({}, r.deps);
  assert.equal(r.opened[0].to, "john@example.com");
  const none = rig(["blah blah", "still nothing", "mumble"]);
  const out = await prepareMail({}, none.deps);
  assert.equal(none.opened.length, 0);
  assert.equal(out.ok, false);
});

test("silence or a cancel anywhere opens nothing", async () => {
  for (const replies of [[], ["bob at x dot org", "yes"], ["bob at x dot org", "yes", "hi"]]) {
    const r = rig(replies);
    const out = await prepareMail({}, r.deps);
    assert.equal(r.opened.length, 0);
    assert.equal(out.ok, false);
  }
});

test("the message is written by the routed model, else from a template", async () => {
  const w = rig(["yes"], { written: '"Dear Sam,\n\nI would like to move our meeting."' });
  await prepareMail({ to: "sam at x dot org", subject: "Meeting", message: "move the meeting" }, w.deps);
  assert.equal(w.opened[0].message, "Dear Sam,\n\nI would like to move our meeting.");
  const t = rig(["yes"]);
  await prepareMail({ to: "sam at x dot org", subject: "Meeting", message: "move the meeting" }, t.deps);
  assert.equal(t.opened[0].message, "Hello,\n\nMove the meeting.");
});

test("a dropped file is attached when the user wants one; none arrives: open without", async () => {
  const yes = rig(["yes"], { drop: { name: "a.pdf", path: "C:\\inbox\\a.pdf" } });
  const out = await prepareMail({ to: "a at b dot co", subject: "S", message: "word for word: M", attach: "file" }, yes.deps);
  assert.equal(yes.opened[0].path, "C:\\inbox\\a.pdf");
  assert.deepEqual(yes.dropMs, [20_000]);
  assert.equal(FILE_MS, 20_000);
  assert.match(out.say, /Press Send/);
  const no = rig(["yes"], { drop: null });
  const late = await prepareMail({ to: "a at b dot co", subject: "S", message: "word for word: M", attach: "file" }, no.deps);
  assert.equal(no.opened[0].path, "");
  assert.match(late.say, /without one/);
  // And no file is waited for unless the user said they want one.
  const plain = rig(["yes"]);
  await prepareMail({ to: "a at b dot co", subject: "S", message: "word for word: M" }, plain.deps);
  assert.deepEqual(plain.dropMs, []);
});

test("a file dropped inside the window is taken, one after it is not", async () => {
  const mk = () => {
    let cur = { name: "old", path: "C:\\old" };
    const fns = new Set();
    return { source: { current: () => cur, subscribe: (f) => (fns.add(f), () => fns.delete(f)) }, set: (d) => ((cur = d), fns.forEach((f) => f())) };
  };
  const a = mk();
  const early = waitForDrop(80, a.source, 30);
  setTimeout(() => a.set({ name: "x", path: "C:\\drop\\x" }), 10);
  setTimeout(() => a.set({ name: "x", path: "C:\\inbox\\x" }), 25);
  assert.deepEqual(await early, { name: "x", path: "C:\\inbox\\x" });
  const b = mk();
  const late = waitForDrop(40, b.source, 30);
  setTimeout(() => b.set({ name: "y", path: "C:\\inbox\\y" }), 120);
  assert.equal(await late, null);
  // A file that was there before the wait began is not taken.
  const c = mk();
  assert.equal(await waitForDrop(30, c.source, 10), null);
});

test("a heard \"send it\" is never obeyed: it is answered with press Send", async () => {
  // before there is an address: said, and asked again
  const early = rig(["send it", "bob at x dot org", "yes", "hi", "hey"]);
  await prepareMail({}, early.deps);
  assert.match(early.said[0], /Press Send when you are ready/);
  assert.equal(early.opened[0].to, "bob@x.org");
  // after the address: the window opens with what there is, and says to press Send
  const mid = rig(["bob at x dot org", "yes", "send it"]);
  const out = await prepareMail({}, mid.deps);
  assert.match(out.say, /Press Send when you are ready/);
  assert.deepEqual(mid.opened, [{ to: "bob@x.org", subject: "", message: "", path: "" }]);
  // while the spelled-back address is confirmed
  const confirm = rig(["bob at x dot org", "send it", "bob at x dot org", "yes", "hi", "hey"]);
  await prepareMail({}, confirm.deps);
  assert.match(confirm.said[0], /Press Send/);
  assert.equal(confirm.opened.length, 1);
});

test("the pure parts", () => {
  assert.equal(toAddress("it is John dot Smith at Example dot com"), "john.smith@example.com");
  assert.equal(toAddress("j o h n at gmail dot com"), "john@gmail.com");
  assert.equal(toAddress("a dash b underscore c at x dot org"), "a-b_c@x.org");
  assert.equal(toAddress("hello there"), "");
  assert.equal(toAddress("john at"), "");
  assert.equal(spelled("jo-e@gmail.com"), "j o dash e at gmail dot com");
  assert.equal(toSubject("about the invoice."), "The invoice");
  assert.equal(literal("word for word, call me"), "Call me");
  assert.equal(literal("tell him I am late"), null);
  assert.equal(template("I am late."), "Hello,\n\nI am late.");
});

test("it is understood from the usual sentences", async () => {
  const reg = [{ feature: "voice", action: mail[0] }];
  const cx = () => ({ call: async () => null, t: (s) => s, open() {} });
  for (const s of ["write an email", "draft an email to bob at example dot com", "email bob at example dot com about lunch", "write an email with a file", "send an email"]) {
    const p = await parse(s, reg, cx);
    assert.equal(p?.entry.action.tool, "mail_prepare", s);
  }
  const p = await parse("write an email with an attachment", reg, cx);
  assert.equal(p?.args.attach, "file");
  assert.equal(mail[0].safety, "prepare");
});

test("no path of the code reaches mail_send", async () => {
  // The action as the app runs it: its real steps go to the (stand-in) feature_call, which fails on mail_send.
  featureCalls.length = 0;
  const v = { call: async () => null, t: (s) => s, open() {} };
  // `ask` needs a live turn: without one the action stops, and still never sends.
  assert.equal((await mail[0].run({ to: "a at b dot co", subject: "S", message: "word for word: M" }, v)).ok, false);
  assert.ok(DENIED.includes("mail_send"));
  assert.ok(featureCalls.every((c) => c.method === "mail_open"));
  // The real open step: exactly the fields gathered, to the stand-in feature_call, as mail_open and nothing else.
  featureCalls.length = 0;
  assert.equal(await openMail(v, { to: "a@b.co", subject: "S", message: "M", path: "C:\inbox\f.pdf" }), true);
  assert.equal(await openMail(v, { to: "a@b.co", subject: "S", message: "M", path: "" }), true);
  assert.deepEqual(featureCalls, [
    { feature: "chat-plus", method: "mail_open", args: { to: "a@b.co", subject: "S", message: "M", path: "C:\inbox\f.pdf" } },
    { feature: "chat-plus", method: "mail_open", args: { to: "a@b.co", subject: "S", message: "M" } },
  ]);
  // The source never names the method in code.
  const src = readFileSync(new URL("../src/features/voice/mail/index.ts", import.meta.url), "utf8");
  const code = src.split("\n").filter((l) => !/^\s*(\/\/|\/?\*)/.test(l)).join("\n");
  assert.equal(code.includes("mail_send"), false);
  // And the deny list in front of the open step refuses it should it ever be asked.
  const cx = voiceCx({ id: "chat-plus", call: (m, a) => internals.invoke("feature_call", { feature: "chat-plus", method: m, args: a }), t: (s) => s, island: { open() {} } });
  await assert.rejects(cx.call("mail_send", { to: "x@y.z" }));
  assert.ok(featureCalls.every((c) => c.method !== "mail_send"));
});
