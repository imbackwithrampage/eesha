// The question card (src/core/question.ts): pick, tick, "Other…", Back, and
// the keys 1–9 and Enter. The keys are the question card's and nobody else's:
// no key ever answers a permission card.

import { test } from "node:test";
import assert from "node:assert/strict";
import { QuestionFlow, questionKey, questionKeyNames } from "../src/core/question.ts";
import { comboCode, comboCodes, comboEvent } from "../src/island/keys.ts";

const option = (label) => ({ label, description: "" });
const ONE = { question: "Which one?", options: [option("A"), option("B"), option("C")], multiSelect: false };
const MANY = { question: "Extras?", options: [option("Tests"), option("Docs")], multiSelect: true };
const SAME = { question: "Sure?", options: [option("A"), option("B")], multiSelect: false };

const key = (k, extra = {}) => ({ key: k, ...extra });
const on = (flow, extra = {}) => ({ view: "question", allowed: true, inField: false, flow, ...extra });

test("an answer is one label per question, a list for a pick-several one", () => {
  const flow = new QuestionFlow([ONE, MANY]);
  flow.choose("B");
  assert.equal(flow.done, false);
  assert.equal(flow.index, 1);
  // Nothing ticked: Done does nothing.
  flow.confirm();
  assert.equal(flow.index, 1);
  flow.choose("Docs");
  flow.choose("Tests");
  flow.choose("Docs");
  assert.deepEqual([...flow.picked], ["Tests"]);
  flow.choose("Docs");
  flow.confirm();
  assert.equal(flow.done, true);
  assert.deepEqual(flow.answers, { "Which one?": "B", "Extras?": ["Tests", "Docs"] });
  assert.deepEqual([...flow.other], []);
  // Answered: nothing more moves it.
  flow.choose("A");
  flow.back();
  assert.deepEqual(flow.answers, { "Which one?": "B", "Extras?": ["Tests", "Docs"] });
});

test("a label that is not an option of the question on screen answers nothing", () => {
  const flow = new QuestionFlow([ONE, MANY]);
  flow.choose("Docs");
  flow.choose("");
  assert.equal(flow.index, 0);
  assert.deepEqual(flow.answers, {});
});

test("Other… takes words for the question it was opened on, and marks it", () => {
  const flow = new QuestionFlow([ONE, MANY]);
  flow.openOther();
  // While the field is open the options do nothing, and blank words are not an answer.
  flow.choose("A");
  flow.submitOther("   ");
  assert.equal(flow.index, 0);
  flow.submitOther("  neither, please ");
  assert.equal(flow.answers["Which one?"], "neither, please");
  assert.deepEqual([...flow.other], ["Which one?"]);
  assert.equal(flow.typing, false);
  // Words are only taken through the field.
  flow.submitOther("not asked");
  assert.equal(flow.index, 1);
  // Opened and left: the options are back, nothing was answered.
  flow.openOther();
  flow.closeOther();
  flow.choose("Tests");
  flow.confirm();
  assert.deepEqual(flow.answers["Extras?"], ["Tests"]);
  assert.deepEqual([...flow.other], ["Which one?"]);
});

test("Back returns to the question before with its ticks, and a new answer replaces the old", () => {
  const flow = new QuestionFlow([MANY, ONE]);
  flow.back();
  assert.equal(flow.index, 0);
  flow.choose("Docs");
  flow.confirm();
  flow.openOther();
  flow.back();
  assert.equal(flow.index, 0);
  assert.equal(flow.typing, false);
  assert.deepEqual([...flow.picked], ["Docs"]);
  // Answered in words the second time: no longer a list, and marked.
  flow.openOther();
  flow.submitOther("Benchmarks");
  assert.equal(flow.answers["Extras?"], "Benchmarks");
  assert.deepEqual([...flow.other], ["Extras?"]);
  // And with a label the third time: the mark goes.
  flow.back();
  flow.choose("Tests");
  flow.confirm();
  assert.deepEqual(flow.answers["Extras?"], ["Tests"]);
  assert.deepEqual([...flow.other], []);
});

