// The online brains of the voice assistant (src/features/voice/brain/claude.ts): a stand-in for the host's
// `feature_call` stands where the chat's headless `ask` is, so no provider and no key is ever touched.
//   node --import ./tests/setup.mjs --test tests/voice-online.test.mjs

import { test } from "node:test";
import assert from "node:assert/strict";
import { emit, internals } from "./tauri.mjs";

const { claude, free, hooks, startChatAloud } = await import("../src/features/voice/brain/claude.ts");
const { route, BRAINS } = await import("../src/features/voice/brain/route.ts");

hooks.cxOf = async () => () => ({});

// ── The stand-in host ─────────────────────────────────────────────────────────

// What the webview supplies for dropping a listener.
globalThis.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };

const real = internals.invoke;
let keys = new Set();
/** Every chat.ask the host was asked for. */
let asks = [];
let cancels = [];
/** One scripted answer per ask: (args, id) => events. A function may throw "fail" to end in an error. */
let script = [];

internals.invoke = async (cmd, args) => {
  if (cmd === "secret_present") return keys.has(args.key);
  if (cmd === "feature_call" && args.feature === "chat") {
    if (args.method === "cancel") {
      cancels.push(args.args.id);
      return { cancelled: true };
    }
    if (args.method === "ask") {
      asks.push(args.args);
      const step = script.shift();
      const id = args.args.id;
      setImmediate(() => {
        if (!step) return emit("chat:ask-done", { id, stop: "error", error: "nothing scripted" });
        const done = step(args.args);
        if (done === null) return; // a host that stays silent
        for (const d of done.words ?? []) emit("chat:ask-delta", { id, text: d });
        emit("chat:ask-done", { id, stop: done.stop ?? "end_turn", text: (done.words ?? []).join(""), tools: done.tools ?? [], messages: done.messages ?? [], error: done.error });
      });
      return { id };
    }
  }
  return real(cmd, args);
};

function reset({ withKeys = [], steps = [] } = {}) {
  keys = new Set(withKeys);
  asks = [];
  cancels = [];
  script = [...steps];
}

const ALL_KEYS = ["anthropic-api-key", "openrouter-api-key", "google-api-key"];
const cxWith = (settings) => ({ settings: { get: async () => settings, onChange: () => () => {} } });
const askOf = (extra = {}) => ({ text: "why is the sky blue", history: [], signal: new AbortController().signal, ...extra });

// ── With a switch off, nothing is asked ───────────────────────────────────────

test("switches off: no request is made although every key exists and the brain is asked directly", async () => {
  reset({ withKeys: ALL_KEYS });
  const cx = cxWith({ claude: false, freeModel: false, claudeSearch: true, brain: "auto" });
  for (const brain of [claude, free]) {
    assert.equal(await brain.ready(cx, {}), false, `${brain.id} ready`);
    await assert.rejects(brain.ask(cx, askOf(), () => {}), /not switched on/);
  }
  assert.equal(asks.length, 0);
});

test("switches off: the router never reaches an online brain (not even one that reports itself ready)", async () => {
  reset({ withKeys: ALL_KEYS });
  const settings = { claude: false, freeModel: false, localServer: false, brain: "auto" };
  const online = BRAINS.filter((b) => b.id === "free" || b.id === "claude");
  const got = await route(online, cxWith(settings), settings, askOf(), () => {});
  assert.equal(got, null);
  // A fixed choice of Claude does not open it either.
  assert.equal(await route(online, cxWith({ ...settings, brain: "claude" }), { ...settings, brain: "claude" }, askOf(), () => {}), null);
  assert.equal(asks.length, 0);
});

test("switch on but no key: not ready and nothing asked", async () => {
  reset();
  const cx = cxWith({ claude: true, freeModel: true });
  assert.equal(await claude.ready(cx, {}), false);
  assert.equal(await free.ready(cx, {}), false);
  assert.equal(asks.length, 0);
});

// ── With a switch on, the right provider is named ─────────────────────────────

test("Claude: named provider anthropic, 1024 tokens, web search only by its own switch", async () => {
  reset({ withKeys: ALL_KEYS, steps: [() => ({ words: ["Because ", "of scattering."] }), () => ({ words: ["Ok."] })] });
  const heard = [];
  const cx = cxWith({ claude: true, claudeSearch: false });
  assert.equal(await claude.ready(cx, {}), true);
  let named = "";
  assert.equal(await claude.ask(cx, askOf({ by: (name) => (named = name) }), (d) => heard.push(d)), "Because of scattering.");
  assert.deepEqual(heard, ["Because ", "of scattering."]);
  assert.equal(named, "Claude");
  assert.equal(asks[0].provider, "anthropic");
  assert.equal(asks[0].max_tokens, 1024);
  assert.equal("web_search" in asks[0], false);
  assert.equal(asks[0].tools, undefined);

  await claude.ask(cxWith({ claude: true, claudeSearch: true }), askOf(), () => {});
  assert.equal(asks[1].web_search, true);
});

test("the router asks Claude only after the others fail, and only with its switch on", async () => {
  reset({ withKeys: ALL_KEYS, steps: [() => ({ words: ["From Claude."] })] });
  const settings = { claude: true, freeModel: false, localServer: false, brain: "auto" };
  const online = BRAINS.filter((b) => b.id === "free" || b.id === "claude");
  const got = await route(online, cxWith(settings), settings, askOf(), () => {});
  assert.deepEqual([got?.brain, got?.text], ["claude", "From Claude."]);
  assert.deepEqual(asks.map((a) => a.provider), ["anthropic"]);
});

