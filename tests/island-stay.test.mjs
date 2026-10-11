// "The small island stays" (Settings › Island): how long the peek strip keeps
// out once it is out, and what puts it away (src/island/fsm.ts). The clock is
// the test's: nothing here waits.

import { beforeEach, afterEach, mock, test } from "node:test";
import assert from "node:assert/strict";
import { IslandStateMachine, agentBusy, stays } from "../src/island/fsm.ts";

beforeEach(() => mock.timers.enable({ apis: ["setTimeout"] }));
afterEach(() => mock.timers.reset());

const after = (ms) => mock.timers.tick(ms);

/** A machine with the setting, and whether an agent session works or waits. */
function machine(stay, busy = false) {
  const fsm = new IslandStateMachine();
  fsm.setStay(stay, busy);
  return fsm;
}

/** The pointer came to the strip and left again. */
function visit(fsm) {
  fsm.mouseEntered();
  fsm.mouseLeft();
}

test("which sessions count as an agent at work or waiting", () => {
  for (const state of ["working", "thinking", "searching", "approval", "question"]) assert.ok(agentBusy([{ state }]), state);
  for (const state of ["idle", "sleeping", "finished", "error", "ratelimit"]) assert.ok(!agentBusy([{ state }]), state);
  // It asks in its terminal: the pill only carries the badge.
  assert.ok(agentBusy([{ state: "idle" }, { state: "idle", pillBadge: "approval" }]));
  assert.ok(!agentBusy([]));
});

test("the three choices", () => {
  assert.equal(stays("moment", true), false);
  assert.equal(stays("agent", false), false);
  assert.equal(stays("agent", true), true);
  assert.equal(stays("always", false), true);
});

test("for a moment: gone two seconds after the pointer leaves, a reveal lasts its time", () => {
  const fsm = machine("moment", true);
  visit(fsm);
  assert.equal(fsm.kept, false);
  after(1900);
  assert.equal(fsm.state, "peek");
  after(200);
  assert.equal(fsm.state, "hidden");

  fsm.reveal(3000);
  after(2900);
  assert.equal(fsm.state, "peek");
  after(200);
  assert.equal(fsm.state, "hidden");
  // Nothing to put away: the shortcut opens the island as it always did.
  fsm.reveal(3000);
  assert.equal(fsm.dismiss(), false);
  assert.equal(fsm.state, "peek");
});

test("until I dismiss it: the pointer leaving and a reveal ending change nothing", () => {
  const fsm = machine("always");
  visit(fsm);
  after(60_000);
  assert.equal(fsm.state, "peek");
  assert.equal(fsm.kept, true);

  const shown = machine("always");
  shown.reveal(3000);
  after(60_000);
  assert.equal(shown.state, "peek");
  // A notice after it, with its own time: still there.
  shown.reveal(1600, true);
  after(60_000);
  assert.equal(shown.state, "peek");
});

test("until I dismiss it: folding the open island goes to the strip, which stays", () => {
  const fsm = machine("always");
  fsm.forceOpen();
  fsm.forcePeek();
  after(60_000);
  assert.equal(fsm.state, "peek");
  // And by the countdown of an island opened by a click.
  fsm.forceOpen();
  after(15_000);
  assert.equal(fsm.state, "peek");
  after(60_000);
  assert.equal(fsm.state, "peek");
});

test("Esc puts it away under the pointer, and only there", () => {
  const fsm = machine("always");
  visit(fsm);
  // The pointer is elsewhere: Esc is the window's in front, and were it to arrive it would do nothing.
  assert.equal(fsm.escape(), false);
  assert.equal(fsm.state, "peek");
  fsm.mouseEntered();
  assert.equal(fsm.escape(), true);
  assert.equal(fsm.state, "hidden");
  // Dismissed, it comes out again the usual ways, and stays again.
  fsm.reveal(3000);
  after(60_000);
  assert.equal(fsm.state, "peek");
});

test("for a moment, Esc on the strip is not the island's", () => {
  const fsm = machine("moment");
  fsm.mouseEntered();
  assert.equal(fsm.kept, false);
  assert.equal(fsm.escape(), false);
  assert.equal(fsm.state, "peek");
});

