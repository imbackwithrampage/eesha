// Questions answered from what the app already knows (docs/VOICE.md §5): what the agents are doing,
// what one is asking (the tool, never the command), plan usage, the services, the time; and the
// weather and a short lookup, each with its switch (`weather.now`, `weather.lookup`: crates/voice/src/weather.rs).
// No new network call but those two. They are actions like any other: base-actions.ts adds them to the
// voice feature's own. The music, the timer, the battery and the calendar are answered by the voice.ts
// of their own features (media_now, timer_left, battery_level, calendar_next) from the state those keep.
// Package voice-questions (VOICE §10). Words are English; they are spoken, so numbers are rounded.
//
// A waiting permission request is described by its tool only: the command and any path are never
// read (someone else may be in the room) and voice never answers the card.

import { Bridge } from "../../../core/bridge";
import type { VoiceAction, VoiceChoice } from "../../../core/feature";
import { PILL_CATALOG, pillDefinition } from "../../../core/pills";
import { familyOf, needsYou, State, type AgentTask } from "../../../core/state";
import { serviceLine } from "../../../views/integrations";

// ── Words ─────────────────────────────────────────────────────────────────────

const list = (items: string[]): string =>
  items.length < 3 ? items.join(" and ") : `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;

/** Names said once each; many of them as a count. */
const names = (tasks: AgentTask[]): string => {
  const n = [...new Set(tasks.map((t) => t.name))];
  return n.length > 3 ? `${n.length} agents` : list(n);
};

const are = (tasks: AgentTask[]) => (new Set(tasks.map((t) => t.name)).size > 1 ? "are" : "is");

/** A tool as it is said: `mcp__github__create_issue` → "github create issue"; nothing that looks like a path or a command. */
export function toolSaid(tool: string): string {
  const said = tool.replace(/^mcp__/, "").replace(/[_\-.]+/g, " ").trim();
  return said && said.length <= 40 && !/[\\/:"'|&;<>$`]/.test(said) ? said : "a tool";
}

// ── Agents ────────────────────────────────────────────────────────────────────

const WORKING = new Set(["working", "thinking", "searching"]);

/** The pills that are an agent or a coding tool, not a service or a chat provider. */
const agents = (): AgentTask[] =>
  State.tasks.filter((t) => {
    const c = pillDefinition(familyOf(t))?.category;
    return c === "workspace" || c === "agent";
  });

export function agentsSaid(tasks: AgentTask[] = agents()): string {
  const working = tasks.filter((t) => WORKING.has(t.state) && !needsYou(t));
  const waiting = tasks.filter(needsYou);
  const failed = tasks.filter((t) => t.state === "error");
  const limited = tasks.filter((t) => t.state === "ratelimit");
  const done = tasks.filter((t) => t.state === "finished");
  const parts = [
    working.length && `${names(working)} ${are(working)} working.`,
    waiting.length && `${names(waiting)} ${are(waiting)} waiting for you.`,
    failed.length && `${names(failed)} hit an error.`,
    limited.length && `${names(limited)} ${are(limited)} rate limited.`,
    done.length && `${names(done)} ${done.length > 1 || new Set(done.map((t) => t.name)).size > 1 ? "have" : "has"} finished.`,
  ].filter(Boolean);
  return parts.length ? parts.join(" ") : "All quiet. No agent is working.";
}

/** What waits for a click: who and which tool, or that it is a question. Never the command or a path. */
export function askingSaid(): string {
  const all = State.approvals.all();
  const card = State.pendingApproval;
  if (!card) return "Nothing is waiting for you.";
  const who = pillDefinition(card.family)?.name ?? "An agent";
  const more = all.length > 1 ? ` ${all.length - 1} more ${all.length > 2 ? "are" : "is"} waiting after it.` : "";
  return card.questions
    ? `${who} has a question for you. Click the card to answer it.${more}`
    : `${who} wants to use ${toolSaid(card.tool)}. Click Allow or Deny on the card.${more}`;
}

// ── Plan ──────────────────────────────────────────────────────────────────────

const resets = (at: number, now: number): string => {
  const m = Math.max(1, Math.round((at - now) / 60e3));
  if (m < 60) return `in ${m} minute${m > 1 ? "s" : ""}`;
  if (m < 48 * 60) return `in about ${Math.round(m / 60)} hour${Math.round(m / 60) > 1 ? "s" : ""}`;
  return `on ${new Date(at).toLocaleDateString("en-US", { weekday: "long" })}`;
};

