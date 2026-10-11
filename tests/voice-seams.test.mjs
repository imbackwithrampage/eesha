// The voice assistant's seams (docs/VOICE.md §5, §7.5, §7.6): how a feature's voice actions are
// collected (src/core/feature.ts), the rules behind `IslandApi.ear / shape / face`
// (src/island/voice.ts), and the names the modules of src/features/voice/ export, which the packages
// that fill them are built against.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { collectVoice, defineVoice } from "../src/core/feature.ts";
import { ACTIVITY_W, NOTCH_W, NOTICE_W } from "../src/core/layout.ts";
import { EAR_HEARD_MS, VOICE, earLook, faceEmote, mascotFace, sameShape, stripWidth, voiceMay } from "../src/island/voice.ts";

const action = (id) => ({ id, tool: `x_${id}`, describe: id, forms: [id], safety: "run", run: async () => {} });

test("a feature's voice actions are read only while it is on", async () => {
  let loaded = 0;
  const features = [
    { id: "timer", voice: async () => ({ default: defineVoice([action("start"), action("cancel")]) }) },
    { id: "plain" },
    { id: "off", voice: async () => (loaded++, { default: [action("never")] }) },
    { id: "media", voice: async () => ({ default: [action("next")] }) },
  ];
  const got = await collectVoice(features, (id) => id !== "off");
  assert.deepEqual(got.map((e) => `${e.feature}.${e.action.id}`), ["timer.start", "timer.cancel", "media.next"]);
  assert.equal(loaded, 0, "the voice.ts of a feature that is off is not even loaded");
  assert.deepEqual(await collectVoice(features, () => false), []);
});

test("a voice.ts that fails to load takes nothing else down", async (t) => {
  t.mock.method(console, "error", () => {});
  const features = [
    { id: "broken", voice: () => Promise.reject(new Error("no such chunk")) },
    { id: "media", voice: async () => ({ default: [action("next")] }) },
  ];
  assert.deepEqual((await collectVoice(features, () => true)).map((e) => e.feature), ["media"]);
});

test("ear, shape and face are the voice feature's, and only while it runs", () => {
  assert.equal(voiceMay(VOICE, () => true), true);
  assert.equal(voiceMay(VOICE, () => false), false);
  assert.equal(voiceMay("media", () => true), false);
  assert.equal(voiceMay("dictation", (id) => id === VOICE), false);
});

test("the ear bar is out, lit, or bright for a moment", () => {
  assert.equal(earLook(false), "off");
  assert.equal(earLook(false, true), "off", "a closed microphone shows nothing, whatever was heard");
  assert.equal(earLook(true), "on");
  assert.equal(earLook(true, true), "heard");
  assert.equal(EAR_HEARD_MS, 300);
});

test("only the voice row may ask for the strip's width, within what a strip can be", () => {
  assert.equal(stripWidth("media", 300), ACTIVITY_W);
  assert.equal(stripWidth(VOICE), ACTIVITY_W);
  assert.equal(stripWidth(VOICE, NaN), ACTIVITY_W);
  assert.equal(stripWidth(VOICE, 300), 300);
  assert.equal(stripWidth(VOICE, 10), NOTCH_W);
  assert.equal(stripWidth(VOICE, 5000), NOTICE_W);
});

test("a shape is told again only when it changed", () => {
  const strip = { mode: "strip", width: 340, height: 32, top: 0 };
  assert.ok(sameShape(strip, { ...strip }));
  assert.ok(!sameShape(strip, { ...strip, mode: "open" }));
  assert.ok(!sameShape(strip, { ...strip, height: 200 }));
  assert.ok(!sameShape(strip, { ...strip, top: 12 }));
});

test("the face voice asks for, and never over a card that waits", () => {
  const own = { held: null, voice: null, look: null };
  assert.deepEqual(mascotFace(null, 0, false), own);
  assert.deepEqual(mascotFace("listening", 0.4, false), { held: null, voice: { face: "listening", level: 0.4 }, look: null });
  assert.deepEqual(mascotFace("talking", 7, false).voice, { face: "talking", level: 1 });
  assert.deepEqual(mascotFace("talking", -1, false).voice, { face: "talking", level: 0 });
  assert.deepEqual(mascotFace("listening", NaN, false).voice, { face: "listening", level: 0 });
  // The two faces the mascot already has are held over its state; thinking looks up and right (§7.6).
  assert.deepEqual(mascotFace("thinking", 0, false), { held: "thinking", voice: null, look: { x: 0.4, y: 0.5 } });
  assert.deepEqual(mascotFace("asking", 0, false), { held: "question", voice: null, look: null });
  // A permission or a question card keeps its own face.
  for (const face of ["listening", "thinking", "talking", "asking"]) assert.deepEqual(mascotFace(face, 1, true), own, face);
});

test("wink and surprised are played once, not worn", () => {
  assert.equal(faceEmote("wink"), "wink");
  assert.equal(faceEmote("surprised"), "surprised");
  for (const face of ["listening", "thinking", "talking", "asking", null]) assert.equal(faceEmote(face), null);
  assert.deepEqual(mascotFace("wink", 0, false), mascotFace(null, 0, false));
});

// The exports each package of docs/VOICE.md §10 fills in and the others import: a name that goes
// missing breaks a neighbour's file. Read from the source, so a module that needs the webview or the
// registry (which Node cannot load) is still checked.
const MODULES = {
  "index.ts": ["voiceCall", "voiceOn", "default"],
  "turn.ts": ["turnView", "onTurn", "startTurns", "listen", "endTurn", "ask"],
  "runner.ts": ["DENIED", "voiceCx", "act", "undoLast", "speak", "speakStream", "hush"],
  "registry.ts": ["startRegistry", "loadRegistry", "entries", "byTool", "onRegistry", "cxOf"],
  "base-actions.ts": ["default"],
  "parse/index.ts": ["normalise", "splitCommands", "isYes", "isNo", "isCancel", "isEnd", "parse", "readArg", "resolveChoice"],
  "brain/needle.ts": ["needleTools", "pushTools", "grounded", "choose"],
  "brain/route.ts": ["builtin", "local", "BRAINS", "SYSTEM", "answer"],
  "brain/split.ts": ["cleanForSpeech", "Splitter"],
  "brain/claude.ts": ["claude", "free", "startChatAloud"],
  "ui/index.ts": ["turnActivity", "capsulePage", "startUi"],
  "settings.ts": ["settingsPage"],
  "wake.ts": ["earFor", "startWake"],
  "answers/index.ts": ["answers", "stateAbout"],
  "mail/index.ts": ["mail"],
};

test("every module of the voice feature exports what its neighbours import", () => {
  for (const [file, names] of Object.entries(MODULES)) {
    const source = readFileSync(new URL(`../src/features/voice/${file}`, import.meta.url), "utf8");
    for (const name of names) {
      const found = name === "default"
        ? /^export default /m.test(source)
        : new RegExp(`^export (?:async )?(?:function|const|class) ${name}[^A-Za-z0-9_$]`, "m").test(source);
      assert.ok(found, `${file} exports ${name}`);
    }
  }
});

test("a fixed reply comes out of the splitter whole", async () => {
  const { Splitter } = await import("../src/features/voice/brain/split.ts");
  const s = new Splitter();
  assert.equal([...s.push("Done."), ...s.end()].join(" "), "Done.");
  assert.deepEqual(s.end(), [], "nothing is said twice");
});
