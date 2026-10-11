// Written text (docs/VOICE.md §4.3): the brains that can answer an open question, and which one
// does. The owner's order is the rule: the built-in model first; the user's own local server from
// chat if they chose it; an online model only if its own switch is on (a free model: one switch;
// Claude with the user's key: another). A key that merely exists opens nothing. A brain that fails or
// is too slow hands over to the next that is on; when none is left the answer is null (§4.4) and the
// runner says so. A command never comes here. Words travel through the chat's headless completion
// (`chat.ask`, its `chat:ask-delta` and `chat:ask-done` events).
// Filled by the package voice-generator (VOICE §10; read §4.3, §4.4, §7.3).

import { Bridge, onEvent } from "../../../core/bridge";
import type { FeatureCtx, VoiceArgs, VoiceEntry } from "../../../core/feature";
import type { VoiceSettings } from "../index";
import { claude, free } from "./claude";

/** The order of `auto`. `builtin`: the local text model. `local`: the user's own server from chat. `free`, `claude`: online, each behind its own switch. */
export type BrainId = "builtin" | "local" | "free" | "claude";

/** One earlier exchange of the conversation; a brain gets three at most. */
export interface Exchange {
  said: string;
  answered: string;
}

export interface BrainAsk {
  /** What was said. */
  text: string;
  history: readonly Exchange[];
  /** The state asked about, when the question is about the app (`stateAbout` of answers/). Never audio, clipboard, notes or file contents. */
  state?: string;
  /** For a brain that can call tools (Claude): the registry, and how a call is run: through the same runner and classes; the text it answers goes back to the model. */
  tools?: readonly VoiceEntry[];
  act?: (entry: VoiceEntry, args: VoiceArgs) => Promise<string>;
  /** An online brain names the model it is about to ask: shown in the capsule when it answers, never said. */
  by?: (name: string) => void;
  /** Aborted by an interruption (§7.4). */
  signal: AbortSignal;
}

export interface Brain {
  id: BrainId;
  /** Switched on and able to answer now (its setting, its download, its key, a server that answers in 300 ms). */
  ready(cx: FeatureCtx, settings: VoiceSettings): Promise<boolean>;
  /**
   * Its engine, started if it must be; answers whether it had to be. Awaited before the time limits
   * begin, and a cold one gets the whole limit for its first word: a local model's one slow start (the
   * load, then its warm-up) is not a slow answer.
   */
  start?(cx: FeatureCtx): Promise<boolean>;
  /** The whole answer; its words go to `on` as they are written. Rejects when it fails or runs out of time. */
  ask(cx: FeatureCtx, ask: BrainAsk, on: (delta: string) => void): Promise<string>;
}

/** VOICE §3.4, "Text model": first token 12 s local and 6 s online; 30 s in all (Claude with tools: 90 s, §4.3); 96 tokens. */
export const LIMITS = { firstLocal: 12_000, firstOnline: 6_000, total: 30_000, totalClaude: 90_000, tokens: 96, reach: 300 };

/** The fixed instructions of every brain: a spoken answer, one or two short sentences, plain words, say so if you don't know. */
export const SYSTEM =
  "You answer out loud. Reply in one or two short sentences, in plain words, with no lists, markdown or emoji. If you do not know, say so.";

/** The messages of a request: up to three earlier exchanges, then the question with the date and the state asked about. */
export function messagesFor(ask: BrainAsk, now: Date = new Date()): { role: "user" | "assistant"; content: string }[] {
  const out: { role: "user" | "assistant"; content: string }[] = [];
  // An earlier question goes in as it was sent, date line and all: the local server keeps what it has
  // already read only while the text begins the same (seen in the app: without it every earlier exchange
  // was read again, 11 s to the first word instead of 5).
  const dated = (text: string, about = "") => `[Today is ${now.toDateString()}.${about}]\n${text}`;
  for (const h of ask.history.slice(-3)) out.push({ role: "user", content: dated(h.said) }, { role: "assistant", content: h.answered });
  out.push({ role: "user", content: dated(ask.text, ask.state ? ` About the app: ${ask.state}` : "") });
  return out;
}

let askCount = 0;

/**
 * The one place that talks to the chat's `ask` (assumed shape, VOICE §4.3: `feature_call("chat","ask",
 * {id, provider, system, messages, max_tokens, stream, …})`, words as `chat:ask-delta {id, text}`, the
 * end as `chat:ask-done {id, stop}`, `feature_call("chat","cancel",{id})`). Resolves with the text.
 */
export async function chatAsk(args: Record<string, unknown>, on: (delta: string) => void, signal: AbortSignal): Promise<string> {
  if (signal.aborted) throw new Error("aborted");
  const id = `voice-${Date.now().toString(36)}-${askCount++}`;
  let text = "";
  let finish!: (stop: string) => void;
  const done = new Promise<string>((resolve) => (finish = resolve));
  const offs = await Promise.all([
    onEvent<{ id: string; text: string }>("chat:ask-delta", (e) => {
      if (e.id !== id) return;
      text += e.text;
      on(e.text);
    }),
    onEvent<{ id: string; stop: string }>("chat:ask-done", (e) => e.id === id && finish(e.stop)),
  ]);
  const cancel = () => {
    void Bridge.featureCall("chat", "cancel", { id });
    finish("cancel");
  };
  signal.addEventListener("abort", cancel, { once: true });
  try {
    void Bridge.featureCall("chat", "ask", { id, max_tokens: LIMITS.tokens, stream: true, ...args }).then((r) => r === null && finish("error"));
    const stop = await done;
    if (stop === "error" || stop === "cancel") throw new Error(`chat.ask ${stop}`);
    return text;
  } finally {
    signal.removeEventListener("abort", cancel);
    for (const off of offs) off();
  }
}