export function planSaid(now = Date.now()): string {
  const one = (who: string, p: typeof State.planUsage): string[] => {
    if (!p) return [];
    const w = (label: string, x: { usedPct: number; resetsAt: number } | undefined) =>
      x ? `${who} ${label}: ${Math.round(x.usedPct)} percent used, resets ${resets(x.resetsAt, now)}.` : "";
    return [w("five hour limit", p.fiveHour), w("weekly limit", p.sevenDay)].filter(Boolean);
  };
  const lines = [...one("Claude", State.planUsage), ...one("Codex", State.codexPlanUsage)];
  return lines.length ? lines.join(" ") : "I don't have the plan numbers yet.";
}

// ── Services ──────────────────────────────────────────────────────────────────

const serviceIds = (): string[] =>
  PILL_CATALOG.filter((p) => p.category === "service" && p.support !== "no" && State.integrations[p.id]?.configured).map((p) => p.id);

export function servicesSaid(only?: string): string {
  const ids = only ? [only] : serviceIds();
  const said = ids.flatMap((id) => {
    const line = serviceLine(id);
    return line ? [`${pillDefinition(id)?.name}: ${line.text.replace(/ · /g, ", ").replace(/\.$/, "")}.`] : [];
  });
  return said.length ? said.join(" ") : "No service is set up yet.";
}

// ── Time ──────────────────────────────────────────────────────────────────────

export function timeSaid(d = new Date()): string {
  const time = d.toLocaleTimeString("en-US", { hour: "numeric", minute: "2-digit" });
  const date = d.toLocaleDateString("en-US", { weekday: "long", month: "long", day: "numeric" });
  return `It's ${time}, ${date}.`;
}

// ── Weather and lookup ────────────────────────────────────────────────────────

interface Weather { city: string; temp: number; unit: string; feels: number; high: number; low: number; text: string; rainChance: number }

const OFFER = "I don't look things up online unless you switch that on in the voice settings. Say search the web for it, and I'll open your browser.";

export function weatherSaid(w: Weather, day: 0 | 1): string {
  const r = (n: number) => Math.round(n);
  const rain = r(w.rainChance) >= 10 ? `, with a ${r(w.rainChance)} percent chance of rain` : "";
  return day === 0
    ? `It's ${r(w.temp)} degrees and ${w.text} in ${w.city}, feels like ${r(w.feels)}. Today's high is ${r(w.high)}, low ${r(w.low)}${rain}.`
    : `Tomorrow in ${w.city}: ${w.text}, high ${r(w.high)}, low ${r(w.low)}${rain}.`;
}

const weather = (day: 0 | 1): VoiceAction => ({
  id: day ? "weather_tomorrow" : "weather_now",
  tool: day ? "weather_tomorrow" : "weather_now",
  describe: day ? "Say tomorrow's weather for the city set in the voice settings." : "Say the weather now for the city set in the voice settings.",
  forms: day
    ? ["(what is|what's) the weather [like] tomorrow", "how is the weather tomorrow", "weather tomorrow", "will it rain tomorrow", "(what is|what's) tomorrow's weather"]
    : ["(what is|what's) the weather [like] [today|now|outside]", "how is the weather [today|now|outside]", "weather [today|now]", "(what is|what's) it like outside", "is it raining"],
  safety: "run",
  async run(_a, v) {
    const s = await Bridge.featureSettings("voice");
    if (!s?.weather) return { say: "Weather by voice is off. You can switch it on in the voice settings.", ok: false };
    if (!String(s.city ?? "").trim()) return { say: "Set a city in the voice settings first.", ok: false };
    const w = await v.call<Weather>("weather.now", { day });
    return w ? { say: weatherSaid(w, day), show: `${Math.round(w.temp)}°${w.unit} · ${w.text}` } : { say: "I couldn't get the weather.", ok: false };
  },
});

