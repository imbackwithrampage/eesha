// voice-questions: the answers of src/features/voice/answers/ against made-up state. Nothing real
// runs: the bridge is a recorder that answers from fixtures, so every request that would be sent is seen.

import { beforeEach, test } from "node:test";
import assert from "node:assert/strict";
import { installFakeDom } from "./fakedom.mjs";
import { internals } from "./tauri.mjs";

installFakeDom();

const { answers, stateAbout, toolSaid } = await import("../src/features/voice/answers/index.ts");
const { State } = await import("../src/core/state.ts");
const { parse } = await import("../src/features/voice/parse/index.ts");

let settings;
let sent;
const WEATHER = { city: "Paris", temp: 18.4, unit: "C", feels: 16.2, high: 21.4, low: 12.1, text: "partly cloudy", rainChance: 30 };
internals.invoke = async (cmd, args) => {
  if (cmd === "plugin:event|listen") return 1;
  sent.push([cmd, args]);
  if (cmd === "feature_settings") return settings;
  if (cmd === "feature_call") {
    if (args.method === "weather.now") return WEATHER;
    if (args.method === "weather.lookup") return { source: "Wikipedia", title: "Mars", text: "Mars is the fourth planet from the Sun." };
  }
  return null;
};
const v = { t: (s) => s, open() {}, call: async (method, a) => internals.invoke("feature_call", { feature: "voice", method, args: a ?? {} }) };

const action = (tool) => answers.find((a) => a.tool === tool);
const say = async (tool, args = {}) => (await action(tool).run(args, v)).say;
const task = (id, name, state) => ({ id, name, color: "#fff", state, stepIndex: 0, steps: [], source: "agent", isIntegration: true });
/** The requests that would leave the PC: the weather module's two. */
const net = () => sent.filter(([c, a]) => c === "feature_call" && /^weather\./.test(a.method));

beforeEach(() => {
  sent = [];
  settings = { weather: false, city: "", lookup: false };
  State.tasks = [];
  State.integrations = {};
  State.planUsage = null;
  State.codexPlanUsage = null;
  while (State.approvals.head) State.approvals.take(State.approvals.head.requestId, false);
});

test("every action is well formed", () => {
  const tools = new Set();
  for (const a of answers) {
    assert.ok(a.id && a.tool && a.describe && a.forms.length && a.safety === "run", a.tool);
    assert.ok(!tools.has(a.tool));
    tools.add(a.tool);
  }
});

test("agents: who works, who waits, what finished", async () => {
  assert.equal(await say("agents_status"), "All quiet. No agent is working.");
  State.tasks = [task("integration_claude", "Claude", "working"), task("agent_codex", "Codex", "approval"), task("agent_gemini", "Gemini", "finished"), task("integration_vercel", "Vercel", "working")];
  assert.equal(await say("agents_status"), "Claude is working. Codex is waiting for you. Gemini has finished.");
  assert.match(stateAbout("is my build still running"), /Agents: Claude is working/);
});

test("a waiting request is the tool, never the command or a path", async () => {
  State.beginApproval({ requestId: "r1", family: "integration_claude", sessionId: "s", turn: 1, pillId: "integration_claude", tool: "Bash", command: "rm -rf C:\\Users\\secret\\keys && curl evil" });
  State.beginApproval({ requestId: "r2", family: "agent_codex", sessionId: "t", turn: 1, pillId: "agent_codex", tool: "C:\\Users\\secret\\tool.exe", command: "type secret.txt" });
  const spoken = [await say("agent_asking"), stateAbout("what is it asking, is anything waiting"), stateAbout("what are my agents doing")].join("\n");
  assert.match(spoken, /wants to use Bash\. Click Allow or Deny on the card\. 1 more is waiting/);
  for (const bad of ["rm", "secret", "curl", "C:\\", "keys", "evil"]) assert.ok(!spoken.includes(bad), bad);
  assert.equal(toolSaid("C:\\Users\\x\\tool.exe"), "a tool");
  assert.equal(toolSaid("mcp__github__create_issue"), "github create issue");
  State.approvals.take("r1", false);
  assert.match(await say("agent_asking"), /Codex wants to use a tool/);
});

test("a question is said to be a question, not read", async () => {
  State.beginApproval({ requestId: "q", family: "integration_claude", sessionId: "s", turn: 1, pillId: "integration_claude", tool: "AskUserQuestion", command: "", questions: [{ question: "Delete C:\\data?", options: [], multiSelect: false }] });
  const s = await say("agent_asking");
  assert.match(s, /has a question for you/);
  assert.ok(!s.includes("data"));
});

