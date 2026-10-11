// The voice assistant's text brains (src/features/voice/brain): which brain answers an open question
// (route.ts) and how its words become speakable chunks (split.ts).
//   node --import ./tests/setup.mjs --test tests/voice-brain.test.mjs

import { test } from "node:test";
import assert from "node:assert/strict";

const { route, order, LIMITS } = await import("../src/features/voice/brain/route.ts");
const { Splitter, cleanForSpeech } = await import("../src/features/voice/brain/split.ts");

// ── Routing ───────────────────────────────────────────────────────────────────

const IDS = ["builtin", "local", "free", "claude"];
const tick = () => new Promise((resolve) => setImmediate(resolve));
const cx = {};

/** A made-up clock: timers fire only when `advance` passes them. */
function fakeClock() {
  let now = 0;
  let timers = [];
  return {
    after(ms, fn) {
      const t = { at: now + ms, fn };
      timers.push(t);
      return () => (timers = timers.filter((x) => x !== t));
    },
    async advance(ms) {
      const to = now + ms;
      for (;;) {
        const due = timers.filter((t) => t.at <= to).sort((a, b) => a.at - b.at)[0];
        if (!due) break;
        timers = timers.filter((t) => t !== due);
        now = due.at;
        due.fn();
        await tick();
      }
      now = to;
      await tick();
    },
  };
}

/** `state`: "ready" answers "<id> says hi"; "down" is not ready; "fails" is ready and rejects; "hang" never answers. */
function brains(states, asked) {
  return IDS.map((id) => ({
    id,
    ready: async () => states[id] !== "down",
    ask: async (_cx, _ask, on) => {
      asked.push(id);
      if (states[id] === "fails") throw new Error("boom");
      if (states[id] === "hang") return new Promise(() => {});
      on(`${id} says `);
      on("hi");
      return `${id} says hi`;
    },
  }));
}
const askOf = (signal = new AbortController().signal) => ({ text: "why is the sky blue", history: [], signal });
const settingsOf = (sw, brain = "auto") => ({ brain, localServer: sw.local, freeModel: sw.free, claude: sw.claude });

test("routing table: every combination of switches, readiness and failures", async () => {
  const states = ["ready", "down", "fails"];
  let rows = 0;
  for (let sw = 0; sw < 8; sw++) {
    const on = { local: !!(sw & 1), free: !!(sw & 2), claude: !!(sw & 4) };
    const gate = { builtin: true, ...on };
    for (let n = 0; n < 81; n++) {
      const st = Object.fromEntries(IDS.map((id, i) => [id, states[Math.floor(n / 3 ** i) % 3]]));
      const asked = [];
      const heard = [];
      const got = await route(brains(st, asked), cx, settingsOf(on), askOf(), (d) => heard.push(d), fakeClock());
      // The rule, written out: in order, only a brain whose switch is on and that is ready is asked; the first that does not fail answers.
      const tried = IDS.filter((id) => gate[id] && st[id] !== "down");
      const winner = tried.find((id) => st[id] === "ready");
      const expectedAsked = winner ? tried.slice(0, tried.indexOf(winner) + 1) : tried;
      assert.deepEqual(asked, expectedAsked, JSON.stringify({ on, st }));
      assert.equal(got?.brain ?? null, winner ?? null, JSON.stringify({ on, st }));
      if (got) assert.equal(got.text, `${winner} says hi`);
      for (const id of ["free", "claude"]) if (!gate[id]) assert.ok(!asked.includes(id), `${id} reached with its switch off`);
      rows++;
    }
  }
  assert.equal(rows, 648);
});

test("a brain that is switched off is not even asked if it is ready (a key that merely exists opens nothing)", async () => {
  const asked = [];
  const got = await route(brains({ builtin: "down", local: "ready", free: "ready", claude: "ready" }, asked), cx, settingsOf({}), askOf(), () => {}, fakeClock());
  assert.equal(got, null);
  assert.deepEqual(asked, []);
});