test("the shortcut and the fold mark put it away wherever the pointer is", () => {
  const fsm = machine("always");
  visit(fsm);
  assert.equal(fsm.dismiss(), true);
  assert.equal(fsm.state, "hidden");
  // Nothing is out any more: the shortcut opens the island again.
  assert.equal(fsm.dismiss(), false);
  // An open island is folded, not dismissed.
  fsm.forceOpen();
  assert.equal(fsm.dismiss(), false);
  assert.equal(fsm.state, "open");
});

test("while an agent is working: it stays for as long as one works or waits, then leaves as it always did", () => {
  const fsm = machine("agent", true);
  visit(fsm);
  after(60_000);
  assert.equal(fsm.state, "peek");
  assert.equal(fsm.kept, true);
  // The last session finished.
  fsm.setStay("agent", false);
  assert.equal(fsm.kept, false);
  after(1900);
  assert.equal(fsm.state, "peek");
  after(200);
  assert.equal(fsm.state, "hidden");
});

test("while an agent is working: work that starts during the countdown keeps the strip", () => {
  const fsm = machine("agent", false);
  visit(fsm);
  after(1000);
  fsm.setStay("agent", true);
  after(60_000);
  assert.equal(fsm.state, "peek");
});

test("the setting changed back to a moment lets a strip that stayed go", () => {
  const fsm = machine("always");
  visit(fsm);
  after(60_000);
  fsm.setStay("moment", false);
  after(2100);
  assert.equal(fsm.state, "hidden");
});

test("full screen, Pause, lock and sleep still hide it: staying is not a hold", () => {
  const fsm = machine("always");
  visit(fsm);
  // What those rules ask before they hide (Island.setFullscreen, Island.away).
  assert.equal(fsm.held, false);
  fsm.forceHidden();
  assert.equal(fsm.state, "hidden");
  after(60_000);
  assert.equal(fsm.state, "hidden");
});

test("a waiting card keeps its own rules: nothing here folds or puts it away", () => {
  const fsm = machine("always");
  // The card came up and was folded to the strip without an answer (Island.foldApproval).
  fsm.pinned = true;
  fsm.forceOpen();
  fsm.forcePeek();
  fsm.mouseEntered();
  assert.equal(fsm.kept, false);
  assert.equal(fsm.escape(), false);
  assert.equal(fsm.dismiss(), false);
  assert.equal(fsm.state, "peek");
  // Open on its card, neither does anything.
  fsm.forceOpen();
  assert.equal(fsm.dismiss(), false);
  assert.equal(fsm.state, "open");
  // Answered: the strip is an ordinary one that stays.
  fsm.forcePeek();
  fsm.unpin();
  assert.equal(fsm.kept, true);
  fsm.mouseLeft();
  after(60_000);
  assert.equal(fsm.state, "peek");
});

test("a strip a feature holds is that feature's to let go", () => {
  const fsm = machine("always");
  const release = fsm.hold();
  visit(fsm);
  assert.equal(fsm.dismiss(), false);
  assert.equal(fsm.state, "peek");
  release();
  assert.equal(fsm.dismiss(), true);
});

test("resting on the fold mark does not open the island; back on the strip it does", () => {
  const fsm = machine("always");
  fsm.mouseEntered();
  fsm.overControl(true);
  after(1000);
  assert.equal(fsm.state, "peek");
  fsm.overControl(false);
  after(300);
  assert.equal(fsm.state, "open");
});

test("a pointer that arrives on a button of the strip opens nothing, whichever is heard first", () => {
  const fsm = machine("moment");
  fsm.overControl(true);
  fsm.mouseEntered();
  after(1000);
  assert.equal(fsm.state, "peek");
  fsm.mouseLeft();
  fsm.mouseEntered();
  after(300);
  assert.equal(fsm.state, "open");
});

test("after a jump to the session the island leaves at once, unless the strip stays", () => {
  const gone = machine("moment");
  gone.mouseEntered();
  gone.forceOpen();
  gone.leave();
  assert.equal(gone.state, "hidden");

  const kept = machine("always");
  kept.mouseEntered();
  kept.forceOpen();
  kept.leave();
  assert.equal(kept.state, "peek");

  // From the shortcut, with the island away: nothing comes out.
  const away = machine("always");
  away.leave();
  assert.equal(away.state, "hidden");
});