test("free model: OpenRouter's free models first, Google's free tier next, and the caption says which", async () => {
  reset({ withKeys: ["openrouter-api-key", "google-api-key"], steps: [() => ({ words: ["Blue light scatters."] })] });
  const cx = cxWith({ freeModel: true });
  const heard = [];
  const named = [];
  const by = (name) => named.push(name);
  const text = await free.ask(cx, askOf({ by }), (d) => heard.push(d));
  assert.equal(text, "Blue light scatters.");
  assert.deepEqual(asks.map((a) => a.provider), ["openrouter-free"]);
  assert.equal(asks[0].max_tokens, 96);
  // The name is for the caption: nothing but the answer is said.
  assert.deepEqual(heard, ["Blue light scatters."]);
  assert.deepEqual(named, ["OpenRouter's free model"]);

  // OpenRouter has no model picked: it fails before a word and Google's free tier answers.
  reset({ withKeys: ["openrouter-api-key", "google-api-key"], steps: [() => ({ stop: "error", error: "Pick a model" }), () => ({ words: ["Scattering."] })] });
  const said = [];
  assert.equal(await free.ask(cx, askOf({ by }), (d) => said.push(d)), "Scattering.");
  assert.deepEqual(asks.map((a) => a.provider), ["openrouter-free", "google"]);
  assert.deepEqual(said, ["Scattering."]);
  assert.equal(named.at(-1), "Google's free tier");

  // Only a Google key: only Google is named. A Claude key alone makes the free model not ready.
  reset({ withKeys: ["google-api-key"], steps: [() => ({ words: ["x"] })] });
  await free.ask(cx, askOf(), () => {});
  assert.deepEqual(asks.map((a) => a.provider), ["google"]);
  reset({ withKeys: ["anthropic-api-key"] });
  assert.equal(await free.ready(cx, {}), false);
});

// ── Tool calls go to the runner, never run directly ───────────────────────────

function timerEntry(spy) {
  return {
    feature: "timer",
    action: {
      id: "start", tool: "timer_start", describe: "Starts a timer.", forms: ["set a timer for {minutes}"],
      args: [{ name: "minutes", kind: "minutes" }], safety: "run",
      run: async (a) => { spy.push(a); return { say: "ran directly" }; },
    },
  };
}

test("tool calls: offered as tools, run only through ask.act, results go back with the messages", async () => {
  const direct = [];
  const entry = timerEntry(direct);
  const transcript = [{ role: "user", content: "q" }, { role: "assistant", content: [{ type: "tool_use", id: "t1" }] }];
  reset({
    withKeys: ALL_KEYS,
    steps: [
      () => ({ stop: "tool_use", tools: [{ id: "t1", name: "timer_start", input: { minutes: 10 } }, { id: "t2", name: "launch_missiles", input: {} }, { id: "t3", name: "timer_start", input: {} }], messages: transcript }),
      () => ({ words: ["Timer set for ten minutes."] }),
    ],
  });
  const acted = [];
  const out = await claude.ask(
    cxWith({ claude: true }),
    askOf({ tools: [entry], act: async (e, a) => { acted.push([e, a]); return "Done."; } }),
    () => {},
  );
  assert.equal(out, "Timer set for ten minutes.");
  // The model was offered the registry's tool (and not the action model's no_action).
  assert.deepEqual(asks[0].tools.map((t) => t.name), ["timer_start"]);
  // The runner was handed the call; the action's own run was never called by this brain.
  assert.equal(acted.length, 1);
  assert.equal(acted[0][0], entry);
  assert.deepEqual(acted[0][1], { minutes: 10 });
  assert.deepEqual(direct, []);
  // Second round: the host's messages plus the results, the unknown tool and the missing detail as errors.
  assert.deepEqual(asks[1].messages, transcript);
  assert.deepEqual(asks[1].results, [
    { id: "t1", content: "Done." },
    { id: "t2", content: "There is no such tool.", error: true },
    { id: "t3", content: "Missing: minutes. Ask the user for it.", error: true },
  ]);
});

test("without ask.act no tool is offered; rounds are capped at five", async () => {
  const entry = timerEntry([]);
  reset({ withKeys: ALL_KEYS, steps: [() => ({ words: ["Plain."] })] });
  await claude.ask(cxWith({ claude: true }), askOf({ tools: [entry] }), () => {});
  assert.equal(asks[0].tools, undefined);

  const loop = () => ({ stop: "tool_use", tools: [{ id: "x", name: "timer_start", input: { minutes: 1 } }], messages: [] });
  reset({ withKeys: ALL_KEYS, steps: Array.from({ length: 9 }, () => loop) });
  let n = 0;
  await claude.ask(cxWith({ claude: true }), askOf({ tools: [entry], act: async () => (n++, "ok") }), () => {});
  assert.equal(asks.length, 5);
  assert.equal(n, 5);
});

test("an interruption cancels the host's ask", async () => {
  reset({ withKeys: ALL_KEYS, steps: [() => null] });
  const ctl = new AbortController();
  const pending = claude.ask(cxWith({ claude: true }), askOf({ signal: ctl.signal }), () => {});
  await new Promise((r) => setTimeout(r, 20));
  ctl.abort();
  await assert.rejects(pending);
  assert.deepEqual(cancels, [asks[0].id]);
});

test("chat answers aloud: starts and stops cleanly, reads nothing by itself", () => {
  const stop = startChatAloud(cxWith({ chatAloud: false }));
  assert.equal(typeof stop, "function");
  stop();
});