const lookup: VoiceAction = {
  id: "lookup",
  tool: "fact_lookup",
  describe: "Look a fact up on Wikipedia and say it in a sentence or two, when the user allowed online lookup.",
  forms: ["who (is|was|are|were) {query}", "tell me about {query}", "what do you know about {query}", "(look up|search) wikipedia for {query}", "ask wikipedia about {query}"],
  args: [{ name: "query", kind: "text", ask: "What should I look up?" }],
  safety: "run",
  async run(a, v) {
    const s = await Bridge.featureSettings("voice");
    if (!s?.lookup) return { say: OFFER, ok: false };
    const r = await v.call<{ source: string; text: string }>("weather.lookup", { query: String(a.query) });
    return r?.text ? { say: `According to ${r.source}: ${r.text}` } : { say: "I couldn't find a short answer for that.", ok: false };
  },
};

// ── The actions ───────────────────────────────────────────────────────────────

const services = (): VoiceChoice[] => serviceIds().map((id) => ({ id, say: [pillDefinition(id)?.name ?? id] }));

export const answers: VoiceAction[] = [
  {
    id: "agents_status",
    tool: "agents_status",
    describe: "Say what the coding agents are doing: working, waiting for the user, finished, failed.",
    forms: [
      "what are [my] (agents|sessions) doing", "(what is|what's) [my] (agent|session|claude|codex) doing", "how are [my] agents [doing]", "agent status",
      "(is|are) [my] (agent|agents|build|session) (done|finished|still running|still working|working)", "(is|are) any [of my] agents (working|running|waiting)",
    ],
    safety: "run",
    run: async () => ({ say: agentsSaid() }),
  },
  {
    id: "agent_asking",
    tool: "agent_asking",
    describe: "Say which tool an agent is waiting to use, or that it has a question. The user must click the card.",
    forms: ["(what is|what's) it asking", "(what is|what's) [the] (agent|claude|codex) asking", "what does it want", "what (is|are) (it|they) waiting for", "who is waiting for me", "is anything waiting [for me]"],
    safety: "run",
    run: async () => ({ say: askingSaid() }),
  },
  {
    id: "plan_usage",
    tool: "plan_usage",
    describe: "Say how much of the Claude or Codex plan limits is used and when they reset.",
    forms: [
      "[what is] [my] plan usage", "how much of [my] (plan|limit|usage) (have i used|is left|is used)", "how close am i to [my] (limit|plan limit)",
      "how much usage (do i have left|is left)", "(what is|how is) my claude plan",
    ],
    safety: "run",
    run: async () => ({ say: planSaid() }),
  },
  {
    id: "service_status",
    tool: "service_status",
    describe: "Say the latest status of the connected services, or of one of them.",
    forms: ["how are [my] services [doing]", "service status", "(how is|what is the status of) {service}", "what is new on {service}"],
    args: [{ name: "service", kind: "choice", choices: services, optional: true }],
    safety: "run",
    run: async (a) => ({ say: servicesSaid(a.service ? String(a.service) : undefined) }),
  },
  {
    id: "time",
    tool: "time_now",
    describe: "Say the time and the date.",
    forms: ["what time is it", "(what is|what's) the time", "tell me the time", "(what is|what's) the (date|day) [today]", "what day is it", "(what is|what's) today"],
    safety: "run",
    run: async () => ({ say: timeSaid() }),
  },
  weather(0),
  weather(1),
  lookup,
];

// ── For an open question ──────────────────────────────────────────────────────

const has = (text: string, re: RegExp) => re.test(text.toLowerCase());

/**
 * The kept state an open question is about ("is my build still running"), in a few plain lines for
 * the text model (`BrainAsk.state`); empty when it is about nothing the app knows. It is synchronous,
 * so it holds only what the page keeps (agents, plan, services, the time); the music, timer, battery
 * and calendar are answered by their features' own actions. A waiting request is its tool, no more.
 */
export function stateAbout(text: string): string {
  const lines: string[] = [];
  if (has(text, /\b(agents?|sessions?|claude|codex|cursor|build|running|working|finished|done|waiting|asking|permission|approval)\b/)) {
    lines.push(`Agents: ${agentsSaid()}`);
    if (State.pendingApproval) lines.push(`Waiting: ${askingSaid()}`);
  }
  if (has(text, /\b(plan|limit|usage|quota|tokens?|reset)\b/)) lines.push(`Plan: ${planSaid()}`);
  if (has(text, /\b(services?|deploys?|deployment|github|vercel|stripe|notion|resend|n8n|cal\.com|pull requests?|prs?|ci)\b/)) lines.push(`Services: ${servicesSaid()}`);
  if (has(text, /\b(time|date|day|today)\b/)) lines.push(`Now: ${timeSaid()}`);
  return lines.join("\n");
}
