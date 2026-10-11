// The battery notices of src/features/huds-power on the strip: the charger going in or out, a low
// battery, a full one, the charge limit. Each is said once, for its time, and then the strip is as
// it was: away, or on what it rests on when the user keeps it out. Nothing comes back by itself.
//
// The island is the real arbiter and state machine (src/island/arbiter.ts, fsm.ts) wired as
// src/island/island.ts wires them: `stripContent` (a notice, else the activity that owns the strip,
// else the pills), `syncActivityHold`, `api().activityChanged` and the flush on `peek`. The clock is
// the test's.

import { test } from "node:test";
import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { installFakeDom } from "./fakedom.mjs";

// A feature's index.ts imports its stylesheet, which only the bundler understands.
registerHooks({
  resolve(specifier, context, nextResolve) {
    if (specifier.endsWith(".css")) return { url: "data:text/javascript,", shortCircuit: true };
    return nextResolve(specifier, context);
  },
});

installFakeDom();
const { ACTIVITY_REVEAL_MS, Arbiter, pickActivities } = await import("../src/island/arbiter.ts");
const { IslandStateMachine } = await import("../src/island/fsm.ts");
const { default: levels } = await import("../src/features/huds-power/index.ts");

/** What Rust tells of the power: on the charger or not, the charge, and whether it is charging. */
const power = (ac, percent, charging = ac) => ({ ac, percent, charging, saver: false });
const text = (row) => row.children.map((c) => c.textContent).filter(Boolean).join(" ");

// One clock for the whole file, forwards only: the feature remembers times between two starts.
let now = 1000;

/**
 * The feature started on an island. `stay`: Settings › Island "The small island stays"; `keep`:
 * "Keep live activities on screen"; `settings`: the feature's own.
 * `strip()` is what the user sees: null when the strip is away, else the notice, the row or the pills.
 */
function island(t, { stay = "moment", keep = false, settings = {} } = {}) {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  t.mock.method(performance, "now", () => now);
  const fsm = new IslandStateMachine();
  fsm.setStay(stay, false);
  const arbiter = new Arbiter({
    blocked: () => false,
    open: () => fsm.state === "open",
    hovered: () => false,
    reveal: (ms, exact) => fsm.reveal(ms, exact),
    changed() {},
  });
  fsm.onTransition = (_from, to) => to === "peek" && arbiter.flush();
  const owner = () => pickActivities(levels.activities).primary;
  let hold = null;
  const syncHold = () => {
    const want = levels.activities.some((a) => a.isActive() && (a.hold || keep));
    if (want && !hold) {
      hold = fsm.hold();
      fsm.reveal(ACTIVITY_REVEAL_MS);
    } else if (!want && hold) {
      hold();
      hold = null;
    }
  };

  const said = [];
  const events = new Map();
  let apply = () => {};
  const stop = levels.start({
    id: "huds-power",
    t: (key, vars) => key.replace(/\{(\w+)\}/g, (_, name) => String(vars?.[name])),
    call: async () => null,
    on: (event, fn) => (events.set(event, fn), () => events.delete(event)),
    // Applied below, at once: nothing here waits for a promise.
    settings: { get: () => new Promise(() => {}), patch: async () => {}, onChange: (fn) => ((apply = fn), () => {}) },
    island: {
      notice: (n) => {
        said.push(n.title);
        arbiter.post(n, "huds-power");
      },
      activityChanged: (id) => {
        const o = owner();
        arbiter.activityChanged(o?.id === id ? o : undefined);
        syncHold();
      },
      redraw() {},
      fullscreen() {},
    },
  });
  // The feature keeps its state between two starts: stopped even when a case fails.
  t.after(stop);
  apply(settings);

  return {
    /** Every notice the feature posted, by title. */
    said,
    row: () => owner()?.render() ?? null,
    strip: () => {
      if (fsm.state === "hidden") return null;
      const n = arbiter.notice?.notice;
      if (n) return `notice: ${[n.title, n.detail].filter(Boolean).join(" ")}`;
      const o = owner();
      return o ? `row: ${text(o.render())}` : "pills";
    },
    after: (ms) => {
      for (let i = 0; i < ms; i++) {
        now += 1;
        t.mock.timers.tick(1);
      }
    },
    power: (p) => events.get("power")(p),
    limit: (percent) => events.get("chargeLimit")({ percent }),
    volume: (level) => events.get("volume")({ level, mute: false, quiet: false }),
    mic: (apps) => events.get("micUse")({ apps }),
  };
}

test("the charger goes in: Charging is said once, for four seconds, and the strip is away again whatever the battery reports", (t) => {
  const s = island(t);
  s.power(power(false, 60));
  assert.equal(s.strip(), null);

  s.power(power(true, 60));
  assert.equal(s.strip(), "notice: Charging 60%");
  // The same level told again while it shows gives it no more time.
  s.after(2000);
  s.power(power(true, 60));
  s.after(1999);
  assert.equal(s.strip(), "notice: Charging 60%");
  s.after(1);
  assert.equal(s.strip(), null);

  // A minute of the same level, then a charge that rises.
  for (const percent of [60, 60, 60, 60, 60, 60, 61, 62]) {
    s.power(power(true, percent));
    s.after(10_000);
    assert.equal(s.strip(), null, `at ${percent}%`);
  }
  assert.deepEqual(s.said, ["Charging"]);

  // And out again: the same.
  s.power(power(false, 62));
  assert.equal(s.strip(), "notice: On battery 62%");
  s.after(4000);
  assert.equal(s.strip(), null);
  s.power(power(false, 62));
  s.after(60_000);
  assert.equal(s.strip(), null);
  assert.deepEqual(s.said, ["Charging", "On battery"]);
});