test("1 to 9 pick an option and Enter confirms, on the question card only", () => {
  const flow = new QuestionFlow([ONE, MANY]);
  assert.deepEqual(questionKey(key("2"), on(flow)), { choose: "B" });
  assert.deepEqual(questionKey(key("3"), on(flow)), { choose: "C" });
  // No fourth option, no zero, no letter: nothing.
  for (const k of ["4", "9", "0", "a", "y", "n", "Y", "N", " ", "Enter", "Escape", "Tab"]) {
    assert.equal(questionKey(key(k), on(flow)), null, k);
  }
  // Ctrl+digit is the pill shortcut; Alt, Shift and Win are not ours either.
  for (const mod of ["ctrlKey", "altKey", "metaKey", "shiftKey"]) {
    assert.equal(questionKey(key("1", { [mod]: true }), on(flow)), null, mod);
  }
  // Enter confirms a pick-several question once something is ticked.
  flow.choose("A");
  assert.equal(questionKey(key("Enter"), on(flow)), null);
  flow.choose("Docs");
  assert.equal(questionKey(key("Enter"), on(flow)), "confirm");
  assert.deepEqual(questionKey(key("1"), on(flow)), { choose: "Tests" });
});

test("keys count only with the pointer on the card or the keyboard given by a click", () => {
  const flow = new QuestionFlow([ONE]);
  assert.equal(questionKey(key("1"), on(flow, { allowed: false })), null);
  // What is typed in a field is the field's.
  assert.equal(questionKey(key("1"), on(flow, { inField: true })), null);
  flow.openOther();
  assert.equal(questionKey(key("1"), on(flow)), null);
  flow.closeOther();
  flow.choose("A");
  // Answered: the keys are done.
  assert.equal(questionKey(key("1"), on(flow)), null);
  assert.equal(questionKey(key("1"), on(null)), null);
});

test("no key ever answers a permission card", () => {
  const flow = new QuestionFlow([SAME, MANY]);
  flow.index = 1;
  flow.picked.add("Docs");
  const keys = [
    "1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "Enter", " ", "y", "Y", "n", "N", "a", "A", "d", "D",
    "Escape", "Tab", "Backspace", "ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown",
  ];
  const mods = [{}, { ctrlKey: true }, { altKey: true }, { shiftKey: true }, { metaKey: true }, { ctrlKey: true, altKey: true }];
  // Every view but the question card, with everything else in the keys' favour.
  const views = ["approval", "finished", "error", "overview", "empty", "prompt", "settings", "section", "gallery", "recap", ""];
  for (const view of views) {
    for (const k of keys) {
      for (const mod of mods) {
        assert.equal(questionKey(key(k, mod), on(flow, { view })), null, `${view} ${k} ${JSON.stringify(mod)}`);
      }
    }
  }
  // And the question card itself never turns a key into anything but a pick or a confirm.
  for (const k of keys) {
    for (const mod of mods) {
      const act = questionKey(key(k, mod), on(flow));
      assert.ok(act === null || act === "confirm" || typeof act.choose === "string", k);
    }
  }
});

test("the island reads the question's own keys under the pointer, and a digit alone for nothing else", () => {
  const flow = new QuestionFlow([ONE, MANY]);
  assert.deepEqual(questionKeyNames(flow), ["1", "2", "3"]);
  flow.choose("A");
  assert.deepEqual(questionKeyNames(flow), ["1", "2", "Enter"]);
  flow.openOther();
  assert.deepEqual(questionKeyNames(flow), []);
  assert.deepEqual(questionKeyNames(null), []);
  // A digit alone is never one of the island's keys, except when asked for by name.
  assert.equal(comboCode("1"), null);
  assert.deepEqual(comboCodes(["1", "2", "Enter"]), [0x0d]);
  assert.deepEqual(comboCodes(["1", "2", "Enter"], true), [0x0d, 0x31, 0x32]);
  // Never a letter alone, asked for or not: Y and N are not keys of any card.
  for (const letter of ["y", "n", "Y", "N", "a"]) {
    assert.equal(comboCode(letter), null);
    assert.equal(comboCode(letter, true), null);
  }
  assert.equal(comboCode("Shift+1", true), null);
  // What Rust sends back is the key the card's listener hears.
  assert.deepEqual(comboEvent(0x32), { ctrlKey: false, altKey: false, shiftKey: false, key: "2", code: "Digit2" });
});