test("a fixed choice goes first, then the others; off asks nobody", async () => {
  const sw = { local: true, free: true, claude: true };
  let asked = [];
  let got = await route(brains({ builtin: "ready", local: "ready", free: "ready", claude: "ready" }, asked), cx, settingsOf(sw, "claude"), askOf(), () => {}, fakeClock());
  assert.equal(got.brain, "claude");
  asked = [];
  got = await route(brains({ builtin: "ready", local: "ready", free: "fails", claude: "ready" }, asked), cx, settingsOf(sw, "free"), askOf(), () => {}, fakeClock());
  assert.deepEqual(asked, ["free", "builtin"]);
  assert.equal(got.brain, "builtin");
  asked = [];
  assert.equal(await route(brains({ builtin: "ready", local: "ready", free: "ready", claude: "ready" }, asked), cx, settingsOf(sw, "off"), askOf(), () => {}, fakeClock()), null);
  assert.deepEqual(asked, []);
  assert.deepEqual(order(brains({}, []), "off"), []);
});

test("time limits: first token 12 s local, 6 s online; then the next brain", async () => {
  const sw = { local: true, free: true, claude: false };
  for (const [hanging, limit, next] of [["builtin", LIMITS.firstLocal, "local"], ["free", LIMITS.firstOnline, "builtin"]]) {
    const clock = fakeClock();
    const asked = [];
    const st = { builtin: "ready", local: "ready", free: "ready", claude: "down", [hanging]: "hang" };
    const run = route(brains(st, asked), cx, settingsOf(sw, hanging), askOf(), () => {}, clock);
    await tick();
    await clock.advance(limit - 1);
    assert.deepEqual(asked, [hanging], `${hanging} still waited at ${limit - 1} ms`);
    await clock.advance(1);
    const got = await run;
    assert.deepEqual(asked, [hanging, next], `${hanging} given up at ${limit} ms`);
    assert.equal(got.brain, next);
  }
});

test("time limits: the first-token clock ends with the first token; 30 s in all; words already said stand", async () => {
  const clock = fakeClock();
  const heard = [];
  let release;
  const slow = {
    id: "builtin",
    ready: async () => true,
    ask: (_cx, ask, on) =>
      new Promise((resolve, reject) => {
        ask.signal.addEventListener("abort", () => reject(new Error("aborted")));
        release = () => resolve("never");
        setTimeout(() => on("Well, "), 0);
      }),
  };
  const next = { id: "local", ready: async () => true, ask: async () => assert.fail("must not be asked after words were said") };
  const run = route([slow, next], cx, settingsOf({ local: true }), askOf(), (d) => heard.push(d), clock);
  await new Promise((resolve) => setTimeout(resolve, 10)); // the first token arrives
  await clock.advance(LIMITS.firstLocal + 1000); // past the first-token limit: the token already came
  assert.deepEqual(heard, ["Well, "]);
  await clock.advance(LIMITS.total - LIMITS.firstLocal - 1000 - 1);
  assert.ok(release);
  await clock.advance(1);
  const got = await run;
  assert.deepEqual(got, { text: "Well, ", brain: "builtin" });
});

test("a brain that ignores its signal is still cut off; an interruption ends the whole route", async () => {
  const clock = fakeClock();
  const deaf = { id: "builtin", ready: async () => true, ask: () => new Promise(() => {}) };
  const never = { id: "local", ready: async () => true, ask: async () => "late" };
  const run = route([deaf, never], cx, settingsOf({ local: false }), askOf(), () => {}, clock);
  await tick();
  await clock.advance(LIMITS.firstLocal);
  assert.equal(await run, null);

  const ctl = new AbortController();
  const run2 = route([deaf, never], cx, settingsOf({ local: true }), askOf(ctl.signal), () => {}, fakeClock());
  await tick();
  ctl.abort();
  assert.equal(await run2, null);
});

// ── Splitter ──────────────────────────────────────────────────────────────────