test("the small island stays until I dismiss it: the strip stays, on the pills, not on the notice", (t) => {
  const s = island(t, { stay: "always" });
  s.power(power(false, 60));
  s.power(power(true, 60));
  assert.equal(s.strip(), "notice: Charging 60%");
  s.after(4000);
  assert.equal(s.strip(), "pills");
  for (const percent of [60, 60, 60, 60, 60, 60, 61, 62]) {
    s.power(power(true, percent));
    s.after(10_000);
    assert.equal(s.strip(), "pills", `at ${percent}%`);
  }
  assert.deepEqual(s.said, ["Charging"]);
});

// What the owner had on: the strip stays, live activities are kept, and it rests on the battery level.
// The resting row used to read "Charging 60%" for as long as the charger was in: the notice's own
// words and glyph, so the notice seemed never to leave.
test("resting on the battery level: after the notice the row is the reading it was, not the notice's words", (t) => {
  const s = island(t, { stay: "always", keep: true, settings: { restOnBattery: true } });
  s.power(power(false, 60));
  assert.equal(s.strip(), "row: Battery 60%");

  s.power(power(true, 60));
  assert.equal(s.strip(), "notice: Charging 60%");
  s.after(4000);
  assert.equal(s.strip(), "row: Battery 60%");
  // The charger is in the glyph's colour.
  assert.ok(s.row().querySelector(".f-huds-power-batt.ok"));
  for (const percent of [60, 60, 60, 60, 60, 60, 61, 62]) {
    s.power(power(true, percent));
    s.after(10_000);
    assert.equal(s.strip(), `row: Battery ${percent}%`);
  }
  // Full and no longer charging: said once, and the row is still the reading.
  s.power(power(true, 100, false));
  assert.equal(s.strip(), "notice: Fully charged 100%");
  s.after(4000);
  assert.equal(s.strip(), "row: Battery 100%");

  s.power(power(false, 100));
  assert.equal(s.strip(), "notice: On battery 100%");
  s.after(4000);
  assert.equal(s.strip(), "row: Battery 100%");
  assert.equal(s.row().querySelector(".f-huds-power-batt.ok"), null);
  assert.deepEqual(s.said, ["Charging", "Fully charged", "On battery"]);
});

test("resting on the battery level with a strip that leaves: it leaves after the notice", (t) => {
  const s = island(t, { settings: { restOnBattery: true } });
  s.power(power(false, 60));
  s.after(ACTIVITY_REVEAL_MS);
  assert.equal(s.strip(), null);
  s.power(power(true, 60));
  assert.equal(s.strip(), "notice: Charging 60%");
  s.after(4000);
  assert.equal(s.strip(), null);
  s.power(power(true, 61));
  s.after(60_000);
  assert.equal(s.strip(), null);
});

test("a low battery, the charge limit and a full battery are each said once and leave", (t) => {
  const s = island(t);
  s.power(power(false, 21));
  s.power(power(false, 20));
  assert.equal(s.strip(), "notice: Battery low 20%");
  s.after(4000);
  assert.equal(s.strip(), null);
  for (const percent of [20, 19, 19, 18]) {
    s.power(power(false, percent));
    s.after(10_000);
    assert.equal(s.strip(), null, `at ${percent}%`);
  }

  s.power(power(true, 18));
  s.after(4000);
  s.limit(80);
  assert.equal(s.strip(), "notice: Charge limit reached 80%");
  s.after(4000);
  assert.equal(s.strip(), null);
  s.power(power(true, 81));
  s.after(60_000);
  assert.equal(s.strip(), null);

  s.power(power(true, 100, false));
  assert.equal(s.strip(), "notice: Fully charged 100%");
  s.after(4000);
  for (let i = 0; i < 6; i++) {
    s.power(power(true, 100, false));
    s.after(10_000);
    assert.equal(s.strip(), null);
  }
  assert.deepEqual(s.said, ["Battery low", "Charging", "Charge limit reached", "Fully charged"]);
});

test("a volume key while Charging shows: the notice follows the level once, then leaves for good", (t) => {
  const s = island(t);
  s.power(power(false, 60));
  s.power(power(true, 60));
  s.after(1000);
  s.volume(0.5);
  assert.equal(s.strip(), "notice: Volume");
  s.after(1600);
  assert.equal(s.strip(), "notice: Charging 60%");
  s.after(4000);
  assert.equal(s.strip(), null);
  s.power(power(true, 60));
  s.after(60_000);
  assert.equal(s.strip(), null);
  // A level by itself still shows, for its 1.6 s.
  s.volume(0.6);
  assert.equal(s.strip(), "notice: Volume");
  s.after(1600);
  assert.equal(s.strip(), null);
});

test("the microphone row does stay: it holds the strip for as long as the microphone is held", (t) => {
  const s = island(t);
  s.mic(["brave"]);
  assert.equal(s.strip(), "row: brave Microphone in use");
  s.after(60_000);
  assert.equal(s.strip(), "row: brave Microphone in use");
  // A battery notice over it, and the row is back.
  s.power(power(false, 60));
  s.power(power(true, 60));
  assert.equal(s.strip(), "notice: Charging 60%");
  s.after(4000);
  assert.equal(s.strip(), "row: brave Microphone in use");
  s.mic([]);
  s.after(100);
  assert.equal(s.strip(), null);
});