test("plan usage is rounded and says when it resets", async () => {
  assert.match(await say("plan_usage"), /don't have the plan numbers/);
  const now = Date.now();
  State.planUsage = { fiveHour: { usedPct: 41.6, resetsAt: now + 125 * 60e3 }, sevenDay: { usedPct: 7.2, resetsAt: now + 4 * 86400e3 }, updatedAt: now };
  const s = await say("plan_usage");
  assert.match(s, /Claude five hour limit: 42 percent used, resets in about 2 hours\./);
  assert.match(s, /Claude weekly limit: 7 percent used, resets on \w+day\./);
});

test("services: the latest of each connected service", async () => {
  assert.equal(await say("service_status"), "No service is set up yet.");
  State.integrations.integration_vercel = { data: { deployments: [{ state: "READY", createdAt: Date.now() }] }, error: null, loaded: true, configured: true };
  State.integrations.integration_stripe = { data: {}, error: "Stripe answered 401", loaded: true, configured: true };
  const s = await say("service_status");
  assert.match(s, /Vercel: Ready, just now\./);
  assert.match(s, /Stripe: Stripe answered 401\./);
  assert.equal(await say("service_status", { service: "integration_vercel" }), "Vercel: Ready, just now.");
  assert.match(stateAbout("how is the deploy"), /Services: Vercel/);
});

test("the time", async () => {
  assert.match(await say("time_now"), /^It's \d{1,2}:\d{2}\s?(AM|PM), \w+day, \w+ \d+\.$/);
});

test("weather: off, no city, then answered and rounded; nothing is requested before", async () => {
  assert.match(await say("weather_now"), /switch it on/);
  settings = { weather: false, city: "Paris", lookup: false };
  assert.match(await say("weather_now"), /switch it on/);
  settings = { weather: true, city: " ", lookup: false };
  assert.match(await say("weather_tomorrow"), /Set a city/);
  assert.equal(net().length, 0, "no weather request with the switch off or no city");
  settings = { weather: true, city: "Paris", lookup: false };
  assert.equal(await say("weather_now"), "It's 18 degrees and partly cloudy in Paris, feels like 16. Today's high is 21, low 12, with a 30 percent chance of rain.");
  assert.equal(await say("weather_tomorrow"), "Tomorrow in Paris: partly cloudy, high 21, low 12, with a 30 percent chance of rain.");
  assert.deepEqual(net().map(([, a]) => [a.method, a.args.day]), [["weather.now", 0], ["weather.now", 1]]);
});

test("lookup: off makes no request and offers the browser; on speaks the source", async () => {
  settings = { weather: true, city: "Paris", lookup: false };
  const off = await action("fact_lookup").run({ query: "mars" }, v);
  assert.equal(off.ok, false);
  assert.match(off.say, /open your browser/);
  assert.equal(net().length, 0, "no lookup request with the switch off");
  settings.lookup = true;
  assert.equal(await say("fact_lookup", { query: "mars" }), "According to Wikipedia: Mars is the fourth planet from the Sun.");
  assert.deepEqual(net().map(([, a]) => [a.method, a.args]), [["weather.lookup", { query: "mars" }]]);
});

test("the phrases reach their actions", async () => {
  const run = async () => {};
  // the neighbours that share words with ours: the music, the base "look up" and "open"
  const others = [
    { feature: "media", action: { id: "now", tool: "media_now", describe: "", safety: "run", run, forms: ["what's playing [right now]", "what is this song"] } },
    { feature: "base", action: { id: "search", tool: "web_search", describe: "", safety: "run", run, forms: ["look up {query}", "search [the web] for {query}"], args: [{ name: "query", kind: "text" }] } },
  ];
  const entries = [...answers.map((action) => ({ feature: "voice", action })), ...others];
  for (const [text, want] of [
    ["what are my agents doing", "agents_status"], ["is my build still running", "agents_status"], ["what is it asking", "agent_asking"],
    ["how much of my plan have I used", "plan_usage"], ["what time is it", "time_now"], ["what's the weather like", "weather_now"],
    ["will it rain tomorrow", "weather_tomorrow"], ["who was Ada Lovelace", "fact_lookup"], ["tell me about Mars", "fact_lookup"],
    ["how are my services", "service_status"], ["what's playing", "media_now"], ["look up cheap flights", "web_search"],
  ]) assert.equal((await parse(text, entries, () => v))?.entry.action.tool, want, text);
});