const flat = (s) => s.replace(/\s+/g, " ").trim();
const whole = (text) => {
  const s = new Splitter();
  return [...s.push(text), ...s.end()];
};

/** A short first clause, then a sentence that runs on. */
const GROWING =
  "Well, that is a rather long way to go, but the road is good and the weather is fine today so nobody here minds the walk at all, and the view from the top of the hill is worth every single step of it.";

/** The sentences the owner would hear; `early`: the first chunk must be out before the text ends. */
const CASES = [
  { text: "The U.S. is big. It is 3.5 times bigger than Texas.", early: true },
  { text: "Dr. Smith paid $3.50 at 10:30 a.m. today. Then he left for the station.", early: true },
  { text: "Visit https://example.com/a?b=1.5 for more. It is free and always open.", early: true },
  { text: "Use `foo.bar(1, 2)` to call it. Then check the result carefully.", early: true },
  { text: "Version 2.0.1 shipped on Jan. 5, which is nice. Pretty good news indeed.", early: true },
  { text: "**Good news.** The *quick* answer is yes, and the long one is also yes.", early: true },
  { text: "- First item is here\n- Second item is here\n1. Third item is here\n", early: true },
  { text: "It is 21°C and 40% humid; wind is 15 km/h. Take a light jacket with you.", early: true },
  { text: "Well, that is a rather long way to go before a comma shows up, so wait. Yes!", early: true },
  { text: "See [the docs](https://example.com/docs) or [1] for details. Thanks a lot!", early: true },
  { text: "Hello 😀 there! How are you today? I am fine, thanks for asking me.", early: true },
  { text: "Look:\n```js\nconst a = 1.5;\nfoo();\n```\nThat was the code. Now it is over.", early: true },
  { text: "Wait... what? That costs $5 million & more! Really?", early: true },
  { text: "# Title here\n\nSome text follows the title. It is short.\n---\nAfter the rule comes more text.", early: true },
  { text: "No full stop at the end of this one", early: false },
  { text: GROWING, early: true },
  { text: "Here is my guess. It costs about 10 000 dollars a year to run all of this, or $5 million in all.", early: true },
];

test("cleanForSpeech: marks, links, code, emoji, units", () => {
  assert.equal(cleanForSpeech("**Bold** and *italic* and ~~gone~~"), "Bold and italic and gone");
  assert.equal(cleanForSpeech("- one\n* two\n3. three\n> quote\n## head"), "one two three quote head");
  assert.equal(cleanForSpeech("See [docs](https://x.io/a) [2] and https://x.io/a/b."), "See docs and a link.");
  assert.equal(cleanForSpeech("Run `npm test` now\n```\nignored()\n```\ndone"), "Run npm test now done");
  assert.equal(cleanForSpeech("Hi 😀 there 👍🏽"), "Hi there");
  assert.equal(cleanForSpeech("21°C, 70°F, 5%, 1 km, 12 km/h, 3 GB, $3.50, €2 million, A & B"), "21 degrees Celsius, 70 degrees Fahrenheit, 5 percent, 1 kilometer, 12 kilometers per hour, 3 gigabytes, 3.50 dollars, 2 million euros, A and B");
  assert.equal(cleanForSpeech("It is 10:30 on 3.5.2026 at snake_case_name"), "It is 10:30 on 3.5.2026 at snake_case_name");
  assert.equal(cleanForSpeech("| a | b |\n|---|---|\n| 1 | 2 |"), "a, b 1, 2");
});