const prefixSent = new WeakSet<object>();

/** The built-in model: llama-server through `generator.endpoint` and `chat.ask {provider: "builtin"}`. */
export const builtin: Brain = {
  id: "builtin",
  async ready(cx) {
    const s = await cx.call<{ installed: boolean }>("generator.status");
    return !!s?.installed;
  },
  async start(cx) {
    const cold = !(await cx.call<{ running: boolean }>("generator.status"))?.running;
    if (!(await cx.call("generator.endpoint"))) throw new Error("the built-in model did not start");
    return cold;
  },
  async ask(cx, ask, on) {
    if (!prefixSent.has(cx)) {
      prefixSent.add(cx);
      void cx.call("generator.prefix", { messages: [{ role: "system", content: SYSTEM }] });
    }
    const ep = await cx.call<{ url: string; model: string }>("generator.endpoint");
    if (!ep) throw new Error("the built-in model did not start");
    const args = { provider: "builtin", url: ep.url, model: ep.model, system: SYSTEM, messages: messagesFor(ask) };
    return chatAsk(args, (d) => { void cx.call("generator.touch"); on(d); }, ask.signal);
  },
};

/** The user's own local server from chat (`ollama`, `lmstudio`, `custom`), behind "My local server from chat". */
export const local: Brain = {
  id: "local",
  async ready(_cx, settings) {
    if (!settings.localServer) return false;
    // debt: the chat package's reachability check (`chat.reach`) is assumed; it answers {ok} or null.
    const reach = Bridge.featureCall<{ ok: boolean }>("chat", "reach", {}).catch(() => null);
    const late = new Promise<null>((resolve) => setTimeout(() => resolve(null), LIMITS.reach));
    return !!(await Promise.race([reach, late]))?.ok;
  },
  ask: (_cx, ask, on) => chatAsk({ provider: "local", system: SYSTEM, messages: messagesFor(ask) }, on, ask.signal),
};

/** Every brain, in the order `auto` tries them. */
export const BRAINS: readonly Brain[] = [builtin, local, free, claude];

/** The switch each brain needs, checked here as well as inside the brain: no online request without it. */
const SWITCH: Record<BrainId, (s: VoiceSettings) => boolean> = {
  builtin: () => true,
  local: (s) => s.localServer,
  free: (s) => s.freeModel,
  claude: (s) => s.claude,
};

/** The brains to try: `auto` is the whole order; a fixed one goes first, then the others; `off` none. */
export function order(brains: readonly Brain[], pick: VoiceSettings["brain"] | undefined): Brain[] {
  if (pick === "off") return [];
  const first = brains.find((b) => b.id === pick);
  return first ? [first, ...brains.filter((b) => b !== first)] : [...brains];
}

/** Timers behind a seam so a test can make the clock up. */
export interface Clock {
  after(ms: number, fn: () => void): () => void;
}
const realClock: Clock = {
  after: (ms, fn) => {
    const h = setTimeout(fn, ms);
    return () => clearTimeout(h);
  },
};

/** One brain under the time limits. Null: it failed or ran out of time before saying anything. Words already said stand (they may be spoken). */
async function timed(b: Brain, cx: FeatureCtx, ask: BrainAsk, on: (d: string) => void, clock: Clock): Promise<string | null> {
  const ctl = new AbortController();
  const stop = () => ctl.abort();
  ask.signal.addEventListener("abort", stop, { once: true });
  let said = "";
  const online = b.id === "free" || b.id === "claude";
  let offFirst = () => {};
  let offTotal = () => {};
  const gone = new Promise<never>((_, reject) => ctl.signal.addEventListener("abort", () => reject(new Error("too slow or interrupted")), { once: true }));
  try {
    const cold = b.start ? await Promise.race([b.start(cx), gone]) : false;
    offFirst = clock.after(online ? LIMITS.firstOnline : cold ? LIMITS.total : LIMITS.firstLocal, stop);
    offTotal = clock.after(b.id === "claude" ? LIMITS.totalClaude : LIMITS.total, stop);
    const whole = await Promise.race([
      b.ask(cx, { ...ask, signal: ctl.signal }, (d) => {
        offFirst();
        said += d;
        on(d);
      }),
      gone,
    ]);
    return whole.trim() ? whole : said.trim() ? said : null;
  } catch {
    return said.trim() ? said : null;
  } finally {
    offFirst();
    offTotal();
    ask.signal.removeEventListener("abort", stop);
  }
}

/** `answer` over a given list of brains, settings and clock. */
export async function route(
  brains: readonly Brain[],
  cx: FeatureCtx,
  settings: VoiceSettings,
  ask: BrainAsk,
  on: (delta: string) => void,
  clock: Clock = realClock,
): Promise<{ text: string; brain: BrainId } | null> {
  for (const b of order(brains, settings.brain)) {
    if (ask.signal.aborted) return null;
    if (!SWITCH[b.id](settings)) continue;
    if (!(await b.ready(cx, settings).catch(() => false))) continue;
    const text = await timed(b, cx, ask, on, clock);
    if (text !== null) return { text, brain: b.id };
  }
  return null;
}

/**
 * Answers through the brain the setting `brain` picks (`auto`, a fixed one, `off`), falling to the
 * next that is on when one fails. Null: nothing qualifies (§4.4), and the runner says so.
 */
export async function answer(cx: FeatureCtx, ask: BrainAsk, on: (delta: string) => void): Promise<{ text: string; brain: BrainId } | null> {
  const settings = (await cx.settings.get()) as unknown as VoiceSettings;
  return route(BRAINS, cx, settings, ask, on);
}