test("the first chunk is out as soon as it is safe", () => {
  const first = (text) => {
    const s = new Splitter();
    for (let i = 0; i < text.length; i++) {
      const out = s.push(text[i]);
      if (out.length) return { at: i, chunk: out[0] };
    }
    return null;
  };
  assert.deepEqual(first("The U.S. is big. It is"), { at: 16, chunk: "The U.S. is big." });
  assert.deepEqual(first("Dr. Smith paid $3.50 at 10:30 a.m. today. Then"), { at: 41, chunk: "Dr. Smith paid 3.50 dollars at 10:30 a.m. today." });
  // a clause after six words, never inside a number
  assert.deepEqual(first("Well, that is a rather long way to go, but still"), { at: 38, chunk: "Well, that is a rather long way to go," });
  assert.deepEqual(first("In 1,000 or 2,000 years, who knows what it will be, maybe"), { at: 51, chunk: "In 1,000 or 2,000 years, who knows what it will be," });
  // under 12 characters joins the next
  assert.deepEqual(first("Hi. How are you doing today? Fine"), { at: 28, chunk: "Hi. How are you doing today?" });
  // inside an open code span or link nothing is cut
  assert.deepEqual(first("Use `a. b. c` now. Ok"), { at: 18, chunk: "Use a. b. c now." });
  assert.equal(first("Use `a. b. c"), null);
});

for (const { text, early } of CASES) {
  test(`splitter: ${JSON.stringify(text.slice(0, 44))}`, () => {
    const expected = whole(text);
    assert.equal(flat(expected.join(" ")), flat(cleanForSpeech(text)), "joined chunks equal the cleaned whole");
    assert.ok(expected.every((c) => c.length > 0));
    // cut into two pieces at every position
    for (let i = 0; i <= text.length; i++) {
      const s = new Splitter();
      const got = [...s.push(text.slice(0, i)), ...s.push(text.slice(i)), ...s.end()];
      assert.deepEqual(got, expected, `cut at ${i}`);
    }
    // one character at a time
    const s = new Splitter();
    const got = [];
    let firstAt = -1;
    for (let i = 0; i < text.length; i++) {
      const out = s.push(text[i]);
      if (out.length && firstAt < 0) firstAt = i;
      got.push(...out);
    }
    got.push(...s.end());
    assert.deepEqual(got, expected);
    if (early) assert.ok(firstAt >= 0 && firstAt < text.length - 1, `first chunk came at ${firstAt} of ${text.length}`);
  });
}

test("after the first chunk, a chunk is at most about 1.2 times as long as the one before it", () => {
  const chunks = whole(GROWING);
  // Cut at a comma where one is in reach, else at a space; the last words of a sentence are never left as a stub.
  assert.deepEqual(chunks, [
    "Well, that is a rather long way to go,",
    "but the road is good and the weather is fine",
    "today so nobody here minds the walk at all,",
    "and the view from the top of the hill is worth",
    "every single step of it.",
  ]);
  assert.equal(chunks.join(" "), GROWING);
  for (let i = 1; i < chunks.length; i++) {
    const cap = Math.round(chunks[i - 1].length * 1.2);
    // Only the end of a sentence may run over, by less than the 12 characters that would join the next anyway.
    assert.ok(chunks[i].length <= (/[.!?]$/.test(chunks[i]) ? cap + 12 : cap), `chunk ${i}: ${chunks[i].length} after ${chunks[i - 1].length}`);
    assert.ok(chunks[i].length >= 12, `chunk ${i} is not under the minimum`);
  }
  // Never inside a number, a sum of money or a word; a sentence that fits is left whole.
  assert.deepEqual(whole("Here is my guess. It costs about 10 000 dollars a year to run all of this, or $5 million in all."), [
    "Here is my guess.",
    "It costs about",
    "10 000 dollars a",
    "year to run all of",
    "this, or 5 million dollars in all.",
  ]);
  assert.deepEqual(whole("Timer set for 5 minutes. I will tell you when it is done."), ["Timer set for 5 minutes.", "I will tell you when it is done."]);
});

test("a chunk is never over 220 characters, and a long text joined stays whole", () => {
  const text = "word ".repeat(120).trim() + ".";
  const chunks = whole(text);
  assert.ok(chunks.length > 2);
  assert.ok(chunks.every((c) => c.length <= 221));
  assert.equal(chunks.join(" "), text);
});
